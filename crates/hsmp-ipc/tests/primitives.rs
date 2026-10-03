//! Seqlock, triple buffer and ring: model tests, property tests and multi-thread torn-read
//! checks. `HSMP_IPC_ITERS` scales the threaded tests (default 1e5; use 1e7 under --release).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use hsmp_ipc::layout::{as_words, as_words_mut};
use hsmp_ipc::ring::{Pop, PushError, Record, Ring, MAX_PAYLOAD};
use hsmp_ipc::seqlock::{ReadError, SeqSlot};
use hsmp_ipc::triple::TripleBuf;
use proptest::prelude::*;

hsmp_ipc::ipc_pod! {
    /// 2 KiB of words for torn-read checks.
    pub struct Words2k { pub w: [u64; 256] }
    /// 16 KiB of words for the triple buffer.
    pub struct Words16k { pub w: [u64; 2048] }
}

fn iters() -> u64 {
    std::env::var("HSMP_IPC_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(100_000)
}

fn mix(seq: u64, i: usize) -> u64 {
    (seq ^ 0x9E37_79B9_7F4A_7C15).wrapping_mul(i as u64 * 2 + 1).rotate_left((i % 63) as u32)
}

fn fill<T: hsmp_ipc::Pod>(v: &mut T, seq: u64) {
    let w = as_words_mut(v);
    w[0] = seq;
    for i in 1..w.len() {
        w[i] = mix(seq, i);
    }
}

fn consistent<T: hsmp_ipc::Pod>(v: &T) -> bool {
    let w = as_words(v);
    let s = w[0];
    (1..w.len()).all(|i| w[i] == mix(s, i))
}

// ---- seqlock ------------------------------------------------------------------------------

#[test]
fn seqlock_single_thread() {
    let s = SeqSlot::<Words2k>::new_boxed();
    assert_eq!(s.read().err(), Some(ReadError::Empty));
    let mut v = Words2k::default();
    fill(&mut v, 1);
    s.write(&v);
    let (r, q) = s.read().unwrap();
    assert!(consistent(&r));
    assert_eq!(q, 2);
    let mut out = Words2k::default();
    assert_eq!(s.read_if_changed(2, &mut out), Ok(None));
    fill(&mut v, 2);
    s.write(&v);
    assert_eq!(s.read_if_changed(2, &mut out), Ok(Some(4)));
    assert_eq!(out.w[0], 2);
    // Writer dies mid-write: Busy forever, until a restarted writer re-initialises.
    s.begin_write_and_abandon();
    assert_eq!(s.seq() % 2, 1);
    assert_eq!(s.read().err(), Some(ReadError::Busy));
    fill(&mut v, 3);
    s.write(&v);
    assert_eq!(s.seq() % 2, 0);
    assert_eq!(s.read().unwrap().0.w[0], 3);
}

#[test]
fn seqlock_no_torn_reads_under_contention() {
    let s: Arc<SeqSlot<Words2k>> = Arc::from(SeqSlot::<Words2k>::new_boxed());
    let stop = Arc::new(AtomicBool::new(false));
    let n = iters();
    let ok = Arc::new(AtomicU64::new(0));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let (s, stop, ok) = (s.clone(), stop.clone(), ok.clone());
        readers.push(std::thread::spawn(move || {
            let mut out = Words2k::default();
            let mut last = 0u64;
            let mut torn = 0u64;
            while !stop.load(Ordering::Relaxed) {
                if let Ok(_q) = s.read_into(&mut out) {
                    if !consistent(&out) {
                        torn += 1;
                    }
                    // Values only move forward.
                    assert!(out.w[0] >= last, "went back {} -> {}", last, out.w[0]);
                    last = out.w[0];
                    ok.fetch_add(1, Ordering::Relaxed);
                }
            }
            torn
        }));
    }
    let mut v = Words2k::default();
    for seq in 1..=n {
        fill(&mut v, seq);
        s.write(&v);
    }
    stop.store(true, Ordering::Relaxed);
    let torn: u64 = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(torn, 0);
    assert!(ok.load(Ordering::Relaxed) > 0);
}

// ---- triple buffer ------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum TbOp {
    Publish,
    Take,
    Current,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn triple_buffer_matches_model(ops in prop::collection::vec(prop_oneof![Just(TbOp::Publish), Just(TbOp::Take), Just(TbOp::Current)], 1..200)) {
        let tb = TripleBuf::<Words2k>::new_boxed();
        let mut next = 1u64;
        let mut model_latest: Option<u64> = None; // newest published, not yet taken
        let mut model_front: Option<u64> = None;  // what the reader holds
        let mut out = Words2k::default();
        let mut gen_seen = 0u32;
        for op in ops {
            prop_assert!(tb.indices_valid());
            match op {
                TbOp::Publish => {
                    let mut v = Words2k::default();
                    fill(&mut v, next);
                    let g = tb.publish(&v).unwrap();
                    prop_assert!(g > gen_seen);
                    gen_seen = g;
                    model_latest = Some(next);
                    next += 1;
                    prop_assert_eq!(tb.published_gen(), g);
                }
                TbOp::Take => {
                    let r = tb.take_into(&mut out);
                    match model_latest.take() {
                        Some(v) => {
                            prop_assert!(r.is_some());
                            prop_assert_eq!(out.w[0], v);
                            prop_assert!(consistent(&out));
                            model_front = Some(v);
                        }
                        None => prop_assert!(r.is_none()),
                    }
                }
                TbOp::Current => {
                    let g = tb.current_into(&mut out);
                    if let Some(v) = model_front {
                        prop_assert!(g > 0);
                        prop_assert_eq!(out.w[0], v);
                    } else {
                        prop_assert_eq!(g, 0);
                    }
                }
            }
        }
    }
}

#[test]
fn triple_buffer_two_threads_no_torn() {
    let tb: Arc<TripleBuf<Words16k>> = Arc::from(TripleBuf::<Words16k>::new_boxed());
    let stop = Arc::new(AtomicBool::new(false));
    let n = (iters() / 10).max(2_000);
    let reader = {
        let (tb, stop) = (tb.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut out = Words16k::default();
            let (mut got, mut last, mut torn) = (0u64, 0u64, 0u64);
            loop {
                let done = stop.load(Ordering::Acquire);
                if tb.take_into(&mut out).is_some() {
                    if !consistent(&out) {
                        torn += 1;
                    }
                    assert!(out.w[0] > last, "not newer: {} after {}", out.w[0], last);
                    last = out.w[0];
                    got += 1;
                }
                if done {
                    break;
                }
            }
            (got, last, torn)
        })
    };
    let mut v = Words16k::default();
    for seq in 1..=n {
        fill(&mut v, seq);
        tb.publish(&v).unwrap();
    }
    stop.store(true, Ordering::Release);
    let (got, last, torn) = reader.join().unwrap();
    assert_eq!(torn, 0);
    assert!(got > 0);
    assert_eq!(last, n, "the reader must end on the newest publish");
}

#[test]
fn triple_buffer_restart_repair() {
    let tb = TripleBuf::<Words2k>::new_boxed();
    let mut v = Words2k::default();
    let mut out = Words2k::default();
    fill(&mut v, 1);
    tb.publish(&v).unwrap();
    assert_eq!(tb.take_into(&mut out), Some(1));
    // A fresh writer/reader pair over the same control words keeps working (indices persist).
    for s in 2..50u64 {
        fill(&mut v, s);
        let g = tb.publish(&v).unwrap();
        assert_eq!(g as u64, s);
        if s % 3 == 0 {
            assert_eq!(tb.take_into(&mut out).map(|g| g as u64), Some(s));
            assert!(consistent(&out));
        }
    }
    assert!(tb.indices_valid());
}

// ---- ring ---------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum RingOp {
    Push(Vec<u8>, bool), // payload, old epoch?
    Pop,
    ResetConsumer,
}

fn ring_op() -> impl Strategy<Value = RingOp> {
    prop_oneof![
        4 => (prop::collection::vec(any::<u8>(), 0..520), prop::bool::weighted(0.1)).prop_map(|(p, o)| RingOp::Push(p, o)),
        4 => Just(RingOp::Pop),
        1 => Just(RingOp::ResetConsumer),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn ring_matches_vecdeque(ops in prop::collection::vec(ring_op(), 1..400)) {
        const EPOCH: u64 = 77;
        let r = Ring::<8>::new_boxed();
        let mut model: VecDeque<(Vec<u8>, bool, u64)> = VecDeque::new();
        let mut rec = Record::default();
        let mut next_seq = 0u64;
        for op in ops {
            match op {
                RingOp::Push(p, old) => {
                    let res = r.push(if old { EPOCH - 1 } else { EPOCH }, 0x0280, 0, next_seq + 1000, &p);
                    if p.len() > MAX_PAYLOAD {
                        prop_assert_eq!(res, Err(PushError::TooBig));
                    } else if model.len() >= 8 {
                        prop_assert_eq!(res, Err(PushError::Full));
                    } else {
                        prop_assert_eq!(res, Ok(next_seq));
                        model.push_back((p, old, next_seq));
                        next_seq += 1;
                    }
                }
                RingOp::Pop => {
                    let res = r.pop(EPOCH, &mut rec);
                    match model.pop_front() {
                        None => prop_assert_eq!(res, Pop::Empty),
                        Some((p, old, seq)) => {
                            prop_assert_eq!(rec.hdr.seq, seq);
                            prop_assert_eq!(rec.hdr.req_id, seq + 1000);
                            if old {
                                prop_assert_eq!(res, Pop::StaleEpoch);
                            } else {
                                prop_assert_eq!(res, Pop::Record);
                                prop_assert_eq!(rec.payload(), &p[..]);
                                prop_assert_eq!(rec.hdr.kind, 0x0280);
                            }
                        }
                    }
                }
                RingOp::ResetConsumer => {
                    r.reset_consumer();
                    model.clear();
                }
            }
            prop_assert_eq!(r.len() as usize, model.len());
        }
    }
}

#[test]
fn ring_epoch_zero_accepts_any_and_peek() {
    let r = Ring::<4>::new_boxed();
    r.push(5, 1, 2, 3, b"x").unwrap();
    let mut rec = Record::default();
    assert!(r.peek(0, &mut rec));
    assert_eq!(rec.payload(), b"x");
    assert!(!r.peek(1, &mut rec));
    assert_eq!(r.pop(0, &mut rec), Pop::Record);
    assert_eq!((rec.hdr.flags, rec.hdr.req_id, rec.hdr.producer_epoch), (2, 3, 5));
}

#[test]
fn ring_two_threads_in_order() {
    let r: Arc<Ring<64>> = Arc::from(Ring::<64>::new_boxed());
    let n = (iters() / 2).max(10_000);
    let consumer = {
        let r = r.clone();
        std::thread::spawn(move || {
            let mut rec = Record::default();
            let mut expect = 0u64;
            while expect < n {
                match r.pop(9, &mut rec) {
                    Pop::Record => {
                        assert_eq!(rec.hdr.seq, expect);
                        let len = (expect % 400) as usize;
                        assert_eq!(rec.payload().len(), len);
                        assert!(rec.payload().iter().all(|&b| b == (expect % 251) as u8));
                        expect += 1;
                    }
                    Pop::Empty => std::hint::spin_loop(),
                    other => panic!("unexpected {:?}", other),
                }
            }
        })
    };
    let mut full = 0u64;
    let mut i = 0u64;
    let mut buf = vec![0u8; 400];
    while i < n {
        let len = (i % 400) as usize;
        buf[..len].fill((i % 251) as u8);
        match r.push(9, 1, 0, i, &buf[..len]) {
            Ok(s) => {
                assert_eq!(s, i);
                i += 1;
            }
            Err(PushError::Full) => {
                full += 1;
                std::hint::spin_loop();
            }
            Err(e) => panic!("{:?}", e),
        }
    }
    consumer.join().unwrap();
    let _ = full;
}

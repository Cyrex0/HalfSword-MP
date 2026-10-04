//! Heap allocations and time on the transport's per-packet path.
//!
//! A counting global allocator (per thread, so other test threads do not
//! disturb it) measures what `Conn::poll_transmit` and `Conn::recv` allocate
//! in steady state. The caller's message buffers are built before counting.
//!
//!     cargo test -p hsmp-net --release --test alloc_hotpath -- --nocapture

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use hsmp_net::net::{crypto, Conn, ConnConfig, SendMode, Side};

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(|c| c.get())
}

fn pair() -> (Conn, Conn) {
    let s = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let cfg = ConnConfig::default();
    (
        Conn::from_handshake(Side::Server, &s, 0, cfg.clone()),
        Conn::from_handshake(Side::Client, &s, 0, cfg),
    )
}

/// Steady state: the server streams one 300-byte channel-0 frame per ms, the
/// client answers every 4th with one of its own (acks piggyback). Returns
/// (allocations per server packet sent and received, ns per round).
fn latest_stream(rounds: u64) -> (f64, f64) {
    let (mut s, mut c) = pair();
    let mut down: Vec<Vec<u8>> = (0..rounds).map(|i| vec![i as u8; 300]).collect();
    let mut up: Vec<Vec<u8>> = (0..rounds / 4 + 1).map(|i| vec![i as u8; 120]).collect();
    let mut t = 1;
    // Warm up: maps and queues reach their steady capacity.
    for _ in 0..200 {
        s.send(SendMode::Latest { key: (t % 8) as u32 }, vec![1; 300]).unwrap();
        let dg = s.poll_transmit(t).unwrap();
        c.recv(t, &dg).unwrap();
        c.send(SendMode::Latest { key: 1 }, vec![1; 120]).unwrap();
        while let Some(a) = c.poll_transmit(t) {
            s.recv(t, &a).unwrap();
        }
        t += 1;
    }
    let mut got = 0usize;
    let a0 = allocs();
    let t0 = Instant::now();
    for i in 0..rounds {
        s.send(SendMode::Latest { key: (i % 8) as u32 }, down.pop().unwrap()).unwrap();
        let dg = s.poll_transmit(t).expect("data");
        got += c.recv(t, &dg).unwrap().len();
        if i % 4 == 0 {
            c.send(SendMode::Latest { key: 1 }, up.pop().unwrap()).unwrap();
        }
        while let Some(a) = c.poll_transmit(t) {
            s.recv(t, &a).unwrap();
        }
        t += 1;
    }
    let ns = t0.elapsed().as_nanos() as f64 / rounds as f64;
    let n = allocs() - a0;
    assert_eq!(got as u64, rounds);
    (n as f64 / rounds as f64, ns)
}

/// Polls of a connection with nothing to send (the server polls every
/// connection on every 5 ms tick). Returns allocations per poll.
fn idle_polls(polls: u64) -> f64 {
    let (mut s, mut c) = pair();
    // One exchange so both sides have state, then silence below the
    // keepalive and ack timers.
    s.send(SendMode::Reliable, vec![1; 50]).unwrap();
    let dg = s.poll_transmit(0).unwrap();
    c.recv(0, &dg).unwrap();
    let a = c.poll_transmit(20).unwrap();
    s.recv(21, &a).unwrap();
    let a0 = allocs();
    for i in 0..polls {
        assert!(s.poll_transmit(22 + i % 500).is_none());
    }
    (allocs() - a0) as f64 / polls as f64
}

/// Reliable messages (200 B) one per ms, acked. Returns allocations per
/// message round trip.
fn reliable_stream(rounds: u64) -> f64 {
    let (mut s, mut c) = pair();
    let mut msgs: Vec<Vec<u8>> = (0..rounds).map(|i| vec![i as u8; 200]).collect();
    let mut t = 1;
    for _ in 0..200 {
        s.send(SendMode::Reliable, vec![1; 200]).unwrap();
        let dg = s.poll_transmit(t).unwrap();
        c.recv(t, &dg).unwrap();
        while let Some(a) = c.poll_transmit(t) {
            s.recv(t, &a).unwrap();
        }
        t += 1;
    }
    let a0 = allocs();
    for _ in 0..rounds {
        s.send(SendMode::Reliable, msgs.pop().unwrap()).unwrap();
        let dg = s.poll_transmit(t).expect("data");
        assert_eq!(c.recv(t, &dg).unwrap().len(), 1);
        while let Some(a) = c.poll_transmit(t) {
            s.recv(t, &a).unwrap();
        }
        t += 1;
    }
    (allocs() - a0) as f64 / rounds as f64
}

#[test]
fn per_packet_allocations_stay_low() {
    let (latest, ns) = latest_stream(20_000);
    let idle = idle_polls(10_000);
    let rel = reliable_stream(20_000);
    println!("alloc_hotpath: latest {latest:.2} allocs/round ({ns:.0} ns/round), idle poll {idle:.2} allocs, reliable {rel:.2} allocs/round");
    // A sent datagram and the delivery (its Vec and its data) are owned by
    // the caller; nothing else may allocate per packet.
    assert!(latest <= 3.8, "latest stream: {latest:.2} allocations per round");
    assert!(idle == 0.0, "an idle poll allocates ({idle:.2})");
    assert!(rel <= 5.5, "reliable stream: {rel:.2} allocations per round");
}

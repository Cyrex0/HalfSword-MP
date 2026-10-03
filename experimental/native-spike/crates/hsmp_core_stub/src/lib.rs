//! Spike stand-in for `crates/hsmp-core`.
//!
//! One UDP socket owned by one background thread. The game thread talks to it
//! only through two bounded lock-free SPSC rings:
//!
//! ```text
//!   game thread --hsmp_core_send--> [outq] --net thread--> send_to(peer)
//!   game thread <--hsmp_core_poll-- [inq ] <--net thread-- recv_from
//! ```
//!
//! The net thread never touches UObjects or Lua. Every `extern "C"` entry
//! point is wrapped in `catch_unwind`, so a Rust panic becomes an error code,
//! never an unwind into C++/Lua frames.

pub mod spsc;
mod wake;

use std::ffi::{c_char, CStr};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use spsc::{PopError, PushError, Spsc, SLOT_BYTES};
use wake::{Doorbell, SocketWait};

pub const VERSION: &str = concat!("hsmp_core_stub ", env!("CARGO_PKG_VERSION"));
const VERSION_C: &[u8] = concat!("hsmp_core_stub ", env!("CARGO_PKG_VERSION"), "\0").as_bytes();

const QUEUE_SLOTS: usize = 1024;
/// Safety-net wakeup period of the net thread. Normal wakeups are event
/// driven (socket readable / doorbell), so this only bounds stop() latency.
const IDLE_WAIT_MS: u32 = 5;

// Error codes shared with the C++ / Lua front-ends.
pub const OK: i32 = 0;
pub const E_NOT_RUNNING: i32 = -1;
pub const E_ALREADY_RUNNING: i32 = -2;
pub const E_BAD_ARG: i32 = -3;
pub const E_IO: i32 = -4;
pub const E_FULL: i32 = -5;
pub const E_TOO_BIG: i32 = -6;
pub const E_BUSY: i32 = -7;
pub const E_EMPTY: i32 = -8;
pub const E_TOO_SMALL: i32 = -9;
pub const E_PANIC: i32 = -99;

#[derive(Default)]
struct Stats {
    sent: AtomicU64,
    recv: AtomicU64,
    send_err: AtomicU64,
    recv_err: AtomicU64,
    drop_in_full: AtomicU64,
    drop_out_full: AtomicU64,
    loops: AtomicU64,
}

struct Shared {
    outq: Spsc,
    inq: Spsc,
    running: AtomicBool,
    local_port: AtomicU16,
    /// Producer / consumer guards for the game-facing side of the rings.
    out_guard: AtomicBool,
    in_guard: AtomicBool,
    /// Set by the game side when it rang the doorbell; cleared by the net
    /// thread (swap, AcqRel) right before it drains `outq`.
    wake_pending: AtomicBool,
    doorbell: Doorbell,
    stats: Stats,
}

fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(|| Shared {
        outq: Spsc::new(QUEUE_SLOTS),
        inq: Spsc::new(QUEUE_SLOTS),
        running: AtomicBool::new(false),
        local_port: AtomicU16::new(0),
        out_guard: AtomicBool::new(false),
        in_guard: AtomicBool::new(false),
        wake_pending: AtomicBool::new(false),
        doorbell: Doorbell::new(),
        stats: Stats::default(),
    })
}

fn thread_slot() -> &'static Mutex<Option<JoinHandle<()>>> {
    static T: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

struct Guard<'a>(&'a AtomicBool);
impl<'a> Guard<'a> {
    fn try_take(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).ok().map(|_| Guard(flag))
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn ffi<F: FnOnce() -> i32>(f: F) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(E_PANIC)
}

// ---------------------------------------------------------------------------
// Safe Rust API (used by hsmp_lua_e and by the tests)
// ---------------------------------------------------------------------------

pub fn now_us() -> u64 {
    epoch().elapsed().as_micros() as u64
}

/// Starts the net thread. `peer` may be `"self"` (loopback to our own port,
/// used by the in-game probe). Returns the bound local port.
pub fn start(bind: &str, peer: &str) -> Result<u16, i32> {
    let sh = shared();
    let mut slot = thread_slot().lock().map_err(|_| E_PANIC)?;
    if sh.running.load(Ordering::Acquire) {
        return Err(E_ALREADY_RUNNING);
    }
    if let Some(old) = slot.take() {
        let _ = old.join();
    }
    let sock = UdpSocket::bind(bind).map_err(|_| E_IO)?;
    let waiter = SocketWait::new(&sock).map_err(|_| E_IO)?; // also makes it non-blocking
    let local = sock.local_addr().map_err(|_| E_IO)?;
    let peer_addr: SocketAddr = if peer == "self" {
        local
    } else {
        peer.to_socket_addrs().map_err(|_| E_BAD_ARG)?.next().ok_or(E_BAD_ARG)?
    };
    // Drain anything left from a previous session.
    let mut scratch = [0u8; SLOT_BYTES];
    while sh.inq.pop(&mut scratch).is_ok() {}
    while sh.outq.pop(&mut scratch).is_ok() {}

    sh.local_port.store(local.port(), Ordering::Release);
    sh.running.store(true, Ordering::Release);
    let handle = std::thread::Builder::new()
        .name("hsmp-net".into())
        .spawn(move || {
            // A panic in here must not take the game down.
            let _ = catch_unwind(AssertUnwindSafe(|| net_loop(sh, sock, waiter, peer_addr)));
            sh.running.store(false, Ordering::Release);
        })
        .map_err(|_| {
            sh.running.store(false, Ordering::Release);
            E_IO
        })?;
    *slot = Some(handle);
    Ok(local.port())
}

pub fn stop() -> i32 {
    let sh = shared();
    sh.running.store(false, Ordering::Release);
    sh.doorbell.ring();
    if let Ok(mut slot) = thread_slot().lock() {
        if let Some(h) = slot.take() {
            let _ = h.join(); // the doorbell wakes it immediately
        }
    }
    OK
}

pub fn is_running() -> bool {
    shared().running.load(Ordering::Acquire)
}

pub fn local_port() -> u16 {
    shared().local_port.load(Ordering::Acquire)
}

pub fn send(msg: &[u8]) -> i32 {
    let sh = shared();
    if !sh.running.load(Ordering::Acquire) {
        return E_NOT_RUNNING;
    }
    let Some(_g) = Guard::try_take(&sh.out_guard) else { return E_BUSY };
    match sh.outq.push(msg) {
        Ok(()) => {
            if !sh.wake_pending.swap(true, Ordering::AcqRel) {
                sh.doorbell.ring();
            }
            OK
        }
        Err(PushError::Full) => {
            sh.stats.drop_out_full.fetch_add(1, Ordering::Relaxed);
            E_FULL
        }
        Err(PushError::TooBig) => E_TOO_BIG,
    }
}

/// Pops one inbound message into `out`; returns its length or an error code.
pub fn poll(out: &mut [u8]) -> i32 {
    let sh = shared();
    let Some(_g) = Guard::try_take(&sh.in_guard) else { return E_BUSY };
    match sh.inq.pop(out) {
        Ok(n) => n as i32,
        Err(PopError::Empty) => E_EMPTY,
        Err(PopError::TooSmall(_)) => E_TOO_SMALL,
    }
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct HsmpStats {
    pub sent: u64,
    pub recv: u64,
    pub send_err: u64,
    pub recv_err: u64,
    pub drop_in_full: u64,
    pub drop_out_full: u64,
    pub loops: u64,
    pub inq_len: u32,
    pub outq_len: u32,
    pub running: u32,
    pub local_port: u32,
}

pub fn stats() -> HsmpStats {
    let sh = shared();
    let s = &sh.stats;
    HsmpStats {
        sent: s.sent.load(Ordering::Relaxed),
        recv: s.recv.load(Ordering::Relaxed),
        send_err: s.send_err.load(Ordering::Relaxed),
        recv_err: s.recv_err.load(Ordering::Relaxed),
        drop_in_full: s.drop_in_full.load(Ordering::Relaxed),
        drop_out_full: s.drop_out_full.load(Ordering::Relaxed),
        loops: s.loops.load(Ordering::Relaxed),
        inq_len: sh.inq.len() as u32,
        outq_len: sh.outq.len() as u32,
        running: sh.running.load(Ordering::Acquire) as u32,
        local_port: sh.local_port.load(Ordering::Acquire) as u32,
    }
}

fn net_loop(sh: &'static Shared, sock: UdpSocket, waiter: SocketWait, peer: SocketAddr) {
    let mut obuf = [0u8; SLOT_BYTES];
    let mut rbuf = [0u8; 2048];
    while sh.running.load(Ordering::Acquire) {
        sh.stats.loops.fetch_add(1, Ordering::Relaxed);
        // 1. flush everything the game queued. The swap pairs with the game's
        //    swap in send(): a push that did not ring is visible to this drain.
        sh.wake_pending.swap(false, Ordering::AcqRel);
        while let Ok(n) = sh.outq.pop(&mut obuf) {
            match sock.send_to(&obuf[..n], peer) {
                Ok(_) => sh.stats.sent.fetch_add(1, Ordering::Relaxed),
                Err(_) => sh.stats.send_err.fetch_add(1, Ordering::Relaxed),
            };
        }
        // 2. drain the (non-blocking) socket
        loop {
            match sock.recv_from(&mut rbuf) {
                Ok((n, _from)) => {
                    if n > SLOT_BYTES || sh.inq.push(&rbuf[..n]).is_err() {
                        sh.stats.drop_in_full.fetch_add(1, Ordering::Relaxed);
                    } else {
                        sh.stats.recv.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    use std::io::ErrorKind::*;
                    match e.kind() {
                        WouldBlock => break,
                        // Windows reports an ICMP port-unreachable for an
                        // earlier send as WSAECONNRESET; harmless for UDP.
                        ConnectionReset => continue,
                        _ => {
                            sh.stats.recv_err.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        }
        // 3. sleep until a datagram arrives or the game rings the doorbell
        if sh.outq.is_empty() {
            waiter.wait(&sh.doorbell, IDLE_WAIT_MS);
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI (consumed by the HSMPNative C++ mod)
// ---------------------------------------------------------------------------

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

#[no_mangle]
pub extern "C" fn hsmp_core_version() -> *const c_char {
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn hsmp_core_now_us() -> u64 {
    catch_unwind(now_us).unwrap_or(0)
}

/// Returns the bound local port (>0) or a negative error code.
#[no_mangle]
pub unsafe extern "C" fn hsmp_core_start(bind: *const c_char, peer: *const c_char) -> i32 {
    ffi(|| {
        let (Some(b), Some(p)) = (cstr(bind), cstr(peer)) else { return E_BAD_ARG };
        match start(b, p) {
            Ok(port) => port as i32,
            Err(e) => e,
        }
    })
}

#[no_mangle]
pub extern "C" fn hsmp_core_stop() -> i32 {
    ffi(stop)
}

#[no_mangle]
pub unsafe extern "C" fn hsmp_core_send(data: *const u8, len: usize) -> i32 {
    ffi(|| {
        if data.is_null() && len != 0 {
            return E_BAD_ARG;
        }
        let msg = if len == 0 { &[][..] } else { std::slice::from_raw_parts(data, len) };
        send(msg)
    })
}

#[no_mangle]
pub unsafe extern "C" fn hsmp_core_poll(out: *mut u8, cap: usize) -> i32 {
    ffi(|| {
        if out.is_null() {
            return E_BAD_ARG;
        }
        poll(std::slice::from_raw_parts_mut(out, cap))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hsmp_core_stats(out: *mut HsmpStats) -> i32 {
    ffi(|| {
        if out.is_null() {
            return E_BAD_ARG;
        }
        *out = stats();
        OK
    })
}

#[no_mangle]
pub extern "C" fn hsmp_core_max_payload() -> u32 {
    SLOT_BYTES as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end: game-side send -> net thread -> UDP loopback -> net thread
    /// -> game-side poll, measuring the round trip.
    #[test]
    fn loopback_self_echo() {
        let port = start("127.0.0.1:0", "self").expect("start");
        assert!(port > 0);
        assert_eq!(start("127.0.0.1:0", "self"), Err(E_ALREADY_RUNNING));
        let mut buf = [0u8; SLOT_BYTES];
        let mut worst = 0u64;
        for i in 0..200u32 {
            let t0 = now_us();
            let msg = format!("ping|{i}|{t0}");
            assert_eq!(send(msg.as_bytes()), OK);
            loop {
                let n = poll(&mut buf);
                if n >= 0 {
                    assert_eq!(&buf[..n as usize], msg.as_bytes());
                    worst = worst.max(now_us() - t0);
                    break;
                }
                assert_eq!(n, E_EMPTY);
                assert!(now_us() - t0 < 2_000_000, "timeout waiting for echo {i}");
                std::thread::yield_now();
            }
        }
        let s = stats();
        assert_eq!(s.sent, 200);
        assert_eq!(s.recv, 200);
        eprintln!("loopback worst RTT {worst} us, stats {s:?}");
        assert_eq!(stop(), OK);
        assert!(!is_running());
        assert_eq!(send(b"x"), E_NOT_RUNNING);
        // restartable
        let _ = start("127.0.0.1:0", "self").expect("restart");
        stop();
    }
}

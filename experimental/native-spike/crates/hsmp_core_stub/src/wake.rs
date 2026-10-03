//! Event-driven wakeup for the net thread (Windows).
//!
//! The net thread sleeps in `WaitForMultipleObjects` on two events:
//!   * the socket's FD_READ event (WSAEventSelect) -> a datagram arrived;
//!   * a process-lifetime "doorbell" event          -> the game queued output.
//! So outbound and inbound latency are not tied to the Windows timer tick
//! (15.6 ms by default), which a plain `recv_from` timeout would be.
//!
//! The doorbell is rung at most once per drain via a `pending` flag, so the
//! game thread pays one `SetEvent` per burst, not per message.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::net::UdpSocket;
    use std::os::windows::io::AsRawSocket;

    type HANDLE = *mut c_void;

    extern "system" {
        fn CreateEventW(attr: *mut c_void, manual_reset: i32, initial: i32, name: *const u16) -> HANDLE;
        fn SetEvent(h: HANDLE) -> i32;
        fn CloseHandle(h: HANDLE) -> i32;
        fn WaitForMultipleObjects(n: u32, handles: *const HANDLE, wait_all: i32, ms: u32) -> u32;
    }
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAEventSelect(s: usize, ev: HANDLE, mask: i32) -> i32;
    }
    const FD_READ: i32 = 1;

    /// Process-lifetime auto-reset event. Never closed, so `ring` can never hit
    /// a recycled handle.
    pub struct Doorbell(HANDLE);
    unsafe impl Send for Doorbell {}
    unsafe impl Sync for Doorbell {}

    impl Doorbell {
        pub fn new() -> Self {
            Doorbell(unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) })
        }
        pub fn ring(&self) {
            if !self.0.is_null() {
                unsafe { SetEvent(self.0) };
            }
        }
    }

    /// Per-session socket readiness event, owned by the net thread.
    pub struct SocketWait {
        ev: HANDLE,
    }
    // Owned by exactly one thread at a time (moved into the net thread).
    unsafe impl Send for SocketWait {}

    impl SocketWait {
        /// Also switches the socket to non-blocking mode (WSAEventSelect does).
        pub fn new(sock: &UdpSocket) -> std::io::Result<Self> {
            let ev = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) };
            if ev.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { WSAEventSelect(sock.as_raw_socket() as usize, ev, FD_READ) } != 0 {
                unsafe { CloseHandle(ev) };
                return Err(std::io::Error::last_os_error());
            }
            Ok(SocketWait { ev })
        }

        /// Blocks until a datagram is readable, the doorbell rings, or `ms` passes.
        pub fn wait(&self, bell: &Doorbell, ms: u32) {
            let hs = [self.ev, bell.0];
            unsafe { WaitForMultipleObjects(2, hs.as_ptr(), 0, ms) };
        }
    }

    impl Drop for SocketWait {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.ev) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::net::UdpSocket;
    pub struct Doorbell;
    impl Doorbell {
        pub fn new() -> Self {
            Doorbell
        }
        pub fn ring(&self) {}
    }
    pub struct SocketWait;
    impl SocketWait {
        pub fn new(sock: &UdpSocket) -> std::io::Result<Self> {
            sock.set_nonblocking(true)?;
            Ok(SocketWait)
        }
        pub fn wait(&self, _bell: &Doorbell, ms: u32) {
            std::thread::sleep(std::time::Duration::from_millis(ms.min(1) as u64));
        }
    }
}

pub use imp::{Doorbell, SocketWait};

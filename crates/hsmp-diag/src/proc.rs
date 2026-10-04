//! A process we watch by HANDLE (a PID reused after the exit can never fool the wait).

use std::path::PathBuf;
use std::time::Duration;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    pub struct Proc(HANDLE);

    // SAFETY: a process handle may be used from any thread.
    unsafe impl Send for Proc {}

    impl Proc {
        pub fn open(pid: u32) -> Option<Proc> {
            // SAFETY: plain Win32 call; a null handle is checked.
            let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            (h != 0).then_some(Proc(h))
        }

        /// True once the process has exited (waits up to `timeout`).
        pub fn wait(&self, timeout: Option<Duration>) -> bool {
            let ms = timeout.map_or(u32::MAX, |d| d.as_millis().min(u32::MAX as u128 - 1) as u32);
            // SAFETY: our own handle.
            unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
        }

        pub fn exit_code(&self) -> Option<u32> {
            let mut c = 0u32;
            // SAFETY: our own handle and a valid out pointer.
            let ok = unsafe { GetExitCodeProcess(self.0, &mut c) } != 0;
            (ok && c != STILL_ACTIVE as u32).then_some(c)
        }

        pub fn image(&self) -> Option<PathBuf> {
            let mut buf = vec![0u16; 32768];
            let mut n = buf.len() as u32;
            // SAFETY: the buffer holds `n` UTF-16 units.
            let ok = unsafe { QueryFullProcessImageNameW(self.0, 0, buf.as_mut_ptr(), &mut n) } != 0;
            ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) {
            // SAFETY: closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct Proc(u32);

    impl Proc {
        pub fn open(pid: u32) -> Option<Proc> {
            std::path::Path::new(&format!("/proc/{pid}")).exists().then_some(Proc(pid))
        }
        pub fn wait(&self, timeout: Option<Duration>) -> bool {
            let t0 = std::time::Instant::now();
            loop {
                if !std::path::Path::new(&format!("/proc/{}", self.0)).exists() {
                    return true;
                }
                if timeout.is_some_and(|t| t0.elapsed() >= t) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        pub fn exit_code(&self) -> Option<u32> {
            None
        }
        pub fn image(&self) -> Option<PathBuf> {
            std::fs::read_link(format!("/proc/{}/exe", self.0)).ok()
        }
    }
}

pub use imp::Proc;

/// Is `pid` a running process?
pub fn alive(pid: u32) -> bool {
    pid != 0 && Proc::open(pid).is_some_and(|p| !p.wait(Some(Duration::ZERO)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process() {
        let p = Proc::open(std::process::id()).unwrap();
        assert!(!p.wait(Some(Duration::ZERO)));
        assert!(alive(std::process::id()));
        assert!(p.image().is_some());
        assert!(!alive(0));
    }

    #[cfg(windows)]
    #[test]
    fn exit_code_of_a_child() {
        let mut c = std::process::Command::new("cmd").args(["/c", "exit 3"]).spawn().unwrap();
        let p = Proc::open(c.id()).unwrap();
        assert!(p.wait(Some(Duration::from_secs(20))));
        assert_eq!(p.exit_code(), Some(3));
        let _ = c.wait();
    }
}

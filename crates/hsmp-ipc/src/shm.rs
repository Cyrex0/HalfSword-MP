//! The mapping: a named, pagefile-backed section created by the game with
//! a user-only DACL and an explicit owner, opened (never created) by the sidecar and the
//! tools, plus the doorbell events, process helpers, randomness and the QPC clock.
//!
//! [`Mapping::anonymous`] gives a heap-backed segment on every platform, for in-process
//! tests. Named mappings exist only on Windows; elsewhere they return `Unsupported`.

use std::io;

use crate::segment::{Segment, SEGMENT_SIZE};

/// The mapping name for a game process: `Local\HSMP.ipc.<abi>.<pid>.<createTimeHex>`.
pub fn mapping_name(pid: u32, create_time: u64) -> String {
    format!("Local\\HSMP.ipc.{}.{}.{:x}", crate::ABI_MAJOR, pid, create_time)
}

/// The doorbell event names for a mapping (`<name>.g2s`, `<name>.s2g`).
pub fn doorbell_names(mapping: &str) -> (String, String) {
    (format!("{}.g2s", mapping), format!("{}.s2g", mapping))
}

/// Parse `--ipc shm:<name>` / `shm:<name>` into the mapping name.
pub fn parse_ipc_arg(s: &str) -> Option<&str> {
    let n = s.strip_prefix("shm:")?;
    if n.is_empty() || n.len() > 200 || n.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(n)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadWrite,
    /// Tools (`ipc-dump`): the view is mapped read-only. Writing through it faults.
    ReadOnly,
}

/// A mapped segment. Unmapped and closed on drop.
pub struct Mapping {
    base: *mut u8,
    len: usize,
    name: String,
    access: Access,
    #[cfg(windows)]
    handle: isize,
    heap: bool,
}

// SAFETY: the mapping is plain memory shared by design; every shared field is atomic or
// accessed through the seqlock / triple-buffer / ring protocols.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// A zeroed, page-aligned, heap-backed segment (tests, in-process tools).
    pub fn anonymous() -> Mapping {
        let layout = std::alloc::Layout::from_size_align(SEGMENT_SIZE, 4096).expect("layout");
        // SAFETY: non-zero size.
        let base = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!base.is_null(), "out of memory for the IPC segment");
        Mapping {
            base,
            len: SEGMENT_SIZE,
            name: String::from("<anonymous>"),
            access: Access::ReadWrite,
            #[cfg(windows)]
            handle: 0,
            heap: true,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn access(&self) -> Access {
        self.access
    }
    pub fn as_ptr(&self) -> *mut u8 {
        self.base
    }

    /// The segment, if the mapping is large enough to hold one. Validate the header
    /// ([`crate::handshake::check_prefix`]) before trusting anything in it.
    pub fn segment(&self) -> Option<&Segment> {
        if self.len < SEGMENT_SIZE || (self.base as usize) % 4096 != 0 {
            return None;
        }
        // SAFETY: in bounds and aligned; Segment is all atomics / cells / Pod.
        Some(unsafe { &*(self.base as *const Segment) })
    }

    /// Raw segment pointer for the creator's one-time initialisation.
    pub fn segment_ptr(&self) -> *mut Segment {
        self.base as *mut Segment
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        if self.heap {
            let layout = std::alloc::Layout::from_size_align(SEGMENT_SIZE, 4096).expect("layout");
            // SAFETY: allocated in `anonymous` with this layout.
            unsafe { std::alloc::dealloc(self.base, layout) };
            return;
        }
        #[cfg(windows)]
        win::unmap(self.base, self.handle);
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
pub use other::*;

#[cfg(not(windows))]
mod other {
    use super::*;

    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "named shared memory is Windows-only"))
    }
    impl Mapping {
        pub fn create(_name: &str) -> io::Result<(Mapping, bool)> {
            unsupported()
        }
        pub fn open(_name: &str, _access: Access) -> io::Result<Mapping> {
            unsupported()
        }
    }
    pub fn current_pid() -> u32 {
        std::process::id()
    }
    pub fn process_create_time(_pid: u32) -> io::Result<u64> {
        unsupported()
    }
    pub fn random_u64() -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64));
        h.finish() | 1
    }
    pub fn qpc() -> u64 {
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64 + 1
    }
    pub fn qpc_freq() -> u64 {
        1_000_000_000
    }
    pub struct Doorbell;
    impl Doorbell {
        pub fn create(_name: &str) -> io::Result<Doorbell> {
            unsupported()
        }
        pub fn open(_name: &str) -> io::Result<Doorbell> {
            unsupported()
        }
        pub fn ring(&self) {}
        pub fn wait(&self, _ms: u32) -> bool {
            false
        }
    }
    pub struct ProcessHandle;
    impl ProcessHandle {
        pub fn open(_pid: u32) -> io::Result<ProcessHandle> {
            unsupported()
        }
        pub fn is_alive(&self) -> bool {
            true
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use core::ffi::c_void;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ, FILE_MAP_WRITE,
        MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenEventW, OpenProcess, OpenProcessToken,
        SetEvent, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    const READ_CONTROL: u32 = 0x0002_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const EVENT_MODIFY_STATE: u32 = 0x0002;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The current user's SID (owned token-information buffer + pointer into it).
    struct UserSid {
        buf: Vec<u64>,
    }
    impl UserSid {
        fn current() -> io::Result<UserSid> {
            // SAFETY: plain Win32 calls with checked results and a correctly sized buffer.
            unsafe {
                let mut tok: HANDLE = 0;
                if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut need = 0u32;
                GetTokenInformation(tok, TokenUser, core::ptr::null_mut(), 0, &mut need);
                let mut buf = vec![0u64; (need as usize).div_ceil(8).max(8)];
                let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32, &mut need);
                CloseHandle(tok);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(UserSid { buf })
            }
        }
        fn psid(&self) -> *mut c_void {
            // SAFETY: the buffer holds a TOKEN_USER written by GetTokenInformation.
            unsafe { (*(self.buf.as_ptr() as *const TOKEN_USER)).User.Sid }
        }
        fn string(&self) -> io::Result<String> {
            // SAFETY: valid SID; the returned string is LocalFree'd.
            unsafe {
                let mut p: *mut u16 = core::ptr::null_mut();
                if ConvertSidToStringSidW(self.psid(), &mut p) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                let s = String::from_utf16_lossy(core::slice::from_raw_parts(p, n));
                LocalFree(p as _);
                Ok(s)
            }
        }
    }

    /// A security descriptor: owner = current user, DACL = current user + SYSTEM, protected.
    struct UserOnlySd {
        psd: *mut c_void,
    }
    impl UserOnlySd {
        fn new() -> io::Result<UserOnlySd> {
            let sid = UserSid::current()?.string()?;
            let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
            let mut psd: *mut c_void = core::ptr::null_mut();
            // SAFETY: valid NUL-terminated SDDL; psd is LocalFree'd on drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut psd, core::ptr::null_mut())
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(UserOnlySd { psd })
        }
        fn attrs(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.psd,
                bInheritHandle: 0,
            }
        }
    }
    impl Drop for UserOnlySd {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(self.psd as _) };
        }
    }

    /// Refuse a kernel object whose owner is not the current user (name squatting).
    fn check_owner(h: HANDLE) -> io::Result<()> {
        let me = UserSid::current()?;
        // SAFETY: GetSecurityInfo allocates `psd` (LocalFree'd); `owner` points into it.
        unsafe {
            let mut owner: *mut c_void = core::ptr::null_mut();
            let mut psd: *mut c_void = core::ptr::null_mut();
            let rc = GetSecurityInfo(
                h,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &mut psd,
            );
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc as i32));
            }
            let same = !owner.is_null() && EqualSid(owner, me.psid()) != 0;
            LocalFree(psd as _);
            if !same {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "owner_mismatch: mapping not owned by the current user"));
            }
        }
        Ok(())
    }

    fn map_view(h: HANDLE, access: Access) -> io::Result<(*mut u8, usize)> {
        let acc = match access {
            Access::ReadWrite => FILE_MAP_READ | FILE_MAP_WRITE,
            Access::ReadOnly => FILE_MAP_READ,
        };
        // SAFETY: valid section handle; size 0 maps the whole section.
        unsafe {
            let v = MapViewOfFile(h, acc, 0, 0, 0);
            if v.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut mbi: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
            VirtualQuery(v.Value, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>());
            Ok((v.Value as *mut u8, mbi.RegionSize))
        }
    }

    pub(super) fn unmap(base: *mut u8, handle: isize) {
        // SAFETY: base/handle came from MapViewOfFile/CreateFileMappingW/OpenFileMappingW.
        unsafe {
            if !base.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: base as *mut c_void });
            }
            if handle != 0 {
                CloseHandle(handle);
            }
        }
    }

    impl Mapping {
        /// Game side: create the section (`SEGMENT_SIZE`, committed, user-only DACL). Returns
        /// `(mapping, existed)`; `existed` = the name was already there (same process
        /// re-open after a mod reload), in which case the owner was verified.
        pub fn create(name: &str) -> io::Result<(Mapping, bool)> {
            let sd = UserOnlySd::new()?;
            let sa = sd.attrs();
            let w = wide(name);
            let size = SEGMENT_SIZE as u64;
            // SAFETY: valid attributes and name; INVALID_HANDLE_VALUE = pagefile-backed.
            let (h, existed) = unsafe {
                let h = CreateFileMappingW(INVALID_HANDLE_VALUE, &sa, PAGE_READWRITE, (size >> 32) as u32, size as u32, w.as_ptr());
                let err = GetLastError();
                (h, err == ERROR_ALREADY_EXISTS)
            };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            if existed {
                if let Err(e) = check_owner(h) {
                    // SAFETY: handle from CreateFileMappingW.
                    unsafe { CloseHandle(h) };
                    return Err(e);
                }
            }
            let (base, len) = match map_view(h, Access::ReadWrite) {
                Ok(v) => v,
                Err(e) => {
                    // SAFETY: as above.
                    unsafe { CloseHandle(h) };
                    return Err(e);
                }
            };
            Ok((Mapping { base, len, name: name.to_string(), access: Access::ReadWrite, handle: h, heap: false }, existed))
        }

        /// Sidecar / tools: open an existing section (never creates it) and verify its
        /// owner is the current user.
        pub fn open(name: &str, access: Access) -> io::Result<Mapping> {
            let w = wide(name);
            let acc = match access {
                Access::ReadWrite => FILE_MAP_READ | FILE_MAP_WRITE,
                Access::ReadOnly => FILE_MAP_READ,
            };
            // SAFETY: valid name.
            let h = unsafe { OpenFileMappingW(acc | READ_CONTROL, 0, w.as_ptr()) };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            if let Err(e) = check_owner(h) {
                // SAFETY: handle from OpenFileMappingW.
                unsafe { CloseHandle(h) };
                return Err(e);
            }
            let (base, len) = match map_view(h, access) {
                Ok(v) => v,
                Err(e) => {
                    // SAFETY: as above.
                    unsafe { CloseHandle(h) };
                    return Err(e);
                }
            };
            Ok(Mapping { base, len, name: name.to_string(), access, handle: h, heap: false })
        }
    }

    pub fn current_pid() -> u32 {
        // SAFETY: trivial.
        unsafe { GetCurrentProcessId() }
    }

    fn ft(f: FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    /// FILETIME of a process' creation (PID-reuse guard).
    pub fn process_create_time(pid: u32) -> io::Result<u64> {
        // SAFETY: handle checked and closed; FILETIMEs written by the call.
        unsafe {
            let h = if pid == current_pid() { GetCurrentProcess() } else { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut c, mut e, mut k, mut u) = (z, z, z, z);
            let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
            if pid != current_pid() {
                CloseHandle(h);
            }
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ft(c))
        }
    }

    /// A non-zero random u64 from the system RNG (epochs).
    pub fn random_u64() -> u64 {
        let mut b = [0u8; 8];
        // SAFETY: valid buffer; the system-preferred RNG needs no algorithm handle.
        let st = unsafe { BCryptGenRandom(core::ptr::null_mut(), b.as_mut_ptr(), 8, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        let v = u64::from_le_bytes(b);
        if st != 0 || v == 0 {
            // Fallback: QPC mixed with the pid (never 0).
            let x = qpc() ^ ((current_pid() as u64) << 32);
            return x.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        }
        v
    }

    pub fn qpc() -> u64 {
        let mut v = 0i64;
        // SAFETY: trivial.
        unsafe { QueryPerformanceCounter(&mut v) };
        v as u64
    }

    pub fn qpc_freq() -> u64 {
        let mut v = 0i64;
        // SAFETY: trivial.
        unsafe { QueryPerformanceFrequency(&mut v) };
        v as u64
    }

    /// An auto-reset event used as a doorbell.
    pub struct Doorbell {
        h: HANDLE,
    }
    unsafe impl Send for Doorbell {}
    unsafe impl Sync for Doorbell {}

    impl Doorbell {
        /// Game side: create (or reuse) the event with the user-only DACL.
        pub fn create(name: &str) -> io::Result<Doorbell> {
            let sd = UserOnlySd::new()?;
            let sa = sd.attrs();
            let w = wide(name);
            // SAFETY: valid attributes and name.
            // SAFETY: valid attributes and name.
            let (h, existed) = unsafe { (CreateEventW(&sa, 0, 0, w.as_ptr()), GetLastError() == ERROR_ALREADY_EXISTS) };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            if existed {
                if let Err(e) = check_owner(h) {
                    // SAFETY: handle from CreateEventW.
                    unsafe { CloseHandle(h) };
                    return Err(e);
                }
            }
            Ok(Doorbell { h })
        }
        /// Sidecar / tools: open an existing event and verify its owner.
        pub fn open(name: &str) -> io::Result<Doorbell> {
            let w = wide(name);
            // SAFETY: valid name.
            let h = unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE | READ_CONTROL, 0, w.as_ptr()) };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            if let Err(e) = check_owner(h) {
                // SAFETY: handle from OpenEventW.
                unsafe { CloseHandle(h) };
                return Err(e);
            }
            Ok(Doorbell { h })
        }
        /// Signal (one syscall, ~1-2 µs).
        pub fn ring(&self) {
            // SAFETY: valid event handle.
            unsafe { SetEvent(self.h) };
        }
        /// Wait up to `ms` for a ring. True if rung.
        pub fn wait(&self, ms: u32) -> bool {
            // SAFETY: valid event handle.
            unsafe { WaitForSingleObject(self.h, ms) == WAIT_OBJECT_0 }
        }
        pub fn raw(&self) -> isize {
            self.h
        }
    }
    impl Drop for Doorbell {
        fn drop(&mut self) {
            // SAFETY: owned handle.
            unsafe { CloseHandle(self.h) };
        }
    }

    /// A SYNCHRONIZE handle on another process: authoritative death detection.
    pub struct ProcessHandle {
        h: HANDLE,
    }
    unsafe impl Send for ProcessHandle {}
    unsafe impl Sync for ProcessHandle {}

    impl ProcessHandle {
        pub fn open(pid: u32) -> io::Result<ProcessHandle> {
            // SAFETY: trivial; checked.
            let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ProcessHandle { h })
        }
        /// One `WaitForSingleObject(h, 0)`: call it at ~1 Hz, off the hot path.
        pub fn is_alive(&self) -> bool {
            // SAFETY: valid handle.
            unsafe { WaitForSingleObject(self.h, 0) != WAIT_OBJECT_0 }
        }
    }
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: owned handle.
            unsafe { CloseHandle(self.h) };
        }
    }
}

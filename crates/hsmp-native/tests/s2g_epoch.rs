//! S2G drain epoch rules: the game delivers only a live, attached sidecar's records. Before any
//! sidecar (epoch 0) nothing is accepted (`pop(0)` would take any producer); after a detach
//! (Closing) the dead sidecar's leftovers are not delivered, and the next sidecar's epoch drops
//! them as stale. (The death path, the game marking the sidecar Absent after its process
//! handle signals, takes the same branch; it needs a second process: CMake harness_F.)

use std::ffi::{CStr, CString};

use hsmp_ipc::handshake::{attach_sidecar, detach_sidecar, SideParams};
use hsmp_ipc::shm::{self, Access, Mapping};
use mlua::ffi;

type L = *mut ffi::lua_State;

fn run(l: L, code: &str) {
    unsafe {
        let c = CString::new(code).unwrap();
        let rc = ffi::luaL_loadstring(l, c.as_ptr());
        let rc = if rc == 0 { ffi::lua_pcall(l, 0, 0, 0) } else { rc };
        if rc != 0 {
            let msg = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
            ffi::lua_settop(l, 0);
            panic!("lua error: {}\n--- chunk ---\n{}", msg, code);
        }
    }
}

fn eval_str(l: L, expr: &str) -> String {
    unsafe {
        let c = CString::new(format!("return tostring({})", expr)).unwrap();
        assert_eq!(ffi::luaL_loadstring(l, c.as_ptr()), 0);
        assert_eq!(ffi::lua_pcall(l, 0, 1, 0), 0);
        let s = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
        ffi::lua_settop(l, 0);
        s
    }
}

#[test]
fn s2g_only_from_the_live_sidecar() {
    unsafe {
        let l = ffi::luaL_newstate();
        ffi::luaL_openlibs(l);
        let n = CString::new("HSMPSync").unwrap();
        assert_eq!(hsmp_native::hsmp_native_open(l as *mut _, n.as_ptr()), 1);
        let name = eval_str(l, "HSMPNative.ipc_open()");
        run(l, r#"N = HSMPNative; N.frame("w"); EV = {}"#);
        let map = Mapping::open(&name, Access::ReadWrite).unwrap();
        let seg = map.segment().unwrap();
        let kind = hsmp_ipc::schema::record_by_name("dev_cmd").map(|r| r.kind).unwrap_or(0x0810);
        let payload = vec![0u8; 240];
        // 1. no sidecar yet (epoch 0): a stray record is not delivered
        seg.s2g().push(0, kind, 0, 1, &payload).unwrap();
        run(l, r#"N.frame("w"); assert(N.poll(16, EV) == 0, "nothing before a sidecar attaches")"#);
        // 2. sidecar A attaches: the stray record is dropped as stale, A's records arrive
        let pid = shm::current_pid();
        let ct = shm::process_create_time(pid).unwrap();
        let pa = SideParams { pid, create_time: ct, epoch: 0xA, caps: u64::MAX, build_id: "A" };
        attach_sidecar(seg, map.len(), pid, Some(ct), &pa).unwrap();
        seg.s2g().push(0xA, kind, 0, 2, &payload).unwrap();
        run(l, r#"N.frame("w"); local n = N.poll(16, EV); assert(n == 1 and EV[1].req_id == 2, "A's record only: " .. n)"#);
        let stale = eval_str(l, "HSMPNative.ipc_info().counters.stale_epoch");
        assert!(stale.parse::<u64>().unwrap() >= 1, "the epoch-0 record was dropped as stale ({})", stale);
        // 3. A detaches, leaving a record behind: not delivered
        seg.s2g().push(0xA, kind, 0, 3, &payload).unwrap();
        detach_sidecar(seg);
        run(l, r#"N.frame("w"); assert(N.poll(16, EV) == 0, "a closed sidecar's leftovers are not delivered")"#);
        // 4. B attaches: A's leftover is stale, B's record arrives
        let pb = SideParams { pid, create_time: ct, epoch: 0xB, caps: u64::MAX, build_id: "B" };
        attach_sidecar(seg, map.len(), pid, Some(ct), &pb).unwrap();
        seg.s2g().push(0xB, kind, 0, 4, &payload).unwrap();
        run(l, r#"N.frame("w"); local n = N.poll(16, EV); assert(n == 1 and EV[1].req_id == 4, "B's record only: " .. n)"#);
        ffi::lua_close(l);
    }
}

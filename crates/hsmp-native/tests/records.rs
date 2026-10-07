//! The typed record API, DevCtl and the process API against the real
//! native module: the shared conformance script `tools/hsmp-tools/lua-tests/lib/
//! records_conformance.lua` (the same checks `hsmp-tools lua-test records` runs against the Lua
//! mock), plus native-only cases (DevCtl fan-out across Lua states, argv quoting, stderr
//! capture, batch files), allocation regressions and opt-in benchmarks.
//!
//! One test function: the native state is process-global (like in the game).
//! Timing-only loops and budgets require `HSMP_NATIVE_BENCH=1 cargo test -p hsmp-native
//! --release --test records -- --nocapture` (PowerShell: set `$env:HSMP_NATIVE_BENCH='1'`
//! before the command, remove it afterward). Correctness and allocation checks always run.

use std::ffi::{c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use hsmp_ipc::handshake::{attach_sidecar, SideParams};
use hsmp_ipc::layout::{Bool, Str, TypeDesc};
use hsmp_ipc::record::{Invalid, VarBuf};
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::{self as sc, flow, Chan, Dir, RawSlot, RecordInfo, SlotForm, SlotInfo, SlotMeta, Stamped};
use hsmp_ipc::segment::Segment;
use hsmp_ipc::seqlock::SeqSlot;
use hsmp_ipc::shm::{self, Access, Mapping};
use hsmp_ipc::triple::TripleBuf;
use hsmp_native::marshal;
use hsmp_native::records::TestSlot;
use mlua::ffi;

type L = *mut ffi::lua_State;

// ---- test-only records (mirrored in records_conformance.lua; checked by signature) -----------

hsmp_ipc::ipc_pod! {
    pub struct TInner {
        pub a: u16,
        pub b: Bool,
        pub _r: u8,
        pub c: f32,
    }
    pub struct TFixed {
        pub u8v: u8,
        pub i8v: i8,
        pub u16v: u16,
        pub u32v: u32,
        pub i16v: i16,
        pub _r: u16,
        pub i32v: i32,
        pub u64v: u64,
        pub i64v: i64,
        pub f32v: f32,
        pub flag: Bool,
        pub _r2: [u8; 3],
        pub f64v: f64,
        pub name: Str<8>,
        pub vec: [f32; 3],
        pub _r3: u32,
        pub inner: TInner,
        pub inners: [TInner; 2],
        pub tags: [Str<4>; 2],
        pub grid: [[u8; 2]; 2],
        pub _r4: [u8; 4],
        pub note: Str<40>,
    }
    pub struct TVar {
        pub id: u32,
        pub n: u16,
        pub flag: Bool,
        pub _r: u8,
        pub name: Str<16>,
    }
    pub struct TRow {
        pub k: u32,
        pub v: f32,
        pub tag: Str<8>,
    }
    pub struct TFloats {
        pub n: u16,
        pub _r: [u16; 3],
    }
}

fn check_fixed(t: &TFixed) -> Result<(), Invalid> {
    if t.u8v == 77 {
        return Err(Invalid::Range("u8v"));
    }
    Ok(())
}
fn check_var(t: &TVar) -> Result<(), Invalid> {
    if t.id == 999 {
        return Err(Invalid::Range("id"));
    }
    Ok(())
}
fn check_row(_h: &TVar, r: &TRow) -> Result<(), Invalid> {
    if r.k == u32::MAX {
        return Err(Invalid::Range("k"));
    }
    Ok(())
}

hsmp_ipc::record!(TFixed, kind = 0x7f10, name = "t_fixed", check = check_fixed);
hsmp_ipc::record!(TVar, kind = 0x7f11, name = "t_var", rows = TRow, count = n, max = 24, check = check_var, check_row = check_row);
hsmp_ipc::record!(TFloats, kind = 0x7f12, name = "t_floats", rows = f32, count = n, max = 8);

static TEST_RECORDS: [RecordInfo; 3] = [
    hsmp_ipc::record_info!(TFixed, cap = 0, flow = flow::G2S | flow::S2G | flow::LOCAL, chan = Chan::None, doc = "test"),
    hsmp_ipc::record_info!(TVar, cap = 0, flow = flow::G2S | flow::S2G, chan = Chan::None, doc = "test"),
    hsmp_ipc::record_info!(TFloats, cap = 0, flow = flow::G2S, chan = Chan::None, doc = "test"),
];

type VarT = VarBuf<TVar, 24>;

fn seq<T: hsmp_ipc::Pod + hsmp_ipc::bytemuck::Pod>() -> &'static dyn RawSlot {
    Box::leak(SeqSlot::<Stamped<T>>::new_boxed())
}
fn tri<T: hsmp_ipc::Pod + hsmp_ipc::bytemuck::Pod>() -> &'static dyn RawSlot {
    Box::leak(TripleBuf::<Stamped<T>>::new_boxed())
}

fn slot(name: &'static str, kind: u16, form: SlotForm, dir: Dir, world_scoped: bool, raw: Vec<&'static dyn RawSlot>) -> TestSlot {
    TestSlot { info: SlotInfo { name, kind, form, dir, cap: 0, world_scoped, doc: "test" }, raw: Box::leak(raw.into_boxed_slice()) }
}

fn test_slots() -> &'static [TestSlot] {
    static S: OnceLock<&'static [TestSlot]> = OnceLock::new();
    S.get_or_init(|| {
        let peers = |f: fn() -> &'static dyn RawSlot| (0..sc::pose::MAX_PEER_SLOTS).map(|_| f()).collect::<Vec<_>>();
        let v = vec![
            slot("t_fixed_out", 0x7f10, SlotForm::Slot, Dir::GameToSidecar, true, vec![seq::<TFixed>()]),
            slot("t_var_out", 0x7f11, SlotForm::Blob, Dir::GameToSidecar, false, vec![tri::<VarT>()]),
            slot("t_local", 0x7f10, SlotForm::Slot, Dir::Local, false, vec![seq::<TFixed>()]),
            slot("t_fixed_in", 0x7f10, SlotForm::Slot, Dir::SidecarToGame, false, vec![seq::<TFixed>()]),
            slot("t_var_in", 0x7f11, SlotForm::Blob, Dir::SidecarToGame, false, vec![tri::<VarT>()]),
            slot("t_peer", 0x7f10, SlotForm::PeerSlot, Dir::SidecarToGame, false, peers(seq::<TFixed>)),
            slot("t_peer_blob", 0x7f11, SlotForm::PeerBlob, Dir::SidecarToGame, false, peers(tri::<VarT>)),
            slot("t_bus", 0x7f10, SlotForm::Bus, Dir::Local, true, vec![]),
            slot("t_bus_keep", 0x7f11, SlotForm::Bus, Dir::Local, false, vec![]),
        ];
        Box::leak(v.into_boxed_slice())
    })
}

// ---- signature (must equal records_conformance.lua's C.signature()) ---------------------------

fn tsig(d: &TypeDesc) -> String {
    match d {
        TypeDesc::Prim { name, .. } => name.to_string(),
        TypeDesc::Bool => "bool".into(),
        TypeDesc::Str { cap } => format!("str{}", cap),
        TypeDesc::Array { elem, len } => format!("[{};{}]", tsig(elem), len),
        TypeDesc::Struct { name, .. } => name.to_string(),
    }
}

fn rust_signature() -> String {
    use hsmp_ipc::IpcType;
    let mut structs: Vec<&TypeDesc> = vec![&TInner::DESC, &TFixed::DESC, &TVar::DESC, &TRow::DESC, &TFloats::DESC];
    structs.sort_by_key(|d| d.name());
    let mut out = Vec::new();
    for d in structs {
        let f: Vec<String> = d.fields().iter().map(|f| format!("{}:{}", f.name, tsig(f.ty))).collect();
        out.push(format!("{}({}){{{}}}", d.name(), d.size(), f.join(",")));
    }
    let mut recs: Vec<&RecordInfo> = TEST_RECORDS.iter().collect();
    recs.sort_by_key(|r| r.name);
    for r in recs {
        let fl: Vec<&str> = flow::NAMES.iter().filter(|(_, b)| r.flow & b != 0).map(|(n, _)| *n).collect();
        out.push(format!(
            "{}={:04x}:{}:{}:{}:{}:{}",
            r.name,
            r.kind,
            r.layout,
            r.row.map_or("nil".to_string(), |d| d.name().to_string()),
            r.count_field.unwrap_or("nil"),
            r.max_rows,
            fl.join(",")
        ));
    }
    let mut slots: Vec<&TestSlot> = test_slots().iter().collect();
    slots.sort_by_key(|s| s.info.name);
    for s in slots {
        let rec = TEST_RECORDS.iter().find(|r| r.kind == s.info.kind).map_or("?", |r| r.name);
        let form = match s.info.form {
            SlotForm::Slot => "slot",
            SlotForm::Blob => "blob",
            SlotForm::PeerSlot => "peer_slot",
            SlotForm::PeerBlob => "peer_blob",
            SlotForm::Bus => "bus",
        };
        let dir = match s.info.dir {
            Dir::GameToSidecar => "g2s",
            Dir::SidecarToGame => "s2g",
            Dir::Local => "local",
        };
        out.push(format!("{}={}:{}:{}:{}", s.info.name, rec, form, dir, s.info.world_scoped));
    }
    out.join("\n")
}

// ---- Lua helpers -------------------------------------------------------------------------------

fn new_state(name: &str) -> L {
    unsafe {
        let l = ffi::luaL_newstate();
        ffi::luaL_openlibs(l);
        let n = CString::new(name).unwrap();
        assert_eq!(hsmp_native::hsmp_native_open(l as *mut _, n.as_ptr()), 1);
        l
    }
}

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
        assert_eq!(ffi::lua_pcall(l, 0, 1, 0), 0, "eval {}", expr);
        let s = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
        ffi::lua_settop(l, 0);
        s
    }
}

// ---- the fake sidecar / tool side (H) ------------------------------------------------------------

static SEG: AtomicUsize = AtomicUsize::new(0);
static SC_EPOCH: AtomicU64 = AtomicU64::new(0);

fn seg() -> &'static Segment {
    // SAFETY: set once from a mapping that lives until the end of the test.
    unsafe { &*(SEG.load(Ordering::Acquire) as *const Segment) }
}

fn sc_meta() -> SlotMeta {
    SlotMeta { writer_epoch: SC_EPOCH.load(Ordering::Acquire), valid: 1, ..Default::default() }
}

fn find_record(name: &str) -> Option<&'static RecordInfo> {
    TEST_RECORDS.iter().find(|r| r.name == name).or_else(|| sc::record_by_name(name))
}

fn find_record_kind(kind: u16) -> Option<&'static RecordInfo> {
    TEST_RECORDS.iter().find(|r| r.kind == kind).or_else(|| sc::record_info(kind))
}

unsafe fn lstr(l: L, idx: c_int) -> Option<String> {
    unsafe {
        if ffi::lua_type(l, idx) != ffi::LUA_TSTRING {
            return None;
        }
        Some(CStr::from_ptr(ffi::lua_tostring(l, idx)).to_string_lossy().into_owned())
    }
}

unsafe fn fail(l: L, msg: &str) -> c_int {
    unsafe {
        ffi::lua_pushnil(l);
        let c = CString::new(msg).unwrap();
        ffi::lua_pushstring(l, c.as_ptr());
    }
    2
}

unsafe fn marshal_arg(l: L, idx: c_int, r: &RecordInfo, check: bool) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    unsafe { marshal::table_to_record(l as *mut c_void, idx, r, &mut v) }.map_err(|e| e.message())?;
    if check {
        marshal::validate(r, &v).map_err(|e| e.message())?;
    }
    Ok(v)
}

/// H.sc_put(slot, t, peer_slot?)
unsafe extern "C-unwind" fn h_sc_put(l: L) -> c_int {
    unsafe {
        let Some(name) = lstr(l, 1) else { return fail(l, "sc_put: name") };
        let Some(ts) = test_slots().iter().find(|s| s.info.name == name) else { return fail(l, "sc_put: unknown slot") };
        let Some(r) = find_record_kind(ts.info.kind) else { return fail(l, "sc_put: record") };
        let peer = if ffi::lua_type(l, 3) == ffi::LUA_TNUMBER { ffi::lua_tointegerx(l, 3, std::ptr::null_mut()) as usize } else { 0 };
        let p = match marshal_arg(l, 2, r, true) {
            Ok(p) => p,
            Err(e) => return fail(l, &format!("sc_put: {}", e)),
        };
        let mut scratch = Vec::new();
        if !ts.raw[peer].put(sc_meta(), r.kind, &p, &mut scratch) {
            return fail(l, "sc_put: too big");
        }
        ffi::lua_pushboolean(l, 1);
        1
    }
}

/// H.sc_rec_event(kind, t, peer, aux, req_id)
unsafe extern "C-unwind" fn h_sc_rec_event(l: L) -> c_int {
    unsafe {
        let Some(name) = lstr(l, 1) else { return fail(l, "sc_rec_event: kind") };
        let Some(r) = find_record(&name) else { return fail(l, "sc_rec_event: unknown kind") };
        let p = match marshal_arg(l, 2, r, true) {
            Ok(p) => p,
            Err(e) => return fail(l, &format!("sc_rec_event: {}", e)),
        };
        let int = |i| if ffi::lua_type(l, i) == ffi::LUA_TNUMBER { ffi::lua_tointegerx(l, i, std::ptr::null_mut()) } else { 0 };
        let (peer, aux, req) = (int(3) as u32, int(4) as u16, int(5) as u64);
        if seg().s2g().push_msg(SC_EPOCH.load(Ordering::Acquire), r.kind, aux, peer, 0, req, &p).is_err() {
            return fail(l, "sc_rec_event: ring");
        }
        ffi::lua_pushboolean(l, 1);
        1
    }
}

/// H.sc_push_n(kind, t, n): push the same record n times (benchmarks).
unsafe extern "C-unwind" fn h_sc_push_n(l: L) -> c_int {
    unsafe {
        let Some(name) = lstr(l, 1) else { return fail(l, "sc_push_n: kind") };
        let Some(r) = find_record(&name) else { return fail(l, "sc_push_n: unknown kind") };
        let p = match marshal_arg(l, 2, r, true) {
            Ok(p) => p,
            Err(e) => return fail(l, &e),
        };
        let n = ffi::lua_tointegerx(l, 3, std::ptr::null_mut());
        for i in 0..n {
            if seg().s2g().push_msg(SC_EPOCH.load(Ordering::Acquire), r.kind, 0, 1, 0, i as u64, &p).is_err() {
                return fail(l, "sc_push_n: ring full");
            }
        }
        ffi::lua_pushboolean(l, 1);
        1
    }
}

/// H.sc_rec_drain() -> {{kind, req_id, data}...}
unsafe extern "C-unwind" fn h_sc_rec_drain(l: L) -> c_int {
    unsafe {
        ffi::lua_createtable(l, 0, 0);
        let t = ffi::lua_gettop(l);
        let mut rec = Record::default();
        let mut n = 0;
        loop {
            match seg().g2s().pop(0, &mut rec) {
                Pop::Empty => break,
                Pop::Record => {
                    let Some(r) = find_record_kind(rec.hdr.kind) else { continue };
                    n += 1;
                    ffi::lua_createtable(l, 0, 3);
                    let e = ffi::lua_gettop(l);
                    let name = CString::new(r.name).unwrap();
                    ffi::lua_pushstring(l, name.as_ptr());
                    ffi::lua_setfield(l, e, c"kind".as_ptr());
                    ffi::lua_pushinteger(l, rec.hdr.req_id as i64);
                    ffi::lua_setfield(l, e, c"req_id".as_ptr());
                    if (r.check)(rec.payload()).is_ok() && marshal::record_to_table(l as *mut c_void, r, rec.payload(), None) {
                        ffi::lua_setfield(l, e, c"data".as_ptr());
                    }
                    ffi::lua_rawseti(l, t, n);
                }
                _ => {}
            }
        }
        1
    }
}

/// H.sc_dev(t): a dev_cmd from `hsmp-tools ipc-ctl` (NOT validated here: the module must drop
/// an invalid one).
unsafe extern "C-unwind" fn h_sc_dev(l: L) -> c_int {
    unsafe {
        let Some(r) = sc::record_by_name("dev_cmd") else { return fail(l, "no dev_cmd") };
        let p = match marshal_arg(l, 1, r, false) {
            Ok(p) => p,
            Err(e) => return fail(l, &e),
        };
        if seg().devctl().push_msg(shm::random_u64(), r.kind, 0, 0, 0, 0, &p).is_err() {
            return fail(l, "devctl full");
        }
        ffi::lua_pushboolean(l, 1);
        1
    }
}

unsafe extern "C-unwind" fn h_sleep(l: L) -> c_int {
    let ms = unsafe { ffi::lua_tointegerx(l, 1, std::ptr::null_mut()) };
    std::thread::sleep(std::time::Duration::from_millis(ms.max(0) as u64));
    0
}

/// Global `H` in `l`.
fn install_h(l: L) {
    unsafe {
        ffi::lua_createtable(l, 0, 8);
        let t = ffi::lua_gettop(l);
        let fns: [(&CStr, ffi::lua_CFunction); 7] = [
            (c"sc_put", h_sc_put),
            (c"sc_rec_event", h_sc_rec_event),
            (c"sc_push_n", h_sc_push_n),
            (c"sc_rec_drain", h_sc_rec_drain),
            (c"sc_dev", h_sc_dev),
            (c"sleep", h_sleep),
            (c"sleep_", h_sleep),
        ];
        for (n, f) in fns {
            ffi::lua_pushcclosure(l, f, 0);
            ffi::lua_setfield(l, t, n.as_ptr());
        }
        let comspec = CString::new(std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".into())).unwrap();
        ffi::lua_pushstring(l, comspec.as_ptr());
        ffi::lua_setfield(l, t, c"comspec".as_ptr());
        ffi::lua_setglobal(l, c"H".as_ptr());
    }
    // Wrap the sidecar helpers so a refusal raises in Lua (a test bug, not a module result).
    run(l, r#"
        for _, k in ipairs({"sc_put", "sc_rec_event", "sc_push_n", "sc_dev"}) do
            local f = H[k]
            H[k] = function(...) local ok, e = f(...); assert(ok, e); return ok end
        end
    "#);
}

fn lib_dir() -> String {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/hsmp-tools/lua-tests/lib");
    d.canonicalize().unwrap().to_string_lossy().replace('\\', "/").trim_start_matches("//?/").to_string()
}

#[cfg(windows)]
fn argv_round_trip(args: &[&str]) -> Vec<String> {
    #[link(name = "shell32")]
    extern "system" {
        fn CommandLineToArgvW(cmd: *const u16, argc: *mut i32) -> *mut *mut u16;
    }
    extern "system" {
        fn LocalFree(h: *mut c_void) -> *mut c_void;
    }
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let cl = hsmp_native::proc::command_line("C:\\x y\\a.exe", &owned).unwrap();
    let w: Vec<u16> = cl.encode_utf16().chain(Some(0)).collect();
    let mut argc = 0;
    // SAFETY: a NUL-terminated wide string; the result is freed with LocalFree.
    unsafe {
        let argv = CommandLineToArgvW(w.as_ptr(), &mut argc);
        let mut out = Vec::new();
        for i in 0..argc as usize {
            let p = *argv.add(i);
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
        }
        LocalFree(argv as *mut c_void);
        out
    }
}

#[test]
fn records_dev_proc_conformance_and_allocations() {
    // The Lua test table mirrors the Rust layouts exactly.
    let a = new_state("HSMPSync");
    let b = new_state("HSMPMenu");
    hsmp_native::api::register_test_schema(&TEST_RECORDS, test_slots());
    let lib = lib_dir();
    run(a, &format!(r#"package.path = "{}/?.lua;" .. package.path"#, lib));
    run(a, r#"C = require("records_conformance")"#);
    assert_eq!(eval_str(a, "C.signature()"), rust_signature(), "records_conformance.lua test schema drifted from tests/records.rs");

    // Open, attach a fake sidecar (so S2G epochs and sidecar-written meta match).
    let name = eval_str(a, "HSMPNative.ipc_open()");
    run(a, r#"assert(HSMPNative.frame("w1") ~= nil)"#);
    let map = Mapping::open(&name, Access::ReadWrite).expect("open mapping");
    let pid = shm::current_pid();
    let ct = shm::process_create_time(pid).unwrap();
    let epoch = 0xC0FFEE;
    let p = SideParams { pid, create_time: ct, epoch, caps: u64::MAX, build_id: "records-test" };
    attach_sidecar(map.segment().unwrap(), map.len(), pid, Some(ct), &p).expect("attach");
    SEG.store(map.segment().unwrap() as *const Segment as usize, Ordering::Release);
    SC_EPOCH.store(epoch, Ordering::Release);
    run(a, r#"HSMPNative.frame("w1")"#);
    install_h(a);

    // The shared conformance script.
    run(a, r#"
        local fails = {}
        local n = 0
        C.run(HSMPNative, H, function(ok, name, detail)
            n = n + 1
            if not ok then fails[#fails + 1] = name .. ": " .. tostring(detail) end
        end)
        assert(#fails == 0, #fails .. " of " .. n .. " conformance checks failed:\n" .. table.concat(fails, "\n"))
        print(string.format("conformance: %d checks passed", n))
    "#);

    // DevCtl fan-out: both states (registered before the commands) see each command once.
    install_h(b);
    run(a, r#"local o = {}; HSMPNative.dev_poll(64, o)"#);
    run(b, r#"local o = {}; HSMPNative.dev_poll(64, o)"#);
    run(a, r#"H.sc_dev({id = 10, op = 3, num = 1}); H.sc_dev({id = 11, op = 2, key = "k", num = 2})"#);
    run(a, r#"local o = {}; local n = HSMPNative.dev_poll(64, o); assert(n == 2 and o[1].data.id == 10 and o[2].data.id == 11, n)"#);
    run(b, r#"local o = {}; local n = HSMPNative.dev_poll(1, o); assert(n == 1 and o[1].data.id == 10, n)
              n = HSMPNative.dev_poll(8, o); assert(n == 1 and o[1].data.id == 11 and o[2] == nil, n)"#);
    let c = new_state("HSMPLate");
    run(c, r#"local o = {}; assert(HSMPNative.dev_poll(8, o) == 0, "a new state starts at now")"#);

    // argv quoting round-trips through CommandLineToArgvW.
    #[cfg(windows)]
    {
        let cases: &[&str] = &["", "a b", "a\"b", "a\\b", "a\\\\\"b", "trail\\", "trail space\\ ", "é ü", "\\\\server\\share\\", "x\ty"];
        let back = argv_round_trip(cases);
        assert_eq!(back[0], "C:\\x y\\a.exe");
        assert_eq!(&back[1..], cases, "argv round trip");
    }

    // stderr is captured too; a .cmd file runs through cmd.exe with every argument quoted.
    let dir = std::env::temp_dir().join(format!("hsmp native test {}", pid));
    std::fs::create_dir_all(&dir).unwrap();
    let bat = dir.join("args.cmd");
    std::fs::write(&bat, "@echo off\r\necho [%1] [%2] [%HSMP_T%]\r\nexit /b 4\r\n").unwrap();
    let bat_s = bat.to_string_lossy().replace('\\', "\\\\");
    run(a, &format!(r#"
        local N = HSMPNative
        local function wait_capture(h)
            for _ = 1, 1000 do
                local d, c, o = N.capture_poll(h)
                if d ~= false then return d, c, o end
                H.sleep(10)
            end
        end
        local h = assert(N.spawn_capture(H.comspec, {{"/c", "echo", "err", "1>&2"}}))
        local d, c, o = wait_capture(h)
        assert(d == true and c == 0 and o == "err \r\n", "stderr capture: " .. tostring(o))
        h = assert(N.spawn_capture("{bat}", {{"a b", "c&d"}}, {{env = {{HSMP_T = "v1"}}}}))
        d, c, o = wait_capture(h)
        assert(d == true and c == 4 and o == '["a b"] ["c&d"] [v1]\r\n', "batch: " .. tostring(c) .. " " .. tostring(o))
        -- capture_poll(h, wait_ms) blocks until the child exits.
        h = assert(N.spawn_capture(H.comspec, {{"/c", "echo", "waited"}}))
        d, c, o = N.capture_poll(h, 5000)
        assert(d == true and c == 0 and o == "waited\r\n", "blocking capture: " .. tostring(d) .. " " .. tostring(o))
        local r, e = N.spawn("C:/definitely/not/here.exe", {{}})
        assert(r == nil and e:match("^spawn: "), tostring(e))
    "#, bat = bat_s));
    let _ = std::fs::remove_dir_all(&dir);

    // ---- allocation and delivery regressions; timing-only work is opt-in -------------------
    let timings = std::env::var("HSMP_NATIVE_BENCH").as_deref() == Ok("1");
    run(a, &format!("BENCH_TIMINGS = {timings}"));
    run(a, &format!("RELEASE = {}", !cfg!(debug_assertions)));
    run(a, r#"while true do local d = H.sc_rec_drain(); if #d == 0 then break end end"#);
    run(a, r#"
        local N = HSMPNative
        local fixed = { u8v = 1, i8v = -2, u16v = 3, u32v = 4, i16v = -5, i32v = 6, u64v = 7, i64v = -8, f32v = 1.5, flag = true,
            f64v = 2.25, name = "Willie", vec = { 1, 2, 3 }, inner = { a = 1, b = true, c = 0.5 },
            inners = { { a = 2, b = false, c = 1 }, { a = 3, b = true, c = 2 } }, tags = { "ab", "cd" },
            grid = { { 1, 2 }, { 3, 4 } }, note = "a note of some length" }
        local rows = {}
        for i = 1, 24 do rows[i] = { k = i, v = i * 0.5, tag = "r" .. i } end
        local var = { id = 1, name = "var", rows = rows }
        local out, vout, ev = {}, {}, {}
        local function bench(label, n, fn)
            for _ = 1, 100 do fn() end
            local t0 = N.now_us()
            for _ = 1, n do fn() end
            local us = (N.now_us() - t0) / n
            print(string.format("TIMING %-36s %8.3f us", label, us))
            return us
        end
        -- allocation check: fixed and variable typed put/get into out, 10k times
        N.put("t_fixed_out", fixed); N.get("t_fixed_out", -1, out)
        N.put("t_var_out", var); N.get("t_var_out", -1, vout)
        collectgarbage("collect"); collectgarbage("stop")
        local k0 = collectgarbage("count")
        for _ = 1, 10000 do
            N.put("t_fixed_out", fixed)
            N.get("t_fixed_out", -1, out)
            N.put("t_var_out", var)
            N.get("t_var_out", -1, vout)
        end
        local grew = collectgarbage("count") - k0
        collectgarbage("restart")
        assert(grew < 1, "typed put/get with out allocated " .. grew .. " KiB over 10k")
        print(string.format("ALLOCATION typed put/get: %.3f KiB over 10k iterations", grew))
        if BENCH_TIMINGS then
        local t_put = bench("put t_fixed (152 B)", 20000, function() N.put("t_fixed_out", fixed) end)
        local ver = N.get("t_fixed_out", -1, out)
        local t_get = bench("get t_fixed changed (into out)", 20000, function() N.get("t_fixed_out", -1, out) end)
        bench("get t_fixed changed (fresh table)", 20000, function() N.get("t_fixed_out", -1) end)
        bench("get t_fixed unchanged", 20000, function() N.get("t_fixed_out", ver, out) end)
        bench("bus_put t_bus (152 B)", 20000, function() N.bus_put("t_bus", fixed) end)
        bench("bus_get t_bus changed (into out)", 20000, function() N.bus_get("t_bus", -1, out) end)
        local t_send = bench("send t_fixed (152 B)", 300, function() N.send("t_fixed", fixed) end)
        H.sc_rec_drain()
        bench("put t_var (24 rows, 408 B blob)", 20000, function() N.put("t_var_out", var) end)
        bench("get t_var (24 rows, into out)", 20000, function() N.get("t_var_out", -1, vout) end)
        bench("send t_var (24 rows)", 300, function() N.send("t_var", var) end)
        H.sc_rec_drain()
        -- Target < 5 us (reported); asserted with a 5x margin so a loaded machine (parallel builds,
        -- game soaks) does not fail the opt-in timing run.
        assert(not RELEASE or (t_put < 25 and t_get < 25 and t_send < 25), string.format("small-record budget: put %.2f get %.2f send %.2f us", t_put, t_get, t_send))
        end
        N.poll(64, ev)
        local function poll_conformance(label, kind, t)
            H.sc_push_n(kind, t, 1000)
            local t0 = BENCH_TIMINGS and N.now_us()
            local got = 0
            while got < 1000 do
                local n = N.poll(64, ev)
                if n == 0 then break end
                got = got + n
            end
            assert(got == 1000, got)
            if BENCH_TIMINGS then
                print(string.format("TIMING %-36s %8.3f us", label, (N.now_us() - t0) / 1000))
            end
        end
        poll_conformance("poll t_fixed (per event)", "t_fixed", fixed)
        poll_conformance("poll t_var 24 rows (per event)", "t_var", var)
    "#);

    unsafe {
        ffi::lua_close(a);
        ffi::lua_close(b);
        ffi::lua_close(c);
    }
    drop(map);
}

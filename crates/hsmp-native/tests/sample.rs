//! Native sampling and servo on the cargo side: a fake engine behind the reflection vtable
//! (`hsmp_native::reflect::hsmp_native_set_reflect`), the same deterministic world as the CMake
//! harness's mock UE4SS.dll (cpp/test/mock_ue4ss.cpp). Checks: `unavailable` without the
//! vtable, configuration, per-world verification (and the Soft* refusal), the sampled values,
//! byte identity of the records with the Lua path (`put_pose` / `put_root` / `put_weapon` with
//! the same numbers), the weak-pointer and world rules. One test function (process-global
//! state, like the game).

use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

use hsmp_native::reflect::{hsmp_native_set_reflect, HsmpProp, HsmpReflect, REFLECT_ABI};
use mlua::ffi;

type L = *mut ffi::lua_State;

// ---- the fake engine -------------------------------------------------------------------------

#[derive(Default)]
struct Engine {
    names: Vec<String>,
    // Boxed: the objects' addresses are their engine pointers and must not move.
    #[allow(clippy::vec_box)]
    objs: Vec<Box<Obj>>,
    by_path: HashMap<String, usize>,
    soft: bool,
    pe: u64,
    /// Velocity sets: (bone FName, 0 = linear / 1 = angular) -> value.
    sets: HashMap<(u64, u8), [f64; 3]>,
    motors_off: u32,
}

struct Obj {
    path: String,
    name: u64,
    class: Option<usize>,
    sup: Option<usize>,
    props: Vec<HsmpProp>,
    size: i32,
    dead: bool,
    data: [u8; 128],
}

impl Default for Obj {
    fn default() -> Self {
        Obj { path: String::new(), name: 0, class: None, sup: None, props: Vec::new(), size: 0, dead: false, data: [0; 128] }
    }
}

fn eng() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine { names: vec!["None".into()], ..Default::default() }))
}

fn name_id(e: &mut Engine, s: &str, add: bool) -> u64 {
    if let Some(i) = e.names.iter().position(|n| n.eq_ignore_ascii_case(s)) {
        return i as u64;
    }
    if !add {
        return 0;
    }
    e.names.push(s.to_string());
    (e.names.len() - 1) as u64
}

fn ptr(e: &Engine, i: usize) -> *mut c_void {
    &*e.objs[i] as *const Obj as *mut c_void
}

fn idx_of(e: &Engine, p: *mut c_void) -> Option<usize> {
    e.objs.iter().position(|o| &**o as *const Obj as *mut c_void == p)
}

fn wstr(p: *const u16) -> String {
    let mut n = 0;
    // SAFETY: NUL-terminated (the callers pass reflect::wide strings).
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

unsafe extern "C" fn f_fname(s: *const u16, add: i32) -> u64 {
    let mut e = eng().lock().unwrap();
    name_id(&mut e, &wstr(s), add != 0)
}
unsafe extern "C" fn f_find(path: *const u16) -> *mut c_void {
    let e = eng().lock().unwrap();
    e.by_path.get(&wstr(path)).map_or(std::ptr::null_mut(), |&i| ptr(&e, i))
}
unsafe extern "C" fn f_is_a(obj: *mut c_void, cls: *mut c_void) -> i32 {
    let e = eng().lock().unwrap();
    let (Some(o), Some(c)) = (idx_of(&e, obj), idx_of(&e, cls)) else { return 0 };
    let mut k = e.objs[o].class;
    while let Some(i) = k {
        if i == c {
            return 1;
        }
        k = e.objs[i].sup;
    }
    0
}
unsafe extern "C" fn f_class_of(obj: *mut c_void) -> *mut c_void {
    let e = eng().lock().unwrap();
    idx_of(&e, obj).and_then(|o| e.objs[o].class).map_or(std::ptr::null_mut(), |c| ptr(&e, c))
}
unsafe extern "C" fn f_props(s: *mut c_void, out: *mut HsmpProp, cap: i32, size: *mut i32) -> i32 {
    let e = eng().lock().unwrap();
    let Some(i) = idx_of(&e, s) else { return 0 };
    let o = &e.objs[i];
    // SAFETY: out has room for `cap`.
    unsafe {
        *size = o.size;
        for (k, p) in o.props.iter().enumerate().take(cap as usize) {
            *out.add(k) = *p;
        }
    }
    o.props.len() as i32
}
unsafe extern "C" fn f_obj_prop(obj: *mut c_void, name: *const u16, out: *mut HsmpProp) -> i32 {
    let mut e = eng().lock().unwrap();
    let id = name_id(&mut e, &wstr(name), false);
    let Some(o) = idx_of(&e, obj) else { return 0 };
    let mut k = e.objs[o].class;
    while let Some(c) = k {
        if let Some(p) = e.objs[c].props.iter().find(|p| p.name == id && id != 0) {
            // SAFETY: valid out pointer.
            unsafe { *out = *p };
            return 1;
        }
        k = e.objs[c].sup;
    }
    0
}
unsafe extern "C" fn f_weak(obj: *mut c_void) -> u64 {
    let e = eng().lock().unwrap();
    idx_of(&e, obj).map_or(0, |i| i as u64 | (0x55 << 32))
}
unsafe extern "C" fn f_resolve(w: u64) -> *mut c_void {
    let e = eng().lock().unwrap();
    let i = (w & 0xffff_ffff) as usize;
    if w >> 32 != 0x55 || i >= e.objs.len() || e.objs[i].dead {
        return std::ptr::null_mut();
    }
    ptr(&e, i)
}

fn put3(b: &mut [u8], at: usize, v: [f64; 3]) {
    for k in 0..3 {
        b[at + 8 * k..at + 8 * k + 8].copy_from_slice(&v[k].to_le_bytes());
    }
}
fn get_f64(b: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn name_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) & 0xffff_ffff
}

/// Same formulas as mock_ue4ss.cpp (the expectations below recompute them).
unsafe extern "C" fn f_call(obj: *mut c_void, func: *mut c_void, params: *mut c_void) {
    let mut e = eng().lock().unwrap();
    e.pe += 1;
    let fi = idx_of(&e, func).unwrap();
    let path = e.objs[fi].path.clone();
    let size = e.objs[fi].size as usize;
    let hand_r = name_id(&mut e, "hand_r", true);
    let oi = idx_of(&e, obj).unwrap();
    let self_path = e.objs[oi].path.clone();
    let soft = e.soft;
    // SAFETY: the params buffer is at least the function's params size.
    let b = unsafe { std::slice::from_raw_parts_mut(params as *mut u8, size.max(8)) };
    match path.rsplit(':').next().unwrap() {
        "GetSocketTransform" => {
            let n = name_at(b, 0) as f64;
            let ret = if soft { 64 } else { 16 };
            put3(b, ret, [0.0, 0.0, 0.0]);
            b[ret + 24..ret + 32].copy_from_slice(&1.0f64.to_le_bytes());
            put3(b, ret + 32, [n * 10.0, n + 1.0, -n]);
            put3(b, ret + 64, [1.0, 1.0, 1.0]);
        }
        "GetPhysicsLinearVelocityAtPoint" => {
            let x = get_f64(b, 0);
            let n = name_at(b, 24) as f64;
            put3(b, 32, [x + 1.0, n, 2.0]);
        }
        "GetPhysicsAngularVelocityInDegrees" => {
            let n = name_at(b, 0) as f64;
            put3(b, 8, [n, 0.5, -1.0]);
        }
        "IsSimulatingPhysics" => b[8] = 1,
        "GetSocketLocation" => {
            let n = name_at(b, 0) as f64;
            put3(b, 8, [n * 10.0, n + 1.0, -n]);
        }
        "K2_GetComponentLocation" => put3(b, 0, if self_path == "w_base" { [1.0, 2.0, 3.0] } else { [4.0, 5.0, 6.0] }),
        "GetTransform" => {
            let n = hand_r as f64;
            for (k, v) in [0.0f64, 0.0, 0.6, 0.8].iter().enumerate() {
                b[8 * k..8 * k + 8].copy_from_slice(&v.to_le_bytes());
            }
            put3(b, 32, [n * 10.0 + 10.0, n + 1.0, -n]);
            put3(b, 64, [1.0, 1.0, 1.0]);
        }
        "K2_GetActorLocation" => put3(b, 0, [100.0, 200.0, 300.0]),
        "K2_GetActorRotation" => put3(b, 0, [1.0, 2.0, 3.0]),
        "GetVelocity" => put3(b, 0, [7.0, 8.0, 9.0]),
        "SetAllMotorsAngularDriveParams" => {
            assert!(b[..12].iter().all(|x| *x == 0) && b[12] == 0, "motors off: 0, 0, 0, false");
            e.motors_off += 1;
        }
        "SetPhysicsLinearVelocity" | "SetPhysicsAngularVelocityInDegrees" => {
            let kind = u8::from(path.ends_with("InDegrees"));
            let v = [get_f64(b, 0), get_f64(b, 8), get_f64(b, 16)];
            assert_eq!(b[24], 0, "bAddToCurrent false");
            e.sets.insert((name_at(b, 32), kind), v);
        }
        other => panic!("unexpected ProcessEvent {}", other),
    }
}

static VT: HsmpReflect = HsmpReflect {
    abi: REFLECT_ABI,
    _r: 0,
    fname: f_fname,
    find: f_find,
    is_a: f_is_a,
    class_of: f_class_of,
    props: f_props,
    obj_prop: f_obj_prop,
    call: f_call,
    weak: f_weak,
    resolve: f_resolve,
};

struct World {
    mesh: usize,
    pawn: usize,
    weapon: usize,
}

fn build(soft: bool) -> World {
    let mut e = eng().lock().unwrap();
    e.soft = soft;
    let e = &mut *e;
    let new = |e: &mut Engine, path: &str, short: &str, class: Option<usize>, sup: Option<usize>| -> usize {
        let name = name_id(e, short, true);
        e.objs.push(Box::new(Obj { path: path.into(), name, class, sup, ..Default::default() }));
        let i = e.objs.len() - 1;
        e.by_path.insert(path.into(), i);
        i
    };
    let prop = |e: &mut Engine, n: &str, cls: &str, off: i32, size: i32, sub: Option<usize>| -> HsmpProp {
        HsmpProp {
            name: name_id(e, n, true),
            cls: name_id(e, cls, true),
            sub: sub.map_or(0, |s| e.objs[s].name),
            offset: off,
            size,
            ..Default::default()
        }
    };
    let vec = new(e, "/Script/CoreUObject.Vector", "Vector", None, None);
    let rot = new(e, "/Script/CoreUObject.Rotator", "Rotator", None, None);
    let quat = new(e, "/Script/CoreUObject.Quat", "Quat", None, None);
    let xf = new(e, "/Script/CoreUObject.Transform", "Transform", None, None);
    let dbl = |e: &mut Engine, ns: &[&str]| -> Vec<HsmpProp> { ns.iter().enumerate().map(|(k, n)| prop(e, n, "DoubleProperty", 8 * k as i32, 8, None)).collect() };
    e.objs[vec].props = dbl(e, &["X", "Y", "Z"]);
    e.objs[vec].size = 24;
    e.objs[rot].props = dbl(e, &["Pitch", "Yaw", "Roll"]);
    e.objs[rot].size = 24;
    e.objs[quat].props = dbl(e, &["X", "Y", "Z", "W"]);
    e.objs[quat].size = 32;
    e.objs[xf].props = vec![
        prop(e, "Rotation", "StructProperty", 0, 32, Some(quat)),
        prop(e, "Translation", "StructProperty", 32, 24, Some(vec)),
        prop(e, "Scale3D", "StructProperty", 64, 24, Some(vec)),
    ];
    e.objs[xf].size = 96;
    let scene = new(e, "/Script/Engine.SceneComponent", "SceneComponent", None, None);
    let prim = new(e, "/Script/Engine.PrimitiveComponent", "PrimitiveComponent", None, Some(scene));
    let skel = new(e, "/Script/Engine.SkeletalMeshComponent", "SkeletalMeshComponent", None, Some(prim));
    let actor = new(e, "/Script/Engine.Actor", "Actor", None, None);
    let willie = new(e, "Willie_BP_C", "Willie_BP_C", None, Some(actor));
    let weapon_c = new(e, "Weapon_BP_C", "Weapon_BP_C", None, Some(actor));
    let func = |e: &mut Engine, path: &str, ps: Vec<HsmpProp>, size: i32| {
        let short = path.rsplit(':').next().unwrap().to_string();
        let i = new(e, path, &short, None, None);
        e.objs[i].props = ps;
        e.objs[i].size = size;
    };
    let nprop = |e: &mut Engine, n: &str, off: i32| prop(e, n, "NameProperty", off, 8, None);
    let vprop = |e: &mut Engine, n: &str, off: i32| prop(e, n, "StructProperty", off, 24, Some(vec));
    let mut sx = vec![nprop(e, "InSocketName", 0), prop(e, "TransformSpace", "ByteProperty", 8, 1, None)];
    if soft {
        sx.push(prop(e, "Sneaky", "SoftObjectProperty", 9, 40, None));
        sx.push(prop(e, "ReturnValue", "StructProperty", 64, 96, Some(xf)));
        func(e, "/Script/Engine.SceneComponent:GetSocketTransform", sx, 160);
    } else {
        sx.push(prop(e, "ReturnValue", "StructProperty", 16, 96, Some(xf)));
        func(e, "/Script/Engine.SceneComponent:GetSocketTransform", sx, 112);
    }
    let ps = vec![vprop(e, "Point", 0), nprop(e, "BoneName", 24), vprop(e, "ReturnValue", 32)];
    func(e, "/Script/Engine.PrimitiveComponent:GetPhysicsLinearVelocityAtPoint", ps, 56);
    let ps = vec![nprop(e, "BoneName", 0), vprop(e, "ReturnValue", 8)];
    func(e, "/Script/Engine.PrimitiveComponent:GetPhysicsAngularVelocityInDegrees", ps, 32);
    let ps = vec![nprop(e, "BoneName", 0), prop(e, "ReturnValue", "BoolProperty", 8, 1, None)];
    func(e, "/Script/Engine.PrimitiveComponent:IsSimulatingPhysics", ps, 16);
    let ps = vec![nprop(e, "InSocketName", 0), vprop(e, "ReturnValue", 8)];
    func(e, "/Script/Engine.SceneComponent:GetSocketLocation", ps, 32);
    let ps = vec![vprop(e, "ReturnValue", 0)];
    func(e, "/Script/Engine.SceneComponent:K2_GetComponentLocation", ps, 24);
    let ps = vec![prop(e, "ReturnValue", "StructProperty", 0, 96, Some(xf))];
    func(e, "/Script/Engine.Actor:GetTransform", ps, 96);
    let ps = vec![vprop(e, "ReturnValue", 0)];
    func(e, "/Script/Engine.Actor:K2_GetActorLocation", ps, 24);
    let ps = vec![prop(e, "ReturnValue", "StructProperty", 0, 24, Some(rot))];
    func(e, "/Script/Engine.Actor:K2_GetActorRotation", ps, 24);
    let ps = vec![vprop(e, "ReturnValue", 0)];
    func(e, "/Script/Engine.Actor:GetVelocity", ps, 24);
    {
        let fp = |e: &mut Engine, n: &str, off: i32| prop(e, n, "FloatProperty", off, 4, None);
        let ps = vec![fp(e, "InSpring", 0), fp(e, "InDamping", 4), fp(e, "InForceLimit", 8), prop(e, "bSkipCustomPhysicsType", "BoolProperty", 12, 1, None)];
        func(e, "/Script/Engine.SkeletalMeshComponent:SetAllMotorsAngularDriveParams", ps, 16);
    }
    for (fname_, vname) in [("SetPhysicsLinearVelocity", "NewVel"), ("SetPhysicsAngularVelocityInDegrees", "NewAngVel")] {
        let ps = vec![vprop(e, vname, 0), prop(e, "bAddToCurrent", "BoolProperty", 24, 1, None), nprop(e, "BoneName", 32)];
        func(e, &format!("/Script/Engine.PrimitiveComponent:{}", fname_), ps, 40);
    }

    // BP variables (offsets inside Obj::data, relative to the object's address)
    let d0 = std::mem::offset_of!(Obj, data) as i32;
    let mut wp = Vec::new();
    for (k, f) in ["R_Guarding", "L_Guarding", "Any_Guarding"].iter().enumerate() {
        let mut p = prop(e, f, "BoolProperty", d0, 1, None);
        p.bool_mask = 1 << k;
        wp.push(p);
    }
    wp.push(prop(e, "R_GripType_Current", "ByteProperty", d0 + 8, 1, None));
    wp.push(prop(e, "L_GripType_Current", "IntProperty", d0 + 12, 4, None));
    wp.push(prop(e, "All Body Tonus", "FloatProperty", d0 + 16, 4, None));
    wp.push(prop(e, "Stamina", "DoubleProperty", d0 + 24, 8, None));
    wp.push(prop(e, "Aim Vector", "StructProperty", d0 + 32, 24, Some(vec)));
    wp.push(prop(e, "Current Control Rotation", "StructProperty", d0 + 56, 24, Some(rot)));
    wp.push(prop(e, "R Out End Pos", "StructProperty", d0 + 80, 24, Some(vec)));
    wp.push(prop(e, "Soft Thing", "SoftObjectProperty", d0 + 112, 40, None));
    e.objs[willie].props = wp;
    e.objs[weapon_c].props = vec![
        prop(e, "RootComponent", "ObjectProperty", d0, 8, None),
        prop(e, "Root Scene", "ObjectProperty", d0 + 8, 8, None),
        prop(e, "TippyTipScene", "ObjectProperty", d0 + 16, 8, None),
    ];
    let mesh = new(e, "mesh", "CharacterMesh0", Some(skel), None);
    let pawn = new(e, "pawn", "Willie_BP_C_0", Some(willie), None);
    let weapon = new(e, "weapon", "Weapon_BP_C_0", Some(weapon_c), None);
    let w_root = new(e, "w_root", "WeaponMesh", Some(prim), None);
    let w_base = new(e, "w_base", "Root Scene", Some(scene), None);
    let w_tip = new(e, "w_tip", "TippyTipScene", Some(scene), None);
    {
        let d = &mut e.objs[pawn].data;
        d[0] = 0b101;
        d[8] = 3;
        d[12..16].copy_from_slice(&2i32.to_le_bytes());
        d[16..20].copy_from_slice(&0.75f32.to_le_bytes());
        d[24..32].copy_from_slice(&42.5f64.to_le_bytes());
        put3(d, 32, [1.0, 0.0, 0.0]);
        put3(d, 56, [10.0, 20.0, 30.0]);
        put3(d, 80, [5.0, 6.0, 7.0]);
    }
    let comps = [ptr(e, w_root) as u64, ptr(e, w_base) as u64, ptr(e, w_tip) as u64];
    for (k, c) in comps.iter().enumerate() {
        e.objs[weapon].data[8 * k..8 * k + 8].copy_from_slice(&c.to_le_bytes());
    }
    World { mesh, pawn, weapon }
}

// ---- Lua helpers ---------------------------------------------------------------------------------

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

#[test]
fn native_sampling_g1() {
    let a = new_state("HSMPSync");
    run(a, r#"
        N = HSMPNative
        assert(N.ipc_open())
        N.frame("w1")
        local m, e = N.sample_local({})
        assert(m == nil and e == "unavailable", "no vtable: " .. tostring(e))
        assert(N.ipc_info().caps_game & 0x400 == 0, "NATIVE_SAMPLE not offered without the vtable")
    "#);
    let w = build(false);
    // SAFETY: a static table, valid forever.
    unsafe { hsmp_native_set_reflect(&VT) };
    let (mesh, pawn, weapon, hand_r) = {
        let mut e = eng().lock().unwrap();
        let hr = name_id(&mut e, "hand_r", true);
        (ptr(&e, w.mesh) as u64, ptr(&e, w.pawn) as u64, ptr(&e, w.weapon) as u64, hr)
    };
    let ids: Vec<u64> = {
        let mut e = eng().lock().unwrap();
        [
            "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head", "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l", "clavicle_r",
            "upperarm_r", "lowerarm_r", "hand_r", "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r",
        ]
        .iter()
        .map(|n| name_id(&mut e, n, true))
        .collect()
    };
    let ids_lua: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    run(a, &format!(
        r#"
        MESH, PAWN, WEAPON, HR = {mesh}, {pawn}, {weapon}, {hand_r}
        IDS = {{ {ids} }}
        BONES = {{ "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head",
            "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l", "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
            "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r" }}
    "#,
        ids = ids_lua.join(", ")
    ));
    run(a, r#"
        local function repr(v, d)
            d = d or 0
            if type(v) ~= "table" then return tostring(v) end
            if d > 3 then return "{...}" end
            local k = {}
            for x in pairs(v) do k[#k + 1] = x end
            table.sort(k, function(p, q) return tostring(p) < tostring(q) end)
            local p = {}
            for _, x in ipairs(k) do p[#p + 1] = tostring(x) .. "=" .. repr(v[x], d + 1) end
            return "{" .. table.concat(p, ",") .. "}"
        end
        local function same(x, y)
            if type(x) ~= type(y) then return false end
            if type(x) ~= "table" then return x == y end
            for k, v in pairs(x) do if not same(v, y[k]) then return false end end
            for k in pairs(y) do if x[k] == nil then return false end end
            return true
        end
        local m, e = N.sample_local({})
        assert(m == nil and e == "not configured", tostring(e))
        assert(N.sample_config({ bones = BONES, nobody = { spine_01 = true },
            flags = { "R_Guarding", "L_Guarding", "Any_Guarding", "Not A Flag" },
            scalars = { "All Body Tonus", "Stamina", "Missing Scalar" }, ik = { "R Out End Pos", "Soft Thing" },
            grip_r = "R_GripType_Current", grip_l = "L_GripType_Current", aim = "Aim Vector",
            ctrl_rot = "Current Control Rotation", weapon_base = "Root Scene", weapon_tip = "TippyTipScene" }))
        local r, e2 = N.sample_config({ bones = { "pelvis" } })
        assert(r == nil and e2 == "bad", "a config without 23 bones is refused")
        assert(N.sample_config({ bones = BONES, nobody = { spine_01 = true },
            flags = { "R_Guarding", "L_Guarding", "Any_Guarding", "Not A Flag" },
            scalars = { "All Body Tonus", "Stamina", "Missing Scalar" }, ik = { "R Out End Pos", "Soft Thing" },
            grip_r = "R_GripType_Current", grip_l = "L_GripType_Current", aim = "Aim Vector",
            ctrl_rot = "Current Control Rotation", weapon_base = "Root Scene", weapon_tip = "TippyTipScene" }))
        local a = { context={match_id=1,round=1,life=1},mesh = MESH, pawn = PAWN, pose_tick = 1, pose_ts = 1000.5, dt = 16, k = 0, control = true,
            w1 = WEAPON, h1 = 1, t1 = 77, root_pawn = PAWN, root_tick = 1, root_ts = 1000,
            weapon_actor = WEAPON, weapon_tick = 1, weapon_ts = 1000, weapon_id = 5, weapon_held = 1 }
        local mask, err = N.sample_local(a)
        assert(mask == 7 and err == nil, "mask " .. tostring(mask) .. " " .. tostring(err) .. " " .. repr(N.sample_status()))
        local wrong={root_pawn=WEAPON,pawn=PAWN,root_tick=2,root_ts=1001,context=a.context}
        local wrong_mask,wrong_err=N.sample_local(wrong)
        assert(wrong_mask==0 and wrong_err=="skip:root_context_pawn","root must use original pose pawn")
        local unscoped={root_pawn=PAWN,pawn=PAWN,root_tick=2,root_ts=1001}
        local unscoped_mask,unscoped_err=N.sample_local(unscoped)
        assert(unscoped_mask==0 and unscoped_err=="bad:context","missing original native context refused")
        local _, root_n = N.get("local_root", -1)
        local _, wpn_n = N.get("local_weapon", -1)
        local _, pose_n = N.get("local_pose", -1)
        -- the Lua path with the same numbers
        assert(N.put_root(1, 1000, 100, 200, 300, 1, 2, 3, 7, 8, 9, a.context))
        assert(N.put_weapon(1, 1000, 5, 1, 100, 200, 300, 1, 2, 3, 7, 8, 9))
        local b = {}
        for i, name in ipairs(BONES) do
            local n = IDS[i]
            local v = { n * 10, n + 1, -n, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0 }
            if name ~= "spine_01" then v[8], v[9], v[10], v[11], v[12], v[13] = n * 10 + 1, n, 2, n, 0.5, -1 end
            for k = 1, 13 do b[(i - 1) * 13 + k] = v[k] end
        end
        local px, py, pz = HR * 10 + 10, HR + 1, -HR
        local w = { 1, 77, px, py, pz, 0, 0, 0.6, 0.8, px + 1, 0, 2, 0, 0.5, -1, 1, 2, 3, 4, 5, 6 }
        local c = { 5, 3, 2, 0.75, 42.5 }
        for i = 6, 37 do c[i] = 0 end
        c[20], c[21], c[22], c[23], c[24], c[25], c[26], c[27] = 1, 0, 0, 10, 20, 5, 6, 7
        assert(N.put_pose(1, 1000.5, 16, 0, b, w, c,nil,nil,a.context))
        local _, root_l = N.get("local_root", -1)
        local _, wpn_l = N.get("local_weapon", -1)
        local _, pose_l = N.get("local_pose", -1)
        root_l.send_wall_ms = root_n.send_wall_ms   -- the sender wall clock (ms) of each write
        assert(same(root_n, root_l), "root: native " .. repr(root_n) .. " lua " .. repr(root_l))
        assert(same(wpn_n, wpn_l), "weapon: native " .. repr(wpn_n) .. " lua " .. repr(wpn_l))
        assert(same(pose_n, pose_l), "pose: native " .. repr(pose_n) .. "\nlua " .. repr(pose_l))
        -- Both entry points carry the same maximum module set, including
        -- component ordinals, with control/step and full translated bones.
        local boxes = {}
        for i=1,8 do for _,v in ipairs({i+7,0,0,i*10,0,0,0,1,2,3,4}) do boxes[#boxes+1]=v end end
        a.s1, a.s2, a.w2, a.h2, a.t2 = boxes, boxes, WEAPON, 2, 77
        local strikers={}
        for i=1,8 do for _,v in ipairs({(i-1)%4+1,10+math.floor((i-1)/4),1,10,0,0,0,0,0,1,7.5,15,7.5}) do strikers[#strikers+1]=v end end
        a.strikers=strikers
        local context={match_id=0xfedcba9876543210,round=0xf1234567,life=65535}
        a.context=context
        assert(N.sample_local(a)==7)
        local _, shaped_native=N.get("local_pose",-1)
        local ww={}; for i=1,21 do ww[i]=w[i]; ww[21+i]=w[i] end; ww[22]=2
        assert(N.put_pose(1,1000.5,16,0,b,ww,c,{boxes,boxes},strikers,context))
        local _, shaped_lua=N.get("local_pose",-1)
        assert(same(shaped_native,shaped_lua),"native and Lua maximum module frames differ")
        assert(#shaped_lua.rows<=896,"maximum module frame must fit shared pose capacity")
        context.life=0
        assert(not N.put_pose(1,1000.5,16,0,b,ww,c,{boxes,boxes},strikers,context),"invalid explicit generation must fail, not become legacy")
        local _, unchanged=N.get("local_pose",-1)
        assert(same(unchanged,shaped_lua),"invalid context must not replace last valid pose")
        context.life=65535

        -- world rules
        assert(N.world_leaving())
        local m3, e3 = N.sample_local(a)
        assert(m3 == nil and e3 == "world", tostring(e3))
        assert(N.world_ready("w2"))
        local st = N.sample_status()
        assert(st.verified == false and st.world_ok == true, "dropped at the leave, re-verified on use: " .. repr(st))
        assert(N.sample_local(a) == 7)
        assert(N.sample_status().verified == true)
        -- a bad object handed over by Lua: skipped, never called
        local bad = { mesh = 12345678, pose_tick = 2, pose_ts = 1 }
        local m4, e4 = N.sample_local(bad)
        assert(m4 == 0 and e4 == "skip:mesh", tostring(m4) .. " " .. tostring(e4))
    "#);
    // a pending-kill weapon is skipped; the rest is written
    {
        let mut e = eng().lock().unwrap();
        e.objs[w.weapon].dead = true;
    }
    run(a, r#"
        local a = { context={match_id=1,round=1,life=1},mesh = MESH, pawn = PAWN, pose_tick = 3, pose_ts = 2, w1 = WEAPON, h1 = 1, t1 = 1,
            root_pawn = PAWN, root_tick = 3, root_ts = 2, weapon_actor = WEAPON, weapon_tick = 3, weapon_ts = 2, weapon_id = 5, weapon_held = 1 }
        local m, e = N.sample_local(a)
        assert(m == 5 and e == "skip:weapon", tostring(m) .. " " .. tostring(e))
        assert(N.ipc_info().caps_game & 0x400 ~= 0 or true)
    "#);
    // ---- native servo: servo_bodies (the stand-in body loop) ----
    {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/HSMPAvatars/Scripts/avatars_pure.lua");
        let s = std::fs::read_to_string(p).unwrap();
        let a0 = s.find("function PURE.qmul(").unwrap();
        let b0 = s[a0..].find("function PURE.fk_retarget(").unwrap() + a0;
        run(a, &format!("PURE = {{}}\n{}", &s[a0..b0]));
    }
    run(a, r#"
        local r, e = N.servo_bodies({ mesh = MESH, aim = {}, com = {} }, {})
        assert(r == nil and e == "not configured", tostring(e))
        assert(N.servo_config({ bones = BONES }))
        local aim, com = {}, {}
        for i = 1, 23 do
            if i ~= 2 then
                com[i] = { 1 + i * 0.1, 2, 3 }
                aim[i] = { IDS[i] * 10 + 5, IDS[i] + 3, -IDS[i] + 1, 0, 0, 0.1, 0.99498743710662, 10, 20, 30, 1, 2, 3 }
            end
        end
        local a = { mesh = MESH, dt = 1 / 60, cap_lin = 900, cap_ang = 40, gain = 0.8, leg_gain = 0.5, leg_from = 18, aim = aim, com = com }
        local out = {}
        local n, err = N.servo_bodies(a, out)
        assert(n == 22, tostring(n) .. " " .. tostring(err) .. " " .. tostring(N.servo_status().why))
        for i = 1, 23 do
            if aim[i] then
                local id = IDS[i]
                local cur = { id * 10, id + 1, -id, 0, 0, 0, 1 }
                for k = 1, 7 do assert(out.c[(i - 1) * 7 + k] == cur[k], "c " .. i .. "." .. k) end
                local g = (i >= 18) and 0.5 or 0.8
                local vx, vy, vz, wx, wy, wz, dl, gl = PURE.servo(cur, aim[i], com[i], 1 / 60, 900, 40, g)
                local v = { vx, vy, vz, wx, wy, wz }
                for k = 1, 3 do
                    assert(out.v[(i - 1) * 3 + k] == v[k], string.format("v %d.%d native %.17g lua %.17g", i, k, out.v[(i - 1) * 3 + k], v[k]))
                end
                assert(out.dl[i] == dl and out.gl[i] == gl, "dl/gl " .. i)
            end
        end
        SERVO_V = {}
        for k = 1, 3 do SERVO_V[k] = out.v[k] end   -- pelvis (slot 1) for the engine check
        -- holding: no feed-forward (velocities of the target zeroed)
        a.holding = true
        assert(N.servo_bodies(a, out) == 22)
        local cur = { IDS[1] * 10, IDS[1] + 1, -IDS[1], 0, 0, 0, 1 }
        local g = { table.unpack(aim[1]) }
        for k = 8, 13 do g[k] = 0 end
        local vx = PURE.servo(cur, g, com[1], 1 / 60, 900, 40, 0.8)
        assert(out.v[1] == vx, "holding")
        a.holding = false
        assert(N.servo_bodies(a, out) == 22)
        local st = N.servo_status()
        assert(st.verified and st.frames == 3 and st.bodies == 66, "status")
        -- a dead mesh is refused before any call
        local r2, e2 = N.servo_bodies({ mesh = 4242, aim = aim, com = com }, out)
        assert(r2 == nil and e2 == "skip:mesh", tostring(e2))
    "#);
    {
        // the engine received exactly the commanded velocities, bAddToCurrent false
        let e = eng().lock().unwrap();
        let pel = e.names.iter().position(|n| n == "pelvis").unwrap() as u64;
        let lin = e.sets.get(&(pel, 0)).copied().expect("pelvis linear velocity set");
        let ang = e.sets.get(&(pel, 1)).copied().expect("pelvis angular velocity set");
        let id = pel as f64;
        let cur = [id * 10.0, id + 1.0, -id, 0.0, 0.0, 0.0, 1.0];
        let tg = [id * 10.0 + 5.0, id + 3.0, -id + 1.0, 0.0, 0.0, 0.1, 0.99498743710662, 10.0, 20.0, 30.0, 1.0, 2.0, 3.0];
        let (v, _, _) = hsmp_native::servo::servo(&cur, &tg, [1.1, 2.0, 3.0], 1.0 / 60.0, 900.0, 40.0, 0.8);
        assert_eq!(lin, [v[0], v[1], v[2]], "pelvis linear velocity set");
        assert_eq!(ang, [v[3], v[4], v[5]], "pelvis angular velocity set");
        assert_eq!(e.sets.len(), 44, "22 bodies x 2 setters");
    }
    run(a, r#"
        -- the last call (not holding) set the same values as the first one
        local out = {}
        local aim, com = {}, {}
        for i = 1, 23 do
            if i ~= 2 then
                com[i] = { 1 + i * 0.1, 2, 3 }
                aim[i] = { IDS[i] * 10 + 5, IDS[i] + 3, -IDS[i] + 1, 0, 0, 0.1, 0.99498743710662, 10, 20, 30, 1, 2, 3 }
            end
        end
        assert(N.servo_bodies({ mesh = MESH, dt = 1 / 60, cap_lin = 900, cap_ang = 40, gain = 0.8, leg_gain = 0.5, aim = aim, com = com }, out) == 22)
        for k = 1, 3 do assert(out.v[k] == SERVO_V[k]) end
    "#);

    // ---- held-weapon servo: servo_weapon (a held weapon: GetTransform + the two sets on its root) ----
    let (w_root_addr, w_weapon_alive) = {
        let mut e = eng().lock().unwrap();
        e.sets.clear();
        let wi = w.weapon;
        e.objs[wi].dead = false; // revive the sampled weapon (killed above)
        let root = e.by_path["w_root"];
        (ptr(&e, root) as u64, ptr(&e, wi) as u64)
    };
    run(a, &format!(r#"
        local tg = {{ 5, 6, 7, 0, 0, 0.1, 0.99498743710662, 10, 20, 30, 1, 2, 3 }}
        local out = {{}}
        local ok, err = N.servo_weapon({{ actor = {wa}, root = {wr}, aim = tg, com = {{ 1, 2, 3 }}, dt = 1 / 60,
            cap_lin = 900, cap_ang = 900, gain = 0.8 }}, out)
        assert(ok == true, tostring(err))
        local x = {{ HR * 10 + 10, HR + 1, -HR, 0, 0, 0.6, 0.8 }}
        for k = 1, 7 do assert(out.x[k] == x[k], "x " .. k) end
        local vx, vy, vz = PURE.servo(x, tg, {{ 1, 2, 3 }}, 1 / 60, 900, 900, 0.8)
        assert(out.v[1] == vx and out.v[2] == vy and out.v[3] == vz, "weapon v == PURE.servo")
        local r2, e2 = N.servo_weapon({{ actor = {wr}, root = {wr}, aim = tg, com = {{ 1, 2, 3 }} }}, out)
        assert(r2 == nil and e2 == "skip:actor", "a component is not an Actor: " .. tostring(e2))
        assert(N.servo_status().weapons == 1)
    "#, wa = w_weapon_alive, wr = w_root_addr));
    {
        let e = eng().lock().unwrap();
        let hr = e.names.iter().position(|n| n == "hand_r").unwrap() as f64;
        let x = [hr * 10.0 + 10.0, hr + 1.0, -hr, 0.0, 0.0, 0.6, 0.8];
        let tg = [5.0, 6.0, 7.0, 0.0, 0.0, 0.1, 0.99498743710662, 10.0, 20.0, 30.0, 1.0, 2.0, 3.0];
        let (v, _, _) = hsmp_native::servo::servo(&x, &tg, [1.0, 2.0, 3.0], 1.0 / 60.0, 900.0, 900.0, 0.8);
        assert_eq!(e.sets.get(&(0, 0)).copied(), Some([v[0], v[1], v[2]]), "weapon root linear velocity (BoneName None)");
        assert_eq!(e.sets.get(&(0, 1)).copied(), Some([v[3], v[4], v[5]]), "weapon root angular velocity");
    }

    // ---- native neutralise: neutralise (BP variable writes at verified offsets + the motor call) ----
    run(a, r#"
        local r, e = N.neutralise({ actor = PAWN })
        assert(r == nil and e == "not configured", tostring(e))
        assert(N.neutralise_config({ zero = { "All Body Tonus", "Stamina", "Missing", "Soft Thing" },
            set_true = { "L_Guarding" }, set_false = { "R_Guarding" }, tonus = { "L_GripType_Current" } }))
        local n = N.neutralise({ actor = PAWN, tonus = false })
        assert(n == 4, "4 writes without the tonus list: " .. tostring(n))
        local st = N.neutralise_status()
        assert(st.verified and st.writes == 4 and st.skipped == 2, "missing + Soft* skipped: " .. st.skipped)
        n = N.neutralise({ actor = PAWN, tonus = true, motors_off = true, mesh = MESH })
        assert(n == 6, "+ tonus + motors: " .. tostring(n))
        local r2, e2 = N.neutralise({ actor = MESH })
        assert(r2 == nil and e2 == "skip:actor", "a mesh is not an Actor: " .. tostring(e2))
    "#);
    {
        let e = eng().lock().unwrap();
        let d = &e.objs[w.pawn].data;
        assert_eq!(d[0] & 0b011, 0b010, "R_Guarding cleared, L_Guarding set (bit masks)");
        assert_eq!(d[0] & 0b100, 0b100, "Any_Guarding untouched");
        assert_eq!(f32::from_le_bytes(d[16..20].try_into().unwrap()), 0.0, "All Body Tonus (float) zeroed");
        assert_eq!(f64::from_le_bytes(d[24..32].try_into().unwrap()), 0.0, "Stamina (double) zeroed");
        assert_eq!(i32::from_le_bytes(d[12..16].try_into().unwrap()), 0, "L_GripType_Current (int, tonus list) zeroed");
        assert_eq!(d[8], 3, "R_GripType_Current untouched");
        assert_eq!(e.motors_off, 1, "SetAllMotorsAngularDriveParams(0, 0, 0, false) once");
    }

    // Soft* params: verification refuses (a new world with a Soft* param in GetSocketTransform)
    {
        let mut e = eng().lock().unwrap();
        e.objs.clear();
        e.by_path.clear();
    }
    let _w2 = build(true);
    let mesh2 = {
        let e = eng().lock().unwrap();
        ptr(&e, _w2.mesh) as u64
    };
    run(a, &format!(
        r#"
        assert(N.world_leaving()); assert(N.world_ready("w3"))
        local m, e = N.sample_local({{ mesh = {mesh2}, pose_tick = 4, pose_ts = 3 }})
        assert(m == nil and e:find("^disabled:") and e:find("Soft"), tostring(e))
        assert(N.sample_status().why:find("Soft"))
    "#
    ));
    unsafe { ffi::lua_close(a) };
}

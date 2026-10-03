//! Native neutralise: `neutralise_bp` in native code (`NATIVE_SERVO`). Every driven stand-in frame,
//! HSMPAvatars zeroes ~29 of the stand-in Willie BP's own variables ("Head Tonus", the root
//! constraint powers, the "Pain *" reactions, the muscle tone), sets the "Block * Muscles" flags
//! and switches the skeletal mesh's joint motors off: ~30 reflected property writes / calls.
//! Here they are writes at verified property offsets (one Lua -> native call) plus one
//! `SetAllMotorsAngularDriveParams` through `ProcessEvent`.
//!
//! Rules (as for native sampling and servo): the actor and mesh come from Lua per call, proven live (weak round
//! trip) and of the right class (`IsA`); property descriptors are looked up by their exact BP
//! names (spaces included) per class, per world, and only Float / Double / Int / Byte / Bool
//! properties are written (anything else, Soft* included, is skipped and counted); the
//! function's params are verified once per world. Lua keeps the originals it restores on
//! release (`p.tonus0`), the grips and the PhysicalAnimation strength.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};

use crate::lua::*;
use crate::native::Native;
use crate::reflect::{self, wide, HsmpProp, HsmpReflect};
use crate::sample::{call_fn, live, verify_fn, FnSpec, Params, Ty, PARAMS_BYTES};

const SET_MOTORS: FnSpec = FnSpec {
    path: "/Script/Engine.SkeletalMeshComponent:SetAllMotorsAngularDriveParams",
    params: &[("InSpring", Ty::Float), ("InDamping", Ty::Float), ("InForceLimit", Ty::Float), ("bSkipCustomPhysicsType", Ty::Bool)],
};

#[derive(Default, Clone)]
struct NeutCfg {
    zero: Vec<String>,
    set_true: Vec<String>,
    set_false: Vec<String>,
    tonus: Vec<String>,
}

struct NWorld {
    motors: (u64, [i32; 4], i32),
    actor_cls: u64,
    mesh_cls: u64,
    /// class (weak) -> descriptors in config order (zero, set_true, set_false, tonus).
    props: HashMap<u64, Vec<Option<HsmpProp>>>,
}

/// Native neutralise state (process-global, inside `SampleState`).
pub struct NeutState {
    cfg: Option<NeutCfg>,
    world: Option<NWorld>,
    why: Option<String>,
    params: Box<Params>,
    pub(crate) calls: u64,
    pub(crate) writes: u64,
    pub(crate) skipped: u64,
    pub(crate) ns_total: u64,
}

impl Default for NeutState {
    fn default() -> Self {
        NeutState { cfg: None, world: None, why: None, params: Box::new(Params([0; PARAMS_BYTES])), calls: 0, writes: 0, skipped: 0, ns_total: 0 }
    }
}

impl NeutState {
    pub fn drop_world(&mut self) {
        self.world = None;
        self.why = None;
    }
}

/// What a property write does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Zero,
    True,
    False,
}

impl Native {
    fn neut_verify(&mut self, vt: &HsmpReflect) -> Result<(), String> {
        if self.sample.neut.world.is_some() {
            return Ok(());
        }
        if let Some(w) = &self.sample.neut.why {
            return Err(w.clone());
        }
        let n = self.sample_names(vt);
        let res = (|| -> Result<NWorld, String> {
            // SAFETY: game thread, inside a Lua call; pointers from StaticFindObject.
            unsafe {
                let motors = verify_fn(vt, &n, &SET_MOTORS)?;
                let actor = (vt.find)(wide("/Script/Engine.Actor").as_ptr());
                let mesh = (vt.find)(wide("/Script/Engine.SkeletalMeshComponent").as_ptr());
                if actor.is_null() || mesh.is_null() {
                    return Err("missing /Script/Engine.Actor or SkeletalMeshComponent".into());
                }
                let actor_cls = crate::sample::keep(vt, actor, "/Script/Engine.Actor")?;
                let mesh_cls = crate::sample::keep(vt, mesh, "/Script/Engine.SkeletalMeshComponent")?;
                Ok(NWorld { motors, actor_cls, mesh_cls, props: HashMap::new() })
            }
        })();
        match res {
            Ok(w) => {
                self.sample.neut.world = Some(w);
                Ok(())
            }
            Err(e) => {
                self.sample.neut.why = Some(e.clone());
                Err(e)
            }
        }
    }

    /// `neutralise_config({zero = {...}, set_true = {...}, set_false = {...}, tonus = {...}})`
    /// -> true | nil, err. The stand-in BP variable names (exact, spaces included).
    pub unsafe fn neutralise_config(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) {
                return nil_err(L, "bad");
            }
            let t = lua_absindex(L, 1);
            let list = |key: &str| -> Option<Vec<String>> {
                let ty = rawget_str(L, t, key);
                if ty == LUA_TNIL {
                    pop(L, 1);
                    return Some(Vec::new());
                }
                if ty != LUA_TTABLE {
                    pop(L, 1);
                    return None;
                }
                let a = lua_gettop(L);
                let mut v = Vec::new();
                for i in 1..=lua_rawlen(L, a) as i64 {
                    lua_rawgeti(L, a, i);
                    let s = arg_str(L, -1).map(str::to_string);
                    pop(L, 1);
                    match s {
                        Some(s) => v.push(s),
                        None => {
                            pop(L, 1);
                            return None;
                        }
                    }
                }
                pop(L, 1);
                Some(v)
            };
            let (Some(zero), Some(set_true), Some(set_false), Some(tonus)) = (list("zero"), list("set_true"), list("set_false"), list("tonus")) else {
                return nil_err(L, "bad");
            };
            if zero.len() + set_true.len() + set_false.len() + tonus.len() > 128 {
                return nil_err(L, "bad");
            }
            self.sample.neut.cfg = Some(NeutCfg { zero, set_true, set_false, tonus });
            self.sample.neut.world = None;
            self.sample.neut.why = None;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// The descriptors of `actor`'s class (cached per class, per world); returns the key.
    unsafe fn neut_props(&mut self, vt: &HsmpReflect, actor: *mut c_void) -> Option<u64> {
        // SAFETY: actor proven live by the caller.
        let key = unsafe { crate::sample::class_key(vt, actor) }?;
        if self.sample.neut.world.as_ref()?.props.contains_key(&key) {
            return Some(key);
        }
        let cfg = self.sample.neut.cfg.clone()?;
        let mut v = Vec::new();
        for name in cfg.zero.iter().chain(&cfg.set_true).chain(&cfg.set_false).chain(&cfg.tonus) {
            let mut p = HsmpProp::default();
            let w = wide(name);
            // SAFETY: live actor; NUL-terminated name.
            let found = unsafe { (vt.obj_prop)(actor, w.as_ptr(), &mut p) } == 1;
            v.push((found && p.offset >= 0 && p.size > 0).then_some(p));
        }
        self.sample.neut.world.as_mut()?.props.insert(key, v);
        Some(key)
    }

    /// `neutralise(a)` -> writes | nil, err. `a`: `actor` (GetAddress integer), `tonus` (bool:
    /// also zero the tonus list), `mesh` (GetAddress integer, optional) + `motors_off` (bool:
    /// `SetAllMotorsAngularDriveParams(0, 0, 0, false)` on it).
    pub unsafe fn neutralise(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) {
                return nil_err(L, "bad");
            }
            let a = lua_absindex(L, 1);
            let Some(vt) = reflect::vt() else { return nil_err(L, "unavailable") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if !self.sample.world_ok {
                return nil_err(L, "world");
            }
            if self.sample.neut.cfg.is_none() {
                return nil_err(L, "not configured");
            }
            if let Err(e) = self.neut_verify(vt) {
                return nil_err(L, &format!("disabled:{}", e));
            }
            let t0 = hsmp_ipc::shm::qpc();
            let addr = |k: &str| -> *mut c_void {
                let ty = rawget_str(L, a, k);
                let v = if ty == LUA_TNUMBER { lua_tointegerx(L, -1, std::ptr::null_mut()) } else { 0 };
                pop(L, 1);
                v as usize as *mut c_void
            };
            let flag = |k: &str| -> bool {
                rawget_str(L, a, k);
                let v = lua_toboolean(L, -1) != 0;
                pop(L, 1);
                v
            };
            let (actor_cls, mesh_cls, motors) = {
                let w = self.sample.neut.world.as_ref().expect("verified");
                (w.actor_cls, w.mesh_cls, w.motors)
            };
            let Some(actor) = live(vt, addr("actor"), actor_cls) else { return nil_err(L, "skip:actor") };
            let tonus = flag("tonus");
            let Some(key) = self.neut_props(vt, actor) else { return nil_err(L, "skip:actor") };
            let n = self.sample_names(vt);
            let cfg_lens = self.sample.neut.cfg.as_ref().map_or((0, 0, 0, 0), |c| (c.zero.len(), c.set_true.len(), c.set_false.len(), c.tonus.len()));
            let (nz, nt, nf, nton) = cfg_lens;
            let mut writes = 0u64;
            let mut skipped = 0u64;
            if let Some(props) = self.sample.neut.world.as_ref().and_then(|w| w.props.get(&key)) {
                let base = actor as *mut u8;
                for (k, p) in props.iter().enumerate() {
                    let op = if k < nz {
                        Op::Zero
                    } else if k < nz + nt {
                        Op::True
                    } else if k < nz + nt + nf {
                        Op::False
                    } else if tonus && k < nz + nt + nf + nton {
                        Op::Zero
                    } else {
                        continue;
                    };
                    let Some(p) = p else {
                        skipped += 1;
                        continue;
                    };
                    // SAFETY: offset / size are the engine's own descriptor of this live
                    // actor's class; only plain value types are written.
                    let at = base.add(p.offset as usize);
                    let ok = match (op, p.cls) {
                        (Op::Zero, c) if c == n.float_prop && p.size == 4 => {
                            std::ptr::write_unaligned(at as *mut f32, 0.0);
                            true
                        }
                        (Op::Zero, c) if c == n.double_prop && p.size == 8 => {
                            std::ptr::write_unaligned(at as *mut f64, 0.0);
                            true
                        }
                        (Op::Zero, c) if (c == n.int_prop || c == n.uint32_prop) && p.size == 4 => {
                            std::ptr::write_unaligned(at as *mut i32, 0);
                            true
                        }
                        (Op::Zero, c) if (c == n.byte_prop || c == n.enum_prop) && p.size == 1 => {
                            *at = 0;
                            true
                        }
                        (Op::Zero | Op::False | Op::True, c) if c == n.bool_prop => {
                            let m = if p.bool_mask == 0 { 0xff } else { p.bool_mask };
                            let b = at.add(p.bool_offset as usize);
                            if op == Op::True {
                                *b |= m;
                            } else {
                                *b &= !m;
                            }
                            true
                        }
                        _ => false,
                    };
                    if ok {
                        writes += 1;
                    } else {
                        skipped += 1;
                    }
                }
            }
            if flag("motors_off") {
                if let Some(mesh) = live(vt, addr("mesh"), mesh_cls) {
                    let ok = call_fn(vt, &mut self.sample.neut.params, motors, mesh, |b, o| {
                        for k in 0..3 {
                            b[o[k] as usize..o[k] as usize + 4].copy_from_slice(&0f32.to_le_bytes());
                        }
                        b[o[3] as usize] = 0;
                    });
                    if ok.is_some() {
                        writes += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    skipped += 1;
                }
            }
            let s = &mut self.sample.neut;
            s.calls += 1;
            s.writes += writes;
            s.skipped += skipped;
            s.ns_total += (hsmp_ipc::shm::qpc().saturating_sub(t0) as u128 * 1_000_000_000 / self.qpc_freq.max(1) as u128) as u64;
            lua_pushinteger(L, writes as i64);
            1
        }
    }

    /// `neutralise_status()` -> {available, configured, verified, why, calls, writes, skipped, avg_us}.
    pub unsafe fn neutralise_status(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            lua_createtable(L, 0, 8);
            let t = lua_gettop(L);
            let s = &self.sample.neut;
            set_bool(L, t, "available", reflect::vt().is_some());
            set_bool(L, t, "configured", s.cfg.is_some());
            set_bool(L, t, "verified", s.world.is_some());
            match &s.why {
                Some(w) => set_str(L, t, "why", w),
                None => set_nil(L, t, "why"),
            }
            set_int(L, t, "calls", s.calls as i64);
            set_int(L, t, "writes", s.writes as i64);
            set_int(L, t, "skipped", s.skipped as i64);
            set_num(L, t, "avg_us", if s.calls > 0 { s.ns_total as f64 / s.calls as f64 / 1000.0 } else { 0.0 });
            1
        }
    }
}

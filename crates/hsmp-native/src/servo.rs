//! Native stand-in body servo (`NATIVE_SERVO`): only the
//! per-body loop of HSMPAvatars' `drive_v2` moves here. For every driven body it reads the
//! current transform (`GetSocketTransform`), runs the velocity servo (a port of Lua's
//! `PURE.servo`, same operations in the same order) and sets the body's linear / angular
//! velocity (`SetPhysicsLinearVelocity` / `SetPhysicsAngularVelocityInDegrees`): 3 reflected
//! calls per body, ~66 per stand-in frame, become one Lua -> native call. Lua keeps
//! everything else: the targets (playback clock, advance / aim, retarget, planting), the
//! gains / caps / yield policy, and every metric (it gets the current transforms, the
//! commanded velocities and the correction sizes back in reused flat arrays).
//!
//! Same binding rules as native sampling (sample.rs header): the mesh comes from Lua per call, proven live and
//! a PrimitiveComponent; the three functions are verified once per world (Soft* refused) and
//! kept as weak handles; everything is dropped untouched at a world leave.

use std::ffi::{c_int, c_void};

use crate::lua::*;
use crate::native::Native;
use crate::reflect::{self, wide, HsmpReflect};
use crate::sample::{call_fn, fname, live, rd_f64, verify_fn, verify_structs, FnSpec, Params, Ty, PARAMS_BYTES};

/// Bodies of a v2 stand-in (`PURE.V2_NB`).
pub const NB: usize = 23;

const SFNS: [FnSpec; 4] = [
    FnSpec {
        path: "/Script/Engine.SceneComponent:GetSocketTransform",
        params: &[("InSocketName", Ty::Name), ("TransformSpace", Ty::Enum), ("ReturnValue", Ty::Transform)],
    },
    FnSpec {
        path: "/Script/Engine.PrimitiveComponent:SetPhysicsLinearVelocity",
        params: &[("NewVel", Ty::Vector), ("bAddToCurrent", Ty::Bool), ("BoneName", Ty::Name)],
    },
    FnSpec {
        path: "/Script/Engine.PrimitiveComponent:SetPhysicsAngularVelocityInDegrees",
        params: &[("NewAngVel", Ty::Vector), ("bAddToCurrent", Ty::Bool), ("BoneName", Ty::Name)],
    },
    // Held-weapon servo: the weapon actor's transform
    FnSpec { path: "/Script/Engine.Actor:GetTransform", params: &[("ReturnValue", Ty::Transform)] },
];

struct SWorld {
    fns: [(u64, [i32; 4], i32); 4],
    prim: u64,
    actor: u64,
    bones: Vec<u64>,
}

/// Native servo state (process-global, inside `SampleState`).
pub struct ServoState {
    bones: Option<Vec<String>>,
    world: Option<SWorld>,
    why: Option<String>,
    params: Box<Params>,
    pub(crate) frames: u64,
    pub(crate) bodies: u64,
    pub(crate) weapons: u64,
    pub(crate) ns_total: u64,
}

impl Default for ServoState {
    fn default() -> Self {
        ServoState { bones: None, world: None, why: None, params: Box::new(Params([0; PARAMS_BYTES])), frames: 0, bodies: 0, weapons: 0, ns_total: 0 }
    }
}

impl ServoState {
    pub fn drop_world(&mut self) {
        self.world = None;
        self.why = None;
    }
}

// ---- the servo math: Lua's PURE.qrot / qmul / qconj / servo, operation for operation -------------

#[inline]
pub fn qrot(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (tx, ty, tz) = (2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0]));
    [v[0] + w * tx + (y * tz - z * ty), v[1] + w * ty + (z * tx - x * tz), v[2] + w * tz + (x * ty - y * tx)]
}

#[inline]
pub fn qmul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

#[inline]
pub fn qconj(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// `PURE.servo(cur, tg, com, dt, cap_lin, cap_ang, gain)`: (vx, vy, vz, wx, wy, wz), dl, gl.
pub fn servo(cur: &[f64; 7], tg: &[f64; 13], com: [f64; 3], dt: f64, cap_lin: f64, cap_ang: f64, gain: f64) -> ([f64; 6], f64, f64) {
    let cq = [cur[3], cur[4], cur[5], cur[6]];
    let tq = [tg[3], tg[4], tg[5], tg[6]];
    let cc = qrot(cq, com);
    let tc = qrot(tq, com);
    let mut vx = (tg[0] + tc[0] - cur[0] - cc[0]) / dt;
    let mut vy = (tg[1] + tc[1] - cur[1] - cc[1]) / dt;
    let mut vz = (tg[2] + tc[2] - cur[2] - cc[2]) / dt;
    let mut e = qmul(tq, qconj(cq));
    if e[3] < 0.0 {
        e = [-e[0], -e[1], -e[2], -e[3]];
    }
    let s = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
    let (mut wx, mut wy, mut wz) = (0.0, 0.0, 0.0);
    if s > 1e-9 {
        let k = (2.0 * s.atan2(e[3])) * (180.0 / std::f64::consts::PI) / s / dt;
        wx = e[0] * k;
        wy = e[1] * k;
        wz = e[2] * k;
    }
    let wr = std::f64::consts::PI / 180.0;
    let (ax, ay, az) = (tg[10] * wr, tg[11] * wr, tg[12] * wr);
    let fx = tg[7] + ay * tc[2] - az * tc[1];
    let fy = tg[8] + az * tc[0] - ax * tc[2];
    let fz = tg[9] + ax * tc[1] - ay * tc[0];
    let (dx, dy, dz) = ((vx - fx) * gain, (vy - fy) * gain, (vz - fz) * gain);
    vx = fx + dx;
    vy = fy + dy;
    vz = fz + dz;
    let dl = (dx * dx + dy * dy + dz * dz).sqrt();
    if dl > cap_lin {
        let kk = cap_lin / dl;
        vx = fx + dx * kk;
        vy = fy + dy * kk;
        vz = fz + dz * kk;
    }
    let (gx, gy, gz) = ((wx - tg[10]) * gain, (wy - tg[11]) * gain, (wz - tg[12]) * gain);
    wx = tg[10] + gx;
    wy = tg[11] + gy;
    wz = tg[12] + gz;
    let gl = (gx * gx + gy * gy + gz * gz).sqrt();
    if gl > cap_ang {
        let kk = cap_ang / gl;
        wx = tg[10] + gx * kk;
        wy = tg[11] + gy * kk;
        wz = tg[12] + gz * kk;
    }
    ([vx, vy, vz, wx, wy, wz], dl, gl)
}

/// Read `t[1..=n]` (absolute index `t`) into `out`: the first `need` must be numbers, the rest
/// default to 0 (Lua's `tg[k] or 0`).
unsafe fn read_arr(L: *mut lua_State, t: c_int, out: &mut [f64], need: usize) -> bool {
    unsafe {
        // Fast path: every element a number (one push batch, one pop).
        if geti_nums(L, t, 1, out) {
            return true;
        }
        for (k, o) in out.iter_mut().enumerate() {
            match geti_num(L, t, k as i64 + 1) {
                Some(v) => *o = v,
                None if k >= need => *o = 0.0,
                None => return false,
            }
        }
        true
    }
}

impl Native {
    fn servo_verify(&mut self, vt: &HsmpReflect) -> Result<(), String> {
        if self.sample.servo.world.is_some() {
            return Ok(());
        }
        if let Some(w) = &self.sample.servo.why {
            return Err(w.clone());
        }
        let n = self.sample_names(vt);
        let Some(bones) = self.sample.servo.bones.clone() else { return Err("not configured".into()) };
        let res = (|| -> Result<SWorld, String> {
            // SAFETY: game thread, inside a Lua call; pointers from StaticFindObject.
            unsafe {
                verify_structs(vt, &n)?;
                let mut fns = [(0u64, [0i32; 4], 0i32); 4];
                for (i, spec) in SFNS.iter().enumerate() {
                    fns[i] = verify_fn(vt, &n, spec)?;
                }
                let prim = (vt.find)(wide("/Script/Engine.PrimitiveComponent").as_ptr());
                if prim.is_null() {
                    return Err("missing /Script/Engine.PrimitiveComponent".into());
                }
                let actor = (vt.find)(wide("/Script/Engine.Actor").as_ptr());
                if actor.is_null() {
                    return Err("missing /Script/Engine.Actor".into());
                }
                let mut ids = Vec::with_capacity(bones.len());
                for b in &bones {
                    let id = fname(vt, b, true);
                    if id == 0 {
                        return Err(format!("bone name {}", b));
                    }
                    ids.push(id);
                }
                let prim = crate::sample::keep(vt, prim, "/Script/Engine.PrimitiveComponent")?;
                let actor = crate::sample::keep(vt, actor, "/Script/Engine.Actor")?;
                Ok(SWorld { fns, prim, actor, bones: ids })
            }
        })();
        match res {
            Ok(w) => {
                self.sample.servo.world = Some(w);
                Ok(())
            }
            Err(e) => {
                self.sample.servo.why = Some(e.clone());
                Err(e)
            }
        }
    }

    /// `servo_config({bones = {23 names}})` -> true | nil, err (the v2 slot bone names).
    pub unsafe fn servo_config(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) || rawget_str(L, 1, "bones") != LUA_TTABLE {
                lua_settop(L, 1);
                return nil_err(L, "bad");
            }
            let t = lua_gettop(L);
            let mut v = Vec::new();
            for i in 1..=lua_rawlen(L, t) as i64 {
                lua_rawgeti(L, t, i);
                let s = arg_str(L, -1).map(str::to_string);
                pop(L, 1);
                match s {
                    Some(s) => v.push(s),
                    None => return nil_err(L, "bad"),
                }
            }
            if v.len() != NB {
                return nil_err(L, "bad");
            }
            self.sample.servo.bones = Some(v);
            self.sample.servo.world = None;
            self.sample.servo.why = None;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// `servo_bodies(a, out)` -> `n` (bodies driven) | `nil, err`.
    ///
    /// `a`: `mesh` (GetAddress integer), `dt` (s), `cap_lin`, `cap_ang`, `gain`, `leg_gain`
    /// (nil = `gain`), `leg_from` (slot, default 18), `holding` (bool: no feed-forward),
    /// `aim` (slot -> 13 numbers), `com` (slot -> 3 numbers). A body is driven when it has both.
    /// `out` (reused): `c` (flat, 7 per slot: position + quaternion before the step), `v`
    /// (flat, 3 per slot: the linear velocity set; the angular one is only set), `dl`, `gl` (per slot: correction sizes).
    /// Errors: `unavailable`, `world`, `not configured`, `disabled:<why>`, `skip:mesh`,
    /// `skip:call` (a function vanished mid-frame; Lua redoes the frame).
    pub unsafe fn servo_bodies(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) || !is_table(L, 2) {
                return nil_err(L, "bad");
            }
            let (a, out) = (lua_absindex(L, 1), lua_absindex(L, 2));
            let Some(vt) = reflect::vt() else { return nil_err(L, "unavailable") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if !self.sample.world_ok {
                return nil_err(L, "world");
            }
            if self.sample.servo.bones.is_none() {
                return nil_err(L, "not configured");
            }
            if let Err(e) = self.servo_verify(vt) {
                return nil_err(L, &format!("disabled:{}", e));
            }
            let t0 = hsmp_ipc::shm::qpc();
            let num = |k: &str, d: f64| -> f64 {
                let ty = rawget_str(L, a, k);
                let v = if ty == LUA_TNUMBER { lua_tonumberx(L, -1, std::ptr::null_mut()) } else { d };
                pop(L, 1);
                v
            };
            let mesh = {
                let ty = rawget_str(L, a, "mesh");
                let v = if ty == LUA_TNUMBER { lua_tointegerx(L, -1, std::ptr::null_mut()) } else { 0 };
                pop(L, 1);
                v as usize as *mut c_void
            };
            let (dt, cap_lin, cap_ang, gain) = (num("dt", 1.0 / 60.0), num("cap_lin", 900.0), num("cap_ang", 900.0), num("gain", 1.0));
            let leg_gain = num("leg_gain", gain);
            let leg_from = num("leg_from", 18.0) as usize;
            rawget_str(L, a, "holding");
            let holding = lua_toboolean(L, -1) != 0;
            pop(L, 1);
            if !(dt > 0.0 && dt.is_finite()) {
                return nil_err(L, "bad");
            }
            let (fns, prim, nb, bones) = {
                let w = self.sample.servo.world.as_ref().expect("verified");
                let mut b = [0u64; NB];
                let nb = w.bones.len().min(NB);
                b[..nb].copy_from_slice(&w.bones[..nb]);
                (w.fns, w.prim, nb, b)
            };
            let Some(mesh) = live(vt, mesh, prim) else { return nil_err(L, "skip:mesh") };
            let (ta, tc) = (rawget_str(L, a, "aim"), rawget_str(L, a, "com"));
            if ta != LUA_TTABLE || tc != LUA_TTABLE {
                lua_settop(L, 2);
                return nil_err(L, "bad");
            }
            let (aim_t, com_t) = (lua_gettop(L) - 1, lua_gettop(L));
            for (key, narr) in [("c", 7 * NB), ("v", 3 * NB), ("dl", NB), ("gl", NB)] {
                subtable(L, out, key, narr as c_int, 0);
            }
            let top = lua_gettop(L);
            let (oc, ov, odl, ogl) = (top - 3, top - 2, top - 1, top);
            let mut driven = 0i64;
            let mut failed = false;
            for i in 0..nb {
                let slot = i as i64 + 1;
                let mut com = [0.0f64; 3];
                let mut tg = [0.0f64; 13];
                let have_com = lua_rawgeti(L, com_t, slot) == LUA_TTABLE && read_arr(L, lua_gettop(L), &mut com, 3);
                pop(L, 1);
                let have_tg = lua_rawgeti(L, aim_t, slot) == LUA_TTABLE && read_arr(L, lua_gettop(L), &mut tg, 7);
                pop(L, 1);
                if !(have_com && have_tg) {
                    continue;
                }
                if holding {
                    for x in &mut tg[7..13] {
                        *x = 0.0;
                    }
                }
                let name = bones[i].to_le_bytes();
                let params = &mut self.sample.servo.params;
                let Some(o) = call_fn(vt, params, fns[0], mesh, |b, o| b[o[0] as usize..o[0] as usize + 8].copy_from_slice(&name)) else {
                    failed = true;
                    break;
                };
                let r = o[2] as usize;
                let pb = &params.0;
                let cur = [rd_f64(pb, r + 32), rd_f64(pb, r + 40), rd_f64(pb, r + 48), rd_f64(pb, r), rd_f64(pb, r + 8), rd_f64(pb, r + 16), rd_f64(pb, r + 24)];
                let g = if i + 1 >= leg_from { leg_gain } else { gain };
                let (v, dl, gl) = servo(&cur, &tg, com, dt, cap_lin, cap_ang, g);
                for (fi, vals) in [(1usize, [v[0], v[1], v[2]]), (2usize, [v[3], v[4], v[5]])] {
                    let ok = call_fn(vt, params, fns[fi], mesh, |b, o| {
                        for (j, x) in vals.iter().enumerate() {
                            let at = o[0] as usize + 8 * j;
                            b[at..at + 8].copy_from_slice(&x.to_le_bytes());
                        }
                        b[o[2] as usize..o[2] as usize + 8].copy_from_slice(&name);
                    });
                    if ok.is_none() {
                        failed = true;
                    }
                }
                if failed {
                    break;
                }
                for (j, x) in cur.iter().enumerate() {
                    lua_pushnumber(L, *x);
                    lua_rawseti(L, oc, (i * 7 + j) as i64 + 1);
                }
                for (j, x) in v[..3].iter().enumerate() {
                    lua_pushnumber(L, *x);
                    lua_rawseti(L, ov, (i * 3 + j) as i64 + 1);
                }
                lua_pushnumber(L, dl);
                lua_rawseti(L, odl, slot);
                lua_pushnumber(L, gl);
                lua_rawseti(L, ogl, slot);
                driven += 1;
            }
            lua_settop(L, 2);
            let s = &mut self.sample.servo;
            s.frames += 1;
            s.bodies += driven as u64;
            s.ns_total += (hsmp_ipc::shm::qpc().saturating_sub(t0) as u128 * 1_000_000_000 / self.qpc_freq.max(1) as u128) as u64;
            if failed {
                return nil_err(L, "skip:call");
            }
            lua_pushinteger(L, driven);
            1
        }
    }

    /// Held-weapon servo: `servo_weapon(a, out)` -> true | nil, err. One held weapon of a stand-in:
    /// `GetTransform` of the weapon actor, the servo (`PURE.servo`), the two velocity sets on its
    /// root component (BoneName None). `a`: `actor`, `root` (GetAddress integers, validated by
    /// Lua this frame), `aim` (13 numbers), `com` (3), `dt`, `cap_lin`, `cap_ang`, `gain`,
    /// `holding`. `out` (reused): `x` (7: the weapon's transform before the step), `v` (3).
    pub unsafe fn servo_weapon(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) || !is_table(L, 2) {
                return nil_err(L, "bad");
            }
            let (a, out) = (lua_absindex(L, 1), lua_absindex(L, 2));
            let Some(vt) = reflect::vt() else { return nil_err(L, "unavailable") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if !self.sample.world_ok {
                return nil_err(L, "world");
            }
            if self.sample.servo.bones.is_none() {
                return nil_err(L, "not configured");
            }
            if let Err(e) = self.servo_verify(vt) {
                return nil_err(L, &format!("disabled:{}", e));
            }
            let num = |k: &str, d: f64| -> f64 {
                let ty = rawget_str(L, a, k);
                let v = if ty == LUA_TNUMBER { lua_tonumberx(L, -1, std::ptr::null_mut()) } else { d };
                pop(L, 1);
                v
            };
            let addr = |k: &str| -> *mut c_void {
                let ty = rawget_str(L, a, k);
                let v = if ty == LUA_TNUMBER { lua_tointegerx(L, -1, std::ptr::null_mut()) } else { 0 };
                pop(L, 1);
                v as usize as *mut c_void
            };
            let (dt, cap_lin, cap_ang, gain) = (num("dt", 1.0 / 60.0), num("cap_lin", 900.0), num("cap_ang", 900.0), num("gain", 1.0));
            rawget_str(L, a, "holding");
            let holding = lua_toboolean(L, -1) != 0;
            pop(L, 1);
            if !(dt > 0.0 && dt.is_finite()) {
                return nil_err(L, "bad");
            }
            let (fns, prim, actor_cls) = {
                let w = self.sample.servo.world.as_ref().expect("verified");
                (w.fns, w.prim, w.actor)
            };
            let Some(actor) = live(vt, addr("actor"), actor_cls) else { return nil_err(L, "skip:actor") };
            let Some(root) = live(vt, addr("root"), prim) else { return nil_err(L, "skip:root") };
            let mut tg = [0.0f64; 13];
            let mut com = [0.0f64; 3];
            let ok_tg = rawget_str(L, a, "aim") == LUA_TTABLE && read_arr(L, lua_gettop(L), &mut tg, 7);
            pop(L, 1);
            let ok_com = rawget_str(L, a, "com") == LUA_TTABLE && read_arr(L, lua_gettop(L), &mut com, 3);
            pop(L, 1);
            if !(ok_tg && ok_com) {
                return nil_err(L, "bad");
            }
            if holding {
                for x in &mut tg[7..13] {
                    *x = 0.0;
                }
            }
            let params = &mut self.sample.servo.params;
            let Some(o) = call_fn(vt, params, fns[3], actor, |_, _| {}) else { return nil_err(L, "skip:call") };
            let r = o[0] as usize;
            let pb = &params.0;
            let x = [rd_f64(pb, r + 32), rd_f64(pb, r + 40), rd_f64(pb, r + 48), rd_f64(pb, r), rd_f64(pb, r + 8), rd_f64(pb, r + 16), rd_f64(pb, r + 24)];
            let (v, _, _) = servo(&x, &tg, com, dt, cap_lin, cap_ang, gain);
            for (fi, vals) in [(1usize, [v[0], v[1], v[2]]), (2usize, [v[3], v[4], v[5]])] {
                let ok = call_fn(vt, params, fns[fi], root, |b, o| {
                    for (j, val) in vals.iter().enumerate() {
                        let at = o[0] as usize + 8 * j;
                        b[at..at + 8].copy_from_slice(&val.to_le_bytes());
                    }
                });
                if ok.is_none() {
                    return nil_err(L, "skip:call");
                }
            }
            subtable(L, out, "x", 7, 0);
            let tx = lua_gettop(L);
            fill_array(L, tx, x.iter().copied());
            pop(L, 1);
            subtable(L, out, "v", 3, 0);
            let tv = lua_gettop(L);
            fill_array(L, tv, v[..3].iter().copied());
            pop(L, 1);
            self.sample.servo.weapons += 1;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// `servo_status()` -> {available, configured, verified, why, frames, bodies, avg_us}.
    pub unsafe fn servo_status(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            lua_createtable(L, 0, 8);
            let t = lua_gettop(L);
            let s = &self.sample.servo;
            set_bool(L, t, "available", reflect::vt().is_some());
            set_bool(L, t, "configured", s.bones.is_some());
            set_bool(L, t, "verified", s.world.is_some());
            match &s.why {
                Some(w) => set_str(L, t, "why", w),
                None => set_nil(L, t, "why"),
            }
            set_int(L, t, "frames", s.frames as i64);
            set_int(L, t, "bodies", s.bodies as i64);
            set_int(L, t, "weapons", s.weapons as i64);
            set_num(L, t, "avg_us", if s.frames > 0 { s.ns_total as f64 / s.frames as f64 / 1000.0 } else { 0.0 });
            1
        }
    }
}

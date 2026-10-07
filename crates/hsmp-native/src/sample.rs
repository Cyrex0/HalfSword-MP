//! Native local pose sampling (`NATIVE_SAMPLE`; design: docs/development/ipc-shared-memory.md).
//!
//! `sample_local(a)` does what HSMPSync's Lua sender does with ~70 reflected calls per sample
//! (23 × GetSocketTransform + the physics velocities of every body, the held weapons, the
//! control BP variables, the root and the weapon actor), through `ProcessEvent` and property
//! offsets, and writes the `local_root` / `local_weapon` / `local_pose` records with the pose
//! writers (`pose_hot.rs`: `write_root` / `write_weapon` / `write_pose`), so the
//! records are byte-identical to the Lua path's. Lua keeps the policy: when to sample, which
//! pawn / mesh / weapons (death, world and weapon-class checks), and the fallback to its own
//! path (the A/B switch).
//!
//! Binding rules (shared by native servo and neutralise), and how they are kept:
//! 1. Game thread only: everything runs inside a Lua -> native call (api.rs checks the
//!    thread); there is no timer and no thread.
//! 2. No raw `UObject*` across calls: objects come from Lua each call (as `GetAddress()`
//!    integers, after Lua's own `IsValid`), are proven live through a weak round trip
//!    (index + serial off the object array; never UE4SS's FWeakObjectPtr, whose serial
//!    allocation faults) and checked with `IsA` before any `ProcessEvent`. Functions and
//!    classes are kept as weak handles (pinned to their address when they have no serial,
//!    `reflect::keep`) and resolved per call (`reflect::get`).
//! 3. World epoch: `world_leaving` / a world-key change drop every cache WITHOUT resolving
//!    it; nothing resolves until `world_ready`.
//! 4. Every function's params are verified once per world against its property chain (name,
//!    class, size, struct; offsets come from the engine); a Soft* param, a missing function
//!    or any mismatch disables native sampling with a reason (`sample_status().why`), and
//!    Lua keeps its path. The math structs (Transform / Vector / Rotator / Quat) are verified
//!    the same way.
//! 5. BP property names with spaces are looked up exactly (`GetPropertyByNameInChain`).
//! 6. No SEH swallowing anywhere.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};

use hsmp_pose::sample::{BONE_NUMS, CONTROL_NUMS, WEAPON_NUMS};

use crate::lua::*;
use crate::native::Native;
use crate::reflect::{self, wide, HsmpProp, HsmpReflect};

/// Bones of a pose sample (`BONE_NUMS / 13`).
const NBONES: usize = BONE_NUMS / 13;
pub(crate) const PARAMS_BYTES: usize = 512;
/// Lua's `finite`: a number with |x| < 1e9.
#[inline]
pub(crate) fn finite(x: f64) -> bool {
    x > -1e9 && x < 1e9
}

#[repr(C, align(16))]
pub(crate) struct Params(pub(crate) [u8; PARAMS_BYTES]);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ty {
    Name,
    Float,
    Enum,
    Bool,
    Vector,
    Rotator,
    Transform,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum F {
    SocketTransform = 0,
    LinVelAt,
    AngVel,
    IsSim,
    SocketLoc,
    CompLoc,
    ActorTransform,
    ActorLoc,
    ActorRot,
    Velocity,
}
const NFNS: usize = 10;

pub(crate) struct FnSpec {
    /// The UFunction's path; `a|b` = the first of these that exists.
    pub(crate) path: &'static str,
    pub(crate) params: &'static [(&'static str, Ty)],
}

/// The UFunctions native sampling calls (the same ones the Lua sender calls).
const FNS: [FnSpec; NFNS] = [
    FnSpec {
        path: "/Script/Engine.SceneComponent:GetSocketTransform",
        params: &[("InSocketName", Ty::Name), ("TransformSpace", Ty::Enum), ("ReturnValue", Ty::Transform)],
    },
    FnSpec {
        path: "/Script/Engine.PrimitiveComponent:GetPhysicsLinearVelocityAtPoint",
        params: &[("Point", Ty::Vector), ("BoneName", Ty::Name), ("ReturnValue", Ty::Vector)],
    },
    FnSpec { path: "/Script/Engine.PrimitiveComponent:GetPhysicsAngularVelocityInDegrees", params: &[("BoneName", Ty::Name), ("ReturnValue", Ty::Vector)] },
    // The UFUNCTION is declared on SceneComponent (PrimitiveComponent only overrides the
    // virtual): in-game the PrimitiveComponent path does not exist.
    FnSpec {
        path: "/Script/Engine.SceneComponent:IsSimulatingPhysics|/Script/Engine.PrimitiveComponent:IsSimulatingPhysics",
        params: &[("BoneName", Ty::Name), ("ReturnValue", Ty::Bool)],
    },
    FnSpec { path: "/Script/Engine.SceneComponent:GetSocketLocation", params: &[("InSocketName", Ty::Name), ("ReturnValue", Ty::Vector)] },
    FnSpec { path: "/Script/Engine.SceneComponent:K2_GetComponentLocation", params: &[("ReturnValue", Ty::Vector)] },
    FnSpec { path: "/Script/Engine.Actor:GetTransform", params: &[("ReturnValue", Ty::Transform)] },
    FnSpec { path: "/Script/Engine.Actor:K2_GetActorLocation", params: &[("ReturnValue", Ty::Vector)] },
    FnSpec { path: "/Script/Engine.Actor:K2_GetActorRotation", params: &[("ReturnValue", Ty::Rotator)] },
    FnSpec { path: "/Script/Engine.Actor:GetVelocity", params: &[("ReturnValue", Ty::Vector)] },
];

/// (path, [(field, struct-or-"", offset)], size): the math structs' layouts we read.
const STRUCTS: [(&str, &[(&str, &str, i32)], i32); 4] = [
    ("/Script/CoreUObject.Vector", &[("X", "", 0), ("Y", "", 8), ("Z", "", 16)], 24),
    ("/Script/CoreUObject.Rotator", &[("Pitch", "", 0), ("Yaw", "", 8), ("Roll", "", 16)], 24),
    ("/Script/CoreUObject.Quat", &[("X", "", 0), ("Y", "", 8), ("Z", "", 16), ("W", "", 24)], 32),
    ("/Script/CoreUObject.Transform", &[("Rotation", "Quat", 0), ("Translation", "Vector", 32), ("Scale3D", "Vector", 64)], 96),
];

const CLASSES: [&str; 3] = ["/Script/Engine.SceneComponent", "/Script/Engine.PrimitiveComponent", "/Script/Engine.Actor"];
const C_SCENE: usize = 0;
const C_PRIM: usize = 1;
const C_ACTOR: usize = 2;

/// FNames used by verification and reads (process-global: the name table never shrinks).
#[derive(Default, Clone, Copy)]
pub(crate) struct Names {
    pub(crate) name_prop: u64,
    pub(crate) byte_prop: u64,
    pub(crate) enum_prop: u64,
    pub(crate) bool_prop: u64,
    pub(crate) struct_prop: u64,
    pub(crate) double_prop: u64,
    pub(crate) float_prop: u64,
    pub(crate) int_prop: u64,
    pub(crate) int64_prop: u64,
    pub(crate) uint32_prop: u64,
    pub(crate) object_prop: u64,
    pub(crate) soft_object: u64,
    pub(crate) soft_class: u64,
    pub(crate) vector: u64,
    pub(crate) rotator: u64,
    pub(crate) transform: u64,
}

/// The names Lua configures once (`sample_config`).
#[derive(Default)]
struct Cfg {
    bones: Vec<String>,
    nobody: Vec<bool>,
    flags: Vec<String>,
    scalars: Vec<String>,
    ik: Vec<String>,
    grip_r: String,
    grip_l: String,
    aim: String,
    ctrl_rot: String,
    w_root: String,
    w_base: String,
    w_tip: String,
}

/// Pawn property slots (indices into a per-class `Vec<Option<HsmpProp>>`).
struct PawnIdx {
    flags: usize,
    scalars: usize,
    ik: usize,
    grip_r: usize,
    grip_l: usize,
    aim: usize,
    ctrl_rot: usize,
    n: usize,
}

/// Per-world verified state. Weak handles only; dropped without resolving at world leave.
struct World {
    fns: [(u64, [i32; 4], i32); NFNS],
    classes: [u64; 3],
    bones: Vec<u64>,
    hand_l: u64,
    hand_r: u64,
    pawn_props: HashMap<u64, Vec<Option<HsmpProp>>>,
    weapon_props: HashMap<u64, Vec<Option<HsmpProp>>>,
}

/// Original generation of the last successful pose-slot write.
#[derive(Clone, Copy)]
pub(crate) struct PoseWritten {
    pub tick: u32,
    pub ts: f64,
    pub context: hsmp_pose::posecodec::v2::Context,
}

/// Native sampling state (process-global, in `Native`).
pub struct SampleState {
    cfg: Option<Cfg>,
    names: Option<Names>,
    world: Option<World>,
    /// Nothing resolves between `world_leaving` and `world_ready`.
    pub(crate) world_ok: bool,
    /// Why native sampling is off (verification failure), until the next world.
    why: Option<String>,
    params: Box<Params>,
    bones: Box<[f64; BONE_NUMS]>,
    weapons: [[f64; WEAPON_NUMS]; 2],
    control: [f64; CONTROL_NUMS],
    // stats
    samples: u64,
    pe_calls: u64,
    ns_total: u64,
    ns_last: u64,
    fallbacks: u64,
    /// Last actual successful pose-slot write, including the Lua sampling path.
    pub(crate) pose_written: Option<PoseWritten>,
    /// Native servo (servo.rs).
    pub(crate) servo: crate::servo::ServoState,
    /// Native neutralise (neutralise.rs).
    pub(crate) neut: crate::neutralise::NeutState,
}

impl Default for SampleState {
    fn default() -> Self {
        SampleState {
            cfg: None,
            names: None,
            world: None,
            world_ok: true,
            why: None,
            params: Box::new(Params([0; PARAMS_BYTES])),
            bones: Box::new([0.0; BONE_NUMS]),
            weapons: [[0.0; WEAPON_NUMS]; 2],
            control: [0.0; CONTROL_NUMS],
            samples: 0,
            pe_calls: 0,
            ns_total: 0,
            ns_last: 0,
            fallbacks: 0,
            pose_written: None,
            servo: Default::default(),
            neut: Default::default(),
        }
    }
}

impl SampleState {
    /// World leave / world change: forget every engine handle without touching it.
    pub fn drop_world(&mut self, leaving: bool) {
        self.pose_written = None;
        self.world = None;
        self.why = None;
        self.servo.drop_world();
        self.neut.drop_world();
        if leaving {
            self.world_ok = false;
        }
    }

    /// `world_ready`: resolving is allowed again.
    pub fn world_ready(&mut self) {
        self.world_ok = true;
    }

    fn pawn_idx(&self) -> PawnIdx {
        let c = self.cfg.as_ref();
        let nf = c.map_or(0, |c| c.flags.len());
        let ns = c.map_or(0, |c| c.scalars.len());
        let ni = c.map_or(0, |c| c.ik.len());
        let flags = 0;
        let scalars = flags + nf;
        let ik = scalars + ns;
        let grip_r = ik + ni;
        PawnIdx { flags, scalars, ik, grip_r, grip_l: grip_r + 1, aim: grip_r + 2, ctrl_rot: grip_r + 3, n: grip_r + 4 }
    }
}

// ---- small helpers over the vtable ---------------------------------------------------------------

#[inline]
pub(crate) fn rd_f64(b: &[u8], off: usize) -> f64 {
    f64::from_le_bytes(b[off..off + 8].try_into().unwrap_or([0; 8]))
}

pub(crate) fn vec3(b: &[u8], off: usize) -> [f64; 3] {
    [rd_f64(b, off), rd_f64(b, off + 8), rd_f64(b, off + 16)]
}

pub(crate) unsafe fn fname(vt: &HsmpReflect, s: &str, add: bool) -> u64 {
    let w = wide(s);
    // SAFETY: NUL-terminated wide string; game thread.
    unsafe { (vt.fname)(w.as_ptr(), add as i32) }
}

/// A handle of `obj` to keep across calls (verification; [`reflect::keep`]), or why not.
pub(crate) unsafe fn keep(vt: &HsmpReflect, obj: *mut c_void, what: &str) -> Result<u64, String> {
    // SAFETY: `obj` comes from StaticFindObject in this call.
    unsafe { reflect::keep(vt, obj) }.ok_or_else(|| format!("{}: not in the object array", what))
}

/// The per-world cache key of `obj`'s class (its object index; the class is not kept, only
/// compared), or None.
pub(crate) unsafe fn class_key(vt: &HsmpReflect, obj: *mut c_void) -> Option<u64> {
    // SAFETY: obj proven live by the caller; a live object's class is live.
    unsafe {
        let cls = (vt.class_of)(obj);
        if cls.is_null() {
            return None;
        }
        let w = (vt.weak)(cls);
        (w != 0).then_some(w & 0xffff_ffff)
    }
}

/// `obj` is a live engine object (weak round trip) of class `cls` (weak), or None.
pub(crate) unsafe fn live(vt: &HsmpReflect, obj: *mut c_void, cls: u64) -> Option<*mut c_void> {
    if obj.is_null() {
        return None;
    }
    // SAFETY: `obj` was handed over by Lua in this call (or read from a live object's
    // property); the weak round trip proves it is in GUObjectArray with a matching serial.
    unsafe {
        let w = (vt.weak)(obj);
        if w == 0 || (vt.resolve)(w) != obj {
            return None;
        }
        let c = reflect::get(vt, cls);
        if c.is_null() || (vt.is_a)(obj, c) == 0 {
            return None;
        }
    }
    Some(obj)
}

pub(crate) fn ty_ok(n: &Names, p: &HsmpProp, t: Ty) -> bool {
    match t {
        Ty::Name => p.cls == n.name_prop && p.size == 8,
        Ty::Float => p.cls == n.float_prop && p.size == 4,
        Ty::Enum => (p.cls == n.byte_prop || p.cls == n.enum_prop) && (1..=8).contains(&p.size),
        Ty::Bool => p.cls == n.bool_prop && p.size >= 1,
        Ty::Vector => p.cls == n.struct_prop && p.sub == n.vector && p.size == 24,
        Ty::Rotator => p.cls == n.struct_prop && p.sub == n.rotator && p.size == 24,
        Ty::Transform => p.cls == n.struct_prop && p.sub == n.transform && p.size == 96,
    }
}

pub(crate) unsafe fn props_of(vt: &HsmpReflect, ustruct: *mut c_void) -> (Vec<HsmpProp>, i32) {
    let mut out = vec![HsmpProp::default(); 32];
    let mut size = 0;
    // SAFETY: `ustruct` resolved from a weak handle / StaticFindObject in this call.
    let n = unsafe { (vt.props)(ustruct, out.as_mut_ptr(), out.len() as i32, &mut size) };
    out.truncate((n.max(0) as usize).min(32));
    if n as usize > 32 {
        out.clear();
        size = -1;
    }
    (out, size)
}

/// The math structs' layouts (Transform / Vector / Rotator / Quat), verified field by field.
pub(crate) unsafe fn verify_structs(vt: &HsmpReflect, n: &Names) -> Result<(), String> {
    // SAFETY: game thread; every pointer comes from StaticFindObject in this call.
    unsafe {
        for (path, fields, size) in STRUCTS {
            let s = (vt.find)(wide(path).as_ptr());
            if s.is_null() {
                return Err(format!("missing {}", path));
            }
            let (props, got) = props_of(vt, s);
            if got != size {
                return Err(format!("{} size {} (want {})", path, got, size));
            }
            for (field, sub, off) in fields.iter() {
                let fid = fname(vt, field, true);
                let Some(p) = props.iter().find(|p| p.name == fid) else { return Err(format!("{}.{} missing", path, field)) };
                let ok = if sub.is_empty() { p.cls == n.double_prop && p.size == 8 } else { p.cls == n.struct_prop && p.sub == fname(vt, sub, true) };
                if !ok || p.offset != *off {
                    return Err(format!("{}.{} layout (offset {}, size {})", path, field, p.offset, p.size));
                }
            }
        }
    }
    Ok(())
}

/// One UFunction against its spec: (weak handle, param offsets in spec order, params size).
/// A missing function, a Soft* param, a param count / name / class / size mismatch refuses.
pub(crate) unsafe fn verify_fn(vt: &HsmpReflect, n: &Names, spec: &FnSpec) -> Result<(u64, [i32; 4], i32), String> {
    // SAFETY: game thread; the pointer comes from StaticFindObject in this call.
    unsafe {
        let f = spec.path.split('|').map(|p| (vt.find)(wide(p).as_ptr())).find(|f| !f.is_null()).unwrap_or(std::ptr::null_mut());
        if f.is_null() {
            return Err(format!("missing {}", spec.path));
        }
        let (props, psize) = props_of(vt, f);
        if psize <= 0 || psize as usize > PARAMS_BYTES {
            return Err(format!("{}: params size {}", spec.path, psize));
        }
        if props.iter().any(|p| p.cls == n.soft_object || p.cls == n.soft_class) {
            return Err(format!("{}: Soft* param refused", spec.path));
        }
        if props.len() != spec.params.len() || spec.params.len() > 4 {
            return Err(format!("{}: {} params (want {})", spec.path, props.len(), spec.params.len()));
        }
        let mut offs = [0i32; 4];
        for (k, (pname, ty)) in spec.params.iter().enumerate() {
            let pid = fname(vt, pname, true);
            let Some(p) = props.iter().find(|p| p.name == pid) else { return Err(format!("{}: no param {}", spec.path, pname)) };
            if !ty_ok(n, p, *ty) || p.offset < 0 || p.offset + p.size > psize {
                return Err(format!("{}: param {} (class/size/offset {} {})", spec.path, pname, p.size, p.offset));
            }
            offs[k] = p.offset;
        }
        Ok((keep(vt, f, spec.path)?, offs, psize))
    }
}

/// Call a verified function (`h` from [`verify_fn`]) on `obj` with the params `fill` writes;
/// the params buffer holds the result afterwards. None if the function is gone.
pub(crate) unsafe fn call_fn(vt: &HsmpReflect, params: &mut Params, h: (u64, [i32; 4], i32), obj: *mut c_void, fill: impl FnOnce(&mut [u8], &[i32; 4])) -> Option<[i32; 4]> {
    let (wf, offs, size) = h;
    // SAFETY: a weak handle verified this world; `obj` proven live and of the function's class
    // by the caller.
    unsafe {
        let func = reflect::get(vt, wf);
        if func.is_null() {
            return None;
        }
        let b = &mut params.0[..size as usize];
        b.fill(0);
        fill(b, &offs);
        (vt.call)(obj, func, params.0.as_mut_ptr() as *mut c_void);
    }
    Some(offs)
}

impl Native {
    pub(crate) fn sample_names(&mut self, vt: &HsmpReflect) -> Names {
        if let Some(n) = self.sample.names {
            return n;
        }
        // SAFETY: game thread, inside a Lua call.
        let n = unsafe {
            Names {
                name_prop: fname(vt, "NameProperty", true),
                byte_prop: fname(vt, "ByteProperty", true),
                enum_prop: fname(vt, "EnumProperty", true),
                bool_prop: fname(vt, "BoolProperty", true),
                struct_prop: fname(vt, "StructProperty", true),
                double_prop: fname(vt, "DoubleProperty", true),
                float_prop: fname(vt, "FloatProperty", true),
                int_prop: fname(vt, "IntProperty", true),
                int64_prop: fname(vt, "Int64Property", true),
                uint32_prop: fname(vt, "UInt32Property", true),
                object_prop: fname(vt, "ObjectProperty", true),
                soft_object: fname(vt, "SoftObjectProperty", true),
                soft_class: fname(vt, "SoftClassProperty", true),
                vector: fname(vt, "Vector", true),
                rotator: fname(vt, "Rotator", true),
                transform: fname(vt, "Transform", true),
            }
        };
        self.sample.names = Some(n);
        n
    }

    /// Verify everything once per world. Err = the reason native sampling is off.
    fn sample_verify(&mut self, vt: &HsmpReflect) -> Result<(), String> {
        if self.sample.world.is_some() {
            return Ok(());
        }
        if let Some(w) = &self.sample.why {
            return Err(w.clone());
        }
        let n = self.sample_names(vt);
        let Some(cfg) = self.sample.cfg.as_ref() else { return Err("not configured".into()) };
        let res = (|| -> Result<World, String> {
            // SAFETY: game thread; every pointer comes from StaticFindObject in this call.
            unsafe {
                verify_structs(vt, &n)?;
                let mut fns = [(0u64, [0i32; 4], 0i32); NFNS];
                for (i, spec) in FNS.iter().enumerate() {
                    fns[i] = verify_fn(vt, &n, spec)?;
                }
                let mut classes = [0u64; 3];
                for (i, path) in CLASSES.iter().enumerate() {
                    let c = (vt.find)(wide(path).as_ptr());
                    if c.is_null() {
                        return Err(format!("missing {}", path));
                    }
                    classes[i] = keep(vt, c, path)?;
                }
                let mut bones = Vec::with_capacity(cfg.bones.len());
                for b in &cfg.bones {
                    let id = fname(vt, b, true);
                    if id == 0 {
                        return Err(format!("bone name {}", b));
                    }
                    bones.push(id);
                }
                Ok(World {
                    fns,
                    classes,
                    bones,
                    hand_l: fname(vt, "hand_l", true),
                    hand_r: fname(vt, "hand_r", true),
                    pawn_props: HashMap::new(),
                    weapon_props: HashMap::new(),
                })
            }
        })();
        match res {
            Ok(w) => {
                self.sample.world = Some(w);
                Ok(())
            }
            Err(e) => {
                self.sample.why = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Call function `f` on `obj` with the params `fill` writes (offsets per the spec order);
    /// the params buffer holds the result afterwards. False if the function is gone.
    unsafe fn pe(&mut self, vt: &HsmpReflect, f: F, obj: *mut c_void, fill: impl FnOnce(&mut [u8], &[i32; 4])) -> Option<[i32; 4]> {
        let h = self.sample.world.as_ref()?.fns[f as usize];
        // SAFETY: `obj` was proven live and of the function's class by the caller.
        let offs = unsafe { call_fn(vt, &mut self.sample.params, h, obj, fill) }?;
        self.sample.pe_calls += 1;
        Some(offs)
    }

    fn pbuf(&self) -> &[u8] {
        &self.sample.params.0
    }

    /// FTransform return at `off`: (translation, quaternion xyzw).
    fn transform_at(&self, off: usize) -> ([f64; 3], [f64; 4]) {
        let b = self.pbuf();
        let q = [rd_f64(b, off), rd_f64(b, off + 8), rd_f64(b, off + 16), rd_f64(b, off + 24)];
        (vec3(b, off + 32), q)
    }

    /// One bone into `self.sample.bones[i*13..]`; false = abort the pose (Lua: return).
    unsafe fn sample_bone(&mut self, vt: &HsmpReflect, mesh: *mut c_void, i: usize, nobody: bool) -> bool {
        let Some(name) = self.sample.world.as_ref().map(|w| w.bones[i]) else { return false };
        // SAFETY: mesh proven live / PrimitiveComponent by the caller.
        unsafe {
            let Some(o) = self.pe(vt, F::SocketTransform, mesh, |b, o| b[o[0] as usize..o[0] as usize + 8].copy_from_slice(&name.to_le_bytes())) else {
                return false;
            };
            let (p, q) = self.transform_at(o[2] as usize);
            let mut r = [p[0], p[1], p[2], q[0], q[1], q[2], q[3], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            if !nobody {
                let Some(o) = self.pe(vt, F::LinVelAt, mesh, |b, o| {
                    for k in 0..3 {
                        let at = o[0] as usize + 8 * k;
                        b[at..at + 8].copy_from_slice(&p[k].to_le_bytes());
                    }
                    b[o[1] as usize..o[1] as usize + 8].copy_from_slice(&name.to_le_bytes());
                }) else {
                    return false;
                };
                let v = vec3(self.pbuf(), o[2] as usize);
                let Some(o) = self.pe(vt, F::AngVel, mesh, |b, o| b[o[0] as usize..o[0] as usize + 8].copy_from_slice(&name.to_le_bytes())) else {
                    return false;
                };
                let w = vec3(self.pbuf(), o[1] as usize);
                r[7..10].copy_from_slice(&v);
                r[10..13].copy_from_slice(&w);
            }
            if !r.iter().all(|x| finite(*x)) {
                return false;
            }
            self.sample.bones[i * 13..i * 13 + 13].copy_from_slice(&r);
        }
        true
    }

    /// Actor location / rotation (pitch, yaw, roll) / velocity of a live actor.
    unsafe fn actor_state(&mut self, vt: &HsmpReflect, actor: *mut c_void) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
        // SAFETY: actor proven live / Actor by the caller.
        unsafe {
            let o = self.pe(vt, F::ActorLoc, actor, |_, _| {})?;
            let pos = vec3(self.pbuf(), o[0] as usize);
            let o = self.pe(vt, F::ActorRot, actor, |_, _| {})?;
            let rot = vec3(self.pbuf(), o[0] as usize);
            let o = self.pe(vt, F::Velocity, actor, |_, _| {})?;
            let vel = vec3(self.pbuf(), o[0] as usize);
            Some((pos, rot, vel))
        }
    }

    /// The property names looked up on a pawn (control) or a weapon actor, in slot order.
    fn prop_names(&self, pawn: bool) -> Vec<String> {
        let Some(cfg) = self.sample.cfg.as_ref() else { return Vec::new() };
        if !pawn {
            return vec![cfg.w_root.clone(), cfg.w_base.clone(), cfg.w_tip.clone()];
        }
        let mut names: Vec<String> = Vec::new();
        names.extend(cfg.flags.iter().cloned());
        names.extend(cfg.scalars.iter().cloned());
        names.extend(cfg.ik.iter().cloned());
        names.push(cfg.grip_r.clone());
        names.push(cfg.grip_l.clone());
        names.push(cfg.aim.clone());
        names.push(cfg.ctrl_rot.clone());
        names
    }

    /// Make sure the property descriptors of `obj`'s class are cached (per class, per world);
    /// returns the cache key. Names are only built on a miss (no allocation once warm).
    unsafe fn class_props(&mut self, vt: &HsmpReflect, obj: *mut c_void, pawn: bool) -> Option<u64> {
        // SAFETY: obj proven live by the caller.
        let key = unsafe { class_key(vt, obj) }?;
        {
            let w = self.sample.world.as_ref()?;
            let map = if pawn { &w.pawn_props } else { &w.weapon_props };
            if map.contains_key(&key) {
                return Some(key);
            }
        }
        let names = self.prop_names(pawn);
        let n = self.sample.names.unwrap_or_default();
        let mut v = Vec::with_capacity(names.len());
        for name in names {
            let mut p = HsmpProp::default();
            let wname = wide(&name);
            // SAFETY: obj live; NUL-terminated name.
            let found = unsafe { (vt.obj_prop)(obj, wname.as_ptr(), &mut p) } == 1;
            let soft = p.cls == n.soft_object || p.cls == n.soft_class;
            v.push((found && !soft && p.offset >= 0 && p.size > 0).then_some(p));
        }
        let w = self.sample.world.as_mut()?;
        let map = if pawn { &mut w.pawn_props } else { &mut w.weapon_props };
        map.insert(key, v);
        Some(key)
    }

    fn cached_props(&self, key: u64, pawn: bool) -> Option<&Vec<Option<HsmpProp>>> {
        let w = self.sample.world.as_ref()?;
        if pawn {
            w.pawn_props.get(&key)
        } else {
            w.weapon_props.get(&key)
        }
    }

    // ---- property reads (Lua semantics) ----

    unsafe fn prop_bytes<'a>(obj: *mut c_void, p: &HsmpProp) -> &'a [u8] {
        // SAFETY: obj live; offset / size from the engine's own property of obj's class.
        unsafe { std::slice::from_raw_parts((obj as *const u8).add(p.offset as usize), p.size as usize) }
    }

    /// `pawn[k] == true`
    unsafe fn read_flag(&self, obj: *mut c_void, p: Option<HsmpProp>) -> bool {
        let n = self.sample.names.unwrap_or_default();
        match p {
            Some(p) if p.cls == n.bool_prop => {
                // SAFETY: as prop_bytes.
                let b = unsafe { Self::prop_bytes(obj, &p) };
                let m = if p.bool_mask == 0 { 0xff } else { p.bool_mask };
                b.get(p.bool_offset as usize).is_some_and(|x| x & m != 0)
            }
            _ => false,
        }
    }

    /// `tonumber(pawn[k]) or 0`
    unsafe fn read_num(&self, obj: *mut c_void, p: Option<HsmpProp>) -> f64 {
        let n = self.sample.names.unwrap_or_default();
        let Some(p) = p else { return 0.0 };
        // SAFETY: as prop_bytes.
        let b = unsafe { Self::prop_bytes(obj, &p) };
        let le = |k: usize| -> u64 {
            let mut a = [0u8; 8];
            a[..k].copy_from_slice(&b[..k]);
            u64::from_le_bytes(a)
        };
        match (p.cls, p.size) {
            (c, 1) if c == n.byte_prop || c == n.enum_prop => b[0] as f64,
            (c, 2) if c == n.enum_prop => le(2) as f64,
            (c, 4) if c == n.enum_prop || c == n.uint32_prop => le(4) as u32 as f64,
            (c, 4) if c == n.int_prop => le(4) as u32 as i32 as f64,
            (c, 8) if c == n.int64_prop => le(8) as i64 as f64,
            (c, 4) if c == n.float_prop => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            (c, 8) if c == n.double_prop => rd_f64(b, 0),
            _ => 0.0,
        }
    }

    /// A Vector property (`nil` if absent / another type).
    unsafe fn read_vec(&self, obj: *mut c_void, p: Option<HsmpProp>, sub: u64) -> Option<[f64; 3]> {
        let n = self.sample.names.unwrap_or_default();
        let p = p.filter(|p| p.cls == n.struct_prop && p.sub == sub && p.size == 24)?;
        // SAFETY: as prop_bytes.
        Some(vec3(unsafe { Self::prop_bytes(obj, &p) }, 0))
    }

    /// An object property's value, proven live and of class `cls`.
    unsafe fn read_obj(&self, vt: &HsmpReflect, obj: *mut c_void, p: Option<HsmpProp>, cls: u64) -> Option<*mut c_void> {
        let n = self.sample.names.unwrap_or_default();
        let p = p.filter(|p| p.cls == n.object_prop && p.size == 8)?;
        // SAFETY: as prop_bytes; an object property of a live object is null or a live object.
        let v = unsafe { Self::prop_bytes(obj, &p) };
        let ptr = u64::from_le_bytes(v[..8].try_into().ok()?) as usize as *mut c_void;
        // SAFETY: weak round trip + IsA.
        unsafe { live(vt, ptr, cls) }
    }

    /// The control layer (put_pose `c`) from the pawn's BP variables.
    unsafe fn sample_control(&mut self, vt: &HsmpReflect, pawn: *mut c_void) -> bool {
        let idx = self.sample.pawn_idx();
        let Some(cfg) = self.sample.cfg.as_ref() else { return false };
        let (nf, ns, ni) = (cfg.flags.len(), cfg.scalars.len(), cfg.ik.len());
        // SAFETY: pawn proven live by the caller.
        let Some(key) = (unsafe { self.class_props(vt, pawn, true) }) else { return false };
        let Some(props) = self.cached_props(key, true) else { return false };
        if props.len() < idx.n {
            return false;
        }
        let n = self.sample.names.unwrap_or_default();
        let mut c = [0.0f64; CONTROL_NUMS];
        // SAFETY: reads at verified offsets of the live pawn.
        unsafe {
            let mut flags: u64 = 0;
            for i in 0..nf.min(64) {
                if self.read_flag(pawn, props[idx.flags + i]) {
                    flags |= 1 << i;
                }
            }
            c[0] = flags as f64;
            c[1] = self.read_num(pawn, props[idx.grip_r]);
            c[2] = self.read_num(pawn, props[idx.grip_l]);
            for i in 0..ns.min(16) {
                let v = self.read_num(pawn, props[idx.scalars + i]);
                c[3 + i] = if finite(v) { v } else { 0.0 };
            }
            let vec3_lua = |v: Option<[f64; 3]>| match v {
                Some(v) if finite(v[0]) => v,
                _ => [0.0; 3],
            };
            let aim = vec3_lua(self.read_vec(pawn, props[idx.aim], n.vector));
            c[19..22].copy_from_slice(&aim);
            if let Some(r) = self.read_vec(pawn, props[idx.ctrl_rot], n.rotator) {
                c[22] = if finite(r[0]) { r[0] } else { 0.0 };
                c[23] = if finite(r[1]) { r[1] } else { 0.0 };
            }
            for i in 0..ni.min(4) {
                let v = vec3_lua(self.read_vec(pawn, props[idx.ik + i], n.vector));
                c[24 + 3 * i..27 + 3 * i].copy_from_slice(&v);
            }
            c[36] = 0.0;
        }
        self.sample.control = c;
        true
    }

    /// One held weapon (put_pose `w`, 21 numbers) or None when it is not really in the hand.
    unsafe fn sample_weapon(&mut self, vt: &HsmpReflect, mesh: *mut c_void, w: *mut c_void, hands: f64, tag: f64) -> Option<[f64; WEAPON_NUMS]> {
        let (classes, hand_l, hand_r) = {
            let wd = self.sample.world.as_ref()?;
            (wd.classes, wd.hand_l, wd.hand_r)
        };
        // SAFETY: every object below is proven live and of the needed class before a call.
        unsafe {
            let w = live(vt, w, classes[C_ACTOR])?;
            let key = self.class_props(vt, w, false)?;
            let pr = self.cached_props(key, false)?;
            let props = [*pr.first()?, *pr.get(1)?, *pr.get(2)?];
            let root = self.read_obj(vt, w, props[0], classes[C_PRIM])?;
            let base = self.read_obj(vt, w, props[1], classes[C_SCENE]);
            let tip = self.read_obj(vt, w, props[2], classes[C_SCENE]);
            let o = self.pe(vt, F::IsSim, root, |_, _| {})?;
            if self.pbuf()[o[1] as usize] == 0 {
                return None;
            }
            let o = self.pe(vt, F::ActorTransform, w, |_, _| {})?;
            let (p, q) = self.transform_at(o[0] as usize);
            let hand = if hands == 2.0 { hand_l } else { hand_r };
            let o = self.pe(vt, F::SocketLoc, mesh, |b, o| b[o[0] as usize..o[0] as usize + 8].copy_from_slice(&hand.to_le_bytes()))?;
            let hb = vec3(self.pbuf(), o[1] as usize);
            let d = ((hb[0] - p[0]).powi(2) + (hb[1] - p[1]).powi(2) + (hb[2] - p[2]).powi(2)).sqrt();
            if d > 150.0 {
                return None;
            }
            let mut v = [0.0; 3];
            if let Some(o) = self.pe(vt, F::LinVelAt, root, |b, o| {
                for k in 0..3 {
                    let at = o[0] as usize + 8 * k;
                    b[at..at + 8].copy_from_slice(&p[k].to_le_bytes());
                }
            }) {
                v = vec3(self.pbuf(), o[2] as usize);
            }
            let mut av = [0.0; 3];
            if let Some(o) = self.pe(vt, F::AngVel, root, |_, _| {}) {
                av = vec3(self.pbuf(), o[1] as usize);
            }
            let loc = |c: Option<*mut c_void>, this: &mut Self| -> [f64; 3] {
                match c.and_then(|c| this.pe(vt, F::CompLoc, c, |_, _| {})) {
                    Some(o) => vec3(this.pbuf(), o[0] as usize),
                    None => p,
                }
            };
            let b = loc(base, self);
            let tp = loc(tip, self);
            let mut out = [0.0; WEAPON_NUMS];
            out[0] = hands;
            out[1] = tag;
            out[2..5].copy_from_slice(&p);
            out[5..9].copy_from_slice(&q);
            out[9..12].copy_from_slice(&v);
            out[12..15].copy_from_slice(&av);
            out[15..18].copy_from_slice(&b);
            out[18..21].copy_from_slice(&tp);
            Some(out)
        }
    }

    // ---- Lua entry points ------------------------------------------------------------------------

    /// `sample_config(cfg)` -> true | nil, err. Names as HSMPSync uses them: `bones` (array),
    /// `nobody` (set of bone names without a body), `flags`, `scalars`, `ik` (arrays),
    /// `grip_r`, `grip_l`, `aim`, `ctrl_rot`, `weapon_root`, `weapon_base`, `weapon_tip`.
    pub unsafe fn sample_config(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) {
                return nil_err(L, "bad");
            }
            let t = lua_absindex(L, 1);
            let strs = |key: &str| -> Option<Vec<String>> {
                let mut v = Vec::new();
                if rawget_str(L, t, key) != LUA_TTABLE {
                    pop(L, 1);
                    return None;
                }
                let a = lua_gettop(L);
                for i in 1..=lua_rawlen(L, a) as i64 {
                    lua_rawgeti(L, a, i);
                    let s = arg_str(L, -1).map(str::to_string);
                    pop(L, 1);
                    v.push(s?);
                }
                pop(L, 1);
                Some(v)
            };
            let one = |key: &str| -> Option<String> {
                rawget_str(L, t, key);
                let s = arg_str(L, -1).map(str::to_string);
                pop(L, 1);
                s
            };
            let (Some(bones), Some(flags), Some(scalars), Some(ik)) = (strs("bones"), strs("flags"), strs("scalars"), strs("ik")) else {
                return nil_err(L, "bad");
            };
            if bones.len() != NBONES || flags.len() > 64 || scalars.len() > 16 || ik.len() > 4 {
                return nil_err(L, "bad");
            }
            let mut nobody = vec![false; bones.len()];
            if rawget_str(L, t, "nobody") == LUA_TTABLE {
                let s = lua_gettop(L);
                for (i, b) in bones.iter().enumerate() {
                    if rawget_str(L, s, b) != LUA_TNIL && lua_toboolean(L, -1) != 0 {
                        nobody[i] = true;
                    }
                    pop(L, 1);
                }
            }
            pop(L, 1);
            let (Some(grip_r), Some(grip_l), Some(aim), Some(ctrl_rot)) = (one("grip_r"), one("grip_l"), one("aim"), one("ctrl_rot")) else {
                return nil_err(L, "bad");
            };
            let w_root = one("weapon_root").unwrap_or_else(|| "RootComponent".into());
            let (Some(w_base), Some(w_tip)) = (one("weapon_base"), one("weapon_tip")) else { return nil_err(L, "bad") };
            self.sample.cfg = Some(Cfg { bones, nobody, flags, scalars, ik, grip_r, grip_l, aim, ctrl_rot, w_root, w_base, w_tip });
            self.sample.world = None;
            self.sample.why = None;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// `sample_local(a)` -> `mask` | `mask, err` | `nil, err`.
    ///
    /// `a` (a reused table): `root_pawn`, `root_tick`, `root_ts` (root record); `weapon_actor`,
    /// `weapon_tick`, `weapon_ts`, `weapon_id`, `weapon_held` (weapon record); `mesh`, `pawn`,
    /// `pose_tick`, `pose_ts`, `dt`, `k`, `control` (bool), `w1`, `h1`, `t1`, `w2`, `h2`, `t2`
    /// (weapon actor / hands / class tag; 0 = none) (pose record). Objects are
    /// `obj:GetAddress()` integers. `mask`: 1 root, 2 weapon, 4 pose written. `err`:
    /// `unavailable` (no engine access: option E / no UE4SS), `world` (between world leave
    /// and ready), `disabled:<why>` (verification refused), `not configured`, or
    /// `skip:<what>` / `bad:<field>` for this sample (Lua then uses its own path).
    pub unsafe fn sample_local(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) {
                return nil_err(L, "bad");
            }
            let t = lua_absindex(L, 1);
            let Some(vt) = reflect::vt() else { return nil_err(L, "unavailable") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if !self.sample.world_ok {
                return nil_err(L, "world");
            }
            if self.sample.cfg.is_none() {
                return nil_err(L, "not configured");
            }
            if let Err(e) = self.sample_verify(vt) {
                return nil_err(L, &format!("disabled:{}", e));
            }
            let t0 = hsmp_ipc::shm::qpc();
            let num = |k: &str| -> f64 {
                let ty = rawget_str(L, t, k);
                let v = if ty == LUA_TNUMBER { lua_tonumberx(L, -1, std::ptr::null_mut()) } else { 0.0 };
                pop(L, 1);
                v
            };
            let addr = |k: &str| -> *mut c_void {
                let ty = rawget_str(L, t, k);
                let v = if ty == LUA_TNUMBER { lua_tointegerx(L, -1, std::ptr::null_mut()) } else { 0 };
                pop(L, 1);
                v as usize as *mut c_void
            };
            let classes = self.sample.world.as_ref().map(|w| w.classes).unwrap_or_default();
            let mut mask = 0i64;
            let mut err: Option<String> = None;
            let fail = |e: &str, err: &mut Option<String>| {
                if err.is_none() {
                    *err = Some(e.to_string());
                }
            };

            // root
            let rp = addr("root_pawn");
            if !rp.is_null() {
                rawget_str(L,t,"context");
                let root_context=crate::pose_hot::read_pose_context(L,-1);pop(L,1);
                // The original context is captured from the very same pawn
                // supplied to the pose sample; never authorize a second actor.
                if rp!=addr("pawn") {fail("skip:root_context_pawn",&mut err);}
                else {
                match live(vt, rp, classes[C_ACTOR]).and_then(|a| self.actor_state(vt, a)) {
                    Some((pos, rot, vel)) => match self.write_root(num("root_tick") as u32, num("root_ts"), pos, rot, vel,root_context) {
                        Ok(()) => mask |= 1,
                        Err(e) => fail(&hot_msg(e), &mut err),
                    },
                    None => fail("skip:root", &mut err),
                }
                }
            }
            // weapon actor
            let wa = addr("weapon_actor");
            if !wa.is_null() {
                match live(vt, wa, classes[C_ACTOR]).and_then(|a| self.actor_state(vt, a)) {
                    Some((pos, rot, vel)) => {
                        match self.write_weapon(num("weapon_tick") as u32, num("weapon_ts"), num("weapon_id") as u32, num("weapon_held") as u32, pos, rot, vel) {
                            Ok(()) => mask |= 2,
                            Err(e) => fail(&hot_msg(e), &mut err),
                        }
                    }
                    None => fail("skip:weapon", &mut err),
                }
            }
            // pose
            let mesh = addr("mesh");
            if !mesh.is_null() {
                let ok = (|| -> Result<(), String> {
                    let mesh = live(vt, mesh, classes[C_PRIM]).ok_or("skip:mesh")?;
                    let nobody = self.sample.cfg.as_ref().map(|c| c.nobody.clone()).unwrap_or_default();
                    for (i, nb) in nobody.iter().enumerate().take(NBONES) {
                        if !self.sample_bone(vt, mesh, i, *nb) {
                            return Err("skip:bone".into());
                        }
                    }
                    let mut nw = 0usize;
                    let mut boxes = Vec::new();
                    for (wk, hk, tk, sk) in [("w1", "h1", "t1", "s1"), ("w2", "h2", "t2", "s2")] {
                        let wp = addr(wk);
                        if wp.is_null() {
                            continue;
                        }
                        if let Some(v) = self.sample_weapon(vt, mesh, wp, num(hk), num(tk)) {
                            self.sample.weapons[nw] = v;
                            rawget_str(L, t, sk);
                            boxes.push(crate::pose_hot::read_weapon_boxes(L, -1));
                            pop(L, 1);
                            nw += 1;
                        }
                    }
                    let pawn = addr("pawn");
                    rawget_str(L, t, "control");
                    let want_ctl = lua_toboolean(L, -1) != 0;
                    pop(L, 1);
                    let mut ctl = false;
                    if want_ctl && !pawn.is_null() {
                        if let Some(p) = live(vt, pawn, classes[C_ACTOR]) {
                            ctl = self.sample_control(vt, p);
                        }
                    }
                    let bones = *self.sample.bones;
                    let weapons = self.sample.weapons;
                    let control = self.sample.control;
                    rawget_str(L,t,"strikers");
                    let strikers=crate::pose_hot::read_body_strikers(L,-1); pop(L,1);
                    rawget_str(L,t,"context");
                    let context=crate::pose_hot::read_pose_context(L,-1); pop(L,1);
                    self.write_pose(num("pose_tick") as u32, num("pose_ts"), num("dt"), num("k"), &bones, &weapons[..nw], ctl.then_some(&control), &boxes, strikers.as_deref(), context)
                        .map_err(hot_msg)?;
                    Ok(())
                })();
                match ok {
                    Ok(()) => mask |= 4,
                    Err(e) => fail(&e, &mut err),
                }
            }
            let dt = (hsmp_ipc::shm::qpc().saturating_sub(t0) as u128 * 1_000_000_000 / self.qpc_freq.max(1) as u128) as u64;
            self.sample.samples += 1;
            self.sample.ns_total += dt;
            self.sample.ns_last = dt;
            if err.is_some() {
                self.sample.fallbacks += 1;
            }
            lua_pushinteger(L, mask);
            match err {
                Some(e) => {
                    push_str(L, &e);
                    2
                }
                None => 1,
            }
        }
    }

    /// `sample_status()` -> {available, configured, verified, world_ok, why, samples,
    /// fallbacks, pe_calls, avg_us, last_us, pose}. `pose` is the last successful
    /// slot write's original generation/timestamp, not a sampling attempt.
    pub unsafe fn sample_status(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            lua_createtable(L, 0, 11);
            let t = lua_gettop(L);
            let s = &self.sample;
            set_bool(L, t, "available", reflect::vt().is_some());
            set_bool(L, t, "configured", s.cfg.is_some());
            set_bool(L, t, "verified", s.world.is_some());
            set_bool(L, t, "world_ok", s.world_ok);
            match &s.why {
                Some(w) => set_str(L, t, "why", w),
                None => set_nil(L, t, "why"),
            }
            set_int(L, t, "samples", s.samples as i64);
            set_int(L, t, "fallbacks", s.fallbacks as i64);
            set_int(L, t, "pe_calls", s.pe_calls as i64);
            set_num(L, t, "avg_us", if s.samples > 0 { s.ns_total as f64 / s.samples as f64 / 1000.0 } else { 0.0 });
            set_num(L, t, "last_us", s.ns_last as f64 / 1000.0);
            if let Some(pose) = s.pose_written {
                lua_createtable(L, 0, 5);
                let p = lua_gettop(L);
                set_int(L, p, "tick", pose.tick as i64);
                set_num(L, p, "ts", pose.ts);
                set_int(L, p, "match_id", pose.context.match_id as i64);
                set_int(L, p, "round", pose.context.round as i64);
                set_int(L, p, "life", pose.context.life as i64);
                rawset_str(L, t, "pose");
            } else {
                set_nil(L, t, "pose");
            }
            1
        }
    }
}

fn hot_msg(e: crate::pose_hot::HotErr) -> String {
    match e {
        crate::pose_hot::HotErr::NotOpen => "not open".into(),
        crate::pose_hot::HotErr::Bad(f) => format!("bad:{}", f),
    }
}

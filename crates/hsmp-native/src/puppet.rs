//! Display-only client fighters. Once a bootstrapped pawn is complete it stops
//! simulating; every frame it shows the authority's played-back physical pose
//! (23 bodies through a hidden Poseable calculator, held weapons, root, control
//! rotation). Combat physics exists only on the authority.
//!
//! Binding does the full engine setup once. Per frame each kept object is only
//! re-resolved through its weak handle; a lost object fails that puppet so the
//! caller rebuilds it.
use crate::{
    reflect::{self, wide, HsmpProp, HsmpReflect},
    sample::{fname, props_of},
};
use hsmp_pose::{
    posecodec::v2,
    poseplay::{Frame, Playback, Sample, WPN_R},
};
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::atomic::{AtomicPtr, Ordering},
    time::Instant,
};

pub type Publish = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut u8, u32) -> i32;
static PUBLISH: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// # Safety
/// `publish` null or valid for the life of the process.
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_set_puppet_publish(publish: Option<Publish>) {
    PUBLISH.store(
        publish.map_or(std::ptr::null_mut(), |f| f as *mut c_void),
        Ordering::Release,
    );
}
fn publisher() -> Option<Publish> {
    let p = PUBLISH.load(Ordering::Acquire);
    // SAFETY: only ever stored from a `Publish` function pointer.
    (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Publish>(p) })
}

const PARAMS: usize = 1024;
const COMPONENT_SPACE: u8 = 1;
const NO_COLLISION: u8 = 0;
// Frames between re-applying the display-only settings (about half a second).
const REASSERT_EVERY: u64 = 30;
// A tick this far behind the last one means the authority restarted its counter.
const TICK_RESET: u32 = 600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Handle {
    weak: u64,
    address: usize,
}
impl Handle {
    unsafe fn keep(vt: &HsmpReflect, p: *mut c_void) -> Option<Self> {
        // SAFETY: the caller proves `p` live in this call.
        (!p.is_null())
            .then(|| unsafe { reflect::keep(vt, p) })
            .flatten()
            .map(|weak| Handle {
                weak,
                address: p as usize,
            })
    }
    unsafe fn live(&self, vt: &HsmpReflect) -> Option<*mut c_void> {
        // SAFETY: resolve only reads the object array.
        let p = unsafe { reflect::get(vt, self.weak) };
        (!p.is_null() && p as usize == self.address).then_some(p)
    }
}

struct Ufn {
    weak: u64,
    params: Vec<(&'static str, HsmpProp)>,
}
impl Ufn {
    unsafe fn new(vt: &HsmpReflect, path: &str, names: &[&'static str]) -> Result<Self, String> {
        unsafe {
            let f = (vt.find)(wide(path).as_ptr());
            if f.is_null() {
                return Err(format!("puppet function unavailable: {path}"));
            }
            let (fields, size) = props_of(vt, f);
            if size < 0 || size as usize > PARAMS {
                return Err(format!("puppet function params: {path}"));
            }
            let boolean = fname(vt, "BoolProperty", true);
            let mut params = Vec::with_capacity(names.len());
            for name in names {
                let id = fname(vt, name, true);
                let p = fields
                    .iter()
                    .find(|p| p.name == id)
                    .ok_or_else(|| format!("puppet function {path}: no {name}"))?;
                if p.offset < 0
                    || (p.offset + p.size) as usize > PARAMS
                    || (p.cls == boolean && p.bool_mask == 0)
                {
                    return Err(format!("puppet function {path}: {name} layout"));
                }
                params.push((*name, *p));
            }
            Ok(Ufn {
                weak: reflect::keep(vt, f)
                    .ok_or_else(|| format!("puppet function weak: {path}"))?,
                params,
            })
        }
    }
    fn p(&self, name: &str) -> HsmpProp {
        self.params
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, p)| *p)
            .unwrap_or_default()
    }
}

struct Args([u8; PARAMS]);
impl Args {
    fn new() -> Self {
        Args([0; PARAMS])
    }
    fn put(&mut self, p: HsmpProp, bytes: &[u8]) -> &mut Self {
        let at = p.offset as usize;
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
        self
    }
    fn flag(&mut self, p: HsmpProp, value: bool) -> &mut Self {
        let at = p.offset as usize + p.bool_offset as usize;
        if value {
            self.0[at] |= p.bool_mask
        } else {
            self.0[at] &= !p.bool_mask
        }
        self
    }
    fn f64s<const N: usize>(&self, p: HsmpProp) -> [f64; N] {
        let at = p.offset as usize;
        std::array::from_fn(|i| {
            f64::from_le_bytes(
                self.0[at + 8 * i..at + 8 * i + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            )
        })
    }
    fn i32(&self, p: HsmpProp) -> i32 {
        let at = p.offset as usize;
        i32::from_le_bytes(self.0[at..at + 4].try_into().unwrap_or([0; 4]))
    }
    fn ptr(&self, p: HsmpProp) -> *mut c_void {
        let at = p.offset as usize;
        u64::from_le_bytes(self.0[at..at + 8].try_into().unwrap_or([0; 8])) as usize as *mut c_void
    }
}

/// Engine FTransform bytes: rotation XYZW, translation, scale (doubles, padded).
fn transform(p: [f64; 3], q: [f64; 4], s: [f64; 3]) -> [u8; 96] {
    let mut out = [0u8; 96];
    for (i, v) in q.iter().enumerate() {
        out[8 * i..8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in p.iter().enumerate() {
        out[32 + 8 * i..40 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in s.iter().enumerate() {
        out[64 + 8 * i..72 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    out
}
fn vec_bytes<const N: usize>(v: [f64; N]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn yaw_quat(yaw_deg: f64) -> [f64; 4] {
    let h = yaw_deg.to_radians() * 0.5;
    [0., 0., h.sin(), h.cos()]
}

struct Fns {
    tick_actor: Ufn,
    collide_actor: Ufn,
    simulate: Ufn,
    collision: Ufn,
    tick_component: Ufn,
    postprocess: Ufn,
    cloth: Ufn,
    add: Ufn,
    asset: Ufn,
    visible: Ufn,
    bones: Ufn,
    bone_index: Ufn,
    scale: Ufn,
    actor_scale: Ufn,
    disable_input: Ufn,
    set_bone: Ufn,
    world: Ufn,
    actor: Ufn,
    control: Ufn,
    poseable: u64,
}
impl Fns {
    unsafe fn new(vt: &HsmpReflect) -> Result<Self, String> {
        unsafe {
            let poseable = (vt.find)(wide("/Script/Engine.PoseableMeshComponent").as_ptr());
            Ok(Fns {
                tick_actor: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:SetActorTickEnabled",
                    &["bEnabled"],
                )?,
                collide_actor: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:SetActorEnableCollision",
                    &["bNewActorEnableCollision"],
                )?,
                simulate: Ufn::new(
                    vt,
                    "/Script/Engine.PrimitiveComponent:SetSimulatePhysics",
                    &["bSimulate"],
                )?,
                collision: Ufn::new(
                    vt,
                    "/Script/Engine.PrimitiveComponent:SetCollisionEnabled",
                    &["NewType"],
                )?,
                tick_component: Ufn::new(
                    vt,
                    "/Script/Engine.ActorComponent:SetComponentTickEnabled",
                    &["bEnabled"],
                )?,
                postprocess: Ufn::new(
                    vt,
                    "/Script/Engine.SkeletalMeshComponent:SetDisablePostProcessBlueprint",
                    &["bInDisablePostProcess"],
                )?,
                cloth: Ufn::new(
                    vt,
                    "/Script/Engine.SkeletalMeshComponent:SuspendClothingSimulation",
                    &[],
                )?,
                add: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:AddComponentByClass",
                    &[
                        "Class",
                        "bManualAttachment",
                        "RelativeTransform",
                        "bDeferredFinish",
                        "ReturnValue",
                    ],
                )?,
                asset: Ufn::new(
                    vt,
                    "/Script/Engine.SkinnedMeshComponent:SetSkinnedAssetAndUpdate",
                    &["NewMesh", "bReinitPose"],
                )?,
                visible: Ufn::new(
                    vt,
                    "/Script/Engine.SceneComponent:SetVisibility",
                    &["bNewVisibility", "bPropagateToChildren"],
                )?,
                bones: Ufn::new(
                    vt,
                    "/Script/Engine.SkinnedMeshComponent:GetNumBones",
                    &["ReturnValue"],
                )?,
                bone_index: Ufn::new(
                    vt,
                    "/Script/Engine.SkinnedMeshComponent:GetBoneIndex",
                    &["BoneName", "ReturnValue"],
                )?,
                scale: Ufn::new(
                    vt,
                    "/Script/Engine.SceneComponent:K2_GetComponentScale",
                    &["ReturnValue"],
                )?,
                actor_scale: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:GetActorScale3D",
                    &["ReturnValue"],
                )?,
                disable_input: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:DisableInput",
                    &["PlayerController"],
                )?,
                set_bone: Ufn::new(
                    vt,
                    "/Script/Engine.PoseableMeshComponent:SetBoneTransformByName",
                    &["BoneName", "InTransform", "BoneSpace"],
                )?,
                world: Ufn::new(
                    vt,
                    "/Script/Engine.SceneComponent:K2_SetWorldTransform",
                    &["NewTransform", "bSweep", "bTeleport"],
                )?,
                actor: Ufn::new(
                    vt,
                    "/Script/Engine.Actor:K2_SetActorTransform",
                    &["NewTransform", "bSweep", "bTeleport"],
                )?,
                control: Ufn::new(
                    vt,
                    "/Script/Engine.Controller:SetControlRotation",
                    &["NewRotation"],
                )?,
                poseable: if poseable.is_null() {
                    0
                } else {
                    reflect::keep(vt, poseable).ok_or("puppet Poseable class weak")?
                },
            })
        }
    }
    unsafe fn call(
        &self,
        vt: &HsmpReflect,
        f: &Ufn,
        obj: *mut c_void,
        args: &mut Args,
    ) -> Result<(), String> {
        unsafe {
            let func = reflect::get(vt, f.weak);
            if func.is_null() || obj.is_null() {
                return Err("puppet function or receiver expired".into());
            }
            (vt.call)(obj, func, args.0.as_mut_ptr().cast());
            Ok(())
        }
    }
    unsafe fn set_bool(
        &self,
        vt: &HsmpReflect,
        f: &Ufn,
        key: &str,
        obj: *mut c_void,
        value: bool,
    ) -> Result<(), String> {
        unsafe { self.call(vt, f, obj, Args::new().flag(f.p(key), value)) }
    }
}

unsafe fn object_field(vt: &HsmpReflect, obj: *mut c_void, field: &str) -> Option<*mut c_void> {
    unsafe {
        let mut p = HsmpProp::default();
        if (vt.obj_prop)(obj, wide(field).as_ptr(), &mut p) != 1 || p.size != 8 || p.offset < 0 {
            return None;
        }
        let value =
            std::ptr::read_unaligned((obj as *const u8).add(p.offset as usize) as *const usize)
                as *mut c_void;
        let weak = if value.is_null() { 0 } else { (vt.weak)(value) };
        (weak != 0 && (vt.resolve)(weak) == value).then_some(value)
    }
}
unsafe fn scalar_field(vt: &HsmpReflect, obj: *mut c_void, field: &str) -> Option<HsmpProp> {
    unsafe {
        let mut p = HsmpProp::default();
        ((vt.obj_prop)(obj, wide(field).as_ptr(), &mut p) == 1
            && p.offset >= 0
            && (p.size == 4 || p.size == 8))
            .then_some(p)
    }
}

struct Weapon {
    actor: Handle,
    root: Option<Handle>,
    scale: [f64; 3],
}
pub(crate) struct Puppet {
    pawn: Handle,
    mesh: Handle,
    calculator: Handle,
    controller: Option<Handle>,
    weapons: [Option<Weapon>; 2],
    bones: [u64; v2::NB],
    count: u32,
    mesh_scale: [f64; 3],
    actor_scale: [f64; 3],
    health: Option<HsmpProp>,
    stamina: Option<HsmpProp>,
    play: Playback,
    last_tick: u32,
    calls: u64,
    frames: u64,
}

/// Every bound puppet, by entity id, with the verified engine functions.
#[derive(Default)]
pub(crate) struct Puppets {
    fns: Option<Fns>,
    map: HashMap<u32, Puppet>,
    base: Option<Instant>,
}

/// One authority row's played-back inputs for a frame.
pub(crate) struct Feed<'a> {
    pub tick: u32,
    pub received: Instant,
    pub pose: &'a [u8],
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub yaw: f64,
    pub health: f64,
    pub stamina: f64,
}

impl Puppets {
    /// Forget every engine handle without touching it (world drop / scope change).
    pub(crate) fn forget(&mut self) {
        self.map.clear();
        self.fns = None;
    }
    pub(crate) fn forget_one(&mut self, id: u32) {
        self.map.remove(&id);
    }
    pub(crate) fn bound(&self, id: u32) -> bool {
        self.map.contains_key(&id)
    }
    /// Fewest poses any bound puppet has actually published (diagnostics).
    pub(crate) fn fewest_shown(&self) -> u64 {
        self.map.values().map(|p| p.frames).min().unwrap_or(0)
    }
    fn ms(&mut self, at: Instant) -> f64 {
        let base = *self.base.get_or_insert(at);
        at.checked_duration_since(base)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }

    /// Turn a finished bootstrap pawn into a display-only puppet.
    pub(crate) unsafe fn bind(
        &mut self,
        vt: &HsmpReflect,
        id: u32,
        pawn: *mut c_void,
        controller: Option<*mut c_void>,
    ) -> Result<(), String> {
        unsafe {
            if publisher().is_none() {
                return Err("puppet pose publisher unavailable".into());
            }
            if self.fns.is_none() {
                self.fns = Some(Fns::new(vt)?);
            }
            let f = self.fns.as_ref().ok_or("puppet functions")?;
            let pawn_h = Handle::keep(vt, pawn).ok_or("puppet pawn unavailable")?;
            let mesh = object_field(vt, pawn, "Mesh").ok_or("puppet mesh unavailable")?;
            let asset = object_field(vt, mesh, "SkinnedAsset")
                .or_else(|| object_field(vt, mesh, "SkeletalMesh"))
                .ok_or("puppet mesh asset unavailable")?;
            f.set_bool(
                vt,
                &f.collide_actor,
                "bNewActorEnableCollision",
                pawn,
                false,
            )?;
            // The root follows the authority; local movement would drift it between frames.
            if let Some(movement) = object_field(vt, pawn, "CharacterMovement") {
                f.set_bool(vt, &f.tick_component, "bEnabled", movement, false)?;
            }
            f.set_bool(vt, &f.simulate, "bSimulate", mesh, false)?;
            f.call(
                vt,
                &f.collision,
                mesh,
                Args::new().put(f.collision.p("NewType"), &[NO_COLLISION]),
            )?;
            f.set_bool(vt, &f.postprocess, "bInDisablePostProcess", mesh, true)?;
            f.call(vt, &f.cloth, mesh, &mut Args::new())?;
            f.set_bool(vt, &f.tick_component, "bEnabled", mesh, false)?;
            let mut args = Args::new();
            f.call(vt, &f.scale, mesh, &mut args)?;
            let mesh_scale: [f64; 3] = args.f64s(f.scale.p("ReturnValue"));
            let mut args = Args::new();
            f.call(vt, &f.actor_scale, pawn, &mut args)?;
            let actor_scale: [f64; 3] = args.f64s(f.actor_scale.p("ReturnValue"));
            if !mesh_scale
                .iter()
                .chain(&actor_scale)
                .all(|v| v.is_finite() && v.abs() > 1e-4)
            {
                return Err("puppet scale unavailable".into());
            }
            let class = reflect::get(vt, f.poseable);
            if class.is_null() {
                return Err("puppet Poseable class unavailable".into());
            }
            let mut args = Args::new();
            args.put(f.add.p("Class"), &(class as usize as u64).to_le_bytes())
                .flag(f.add.p("bManualAttachment"), true)
                .put(
                    f.add.p("RelativeTransform"),
                    &transform([0.; 3], [0., 0., 0., 1.], [1.; 3]),
                )
                .flag(f.add.p("bDeferredFinish"), false);
            f.call(vt, &f.add, pawn, &mut args)?;
            let calculator = args.ptr(f.add.p("ReturnValue"));
            let calc_h = Handle::keep(vt, calculator).ok_or("puppet calculator creation failed")?;
            if (vt.is_a)(calculator, class) == 0 {
                return Err("puppet calculator class".into());
            }
            let mut args = Args::new();
            args.put(f.asset.p("NewMesh"), &(asset as usize as u64).to_le_bytes())
                .flag(f.asset.p("bReinitPose"), true);
            f.call(vt, &f.asset, calculator, &mut args)?;
            let mut args = Args::new();
            args.flag(f.visible.p("bNewVisibility"), false)
                .flag(f.visible.p("bPropagateToChildren"), false);
            f.call(vt, &f.visible, calculator, &mut args)?;
            f.call(
                vt,
                &f.collision,
                calculator,
                Args::new().put(f.collision.p("NewType"), &[NO_COLLISION]),
            )?;
            f.set_bool(vt, &f.tick_component, "bEnabled", calculator, false)?;
            let count = |c: *mut c_void| -> Result<i32, String> {
                let mut args = Args::new();
                f.call(vt, &f.bones, c, &mut args)?;
                Ok(args.i32(f.bones.p("ReturnValue")))
            };
            let (n_mesh, n_calc) = (count(mesh)?, count(calculator)?);
            if n_mesh <= 0 || n_mesh != n_calc || n_mesh > 1024 {
                return Err(format!(
                    "puppet bone dictionaries differ ({n_mesh}/{n_calc})"
                ));
            }
            let mut bones = [0u64; v2::NB];
            for (i, name) in v2::BONES.iter().enumerate() {
                bones[i] = fname(vt, name, true);
                let mut args = Args::new();
                args.put(f.bone_index.p("BoneName"), &bones[i].to_le_bytes());
                f.call(vt, &f.bone_index, calculator, &mut args)?;
                if args.i32(f.bone_index.p("ReturnValue")) < 0 {
                    return Err(format!("puppet bone missing: {name}"));
                }
            }
            let mut weapons: [Option<Weapon>; 2] = [None, None];
            for (slot, field) in ["Weapon R", "Weapon L"].into_iter().enumerate() {
                let Some(w) = object_field(vt, pawn, field) else {
                    continue;
                };
                if weapons
                    .iter()
                    .flatten()
                    .any(|o| o.actor.address == w as usize)
                {
                    continue;
                }
                f.set_bool(vt, &f.tick_actor, "bEnabled", w, false)?;
                f.set_bool(vt, &f.collide_actor, "bNewActorEnableCollision", w, false)?;
                let mut root_h = None;
                if let Some(root) = object_field(vt, w, "RootComponent") {
                    let prim = (vt.find)(wide("/Script/Engine.PrimitiveComponent").as_ptr());
                    if !prim.is_null() && (vt.is_a)(root, prim) != 0 {
                        f.set_bool(vt, &f.simulate, "bSimulate", root, false)?;
                        root_h = Handle::keep(vt, root);
                    }
                }
                let mut args = Args::new();
                f.call(vt, &f.actor_scale, w, &mut args)?;
                let scale: [f64; 3] = args.f64s(f.actor_scale.p("ReturnValue"));
                weapons[slot] = Handle::keep(vt, w).map(|actor| Weapon {
                    actor,
                    root: root_h,
                    scale,
                });
            }
            let controller = match controller {
                Some(pc) => {
                    let mut args = Args::new();
                    args.put(
                        f.disable_input.p("PlayerController"),
                        &(pc as usize as u64).to_le_bytes(),
                    );
                    f.call(vt, &f.disable_input, pawn, &mut args)?;
                    Some(Handle::keep(vt, pc).ok_or("puppet controller unavailable")?)
                }
                None => None,
            };
            let mesh_h = Handle::keep(vt, mesh).ok_or("puppet mesh weak")?;
            self.map.insert(
                id,
                Puppet {
                    pawn: pawn_h,
                    mesh: mesh_h,
                    calculator: calc_h,
                    controller,
                    weapons,
                    bones,
                    count: n_mesh as u32,
                    mesh_scale,
                    actor_scale,
                    health: scalar_field(vt, pawn, "Health"),
                    stamina: scalar_field(vt, pawn, "Stamina"),
                    play: Playback::new(),
                    last_tick: 0,
                    calls: 0,
                    frames: 0,
                },
            );
            Ok(())
        }
    }

    /// Feed one authority row and show the played-back pose. Err = this puppet is lost.
    pub(crate) unsafe fn apply(
        &mut self,
        vt: &HsmpReflect,
        id: u32,
        feed: &Feed<'_>,
        now: Instant,
    ) -> Result<(), String> {
        unsafe {
            let rx = self.ms(feed.received);
            let now_ms = self.ms(now);
            let publish = publisher().ok_or("puppet pose publisher unavailable")?;
            let f = self.fns.as_ref().ok_or("puppet functions")?;
            let puppet = self.map.get_mut(&id).ok_or("puppet not bound")?;
            if feed.tick.saturating_add(TICK_RESET) < puppet.last_tick {
                // The authority restarted its tick inside this generation.
                puppet.play = Playback::new();
                puppet.last_tick = 0;
            }
            if feed.tick > puppet.last_tick {
                puppet.last_tick = feed.tick;
                if let Some(frame) = (!feed.pose.is_empty())
                    .then(|| Frame::from_wire(feed.tick, feed.pose))
                    .flatten()
                {
                    let ts = frame.ts;
                    puppet.play.push_pose(rx, frame);
                    let p = feed.position.map(|v| v as f32);
                    let v = feed.velocity.map(|v| v as f32);
                    puppet.play.push_root(rx, ts, p, v, feed.yaw as f32);
                }
            }
            let pawn = puppet.pawn.live(vt).ok_or("puppet pawn lost")?;
            let mesh = puppet.mesh.live(vt).ok_or("puppet mesh lost")?;
            let calculator = puppet.calculator.live(vt).ok_or("puppet calculator lost")?;
            // Late Blueprint latent actions (and possession) can switch simulation or
            // tick back on; local physics would then fight the published pose.
            if puppet.calls % REASSERT_EVERY == 0 {
                f.set_bool(vt, &f.tick_actor, "bEnabled", pawn, false)?;
                f.set_bool(
                    vt,
                    &f.collide_actor,
                    "bNewActorEnableCollision",
                    pawn,
                    false,
                )?;
                f.set_bool(vt, &f.simulate, "bSimulate", mesh, false)?;
                f.set_bool(vt, &f.tick_component, "bEnabled", mesh, false)?;
                for weapon in puppet.weapons.iter().flatten() {
                    if let Some(actor) = weapon.actor.live(vt) {
                        f.set_bool(vt, &f.tick_actor, "bEnabled", actor, false)?;
                        if let Some(root) = weapon.root.and_then(|r| r.live(vt)) {
                            f.set_bool(vt, &f.simulate, "bSimulate", root, false)?;
                        }
                    }
                }
            }
            puppet.calls += 1;
            let sample: Option<Sample> = puppet.play.sample(now_ms);
            let (root_p, root_yaw) = match sample.as_ref().and_then(|s| s.root) {
                Some((p, yaw)) => (p.map(f64::from), f64::from(yaw)),
                None => (feed.position, feed.yaw),
            };
            let mut args = Args::new();
            args.put(
                f.actor.p("NewTransform"),
                &transform(root_p, yaw_quat(root_yaw), puppet.actor_scale),
            )
            .flag(f.actor.p("bSweep"), false)
            .flag(f.actor.p("bTeleport"), true);
            f.call(vt, &f.actor, pawn, &mut args)?;
            if let Some(s) = sample.as_ref().filter(|s| s.mask & 1 != 0) {
                let pelvis = s.bones[0];
                let origin = [pelvis[0] as f64, pelvis[1] as f64, pelvis[2] as f64];
                let mut args = Args::new();
                args.put(
                    f.world.p("NewTransform"),
                    &transform(origin, [0., 0., 0., 1.], puppet.mesh_scale),
                )
                .flag(f.world.p("bSweep"), false)
                .flag(f.world.p("bTeleport"), true);
                f.call(vt, &f.world, mesh, &mut args)?;
                for (i, name) in puppet.bones.iter().enumerate() {
                    if s.mask & (1 << i) == 0 {
                        continue;
                    }
                    let b = s.bones[i];
                    let p = [0, 1, 2].map(|k| (b[k] as f64 - origin[k]) / puppet.mesh_scale[k]);
                    let q = [b[3] as f64, b[4] as f64, b[5] as f64, b[6] as f64];
                    let mut args = Args::new();
                    args.put(f.set_bone.p("BoneName"), &name.to_le_bytes())
                        .put(f.set_bone.p("InTransform"), &transform(p, q, [1.; 3]))
                        .put(f.set_bone.p("BoneSpace"), &[COMPONENT_SPACE]);
                    f.call(vt, &f.set_bone, calculator, &mut args)?;
                }
                let mut reason = [0u8; 160];
                if publish(
                    calculator,
                    mesh,
                    puppet.count,
                    reason.as_mut_ptr(),
                    reason.len() as u32,
                ) != 1
                {
                    let end = reason.iter().position(|b| *b == 0).unwrap_or(reason.len());
                    return Err(String::from_utf8_lossy(&reason[..end]).into_owned());
                }
                for (slot, weapon) in puppet.weapons.iter().enumerate() {
                    let Some(weapon) = weapon else { continue };
                    if s.mask & (1 << (WPN_R + slot)) == 0 {
                        continue;
                    }
                    let Some(actor) = weapon.actor.live(vt) else {
                        continue;
                    };
                    let x = s.bones[WPN_R + slot];
                    let mut args = Args::new();
                    args.put(
                        f.actor.p("NewTransform"),
                        &transform(
                            [x[0], x[1], x[2]].map(f64::from),
                            [x[3], x[4], x[5], x[6]].map(f64::from),
                            weapon.scale,
                        ),
                    )
                    .flag(f.actor.p("bSweep"), false)
                    .flag(f.actor.p("bTeleport"), true);
                    f.call(vt, &f.actor, actor, &mut args)?;
                }
                if let (Some(controller), Some(control)) = (
                    puppet.controller,
                    s.extra.as_ref().and_then(|e| e.control.as_ref()),
                ) {
                    if let Some(pc) = controller.live(vt) {
                        let rotation = [control.ctrl_pitch as f64, control.ctrl_yaw as f64, 0.];
                        if rotation.iter().all(|v| v.is_finite()) {
                            f.call(
                                vt,
                                &f.control,
                                pc,
                                Args::new().put(f.control.p("NewRotation"), &vec_bytes(rotation)),
                            )?;
                        }
                    }
                }
                puppet.frames += 1;
            }
            for (prop, value) in [(puppet.health, feed.health), (puppet.stamina, feed.stamina)] {
                let Some(p) = prop else { continue };
                if !value.is_finite() {
                    continue;
                }
                let at = (pawn as *mut u8).add(p.offset as usize);
                if p.size == 4 {
                    std::ptr::write_unaligned(at as *mut f32, value as f32);
                } else {
                    std::ptr::write_unaligned(at as *mut f64, value);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_transform_layout_and_yaw() {
        let t = transform([1., 2., 3.], [0., 0., 0., 1.], [2.; 3]);
        assert_eq!(f64::from_le_bytes(t[24..32].try_into().unwrap()), 1.);
        assert_eq!(f64::from_le_bytes(t[32..40].try_into().unwrap()), 1.);
        assert_eq!(f64::from_le_bytes(t[48..56].try_into().unwrap()), 3.);
        assert_eq!(f64::from_le_bytes(t[56..64].try_into().unwrap()), 0.);
        assert_eq!(f64::from_le_bytes(t[80..88].try_into().unwrap()), 2.);
        let q = yaw_quat(90.);
        assert!(
            (q[2] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12
                && (q[3] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12
        );
    }
    #[test]
    fn args_bool_masks_only_touch_their_bit() {
        let p = HsmpProp {
            offset: 4,
            size: 1,
            bool_offset: 0,
            bool_mask: 0x04,
            ..Default::default()
        };
        let mut a = Args::new();
        a.0[4] = 0x01;
        a.flag(p, true);
        assert_eq!(a.0[4], 0x05);
        a.flag(p, false);
        assert_eq!(a.0[4], 0x01);
    }
}

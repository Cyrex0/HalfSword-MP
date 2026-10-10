//! Compact gameplay client provider. Source-owned immutable DTOs qualify every staged native call.
use crate::{
    lua::*,
    native::Native,
    native_presentation::{Guard, Object, ResultInfo, Text, Transform},
    reflect::{self, HsmpReflect},
};
use hsmp_server::{native_gameplay_wire as gp, native_service::GameplayScene, native_wire as w};
use std::{
    collections::HashMap,
    ffi::{c_int, c_void},
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Value {
    pub kind: u32,
    pub pad: u32,
    pub value: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NativeState {
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub velocity: [f64; 3],
    pub health: Value,
    pub stamina: Value,
}
#[repr(C)]
#[derive(Default)]
pub struct Proof {
    pub world: Object,
    pub pawn: Object,
    pub controller: Object,
    pub view_target: Object,
    pub camera_manager: Object,
    pub hud: Object,
    pub flags: u32,
    pub own: u32,
    pub visible_meshes: u32,
    pub pad: u32,
}
#[repr(C)]
pub struct Provider {
    pub abi: u32,
    pub pad: u32,
    pub begin: unsafe extern "C" fn(
        Object,
        Object,
        Text,
        *const Transform,
        u32,
        *const Guard,
        *mut u64,
        *mut Object,
        *mut ResultInfo,
    ) -> i32,
    pub current: unsafe extern "C" fn(u64, *const Guard, *mut Object, *mut ResultInfo) -> i32,
    pub construct: unsafe extern "C" fn(u64, *const Guard, *mut Object, *mut ResultInfo) -> i32,
    pub finish: unsafe extern "C" fn(u64, *const Guard, *mut ResultInfo) -> i32,
    pub apply: unsafe extern "C" fn(
        u64,
        *const NativeState,
        *const Guard,
        *mut Proof,
        *mut ResultInfo,
    ) -> i32,
    pub clear: unsafe extern "C" fn(u64, *const Guard, *mut ResultInfo) -> i32,
    pub discard: unsafe extern "C" fn(u64),
    pub complete:
        unsafe extern "C" fn(*const u64, u32, *const Guard, *mut Proof, *mut ResultInfo) -> i32,
}
static PROVIDER: AtomicPtr<Provider> = AtomicPtr::new(std::ptr::null_mut());
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_set_gameplay(p: *const Provider) {
    let admitted = if !p.is_null() && unsafe { (*p).abi } == 1 {
        p.cast_mut()
    } else {
        std::ptr::null_mut()
    };
    PROVIDER.store(admitted, Ordering::Release);
}
fn provider() -> Result<&'static Provider, String> {
    unsafe {
        PROVIDER
            .load(Ordering::Acquire)
            .as_ref()
            .ok_or_else(|| "native gameplay provider unavailable".into())
    }
}
struct Pawn {
    handle: u64,
    scene: GameplayScene,
    descriptor: Arc<gp::Bootstrap>,
    world: Object,
    controller: Object,
    own: bool,
    constructed: bool,
    finished: bool,
    original_pawn: Object,
}
#[derive(Default)]
pub(crate) struct State {
    pawns: HashMap<u32, Pawn>,
    offered: Option<GameplayScene>,
    applied: Option<GameplayScene>,
}
impl State {
    fn take_pawns(&mut self) -> HashMap<u32, Pawn> {
        // Core exports the new scope's scene before clearing old native pawns.
        // This authority DTO remains generation-qualified and keeps its original receipt.
        self.applied = None;
        std::mem::take(&mut self.pawns)
    }
    pub(crate) fn discard(&mut self) {
        self.offered = None;
        self.applied = None;
        if let Ok(p) = provider() {
            for (_, pawn) in self.pawns.drain() {
                unsafe { (p.discard)(pawn.handle) }
            }
        } else {
            self.pawns.clear();
        }
    }
}
struct Context {
    native: *const Native,
    vt: *const HsmpReflect,
    scene: GameplayScene,
    world: Object,
    controller: Object,
    key: Vec<u8>,
    cleanup: bool,
    application: bool,
}
impl Context {
    fn new(
        n: &Native,
        vt: &HsmpReflect,
        scene: GameplayScene,
        world: Object,
        controller: Object,
    ) -> Result<Self, String> {
        Ok(Self {
            native: n,
            vt,
            scene,
            world,
            controller,
            key: n
                .world_key
                .clone()
                .ok_or("gameplay world token unavailable")?,
            cleanup: false,
            application: false,
        })
    }
    unsafe fn valid(&self) -> bool {
        unsafe {
            let n = &*self.native;
            let vt = &*self.vt;
            if n.poisoned
                || !n.native_host.is_client()
                || !n.sample.world_ok
                || n.game_thread != Some(std::thread::current().id())
                || n.world_key.as_ref() != Some(&self.key)
                || !n.native_guard_ok(vt)
                || reflect::get(vt, self.world.weak) as u64 != self.world.address
                || reflect::get(vt, self.controller.weak) as u64 != self.controller.address
            {
                return false;
            }
            if self.cleanup {
                return true;
            }
            let Some(scene) = n
                .native_host
                .client
                .as_ref()
                .and_then(|c| c.gameplay_scene())
            else {
                return false;
            };
            same_generation(&self.scene, &scene) && (!self.application || self.scene.fresh())
        }
    }
    fn ffi(&mut self) -> Guard {
        Guard {
            context: (self as *mut Self).cast(),
            check: check,
        }
    }
}
fn same_generation(a: &GameplayScene, b: &GameplayScene) -> bool {
    a.peer_id == b.peer_id
        && a.directory.epoch == b.directory.epoch
        && a.directory.seq == b.directory.seq
        && a.directory.entities == b.directory.entities
        && a.descriptors.len() == b.descriptors.len()
        && a.descriptors.iter().zip(&b.descriptors).all(|(x, y)| {
            Arc::ptr_eq(x, y) && x.reference == y.reference && x.revision == y.revision
        })
}
unsafe extern "C" fn check(p: *mut c_void) -> i32 {
    if p.is_null() {
        return 0;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        (&*p.cast::<Context>()).valid() as i32
    }))
    .unwrap_or(0)
}
unsafe fn object(vt: &HsmpReflect, address: i64) -> Result<Object, String> {
    unsafe {
        if address <= 0 {
            return Err("gameplay original object unavailable".into());
        }
        let pointer = address as usize as *mut c_void;
        let weak = reflect::keep(vt, pointer).ok_or("gameplay weak unavailable")?;
        if reflect::get(vt, weak) != pointer {
            return Err("gameplay original object changed".into());
        }
        Ok(Object {
            weak,
            address: address as u64,
        })
    }
}
unsafe fn int(L: *mut lua_State, t: c_int, key: &str) -> Option<i64> {
    unsafe {
        rawget_str(L, t, key);
        let v = arg_int(L, -1);
        pop(L, 1);
        v
    }
}
fn offered_scene(
    offered: Option<&GameplayScene>,
    current: &GameplayScene,
    requested: (u64, u32, u32),
    application: bool,
) -> Result<GameplayScene, String> {
    let original = offered.ok_or("native gameplay result lease unavailable")?;
    if requested
        != (
            original.directory.epoch,
            original.directory.seq,
            original.result.authority_tick,
        )
        || !same_generation(original, current)
        || current.result.authority_tick < original.result.authority_tick
        || current.received < original.received
    {
        return Err("native gameplay result generation changed".into());
    }
    if application && !original.fresh() {
        return Err("native gameplay result is stale".into());
    }
    Ok(original.clone())
}
unsafe fn scene(n: &Native, L: *mut lua_State, application: bool) -> Result<GameplayScene, String> {
    unsafe {
        let current = n
            .native_host
            .client
            .as_ref()
            .ok_or("not a native gameplay client")?
            .gameplay_scene()
            .ok_or("no complete native gameplay result")?;
        let requested = is_table(L, 1)
            .then(|| {
                Some((
                    int(L, 1, "epoch")? as u64,
                    u32::try_from(int(L, 1, "dir_seq")?).ok()?,
                    u32::try_from(int(L, 1, "authority_tick")?).ok()?,
                ))
            })
            .flatten();
        let Some(requested) = requested else {
            return Err("native gameplay result generation changed".into());
        };
        offered_scene(
            n.native_host.gameplay.offered.as_ref(),
            &current,
            requested,
            application,
        )
    }
}
unsafe fn json(L: *mut lua_State, v: &serde_json::Value) -> Result<(), String> {
    unsafe {
        if lua_checkstack(L, 8) == 0 {
            return Err("gameplay recipe stack capacity".into());
        }
        match v {
            serde_json::Value::Null => lua_pushboolean(L, 0),
            serde_json::Value::Bool(v) => lua_pushboolean(L, *v as i32),
            serde_json::Value::Number(v) => {
                if let Some(i) = v.as_i64() {
                    lua_pushinteger(L, i)
                } else {
                    lua_pushnumber(L, v.as_f64().ok_or("gameplay recipe number")?)
                }
            }
            serde_json::Value::String(s) => push_str(L, s),
            serde_json::Value::Array(a) => {
                lua_createtable(L, a.len() as i32, 0);
                let t = lua_gettop(L);
                for (i, v) in a.iter().enumerate() {
                    json(L, v)?;
                    lua_rawseti(L, t, i as i64 + 1)
                }
            }
            serde_json::Value::Object(o) => {
                lua_createtable(L, 0, o.len() as i32);
                let t = lua_gettop(L);
                for (k, v) in o {
                    json(L, v)?;
                    rawset_str(L, t, k);
                }
            }
        }
        Ok(())
    }
}
unsafe fn array(L: *mut lua_State, t: c_int, name: &str, v: impl IntoIterator<Item = f64>) {
    unsafe {
        lua_createtable(L, 0, 0);
        let a = lua_gettop(L);
        fill_array(L, a, v.into_iter());
        rawset_str(L, t, name)
    }
}
fn native_value(v: gp::NativeScalar) -> Value {
    Value {
        kind: if matches!(v, gp::NativeScalar::F32(_)) {
            1
        } else {
            2
        },
        pad: 0,
        value: v.value(),
    }
}
unsafe fn push_scene(L: *mut lua_State, s: &GameplayScene, proved: bool) -> Result<(), String> {
    unsafe {
        lua_createtable(L, 0, 13);
        let out = lua_gettop(L);
        set_int(L, out, "epoch", s.directory.epoch as i64);
        set_int(L, out, "dir_seq", s.directory.seq as i64);
        set_int(L, out, "authority_tick", s.result.authority_tick as i64);
        set_int(L, out, "frame_seq", s.result.authority_tick as i64);
        set_int(L, out, "state", s.directory.state as i64);
        set_int(L, out, "peer_id", s.peer_id as i64);
        set_bool(L, out, "fresh", s.fresh());
        set_bool(L, out, "gameplay_proof", proved);
        set_num(
            L,
            out,
            "received_age_ms",
            s.received.elapsed().as_secs_f64() * 1000.,
        );
        set_str(L, out, "arena", &s.directory.arena);
        set_str(
            L,
            out,
            "generation",
            &format!(
                "{}:{}:{:?}",
                s.directory.epoch,
                s.directory.seq,
                s.descriptors
                    .iter()
                    .map(|d| (d.reference.id, d.reference.incarnation, d.revision))
                    .collect::<Vec<_>>()
            ),
        );
        lua_createtable(L, s.directory.entities.len() as i32, 0);
        let rows = lua_gettop(L);
        for (i, e) in s.directory.entities.iter().enumerate() {
            let d = s
                .descriptors
                .iter()
                .find(|d| d.reference == e.reference)
                .ok_or("gameplay bootstrap descriptor")?;
            let v = s
                .result
                .entities
                .iter()
                .find(|v| v.reference == e.reference)
                .ok_or("gameplay result row")?;
            lua_createtable(L, 0, 20);
            let r = lua_gettop(L);
            set_int(L, r, "epoch", e.reference.epoch as i64);
            set_int(L, r, "id", e.reference.id as i64);
            set_int(L, r, "incarnation", e.reference.incarnation as i64);
            set_int(L, r, "owner_peer", e.owner_peer as i64);
            set_int(L, r, "slot", e.slot as i64);
            set_int(L, r, "kind", e.kind as i64);
            set_int(L, r, "controller", e.controller as i64);
            set_int(L, r, "revision", d.revision as i64);
            set_int(L, r, "request_seq", v.request_seq as i64);
            set_int(L, r, "delivery_seq", v.delivery_seq as i64);
            set_int(L, r, "buttons", v.buttons as i64);
            array(L, r, "axes", v.axes.map(f64::from));
            array(L, r, "position", v.position);
            array(L, r, "rotation", v.rotation);
            array(L, r, "velocity", v.velocity);
            for (name, value) in [("health", v.health), ("stamina", v.stamina)] {
                lua_createtable(L, 0, 2);
                let t = lua_gettop(L);
                set_str(
                    L,
                    t,
                    "kind",
                    if matches!(value, gp::NativeScalar::F32(_)) {
                        "f32"
                    } else {
                        "f64"
                    },
                );
                set_num(L, t, "value", value.value());
                rawset_str(L, r, name);
            }
            json(
                L,
                &serde_json::to_value(&d.recipe).map_err(|_| "gameplay bootstrap encoding")?,
            )?;
            rawset_str(L, r, "recipe");
            lua_rawseti(L, rows, i as i64 + 1);
        }
        rawset_str(L, out, "entities");
        Ok(())
    }
}
unsafe fn wrapper(
    L: *mut lua_State,
    p: &Provider,
    handle: u64,
    context: &mut Context,
    pawn: Object,
) -> Result<(), String> {
    unsafe {
        if !context.valid() {
            return Err("gameplay wrapper generation changed".into());
        }
        let before = lua_gettop(L);
        let mut reason = [0u8; 192];
        let factory = crate::native_source_scope::object_factory()?;
        if factory(L, pawn.address, reason.as_mut_ptr().cast(), 192) != 1 {
            let len = reason.iter().position(|v| *v == 0).unwrap_or(192);
            return Err(format!(
                "gameplay wrapper: {}",
                String::from_utf8_lossy(&reason[..len])
            ));
        }
        if lua_gettop(L) != before + 1 || lua_type(L, -1) != 7 || !context.valid() {
            return Err("gameplay wrapper stack or generation".into());
        }
        let mut current = Object::default();
        let mut r = ResultInfo::default();
        if (p.current)(handle, &context.ffi(), &mut current, &mut r) != 1
            || r.complete != 1
            || current.address != pawn.address
            || !context.valid()
        {
            return Err(format!("gameplay wrapper final original: {}", r.reason()));
        }
        Ok(())
    }
}
impl Native {
    pub unsafe fn native_gameplay_metrics(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let metrics = if let Some(host) = self.native_host.host.as_ref() {
                host.compression_metrics()
            } else if let Some(client) = self.native_host.client.as_ref() {
                client.compression_metrics()
            } else {
                return nil_err(L, "native gameplay transport absent");
            };
            lua_createtable(L, 0, 8);
            let t = lua_gettop(L);
            for (k, v) in [
                ("encode_samples", metrics.encode_samples as u64),
                ("raw_bytes", metrics.raw_bytes),
                ("wire_bytes", metrics.wire_bytes),
                ("encode_us", metrics.encode_us),
                ("decode_samples", metrics.decode_samples as u64),
                ("decode_raw_bytes", metrics.decode_raw_bytes),
                ("decode_wire_bytes", metrics.decode_wire_bytes),
                ("decode_us", metrics.decode_us),
            ] {
                set_int(L, t, k, v as i64);
            }
            1
        }
    }
    pub unsafe fn native_gameplay_scene(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            self.native_host.gameplay.offered = None;
            let Some(s) = self
                .native_host
                .client
                .as_ref()
                .and_then(|c| c.gameplay_scene())
            else {
                lua_pushnil(L);
                return 1;
            };
            match push_scene(L, &s, false) {
                Ok(()) => {
                    // Retain exactly the result returned to Lua, including its network receipt.
                    self.native_host.gameplay.offered = Some(s);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    /// begin(scene, originalWorldAddress, originalLocalPCAddress, entityId) -> handle,freshPawn.
    pub unsafe fn native_gameplay_begin(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let top = lua_gettop(L);
            let result = (|| -> Result<(), String> {
                let s = scene(self, L, false)?;
                let id = u32::try_from(arg_int(L, 4).ok_or("gameplay entity id")?)
                    .map_err(|_| "gameplay entity id")?;
                if self.native_host.gameplay.pawns.contains_key(&id) {
                    return Err("gameplay pawn already begun".into());
                }
                let e = s
                    .directory
                    .entities
                    .iter()
                    .find(|e| e.reference.id == id)
                    .ok_or("gameplay entity absent")?;
                let descriptor = s
                    .descriptors
                    .iter()
                    .find(|d| d.reference == e.reference)
                    .cloned()
                    .ok_or("gameplay original recipe")?;
                let own = e.owner_peer == s.peer_id && e.kind == w::HUMAN;
                let state = s
                    .result
                    .entities
                    .iter()
                    .find(|v| v.reference == e.reference)
                    .ok_or("gameplay state absent")?;
                let vt = reflect::vt().ok_or("gameplay reflection unavailable")?;
                let world = object(vt, arg_int(L, 2).ok_or("gameplay world")?)?;
                let controller = object(vt, arg_int(L, 3).ok_or("gameplay local PC")?)?;
                let mut context = Context::new(self, vt, s.clone(), world, controller)?;
                if !context.valid() {
                    return Err("gameplay original admission".into());
                }
                let p = provider()?;
                let class = reflect::wide(&descriptor.recipe.actor_class);
                let text = Text {
                    data: class.as_ptr(),
                    len: (class.len() - 1) as u32,
                    pad: 0,
                };
                let rad = state.rotation.map(f64::to_radians);
                let (sp, cp) = (rad[0] * 0.5).sin_cos();
                let (sy, cy) = (rad[1] * 0.5).sin_cos();
                let (sr, cr) = (rad[2] * 0.5).sin_cos();
                let transform = Transform {
                    p: state.position,
                    q: [
                        cr * sp * sy - sr * cp * cy,
                        -cr * sp * cy - sr * cp * sy,
                        cr * cp * sy - sr * sp * cy,
                        cr * cp * cy + sr * sp * sy,
                    ],
                    scale: descriptor.recipe.construction.actor_scale,
                };
                let mut handle = 0;
                let mut pawn = Object::default();
                let mut r = ResultInfo::default();
                if (p.begin)(
                    world,
                    controller,
                    text,
                    &transform,
                    own as u32,
                    &context.ffi(),
                    &mut handle,
                    &mut pawn,
                    &mut r,
                ) != 1
                    || handle == 0
                    || r.complete != 1
                    || !context.valid()
                {
                    if handle != 0 {
                        (p.discard)(handle);
                    }
                    return Err(format!("gameplay begin: {}", r.reason()));
                }
                lua_pushinteger(L, handle as i64);
                if let Err(e) = wrapper(L, p, handle, &mut context, pawn) {
                    (p.discard)(handle);
                    return Err(e);
                }
                self.native_host.gameplay.pawns.insert(
                    id,
                    Pawn {
                        handle,
                        scene: s,
                        descriptor,
                        world,
                        controller,
                        own,
                        constructed: false,
                        finished: false,
                        original_pawn: pawn,
                    },
                );
                Ok(())
            })();
            match result {
                Ok(()) => 2,
                Err(e) => {
                    lua_settop(L, top);
                    nil_err(L, &e)
                }
            }
        }
    }
    pub unsafe fn native_gameplay_current(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.gameplay_stage(L, 0) }
    }
    pub unsafe fn native_gameplay_construct(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.gameplay_stage(L, 1) }
    }
    pub unsafe fn native_gameplay_finish(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.gameplay_stage(L, 2) }
    }
    unsafe fn gameplay_stage(&mut self, L: *mut lua_State, stage: u8) -> c_int {
        unsafe {
            let top = lua_gettop(L);
            let result = (|| -> Result<(), String> {
                let handle = arg_int(L, 1).filter(|v| *v > 0).ok_or("gameplay handle")? as u64;
                let pawn = self
                    .native_host
                    .gameplay
                    .pawns
                    .values()
                    .find(|p| p.handle == handle)
                    .ok_or("gameplay original handle")?;
                if stage == 1 && pawn.constructed
                    || stage == 2 && (!pawn.constructed || pawn.finished)
                {
                    return Err("gameplay construction order").map_err(str::to_owned);
                }
                let vt = reflect::vt().ok_or("gameplay reflection")?;
                let mut context =
                    Context::new(self, vt, pawn.scene.clone(), pawn.world, pawn.controller)?;
                if !context.valid() {
                    return Err("gameplay staged generation changed".into());
                }
                let p = provider()?;
                let mut object = Object::default();
                let mut r = ResultInfo::default();
                let ok = match stage {
                    0 => (p.current)(handle, &context.ffi(), &mut object, &mut r),
                    1 => (p.construct)(handle, &context.ffi(), &mut object, &mut r),
                    _ => (p.finish)(handle, &context.ffi(), &mut r),
                };
                if ok != 1 || r.complete != 1 || !context.valid() {
                    return Err(format!("gameplay stage: {}", r.reason()));
                }
                if stage != 0 {
                    lua_pushboolean(L, 1);
                }
                if stage != 2 {
                    wrapper(L, p, handle, &mut context, object)?;
                }
                if let Some(pawn) = self
                    .native_host
                    .gameplay
                    .pawns
                    .values_mut()
                    .find(|p| p.handle == handle)
                {
                    if stage == 1 {
                        pawn.constructed = true;
                    }
                    if stage == 2 {
                        pawn.finished = true;
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    if stage == 1 {
                        2
                    } else {
                        1
                    }
                }
                Err(e) => {
                    lua_settop(L, top);
                    nil_err(L, &e)
                }
            }
        }
    }
    pub unsafe fn native_gameplay_apply(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let top = lua_gettop(L);
            let result = (|| -> Result<GameplayScene, String> {
                let s = scene(self, L, true)?;
                if self.native_host.gameplay.pawns.len() != s.directory.entities.len() {
                    return Err("native gameplay pawn set incomplete".into());
                }
                let vt = reflect::vt().ok_or("gameplay reflection")?;
                let p = provider()?;
                let mut handles = Vec::with_capacity(s.result.entities.len());
                let mut ownership = Vec::with_capacity(s.result.entities.len());
                for row in &s.result.entities {
                    let pawn = self
                        .native_host
                        .gameplay
                        .pawns
                        .get(&row.reference.id)
                        .ok_or("gameplay original pawn missing")?;
                    if !pawn.finished
                        || pawn.descriptor.reference != row.reference
                        || !same_generation(&pawn.scene, &s)
                    {
                        return Err("gameplay pawn generation changed".into());
                    }
                    let mut context =
                        Context::new(self, vt, s.clone(), pawn.world, pawn.controller)?;
                    context.application = true;
                    if !context.valid() {
                        return Err("gameplay apply admission changed".into());
                    }
                    let state = NativeState {
                        position: row.position,
                        rotation: row.rotation,
                        velocity: row.velocity,
                        health: native_value(row.health),
                        stamina: native_value(row.stamina),
                    };
                    let mut proof = Proof::default();
                    let mut r = ResultInfo::default();
                    if (p.apply)(pawn.handle, &state, &context.ffi(), &mut proof, &mut r) != 1
                        || r.complete != 1
                        || !context.valid()
                    {
                        let reason = r.reason();
                        return Err(
                            if matches!(
                                reason.as_str(),
                                "native gameplay own native view target pending"
                                    | "native gameplay own camera manager pending"
                            ) {
                                reason
                            } else {
                                format!("native gameplay apply: {reason}")
                            },
                        );
                    }
                    let required = if pawn.own { 63 } else { 3 };
                    if proof.flags & required != required
                        || proof.own != pawn.own as u32
                        || proof.visible_meshes == 0
                        || proof.world.address != pawn.world.address
                    {
                        return Err("native gameplay readiness proof incomplete".into());
                    }
                    handles.push(pawn.handle);
                    ownership.push((pawn.own, pawn.world, pawn.controller, pawn.original_pawn));
                }
                let first = self
                    .native_host
                    .gameplay
                    .pawns
                    .get(&s.result.entities[0].reference.id)
                    .ok_or("gameplay full set original")?;
                let mut context = Context::new(self, vt, s.clone(), first.world, first.controller)?;
                context.application = true;
                if !context.valid() {
                    return Err("gameplay full set admission changed".into());
                }
                let mut proofs = (0..handles.len())
                    .map(|_| Proof::default())
                    .collect::<Vec<_>>();
                let mut complete = ResultInfo::default();
                if (p.complete)(
                    handles.as_ptr(),
                    handles.len() as u32,
                    &context.ffi(),
                    proofs.as_mut_ptr(),
                    &mut complete,
                ) != 1
                    || complete.complete != 1
                    || !context.valid()
                {
                    return Err(format!("gameplay full set proof: {}", complete.reason()));
                }
                for (proof, (own, world, controller, original)) in proofs.iter().zip(ownership) {
                    let required = if own { 63 } else { 3 };
                    if proof.flags & required != required
                        || proof.own != own as u32
                        || proof.visible_meshes == 0
                        || proof.world.address != world.address
                        || proof.pawn.address != original.address
                        || (own && proof.controller.address != controller.address)
                    {
                        return Err("gameplay full set identity/readiness proof incomplete".into());
                    }
                }
                if !s.fresh() {
                    return Err("native gameplay result is stale".into());
                }
                // Lua's native equipment verification runs next. No network readiness
                // or input proof is published until confirm closes those callbacks.
                self.native_host.gameplay.applied = Some(s.clone());
                Ok(s)
            })();
            match result {
                Ok(s) => {
                    lua_pushboolean(L, 1);
                    match push_scene(L, &s, true) {
                        Ok(()) => 2,
                        Err(e) => {
                            lua_settop(L, top);
                            nil_err(L, &e)
                        }
                    }
                }
                Err(e) => {
                    lua_settop(L, top);
                    nil_err(L, &e)
                }
            }
        }
    }
    pub unsafe fn native_gameplay_confirm(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let top = lua_gettop(L);
            let result = (|| -> Result<GameplayScene, String> {
                let original = self
                    .native_host
                    .gameplay
                    .applied
                    .take()
                    .ok_or("no pending native gameplay apply")?;
                if !is_table(L, 1)
                    || int(L, 1, "epoch").map(|v| v as u64) != Some(original.directory.epoch)
                    || int(L, 1, "dir_seq") != Some(original.directory.seq as i64)
                    || int(L, 1, "authority_tick") != Some(original.result.authority_tick as i64)
                {
                    return Err("native gameplay confirmation result changed".into());
                }
                if !original.fresh() {
                    return Err("native gameplay result is stale".into());
                }
                let vt = reflect::vt().ok_or("gameplay reflection unavailable")?;
                let p = provider()?;
                let mut handles = Vec::with_capacity(original.result.entities.len());
                let mut expected = Vec::with_capacity(original.result.entities.len());
                for row in &original.result.entities {
                    let pawn = self
                        .native_host
                        .gameplay
                        .pawns
                        .get(&row.reference.id)
                        .ok_or("gameplay confirmation pawn missing")?;
                    if !pawn.finished
                        || pawn.descriptor.reference != row.reference
                        || !same_generation(&pawn.scene, &original)
                    {
                        return Err("gameplay confirmation original generation changed".into());
                    }
                    handles.push(pawn.handle);
                    expected.push((pawn.own, pawn.world, pawn.controller, pawn.original_pawn));
                }
                let first = expected
                    .first()
                    .ok_or("gameplay confirmation empty roster")?;
                let mut context = Context::new(self, vt, original.clone(), first.1, first.2)?;
                context.application = true;
                if !context.valid() {
                    return Err("gameplay confirmation admission changed".into());
                }
                let mut proofs = (0..handles.len())
                    .map(|_| Proof::default())
                    .collect::<Vec<_>>();
                let mut r = ResultInfo::default();
                if (p.complete)(
                    handles.as_ptr(),
                    handles.len() as u32,
                    &context.ffi(),
                    proofs.as_mut_ptr(),
                    &mut r,
                ) != 1
                    || r.complete != 1
                    || !context.valid()
                {
                    return Err(format!(
                        "gameplay confirmation full native proof: {}",
                        r.reason()
                    ));
                }
                for (proof, (own, world, controller, pawn)) in proofs.iter().zip(expected) {
                    let required = if own { 63 } else { 3 };
                    if proof.flags & required != required
                        || proof.own != own as u32
                        || proof.visible_meshes == 0
                        || proof.world.address != world.address
                        || proof.pawn.address != pawn.address
                        || (own && proof.controller.address != controller.address)
                    {
                        return Err("gameplay confirmation readiness identity changed".into());
                    }
                }
                self.native_host
                    .client
                    .as_ref()
                    .ok_or("gameplay client missing")?
                    .gameplay_applied(&original)
                    .map_err(str::to_owned)?;
                Ok(original)
            })();
            match result {
                Ok(s) => {
                    lua_pushboolean(L, 1);
                    match push_scene(L, &s, true) {
                        Ok(()) => 2,
                        Err(e) => {
                            lua_settop(L, top);
                            nil_err(L, &e)
                        }
                    }
                }
                Err(e) => {
                    lua_settop(L, top);
                    nil_err(L, &e)
                }
            }
        }
    }
    pub unsafe fn native_gameplay_clear(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if lua_gettop(L) > 0 && lua_type(L, 1) != LUA_TBOOLEAN {
                return nil_err(L, "gameplay clear boolean required");
            }
            if lua_gettop(L) > 0 && lua_toboolean(L, 1) != 0 {
                self.native_host.gameplay.discard();
                lua_pushboolean(L, 1);
                return 1;
            }
            let p = match provider() {
                Ok(p) => p,
                Err(e) => {
                    self.native_host.gameplay.pawns.clear();
                    return nil_err(L, &e);
                }
            };
            let pawns = self.native_host.gameplay.take_pawns();
            for (_, pawn) in pawns {
                let vt = reflect::vt();
                let context = vt.and_then(|vt| {
                    Context::new(self, vt, pawn.scene, pawn.world, pawn.controller).ok()
                });
                if let Some(mut context) = context {
                    context.cleanup = true;
                    if context.valid() {
                        let mut r = ResultInfo::default();
                        if (p.clear)(pawn.handle, &context.ffi(), &mut r) != 1 || r.complete != 1 {
                            (p.discard)(pawn.handle);
                            continue;
                        }
                    } else {
                        (p.discard)(pawn.handle);
                    }
                } else {
                    (p.discard)(pawn.handle);
                }
            }
            lua_pushboolean(L, 1);
            1
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn scene_fixture() -> GameplayScene {
        let source =
            serde_json::from_slice::<hsmp_server::native_descriptor::SourceRecipe>(include_bytes!(
                "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
            ))
            .unwrap();
        let recipe = gp::Recipe {
            schema: gp::RECIPE_SCHEMA,
            actor_class: source.actor_class,
            team: source.team,
            passport: source.passport,
            construction: source.construction,
            equipment: gp::Equipment {
                armor: source.equipment.armor,
                weapons: source
                    .equipment
                    .weapons
                    .into_iter()
                    .map(|weapon| gp::Weapon {
                        id: weapon.id,
                        actor_class: weapon.actor_class,
                        passport: weapon.passport,
                    })
                    .collect(),
                hands: source.equipment.hands,
                sheaths: source.equipment.sheaths,
            },
        };
        recipe.validate().unwrap();
        let epoch = (1u64 << 63) + 7;
        let entities = (0..2)
            .map(|slot| w::Entity {
                reference: w::EntityRef {
                    epoch,
                    id: slot as u32 + 1,
                    incarnation: 5,
                },
                owner_peer: 9001 + slot as u32,
                slot,
                kind: w::HUMAN,
                controller: 0,
                team: Some(recipe.team),
            })
            .collect::<Vec<_>>();
        let result = gp::ResultFrame {
            epoch,
            directory_seq: 7,
            authority_tick: 367,
            entities: entities
                .iter()
                .map(|entity| gp::State {
                    reference: entity.reference,
                    request_seq: 1,
                    delivery_seq: 1,
                    buttons: 1,
                    axes: [1., 0., 0., 0., 0., 0., 0., 0.],
                    position: [1.0000000000000002, 20., 30.],
                    rotation: [0., 90., 0.],
                    velocity: [1., 2., 3.],
                    health: gp::NativeScalar::F32(100f32.to_bits()),
                    stamina: gp::NativeScalar::F64(99f64.to_bits()),
                })
                .collect(),
        };
        result.validate().unwrap();
        GameplayScene {
            descriptors: entities
                .iter()
                .map(|entity| {
                    Arc::new(gp::Bootstrap {
                        reference: entity.reference,
                        slot: entity.slot,
                        directory_seq: 7,
                        revision: 1,
                        source_frame_seq: 1,
                        recipe: recipe.clone(),
                    })
                })
                .collect(),
            directory: w::Directory {
                epoch,
                seq: 7,
                state: w::READY,
                arena: "Native test".into(),
                error: String::new(),
                entities,
            },
            result: Arc::new(result),
            peer_id: 9001,
            received: Instant::now()
                .checked_sub(Duration::from_millis(50))
                .unwrap(),
        }
    }
    fn request(scene: &GameplayScene) -> (u64, u32, u32) {
        (
            scene.directory.epoch as i64 as u64,
            scene.directory.seq,
            scene.result.authority_tick,
        )
    }
    fn newer(scene: &GameplayScene) -> GameplayScene {
        let mut next = scene.clone();
        let mut result = (*next.result).clone();
        result.authority_tick += 1;
        result.entities[0].position[0] = 200.;
        next.result = Arc::new(result);
        next.received = Instant::now();
        next
    }
    #[test]
    fn offered_result_survives_newer_tick_without_replacing_arc_or_receipt() {
        let original = scene_fixture();
        let next = newer(&original);
        let acquired = offered_scene(Some(&original), &next, request(&original), true).unwrap();
        assert!(Arc::ptr_eq(&acquired.result, &original.result));
        assert!(!Arc::ptr_eq(&acquired.result, &next.result));
        assert_eq!(acquired.received, original.received);
        assert_eq!(acquired.result.authority_tick, 367);
        assert_eq!(
            acquired.result.entities[0].position[0].to_bits(),
            1.0000000000000002f64.to_bits()
        );
        assert!(acquired
            .descriptors
            .iter()
            .zip(&original.descriptors)
            .all(|(a, b)| Arc::ptr_eq(a, b)));
    }
    #[test]
    fn new_scope_cleanup_preserves_exported_result_before_first_begin() {
        let original = scene_fixture();
        let next = newer(&original);
        let mut state = State {
            offered: Some(original.clone()),
            applied: Some(original.clone()),
            ..Default::default()
        };
        assert!(state.take_pawns().is_empty());
        assert!(state.applied.is_none());
        let acquired =
            offered_scene(state.offered.as_ref(), &next, request(&original), false).unwrap();
        assert!(Arc::ptr_eq(&acquired.result, &original.result));
        assert_eq!(acquired.received, original.received);
        state.discard();
        assert!(state.offered.is_none());
    }
    #[test]
    fn newer_result_does_not_renew_expired_offered_receipt() {
        let mut original = scene_fixture();
        original.received = Instant::now()
            .checked_sub(Duration::from_millis(300))
            .unwrap();
        let next = newer(&original);
        assert!(next.fresh());
        assert_eq!(
            offered_scene(Some(&original), &next, request(&original), true)
                .err()
                .as_deref(),
            Some("native gameplay result is stale")
        );
        // Cold bootstrap remains generation-bound rather than input-age-bound.
        let cold = offered_scene(Some(&original), &next, request(&original), false).unwrap();
        assert_eq!(cold.received, original.received);
        assert!(!cold.fresh());
    }
    #[test]
    fn offered_result_refuses_forged_tuple_replaced_lease_or_true_generation_change() {
        let original = scene_fixture();
        let next = newer(&original);
        assert!(offered_scene(None, &next, request(&original), true).is_err());
        for key in [
            (original.directory.epoch ^ 1, 7, 367),
            (original.directory.epoch, 8, 367),
            request(&next),
        ] {
            assert!(offered_scene(Some(&original), &next, key, true).is_err());
        }
        assert!(offered_scene(Some(&next), &next, request(&original), true).is_err());
        let mut changed = next.clone();
        changed.directory.seq += 1;
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = next.clone();
        changed.directory.entities[1].reference.incarnation += 1;
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = next.clone();
        changed.descriptors[1] = Arc::new((*changed.descriptors[1]).clone());
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = next.clone();
        Arc::make_mut(&mut changed.descriptors[1]).revision += 1;
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = next.clone();
        changed.peer_id += 1;
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = original.clone();
        Arc::make_mut(&mut changed.result).authority_tick -= 1;
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
        changed = next;
        changed.received = original
            .received
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        assert!(offered_scene(Some(&original), &changed, request(&original), true).is_err());
    }
    #[test]
    fn private_provider_layout() {
        assert_eq!(std::mem::size_of::<Value>(), 16);
        assert_eq!(std::mem::size_of::<NativeState>(), 104);
        assert_eq!(std::mem::size_of::<Proof>(), 112);
        assert_eq!(std::mem::size_of::<Provider>(), 72);
        assert_eq!(std::mem::offset_of!(Provider, complete), 64);
        assert_eq!(std::mem::offset_of!(Provider, clear), 48);
        assert_eq!(std::mem::offset_of!(Provider, discard), 56);
    }
    #[test]
    fn exact_scalar_type_is_preserved() {
        let value = gp::NativeScalar::F32((-0f32).to_bits());
        let raw = native_value(value);
        assert_eq!(raw.kind, 1);
        assert_eq!((raw.value as f32).to_bits(), (-0f32).to_bits());
        assert_eq!(
            native_value(gp::NativeScalar::F64(1.0000000000000002f64.to_bits()))
                .value
                .to_bits(),
            1.0000000000000002f64.to_bits()
        );
    }
}

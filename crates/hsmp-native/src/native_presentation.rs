//! Native scene orchestration. Socket threads see DTOs; the provider receives
//! borrowed, qualified game-thread objects and caller-owned bounded arrays.
use crate::{
    lua::*,
    native::Native,
    native_descriptor_binding::read_recipe,
    reflect::{self, HsmpReflect},
};
use hsmp_server::{native_descriptor as d, native_wire as w};
use std::{
    collections::HashMap,
    ffi::{c_int, c_void},
    sync::atomic::{AtomicPtr, Ordering},
};
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Object {
    pub weak: u64,
    pub address: u64,
}
#[repr(C)]
pub struct Guard {
    pub context: *mut c_void,
    pub check: unsafe extern "C" fn(*mut c_void) -> i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Text {
    pub data: *const u16,
    pub len: u32,
    pub pad: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Transform {
    pub p: [f64; 3],
    pub q: [f64; 4],
    pub scale: [f64; 3],
}
#[repr(C)]
pub struct Parameter {
    pub name: Text,
    pub association: u32,
    pub index: i32,
}
#[repr(C)]
pub struct Material {
    pub slot: u32,
    pub pad: u32,
    pub base: Text,
    pub scalars: *const Parameter,
    pub scalar_count: u32,
    pub pad_s: u32,
    pub vectors: *const Parameter,
    pub vector_count: u32,
    pub pad_v: u32,
    pub textures: *const Parameter,
    pub texture_count: u32,
    pub pad_t: u32,
}
#[repr(C)]
pub struct VertexLod {
    pub lod: u32,
    pub count: u32,
    pub rgba: *const u8,
    pub bytes: u32,
    pub pad: u32,
}
#[repr(C)]
pub struct Component {
    pub id: u32,
    pub parent: u32,
    pub kind: u32,
    pub visible: u32,
    pub asset: Text,
    pub skeleton: Text,
    pub socket: Text,
    pub relative: Transform,
    pub bones: *const Text,
    pub bone_count: u32,
    pub pad_b: u32,
    pub morphs: *const Text,
    pub morph_count: u32,
    pub pad_m: u32,
    pub hidden_bones: *const Text,
    pub hidden_count: u32,
    pub pad_h: u32,
    pub materials: *const Material,
    pub material_count: u32,
    pub pad_mat: u32,
    pub vertex_state: u32,
    pub pad_vertex: u32,
    pub vertex_lods: *const VertexLod,
    pub vertex_count: u32,
    pub pad_lod: u32,
}
#[repr(C)]
pub struct Frame {
    pub world: Transform,
    pub bones: *mut Transform,
    pub bone_count: u32,
    pub pad_b: u32,
    pub morphs: *mut f32,
    pub morph_count: u32,
    pub pad_m: u32,
    pub scalars: *mut f32,
    pub scalar_count: u32,
    pub pad_s: u32,
    pub vectors: *mut f32,
    pub vector_count: u32,
    pub pad_v: u32,
    pub textures: *mut Object,
    pub texture_count: u32,
    pub pad_t: u32,
}
#[repr(C)]
pub struct ResultInfo {
    pub complete: u32,
    pub operations: u32,
    pub reason: [u8; 192],
}
impl Default for ResultInfo {
    fn default() -> Self {
        Self {
            complete: 0,
            operations: 0,
            reason: [0; 192],
        }
    }
}
impl ResultInfo {
    fn reason(&self) -> String {
        let n = self.reason.iter().position(|x| *x == 0).unwrap_or(192);
        String::from_utf8_lossy(&self.reason[..n]).into_owned()
    }
}
#[repr(C)]
#[derive(Default)]
pub struct Lifecycle {
    pub known: u32,
    pub flags: u32,
    pub destroying: u32,
    pub listed: u32,
    pub authority: u32,
    pub local_role: u32,
    pub remote_role: u32,
    pub root_live: u32,
    pub name: u64,
    pub class_weak: u64,
    pub class_address: u64,
    pub level_weak: u64,
    pub level_address: u64,
    pub root_weak: u64,
    pub root_address: u64,
}
#[repr(C)]
pub struct Retirement {
    pub qualified: u32,
    pub dispatched: u32,
    pub alive_after: u32,
    pub weak_present: u32,
    pub weak: u64,
    pub address: u64,
    pub before: Lifecycle,
    pub after: Lifecycle,
    pub reason: [u8; 192],
}
impl Default for Retirement {
    fn default() -> Self {
        Self {
            qualified: 0,
            dispatched: 0,
            alive_after: 2,
            weak_present: 2,
            weak: 0,
            address: 0,
            before: Lifecycle::default(),
            after: Lifecycle::default(),
            reason: [0; 192],
        }
    }
}
unsafe fn push_lifecycle(L: *mut lua_State, state: &Lifecycle) {
    unsafe {
        lua_createtable(L, 0, 14);
        let t = lua_gettop(L);
        set_int(L, t, "known", state.known as i64);
        for (key, bit, value) in [
            ("object_flags", 1, state.flags),
            ("actor_destroying", 2, state.destroying),
            ("world_listed", 4, state.listed),
            ("authority_from_role", 8, state.authority),
            ("local_role", 8, state.local_role),
            ("remote_role", 8, state.remote_role),
            ("root_live", 16, state.root_live),
        ] {
            lua_createtable(L, 0, 2);
            let fact = lua_gettop(L);
            set_bool(L, fact, "known", state.known & bit != 0);
            if state.known & bit != 0 {
                if matches!(
                    key,
                    "actor_destroying" | "world_listed" | "authority_from_role" | "root_live"
                ) {
                    set_bool(L, fact, "value", value != 0);
                } else {
                    set_int(L, fact, "value", value as i64);
                }
            }
            rawset_str(L, t, key);
        }
        for (key, bit, value) in [
            ("name", 64, state.name),
            ("class_weak", 64, state.class_weak),
            ("class_address", 64, state.class_address),
            ("level_weak", 32, state.level_weak),
            ("level_address", 32, state.level_address),
            ("root_weak", 16, state.root_weak),
            ("root_address", 16, state.root_address),
        ] {
            if state.known & bit != 0 {
                set_int(L, t, key, value as i64);
            }
        }
        lua_createtable(L, 0, 2);
        let end = lua_gettop(L);
        set_bool(L, end, "known", false);
        set_str(L, end, "reason", "native EndPlay event not observed");
        rawset_str(L, t, "end_play");
    }
}
#[repr(C)]
pub struct Provider {
    pub abi: u32,
    pub pad: u32,
    pub inspect: unsafe extern "C" fn(Object, Object, Object, *const Guard, *mut ResultInfo) -> i32,
    pub capture: unsafe extern "C" fn(
        Object,
        Object,
        Object,
        *const Component,
        *mut Frame,
        *const Guard,
        *mut ResultInfo,
    ) -> i32,
    pub create:
        unsafe extern "C" fn(Object, *const Component, u32, *const Guard, *mut ResultInfo) -> u64,
    pub apply: unsafe extern "C" fn(
        Object,
        u64,
        *const Component,
        *const Frame,
        u32,
        *const Guard,
        *mut ResultInfo,
    ) -> i32,
    pub destroy: unsafe extern "C" fn(Object, u64, *const Guard),
    pub discard: unsafe extern "C" fn(u64),
    pub retire:
        unsafe extern "C" fn(Object, Object, u32, Object, *const Guard, *mut Retirement) -> i32,
    pub probe_retirement:
        unsafe extern "C" fn(Object, Object, *const Guard, *mut Retirement) -> i32,
    pub forget_retirements: unsafe extern "C" fn(),
}
static PROVIDER: AtomicPtr<Provider> = AtomicPtr::new(std::ptr::null_mut());
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_set_presentation(p: *const Provider) {
    if p.is_null() || unsafe { (*p).abi } == 5 {
        PROVIDER.store(p as *mut Provider, Ordering::Release);
    }
}
fn provider() -> Result<&'static Provider, String> {
    let p = PROVIDER.load(Ordering::Acquire);
    if p.is_null() {
        Err("native presentation provider unavailable".into())
    } else {
        Ok(unsafe { &*p })
    }
}
struct Source {
    world: Object,
    pawn: Object,
    controller: Object,
    pawn_controller: reflect::HsmpProp,
    controller_pawn: reflect::HsmpProp,
    components: HashMap<u32, (Object, Object)>,
    prepared: Prepared,
}
struct Mirror {
    reference: w::EntityRef,
    revision: u32,
    world: Object,
    handle: u64,
    prepared: Prepared,
}
#[derive(Default)]
pub struct State {
    sources: HashMap<w::EntityRef, Source>,
    mirrors: HashMap<u32, Mirror>,
    pub(crate) pending: Option<w::RenderWorld>,
    pub(crate) input: Option<crate::native_input_capture::Reader>,
    ready: Option<(u64, u32, Vec<(w::EntityRef, u32)>)>,
}
impl State {
    fn drop_visuals(&mut self) {
        if let Ok(p) = provider() {
            for m in self.mirrors.values() {
                unsafe { (p.discard)(m.handle) }
            }
        }
        self.sources.clear();
        self.mirrors.clear();
        self.pending = None;
        self.input = None;
        self.ready = None;
    }
    pub fn drop_world(&mut self) {
        self.drop_visuals();
        if let Ok(p) = provider() {
            unsafe { (p.forget_retirements)() }
        }
    }
}
struct Arena {
    strings: Vec<Vec<u16>>,
    parameters: Vec<Vec<Parameter>>,
    names: Vec<Vec<Text>>,
    materials: Vec<Vec<Material>>,
    colors: Vec<Vec<u8>>,
    lods: Vec<Vec<VertexLod>>,
}
struct Prepared {
    _arena: Arena,
    components: Vec<Component>,
}
// Every pointer here targets an owned UTF16/parameter/color allocation in _arena.
// There are no UObject pointers; provider invocation remains game-thread guarded.
unsafe impl Send for Prepared {}
impl Prepared {
    fn new(recipe: &d::SourceRecipe) -> Self {
        let mut arena = Arena::new();
        let components = recipe
            .components
            .iter()
            .map(|c| arena.component(c))
            .collect();
        Self {
            _arena: arena,
            components,
        }
    }
}
impl Arena {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
            parameters: Vec::new(),
            names: Vec::new(),
            materials: Vec::new(),
            colors: Vec::new(),
            lods: Vec::new(),
        }
    }
    fn text(&mut self, s: &str) -> Text {
        let v = s
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let t = Text {
            data: v.as_ptr(),
            len: v.len() as u32 - 1,
            pad: 0,
        };
        self.strings.push(v);
        t
    }
    fn names(&mut self, strings: impl IntoIterator<Item = String>) -> (*const Text, u32) {
        let v = strings
            .into_iter()
            .map(|s| self.text(&s))
            .collect::<Vec<_>>();
        let out = (v.as_ptr(), v.len() as u32);
        self.names.push(v);
        out
    }
    fn parameters(
        &mut self,
        items: impl IntoIterator<Item = d::ParameterInfo>,
    ) -> (*const Parameter, u32) {
        let v = items
            .into_iter()
            .map(|i| Parameter {
                name: self.text(&i.name),
                association: i.association as u32,
                index: i.index,
            })
            .collect::<Vec<_>>();
        let out = (v.as_ptr(), v.len() as u32);
        self.parameters.push(v);
        out
    }
    fn component(&mut self, c: &d::RenderComponent) -> Component {
        let (bones, bone_count) = self.names(c.bones.iter().map(|b| b.name.clone()));
        let (morphs, morph_count) = self.names(c.morphs.iter().map(|b| b.name.clone()));
        let (hidden_bones, hidden_count) = self.names(c.hidden_bones.clone());
        let mut materials = Vec::new();
        for m in &c.materials {
            let (scalars, scalar_count) = self.parameters(m.scalars.iter().map(|p| p.info.clone()));
            let (vectors, vector_count) = self.parameters(m.vectors.iter().map(|p| p.info.clone()));
            let (textures, texture_count) =
                self.parameters(m.textures.iter().map(|p| p.info.clone()));
            materials.push(Material {
                slot: m.slot as u32,
                pad: 0,
                base: self.text(&m.base),
                scalars,
                scalar_count,
                pad_s: 0,
                vectors,
                vector_count,
                pad_v: 0,
                textures,
                texture_count,
                pad_t: 0,
            });
        }
        let material_count = materials.len() as u32;
        let material_ptr = materials.as_ptr();
        self.materials.push(materials);
        let mut lods = Vec::new();
        for lod in &c.vertex_colors {
            let mut rgba = Vec::with_capacity(lod.vertex_count as usize * 4);
            for run in &lod.runs {
                for _ in 0..run.count {
                    rgba.extend(run.color);
                }
            }
            lods.push(VertexLod {
                lod: lod.lod as u32,
                count: lod.vertex_count,
                rgba: rgba.as_ptr(),
                bytes: rgba.len() as u32,
                pad: 0,
            });
            self.colors.push(rgba);
        }
        let vertex_lods = lods.as_ptr();
        let vertex_count = lods.len() as u32;
        self.lods.push(lods);
        Component {
            id: c.id,
            parent: c.parent,
            kind: match c.kind {
                d::ComponentKind::Skeletal => 0,
                d::ComponentKind::Static => 1,
                d::ComponentKind::Groom => 2,
                d::ComponentKind::Procedural => 3,
                d::ComponentKind::Scene => 4,
            },
            visible: (c.visible && !c.hidden) as u32,
            asset: self.text(if c.kind == d::ComponentKind::Scene {
                &c.component_class
            } else {
                &c.asset
            }),
            skeleton: self.text(&c.skeleton),
            socket: self.text(&c.socket),
            relative: Transform {
                p: c.relative.translation,
                q: c.relative.rotation,
                scale: c.relative.scale,
            },
            bones,
            bone_count,
            pad_b: 0,
            morphs,
            morph_count,
            pad_m: 0,
            hidden_bones,
            hidden_count,
            pad_h: 0,
            materials: material_ptr,
            material_count,
            pad_mat: 0,
            vertex_state: match c.vertex_state {
                d::VertexState::NativeAsset => 0,
                d::VertexState::Captured => 1,
                d::VertexState::RuntimeOverride => 2,
                d::VertexState::Unavailable => 3,
                d::VertexState::NotApplicable => 4,
            },
            pad_vertex: 0,
            vertex_lods,
            vertex_count,
            pad_lod: 0,
        }
    }
}
struct FrameStorage {
    world: Transform,
    bones: Vec<Transform>,
    morphs: Vec<f32>,
    scalars: Vec<f32>,
    vectors: Vec<f32>,
    textures: Vec<Object>,
}
impl FrameStorage {
    fn source(c: &d::RenderComponent) -> Self {
        Self {
            world: Transform::default(),
            bones: vec![Transform::default(); c.bones.len()],
            morphs: vec![0.0; c.morphs.len()],
            scalars: vec![0.0; c.materials.iter().map(|m| m.scalars.len()).sum()],
            vectors: vec![0.0; c.materials.iter().map(|m| m.vectors.len() * 4).sum()],
            textures: vec![Object::default(); c.materials.iter().map(|m| m.textures.len()).sum()],
        }
    }
    fn ffi(&mut self) -> Frame {
        Frame {
            world: self.world,
            bones: self.bones.as_mut_ptr(),
            bone_count: self.bones.len() as u32,
            pad_b: 0,
            morphs: self.morphs.as_mut_ptr(),
            morph_count: self.morphs.len() as u32,
            pad_m: 0,
            scalars: self.scalars.as_mut_ptr(),
            scalar_count: self.scalars.len() as u32,
            pad_s: 0,
            vectors: self.vectors.as_mut_ptr(),
            vector_count: (self.vectors.len() / 4) as u32,
            pad_v: 0,
            textures: self.textures.as_mut_ptr(),
            texture_count: self.textures.len() as u32,
            pad_t: 0,
        }
    }
}
fn numbers(t: Transform) -> [f64; 10] {
    [
        t.p[0], t.p[1], t.p[2], t.q[0], t.q[1], t.q[2], t.q[3], t.scale[0], t.scale[1], t.scale[2],
    ]
}
fn transform(v: [f64; 10]) -> Transform {
    Transform {
        p: [v[0], v[1], v[2]],
        q: [v[3], v[4], v[5], v[6]],
        scale: [v[7], v[8], v[9]],
    }
}
unsafe fn integer(L: *mut lua_State, t: c_int, name: &str) -> Option<i64> {
    unsafe {
        rawget_str(L, t, name);
        let v = arg_int(L, -1);
        pop(L, 1);
        v
    }
}
unsafe fn object(vt: &HsmpReflect, address: i64) -> Result<Object, String> {
    unsafe {
        let p = address as usize as *mut std::ffi::c_void;
        if p.is_null() {
            return Err("native object unavailable".into());
        }
        let weak = reflect::keep(vt, p).ok_or("native object weak identity")?;
        if reflect::get(vt, weak) != p {
            return Err("native object changed".into());
        }
        Ok(Object {
            weak,
            address: address as u64,
        })
    }
}
unsafe fn field_object(
    vt: &HsmpReflect,
    L: *mut lua_State,
    t: c_int,
    name: &str,
) -> Result<Object, String> {
    unsafe {
        object(
            vt,
            integer(L, t, name).ok_or_else(|| format!("native binding {name}"))?,
        )
    }
}
unsafe fn object_field(vt: &HsmpReflect, obj: Object, name: &str) -> Result<u64, String> {
    unsafe {
        let p = object_property(vt, obj, name)?;
        read_object_property(vt, obj, p)
    }
}
unsafe fn object_property(
    vt: &HsmpReflect,
    obj: Object,
    name: &str,
) -> Result<reflect::HsmpProp, String> {
    unsafe {
        let pointer = reflect::get(vt, obj.weak);
        if pointer as u64 != obj.address {
            return Err("source object changed".into());
        }
        let mut p = reflect::HsmpProp::default();
        if (vt.obj_prop)(pointer, reflect::wide(name).as_ptr(), &mut p) != 1
            || p.cls != (vt.fname)(reflect::wide("ObjectProperty").as_ptr(), 1)
            || p.size != 8
            || p.offset < 0
        {
            return Err(format!("source {name} object property"));
        }
        if reflect::get(vt, obj.weak) != pointer {
            return Err("source object changed".into());
        }
        Ok(p)
    }
}
unsafe fn read_object_property(
    vt: &HsmpReflect,
    obj: Object,
    p: reflect::HsmpProp,
) -> Result<u64, String> {
    unsafe {
        let pointer = reflect::get(vt, obj.weak);
        if pointer as u64 != obj.address || pointer.is_null() {
            return Err("source object changed".into());
        }
        Ok(u64::from_le_bytes(
            std::slice::from_raw_parts((pointer as *const u8).add(p.offset as usize), 8)
                .try_into()
                .map_err(|_| "source object bytes")?,
        ))
    }
}
// The callback borrows Native only within a synchronous provider invocation.
// No Lua callbacks or global Native lock acquisition occur here.
struct GuardContext {
    native: *const Native,
    vt: *const HsmpReflect,
    world: Object,
    key: Vec<u8>,
    source: Option<(Object, Object, reflect::HsmpProp, reflect::HsmpProp)>,
}
impl GuardContext {
    fn new(
        native: &Native,
        vt: &HsmpReflect,
        world: Object,
        source: Option<&Source>,
    ) -> Result<Self, String> {
        let key = native
            .world_key
            .clone()
            .ok_or("native world token unavailable")?;
        Ok(Self {
            native,
            vt,
            world,
            key,
            source: source.map(|s| (s.pawn, s.controller, s.pawn_controller, s.controller_pawn)),
        })
    }
    fn ffi(&mut self) -> Guard {
        Guard {
            context: (self as *mut Self).cast(),
            check: check_guard,
        }
    }
    unsafe fn valid(&self) -> bool {
        unsafe {
            let native = &*self.native;
            let vt = &*self.vt;
            if !native.native_guard_ok(vt)
                || native.world_key.as_deref() != Some(self.key.as_slice())
                || reflect::get(vt, self.world.weak) as u64 != self.world.address
            {
                return false;
            }
            if let Some((pawn, controller, pc, cp)) = self.source {
                if reflect::get(vt, pawn.weak) as u64 != pawn.address
                    || reflect::get(vt, controller.weak) as u64 != controller.address
                {
                    return false;
                }
                if read_object_property(vt, pawn, pc).ok() != Some(controller.address)
                    || read_object_property(vt, controller, cp).ok() != Some(pawn.address)
                {
                    return false;
                }
            }
            true
        }
    }
}
unsafe extern "C" fn check_guard(context: *mut c_void) -> i32 {
    if context.is_null() {
        return 0;
    }
    // Never unwind through the C provider. Failure immediately abandons the op.
    std::panic::catch_unwind(|| unsafe { (&*context.cast::<GuardContext>()).valid() as i32 })
        .unwrap_or(0)
}
unsafe fn assigned_controller(vt: &HsmpReflect, world: Object, index: u8) -> Result<u64, String> {
    unsafe {
        let function =
            (vt.find)(reflect::wide("/Script/Engine.GameplayStatics:GetPlayerController").as_ptr());
        let target = (vt.find)(reflect::wide("/Script/Engine.Default__GameplayStatics").as_ptr());
        if function.is_null() || target.is_null() {
            return Err("source player lookup unavailable".into());
        }
        let (props, size) = crate::sample::props_of(vt, function);
        if props.len() != 3 || size <= 0 || size > 64 {
            return Err("source player lookup ABI".into());
        }
        let get = |name: &str, class: &str, width: i32| -> Result<usize, String> {
            let name = (vt.fname)(reflect::wide(name).as_ptr(), 1);
            let class = (vt.fname)(reflect::wide(class).as_ptr(), 1);
            let p = props
                .iter()
                .find(|p| p.name == name)
                .ok_or("source player lookup field")?;
            if p.cls != class || p.size != width || p.offset < 0 || p.offset + width > size {
                return Err("source player lookup field ABI".into());
            }
            Ok(p.offset as usize)
        };
        let context = get("WorldContextObject", "ObjectProperty", 8)?;
        let player = get("PlayerIndex", "IntProperty", 4)?;
        let result = get("ReturnValue", "ObjectProperty", 8)?;
        if reflect::get(vt, world.weak) as u64 != world.address {
            return Err("source world changed".into());
        }
        let mut params = crate::sample::Params([0; crate::sample::PARAMS_BYTES]);
        params.0[context..context + 8].copy_from_slice(&world.address.to_le_bytes());
        params.0[player..player + 4].copy_from_slice(&(index as i32).to_le_bytes());
        (vt.call)(target, function, params.0.as_mut_ptr().cast());
        if reflect::get(vt, world.weak) as u64 != world.address {
            return Err("source world changed".into());
        }
        Ok(u64::from_le_bytes(
            params.0[result..result + 8]
                .try_into()
                .map_err(|_| "source controller bytes")?,
        ))
    }
}
unsafe fn push_scene(L: *mut lua_State, scene: &hsmp_server::native_service::Scene) {
    unsafe {
        lua_createtable(L, 0, 6);
        let t = lua_gettop(L);
        set_int(L, t, "epoch", scene.directory.epoch as i64);
        set_int(L, t, "dir_seq", scene.directory.seq as i64);
        set_int(L, t, "frame_seq", scene.frame.world.frame_seq as i64);
        set_int(L, t, "state", scene.directory.state as i64);
        set_int(L, t, "peer_id", scene.peer_id as i64);
        lua_createtable(L, scene.directory.entities.len() as c_int, 0);
        let rows = lua_gettop(L);
        for (i, e) in scene.directory.entities.iter().enumerate() {
            lua_createtable(L, 0, 9);
            let row = lua_gettop(L);
            set_int(L, row, "epoch", e.reference.epoch as i64);
            set_int(L, row, "id", e.reference.id as i64);
            set_int(L, row, "incarnation", e.reference.incarnation as i64);
            set_int(L, row, "owner_peer", e.owner_peer as i64);
            set_int(L, row, "kind", e.kind as i64);
            if let Some(s) = scene
                .frame
                .world
                .entities
                .iter()
                .find(|s| s.reference == e.reference)
            {
                lua_createtable(L, 3, 0);
                let a = lua_gettop(L);
                fill_array(L, a, s.root.pos.iter().map(|x| *x as f64));
                rawset_str(L, row, "position");
                set_num(L, row, "health", s.vitals.health().unwrap_or(-1.0) as f64);
                if let Some(control) =
                    hsmp_pose::posecodec::v2::decode(&s.pose).and_then(|p| p.control)
                {
                    set_num(L, row, "look_yaw", control.ctrl_yaw as f64);
                }
            }
            lua_rawseti(L, rows, i as i64 + 1);
        }
        rawset_str(L, t, "entities");
    }
}
impl Native {
    pub unsafe fn native_scene_assets(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result = (|| -> Result<(), String> {
                let client = self
                    .native_host
                    .client
                    .as_ref()
                    .ok_or("not a native client")?;
                let scene = client.scene().ok_or("no coherent native scene")?;
                let mut assets = std::collections::BTreeSet::new();
                for descriptor in &scene.descriptors {
                    descriptor
                        .recipe
                        .validate_mirror_profile()
                        .map_err(str::to_owned)?;
                    for c in &descriptor.recipe.components {
                        if c.kind == d::ComponentKind::Scene {
                            assets.insert(c.component_class.clone());
                        }
                        for s in [&c.asset, &c.skeleton] {
                            if !s.is_empty() {
                                assets.insert(s.clone());
                            }
                        }
                        for m in &c.materials {
                            assets.insert(m.base.clone());
                            for t in &m.textures {
                                if !t.value.is_empty() {
                                    assets.insert(t.value.clone());
                                }
                            }
                        }
                    }
                }
                lua_createtable(L, assets.len() as c_int, 0);
                let rows = lua_gettop(L);
                for (i, path) in assets.iter().enumerate() {
                    lua_pushlstring(L, path.as_ptr().cast(), path.len());
                    lua_rawseti(L, rows, i as i64 + 1);
                }
                let scope = format!(
                    "{}:{}:{:?}",
                    scene.directory.epoch,
                    scene.directory.seq,
                    scene
                        .descriptors
                        .iter()
                        .map(|d| (d.reference.id, d.reference.incarnation, d.revision))
                        .collect::<Vec<_>>()
                );
                lua_pushlstring(L, scope.as_ptr().cast(), scope.len());
                Ok(())
            })();
            match result {
                Ok(()) => 2,
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_inspect_component(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result = (|| -> Result<(), String> {
                if !self.native_host.is_host() || !self.sample.world_ok {
                    return Err("source role/world".into());
                }
                let p = provider()?;
                let vt = reflect::vt().ok_or("reflection unavailable")?;
                let world = object(vt, arg_int(L, 1).ok_or("inspect world")?)?;
                let owner = object(vt, arg_int(L, 2).ok_or("inspect owner")?)?;
                let component = object(vt, arg_int(L, 3).ok_or("inspect component")?)?;
                let mut context = GuardContext::new(self, vt, world, None)?;
                let guard = context.ffi();
                let mut r = ResultInfo::default();
                if !context.valid()
                    || (p.inspect)(world, owner, component, &guard, &mut r) != 1
                    || r.complete != 1
                    || !context.valid()
                {
                    return Err(r.reason());
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    lua_pushlstring(L, b"verified_component".as_ptr().cast(), 18);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn host_describe(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result = (|| -> Result<(), String> {
                if !self.native_host.is_host() || !self.sample.world_ok {
                    return Err("source role/world".into());
                }
                if !is_table(L, 1) || !is_table(L, 3) {
                    return Err("descriptor metadata/bindings".into());
                }
                let recipe = read_recipe(L, 2)?;
                let vt = reflect::vt().ok_or("reflection unavailable")?;
                let uint = |name| {
                    integer(L, 1, name)
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| format!("descriptor {name}"))
                };
                let reference = w::EntityRef {
                    epoch: integer(L, 1, "epoch").ok_or("descriptor epoch")? as u64,
                    id: uint("id")?,
                    incarnation: uint("incarnation")?,
                };
                let descriptor = w::Descriptor {
                    reference,
                    slot: u16::try_from(uint("slot")?).map_err(|_| "descriptor slot")?,
                    directory_seq: uint("dir_seq")?,
                    revision: uint("revision")?,
                    source_frame_seq: uint("frame_seq")?,
                    recipe,
                };
                let world = field_object(vt, L, 3, "world")?;
                let pawn = field_object(vt, L, 3, "pawn")?;
                let controller = field_object(vt, L, 3, "controller")?;
                let directory = self
                    .native_host
                    .directory()
                    .ok_or("source directory unavailable")?;
                let entity = directory
                    .entities
                    .iter()
                    .find(|e| e.reference == reference && e.slot == descriptor.slot)
                    .ok_or("source directory actor generation")?;
                if descriptor.directory_seq != directory.seq
                    || object_field(vt, pawn, "Controller")? != controller.address
                    || object_field(vt, controller, "Pawn")? != pawn.address
                {
                    return Err("source actor/controller binding changed".into());
                }
                if entity.kind == w::HUMAN
                    && assigned_controller(vt, world, entity.controller)? != controller.address
                {
                    return Err("source assigned controller changed".into());
                }
                if let Some(previous) = self.presentation.sources.get(&reference) {
                    if previous.world.weak != world.weak
                        || previous.world.address != world.address
                        || previous.pawn.weak != pawn.weak
                        || previous.pawn.address != pawn.address
                        || previous.controller.weak != controller.weak
                        || previous.controller.address != controller.address
                    {
                        return Err("native incarnation changed".into());
                    }
                }
                rawget_str(L, 3, "components");
                if !is_table(L, -1)
                    || lua_rawlen(L, -1) as usize != descriptor.recipe.components.len()
                {
                    pop(L, 1);
                    return Err("source component binding count".into());
                }
                let rows = lua_absindex(L, -1);
                let mut components = HashMap::new();
                for i in 1..=lua_rawlen(L, rows) {
                    lua_rawgeti(L, rows, i as i64);
                    let t = lua_absindex(L, -1);
                    let id = integer(L, t, "id")
                        .and_then(|x| u32::try_from(x).ok())
                        .ok_or("component id")?;
                    if !descriptor.recipe.components.iter().any(|c| c.id == id)
                        || components.contains_key(&id)
                    {
                        pop(L, 2);
                        return Err("source component dictionary".into());
                    }
                    let component = field_object(vt, L, t, "address")?;
                    let owner = field_object(vt, L, t, "owner")?;
                    components.insert(id, (component, owner));
                    pop(L, 1);
                }
                pop(L, 1);
                let prepared = Prepared::new(&descriptor.recipe);
                let pawn_controller = object_property(vt, pawn, "Controller")?;
                let controller_pawn = object_property(vt, controller, "Pawn")?;
                if read_object_property(vt, pawn, pawn_controller)? != controller.address
                    || read_object_property(vt, controller, controller_pawn)? != pawn.address
                {
                    return Err("source possession changed during descriptor copy".into());
                }
                let host = self.native_host.host.as_ref().ok_or("not a source host")?;
                host.source_team(reference, descriptor.recipe.team)
                    .map_err(str::to_owned)?;
                host.publish_descriptor(descriptor).map_err(str::to_owned)?;
                self.presentation.sources.insert(
                    reference,
                    Source {
                        world,
                        pawn,
                        controller,
                        pawn_controller,
                        controller_pawn,
                        components,
                        prepared,
                    },
                );
                Ok(())
            })();
            match result {
                Ok(()) => {
                    lua_pushboolean(L, 1);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_capture_render(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            self.presentation.pending = None;
            let result = (|| -> Result<(), String> {
                if !self.native_host.is_host() || !self.sample.world_ok {
                    return Err("source role/world".into());
                }
                let p = provider()?;
                let vt = reflect::vt().ok_or("reflection unavailable")?;
                let (world, _) = self
                    .sample
                    .native_pending
                    .as_ref()
                    .ok_or("no staged native world")?;
                let world = world.clone();
                let host = self.native_host.host.as_ref().ok_or("not a source host")?;
                let mut entities = Vec::new();
                for entity in &world.entities {
                    let desc = host
                        .descriptor(entity.reference.id)
                        .filter(|d| {
                            d.reference == entity.reference
                                && d.directory_seq == world.directory_seq
                        })
                        .ok_or("no original source descriptor")?;
                    let binding = self
                        .presentation
                        .sources
                        .get(&entity.reference)
                        .ok_or("no original source bindings")?;
                    if reflect::get(vt, binding.pawn.weak) as u64 != binding.pawn.address
                        || reflect::get(vt, binding.controller.weak) as u64
                            != binding.controller.address
                    {
                        return Err("source pawn/controller changed".into());
                    }
                    if object_field(vt, binding.pawn, "Controller")? != binding.controller.address
                        || object_field(vt, binding.controller, "Pawn")? != binding.pawn.address
                    {
                        return Err("source actor/controller binding changed".into());
                    }
                    let mut context = GuardContext::new(self, vt, binding.world, Some(binding))?;
                    let guard = context.ffi();
                    let mut components = Vec::new();
                    for (index, c) in desc.recipe.components.iter().enumerate() {
                        let (input, owner) = *binding
                            .components
                            .get(&c.id)
                            .ok_or("source component binding")?;
                        let recipe = &binding.prepared.components[index];
                        let mut values = FrameStorage::source(c);
                        let mut frame = values.ffi();
                        let mut r = ResultInfo::default();
                        if !context.valid()
                            || (p.capture)(
                                binding.world,
                                owner,
                                input,
                                recipe,
                                &mut frame,
                                &guard,
                                &mut r,
                            ) != 1
                            || r.complete != 1
                            || !context.valid()
                        {
                            return Err(format!("source component {}: {}", c.id, r.reason()));
                        }
                        values.world = frame.world;
                        let (mut si, mut vi, mut ti) = (0, 0, 0);
                        let mut materials = Vec::new();
                        for m in &c.materials {
                            let scalars = values.scalars[si..si + m.scalars.len()].to_vec();
                            si += m.scalars.len();
                            let vectors = values.vectors[vi..vi + m.vectors.len() * 4]
                                .chunks_exact(4)
                                .map(|v| [v[0], v[1], v[2], v[3]])
                                .collect();
                            vi += m.vectors.len() * 4;
                            let mut textures = Vec::new();
                            for t in &m.textures {
                                let actual = values.textures[ti];
                                ti += 1;
                                let expected = if t.value.is_empty() {
                                    std::ptr::null_mut()
                                } else {
                                    (vt.find)(reflect::wide(&t.value).as_ptr())
                                };
                                if !expected.is_null() {
                                    reflect::keep(vt, expected)
                                        .ok_or("source texture weak identity")?;
                                }
                                if (expected as u64) != actual.address
                                    || (actual.address != 0
                                        && reflect::get(vt, actual.weak) as u64 != actual.address)
                                {
                                    return Err("source texture recipe changed".into());
                                }
                                textures.push(t.value.clone());
                            }
                            materials.push(w::RenderMaterial {
                                scalars,
                                vectors,
                                textures,
                            });
                        }
                        components.push(w::RenderComponent {
                            id: c.id,
                            transform: numbers(values.world),
                            bones: values.bones.into_iter().map(numbers).collect(),
                            morphs: values.morphs,
                            materials,
                        });
                    }
                    entities.push(w::RenderEntity {
                        reference: entity.reference,
                        revision: desc.revision,
                        components,
                    });
                }
                let render = w::RenderWorld { world, entities };
                w::encode_render_world(&render).map_err(str::to_owned)?;
                self.presentation.pending = Some(render);
                Ok(())
            })();
            match result {
                Ok(()) => {
                    lua_pushboolean(L, 1);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_scene(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(client) = self.native_host.client.as_ref() else {
                return nil_err(L, "not a native client");
            };
            let Some(scene) = client.scene() else {
                lua_pushnil(L);
                return 1;
            };
            push_scene(L, &scene);
            1
        }
    }
    pub unsafe fn native_present(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result = (|| -> Result<hsmp_server::native_service::Scene, String> {
                if !self.native_host.is_client() || !self.sample.world_ok {
                    return Err("client role/world".into());
                }
                let p = provider()?;
                let vt = reflect::vt().ok_or("reflection unavailable")?;
                let world = object(vt, arg_int(L, 1).ok_or("presentation world")?)?;
                let mut context = GuardContext::new(self, vt, world, None)?;
                let guard = context.ffi();
                let client = self
                    .native_host
                    .client
                    .as_ref()
                    .ok_or("not a native client")?;
                let scene = client.scene().ok_or("no complete current source scene")?;
                for desc in &scene.descriptors {
                    desc.recipe
                        .validate_mirror_profile()
                        .map_err(str::to_owned)?;
                }
                let valid = scene
                    .descriptors
                    .iter()
                    .map(|d| (d.reference, d.revision))
                    .collect::<Vec<_>>();
                let obsolete = self
                    .presentation
                    .mirrors
                    .iter()
                    .filter(|(_, m)| {
                        m.world.weak != world.weak
                            || m.world.address != world.address
                            || !valid.contains(&(m.reference, m.revision))
                    })
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                for id in obsolete {
                    if let Some(m) = self.presentation.mirrors.remove(&id) {
                        if m.world.weak == world.weak
                            && m.world.address == world.address
                            && context.valid()
                        {
                            (p.destroy)(world, m.handle, &guard);
                        } else {
                            (p.discard)(m.handle);
                        }
                    }
                }
                for desc in &scene.descriptors {
                    if !self.presentation.mirrors.contains_key(&desc.reference.id) {
                        let prepared = Prepared::new(&desc.recipe);
                        let recipes = &prepared.components;
                        let mut r = ResultInfo::default();
                        if !context.valid() {
                            return Err("mirror world changed".into());
                        }
                        let handle = (p.create)(
                            world,
                            recipes.as_ptr(),
                            recipes.len() as u32,
                            &guard,
                            &mut r,
                        );
                        if handle == 0 || r.complete != 1 || !context.valid() {
                            if handle != 0 {
                                (p.discard)(handle);
                            }
                            return Err(format!("mirror create: {}", r.reason()));
                        }
                        self.presentation.mirrors.insert(
                            desc.reference.id,
                            Mirror {
                                reference: desc.reference,
                                revision: desc.revision,
                                world,
                                handle,
                                prepared,
                            },
                        );
                    }
                    let source = scene
                        .frame
                        .entities
                        .iter()
                        .find(|e| e.reference == desc.reference && e.revision == desc.revision)
                        .ok_or("mirror source frame generation")?;
                    let mut storage = Vec::new();
                    for recipe in &desc.recipe.components {
                        let frame = source
                            .components
                            .iter()
                            .find(|c| c.id == recipe.id)
                            .ok_or("mirror complete component frame")?;
                        let mut values = FrameStorage::source(recipe);
                        values.world = transform(frame.transform);
                        values.bones = frame.bones.iter().copied().map(transform).collect();
                        values.morphs = frame.morphs.clone();
                        values.scalars.clear();
                        values.vectors.clear();
                        values.textures.clear();
                        for m in &frame.materials {
                            values.scalars.extend(&m.scalars);
                            for v in &m.vectors {
                                values.vectors.extend(v);
                            }
                            for t in &m.textures {
                                if t.is_empty() {
                                    values.textures.push(Object::default());
                                } else {
                                    let asset = (vt.find)(reflect::wide(t).as_ptr());
                                    if asset.is_null() {
                                        return Err("mirror texture asset unavailable".into());
                                    }
                                    values.textures.push(object(vt, asset as i64)?);
                                }
                            }
                        }
                        storage.push(values);
                    }
                    let frames = storage
                        .iter_mut()
                        .map(FrameStorage::ffi)
                        .collect::<Vec<_>>();
                    let mirror = self
                        .presentation
                        .mirrors
                        .get(&desc.reference.id)
                        .ok_or("mirror handle")?;
                    let mut r = ResultInfo::default();
                    if !context.valid()
                        || (p.apply)(
                            world,
                            mirror.handle,
                            mirror.prepared.components.as_ptr(),
                            frames.as_ptr(),
                            frames.len() as u32,
                            &guard,
                            &mut r,
                        ) != 1
                        || r.complete != 1
                        || !context.valid()
                    {
                        return Err(format!("mirror complete readback: {}", r.reason()));
                    }
                }
                let ready = (scene.directory.epoch, scene.directory.seq, valid.clone());
                if self.presentation.ready.as_ref() != Some(&ready) {
                    client
                        .mirror_ready(w::MirrorReady {
                            epoch: scene.directory.epoch,
                            directory_seq: scene.directory.seq,
                            frame_seq: scene.frame.world.frame_seq,
                            entities: valid,
                        })
                        .map_err(str::to_owned)?;
                    self.presentation.ready = Some(ready);
                }
                Ok(scene)
            })();
            match result {
                Ok(scene) => {
                    lua_pushboolean(L, 1);
                    push_scene(L, &scene);
                    2
                }
                Err(e) => {
                    self.presentation.ready = None;
                    nil_err(L, &e)
                }
            }
        }
    }
    pub unsafe fn native_retire_actor(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() {
                return nil_err(L, "client retirement role required");
            }
            let result = (|| -> Result<(i32, Retirement), String> {
                let p = provider()?;
                let vt = reflect::vt().ok_or("native reflection unavailable")?;
                if !self.native_guard_ok(vt) {
                    return Err("native retirement world guard unavailable".into());
                }
                let world = object(vt, arg_int(L, 1).ok_or("native retirement world")?)?;
                let actor = object(vt, arg_int(L, 2).ok_or("native retirement actor")?)?;
                let kind = arg_int(L, 3).ok_or("native retirement kind")?;
                if kind != 0 {
                    return Err("native retirement kind unsupported".into());
                }
                let mut context = GuardContext::new(self, vt, world, None)?;
                let guard = context.ffi();
                if !context.valid() {
                    return Err("native retirement world changed".into());
                }
                let mut out = Retirement::default();
                let status = (p.retire)(world, actor, 0, Object::default(), &guard, &mut out);
                // The provider owns original actor qualification. Never look up
                // or dereference that actor again after its native dispatch.
                Ok((status, out))
            })();
            match result {
                Ok((status, out)) => {
                    lua_createtable(L, 0, 7);
                    let t = lua_gettop(L);
                    set_bool(
                        L,
                        t,
                        "ok",
                        status == 1
                            && out.qualified == 1
                            && out.dispatched == 1
                            && out.alive_after == 0,
                    );
                    set_bool(L, t, "qualified", out.qualified == 1);
                    set_bool(L, t, "dispatched", out.dispatched == 1);
                    set_int(L, t, "alive_after", out.alive_after as i64);
                    set_int(L, t, "weak", out.weak as i64);
                    set_int(L, t, "address", out.address as i64);
                    set_int(L, t, "weak_present", out.weak_present as i64);
                    push_lifecycle(L, &out.before);
                    rawset_str(L, t, "before");
                    push_lifecycle(L, &out.after);
                    rawset_str(L, t, "after");
                    let n = out
                        .reason
                        .iter()
                        .position(|b| *b == 0)
                        .unwrap_or(out.reason.len());
                    set_str(L, t, "reason", &String::from_utf8_lossy(&out.reason[..n]));
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_probe_retirement(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() {
                return nil_err(L, "client retirement role required");
            }
            let result = (|| -> Result<(i32, Retirement), String> {
                let p = provider()?;
                let vt = reflect::vt().ok_or("native reflection unavailable")?;
                if !self.native_guard_ok(vt) {
                    return Err("native retirement world guard unavailable".into());
                }
                let world = object(vt, arg_int(L, 1).ok_or("native retirement world")?)?;
                // Original scalar proof only: do not reconstruct an expired
                // actor from its address or ask it for a fresh weak handle.
                let original = Object {
                    weak: arg_int(L, 2).ok_or("native original retirement weak")? as u64,
                    address: arg_int(L, 3).ok_or("native original retirement address")? as u64,
                };
                if original.weak == 0 || original.address == 0 {
                    return Err("native original retirement identity missing".into());
                }
                let mut context = GuardContext::new(self, vt, world, None)?;
                let guard = context.ffi();
                if !context.valid() {
                    return Err("native retirement world changed".into());
                }
                let mut out = Retirement::default();
                let status = (p.probe_retirement)(world, original, &guard, &mut out);
                Ok((status, out))
            })();
            match result {
                Ok((status, out)) => {
                    lua_createtable(L, 0, 7);
                    let t = lua_gettop(L);
                    set_bool(
                        L,
                        t,
                        "ok",
                        status == 1
                            && out.qualified == 1
                            && out.dispatched == 0
                            && out.alive_after == 0,
                    );
                    set_bool(L, t, "qualified", out.qualified == 1);
                    set_bool(L, t, "dispatched", out.dispatched == 1);
                    set_int(L, t, "alive_after", out.alive_after as i64);
                    set_int(L, t, "weak", out.weak as i64);
                    set_int(L, t, "address", out.address as i64);
                    set_int(L, t, "weak_present", out.weak_present as i64);
                    push_lifecycle(L, &out.before);
                    rawset_str(L, t, "before");
                    push_lifecycle(L, &out.after);
                    rawset_str(L, t, "after");
                    let n = out
                        .reason
                        .iter()
                        .position(|b| *b == 0)
                        .unwrap_or(out.reason.len());
                    set_str(L, t, "reason", &String::from_utf8_lossy(&out.reason[..n]));
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_forget_retirements(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() {
                return nil_err(L, "client retirement role required");
            }
            match provider() {
                Ok(p) => {
                    (p.forget_retirements)();
                    lua_pushboolean(L, 1);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn native_clear_mirrors(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() {
                return nil_err(L, "client presentation role required");
            }
            if let (Ok(p), Some(vt), Some(address)) = (provider(), reflect::vt(), arg_int(L, 1)) {
                if let Ok(world) = object(vt, address) {
                    let mut context = GuardContext::new(self, vt, world, None).ok();
                    let guard = context.as_mut().map(GuardContext::ffi);
                    for m in self.presentation.mirrors.values() {
                        if m.world.weak == world.weak
                            && m.world.address == world.address
                            && context.as_ref().is_some_and(|c| c.valid())
                        {
                            if let Some(guard) = &guard {
                                (p.destroy)(world, m.handle, guard);
                            } else {
                                (p.discard)(m.handle);
                            }
                        } else {
                            (p.discard)(m.handle);
                        }
                    }
                    self.presentation.mirrors.clear();
                }
            }
            // Reconnect/scene clear in the same engine world must retain driver
            // retirement proofs. Actual world-leaving clears them separately.
            self.presentation.drop_visuals();
            lua_pushboolean(L, 1);
            1
        }
    }
}

#[cfg(test)]
mod presentation_binding_tests {
    use super::*;
    #[test]
    fn lifecycle_marshaling_preserves_false_and_unknown_facts() {
        unsafe {
            let L = mlua::ffi::luaL_newstate();
            assert!(!L.is_null());
            mlua::ffi::luaL_openlibs(L);
            let state = Lifecycle {
                known: 1 | 4 | 64,
                flags: 0x40000000,
                listed: 0,
                name: 9007199254740993,
                ..Lifecycle::default()
            };
            push_lifecycle(L.cast(), &state);
            let name = std::ffi::CString::new("sample").unwrap();
            mlua::ffi::lua_setglobal(L, name.as_ptr());
            let code=std::ffi::CString::new("assert(sample.world_listed.known and sample.world_listed.value==false); assert(sample.object_flags.value==0x40000000); assert(sample.local_role.known==false and sample.local_role.value==nil); assert(sample.end_play.known==false); assert(sample.name==9007199254740993)").unwrap();
            assert_eq!(mlua::ffi::luaL_loadstring(L, code.as_ptr()), 0);
            assert_eq!(mlua::ffi::lua_pcall(L, 0, 0, 0), 0);
            mlua::ffi::lua_close(L);
        }
    }
    #[test]
    fn scene_anchor_binding_preserves_class_transform_and_empty_render_dictionary() {
        let mut recipe = d::SourceRecipe::decode_recipe(include_bytes!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
        ))
        .unwrap();
        let mut anchor = recipe.components[0].clone();
        anchor.id = 77;
        anchor.parent = recipe.components[0].id;
        anchor.role = "anchor".into();
        anchor.component_class = "/Script/Engine.SceneComponent".into();
        anchor.kind = d::ComponentKind::Scene;
        anchor.geometry = d::Geometry::NotApplicable;
        anchor.scene = d::SceneEvidence::Scene;
        anchor.asset.clear();
        anchor.skeleton.clear();
        anchor.physics_asset.clear();
        anchor.collision = None;
        anchor.bones.clear();
        anchor.materials.clear();
        anchor.morphs.clear();
        anchor.hidden_bones.clear();
        anchor.groom.clear();
        anchor.vertex_state = d::VertexState::NotApplicable;
        anchor.vertex_colors.clear();
        anchor.deformer.clear();
        anchor.cloth = false;
        anchor.relative.translation = [1.2345678901234567, -2.75, 7.0];
        recipe.components.insert(0, anchor.clone());
        recipe.validate_mirror_profile().unwrap();
        let prepared = Prepared::new(&recipe);
        let bound = &prepared.components[0];
        assert_eq!(
            (bound.kind, bound.vertex_state, bound.id, bound.parent),
            (4, 4, 77, anchor.parent)
        );
        assert_eq!(bound.relative.p, anchor.relative.translation);
        let asset =
            unsafe { std::slice::from_raw_parts(bound.asset.data, bound.asset.len as usize) };
        assert_eq!(String::from_utf16(asset).unwrap(), anchor.component_class);
        assert_eq!(
            (
                bound.bone_count,
                bound.morph_count,
                bound.material_count,
                bound.vertex_count
            ),
            (0, 0, 0, 0)
        );
        let frame = FrameStorage::source(&anchor);
        assert!(
            frame.bones.is_empty()
                && frame.morphs.is_empty()
                && frame.scalars.is_empty()
                && frame.vectors.is_empty()
                && frame.textures.is_empty()
        );
        assert_eq!(std::mem::size_of::<Lifecycle>(), 88);
        assert_eq!(std::mem::size_of::<Retirement>(), 400);
    }
}

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
impl Default for Text {
    fn default() -> Self {
        Self {
            data: std::ptr::null(),
            len: 0,
            pad: 0,
        }
    }
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
#[derive(Clone, Copy, Default)]
pub struct SplineProfile {
    pub position_count: u32,
    pub rotation_count: u32,
    pub scale_count: u32,
    pub reparam_count: u32,
    pub metadata_null: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VertexStateProof {
    pub lod_info_count: u32,
    pub no_override: u32,
    pub asset_present: u32,
    pub material_count: u32,
    pub material_null_mask: u32,
    pub component_kind: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FinishTarget {
    pub owner: Object,
    pub component: Object,
    pub asset: Object,
    pub scene_kind: u32,
    pub owned_mirror: u32,
    pub socket: Text,
    pub arm: SpringArmFrame,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SpringArmFrame {
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SplineVectorPoint {
    pub key: f32,
    pub interp: u32,
    pub out: [f64; 3],
    pub arrive: [f64; 3],
    pub leave: [f64; 3],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SplineQuatPoint {
    pub key: f32,
    pub interp: u32,
    pub out: [f64; 4],
    pub arrive: [f64; 4],
    pub leave: [f64; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SplineFloatPoint {
    pub key: f32,
    pub out: f32,
    pub arrive: f32,
    pub leave: f32,
    pub interp: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SplineSettings {
    pub allow_spline_editing_per_instance: u32,
    pub reparam_steps_per_segment: i32,
    pub duration: f32,
    pub stationary_endpoints: u32,
    pub spline_has_been_edited: u32,
    pub modified_by_construction_script: u32,
    pub input_spline_points_to_construction_script: u32,
    pub draw_debug: u32,
    pub closed_loop: u32,
    pub loop_position_override: u32,
    pub loop_position: f32,
    pub pad: u32,
    pub default_up_vector: [f64; 3],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SplineCurve<T> {
    pub points: *mut T,
    pub count: u32,
    pub looped: u32,
    pub loop_key_offset: f32,
    pub pad: u32,
}
impl<T> Default for SplineCurve<T> {
    fn default() -> Self {
        Self {
            points: std::ptr::null_mut(),
            count: 0,
            looped: 0,
            loop_key_offset: 0.0,
            pad: 0,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SplineFrame {
    pub visible: u32,
    pub hidden: u32,
    pub owner_hidden: u32,
    pub version: u32,
    pub settings: SplineSettings,
    pub position: SplineCurve<SplineVectorPoint>,
    pub rotation: SplineCurve<SplineQuatPoint>,
    pub scale: SplineCurve<SplineVectorPoint>,
    pub reparam: SplineCurve<SplineFloatPoint>,
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
    pub spline: SplineProfile,
    pub pad_spline: u32,
    pub spring_arm_socket: Text,
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
    pub spline: *mut SplineFrame,
    pub spring_arm: *mut SpringArmFrame,
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
    pub(crate) fn reason(&self) -> String {
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
#[repr(C)]
pub struct ActorScope {
    pub qualified: u32,
    pub pad: u32,
    pub weak: u64,
    pub address: u64,
    pub state: Lifecycle,
    pub reason: [u8; 192],
}
impl Default for ActorScope {
    fn default() -> Self {
        Self {
            qualified: 0,
            pad: 0,
            weak: 0,
            address: 0,
            state: Lifecycle::default(),
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
    pub actor_scope: unsafe extern "C" fn(Object, Object, *const Guard, *mut ActorScope) -> i32,
    pub describe_spline: unsafe extern "C" fn(
        Object,
        Object,
        Object,
        *const Guard,
        *mut SplineProfile,
        *mut ResultInfo,
    ) -> i32,
    pub describe_vertex_state: unsafe extern "C" fn(
        Object,
        Object,
        Object,
        *const Guard,
        *mut VertexStateProof,
        *mut ResultInfo,
    ) -> i32,
    pub finish_scene_sets: unsafe extern "C" fn(
        Object,
        *const FinishTarget,
        u32,
        *const u64,
        u32,
        *const Guard,
        *mut ResultInfo,
    ) -> i32,
}
static PROVIDER: AtomicPtr<Provider> = AtomicPtr::new(std::ptr::null_mut());
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_set_presentation(p: *const Provider) {
    if p.is_null() || unsafe { (*p).abi } == 11 {
        PROVIDER.store(p as *mut Provider, Ordering::Release);
    } else {
        PROVIDER.store(std::ptr::null_mut(), Ordering::Release);
    }
}
pub(crate) fn provider() -> Result<&'static Provider, String> {
    let p = PROVIDER.load(Ordering::Acquire);
    if p.is_null() {
        Err("native presentation provider unavailable".into())
    } else {
        Ok(unsafe { &*p })
    }
}
#[derive(Clone, Copy)]
struct VertexTarget {
    owner: Object,
    component: Object,
    asset: Object,
    scene_kind: u32,
}
struct Source {
    world: Object,
    pawn: Object,
    controller: Object,
    pawn_controller: reflect::HsmpProp,
    controller_pawn: reflect::HsmpProp,
    components: HashMap<u32, (Object, Object)>,
    vertices: Vec<VertexTarget>,
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
            kind: match (&c.kind, &c.scene) {
                (d::ComponentKind::Scene, d::SceneEvidence::Camera) => 6,
                (d::ComponentKind::Scene, d::SceneEvidence::SpringArm { .. }) => 7,
                (d::ComponentKind::Skeletal, _) if c.geometry == d::Geometry::NativeEmpty => 9,
                (d::ComponentKind::Skeletal, _) => 0,
                (d::ComponentKind::Static, _) if c.geometry == d::Geometry::NativeEmpty => 8,
                (d::ComponentKind::Static, _) => 1,
                (d::ComponentKind::Groom, _) => 2,
                (d::ComponentKind::Procedural, _) => 3,
                (d::ComponentKind::Scene, _) => 4,
                (d::ComponentKind::Spline, _) => 5,
            },
            visible: (c.visible && !c.hidden) as u32,
            asset: self.text(
                if matches!(c.kind, d::ComponentKind::Scene | d::ComponentKind::Spline)
                    || c.geometry == d::Geometry::NativeEmpty
                {
                    &c.component_class
                } else {
                    &c.asset
                },
            ),
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
            spline: c
                .spline_profile
                .as_ref()
                .map(|p| SplineProfile {
                    position_count: p.position_count.into(),
                    rotation_count: p.rotation_count.into(),
                    scale_count: p.scale_count.into(),
                    reparam_count: p.reparam_count.into(),
                    metadata_null: p.metadata_null.into(),
                })
                .unwrap_or_default(),
            pad_spline: 0,
            spring_arm_socket: self.text(match &c.scene {
                d::SceneEvidence::SpringArm { socket_name, .. } => socket_name,
                _ => "",
            }),
        }
    }
}
struct SplineStorage {
    value: Box<SplineFrame>,
    position: Vec<SplineVectorPoint>,
    rotation: Vec<SplineQuatPoint>,
    scale: Vec<SplineVectorPoint>,
    reparam: Vec<SplineFloatPoint>,
}
impl SplineStorage {
    fn source(p: &d::SplineProfile) -> Self {
        Self {
            value: Box::default(),
            position: vec![SplineVectorPoint::default(); p.position_count as usize],
            rotation: vec![SplineQuatPoint::default(); p.rotation_count as usize],
            scale: vec![SplineVectorPoint::default(); p.scale_count as usize],
            reparam: vec![SplineFloatPoint::default(); p.reparam_count as usize],
        }
    }
    fn ffi(&mut self) -> &mut SplineFrame {
        self.value.position.points = self.position.as_mut_ptr();
        self.value.position.count = self.position.len() as u32;
        self.value.rotation.points = self.rotation.as_mut_ptr();
        self.value.rotation.count = self.rotation.len() as u32;
        self.value.scale.points = self.scale.as_mut_ptr();
        self.value.scale.count = self.scale.len() as u32;
        self.value.reparam.points = self.reparam.as_mut_ptr();
        self.value.reparam.count = self.reparam.len() as u32;
        &mut self.value
    }
    fn from_wire(f: &w::NativeSplineFrame) -> Result<Self, String> {
        f.validate().map_err(str::to_owned)?;
        let s = &f.settings;
        let mut out = Self {
            value: Box::new(SplineFrame {
                visible: f.visible.into(),
                hidden: f.hidden.into(),
                owner_hidden: f.owner_hidden.into(),
                version: f.version,
                settings: SplineSettings {
                    allow_spline_editing_per_instance: s.allow_spline_editing_per_instance.into(),
                    reparam_steps_per_segment: s.reparam_steps_per_segment,
                    duration: s.duration,
                    stationary_endpoints: s.stationary_endpoints.into(),
                    spline_has_been_edited: s.spline_has_been_edited.into(),
                    modified_by_construction_script: s.modified_by_construction_script.into(),
                    input_spline_points_to_construction_script: s
                        .input_spline_points_to_construction_script
                        .into(),
                    draw_debug: s.draw_debug.into(),
                    closed_loop: s.closed_loop.into(),
                    loop_position_override: s.loop_position_override.into(),
                    loop_position: s.loop_position,
                    pad: 0,
                    default_up_vector: s.default_up_vector,
                },
                ..Default::default()
            }),
            position: f
                .position
                .points
                .iter()
                .map(|p| SplineVectorPoint {
                    key: p.key,
                    interp: p.interp.into(),
                    out: p.out,
                    arrive: p.arrive,
                    leave: p.leave,
                })
                .collect(),
            rotation: f
                .rotation
                .points
                .iter()
                .map(|p| SplineQuatPoint {
                    key: p.key,
                    interp: p.interp.into(),
                    out: p.out,
                    arrive: p.arrive,
                    leave: p.leave,
                })
                .collect(),
            scale: f
                .scale
                .points
                .iter()
                .map(|p| SplineVectorPoint {
                    key: p.key,
                    interp: p.interp.into(),
                    out: p.out,
                    arrive: p.arrive,
                    leave: p.leave,
                })
                .collect(),
            reparam: f
                .reparam
                .points
                .iter()
                .map(|p| SplineFloatPoint {
                    key: p.key,
                    interp: p.interp.into(),
                    out: p.out,
                    arrive: p.arrive,
                    leave: p.leave,
                })
                .collect(),
        };
        out.value.position.looped = f.position.looped.into();
        out.value.position.loop_key_offset = f.position.loop_key_offset;
        out.value.rotation.looped = f.rotation.looped.into();
        out.value.rotation.loop_key_offset = f.rotation.loop_key_offset;
        out.value.scale.looped = f.scale.looped.into();
        out.value.scale.loop_key_offset = f.scale.loop_key_offset;
        out.value.reparam.looped = f.reparam.looped.into();
        out.value.reparam.loop_key_offset = f.reparam.loop_key_offset;
        out.ffi();
        Ok(out)
    }
    fn to_wire(&self) -> Result<w::NativeSplineFrame, String> {
        let b = |v: u32| match v {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("native spline invalid boolean".to_owned()),
        };
        let mode = |v: u32| {
            u8::try_from(v)
                .ok()
                .filter(|v| *v <= 5)
                .ok_or_else(|| "native spline invalid interpolation".to_owned())
        };
        let f = &self.value;
        let s = &f.settings;
        if f.position.count as usize != self.position.len()
            || f.rotation.count as usize != self.rotation.len()
            || f.scale.count as usize != self.scale.len()
            || f.reparam.count as usize != self.reparam.len()
        {
            return Err("native spline returned array counts".into());
        }
        let frame = w::NativeSplineFrame {
            visible: b(f.visible)?,
            hidden: b(f.hidden)?,
            owner_hidden: b(f.owner_hidden)?,
            version: f.version,
            settings: w::NativeSplineSettings {
                allow_spline_editing_per_instance: b(s.allow_spline_editing_per_instance)?,
                reparam_steps_per_segment: s.reparam_steps_per_segment,
                duration: s.duration,
                stationary_endpoints: b(s.stationary_endpoints)?,
                spline_has_been_edited: b(s.spline_has_been_edited)?,
                modified_by_construction_script: b(s.modified_by_construction_script)?,
                input_spline_points_to_construction_script: b(
                    s.input_spline_points_to_construction_script
                )?,
                draw_debug: b(s.draw_debug)?,
                closed_loop: b(s.closed_loop)?,
                loop_position_override: b(s.loop_position_override)?,
                loop_position: s.loop_position,
                default_up_vector: s.default_up_vector,
            },
            position: w::NativeSplineCurve {
                looped: b(f.position.looped)?,
                loop_key_offset: f.position.loop_key_offset,
                points: self
                    .position
                    .iter()
                    .map(|p| {
                        Ok(w::NativeSplineVectorPoint {
                            key: p.key,
                            out: p.out,
                            arrive: p.arrive,
                            leave: p.leave,
                            interp: mode(p.interp)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
            rotation: w::NativeSplineCurve {
                looped: b(f.rotation.looped)?,
                loop_key_offset: f.rotation.loop_key_offset,
                points: self
                    .rotation
                    .iter()
                    .map(|p| {
                        Ok(w::NativeSplineQuatPoint {
                            key: p.key,
                            out: p.out,
                            arrive: p.arrive,
                            leave: p.leave,
                            interp: mode(p.interp)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
            scale: w::NativeSplineCurve {
                looped: b(f.scale.looped)?,
                loop_key_offset: f.scale.loop_key_offset,
                points: self
                    .scale
                    .iter()
                    .map(|p| {
                        Ok(w::NativeSplineVectorPoint {
                            key: p.key,
                            out: p.out,
                            arrive: p.arrive,
                            leave: p.leave,
                            interp: mode(p.interp)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
            reparam: w::NativeSplineCurve {
                looped: b(f.reparam.looped)?,
                loop_key_offset: f.reparam.loop_key_offset,
                points: self
                    .reparam
                    .iter()
                    .map(|p| {
                        Ok(w::NativeSplineFloatPoint {
                            key: p.key,
                            out: p.out,
                            arrive: p.arrive,
                            leave: p.leave,
                            interp: mode(p.interp)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
        };
        frame.validate().map_err(str::to_owned)?;
        Ok(frame)
    }
}
struct FrameStorage {
    world: Transform,
    bones: Vec<Transform>,
    morphs: Vec<f32>,
    scalars: Vec<f32>,
    vectors: Vec<f32>,
    textures: Vec<Object>,
    spline: Option<SplineStorage>,
    spring_arm: Option<Box<SpringArmFrame>>,
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
            spline: c.spline_profile.as_ref().map(SplineStorage::source),
            spring_arm: matches!(c.scene, d::SceneEvidence::SpringArm { .. })
                .then(|| Box::new(SpringArmFrame::default())),
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
            spline: self
                .spline
                .as_mut()
                .map(|s| s.ffi() as *mut SplineFrame)
                .unwrap_or(std::ptr::null_mut()),
            spring_arm: self
                .spring_arm
                .as_mut()
                .map(|a| &mut **a as *mut SpringArmFrame)
                .unwrap_or(std::ptr::null_mut()),
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
type RosterName = unsafe extern "C" fn(*const c_void) -> *const u64;
type RosterFlags = unsafe extern "C" fn(*const c_void) -> *const u32;
type RosterWorld = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type RosterOuter = unsafe extern "C" fn(*const c_void) -> *const *const c_void;
#[derive(Clone, Copy)]
struct RosterExports {
    name: RosterName,
    flags: RosterFlags,
    world: RosterWorld,
    outer: RosterOuter,
}
#[cfg(windows)]
fn roster_exports() -> Result<RosterExports, String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    }
    unsafe {
        let module = GetModuleHandleW(reflect::wide("UE4SS.dll").as_ptr());
        if module.is_null() {
            return Err("source roster native metadata unavailable".into());
        }
        let get = |name: &std::ffi::CStr| -> Result<*mut c_void, String> {
            let p = GetProcAddress(module, name.as_ptr());
            if p.is_null() {
                Err("source roster native exports unavailable".into())
            } else {
                Ok(p)
            }
        };
        Ok(RosterExports {
            name: std::mem::transmute::<*mut c_void, RosterName>(get(
                c"?GetNamePrivate@UObjectBase@Unreal@RC@@QEBAAEBVFName@23@XZ",
            )?),
            flags: std::mem::transmute::<*mut c_void, RosterFlags>(get(
                c"?GetObjectFlags@UObjectBase@Unreal@RC@@QEBAAEBW4EObjectFlags@23@XZ",
            )?),
            world: std::mem::transmute::<*mut c_void, RosterWorld>(get(
                c"?GetWorld@UObject@Unreal@RC@@QEBAPEAVUWorld@23@XZ",
            )?),
            outer: std::mem::transmute::<*mut c_void, RosterOuter>(get(
                c"?GetOuterPrivate@UObjectBase@Unreal@RC@@QEBAAEAPEBVUObject@23@XZ",
            )?),
        })
    }
}
struct RosterPath(Vec<RosterIdentity>);
impl RosterPath {
    unsafe fn capture(
        vt: &HsmpReflect,
        x: RosterExports,
        root: RosterIdentity,
    ) -> Result<Self, String> {
        unsafe {
            let mut nodes = vec![root];
            loop {
                let last = *nodes.last().ok_or("source roster hierarchy empty")?;
                let field = (x.outer)(last.verify(vt, x)?);
                if field.is_null() {
                    return Err("source roster hierarchy getter unavailable".into());
                }
                let next = *field;
                last.verify(vt, x)?;
                if next.is_null() {
                    break;
                }
                if nodes.len() >= 64 || nodes.iter().any(|n| n.object.address == next as u64) {
                    return Err("source roster hierarchy bound/cycle".into());
                }
                nodes.push(RosterIdentity::capture(vt, x, object(vt, next as i64)?)?);
            }
            let path = Self(nodes);
            path.verify(vt, x)?;
            Ok(path)
        }
    }
    unsafe fn verify(&self, vt: &HsmpReflect, x: RosterExports) -> Result<(), String> {
        unsafe {
            for (i, node) in self.0.iter().enumerate() {
                let field = (x.outer)(node.verify(vt, x)?);
                if field.is_null()
                    || *field as u64 != self.0.get(i + 1).map_or(0, |n| n.object.address)
                {
                    return Err("source roster original hierarchy changed".into());
                }
                node.verify(vt, x)?;
            }
            Ok(())
        }
    }
}
#[cfg(not(windows))]
fn roster_exports() -> Result<RosterExports, String> {
    Err("source roster native exports unavailable".into())
}
#[derive(Clone, Copy)]
struct RosterIdentity {
    object: Object,
    class: Object,
    name: u64,
    class_name: u64,
}
impl RosterIdentity {
    unsafe fn live(vt: &HsmpReflect, x: RosterExports, o: Object) -> Result<*mut c_void, String> {
        unsafe {
            let p = reflect::get(vt, o.weak);
            if o.weak == 0 || p.is_null() || p as u64 != o.address {
                return Err("source roster original slot/address changed".into());
            }
            let flags = (x.flags)(p);
            if flags.is_null() || *flags & 0x4000_0000 != 0 {
                return Err("source roster original object garbage".into());
            }
            Ok(p)
        }
    }
    unsafe fn capture(vt: &HsmpReflect, x: RosterExports, o: Object) -> Result<Self, String> {
        unsafe {
            let p = Self::live(vt, x, o)?;
            let class = object(vt, (vt.class_of)(p) as i64)?;
            let cp = Self::live(vt, x, class)?;
            let name = (x.name)(p);
            let class_name = (x.name)(cp);
            if name.is_null() || class_name.is_null() {
                return Err("source roster FName unavailable".into());
            }
            let out = Self {
                object: o,
                class,
                name: *name,
                class_name: *class_name,
            };
            out.verify(vt, x)?;
            Ok(out)
        }
    }
    unsafe fn verify(self, vt: &HsmpReflect, x: RosterExports) -> Result<*mut c_void, String> {
        unsafe {
            let p = Self::live(vt, x, self.object)?;
            let cp = Self::live(vt, x, self.class)?;
            let name = (x.name)(p);
            let class_name = (x.name)(cp);
            if name.is_null()
                || class_name.is_null()
                || *name != self.name
                || *class_name != self.class_name
                || (vt.class_of)(p) != cp
            {
                return Err("source roster original FName/class changed".into());
            }
            Ok(p)
        }
    }
}
unsafe fn roster_property(
    vt: &HsmpReflect,
    x: RosterExports,
    o: RosterIdentity,
    name: &str,
    kind: &str,
    width: i32,
) -> Result<reflect::HsmpProp, String> {
    unsafe {
        let pointer = o.verify(vt, x)?;
        let mut p = reflect::HsmpProp::default();
        let mut size = 0;
        let mut scratch = reflect::HsmpProp::default();
        let count = (vt.props)(o.class.address as *mut c_void, &mut scratch, 1, &mut size);
        if count < 0
            || count > 4096
            || (vt.obj_prop)(pointer, reflect::wide(name).as_ptr(), &mut p) != 1
            || p.name != (vt.fname)(reflect::wide(name).as_ptr(), 1)
            || p.cls != (vt.fname)(reflect::wide(kind).as_ptr(), 1)
            || p.size != width
            || p.offset < 0
            || p.offset.checked_add(width).is_none_or(|end| end > size)
        {
            return Err(format!("source roster {name} property ABI"));
        }
        o.verify(vt, x)?;
        Ok(p)
    }
}
unsafe fn roster_read_i32(
    vt: &HsmpReflect,
    x: RosterExports,
    o: RosterIdentity,
    p: reflect::HsmpProp,
) -> Result<i32, String> {
    unsafe {
        let pointer = o.verify(vt, x)?;
        let value =
            std::ptr::read_unaligned((pointer as *const u8).add(p.offset as usize).cast::<i32>());
        o.verify(vt, x)?;
        Ok(value)
    }
}
unsafe fn roster_read_pointer(
    vt: &HsmpReflect,
    x: RosterExports,
    o: RosterIdentity,
    p: reflect::HsmpProp,
) -> Result<u64, String> {
    unsafe {
        let pointer = o.verify(vt, x)?;
        let value =
            std::ptr::read_unaligned((pointer as *const u8).add(p.offset as usize).cast::<u64>());
        o.verify(vt, x)?;
        Ok(value)
    }
}
struct RosterBinding {
    entity: w::Entity,
    world: RosterIdentity,
    pawn: RosterIdentity,
    controller: RosterIdentity,
    pawn_controller: reflect::HsmpProp,
    controller_pawn: reflect::HsmpProp,
    team: reflect::HsmpProp,
    paths: [RosterPath; 3],
    indexed: Option<RosterSlots>,
}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct RosterArray {
    data: u64,
    count: i32,
    capacity: i32,
}
struct RosterSlots {
    instance: RosterIdentity,
    instance_path: RosterPath,
    world_instance: reflect::HsmpProp,
    players: reflect::HsmpProp,
    header: RosterArray,
    slots: Vec<(
        RosterIdentity,
        RosterPath,
        reflect::HsmpProp,
        RosterIdentity,
    )>,
    index: usize,
    controller_player: reflect::HsmpProp,
    world_context: u64,
}
#[cfg(windows)]
fn roster_mapping_build() -> Result<(), String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    unsafe {
        let image = GetModuleHandleW(std::ptr::null()).cast::<u8>();
        if image.is_null() || std::ptr::read_unaligned(image.cast::<u16>()) != 0x5a4d {
            return Err("source roster unsupported native image".into());
        }
        let pe = std::ptr::read_unaligned(image.add(0x3c).cast::<u32>()) as usize;
        if pe > 0x1000 || std::ptr::read_unaligned(image.add(pe).cast::<u32>()) != 0x4550 {
            return Err("source roster unsupported native image".into());
        }
        let size = std::ptr::read_unaligned(image.add(pe + 24 + 56).cast::<u32>()) as usize;
        for (rva, bytes) in [
            (
                0x35cf760,
                &[
                    0x48, 0x8b, 0x41, 0x30, 0x48, 0x85, 0xc0, 0x74, 0x08, 0x48, 0x8b, 0x80, 0xc0,
                    0x02, 0x00, 0x00, 0xc3, 0xc3,
                ][..],
            ),
            (
                0x35e1b80,
                &[
                    0x48, 0x8b, 0x01, 0x48, 0x8b, 0x40, 0x30, 0x48, 0x85, 0xc0, 0x74, 0x0a, 0x3b,
                    0xdd, 0x0f, 0x84, 0xc2, 0x00, 0x00, 0x00, 0xff, 0xc3, 0x48, 0x83, 0xc1, 0x08,
                    0x48, 0x3b, 0xca, 0x75, 0xe1,
                ][..],
            ),
        ] {
            if rva + bytes.len() > size
                || std::slice::from_raw_parts(image.add(rva), bytes.len()) != bytes
            {
                return Err("source roster unsupported native player mapping".into());
            }
        }
        Ok(())
    }
}
#[cfg(not(windows))]
fn roster_mapping_build() -> Result<(), String> {
    Err("source roster unsupported native player mapping".into())
}
unsafe fn roster_array(
    vt: &HsmpReflect,
    x: RosterExports,
    o: RosterIdentity,
    p: reflect::HsmpProp,
) -> Result<RosterArray, String> {
    unsafe {
        let pointer = o.verify(vt, x)?;
        let header = std::ptr::read_unaligned(
            (pointer as *const u8)
                .add(p.offset as usize)
                .cast::<RosterArray>(),
        );
        if header.count <= 0
            || header.count as usize > w::MAX_ENTITIES
            || header.capacity < header.count
            || header.capacity as usize > w::MAX_ENTITIES
            || header.data == 0
        {
            return Err("source roster local-player array bounds".into());
        }
        o.verify(vt, x)?;
        Ok(header)
    }
}
impl RosterSlots {
    unsafe fn capture(
        vt: &HsmpReflect,
        x: RosterExports,
        world: RosterIdentity,
        controller: RosterIdentity,
        index: u8,
    ) -> Result<Self, String> {
        unsafe {
            roster_mapping_build()?;
            let world_instance =
                roster_property(vt, x, world, "OwningGameInstance", "ObjectProperty", 8)?;
            let instance = RosterIdentity::capture(
                vt,
                x,
                object(
                    vt,
                    roster_read_pointer(vt, x, world, world_instance)? as i64,
                )?,
            )?;
            let players = roster_property(vt, x, instance, "LocalPlayers", "ArrayProperty", 16)?;
            if world_instance.offset != 0x1d8 || players.offset != 0x38 {
                return Err("source roster local-player layout".into());
            }
            let header = roster_array(vt, x, instance, players)?;
            if usize::from(index) >= header.count as usize {
                return Err("source roster original indexed slot unavailable".into());
            }
            let mut slots = Vec::with_capacity(header.count as usize);
            for i in 0..header.count as usize {
                let address = std::ptr::read_unaligned((header.data as *const u64).add(i));
                let player = RosterIdentity::capture(vt, x, object(vt, address as i64)?)?;
                let pc = roster_property(vt, x, player, "PlayerController", "ObjectProperty", 8)?;
                if pc.offset != 0x30 {
                    return Err("source roster local-player controller layout".into());
                }
                let original_pc = RosterIdentity::capture(
                    vt,
                    x,
                    object(vt, roster_read_pointer(vt, x, player, pc)? as i64)?,
                )?;
                // GetPlayerController counts only nonnull local controllers.
                // This profile admits a complete dense array, never fallback.
                slots.push((player, RosterPath::capture(vt, x, player)?, pc, original_pc));
            }
            let controller_player =
                roster_property(vt, x, controller, "Player", "ObjectProperty", 8)?;
            if controller_player.offset != 0x330 {
                return Err("source roster controller player layout".into());
            }
            let instance_pointer = instance.verify(vt, x)?;
            let world_context =
                std::ptr::read_unaligned((instance_pointer as *const u8).add(0x30).cast::<u64>());
            if world_context == 0
                || std::ptr::read_unaligned((world_context as *const u8).add(0x2c0).cast::<u64>())
                    != world.object.address
            {
                return Err("source roster original game-instance world context".into());
            }
            let out = Self {
                instance,
                instance_path: RosterPath::capture(vt, x, instance)?,
                world_instance,
                players,
                header,
                slots,
                index: usize::from(index),
                controller_player,
                world_context,
            };
            out.verify(vt, x, world, controller)?;
            Ok(out)
        }
    }
    unsafe fn verify(
        &self,
        vt: &HsmpReflect,
        x: RosterExports,
        world: RosterIdentity,
        controller: RosterIdentity,
    ) -> Result<(), String> {
        unsafe {
            self.instance_path.verify(vt, x)?;
            let pointer = self.instance.verify(vt, x)?;
            if std::ptr::read_unaligned((pointer as *const u8).add(0x30).cast::<u64>())
                != self.world_context
                || std::ptr::read_unaligned(
                    (self.world_context as *const u8).add(0x2c0).cast::<u64>(),
                ) != world.object.address
            {
                return Err("source roster original game-instance world context changed".into());
            }
            if roster_read_pointer(vt, x, world, self.world_instance)?
                != self.instance.object.address
                || roster_array(vt, x, self.instance, self.players)? != self.header
            {
                return Err("source roster original local-player array changed".into());
            }
            for (i, (player, path, pc, original_pc)) in self.slots.iter().enumerate() {
                path.verify(vt, x)?;
                original_pc.verify(vt, x)?;
                if std::ptr::read_unaligned((self.header.data as *const u64).add(i))
                    != player.object.address
                {
                    return Err("source roster original indexed player changed".into());
                }
                if roster_read_pointer(vt, x, *player, *pc)? != original_pc.object.address {
                    return Err("source roster original local controller prefix changed".into());
                }
            }
            let (player, _, pc, _) = &self.slots[self.index];
            if roster_read_pointer(vt, x, *player, *pc)? != controller.object.address
                || roster_read_pointer(vt, x, controller, self.controller_player)?
                    != player.object.address
            {
                return Err("source roster original indexed controller changed".into());
            }
            Ok(())
        }
    }
}
impl RosterBinding {
    unsafe fn pure_team(&self, vt: &HsmpReflect, x: RosterExports) -> Result<i32, String> {
        unsafe {
            for path in &self.paths {
                path.verify(vt, x)?;
            }
            if let Some(indexed) = &self.indexed {
                indexed.verify(vt, x, self.world, self.controller)?;
            }
            self.world.verify(vt, x)?;
            if roster_read_pointer(vt, x, self.pawn, self.pawn_controller)?
                != self.controller.object.address
                || roster_read_pointer(vt, x, self.controller, self.controller_pawn)?
                    != self.pawn.object.address
            {
                return Err("source roster original possession changed".into());
            }
            roster_read_i32(vt, x, self.pawn, self.team)
        }
    }
    unsafe fn qualify(
        &self,
        vt: &HsmpReflect,
        x: RosterExports,
        mut admit: impl FnMut() -> Result<(), String>,
    ) -> Result<i32, String> {
        unsafe {
            self.pure_team(vt, x)?;
            for o in [self.pawn, self.controller] {
                admit()?;
                let p = o.verify(vt, x)?;
                let world = (x.world)(p) as u64;
                o.verify(vt, x)?;
                admit()?;
                if world != self.world.object.address {
                    return Err("source roster native world changed".into());
                }
            }
            if self.entity.kind == w::HUMAN
                && roster_assigned_controller(
                    vt,
                    x,
                    self.world,
                    self.entity.controller,
                    &mut admit,
                )? != self.controller.object.address
            {
                return Err("source roster native indexed controller changed".into());
            }
            self.pure_team(vt, x)
        }
    }
}
unsafe fn roster_assigned_controller(
    vt: &HsmpReflect,
    x: RosterExports,
    world: RosterIdentity,
    index: u8,
    mut admit: impl FnMut() -> Result<(), String>,
) -> Result<u64, String> {
    unsafe {
        admit()?;
        let function = RosterIdentity::capture(
            vt,
            x,
            object(
                vt,
                (vt.find)(
                    reflect::wide("/Script/Engine.GameplayStatics:GetPlayerController").as_ptr(),
                ) as i64,
            )?,
        )?;
        admit()?;
        let target = RosterIdentity::capture(
            vt,
            x,
            object(
                vt,
                (vt.find)(reflect::wide("/Script/Engine.Default__GameplayStatics").as_ptr()) as i64,
            )?,
        )?;
        admit()?;
        let (props, size) = crate::sample::props_of(vt, function.verify(vt, x)?);
        if size != 24 || props.len() != 3 {
            return Err("source roster indexed controller ABI".into());
        }
        for (name, kind, offset, width) in [
            ("WorldContextObject", "ObjectProperty", 0, 8),
            ("PlayerIndex", "IntProperty", 8, 4),
            ("ReturnValue", "ObjectProperty", 16, 8),
        ] {
            if !props.iter().any(|p| {
                p.name == (vt.fname)(reflect::wide(name).as_ptr(), 1)
                    && p.cls == (vt.fname)(reflect::wide(kind).as_ptr(), 1)
                    && p.offset == offset
                    && p.size == width
            }) {
                return Err("source roster indexed controller fields".into());
            }
        }
        let mut params = crate::sample::Params([0; crate::sample::PARAMS_BYTES]);
        params.0[..8].copy_from_slice(&world.object.address.to_le_bytes());
        params.0[8..12].copy_from_slice(&(index as i32).to_le_bytes());
        admit()?;
        world.verify(vt, x)?;
        let target_pointer = target.verify(vt, x)?;
        let function_pointer = function.verify(vt, x)?;
        (vt.call)(
            target_pointer,
            function_pointer,
            params.0.as_mut_ptr().cast(),
        );
        world.verify(vt, x)?;
        target.verify(vt, x)?;
        function.verify(vt, x)?;
        admit()?;
        Ok(u64::from_le_bytes(
            params.0[16..24]
                .try_into()
                .map_err(|_| "source roster indexed controller result")?,
        ))
    }
}
fn check_roster_teams(
    expected: &[i32],
    mut read: impl FnMut(usize) -> Result<i32, String>,
) -> Result<(), String> {
    for (i, team) in expected.iter().enumerate() {
        if read(i)? != *team {
            return Err("source roster native team changed".into());
        }
    }
    Ok(())
}
fn require_acknowledged_team(known: Option<i32>, recipe: i32, native: i32) -> Result<(), String> {
    if known != Some(recipe) || native != recipe {
        return Err("source descriptor native team is not acknowledged".into());
    }
    Ok(())
}
fn roster_admission(
    n: &Native,
    vt: &HsmpReflect,
    d: &w::Directory,
    key: &[u8],
) -> Result<(), String> {
    if n.poisoned
        || !n.native_host.is_host()
        || !n.sample.world_ok
        || n.game_thread != Some(std::thread::current().id())
        || n.world_key.as_deref() != Some(key)
        || !n.native_guard_ok(vt)
    {
        return Err("source roster role/thread/world admission".into());
    }
    if !n.native_host.directory().is_some_and(|current| {
        current.epoch == d.epoch && current.seq == d.seq && current.entities == d.entities
    }) {
        return Err("source roster directory generation changed".into());
    }
    Ok(())
}
// The callback borrows Native only within a synchronous provider invocation.
// No Lua callbacks or global Native lock acquisition occur here.
struct GuardContext {
    native: *const Native,
    vt: *const HsmpReflect,
    world: Object,
    key: Vec<u8>,
    source: Vec<(Object, Object, reflect::HsmpProp, reflect::HsmpProp)>,
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
            source: source
                .map(|s| vec![(s.pawn, s.controller, s.pawn_controller, s.controller_pawn)])
                .unwrap_or_default(),
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
            for &(pawn, controller, pc, cp) in &self.source {
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
        lua_pushboolean(L, scene.fresh() as c_int);
        rawset_str(L, t, "fresh");
        let generation = scene.generation();
        lua_pushlstring(L, generation.as_ptr().cast(), generation.len());
        rawset_str(L, t, "generation");
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
    /// Complete native-asset proof after all component/native/Lua callbacks.
    /// The provider's final whole-set census and this final guard are pure;
    /// publication must perform no engine getter after successful return.
    pub(crate) unsafe fn finish_native_vertices(
        &self,
        render: &w::RenderWorld,
    ) -> Result<(), String> {
        unsafe {
            if !self.native_host.is_host() || !self.sample.world_ok {
                return Err("source vertex role/world".into());
            }
            let directory = self
                .native_host
                .directory()
                .ok_or("source vertex directory")?;
            if directory.epoch != render.world.epoch || directory.seq != render.world.directory_seq
            {
                return Err("source vertex directory changed".into());
            }
            let p = provider()?;
            let vt = reflect::vt().ok_or("reflection unavailable")?;
            let mut targets = Vec::new();
            let mut original_world: Option<Object> = None;
            let mut originals = Vec::new();
            for entity in &render.entities {
                if !directory
                    .entities
                    .iter()
                    .any(|e| e.reference == entity.reference)
                {
                    return Err("source vertex entity generation".into());
                }
                let binding = self
                    .presentation
                    .sources
                    .get(&entity.reference)
                    .ok_or("source vertex binding")?;
                let descriptor = self
                    .native_host
                    .host
                    .as_ref()
                    .ok_or("source vertex host")?
                    .descriptor(entity.reference.id)
                    .ok_or("source vertex descriptor")?;
                if descriptor.reference != entity.reference
                    || descriptor.revision != entity.revision
                    || descriptor.directory_seq != render.world.directory_seq
                {
                    return Err("source vertex descriptor generation".into());
                }
                if original_world.is_some_and(|w| {
                    w.weak != binding.world.weak || w.address != binding.world.address
                }) {
                    return Err("source vertex mixed worlds".into());
                }
                original_world = Some(binding.world);
                originals.push((
                    binding.pawn,
                    binding.controller,
                    binding.pawn_controller,
                    binding.controller_pawn,
                ));
                targets.extend(binding.vertices.iter().map(|v| FinishTarget {
                    owner: v.owner,
                    component: v.component,
                    asset: v.asset,
                    scene_kind: v.scene_kind,
                    ..FinishTarget::default()
                }));
                for (index, c) in descriptor.recipe.components.iter().enumerate() {
                    let scene_kind = match &c.scene {
                        d::SceneEvidence::Camera => 6,
                        d::SceneEvidence::SpringArm { .. } => 7,
                        _ => continue,
                    };
                    let (component, owner) = *binding
                        .components
                        .get(&c.id)
                        .ok_or("scene finish component")?;
                    let frame = entity
                        .components
                        .iter()
                        .find(|f| f.id == c.id)
                        .ok_or("scene finish frame")?;
                    if (scene_kind == 7) != frame.spring_arm.is_some() {
                        return Err("source spring arm frame presence".into());
                    }
                    let arm = match &frame.spring_arm {
                        Some(a) => {
                            a.validate().map_err(str::to_owned)?;
                            SpringArmFrame {
                                translation: a.translation,
                                rotation: a.rotation,
                            }
                        }
                        None => SpringArmFrame::default(),
                    };
                    targets.push(FinishTarget {
                        owner,
                        component,
                        scene_kind,
                        socket: binding.prepared.components[index].spring_arm_socket,
                        arm,
                        ..FinishTarget::default()
                    });
                }
            }
            if originals.len() > w::MAX_ENTITIES || targets.len() > w::MAX_ENTITIES * 64 {
                return Err("source vertex complete-set bounds".into());
            }
            let world = original_world.ok_or("source vertex empty world")?;
            let mut context = GuardContext::new(self, vt, world, None)?;
            context.source = originals;
            let guard = context.ffi();
            let mut r = ResultInfo::default();
            if !context.valid()
                || (p.finish_scene_sets)(
                    world,
                    targets.as_ptr(),
                    targets.len() as u32,
                    std::ptr::null(),
                    0,
                    &guard,
                    &mut r,
                ) != 1
                || r.complete != 1
                || !context.valid()
            {
                return Err(format!("source whole-set vertex proof: {}", r.reason()));
            }
            let latest = self
                .native_host
                .directory()
                .ok_or("source vertex final directory")?;
            if latest.epoch != render.world.epoch
                || latest.seq != render.world.directory_seq
                || !self.native_host.is_host()
                || !self.sample.world_ok
            {
                return Err("source vertex final generation".into());
            }
            Ok(())
        }
    }
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
                        if matches!(c.kind, d::ComponentKind::Scene | d::ComponentKind::Spline) {
                            assets.insert(c.component_class.clone());
                        }
                        for s in [&c.asset, &c.skeleton] {
                            if !s.is_empty() {
                                assets.insert(s.clone());
                            }
                        }
                        for m in &c.materials {
                            if !m.base.is_empty() {
                                assets.insert(m.base.clone());
                            }
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
                let scope = scene.generation();
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
    pub unsafe fn native_source_roster_facts(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result =
                (|| -> Result<(hsmp_server::native_service::SourceRosterFacts, u32), String> {
                    if !is_table(L, 1) || !is_table(L, 2) {
                        return Err("source roster metadata/bindings".into());
                    }
                    if self.poisoned
                        || !self.native_host.is_host()
                        || !self.sample.world_ok
                        || self.game_thread != Some(std::thread::current().id())
                    {
                        return Err("source roster role/thread/world admission".into());
                    }
                    let directory = self
                        .native_host
                        .directory()
                        .ok_or("source roster directory unavailable")?;
                    if integer(L, 1, "epoch").map(|v| v as u64) != Some(directory.epoch)
                        || integer(L, 1, "dir_seq").and_then(|v| u32::try_from(v).ok())
                            != Some(directory.seq)
                    {
                        return Err("source roster original directory".into());
                    }
                    let vt = reflect::vt().ok_or("source roster reflection unavailable")?;
                    let key = self
                        .world_key
                        .clone()
                        .ok_or("source roster world token unavailable")?;
                    roster_admission(self, vt, &directory, &key)?;
                    let x = roster_exports()?;
                    let count = lua_rawlen(L, 2) as usize;
                    if count != directory.entities.len() || count == 0 || count > w::MAX_ENTITIES {
                        return Err("source roster complete binding count".into());
                    }
                    let mut bindings = Vec::with_capacity(count);
                    for i in 1..=count {
                        lua_rawgeti(L, 2, i as i64);
                        let row = lua_absindex(L, -1);
                        let uint = |name| {
                            integer(L, row, name)
                                .and_then(|v| u32::try_from(v).ok())
                                .ok_or_else(|| format!("source roster {name}"))
                        };
                        let reference = w::EntityRef {
                            epoch: integer(L, row, "epoch").ok_or("source roster epoch")? as u64,
                            id: uint("id")?,
                            incarnation: uint("incarnation")?,
                        };
                        let entity = directory
                            .entities
                            .iter()
                            .find(|e| {
                                e.reference == reference
                                    && u32::from(e.slot) == uint("slot").unwrap_or(u32::MAX)
                                    && u32::from(e.kind) == uint("kind").unwrap_or(u32::MAX)
                                    && u32::from(e.controller)
                                        == uint("controller_index").unwrap_or(u32::MAX)
                            })
                            .ok_or("source roster original entity binding")?
                            .clone();
                        if bindings
                            .iter()
                            .any(|b: &RosterBinding| b.entity.reference == reference)
                        {
                            return Err("source roster duplicate entity".into());
                        }
                        roster_admission(self, vt, &directory, &key)?;
                        let world =
                            RosterIdentity::capture(vt, x, field_object(vt, L, row, "world")?)?;
                        let pawn =
                            RosterIdentity::capture(vt, x, field_object(vt, L, row, "pawn")?)?;
                        let controller = RosterIdentity::capture(
                            vt,
                            x,
                            field_object(vt, L, row, "controller")?,
                        )?;
                        if bindings.first().is_some_and(|b: &RosterBinding| {
                            b.world.object.address != world.object.address
                                || b.world.object.weak != world.object.weak
                        }) {
                            return Err("source roster mixed native worlds".into());
                        }
                        let pawn_controller =
                            roster_property(vt, x, pawn, "Controller", "ObjectProperty", 8)?;
                        let controller_pawn =
                            roster_property(vt, x, controller, "Pawn", "ObjectProperty", 8)?;
                        let team = roster_property(vt, x, pawn, "Team Int", "IntProperty", 4)?;
                        let paths = [
                            RosterPath::capture(vt, x, world)?,
                            RosterPath::capture(vt, x, pawn)?,
                            RosterPath::capture(vt, x, controller)?,
                        ];
                        let indexed = if entity.kind == w::HUMAN {
                            Some(RosterSlots::capture(
                                vt,
                                x,
                                world,
                                controller,
                                entity.controller,
                            )?)
                        } else {
                            None
                        };
                        bindings.push(RosterBinding {
                            entity,
                            world,
                            pawn,
                            controller,
                            pawn_controller,
                            controller_pawn,
                            team,
                            paths,
                            indexed,
                        });
                        pop(L, 1);
                    }
                    let teams = bindings
                        .iter()
                        .map(|b| b.qualify(vt, x, || roster_admission(self, vt, &directory, &key)))
                        .collect::<Result<Vec<_>, _>>()?;
                    // GetWorld and indexed-controller callbacks for later actors may
                    // mutate an earlier actor. Finish with fresh, callback-free
                    // original identities, possession and every observed team.
                    roster_admission(self, vt, &directory, &key)?;
                    check_roster_teams(&teams, |i| bindings[i].pure_team(vt, x))?;
                    let facts = hsmp_server::native_service::SourceRosterFacts {
                        epoch: directory.epoch,
                        directory_seq: directory.seq,
                        entities: bindings
                            .iter()
                            .zip(teams)
                            .map(|(b, team)| {
                                let mut e = b.entity.clone();
                                e.team = Some(team);
                                e
                            })
                            .collect(),
                    };
                    let ack_seq = self
                        .native_host
                        .host
                        .as_ref()
                        .ok_or("not a native source host")?
                        .source_roster(facts.clone())
                        .map_err(str::to_owned)?;
                    Ok((facts, ack_seq))
                })();
            match result {
                Err(e) => nil_err(L, &e),
                Ok((facts, ack_seq)) => {
                    lua_pushboolean(L, 1);
                    lua_createtable(L, 0, 4);
                    let t = lua_gettop(L);
                    set_int(L, t, "epoch", facts.epoch as i64);
                    set_int(L, t, "base_dir_seq", facts.directory_seq as i64);
                    set_int(L, t, "ack_dir_seq", i64::from(ack_seq));
                    lua_createtable(L, facts.entities.len() as c_int, 0);
                    let rows = lua_gettop(L);
                    for (i, e) in facts.entities.iter().enumerate() {
                        lua_createtable(L, 0, 7);
                        let row = lua_gettop(L);
                        set_int(L, row, "epoch", e.reference.epoch as i64);
                        set_int(L, row, "id", e.reference.id as i64);
                        set_int(L, row, "incarnation", e.reference.incarnation as i64);
                        set_int(L, row, "slot", i64::from(e.slot));
                        set_int(L, row, "kind", i64::from(e.kind));
                        set_int(L, row, "controller", i64::from(e.controller));
                        let Some(team) = e.team else {
                            return nil_err(L, "source roster native team unavailable");
                        };
                        set_int(L, row, "team", i64::from(team));
                        lua_rawseti(L, rows, i as i64 + 1);
                    }
                    rawset_str(L, t, "entities");
                    2
                }
            }
        }
    }
    pub unsafe fn host_describe(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let result = (|| -> Result<String, String> {
                if !self.native_host.is_host() || !self.sample.world_ok {
                    return Err("source role/world".into());
                }
                if !is_table(L, 1) || !is_table(L, 3) {
                    return Err("descriptor metadata/bindings".into());
                }
                let recipe = read_recipe(L, 2)?;
                let encoding = recipe.encoding_stats().map_err(str::to_owned)?.diagnostic();
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
                let team_exports = roster_exports()?;
                let team_pawn = RosterIdentity::capture(vt, team_exports, pawn)?;
                let team_property =
                    roster_property(vt, team_exports, team_pawn, "Team Int", "IntProperty", 4)?;
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
                require_acknowledged_team(
                    entity.team,
                    descriptor.recipe.team,
                    roster_read_i32(vt, team_exports, team_pawn, team_property)?,
                )?;
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
                let mut vertices = Vec::new();
                for c in &descriptor.recipe.components {
                    let empty = c.geometry == d::Geometry::NativeEmpty;
                    if c.vertex_state != d::VertexState::NativeAsset && !empty {
                        continue;
                    }
                    if c.kind != d::ComponentKind::Static
                        && !(empty && c.kind == d::ComponentKind::Skeletal)
                    {
                        return Err("native asset proof requires cooked static component".into());
                    }
                    let (component, owner) = *components.get(&c.id).ok_or("vertex component")?;
                    let asset = if empty {
                        if c.vertex_state != d::VertexState::NotApplicable || !c.asset.is_empty() {
                            return Err("source empty static profile".into());
                        }
                        Object::default()
                    } else {
                        let expected = (vt.find)(reflect::wide(&c.asset).as_ptr());
                        object(vt, expected as i64)?
                    };
                    let skeletal = c.kind == d::ComponentKind::Skeletal;
                    if object_field(
                        vt,
                        component,
                        if skeletal {
                            "SkinnedAsset"
                        } else {
                            "StaticMesh"
                        },
                    )? != asset.address
                        || (skeletal && object_field(vt, component, "SkeletalMesh")? != 0)
                    {
                        return Err("source original vertex asset binding".into());
                    }
                    vertices.push(VertexTarget {
                        owner,
                        component,
                        asset,
                        scene_kind: if empty {
                            if skeletal {
                                9
                            } else {
                                8
                            }
                        } else {
                            0
                        },
                    });
                }
                let pawn_controller = object_property(vt, pawn, "Controller")?;
                let controller_pawn = object_property(vt, controller, "Pawn")?;
                if read_object_property(vt, pawn, pawn_controller)? != controller.address
                    || read_object_property(vt, controller, controller_pawn)? != pawn.address
                {
                    return Err("source possession changed during descriptor copy".into());
                }
                let host = self.native_host.host.as_ref().ok_or("not a source host")?;
                if !host.directory().is_some_and(|d| {
                    d.epoch == directory.epoch
                        && d.seq == directory.seq
                        && d.entities.iter().any(|e| {
                            e.reference == reference && e.team == Some(descriptor.recipe.team)
                        })
                }) || roster_read_i32(vt, team_exports, team_pawn, team_property)?
                    != descriptor.recipe.team
                {
                    return Err("source descriptor native team changed during copy".into());
                }
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
                        vertices,
                        prepared,
                    },
                );
                Ok(encoding)
            })();
            match result {
                Ok(encoding) => {
                    lua_pushboolean(L, 1);
                    lua_pushlstring(L, encoding.as_ptr().cast(), encoding.len());
                    2
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
                let mut recipes = Vec::new();
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
                        let spline = values
                            .spline
                            .as_ref()
                            .map(SplineStorage::to_wire)
                            .transpose()?;
                        components.push(w::RenderComponent {
                            id: c.id,
                            transform: numbers(values.world),
                            bones: values.bones.into_iter().map(numbers).collect(),
                            morphs: values.morphs,
                            materials,
                            spline,
                            spring_arm: values.spring_arm.as_ref().map(|a| {
                                w::NativeSpringArmFrame {
                                    translation: a.translation,
                                    rotation: a.rotation,
                                }
                            }),
                        });
                    }
                    entities.push(w::RenderEntity {
                        reference: entity.reference,
                        revision: desc.revision,
                        components,
                    });
                    recipes.push(desc);
                }
                let render = w::RenderWorld { world, entities };
                w::scene_stream::encode_scene(&render, &recipes).map_err(str::to_owned)?;
                self.finish_native_vertices(&render)?;
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
                if scene.directory.state == w::LIVE && !scene.fresh() {
                    return Err("native applied scene is stale".into());
                }
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
                        if (recipe.kind == d::ComponentKind::Spline) != frame.spline.is_some() {
                            return Err("mirror spline frame presence".into());
                        }
                        values.spline = frame
                            .spline
                            .as_ref()
                            .map(SplineStorage::from_wire)
                            .transpose()?;
                        if matches!(recipe.scene, d::SceneEvidence::SpringArm { .. })
                            != frame.spring_arm.is_some()
                        {
                            return Err("mirror spring arm frame presence".into());
                        }
                        values.spring_arm = frame
                            .spring_arm
                            .as_ref()
                            .map(|a| {
                                a.validate().map_err(str::to_owned)?;
                                Ok::<_, String>(Box::new(SpringArmFrame {
                                    translation: a.translation,
                                    rotation: a.rotation,
                                }))
                            })
                            .transpose()?;
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
                let handles = scene
                    .descriptors
                    .iter()
                    .map(|d| {
                        self.presentation
                            .mirrors
                            .get(&d.reference.id)
                            .map(|m| m.handle)
                            .ok_or("complete native mirror set")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut r = ResultInfo::default();
                if !context.valid()
                    || (p.finish_scene_sets)(
                        world,
                        std::ptr::null(),
                        0,
                        handles.as_ptr(),
                        handles.len() as u32,
                        &guard,
                        &mut r,
                    ) != 1
                    || r.complete != 1
                    || !context.valid()
                {
                    return Err(format!("mirror whole-set vertex proof: {}", r.reason()));
                }
                let ready = (scene.directory.epoch, scene.directory.seq, valid.clone());
                client.native_applied(&scene).map_err(str::to_owned)?;
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
    pub unsafe fn native_actor_scope(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() {
                return nil_err(L, "client actor scope role required");
            }
            let result = (|| -> Result<(i32, ActorScope), String> {
                let p = provider()?;
                let vt = reflect::vt().ok_or("native reflection unavailable")?;
                if !self.native_guard_ok(vt) {
                    return Err("native actor scope world guard unavailable".into());
                }
                let world = object(vt, arg_int(L, 1).ok_or("native actor scope world")?)?;
                let actor = object(vt, arg_int(L, 2).ok_or("native actor scope actor")?)?;
                let mut context = GuardContext::new(self, vt, world, None)?;
                let guard = context.ffi();
                if !context.valid() {
                    return Err("native actor scope world changed".into());
                }
                let mut out = ActorScope::default();
                let status = (p.actor_scope)(world, actor, &guard, &mut out);
                Ok((status, out))
            })();
            match result {
                Ok((status, out)) => {
                    lua_createtable(L, 0, 6);
                    let t = lua_gettop(L);
                    set_bool(L, t, "ok", status == 1 && out.qualified == 1);
                    set_bool(L, t, "qualified", out.qualified == 1);
                    set_int(L, t, "weak", out.weak as i64);
                    set_int(L, t, "address", out.address as i64);
                    push_lifecycle(L, &out.state);
                    rawset_str(L, t, "state");
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
                let kind = arg_int(L, 3).ok_or("native retirement kind")?;
                let original = arg_int(L, 4).ok_or("native retirement original scope weak")? as u64;
                let actor = Object {
                    weak: original,
                    address: arg_int(L, 2).ok_or("native retirement actor")? as u64,
                };
                // Resolve the original scope handle first. Never ask a reused
                // raw address for a new weak generation before destruction.
                let pointer = reflect::get(vt, original);
                if original == 0
                    || actor.address == 0
                    || pointer.is_null()
                    || pointer as u64 != actor.address
                {
                    return Err("native retirement scoped actor generation changed".into());
                }
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
mod source_roster_native_tests {
    use super::*;
    #[repr(C)]
    #[derive(Default)]
    struct Fake {
        flags: u32,
        pad: u32,
        name: u64,
        class: u64,
        outer: u64,
        world: u64,
        controller: u64,
        context: u64,
        players: RosterArray,
        instance: u64,
        player: u64,
        pawn: u64,
        team: i32,
        action: u32,
        callback: u64,
        calls: u32,
    }
    impl Default for RosterArray {
        fn default() -> Self {
            Self {
                data: 0,
                count: 0,
                capacity: 0,
            }
        }
    }
    unsafe extern "C" fn name(p: *const c_void) -> *const u64 {
        unsafe { &(*p.cast::<Fake>()).name }
    }
    unsafe extern "C" fn flags(p: *const c_void) -> *const u32 {
        unsafe { &(*p.cast::<Fake>()).flags }
    }
    unsafe extern "C" fn outer(p: *const c_void) -> *const *const c_void {
        unsafe { (&(*p.cast::<Fake>()).outer as *const u64).cast() }
    }
    unsafe extern "C" fn world(p: *const c_void) -> *mut c_void {
        unsafe {
            let f = &mut *p.cast_mut().cast::<Fake>();
            f.calls += 1;
            if f.callback != 0 {
                let earlier = &mut *(f.callback as *mut Fake);
                match f.action {
                    1 => earlier.team += 1,
                    2 => earlier.flags |= 0x40000000,
                    3 => earlier.name += 1,
                    4 => earlier.class = 0,
                    5 => earlier.outer = f.world,
                    6 => earlier.controller = 0,
                    _ => {}
                }
            }
            f.world as *mut c_void
        }
    }
    unsafe extern "C" fn resolve(w: u64) -> *mut c_void {
        w as *mut c_void
    }
    unsafe extern "C" fn weak(p: *mut c_void) -> u64 {
        p as u64
    }
    unsafe extern "C" fn class(p: *mut c_void) -> *mut c_void {
        unsafe { (*p.cast::<Fake>()).class as *mut c_void }
    }
    unsafe extern "C" fn fname(_: *const u16, _: i32) -> u64 {
        0
    }
    unsafe extern "C" fn find(_: *const u16) -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn isa(_: *mut c_void, _: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn props(
        _: *mut c_void,
        _: *mut reflect::HsmpProp,
        _: i32,
        _: *mut i32,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn prop(_: *mut c_void, _: *const u16, _: *mut reflect::HsmpProp) -> i32 {
        0
    }
    unsafe extern "C" fn call(_: *mut c_void, _: *mut c_void, _: *mut c_void) {}
    fn vt() -> HsmpReflect {
        HsmpReflect {
            abi: 1,
            _r: 0,
            fname,
            find,
            is_a: isa,
            class_of: class,
            props,
            obj_prop: prop,
            call,
            weak,
            resolve,
        }
    }
    fn exports() -> RosterExports {
        RosterExports {
            name,
            flags,
            outer,
            world,
        }
    }
    fn obj(f: &Fake) -> Object {
        let address = f as *const Fake as u64;
        Object {
            weak: address,
            address,
        }
    }
    fn field(offset: usize) -> reflect::HsmpProp {
        reflect::HsmpProp {
            offset: offset as i32,
            size: 8,
            ..Default::default()
        }
    }
    fn id(f: &Fake, c: &Fake) -> RosterIdentity {
        RosterIdentity {
            object: obj(f),
            class: obj(c),
            name: f.name,
            class_name: c.name,
        }
    }
    fn path(i: RosterIdentity) -> RosterPath {
        RosterPath(vec![i])
    }
    fn binding(w: &Fake, p: &Fake, c: &Fake, cls: &Fake) -> RosterBinding {
        let world = id(w, cls);
        let pawn = id(p, cls);
        let controller = id(c, cls);
        RosterBinding {
            entity: w::Entity {
                reference: w::EntityRef {
                    epoch: 1,
                    id: 1,
                    incarnation: 1,
                },
                slot: 0,
                kind: w::AI,
                controller: 255,
                owner_peer: 0,
                team: None,
            },
            world,
            pawn,
            controller,
            pawn_controller: field(std::mem::offset_of!(Fake, controller)),
            controller_pawn: field(std::mem::offset_of!(Fake, pawn)),
            team: field(std::mem::offset_of!(Fake, team)),
            paths: [path(world), path(pawn), path(controller)],
            indexed: None,
        }
    }
    #[test]
    fn source_roster_final_census_rejects_later_getter_mutation_of_earlier_original() {
        for action in 1..=6 {
            let mut cls = Box::new(Fake {
                name: 99,
                ..Default::default()
            });
            let cp = &mut *cls as *mut Fake as u64;
            let w = Box::new(Fake {
                name: 1,
                class: cp,
                ..Default::default()
            });
            let wp = obj(&w).address;
            let mut a = Box::new(Fake {
                name: 2,
                class: cp,
                world: wp,
                team: -7,
                ..Default::default()
            });
            let ac = Box::new(Fake {
                name: 3,
                class: cp,
                world: wp,
                pawn: obj(&a).address,
                ..Default::default()
            });
            a.controller = obj(&ac).address;
            let mut b = Box::new(Fake {
                name: 4,
                class: cp,
                world: wp,
                team: 12,
                ..Default::default()
            });
            let bc = Box::new(Fake {
                name: 5,
                class: cp,
                world: wp,
                pawn: obj(&b).address,
                action,
                callback: obj(&a).address,
                ..Default::default()
            });
            b.controller = obj(&bc).address;
            let earlier = binding(&w, &a, &ac, &cls);
            let later = binding(&w, &b, &bc, &cls);
            let vt = vt();
            let x = exports();
            let first = unsafe { earlier.pure_team(&vt, x).unwrap() };
            let second = unsafe { later.qualify(&vt, x, || Ok(())).unwrap() };
            assert!(
                check_roster_teams(&[first, second], |i| unsafe {
                    if i == 0 {
                        earlier.pure_team(&vt, x)
                    } else {
                        later.pure_team(&vt, x)
                    }
                })
                .is_err(),
                "mutation {action}"
            );
            // The old PC and its possession link remain allocated throughout.
            assert_eq!(ac.pawn, obj(&a).address);
        }
    }
    #[test]
    fn source_roster_native_admission_failure_stops_before_the_next_getter() {
        let cls = Box::new(Fake {
            name: 99,
            ..Default::default()
        });
        let cp = obj(&cls).address;
        let w = Box::new(Fake {
            name: 1,
            class: cp,
            ..Default::default()
        });
        let wp = obj(&w).address;
        let mut p = Box::new(Fake {
            name: 2,
            class: cp,
            world: wp,
            ..Default::default()
        });
        let c = Box::new(Fake {
            name: 3,
            class: cp,
            world: wp,
            pawn: obj(&p).address,
            ..Default::default()
        });
        p.controller = obj(&c).address;
        let row = binding(&w, &p, &c, &cls);
        let mut admissions = 0;
        assert!(unsafe {
            row.qualify(&vt(), exports(), || {
                admissions += 1;
                if admissions == 2 {
                    Err("original world changed".into())
                } else {
                    Ok(())
                }
            })
        }
        .is_err());
        assert_eq!(p.calls, 1);
        assert_eq!(c.calls, 0);
    }
    #[test]
    fn source_roster_pure_slot_witness_rejects_mapping_and_world_context_changes() {
        let cls = Box::new(Fake {
            name: 99,
            ..Default::default()
        });
        let cp = obj(&cls).address;
        let mut w = Box::new(Fake {
            name: 1,
            class: cp,
            ..Default::default()
        });
        let wp = obj(&w).address;
        let mut gi = Box::new(Fake {
            name: 2,
            class: cp,
            ..Default::default()
        });
        w.instance = obj(&gi).address;
        let mut context = Box::new([0u64; 89]);
        context[0x2c0 / 8] = wp;
        gi.context = context.as_ptr() as u64;
        let mut pc = Box::new(Fake {
            name: 3,
            class: cp,
            ..Default::default()
        });
        let original_pawn = Box::new(Fake {
            name: 6,
            class: cp,
            world: wp,
            controller: obj(&pc).address,
            ..Default::default()
        });
        pc.pawn = obj(&original_pawn).address;
        let mut player = Box::new(Fake {
            name: 4,
            class: cp,
            controller: obj(&pc).address,
            ..Default::default()
        });
        pc.player = obj(&player).address;
        let mut slots = Box::new([obj(&player).address]);
        gi.players = RosterArray {
            data: slots.as_ptr() as u64,
            count: 1,
            capacity: 1,
        };
        let wi = id(&w, &cls);
        let instance = id(&gi, &cls);
        let ci = id(&pc, &cls);
        let pi = id(&player, &cls);
        let witness = RosterSlots {
            instance,
            instance_path: path(instance),
            world_instance: field(std::mem::offset_of!(Fake, instance)),
            players: field(std::mem::offset_of!(Fake, players)),
            header: gi.players,
            slots: vec![(
                pi,
                path(pi),
                field(std::mem::offset_of!(Fake, controller)),
                ci,
            )],
            index: 0,
            controller_player: field(std::mem::offset_of!(Fake, player)),
            world_context: gi.context,
        };
        let vt = vt();
        let x = exports();
        unsafe {
            witness.verify(&vt, x, wi, ci).unwrap();
        }
        for mutation in 0..5 {
            match mutation {
                0 => slots[0] = 0,
                1 => player.controller = 0,
                2 => context[0x2c0 / 8] = 0,
                3 => gi.context = 0,
                _ => gi.players.count = 0,
            }
            assert!(unsafe { witness.verify(&vt, x, wi, ci) }.is_err());
            slots[0] = pi.object.address;
            player.controller = ci.object.address;
            context[0x2c0 / 8] = wp;
            gi.context = witness.world_context;
            gi.players = witness.header;
        }
        // This models the later actor's callback replacing the first index
        // while the original PC and Pawn are still live and unchanged.
        let mut later_pawn = Box::new(Fake {
            name: 7,
            class: cp,
            world: wp,
            ..Default::default()
        });
        let later_controller = Box::new(Fake {
            name: 8,
            class: cp,
            world: wp,
            pawn: obj(&later_pawn).address,
            callback: pi.object.address,
            action: 6,
            ..Default::default()
        });
        later_pawn.controller = obj(&later_controller).address;
        let later = binding(&w, &later_pawn, &later_controller, &cls);
        unsafe {
            later.qualify(&vt, x, || Ok(())).unwrap();
        }
        assert!(unsafe { witness.verify(&vt, x, wi, ci) }.is_err());
        assert_eq!(pc.name, 3);
        assert_eq!(pc.pawn, obj(&original_pawn).address);
        assert_eq!(original_pawn.controller, ci.object.address);
    }
    #[test]
    fn source_roster_acknowledged_team_never_defaults_unknown_to_zero() {
        assert!(require_acknowledged_team(None, 0, 0).is_err());
        assert!(require_acknowledged_team(Some(12), 12, 2).is_err());
        require_acknowledged_team(Some(-7), -7, -7).unwrap();
        require_acknowledged_team(Some(0), 0, 0).unwrap();
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
        let mut recipe = serde_json::from_slice::<d::SourceRecipe>(include_bytes!(
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
        assert_eq!(std::mem::size_of::<ActorScope>(), 304);
        assert_eq!(std::mem::size_of::<Component>(), 272);
        assert_eq!(std::mem::size_of::<Frame>(), 176);
        assert_eq!(std::mem::size_of::<SplineProfile>(), 20);
        assert_eq!(std::mem::size_of::<VertexStateProof>(), 24);
        assert_eq!(std::mem::size_of::<SpringArmFrame>(), 56);
        assert_eq!(std::mem::size_of::<FinishTarget>(), 128);
        assert_eq!(std::mem::size_of::<Provider>(), 112);
        assert_eq!(std::mem::size_of::<SplineSettings>(), 72);
        assert_eq!(std::mem::size_of::<SplineVectorPoint>(), 80);
        assert_eq!(std::mem::size_of::<SplineQuatPoint>(), 104);
        assert_eq!(std::mem::size_of::<SplineFloatPoint>(), 20);
        assert_eq!(std::mem::size_of::<SplineFrame>(), 184);
    }
    #[test]
    fn native_camera_and_spring_arm_keep_exact_classes_and_owned_socket_output() {
        let recipe = serde_json::from_slice::<d::SourceRecipe>(include_bytes!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
        ))
        .unwrap();
        let mut component = recipe.components[0].clone();
        component.kind = d::ComponentKind::Scene;
        component.component_class = "/Script/Engine.CameraComponent".into();
        component.scene = d::SceneEvidence::Camera;
        let mut arena = Arena::new();
        let camera = arena.component(&component);
        assert_eq!(camera.kind, 6);
        assert_eq!(camera.spring_arm_socket.len, 0);
        assert!(FrameStorage::source(&component).ffi().spring_arm.is_null());
        component.component_class = "/Script/Engine.SpringArmComponent".into();
        component.scene = d::SceneEvidence::SpringArm {
            draw_debug_lag_markers: false,
            socket_name: "Native actual socket".into(),
        };
        let arm = arena.component(&component);
        assert_eq!(arm.kind, 7);
        let class = unsafe { std::slice::from_raw_parts(arm.asset.data, arm.asset.len as usize) };
        assert_eq!(
            String::from_utf16(class).unwrap(),
            component.component_class
        );
        let socket = unsafe {
            std::slice::from_raw_parts(
                arm.spring_arm_socket.data,
                arm.spring_arm_socket.len as usize,
            )
        };
        assert_eq!(String::from_utf16(socket).unwrap(), "Native actual socket");
        let mut storage = FrameStorage::source(&component);
        let actual = SpringArmFrame {
            translation: [-0.0, 1.2345678901234567, -2.75],
            rotation: [2.0, 3.0, 4.0, 5.0],
        };
        **storage.spring_arm.as_mut().unwrap() = actual;
        let original_pointer = storage.ffi().spring_arm;
        let mut moved = Vec::with_capacity(1);
        moved.push(storage);
        moved.push(FrameStorage::source(&component));
        assert_eq!(moved[0].ffi().spring_arm, original_pointer);
        let observed = unsafe { &*original_pointer };
        assert_eq!(observed.translation[0].to_bits(), (-0.0f64).to_bits());
        assert_eq!(observed.translation, actual.translation);
        assert_eq!(observed.rotation, actual.rotation);
    }
    #[test]
    fn empty_static_binding_keeps_original_material_parent_and_class_evidence() {
        let recipe = serde_json::from_slice::<d::SourceRecipe>(include_bytes!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
        ))
        .unwrap();
        let mut component = recipe.components[0].clone();
        component.kind = d::ComponentKind::Static;
        component.component_class = "/Script/Engine.StaticMeshComponent".into();
        component.geometry = d::Geometry::NativeEmpty;
        component.asset.clear();
        component.skeleton.clear();
        component.bones.clear();
        component.morphs.clear();
        component.hidden_bones.clear();
        component.vertex_colors.clear();
        component.vertex_state = d::VertexState::NotApplicable;
        component.parent = 79;
        let mut arena = Arena::new();
        let bound = arena.component(&component);
        assert_eq!((bound.kind, bound.vertex_state, bound.parent), (8, 4, 79));
        assert_eq!(bound.material_count as usize, component.materials.len());
        assert_eq!(bound.relative.p, component.relative.translation);
        let class =
            unsafe { std::slice::from_raw_parts(bound.asset.data, bound.asset.len as usize) };
        assert_eq!(
            String::from_utf16(class).unwrap(),
            component.component_class
        );
        assert!(component.asset.is_empty());
        let mut storage = FrameStorage::source(&component);
        let frame = storage.ffi();
        assert!(frame.spring_arm.is_null() && frame.spline.is_null());
        assert_eq!(frame.scalar_count as usize, storage.scalars.len());
        let original = VertexTarget {
            owner: Object {
                weak: 1,
                address: 10,
            },
            component: Object {
                weak: 2,
                address: 20,
            },
            asset: Object::default(),
            scene_kind: 8,
        };
        assert_eq!(original.asset.address, 0);
        assert_eq!(std::mem::size_of::<VertexTarget>(), 56);
        assert_eq!(std::mem::size_of::<VertexStateProof>(), 24);
        fn requires_send<T: Send>() {}
        requires_send::<VertexTarget>();
    }
    #[test]
    fn empty_skeletal_binding_preserves_null_material_positions_and_kind() {
        let recipe = serde_json::from_slice::<d::SourceRecipe>(include_bytes!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
        ))
        .unwrap();
        let mut component = recipe.components[0].clone();
        component.kind = d::ComponentKind::Skeletal;
        component.component_class = "/Script/Engine.SkeletalMeshComponent".into();
        component.geometry = d::Geometry::NativeEmpty;
        component.asset.clear();
        component.skeleton.clear();
        component.physics_asset.clear();
        component.bones.clear();
        component.morphs.clear();
        component.hidden_bones.clear();
        component.vertex_colors.clear();
        component.vertex_state = d::VertexState::NotApplicable;
        component.scene = d::SceneEvidence::NotApplicable;
        component.spline_profile = None;
        component.collision = Some(d::Collision {
            enabled: 3,
            object_type: 1,
            profile: "Actual source profile".into(),
            responses: [0; 32],
            simulating: false,
        });
        component.materials = vec![
            d::Material {
                slot: 0,
                base: "/Game/Test/Actual.Material".into(),
                scalars: vec![],
                vectors: vec![],
                textures: vec![],
            },
            d::Material {
                slot: 1,
                base: String::new(),
                scalars: vec![],
                vectors: vec![],
                textures: vec![],
            },
            d::Material {
                slot: 2,
                base: String::new(),
                scalars: vec![],
                vectors: vec![],
                textures: vec![],
            },
        ];
        component.validate().unwrap();
        let mut arena = Arena::new();
        let bound = arena.component(&component);
        assert_eq!(
            (bound.kind, bound.vertex_state, bound.material_count),
            (9, 4, 3)
        );
        let class =
            unsafe { std::slice::from_raw_parts(bound.asset.data, bound.asset.len as usize) };
        assert_eq!(
            String::from_utf16(class).unwrap(),
            component.component_class
        );
        let slots = unsafe { std::slice::from_raw_parts(bound.materials, 3) };
        assert_eq!((slots[0].slot, slots[1].slot, slots[2].slot), (0, 1, 2));
        assert!(slots[0].base.len > 0);
        assert_eq!((slots[1].base.len, slots[2].base.len), (0, 0));
        assert_eq!(
            (
                slots[1].scalar_count,
                slots[1].vector_count,
                slots[1].texture_count
            ),
            (0, 0, 0)
        );
        let mut storage = FrameStorage::source(&component);
        let frame = storage.ffi();
        assert_eq!(
            (
                frame.bone_count,
                frame.morph_count,
                frame.scalar_count,
                frame.vector_count,
                frame.texture_count
            ),
            (0, 0, 0, 0, 0)
        );
        assert!(frame.spline.is_null() && frame.spring_arm.is_null());
        let proof = VertexStateProof {
            lod_info_count: 0,
            no_override: 1,
            asset_present: 0,
            material_count: 3,
            material_null_mask: 6,
            component_kind: 0,
        };
        assert_eq!(
            (
                proof.material_count,
                proof.material_null_mask,
                proof.component_kind
            ),
            (3, 6, 0)
        );
        assert_eq!(std::mem::size_of::<VertexStateProof>(), 24);
    }
    fn raw_spline_fixture() -> w::NativeSplineFrame {
        w::NativeSplineFrame {
            visible: true,
            hidden: false,
            owner_hidden: true,
            version: u32::MAX,
            settings: w::NativeSplineSettings {
                allow_spline_editing_per_instance: true,
                reparam_steps_per_segment: 7,
                duration: 1.25,
                stationary_endpoints: true,
                spline_has_been_edited: false,
                modified_by_construction_script: true,
                input_spline_points_to_construction_script: false,
                draw_debug: true,
                closed_loop: false,
                loop_position_override: true,
                loop_position: -0.0,
                default_up_vector: [1.2345678901234567, 0.0, -2.0],
            },
            position: w::NativeSplineCurve {
                looped: true,
                loop_key_offset: -2.5,
                points: vec![w::NativeSplineVectorPoint {
                    key: 0.5,
                    out: [1.2345678901234567, 2.0, 3.0],
                    arrive: [0.0; 3],
                    leave: [4.0, 5.0, 6.0],
                    interp: 5,
                }],
            },
            rotation: w::NativeSplineCurve {
                looped: false,
                loop_key_offset: 0.0,
                points: vec![w::NativeSplineQuatPoint {
                    key: 1.5,
                    out: [2.0, 3.0, 4.0, 5.0],
                    arrive: [0.0; 4],
                    leave: [7.0, 8.0, 9.0, 10.0],
                    interp: 3,
                }],
            },
            scale: w::NativeSplineCurve {
                looped: false,
                loop_key_offset: 0.0,
                points: vec![],
            },
            reparam: w::NativeSplineCurve {
                looped: false,
                loop_key_offset: 1.25,
                points: vec![w::NativeSplineFloatPoint {
                    key: 2.0,
                    out: 3.0,
                    arrive: 4.0,
                    leave: 5.0,
                    interp: 4,
                }],
            },
        }
    }
    #[test]
    fn raw_spline_marshaled_arrays_preserve_all_dynamic_fields_after_move() {
        let raw = raw_spline_fixture();
        let stored = SplineStorage::from_wire(&raw).unwrap();
        let mut moved = vec![stored];
        let ffi = moved[0].ffi();
        assert_eq!(ffi.position.count, 1);
        assert_eq!(ffi.scale.count, 0);
        assert!(!ffi.rotation.points.is_null());
        assert_eq!(moved[0].to_wire().unwrap(), raw);
        assert_eq!(
            moved[0].value.settings.loop_position.to_bits(),
            (-0.0f32).to_bits()
        );
        assert_eq!(moved[0].rotation[0].arrive, [0.0; 4]);
        assert_eq!(moved[0].rotation[0].out, [2.0, 3.0, 4.0, 5.0]);
        moved[0].value.position.count = 0;
        assert!(moved[0].to_wire().is_err());
    }
    #[test]
    fn raw_spline_marshaling_refuses_invalid_native_flags_modes_and_nonfinite() {
        let raw = raw_spline_fixture();
        let mut storage = SplineStorage::from_wire(&raw).unwrap();
        storage.value.visible = 2;
        assert!(storage.to_wire().is_err());
        storage.value.visible = 1;
        storage.rotation[0].interp = 6;
        assert!(storage.to_wire().is_err());
        storage.rotation[0].interp = 3;
        storage.reparam[0].arrive = f32::INFINITY;
        assert!(storage.to_wire().is_err());
        let mut raw = raw;
        raw.position.points[0].out[2] = f64::NAN;
        assert!(SplineStorage::from_wire(&raw).is_err());
    }
    #[test]
    fn native_spline_binding_uses_exact_class_profile_and_owned_caller_arrays() {
        let recipe = serde_json::from_slice::<d::SourceRecipe>(include_bytes!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
        ))
        .unwrap();
        let mut component = recipe.components[0].clone();
        component.kind = d::ComponentKind::Spline;
        component.component_class = "/Script/Engine.SplineComponent".into();
        component.asset.clear();
        component.spline_profile = Some(d::SplineProfile {
            position_count: 2,
            rotation_count: 1,
            scale_count: 0,
            reparam_count: 11,
            metadata_null: true,
        });
        let mut arena = Arena::new();
        let bound = arena.component(&component);
        assert_eq!(bound.kind, 5);
        assert_eq!(
            (
                bound.spline.position_count,
                bound.spline.rotation_count,
                bound.spline.scale_count,
                bound.spline.reparam_count,
                bound.spline.metadata_null
            ),
            (2, 1, 0, 11, 1)
        );
        let class =
            unsafe { std::slice::from_raw_parts(bound.asset.data, bound.asset.len as usize) };
        assert_eq!(
            String::from_utf16(class).unwrap(),
            component.component_class
        );
        let mut storage = FrameStorage::source(&component);
        let frame = storage.ffi();
        assert!(!frame.spline.is_null());
        let spline = unsafe { &*frame.spline };
        assert_eq!(
            (
                spline.position.count,
                spline.rotation.count,
                spline.scale.count,
                spline.reparam.count
            ),
            (2, 1, 0, 11)
        );
        component.spline_profile = None;
        component.kind = d::ComponentKind::Scene;
        assert!(FrameStorage::source(&component).ffi().spline.is_null());
    }
}

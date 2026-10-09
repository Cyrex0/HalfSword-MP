//! Exact source recipes. Validation establishes a bounded, complete DTO, not
//! native construction or successful client rendering. Empty nullable asset
//! strings mean an observed native null; missing fields never acquire defaults.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const SCHEMA: u16 = 3;
pub const MAX_RECIPE_BYTES: usize = 60 * 1024;
pub const MAX_COMPONENTS: usize = 64;
pub const MAX_BONES: usize = 512;
pub const MAX_MATERIALS: usize = 32;
pub const MAX_PARAMETERS: usize = 128;
pub const MAX_WEAPONS: usize = 32;
/// Resource caps, not inferred native spline point counts or settings.
pub const MAX_SPLINE_POINTS: usize = 64;
pub const MAX_SPLINE_REPARAM_POINTS: usize = 1024;
/// Caps RLE expansion before native buffer allocation, independent of JSON size.
pub const MAX_VERTEX_COLORS: usize = 1_000_000;

macro_rules! dto {
    ($($name:ident { $($field:ident : $ty:ty),* $(,)? })*) => {$ (
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    )*};
}
dto! {
    SlotFlag { slot: u8, value: bool }
    ArmorPassport {
        class: String, id: i32, core_removed: bool, module1: i32, module2: i32, module3: i32,
        bg_color: [f32; 4], leather_color: [f32; 4], fabric1: [f32; 4], fabric2: [f32; 4], fabric3: [f32; 4],
        steel: u8, metal: u8, rust: bool, dirt: bool, price: f64, pslot: u8,
        up_ap: bool, low_ap: bool, req_up_ap: bool, req_low_ap: bool,
        slots_blocked: Vec<SlotFlag>, req_hier: bool, tier: u8
    }
    WeaponPassport {
        class: String, id: i32, name: String, head_sub1: String, head_sub2: String, head: String,
        guard: String, pommel: String, grip: String,
        head_size: [f64; 3], guard_size: [f64; 3], grip_size: [f64; 3], pommel_size: [f64; 3],
        mass_head: f64, mass_guard: f64, mass_grip: f64, mass_pommel: f64,
        mat_steel: u8, mat_colored: u8, mat_wood: u8, mat_leather: u8,
        color_wood: [f32; 4], color_leather: [f32; 4], price: f64, tier: u8
    }
    ArmorInSlot { slot: u8, passport: ArmorPassport }
    WeaponInSlot { slot: u8, passport: WeaponPassport }
    EquipmentPassport { armor: Vec<ArmorInSlot>, sheaths: Vec<WeaponInSlot>, hands: Vec<WeaponInSlot> }
    CharacterPassport {
        actor_class: String, id: i32, name: String, height: f64, weight: f64, skill: f64,
        equipment: EquipmentPassport, face_type: i32, eye_color: i32, hair_color: [f32; 4], hair_length: f64
    }
    Construction {
        start_body_condition: [f64; 8], is_zombie: bool, scale_mutation_inhibitor: f64,
        spawn_in_pants: bool, bolts_in_quiver: i32, blossfechten_gear: bool,
        actor_scale: [f64; 3], character_scale: [f64; 3], height_rate: f64, muscle_rate: f64, mass_scale: f64
    }
    ItemSlot { slot: u8, item: u32 }
    WeaponBinding { field: String, item: u32 }
    WeaponInstance { id: u32, actor_class: String, passport: WeaponPassport, components: Vec<u32> }
    LiveEquipment { armor: Vec<ArmorInSlot>, weapons: Vec<WeaponInstance>, hands: Vec<ItemSlot>, sheaths: Vec<WeaponBinding> }
    Transform { translation: [f64; 3], rotation: [f64; 4], scale: [f64; 3] }
    Bone { name: String, parent: i16 }
    ParameterInfo { name: String, association: u8, index: i32 }
    ScalarParameter { info: ParameterInfo, value: f32 }
    VectorParameter { info: ParameterInfo, value: [f32; 4] }
    TextureParameter { info: ParameterInfo, value: String }
    Material {
        slot: u16, base: String, scalars: Vec<ScalarParameter>, vectors: Vec<VectorParameter>, textures: Vec<TextureParameter>
    }
    Morph { name: String, value: f32 }
    ColorRun { count: u32, color: [u8; 4] }
    VertexLod { lod: u16, vertex_count: u32, runs: Vec<ColorRun> }
    Collision { enabled: u8, object_type: u8, profile: String, responses: [u8; 32], simulating: bool }
    SplineProfile {
        position_count: u16, rotation_count: u16, scale_count: u16,
        reparam_count: u16, metadata_null: bool
    }
    GroomGroup {
        hair_length: f32, hair_width: f32, hair_width_override: bool,
        root_scale: f32, root_scale_override: bool, tip_scale: f32, tip_scale_override: bool,
        shadow_density: f32, shadow_density_override: bool, raytracing_radius_scale: f32,
        raytracing_radius_scale_override: bool, use_raytracing_geometry: bool, use_raytracing_geometry_override: bool,
        lod_bias: f32, stable_rasterization: bool, stable_rasterization_override: bool,
        scatter_scene_lighting: bool, scatter_scene_lighting_override: bool,
        support_voxelization: bool, support_voxelization_override: bool,
        length_scale: f32, length_scale_override: bool
    }
    GroomRecipe {
        binding_asset: String, source_mesh: String, cache: String, physics_asset: String,
        attachment_name: String, use_cards: bool, simulation: bool, groups: Vec<GroomGroup>
    }
    Topology {
        in_process: bool, parts: Vec<SlotFlag>, dismembered_bones: Vec<String>,
        detached: Vec<u32>, gore: Vec<u32>, vertex_state: VertexState
    }
    SourceRecipe {
        schema: u16, actor_class: String, team: i32, passport: CharacterPassport, construction: Construction,
        equipment: LiveEquipment, components: Vec<RenderComponent>, topology: Topology
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SceneEvidence {
    NotApplicable,
    Scene,
    Spline { draw_debug: bool },
    HiddenCapsule,
}

fn required_collision<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Collision>, D::Error> {
    Option::<Collision>::deserialize(deserializer)
}
fn required_spline_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SplineProfile>, D::Error> {
    Option::<SplineProfile>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderComponent {
    pub id: u32,
    pub owner: u32,
    pub name: String,
    pub role: String,
    pub component_class: String,
    pub kind: ComponentKind,
    pub geometry: Geometry,
    pub scene: SceneEvidence,
    pub asset: String,
    pub skeleton: String,
    pub physics_asset: String,
    pub parent: u32,
    pub socket: String,
    pub relative: Transform,
    pub visible: bool,
    pub hidden: bool,
    // This field must be present: null means proven nonprimitive SceneComponent.
    #[serde(deserialize_with = "required_collision")]
    pub collision: Option<Collision>,
    // Presence is mandatory; null means spline replay is not applicable.
    #[serde(deserialize_with = "required_spline_profile")]
    pub spline_profile: Option<SplineProfile>,
    pub bones: Vec<Bone>,
    pub materials: Vec<Material>,
    pub morphs: Vec<Morph>,
    pub hidden_bones: Vec<String>,
    pub groom: Vec<GroomRecipe>,
    pub vertex_state: VertexState,
    pub vertex_colors: Vec<VertexLod>,
    pub deformer: String,
    pub cloth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    Skeletal,
    Static,
    Groom,
    Procedural,
    Scene,
    Spline,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Geometry {
    Cooked,
    RuntimeTransient,
    RuntimeMerged,
    Procedural,
    NotApplicable,
    NativeSpline,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VertexState {
    NativeAsset,
    Captured,
    RuntimeOverride,
    Unavailable,
    NotApplicable,
}

fn text(s: &str, max: usize, empty: bool) -> bool {
    (empty || !s.is_empty()) && s.len() <= max && !s.contains('\0')
}
fn asset(s: &str, nullable: bool) -> bool {
    (nullable && s.is_empty())
        || (text(s, 512, false)
            && (s.starts_with("/Game/") || s.starts_with("/Engine/") || s.starts_with("/Script/"))
            && !s.contains("Transient")
            && !s.contains(' ')
            && s.contains('.')
            && !s.contains(':'))
}
fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}
fn finite32(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}
fn sorted_slots<T>(v: &[T], max: u8, slot: impl Fn(&T) -> u8) -> bool {
    v.len() <= max as usize
        && v.iter().all(|x| slot(x) < max)
        && v.windows(2).all(|p| slot(&p[0]) < slot(&p[1]))
}
fn flags(v: &[SlotFlag], max: u8) -> bool {
    sorted_slots(v, max, |x| x.slot)
}
impl ArmorPassport {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !asset(&self.class, true)
            || self.pslot >= 17
            || !flags(&self.slots_blocked, 17)
            || !self.price.is_finite()
            || !finite32(&self.bg_color)
            || !finite32(&self.leather_color)
            || !finite32(&self.fabric1)
            || !finite32(&self.fabric2)
            || !finite32(&self.fabric3)
        {
            return Err("armor passport");
        }
        Ok(())
    }
}
impl WeaponPassport {
    pub fn validate(&self) -> Result<(), &'static str> {
        for s in [
            &self.class,
            &self.head_sub1,
            &self.head_sub2,
            &self.head,
            &self.guard,
            &self.pommel,
            &self.grip,
        ] {
            if !asset(s, true) {
                return Err("weapon class");
            }
        }
        if !text(&self.name, 128, true)
            || !finite(&self.head_size)
            || !finite(&self.guard_size)
            || !finite(&self.grip_size)
            || !finite(&self.pommel_size)
            || !finite(&[
                self.mass_head,
                self.mass_guard,
                self.mass_grip,
                self.mass_pommel,
                self.price,
            ])
            || !finite32(&self.color_wood)
            || !finite32(&self.color_leather)
        {
            return Err("weapon passport");
        }
        Ok(())
    }
}
fn armor_map(v: &[ArmorInSlot], live: bool) -> Result<(), &'static str> {
    if !sorted_slots(v, 17, |x| x.slot) {
        return Err("armor slots");
    }
    for x in v {
        x.passport.validate()?;
        if x.slot != x.passport.pslot || (live && x.passport.class.is_empty()) {
            return Err("armor slot binding");
        }
    }
    Ok(())
}
fn weapon_map(v: &[WeaponInSlot], max: u8) -> Result<(), &'static str> {
    if !sorted_slots(v, max, |x| x.slot) {
        return Err("weapon slots");
    }
    for x in v {
        x.passport.validate()?;
    }
    Ok(())
}
impl CharacterPassport {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !asset(&self.actor_class, true)
            || !text(&self.name, 128, true)
            || !finite(&[self.height, self.weight, self.skill, self.hair_length])
            || !finite32(&self.hair_color)
        {
            return Err("character passport");
        }
        armor_map(&self.equipment.armor, false)?;
        weapon_map(&self.equipment.sheaths, 6)?;
        weapon_map(&self.equipment.hands, 2)
    }
}
impl Transform {
    pub fn validate(&self) -> Result<(), &'static str> {
        let q = self.rotation.iter().map(|x| x * x).sum::<f64>();
        if !finite(&self.translation)
            || !finite(&self.rotation)
            || !finite(&self.scale)
            || !(0.999..=1.001).contains(&q)
        {
            return Err("render transform");
        }
        Ok(())
    }
}
impl ParameterInfo {
    fn valid(&self) -> bool {
        text(&self.name, 128, false) && self.association <= 2 && self.index >= -1
    }
    fn key(&self) -> (&str, u8, i32) {
        (&self.name, self.association, self.index)
    }
}
fn unique_parameters<'a>(v: impl Iterator<Item = &'a ParameterInfo>) -> bool {
    let mut set = HashSet::new();
    v.into_iter().all(|x| x.valid() && set.insert(x.key()))
}
impl Material {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.slot as usize >= MAX_MATERIALS
            || !asset(&self.base, false)
            || self.scalars.len() + self.vectors.len() + self.textures.len() > MAX_PARAMETERS
            || !unique_parameters(self.scalars.iter().map(|x| &x.info))
            || !unique_parameters(self.vectors.iter().map(|x| &x.info))
            || !unique_parameters(self.textures.iter().map(|x| &x.info))
            || self.scalars.iter().any(|x| !x.value.is_finite())
            || self.vectors.iter().any(|x| !finite32(&x.value))
            || self.textures.iter().any(|x| !asset(&x.value, true))
        {
            return Err("render material");
        }
        Ok(())
    }
}
impl RenderComponent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id == 0
            || !text(&self.name, 128, false)
            || !text(&self.role, 32, false)
            || !text(&self.socket, 128, true)
            || !asset(&self.component_class, false)
            || !asset(&self.physics_asset, true)
            || !asset(&self.deformer, true)
            || self.parent == self.id
        {
            return Err("render component identity");
        }
        if self.geometry == Geometry::Cooked && !asset(&self.asset, false) {
            return Err("render asset");
        }
        if self.geometry != Geometry::Cooked && !text(&self.asset, 512, true) {
            return Err("runtime geometry identity");
        }
        self.relative.validate()?;
        if let Some(collision) = &self.collision {
            if collision.enabled > 5
                || collision.object_type > 31
                || !text(&collision.profile, 128, true)
                || collision.responses.iter().any(|x| *x > 2)
            {
                return Err("source collision");
            }
        }
        if !matches!(self.kind, ComponentKind::Scene | ComponentKind::Spline)
            && (self.collision.is_none()
                || self.scene != SceneEvidence::NotApplicable
                || self.geometry == Geometry::NotApplicable
                || self.vertex_state == VertexState::NotApplicable)
        {
            return Err("render applicability");
        }
        if (self.kind == ComponentKind::Spline) != self.spline_profile.is_some()
            || (self.kind != ComponentKind::Spline && self.geometry == Geometry::NativeSpline)
        {
            return Err("spline applicability");
        }
        if self.bones.len() > MAX_BONES
            || self.materials.len() > MAX_MATERIALS
            || self.morphs.len() > MAX_PARAMETERS
            || self.hidden_bones.len() > MAX_BONES
            || self.groom.len() > 1
            || self.vertex_colors.len() > 16
        {
            return Err("render bound");
        }
        if (self.vertex_state == VertexState::Captured) != !self.vertex_colors.is_empty() {
            return Err("source vertex capture state");
        }
        for (i, lod) in self.vertex_colors.iter().enumerate() {
            if lod.lod as usize != i
                || lod.vertex_count == 0
                || lod.vertex_count > 1_000_000
                || lod.runs.is_empty()
                || lod.runs.len() > 4096
                || lod.runs.iter().any(|r| r.count == 0)
                || lod.runs.iter().map(|r| r.count as u64).sum::<u64>() != lod.vertex_count as u64
                || lod.runs.windows(2).any(|r| r[0].color == r[1].color)
            {
                return Err("source vertex colors incomplete");
            }
        }
        match self.kind {
            ComponentKind::Skeletal => {
                if self.bones.is_empty() || !asset(&self.skeleton, false) || !self.groom.is_empty()
                {
                    return Err("skeletal recipe");
                }
            }
            ComponentKind::Static | ComponentKind::Procedural => {
                if !self.bones.is_empty() || !self.skeleton.is_empty() || !self.groom.is_empty() {
                    return Err("static recipe");
                }
            }
            ComponentKind::Groom => {
                if !self.bones.is_empty() || !self.skeleton.is_empty() || self.groom.len() != 1 {
                    return Err("groom recipe");
                }
            }
            ComponentKind::Scene => {
                if self.role != "anchor"
                    || self.geometry != Geometry::NotApplicable
                    || self.vertex_state != VertexState::NotApplicable
                    || !self.asset.is_empty()
                    || !self.skeleton.is_empty()
                    || !self.physics_asset.is_empty()
                    || !self.deformer.is_empty()
                    || self.cloth
                    || !self.bones.is_empty()
                    || !self.materials.is_empty()
                    || !self.morphs.is_empty()
                    || !self.hidden_bones.is_empty()
                    || !self.groom.is_empty()
                    || !self.vertex_colors.is_empty()
                {
                    return Err("scene anchor applicability");
                }
                match &self.scene {
                    SceneEvidence::Scene
                        if self.component_class == "/Script/Engine.SceneComponent"
                            && self.collision.is_none() => {}
                    SceneEvidence::Spline { draw_debug: false }
                        if self.component_class == "/Script/Engine.SplineComponent"
                            && self.collision.is_some() => {}
                    SceneEvidence::HiddenCapsule
                        if self.component_class == "/Script/Engine.CapsuleComponent"
                            && self.collision.is_some()
                            && (!self.visible || self.hidden) => {}
                    _ => return Err("native anchor rendering not proved absent"),
                }
            }
            ComponentKind::Spline => {
                if self.component_class != "/Script/Engine.SplineComponent"
                    || self.geometry != Geometry::NativeSpline
                    || self.scene != SceneEvidence::NotApplicable
                    || self.vertex_state != VertexState::NotApplicable
                    || self.collision.is_none()
                    || !self.asset.is_empty()
                    || !self.skeleton.is_empty()
                    || !self.physics_asset.is_empty()
                    || !self.deformer.is_empty()
                    || self.cloth
                    || !self.bones.is_empty()
                    || !self.materials.is_empty()
                    || !self.morphs.is_empty()
                    || !self.hidden_bones.is_empty()
                    || !self.groom.is_empty()
                    || !self.vertex_colors.is_empty()
                {
                    return Err("native spline applicability");
                }
                self.spline_profile
                    .as_ref()
                    .ok_or("native spline profile")?
                    .validate()?;
            }
        }
        let mut names = HashSet::new();
        for (i, b) in self.bones.iter().enumerate() {
            if !text(&b.name, 128, false)
                || !names.insert(&b.name)
                || b.parent < -1
                || b.parent as isize >= i as isize
            {
                return Err("render bone dictionary");
            }
        }
        let mut hidden = HashSet::new();
        if self
            .hidden_bones
            .iter()
            .any(|b| !names.contains(b) || !hidden.insert(b))
        {
            return Err("hidden bone dictionary");
        }
        let mut morphs = HashSet::new();
        if self
            .morphs
            .iter()
            .any(|m| !text(&m.name, 128, false) || !m.value.is_finite() || !morphs.insert(&m.name))
        {
            return Err("render morphs");
        }
        for (slot, m) in self.materials.iter().enumerate() {
            m.validate()?;
            if m.slot as usize != slot {
                return Err("material slots");
            }
        }
        for g in &self.groom {
            if !asset(&g.binding_asset, true)
                || !asset(&g.source_mesh, true)
                || !asset(&g.cache, true)
                || !asset(&g.physics_asset, true)
                || !text(&g.attachment_name, 128, true)
                || g.groups.len() > 32
            {
                return Err("groom asset recipe");
            }
            for x in &g.groups {
                if !finite32(&[
                    x.hair_length,
                    x.hair_width,
                    x.root_scale,
                    x.tip_scale,
                    x.shadow_density,
                    x.raytracing_radius_scale,
                    x.lod_bias,
                    x.length_scale,
                ]) {
                    return Err("groom group");
                }
            }
        }
        Ok(())
    }
}
impl SplineProfile {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.metadata_null {
            return Err("native spline metadata unsupported");
        }
        if self.position_count as usize > MAX_SPLINE_POINTS
            || self.rotation_count as usize > MAX_SPLINE_POINTS
            || self.scale_count as usize > MAX_SPLINE_POINTS
            || self.reparam_count as usize > MAX_SPLINE_REPARAM_POINTS
        {
            return Err("native spline point bound");
        }
        Ok(())
    }
}
impl SourceRecipe {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != SCHEMA
            || !asset(&self.actor_class, false)
            || self.components.is_empty()
            || self.components.len() > MAX_COMPONENTS
        {
            return Err("source recipe");
        }
        self.passport.validate()?;
        let c = &self.construction;
        if !finite(&c.start_body_condition)
            || !finite(&c.actor_scale)
            || !finite(&c.character_scale)
            || !finite(&[
                c.scale_mutation_inhibitor,
                c.height_rate,
                c.muscle_rate,
                c.mass_scale,
            ])
        {
            return Err("source construction");
        }
        armor_map(&self.equipment.armor, true)?;
        if self.equipment.weapons.len() > MAX_WEAPONS
            || !sorted_slots(&self.equipment.hands, 2, |x| x.slot)
            || self.equipment.sheaths.len() > 5
        {
            return Err("live equipment bound");
        }
        let mut items = HashSet::new();
        for w in &self.equipment.weapons {
            if w.id == 0
                || !items.insert(w.id)
                || !asset(&w.actor_class, false)
                || w.components.len() > MAX_COMPONENTS
            {
                return Err("weapon instance");
            }
            w.passport.validate()?;
        }
        if self
            .equipment
            .hands
            .iter()
            .any(|s| !items.contains(&s.item))
        {
            return Err("live weapon slot binding");
        }
        let mut sheaths = HashSet::new();
        for s in &self.equipment.sheaths {
            if ![
                "Weapon Slot R 1",
                "Weapon Slot R 2",
                "Weapon Slot Back",
                "Weapon Slot L 1",
                "Weapon Slot L 2",
            ]
            .contains(&s.field.as_str())
                || !sheaths.insert(&s.field)
                || !items.contains(&s.item)
            {
                return Err("live sheath binding");
            }
        }
        let mut components = HashSet::new();
        let mut vertex_colors = 0usize;
        for c in &self.components {
            c.validate()?;
            if !components.insert(c.id) || (c.owner != 0 && !items.contains(&c.owner)) {
                return Err("component owner");
            }
            for lod in &c.vertex_colors {
                vertex_colors = vertex_colors
                    .checked_add(lod.vertex_count as usize)
                    .ok_or("source vertex expansion bound")?;
                if vertex_colors > MAX_VERTEX_COLORS {
                    return Err("source vertex expansion bound");
                }
            }
        }
        for c in &self.components {
            if c.parent != 0 && !components.contains(&c.parent) {
                return Err("component attachment");
            }
            let mut seen = HashSet::new();
            let mut parent = c.parent;
            while parent != 0 {
                if !seen.insert(parent) {
                    return Err("attachment cycle");
                }
                parent = self
                    .components
                    .iter()
                    .find(|x| x.id == parent)
                    .ok_or("component parent")?
                    .parent;
            }
        }
        for w in &self.equipment.weapons {
            let mut used = HashSet::new();
            if w.components.is_empty()
                || w.components.iter().any(|id| {
                    !used.insert(id)
                        || !self
                            .components
                            .iter()
                            .any(|c| c.id == *id && c.owner == w.id)
                })
            {
                return Err("weapon component binding");
            }
            if self.components.iter().filter(|c| c.owner == w.id).count() != used.len() {
                return Err("weapon component binding incomplete");
            }
        }
        let t = &self.topology;
        if !flags(&t.parts, 15)
            || t.dismembered_bones.len() > MAX_BONES
            || t.detached.len() > MAX_COMPONENTS
            || t.gore.len() > MAX_COMPONENTS
            || t.dismembered_bones.iter().any(|s| !text(s, 128, false))
        {
            return Err("source topology");
        }
        for list in [&t.detached, &t.gore] {
            let mut seen = HashSet::new();
            if list
                .iter()
                .any(|id| !components.contains(id) || !seen.insert(id))
            {
                return Err("topology component binding");
            }
        }
        Ok(())
    }
    /// The display profile covers intact cooked meshes and exact native spline
    /// replay profiles. Native
    /// host readiness remains independent; a refused profile is not a healthy
    /// body, an empty census, or successful mirror readback.
    pub fn validate_mirror_profile(&self) -> Result<(), &'static str> {
        self.validate()?;
        let t = &self.topology;
        if t.in_process
            || t.parts.iter().any(|x| x.value)
            || !t.dismembered_bones.is_empty()
            || !t.detached.is_empty()
            || !t.gore.is_empty()
        {
            return Err("native cut geometry unsupported");
        }
        if !matches!(
            t.vertex_state,
            VertexState::NativeAsset | VertexState::Captured
        ) {
            return Err("native vertex state incomplete");
        }
        for c in &self.components {
            if matches!(c.kind, ComponentKind::Scene | ComponentKind::Spline) {
                // Native capture must keep verifying these live class/flags.
                continue;
            }
            if c.geometry != Geometry::Cooked || c.kind == ComponentKind::Procedural {
                return Err("native runtime geometry unsupported");
            }
            if !matches!(
                c.vertex_state,
                VertexState::NativeAsset | VertexState::Captured
            ) {
                return Err("native vertex state incomplete");
            }
            if !c.deformer.is_empty() || c.cloth {
                return Err("native deformation unsupported");
            }
            if c.kind == ComponentKind::Groom {
                return Err("native groom display unverified");
            }
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "source recipe serialization")?;
        if bytes.len() > MAX_RECIPE_BYTES {
            return Err("source recipe byte bound");
        }
        Ok(bytes)
    }
    pub fn decode_recipe(bytes: &[u8]) -> Result<Self, &'static str> {
        decode_recipe(bytes)
    }
}
pub fn decode_recipe(bytes: &[u8]) -> Result<SourceRecipe, &'static str> {
    if bytes.is_empty() || bytes.len() > MAX_RECIPE_BYTES {
        return Err("source recipe byte bound");
    }
    let recipe: SourceRecipe = serde_json::from_slice(bytes).map_err(|_| "source recipe schema")?;
    recipe.validate()?;
    Ok(recipe)
}

/// Synthetic offline data for codec/coherence tests; no native display proof.
#[cfg(test)]
pub(crate) fn fixture_recipe() -> SourceRecipe {
    serde_json::from_str(include_str!(
        "../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
    ))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> SourceRecipe {
        fixture_recipe()
    }
    #[test]
    fn exact_recipe_roundtrip_and_missing_fields_refuse() {
        let recipe = fixture();
        recipe.validate_mirror_profile().unwrap();
        let bytes = recipe.canonical_bytes().unwrap();
        assert_eq!(decode_recipe(&bytes).unwrap(), recipe);
        let mut value = serde_json::to_value(&recipe).unwrap();
        value["passport"]
            .as_object_mut()
            .unwrap()
            .remove("hair_length");
        assert!(decode_recipe(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = serde_json::to_value(&recipe).unwrap();
        value["invented_default"] = true.into();
        assert!(decode_recipe(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    fn anchor(id: u32, parent: u32, evidence: SceneEvidence) -> RenderComponent {
        let mut c = fixture().components.remove(0);
        c.id = id;
        c.parent = parent;
        c.name = format!("Offline anchor {id}");
        c.role = "anchor".into();
        c.kind = ComponentKind::Scene;
        c.geometry = Geometry::NotApplicable;
        c.vertex_state = VertexState::NotApplicable;
        c.component_class = match evidence {
            SceneEvidence::Scene => "/Script/Engine.SceneComponent",
            SceneEvidence::Spline { .. } => "/Script/Engine.SplineComponent",
            SceneEvidence::HiddenCapsule => "/Script/Engine.CapsuleComponent",
            SceneEvidence::NotApplicable => unreachable!(),
        }
        .into();
        if evidence == SceneEvidence::Scene {
            c.collision = None;
        }
        c.scene = evidence;
        c.asset.clear();
        c.skeleton.clear();
        c.physics_asset.clear();
        c.bones.clear();
        c.materials.clear();
        c.morphs.clear();
        c.hidden_bones.clear();
        c.groom.clear();
        c.vertex_colors.clear();
        c.hidden = true;
        c.relative.translation = [1.0000000000000002, -2.5, 3.0];
        c
    }
    #[test]
    fn exact_scene_spline_hidden_capsule_graph_roundtrips() {
        let mut recipe = fixture();
        recipe.components[0].parent = 2;
        recipe.components.extend([
            anchor(2, 3, SceneEvidence::Scene),
            anchor(3, 4, SceneEvidence::Spline { draw_debug: false }),
            anchor(4, 0, SceneEvidence::HiddenCapsule),
        ]);
        recipe.validate_mirror_profile().unwrap();
        let decoded = SourceRecipe::decode_recipe(&recipe.canonical_bytes().unwrap()).unwrap();
        assert_eq!(decoded, recipe);
        assert_eq!(decoded.components[1].collision, None);
        assert!(decoded.components[2].collision.is_some());
        assert_eq!(
            decoded.components[1].relative.translation[0].to_bits(),
            1.0000000000000002f64.to_bits()
        );
        recipe.components[2].parent = 2;
        assert_eq!(recipe.validate(), Err("attachment cycle"));
    }
    #[test]
    fn anchor_applicability_and_render_absence_never_default() {
        let mut scene = anchor(2, 0, SceneEvidence::Scene);
        scene.validate().unwrap();
        scene.component_class = "/Script/Engine.UnknownSceneSubclass".into();
        assert!(scene.validate().is_err());
        let mut scene = anchor(2, 0, SceneEvidence::Spline { draw_debug: true });
        assert!(scene.validate().is_err());
        scene.scene = SceneEvidence::Spline { draw_debug: false };
        scene.collision = None;
        assert!(scene.validate().is_err());
        let mut capsule = anchor(2, 0, SceneEvidence::HiddenCapsule);
        capsule.hidden = false;
        capsule.visible = true;
        assert!(capsule.validate().is_err());
        let mut recipe = fixture();
        recipe.components[0].collision = None;
        assert!(recipe.validate().is_err());
        let recipe = fixture();
        for missing in ["component_class", "scene", "collision", "spline_profile"] {
            let mut value = serde_json::to_value(&recipe).unwrap();
            value["components"][0]
                .as_object_mut()
                .unwrap()
                .remove(missing);
            assert!(
                SourceRecipe::decode_recipe(&serde_json::to_vec(&value).unwrap()).is_err(),
                "missing {missing}"
            );
        }
    }
    #[test]
    fn spline_profile_is_explicit_exact_and_bounded() {
        let mut c = anchor(2, 0, SceneEvidence::Spline { draw_debug: false });
        c.kind = ComponentKind::Spline;
        c.geometry = Geometry::NativeSpline;
        c.scene = SceneEvidence::NotApplicable;
        c.visible = true;
        c.hidden = false;
        c.spline_profile = Some(SplineProfile {
            position_count: 3,
            rotation_count: 2,
            scale_count: 1,
            reparam_count: 21,
            metadata_null: true,
        });
        let mut recipe = fixture();
        recipe.components[0].parent = c.id;
        recipe.components.push(c.clone());
        recipe.validate_mirror_profile().unwrap();
        assert_eq!(
            SourceRecipe::decode_recipe(&recipe.canonical_bytes().unwrap()).unwrap(),
            recipe
        );
        for missing in [
            "position_count",
            "rotation_count",
            "scale_count",
            "reparam_count",
            "metadata_null",
        ] {
            let mut json = serde_json::to_value(&recipe).unwrap();
            json["components"][1]["spline_profile"]
                .as_object_mut()
                .unwrap()
                .remove(missing);
            assert!(
                SourceRecipe::decode_recipe(&serde_json::to_vec(&json).unwrap()).is_err(),
                "missing {missing}"
            );
        }
        let mut absent = recipe.clone();
        absent.components[1].spline_profile = None;
        assert!(absent.validate().is_err());
        let mut attached = recipe.clone();
        attached.components[0].spline_profile = c.spline_profile.clone();
        assert!(attached.validate().is_err());
        let profile = c.spline_profile.as_mut().unwrap();
        profile.metadata_null = false;
        assert_eq!(
            profile.validate(),
            Err("native spline metadata unsupported")
        );
        profile.metadata_null = true;
        for (position, rotation, scale, reparam) in
            [(65, 0, 0, 0), (0, 65, 0, 0), (0, 0, 65, 0), (0, 0, 0, 1025)]
        {
            let invalid = SplineProfile {
                position_count: position,
                rotation_count: rotation,
                scale_count: scale,
                reparam_count: reparam,
                metadata_null: true,
            };
            assert_eq!(invalid.validate(), Err("native spline point bound"));
        }
        // Empty raw arrays are actual counts, not guessed missing data.
        SplineProfile {
            position_count: 0,
            rotation_count: 0,
            scale_count: 0,
            reparam_count: 0,
            metadata_null: true,
        }
        .validate()
        .unwrap();
        c.component_class = "/Script/Engine.CustomSplineSubclass".into();
        assert!(c.validate().is_err());
        c.component_class = "/Script/Engine.SplineComponent".into();
        c.bones.push(Bone {
            name: "invented".into(),
            parent: -1,
        });
        assert!(c.validate().is_err());
        let mut scene_recipe = fixture();
        scene_recipe
            .components
            .push(anchor(2, 0, SceneEvidence::Scene));
        let mut absent_collision = serde_json::to_value(scene_recipe).unwrap();
        absent_collision["components"][1]
            .as_object_mut()
            .unwrap()
            .remove("collision");
        assert!(
            SourceRecipe::decode_recipe(&serde_json::to_vec(&absent_collision).unwrap()).is_err()
        );
        let mut old = serde_json::to_value(recipe).unwrap();
        old["schema"] = 2.into();
        assert!(SourceRecipe::decode_recipe(&serde_json::to_vec(&old).unwrap()).is_err());
    }
    #[test]
    fn topology_components_and_complete_bones_never_default() {
        let mut recipe = fixture();
        recipe.components[0].bones[1].parent = 1;
        assert!(recipe.validate().is_err());
        let mut recipe = fixture();
        recipe.topology.parts.push(SlotFlag {
            slot: 3,
            value: true,
        });
        recipe.validate().unwrap();
        assert_eq!(
            recipe.validate_mirror_profile(),
            Err("native cut geometry unsupported")
        );
        let mut recipe = fixture();
        recipe.components[0].vertex_state = VertexState::Unavailable;
        assert_eq!(
            recipe.validate_mirror_profile(),
            Err("native vertex state incomplete")
        );
        let mut recipe = fixture();
        recipe.components[0].parent = 999;
        assert!(recipe.validate().is_err());
        let mut recipe = fixture();
        recipe.components[0].relative.scale[0] = f64::NAN;
        assert!(recipe.canonical_bytes().is_err());
    }
    #[test]
    fn armor_sparse_false_and_double_precision_survive() {
        let mut recipe = fixture();
        recipe.passport.height = 0.500000000000123;
        let mut armor: ArmorPassport = serde_json::from_str(include_str!(
            "../../tools/hsmp-tools/lua-tests/fixtures/native_armor_passport.json"
        ))
        .unwrap();
        armor.slots_blocked = vec![
            SlotFlag {
                slot: 2,
                value: false,
            },
            SlotFlag {
                slot: 5,
                value: true,
            },
        ];
        recipe.equipment.armor.push(ArmorInSlot {
            slot: armor.pslot,
            passport: armor,
        });
        let decoded = decode_recipe(&recipe.canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            decoded.passport.height.to_bits(),
            recipe.passport.height.to_bits()
        );
        assert_eq!(
            decoded.equipment.armor[0].passport.slots_blocked,
            recipe.equipment.armor[0].passport.slots_blocked
        );
        recipe.equipment.armor[0]
            .passport
            .slots_blocked
            .push(SlotFlag {
                slot: 5,
                value: false,
            });
        assert!(recipe.validate().is_err());
    }
    #[test]
    fn native_rgba_runs_are_complete_canonical_and_bounded() {
        let mut recipe = fixture();
        recipe.components[0].vertex_state = VertexState::Captured;
        recipe.topology.vertex_state = VertexState::Captured;
        recipe.components[0].vertex_colors = vec![VertexLod {
            lod: 0,
            vertex_count: 3,
            runs: vec![
                ColorRun {
                    count: 2,
                    color: [255, 0, 128, 0],
                },
                ColorRun {
                    count: 1,
                    color: [0, 255, 0, 255],
                },
            ],
        }];
        recipe.validate_mirror_profile().unwrap();
        assert_eq!(
            decode_recipe(&recipe.canonical_bytes().unwrap()).unwrap(),
            recipe
        );
        let mut invalid = recipe.clone();
        invalid.components[0].vertex_colors[0].vertex_count = 4;
        assert_eq!(invalid.validate(), Err("source vertex colors incomplete"));
        let mut invalid = recipe.clone();
        invalid.components[0].vertex_colors[0].runs[1].color = [255, 0, 128, 0];
        assert!(invalid.validate().is_err());
        let mut invalid = recipe.clone();
        invalid.components[0].vertex_colors[0].lod = 1;
        assert!(invalid.validate().is_err());
        recipe.components[0].vertex_colors[0].vertex_count = MAX_VERTEX_COLORS as u32;
        recipe.components[0].vertex_colors[0].runs = vec![ColorRun {
            count: MAX_VERTEX_COLORS as u32,
            color: [255, 255, 255, 255],
        }];
        let mut component = recipe.components[0].clone();
        component.id = 2;
        recipe.components.push(component);
        assert_eq!(recipe.validate(), Err("source vertex expansion bound"));
    }
    #[test]
    fn transient_flag_does_not_assert_an_observed_merge_recipe() {
        let mut recipe = fixture();
        recipe.components[0].geometry = Geometry::RuntimeTransient;
        recipe.components[0].asset = "/Engine/Transient.SkeletalMesh_1".into();
        recipe.validate().unwrap();
        assert_eq!(
            decode_recipe(&recipe.canonical_bytes().unwrap()).unwrap(),
            recipe
        );
        assert_eq!(
            recipe.validate_mirror_profile(),
            Err("native runtime geometry unsupported")
        );
    }
}

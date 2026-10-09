//! Additive native authority messages. These are network DTOs, never shared-memory or engine objects.
use hsmp_ipc::schema::{
    combat::Vitals,
    pose::{Root, POSE_FRAME_MAX},
};
use hsmp_net::net::wire::{Put, Reader};

pub const K_DIRECTORY: u16 = 0x0AC0;
pub const K_WORLD: u16 = 0x0A10;
pub const K_INPUT: u16 = 0x0A80;
pub const K_DESCRIPTOR: u16 = 0x0AC1;
pub const K_RENDER_WORLD: u16 = 0x0A11;
pub const K_RENDER_WORLD_V2: u16 = 0x0A12;
pub const K_MIRROR_READY: u16 = 0x0A81;
pub const MAX_ENTITIES: usize = 32;
pub const MAX_WORLD_BYTES: usize = 17
    + MAX_ENTITIES
        * (16 + std::mem::size_of::<Root>() + std::mem::size_of::<Vitals>() + 2 + POSE_FRAME_MAX);
const _: () = assert!(MAX_WORLD_BYTES + hsmp_ipc::wire::HDR <= hsmp_net::net::frag::MAX_MESSAGE);
pub const MAX_INPUTS: usize = 256;
pub const INPUT_TIMEOUT_MS: u64 = 250;
pub const BUTTONS_ALL: u32 = 0x7F;
pub const RELEASE_ALL: u32 = 1;
pub const HUMAN: u8 = 0;
pub const AI: u8 = 1;
pub const BOOTING: u8 = 0;
pub const READY: u8 = 1;
pub const LIVE: u8 = 2;
pub const VICTORY: u8 = 3;
pub const DEFEAT: u8 = 4;
pub const FAULT: u8 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EntityRef {
    pub epoch: u64,
    pub id: u32,
    pub incarnation: u32,
}
impl EntityRef {
    pub fn valid(self) -> bool {
        self.epoch != 0 && self.id != 0 && self.incarnation != 0
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub reference: EntityRef,
    pub owner_peer: u32,
    pub slot: u16,
    pub kind: u8,
    pub controller: u8,
    pub team: Option<i32>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Directory {
    pub epoch: u64,
    pub seq: u32,
    pub state: u8,
    pub arena: String,
    pub error: String,
    pub entities: Vec<Entity>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputFrame {
    pub reference: EntityRef,
    pub seq: u32,
    pub delivery_seq: u32,
    /// Capture time on the welcome's server timeline, filled by the embedded client.
    pub sample_ms: u64,
    pub buttons: u32,
    pub flags: u32,
    pub axes: [f32; 8],
}
impl InputFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.reference.valid() || self.seq == 0 {
            return Err("input identity");
        }
        if self.buttons & !BUTTONS_ALL != 0 || self.flags & !RELEASE_ALL != 0 {
            return Err("input flags");
        }
        for (i, x) in self.axes.iter().enumerate() {
            let (lo, hi) = match i {
                2 | 3 => (-100.0, 100.0),
                4 | 5 => (0.0, 1.0),
                _ => (-1.0, 1.0),
            };
            if !x.is_finite() || *x < lo || *x > hi {
                return Err("input axes");
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySnapshot {
    pub reference: EntityRef,
    pub root: Root,
    pub vitals: Vitals,
    pub pose: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub epoch: u64,
    pub directory_seq: u32,
    pub frame_seq: u32,
    pub entities: Vec<EntitySnapshot>,
}

/// The recipe is source-owned. No pointer or transient UObject name is a loadable recipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub reference: EntityRef,
    pub slot: u16,
    pub directory_seq: u32,
    pub revision: u32,
    pub source_frame_seq: u32,
    pub recipe: crate::native_descriptor::SourceRecipe,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RenderComponent {
    pub id: u32,
    /// Component-to-world p3/q4/scale3, native double precision.
    pub transform: [f64; 10],
    /// Complete source dictionary order, in component space.
    pub bones: Vec<[f64; 10]>,
    pub morphs: Vec<f32>,
    pub materials: Vec<RenderMaterial>,
    /// Present only for a descriptor-qualified native spline component.
    pub spline: Option<NativeSplineFrame>,
}

pub const MAX_SPLINE_POINTS: usize = 64;
pub const MAX_SPLINE_REPARAM_POINTS: usize = 1024;

/// Raw native interpolation data. Quaternion tangents may be zero or nonunit;
/// they are copied exactly, never normalized as actor transforms.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineVectorPoint {
    pub key: f32,
    pub out: [f64; 3],
    pub arrive: [f64; 3],
    pub leave: [f64; 3],
    pub interp: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineQuatPoint {
    pub key: f32,
    pub out: [f64; 4],
    pub arrive: [f64; 4],
    pub leave: [f64; 4],
    pub interp: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineFloatPoint {
    pub key: f32,
    pub out: f32,
    pub arrive: f32,
    pub leave: f32,
    pub interp: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineCurve<T> {
    pub looped: bool,
    pub loop_key_offset: f32,
    pub points: Vec<T>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineSettings {
    pub allow_spline_editing_per_instance: bool,
    pub reparam_steps_per_segment: i32,
    pub duration: f32,
    pub stationary_endpoints: bool,
    pub spline_has_been_edited: bool,
    pub modified_by_construction_script: bool,
    pub input_spline_points_to_construction_script: bool,
    pub draw_debug: bool,
    pub closed_loop: bool,
    pub loop_position_override: bool,
    pub loop_position: f32,
    pub default_up_vector: [f64; 3],
}
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSplineFrame {
    pub visible: bool,
    pub hidden: bool,
    pub owner_hidden: bool,
    pub version: u32,
    pub settings: NativeSplineSettings,
    pub position: NativeSplineCurve<NativeSplineVectorPoint>,
    pub rotation: NativeSplineCurve<NativeSplineQuatPoint>,
    pub scale: NativeSplineCurve<NativeSplineVectorPoint>,
    pub reparam: NativeSplineCurve<NativeSplineFloatPoint>,
}
impl NativeSplineFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        let vector_point = |p: &NativeSplineVectorPoint| {
            p.key.is_finite()
                && p.interp <= 5
                && p.out
                    .iter()
                    .chain(&p.arrive)
                    .chain(&p.leave)
                    .all(|v| v.is_finite())
        };
        let quat_point = |p: &NativeSplineQuatPoint| {
            p.key.is_finite()
                && p.interp <= 5
                && p.out
                    .iter()
                    .chain(&p.arrive)
                    .chain(&p.leave)
                    .all(|v| v.is_finite())
        };
        let float_point = |p: &NativeSplineFloatPoint| {
            p.interp <= 5
                && [p.key, p.out, p.arrive, p.leave]
                    .iter()
                    .all(|v| v.is_finite())
        };
        let settings = &self.settings;
        if self.position.points.len() > MAX_SPLINE_POINTS
            || self.rotation.points.len() > MAX_SPLINE_POINTS
            || self.scale.points.len() > MAX_SPLINE_POINTS
            || self.reparam.points.len() > MAX_SPLINE_REPARAM_POINTS
        {
            return Err("render spline point bound");
        }
        if !settings.duration.is_finite()
            || !settings.loop_position.is_finite()
            || !settings.default_up_vector.iter().all(|v| v.is_finite())
            || ![
                self.position.loop_key_offset,
                self.rotation.loop_key_offset,
                self.scale.loop_key_offset,
                self.reparam.loop_key_offset,
            ]
            .iter()
            .all(|v| v.is_finite())
            || !self
                .position
                .points
                .iter()
                .chain(&self.scale.points)
                .all(vector_point)
            || !self.rotation.points.iter().all(quat_point)
            || !self.reparam.points.iter().all(float_point)
        {
            return Err("render spline values");
        }
        Ok(())
    }
}
fn read_bool(r: &mut Reader<'_>) -> Result<bool, &'static str> {
    match r.u8().or_else(fail)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("render spline boolean"),
    }
}
fn read_f32(r: &mut Reader<'_>) -> Result<f32, &'static str> {
    Ok(f32::from_bits(r.u32().or_else(fail)?))
}
fn read_f64_array<const N: usize>(r: &mut Reader<'_>) -> Result<[f64; N], &'static str> {
    let mut values = [0.0; N];
    for value in &mut values {
        *value = f64::from_bits(r.u64().or_else(fail)?);
    }
    Ok(values)
}
fn put_f64_array<const N: usize>(b: &mut Vec<u8>, values: &[f64; N]) {
    for value in values {
        b.put_u64(value.to_bits());
    }
}
fn encode_spline_curve<T>(
    b: &mut Vec<u8>,
    curve: &NativeSplineCurve<T>,
    point: impl Fn(&mut Vec<u8>, &T),
) {
    b.put_u8(u8::from(curve.looped));
    b.put_u32(curve.loop_key_offset.to_bits());
    b.put_u16(curve.points.len() as u16);
    for value in &curve.points {
        point(b, value);
    }
}
fn decode_spline_curve<T>(
    r: &mut Reader<'_>,
    limit: usize,
    point_bytes: usize,
    point: impl Fn(&mut Reader<'_>) -> Result<T, &'static str>,
) -> Result<NativeSplineCurve<T>, &'static str> {
    let looped = read_bool(r)?;
    let loop_key_offset = read_f32(r)?;
    let count = r.u16().or_else(fail)? as usize;
    if count > limit {
        return Err("render spline point bound");
    }
    if r.remaining() < count * point_bytes {
        return Err("short render spline curve");
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        points.push(point(r)?);
    }
    Ok(NativeSplineCurve {
        looped,
        loop_key_offset,
        points,
    })
}
fn encode_spline(b: &mut Vec<u8>, frame: &NativeSplineFrame) -> Result<(), &'static str> {
    frame.validate()?;
    for flag in [frame.visible, frame.hidden, frame.owner_hidden] {
        b.put_u8(u8::from(flag));
    }
    b.put_u32(frame.version);
    let s = &frame.settings;
    b.put_u8(u8::from(s.allow_spline_editing_per_instance));
    b.put_u32(s.reparam_steps_per_segment as u32);
    b.put_u32(s.duration.to_bits());
    for flag in [
        s.stationary_endpoints,
        s.spline_has_been_edited,
        s.modified_by_construction_script,
        s.input_spline_points_to_construction_script,
        s.draw_debug,
        s.closed_loop,
        s.loop_position_override,
    ] {
        b.put_u8(u8::from(flag));
    }
    b.put_u32(s.loop_position.to_bits());
    put_f64_array(b, &s.default_up_vector);
    let vector = |b: &mut Vec<u8>, p: &NativeSplineVectorPoint| {
        b.put_u32(p.key.to_bits());
        put_f64_array(b, &p.out);
        put_f64_array(b, &p.arrive);
        put_f64_array(b, &p.leave);
        b.put_u8(p.interp);
    };
    encode_spline_curve(b, &frame.position, vector);
    encode_spline_curve(b, &frame.rotation, |b, p| {
        b.put_u32(p.key.to_bits());
        put_f64_array(b, &p.out);
        put_f64_array(b, &p.arrive);
        put_f64_array(b, &p.leave);
        b.put_u8(p.interp);
    });
    encode_spline_curve(b, &frame.scale, vector);
    encode_spline_curve(b, &frame.reparam, |b, p| {
        for value in [p.key, p.out, p.arrive, p.leave] {
            b.put_u32(value.to_bits());
        }
        b.put_u8(p.interp);
    });
    Ok(())
}
fn decode_spline(r: &mut Reader<'_>) -> Result<NativeSplineFrame, &'static str> {
    let visible = read_bool(r)?;
    let hidden = read_bool(r)?;
    let owner_hidden = read_bool(r)?;
    let version = r.u32().or_else(fail)?;
    let settings = NativeSplineSettings {
        allow_spline_editing_per_instance: read_bool(r)?,
        reparam_steps_per_segment: r.u32().or_else(fail)? as i32,
        duration: read_f32(r)?,
        stationary_endpoints: read_bool(r)?,
        spline_has_been_edited: read_bool(r)?,
        modified_by_construction_script: read_bool(r)?,
        input_spline_points_to_construction_script: read_bool(r)?,
        draw_debug: read_bool(r)?,
        closed_loop: read_bool(r)?,
        loop_position_override: read_bool(r)?,
        loop_position: read_f32(r)?,
        default_up_vector: read_f64_array(r)?,
    };
    let vector = |r: &mut Reader<'_>| {
        Ok(NativeSplineVectorPoint {
            key: read_f32(r)?,
            out: read_f64_array(r)?,
            arrive: read_f64_array(r)?,
            leave: read_f64_array(r)?,
            interp: r.u8().or_else(fail)?,
        })
    };
    let position = decode_spline_curve(r, MAX_SPLINE_POINTS, 77, vector)?;
    let rotation = decode_spline_curve(r, MAX_SPLINE_POINTS, 101, |r| {
        Ok(NativeSplineQuatPoint {
            key: read_f32(r)?,
            out: read_f64_array(r)?,
            arrive: read_f64_array(r)?,
            leave: read_f64_array(r)?,
            interp: r.u8().or_else(fail)?,
        })
    })?;
    let scale = decode_spline_curve(r, MAX_SPLINE_POINTS, 77, vector)?;
    let reparam = decode_spline_curve(r, MAX_SPLINE_REPARAM_POINTS, 17, |r| {
        Ok(NativeSplineFloatPoint {
            key: read_f32(r)?,
            out: read_f32(r)?,
            arrive: read_f32(r)?,
            leave: read_f32(r)?,
            interp: r.u8().or_else(fail)?,
        })
    })?;
    let frame = NativeSplineFrame {
        visible,
        hidden,
        owner_hidden,
        version,
        settings,
        position,
        rotation,
        scale,
        reparam,
    };
    frame.validate()?;
    Ok(frame)
}
#[derive(Debug, Clone, PartialEq)]
pub struct RenderMaterial {
    pub scalars: Vec<f32>,
    pub vectors: Vec<[f32; 4]>,
    pub textures: Vec<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RenderEntity {
    pub reference: EntityRef,
    pub revision: u32,
    pub components: Vec<RenderComponent>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RenderWorld {
    pub world: World,
    pub entities: Vec<RenderEntity>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorReady {
    pub epoch: u64,
    pub directory_seq: u32,
    pub frame_seq: u32,
    pub entities: Vec<(EntityRef, u32)>,
}

pub fn encode_descriptor(d: &Descriptor) -> Result<Vec<u8>, &'static str> {
    if !d.reference.valid()
        || d.slot as usize >= MAX_ENTITIES
        || d.directory_seq == 0
        || d.revision == 0
        || d.source_frame_seq == 0
    {
        return Err("descriptor identity");
    }
    let recipe = d
        .recipe
        .canonical_bytes()
        .map_err(|_| "descriptor recipe")?;
    if recipe.len() > 60 * 1024 {
        return Err("descriptor bound");
    }
    let mut b = Vec::with_capacity(30 + recipe.len());
    put_ref(&mut b, d.reference);
    b.put_u16(d.slot);
    b.put_u32(d.directory_seq);
    b.put_u32(d.revision);
    b.put_u32(d.source_frame_seq);
    b.put(&recipe);
    Ok(b)
}
pub fn decode_descriptor(b: &[u8]) -> Result<Descriptor, &'static str> {
    if b.len() > 60 * 1024 + 30 {
        return Err("descriptor bound");
    }
    let mut r = Reader::new(b);
    let reference = get_ref(&mut r)?;
    let slot = r.u16().or_else(fail)?;
    let directory_seq = r.u32().or_else(fail)?;
    let revision = r.u32().or_else(fail)?;
    let source_frame_seq = r.u32().or_else(fail)?;
    let recipe = crate::native_descriptor::SourceRecipe::decode_recipe(&b[30..])
        .map_err(|_| "descriptor recipe")?;
    let d = Descriptor {
        reference,
        slot,
        directory_seq,
        revision,
        source_frame_seq,
        recipe,
    };
    encode_descriptor(&d)?;
    Ok(d)
}
fn transform_ok(v: &[f64; 10]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() < 1e9)
        && (v[3..7].iter().map(|x| x * x).sum::<f64>() - 1.0).abs() < 0.01
        && v[7..].iter().all(|x| x.abs() < 1000.0)
}
pub fn encode_render_world(v: &RenderWorld) -> Result<Vec<u8>, &'static str> {
    encode_render_world_revision(v, false)
}
pub fn encode_render_world_v2(v: &RenderWorld) -> Result<Vec<u8>, &'static str> {
    encode_render_world_revision(v, true)
}
fn encode_render_world_revision(v: &RenderWorld, v2: bool) -> Result<Vec<u8>, &'static str> {
    let core = encode_world(&v.world)?;
    if v.entities.len() != v.world.entities.len() {
        return Err("render entity count");
    }
    let mut b = Vec::with_capacity(core.len() + 1024);
    b.put_u16(core.len() as u16);
    b.put(&core);
    for (e, source) in v.entities.iter().zip(&v.world.entities) {
        if e.reference != source.reference
            || e.revision == 0
            || e.components.is_empty()
            || e.components.len() > 64
        {
            return Err("render entity");
        }
        b.put_u32(e.revision);
        b.put_u8(e.components.len() as u8);
        let mut ids = std::collections::HashSet::new();
        for c in &e.components {
            if !v2 && c.spline.is_some() {
                return Err("render revision 1 has no spline support");
            }
            if c.id == 0
                || !ids.insert(c.id)
                || c.bones.len() > 512
                || c.morphs.len() > 512
                || !transform_ok(&c.transform)
                || !c.bones.iter().all(transform_ok)
                || !c.morphs.iter().all(|x| x.is_finite())
            {
                return Err("render component");
            }
            b.put_u32(c.id);
            for x in c.transform {
                b.put_u64(x.to_bits());
            }
            b.put_u16(c.bones.len() as u16);
            for bone in &c.bones {
                for x in bone {
                    b.put_u64(x.to_bits());
                }
            }
            b.put_u16(c.morphs.len() as u16);
            for x in &c.morphs {
                b.put_u32(x.to_bits());
            }
            if c.materials.len() > 32 {
                return Err("render material bound");
            }
            b.put_u8(c.materials.len() as u8);
            for m in &c.materials {
                if m.scalars.len() > 128
                    || m.vectors.len() > 128
                    || m.textures.len() > 128
                    || !m.scalars.iter().all(|x| x.is_finite())
                    || !m.vectors.iter().flatten().all(|x| x.is_finite())
                {
                    return Err("render material");
                }
                b.put_u8(m.scalars.len() as u8);
                for x in &m.scalars {
                    b.put_u32(x.to_bits());
                }
                b.put_u8(m.vectors.len() as u8);
                for v in &m.vectors {
                    for x in v {
                        b.put_u32(x.to_bits());
                    }
                }
                b.put_u8(m.textures.len() as u8);
                for t in &m.textures {
                    if !text_ok(t, 512) {
                        return Err("render texture");
                    }
                    b.put_u16(t.len() as u16);
                    b.put(t.as_bytes());
                }
            }
            if v2 {
                b.put_u8(u8::from(c.spline.is_some()));
                if let Some(spline) = &c.spline {
                    encode_spline(&mut b, spline)?;
                }
            }
            if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
                return Err("render world bound");
            }
        }
    }
    Ok(b)
}
pub fn decode_render_world(b: &[u8]) -> Result<RenderWorld, &'static str> {
    decode_render_world_revision(b, false)
}
pub fn decode_render_world_v2(b: &[u8]) -> Result<RenderWorld, &'static str> {
    decode_render_world_revision(b, true)
}
fn decode_render_world_revision(b: &[u8], v2: bool) -> Result<RenderWorld, &'static str> {
    if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
        return Err("render world bound");
    }
    let mut r = Reader::new(b);
    let n = r.u16().or_else(fail)? as usize;
    let world = decode_world(r.bytes(n).or_else(fail)?)?;
    let mut entities = Vec::with_capacity(world.entities.len());
    for source in &world.entities {
        let revision = r.u32().or_else(fail)?;
        let n = r.u8().or_else(fail)? as usize;
        if n > 64 {
            return Err("render component bound");
        }
        let mut components = Vec::with_capacity(n);
        for _ in 0..n {
            let id = r.u32().or_else(fail)?;
            let mut transform = [0f64; 10];
            for x in &mut transform {
                *x = f64::from_bits(r.u64().or_else(fail)?);
            }
            let n = r.u16().or_else(fail)? as usize;
            if n > 512 {
                return Err("render bone bound");
            }
            let mut bones = Vec::with_capacity(n);
            for _ in 0..n {
                let mut bone = [0f64; 10];
                for x in &mut bone {
                    *x = f64::from_bits(r.u64().or_else(fail)?);
                }
                bones.push(bone);
            }
            let n = r.u16().or_else(fail)? as usize;
            if n > 512 {
                return Err("render morph bound");
            }
            let mut morphs = Vec::with_capacity(n);
            for _ in 0..n {
                morphs.push(f32::from_bits(r.u32().or_else(fail)?));
            }
            let n = r.u8().or_else(fail)? as usize;
            if n > 32 {
                return Err("render material bound");
            }
            let mut materials = Vec::with_capacity(n);
            for _ in 0..n {
                let n = r.u8().or_else(fail)? as usize;
                if n > 128 {
                    return Err("render scalar bound");
                }
                let mut scalars = Vec::with_capacity(n);
                for _ in 0..n {
                    scalars.push(f32::from_bits(r.u32().or_else(fail)?));
                }
                let n = r.u8().or_else(fail)? as usize;
                if n > 128 {
                    return Err("render vector bound");
                }
                let mut vectors = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut v = [0f32; 4];
                    for x in &mut v {
                        *x = f32::from_bits(r.u32().or_else(fail)?);
                    }
                    vectors.push(v);
                }
                let n = r.u8().or_else(fail)? as usize;
                if n > 128 {
                    return Err("render texture bound");
                }
                let mut textures = Vec::with_capacity(n);
                for _ in 0..n {
                    let n = r.u16().or_else(fail)? as usize;
                    if n > 512 {
                        return Err("render texture bound");
                    }
                    textures.push(
                        std::str::from_utf8(r.bytes(n).or_else(fail)?)
                            .map_err(|_| "texture utf8")?
                            .to_owned(),
                    );
                }
                materials.push(RenderMaterial {
                    scalars,
                    vectors,
                    textures,
                });
            }
            components.push(RenderComponent {
                id,
                transform,
                bones,
                morphs,
                materials,
                spline: if v2 && read_bool(&mut r)? {
                    Some(decode_spline(&mut r)?)
                } else {
                    None
                },
            });
        }
        entities.push(RenderEntity {
            reference: source.reference,
            revision,
            components,
        });
    }
    if !r.is_empty() {
        return Err("trailing render bytes");
    }
    let v = RenderWorld { world, entities };
    encode_render_world_revision(&v, v2)?;
    Ok(v)
}
pub fn encode_mirror_ready(v: &MirrorReady) -> Result<Vec<u8>, &'static str> {
    if v.epoch == 0
        || v.directory_seq == 0
        || v.frame_seq == 0
        || v.entities.is_empty()
        || v.entities.len() > MAX_ENTITIES
    {
        return Err("mirror ready");
    }
    let mut b = Vec::new();
    b.put_u64(v.epoch);
    b.put_u32(v.directory_seq);
    b.put_u32(v.frame_seq);
    b.put_u8(v.entities.len() as u8);
    let mut seen = std::collections::HashSet::new();
    for (r, rev) in &v.entities {
        if !r.valid() || r.epoch != v.epoch || *rev == 0 || !seen.insert(r.id) {
            return Err("mirror ready identity");
        }
        put_ref(&mut b, *r);
        b.put_u32(*rev);
    }
    Ok(b)
}
pub fn decode_mirror_ready(b: &[u8]) -> Result<MirrorReady, &'static str> {
    let mut r = Reader::new(b);
    let epoch = r.u64().or_else(fail)?;
    let directory_seq = r.u32().or_else(fail)?;
    let frame_seq = r.u32().or_else(fail)?;
    let n = r.u8().or_else(fail)? as usize;
    if n > MAX_ENTITIES {
        return Err("mirror ready bound");
    }
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        entities.push((get_ref(&mut r)?, r.u32().or_else(fail)?));
    }
    if !r.is_empty() {
        return Err("trailing mirror bytes");
    }
    let v = MirrorReady {
        epoch,
        directory_seq,
        frame_seq,
        entities,
    };
    encode_mirror_ready(&v)?;
    Ok(v)
}

fn fail<T>(_: hsmp_net::net::wire::Truncated) -> Result<T, &'static str> {
    Err("truncated")
}
fn put_ref(b: &mut Vec<u8>, r: EntityRef) {
    b.put_u64(r.epoch);
    b.put_u32(r.id);
    b.put_u32(r.incarnation);
}
fn get_ref(r: &mut Reader<'_>) -> Result<EntityRef, &'static str> {
    let v = EntityRef {
        epoch: r.u64().or_else(fail)?,
        id: r.u32().or_else(fail)?,
        incarnation: r.u32().or_else(fail)?,
    };
    if !v.valid() {
        return Err("entity identity");
    }
    Ok(v)
}
fn text_ok(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn put_text(b: &mut Vec<u8>, s: &str) {
    b.put_u8(s.len() as u8);
    b.put(s.as_bytes());
}
fn get_text(r: &mut Reader<'_>, max: usize) -> Result<String, &'static str> {
    let n = r.u8().or_else(fail)? as usize;
    if n > max {
        return Err("string bound");
    }
    let s = std::str::from_utf8(r.bytes(n).or_else(fail)?).map_err(|_| "utf8")?;
    if !text_ok(s, max) {
        return Err("string");
    }
    Ok(s.to_owned())
}
pub fn validate_directory(d: &Directory) -> Result<(), &'static str> {
    if d.epoch == 0
        || d.seq == 0
        || d.state > FAULT
        || !text_ok(&d.arena, 40)
        || !text_ok(&d.error, 96)
        || d.entities.is_empty()
        || d.entities.len() > MAX_ENTITIES
    {
        return Err("directory");
    }
    let mut ids = std::collections::HashSet::new();
    let mut slots = std::collections::HashSet::new();
    let mut owners = std::collections::HashSet::new();
    for e in &d.entities {
        if !e.reference.valid()
            || e.reference.epoch != d.epoch
            || !ids.insert(e.reference.id)
            || e.slot as usize >= MAX_ENTITIES
            || !slots.insert(e.slot)
            || e.kind > AI
            || (e.kind == AI && (e.owner_peer != 0 || e.controller != 255))
            || (e.kind == HUMAN && e.controller > 1)
            || (e.owner_peer != 0 && !owners.insert(e.owner_peer))
        {
            return Err("directory entity");
        }
    }
    Ok(())
}
pub fn encode_directory(d: &Directory) -> Result<Vec<u8>, &'static str> {
    validate_directory(d)?;
    let mut b = Vec::with_capacity(64 + d.entities.len() * 26);
    b.put_u64(d.epoch);
    b.put_u32(d.seq);
    b.put_u8(d.state);
    b.put_u8(d.entities.len() as u8);
    put_text(&mut b, &d.arena);
    put_text(&mut b, &d.error);
    for e in &d.entities {
        put_ref(&mut b, e.reference);
        b.put_u32(e.owner_peer);
        b.put_u16(e.slot);
        b.put_u8(e.kind);
        b.put_u8(e.controller);
        b.put_u8(e.team.is_some() as u8);
        if let Some(team) = e.team {
            b.put_u32(team as u32);
        }
    }
    Ok(b)
}
pub fn decode_directory(b: &[u8]) -> Result<Directory, &'static str> {
    let mut r = Reader::new(b);
    let epoch = r.u64().or_else(fail)?;
    let seq = r.u32().or_else(fail)?;
    let state = r.u8().or_else(fail)?;
    let n = r.u8().or_else(fail)? as usize;
    if n > MAX_ENTITIES {
        return Err("entity bound");
    }
    let arena = get_text(&mut r, 40)?;
    let error = get_text(&mut r, 96)?;
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        let reference = get_ref(&mut r)?;
        let owner_peer = r.u32().or_else(fail)?;
        let slot = r.u16().or_else(fail)?;
        let kind = r.u8().or_else(fail)?;
        let controller = r.u8().or_else(fail)?;
        let team = match r.u8().or_else(fail)? {
            0 => None,
            1 => Some(r.u32().or_else(fail)? as i32),
            _ => return Err("team presence"),
        };
        entities.push(Entity {
            reference,
            owner_peer,
            slot,
            kind,
            controller,
            team,
        });
    }
    if !r.is_empty() {
        return Err("trailing bytes");
    }
    let d = Directory {
        epoch,
        seq,
        state,
        arena,
        error,
        entities,
    };
    validate_directory(&d)?;
    Ok(d)
}
pub fn encode_input(i: &InputFrame) -> Result<Vec<u8>, &'static str> {
    i.validate()?;
    let mut b = Vec::with_capacity(64);
    put_ref(&mut b, i.reference);
    b.put_u32(i.seq);
    b.put_u32(i.delivery_seq);
    b.put_u64(i.sample_ms);
    b.put_u32(i.buttons);
    b.put_u32(i.flags);
    for x in i.axes {
        b.put_u32(x.to_bits());
    }
    Ok(b)
}
pub fn decode_input(b: &[u8]) -> Result<InputFrame, &'static str> {
    let mut r = Reader::new(b);
    let mut i = InputFrame {
        reference: get_ref(&mut r)?,
        seq: r.u32().or_else(fail)?,
        delivery_seq: r.u32().or_else(fail)?,
        sample_ms: r.u64().or_else(fail)?,
        buttons: r.u32().or_else(fail)?,
        flags: r.u32().or_else(fail)?,
        ..Default::default()
    };
    for x in &mut i.axes {
        *x = f32::from_bits(r.u32().or_else(fail)?);
    }
    if !r.is_empty() {
        return Err("trailing bytes");
    }
    i.validate()?;
    Ok(i)
}
pub fn validate_world(w: &World) -> Result<(), &'static str> {
    if w.epoch == 0
        || w.directory_seq == 0
        || w.frame_seq == 0
        || w.entities.is_empty()
        || w.entities.len() > MAX_ENTITIES
    {
        return Err("world");
    }
    let context = hsmp_pose::posecodec::v2::Context {
        match_id: w.epoch,
        round: 1,
        life: 1,
    };
    let mut ids = std::collections::HashSet::new();
    for e in &w.entities {
        if !e.reference.valid()
            || e.reference.epoch != w.epoch
            || !ids.insert(e.reference.id)
            || e.pose.len() > POSE_FRAME_MAX
        {
            return Err("snapshot identity");
        }
        hsmp_ipc::record::view::<Root>(bytemuck::bytes_of(&e.root)).map_err(|_| "root")?;
        hsmp_ipc::record::view::<Vitals>(bytemuck::bytes_of(&e.vitals)).map_err(|_| "vitals")?;
        if e.root.match_id != w.epoch
            || e.root.round != 1
            || e.root.life != 1
            || e.vitals.match_id != w.epoch
            || e.vitals.round != 1
            || e.vitals.life != 1
        {
            return Err("snapshot context");
        }
        let p = hsmp_pose::posecodec::v2::decode(&e.pose).ok_or("pose")?;
        if p.context != Some(context) || p.ts.floor() as u32 != e.root.ts {
            return Err("pose context");
        }
    }
    Ok(())
}
pub fn encode_world(w: &World) -> Result<Vec<u8>, &'static str> {
    validate_world(w)?;
    let mut b = Vec::with_capacity(16 + w.entities.len() * 1300);
    b.put_u64(w.epoch);
    b.put_u32(w.directory_seq);
    b.put_u32(w.frame_seq);
    b.put_u8(w.entities.len() as u8);
    for e in &w.entities {
        put_ref(&mut b, e.reference);
        b.put(bytemuck::bytes_of(&e.root));
        b.put(bytemuck::bytes_of(&e.vitals));
        b.put_u16(e.pose.len() as u16);
        b.put(&e.pose);
    }
    if b.len() + 8 > hsmp_net::net::frag::MAX_MESSAGE {
        return Err("world bound");
    }
    Ok(b)
}
pub fn decode_world(b: &[u8]) -> Result<World, &'static str> {
    let mut r = Reader::new(b);
    let epoch = r.u64().or_else(fail)?;
    let directory_seq = r.u32().or_else(fail)?;
    let frame_seq = r.u32().or_else(fail)?;
    let n = r.u8().or_else(fail)? as usize;
    if n > MAX_ENTITIES {
        return Err("entity bound");
    }
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        let reference = get_ref(&mut r)?;
        let root =
            bytemuck::pod_read_unaligned(r.bytes(std::mem::size_of::<Root>()).or_else(fail)?);
        let vitals =
            bytemuck::pod_read_unaligned(r.bytes(std::mem::size_of::<Vitals>()).or_else(fail)?);
        let len = r.u16().or_else(fail)? as usize;
        if len > POSE_FRAME_MAX {
            return Err("pose bound");
        }
        let pose = r.bytes(len).or_else(fail)?.to_vec();
        entities.push(EntitySnapshot {
            reference,
            root,
            vitals,
            pose,
        });
    }
    if !r.is_empty() {
        return Err("trailing bytes");
    }
    let w = World {
        epoch,
        directory_seq,
        frame_seq,
        entities,
    };
    validate_world(&w)?;
    Ok(w)
}
pub fn matches_directory(w: &World, d: &Directory) -> bool {
    w.epoch == d.epoch
        && w.directory_seq == d.seq
        && w.entities.len() == d.entities.len()
        && w.entities
            .iter()
            .all(|e| d.entities.iter().any(|x| x.reference == e.reference))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn spline_fixture() -> NativeSplineFrame {
        let vector = NativeSplineVectorPoint {
            key: -0.0,
            out: [0.12345678901234567, -0.0, 3.0],
            arrive: [0.0, 0.0, 0.0],
            leave: [-2.0, 4.0, 8.0],
            interp: 5,
        };
        NativeSplineFrame {
            visible: true,
            hidden: false,
            owner_hidden: false,
            version: u32::MAX,
            settings: NativeSplineSettings {
                allow_spline_editing_per_instance: true,
                reparam_steps_per_segment: -3,
                duration: 1.25,
                stationary_endpoints: false,
                spline_has_been_edited: true,
                modified_by_construction_script: false,
                input_spline_points_to_construction_script: true,
                draw_debug: true,
                closed_loop: true,
                loop_position_override: true,
                loop_position: -0.0,
                default_up_vector: [0.0, -0.0, 1.0],
            },
            position: NativeSplineCurve {
                looped: true,
                loop_key_offset: 3.25,
                points: vec![vector.clone()],
            },
            rotation: NativeSplineCurve {
                looped: false,
                loop_key_offset: -0.0,
                points: vec![NativeSplineQuatPoint {
                    key: 0.125,
                    out: [2.0, -3.0, 4.0, 5.0],
                    arrive: [0.0; 4],
                    leave: [-0.0, 2.0, 3.0, 4.0],
                    interp: 3,
                }],
            },
            scale: NativeSplineCurve {
                looped: false,
                loop_key_offset: 0.0,
                points: vec![vector],
            },
            reparam: NativeSplineCurve {
                looped: false,
                loop_key_offset: 0.0,
                points: vec![NativeSplineFloatPoint {
                    key: -0.0,
                    out: 3.0,
                    arrive: 0.0,
                    leave: -0.0,
                    interp: 0,
                }],
            },
        }
    }
    fn spline_world() -> RenderWorld {
        let reference = EntityRef {
            epoch: 9,
            id: 1,
            incarnation: 2,
        };
        RenderWorld {
            world: World {
                epoch: 9,
                directory_seq: 1,
                frame_seq: 1,
                entities: vec![EntitySnapshot {
                    reference,
                    root: Root {
                        rot: [0.0, 0.0, 0.0, 1.0],
                        match_id: 9,
                        round: 1,
                        life: 1,
                        ..Default::default()
                    },
                    vitals: Vitals {
                        match_id: 9,
                        round: 1,
                        life: 1,
                        ..Default::default()
                    },
                    pose: hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full {
                        context: Some(hsmp_pose::posecodec::v2::Context {
                            match_id: 9,
                            round: 1,
                            life: 1,
                        }),
                        ..Default::default()
                    }),
                }],
            },
            entities: vec![RenderEntity {
                reference,
                revision: 4,
                components: vec![RenderComponent {
                    id: 1,
                    transform: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
                    bones: vec![],
                    morphs: vec![],
                    materials: vec![],
                    spline: Some(spline_fixture()),
                }],
            }],
        }
    }
    #[test]
    fn native_spline_v2_preserves_raw_curves_and_rejects_all_truncations() {
        let frame = spline_world();
        let encoded = encode_render_world_v2(&frame).unwrap();
        let decoded = decode_render_world_v2(&encoded).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(
            encode_render_world_v2(&decoded).unwrap(),
            encoded,
            "signed zero and all raw bits must survive"
        );
        for n in 0..encoded.len() {
            assert!(
                decode_render_world_v2(&encoded[..n]).is_err(),
                "accepted V2 truncation {n}"
            );
        }
        let mut extra = encoded.clone();
        extra.push(0);
        assert!(decode_render_world_v2(&extra).is_err());
        assert!(decode_render_world(&encoded).is_err());
        assert!(encode_render_world(&frame).is_err());
        let mut no_spline = frame;
        no_spline.entities[0].components[0].spline = None;
        let v2 = encode_render_world_v2(&no_spline).unwrap();
        assert_eq!(decode_render_world_v2(&v2).unwrap(), no_spline);
        let v1 = encode_render_world(&no_spline).unwrap();
        assert!(decode_render_world_v2(&v1).is_err());
    }
    #[test]
    fn native_spline_decoder_checks_flags_counts_and_finiteness_before_use() {
        let spline = spline_fixture();
        let mut bytes = Vec::new();
        encode_spline(&mut bytes, &spline).unwrap();
        // Fixed scalar prefix is 51 bytes; position header is bool/f32/u16.
        for offset in [0, 1, 2, 7, 16, 17, 18, 19, 20, 21, 22, 51] {
            let mut bad = bytes.clone();
            bad[offset] = 2;
            assert!(
                decode_spline(&mut Reader::new(&bad)).is_err(),
                "accepted invalid flag at {offset}"
            );
        }
        let mut bad = bytes.clone();
        bad[56..58].copy_from_slice(&65u16.to_le_bytes());
        assert_eq!(
            decode_spline(&mut Reader::new(&bad)).unwrap_err(),
            "render spline point bound"
        );
        let mut bad = bytes.clone();
        bad[58..62].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(decode_spline(&mut Reader::new(&bad)).is_err());
        let mut bad = bytes;
        bad[134] = 6;
        assert!(decode_spline(&mut Reader::new(&bad)).is_err());
        let mut bad = spline.clone();
        bad.rotation.points[0].leave[2] = f64::INFINITY;
        assert!(bad.validate().is_err());
        let mut bad = spline.clone();
        bad.reparam
            .points
            .resize(MAX_SPLINE_REPARAM_POINTS + 1, bad.reparam.points[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = spline;
        bad.settings.default_up_vector[1] = f64::NAN;
        assert!(bad.validate().is_err());
    }
    #[test]
    fn native_spline_aggregate_keeps_the_existing_message_bound() {
        let mut frame = spline_world();
        let spline = frame.entities[0].components[0].spline.as_mut().unwrap();
        spline
            .position
            .points
            .resize(MAX_SPLINE_POINTS, spline.position.points[0].clone());
        spline
            .rotation
            .points
            .resize(MAX_SPLINE_POINTS, spline.rotation.points[0].clone());
        spline
            .scale
            .points
            .resize(MAX_SPLINE_POINTS, spline.scale.points[0].clone());
        spline
            .reparam
            .points
            .resize(MAX_SPLINE_REPARAM_POINTS, spline.reparam.points[0].clone());
        assert!(encode_render_world_v2(&frame).is_ok());
        let mut second = frame.entities[0].components[0].clone();
        second.id = 2;
        frame.entities[0].components.push(second);
        assert_eq!(
            encode_render_world_v2(&frame).unwrap_err(),
            "render world bound"
        );
        let too_large = vec![0; hsmp_net::net::frag::MAX_MESSAGE];
        assert_eq!(
            decode_render_world_v2(&too_large).unwrap_err(),
            "render world bound"
        );
    }
    #[test]
    fn native_wire_rejects_all_truncations_trailing_bytes_and_unbounded_fields() {
        let reference = EntityRef {
            epoch: 9,
            id: 1,
            incarnation: 2,
        };
        let d = Directory {
            epoch: 9,
            seq: 1,
            state: READY,
            arena: "Map_Arena_Yard".into(),
            error: String::new(),
            entities: vec![Entity {
                reference,
                owner_peer: 1,
                slot: 0,
                kind: HUMAN,
                controller: 0,
                team: None,
            }],
        };
        let encoded = encode_directory(&d).unwrap();
        assert_eq!(decode_directory(&encoded).unwrap(), d);
        for n in 0..encoded.len() {
            assert!(
                decode_directory(&encoded[..n]).is_err(),
                "accepted directory truncation {n}"
            );
        }
        let mut extra = encoded.clone();
        extra.push(0);
        assert!(decode_directory(&extra).is_err());
        let input = InputFrame {
            reference,
            seq: 1,
            sample_ms: 4,
            axes: [0.2, 0.0, 3.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let encoded = encode_input(&input).unwrap();
        assert_eq!(decode_input(&encoded).unwrap(), input);
        for n in 0..encoded.len() {
            assert!(
                decode_input(&encoded[..n]).is_err(),
                "accepted input truncation {n}"
            );
        }
        let mut bad = input;
        bad.axes[2] = f32::NAN;
        assert!(encode_input(&bad).is_err());
        bad = input;
        bad.buttons = 0x80;
        assert!(encode_input(&bad).is_err());
        let mut bad = d.clone();
        bad.entities[0].slot = 32;
        assert!(encode_directory(&bad).is_err());
        bad = d.clone();
        bad.entities.push(bad.entities[0].clone());
        assert!(encode_directory(&bad).is_err());
        let context = hsmp_pose::posecodec::v2::Context {
            match_id: 9,
            round: 1,
            life: 1,
        };
        let world = World {
            epoch: 9,
            directory_seq: 1,
            frame_seq: 1,
            entities: vec![EntitySnapshot {
                reference,
                root: Root {
                    rot: [0.0, 0.0, 0.0, 1.0],
                    match_id: 9,
                    round: 1,
                    life: 1,
                    ..Default::default()
                },
                vitals: Vitals {
                    match_id: 9,
                    round: 1,
                    life: 1,
                    ..Default::default()
                },
                pose: hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full {
                    context: Some(context),
                    ..Default::default()
                }),
            }],
        };
        let encoded = encode_world(&world).unwrap();
        assert_eq!(decode_world(&encoded).unwrap(), world);
        for n in 0..encoded.len() {
            assert!(
                decode_world(&encoded[..n]).is_err(),
                "accepted world truncation {n}"
            );
        }
        let mut wrong = world.clone();
        wrong.entities[0].root.life = 2;
        assert!(encode_world(&wrong).is_err());
        wrong = world.clone();
        wrong.entities[0].pose.resize(POSE_FRAME_MAX + 1, 0);
        assert!(encode_world(&wrong).is_err());
        wrong = world.clone();
        wrong.entities[0].reference.incarnation += 1;
        assert!(!matches_directory(&wrong, &d));
        wrong = world;
        wrong.epoch += 1;
        assert!(!matches_directory(&wrong, &d));
    }
    #[test]
    fn native_render_frames_are_complete_bounded_and_preserve_native_double_values() {
        let reference = EntityRef {
            epoch: 9,
            id: 1,
            incarnation: 2,
        };
        let context = hsmp_pose::posecodec::v2::Context {
            match_id: 9,
            round: 1,
            life: 1,
        };
        let world = World {
            epoch: 9,
            directory_seq: 1,
            frame_seq: 1,
            entities: vec![EntitySnapshot {
                reference,
                root: Root {
                    rot: [0.0, 0.0, 0.0, 1.0],
                    match_id: 9,
                    round: 1,
                    life: 1,
                    ..Default::default()
                },
                vitals: Vitals {
                    match_id: 9,
                    round: 1,
                    life: 1,
                    ..Default::default()
                },
                pose: hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full {
                    context: Some(context),
                    ..Default::default()
                }),
            }],
        };
        let transform = [
            0.12345678901234567,
            2.0,
            3.0,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
            1.0,
        ];
        let frame = RenderWorld {
            world,
            entities: vec![RenderEntity {
                reference,
                revision: 4,
                components: vec![RenderComponent {
                    id: 1,
                    transform,
                    bones: vec![transform],
                    morphs: vec![0.125],
                    materials: vec![RenderMaterial {
                        scalars: vec![0.5],
                        vectors: vec![[0.1, 0.2, 0.3, 1.0]],
                        textures: vec!["/Game/Test.Tex".into()],
                    }],
                    spline: None,
                }],
            }],
        };
        let bytes = encode_render_world(&frame).unwrap();
        assert_eq!(decode_render_world(&bytes).unwrap(), frame);
        for n in 0..bytes.len() {
            assert!(
                decode_render_world(&bytes[..n]).is_err(),
                "accepted render truncation {n}"
            );
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode_render_world(&extra).is_err());
        let mut bad = frame.clone();
        bad.entities[0].components[0].bones.resize(513, transform);
        assert!(encode_render_world(&bad).is_err());
        bad = frame.clone();
        bad.entities[0].components[0].bones.resize(512, transform);
        let copy = bad.entities[0].components[0].clone();
        bad.entities[0].components.push(copy);
        bad.entities[0].components[1].id = 2;
        assert_eq!(encode_render_world(&bad).unwrap_err(), "render world bound");
        bad = frame;
        bad.entities[0].components[0].transform[0] = f64::NAN;
        assert!(encode_render_world(&bad).is_err());
    }
}

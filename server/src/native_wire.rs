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
            if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
                return Err("render world bound");
            }
        }
    }
    Ok(b)
}
pub fn decode_render_world(b: &[u8]) -> Result<RenderWorld, &'static str> {
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
    encode_render_world(&v)?;
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
mod tests {
    use super::*;
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

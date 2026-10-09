//! Lossless native scene batches. The 64 KiB transport message limit is unchanged.
//! One admitted immutable batch is finished before a newer batch can replace it.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const PART_BYTES: usize = 60 * 1024;
pub const ACK_ADMITTED: u8 = 0;
pub const ACK_COMPLETE: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub epoch: u64,
    pub directory_seq: u32,
    pub frame_seq: u32,
    pub bytes: u32,
    pub digest: [u8; 32],
}
impl Token {
    pub fn parts(&self) -> usize {
        (self.bytes as usize).div_ceil(PART_BYTES)
    }
    fn valid(&self) -> bool {
        self.epoch != 0 && self.directory_seq != 0 && self.frame_seq != 0 && self.bytes != 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub token: Token,
    pub entities: Vec<(EntityRef, u32)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub token: Token,
    pub stage: u8,
}
#[derive(Clone, Debug)]
pub struct Batch {
    pub manifest: Manifest,
    body: Arc<[u8]>,
}

/// Checked allocation bound derived from the complete accepted recipe dictionaries.
/// Every scalar and curve point keeps its existing native representation.
pub fn frame_bound(recipes: &[Arc<Descriptor>]) -> Result<usize, &'static str> {
    if recipes.is_empty() || recipes.len() > MAX_ENTITIES {
        return Err("scene recipe count");
    }
    let mut n = 2usize + MAX_WORLD_BYTES;
    for d in recipes {
        d.recipe.validate()?;
        n = n.checked_add(5).ok_or("scene size overflow")?;
        for c in &d.recipe.components {
            let mut bytes = 91 + c.bones.len() * 80 + c.morphs.len() * 4;
            for m in &c.materials {
                bytes +=
                    3 + m.scalars.len() * 4 + m.vectors.len() * 16 + m.textures.len() * (2 + 512);
            }
            if let Some(s) = &c.spline_profile {
                bytes += 79
                    + (usize::from(s.position_count) + usize::from(s.scale_count)) * 77
                    + usize::from(s.rotation_count) * 101
                    + usize::from(s.reparam_count) * 17;
            }
            if matches!(
                c.scene,
                crate::native_descriptor::SceneEvidence::SpringArm { .. }
            ) {
                bytes += 56;
            }
            n = n.checked_add(bytes).ok_or("scene size overflow")?;
        }
    }
    if n > u32::MAX as usize {
        return Err("scene size bound");
    }
    Ok(n)
}
pub fn matches_recipes(frame: &RenderWorld, recipes: &[Arc<Descriptor>]) -> bool {
    frame.entities.len() == recipes.len()
        && frame.entities.iter().zip(recipes).all(|(e, d)| {
            d.reference == e.reference
                && d.directory_seq == frame.world.directory_seq
                && d.revision == e.revision
                && d.source_frame_seq <= frame.world.frame_seq
                && e.components.len() == d.recipe.components.len()
                && e.components.iter().all(|c| {
                    d.recipe.components.iter().any(|r| {
                        r.id == c.id
                            && r.bones.len() == c.bones.len()
                            && r.morphs.len() == c.morphs.len()
                            && r.materials.len() == c.materials.len()
                            && match (&r.spline_profile, &c.spline) {
                                (Some(a), Some(b)) => {
                                    a.metadata_null
                                        && b.validate().is_ok()
                                        && b.position.points.len() == usize::from(a.position_count)
                                        && b.rotation.points.len() == usize::from(a.rotation_count)
                                        && b.scale.points.len() == usize::from(a.scale_count)
                                        && b.reparam.points.len() == usize::from(a.reparam_count)
                                }
                                (None, None) => true,
                                _ => false,
                            }
                            && match (&r.scene, &c.spring_arm) {
                                (
                                    crate::native_descriptor::SceneEvidence::SpringArm { .. },
                                    Some(a),
                                ) => a.validate().is_ok(),
                                (
                                    crate::native_descriptor::SceneEvidence::SpringArm { .. },
                                    None,
                                ) => false,
                                (_, None) => true,
                                (_, Some(_)) => false,
                            }
                            && r.materials.iter().zip(&c.materials).all(|(a, b)| {
                                a.scalars.len() == b.scalars.len()
                                    && a.vectors.len() == b.vectors.len()
                                    && a.textures.len() == b.textures.len()
                            })
                    })
                })
        })
}
pub fn encode_scene(
    frame: &RenderWorld,
    recipes: &[Arc<Descriptor>],
) -> Result<Vec<u8>, &'static str> {
    if !matches_recipes(frame, recipes) {
        return Err("scene recipe generation");
    }
    encode_render_world_revision(frame, 3, frame_bound(recipes)?)
}
pub fn decode_scene(body: &[u8], recipes: &[Arc<Descriptor>]) -> Result<RenderWorld, &'static str> {
    let frame = decode_render_world_revision(body, 3, frame_bound(recipes)?)?;
    if !matches_recipes(&frame, recipes) {
        return Err("scene recipe generation");
    }
    Ok(frame)
}
fn put_token(b: &mut Vec<u8>, t: &Token) {
    b.put_u64(t.epoch);
    b.put_u32(t.directory_seq);
    b.put_u32(t.frame_seq);
    b.put_u32(t.bytes);
    b.put(&t.digest);
}
fn read_token(r: &mut Reader<'_>) -> Result<Token, &'static str> {
    let t = Token {
        epoch: r.u64().or_else(fail)?,
        directory_seq: r.u32().or_else(fail)?,
        frame_seq: r.u32().or_else(fail)?,
        bytes: r.u32().or_else(fail)?,
        digest: r
            .bytes(32)
            .or_else(fail)?
            .try_into()
            .map_err(|_| "scene digest")?,
    };
    if !t.valid() {
        return Err("scene token");
    }
    Ok(t)
}
pub fn encode_manifest(m: &Manifest) -> Result<Vec<u8>, &'static str> {
    if !m.token.valid() || m.entities.is_empty() || m.entities.len() > MAX_ENTITIES {
        return Err("scene manifest");
    }
    let mut seen = std::collections::HashSet::new();
    let mut b = Vec::new();
    put_token(&mut b, &m.token);
    b.put_u8(m.entities.len() as u8);
    for (r, v) in &m.entities {
        if !r.valid() || r.epoch != m.token.epoch || *v == 0 || !seen.insert(r.id) {
            return Err("scene manifest identity");
        }
        put_ref(&mut b, *r);
        b.put_u32(*v);
    }
    Ok(b)
}
pub fn decode_manifest(b: &[u8]) -> Result<Manifest, &'static str> {
    let mut r = Reader::new(b);
    let token = read_token(&mut r)?;
    let n = r.u8().or_else(fail)? as usize;
    if n == 0 || n > MAX_ENTITIES {
        return Err("scene manifest count");
    }
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        entities.push((get_ref(&mut r)?, r.u32().or_else(fail)?));
    }
    if !r.is_empty() {
        return Err("scene manifest trailing");
    }
    let m = Manifest { token, entities };
    encode_manifest(&m)?;
    Ok(m)
}
pub fn encode_ack(a: &Ack) -> Result<Vec<u8>, &'static str> {
    if !a.token.valid() || a.stage > ACK_COMPLETE {
        return Err("scene ack");
    }
    let mut b = Vec::new();
    put_token(&mut b, &a.token);
    b.put_u8(a.stage);
    Ok(b)
}
pub fn decode_ack(b: &[u8]) -> Result<Ack, &'static str> {
    let mut r = Reader::new(b);
    let a = Ack {
        token: read_token(&mut r)?,
        stage: r.u8().or_else(fail)?,
    };
    if !r.is_empty() {
        return Err("scene ack trailing");
    }
    encode_ack(&a)?;
    Ok(a)
}
impl Batch {
    pub fn new(frame: &RenderWorld, recipes: &[Arc<Descriptor>]) -> Result<Self, &'static str> {
        let body = encode_scene(frame, recipes)?;
        let token = Token {
            epoch: frame.world.epoch,
            directory_seq: frame.world.directory_seq,
            frame_seq: frame.world.frame_seq,
            bytes: u32::try_from(body.len()).map_err(|_| "scene bytes")?,
            digest: Sha256::digest(&body).into(),
        };
        Ok(Self {
            manifest: Manifest {
                token,
                entities: frame
                    .entities
                    .iter()
                    .map(|e| (e.reference, e.revision))
                    .collect(),
            },
            body: body.into(),
        })
    }
    pub fn part(&self, index: usize) -> Result<Vec<u8>, &'static str> {
        if index >= self.manifest.token.parts() {
            return Err("scene part index");
        }
        let mut b = Vec::with_capacity(PART_BYTES + 56);
        put_token(&mut b, &self.manifest.token);
        b.put_u32(index as u32);
        let start = index * PART_BYTES;
        b.put(&self.body[start..(start + PART_BYTES).min(self.body.len())]);
        if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
            return Err("scene part message bound");
        }
        Ok(b)
    }
}
pub fn manifest_recipes(
    m: &Manifest,
    directory: &Directory,
    recipes: &[Arc<Descriptor>],
) -> Result<(), &'static str> {
    if m.token.epoch != directory.epoch
        || m.token.directory_seq != directory.seq
        || m.entities.len() != directory.entities.len()
        || m.entities.len() != recipes.len()
        || !m
            .entities
            .iter()
            .zip(&directory.entities)
            .zip(recipes)
            .all(|(((r, v), e), d)| {
                *r == e.reference
                    && *r == d.reference
                    && *v == d.revision
                    && d.directory_seq == directory.seq
                    && d.source_frame_seq <= m.token.frame_seq
            })
        || m.token.bytes as usize > frame_bound(recipes)?
    {
        return Err("scene manifest recipe generation");
    }
    Ok(())
}
#[derive(Default)]
pub struct Assembly {
    manifest: Option<Manifest>,
    body: Vec<u8>,
    have: Vec<bool>,
    got: usize,
    complete: Option<Token>,
    retired: std::collections::VecDeque<Token>,
}
impl Assembly {
    pub fn retire(&mut self) {
        for token in [self.manifest.take().map(|m| m.token), self.complete.take()]
            .into_iter()
            .flatten()
        {
            if !self.retired.contains(&token) {
                self.retired.push_back(token);
            }
        }
        while self.retired.len() > MAX_ENTITIES {
            self.retired.pop_front();
        }
        self.body.clear();
        self.have.clear();
        self.got = 0;
    }
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }
    pub fn offer(&mut self, m: Manifest) -> Result<Option<Ack>, &'static str> {
        if self.retired.contains(&m.token) {
            return Ok(None);
        }
        if let Some(t) = &self.complete {
            if *t == m.token {
                return Ok(Some(Ack {
                    token: t.clone(),
                    stage: ACK_COMPLETE,
                }));
            }
            if t.epoch == m.token.epoch
                && t.directory_seq == m.token.directory_seq
                && t.frame_seq > m.token.frame_seq
            {
                return Ok(None);
            }
        }
        if let Some(old) = &self.manifest {
            if *old != m {
                if old.token.epoch == m.token.epoch
                    && (m.token.directory_seq > old.token.directory_seq
                        || (m.token.directory_seq == old.token.directory_seq
                            && m.token.frame_seq > old.token.frame_seq
                            && m.entities != old.entities))
                {
                    self.retire();
                } else {
                    return Err("scene batch replaced before complete");
                }
            }
        }
        if self.manifest.is_none() {
            self.manifest = Some(m);
        }
        Ok(None)
    }
    /// Called only once the entire recipe vector is accepted; no data is sent before this ACK.
    pub fn admit(
        &mut self,
        d: &Directory,
        recipes: &[Arc<Descriptor>],
    ) -> Result<Option<Ack>, &'static str> {
        let Some(m) = &self.manifest else {
            return Ok(None);
        };
        manifest_recipes(m, d, recipes)?;
        if !self.have.is_empty() {
            return Ok(None);
        }
        self.body = vec![0; m.token.bytes as usize];
        self.have = vec![false; m.token.parts()];
        self.got = 0;
        Ok(Some(Ack {
            token: m.token.clone(),
            stage: ACK_ADMITTED,
        }))
    }
    pub fn part(&mut self, b: &[u8]) -> Result<Option<(Manifest, Vec<u8>)>, &'static str> {
        if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
            return Err("scene part bound");
        }
        let mut r = Reader::new(b);
        let token = read_token(&mut r)?;
        if self.retired.contains(&token) {
            return Ok(None);
        }
        let index = r.u32().or_else(fail)? as usize;
        if self.complete.as_ref().is_some_and(|t| {
            t.epoch == token.epoch
                && t.directory_seq == token.directory_seq
                && t.frame_seq >= token.frame_seq
        }) {
            return Ok(None);
        }
        let Some(m) = &self.manifest else {
            return Err("scene part without manifest");
        };
        if token.epoch == m.token.epoch
            && (token.directory_seq < m.token.directory_seq
                || (token.directory_seq == m.token.directory_seq
                    && token.frame_seq < m.token.frame_seq))
        {
            return Ok(None);
        }
        if token != m.token || self.have.is_empty() || index >= self.have.len() {
            return Err("scene part generation");
        }
        let start = index * PART_BYTES;
        let end = (start + PART_BYTES).min(self.body.len());
        let data = r.bytes(end - start).or_else(fail)?;
        if !r.is_empty() {
            return Err("scene part length");
        }
        if self.have[index] {
            if self.body[start..end] != *data {
                return Err("scene conflicting duplicate");
            };
            return Ok(None);
        }
        self.body[start..end].copy_from_slice(data);
        self.have[index] = true;
        self.got += 1;
        if self.got != self.have.len() {
            return Ok(None);
        }
        if <[u8; 32]>::from(Sha256::digest(&self.body)) != m.token.digest {
            return Err("scene digest mismatch");
        }
        Ok(Some((m.clone(), std::mem::take(&mut self.body))))
    }
    pub fn finish(&mut self, token: Token) {
        self.complete = Some(token);
        self.manifest = None;
        self.have.clear();
        self.got = 0;
    }
}
pub fn part_token(b: &[u8]) -> Result<Token, &'static str> {
    read_token(&mut Reader::new(b))
}
/// Queue acceptance, admission and completion are separate transitions. Backpressure never advances the cursor.
pub struct Sender {
    pub batch: Arc<Batch>,
    cursor: usize,
    manifest_queued: bool,
    admitted: bool,
}
impl Sender {
    pub fn new(batch: Arc<Batch>) -> Self {
        Self {
            batch,
            cursor: 0,
            manifest_queued: false,
            admitted: false,
        }
    }
    pub fn next(&self) -> Result<Option<(u16, u32, Vec<u8>)>, &'static str> {
        if !self.manifest_queued {
            return Ok(Some((
                K_SCENE_MANIFEST,
                0,
                encode_manifest(&self.batch.manifest)?,
            )));
        }
        if self.admitted && self.cursor < self.batch.manifest.token.parts() {
            return Ok(Some((
                K_SCENE_PART,
                self.cursor as u32,
                self.batch.part(self.cursor)?,
            )));
        }
        Ok(None)
    }
    pub fn queued(&mut self, kind: u16, index: u32) {
        if kind == K_SCENE_MANIFEST {
            self.manifest_queued = true
        } else if kind == K_SCENE_PART && self.cursor == index as usize {
            self.cursor += 1
        }
    }
    pub fn ack(&mut self, a: &Ack) -> Result<bool, &'static str> {
        if a.token != self.batch.manifest.token || !self.manifest_queued {
            return Err("scene ack generation");
        }
        if a.stage == ACK_ADMITTED {
            self.admitted = true;
            Ok(false)
        } else if a.stage == ACK_COMPLETE
            && self.admitted
            && self.cursor == self.batch.manifest.token.parts()
        {
            Ok(true)
        } else {
            Err("scene ack before parts queued")
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture() -> (Directory, Vec<Arc<Descriptor>>, RenderWorld) {
        let reference = EntityRef {
            epoch: 19,
            id: 1,
            incarnation: 2,
        };
        let directory = Directory {
            epoch: 19,
            seq: 3,
            state: READY,
            arena: "Map_Arena_Yard".into(),
            error: String::new(),
            entities: vec![Entity {
                reference,
                owner_peer: 9001,
                slot: 0,
                kind: HUMAN,
                controller: 0,
                team: None,
            }],
        };
        let mut recipe = crate::native_descriptor::fixture_recipe();
        let base = recipe.components[0].clone();
        recipe.components.clear();
        for id in 1..=2 {
            let mut c = base.clone();
            c.id = id;
            c.name = format!("Mesh{id}");
            c.bones = (0..512)
                .map(|i| crate::native_descriptor::Bone {
                    name: format!("Bone{i}"),
                    parent: if i == 0 { -1 } else { 0 },
                })
                .collect();
            recipe.components.push(c);
        }
        recipe.validate().unwrap();
        let descriptor = Arc::new(Descriptor {
            reference,
            slot: 0,
            directory_seq: 3,
            revision: 4,
            source_frame_seq: 1,
            recipe,
        });
        let transform = [-0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let frame = RenderWorld {
            world: crate::native_service::tests::fixture_world(&directory, 1, false),
            entities: vec![RenderEntity {
                reference,
                revision: 4,
                components: descriptor
                    .recipe
                    .components
                    .iter()
                    .map(|c| RenderComponent {
                        id: c.id,
                        transform,
                        bones: vec![transform; c.bones.len()],
                        morphs: c.morphs.iter().map(|m| m.value).collect(),
                        materials: c
                            .materials
                            .iter()
                            .map(|m| RenderMaterial {
                                scalars: m.scalars.iter().map(|p| p.value).collect(),
                                vectors: m.vectors.iter().map(|p| p.value).collect(),
                                textures: m.textures.iter().map(|p| p.value.clone()).collect(),
                            })
                            .collect(),
                        spline: None,
                        spring_arm: None,
                    })
                    .collect(),
            }],
        };
        (directory, vec![descriptor], frame)
    }
    #[test]
    fn native_scene_stream_is_lossless_over_message_bound_and_atomic_under_reorder() {
        let (d, recipes, frame) = fixture();
        assert_eq!(encode_render_world_v3(&frame), Err("render world bound"));
        let batch = Batch::new(&frame, &recipes).unwrap();
        assert!(batch.body.len() > hsmp_net::net::frag::MAX_MESSAGE);
        assert!(batch.body.len() <= frame_bound(&recipes).unwrap());
        let mut a = Assembly::default();
        a.offer(decode_manifest(&encode_manifest(&batch.manifest).unwrap()).unwrap())
            .unwrap();
        assert!(
            a.part(&batch.part(0).unwrap()).is_err(),
            "no part before accepted recipe admission"
        );
        assert_eq!(a.admit(&d, &recipes).unwrap().unwrap().stage, ACK_ADMITTED);
        let last = batch.manifest.token.parts() - 1;
        let b = batch.part(last).unwrap();
        assert!(b.len() + hsmp_ipc::wire::HDR <= hsmp_net::net::frag::MAX_MESSAGE);
        assert!(a.part(&b).unwrap().is_none());
        assert!(a.part(&b).unwrap().is_none());
        let mut conflict = b.clone();
        *conflict.last_mut().unwrap() ^= 1;
        assert!(a.part(&conflict).is_err());
        let mut completed = None;
        for i in (0..last).rev() {
            completed = a.part(&batch.part(i).unwrap()).unwrap();
        }
        let (manifest, body) = completed.unwrap();
        let decoded = decode_scene(&body, &recipes).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(
            decoded.entities[0].components[0].bones[0][0].to_bits(),
            (-0.0f64).to_bits()
        );
        a.finish(manifest.token.clone());
        assert_eq!(a.offer(manifest).unwrap().unwrap().stage, ACK_COMPLETE);
    }
    #[test]
    fn native_scene_stream_preserves_eight_entities_without_reducing_bone_data() {
        let (mut directory, original_recipes, original_frame) = fixture();
        let original_entity = directory.entities[0].clone();
        directory.entities.clear();
        let mut recipes = Vec::new();
        let mut frame = original_frame.clone();
        frame.entities.clear();
        for slot in 0..8u16 {
            let mut entity = original_entity.clone();
            entity.reference.id = u32::from(slot) + 1;
            entity.owner_peer = 9001 + u32::from(slot);
            entity.slot = slot;
            entity.controller = slot as u8;
            let mut descriptor = (*original_recipes[0]).clone();
            descriptor.reference = entity.reference;
            descriptor.slot = slot;
            let mut rendered = original_frame.entities[0].clone();
            rendered.reference = entity.reference;
            // Different native scalar bits per player prevent a duplicated
            // first-player payload from satisfying the full-scene comparison.
            rendered.components[0].bones[17][0] = f64::from(slot) + 0.125;
            directory.entities.push(entity);
            recipes.push(Arc::new(descriptor));
            frame.entities.push(rendered);
        }
        frame.world = crate::native_service::tests::fixture_world(&directory, 1, false);
        let expected = encode_scene(&frame, &recipes).unwrap();
        let batch = Batch::new(&frame, &recipes).unwrap();
        assert!(expected.len() > 8 * 64 * 1024);
        let mut assembly = Assembly::default();
        assembly.offer(batch.manifest.clone()).unwrap();
        assembly.admit(&directory, &recipes).unwrap();
        let mut complete = None;
        for index in (0..batch.manifest.token.parts()).rev() {
            let part = batch.part(index).unwrap();
            assert!(part.len() + hsmp_ipc::wire::HDR <= hsmp_net::net::frag::MAX_MESSAGE);
            complete = assembly.part(&part).unwrap();
        }
        let (_, body) = complete.unwrap();
        assert_eq!(
            body, expected,
            "all eight players retain the exact encoded bits"
        );
        let decoded = decode_scene(&body, &recipes).unwrap();
        assert_eq!(decoded, frame);
        assert!(decoded.entities.iter().all(|e| {
            e.components.len() == 2 && e.components.iter().all(|c| c.bones.len() == 512)
        }));
        eprintln!(
            "offline_eight_entity_scene_bytes={} parts={}",
            body.len(),
            batch.manifest.token.parts()
        );
    }
    #[test]
    fn native_scene_stream_rejects_recipe_bound_revision_part_length_and_digest() {
        let (d, recipes, frame) = fixture();
        let batch = Batch::new(&frame, &recipes).unwrap();
        let mut oversized = batch.manifest.clone();
        oversized.token.bytes = (frame_bound(&recipes).unwrap() + 1) as u32;
        let mut a = Assembly::default();
        a.offer(oversized).unwrap();
        assert!(a.admit(&d, &recipes).is_err());
        assert!(a.body.is_empty());
        let mut wrong = batch.manifest.clone();
        wrong.entities[0].1 += 1;
        assert!(manifest_recipes(&wrong, &d, &recipes).is_err());
        let mut a = Assembly::default();
        a.offer(batch.manifest.clone()).unwrap();
        a.admit(&d, &recipes).unwrap();
        let mut short = batch.part(0).unwrap();
        short.pop();
        assert!(a.part(&short).is_err());
        let mut wrong = batch.part(0).unwrap();
        *wrong.last_mut().unwrap() ^= 1;
        assert!(a.part(&wrong).unwrap().is_none());
        assert!(a.part(&batch.part(1).unwrap()).is_err());
        let mut changed = frame.clone();
        changed.entities[0].components[0].bones.pop();
        assert!(Batch::new(&changed, &recipes).is_err());
    }
    #[test]
    fn native_scene_stream_sender_keeps_batch_cursor_until_admitted_queued_and_complete() {
        let (_, recipes, frame) = fixture();
        let batch = Arc::new(Batch::new(&frame, &recipes).unwrap());
        let mut s = Sender::new(batch.clone());
        let first = s.next().unwrap().unwrap();
        assert_eq!(
            s.next().unwrap().unwrap(),
            first,
            "backpressure retry does not advance"
        );
        s.queued(first.0, first.1);
        assert!(s.next().unwrap().is_none());
        let mut ack = Ack {
            token: batch.manifest.token.clone(),
            stage: ACK_COMPLETE,
        };
        assert!(s.ack(&ack).is_err());
        ack.stage = ACK_ADMITTED;
        assert!(!s.ack(&ack).unwrap());
        while let Some((kind, index, _)) = s.next().unwrap() {
            s.queued(kind, index)
        }
        ack.stage = ACK_COMPLETE;
        assert!(s.ack(&ack).unwrap());
        ack.token.frame_seq += 1;
        assert!(s.ack(&ack).is_err());
    }
}

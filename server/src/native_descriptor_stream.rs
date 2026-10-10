//! Complete bounded recipes, carried as immutable reliable byte parts.
use super::*;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
const PART_BYTES: usize = 60 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Header {
    reference: EntityRef,
    slot: u16,
    directory_seq: u32,
    revision: u32,
    source_frame_seq: u32,
    bytes: u32,
    digest: [u8; 32],
}
impl Header {
    fn valid(&self) -> bool {
        self.reference.valid()
            && (self.slot as usize) < MAX_ENTITIES
            && self.directory_seq != 0
            && self.revision != 0
            && self.source_frame_seq != 0
            && self.bytes > 30
            && self.bytes as usize <= crate::native_descriptor::MAX_RECIPE_BYTES + 30
    }
    fn parts(&self) -> usize {
        (self.bytes as usize).div_ceil(PART_BYTES)
    }
}
fn put_header(b: &mut Vec<u8>, h: &Header) {
    put_ref(b, h.reference);
    b.put_u16(h.slot);
    b.put_u32(h.directory_seq);
    b.put_u32(h.revision);
    b.put_u32(h.source_frame_seq);
    b.put_u32(h.bytes);
    b.put(&h.digest);
}
fn read_header(r: &mut Reader<'_>) -> Result<Header, &'static str> {
    let h = Header {
        reference: get_ref(r)?,
        slot: r.u16().or_else(fail)?,
        directory_seq: r.u32().or_else(fail)?,
        revision: r.u32().or_else(fail)?,
        source_frame_seq: r.u32().or_else(fail)?,
        bytes: r.u32().or_else(fail)?,
        digest: r
            .bytes(32)
            .or_else(fail)?
            .try_into()
            .map_err(|_| "descriptor digest")?,
    };
    if !h.valid() {
        return Err("descriptor stream header");
    };
    Ok(h)
}
#[derive(Clone)]
pub struct Batch {
    header: Header,
    body: Arc<[u8]>,
    gameplay: bool,
}
impl Batch {
    pub fn new(d: &Descriptor) -> Result<Self, &'static str> {
        let body = encode_descriptor(d)?;
        let header = Header {
            reference: d.reference,
            slot: d.slot,
            directory_seq: d.directory_seq,
            revision: d.revision,
            source_frame_seq: d.source_frame_seq,
            bytes: u32::try_from(body.len()).map_err(|_| "descriptor bytes")?,
            digest: Sha256::digest(&body).into(),
        };
        if !header.valid() {
            return Err("descriptor batch bound");
        };
        Ok(Self {
            header,
            body: body.into(),
            gameplay: false,
        })
    }
    pub fn gameplay(d: &crate::native_gameplay_wire::Bootstrap) -> Result<Self, &'static str> {
        let body = crate::native_gameplay_wire::encode_bootstrap(d)?;
        let header = Header {
            reference: d.reference,
            slot: d.slot,
            directory_seq: d.directory_seq,
            revision: d.revision,
            source_frame_seq: d.source_frame_seq,
            bytes: u32::try_from(body.len()).map_err(|_| "gameplay bootstrap bytes")?,
            digest: Sha256::digest(&body).into(),
        };
        if !header.valid() {
            return Err("gameplay bootstrap batch bound");
        }
        Ok(Self {
            header,
            body: body.into(),
            gameplay: true,
        })
    }
    pub fn kind(&self) -> u16 {
        if self.gameplay {
            K_GAMEPLAY_BOOTSTRAP_PART
        } else {
            K_DESCRIPTOR_PART
        }
    }
    pub fn reference(&self) -> EntityRef {
        self.header.reference
    }
    pub fn revision(&self) -> u32 {
        self.header.revision
    }
    pub fn directory_seq(&self) -> u32 {
        self.header.directory_seq
    }
    fn part(&self, index: usize) -> Result<Vec<u8>, &'static str> {
        if index >= self.header.parts() {
            return Err("descriptor part index");
        }
        let mut b = Vec::new();
        put_header(&mut b, &self.header);
        b.put_u32(index as u32);
        let start = index * PART_BYTES;
        b.put(&self.body[start..(start + PART_BYTES).min(self.body.len())]);
        if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
            return Err("descriptor part message bound");
        };
        Ok(b)
    }
}
pub struct Sender {
    pub batch: Arc<Batch>,
    cursor: usize,
}
impl Sender {
    pub fn new(batch: Arc<Batch>) -> Self {
        Self { batch, cursor: 0 }
    }
    pub fn next(&self) -> Result<Option<Vec<u8>>, &'static str> {
        if self.cursor == self.batch.header.parts() {
            Ok(None)
        } else {
            self.batch.part(self.cursor).map(Some)
        }
    }
    pub fn queued(&mut self) {
        self.cursor += 1
    }
    pub fn started(&self) -> bool {
        self.cursor != 0
    }
    pub fn ordinal(&self) -> u32 {
        self.cursor as u32
    }
    pub fn token(&self) -> scene_stream::Token {
        scene_stream::Token {
            epoch: self.batch.header.reference.epoch,
            directory_seq: self.batch.header.directory_seq,
            frame_seq: self.batch.header.source_frame_seq,
            bytes: self.batch.header.bytes,
            digest: self.batch.header.digest,
        }
    }
}
struct Pending {
    header: Header,
    body: Vec<u8>,
    have: Vec<bool>,
    got: usize,
    ready: Option<Descriptor>,
    gameplay_ready: Option<crate::native_gameplay_wire::Bootstrap>,
}
#[derive(Default)]
pub struct Assembly {
    rows: HashMap<(u64, u16), Pending>,
    current: Option<(u64, u32)>,
    retired_epochs: std::collections::HashSet<u64>,
    gameplay: bool,
}
impl Assembly {
    pub fn gameplay() -> Self {
        Self {
            gameplay: true,
            ..Self::default()
        }
    }
    pub fn part(&mut self, b: &[u8]) -> Result<(), &'static str> {
        if b.len() + hsmp_ipc::wire::HDR > hsmp_net::net::frag::MAX_MESSAGE {
            return Err("descriptor part bound");
        }
        let mut r = Reader::new(b);
        let h = read_header(&mut r)?;
        let index = r.u32().or_else(fail)? as usize;
        if index >= h.parts() {
            return Err("descriptor part index");
        }
        if self.retired_epochs.contains(&h.reference.epoch)
            || self
                .current
                .is_some_and(|(e, s)| e == h.reference.epoch && s > h.directory_seq)
        {
            return Ok(());
        }
        let key = (h.reference.epoch, h.slot);
        if !self.rows.keys().any(|(e, _)| *e == h.reference.epoch)
            && self
                .rows
                .keys()
                .map(|(e, _)| *e)
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 2
        {
            return Err("descriptor future epoch budget");
        }
        if let Some(old) = self.rows.get(&key) {
            if old.header.reference.epoch == h.reference.epoch
                && (old.header.directory_seq > h.directory_seq
                    || (old.header.directory_seq == h.directory_seq
                        && old.header.revision > h.revision))
            {
                return Ok(());
            }
            if old.header.directory_seq == h.directory_seq
                && old.header.revision == h.revision
                && old.header != h
            {
                return Err("descriptor conflicting header");
            }
        }
        let start = index * PART_BYTES;
        let end = (start + PART_BYTES).min(h.bytes as usize);
        let data = r.bytes(end - start).or_else(fail)?;
        if !r.is_empty() {
            return Err("descriptor part length");
        }
        if !self.rows.get(&key).is_some_and(|old| old.header == h) {
            self.rows.insert(
                key,
                Pending {
                    body: vec![0; h.bytes as usize],
                    have: vec![false; h.parts()],
                    header: h.clone(),
                    got: 0,
                    ready: None,
                    gameplay_ready: None,
                },
            );
        }
        let p = self.rows.get_mut(&key).ok_or("descriptor assembly")?;
        if p.have[index] {
            if p.body[start..end] != *data {
                return Err("descriptor conflicting part");
            };
            return Ok(());
        }
        p.body[start..end].copy_from_slice(data);
        p.have[index] = true;
        p.got += 1;
        if p.got == p.have.len() {
            if <[u8; 32]>::from(Sha256::digest(&p.body)) != h.digest {
                return Err("descriptor digest mismatch");
            }
            if self.gameplay {
                let d = crate::native_gameplay_wire::decode_bootstrap(&p.body)?;
                if d.reference != h.reference
                    || d.slot != h.slot
                    || d.directory_seq != h.directory_seq
                    || d.revision != h.revision
                    || d.source_frame_seq != h.source_frame_seq
                {
                    return Err("gameplay bootstrap body header mismatch");
                }
                p.gameplay_ready = Some(d);
            } else {
                let d = decode_descriptor(&p.body)?;
                if d.reference != h.reference
                    || d.slot != h.slot
                    || d.directory_seq != h.directory_seq
                    || d.revision != h.revision
                    || d.source_frame_seq != h.source_frame_seq
                {
                    return Err("descriptor body header mismatch");
                }
                p.ready = Some(d);
            }
        }
        Ok(())
    }
    /// Complete parts may precede their ordered directory. No partially decoded recipe is exposed.
    pub fn ready(&mut self, d: &Directory) -> Result<Vec<Descriptor>, &'static str> {
        if self.gameplay {
            return Err("gameplay bootstrap assembly requires typed admission");
        }
        self.admit_directory(d)?;
        Ok(self
            .rows
            .values_mut()
            .filter_map(|p| {
                if Self::matches(&p.header, d) {
                    p.ready.take()
                } else {
                    None
                }
            })
            .collect())
    }
    pub fn ready_gameplay(
        &mut self,
        d: &Directory,
    ) -> Result<Vec<crate::native_gameplay_wire::Bootstrap>, &'static str> {
        if !self.gameplay {
            return Err("full recipe assembly requires typed admission");
        }
        self.admit_directory(d)?;
        Ok(self
            .rows
            .values_mut()
            .filter_map(|p| {
                if Self::matches(&p.header, d) {
                    p.gameplay_ready.take()
                } else {
                    None
                }
            })
            .collect())
    }
    fn matches(h: &Header, d: &Directory) -> bool {
        h.directory_seq == d.seq
            && h.reference.epoch == d.epoch
            && d.entities
                .iter()
                .any(|e| e.slot == h.slot && e.reference == h.reference)
    }
    fn admit_directory(&mut self, d: &Directory) -> Result<(), &'static str> {
        if self.retired_epochs.contains(&d.epoch) {
            return Err("retired descriptor epoch directory");
        }
        if let Some((old, _)) = self.current {
            if old != d.epoch {
                if self.retired_epochs.len() >= MAX_ENTITIES {
                    return Err("descriptor epoch history bound");
                }
                self.retired_epochs.insert(old);
            }
        }
        self.current = Some((d.epoch, d.seq));
        self.rows.retain(|_, p| {
            !self.retired_epochs.contains(&p.header.reference.epoch)
                && (p.header.reference.epoch != d.epoch || p.header.directory_seq >= d.seq)
        });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gameplay_bootstrap_parts_are_typed_and_admit_only_complete_original_directory() {
        let (directory, recipes, _) = scene_stream::tests::fixture();
        let original = &recipes[0];
        let bootstrap = crate::native_gameplay_wire::Bootstrap {
            reference: original.reference,
            slot: original.slot,
            directory_seq: original.directory_seq,
            revision: original.revision,
            source_frame_seq: original.source_frame_seq,
            recipe: crate::native_gameplay_wire::fixture_recipe(),
        };
        let batch = Batch::gameplay(&bootstrap).unwrap();
        assert_eq!(batch.kind(), K_GAMEPLAY_BOOTSTRAP_PART);
        let part = batch.part(0).unwrap();
        let mut assembly = Assembly::gameplay();
        assembly.part(&part).unwrap();
        let mut wrong = directory.clone();
        wrong.entities.clear();
        assert!(assembly.ready_gameplay(&wrong).unwrap().is_empty());
        assert_eq!(
            assembly.ready_gameplay(&directory).unwrap(),
            vec![bootstrap]
        );
        assert!(Assembly::default().part(&part).is_err());
        assert!(Assembly::gameplay()
            .part(&Batch::new(original).unwrap().part(0).unwrap())
            .is_err());
    }
    #[test]
    fn native_descriptor_parts_preserve_large_recipe_and_nulls_before_exposure() {
        let (directory, recipes, _) = scene_stream::tests::fixture();
        let mut d = (*recipes[0]).clone();
        for (ci, c) in d.recipe.components.iter_mut().enumerate() {
            let original = crate::native_descriptor::Material {
                slot: 0,
                base: "/Game/Materials/M_Test.M_Test".into(),
                scalars: Vec::new(),
                vectors: Vec::new(),
                textures: Vec::new(),
            };
            c.materials = (0..32)
                .map(|slot| {
                    let mut m = original.clone();
                    m.slot = slot;
                    m.scalars.clear();
                    m.vectors.clear();
                    m.textures = (0..8)
                        .map(|index| crate::native_descriptor::TextureParameter {
                            info: crate::native_descriptor::ParameterInfo {
                                name: format!("Texture{index}"),
                                association: 0,
                                index: -1,
                            },
                            value: format!("/Game/T_{ci}_{slot}_{index}.{}", "X".repeat(470)),
                        })
                        .collect();
                    m
                })
                .collect();
        }
        d.recipe.validate().unwrap();
        let batch = Batch::new(&d).unwrap();
        assert!(batch.body.len() > hsmp_net::net::frag::MAX_MESSAGE);
        assert!(batch.body.len() <= crate::native_descriptor::MAX_RECIPE_BYTES + 30);
        let mut a = Assembly::default();
        let last = batch.header.parts() - 1;
        let part = batch.part(last).unwrap();
        a.part(&part).unwrap();
        a.part(&part).unwrap();
        assert!(a.ready(&directory).unwrap().is_empty());
        let mut conflict = part.clone();
        *conflict.last_mut().unwrap() ^= 1;
        assert!(a.part(&conflict).is_err());
        for i in (0..last).rev() {
            let part = batch.part(i).unwrap();
            assert!(part.len() + hsmp_ipc::wire::HDR <= hsmp_net::net::frag::MAX_MESSAGE);
            a.part(&part).unwrap()
        }
        assert_eq!(a.ready(&directory).unwrap(), vec![d]);
        assert!(a.ready(&directory).unwrap().is_empty());
        let mut sender = Sender::new(Arc::new(batch));
        let first = sender.next().unwrap();
        assert_eq!(first, sender.next().unwrap());
        sender.queued();
        assert_ne!(first, sender.next().unwrap());
    }
    #[test]
    fn native_descriptor_parts_reject_unknown_header_truncation_hash_and_reuse() {
        let (_, recipes, _) = scene_stream::tests::fixture();
        let batch = Batch::new(&recipes[0]).unwrap();
        let mut a = Assembly::default();
        let mut truncated = batch.part(0).unwrap();
        truncated.pop();
        assert!(a.part(&truncated).is_err());
        let mut oversized = batch.part(0).unwrap();
        oversized[30..34].copy_from_slice(
            &((crate::native_descriptor::MAX_RECIPE_BYTES + 31) as u32).to_le_bytes(),
        );
        assert!(a.part(&oversized).is_err());
        let mut bad = batch.part(0).unwrap();
        *bad.last_mut().unwrap() ^= 1;
        assert!(a.part(&bad).is_err());
        let mut a = Assembly::default();
        a.part(&batch.part(0).unwrap()).unwrap();
        let mut reused = batch.part(0).unwrap();
        reused[12..16].copy_from_slice(&999u32.to_le_bytes());
        assert!(a.part(&reused).is_err());
    }
    #[test]
    fn native_descriptor_future_epoch_is_separate_from_old_large_trailing_parts() {
        let (old_directory, recipes, _) = scene_stream::tests::fixture();
        let mut old = Batch::new(&recipes[0]).unwrap();
        // A maximal old incomplete transfer is never decoded or exposed.
        old.body = vec![0; PART_BYTES * 8 + 100].into();
        old.header.bytes = old.body.len() as u32;
        old.header.digest = Sha256::digest(&old.body).into();
        let mut new = (*recipes[0]).clone();
        new.reference.epoch = 22;
        let new_batch = Batch::new(&new).unwrap();
        let mut a = Assembly::default();
        a.ready(&old_directory).unwrap();
        a.part(&old.part(0).unwrap()).unwrap();
        for i in 0..new_batch.header.parts() {
            a.part(&new_batch.part(i).unwrap()).unwrap()
        }
        a.part(&old.part(8).unwrap()).unwrap();
        assert!(a.ready(&old_directory).unwrap().is_empty());
        let mut current = old_directory.clone();
        current.epoch = 22;
        current.entities[0].reference.epoch = 22;
        assert_eq!(a.ready(&current).unwrap(), vec![new]);
        a.part(&old.part(6).unwrap()).unwrap();
        assert_eq!(a.rows.len(), 1);
        assert!(a.rows.contains_key(&(22, 0)));
        assert!(
            a.ready(&old_directory).is_err(),
            "retired epoch cannot re-expose old metadata"
        );
    }
}

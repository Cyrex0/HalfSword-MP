//! Embedded network services. Bounded immutable DTOs cross the game/network thread boundary.
use crate::native_gameplay_wire as gp;
use crate::native_wire::{self as w, Directory, InputFrame, World};
use anyhow::{Context, Result};
use hsmp_net::net::{Client, ClientConfig, ClientEvent, ConnConfig, SendMode};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
fn decode_compressed_authority(
    negotiated: bool,
    hint: u32,
    payload: &[u8],
) -> Result<(u16, Vec<u8>), &'static str> {
    if !negotiated {
        return Err("unnegotiated native compression");
    }
    let limit = match hint {
        v if v == w::K_GAMEPLAY_RESULT as u32 => gp::MAX_RESULT_BYTES,
        v if v == w::K_DESCRIPTOR_PART as u32 || v == w::K_GAMEPLAY_BOOTSTRAP_PART as u32 => {
            64 * 1024
        }
        _ => return Err("native compressed authority kind refused"),
    };
    let (kind, body) = hsmp_net::net::compression::unpack(payload, limit)?;
    if kind as u32 != hint {
        return Err("native compressed authority kind mismatch");
    }
    Ok((kind, body))
}

#[derive(Default)]
struct Shared {
    directory: Option<Directory>,
    world: Option<Arc<World>>,
    published: Option<World>,
    inputs: VecDeque<(InputFrame, Instant)>,
    status: Option<(u8, String)>,
    peer_id: u32,
    server_clock: Option<(u64, Instant)>,
    connected: bool,
    presentation: bool,
    gameplay_client: bool,
    gameplay: Option<Arc<gp::ResultFrame>>,
    gameplay_out: Option<gp::ResultFrame>,
    gameplay_received: Option<Instant>,
    gameplay_receipts: HashMap<u32, w::MirrorReady>,
    gameplay_control_out: VecDeque<w::MirrorReady>,
    gameplay_applied: Option<(w::MirrorReady, Instant)>,
    gameplay_inputs: VecDeque<(gp::Request, Instant)>,
    native_compression: bool,
    codec: CompressionMetrics,
    error: String,
    world_reset: bool,
    descriptors: HashMap<u32, Arc<w::Descriptor>>,
    descriptor_out: VecDeque<w::Descriptor>,
    gameplay_descriptors: HashMap<u32, Arc<gp::Bootstrap>>,
    gameplay_descriptor_out: VecDeque<gp::Bootstrap>,
    gameplay_descriptor_stream: Option<w::descriptor_stream::Assembly>,
    render: Option<Arc<w::RenderWorld>>,
    render_out: Option<w::RenderWorld>,
    control_out: VecDeque<w::MirrorReady>,
    mirror_receipts: HashMap<u32, w::MirrorReady>,
    render_received: Option<Instant>,
    source_roster: Option<SourceRosterFacts>,
    stream: w::scene_stream::Assembly,
    stream_ack: Option<w::scene_stream::Ack>,
    applied: Option<(w::MirrorReady, Instant)>,
    descriptor_stream: w::descriptor_stream::Assembly,
}
#[derive(Clone, Copy, Default, Debug)]
pub struct CompressionMetrics {
    pub encode_samples: u32,
    pub raw_bytes: u64,
    pub wire_bytes: u64,
    pub encode_us: u64,
    pub decode_samples: u32,
    pub decode_raw_bytes: u64,
    pub decode_wire_bytes: u64,
    pub decode_us: u64,
}
/// A complete, game-thread observed roster. Native pointers never cross into
/// the network thread; its original directory identity is checked again there.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRosterFacts {
    pub epoch: u64,
    pub directory_seq: u32,
    pub entities: Vec<w::Entity>,
}
impl SourceRosterFacts {
    pub fn matches(&self, directory: &Directory) -> bool {
        self.epoch == directory.epoch
            && self.directory_seq == directory.seq
            && self.entities.len() == directory.entities.len()
            && !self.entities.is_empty()
            && self.entities.len() <= w::MAX_ENTITIES
            && self.entities.iter().enumerate().all(|(i, fact)| {
                fact.team.is_some()
                    && !self.entities[..i]
                        .iter()
                        .any(|old| old.reference.id == fact.reference.id)
                    && directory.entities.iter().any(|original| {
                        fact.reference == original.reference
                            && fact.owner_peer == original.owner_peer
                            && fact.slot == original.slot
                            && fact.kind == original.kind
                            && fact.controller == original.controller
                    })
            })
    }
}
#[derive(Clone)]
pub struct Scene {
    pub directory: Directory,
    pub descriptors: Vec<Arc<w::Descriptor>>,
    pub frame: Arc<w::RenderWorld>,
    pub peer_id: u32,
    pub received: Instant,
}
impl Scene {
    pub fn fresh(&self) -> bool {
        self.received.elapsed().as_millis() < w::INPUT_TIMEOUT_MS as u128
    }
    pub fn generation(&self) -> String {
        format!(
            "{}:{}:{:?}",
            self.directory.epoch,
            self.directory.seq,
            self.descriptors
                .iter()
                .map(|d| (d.reference.id, d.reference.incarnation, d.revision))
                .collect::<Vec<_>>()
        )
    }
}
/// Complete compact result and bootstrap recipes, with the original network receipt.
#[derive(Clone)]
pub struct GameplayScene {
    pub directory: Directory,
    pub descriptors: Vec<Arc<gp::Bootstrap>>,
    pub result: Arc<gp::ResultFrame>,
    pub peer_id: u32,
    pub received: Instant,
}
impl GameplayScene {
    pub fn fresh(&self) -> bool {
        self.received.elapsed().as_millis() < w::INPUT_TIMEOUT_MS as u128
    }
    pub fn receipt(&self) -> w::MirrorReady {
        w::MirrorReady {
            epoch: self.directory.epoch,
            directory_seq: self.directory.seq,
            frame_seq: self.result.authority_tick,
            entities: self
                .descriptors
                .iter()
                .map(|d| (d.reference, d.revision))
                .collect(),
        }
    }
}
#[derive(Default)]
pub(crate) struct Bridge {
    shared: Mutex<Shared>,
}
impl Bridge {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(crate) fn set_directory(&self, d: Directory) {
        let mut s = self.lock();
        if s.directory
            .as_ref()
            .is_none_or(|old| old.epoch != d.epoch || old.seq != d.seq)
        {
            s.world = None;
            s.published = None;
            s.render = None;
            s.render_out = None;
            s.control_out.clear();
            s.mirror_receipts.clear();
            s.applied = None;
            s.gameplay = None;
            s.gameplay_out = None;
            s.gameplay_received = None;
            s.gameplay_receipts.clear();
            s.gameplay_control_out.clear();
            s.gameplay_applied = None;
            s.gameplay_inputs.clear();
            s.stream_ack = None;
            s.source_roster = None;
            if !s
                .stream
                .manifest()
                .is_some_and(|m| m.token.epoch == d.epoch && m.token.directory_seq == d.seq)
            {
                s.stream = Default::default();
            }
            s.descriptor_out.clear();
            s.gameplay_descriptor_out.clear();
            s.gameplay_descriptors.retain(|_, r| {
                r.directory_seq == d.seq
                    && d.entities
                        .iter()
                        .any(|e| e.reference == r.reference && e.slot == r.slot)
            });
            s.descriptors.retain(|_, r| {
                r.directory_seq == d.seq
                    && d.entities
                        .iter()
                        .any(|e| e.reference == r.reference && e.slot == r.slot)
            });
            s.inputs
                .retain(|(i, _)| d.entities.iter().any(|e| e.reference == i.reference));
        }
        s.directory = Some(d);
    }
    pub(crate) fn push_input(&self, i: InputFrame) -> bool {
        let mut s = self.lock();
        if s.world_reset || s.inputs.len() >= w::MAX_INPUTS {
            return false;
        }
        s.inputs.push_back((i, Instant::now()));
        true
    }
    pub(crate) fn clear_inputs(&self) {
        self.lock().inputs.clear();
    }
    pub(crate) fn take_inputs(&self, max: usize) -> Vec<InputFrame> {
        let mut s = self.lock();
        let mut out = Vec::with_capacity(max.min(32));
        while out.len() < max.min(32) {
            let Some((i, queued)) = s.inputs.pop_front() else {
                break;
            };
            if i.flags & w::RELEASE_ALL != 0
                || queued.elapsed().as_millis() < w::INPUT_TIMEOUT_MS as u128
            {
                out.push(i);
            }
        }
        out
    }
    pub(crate) fn take_publication(&self) -> Option<World> {
        self.lock().published.take()
    }
    pub(crate) fn take_status(&self) -> Option<(u8, String)> {
        self.lock().status.take()
    }
    pub(crate) fn publish_status(&self, state: u8, error: &str) -> Result<(), &'static str> {
        if state > w::FAULT || error.len() > 96 || error.contains('\0') {
            return Err("native status");
        }
        self.lock().status = Some((state, error.to_owned()));
        Ok(())
    }
    pub(crate) fn reset_pending(&self) -> bool {
        self.lock().world_reset
    }
    pub(crate) fn request_world_reset(&self) {
        let mut s = self.lock();
        s.world_reset = true;
        s.inputs.clear();
        s.published = None;
        s.world = None;
        s.render = None;
        s.render_out = None;
        s.descriptors.clear();
        s.gameplay_descriptors.clear();
        s.gameplay_descriptor_out.clear();
        s.gameplay_descriptor_stream = None;
        s.descriptor_out.clear();
        s.mirror_receipts.clear();
        s.control_out.clear();
        s.stream = Default::default();
        s.descriptor_stream = Default::default();
        s.stream_ack = None;
        s.applied = None;
        s.gameplay = None;
        s.gameplay_out = None;
        s.gameplay_received = None;
        s.gameplay_receipts.clear();
        s.gameplay_control_out.clear();
        s.gameplay_applied = None;
        s.gameplay_inputs.clear();
        s.source_roster = None;
    }
    pub(crate) fn take_world_reset(&self) -> bool {
        std::mem::take(&mut self.lock().world_reset)
    }
    pub(crate) fn publish_world(&self, world: World) -> Result<(), &'static str> {
        w::validate_world(&world)?;
        let mut s = self.lock();
        if s.world_reset {
            return Err("native world reset pending");
        }
        let d = s.directory.as_ref().ok_or("no directory")?;
        if !w::matches_directory(&world, d) {
            return Err("stale native directory");
        }
        if s.published
            .as_ref()
            .is_some_and(|old| old.frame_seq >= world.frame_seq)
        {
            return Err("stale native frame");
        }
        s.published = Some(world);
        Ok(())
    }
    pub(crate) fn publish_gameplay(&self, frame: gp::ResultFrame) -> Result<(), &'static str> {
        frame.validate()?;
        let mut s = self.lock();
        if s.world_reset || !s.directory.as_ref().is_some_and(|d| frame.matches(d)) {
            return Err("stale gameplay directory");
        }
        if s.gameplay
            .as_ref()
            .is_some_and(|old| old.authority_tick >= frame.authority_tick)
        {
            return Err("stale gameplay authority tick");
        }
        if s.gameplay.as_ref().is_some_and(|old| {
            frame.entities.iter().any(|e| {
                old.entities.iter().any(|p| {
                    p.reference == e.reference
                        && (p.request_seq > e.request_seq || p.delivery_seq > e.delivery_seq)
                })
            })
        }) {
            return Err("gameplay execution acknowledgement regressed");
        }
        s.gameplay = Some(Arc::new(frame.clone()));
        s.gameplay_received = Some(Instant::now());
        s.gameplay_out = Some(frame);
        Ok(())
    }
    pub(crate) fn take_gameplay(&self) -> Option<gp::ResultFrame> {
        self.lock().gameplay_out.take()
    }
    pub(crate) fn compress_gameplay(&self, body: &[u8]) -> Option<Vec<u8>> {
        let start = Instant::now();
        let packed = hsmp_net::net::compression::pack(w::K_GAMEPLAY_RESULT, body);
        let elapsed = start.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let mut s = self.lock();
        if s.codec.encode_samples < 8 {
            s.codec.encode_samples += 1;
            s.codec.raw_bytes += body.len() as u64;
            s.codec.wire_bytes += packed.as_ref().map_or(body.len(), Vec::len) as u64;
            s.codec.encode_us += elapsed;
        }
        packed
    }
    pub(crate) fn accept_gameplay_ready(
        &self,
        peer: u32,
        receipt: w::MirrorReady,
    ) -> Result<(), &'static str> {
        w::encode_mirror_ready(&receipt)?;
        let mut s = self.lock();
        let d = s.directory.as_ref().ok_or("no gameplay directory")?;
        let frame = s.gameplay.as_ref().ok_or("no gameplay result")?;
        if receipt.epoch != d.epoch
            || receipt.directory_seq != d.seq
            || receipt.frame_seq > frame.authority_tick
            || receipt.entities.len() != d.entities.len()
            || !d
                .entities
                .iter()
                .any(|e| e.owner_peer == peer && e.kind == w::HUMAN)
            || receipt.entities.iter().any(|(r, v)| {
                !d.entities.iter().any(|e| e.reference == *r)
                    || !s
                        .gameplay_descriptors
                        .get(&r.id)
                        .is_some_and(|d| d.reference == *r && d.revision == *v)
            })
        {
            return Err("gameplay readiness generation");
        }
        s.gameplay_receipts.insert(peer, receipt);
        Ok(())
    }
    pub(crate) fn gameplay_ready(&self, peer: u32) -> bool {
        let s = self.lock();
        s.gameplay_receipts.get(&peer).is_some_and(|r| {
            s.directory.as_ref().is_some_and(|d| {
                r.epoch == d.epoch
                    && r.directory_seq == d.seq
                    && r.entities.len() == d.entities.len()
            }) && r.entities.iter().all(|(reference, v)| {
                s.gameplay_descriptors
                    .get(&reference.id)
                    .is_some_and(|d| d.reference == *reference && d.revision == *v)
            })
        })
    }
    pub(crate) fn publish_descriptor(&self, d: w::Descriptor) -> Result<(), &'static str> {
        w::encode_descriptor(&d)?;
        let mut s = self.lock();
        if s.world_reset
            || !s.directory.as_ref().is_some_and(|dir| {
                dir.seq == d.directory_seq
                    && dir
                        .entities
                        .iter()
                        .any(|e| e.reference == d.reference && e.slot == d.slot)
            })
        {
            return Err("stale descriptor directory");
        }
        if let Some(old) = s.descriptors.get(&d.reference.id) {
            if old.reference == d.reference {
                if d.revision < old.revision {
                    return Err("stale descriptor revision");
                }
                if d.revision == old.revision {
                    return if **old == d {
                        Ok(())
                    } else {
                        Err("descriptor revision changed content")
                    };
                }
            }
        }
        s.render = None;
        s.render_out = None;
        s.mirror_receipts.clear();
        s.applied = None;
        s.gameplay_applied = None;
        s.gameplay_receipts.clear();
        if s.stream.manifest().is_some_and(|m| {
            m.entities
                .iter()
                .any(|(r, v)| r.id == d.reference.id && (*r != d.reference || *v < d.revision))
        }) {
            s.stream.retire();
            s.stream_ack = None;
        }
        s.descriptor_out
            .retain(|old| old.reference.id != d.reference.id);
        s.descriptor_out.push_back(d.clone());
        s.descriptors.insert(d.reference.id, Arc::new(d));
        Ok(())
    }
    pub(crate) fn publish_gameplay_descriptor(&self, d: gp::Bootstrap) -> Result<(), &'static str> {
        gp::encode_bootstrap(&d)?;
        let mut s = self.lock();
        if s.world_reset
            || !s.directory.as_ref().is_some_and(|dir| {
                dir.seq == d.directory_seq
                    && dir
                        .entities
                        .iter()
                        .any(|e| e.reference == d.reference && e.slot == d.slot)
            })
        {
            return Err("stale gameplay bootstrap directory");
        }
        if let Some(old) = s.gameplay_descriptors.get(&d.reference.id) {
            if old.reference == d.reference {
                if d.revision < old.revision {
                    return Err("stale gameplay bootstrap revision");
                }
                if d.revision == old.revision {
                    return if **old == d {
                        Ok(())
                    } else {
                        Err("gameplay bootstrap revision changed content")
                    };
                }
            }
        }
        s.gameplay_applied = None;
        s.gameplay_receipts.clear();
        s.gameplay_control_out.clear();
        s.gameplay_descriptor_out
            .retain(|old| old.reference.id != d.reference.id);
        s.gameplay_descriptor_out.push_back(d.clone());
        s.gameplay_descriptors.insert(d.reference.id, Arc::new(d));
        Ok(())
    }
    pub(crate) fn replay_descriptors(&self) {
        let mut s = self.lock();
        let mut rows = s
            .descriptors
            .values()
            .map(|r| (**r).clone())
            .collect::<Vec<_>>();
        rows.sort_by_key(|r| r.slot);
        s.descriptor_out = rows.into();
        let mut rows = s
            .gameplay_descriptors
            .values()
            .map(|r| (**r).clone())
            .collect::<Vec<_>>();
        rows.sort_by_key(|r| r.slot);
        s.gameplay_descriptor_out = rows.into();
    }
    pub(crate) fn take_descriptors(&self) -> Vec<w::Descriptor> {
        self.lock().descriptor_out.drain(..).collect()
    }
    pub(crate) fn take_gameplay_descriptors(&self) -> Vec<gp::Bootstrap> {
        self.lock().gameplay_descriptor_out.drain(..).collect()
    }
    pub(crate) fn take_source_roster(&self) -> Option<SourceRosterFacts> {
        self.lock().source_roster.take()
    }
    pub(crate) fn source_roster(&self, facts: SourceRosterFacts) -> Result<u32, &'static str> {
        let mut s = self.lock();
        if s.world_reset || !s.directory.as_ref().is_some_and(|d| facts.matches(d)) {
            return Err("source roster directory generation");
        }
        if s.source_roster.as_ref().is_some_and(|old| old != &facts) {
            return Err("source roster facts already pending");
        }
        let d = s
            .directory
            .as_ref()
            .ok_or("source roster directory unavailable")?;
        let changed = facts.entities.iter().any(|f| {
            d.entities
                .iter()
                .any(|e| e.reference == f.reference && e.team != f.team)
        });
        let ack_seq = if changed {
            d.seq
                .checked_add(1)
                .ok_or("source roster directory counter exhausted")?
        } else {
            d.seq
        };
        s.source_roster = Some(facts);
        Ok(ack_seq)
    }
    pub(crate) fn descriptor(&self, id: u32) -> Option<Arc<w::Descriptor>> {
        self.lock().descriptors.get(&id).cloned()
    }
    fn render_matches(s: &Shared, frame: &w::RenderWorld) -> bool {
        s.directory
            .as_ref()
            .is_some_and(|d| w::matches_directory(&frame.world, d))
            && frame.entities.iter().all(|e| {
                s.descriptors.get(&e.reference.id).is_some_and(|d| {
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
                                        (Some(profile), Some(spline)) => {
                                            profile.metadata_null
                                                && spline.position.points.len()
                                                    == usize::from(profile.position_count)
                                                && spline.rotation.points.len()
                                                    == usize::from(profile.rotation_count)
                                                && spline.scale.points.len()
                                                    == usize::from(profile.scale_count)
                                                && spline.reparam.points.len()
                                                    == usize::from(profile.reparam_count)
                                                && spline.validate().is_ok()
                                        }
                                        (None, None) => true,
                                        _ => false,
                                    }
                                    && match (&r.scene, &c.spring_arm) {
                                        (
                                            crate::native_descriptor::SceneEvidence::SpringArm {
                                                ..
                                            },
                                            Some(arm),
                                        ) => arm.validate().is_ok(),
                                        (
                                            crate::native_descriptor::SceneEvidence::SpringArm {
                                                ..
                                            },
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
            })
    }
    pub(crate) fn publish_render(&self, frame: w::RenderWorld) -> Result<(), &'static str> {
        let mut s = self.lock();
        if s.world_reset || !Self::render_matches(&s, &frame) {
            return Err("render descriptor generation");
        }
        let recipes = Self::recipes(&s).ok_or("render recipe vector")?;
        w::scene_stream::encode_scene(&frame, &recipes)?;
        if s.render
            .as_ref()
            .is_some_and(|old| old.world.frame_seq >= frame.world.frame_seq)
        {
            return Err("stale render frame");
        }
        let frame = Arc::new(frame);
        s.render_out = Some((*frame).clone());
        s.render = Some(frame);
        s.render_received = Some(Instant::now());
        Ok(())
    }
    pub(crate) fn take_render(&self) -> Option<w::RenderWorld> {
        self.lock().render_out.take()
    }
    fn recipes(s: &Shared) -> Option<Vec<Arc<w::Descriptor>>> {
        s.directory
            .as_ref()?
            .entities
            .iter()
            .map(|e| s.descriptors.get(&e.reference.id).cloned())
            .collect()
    }
    pub(crate) fn scene_batch(
        &self,
        frame: &w::RenderWorld,
    ) -> Result<w::scene_stream::Batch, &'static str> {
        let s = self.lock();
        if !Self::render_matches(&s, frame) {
            return Err("scene batch generation");
        }
        w::scene_stream::Batch::new(frame, &Self::recipes(&s).ok_or("scene batch recipes")?)
    }
    pub(crate) fn manifest_current(&self, m: &w::scene_stream::Manifest) -> bool {
        let s = self.lock();
        s.directory
            .as_ref()
            .zip(Self::recipes(&s))
            .is_some_and(|(d, r)| w::scene_stream::manifest_recipes(m, d, &r).is_ok())
    }
    fn admit_stream(&self) -> Result<(), &'static str> {
        let mut s = self.lock();
        let Some(d) = s.directory.clone() else {
            return Ok(());
        };
        let Some(m) = s.stream.manifest() else {
            return Ok(());
        };
        if m.token.epoch != d.epoch || m.token.directory_seq != d.seq {
            return Ok(());
        }
        let Some(recipes) = Self::recipes(&s) else {
            return Ok(());
        };
        if !m
            .entities
            .iter()
            .zip(&recipes)
            .all(|((r, v), d)| *r == d.reference && *v == d.revision)
        {
            return Ok(());
        }
        if let Some(ack) = s.stream.admit(&d, &recipes)? {
            s.stream_ack = Some(ack);
        }
        Ok(())
    }
    fn finish_descriptors(&self) -> Result<(), &'static str> {
        let ready = {
            let mut s = self.lock();
            let Some(d) = s.directory.clone() else {
                return Ok(());
            };
            s.descriptor_stream.ready(&d)?
        };
        for d in ready {
            self.publish_descriptor(d)?;
        }
        let ready = {
            let mut s = self.lock();
            let directory = s.directory.clone();
            match (directory, s.gameplay_descriptor_stream.as_mut()) {
                (Some(d), Some(stream)) => stream.ready_gameplay(&d)?,
                _ => Vec::new(),
            }
        };
        for d in ready {
            self.publish_gameplay_descriptor(d)?;
        }
        Ok(())
    }
    fn receive_scene_part(&self, payload: &[u8]) -> Result<(), &'static str> {
        let token = w::scene_stream::part_token(payload)?;
        if self
            .lock()
            .directory
            .as_ref()
            .is_some_and(|d| d.epoch == token.epoch && d.seq > token.directory_seq)
        {
            return Ok(());
        }
        let completed = self.lock().stream.part(payload)?;
        let Some((m, body)) = completed else {
            return Ok(());
        };
        let recipes = Self::recipes(&self.lock()).ok_or("scene recipes vanished")?;
        let frame = w::scene_stream::decode_scene(&body, &recipes)?;
        if frame.world.epoch != m.token.epoch
            || frame.world.directory_seq != m.token.directory_seq
            || frame.world.frame_seq != m.token.frame_seq
        {
            return Err("scene body manifest generation");
        }
        self.publish_render(frame)?;
        let mut s = self.lock();
        s.stream.finish(m.token.clone());
        s.stream_ack = Some(w::scene_stream::Ack {
            token: m.token,
            stage: w::scene_stream::ACK_COMPLETE,
        });
        Ok(())
    }
    pub(crate) fn accept_mirror(
        &self,
        peer: u32,
        receipt: w::MirrorReady,
    ) -> Result<(), &'static str> {
        w::encode_mirror_ready(&receipt)?;
        let mut s = self.lock();
        let dir = s.directory.as_ref().ok_or("no directory")?;
        let frame = s.render.as_ref().ok_or("no source render frame")?;
        if receipt.epoch != dir.epoch
            || receipt.directory_seq != dir.seq
            || receipt.frame_seq > frame.world.frame_seq
            || receipt.entities.len() != dir.entities.len()
            || !dir
                .entities
                .iter()
                .any(|e| e.owner_peer == peer && e.kind == w::HUMAN)
            || !receipt.entities.iter().all(|(r, revision)| {
                s.descriptors.get(&r.id).is_some_and(|d| {
                    d.reference == *r
                        && d.revision == *revision
                        && d.source_frame_seq <= receipt.frame_seq
                })
            })
        {
            return Err("mirror readiness generation");
        }
        s.mirror_receipts.insert(peer, receipt);
        Ok(())
    }
    pub(crate) fn mirror_ready(&self, peer: u32) -> bool {
        let s = self.lock();
        s.mirror_receipts.get(&peer).is_some_and(|r| {
            s.directory
                .as_ref()
                .is_some_and(|d| r.epoch == d.epoch && r.directory_seq == d.seq)
        })
    }
    fn send_receipt(&self, send: impl FnOnce(&w::MirrorReady) -> bool) {
        let receipt = {
            let mut s = self.lock();
            while s.control_out.front().is_some_and(|r| {
                !s.directory
                    .as_ref()
                    .is_some_and(|d| d.epoch == r.epoch && d.seq == r.directory_seq)
                    || !r.entities.iter().all(|(r, v)| {
                        s.descriptors
                            .get(&r.id)
                            .is_some_and(|d| d.reference == *r && d.revision == *v)
                    })
            }) {
                s.control_out.pop_front();
            }
            s.control_out.front().cloned()
        };
        if let Some(receipt) = receipt {
            if send(&receipt) {
                let mut s = self.lock();
                if s.control_out.front() == Some(&receipt) {
                    s.control_out.pop_front();
                }
            }
        }
    }
}
struct ThreadService {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for ThreadService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub struct HostHandle {
    bridge: Arc<Bridge>,
    _service: ThreadService,
    pub address: SocketAddr,
}
impl HostHandle {
    pub fn start(bind: SocketAddr, identity_dir: &Path, arena: &str) -> Result<Self> {
        Self::start_with_parent(bind, identity_dir, arena, None)
    }
    pub fn start_with_parent(
        bind: SocketAddr,
        identity_dir: &Path,
        arena: &str,
        parent: Option<Arc<hsmp_ipc::shm::ProcessHandle>>,
    ) -> Result<Self> {
        Self::start_with_parent_mode(
            bind,
            identity_dir,
            arena,
            parent,
            crate::native_mode::Mode::Pvp,
        )
    }
    pub fn start_with_parent_mode(
        bind: SocketAddr,
        identity_dir: &Path,
        arena: &str,
        parent: Option<Arc<hsmp_ipc::shm::ProcessHandle>>,
        mode: crate::native_mode::Mode,
    ) -> Result<Self> {
        if arena.is_empty() || arena.len() > 40 || arena.contains('\0') {
            anyhow::bail!("invalid native arena");
        }
        let sock = std::net::UdpSocket::bind(bind).context("native host UDP bind")?;
        sock.set_nonblocking(true)?;
        let address = sock.local_addr()?;
        let key = crate::net::load_or_create_key(&identity_dir.join("server_identity.key"))?;
        let bridge = Arc::new(Bridge::default());
        let transport = crate::net::Net::with_caps(
            key,
            Some(crate::build_id::content_hash()),
            hsmp_net::net::caps::NATIVE_WORLD
                | hsmp_net::net::caps::NATIVE_PRESENTATION
                | hsmp_net::net::caps::NATIVE_RENDER_V2
                | hsmp_net::net::caps::NATIVE_VERTEX_STATE
                | hsmp_net::net::caps::NATIVE_RENDER_V3
                | hsmp_net::net::caps::NATIVE_EMPTY_STATIC
                | hsmp_net::net::caps::NATIVE_EMPTY_SKELETAL
                | hsmp_net::net::caps::NATIVE_SCENE_STREAM
                | gp::CAP_NATIVE_GAMEPLAY
                | gp::CAP_NATIVE_GAMEPLAY_QUATERNION
                | gp::CAP_NATIVE_GAMEPLAY_CACHE
                | gp::CAP_NATIVE_COMPRESSION,
        );
        let state = Arc::new(crate::server::ServerState::with_native_mode(
            2,
            transport,
            bridge.clone(),
            arena,
            mode,
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("hsmp-native-host".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(_) => return,
                };
                rt.block_on(async move {
                    let socket = match tokio::net::UdpSocket::from_std(sock) {
                        Ok(s) => Arc::new(s),
                        Err(_) => return,
                    };
                    let recv =
                        tokio::spawn(crate::server::recv_loop(socket.clone(), state.clone()));
                    let ticks =
                        tokio::spawn(crate::server::tick_loop(socket.clone(), state.clone(), 60));
                    while !thread_stop.load(Ordering::Acquire)
                        && parent.as_ref().is_none_or(|p| p.is_alive())
                    {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    recv.abort();
                    ticks.abort();
                    crate::server::shutdown(&socket, &state, 0, "native host stopped").await;
                });
            })?;
        Ok(Self {
            bridge,
            _service: ThreadService {
                stop,
                thread: Some(thread),
            },
            address,
        })
    }
    pub fn directory(&self) -> Option<Directory> {
        self.bridge.lock().directory.clone()
    }
    pub fn descriptor(&self, id: u32) -> Option<Arc<w::Descriptor>> {
        self.bridge.descriptor(id)
    }
    pub fn gameplay_descriptor(&self, id: u32) -> Option<Arc<gp::Bootstrap>> {
        self.bridge.lock().gameplay_descriptors.get(&id).cloned()
    }
    pub fn publish_gameplay_descriptor(&self, d: gp::Bootstrap) -> Result<(), &'static str> {
        self.bridge.publish_gameplay_descriptor(d)
    }
    pub fn publish_descriptor(&self, d: w::Descriptor) -> Result<(), &'static str> {
        self.bridge.publish_descriptor(d)
    }
    pub fn source_roster(&self, facts: SourceRosterFacts) -> Result<u32, &'static str> {
        self.bridge.source_roster(facts)
    }
    pub fn publish_render(&self, frame: w::RenderWorld) -> Result<(), &'static str> {
        self.bridge.publish_render(frame)
    }
    pub fn inputs(&self, max: usize) -> Vec<InputFrame> {
        self.bridge.take_inputs(max)
    }
    pub fn status(&self, state: u8, error: &str) -> Result<(), &'static str> {
        self.bridge.publish_status(state, error)
    }
    pub fn world_changed(&self) {
        self.bridge.request_world_reset();
    }
    /// Game thread only at the caller: the DTO must come from a fresh coherent native sample.
    pub fn publish_world(&self, world: World) -> Result<(), &'static str> {
        self.bridge.publish_world(world)
    }
    pub fn publish_gameplay(&self, frame: gp::ResultFrame) -> Result<(), &'static str> {
        self.bridge.publish_gameplay(frame)
    }
    pub fn compression_metrics(&self) -> CompressionMetrics {
        self.bridge.lock().codec
    }
}
pub struct ClientHandle {
    bridge: Arc<Bridge>,
    _service: ThreadService,
}
impl ClientHandle {
    pub fn start(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::start_role(server, identity_dir, nick, pinned_key, false, false)
    }
    pub fn start_presentation(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::start_role(server, identity_dir, nick, pinned_key, true, false)
    }
    pub fn start_gameplay(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::start_role(server, identity_dir, nick, pinned_key, false, true)
    }
    fn start_role(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
        presentation: bool,
        gameplay: bool,
    ) -> Result<Self> {
        if nick.len() > 32 {
            anyhow::bail!("native client nick exceeds 32 bytes");
        }
        let seed = crate::net::load_or_create_key(&identity_dir.join("identity"))?;
        let local = if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let sock = std::net::UdpSocket::bind(local)?;
        sock.set_nonblocking(true)?;
        let mut cfg = ClientConfig::new(seed, nick);
        cfg.pinned_server_key = pinned_key;
        cfg.build = crate::build_id::build_tag("hsmp-native");
        cfg.content_hash = crate::build_id::content_hash();
        cfg.caps |= hsmp_net::net::caps::NATIVE_WORLD | hsmp_net::net::caps::MODES;
        if gameplay {
            cfg.caps |= gp::CAP_NATIVE_GAMEPLAY
                | gp::CAP_NATIVE_GAMEPLAY_QUATERNION
                | gp::CAP_NATIVE_GAMEPLAY_CACHE
                | gp::CAP_NATIVE_COMPRESSION;
        }
        if presentation {
            cfg.caps |= hsmp_net::net::caps::NATIVE_PRESENTATION
                | hsmp_net::net::caps::NATIVE_RENDER_V2
                | hsmp_net::net::caps::NATIVE_VERTEX_STATE
                | hsmp_net::net::caps::NATIVE_RENDER_V3
                | hsmp_net::net::caps::NATIVE_EMPTY_STATIC
                | hsmp_net::net::caps::NATIVE_EMPTY_SKELETAL
                | hsmp_net::net::caps::NATIVE_SCENE_STREAM;
        }
        let bridge = Arc::new(Bridge::default());
        let network_bridge = bridge.clone();
        bridge.lock().presentation = presentation;
        bridge.lock().gameplay_client = gameplay;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("hsmp-native-client".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(_) => return,
                };
                rt.block_on(client_loop(sock, server, cfg, network_bridge, thread_stop));
            })?;
        Ok(Self {
            bridge,
            _service: ThreadService {
                stop,
                thread: Some(thread),
            },
        })
    }
    pub fn directory(&self) -> Option<Directory> {
        self.bridge.lock().directory.clone()
    }
    pub fn snapshot(&self) -> Option<Arc<World>> {
        self.bridge.lock().world.clone()
    }
    pub fn peer_id(&self) -> u32 {
        self.bridge.lock().peer_id
    }
    pub fn connected(&self) -> bool {
        self.bridge.lock().connected
    }
    pub fn compression_metrics(&self) -> CompressionMetrics {
        self.bridge.lock().codec
    }
    pub fn error(&self) -> String {
        self.bridge.lock().error.clone()
    }
    /// One immutable read; the consumer never mixes independent generation reads.
    pub fn scene(&self) -> Option<Scene> {
        let s = self.bridge.lock();
        if !s.connected {
            return None;
        }
        let frame = s.render.as_ref()?;
        if !Bridge::render_matches(&s, frame) {
            return None;
        }
        let directory = s.directory.clone()?;
        let descriptors = directory
            .entities
            .iter()
            .map(|e| s.descriptors.get(&e.reference.id).cloned())
            .collect::<Option<Vec<_>>>()?;
        Some(Scene {
            directory,
            descriptors,
            frame: frame.clone(),
            peer_id: s.peer_id,
            received: s.render_received?,
        })
    }
    pub fn gameplay_scene(&self) -> Option<GameplayScene> {
        let s = self.bridge.lock();
        if !s.connected || !s.gameplay_client {
            return None;
        }
        let directory = s.directory.clone()?;
        let result = s.gameplay.clone()?;
        if !result.matches(&directory) {
            return None;
        }
        let descriptors = directory
            .entities
            .iter()
            .map(|e| {
                s.gameplay_descriptors
                    .get(&e.reference.id)
                    .filter(|d| {
                        d.reference == e.reference
                            && d.directory_seq == directory.seq
                            && d.slot == e.slot
                    })
                    .cloned()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(GameplayScene {
            directory,
            descriptors,
            result,
            peer_id: s.peer_id,
            received: s.gameplay_received?,
        })
    }
    /// Only the native gameplay apply/readback endpoint may acknowledge this same immutable result.
    pub fn gameplay_applied(&self, scene: &GameplayScene) -> Result<(), &'static str> {
        if !scene.fresh() {
            return Err("native gameplay result is stale");
        }
        let receipt = scene.receipt();
        let mut s = self.bridge.lock();
        if !s.connected
            || !s.gameplay_client
            || !s
                .gameplay
                .as_ref()
                .is_some_and(|current| current.authority_tick >= scene.result.authority_tick)
            || s.gameplay_received
                .is_none_or(|received| received < scene.received)
            || !s
                .directory
                .as_ref()
                .is_some_and(|d| d.epoch == scene.directory.epoch && d.seq == scene.directory.seq)
            || receipt.entities.iter().any(|(r, v)| {
                !s.gameplay_descriptors
                    .get(&r.id)
                    .is_some_and(|d| d.reference == *r && d.revision == *v)
            })
        {
            return Err("native gameplay apply generation changed");
        }
        if s.gameplay_applied.as_ref().is_some_and(|(old, _)| {
            old.epoch == receipt.epoch
                && old.directory_seq == receipt.directory_seq
                && old.frame_seq > receipt.frame_seq
        }) {
            return Err("stale gameplay apply tick");
        }
        s.gameplay_applied = Some((receipt.clone(), scene.received));
        s.gameplay_control_out
            .retain(|r| r.epoch != receipt.epoch || r.directory_seq != receipt.directory_seq);
        if s.gameplay_control_out.len() >= w::MAX_ENTITIES {
            return Err("gameplay readiness queue full");
        }
        s.gameplay_control_out.push_back(receipt);
        Ok(())
    }
    pub fn mirror_ready(&self, receipt: w::MirrorReady) -> Result<(), &'static str> {
        w::encode_mirror_ready(&receipt)?;
        let scene = self.scene().ok_or("no coherent native scene")?;
        if receipt.epoch != scene.directory.epoch
            || receipt.directory_seq != scene.directory.seq
            || receipt.frame_seq > scene.frame.world.frame_seq
            || scene
                .descriptors
                .iter()
                .any(|d| d.source_frame_seq > receipt.frame_seq)
            || receipt.entities
                != scene
                    .descriptors
                    .iter()
                    .map(|d| (d.reference, d.revision))
                    .collect::<Vec<_>>()
        {
            return Err("client mirror generation");
        }
        let mut s = self.bridge.lock();
        if s.control_out.len() >= 32 {
            return Err("mirror control queue full");
        }
        s.control_out.push_back(receipt);
        Ok(())
    }
    /// Records only a complete native apply/readback. Assembly ACK cannot call this path.
    pub fn native_applied(&self, scene: &Scene) -> Result<(), &'static str> {
        let mut s = self.bridge.lock();
        if !s.connected
            || !Bridge::render_matches(&s, &scene.frame)
            || !s
                .directory
                .as_ref()
                .is_some_and(|d| d.epoch == scene.directory.epoch && d.seq == scene.directory.seq)
        {
            return Err("client mirror generation");
        }
        s.applied = Some((
            w::MirrorReady {
                epoch: scene.directory.epoch,
                directory_seq: scene.directory.seq,
                frame_seq: scene.frame.world.frame_seq,
                entities: scene
                    .descriptors
                    .iter()
                    .map(|d| (d.reference, d.revision))
                    .collect(),
            },
            scene.received,
        ));
        Ok(())
    }
    pub fn input(&self, mut frame: InputFrame) -> Result<(), &'static str> {
        frame.validate()?;
        if frame.flags != 0 || frame.delivery_seq != 0 {
            return Err("client input flags");
        }
        let s = self.bridge.lock();
        if !s.connected
            || !s.directory.as_ref().is_some_and(|d| {
                d.entities.iter().any(|e| {
                    e.reference == frame.reference
                        && e.owner_peer == s.peer_id
                        && e.owner_peer != 0
                        && e.kind == w::HUMAN
                })
            })
        {
            return Err("input ownership");
        }
        if s.presentation
            && !s.applied.as_ref().is_some_and(|(r, received)| {
                received.elapsed().as_millis() < w::INPUT_TIMEOUT_MS as u128
                    && s.directory.as_ref().is_some_and(|d| {
                        d.state == w::LIVE && r.epoch == d.epoch && r.directory_seq == d.seq
                    })
                    && r.entities.iter().all(|(r, v)| {
                        s.descriptors
                            .get(&r.id)
                            .is_some_and(|d| d.reference == *r && d.revision == *v)
                    })
            })
        {
            return Err("native applied scene is stale");
        }
        if s.gameplay_client
            && !s.gameplay_applied.as_ref().is_some_and(|(r, received)| {
                received.elapsed().as_millis() < w::INPUT_TIMEOUT_MS as u128
                    && s.directory.as_ref().is_some_and(|d| {
                        d.state == w::LIVE && r.epoch == d.epoch && r.directory_seq == d.seq
                    })
                    && r.entities.iter().all(|(reference, v)| {
                        s.gameplay_descriptors
                            .get(&reference.id)
                            .is_some_and(|d| d.reference == *reference && d.revision == *v)
                    })
            })
        {
            return Err("native gameplay result is stale");
        }
        let (clock, captured) = s.server_clock.ok_or("no server clock")?;
        frame.sample_ms = clock.saturating_add(captured.elapsed().as_millis() as u64);
        let gameplay_directory = s
            .gameplay_client
            .then(|| s.directory.as_ref().map(|d| d.seq))
            .flatten();
        drop(s);
        if let Some(directory_seq) = gameplay_directory {
            let mut s = self.bridge.lock();
            if s.gameplay_inputs.len() >= w::MAX_INPUTS {
                return Err("gameplay input queue full");
            }
            s.gameplay_inputs.push_back((
                gp::Request {
                    directory_seq,
                    input: frame,
                },
                Instant::now(),
            ));
            return Ok(());
        }
        if !self.bridge.push_input(frame) {
            return Err("input queue full");
        }
        Ok(())
    }
}
async fn client_loop(
    sock: std::net::UdpSocket,
    server: SocketAddr,
    mut cfg: ClientConfig,
    bridge: Arc<Bridge>,
    stop: Arc<AtomicBool>,
) {
    let socket = match tokio::net::UdpSocket::from_std(sock) {
        Ok(s) => s,
        Err(_) => return,
    };
    let started = Instant::now();
    let now = || started.elapsed().as_millis() as u64;
    let conn = ConnConfig {
        dead_after_ms: hsmp_net::net::conn::CLIENT_DEAD_AFTER_MS,
        ..Default::default()
    };
    let mut client = Client::new(
        copy_client_config(&cfg),
        conn.clone(),
        0,
        &mut rand::rngs::OsRng,
    );
    let mut timer = tokio::time::interval(Duration::from_millis(5));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut buf = [0u8; hsmp_net::net::MAX_DATAGRAM];
    let mut reconnect_at = None;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        tokio::select! {
            _ = timer.tick() => {},
            result = socket.recv_from(&mut buf) => { if let Ok((n, from)) = result { if from == server { client.handle(now(), &buf[..n]); } } },
        }
        if reconnect_at.is_some_and(|t| now() >= t) {
            client = Client::new(
                copy_client_config(&cfg),
                conn.clone(),
                now(),
                &mut rand::rngs::OsRng,
            );
            reconnect_at = None;
        }
        let inputs = {
            let mut s = bridge.lock();
            s.inputs.drain(..).collect::<Vec<_>>()
        };
        for (input, captured) in inputs {
            if captured.elapsed().as_millis() >= w::INPUT_TIMEOUT_MS as u128 {
                continue;
            }
            if let Ok(payload) = w::encode_input(&input) {
                let bytes = hsmp_ipc::wire::message(w::K_INPUT, 0, 0, &payload);
                if client.send(SendMode::Ordered, bytes).is_err() {
                    let mut s = bridge.lock();
                    s.error = "native input transport full".into();
                }
            }
        }
        let gameplay_inputs = {
            let mut s = bridge.lock();
            s.gameplay_inputs.drain(..).collect::<Vec<_>>()
        };
        for (request, captured) in gameplay_inputs {
            if captured.elapsed().as_millis() >= w::INPUT_TIMEOUT_MS as u128 {
                continue;
            }
            if let Ok(payload) = gp::encode_request(&request) {
                if client
                    .send(
                        SendMode::Ordered,
                        hsmp_ipc::wire::message(w::K_GAMEPLAY_REQUEST, 0, 0, &payload),
                    )
                    .is_err()
                {
                    bridge.lock().error = "native gameplay request transport full".into();
                }
            }
        }
        let ready = bridge.lock().gameplay_control_out.front().cloned();
        if let Some(receipt) = ready {
            if let Ok(payload) = w::encode_mirror_ready(&receipt) {
                if client
                    .send(
                        SendMode::Ordered,
                        hsmp_ipc::wire::message(w::K_GAMEPLAY_READY, 0, 0, &payload),
                    )
                    .is_ok()
                {
                    let mut s = bridge.lock();
                    if s.gameplay_control_out.front() == Some(&receipt) {
                        s.gameplay_control_out.pop_front();
                    }
                }
            }
        }
        bridge.send_receipt(|receipt| {
            if let Ok(payload) = w::encode_mirror_ready(&receipt) {
                return client
                    .send(
                        SendMode::Ordered,
                        hsmp_ipc::wire::message(w::K_MIRROR_READY, 0, 0, &payload),
                    )
                    .is_ok();
            }
            false
        });
        if let Err(e) = bridge
            .finish_descriptors()
            .and_then(|()| bridge.admit_stream())
        {
            let mut s = bridge.lock();
            s.error = e.into();
            s.connected = false;
            return;
        }
        let ack = bridge.lock().stream_ack.clone();
        if let Some(a) = ack {
            if let Ok(payload) = w::scene_stream::encode_ack(&a) {
                if client
                    .send(
                        SendMode::ReliableLatest {
                            key: hsmp_net::proto_v5::keys::key(0x97, 0),
                        },
                        hsmp_ipc::wire::message(w::K_SCENE_ACK, 0, 0, &payload),
                    )
                    .is_ok()
                {
                    let mut s = bridge.lock();
                    if s.stream_ack.as_ref() == Some(&a) {
                        s.stream_ack = None;
                    }
                }
            }
        }
        while let Some(dg) = client.poll_transmit(now()) {
            let _ = socket.send_to(&dg, server).await;
        }
        while let Some(event) = client.poll_event() {
            match event {
                ClientEvent::Connected {
                    server_key, caps, ..
                } => {
                    bridge.lock().native_compression = cfg.caps & gp::CAP_NATIVE_COMPRESSION != 0
                        && caps & gp::CAP_NATIVE_COMPRESSION != 0;
                    if caps & hsmp_net::net::caps::NATIVE_WORLD == 0 {
                        bridge.lock().error = "server has no native authority".into();
                        return;
                    }
                    if cfg.caps & gp::CAP_NATIVE_GAMEPLAY != 0 && !gp::gameplay_capable(caps) {
                        bridge.lock().error =
                            "server has no exact-quaternion native gameplay".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_PRESENTATION != 0
                        && caps & hsmp_net::net::caps::NATIVE_PRESENTATION == 0
                    {
                        bridge.lock().error = "server has no native presentation".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_RENDER_V2 != 0
                        && caps & hsmp_net::net::caps::NATIVE_RENDER_V2 == 0
                    {
                        bridge.lock().error =
                            "server does not support the required native scene protocol".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_VERTEX_STATE != 0
                        && caps & hsmp_net::net::caps::NATIVE_VERTEX_STATE == 0
                    {
                        bridge.lock().error = "server does not verify native vertex state".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_RENDER_V3 != 0
                        && caps & hsmp_net::net::caps::NATIVE_RENDER_V3 == 0
                    {
                        bridge.lock().error =
                            "server does not preserve native spring arm state".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_EMPTY_STATIC != 0
                        && caps & hsmp_net::net::caps::NATIVE_EMPTY_STATIC == 0
                    {
                        bridge.lock().error =
                            "server does not verify empty native static components".into();
                        return;
                    }
                    if cfg.caps & hsmp_net::net::caps::NATIVE_EMPTY_SKELETAL != 0
                        && caps & hsmp_net::net::caps::NATIVE_EMPTY_SKELETAL == 0
                    {
                        bridge.lock().error =
                            "server does not verify empty native skeletal components".into();
                        return;
                    }
                    cfg.pinned_server_key = Some(server_key);
                    if cfg.caps & hsmp_net::net::caps::NATIVE_PRESENTATION != 0
                        && caps & hsmp_net::net::caps::NATIVE_SCENE_STREAM == 0
                    {
                        bridge.lock().error =
                            "server does not support bounded native scene streams".into();
                        return;
                    }
                    let mut s = bridge.lock();
                    s.connected = true;
                    s.error.clear();
                }
                ClientEvent::Message(d) => {
                    if let Ok((h, payload)) = hsmp_ipc::wire::split(&d.data) {
                        let decoded = if h.kind == w::K_NATIVE_COMPRESSED {
                            let started = Instant::now();
                            let negotiated = bridge.lock().native_compression;
                            let unpacked = decode_compressed_authority(negotiated, h.peer, payload);
                            if let Ok((kind, body)) = &unpacked {
                                if *kind == w::K_GAMEPLAY_RESULT {
                                    let mut s = bridge.lock();
                                    if s.codec.decode_samples < 8 {
                                        s.codec.decode_samples += 1;
                                        s.codec.decode_raw_bytes += body.len() as u64;
                                        s.codec.decode_wire_bytes += payload.len() as u64;
                                        s.codec.decode_us +=
                                            started.elapsed().as_micros().min(u64::MAX as u128)
                                                as u64;
                                    }
                                }
                            }
                            match unpacked {
                                Ok((kind, bytes))
                                    if matches!(
                                        kind,
                                        w::K_GAMEPLAY_RESULT
                                            | w::K_DESCRIPTOR_PART
                                            | w::K_GAMEPLAY_BOOTSTRAP_PART
                                    ) && h.peer == kind as u32 =>
                                {
                                    Some((kind, bytes))
                                }
                                _ => {
                                    bridge.lock().error =
                                        "native compressed authority record refused".into();
                                    return;
                                }
                            }
                        } else {
                            None
                        };
                        let (kind, payload) = decoded
                            .as_ref()
                            .map_or((h.kind, payload), |(kind, bytes)| (*kind, bytes.as_slice()));
                        match kind {
                            hsmp_ipc::schema::session::K_WELCOME => {
                                if let Ok(v) = hsmp_ipc::record::view::<
                                    hsmp_ipc::schema::session::Welcome,
                                >(payload)
                                {
                                    let mut s = bridge.lock();
                                    s.peer_id = v.head().peer_id;
                                    s.server_clock =
                                        Some((v.head().server_time_ms, Instant::now()));
                                }
                            }
                            w::K_DIRECTORY => {
                                if let Ok(dir) = w::decode_directory(payload) {
                                    let accept = {
                                        let s = bridge.lock();
                                        s.directory.as_ref().is_none_or(|old| {
                                            old.epoch != dir.epoch
                                                || dir.seq > old.seq
                                                || (dir.seq == old.seq
                                                    && dir.entities == old.entities
                                                    && dir != *old)
                                        })
                                    };
                                    if accept {
                                        bridge.set_directory(dir);
                                    }
                                }
                            }
                            w::K_WORLD => {
                                if let Ok(world) = w::decode_world(payload) {
                                    let mut s = bridge.lock();
                                    if s.directory
                                        .as_ref()
                                        .is_some_and(|d| w::matches_directory(&world, d))
                                        && s.world
                                            .as_ref()
                                            .is_none_or(|old| world.frame_seq > old.frame_seq)
                                    {
                                        s.world = Some(Arc::new(world));
                                    }
                                }
                            }
                            w::K_GAMEPLAY_RESULT => {
                                if let Ok(frame) = gp::decode_result(payload) {
                                    let mut s = bridge.lock();
                                    if s.gameplay_client
                                        && s.directory.as_ref().is_some_and(|d| frame.matches(d))
                                        && s.gameplay.as_ref().is_none_or(|old| {
                                            frame.authority_tick > old.authority_tick
                                        })
                                        && s.gameplay.as_ref().is_none_or(|old| {
                                            frame.entities.iter().all(|e| {
                                                old.entities
                                                    .iter()
                                                    .filter(|p| p.reference == e.reference)
                                                    .all(|p| {
                                                        e.request_seq >= p.request_seq
                                                            && e.delivery_seq >= p.delivery_seq
                                                    })
                                            })
                                        })
                                    {
                                        s.gameplay = Some(Arc::new(frame));
                                        s.gameplay_received = Some(Instant::now());
                                    }
                                }
                            }
                            w::K_DESCRIPTOR => {
                                bridge.lock().error =
                                    "server sent an incompatible native recipe record".into();
                                return;
                            }
                            w::K_DESCRIPTOR_PART => {
                                let result = { bridge.lock().descriptor_stream.part(payload) };
                                if let Err(e) = result {
                                    let mut s = bridge.lock();
                                    s.error = e.into();
                                    s.connected = false;
                                    return;
                                }
                            }
                            w::K_GAMEPLAY_BOOTSTRAP_PART => {
                                let result = {
                                    let mut s = bridge.lock();
                                    if !s.gameplay_client {
                                        Err("gameplay bootstrap capability not negotiated")
                                    } else {
                                        s.gameplay_descriptor_stream
                                            .get_or_insert_with(
                                                w::descriptor_stream::Assembly::gameplay,
                                            )
                                            .part(payload)
                                    }
                                };
                                if let Err(error) = result {
                                    let mut s = bridge.lock();
                                    s.error = error.into();
                                    s.connected = false;
                                }
                            }
                            w::K_RENDER_WORLD | w::K_RENDER_WORLD_V2 | w::K_RENDER_WORLD_V3 => {
                                let mut s = bridge.lock();
                                s.error = "server sent an incompatible native scene frame".into();
                                s.connected = false;
                                s.render = None;
                                s.render_received = None;
                                return;
                            }
                            w::K_SCENE_MANIFEST => {
                                let result = w::scene_stream::decode_manifest(payload)
                                    .and_then(|m| bridge.lock().stream.offer(m));
                                match result {
                                    Ok(Some(a)) => bridge.lock().stream_ack = Some(a),
                                    Ok(None) => {}
                                    Err(e) => {
                                        let mut s = bridge.lock();
                                        s.error = e.into();
                                        s.connected = false;
                                        return;
                                    }
                                }
                            }
                            w::K_SCENE_PART => {
                                if let Err(e) = bridge.receive_scene_part(payload) {
                                    let mut s = bridge.lock();
                                    s.error = e.into();
                                    s.connected = false;
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                ClientEvent::Closed { code, .. } => {
                    let mut s = bridge.lock();
                    s.connected = false;
                    s.peer_id = 0;
                    s.world = None;
                    s.directory = None;
                    s.inputs.clear();
                    s.descriptors.clear();
                    s.gameplay_descriptors.clear();
                    s.gameplay_descriptor_out.clear();
                    s.gameplay_descriptor_stream = None;
                    s.render = None;
                    s.control_out.clear();
                    s.stream = Default::default();
                    s.stream_ack = None;
                    s.applied = None;
                    s.render_received = None;
                    s.descriptor_stream = Default::default();
                    s.server_clock = None;
                    if matches!(
                        code,
                        hsmp_net::net::close_code::KICKED
                            | hsmp_net::net::close_code::REPLACED
                            | hsmp_net::net::close_code::SERVER_CLOSING
                    ) {
                        s.error = "native session closed".into();
                        return;
                    }
                    reconnect_at = Some(now() + 500);
                }
                ClientEvent::Rejected { text, .. } => {
                    let mut s = bridge.lock();
                    s.connected = false;
                    s.error = text;
                    return;
                }
                ClientEvent::Failed(reason) => {
                    let mut s = bridge.lock();
                    s.connected = false;
                    s.world = None;
                    s.directory = None;
                    s.inputs.clear();
                    s.descriptors.clear();
                    s.gameplay_descriptors.clear();
                    s.gameplay_descriptor_out.clear();
                    s.gameplay_descriptor_stream = None;
                    s.render = None;
                    s.control_out.clear();
                    s.stream = Default::default();
                    s.stream_ack = None;
                    s.applied = None;
                    s.render_received = None;
                    s.descriptor_stream = Default::default();
                    s.peer_id = 0;
                    s.server_clock = None;
                    s.error = reason.into();
                    if reason == "no answer from the server" {
                        reconnect_at = Some(now() + 500);
                    } else {
                        return;
                    }
                }
            }
        }
    }
    client.close(
        now(),
        hsmp_net::net::close_code::NORMAL,
        "native client stopped",
    );
    for _ in 0..4 {
        while let Some(dg) = client.poll_transmit(now()) {
            let _ = socket.send_to(&dg, server).await;
        }
        tokio::time::sleep(Duration::from_millis(31)).await;
    }
}
fn copy_client_config(cfg: &ClientConfig) -> ClientConfig {
    let mut out = ClientConfig::new(cfg.player_seed, &cfg.nick);
    out.content_hash = cfg.content_hash;
    out.caps = cfg.caps;
    out.pinned_server_key = cfg.pinned_server_key;
    out.build = cfg.build.clone();
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn gameplay_compression_requires_negotiated_authenticated_kind_and_bound() {
        let body = vec![0u8; 317];
        let packed = hsmp_net::net::compression::pack(w::K_GAMEPLAY_RESULT, &body).unwrap();
        assert_eq!(
            decode_compressed_authority(true, w::K_GAMEPLAY_RESULT as u32, &packed).unwrap(),
            (w::K_GAMEPLAY_RESULT, body)
        );
        assert!(decode_compressed_authority(false, w::K_GAMEPLAY_RESULT as u32, &packed).is_err());
        assert!(decode_compressed_authority(true, w::K_DESCRIPTOR_PART as u32, &packed).is_err());
        assert!(decode_compressed_authority(true, w::K_GAMEPLAY_READY as u32, &[]).is_err());
        let oversized = hsmp_net::net::compression::pack(
            w::K_GAMEPLAY_RESULT,
            &vec![0; gp::MAX_RESULT_BYTES + 1],
        )
        .unwrap();
        assert!(
            decode_compressed_authority(true, w::K_GAMEPLAY_RESULT as u32, &oversized).is_err()
        );
        let nested =
            hsmp_net::net::compression::pack(w::K_NATIVE_COMPRESSED, &vec![0; 317]).unwrap();
        assert!(decode_compressed_authority(true, w::K_GAMEPLAY_RESULT as u32, &nested).is_err());
    }
    fn gameplay_fixture() -> (ClientHandle, Arc<Bridge>) {
        let (directory, recipes, _) = w::scene_stream::tests::fixture();
        let bridge = Arc::new(Bridge::default());
        bridge.set_directory(directory.clone());
        for d in recipes {
            bridge
                .publish_gameplay_descriptor(gp::Bootstrap {
                    reference: d.reference,
                    slot: d.slot,
                    directory_seq: d.directory_seq,
                    revision: d.revision,
                    source_frame_seq: d.source_frame_seq,
                    recipe: gp::fixture_recipe(),
                })
                .unwrap();
        }
        let frame = gp::ResultFrame {
            epoch: directory.epoch,
            directory_seq: directory.seq,
            authority_tick: 1,
            entities: directory
                .entities
                .iter()
                .map(|e| gp::State {
                    reference: e.reference,
                    request_seq: 0,
                    delivery_seq: 0,
                    buttons: 0,
                    axes: [0.; 8],
                    position: [0.; 3],
                    rotation: [0.; 3],
                    velocity: [0.; 3],
                    orientation: [0., 0., 0., 1.],
                    cache_rotation: [0.; 3],
                    health: gp::NativeScalar::F32(100f32.to_bits()),
                    stamina: gp::NativeScalar::F64(99f64.to_bits()),
                })
                .collect(),
        };
        bridge.publish_gameplay(frame).unwrap();
        {
            let mut s = bridge.lock();
            s.connected = true;
            s.gameplay_client = true;
            s.peer_id = 9001;
        }
        let client = ClientHandle {
            bridge: bridge.clone(),
            _service: ThreadService {
                stop: Arc::new(AtomicBool::new(false)),
                thread: None,
            },
        };
        (client, bridge)
    }
    #[test]
    fn gameplay_bootstrap_native_ack_is_separate_from_scene_assembly() {
        let (client, bridge) = gameplay_fixture();
        let scene = client.gameplay_scene().unwrap();
        assert!(scene.fresh());
        assert!(bridge.lock().render.is_none());
        assert!(!bridge.gameplay_ready(9001));
        client.gameplay_applied(&scene).unwrap();
        let receipt = bridge.lock().gameplay_control_out.front().cloned().unwrap();
        bridge.accept_gameplay_ready(9001, receipt.clone()).unwrap();
        assert!(bridge.gameplay_ready(9001));
        assert!(!bridge.mirror_ready(9001));
        assert!(bridge.accept_gameplay_ready(123456, receipt).is_err());
    }
    #[test]
    fn gameplay_original_receipt_and_result_arc_never_renew() {
        let (client, bridge) = gameplay_fixture();
        let old = client.gameplay_scene().unwrap();
        let mut next = (*old.result).clone();
        next.authority_tick += 1;
        bridge.publish_gameplay(next).unwrap();
        client.gameplay_applied(&old).unwrap();
        assert_eq!(
            bridge.lock().gameplay_applied.as_ref().unwrap().1,
            old.received
        );
        assert_eq!(old.result.authority_tick, 1);
        let mut stale = old;
        stale.received = Instant::now()
            .checked_sub(Duration::from_millis(300))
            .unwrap();
        assert!(!stale.fresh());
        assert!(client.gameplay_applied(&stale).is_err());
    }
    #[test]
    fn gameplay_authority_ticks_and_execution_ack_are_monotonic() {
        let (_, bridge) = gameplay_fixture();
        let original = bridge.lock().gameplay.as_ref().unwrap().as_ref().clone();
        assert!(bridge.publish_gameplay(original.clone()).is_err());
        let mut next = original.clone();
        next.authority_tick = 2;
        next.entities[0].request_seq = 2;
        next.entities[0].delivery_seq = 1;
        bridge.publish_gameplay(next.clone()).unwrap();
        next.authority_tick = 3;
        next.entities[0].request_seq = 0;
        next.entities[0].delivery_seq = 0;
        assert!(bridge.publish_gameplay(next).is_err());
        let mut wrong = original;
        wrong.authority_tick = 4;
        wrong.directory_seq += 1;
        assert!(bridge.publish_gameplay(wrong).is_err());
    }
    #[test]
    fn compact_gameplay_bootstrap_results_and_ordered_edges_cross_authenticated_udp() {
        let temp = std::env::temp_dir().join(format!(
            "hsmp-gameplay-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        {
            let host = HostHandle::start_with_parent_mode(
                "127.0.0.1:0".parse().unwrap(),
                &temp.join("host"),
                "Map_Arena_Yard",
                None,
                crate::native_mode::Mode::Pvp,
            )
            .unwrap();
            host.status(w::READY, "").unwrap();
            let a = ClientHandle::start_gameplay(host.address, &temp.join("a"), "A", None).unwrap();
            let b = ClientHandle::start_gameplay(host.address, &temp.join("b"), "B", None).unwrap();
            wait_for(|| {
                a.connected()
                    && b.connected()
                    && host
                        .directory()
                        .is_some_and(|d| d.entities.iter().all(|e| e.owner_peer != 0))
            });
            let directory = host.directory().unwrap();
            wait_for(|| {
                a.directory().is_some_and(|d| d.seq == directory.seq)
                    && b.directory().is_some_and(|d| d.seq == directory.seq)
            });
            for e in &directory.entities {
                host.publish_gameplay_descriptor(gp::Bootstrap {
                    reference: e.reference,
                    slot: e.slot,
                    directory_seq: directory.seq,
                    revision: 1,
                    source_frame_seq: 1,
                    recipe: gp::fixture_recipe(),
                })
                .unwrap();
            }
            let mut result = gp::ResultFrame {
                epoch: directory.epoch,
                directory_seq: directory.seq,
                authority_tick: 1,
                entities: directory
                    .entities
                    .iter()
                    .map(|e| gp::State {
                        reference: e.reference,
                        request_seq: 0,
                        delivery_seq: 0,
                        buttons: 0,
                        axes: [0.; 8],
                        position: [-0., 1.0000000000000002, 2.],
                        rotation: [0.; 3],
                        velocity: [0.; 3],
                        orientation: [-0., 0.5000000000000001, -0.5, 0.7071067811865475],
                        cache_rotation: [-0., 90.00000000000001, 1.0000000000000002],
                        health: gp::NativeScalar::F32(100.125f32.to_bits()),
                        stamina: gp::NativeScalar::F64(70.12345678901234f64.to_bits()),
                    })
                    .collect(),
            };
            wait_for(|| {
                host.publish_gameplay(result.clone()).unwrap();
                result.authority_tick += 1;
                a.gameplay_scene().is_some() && b.gameplay_scene().is_some()
            });
            let first = a.gameplay_scene().unwrap();
            assert!(a.scene().is_none());
            assert_eq!(
                first.result.entities[0].position[1].to_bits(),
                1.0000000000000002f64.to_bits()
            );
            for (actual, expected) in first.result.entities[0]
                .orientation
                .into_iter()
                .zip(result.entities[0].orientation)
            {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
            for (actual, expected) in first.result.entities[0]
                .cache_rotation
                .into_iter()
                .zip(result.entities[0].cache_rotation)
            {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
            assert_eq!(
                first.result.entities[0].health,
                gp::NativeScalar::F32(100.125f32.to_bits())
            );
            a.gameplay_applied(&first).unwrap();
            b.gameplay_applied(&b.gameplay_scene().unwrap()).unwrap();
            wait_for(|| {
                host.publish_gameplay(result.clone()).unwrap();
                result.authority_tick += 1;
                a.directory().is_some_and(|d| d.state == w::LIVE)
                    && b.directory().is_some_and(|d| d.state == w::LIVE)
            });
            let own = directory
                .entities
                .iter()
                .find(|e| e.owner_peer == a.peer_id())
                .unwrap()
                .reference;
            let other = directory
                .entities
                .iter()
                .find(|e| e.owner_peer == b.peer_id())
                .unwrap()
                .reference;
            let scene = a.gameplay_scene().unwrap();
            a.gameplay_applied(&scene).unwrap();
            assert!(a
                .input(InputFrame {
                    reference: other,
                    seq: 1,
                    ..Default::default()
                })
                .is_err());
            a.input(InputFrame {
                reference: own,
                seq: 1,
                buttons: 1,
                axes: [0.5, 0., 0., 0., 0., 0., 0., 0.],
                ..Default::default()
            })
            .unwrap();
            a.input(InputFrame {
                reference: own,
                seq: 2,
                buttons: 0,
                ..Default::default()
            })
            .unwrap();
            let mut delivered = Vec::new();
            wait_for(|| {
                delivered.extend(
                    host.inputs(32)
                        .into_iter()
                        .filter(|f| f.reference == own && f.flags == 0),
                );
                delivered.len() >= 2
            });
            assert_eq!(
                delivered
                    .iter()
                    .map(|f| (f.seq, f.delivery_seq, f.buttons))
                    .collect::<Vec<_>>(),
                vec![(1, 1, 1), (2, 2, 0)]
            );
            let row = result
                .entities
                .iter_mut()
                .find(|e| e.reference == own)
                .unwrap();
            row.request_seq = 2;
            row.delivery_seq = 2;
            row.buttons = 0;
            host.publish_gameplay(result.clone()).unwrap();
            result.authority_tick += 1;
            wait_for(|| {
                a.gameplay_scene().is_some_and(|s| {
                    s.result
                        .entities
                        .iter()
                        .any(|e| e.reference == own && e.request_seq == 2)
                })
            });
            let metrics = host.compression_metrics();
            assert!(metrics.encode_samples > 0);
            assert!(metrics.wire_bytes < metrics.raw_bytes);
            assert!(a.compression_metrics().decode_samples > 0);
            {
                let mut s = a.bridge.lock();
                let received = Instant::now()
                    .checked_sub(Duration::from_millis(300))
                    .unwrap();
                if let Some((_, receipt)) = s.gameplay_applied.as_mut() {
                    *receipt = received;
                }
            }
            assert!(a
                .input(InputFrame {
                    reference: own,
                    seq: 3,
                    ..Default::default()
                })
                .is_err());
        }
        assert!(
            temp.starts_with(std::env::temp_dir())
                && temp
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("hsmp-gameplay-")
        );
        std::fs::remove_dir_all(temp).unwrap();
    }
    fn wait_for(mut f: impl FnMut() -> bool) {
        let started = Instant::now();
        while !f() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "native loop did not converge"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub(crate) fn fixture_world(d: &Directory, seq: u32, ai_dead: bool) -> World {
        let entities = d
            .entities
            .iter()
            .map(|e| {
                let context = hsmp_pose::posecodec::v2::Context {
                    match_id: d.epoch,
                    round: 1,
                    life: 1,
                };
                let pose = hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full {
                    context: Some(context),
                    ts: 100.0,
                    ..Default::default()
                });
                let root = hsmp_ipc::schema::pose::Root {
                    tick: seq,
                    ts: 100,
                    rot: [0.0, 0.0, 0.0, 1.0],
                    match_id: d.epoch,
                    round: 1,
                    life: 1,
                    ..Default::default()
                };
                let vitals = hsmp_ipc::schema::combat::Vitals {
                    seq,
                    flags: if ai_dead && e.kind == w::AI {
                        hsmp_ipc::schema::combat::VF_DEAD
                    } else {
                        0
                    },
                    match_id: d.epoch,
                    round: 1,
                    life: 1,
                    ..Default::default()
                };
                w::EntitySnapshot {
                    reference: e.reference,
                    root,
                    vitals,
                    pose,
                }
            })
            .collect();
        World {
            epoch: d.epoch,
            directory_seq: d.seq,
            frame_seq: seq,
            entities,
        }
    }
    #[test]
    fn native_scene_stream_complete_ack_is_not_mirror_ready_and_stale_prepare_cannot_input() {
        let (directory, recipes, frame) = w::scene_stream::tests::fixture();
        let bridge = Arc::new(Bridge::default());
        bridge.set_directory(directory.clone());
        let batch = w::scene_stream::Batch::new(&frame, &recipes).unwrap();
        bridge.lock().stream.offer(batch.manifest.clone()).unwrap();
        bridge.admit_stream().unwrap();
        assert!(
            bridge.lock().stream_ack.is_none(),
            "wait for reliable recipes before allocating"
        );
        assert!(bridge.lock().render.is_none());
        for d in &recipes {
            bridge.publish_descriptor((**d).clone()).unwrap()
        }
        bridge.admit_stream().unwrap();
        assert_eq!(
            bridge.lock().stream_ack.as_ref().unwrap().stage,
            w::scene_stream::ACK_ADMITTED
        );
        assert!(!bridge.mirror_ready(9001));
        for i in 0..batch.manifest.token.parts() {
            bridge.receive_scene_part(&batch.part(i).unwrap()).unwrap()
        }
        assert_eq!(**bridge.lock().render.as_ref().unwrap(), frame);
        assert_eq!(
            bridge.lock().stream_ack.as_ref().unwrap().stage,
            w::scene_stream::ACK_COMPLETE
        );
        assert!(
            !bridge.mirror_ready(9001),
            "decoded body is not native apply proof"
        );
        let received = Instant::now()
            .checked_sub(Duration::from_millis(300))
            .expect("stale scene fixture clock supports 300ms history");
        {
            let mut s = bridge.lock();
            s.connected = true;
            s.peer_id = 9001;
            s.presentation = true;
            s.render_received = Some(received);
        }
        let client = ClientHandle {
            bridge: bridge.clone(),
            _service: ThreadService {
                stop: Arc::new(AtomicBool::new(false)),
                thread: None,
            },
        };
        let scene = client.scene().unwrap();
        assert!(
            !scene.fresh(),
            "generation-valid READY scene survives preparation latency"
        );
        client.native_applied(&scene).unwrap();
        let receipt = w::MirrorReady {
            epoch: directory.epoch,
            directory_seq: directory.seq,
            frame_seq: 1,
            entities: batch.manifest.entities.clone(),
        };
        client.mirror_ready(receipt.clone()).unwrap();
        bridge.send_receipt(|_| false);
        assert_eq!(
            bridge.lock().control_out.front(),
            Some(&receipt),
            "unsent native readiness survives backpressure"
        );
        let mut attempts = 0;
        bridge.send_receipt(|r| {
            attempts += 1;
            assert_eq!(r, &receipt);
            true
        });
        assert_eq!(attempts, 1);
        assert!(bridge.lock().control_out.is_empty());
        bridge.accept_mirror(9001, receipt).unwrap();
        assert!(bridge.mirror_ready(9001));
        let mut live = directory;
        live.state = w::LIVE;
        bridge.set_directory(live);
        assert_eq!(
            client.input(InputFrame {
                reference: frame.entities[0].reference,
                seq: 1,
                ..Default::default()
            }),
            Err("native applied scene is stale")
        );
        assert_eq!(bridge.lock().render_received, Some(received));
        let mut revised = (*recipes[0]).clone();
        revised.revision += 1;
        bridge.publish_descriptor(revised).unwrap();
        assert!(client.scene().is_none());
        assert!(bridge.lock().applied.is_none());
        assert!(!bridge.mirror_ready(9001));
    }
    #[test]
    fn native_scene_retired_same_directory_parts_cannot_disconnect_new_revision() {
        let (directory, recipes, frame) = w::scene_stream::tests::fixture();
        let bridge = Bridge::default();
        bridge.set_directory(directory);
        bridge.publish_descriptor((*recipes[0]).clone()).unwrap();
        let old = w::scene_stream::Batch::new(&frame, &recipes).unwrap();
        bridge.lock().stream.offer(old.manifest.clone()).unwrap();
        bridge.admit_stream().unwrap();
        bridge.receive_scene_part(&old.part(0).unwrap()).unwrap();
        assert!(bridge.lock().render.is_none());
        let mut revised = (*recipes[0]).clone();
        revised.revision += 1;
        bridge.publish_descriptor(revised.clone()).unwrap();
        bridge.receive_scene_part(&old.part(1).unwrap()).unwrap();
        assert!(bridge.lock().render.is_none());
        let mut next = frame;
        next.world.frame_seq += 1;
        next.entities[0].revision = revised.revision;
        let batch = w::scene_stream::Batch::new(&next, &[Arc::new(revised)]).unwrap();
        bridge.lock().stream.offer(batch.manifest.clone()).unwrap();
        bridge.admit_stream().unwrap();
        for i in 0..batch.manifest.token.parts() {
            bridge.receive_scene_part(&batch.part(i).unwrap()).unwrap()
        }
        assert_eq!(**bridge.lock().render.as_ref().unwrap(), next);
        assert!(!bridge.mirror_ready(9001));
    }
    #[test]
    fn native_attachment_readiness_requires_the_complete_current_descriptor_profile() {
        use crate::native_descriptor::{
            ComponentKind, Geometry, SceneEvidence, SplineProfile, VertexState,
        };
        let bridge = Bridge::default();
        let reference = w::EntityRef {
            epoch: 19,
            id: 1,
            incarnation: 2,
        };
        let directory = Directory {
            epoch: 19,
            seq: 3,
            state: w::READY,
            arena: "Map_Arena_Yard".into(),
            error: String::new(),
            entities: vec![w::Entity {
                reference,
                owner_peer: 9001,
                slot: 0,
                kind: w::HUMAN,
                controller: 0,
                team: None,
            }],
        };
        bridge.set_directory(directory.clone());
        let mut recipe = crate::native_descriptor::fixture_recipe();
        let mut component = recipe.components[0].clone();
        component.id = 2;
        component.name = "Aim Spline".into();
        component.role = "attachment".into();
        component.component_class = "/Script/Engine.SplineComponent".into();
        component.kind = ComponentKind::Spline;
        component.geometry = Geometry::NativeSpline;
        component.scene = SceneEvidence::NotApplicable;
        component.vertex_state = VertexState::NotApplicable;
        component.asset.clear();
        component.skeleton.clear();
        component.physics_asset.clear();
        component.bones.clear();
        component.materials.clear();
        component.morphs.clear();
        component.hidden_bones.clear();
        component.groom.clear();
        component.vertex_colors.clear();
        component.deformer.clear();
        component.cloth = false;
        component.parent = 0;
        component.spline_profile = Some(SplineProfile {
            position_count: 1,
            rotation_count: 1,
            scale_count: 1,
            reparam_count: 1,
            metadata_null: true,
        });
        recipe.components.push(component);
        let mut arm = recipe.components[1].clone();
        arm.id = 3;
        arm.name = "CameraBoom(Shoulder)".into();
        arm.role = "anchor".into();
        arm.component_class = "/Script/Engine.SpringArmComponent".into();
        arm.kind = ComponentKind::Scene;
        arm.geometry = Geometry::NotApplicable;
        arm.scene = SceneEvidence::SpringArm {
            draw_debug_lag_markers: false,
            socket_name: "SpringEndpoint".into(),
        };
        arm.collision = None;
        arm.spline_profile = None;
        recipe.components.push(arm);
        let descriptor = w::Descriptor {
            reference,
            slot: 0,
            directory_seq: 3,
            revision: 4,
            source_frame_seq: 1,
            recipe,
        };
        bridge.publish_descriptor(descriptor.clone()).unwrap();
        let transform = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let frame = w::RenderWorld {
            world: fixture_world(&directory, 1, false),
            entities: vec![w::RenderEntity {
                reference,
                revision: 4,
                components: descriptor
                    .recipe
                    .components
                    .iter()
                    .map(|c| w::RenderComponent {
                        id: c.id,
                        transform,
                        bones: vec![transform; c.bones.len()],
                        morphs: c.morphs.iter().map(|m| m.value).collect(),
                        materials: c
                            .materials
                            .iter()
                            .map(|m| w::RenderMaterial {
                                scalars: m.scalars.iter().map(|p| p.value).collect(),
                                vectors: m.vectors.iter().map(|p| p.value).collect(),
                                textures: m.textures.iter().map(|p| p.value.clone()).collect(),
                            })
                            .collect(),
                        spline: c
                            .spline_profile
                            .as_ref()
                            .map(|_| w::tests::spline_fixture()),
                        spring_arm: matches!(c.scene, SceneEvidence::SpringArm { .. }).then_some(
                            w::NativeSpringArmFrame {
                                translation: [-20.0, 0.0, 0.0],
                                rotation: [0.0, 0.0, 0.0, 1.0],
                            },
                        ),
                    })
                    .collect(),
            }],
        };
        let receipt = w::MirrorReady {
            epoch: 19,
            directory_seq: 3,
            frame_seq: 1,
            entities: vec![(reference, 4)],
        };
        for change in 0..10 {
            let mut bad = frame.clone();
            match change {
                0 => bad.entities[0].components[1].spline = None,
                1 => bad.entities[0].components[0].spline = Some(w::tests::spline_fixture()),
                2 => bad.entities[0].components[1]
                    .spline
                    .as_mut()
                    .unwrap()
                    .position
                    .points
                    .clear(),
                3 => bad.entities[0].components[1]
                    .spline
                    .as_mut()
                    .unwrap()
                    .rotation
                    .points
                    .clear(),
                4 => bad.entities[0].components[1]
                    .spline
                    .as_mut()
                    .unwrap()
                    .scale
                    .points
                    .clear(),
                5 => bad.entities[0].components[1]
                    .spline
                    .as_mut()
                    .unwrap()
                    .reparam
                    .points
                    .clear(),
                6 => bad.entities[0].reference.incarnation += 1,
                7 => bad.entities[0].components[2].spring_arm = None,
                8 => {
                    bad.entities[0].components[0].spring_arm =
                        frame.entities[0].components[2].spring_arm.clone()
                }
                _ => {
                    bad.entities[0].components[2]
                        .spring_arm
                        .as_mut()
                        .unwrap()
                        .translation[0] = f64::NAN
                }
            }
            assert!(
                bridge.publish_render(bad).is_err(),
                "accepted incomplete profile {change}"
            );
            assert!(bridge.accept_mirror(9001, receipt.clone()).is_err());
            assert!(!bridge.mirror_ready(9001));
        }
        bridge.publish_render(frame.clone()).unwrap();
        bridge.accept_mirror(9001, receipt.clone()).unwrap();
        assert!(bridge.mirror_ready(9001));
        let mut newer = descriptor;
        newer.revision += 1;
        newer.recipe.components[1]
            .spline_profile
            .as_mut()
            .unwrap()
            .position_count += 1;
        bridge.publish_descriptor(newer).unwrap();
        assert!(!bridge.mirror_ready(9001));
        assert!(bridge.publish_render(frame).is_err());
        assert!(bridge.accept_mirror(9001, receipt).is_err());
    }
    #[test]
    fn native_descriptor_scene_and_ready_reject_stale_incarnations_and_missing_bones() {
        let bridge = Arc::new(Bridge::default());
        let reference = w::EntityRef {
            epoch: 19,
            id: 1,
            incarnation: 2,
        };
        let mut directory = Directory {
            epoch: 19,
            seq: 3,
            state: w::READY,
            arena: "Map_Arena_Yard".into(),
            error: String::new(),
            entities: vec![w::Entity {
                reference,
                owner_peer: 9001,
                slot: 0,
                kind: w::HUMAN,
                controller: 0,
                team: None,
            }],
        };
        bridge.set_directory(directory.clone());
        let recipe = crate::native_descriptor::fixture_recipe();
        let descriptor = w::Descriptor {
            reference,
            slot: 0,
            directory_seq: 3,
            revision: 4,
            source_frame_seq: 1,
            recipe,
        };
        bridge.publish_descriptor(descriptor.clone()).unwrap();
        let transform = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let components = descriptor
            .recipe
            .components
            .iter()
            .map(|c| w::RenderComponent {
                id: c.id,
                transform,
                bones: vec![transform; c.bones.len()],
                morphs: c.morphs.iter().map(|m| m.value).collect(),
                materials: c
                    .materials
                    .iter()
                    .map(|m| w::RenderMaterial {
                        scalars: m.scalars.iter().map(|p| p.value).collect(),
                        vectors: m.vectors.iter().map(|p| p.value).collect(),
                        textures: m.textures.iter().map(|p| p.value.clone()).collect(),
                    })
                    .collect(),
                spline: None,
                spring_arm: None,
            })
            .collect();
        let mut frame = w::RenderWorld {
            world: fixture_world(&directory, 1, false),
            entities: vec![w::RenderEntity {
                reference,
                revision: 4,
                components,
            }],
        };
        bridge.publish_render(frame.clone()).unwrap();
        let receipt = w::MirrorReady {
            epoch: 19,
            directory_seq: 3,
            frame_seq: 1,
            entities: vec![(reference, 4)],
        };
        bridge.accept_mirror(9001, receipt.clone()).unwrap();
        assert!(bridge.mirror_ready(9001));
        frame.world.frame_seq = 2;
        bridge.publish_render(frame.clone()).unwrap();
        bridge.accept_mirror(9001, receipt.clone()).unwrap(); // original applied frame, no arrival restamp
        let mut wrong = receipt.clone();
        wrong.entities[0].0.incarnation += 1;
        assert!(bridge.accept_mirror(9001, wrong).is_err());
        assert!(bridge.accept_mirror(9002, receipt).is_err());
        frame.world.frame_seq = 3;
        frame.entities[0].components[0].bones.pop();
        assert!(bridge.publish_render(frame).is_err());
        directory.seq += 1;
        directory.entities[0].reference.incarnation += 1;
        bridge.set_directory(directory);
        assert!(!bridge.mirror_ready(9001));
        assert!(bridge.lock().render.is_none());
        assert!(bridge.publish_descriptor(descriptor).is_err());
    }
    #[test]
    fn native_render_multipart_reaches_a_presentation_client_over_authenticated_udp() {
        let dir = std::env::temp_dir().join(format!(
            "hsmp-native-scene-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        {
            let host = HostHandle::start_with_parent_mode(
                "127.0.0.1:0".parse().unwrap(),
                &dir.join("host"),
                "Map_Arena_Yard",
                None,
                crate::native_mode::Mode::Diagnostic,
            )
            .unwrap();
            host.status(w::READY, "").unwrap();
            let client =
                ClientHandle::start_presentation(host.address, &dir.join("client"), "Scene", None)
                    .unwrap();
            wait_for(|| {
                client.connected()
                    && host.directory().is_some_and(|d| {
                        d.entities.iter().any(|e| e.owner_peer == client.peer_id())
                    })
            });
            let directory = host.directory().unwrap();
            wait_for(|| client.directory().is_some_and(|d| d.seq == directory.seq));
            let mut recipe = crate::native_descriptor::fixture_recipe();
            recipe.components[0].bones = (0..512)
                .map(|i| crate::native_descriptor::Bone {
                    name: format!("Bone{i}"),
                    parent: if i == 0 { -1 } else { 0 },
                })
                .collect();
            let mut arm = recipe.components[0].clone();
            arm.id = 2;
            arm.name = "Offline UDP SpringArm".into();
            arm.role = "anchor".into();
            arm.component_class = "/Script/Engine.SpringArmComponent".into();
            arm.kind = crate::native_descriptor::ComponentKind::Scene;
            arm.geometry = crate::native_descriptor::Geometry::NotApplicable;
            arm.scene = crate::native_descriptor::SceneEvidence::SpringArm {
                draw_debug_lag_markers: false,
                socket_name: "OfflineExactSocket".into(),
            };
            arm.vertex_state = crate::native_descriptor::VertexState::NotApplicable;
            arm.collision = None;
            arm.spline_profile = None;
            arm.asset.clear();
            arm.skeleton.clear();
            arm.physics_asset.clear();
            arm.bones.clear();
            arm.materials.clear();
            arm.morphs.clear();
            arm.hidden_bones.clear();
            arm.groom.clear();
            arm.vertex_colors.clear();
            arm.deformer.clear();
            arm.cloth = false;
            arm.parent = 0;
            arm.socket.clear();
            recipe.components.push(arm);
            for entity in &directory.entities {
                host.publish_descriptor(w::Descriptor {
                    reference: entity.reference,
                    slot: entity.slot,
                    directory_seq: directory.seq,
                    revision: 1,
                    source_frame_seq: 1,
                    recipe: recipe.clone(),
                })
                .unwrap();
            }
            let transform = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
            let frame = w::RenderWorld {
                world: fixture_world(&directory, 1, false),
                entities: directory
                    .entities
                    .iter()
                    .map(|e| w::RenderEntity {
                        reference: e.reference,
                        revision: 1,
                        components: recipe
                            .components
                            .iter()
                            .map(|c| w::RenderComponent {
                                id: c.id,
                                transform,
                                bones: vec![transform; c.bones.len()],
                                morphs: c.morphs.iter().map(|m| m.value).collect(),
                                materials: c
                                    .materials
                                    .iter()
                                    .map(|m| w::RenderMaterial {
                                        scalars: m.scalars.iter().map(|p| p.value).collect(),
                                        vectors: m.vectors.iter().map(|p| p.value).collect(),
                                        textures: m
                                            .textures
                                            .iter()
                                            .map(|p| p.value.clone())
                                            .collect(),
                                    })
                                    .collect(),
                                spline: None,
                                spring_arm: matches!(
                                    c.scene,
                                    crate::native_descriptor::SceneEvidence::SpringArm { .. }
                                )
                                .then_some(
                                    w::NativeSpringArmFrame {
                                        translation: [-0.0, -240.25, 20.125],
                                        rotation: [-0.0, 0.25, -0.5, 0.75],
                                    },
                                ),
                            })
                            .collect(),
                    })
                    .collect(),
            };
            assert_eq!(w::encode_render_world_v3(&frame), Err("render world bound"));
            let started = Instant::now();
            host.publish_render(frame.clone()).unwrap();
            wait_for(|| client.bridge.lock().render.is_some());
            assert_eq!(**client.bridge.lock().render.as_ref().unwrap(), frame);
            let recipes = Bridge::recipes(&client.bridge.lock()).unwrap();
            let encoded = w::scene_stream::encode_scene(&frame, &recipes).unwrap();
            assert!(encoded.len() > 100 * 1024);
            assert_eq!(
                w::scene_stream::encode_scene(
                    client.bridge.lock().render.as_ref().unwrap(),
                    &recipes
                )
                .unwrap(),
                encoded,
                "native endpoint bits survive authenticated delivery"
            );
            eprintln!(
                "offline_udp_scene_bytes={} assembly_ms={}",
                encoded.len(),
                started.elapsed().as_millis()
            );
            assert!(client.connected());
            assert!(client.error().is_empty());
        }
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(canonical.starts_with(root));
        std::fs::remove_dir_all(canonical).unwrap();
    }
    #[test]
    fn real_udp_two_players_native_snapshot_and_held_release() {
        let dir = std::env::temp_dir().join(format!(
            "hsmp-embedded-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let host = HostHandle::start_with_parent_mode(
            "127.0.0.1:0".parse().unwrap(),
            &dir.join("host"),
            "Map_Arena_Yard",
            None,
            crate::native_mode::Mode::Diagnostic,
        )
        .unwrap();
        host.status(w::READY, "").unwrap();
        let a = ClientHandle::start(host.address, &dir.join("a"), "A", None).unwrap();
        let b = ClientHandle::start(host.address, &dir.join("b"), "B", None).unwrap();
        wait_for(|| {
            host.directory().is_some_and(|d| {
                d.state == w::READY
                    && d.entities
                        .iter()
                        .filter(|e| e.kind == w::HUMAN && e.owner_peer != 0)
                        .count()
                        == 2
            })
        });
        let directory = host.directory().unwrap();
        wait_for(|| {
            a.directory().is_some_and(|d| d.seq == directory.seq)
                && b.directory().is_some_and(|d| d.seq == directory.seq)
        });
        let mine = directory
            .entities
            .iter()
            .find(|e| e.owner_peer == a.peer_id())
            .unwrap()
            .reference;
        let other = directory
            .entities
            .iter()
            .find(|e| e.owner_peer == b.peer_id())
            .unwrap()
            .reference;
        assert!(a
            .input(InputFrame {
                reference: other,
                seq: 1,
                buttons: 1,
                ..Default::default()
            })
            .is_err());
        a.input(InputFrame {
            reference: mine,
            seq: 1,
            buttons: 1,
            ..Default::default()
        })
        .unwrap();
        let mut saw_input = false;
        wait_for(|| {
            saw_input |= host
                .inputs(32)
                .iter()
                .any(|i| i.reference == mine && i.buttons == 1 && i.delivery_seq == 1);
            saw_input
        });
        let mut released = false;
        wait_for(|| {
            released |= host.inputs(32).iter().any(|i| {
                i.reference == mine
                    && i.flags == w::RELEASE_ALL
                    && i.buttons == 0
                    && i.delivery_seq == 2
            });
            released
        });
        host.publish_world(fixture_world(&directory, 1, false))
            .unwrap();
        wait_for(|| {
            a.snapshot().is_some_and(|w| w.frame_seq == 1)
                && b.snapshot().is_some_and(|w| w.frame_seq == 1)
        });
        assert_eq!(a.snapshot().unwrap(), b.snapshot().unwrap());
        // The same authenticated identity resumes on a fresh socket, retaining its player seat
        // while rotating the actor capability. The old actor capability cannot control it.
        let resumed = ClientHandle::start(host.address, &dir.join("a"), "A", None).unwrap();
        wait_for(|| {
            resumed.directory().is_some_and(|d| {
                d.entities.iter().any(|e| {
                    e.owner_peer == resumed.peer_id()
                        && e.reference.id == mine.id
                        && e.reference.incarnation > mine.incarnation
                })
            })
        });
        let rebound = resumed.directory().unwrap();
        assert_eq!(
            resumed.peer_id(),
            directory
                .entities
                .iter()
                .find(|e| e.reference == mine)
                .unwrap()
                .owner_peer
        );
        assert!(resumed
            .input(InputFrame {
                reference: mine,
                seq: 2,
                buttons: 1,
                ..Default::default()
            })
            .is_err());
        assert!(host
            .publish_world(fixture_world(&directory, 2, true))
            .is_err());
        let own = rebound
            .entities
            .iter()
            .find(|e| e.owner_peer == resumed.peer_id())
            .unwrap()
            .reference;
        resumed
            .input(InputFrame {
                reference: own,
                seq: 1,
                buttons: 2,
                ..Default::default()
            })
            .unwrap();
        let mut fresh = false;
        wait_for(|| {
            fresh |= host
                .inputs(32)
                .iter()
                .any(|i| i.reference == own && i.buttons == 2 && i.delivery_seq == 1);
            fresh
        });
        host.publish_world(fixture_world(&rebound, 3, true))
            .unwrap();
        wait_for(|| {
            resumed.snapshot().is_some_and(|w| w.frame_seq == 3)
                && b.snapshot().is_some_and(|w| w.frame_seq == 3)
        });
        assert_eq!(
            resumed.directory().unwrap().state,
            w::LIVE,
            "snapshot health cannot substitute for a native source phase"
        );
        host.status(w::VICTORY, "").unwrap();
        wait_for(|| {
            resumed.directory().is_some_and(|d| d.state == w::VICTORY)
                && b.directory().is_some_and(|d| d.state == w::VICTORY)
        });
        drop(a);
        drop(resumed);
        drop(b);
        drop(host);
        // A single test-owned temp directory, validated against the temp root before cleanup.
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(canonical.starts_with(root));
        std::fs::remove_dir_all(canonical).unwrap();
    }
    #[test]
    fn full_queue_is_bounded_and_stale_inputs_never_replay_delta() {
        let bridge = Arc::new(Bridge::default());
        let frame = InputFrame {
            reference: w::EntityRef {
                epoch: 1,
                id: 1,
                incarnation: 1,
            },
            seq: 1,
            axes: [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        for _ in 0..w::MAX_INPUTS {
            assert!(bridge.push_input(frame));
        }
        assert!(!bridge.push_input(frame));
        let mut s = bridge.lock();
        for (_, at) in &mut s.inputs {
            *at = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        }
        assert_eq!(s.inputs.len(), w::MAX_INPUTS);
        drop(s);
        assert!(bridge.take_inputs(32).is_empty());
        let release = InputFrame {
            flags: w::RELEASE_ALL,
            buttons: 0,
            axes: [0.0; 8],
            ..frame
        };
        assert!(bridge.push_input(release));
        bridge.lock().inputs[0].1 = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        assert_eq!(bridge.take_inputs(32), vec![release]);
    }
}

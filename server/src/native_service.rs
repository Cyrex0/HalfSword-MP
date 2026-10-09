//! Embedded network services. Bounded immutable DTOs cross the game/network thread boundary.
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
    error: String,
    world_reset: bool,
    descriptors: HashMap<u32, Arc<w::Descriptor>>,
    descriptor_out: VecDeque<w::Descriptor>,
    render: Option<Arc<w::RenderWorld>>,
    render_out: Option<w::RenderWorld>,
    control_out: VecDeque<w::MirrorReady>,
    mirror_receipts: HashMap<u32, w::MirrorReady>,
    render_received: Option<Instant>,
    source_teams: HashMap<w::EntityRef, i32>,
}
#[derive(Clone)]
pub struct Scene {
    pub directory: Directory,
    pub descriptors: Vec<Arc<w::Descriptor>>,
    pub frame: Arc<w::RenderWorld>,
    pub peer_id: u32,
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
            s.descriptor_out.clear();
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
        s.descriptor_out.clear();
        s.mirror_receipts.clear();
        s.control_out.clear();
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
        s.descriptor_out
            .retain(|old| old.reference.id != d.reference.id);
        s.descriptor_out.push_back(d.clone());
        s.descriptors.insert(d.reference.id, Arc::new(d));
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
    }
    pub(crate) fn take_descriptors(&self) -> Vec<w::Descriptor> {
        self.lock().descriptor_out.drain(..).collect()
    }
    pub(crate) fn take_source_teams(&self) -> HashMap<w::EntityRef, i32> {
        std::mem::take(&mut self.lock().source_teams)
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
        w::encode_render_world_v2(&frame)?;
        let mut s = self.lock();
        if s.world_reset || !Self::render_matches(&s, &frame) {
            return Err("render descriptor generation");
        }
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
                | hsmp_net::net::caps::NATIVE_RENDER_V2,
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
    pub fn publish_descriptor(&self, d: w::Descriptor) -> Result<(), &'static str> {
        self.bridge.publish_descriptor(d)
    }
    pub fn source_team(&self, reference: w::EntityRef, team: i32) -> Result<(), &'static str> {
        let mut s = self.bridge.lock();
        if !s
            .directory
            .as_ref()
            .is_some_and(|d| d.entities.iter().any(|e| e.reference == reference))
        {
            return Err("source team generation");
        }
        s.source_teams.insert(reference, team);
        Ok(())
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
        Self::start_role(server, identity_dir, nick, pinned_key, false)
    }
    pub fn start_presentation(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::start_role(server, identity_dir, nick, pinned_key, true)
    }
    fn start_role(
        server: SocketAddr,
        identity_dir: &Path,
        nick: &str,
        pinned_key: Option<[u8; 32]>,
        presentation: bool,
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
        if presentation {
            cfg.caps |=
                hsmp_net::net::caps::NATIVE_PRESENTATION | hsmp_net::net::caps::NATIVE_RENDER_V2;
        }
        let bridge = Arc::new(Bridge::default());
        let network_bridge = bridge.clone();
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
    pub fn error(&self) -> String {
        self.bridge.lock().error.clone()
    }
    /// One immutable read; the consumer never mixes independent generation reads.
    pub fn scene(&self) -> Option<Scene> {
        let s = self.bridge.lock();
        if !s.connected || s.render_received?.elapsed().as_millis() >= w::INPUT_TIMEOUT_MS as u128 {
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
        })
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
        let (clock, captured) = s.server_clock.ok_or("no server clock")?;
        frame.sample_ms = clock.saturating_add(captured.elapsed().as_millis() as u64);
        drop(s);
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
        let receipts = {
            let mut s = bridge.lock();
            s.control_out.drain(..).collect::<Vec<_>>()
        };
        for receipt in receipts {
            if let Ok(payload) = w::encode_mirror_ready(&receipt) {
                let _ = client.send(
                    SendMode::Ordered,
                    hsmp_ipc::wire::message(w::K_MIRROR_READY, 0, 0, &payload),
                );
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
                    if caps & hsmp_net::net::caps::NATIVE_WORLD == 0 {
                        bridge.lock().error = "server has no native authority".into();
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
                    cfg.pinned_server_key = Some(server_key);
                    let mut s = bridge.lock();
                    s.connected = true;
                    s.error.clear();
                }
                ClientEvent::Message(d) => {
                    if let Ok((h, payload)) = hsmp_ipc::wire::split(&d.data) {
                        match h.kind {
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
                            w::K_DESCRIPTOR => {
                                if let Ok(d) = w::decode_descriptor(payload) {
                                    if let Err(e) = bridge.publish_descriptor(d) {
                                        bridge.lock().error = e.into();
                                    }
                                }
                            }
                            w::K_RENDER_WORLD => {
                                let mut s = bridge.lock();
                                s.error = "server sent an incompatible native scene frame".into();
                                s.connected = false;
                                s.render = None;
                                s.render_received = None;
                                return;
                            }
                            w::K_RENDER_WORLD_V2 => {
                                if let Ok(frame) = w::decode_render_world_v2(payload) {
                                    let _ = bridge.publish_render(frame);
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
                    s.render = None;
                    s.control_out.clear();
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
                    s.render = None;
                    s.control_out.clear();
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
mod tests {
    use super::*;
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
    fn fixture_world(d: &Directory, seq: u32, ai_dead: bool) -> World {
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
    fn native_spline_readiness_requires_the_complete_current_descriptor_profile() {
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
        for change in 0..7 {
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
                _ => bad.entities[0].reference.incarnation += 1,
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
    fn native_render_v2_reaches_a_presentation_client_over_authenticated_udp() {
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
            let recipe = crate::native_descriptor::fixture_recipe();
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
                            })
                            .collect(),
                    })
                    .collect(),
            };
            host.publish_render(frame.clone()).unwrap();
            wait_for(|| client.bridge.lock().render.is_some());
            assert_eq!(**client.bridge.lock().render.as_ref().unwrap(), frame);
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

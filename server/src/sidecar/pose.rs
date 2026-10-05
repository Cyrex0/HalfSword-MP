//! Pose: remote peer playback buffers and the `PeerPlay` slot writer, the relayed `root` /
//! `pose` records (into poseplay and the `peer_root` slot, copied as they are), the sent-frame
//! report and the latency report task. The local root / weapon / pose records are framed by
//! the `hsmp-ipc` thread (ipc_shm.rs `hot_record`) without a decode.
//!
//! See docs/development/server-modules.md.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Milliseconds elapsed since `t` (0 if `t` is in the future).
fn ms_since(t: SystemTime) -> f64 {
    SystemTime::now().duration_since(t).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// How often each remote peer's interpolated pose is written to its `PeerPlay` slot. The
/// game reads it every rendered frame, so this bounds the staleness of what it applies
/// (avg 1 ms).
const PLAY_WRITE: Duration = Duration::from_millis(2);

/// Monotonic local ms (receive stamps + playback clock for poseplay).
pub(super) fn mono_ms() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Per-remote-peer playback buffers (fed by the UDP receive path, sampled by
/// the play-writer task). std Mutex: critical sections are a few µs.
fn playbacks() -> &'static std::sync::Mutex<HashMap<u32, PeerPlay>> {
    static P: std::sync::OnceLock<std::sync::Mutex<HashMap<u32, PeerPlay>>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub(super) fn with_play<R>(f: impl FnOnce(&mut HashMap<u32, PeerPlay>) -> R) -> R {
    let mut g = playbacks().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

/// The game's requested look-ahead (`PoseLead` slot, ms: HSMPAvatars' last physics step +
/// the predicted next one), clamped; 0 without the pose link.
fn pose_lead() -> f64 {
    match super::ipc_shm::pose_link() {
        Some(l) => {
            let v = l.pose_lead();
            if v.is_finite() { v.clamp(0.0, poseplay::LEAD_MAX_MS) } else { 0.0 }
        }
        None => 0.0,
    }
}

/// HSMP-SHM: one peer's playback sample as the typed `PeerPlay` slot (the same fields as
/// the play line; `b` carries every slot, `mask` / `vmask` say which are valid).
pub(super) fn peer_play_of(peer_id: u32, seq: u64, s: &poseplay::Sample) -> hsmp_ipc::schema::pose::PeerPlay {
    use hsmp_ipc::schema::pose::{PeerPlay, PlayWeapon, PEER_PLAY_HAS_CONTROL, PEER_PLAY_HAS_ROOT, PEER_PLAY_V2};
    let mut p = PeerPlay::default();
    p.peer_id = peer_id;
    p.play_seq = seq;
    p.mode = match s.mode { poseplay::Mode::Interp => 0, poseplay::Mode::Extrap => 1, poseplay::Mode::Hold => 2, poseplay::Mode::Stale => 3 };
    p.cut = s.cut;
    p.pt = s.pt.max(0.0);
    p.age = if s.age.is_finite() { s.age as f32 } else { -1.0 };
    p.delay = s.delay as f32;
    p.jit = s.jitter as f32;
    p.lead = s.lead as f32;
    p.iv = s.interval as f32;
    p.rate = if s.rate.is_finite() { s.rate as f32 } else { 1.0 };
    p.mask = s.mask;
    p.vmask = s.vmask;
    let mut flags = 0;
    if s.v2 { flags |= PEER_PLAY_V2; }
    if let Some((r, yaw)) = s.root {
        flags |= PEER_PLAY_HAS_ROOT;
        p.root = [r[0], r[1], r[2], yaw];
    }
    for i in 0..poseplay::SLOTS.min(p.b.len()) {
        p.b[i][..7].copy_from_slice(&s.bones[i]);
        p.b[i][7..].copy_from_slice(&s.vel[i]);
    }
    if let Some(e) = &s.extra {
        if let Some(c)=e.context {
            p.match_id=c.match_id; p.round=c.round; p.life=c.life; p.has_context=true.into();
        }
        p.k = e.k;
        p.st = e.step as f32;
        for (k, w) in e.weapons.iter().enumerate() {
            if s.mask & (1 << (poseplay::WPN_R + k)) == 0 { continue; }
            if let Some((hands, id, base, tip)) = w {
                p.w[k] = PlayWeapon { present: 1, hands: *hands as u32, class_id: *id as u32, _r: 0, base: *base, tip: *tip };
            }
        }
        if let Some(c) = &e.control {
            flags |= PEER_PLAY_HAS_CONTROL;
            let cc = &mut p.control;
            cc.flags = c.flags;
            cc.grip_r = c.grip_r as u32;
            cc.grip_l = c.grip_l as u32;
            cc.scalars = c.scalars;
            cc.aim = c.aim;
            cc.ctrl_pitch = c.ctrl_pitch;
            cc.ctrl_yaw = c.ctrl_yaw;
            cc.ik = c.ik;
            cc.ik_world = c.ik_world.iter().enumerate().fold(0u32, |a, (i, w)| a | ((*w as u32) << i));
        }
    }
    p.flags = flags;
    p
}

/// The play writer (the `hsmp-poseplay` thread is the single writer of the `PeerPlay`
/// slots): sample every peer at `now + lead`; a Stale sample is written once.
fn write_play_slots(l: &'static super::ipc_shm::ShmLink, now: f64, lead: f64) {
    let mut out: Vec<(u32, u64, poseplay::Sample)> = Vec::new();
    with_play(|m| {
        for (id, pp) in m.iter_mut() {
            let Some(s) = pp.pb.sample_lead(now, lead) else { continue };
            if s.mode == poseplay::Mode::Stale {
                if pp.stale_written { continue; }
                pp.stale_written = true;
            } else {
                pp.stale_written = false;
            }
            pp.seq += 1;
            out.push((*id, pp.seq, s));
        }
    });
    for (id, seq, s) in out {
        let t0 = std::time::Instant::now();
        let Some(slot) = l.slot_for(id) else { continue };
        let mut p = peer_play_of(id, seq, &s);
        p.meta = l.meta();
        l.write_play(slot, &p);
        let us = t0.elapsed().as_secs_f64() * 1000.0;
        with_play(|m| if let Some(pp) = m.get_mut(&id) { pp.w.add(us) });
    }
}

/// One log line per remote peer: receive rate, ordering faults, buffer
/// state, playback mode mix and PeerPlay slot write cost.
fn play_report(secs: f64) -> Vec<String> {
    with_play(|m| m.iter_mut().map(|(id, pp)| {
        let st = std::mem::take(&mut pp.pb.stats);
        let n = (st.interp + st.extrap + st.hold + st.stale).max(1) as f64;
        let pct = |x: u32| 100.0 * x as f64 / n;
        let line = format!(
            "pose peer {}: rx {:.1} Hz (accepted {} reordered {} dup {} late {} cut {} restart {}) | delay {:.0} ms jitter(p90) {:.1} ms interval {:.1} ms | interp {:.0}% extrap {:.0}% hold {:.0}% stale {:.0}% | play-slot write [{}]",
            id, pp.rx_pose as f64 / secs, st.accepted, st.reordered, st.dup, st.late, st.cuts, st.restarts,
            pp.pb.delay, pp.pb.clock.j95, pp.pb.interval(),
            pct(st.interp), pct(st.extrap), pct(st.hold), pct(st.stale), pp.w.take());
        pp.rx_pose = 0;
        line
    }).collect())
}

// Latency budget summary every 5 s (per hop, see `lat`).
pub(super) fn spawn_latency_report() {
    tokio::spawn(async {
        let mut t = time::interval(Duration::from_secs(5));
        t.tick().await;
        loop {
            t.tick().await;
            let line = lat::with(|s| format!(
                "lua->sidecar root [{}] skel [{}] | send->recv one-way [{}] | transit jitter [{}] | recv->slot [{}]",
                s.lua_root.take(), s.lua_skel.take(), s.oneway.take(), s.jitter.take(), s.write.take()));
            info!("latency ms: {}", line);
            for l in play_report(5.0) { info!("{}", l); }
            if let Some(l) = tx_report(5.0) { info!("{}", l); }
            if let Some(l) = link_report() { info!("{}", l); }
        }
    });
}

// Remote pose playback writer (see poseplay.rs): samples every peer's jitter buffer on
// the sidecar clock every PLAY_WRITE into the PeerPlay slots, on its own OS thread.
pub(super) fn spawn_play_writer() -> tokio::task::JoinHandle<()> {
    std::thread::Builder::new()
        .name("hsmp-poseplay".into())
        .spawn(move || {
            let mut next = std::time::Instant::now();
            loop {
                if let Some(l) = super::ipc_shm::pose_link() {
                    write_play_slots(l, mono_ms(), pose_lead());
                }
                next += PLAY_WRITE;
                let now = std::time::Instant::now();
                if next > now { std::thread::sleep(next - now); } else { next = now; } // skip, never burst
            }
        })
        .expect("spawn the pose play writer thread");
    tokio::spawn(std::future::pending::<()>())
}

// ---- relayed root / pose records (protocol v6) -----------------------------------------------

/// A relayed `root` record (`peer` = its owner, from the wire header), validated: into the
/// owner's playback (the capsule on the pose timeline) and, as it is, into its `peer_root`
/// slot (+ the owner id). Latency stats from its sender stamps.
pub(super) fn on_root(peer: u32, r: &hsmp_ipc::schema::pose::Root, payload: &[u8]) {
    let rxm = mono_ms();
    with_play(|m| {
        m.entry(peer).or_insert_with(PeerPlay::new).pb.push_scoped_root(
            posecodec::v2::Context{match_id:r.match_id,round:r.round,life:r.life},
            rxm,r.ts as f64,r.pos,r.vel,poseplay::quat_yaw(r.rot));
    });
    let rx = SystemTime::now();
    let rx_ms = rx.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);
    if let Some(l) = super::ipc_shm::pose_link() {
        // PeerRoot = {peer_id, _r, root}: the record bytes are copied behind the id.
        let mut rec = [0u8; core::mem::size_of::<hsmp_ipc::schema::pose::PeerRoot>()];
        rec[..4].copy_from_slice(&peer.to_le_bytes());
        rec[8..].copy_from_slice(payload);
        l.post_record("peer_root", Some(peer), hsmp_ipc::schema::pose::K_PEER_ROOT, &rec);
    }
    let write_ms = ms_since(rx);
    let (ts, send_wall_ms) = (r.ts, r.send_wall_ms);
    lat::with(|s| {
        if send_wall_ms > 0 { s.oneway.add((rx_ms - send_wall_ms as f64).max(0.0)); }
        s.write.add(write_ms);
        if let Some((pts, prx)) = s.last.get(&peer).copied() {
            let dts = ts.wrapping_sub(pts) as f64;
            let drx = rx_ms - prx;
            if dts > 0.0 && dts < 500.0 { s.jitter.add((drx - dts).abs()); }
        }
        s.last.insert(peer, (ts, rx_ms));
    });
}

/// A relayed `pose` record: the codec v2 frame, decoded ONCE, into the owner's jitter buffer;
/// `aux` = the relay interval for this sender (0 = unknown). A frame that does not decode is
/// dropped (the server refuses those too).
pub(super) fn on_pose(peer: u32, tick: u32, frame: &[u8], interval_ms: u16) -> bool {
    let rxm = mono_ms();
    let Some(f) = posecodec::v2::decode(frame) else {
        debug!(peer, len = frame.len(), "pose frame malformed; dropped");
        return false;
    };
    let fr = poseplay::Frame::from_v2(tick, &f);
    with_play(|m| {
        let pp = m.entry(peer).or_insert_with(PeerPlay::new);
        pp.rx_pose += 1;
        if interval_ms > 0 { pp.pb.set_interval_hint(interval_ms as f32); }
        pp.pb.push_pose(rxm, fr);
    });
    true
}

// ---- sent pose frames (5 s report) ----------------------------------------------------------

static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
static TX_BYTES: AtomicU64 = AtomicU64::new(0);
static TX_CTL: AtomicU64 = AtomicU64::new(0);

/// Count one `pose` record leaving for the server (`payload` = PoseHead + frame bytes). No
/// decode: the control flag is header byte 9 of the frame.
pub(super) fn note_tx(payload: &[u8]) {
    const HEAD: usize = core::mem::size_of::<hsmp_ipc::schema::pose::PoseHead>();
    let frame = payload.get(HEAD..).unwrap_or(&[]);
    TX_FRAMES.fetch_add(1, Relaxed);
    TX_BYTES.fetch_add(frame.len() as u64, Relaxed);
    if frame.get(9).is_some_and(|f| f & 1 != 0) { TX_CTL.fetch_add(1, Relaxed); }
}

fn tx_report(secs: f64) -> Option<String> {
    let n = TX_FRAMES.swap(0, Relaxed);
    let b = TX_BYTES.swap(0, Relaxed);
    let c = TX_CTL.swap(0, Relaxed);
    if n == 0 { return None; }
    Some(format!("pose v2 tx: {:.1} frames/s, {:.0} B/frame, {:.1} KB/s payload, control in {}%",
        n as f64 / secs, b as f64 / n as f64, b as f64 / secs / 1000.0, 100 * c / n))
}

/// The server link over the last report interval: RTT and loss (the lines bug reports read).
fn link_report() -> Option<String> {
    static LAST: std::sync::Mutex<(u64, u64, u64)> = std::sync::Mutex::new((0, 0, 0));
    let s = super::net::conn_stats()?;
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let (sent0, lost0, re0) = *last;
    // a new connection restarts the counters
    let (ds, dl, dr) = if s.pkts_sent >= sent0 { (s.pkts_sent - sent0, s.pkts_lost.saturating_sub(lost0), s.retransmits.saturating_sub(re0)) } else { (s.pkts_sent, s.pkts_lost, s.retransmits) };
    *last = (s.pkts_sent, s.pkts_lost, s.retransmits);
    let loss = if ds > 0 { 100.0 * dl as f64 / ds as f64 } else { 0.0 };
    Some(format!(
        "link: rtt {:.0} ms (min {:.0}, var {:.0}) | loss {:.2}% ({dl}/{ds} pkts, {dr} retransmits) | rx {} pkts | total lost {} spurious {} | pacing {:.0} KB/s",
        s.srtt_ms, s.min_rtt_ms, s.rttvar_ms, loss, s.pkts_recv, s.pkts_lost, s.spurious_lost, s.cc_rate_bps / 1024.0
    ))
}

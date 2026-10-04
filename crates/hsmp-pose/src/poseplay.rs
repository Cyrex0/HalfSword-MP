//! Receive-side pose playback for remote characters (sidecar only).
//!
//! Every remote peer's pose frames (16 hero bones + weapon, sender game-clock
//! `ts`) and root snapshots land here as they arrive from UDP. The sidecar
//! samples the buffer every few ms on its own clock and writes the result to its
//! pose_play record; HSMPAvatars just applies the newest sample each game
//! frame. Doing the timing work here instead of in Lua means:
//!   * exact arrival times (no game-frame quantisation of the jitter estimate);
//!   * every frame is used, not just the newest one present at each game read;
//!   * the logic is unit-tested.
//!
//! Pipeline per peer:
//!   push_pose/push_root  -> dedup (tick/ts), reorder (sorted insert), late
//!                           drop (already played), discontinuity "cut"
//!                           (teleport / respawn / round reset) and sender
//!                           restart detection.
//!   Clock                -> sender->local offset = min(rx - ts) over a 2 s
//!                           window (tracks drift); jitter = p90 above min (JITTER_QUANTILE).
//!   delay                -> interval + p90 jitter + margin, clamped; grows
//!                           fast, shrinks slowly.
//!   playback clock pt    -> advances with real time, slewed at most ±10 % to
//!                           follow now - offset - delay (no pops); snaps only
//!                           on a >250 ms error.
//!   sample(pt)           -> Hermite (C1, tangents from neighbours) for
//!                           positions, nlerp for rotations; bounded, damped
//!                           extrapolation (≤ EXTRAP_MS, ≤ EXTRAP_MAX_UU) on
//!                           loss; hold; stale after STALE_MS.

#![allow(dead_code)]

use crate::posecodec::{self, v2, PoseFrame};
use std::collections::VecDeque;
use std::sync::Arc;

/// Playback slots: the 23 codec-v2 bones, then the right / left weapon.
/// v1 frames (16 hero bones + "Weapon") are mapped onto the same slots.
pub const SLOTS: usize = v2::NB + 2;
pub const WPN_R: usize = v2::NB;
pub const WPN_L: usize = v2::NB + 1;
const PELVIS: usize = 0;
pub fn slot_name(i: usize) -> &'static str {
    if i < v2::NB { v2::BONES[i] } else if i == WPN_R { "weapon_r" } else { "weapon_l" }
}

/// Per-frame data that is not interpolated (taken from the frame at or
/// before the playback time): weapon geometry and the control layer.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// Per weapon slot (R, L): (hands, class id, blade base, blade tip) in weapon space.
    pub weapons: [Option<(u8, u8, [f32; 3], [f32; 3])>; 2],
    pub control: Option<v2::Control>,
    pub k: f32,
    /// The sender's frame-start stamp when the frame carries its physics
    /// step (Frame::ts is then the time the bodies stand for, ts + step):
    /// the clock offset / jitter estimate uses this, whose arrival does not
    /// swing with the frame length.
    pub clock_ts: Option<f64>,
    /// That step (ms; 0 = none): the game subtracts it from the label it
    /// reports to lag comp, which keeps the plain frame-start stamps.
    pub step: f64,
}

// ---- tunables (documented in docs/development/subsystems/replication.md) ---
/// Delay floor / ceiling (ms).
pub const DELAY_MIN_MS: f64 = 20.0;
pub const DELAY_MAX_MS: f64 = 250.0;
/// Added on top of interval + p95 jitter (covers the sidecar write cadence
/// and the game's per-frame read).
pub const DELAY_MARGIN_MS: f64 = 6.0;
/// Codec v2 (velocities known): floor and margin of the buffer (ms).
pub const DELAY_MIN_V2_MS: f64 = 16.0;
pub const DELAY_MARGIN_V2_MS: f64 = 6.0;
/// Codec v2: extrapolate at full sender velocity this long before easing out.
pub const EXTRAP_FULL_MS: f64 = 34.0;
/// Codec v2: sender-velocity extrapolation is accurate for about one 60 Hz
/// frame. A longer received frame interval (relay decimation to 30 / 15 /
/// 7.5 Hz) is covered by buffer instead: the v2 delay adds
/// `interval - V2_EXTRAP_OK_MS`, so the playback point stays inside a pair of
/// received frames and interpolates with the sender's exact tangents.
pub const V2_EXTRAP_OK_MS: f64 = 17.0;
/// ... but the interval part alone never lifts the buffer past this (ms).
pub const V2_INTERVAL_DELAY_CAP_MS: f64 = 66.0;
/// A measured frame spacing up to this (ms, ~18 fps) can be a slow sender; above it, decimation.
pub const V2_SENDER_FPS_MAX_IV_MS: f64 = 55.0;
/// Codec v2 frames (sender velocities = exact tangents) are joined with
/// Hermite and extrapolated up to this gap (a 7.5 Hz relay = 133 ms).
pub const HERMITE_MAX_GAP_V2_MS: f64 = 270.0;
/// Window for the clock-offset / jitter estimate.
pub const CLOCK_WINDOW_MS: f64 = 2000.0;
/// Window of the lateness quantile that sizes the buffer.
pub const JITTER_WINDOW_MS: f64 = 8000.0;
/// Quantile of arrival lateness the buffer covers; rarer spikes are ridden
/// out with extrapolation/hold instead of adding latency for everyone.
pub const JITTER_QUANTILE: f64 = 0.95;
/// Extrapolate at most this far past the newest frame, then hold.
pub const EXTRAP_MS: f64 = 100.0;
/// ... and never move a bone further than this while extrapolating (uu).
pub const EXTRAP_MAX_UU: f32 = 30.0;
/// Newest frame older than this (sender time) => stale (stop driving).
pub const STALE_MS: f64 = 1000.0;
/// Frames further apart than this are joined linearly (no Hermite overshoot).
pub const HERMITE_MAX_GAP_MS: f64 = 120.0;
/// Pelvis jump that counts as a teleport (respawn / round reset).
pub const CUT_DIST_UU: f32 = 300.0;
pub const CUT_SPEED_UUPS: f32 = 3000.0;
/// A frame this long (sender ms) after the previous one starts over (cut).
pub const GAP_CUT_MS: f64 = 600.0;
/// Playback clock: slew limit and hard re-sync threshold.
pub const SLEW_MAX: f64 = 0.10;
pub const SLEW_TIME_MS: f64 = 200.0;
pub const RESYNC_MS: f64 = 250.0;
/// Starving buffer (the playback clock ran past the newest frame): the clock
/// slows down toward STRETCH_MIN_RATE over STRETCH_FULL_MS of overrun, so a
/// late burst plays as a brief slow-down and a catch-up instead of a freeze
/// and a jump. Not beyond STRETCH_MAX_LAG_MS behind the target (then the
/// stream is gone, not late).
pub const STRETCH_MIN_RATE: f64 = 0.6;
pub const STRETCH_FULL_MS: f64 = 40.0;
pub const STRETCH_MAX_LAG_MS: f64 = 150.0;
/// Catch-up after a stretch may run this much faster than real time.
pub const SLEW_MAX_UP: f64 = 0.15;
/// The playback rate follows its wanted value with this time constant (ms):
/// no step in the shown speed.
pub const RATE_TAU_MS: f64 = 40.0;
/// Correction blending after late data (see `blend_step`).
pub const BLEND_TAU_MS: f64 = 100.0;
pub const BLEND_MAX_UU: f32 = 120.0;
/// Largest look-ahead the game may ask for (`sample_lead`), ms.
pub const LEAD_MAX_MS: f64 = 120.0;
/// Look-ahead prediction: sender acceleration is applied this long (ms), at most this hard (uu/s^2).
pub const ACCEL_EXTRAP_MS: f64 = 60.0;
pub const ACCEL_MAX: f32 = 20000.0;
/// Sender clock jumped back this far => sender restarted (the stream is
/// reset). The same threshold as lag comp's RESET_BACKSTEP_MS (at
/// 5 s a malicious 2-5 s step-back would leave stale frames playing for seconds).
pub const RESTART_BACK_MS: f64 = 2000.0;
const KEEP_FRAMES: usize = 48;

pub type Xf = [f32; 7]; // px,py,pz,qx,qy,qz,qw

/// Linear (uu/s, at the bone origin) + angular (deg/s) velocity.
pub type Vel = [f32; 6];
const IDENT: Xf = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

#[derive(Clone, Debug)]
pub struct Frame {
    pub ts: f64,
    pub tick: u32,
    pub mask: u32,
    pub b: [Xf; SLOTS],
    /// Slots with a sender-measured velocity (codec v2 bodies + weapons).
    pub vmask: u32,
    pub vel: [Vel; SLOTS],
    pub v2: bool,
    pub extra: Option<Arc<Extra>>,
}

impl Frame {
    /// v1 frame (16 hero bones + "Weapon") onto the v2 slots.
    pub fn from_pose(tick: u32, f: &PoseFrame) -> Frame {
        let mut b = [IDENT; SLOTS];
        let mut mask = 0u32;
        for (idx, p, q) in &f.bones {
            let h = *idx as usize;
            let i = if h < v2::HERO_TO_V2.len() { v2::HERO_TO_V2[h] } else if h == posecodec::WEAPON { WPN_R } else { continue };
            if p.iter().chain(q.iter()).all(|v| v.is_finite()) {
                b[i] = [p[0], p[1], p[2], q[0], q[1], q[2], q[3]];
                mask |= 1 << i;
            }
        }
        Frame { ts: f.ts as f64, tick, mask, b, vmask: 0, vel: [[0.0; 6]; SLOTS], v2: false, extra: None }
    }

    /// Full v2 state.
    pub fn from_v2(tick: u32, f: &v2::Full) -> Frame {
        let mut b = [IDENT; SLOTS];
        let mut vel = [[0.0; 6]; SLOTS];
        let (mut mask, mut vmask) = (0u32, 0u32);
        for (i, x) in f.bones.iter().enumerate() {
            b[i] = [x.p[0], x.p[1], x.p[2], x.q[0], x.q[1], x.q[2], x.q[3]];
            mask |= 1 << i;
            if v2::HAS_BODY[i] {
                vel[i] = [x.v[0], x.v[1], x.v[2], x.w[0], x.w[1], x.w[2]];
                vmask |= 1 << i;
            }
        }
        let mut extra = Extra { control: f.control.clone(), k: f.k, ..Default::default() };
        for w in &f.weapons {
            let s = if w.hands & 1 != 0 || w.hands == 0 { 0 } else { 1 };
            if extra.weapons[s].is_some() { continue; }
            let i = WPN_R + s;
            b[i] = [w.p[0], w.p[1], w.p[2], w.q[0], w.q[1], w.q[2], w.q[3]];
            vel[i] = [w.v[0], w.v[1], w.v[2], w.w[0], w.w[1], w.w[2]];
            mask |= 1 << i;
            vmask |= 1 << i;
            extra.weapons[s] = Some((w.hands, w.id, w.base, w.tip));
        }
        let step = if f.step.is_finite() && f.step > 0.0 { f.step as f64 } else { 0.0 };
        if step > 0.0 { extra.clock_ts = Some(f.ts); extra.step = step; }
        Frame { ts: f.ts + step, tick, mask, b, vmask, vel, v2: true, extra: Some(Arc::new(extra)) }
    }

    /// Any pose body (v1 or v2) as received from the wire.
    pub fn from_wire(tick: u32, buf: &[u8]) -> Option<Frame> {
        if v2::is_v2(buf) { return v2::decode(buf).map(|f| Frame::from_v2(tick, &f)); }
        posecodec::decode(buf).map(|f| Frame::from_pose(tick, &f))
    }
    fn has(&self, i: usize) -> bool { self.mask & (1 << i) != 0 }
    fn has_vel(&self, i: usize) -> bool { self.vmask & (1 << i) != 0 }
}

#[derive(Clone, Copy, Debug)]
pub struct RootSample { pub ts: f64, pub pos: [f32; 3], pub vel: [f32; 3], pub yaw: f32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode { Interp, Extrap, Hold, Stale }

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self { Mode::Interp => "interp", Mode::Extrap => "extrap", Mode::Hold => "hold", Mode::Stale => "stale" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push { Accepted, Reordered, Duplicate, Late, Cut, Restart, Bad }

#[derive(Clone, Debug)]
pub struct Sample {
    /// Sender-clock time the pose was evaluated at (playback clock + `lead`).
    pub pt: f64,
    /// How far past the playback clock the pose was evaluated (ms; 0 = `sample`).
    pub lead: f64,
    pub mode: Mode,
    /// How far the newest frame lags the sender's estimated "now" (ms).
    pub age: f64,
    pub delay: f64,
    pub jitter: f64,
    /// The frame interval the buffer is sized for (ms).
    pub interval: f64,
    /// Playback-clock rate when sampled (1 = real time); the game's own clock follows it.
    pub rate: f64,
    /// Increments on every discontinuity; the game snaps the body when it changes.
    pub cut: u32,
    pub mask: u32,
    pub bones: [Xf; SLOTS],
    pub root: Option<([f32; 3], f32)>,
    pub vmask: u32,
    pub vel: [Vel; SLOTS],
    /// The source frames were codec v2 (selects the play-record format).
    pub v2: bool,
    pub extra: Option<Arc<Extra>>,
}

/// Sender->local clock mapping from arrival times.
#[derive(Default, Debug)]
pub struct Clock {
    win: VecDeque<(f64, f64)>, // (rx, rx - ts)
    /// (rx, lateness above the offset at that moment) over JITTER_WINDOW_MS.
    late: VecDeque<(f64, f64)>,
    pub offset: Option<f64>,
    pub j95: f64,
}

impl Clock {
    pub fn observe(&mut self, rx: f64, ts: f64) {
        self.win.push_back((rx, rx - ts));
        while let Some(&(r, _)) = self.win.front() {
            if rx - r > CLOCK_WINDOW_MS && self.win.len() > 8 { self.win.pop_front(); } else { break; }
        }
        let min = self.win.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
        self.offset = Some(min);
        // Lateness is kept longer than the offset window: a 300 ms spike every few
        // seconds is a few % of it and is ridden out by the stretch, while steady
        // jitter sets the buffer. The offset itself still follows drift and route
        // changes within CLOCK_WINDOW_MS.
        self.late.push_back((rx, rx - ts - min));
        while let Some(&(r, _)) = self.late.front() {
            if rx - r > JITTER_WINDOW_MS && self.late.len() > 8 { self.late.pop_front(); } else { break; }
        }
        let mut d: Vec<f64> = self.late.iter().map(|x| x.1.max(0.0)).collect();
        let k = ((d.len() as f64 * JITTER_QUANTILE).ceil() as usize).clamp(1, d.len()) - 1;
        let (_, q, _) = d.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        self.j95 = *q;
    }
    pub fn reset(&mut self) { *self = Clock::default(); }
}


/// What `blend_step` showed last time.
#[derive(Clone, Debug)]
struct Blend {
    at: f64,
    lead: f64,
    newest: f64,
    mode: Mode,
    cut: u32,
    mask: u32,
    shown: [Xf; SLOTS],
    vel: [Vel; SLOTS],
    off: [[f32; 3]; SLOTS],
    qoff: [[f32; 4]; SLOTS],
}
#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub accepted: u32, pub reordered: u32, pub dup: u32, pub late: u32, pub cuts: u32,
    pub restarts: u32, pub interp: u32, pub extrap: u32, pub hold: u32, pub stale: u32,
}

#[derive(Default, Debug)]
pub struct Playback {
    pub frames: VecDeque<Frame>,
    pub roots: VecDeque<RootSample>,
    pub clock: Clock,
    pub pt: Option<f64>,
    last_now: f64,
    pub delay: f64,
    pub cut: u32,
    /// Experiments / tests: pin the buffer delay (ms) instead of adapting.
    pub fixed_delay: Option<f64>,
    /// The relay's interval (ms) for this sender (S2CSkeletalBroadcastRated);
    /// None = measure it from the frames.
    pub interval_hint: Option<f64>,
    pub stats: Stats,
    /// Current playback-clock rate (1 = real time; below 1 while the buffer starves).
    pub rate: f64,
    blend: Option<Blend>,
}

fn dist3(a: &[f32], b: &[f32]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Playback {
    pub fn new() -> Self { Playback { delay: 60.0, rate: 1.0, ..Default::default() } }

    /// The server forwards this sender's frames every `ms`.
    pub fn set_interval_hint(&mut self, ms: f32) {
        self.interval_hint = (ms.is_finite() && ms > 0.0).then_some(ms as f64);
    }

    fn reset(&mut self) {
        self.frames.clear();
        self.roots.clear();
        self.clock.reset();
        self.pt = None;
        self.blend = None;
        self.stats.restarts += 1;
    }

    /// Median spacing of the newest buffered frames (ms); 1000/60 until known.
    /// Only the last 9 frames count, so a relay rate change shows
    /// within a few frames.
    pub fn measured_interval(&self) -> f64 {
        let n = self.frames.len();
        let from = n.saturating_sub(9);
        let mut d: Vec<f64> = self.frames.iter().skip(from).zip(self.frames.iter().skip(from + 1))
            .map(|(a, b)| b.ts - a.ts).filter(|x| *x > 0.0).collect();
        if d.is_empty() { return 1000.0 / 60.0; }
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        d[d.len() / 2].clamp(4.0, 200.0)
    }

    /// The interval new frames arrive at: the relay's hint when it sent one
    /// (S2CSkeletalBroadcastRated), else the measured spacing.
    pub fn interval(&self) -> f64 {
        match self.interval_hint {
            Some(h) => h.clamp(4.0, 250.0),
            None => self.measured_interval(),
        }
    }

    fn update_delay(&mut self) {
        if let Some(d) = self.fixed_delay { self.delay = d; return; }
        // v2 frames carry the sender's velocities, so a frame that has not
        // arrived yet is extrapolated accurately for a frame or two: the
        // buffer only has to cover arrival jitter, not a whole interval.
        let v2 = self.frames.back().map(|f| f.v2).unwrap_or(false);
        let target = if v2 {
            // A decimated stream (interval > one 60 Hz frame) also
            // buffers the part of the interval extrapolation cannot cover.
            // The interval part is bounded so that it never takes the buffer
            // past V2_INTERVAL_DELAY_CAP_MS on its own: a sender running at
            // 20-35 fps (two games on one GPU, LordsHall) has 30-50 ms
            // frame spacing, and the full extra puts the display latency over
            // the pose latency budget; the game's look-ahead (`.pose_lead`) and
            // extrapolation cover the rest. Jitter is never capped.
            // (Relay decimation - a hint, or a spacing no frame rate explains,
            // 15 Hz and below - keeps the full extra.)
            let base = self.clock.j95 + DELAY_MARGIN_V2_MS;
            let iv = self.interval();
            let mut extra = (iv - V2_EXTRAP_OK_MS).max(0.0);
            if self.interval_hint.is_none() && iv <= V2_SENDER_FPS_MAX_IV_MS {
                extra = extra.min((V2_INTERVAL_DELAY_CAP_MS - base).max(0.0));
            }
            (base + extra).clamp(DELAY_MIN_V2_MS, DELAY_MAX_MS)
        } else {
            (self.interval() + self.clock.j95 + DELAY_MARGIN_MS).clamp(DELAY_MIN_MS, DELAY_MAX_MS)
        };
        // Grow fast (stop extrapolating soon), shrink slowly (no oscillation).
        let a = if target > self.delay { 0.3 } else { 0.02 };
        self.delay += (target - self.delay) * a;
    }

    fn played(&self) -> f64 { self.pt.unwrap_or(f64::NEG_INFINITY) }

    pub fn push_pose(&mut self, rx: f64, frame: Frame) -> Push {
        if frame.mask & 1 == 0 || !frame.ts.is_finite() { return Push::Bad; }
        if let Some(last) = self.frames.back() {
            if frame.ts < last.ts - RESTART_BACK_MS { self.reset(); }
        }
        if self.frames.iter().any(|f| f.tick == frame.tick || f.ts == frame.ts) {
            self.stats.dup += 1;
            return Push::Duplicate;
        }
        // A frame stamped with its physics step keeps the clock on its
        // frame-start stamp (Extra::clock_ts).
        let cts = frame.extra.as_ref().and_then(|e| e.clock_ts).unwrap_or(frame.ts);
        self.clock.observe(rx, cts);
        if frame.ts <= self.played() && !self.frames.is_empty() {
            self.stats.late += 1;
            return Push::Late;
        }
        let newest = self.frames.back().map(|f| f.ts).unwrap_or(f64::NEG_INFINITY);
        let res;
        if frame.ts > newest {
            // Discontinuity: a jump no body can make => drop history so we
            // never interpolate through walls, and tell the game to snap.
            // A long gap (the owner loaded a level, respawned, was paused) is a cut
            // too: never interpolate from the old pose to the new one.
            let cut = self.frames.back().map(|p| {
                let d = dist3(&p.b[PELVIS], &frame.b[PELVIS]);
                let dt = ((frame.ts - p.ts) / 1000.0).max(1e-3) as f32;
                (d > CUT_DIST_UU && d / dt > CUT_SPEED_UUPS) || frame.ts - p.ts > GAP_CUT_MS
            }).unwrap_or(false);
            if cut {
                self.frames.clear();
                self.cut += 1;
                self.stats.cuts += 1;
                // Show the new place now instead of after the buffer delay.
                self.pt = None;
                res = Push::Cut;
            } else {
                res = Push::Accepted;
            }
            self.frames.push_back(frame);
        } else {
            let pos = self.frames.iter().position(|f| f.ts > frame.ts).unwrap_or(self.frames.len());
            self.frames.insert(pos, frame);
            self.stats.reordered += 1;
            res = Push::Reordered;
        }
        while self.frames.len() > KEEP_FRAMES { self.frames.pop_front(); }
        if res != Push::Reordered { self.stats.accepted += 1; }
        self.update_delay();
        res
    }

    pub fn push_root(&mut self, rx: f64, ts: f64, pos: [f32; 3], vel: [f32; 3], yaw: f32) -> Push {
        if !ts.is_finite() || !pos.iter().chain(vel.iter()).all(|v| v.is_finite()) { return Push::Bad; }
        if let Some(last) = self.roots.back() {
            if ts < last.ts - RESTART_BACK_MS { self.roots.clear(); }
        }
        if self.roots.iter().any(|r| r.ts == ts) { return Push::Duplicate; }
        self.clock.observe(rx, ts);
        let s = RootSample { ts, pos, vel, yaw };
        let newest = self.roots.back().map(|r| r.ts).unwrap_or(f64::NEG_INFINITY);
        if ts > newest {
            if let Some(p) = self.roots.back() {
                let d = dist3(&p.pos, &pos);
                let dt = ((ts - p.ts) / 1000.0).max(1e-3) as f32;
                if (d > CUT_DIST_UU && d / dt > CUT_SPEED_UUPS) || ts - p.ts > GAP_CUT_MS { self.roots.clear(); }
            }
            self.roots.push_back(s);
        } else {
            let at = self.roots.iter().position(|r| r.ts > ts).unwrap_or(self.roots.len());
            self.roots.insert(at, s);
        }
        while self.roots.len() > KEEP_FRAMES { self.roots.pop_front(); }
        if self.frames.is_empty() { self.update_delay(); }
        Push::Accepted
    }

    /// Advance the playback clock to local time `now` (ms) and sample.
    pub fn sample(&mut self, now: f64) -> Option<Sample> { self.sample_lead(now, 0.0) }

    /// Like `sample`, but the pose is evaluated `lead` ms past the playback
    /// clock (which itself advances exactly as in `sample`). The game asks for
    /// the time its bodies will stand for after the coming physics step
    /// (`.pose_lead`): served by interpolation from the buffered frames when
    /// they reach that far, instead of the game extrapolating a sample by up
    /// to two frames (100 ms at 20 fps). `Sample::pt` is the evaluated time,
    /// `Sample::lead` the shift.

    /// Correction blending. When late frames arrive while the playback point was
    /// extrapolating or holding, the new trajectory differs from what was shown:
    /// instead of stepping to it, the difference becomes an offset that decays
    /// with BLEND_TAU_MS. A difference above BLEND_MAX_UU is shown at once.
    fn blend_step(&mut self, mut so: SampleOut, at: f64, lead: f64) -> SampleOut {
        let newest = self.frames.back().map_or(f64::NEG_INFINITY, |f| f.ts);
        let cut = self.cut;
        let prev = self.blend.take();
        if so.mode == Mode::Stale || so.mask == 0 { return so; }
        let mut off = [[0f32; 3]; SLOTS];
        let mut qoff = [[0f32, 0.0, 0.0, 1.0]; SLOTS];
        if let Some(b) = prev.filter(|b| b.cut == cut) {
            let dt = (at - b.at).max(0.0);
            let k = (-dt / BLEND_TAU_MS).exp() as f32;
            // only after real starvation: the playback clock itself (not the requested
            // look-ahead) was more than a frame past the newest frame
            let jump = b.mode != Mode::Interp && newest > b.newest && b.at - b.lead - b.newest > V2_EXTRAP_OK_MS;
            for i in 0..SLOTS {
                if so.mask & b.mask & (1 << i) == 0 { continue; }
                if jump {
                    // where the shown body was heading vs where the new data puts it
                    let s = dt as f32 / 1000.0;
                    let mut d = [0f32; 3];
                    for a in 0..3 { d[a] = b.shown[i][a] + b.vel[i][a] * s - so.bones[i][a]; }
                    if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > BLEND_MAX_UU { continue; }
                    off[i] = d;
                    let qs = [b.shown[i][3], b.shown[i][4], b.shown[i][5], b.shown[i][6]];
                    let qr = [so.bones[i][3], so.bones[i][4], so.bones[i][5], so.bones[i][6]];
                    qoff[i] = v2::qnorm(v2::qmul(qs, [-qr[0], -qr[1], -qr[2], qr[3]]));
                } else {
                    for a in 0..3 { off[i][a] = b.off[i][a] * k; }
                    qoff[i] = nlerp(&[0.0, 0.0, 0.0, 1.0], &b.qoff[i], k);
                }
            }
        }
        let raw_vel = so.vel;
        for i in 0..SLOTS {
            if so.mask & (1 << i) == 0 { continue; }
            let o = off[i];
            if o != [0.0; 3] {
                for a in 0..3 {
                    so.bones[i][a] += o[a];
                    // the decaying offset moves the body too
                    so.vel[i][a] -= o[a] * (1000.0 / BLEND_TAU_MS) as f32;
                }
            }
            if qoff[i][3] < 0.999_999 {
                let q = v2::qnorm(v2::qmul(qoff[i], [so.bones[i][3], so.bones[i][4], so.bones[i][5], so.bones[i][6]]));
                so.bones[i][3..7].copy_from_slice(&q);
            }
        }
        self.blend = Some(Blend { at, lead, newest, mode: so.mode, cut, mask: so.mask, shown: so.bones, vel: raw_vel, off, qoff });
        so
    }
    pub fn sample_lead(&mut self, now: f64, lead: f64) -> Option<Sample> {
        let offset = self.clock.offset?;
        let target = now - offset - self.delay;
        let pt = match self.pt {
            Some(pt) if (target - pt).abs() <= RESYNC_MS => {
                let dt = (now - self.last_now).clamp(0.0, 100.0);
                let mut want = 1.0 + ((target - pt) / SLEW_TIME_MS).clamp(-SLEW_MAX, SLEW_MAX_UP);
                // Starving: slow down instead of running into extrapolation and hold.
                if let Some(n) = self.frames.back().map(|f| f.ts) {
                    let over = pt - n;
                    if over > 0.0 && target - pt < STRETCH_MAX_LAG_MS {
                        let s = (over / STRETCH_FULL_MS).min(1.0);
                        want = want.min(1.0 - (1.0 - STRETCH_MIN_RATE) * s);
                    }
                }
                if !(self.rate > 0.0) { self.rate = 1.0; }
                self.rate += (want - self.rate) * (dt / RATE_TAU_MS).min(1.0);
                pt + dt * self.rate
            }
            _ => { self.rate = 1.0; target }
        };
        self.pt = Some(pt);
        self.last_now = now;
        let sender_now = now - offset;
        let lead = if lead.is_finite() { lead.clamp(0.0, LEAD_MAX_MS) } else { 0.0 };
        let at = pt + lead;

        let so = if self.frames.is_empty() {
            SampleOut { mode: Mode::Stale, mask: 0, bones: [IDENT; SLOTS], vmask: 0, vel: [[0.0; 6]; SLOTS], v2: false, extra: None }
        } else {
            let newest = self.frames.back().unwrap();
            sample_frames_lead(&self.frames, at, sender_now - newest.ts, lead)
        };
        let so = self.blend_step(so, at, lead);
        let age = self.frames.back().map(|f| sender_now - f.ts).unwrap_or(f64::INFINITY);
        match so.mode {
            Mode::Interp => self.stats.interp += 1,
            Mode::Extrap => self.stats.extrap += 1,
            Mode::Hold => self.stats.hold += 1,
            Mode::Stale => self.stats.stale += 1,
        }
        let root = sample_roots(&self.roots, at);
        if self.frames.is_empty() && root.is_none() { return None; }
        Some(Sample { pt: at, lead, mode: so.mode, age, delay: self.delay, jitter: self.clock.j95, interval: self.interval(), rate: self.rate, cut: self.cut,
                      mask: so.mask, bones: so.bones, root, vmask: so.vmask, vel: so.vel, v2: so.v2, extra: so.extra })
    }
}

// ---- interpolation ----------------------------------------------------------

/// Cubic Hermite between p0 (tangent m0) and p1 (tangent m1), s in [0,1],
/// tangents in units per ms, h = segment length in ms.
pub fn hermite(p0: f32, m0: f32, p1: f32, m1: f32, s: f32, h: f32) -> f32 {
    let s2 = s * s;
    let s3 = s2 * s;
    (2.0 * s3 - 3.0 * s2 + 1.0) * p0 + (s3 - 2.0 * s2 + s) * h * m0
        + (-2.0 * s3 + 3.0 * s2) * p1 + (s3 - s2) * h * m1
}

pub fn nlerp(a: &[f32], b: &[f32], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut q = [0f32; 4];
    for k in 0..4 { q[k] = a[k] + (b[k] * s - a[k]) * t; }
    let l = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
    if l < 1e-8 { return [0.0, 0.0, 0.0, 1.0]; }
    for c in q.iter_mut() { *c /= l; }
    q
}

/// Spherical linear interpolation (constant angular speed; exact for a
/// rotation about a fixed axis at a constant rate, which a 60 Hz sample
/// pair of a swing nearly is).
pub fn slerp(a: &[f32], b: &[f32], t: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sgn = if dot < 0.0 { dot = -dot; -1.0 } else { 1.0 };
    if dot > 0.9995 { return nlerp(a, b, t); }
    let th = dot.min(1.0).acos();
    let st = th.sin();
    let (wa, wb) = (((1.0 - t) * th).sin() / st, (t * th).sin() / st * sgn);
    let mut q = [0f32; 4];
    for k in 0..4 { q[k] = a[k] * wa + b[k] * wb; }
    let l = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
    if !(l > 1e-8) { return [0.0, 0.0, 0.0, 1.0]; }
    for c in q.iter_mut() { *c /= l; }
    q
}

/// Tangent (uu per ms) of slot `i` at frame `k`, from its neighbours,
/// limited so a sharp reversal cannot overshoot wildly.
fn tangent(fr: &VecDeque<Frame>, k: usize, i: usize, axis: usize) -> f32 {
    let n = fr.len();
    let get = |j: usize| if fr[j].has(i) { Some((fr[j].ts, fr[j].b[i][axis])) } else { None };
    let (lo, hi) = (if k > 0 { k - 1 } else { k }, if k + 1 < n { k + 1 } else { k });
    match (get(lo), get(hi)) {
        (Some((t0, v0)), Some((t1, v1))) if t1 > t0 && t1 - t0 <= 2.0 * HERMITE_MAX_GAP_MS => ((v1 - v0) as f64 / (t1 - t0)) as f32,
        _ => 0.0,
    }
}

struct SampleOut {
    mode: Mode,
    mask: u32,
    bones: [Xf; SLOTS],
    vmask: u32,
    vel: [Vel; SLOTS],
    v2: bool,
    extra: Option<Arc<Extra>>,
}

fn hold(f: &Frame, mode: Mode) -> SampleOut {
    SampleOut { mode, mask: f.mask, bones: f.b, vmask: f.vmask, vel: f.vel, v2: f.v2, extra: f.extra.clone() }
}

fn sample_frames(fr: &VecDeque<Frame>, t: f64, age: f64) -> SampleOut { sample_frames_lead(fr, t, age, 0.0) }

/// `lead` (ms, `sample_lead`): the game asked for a pose that far past the
/// playback clock. Past the newest frame that is a deliberate prediction, not
/// loss concealment: the sender velocity is kept for EXTRAP_FULL_MS + lead
/// (the ease-out starts after it) and the displacement cap grows with it.
/// Decaying it from 34 ms on makes the predicted pose lag the real motion at
/// 20-30 fps (LordsHall: aim velocity half the target's).
fn sample_frames_lead(fr: &VecDeque<Frame>, t: f64, age: f64, lead: f64) -> SampleOut {
    let n = fr.len();
    let newest = &fr[n - 1];
    if age > STALE_MS { return hold(newest, Mode::Stale); }
    if t <= fr[0].ts { return hold(&fr[0], Mode::Hold); }
    if t >= newest.ts {
        let dt = t - newest.ts;
        if n >= 2 {
            let prev = &fr[n - 2];
            let span = newest.ts - prev.ts;
            let max_gap = if newest.v2 { HERMITE_MAX_GAP_V2_MS } else { HERMITE_MAX_GAP_MS };
            if span > 0.0 && span <= max_gap {
                // Velocity decays linearly to 0 at EXTRAP_MS: displacement is
                // continuous and the motion eases out instead of stopping dead.
                // Sender velocities are used when present (v2), else the
                // last frame-to-frame motion.
                let ext = EXTRAP_MS + lead;
                let e = dt.min(ext);
                // v2: full sender velocity for EXTRAP_FULL_MS (the normal
                // case of a buffer shorter than one interval), then the
                // linear ease-out. v1: ease-out from the start.
                let full = if newest.v2 { e.min(EXTRAP_FULL_MS + lead) } else { 0.0 };
                let rest = (e - full).max(0.0);
                let span_out = ext - full;
                let cap_uu = EXTRAP_MAX_UU + 0.6 * lead as f32;
                let disp_t = (full + rest - rest * rest / (2.0 * span_out.max(1.0))) as f32;
                let decay = if rest > 0.0 { (1.0 - rest / span_out.max(1.0)).max(0.0) as f32 } else { 1.0 };
                let mut out = hold(newest, if dt <= ext { Mode::Extrap } else { Mode::Hold });
                for i in 0..SLOTS {
                    if !(newest.has(i) && prev.has(i)) { continue; }
                    let mut d = [0f32; 3];
                    // A requested look-ahead (game prediction) also follows the
                    // sender's acceleration (from its last two velocities) for
                    // up to ACCEL_EXTRAP_MS: a foot touching down or a swing
                    // reversing then decelerates in the prediction instead of
                    // overshooting by v * t (LordsHall: 5-20 uu foot
                    // errors from a constant-velocity look-ahead).
                    let use_acc = lead > 0.0 && newest.has_vel(i) && prev.has_vel(i) && span >= 4.0;
                    let ta = (dt.min(ACCEL_EXTRAP_MS) / 1000.0) as f32;
                    for a in 0..3 {
                        let v = if newest.has_vel(i) { newest.vel[i][a] / 1000.0 } else { (newest.b[i][a] - prev.b[i][a]) / span as f32 };
                        d[a] = v * disp_t;
                        if use_acc {
                            let acc = ((newest.vel[i][a] - prev.vel[i][a]) / (span as f32 / 1000.0)).clamp(-ACCEL_MAX, ACCEL_MAX);
                            d[a] += 0.5 * acc * ta * ta;
                        }
                    }
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let k = if len > cap_uu { cap_uu / len } else { 1.0 };
                    for a in 0..3 { out.bones[i][a] = newest.b[i][a] + d[a] * k; }
                    // Rotation: integrate the sender's angular velocity (deg/s, world).
                    if newest.has_vel(i) {
                        let s = (disp_t * k) as f32 / 1000.0;
                        let w = [newest.vel[i][3].to_radians() * s, newest.vel[i][4].to_radians() * s, newest.vel[i][5].to_radians() * s];
                        let ang = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
                        if ang > 1e-9 {
                            let h = (ang / 2.0).sin() / ang;
                            let dq = [w[0] * h, w[1] * h, w[2] * h, (ang / 2.0).cos()];
                            let q0 = [newest.b[i][3], newest.b[i][4], newest.b[i][5], newest.b[i][6]];
                            let q = v2::qnorm(v2::qmul(dq, q0));
                            out.bones[i][3..7].copy_from_slice(&q);
                        }
                    }
                    // The body is decelerating to a hold: report that.
                    for c in out.vel[i].iter_mut() { *c *= decay * k; }
                    if use_acc {
                        for a in 0..3 {
                            let acc = ((newest.vel[i][a] - prev.vel[i][a]) / (span as f32 / 1000.0)).clamp(-ACCEL_MAX, ACCEL_MAX);
                            out.vel[i][a] += acc * ta * decay * k;
                        }
                    }
                }
                return out;
            }
        }
        let mut out = hold(newest, Mode::Hold);
        out.vel = [[0.0; 6]; SLOTS];
        return out;
    }
    // Segment k..k+1 containing t.
    let mut k = 0;
    for j in (0..n - 1).rev() { if fr[j].ts <= t { k = j; break; } }
    let (a, b) = (&fr[k], &fr[k + 1]);
    let h = b.ts - a.ts;
    let s = if h > 0.0 { ((t - a.ts) / h) as f32 } else { 0.0 };
    let mask = a.mask & b.mask;
    let vmask = a.vmask & b.vmask;
    let mut out = SampleOut { mode: Mode::Interp, mask, bones: a.b, vmask, vel: [[0.0; 6]; SLOTS], v2: a.v2 && b.v2,
                              extra: if s < 0.5 { a.extra.clone() } else { b.extra.clone() } };
    for i in 0..SLOTS {
        if mask & (1 << i) == 0 { continue; }
        let (pa, pb) = (&a.b[i], &b.b[i]);
        let max_gap = if vmask & (1 << i) != 0 { HERMITE_MAX_GAP_V2_MS } else { HERMITE_MAX_GAP_MS };
        if h <= max_gap {
            let (m0, m1) = if vmask & (1 << i) != 0 {
                // Sender-measured velocities: the exact tangents (uu/ms).
                ([a.vel[i][0] / 1000.0, a.vel[i][1] / 1000.0, a.vel[i][2] / 1000.0],
                 [b.vel[i][0] / 1000.0, b.vel[i][1] / 1000.0, b.vel[i][2] / 1000.0])
            } else {
                let chord = dist3(pa, pb) / h as f32;
                let lim = 2.0 * chord + 0.05; // uu/ms
                let mut m0 = [tangent(fr, k, i, 0), tangent(fr, k, i, 1), tangent(fr, k, i, 2)];
                let mut m1 = [tangent(fr, k + 1, i, 0), tangent(fr, k + 1, i, 1), tangent(fr, k + 1, i, 2)];
                for m in [&mut m0, &mut m1] {
                    let l = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
                    if l > lim { for c in m.iter_mut() { *c *= lim / l; } }
                }
                (m0, m1)
            };
            for ax in 0..3 { out.bones[i][ax] = hermite(pa[ax], m0[ax], pb[ax], m1[ax], s, h as f32); }
        } else {
            for ax in 0..3 { out.bones[i][ax] = pa[ax] + (pb[ax] - pa[ax]) * s; }
        }
        let q = slerp(&pa[3..7], &pb[3..7], s);
        out.bones[i][3..7].copy_from_slice(&q);
        if vmask & (1 << i) != 0 {
            for c in 0..6 { out.vel[i][c] = a.vel[i][c] + (b.vel[i][c] - a.vel[i][c]) * s; }
        }
    }
    out
}

fn shortest_deg(a: f32, b: f32) -> f32 { (b - a + 180.0).rem_euclid(360.0) - 180.0 }

/// Root (capsule) at sender time t: Hermite with the sender's own velocity
/// (uu/s), bounded extrapolation, hold.
fn sample_roots(rs: &VecDeque<RootSample>, t: f64) -> Option<([f32; 3], f32)> {
    let n = rs.len();
    if n == 0 { return None; }
    if t <= rs[0].ts { return Some((rs[0].pos, rs[0].yaw)); }
    let last = rs[n - 1];
    if t >= last.ts {
        let e = (t - last.ts).min(EXTRAP_MS);
        let disp_s = ((e - e * e / (2.0 * EXTRAP_MS)) / 1000.0) as f32;
        let mut p = last.pos;
        let mut d = [last.vel[0] * disp_s, last.vel[1] * disp_s, last.vel[2] * disp_s];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len > EXTRAP_MAX_UU { for c in d.iter_mut() { *c *= EXTRAP_MAX_UU / len; } }
        for a in 0..3 { p[a] += d[a]; }
        return Some((p, last.yaw));
    }
    let mut k = 0;
    for j in (0..n - 1).rev() { if rs[j].ts <= t { k = j; break; } }
    let (a, b) = (rs[k], rs[k + 1]);
    let h = (b.ts - a.ts) as f32;
    let s = if h > 0.0 { ((t - a.ts) as f32) / h } else { 0.0 };
    let mut p = [0f32; 3];
    for ax in 0..3 {
        p[ax] = if (h as f64) <= HERMITE_MAX_GAP_MS {
            hermite(a.pos[ax], a.vel[ax] / 1000.0, b.pos[ax], b.vel[ax] / 1000.0, s, h)
        } else {
            a.pos[ax] + (b.pos[ax] - a.pos[ax]) * s
        };
    }
    Some((p, a.yaw + shortest_deg(a.yaw, b.yaw) * s))
}

/// Yaw (degrees) of a UE quaternion (xyzw), same formula as FQuat::Rotator.
pub fn quat_yaw(q: [f32; 4]) -> f32 {
    let (x, y, z, w) = (q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64);
    (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z)).to_degrees() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posecodec;
    const HR: usize = 16; // hand_r slot

    /// A body moving along +X at `speed` uu/s with the right hand swinging on
    /// a 60 cm circle at `w` rad/s; slot 16 = weapon 50 uu beyond the hand.
    fn frame_at(tick: u32, ts: f64, speed: f32, w: f32) -> Frame {
        let t = (ts / 1000.0) as f32;
        let mut b = [[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]; SLOTS];
        b[0] = [speed * t, 0.0, 100.0, 0.0, 0.0, 0.0, 1.0];
        let (s, c) = (w * t).sin_cos();
        b[HR] = [speed * t + 60.0 * c, 60.0 * s, 140.0, 0.0, 0.0, (w * t / 2.0).sin(), (w * t / 2.0).cos()];
        b[WPN_R] = [speed * t + 110.0 * c, 110.0 * s, 140.0, 0.0, 0.0, (w * t / 2.0).sin(), (w * t / 2.0).cos()];
        Frame { ts, tick, mask: 1 | (1 << HR) | (1 << WPN_R), b, vmask: 0, vel: [[0.0; 6]; SLOTS], v2: false, extra: None }
    }

    fn feed(pb: &mut Playback, ticks: std::ops::Range<u32>, period: f64, lat: f64) {
        for k in ticks {
            let ts = 10_000.0 + k as f64 * period;
            pb.push_pose(ts + lat, frame_at(k, ts, 300.0, 6.0));
        }
    }

    #[test]
    fn duplicates_and_reordering_are_handled() {
        let mut pb = Playback::new();
        assert_eq!(pb.push_pose(100.0, frame_at(1, 0.0, 0.0, 0.0)), Push::Accepted);
        assert_eq!(pb.push_pose(140.0, frame_at(3, 33.3, 0.0, 0.0)), Push::Accepted);
        // Late-arriving tick 2 slots in between.
        assert_eq!(pb.push_pose(141.0, frame_at(2, 16.7, 0.0, 0.0)), Push::Reordered);
        assert_eq!(pb.push_pose(142.0, frame_at(2, 16.7, 0.0, 0.0)), Push::Duplicate);
        let ts: Vec<f64> = pb.frames.iter().map(|f| f.ts).collect();
        assert_eq!(ts, vec![0.0, 16.7, 33.3]);
        // Once played past, an old frame is late and ignored.
        pb.sample(1000.0);
        assert!(pb.pt.unwrap() > 33.3);
        assert_eq!(pb.push_pose(1001.0, frame_at(9, 20.0, 0.0, 0.0)), Push::Late);
    }

    #[test]
    fn hermite_is_exact_on_endpoints_and_smooth() {
        assert!((hermite(1.0, 0.0, 5.0, 0.0, 0.0, 10.0) - 1.0).abs() < 1e-6);
        assert!((hermite(1.0, 0.0, 5.0, 0.0, 1.0, 10.0) - 5.0).abs() < 1e-6);
        // Constant velocity is reproduced exactly (linear motion stays linear).
        let v = 0.3; // uu/ms
        for s in [0.1f32, 0.33, 0.5, 0.9] {
            let p = hermite(0.0, v, 10.0 * v, v, s, 10.0);
            assert!((p - 10.0 * v * s).abs() < 1e-4);
        }
    }

    #[test]
    fn interpolation_tracks_a_fast_swing_closely() {
        // 60 Hz frames, 40 ms one-way latency, hand at 6 rad/s on 60 cm (360 uu/s).
        let mut pb = Playback::new();
        feed(&mut pb, 0..60, 1000.0 / 60.0, 40.0);
        let now = 10_000.0 + 59.0 * 1000.0 / 60.0 + 40.0;
        let s = pb.sample(now).unwrap();
        assert_eq!(s.mode, Mode::Interp, "{:?}", s.mode);
        let truth = frame_at(0, s.pt, 300.0, 6.0);
        for i in [0usize, HR, WPN_R] {
            let err = dist3(&s.bones[i], &truth.b[i]);
            assert!(err < 0.6, "slot {} err {} uu", i, err);
        }
        // Buffer delay adapts to a steady stream: ~ interval + margin.
        assert!(s.delay < 40.0, "delay {}", s.delay);
    }

    #[test]
    fn playback_clock_is_smooth_under_jitter() {
        // ±15 ms uniform jitter, sampled at 144 fps: pt must never go
        // backwards and its rate must stay within the slew limit.
        let mut pb = Playback::new();
        let mut rng = 12345u64;
        let mut rnd = || { rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((rng >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0 };
        let period = 1000.0 / 60.0;
        let mut arrivals: Vec<(f64, u32)> = (0..600).map(|k| (10_000.0 + k as f64 * period + 50.0 + 15.0 * rnd(), k)).collect();
        arrivals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut ai = 0;
        let mut now = 10_100.0;
        let mut last_pt: Option<f64> = None;
        let (mut interp, mut total) = (0, 0);
        while now < 10_000.0 + 590.0 * period {
            while ai < arrivals.len() && arrivals[ai].0 <= now {
                let k = arrivals[ai].1;
                pb.push_pose(arrivals[ai].0, frame_at(k, 10_000.0 + k as f64 * period, 300.0, 6.0));
                ai += 1;
            }
            if let Some(s) = pb.sample(now) {
                if let Some(lp) = last_pt {
                    let step = s.pt - lp;
                    assert!(step >= 0.0, "pt went backwards by {}", step);
                    assert!(step <= (1000.0 / 144.0) * (1.0 + SLEW_MAX) + 1e-6 || total < 5, "pt jumped {}", step);
                }
                last_pt = Some(s.pt);
                total += 1;
                if s.mode == Mode::Interp { interp += 1; }
            }
            now += 1000.0 / 144.0;
        }
        // After settling, nearly every rendered frame is a true interpolation.
        assert!(interp as f64 / total as f64 > 0.95, "interp {}/{}", interp, total);
        assert!(pb.delay > 20.0 && pb.delay < 70.0, "delay {}", pb.delay);
    }

    #[test]
    fn loss_extrapolates_bounded_then_holds_then_stale() {
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        feed(&mut pb, 0..30, period, 30.0);
        let last_ts = 10_000.0 + 29.0 * period;
        // Stream stops. Just past the newest frame: extrapolating.
        let now0 = last_ts + 30.0 + pb.delay + 20.0;
        let s = pb.sample(now0).unwrap();
        assert_eq!(s.mode, Mode::Extrap);
        let newest = pb.frames.back().unwrap().clone();
        for i in [0usize, HR, WPN_R] { assert!(dist3(&s.bones[i], &newest.b[i]) <= EXTRAP_MAX_UU + 1e-3); }
        // Way past (the clock slows while starving, so it takes longer): hold
        // (still bounded), then stale after STALE_MS.
        let mut t = now0;
        let mut s = pb.sample(t).unwrap();
        while s.mode == Mode::Extrap && t < now0 + 400.0 { t += 8.0; s = pb.sample(t).unwrap(); }
        assert_eq!(s.mode, Mode::Hold);
        for i in [0usize, HR, WPN_R] { assert!(dist3(&s.bones[i], &newest.b[i]) <= EXTRAP_MAX_UU + 1e-3); }
        let mut now = now0 + 100.0;
        let mut mode = s.mode;
        while now < now0 + STALE_MS + 400.0 { now += 16.0; mode = pb.sample(now).unwrap().mode; }
        assert_eq!(mode, Mode::Stale);
    }

    #[test]
    fn single_lost_frame_is_interpolated_across() {
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        for k in 0..40u32 {
            if k == 30 { continue; } // lost
            let ts = 10_000.0 + k as f64 * period;
            pb.push_pose(ts + 30.0, frame_at(k, ts, 300.0, 6.0));
        }
        // Sample exactly where the lost frame would have been.
        pb.pt = Some(10_000.0 + 30.0 * period - 1.0);
        let off = pb.clock.offset.unwrap();
        let now = 10_000.0 + 30.0 * period + off + pb.delay;
        pb.last_now = now;
        let s = pb.sample(now).unwrap();
        assert_eq!(s.mode, Mode::Interp);
        let truth = frame_at(0, s.pt, 300.0, 6.0);
        assert!(dist3(&s.bones[HR], &truth.b[HR]) < 2.0);
    }

    #[test]
    fn teleport_cuts_history_and_snaps() {
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        feed(&mut pb, 0..20, period, 30.0);
        pb.sample(10_000.0 + 19.0 * period + 30.0);
        let mut f = frame_at(20, 10_000.0 + 20.0 * period, 300.0, 6.0);
        f.b[0][0] += 5000.0; // respawn across the arena
        assert_eq!(pb.push_pose(10_000.0 + 20.0 * period + 30.0, f), Push::Cut);
        assert_eq!(pb.frames.len(), 1);
        let s = pb.sample(10_000.0 + 20.0 * period + 31.0).unwrap();
        assert_eq!(s.cut, 1);
        assert!(s.bones[0][0] > 4000.0, "shows the new place immediately: {}", s.bones[0][0]);
    }

    #[test]
    fn a_long_gap_starts_over() {
        let mut pb = Playback::new();
        feed(&mut pb, 0..10, 1000.0 / 60.0, 30.0);
        let cut0 = pb.cut;
        // 800 ms later, 50 uu away: slow enough for a walk, but nothing to interpolate.
        let ts = 10_000.0 + 9.0 * 1000.0 / 60.0 + 800.0;
        let mut f = frame_at(99, ts, 300.0, 6.0);
        f.b[0][0] += 50.0;
        assert_eq!(pb.push_pose(ts + 30.0, f), Push::Cut);
        assert_eq!(pb.cut, cut0 + 1);
        assert_eq!(pb.frames.len(), 1);
    }

    #[test]
    fn sender_restart_resets_buffer() {
        let mut pb = Playback::new();
        feed(&mut pb, 0..10, 16.0, 20.0);
        let r = pb.push_pose(500.0, frame_at(1, 10.0, 0.0, 0.0));
        assert_eq!(r, Push::Accepted);
        assert_eq!(pb.frames.len(), 1);
        assert_eq!(pb.stats.restarts, 1);
    }

    #[test]
    fn clock_drift_is_tracked() {
        // Sender clock runs 0.05 % fast; the offset window follows it so the
        // buffer never starves or grows without bound over 60 s.
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        let mut now = 0.0;
        for k in 0..3600u32 {
            let ts = 10_000.0 + k as f64 * period * 1.0005;
            let rx = k as f64 * period + 40.0;
            pb.push_pose(rx, frame_at(k, ts, 300.0, 6.0));
            now = rx;
            pb.sample(now);
        }
        let s = pb.sample(now + 1.0).unwrap();
        assert_eq!(s.mode, Mode::Interp);
        assert!(s.age < 60.0);
    }

    #[test]
    fn root_hermite_uses_sender_velocity() {
        let mut pb = Playback::new();
        for k in 0..10u32 {
            let ts = 1000.0 + k as f64 * 16.0;
            let x = 200.0 * (ts as f32 / 1000.0);
            pb.push_root(ts + 25.0, ts, [x, 0.0, 90.0], [200.0, 0.0, 0.0], 45.0);
        }
        let (p, yaw) = sample_roots(&pb.roots, 1000.0 + 4.5 * 16.0).unwrap();
        assert!((p[0] - 200.0 * (1072.0 / 1000.0)).abs() < 0.01);
        assert!((yaw - 45.0).abs() < 1e-3);
    }

    #[test]
    fn wire_frame_feeds_playback() {
        // posecodec -> Frame keeps the weapon slot.
        let bones = [(0u8, [0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 1.0]), (16u8, [80.0, 0.0, 140.0, 0.0, 0.0, 0.0, 1.0])];
        let f = posecodec::decode(&posecodec::encode(1234, &bones)).unwrap();
        let fr = Frame::from_pose(5, &f);
        assert_eq!(fr.mask, 1 | (1 << WPN_R));
        assert!((fr.b[WPN_R][0] - 80.0).abs() < 0.6);
    }

    /// Analytic v2 body: walking at 150 uu/s, the right arm swinging
    /// (shoulder + elbow rotating at up to ~1100 deg/s), a sword in the hand.
    fn v2_state(t_ms: f64) -> v2::Full {
        use v2::*;
        let t = (t_ms / 1000.0) as f32;
        let mut f = Full { ts: t_ms, k: 0.9966, ..Default::default() };
        let axis = |a: [f32; 3], deg: f32| -> Quat {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            let h = deg.to_radians() / 2.0;
            [a[0] / l * h.sin(), a[1] / l * h.sin(), a[2] / l * h.sin(), h.cos()]
        };
        f.bones[0].p = [150.0 * t, 20.0, 95.0];
        f.bones[0].q = axis([0.0, 0.0, 1.0], 30.0 * t);
        for i in 1..NB {
            let par = f.bones[PARENT[i]];
            let lq = match i {
                14 => axis([0.0, 1.0, 0.3], 90.0 * (6.0 * t).sin()),
                15 => axis([0.0, 0.0, 1.0], 45.0 + 40.0 * (6.0 * t + 0.5).sin()),
                _ => [0.0, 0.0, 0.0, 1.0],
            };
            f.bones[i].q = qnorm(qmul(par.q, lq));
            f.bones[i].p = [par.p[0], par.p[1], par.p[2]];
            let o = qrot(par.q, [REF_T[i][0] * f.k, REF_T[i][1] * f.k, REF_T[i][2] * f.k]);
            for a in 0..3 { f.bones[i].p[a] += o[a]; }
        }
        let h = f.bones[16];
        f.weapons.push(Weapon { hands: 1, id: 3, p: h.p, q: h.q, base: [0.0, 0.0, 15.0], tip: [-100.0, 0.0, 0.0], ..Default::default() });
        f
    }

    /// v2 state with exact velocities (central differences of the analytic motion).
    fn v2_with_vel(t_ms: f64) -> v2::Full {
        let (a, b) = (v2_state(t_ms - 0.05), v2_state(t_ms + 0.05));
        let mut f = v2_state(t_ms);
        let rv = |qa: [f32; 4], qb: [f32; 4]| -> [f32; 3] {
            let d = v2::qmul(qb, v2::qconj(qa));
            let d = if d[3] < 0.0 { [-d[0], -d[1], -d[2], -d[3]] } else { d };
            let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if s < 1e-9 { return [0.0; 3]; }
            let ang = 2.0 * s.atan2(d[3]);
            let k = ang.to_degrees() / s / 0.0001;
            [d[0] * k, d[1] * k, d[2] * k]
        };
        for i in 0..v2::NB {
            if !v2::HAS_BODY[i] { continue; }
            for ax in 0..3 { f.bones[i].v[ax] = (b.bones[i].p[ax] - a.bones[i].p[ax]) / 0.0001; }
            f.bones[i].w = rv(a.bones[i].q, b.bones[i].q);
        }
        for ax in 0..3 { f.weapons[0].v[ax] = (b.weapons[0].p[ax] - a.weapons[0].p[ax]) / 0.0001; }
        f.weapons[0].w = rv(a.weapons[0].q, b.weapons[0].q);
        f
    }

    #[test]
    fn v2_wire_playback_matches_the_true_motion_between_frames() {
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        for k in 0..60u32 {
            let ts = 10_000.0 + k as f64 * period;
            let buf = v2::encode(&v2_with_vel(ts));
            let fr = Frame::from_wire(k, &buf).expect("v2 frame");
            assert!(fr.v2 && fr.vmask & (1 << HR) != 0 && fr.has(WPN_R));
            pb.push_pose(ts + 40.0, fr);
        }
        // Sample at many sub-frame instants inside the buffered range.
        let (mut worst, mut worst_tip, mut worst_rot) = (0f32, 0f32, 0f32);
        for j in 0..200 {
            let t = 10_000.0 + 20.0 * period + j as f64 * 0.137;
            let so = sample_frames(&pb.frames, t, 0.0);
            assert_eq!(so.mode, Mode::Interp);
            let truth = v2_state(t);
            for i in 0..v2::NB {
                worst = worst.max(dist3(&so.bones[i], &[truth.bones[i].p[0], truth.bones[i].p[1], truth.bones[i].p[2]]));
                worst_rot = worst_rot.max(v2::qangle_deg([so.bones[i][3], so.bones[i][4], so.bones[i][5], so.bones[i][6]], truth.bones[i].q));
            }
            let w = &so.bones[WPN_R];
            let tip = v2::qrot([w[3], w[4], w[5], w[6]], [-100.0, 0.0, 0.0]);
            let tt = v2::blade_world(&truth.weapons[0]).1;
            worst_tip = worst_tip.max(v2::dist([w[0] + tip[0], w[1] + tip[1], w[2] + tip[2]], tt));
        }
        assert!(worst < 1.0, "worst bone error {} uu", worst);
        assert!(worst_rot < 1.0, "worst rotation error {} deg", worst_rot);
        assert!(worst_tip < 3.0, "worst blade tip error {} uu", worst_tip);
        println!("v2 playback: worst bone {:.3} uu, worst rot {:.3} deg, worst tip {:.3} uu", worst, worst_rot, worst_tip);
    }

    /// Hand error of a decimated v2 stream: the sender samples at
    /// 60 Hz, the relay forwards every `k`-th frame (60/30/15/7.5 Hz), 40 ms
    /// one-way + ±6 ms jitter, played every 8 ms. Returns (hand p95, hand
    /// max, worst snap between consecutive samples, hold fraction) in uu.
    fn decimated_hand_error(k: u32, hint: bool, swing: f64) -> (f32, f32, f32, f32) {
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        let mut seed = 7u64;
        let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) as f64 / (1u64 << 31) as f64 };
        let state = |t: f64| v2_with_vel(10_000.0 + (t - 10_000.0) * swing);
        let mut arrivals: Vec<(f64, u32, f64)> = Vec::new();
        for n in 0..(60 * 8) {
            if n % k != 0 { continue; }
            let ts = 10_000.0 + n as f64 * period;
            arrivals.push((ts + 40.0 + 12.0 * rnd() - 6.0, n, ts));
        }
        arrivals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut errs, mut snap, mut holds, mut total) = (Vec::new(), 0f32, 0u32, 0u32);
        let mut last: Option<([f32; 3], [f32; 3])> = None;
        let mut ai = 0;
        let mut now = 10_000.0;
        while now < 10_000.0 + 7.5 * 1000.0 {
            while ai < arrivals.len() && arrivals[ai].0 <= now {
                let (rx, n, ts) = arrivals[ai];
                let mut f = state(ts);
                f.ts = ts;
                // Scale velocities with the swing factor (the motion is time-scaled).
                for b in f.bones.iter_mut() { for a in 0..3 { b.v[a] *= swing as f32; b.w[a] *= swing as f32; } }
                for w in f.weapons.iter_mut() { for a in 0..3 { w.v[a] *= swing as f32; w.w[a] *= swing as f32; } }
                if hint { pb.set_interval_hint((period * k as f64) as f32); }
                pb.push_pose(rx, Frame::from_wire(n, &v2::encode(&f)).unwrap());
                ai += 1;
            }
            if let Some(s) = pb.sample(now) {
                if now > 11_500.0 {
                    let t = state(s.pt);
                    let truth = [t.bones[HR].p[0], t.bones[HR].p[1], t.bones[HR].p[2]];
                    let played = [s.bones[HR][0], s.bones[HR][1], s.bones[HR][2]];
                    errs.push(dist3(&played, &truth));
                    if let Some((lp, lt)) = last {
                        let dp = [played[0] - lp[0], played[1] - lp[1], played[2] - lp[2]];
                        let dt = [truth[0] - lt[0], truth[1] - lt[1], truth[2] - lt[2]];
                        snap = snap.max(dist3(&dp, &dt));
                    }
                    last = Some((played, truth));
                    total += 1;
                    if s.mode == Mode::Hold { holds += 1; }
                }
            }
            now += 8.0;
        }
        if swing == 1.0 && hint { println!("  ({:.1} Hz: buffer delay {:.1} ms)", 60.0 / k as f64, pb.delay); }
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (errs[(errs.len() as f64 * 0.95) as usize], *errs.last().unwrap(), snap, holds as f32 / total.max(1) as f32)
    }

    /// Decimated v2 streams interpolate (no long holds, no snaps).
    #[test]
    fn decimated_v2_streams_play_back_smoothly() {
        for swing in [1.0, 2.5] {
            for (k, hz) in [(1u32, "60"), (2, "30"), (4, "15"), (8, "7.5")] {
                for hint in [false, true] {
                    let (p95, max, snap, hold) = decimated_hand_error(k, hint, swing);
                    println!("swing x{swing} {hz:>3} Hz hint {hint:5}: hand p95 {p95:6.2} max {max:6.2} snap {snap:6.2} uu, hold {:.0}%", hold * 100.0);
                    if swing == 1.0 && std::env::var("HSMP_MEASURE_ONLY").is_err() {
                        let (lim_p95, lim_snap) = match k { 1 => (3.0, 3.0), 2 => (4.0, 4.0), 4 => (8.0, 6.0), _ => (20.0, 10.0) };
                        assert!(p95 < lim_p95, "{hz} Hz hint {hint}: p95 {p95}");
                        assert!(snap < lim_snap, "{hz} Hz hint {hint}: snap {snap}");
                        assert!(hold < 0.02, "{hz} Hz hint {hint}: hold {hold}");
                    }
                }
            }
        }
    }

    /// When the relay drops a sender from 60 to 7.5 Hz (a replan),
    /// the server's interval hint grows the buffer at once; measuring the
    /// spacing alone needs several 133 ms frames first.
    #[test]
    fn interval_hint_covers_a_relay_rate_change_at_once() {
        let run = |hint: bool| {
            let mut pb = Playback::new();
            let period = 1000.0 / 60.0;
            let mut holds_after = 0;
            let mut now = 10_000.0;
            let mut n = 0u32;
            while now < 14_000.0 {
                let ts = 10_000.0 + n as f64 * period;
                if ts + 40.0 <= now {
                    let k = if ts < 12_000.0 { 1 } else { 8 };
                    if n % k == 0 {
                        if hint { pb.set_interval_hint((period * k as f64) as f32); }
                        let mut f = v2_with_vel(ts);
                        f.ts = ts;
                        pb.push_pose(ts + 40.0, Frame::from_wire(n, &v2::encode(&f)).unwrap());
                    }
                    n += 1;
                    continue;
                }
                if let Some(s) = pb.sample(now) {
                    if now > 12_100.0 && now < 13_000.0 && s.mode == Mode::Hold { holds_after += 1; }
                }
                now += 8.0;
            }
            holds_after
        };
        let (with, without) = (run(true), run(false));
        println!("held (frozen) samples in the 0.9 s after a 60 -> 7.5 Hz change: hint {with}, measured {without}");
        assert!(with <= without, "the hint never makes it worse");
        assert!(with < without || without == 0, "the hint shortens the freeze ({with} vs {without})");
    }

    /// A 2-5 s sender timestamp step-back resets the stream (the
    /// same threshold as lag comp), so stale frames never keep playing.
    #[test]
    fn a_two_second_step_back_resets_the_stream() {
        let mut pb = Playback::new();
        feed(&mut pb, 0..30, 1000.0 / 60.0, 30.0);
        let newest = pb.frames.back().unwrap().ts;
        let r = pb.push_pose(20_000.0, frame_at(500, newest - RESTART_BACK_MS - 1.0, 0.0, 0.0));
        assert_eq!(r, Push::Accepted);
        assert_eq!(pb.frames.len(), 1, "old frames dropped");
        assert_eq!(pb.stats.restarts, 1);
        // A small reorder (< 2 s) is still a reorder, not a restart.
        let mut pb = Playback::new();
        feed(&mut pb, 0..30, 1000.0 / 60.0, 30.0);
        assert_eq!(pb.push_pose(20_000.0, frame_at(900, 10_000.0 + 5.5, 0.0, 0.0)), Push::Reordered);
        assert_eq!(RESTART_BACK_MS, 2000.0, "lag comp RESET_BACKSTEP_MS");
    }

    #[test]
    fn physics_step_places_the_pose_and_keeps_the_clock_on_the_frame_start() {
        let mut f = v2_with_vel(10_000.0);
        f.step = 33.0;
        let buf = v2::encode(&f);
        let d = v2::decode(&buf).expect("decodes");
        assert_eq!(d.step, 33.0);
        let fr = Frame::from_wire(1, &buf).unwrap();
        assert!((fr.ts - 10_033.0).abs() < 0.01, "pose placed at {}", fr.ts);
        assert_eq!(fr.extra.as_ref().unwrap().clock_ts, Some(10_000.0));
        // without a step: unchanged frame, no clock stamp, one byte shorter
        let mut g = v2_with_vel(10_000.0);
        g.step = 0.0;
        let b2 = v2::encode(&g);
        assert_eq!(b2.len() + 1, buf.len());
        let fr2 = Frame::from_wire(2, &b2).unwrap();
        assert!((fr2.ts - 10_000.0).abs() < 0.01 && fr2.extra.as_ref().unwrap().clock_ts.is_none());
        // the clock follows the frame-start stamp: rx - 10000
        let mut pb = Playback::new();
        pb.push_pose(10_050.0, fr);
        assert_eq!(pb.clock.offset, Some(50.0));
    }

    #[test]
    fn look_ahead_samples_past_the_clock_and_keeps_velocity() {
        // A body moving at a constant 300 uu/s along X, frames at 60 Hz.
        let mut pb = Playback::new();
        let period = 1000.0 / 60.0;
        let mut rx = 0.0;
        for k in 0..60u32 {
            let ts = 10_000.0 + k as f64 * period;
            let mut f = frame_at(k, ts, 300.0, 0.0);
            f.v2 = true;
            for i in 0..SLOTS { if f.has(i) { f.vel[i] = [300.0, 0.0, 0.0, 0.0, 0.0, 0.0]; f.vmask |= 1 << i; } }
            rx = ts + 40.0;
            pb.push_pose(rx, f);
        }
        let s0 = pb.sample(rx).unwrap();
        let s1 = pb.sample_lead(rx + 0.001, 30.0).unwrap();
        assert!((s1.lead - 30.0).abs() < 1e-9 && (s1.pt - s0.pt - 30.0).abs() < 0.1, "pt {} vs {}", s1.pt, s0.pt);
        // 30 ms further along the motion (interpolated or extrapolated alike)
        let dx = s1.bones[PELVIS][0] - s0.bones[PELVIS][0];
        assert!((dx - 9.0).abs() < 0.5, "moved {dx} uu in 30 ms (want 9)");
        // past the newest frame the requested look-ahead keeps the sender's full velocity
        let s2 = pb.sample_lead(rx + 0.002, 110.0).unwrap();
        assert_eq!(s2.mode, Mode::Extrap);
        assert!((s2.vel[PELVIS][0] - 300.0).abs() < 1.0, "velocity decayed to {}", s2.vel[PELVIS][0]);
        // the playback clock itself is not moved by the look-ahead
        let s3 = pb.sample(rx + 0.003).unwrap();
        assert!((s3.pt - s0.pt).abs() < 0.1);
    }
}

#[cfg(test)]
mod replay_tests {
    //! POSE_REPLAY=<.probe_send.txt>: replay a real in-game sender record
    //! (every game frame of the owner's 23 bones) through codec v2 + the
    //! jitter buffer at 60 Hz and measure playback error vs. the truth for a
    //! range of buffer delays / Lua lead times (see
    //! docs/development/subsystems/replication.md).
    use super::*;
    use crate::posecodec::v2;

    struct R { ts: f64, b: Vec<[f32; 7]> }

    fn load(path: &str) -> Vec<R> {
        let mut out = Vec::new();
        for l in std::fs::read_to_string(path).unwrap().lines() {
            let t: Vec<&str> = l.split_whitespace().collect();
            if t.len() < 2 + 23 * 7 || t[0] != "S" { continue; }
            let ts: f64 = t[1].parse().unwrap();
            let mut b = Vec::new();
            for i in 0..23 { let mut x = [0f32; 7]; for k in 0..7 { x[k] = t[2 + i * 7 + k].parse().unwrap(); } b.push(x); }
            out.push(R { ts, b });
        }
        out.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap());
        out.dedup_by(|a, b| a.ts == b.ts);
        out
    }

    fn truth(r: &[R], t: f64) -> Option<Vec<[f32; 7]>> {
        let i = r.partition_point(|x| x.ts <= t);
        if i == 0 || i >= r.len() || r[i].ts - r[i - 1].ts > 40.0 { return None; }
        let (a, b) = (&r[i - 1], &r[i]);
        let u = ((t - a.ts) / (b.ts - a.ts)) as f32;
        Some((0..23).map(|k| {
            let q = slerp(&a.b[k][3..7], &b.b[k][3..7], u);
            [a.b[k][0] + (b.b[k][0] - a.b[k][0]) * u, a.b[k][1] + (b.b[k][1] - a.b[k][1]) * u,
             a.b[k][2] + (b.b[k][2] - a.b[k][2]) * u, q[0], q[1], q[2], q[3]]
        }).collect())
    }

    fn full_at(r: &[R], i: usize) -> v2::Full {
        let mut f = v2::Full { ts: r[i].ts, k: 0.9933, ..Default::default() };
        let (lo, hi) = (i.saturating_sub(1), (i + 1).min(r.len() - 1));
        let dt = ((r[hi].ts - r[lo].ts) / 1000.0) as f32;
        for k in 0..23 {
            let x = r[i].b[k];
            f.bones[k].p = [x[0], x[1], x[2]];
            f.bones[k].q = [x[3], x[4], x[5], x[6]];
            if dt > 0.0 {
                for a in 0..3 { f.bones[k].v[a] = (r[hi].b[k][a] - r[lo].b[k][a]) / dt; }
                let qa = [r[lo].b[k][3], r[lo].b[k][4], r[lo].b[k][5], r[lo].b[k][6]];
                let qb = [r[hi].b[k][3], r[hi].b[k][4], r[hi].b[k][5], r[hi].b[k][6]];
                let d = v2::qmul(qb, v2::qconj(qa));
                let d = if d[3] < 0.0 { [-d[0], -d[1], -d[2], -d[3]] } else { d };
                let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if s > 1e-9 { let kk = (2.0 * s.atan2(d[3])).to_degrees() / s / dt; f.bones[k].w = [d[0] * kk, d[1] * kk, d[2] * kk]; }
            }
            if !v2::HAS_BODY[k] { f.bones[k].v = [0.0; 3]; f.bones[k].w = [0.0; 3]; }
        }
        f
    }

    /// Advance a playback sample by `ms` with its velocities (HSMPAvatars PURE.advance).
    fn advance(b: &Xf, v: &Vel, ms: f32) -> Xf {
        let s = ms / 1000.0;
        let mut o = *b;
        for a in 0..3 { o[a] += v[a] * s; }
        let w = [v[3].to_radians() * s, v[4].to_radians() * s, v[5].to_radians() * s];
        let ang = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
        if ang > 1e-9 {
            let h = (ang / 2.0).sin() / ang;
            let q = v2::qnorm(v2::qmul([w[0] * h, w[1] * h, w[2] * h, (ang / 2.0).cos()], [b[3], b[4], b[5], b[6]]));
            o[3..7].copy_from_slice(&q);
        }
        o
    }

    #[test]
    fn replay_real_sender_record() {
        let Ok(path) = std::env::var("POSE_REPLAY") else { return };
        let r = load(&path);
        if r.len() < 100 { return; }
        let from: f64 = std::env::var("POSE_REPLAY_FROM").ok().and_then(|s| s.parse().ok()).unwrap_or(r[0].ts);
        let to: f64 = std::env::var("POSE_REPLAY_TO").ok().and_then(|s| s.parse().ok()).unwrap_or(r[r.len() - 1].ts);
        println!("replay {} sender frames {:.0}..{:.0} ms", r.len(), from, to);
        // (jitter span ms, loss %, fixed delay or adaptive, lead ms)
        let mut cases: Vec<(f64, f64, Option<f64>, f32)> = Vec::new();
        for d in [8.0, 12.0, 16.0, 20.0, 25.0, 33.0] { for l in [0.0f32, 8.0, 16.0] { cases.push((0.0, 0.0, Some(d), l)); cases.push((12.0, 0.0, Some(d), l)); } }
        for (j, loss) in [(0.0, 0.0), (24.0, 1.0), (50.0, 2.0)] { cases.push((j, loss, None, 0.0)); }
        for (jitter, loss, delay, lead) in cases {
            let mut pb = Playback::new();
            pb.fixed_delay = delay;
            let mut next = from;
            let mut seed = 99u64;
            let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1); (seed >> 33) as f64 / (1u64 << 31) as f64 };
            // Arrival times (sender clock): 20 ms base + uniform jitter, random loss.
            let mut arrivals: Vec<(f64, Frame)> = Vec::new();
            for (i, x) in r.iter().enumerate() {
                if x.ts < from || x.ts > to || x.ts < next { continue; }
                next = x.ts.max(next) + 1000.0 / 60.0;
                if rnd() * 100.0 < loss { continue; }
                let buf = v2::encode(&full_at(&r, i));
                arrivals.push((x.ts + 20.0 + jitter * rnd(), Frame::from_wire(i as u32, &buf).unwrap()));
            }
            arrivals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let (mut hand, mut hand_rot, mut arm, mut idle_jump, mut delays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut ai = 0;
            let mut now = from + 200.0;
            let mut last: Option<(Xf, Vec<[f32; 7]>)> = None;
            while now < to {
                while ai < arrivals.len() && arrivals[ai].0 <= now { let f = arrivals[ai].1.clone(); pb.push_pose(arrivals[ai].0, f); ai += 1; }
                if let Some(s) = pb.sample(now) {
                    delays.push(s.delay as f32);
                    if let Some(t) = truth(&r, s.pt + lead as f64) {
                        for k in [9usize, 10, 11, 12, 13, 14, 15, 16] {
                            let b = advance(&s.bones[k], &s.vel[k], lead);
                            let e = dist3(&b, &t[k]);
                            arm.push(e);
                            if k == 12 || k == 16 {
                                hand.push(e);
                                hand_rot.push(v2::qangle_deg([b[3], b[4], b[5], b[6]], [t[k][3], t[k][4], t[k][5], t[k][6]]));
                            }
                        }
                        // Owner idle => playback idle: per-sample motion of the
                        // played hand while the true hand moved < 0.1 uu.
                        if let Some((lb, lt)) = &last {
                            if dist3(&t[16], &lt[16]) < 0.1 { idle_jump.push(dist3(&s.bones[16], lb)); }
                        }
                        last = Some((s.bones[16], t));
                    }
                }
                now += 8.0;
            }
            let p = |v: &mut Vec<f32>, q: f64| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); if v.is_empty() { f32::NAN } else { v[((v.len() - 1) as f64 * q) as usize] } };
            println!("jitter {:>4.0} loss {:>3.1}% delay {:>9} lead {:>4.0} | buffer p50 {:5.1} ms | hand uu p50 {:5.2} p95 {:5.2} | hand deg p95 {:5.2} | arm chain uu p95 {:5.2} | idle hand jump p95 {:5.2} uu",
                jitter, loss, delay.map(|d| format!("{:.0} ms", d)).unwrap_or("adaptive".into()), lead, p(&mut delays, 0.5),
                p(&mut hand, 0.5), p(&mut hand, 0.95), p(&mut hand_rot, 0.95), p(&mut arm, 0.95), p(&mut idle_jump, 0.95));
        }
    }
}

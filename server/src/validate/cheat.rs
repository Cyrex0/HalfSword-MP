//! Per-player cheat-score counters. Every validation path
//! that sees a suspicious claim bumps a counter here; `snapshot` / `to_json`
//! expose them for RCON / the admin panel. Counters never act on their own:
//! a single honest desync can trip one, so kicking is left to an admin or a
//! later policy keyed on `score`.

use crate::proto::PeerId;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Hit without timestamps against a victim that streams history.
    NoTimestamp,
    /// victim_view_ts far outside the server-predicted window (backtrack).
    ViewHintOutlier,
    /// attacker_ts does not match the claim's arrival through the clock map.
    TsInconsistent,
    /// Hit rejected by lag-comp geometry / rewind cap.
    GeometryReject,
    /// Clash reports beyond the per-(reporter, other) rate.
    ParryRateLimited,
    /// Clash report the server judged geometrically impossible.
    ParryInvalid,
    /// Claimed damage above the plausibility cap (clamped).
    DamageClamped,
    /// Claimed damage above 1.5× the cap.
    DamageSevere,
    /// Sender clock runs at a different rate than the server (speedhack).
    ClockRate,
    /// Streamed blade / arm longer than the weapon / skeleton allows (reach).
    ReachImplausible,
    /// Owner kept reporting itself alive after server-validated lethal damage
    /// (god mode; flagged, and the death declared only where enforced).
    GodMode,
    /// Stream jitter far above the transport's measured jitter (lag switch /
    /// timestamp noise on its own pose stream).
    LagSwitch,
    /// Pose / blade sample off its validated root, or moving faster than a
    /// body can (teleport strike).
    Teleport,
    /// Damage claims beyond the per-attacker claim budget (flood).
    ClaimFlood,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counters {
    pub no_timestamp: u32,
    pub view_hint_outlier: u32,
    pub ts_inconsistent: u32,
    pub geometry_reject: u32,
    pub parry_rate_limited: u32,
    pub parry_invalid: u32,
    pub damage_clamped: u32,
    pub damage_severe: u32,
    pub clock_rate: u32,
    pub reach_implausible: u32,
    pub god_mode: u32,
    pub lag_switch: u32,
    pub teleport: u32,
    pub claim_flood: u32,
    /// Health points removed from claims by the damage cap.
    pub damage_clamped_hp: f32,
}

impl Counters {
    fn bump(&mut self, k: Kind) {
        let c = match k {
            Kind::NoTimestamp => &mut self.no_timestamp,
            Kind::ViewHintOutlier => &mut self.view_hint_outlier,
            Kind::TsInconsistent => &mut self.ts_inconsistent,
            Kind::GeometryReject => &mut self.geometry_reject,
            Kind::ParryRateLimited => &mut self.parry_rate_limited,
            Kind::ParryInvalid => &mut self.parry_invalid,
            Kind::DamageClamped => &mut self.damage_clamped,
            Kind::DamageSevere => &mut self.damage_severe,
            Kind::ClockRate => &mut self.clock_rate,
            Kind::ReachImplausible => &mut self.reach_implausible,
            Kind::GodMode => &mut self.god_mode,
            Kind::LagSwitch => &mut self.lag_switch,
            Kind::Teleport => &mut self.teleport,
            Kind::ClaimFlood => &mut self.claim_flood,
        };
        *c = c.saturating_add(1);
    }

    /// Weighted score: exploit signatures weigh more than noise that honest
    /// players on a bad link also produce (geometry rejects, rate limits).
    pub fn score(&self) -> f32 {
        5.0 * self.no_timestamp as f32
            + 3.0 * self.view_hint_outlier as f32
            + 3.0 * self.ts_inconsistent as f32
            + 0.2 * self.geometry_reject as f32
            + 0.2 * self.parry_rate_limited as f32
            + 2.0 * self.parry_invalid as f32
            + 1.0 * self.damage_clamped as f32
            + 4.0 * self.damage_severe as f32
            + 4.0 * self.clock_rate as f32
            + 3.0 * self.reach_implausible as f32
            + 10.0 * self.god_mode as f32
            + 0.5 * self.lag_switch as f32
            + 2.0 * self.teleport as f32
            + 0.05 * self.claim_flood as f32
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub peer_id: PeerId,
    pub score: f32,
    #[serde(flatten)]
    pub counters: Counters,
}

fn table() -> &'static Mutex<HashMap<PeerId, Counters>> {
    static T: OnceLock<Mutex<HashMap<PeerId, Counters>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn bump(peer: PeerId, k: Kind) {
    table().lock().unwrap().entry(peer).or_default().bump(k);
}

pub fn add_clamped_hp(peer: PeerId, hp: f32) {
    if hp.is_finite() && hp > 0.0 {
        table().lock().unwrap().entry(peer).or_default().damage_clamped_hp += hp;
    }
}

pub fn get(peer: PeerId) -> Counters {
    table().lock().unwrap().get(&peer).cloned().unwrap_or_default()
}

pub fn forget(peer: PeerId) {
    table().lock().unwrap().remove(&peer);
}

/// All players with any counter, highest score first.
pub fn snapshot() -> Vec<Entry> {
    let mut v: Vec<Entry> = table().lock().unwrap().iter()
        .map(|(&peer_id, c)| Entry { peer_id, score: c.score(), counters: c.clone() })
        .collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.peer_id.cmp(&b.peer_id)));
    v
}

/// RCON / admin payload: `[{"peer_id":..,"score":..,"no_timestamp":..,...}]`.
pub fn to_json() -> String {
    serde_json::to_string(&snapshot()).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_score_and_json() {
        let p = 9_001;
        bump(p, Kind::NoTimestamp);
        bump(p, Kind::DamageClamped);
        add_clamped_hp(p, 42.0);
        let c = get(p);
        assert_eq!(c.no_timestamp, 1);
        assert_eq!(c.damage_clamped, 1);
        assert_eq!(c.score(), 6.0);
        let j = to_json();
        assert!(j.contains("\"peer_id\":9001") && j.contains("\"damage_clamped_hp\":42.0"), "{}", j);
        forget(p);
        assert_eq!(get(p), Counters::default());
    }
}

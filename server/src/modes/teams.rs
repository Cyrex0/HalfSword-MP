//! Team assignment and balance.
//! Pure and deterministic: the result depends only on the set of seats, never
//! on input order, hash order or time.

use super::traits::{SeatId, SeatInfo, TeamId, MAX_TEAMS, NO_TEAM};
use std::collections::BTreeMap;

/// Balance `seats` over teams `1..=n_teams`.
///
/// Seats are ranked by rating (high first), then `key`, then seat id (a total
/// order, so input order is irrelevant). Each one in turn goes to the team
/// with the fewest members, then the lowest rating total, then the lowest id.
/// Sizes therefore differ by at most one and strong players are spread.
pub fn balance(seats: &[SeatInfo], n_teams: u8) -> BTreeMap<SeatId, TeamId> {
    let n = n_teams.clamp(1, MAX_TEAMS);
    // One entry per seat (the first listed wins), then the total order.
    let mut by_seat: BTreeMap<SeatId, &SeatInfo> = BTreeMap::new();
    for s in seats {
        by_seat.entry(s.seat).or_insert(s);
    }
    let mut order: Vec<&SeatInfo> = by_seat.into_values().collect();
    order.sort_by(|a, b| b.rating.cmp(&a.rating).then_with(|| a.key.cmp(&b.key)).then_with(|| a.seat.cmp(&b.seat)));
    let mut size = vec![0u32; n as usize];
    let mut total = vec![0i64; n as usize];
    let mut out = BTreeMap::new();
    for s in order {
        let t = (0..n as usize).min_by_key(|&t| (size[t], total[t], t)).unwrap();
        size[t] += 1;
        total[t] += s.rating as i64;
        out.insert(s.seat, t as TeamId + 1);
    }
    out
}

/// Team for a seat joining a running match: the smallest team, ties to the
/// lower score, then the lower id ("auto-balance on join").
/// `sizes` maps team -> (members, score) for teams `1..=n_teams`; missing
/// teams count as empty.
pub fn pick_team(sizes: &BTreeMap<TeamId, (u32, u32)>, n_teams: u8) -> TeamId {
    let n = n_teams.clamp(1, MAX_TEAMS);
    (1..=n).min_by_key(|t| {
        let (m, s) = sizes.get(t).copied().unwrap_or((0, 0));
        (m, s, *t)
    }).unwrap()
}

/// Member counts per team `1..=n_teams` (0 for empty teams).
pub fn team_sizes<'a>(assign: impl IntoIterator<Item = (&'a SeatId, &'a TeamId)>, n_teams: u8) -> BTreeMap<TeamId, u32> {
    let mut m: BTreeMap<TeamId, u32> = (1..=n_teams.clamp(1, MAX_TEAMS)).map(|t| (t, 0)).collect();
    for (_, t) in assign {
        if let Some(c) = m.get_mut(t) {
            *c += 1;
        }
    }
    m
}

/// Uneven-team compensation: each team gets
/// `(largest - own size) * per_missing` extra CUSTOM budget points.
pub fn uneven_budget(sizes: &BTreeMap<TeamId, u32>, per_missing: u16) -> BTreeMap<TeamId, u16> {
    let max = sizes.values().copied().max().unwrap_or(0);
    sizes.iter().map(|(&t, &n)| (t, ((max - n) as u16).saturating_mul(per_missing))).collect()
}

/// Class upgrades a short team gets under CLASSES ONLY: one per missing
/// member, at most 3 (the ladder has 4 rungs).
pub fn uneven_class_steps(sizes: &BTreeMap<TeamId, u32>, team: TeamId) -> u8 {
    let max = sizes.values().copied().max().unwrap_or(0);
    let own = sizes.get(&team).copied().unwrap_or(max);
    (max - own).min(3) as u8
}

/// Class ladder for the uneven-team upgrade (Peasant -> Brute
/// -> MAA -> Knight). `duelist` is not on the ladder; by cost it sits
/// between brute and man_at_arms, so it upgrades to man_at_arms.
pub const CLASS_LADDER: [&str; 4] = ["peasant", "brute", "man_at_arms", "knight"];

pub fn upgrade_class(class: &str, steps: u8) -> &str {
    if steps == 0 {
        return class;
    }
    let rung = match class {
        "duelist" => 1, // treated as brute-level: one step => man_at_arms
        c => match CLASS_LADDER.iter().position(|x| *x == c) {
            Some(i) => i,
            None => return class, // unknown / "none": leave it alone
        },
    };
    CLASS_LADDER[(rung + steps as usize).min(CLASS_LADDER.len() - 1)]
}

/// Forced cloth tint per team: index into the 8-entry tint table
/// (`hsmp_catalog.lua` C.tints: 1 CRIMSON, 2 AZURE, 3 FOREST, 4 OCHRE, ...).
pub fn team_tint(team: TeamId) -> Option<u8> {
    match team {
        1..=7 => Some(team),
        _ => None,
    }
}

/// Map side used by `team` in `round` (1-based): sides rotate every round so
/// two teams swap ends each round.
pub fn spawn_side(team: TeamId, round: u32, n_teams: u8) -> u8 {
    if team == NO_TEAM || n_teams == 0 {
        return 0;
    }
    let n = n_teams as u32;
    (((team as u32 - 1) + round.saturating_sub(1)) % n) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seat: SeatId, key: &str, rating: i32) -> SeatInfo {
        SeatInfo { seat, key: key.into(), rating }
    }

    #[test]
    fn balance_is_even_and_spreads_rating() {
        let seats = vec![s(1, "a", 1500), s(2, "b", 1400), s(3, "c", 1000), s(4, "d", 900), s(5, "e", 800)];
        let a = balance(&seats, 2);
        let sizes = team_sizes(a.iter(), 2);
        assert_eq!(sizes[&1] + sizes[&2], 5);
        assert!(sizes[&1].abs_diff(sizes[&2]) <= 1);
        assert_ne!(a[&1], a[&2], "two best players split");
    }

    #[test]
    fn balance_ignores_input_order() {
        let mut seats = vec![s(1, "x", 0), s(2, "y", 0), s(3, "z", 0), s(4, "w", 0)];
        let a = balance(&seats, 2);
        seats.reverse();
        assert_eq!(a, balance(&seats, 2));
    }

    #[test]
    fn pick_team_smallest_then_score_then_id() {
        let mut m = BTreeMap::new();
        m.insert(1, (2, 0));
        m.insert(2, (1, 5));
        assert_eq!(pick_team(&m, 2), 2);
        m.insert(2, (2, 5));
        assert_eq!(pick_team(&m, 2), 1, "tie on size -> lower score");
        m.insert(1, (2, 5));
        assert_eq!(pick_team(&m, 2), 1, "full tie -> lower id");
        assert_eq!(pick_team(&m, 3), 3, "empty team 3");
    }

    #[test]
    fn uneven_compensation() {
        let mut sizes = BTreeMap::new();
        sizes.insert(1, 5);
        sizes.insert(2, 4);
        let b = uneven_budget(&sizes, 10);
        assert_eq!((b[&1], b[&2]), (0, 10));
        assert_eq!(uneven_class_steps(&sizes, 2), 1);
        assert_eq!(uneven_class_steps(&sizes, 1), 0);
        assert_eq!(upgrade_class("peasant", 1), "brute");
        assert_eq!(upgrade_class("brute", 1), "man_at_arms");
        assert_eq!(upgrade_class("duelist", 1), "man_at_arms");
        assert_eq!(upgrade_class("knight", 2), "knight");
        assert_eq!(upgrade_class("none", 1), "none");
    }

    #[test]
    fn sides_swap_each_round() {
        assert_eq!((spawn_side(1, 1, 2), spawn_side(2, 1, 2)), (0, 1));
        assert_eq!((spawn_side(1, 2, 2), spawn_side(2, 2, 2)), (1, 0));
        assert_eq!(team_tint(1), Some(1));
        assert_eq!(team_tint(0), None);
    }
}

//! Vitals rules on the `vitals` record (owner-authoritative vitals stream,
//! docs/development/subsystems/vitals.md).
//!
//! Included from proto.rs as `proto::vitals`. The record itself
//! (`hsmp_ipc::schema::combat::Vitals`: seq, dismemberment bitmask, flags, 19 scalars
//! quantised ONCE by the game at 1/64, 0xFFFF = unknown) is the ONE representation from the
//! game's slot to every receiver's `peer_vitals` slot; nothing here encodes or decodes it.
//! What remains is the owner sidecar's send policy (deadband, urgency) and small helpers.
//!
//! Scalar order = `VITALS_NAMES` (HSMPCombat `VITALS`). Append only.
#![allow(dead_code)]

#[allow(unused_imports)]
pub use hsmp_ipc::schema::combat::{
    vitals_dq, vitals_q, Vitals, VF_ALL as F_ALL, VF_DEAD as F_DEAD, VF_DOWNED as F_DOWNED, VF_FALLEN as F_FALLEN,
    VF_HEADLESS as F_HEADLESS, VF_PAIN_SHOCK as F_PAIN_SHOCK, VITALS_HP_CAP as HP_CAP, VITALS_N as N,
    VITALS_NAMES as NAMES, VITALS_SCALE as SCALE, VITALS_STAMINA_CAP as STAMINA_CAP, VITALS_UNKNOWN as UNKNOWN,
};

pub const I_HEALTH: usize = 0;
/// Neck Health (gates the Snap Neck rule of the damage cap).
pub const I_NECK: usize = 2;
pub const I_STAMINA: usize = 13;
/// Indices 0..=12 are health-type (anti-cheat ceiling HP_CAP).
pub const LAST_HEALTH_TYPE: usize = 12;

/// Per-field send deadband (owner side): a change smaller than this since
/// the last SENT frame does not by itself trigger a send.
pub const DEADBAND: [f32; N] = [
    0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, // health-type
    0.5, 0.5,  // consciousness
    0.5, 0.5,  // stamina, exhaustion
    0.05, 0.05, 0.25, // bleeding, blood rate, pain
    0.05,      // fallen rate
];

/// A record with every scalar unknown (tests, tools).
pub fn unknown() -> Vitals {
    Vitals { seq: 0, dism: 0, flags: 0, v: [UNKNOWN; N] }
}

/// Scalar `i` as a value (None = unknown).
#[inline]
pub fn get(f: &Vitals, i: usize) -> Option<f32> {
    vitals_dq(f.v[i])
}

/// Set scalar `i` (quantised and clamped like the game does; non-finite = unknown).
#[inline]
pub fn set(f: &mut Vitals, i: usize, x: f32) {
    f.v[i] = vitals_q(i, x);
}

/// The deadband test: true when `a` differs from `prev` enough to be worth a
/// datagram (flags, parts, a field appearing / vanishing, or any field moving by at
/// least its DEADBAND).
pub fn differs(a: &Vitals, prev: &Vitals) -> bool {
    if a.flags != prev.flags || a.dism != prev.dism {
        return true;
    }
    a.v.iter().zip(prev.v.iter()).enumerate().any(|(i, (&x, &y))| match (x == UNKNOWN, y == UNKNOWN) {
        (false, false) => (x as f32 - y as f32).abs() >= DEADBAND[i] * SCALE,
        (true, true) => false,
        _ => true,
    })
}

/// Flags or dismemberment changed: send at once, ignoring the rate cap.
pub fn urgent_vs(a: &Vitals, prev: &Vitals) -> bool {
    a.flags != prev.flags || a.dism != prev.dism
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vitals {
        let mut f = unknown();
        let vals = [87.5, 100.0, 25.0, 61.3, 59.9, 100.0, 12.25, 0.0, 99.99, 100.0, 100.0,
                    53.556266, 88.198543, 97.355716, 0.0, 1.75, 1.5, 33.3, 0.558];
        for (i, x) in vals.iter().enumerate() {
            set(&mut f, i, *x);
        }
        f.flags = F_FALLEN | F_PAIN_SHOCK;
        f
    }

    #[test]
    fn quantum_and_unknown() {
        let f = sample();
        assert!((get(&f, 11).unwrap() - 53.556266).abs() <= 0.5 / SCALE + 1e-4);
        let mut g = unknown();
        set(&mut g, 0, -12.0);
        set(&mut g, 1, 1e9);
        set(&mut g, 2, f32::INFINITY);
        assert_eq!(get(&g, 0), Some(0.0));
        assert!(g.dead());
        assert_eq!(get(&g, 1), Some(HP_CAP));
        assert!(get(&g, 2).is_none() && get(&g, 3).is_none());
        assert_eq!(unknown().health(), None);
        assert!(!unknown().dead());
        assert_eq!(core::mem::size_of::<Vitals>(), 48, "one record: 48 bytes (was a 43+ byte codec frame)");
    }

    #[test]
    fn deadband_rules() {
        let a = sample();
        let mut b = a;
        set(&mut b, I_STAMINA, get(&a, I_STAMINA).unwrap() + 0.3); // below the 0.5 stamina band
        set(&mut b, 0, 87.5 - 0.2);                                  // below the 0.25 health band
        assert!(!differs(&b, &a));
        set(&mut b, 0, 87.5 - 0.3);                                  // ≥ 0.25
        assert!(differs(&b, &a));
        let mut c = a;
        c.v[5] = UNKNOWN; // field became unreadable
        assert!(differs(&c, &a));
        let mut d = a;
        d.flags |= F_DOWNED;
        assert!(differs(&d, &a) && urgent_vs(&d, &a));
        let mut e = a;
        e.dism |= 1 << 8;
        assert!(urgent_vs(&e, &a));
        assert!(!urgent_vs(&b, &a));
    }
}

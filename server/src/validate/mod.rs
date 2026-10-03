//! Server-side validation of client claims.
//!
//! - [`damage`]: damage plausibility, `f(weapon class, contact speed, victim
//!   armour layer)`, a server-side cap that replaces trust in the
//!   attacker-reported Health loss.
//! - [`cheat`]: per-player cheat-score counters (lag-comp exploits, clash
//!   spam, clamped damage), snapshot as JSON for RCON/admin.
//! - [`pose`]: pose plausibility (weapon geometry per class, arm reach).
//! - [`rate`]: token bucket shared by the per-claim rate limits.
//!
//! Clash (parry) adjudication needs the rewound history and lives in
//! `lagcomp.rs` (`Store::parried`), rate-limited with [`rate::Bucket`].

#![allow(dead_code)] // RCON / admin consumers land later

pub mod cheat;
pub mod damage;
pub mod input;
pub mod pose;
pub mod rate;

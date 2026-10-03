//! HSMP combat simulator.
//!
//! Drives the REAL server combat code — `lagcomp` (rewind, clock maps, swept
//! blades, clash adjudication), `combat::Engine` (claims, holds, dedup, owner
//! acks) and `validate` (damage caps, reach, cheat counters) — with synthetic
//! 2–8 player fights: procedural pose streams (swings, thrusts, parries,
//! feints, clinches, falls), per-client clocks with drift, netsim-profile
//! links (latency, jitter, loss, duplication, reordering, spikes), each
//! client's jitter-buffered display of the others, and the HSMPCombat claim
//! path (ats / vts / vats, per-field deltas, clash reports, resends).
//!
//! See `sim` for the model and `tests/combat_sim.rs` for the G0 thresholds.

#![allow(dead_code, unused_imports, unused_variables, unused_mut, clippy::all)]

#[path = "../../../server/src/proto.rs"]
pub mod proto;
pub use hsmp_pose::posecodec;
#[path = "../../../server/src/lagcomp.rs"]
pub mod lagcomp;
#[path = "../../../server/src/combat.rs"]
pub mod combat;
#[path = "../../../server/src/validate/mod.rs"]
pub mod validate;

/// `loadout::catalog` lifted from server/src/loadout.rs by build.rs.
pub mod loadout {
    include!(concat!(env!("OUT_DIR"), "/catalog.rs"));
    pub use catalog::KitSel;
}

pub mod sim;

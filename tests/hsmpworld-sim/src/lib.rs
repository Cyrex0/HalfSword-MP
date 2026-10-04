//! HSMP world-sync simulator.
//!
//! Every client is a real HSMPWorld (`mods/HSMPWorld/Scripts/main.lua`) in its own Lua 5.4
//! state, on a mocked UE4SS whose bodies are simulated by `phys` (each client steps its own
//! physics with its own frame times, so free bodies drift apart exactly as unsynchronised
//! solvers do). Their shared-memory records go through the REAL sidecar tables
//! (`server/src/world_rx.rs`), netsim-profile links (`net`) and the REAL server rules
//! (`server/src/world.rs`), byte for byte (the records are framed and validated as on the
//! wire).
//!
//! `sim` drives scenarios with mod-side actions only (pawns walking into props, pickups,
//! throws by impulse, pokes) and measures what the players would see: per-object position
//! and rotation divergence between instances over time, snaps, ownership flips, hold
//! conflicts and world bandwidth. See `tests/world_sync.rs` for the G0 thresholds and
//! docs/development/subsystems/world-replication.md ("Measuring sync").
//!
//! Diagnostics (environment, stderr): `SIMTRACE=<phase>` every moving body's pose per client
//! (add `SIMFW=1` for the follower's state and buffer), `SIMSNAP=1` each snap and ownership
//! fight, `SIMPATH=1` follower path errors above 20 cm, `SIMDBG=1` held items far from the
//! stand-in's hand. `HSMPWORLD_MAIN=<file>` runs another main.lua (before / after comparisons).

pub mod proto {
    pub type PeerId = u32;
}

#[allow(dead_code, clippy::all)]
#[path = "../../../server/src/world.rs"]
pub mod world;

#[allow(dead_code, clippy::map_entry)]
#[path = "../../../server/src/world_rx.rs"]
pub mod world_rx;

pub mod game;
pub mod net;
pub mod phys;
pub mod report;
pub mod sim;

/// Deterministic xorshift RNG (every run is a pure function of its seed).
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng { Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1) }
    pub fn u(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 { (self.u() >> 11) as f64 / (1u64 << 53) as f64 }
    pub fn range(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.f() }
    pub fn fork(&mut self, k: u64) -> Rng { Rng::new(self.u() ^ k.wrapping_mul(0xA24B_AED4_963E_E407)) }
}

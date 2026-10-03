//! The server under test: the real `lagcomp::Store` and `combat::Engine`,
//! driven with the simulator's clock (server ms = true sim ms). This is the
//! only seam between the simulator and the server code; the "before" column
//! of the report was produced by swapping this file for an adapter over the
//! pre-change API (docs/development/subsystems/combat.md).

use crate::combat::{self, Ctx, Engine, Verdict};
use crate::lagcomp::{self, Store};
use crate::proto::{DamageEvent, PeerId};

pub struct Core {
    pub lc: Store,
    eng: Engine,
}

impl Core {
    pub fn new() -> Core { Core { lc: Store::default(), eng: Engine::default() } }

    pub fn on_claim(&mut self, ctx: &Ctx, hit: &mut DamageEvent, now_ms: i64) -> Verdict {
        self.eng.on_damage(&mut self.lc, ctx, hit, now_ms)
    }

    pub fn flush(&mut self, now_ms: i64) -> Vec<(PeerId, DamageEvent, Verdict)> {
        self.eng.flush_pending(&mut self.lc, now_ms)
    }

    pub fn reject_decision(&mut self, attacker: PeerId, hit_id: u32, reason: &str) {
        self.eng.reject_decision(attacker, hit_id, reason)
    }

    pub fn owner_ack(&mut self, attacker: PeerId, hit_id: u32, now_ms: i64) -> bool {
        self.eng.on_owner_ack(&mut self.lc, attacker, hit_id, now_ms)
    }

    pub fn note_death(&mut self, id: PeerId, now_ms: i64) { self.lc.note_death(id, now_ms) }

    /// Newly validated clashes (reporter, other) for the glint.
    pub fn judge_due(&mut self, now_ms: i64) -> Vec<(PeerId, PeerId)> { self.lc.judge_due(now_ms) }

    /// One-shot early "confirm" for an accepted (forwarded or held) claim.
    pub fn take_confirm(&mut self, attacker: PeerId, hit_id: u32) -> bool { self.eng.take_confirm(attacker, hit_id) }
    pub const EARLY_CONFIRM: bool = true;
    pub const CONFIRM_REASON: &'static str = combat::CONFIRM_REASON;
}

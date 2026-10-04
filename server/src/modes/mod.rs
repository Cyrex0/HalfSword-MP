//! HSMP game-mode framework. Integration contract:
//! docs/development/subsystems/modes.md.
//!
//! **The core owns time and connectivity; the mode owns meaning.** A mode
//! is a pure state machine over `ModeCtx` (server ms, roster view, event
//! outbox). It never sends packets. The reducer calls the hooks, drains
//! `ctx.out`, and reconciles its phase from `GameMode::status()`.
//!
//! This directory compiles standalone as the `hsmp-modes` crate
//! (crates/hsmp-modes) and can also be mounted inside hsmp-server as
//! `mod modes;`: the files only use `super::` paths and depend on serde only.

pub mod duel;
pub mod ffa_lms;
pub mod lts;
pub mod round;
pub mod teams;
pub mod traits;
pub mod zone;

pub use traits::*;

/// Mode ids accepted by `create` (`--mode`, `SetConfig.mode`).
pub const MODE_IDS: &[&str] = &["duel", "ffa", "lts"];

/// Build a mode by id. Unknown ids return None (the reducer refuses the
/// config). `"lms"` is accepted as an alias of `"ffa"`.
pub fn create(id: &str) -> Option<Box<dyn GameMode>> {
    match id.trim().to_ascii_lowercase().as_str() {
        "duel" => Some(Box::new(duel::Duel::new())),
        "ffa" | "lms" | "ffa_lms" => Some(Box::new(ffa_lms::FfaLms::new())),
        "lts" => Some(Box::new(lts::Lts::new())),
        _ => None,
    }
}

/// Static info for every mode (browser labels, validation).
pub fn infos() -> [&'static ModeInfo; 3] {
    [&duel::INFO, &ffa_lms::INFO, &lts::INFO]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_knows_every_id() {
        for id in MODE_IDS {
            let m = create(id).unwrap();
            assert_eq!(m.info().id, *id);
            assert_eq!(m.status(), &ModeStatus::Idle);
        }
        assert!(create("br").is_none());
        assert_eq!(create(" LMS ").unwrap().info().id, "ffa");
        assert_eq!(infos().len(), MODE_IDS.len());
    }

    #[test]
    fn modes_are_send() {
        fn is_send<T: Send>() {}
        is_send::<Box<dyn GameMode>>();
    }
}

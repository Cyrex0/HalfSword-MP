//! `hsmp-tools ipc-ctl`: push one `dev_cmd` record into a running game's DevCtl ring (dev
//! builds, `DEVCTL` cap; see docs/development/ipc-shared-memory.md). The mods read it with
//! `IPC.dev_poll` (the native module fans it out to every Lua state).
//!
//!   hsmp-tools ipc-ctl --pid <gamePID> autotest pick_arena Alley
//!   hsmp-tools ipc-ctl --pid <gamePID> tune servo 0.8
//!   hsmp-tools ipc-ctl --pid <gamePID> tdiag on
//!
//! Prints `{"id":<dev_cmd id>,"req_id":"<ring req id>"}`. Exit 0 = queued, 3 = the ring is
//! full (the game is not polling), 2 = error.

use anyhow::{bail, Context, Result};
use hsmp_ipc::schema::dev::{DevCmd, DEV_OP_AUTOTEST, DEV_OP_TDIAG, DEV_OP_TUNE};
use hsmp_ipc::shm::Access;
use hsmp_tools::ipcgame as ig;

#[derive(clap::Args)]
pub struct Args {
    /// The game process id (the mapping name is derived from it and its creation time).
    #[arg(long)]
    pub pid: Option<u32>,
    /// The mapping name, instead of --pid.
    #[arg(long)]
    pub name: Option<String>,
    /// The command id echoed in the mod's log line (default: derived from the clock).
    #[arg(long)]
    pub id: Option<u32>,
    #[command(subcommand)]
    pub op: Op,
}

#[derive(clap::Subcommand)]
pub enum Op {
    /// An autotest command for HSMPMenu (`join`, `host`, `pick_arena`, ...), with an optional argument.
    Autotest { cmd: String, arg: Option<String> },
    /// A pose-tune knob for HSMPAvatars (`servo`, `wpn`, `gain`, ...) and its value.
    Tune { key: String, value: f64 },
    /// HSMPSync transport diagnostics on / off.
    Tdiag { state: String },
}

/// The record for `op` (validated by the schema).
pub fn build(id: u32, op: &Op) -> Result<DevCmd> {
    let c = match op {
        Op::Autotest { cmd, arg } => {
            if cmd.is_empty() || cmd.len() > 32 {
                bail!("autotest command must be 1..=32 bytes");
            }
            if arg.as_deref().is_some_and(|a| a.len() > 192) {
                bail!("autotest argument over 192 bytes");
            }
            DevCmd::new(id, DEV_OP_AUTOTEST, cmd, arg.as_deref().unwrap_or(""), 0.0)
        }
        Op::Tune { key, value } => {
            if key.is_empty() || key.len() > 32 {
                bail!("tune key must be 1..=32 bytes");
            }
            if !value.is_finite() {
                bail!("tune value must be finite");
            }
            DevCmd::new(id, DEV_OP_TUNE, key, "", *value)
        }
        Op::Tdiag { state } => {
            let on = match state.as_str() {
                "on" | "1" | "true" => 1.0,
                "off" | "0" | "false" => 0.0,
                other => bail!("tdiag on|off, not {other:?}"),
            };
            DevCmd::new(id, DEV_OP_TDIAG, "tdiag", "", on)
        }
    };
    hsmp_ipc::schema::check_payload(c_kind(), hsmp_ipc::bytemuck::bytes_of(&c)).map_err(|e| anyhow::anyhow!("dev_cmd: {e}"))?;
    Ok(c)
}

fn c_kind() -> u16 {
    hsmp_ipc::schema::dev::K_DEV_CMD
}

pub fn run(a: Args) -> Result<i32> {
    let id = a.id.unwrap_or_else(|| (hsmp_ipc::shm::random_u64() as u32).max(1));
    let c = build(id, &a.op)?;
    let m = super::ipc_dump::open(a.pid, a.name.as_deref(), Access::ReadWrite)?;
    let s = ig::segment(&m)?;
    let caps = s.header.game.caps.load(std::sync::atomic::Ordering::Acquire);
    if caps & hsmp_ipc::schema::CAP_DEVCTL == 0 {
        eprintln!("ipc-ctl: warning: the game does not offer DEVCTL (not a dev build?); the command may be ignored");
    }
    match ig::devctl_push(s, &c) {
        Ok(req) => {
            println!("{}", serde_json::json!({"id": id, "req_id": format!("{req:016x}")}));
            Ok(0)
        }
        Err(e) if format!("{e}").contains("Full") => {
            eprintln!("ipc-ctl: DevCtl ring full (is the game polling dev_poll?)");
            Ok(3)
        }
        Err(e) => Err(e).context("devctl"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_every_op() {
        let c = build(5, &Op::Autotest { cmd: "pick_arena".into(), arg: Some("Alley".into()) }).unwrap();
        assert_eq!((c.id, c.op), (5, DEV_OP_AUTOTEST));
        assert_eq!(c.key, "pick_arena");
        assert_eq!(c.arg, "Alley");
        let c = build(6, &Op::Tune { key: "servo".into(), value: 0.8 }).unwrap();
        assert_eq!((c.op, c.num), (DEV_OP_TUNE, 0.8));
        let c = build(7, &Op::Tdiag { state: "off".into() }).unwrap();
        assert_eq!((c.op, c.num), (DEV_OP_TDIAG, 0.0));
        assert!(build(8, &Op::Tdiag { state: "maybe".into() }).is_err());
        assert!(build(8, &Op::Tune { key: "x".into(), value: f64::NAN }).is_err());
        assert!(build(8, &Op::Autotest { cmd: "x".repeat(33), arg: None }).is_err());
    }

    #[test]
    fn push_lands_in_the_devctl_ring() {
        let g = ig::GameHost::anonymous(hsmp_ipc::schema::CAP_DEVCTL).unwrap();
        let c = build(9, &Op::Tune { key: "gain".into(), value: 1.5 }).unwrap();
        ig::devctl_push(g.seg(), &c).unwrap();
        let mut r = hsmp_ipc::ring::Record::default();
        assert_eq!(g.seg().devctl().pop(0, &mut r), hsmp_ipc::ring::Pop::Record);
        assert_eq!(r.hdr.kind, hsmp_ipc::schema::dev::K_DEV_CMD);
        let j = ig::record_json(&r);
        assert_eq!(j["kind"], "dev_cmd");
        assert_eq!(j["v"]["key"], "gain");
        assert_eq!(j["v"]["num"], 1.5);
        assert!(j["v"].get("_invalid").is_none(), "{j}");
    }
}

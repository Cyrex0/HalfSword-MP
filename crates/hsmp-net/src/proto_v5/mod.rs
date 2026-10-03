//! Protocol constants shared by every message (`docs/development/protocol.md`).
//!
//! The v5 serde message types are gone: every application message is a typed v6 record
//! (`hsmp_ipc::wire`: `[WireHdr kind,aux,peer][record payload]`, docs/development/
//! ipc-shared-memory.md). What stays here is what the records and the channels
//! still name: the numeric codes ([`codes`]: `Phase`, `CmdReason`, `AdminRole`, ...) and the
//! channel-0 / supersede keys ([`keys`]).

pub mod codes;
pub mod keys;

pub use codes::*;

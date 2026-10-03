//! HSMP-SHM: the shared-memory IPC between the game's Lua mods (through the HSMPNative
//! module) and `hsmp-sidecar`. Design: `docs/development/ipc-shared-memory.md`.
//!
//! One named, pagefile-backed segment per game process ([`shm::Mapping`]), laid out as one
//! `#[repr(C)]` [`segment::Segment`]:
//! - [`seqlock::SeqSlot`]: latest-value slots for small streams (pose, root, vitals, ...);
//! - [`triple::TripleBuf`]: lock-free triple buffers for large state blobs (session, world);
//! - [`ring::Ring`]: SPSC message rings with ids for commands and events;
//! - [`header`]: the frozen prefix (magic, ABI, layout hash, refuse code), per-side blocks
//!   with epochs, heartbeats and counters, and the world / session epochs;
//! - [`schema`]: every payload, one file per domain, with kind ids and capability bits;
//! - [`record`] / [`wire`]: typed records (validated in place) and their 8-byte wire header;
//! - [`handshake`]: init (game), validate and attach (sidecar), refusal;
//! - [`gen`]: the C header and Lua schema generators.
//!
//! Every primitive has exactly one writer thread in one process. All cross-process data is
//! accessed through atomics (payload words with relaxed `AtomicU64` copies), so there is no
//! data race in the Rust memory model even when a reader races a writer, and readers clamp
//! every count and length, so a scribbled segment yields wrong data, never a fault.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "json")]
pub mod debug_json;
pub mod gen;
pub mod handshake;
pub mod header;
pub mod layout;
pub mod record;
pub mod ring;
pub mod schema;
pub mod segment;
pub mod seqlock;
pub mod shm;
pub mod triple;
pub mod wire;

/// Re-exported for the `ipc_pod!` expansion.
pub use bytemuck;

/// Incompatible layout change.
pub const ABI_MAJOR: u16 = 2;
/// Append-only change (gated by caps; also changes the layout hash).
pub const ABI_MINOR: u16 = 0;

pub use header::{Header, RefuseCode, SideState};
pub use layout::{IpcType, Pod};
pub use segment::{Segment, LAYOUT_HASH, SEGMENT_SIZE};
pub use shm::Mapping;

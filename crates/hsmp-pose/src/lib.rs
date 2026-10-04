//! HSMP pose: the codec (v1 `posecodec`, codec v2 `posecodec::v2`), the game-side sample
//! encoder (`sample`: the ONE place a pose frame, a root and a weapon record are built,
//! used by HSMPNative's hot paths and the tools that stand in for the game) and the
//! receiver jitter buffer (`poseplay`). Pure Rust, no IO.
//!
//! The canonical pose record is the codec-v2 frame produced once at the source
//! (docs/development/ipc-shared-memory.md): nobody re-encodes it.

pub mod feelsim;
pub mod posecodec;
pub mod poseplay;
pub mod sample;

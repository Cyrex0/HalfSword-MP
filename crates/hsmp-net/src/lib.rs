//! HSMP wire protocol (v6; see `docs/development/protocol.md`).
//!
//! * [`net`]: the transport. Handshake (X25519 + HKDF-SHA256 +
//!   ChaCha20-Poly1305, stateless cookie, Ed25519 player identity), the
//!   connection layer (`conn_id | pkt_seq | ack | ack_bits | flags`, a 1024
//!   packet replay window, per-direction keys, key update) and three channels
//!   (0 unreliable-latest, 1 reliable-unordered, 2 reliable-ordered) with RTO
//!   retransmission and fragmentation.
//! * [`proto_v5`]: the protocol codes and channel keys. Application messages are typed v6
//!   records (`hsmp_ipc::wire`), not part of this crate.
//! * [`fuzz`]: panic-free entry points for `cargo-fuzz` targets.
//!
//! Everything is sans-IO: callers pass datagrams and a monotonic `now_ms`
//! in, and take datagrams out. No sockets, no threads, no clocks inside.

#![forbid(unsafe_code)]

pub mod build;
pub mod fuzz;
pub mod net;
pub mod proto_v5;

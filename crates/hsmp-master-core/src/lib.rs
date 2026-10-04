//! The HSMP server list, shared by the game server (signing), the self-hosted `hsmp-master`
//! and the Cloudflare Worker in `master-cf/` (validation and the registry state machine).
//!
//! API (all JSON): `POST /v1/register`, `POST /v1/heartbeat/{id}`, `GET /v1/servers`,
//! `DELETE /v1/servers/{id}`, and the punch relay (`GET /v1/punch/listen/{id}` WebSocket,
//! `POST /v1/punch`). See docs/hosting/master-server.md.

pub mod auth;
mod bucket;
pub mod dashboard;
pub mod discord;
pub mod fields;
pub mod ip;
pub mod punch;
pub mod registry;
pub mod reports;

pub use auth::SIG_HEADER;
pub use fields::{DeleteReq, HeartbeatReq, RegisterReq, RegisterResp, MAX_BODY_BYTES};
pub use registry::{Config, Effect, Entry, Listing, PunchOrder, Registry, Reply};

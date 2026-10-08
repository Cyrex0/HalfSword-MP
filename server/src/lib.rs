//! Reusable server and embedded native networking. Engine objects never cross this boundary.

mod app;
mod proto;
mod server;
mod combat;
mod lagcomp;
mod interact;
mod validate;
pub(crate) use hsmp_pose::posecodec;
mod loadout;
mod master_client;
mod nat;
mod rcon;
mod perf;
mod relay;
mod world;
mod query;
mod server_info;
mod spawns;
mod net;
mod events;
mod proc_util;
mod ipkey;
mod build_id;
mod log_init;
mod server_report;
mod stats;
mod server_mods;
pub mod native_wire;
pub mod native_mode;
pub mod native_descriptor;
pub mod native_service;
#[cfg(test)]
mod alloc_count;

pub use app::run;

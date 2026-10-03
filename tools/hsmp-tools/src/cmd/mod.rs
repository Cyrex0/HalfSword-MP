//! One module per subcommand; each has `Args` (clap) and `run(Args) -> anyhow::Result<i32>`
//! (the process exit code).

pub mod check_bp_names;
pub mod crash_triage;
pub mod dump_diff;
pub mod gen_ipc;
pub mod ipc_ctl;
pub mod ipc_dump;
pub mod ipc_game;
pub mod ipc_stress;
pub mod gen_map_data;
pub mod gvas_diff;
pub mod kismet_pp;
pub mod lua_check;
pub mod lua_test;
pub mod mapdump_summary;
pub mod net_bench;
pub mod netsim;
pub mod pose_probe;
pub mod rcon;

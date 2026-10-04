//! `hsmp-tools <subcommand>`: HSMP developer tools.
//! See docs/development/tools.md for the full map.

use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(name = "hsmp-tools", version, about = "HSMP developer tools (netsim, lints, generators, Lua tests)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// UDP impairment proxy (latency / jitter / loss / dup / spikes); --self-test measures it
    Netsim(cmd::netsim::Args),
    /// Lint: CamelCase calls of space-named Blueprint functions in mod Lua
    CheckBpNames(cmd::check_bp_names::Args),
    /// Parse-check every Lua file of the mods under Lua 5.4
    LuaCheck(cmd::lua_check::Args),
    /// Run Lua test suites (tools/hsmp-tools/lua-tests/*.lua) under mlua
    LuaTest(cmd::lua_test::Args),
    /// Write the generated IPC schema files (C header, Lua schema); --check diffs them
    GenIpc(cmd::gen_ipc::Args),
    /// Read-only live view of a game's HSMP-SHM segment (--pid/--name, --follow, --json, --get)
    IpcDump(cmd::ipc_dump::Args),
    /// Write one command to a game's DevCtl ring (dev builds)
    IpcCtl(cmd::ipc_ctl::Args),
    /// Fake game for tests: create the segment, spawn the sidecar with --ipc shm:, log the game view
    IpcGame(cmd::ipc_game::GameArgs),
    /// Write one game-side slot / blob / G2S message into a segment (tests)
    IpcPut(cmd::ipc_game::PutArgs),
    /// Cross-process stress of the shared-memory IPC (kill/restart, torn reads, refusals)
    IpcStress(cmd::ipc_stress::Args),
    /// Generate server/data/maps/*.json + shared/hsmp_arenas.lua from docs/arena_static
    GenMapData(cmd::gen_map_data::Args),
    /// Heuristic diff of scalar properties between two GVAS .sav files
    GvasDiff(cmd::gvas_diff::Args),
    /// Diff two HSMP property dumps ("== Section:" / "  key = value")
    DumpDiff(cmd::dump_diff::Args),
    /// Offline benchmark of the network / IPC stack: server + N synth games + sidecars behind a counting proxy
    NetBench(cmd::net_bench::Args),
    /// End-to-end pose pipeline probe (server + 2 sidecars, optional netsim)
    PoseProbe(cmd::pose_probe::Args),
    /// Send lines to the server's RCON TCP port and print the last reply
    Rcon(cmd::rcon::Args),
    /// Kismet bytecode pretty-printer for CUE4Parse JSON dumps
    KismetPp(cmd::kismet_pp::Args),
    /// Build docs/arena_static/_summary.md from MapDump output
    MapdumpSummary(cmd::mapdump_summary::Args),
    /// Symbolise + classify game crash dumps (Saved/Crashes) -> JSON; exit 1 on new crashes
    CrashTriage(cmd::crash_triage::Args),
    /// Write a bug-report zip in the launcher's upload format (Worker e2e test)
    ReportFixture(cmd::report_fixture::Args),
    /// NAT traversal test stand-ins: a local STUN server, a punch-probe catcher
    Natlab(cmd::natlab::Args),
}

fn main() {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Netsim(a) => cmd::netsim::run(a),
        Cmd::CheckBpNames(a) => cmd::check_bp_names::run(a),
        Cmd::LuaCheck(a) => cmd::lua_check::run(a),
        Cmd::LuaTest(a) => cmd::lua_test::run(a),
        Cmd::GenIpc(a) => cmd::gen_ipc::run(a),
        Cmd::IpcDump(a) => cmd::ipc_dump::run(a),
        Cmd::IpcCtl(a) => cmd::ipc_ctl::run(a),
        Cmd::IpcGame(a) => cmd::ipc_game::run_game(a),
        Cmd::IpcPut(a) => cmd::ipc_game::run_put(a),
        Cmd::IpcStress(a) => cmd::ipc_stress::run(a),
        Cmd::GenMapData(a) => cmd::gen_map_data::run(a),
        Cmd::GvasDiff(a) => cmd::gvas_diff::run(a),
        Cmd::DumpDiff(a) => cmd::dump_diff::run(a),
        Cmd::NetBench(a) => cmd::net_bench::run(a),
        Cmd::PoseProbe(a) => cmd::pose_probe::run(a),
        Cmd::Rcon(a) => cmd::rcon::run(a),
        Cmd::KismetPp(a) => cmd::kismet_pp::run(a),
        Cmd::MapdumpSummary(a) => cmd::mapdump_summary::run(a),
        Cmd::CrashTriage(a) => cmd::crash_triage::run(a),
        Cmd::ReportFixture(a) => cmd::report_fixture::run(a),
        Cmd::Natlab(a) => cmd::natlab::run(a),
    };
    match r {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    }
}

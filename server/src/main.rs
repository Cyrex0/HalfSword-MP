//! hsmp-server — Half Sword Multiplayer relay server.
//!
//! Dedicated-server features:
//!   • Persistent banlist loaded from --bans-file (default ./bans.txt) at
//!     startup and auto-saved on every ban/unban. One `ip\n` per line, `#`
//!     comments ignored.
//!   • Optional RCON TCP admin port (--rcon-bind) — line-based protocol:
//!       AUTH <password>
//!       KICK <peer_id>
//!       BAN <peer_id>
//!       UNBAN <ip>
//!       LIST
//!       SAY <text>
//!       SHUTDOWN
//!       REPORT            the newest 10 s stats report (lines, then END)
//!       ADMIN ADD|REMOVE <peer_id|player_id|key>, ADMIN LIST
//!     Each command answers with a one-line text status.
//!   • Admins: never the first joiner. Listen host:
//!     --owner-key / --owner-key-file. Dedicated: --admin-key, --admins-file,
//!     RCON ADMIN ADD. No admin: READY players start the match themselves.
//!   • Graceful shutdown on Ctrl+C (SIGINT) or the RCON `SHUTDOWN` verb.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{info, warn};

mod proto;
mod server;
mod combat;
mod lagcomp;
mod interact; // interaction channel validation (docs/development/subsystems/interact.md)
mod validate;
pub(crate) use hsmp_pose::posecodec;
mod loadout;
mod master_client;
mod nat; // NAT traversal: router port mapping, STUN, punch probes
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
mod ipkey; // per-IP limits key IPv6 by /64
mod build_id;
mod log_init; // stdout + the --log-dir files
mod server_report; // --report: the redacted bug-report zip of this server's logs
mod stats; // the 10 s stats line, the shutdown summary, RCON REPORT

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// UDP bind address.
    #[arg(long, default_value = "0.0.0.0:7777")]
    bind: String,

    /// Server tick rate, Hz. Snapshots are broadcast at this rate.
    #[arg(long, default_value_t = 30)]
    tick_hz: u32,

    /// Max peers permitted at once. Lobby refuses JOIN beyond this.
    #[arg(long, default_value_t = 8)]
    max_peers: usize,

    /// Public-facing server name advertised to the master registry.
    #[arg(long, default_value = "Half Sword MP")]
    name: String,

    /// Game mode advertised to the master registry (duel/ffa/coop/...).
    #[arg(long, default_value = "duel")]
    mode: String,

    /// Arena map name advertised to the master registry so joiners load
    /// the same arena. Falls back to env HSMP_LOBBY_MAP; empty = unknown.
    #[arg(long, default_value = "")]
    map: String,

    /// Region tag shown in the server browser (e.g. EU, NA-East). Falls back
    /// to env HSMP_REGION. Name / mode fall back to HSMP_SERVER_NAME /
    /// HSMP_SERVER_MODE when their flags are left at the defaults.
    #[arg(long, default_value = "")]
    region: String,

    /// Persistent banlist file. Loaded on startup, saved on every change.
    /// One IP per line; `#` comments and blank lines ignored.
    #[arg(long, default_value = "bans.txt")]
    bans_file: PathBuf,

    /// Optional TCP RCON admin bind, e.g. 127.0.0.1:2345. Omit to disable. RCON is plain
    /// text: a non-loopback address is refused unless --rcon-allow-remote is also given
    /// (reach a loopback RCON through an SSH tunnel instead).
    #[arg(long)]
    rcon_bind: Option<String>,

    /// Allow --rcon-bind on a non-loopback address (password and commands travel in plain
    /// text; requires a password of at least 16 characters).
    /// The env var accepts 1/0, true/false, yes/no, on/off.
    #[arg(long, env = "HSMP_RCON_ALLOW_REMOTE", action = clap::ArgAction::SetTrue,
          value_parser = clap::builder::BoolishValueParser::new())]
    rcon_allow_remote: bool,

    /// RCON password. Required if --rcon-bind is set. Falls back to env
    /// HSMP_RCON_PASSWORD if the flag is absent.
    #[arg(long)]
    rcon_password: Option<String>,

    /// Per-client downstream budget in KB/s for replicated streams. Stream
    /// rates are thinned by distance relevance to fit it (the two nearest
    /// players never below 30 Hz). 128 (1 Mbit/s) fits 8 players with 60 Hz
    /// skeletons; raise it on a well-connected host for 16+.
    #[arg(long, default_value_t = 128)]
    client_budget_kbps: u64,

    /// Server identity (X25519 static key, 32 bytes; created on first run).
    /// Default: `$HSMP_STATE_DIR/server_identity.key`, else
    /// `%LOCALAPPDATA%\HSMP\server_identity.key`. Its public half is what
    /// clients pin (query reply / master listing `server_key`).
    #[arg(long)]
    key_file: Option<PathBuf>,

    /// Enforce this content hash instead of the built-in one (64 hex chars, see
    /// `--build-info`). Clients with other mod files or server data get a readable
    /// reject before the cookie.
    #[arg(long)]
    content_hash: Option<String>,

    /// Development only: accept clients whose mod files or server data differ from
    /// this build's (no content check). The protocol version is still checked.
    #[arg(long, env = "HSMP_ALLOW_MISMATCHED_CONTENT", action = clap::ArgAction::SetTrue,
          value_parser = clap::builder::BoolishValueParser::new())]
    allow_mismatched_content: bool,

    /// Print this build's identity (version, protocol, IPC ABI, content hash) as JSON and exit.
    #[arg(long)]
    build_info: bool,

    /// Print the identity of the mod files in the checkout at this path (this build's
    /// version, protocol and IPC ABI, the checkout's content hash) as JSON and exit.
    /// Deploy writes it as hsmp/build.json.
    #[arg(long, value_name = "REPO")]
    mods_identity: Option<PathBuf>,

    /// As --mods-identity, as the Lua module deploy writes into every mod (hsmp_build_id.lua).
    #[arg(long, value_name = "REPO")]
    mods_identity_lua: Option<PathBuf>,

    /// Enable the RCON debug verbs for the test gate (`DEBUG KILL <seat>`).
    /// Never on a public server.
    #[arg(long)]
    debug_verbs: bool,

    /// Append structured JSONL events (phase transitions, command results,
    /// seat restores) to this file (docs/development/testing.md).
    #[arg(long)]
    events: Option<PathBuf>,

    /// Write {"v":1,"role":"server","pid":..} here at startup (atomic) and
    /// remove it on clean exit (docs/development/testing.md).
    #[arg(long)]
    pid_file: Option<PathBuf>,

    /// Log files: `server-<YYYYMMDD>.log` and `server-events-<YYYYMMDD>.jsonl` (one JSON object
    /// per log event), kept 14 days / 500 MB. Default: HSMP_LOG_DIR, else
    /// `<state dir>/logs/server` (`$HSMP_STATE_DIR`, else %LOCALAPPDATA%\HSMP). A listen host
    /// (HSMP_LISTEN_HOST=1) logs to the folder the game passes (`server.log`), else stdout only.
    #[arg(long)]
    log_dir: Option<PathBuf>,

    /// No log files (stdout only).
    #[arg(long)]
    no_log_file: bool,

    /// Log level: error, warn, info (default), debug, trace, or a full RUST_LOG-style filter
    /// (`hsmp_server=debug,hsmp_net=info`). Default: RUST_LOG, else info.
    #[arg(long)]
    log_level: Option<String>,

    /// Write a bug-report zip of this server's recent logs (redacted: user names, keys,
    /// passwords; player IP addresses numbered) and exit. See --report-out / --report-upload.
    #[arg(long)]
    report: bool,

    /// `--report`: where to write the zip (default `<log dir>/hsmp-server-report-<stamp>.zip`).
    #[arg(long, requires = "report")]
    report_out: Option<PathBuf>,

    /// `--report`: also upload it to the HSMP master (`/v1/reports`) and print the report id.
    #[arg(long, requires = "report")]
    report_upload: bool,

    /// `--report`: keep player IP addresses (the server's own public address is still removed).
    #[arg(long, requires = "report")]
    report_keep_ips: bool,

    /// `--report`: how many days of logs to include.
    #[arg(long, default_value_t = 2, requires = "report")]
    report_days: u64,

    /// Exit cleanly when this process (e.g. the listen host's game) exits.
    #[arg(long)]
    parent_pid: Option<u32>,

    /// Listen server: the hosting player's Ed25519 player key (64 hex). That
    /// player is the server's OWNER (admin) whenever connected, also after
    /// a drop and reconnect. Without an owner or admins nobody is admin
    /// (never "the first to join").
    #[arg(long)]
    owner_key: Option<String>,

    /// Listen server: a file holding the host's player key (the host
    /// sidecar writes `<state-dir>/.player_key`). Re-read at each join until
    /// that player has joined, then fixed.
    #[arg(long)]
    owner_key_file: Option<PathBuf>,

    /// Dedicated server: a player key (64 hex, `hsmp-sidecar
    /// --print-player-key`) that is admin. Repeatable / comma-separated.
    #[arg(long = "admin-key")]
    admin_keys: Vec<String>,

    /// Dedicated server: admin player keys, one per line (`#` comments).
    /// Re-read when it changes; RCON `ADMIN ADD` appends to it.
    #[arg(long)]
    admins_file: Option<PathBuf>,

    /// Open the UDP port on the router automatically (UPnP-IGD, PCP, NAT-PMP), renew the
    /// lease and remove it on a clean shutdown: auto | off.
    #[arg(long, env = "HSMP_PORT_MAP", default_value = "auto")]
    port_map: String,

    /// STUN servers (host:port, comma-separated) asked from the game port for the public
    /// endpoint; "" = the public defaults (Cloudflare, Google), "off" = none (no NAT
    /// detection, no hole punching).
    #[arg(long, env = "HSMP_STUN_SERVERS", default_value = "")]
    stun: String,

    /// Take hole-punch requests relayed by the server list when the router port is not
    /// open: auto | off.
    #[arg(long, env = "HSMP_PUNCH", default_value = "auto")]
    punch: String,

    /// Test only: behave as if behind a NAT that drops unsolicited inbound UDP.
    #[arg(long, hide = true)]
    emulate_nat: bool,
}

fn switch_on(v: &str) -> bool {
    !matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no")
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(u_period: u32) -> u32;
}

/// UDP socket with large kernel buffers: fan-out bursts (N peers × frames)
/// must not overflow the default 64 KB buffers and silently drop packets.
fn bind_udp(addr: &str) -> Result<UdpSocket> {
    let sa: std::net::SocketAddr = addr.parse().context("parse --bind")?;
    let domain = if sa.is_ipv4() { socket2::Domain::IPV4 } else { socket2::Domain::IPV6 };
    let s = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    let _ = s.set_recv_buffer_size(8 << 20);
    let _ = s.set_send_buffer_size(8 << 20);
    s.set_nonblocking(true)?;
    s.bind(&sa.into())?;
    Ok(UdpSocket::from_std(s.into())?)
}

/// RCON safety defaults: a blank password is refused, and a
/// non-loopback bind needs `--rcon-allow-remote` plus a password of at least 16 characters.
fn check_rcon_config(bind: &str, password: &str, allow_remote: bool) -> Result<()> {
    if password.trim().is_empty() || password.trim() != password {
        anyhow::bail!("the RCON password must not be blank or start/end with whitespace");
    }
    let sa: std::net::SocketAddr = bind.parse().context("parse --rcon-bind (expected ip:port, e.g. 127.0.0.1:2345)")?;
    if !sa.ip().is_loopback() {
        if !allow_remote {
            anyhow::bail!(
                "--rcon-bind {bind} is not a loopback address. RCON is plain text: bind it to 127.0.0.1 and reach it \
                 through an SSH tunnel, or pass --rcon-allow-remote (HSMP_RCON_ALLOW_REMOTE=1) to accept the risk"
            );
        }
        if password.chars().count() < 16 {
            anyhow::bail!("--rcon-allow-remote needs an RCON password of at least 16 characters");
        }
        warn!(bind, "RCON listens on a non-loopback address in PLAIN TEXT (--rcon-allow-remote)");
    }
    Ok(())
}

#[cfg(test)]
mod rcon_config_tests {
    use super::check_rcon_config;

    #[test]
    fn allow_remote_flag_and_env_parse() {
        use clap::Parser;
        let parse = |a: &[&str]| super::Args::try_parse_from(a).map(|x| x.rcon_allow_remote);
        assert!(!parse(&["hsmp-server"]).unwrap());
        assert!(parse(&["hsmp-server", "--rcon-allow-remote"]).unwrap());
        // the Docker image and the docs set HSMP_RCON_ALLOW_REMOTE=1
        std::env::set_var("HSMP_RCON_ALLOW_REMOTE", "1");
        let one = parse(&["hsmp-server"]);
        std::env::set_var("HSMP_RCON_ALLOW_REMOTE", "0");
        let zero = parse(&["hsmp-server"]);
        std::env::remove_var("HSMP_RCON_ALLOW_REMOTE");
        assert!(one.unwrap());
        assert!(!zero.unwrap());
    }

    #[test]
    fn loopback_is_the_default_and_remote_needs_opt_in() {
        assert!(check_rcon_config("127.0.0.1:2345", "pw", false).is_ok());
        assert!(check_rcon_config("[::1]:2345", "pw", false).is_ok());
        assert!(check_rcon_config("0.0.0.0:2345", "a-long-enough-password", false).is_err());
        assert!(check_rcon_config("0.0.0.0:2345", "short", true).is_err());
        assert!(check_rcon_config("0.0.0.0:2345", "a-long-enough-password", true).is_ok());
        assert!(check_rcon_config("127.0.0.1:2345", " ", false).is_err());
        assert!(check_rcon_config("127.0.0.1:2345", "pw ", false).is_err());
        assert!(check_rcon_config("localhost:2345", "pw", false).is_err(), "an ip:port, not a host name");
    }
}

#[cfg(test)]
mod json_key_order_tests {
    /// The state files the sidecar writes are parsed by Lua patterns that expect
    /// serde_json's default, key-sorted maps. In one Cargo workspace a single crate that
    /// enables serde_json's `preserve_order` would switch every co-built binary to
    /// insertion order; this test fails if that ever happens.
    #[test]
    fn serde_json_maps_are_key_sorted() {
        let v = serde_json::json!({ "wins": 1, "seat": 2, "team": 0, "spawn": null });
        assert_eq!(v.to_string(), r#"{"seat":2,"spawn":null,"team":0,"wins":1}"#);
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.build_info {
        println!("{}", build_id::identity().to_json());
        return Ok(());
    }
    if let Some((repo, lua)) = args.mods_identity.as_ref().map(|r| (r, false)).or(args.mods_identity_lua.as_ref().map(|r| (r, true))) {
        let mut id = build_id::identity();
        id.content_hash = hsmp_net::build::content::content_hash_of_dir(repo)
            .with_context(|| format!("{} not found under {}", hsmp_net::build::content::TEMPLATE, repo.display()))?;
        if lua { print!("{}", id.to_lua()) } else { println!("{}", id.to_json()) }
        return Ok(());
    }
    let listen_host = std::env::var("HSMP_LISTEN_HOST").is_ok_and(|v| v.trim() == "1");
    let log_dir = server_log_dir(args.log_dir.as_deref(), listen_host, args.no_log_file);
    let filter = log_init::filter_spec(args.log_level.as_deref(), std::env::var("RUST_LOG").ok().as_deref(), "hsmp_server", "hsmp_server=info");
    if args.report {
        log_init::init_with(&filter, None);
        let dir = log_dir.clone().unwrap_or_else(|| state_base().join("logs").join("server"));
        return server_report::run(&dir, &server_report::Opts {
            out: args.report_out.clone(),
            upload: args.report_upload,
            keep_ips: args.report_keep_ips,
            days: args.report_days,
        }).await;
    }
    let _log_guard = log_init::init_with(&filter, log_dir.as_deref().map(|dir| log_init::FileOpts { dir, name: "server", daily: !listen_host, events: true }));
    install_panic_log();
    stats::start();
    if let Some(d) = &log_dir {
        info!(dir = %d.display(), daily = !listen_host, filter = %filter, "log files");
    }
    info!(bind = %args.bind, tick_hz = args.tick_hz, max_peers = args.max_peers, "hsmp-server starting");

    // Windows quantises tokio timers to the 15.6 ms system tick, which made a
    // "30 Hz" tick run at ~21 Hz (31/47 ms). 1 ms resolution fixes cadence.
    #[cfg(windows)]
    unsafe { timeBeginPeriod(1); }

    if let Err(e) = events::init(args.events.as_deref(), "server") {
        warn!(error = %e, path = ?args.events, "cannot open --events file");
    }
    let _pid_file = match &args.pid_file {
        Some(p) => match proc_util::write_pid_file(p, "server", args.parent_pid) {
            Ok(g) => Some(g),
            Err(e) => { warn!(error = %e, path = ?p, "cannot write --pid-file"); None }
        },
        None => None,
    };

    let socket = Arc::new(bind_udp(&args.bind)?);
    info!(local = %socket.local_addr()?, "udp socket bound");

    // Encrypted transport: persistent server identity (pinned by clients).
    let key_path = args.key_file.clone().unwrap_or_else(net::default_key_path);
    let static_key = net::load_or_create_key(&key_path)
        .with_context(|| format!("server key {}", key_path.display()))?;
    let content_hash = resolve_content_hash(args.content_hash.as_deref(), args.allow_mismatched_content)?;
    match content_hash {
        Some(h) => info!(version = build_id::VERSION, content_hash = %hex::encode(h), "content check on: clients must run the same mod files"),
        None => warn!(version = build_id::VERSION, built_with = build_id::CONTENT_HASH_HEX,
                      "content check OFF (--allow-mismatched-content): clients with other mod files can join"),
    }
    let transport = net::Net::new(static_key, content_hash);
    let server_key_hex = hex::encode(transport.static_public());
    info!(key_file = %key_path.display(), server_key = %server_key_hex, fingerprint = %transport.fingerprint(),
          proto = %format!("v{}..=v{}", hsmp_net::net::VERSION_MIN, hsmp_net::net::VERSION_MAX), "server identity");
    let state = Arc::new(server::ServerState::with_net(args.max_peers, transport));
    server::configure_session(&state, server::SessionOpts {
        debug_verbs: args.debug_verbs,
        tick_hz: args.tick_hz,
        mode: env_or_mode(&args.mode),
    }).await;
    // Who is admin: listen host key, configured admins, or nobody.
    server::configure_admins(&state, server::AdminOpts {
        owner_key: args.owner_key.clone(),
        owner_key_file: args.owner_key_file.clone(),
        admin_keys: args.admin_keys.clone(),
        admins_file: args.admins_file.clone(),
    }).await?;

    // Load the persistent banlist if present.
    if let Err(e) = server::load_banlist(&state, &args.bans_file).await {
        warn!(error = %e, "banlist load failed; starting empty");
    }
    // Point the server at the banlist file so it saves on every change.
    server::set_banlist_path(&state, args.bans_file.clone()).await;

    // What the server browser sees (master listing + UDP query replies).
    let env_or = |flag: &str, default: &str, var: &str| -> String {
        let v = if flag == default { std::env::var(var).ok() } else { None };
        let s = v.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| flag.to_string());
        s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
    };
    server_info::init(server_info::Advertised {
        name: query::clip(&env_or(&args.name, "Half Sword MP", "HSMP_SERVER_NAME"), 48),
        mode: query::clip(&env_or(&args.mode, "duel", "HSMP_SERVER_MODE"), 32),
        default_map: query::clip(&env_or(&args.map, "", "HSMP_LOBBY_MAP"), 64),
        region: query::clip(&env_or(&args.region, "", "HSMP_REGION"), 16),
        max_players: args.max_peers as u32,
        password: false, // join passwords are not implemented server-side yet
        server_key: server_key_hex.clone(),
        content_hash: content_hash.map(hex::encode).unwrap_or_default(),
    });

    info!(version = build_id::VERSION, protocol = %format!("v{}..=v{}", hsmp_net::net::VERSION_MIN, hsmp_net::net::VERSION_MAX),
          bind = %args.bind, local = %socket.local_addr()?, listen_host, max_peers = args.max_peers, tick_hz = args.tick_hz,
          name = %server_info::advertised().name, mode = %server_info::advertised().mode, region = %server_info::advertised().region,
          map = %server_info::advertised().default_map, content_check = content_hash.is_some(),
          master = %std::env::var("HSMP_MASTER_URL").unwrap_or_default(), rcon = ?args.rcon_bind, client_budget_kbps = args.client_budget_kbps,
          "server config");
    server::seed_arena(&state, &env_or(&args.map, "", "HSMP_LOBBY_MAP")).await;
    relay::set_client_budget_kbps(args.client_budget_kbps);
    perf::spawn_reporter();
    stats::spawn(state.clone());
    let rx = tokio::spawn(server::recv_loop(socket.clone(), state.clone()));
    let tick = tokio::spawn(server::tick_loop(
        socket.clone(),
        state.clone(),
        args.tick_hz,
    ));

    // NAT traversal: router port mapping, STUN from this socket, punch probes (nat/).
    let bound = socket.local_addr()?;
    // A loopback-bound server cannot reach public STUN servers: only an explicit list.
    let stun = if args.stun.trim().is_empty() && bound.ip().is_loopback() { Vec::new() } else { nat::stun_servers(&args.stun) };
    let nat = nat::spawn(socket.clone(), bound, nat::Opts {
        port_map: switch_on(&args.port_map),
        stun,
        punch: switch_on(&args.punch),
        emulate: args.emulate_nat,
    });
    // The listen host's owner hears how reachable the server is (NET_STATUS notice).
    tokio::spawn(server::net_status_notices(state.clone()));

    // Register with master if HSMP_MASTER_URL set.
    let mut master = None;
    if let Some(master_url) = std::env::var("HSMP_MASTER_URL").ok().filter(|u| !u.trim().is_empty()) {
        let bind = socket.local_addr()?;
        info!(master = %master_url, "registering with master");
        stats::master_url(&master_url);
        match master_client::MasterClient::new(master_url, bind, &static_key) {
            Ok(c) => {
                let c = Arc::new(c);
                // Runs forever: retries registration with backoff, re-registers if
                // the master forgets us, heartbeats live player count + map.
                tokio::spawn(c.clone().run(state.clone(), socket.clone()));
                master = Some(c);
            }
            Err(e) => warn!(error = %e, "master client disabled"),
        }
    }

    // Optional RCON admin TCP listener.
    let (rcon_shutdown_tx, mut rcon_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    if let Some(bind) = args.rcon_bind.clone() {
        let pw = args.rcon_password.clone()
            .or_else(|| std::env::var("HSMP_RCON_PASSWORD").ok())
            .context("--rcon-bind set but --rcon-password / HSMP_RCON_PASSWORD not provided")?;
        check_rcon_config(&bind, &pw, args.rcon_allow_remote)?;
        let st = state.clone();
        let sock = socket.clone();
        let sd_tx = rcon_shutdown_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = rcon::run(bind, pw, st, sock, sd_tx).await {
                warn!(error = %e, "rcon listener exited");
            }
        });
    }

    let mut host_gone = false;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("ctrl-c received; shutting down"),
        _ = rcon_shutdown_rx.recv() => info!("rcon SHUTDOWN; shutting down"),
        _ = proc_util::wait_parent_exit(args.parent_pid) => {
            info!(parent_pid = ?args.parent_pid, "parent process exited; shutting down");
            host_gone = true;
        }
        r = rx => warn!(?r, "recv task exited"),
        r = tick => warn!(?r, "tick task exited"),
    }

    // Tell every client the server is closing (S2CServerClosing + close), so
    // they show it instead of timing out.
    if host_gone {
        server::shutdown(&socket, &state, hsmp_net::proto_v5::ClosingReason::HOST_LEFT, "Host closed the server").await;
    } else {
        server::shutdown(&socket, &state, hsmp_net::proto_v5::ClosingReason::SHUTDOWN, "Server shut down").await;
    }
    if let Some(m) = &master {
        m.deregister().await;
    }
    stats::shutdown_summary();
    nat.shutdown().await;
    events::emit("server_stop", serde_json::json!({}));
    Ok(())
}

/// `$HSMP_STATE_DIR`, else %LOCALAPPDATA%\HSMP, else the working directory (as the identity key).
fn state_base() -> PathBuf {
    net::default_key_path().parent().map(PathBuf::from).unwrap_or_default()
}

/// `--log-dir` / HSMP_LOG_DIR; a listen host without one logs to stdout only; a dedicated
/// server defaults to `<state dir>/logs/server`.
fn server_log_dir(arg: Option<&std::path::Path>, listen_host: bool, off: bool) -> Option<PathBuf> {
    if off {
        return None;
    }
    match log_init::dir_from(arg) {
        Some(d) => Some(d),
        None if listen_host => None,
        None => Some(state_base().join("logs").join("server")),
    }
}

/// A panic anywhere is logged with its backtrace (and the file flushed) before the default
/// hook runs.
fn install_panic_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!(thread = std::thread::current().name().unwrap_or("?"), "panic: {info}
backtrace:
{bt}");
        log_init::flush();
        prev(info);
    }));
}

/// The advertised mode (flag, else HSMP_SERVER_MODE), as main() resolves it.
fn env_or_mode(flag: &str) -> String {
    if flag == "duel" {
        if let Ok(v) = std::env::var("HSMP_SERVER_MODE") {
            if !v.trim().is_empty() { return v.trim().to_string(); }
        }
    }
    flag.to_string()
}

/// The content hash the handshake enforces: `--content-hash`, else the one this build
/// embeds; `None` with `--allow-mismatched-content`.
fn resolve_content_hash(flag: Option<&str>, allow_mismatched: bool) -> Result<Option<[u8; 32]>> {
    if allow_mismatched {
        return Ok(None);
    }
    match flag.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => Ok(Some(hsmp_net::build::parse_hash(h).context("--content-hash: expected 64 hex chars")?)),
        None => Ok(Some(build_id::content_hash())),
    }
}

#[cfg(test)]
mod content_check_tests {
    #[test]
    fn content_check_is_on_by_default() {
        use clap::Parser;
        let a = super::Args::try_parse_from(["hsmp-server"]).unwrap();
        let h = super::resolve_content_hash(a.content_hash.as_deref(), a.allow_mismatched_content).unwrap();
        assert_eq!(h, Some(super::build_id::content_hash()));
        let a = super::Args::try_parse_from(["hsmp-server", "--allow-mismatched-content"]).unwrap();
        assert_eq!(super::resolve_content_hash(a.content_hash.as_deref(), a.allow_mismatched_content).unwrap(), None);
        let x = "ab".repeat(32);
        assert_eq!(super::resolve_content_hash(Some(&x), false).unwrap(), Some([0xab; 32]));
        assert!(super::resolve_content_hash(Some("abc"), false).is_err());
    }
}

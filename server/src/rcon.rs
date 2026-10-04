//! RCON — line-based TCP admin for the dedicated server.
//!
//! Protocol: UTF-8, LF-terminated commands and responses.
//!
//! Session:
//!   > AUTH <password>
//!   < OK authenticated
//!
//! Commands (after AUTH):
//!   > LIST                → one line per peer: "<id> <nick> <addr>"
//!                           then: "END"
//!   > KICK <peer_id>      → "OK kicked" | "ERR no such peer"
//!   > BAN  <peer_id>      → "OK banned <ip>" | "ERR no such peer"
//!   > UNBAN <ip>          → "OK unbanned" | "ERR not banned"
//!   > SAY  <text>         → "OK broadcast" — delivered as SERVER chat
//!   > BANS                → one IP per line, then "END"
//!   > SHUTDOWN            → "OK stopping"; server exits
//!
//! Match control: the same typed command path as a
//! player's `C2SCommand` (`server::run_command`, actor = rcon, always admin):
//!   > MAP <arena>         → "OK Map_Arena_Pit" | "ERR <reason>"   (Pit, Map_Arena_Pit, a path)
//!   > START [FORCE]       → "OK starting on <arena>" | "ERR start blocked: 1 of 2 peers ready"
//!   > ABORT               → "OK aborted" | "OK already in the lobby"
//!   > BESTOF <n>          → "OK config updated" | "ERR <reason>"   (lobby only, 1..=31)
//!   > KIT <mode> [budget] → kit rules (0 free, 1 classes, 2 custom; lobby only)
//!   > STATUS              → "OK {json}": phase, round, match_id, arena, roster by seat
//!   > REPORT              → the newest 10 s stats report: the `stats:` line, one `stats peer` line per player, "END"
//!   > DEBUG KILL <seat>   → "OK killed seat N ..." | "ERR <reason>"   (`--debug-verbs` only)
//!   > ADMIN ADD <peer_id|player_id|key>    → "OK admin <player_id>" (appended to
//!                           --admins-file when set, else for this run)
//!   > ADMIN REMOVE <...>  → "OK not admin <player_id>"
//!   > ADMIN LIST          → "OK <key> <owner|config|file|runtime> <online peer N nick|offline>" lines, "END"
//! BANS (RCON) lists full IPs; in-game admins only ever see them masked.
//! KICK and BAN take the same path (Command::Kick / Command::Ban).
//!
//! Any unauthenticated command (other than AUTH) returns "ERR auth required".
//!
//! Limits (`Limits`): lines are read bounded (4 KiB), a
//! connection must authenticate within 10 s, at most 16 sessions (4 per IP),
//! 5 failed AUTHs per IP lock that IP out for a minute, and an accept error
//! never ends the listener (it backs off and keeps accepting).

use crate::proto::v5;
use hsmp_ipc::layout::Str;
use hsmp_ipc::schema::session as rec;
use crate::server::{self, Actor, ServerState};
use anyhow::Result;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, Semaphore};
use tracing::{info, warn};

/// Limits of the (internet-facing, README `--rcon-bind 0.0.0.0:2345`) RCON
/// listener.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Longest accepted line (bytes, without the LF). Longer: the session ends.
    pub max_line: usize,
    /// Time to authenticate after connecting (the whole AUTH exchange).
    pub auth_timeout: Duration,
    /// An authenticated session idle this long is closed.
    pub idle_timeout: Duration,
    /// Concurrent sessions in total / per source IP.
    pub max_sessions: usize,
    pub max_per_ip: usize,
    /// Failed AUTHs per IP within `fail_window` before that IP is refused
    /// until the window passes.
    pub max_failures: u32,
    pub fail_window: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_line: 4096,
            auth_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30 * 60),
            max_sessions: 16,
            max_per_ip: 4,
            max_failures: 5,
            fail_window: Duration::from_secs(60),
        }
    }
}

/// Per-IP bookkeeping: open sessions and recent AUTH failures.
#[derive(Default)]
struct Gate {
    open: HashMap<IpAddr, usize>,
    fails: HashMap<IpAddr, (u32, std::time::Instant)>,
}

impl Gate {
    fn admit(&mut self, ip: IpAddr, lim: &Limits, now: std::time::Instant) -> Result<(), &'static str> {
        if self.fails.len() > 4096 {
            self.fails.retain(|_, (_, t)| now.duration_since(*t) < lim.fail_window);
        }
        if let Some((n, t)) = self.fails.get(&ip).copied() {
            if now.duration_since(t) >= lim.fail_window {
                self.fails.remove(&ip);
            } else if n >= lim.max_failures {
                return Err("too many failed logins; try later");
            }
        }
        if self.open.get(&ip).copied().unwrap_or(0) >= lim.max_per_ip {
            return Err("too many sessions from this address");
        }
        *self.open.entry(ip).or_default() += 1;
        Ok(())
    }
    fn close(&mut self, ip: IpAddr) {
        if let Some(n) = self.open.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 { self.open.remove(&ip); }
        }
    }
    fn failed(&mut self, ip: IpAddr, now: std::time::Instant) {
        let e = self.fails.entry(ip).or_insert((0, now));
        e.0 += 1;
        e.1 = now;
    }
}

pub async fn run(
    bind: String,
    password: String,
    state: Arc<ServerState>,
    socket: Arc<UdpSocket>,
    shutdown_tx: mpsc::Sender<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&bind).await?;
    info!(bind = %bind, "rcon listening");
    serve(listener, password, state, socket, shutdown_tx, Limits::default()).await
}

/// Accept loop. Never ends on an accept error (EMFILE, ECONNABORTED ...):
/// it logs, backs off (10 ms doubling to 1 s) and keeps accepting.
pub async fn serve(
    listener: TcpListener,
    password: String,
    state: Arc<ServerState>,
    socket: Arc<UdpSocket>,
    shutdown_tx: mpsc::Sender<()>,
    lim: Limits,
) -> Result<()> {
    let lim = Arc::new(lim);
    let sessions = Arc::new(Semaphore::new(lim.max_sessions));
    let gate = Arc::new(std::sync::Mutex::new(Gate::default()));
    let mut backoff = Duration::from_millis(10);
    loop {
        let (mut stream, from) = match listener.accept().await {
            Ok(x) => { backoff = Duration::from_millis(10); x }
            Err(e) => {
                warn!(error = %e, backoff_ms = backoff.as_millis() as u64, "rcon accept error; retrying");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(1));
                continue;
            }
        };
        let Ok(permit) = sessions.clone().try_acquire_owned() else {
            warn!(%from, "rcon: session limit reached; connection refused");
            tokio::spawn(async move { let _ = stream.write_all(b"ERR busy\n").await; });
            continue;
        };
        // Per-IP limits key IPv6 by /64.
        let ip = crate::ipkey::ip_key(from.ip());
        let admitted = gate.lock().unwrap_or_else(|e| e.into_inner()).admit(ip, &lim, std::time::Instant::now());
        if let Err(why) = admitted {
            warn!(%from, why, "rcon: connection refused");
            tokio::spawn(async move { let _ = stream.write_all(format!("ERR {}\n", why).as_bytes()).await; });
            continue;
        }
        let st = state.clone();
        let sock = socket.clone();
        let pw = password.clone();
        let sd_tx = shutdown_tx.clone();
        let lim = lim.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let _permit = permit;
            // A failed AUTH is counted before its reply is written: a client that
            // resets the connection makes the write fail, and the session would
            // return Err with the failure uncounted.
            let fail_gate = gate.clone();
            let on_fail = move || fail_gate.lock().unwrap_or_else(|e| e.into_inner()).failed(ip, std::time::Instant::now());
            let r = handle_client(stream, from.to_string(), st, sock, pw, sd_tx, &lim, on_fail).await;
            gate.lock().unwrap_or_else(|e| e.into_inner()).close(ip);
            if let Err(e) = r {
                warn!(from = %from, error = %e, "rcon session error");
            }
        });
    }
}

/// How a session ended (for the per-IP failure throttle).
#[derive(Debug, PartialEq)]
enum Session {
    Ended,
    AuthFailed,
}

/// Read one LF-terminated line of at most `max` bytes into `buf` (without
/// the LF). Ok(false) = EOF before any byte. A longer line is an error and
/// is never buffered beyond `max` (no unbounded pre-auth allocation).
async fn read_line_bounded<R: AsyncBufRead + Unpin>(rd: &mut R, buf: &mut Vec<u8>, max: usize) -> std::io::Result<bool> {
    buf.clear();
    loop {
        let avail = rd.fill_buf().await?;
        if avail.is_empty() {
            return Ok(!buf.is_empty());
        }
        match avail.iter().position(|b| *b == b'\n') {
            Some(i) => {
                if buf.len() + i > max {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
                }
                buf.extend_from_slice(&avail[..i]);
                rd.consume(i + 1);
                return Ok(true);
            }
            None => {
                let n = avail.len();
                if buf.len() + n > max {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
                }
                buf.extend_from_slice(avail);
                rd.consume(n);
            }
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    from: String,
    state: Arc<ServerState>,
    socket: Arc<UdpSocket>,
    password: String,
    shutdown_tx: mpsc::Sender<()>,
    lim: &Limits,
    on_fail: impl FnOnce(),
) -> Result<Session> {
    info!(%from, "rcon connect");
    let mut on_fail = Some(on_fail);
    let mut fail = move || { if let Some(f) = on_fail.take() { f() } };
    let (rd, mut wr) = stream.into_split();
    let mut rd = BufReader::with_capacity(lim.max_line.min(8192).max(64), rd);
    let mut raw = Vec::with_capacity(256);
    let mut authenticated = false;
    let auth_deadline = tokio::time::Instant::now() + lim.auth_timeout;

    loop {
        // Before AUTH the whole exchange has one deadline; after it, an idle
        // session times out.
        let deadline = if authenticated { tokio::time::Instant::now() + lim.idle_timeout } else { auth_deadline };
        let got = tokio::select! {
            r = read_line_bounded(&mut rd, &mut raw, lim.max_line) => match r {
                Ok(g) => g,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    warn!(%from, max = lim.max_line, "rcon: line too long; closing");
                    if !authenticated { fail(); }
                    let _ = wr.write_all(b"ERR line too long\n").await;
                    return Ok(if authenticated { Session::Ended } else { Session::AuthFailed });
                }
                Err(e) => return Err(e.into()),
            },
            _ = tokio::time::sleep_until(deadline) => {
                let msg: &[u8] = if authenticated { b"ERR idle timeout\n" } else { b"ERR auth timeout\n" };
                let _ = wr.write_all(msg).await;
                return Ok(Session::Ended);
            }
        };
        if !got {
            info!(%from, "rcon disconnect");
            return Ok(Session::Ended);
        }
        let line = String::from_utf8_lossy(&raw).into_owned();
        let cmd = line.trim();
        if cmd.is_empty() { continue; }

        if !authenticated {
            if let Some(pw) = cmd.strip_prefix("AUTH ") {
                if constant_time_eq(pw.as_bytes(), password.as_bytes()) {
                    authenticated = true;
                    wr.write_all(b"OK authenticated\n").await?;
                    info!(%from, "rcon authed");
                } else {
                    warn!(%from, "rcon: bad password");
                    fail();
                    wr.write_all(b"ERR bad password\n").await?;
                    return Ok(Session::AuthFailed);
                }
            } else {
                wr.write_all(b"ERR auth required\n").await?;
            }
            continue;
        }

        // Authed commands.
        let (verb, rest) = split_once(cmd);
        match verb {
            // LIST / BANS: build the reply under the game lock, drop the
            // lock, then write (a client that stops reading would otherwise
            // block the write with the lock held and freeze the receive
            // loop and the tick).
            "LIST" => {
                let out = {
                    let inner = state.lock().lock().await;
                    list_reply(inner.peers.iter().map(|(a, p)| (p.id, p.nick.as_str(), *a)))
                };
                wr.write_all(out.as_bytes()).await?;
            }
            "BANS" => {
                let out = {
                    let inner = state.lock().lock().await;
                    bans_reply(inner.banned_ips.iter())
                };
                wr.write_all(out.as_bytes()).await?;
            }
            "KICK" | "BAN" => {
                let Some(peer_id) = rest.trim().parse::<u32>().ok() else {
                    wr.write_all(b"ERR no such peer\n").await?;
                    continue;
                };
                let ip_s = {
                    let inner = state.lock().lock().await;
                    inner.peers.iter().find(|(_, p)| p.id == peer_id).map(|(a, _)| a.ip().to_string())
                };
                let mut cmd = rec::Command::new(0, if verb == "BAN" { rec::cmd_op::BAN } else { rec::cmd_op::KICK });
                cmd.peer_id = peer_id;
                cmd.text = Str::new(if verb == "BAN" { "banned" } else { "rcon-kick" });
                let r = server::run_command(&socket, &state, Actor::Rcon, &cmd).await;
                let line = match (r.ok.get(), verb) {
                    (true, "BAN") => format!("OK banned {}\n", ip_s.unwrap_or_default()),
                    (true, _) => "OK kicked\n".to_string(),
                    (false, _) if r.reason_code == v5::CmdReason::UNKNOWN_PLAYER => "ERR no such peer\n".to_string(),
                    (false, _) => format!("ERR {}\n", r.reason_text.lossy()),
                };
                wr.write_all(line.as_bytes()).await?;
            }
            "MAP" | "START" | "ABORT" | "BESTOF" | "KIT" => {
                let arg = rest.trim();
                let cmd = match verb {
                    "MAP" if !arg.is_empty() => Ok(rec::Command { text: Str::new(arg), ..rec::Command::new(0, rec::cmd_op::PICK_ARENA) }),
                    "MAP" => Err("usage: MAP <arena>".to_string()),
                    "START" => Ok(rec::Command { flag: arg.eq_ignore_ascii_case("FORCE").into(), ..rec::Command::new(0, rec::cmd_op::START) }),
                    "ABORT" => Ok(rec::Command::new(0, rec::cmd_op::ABORT)),
                    "BESTOF" => match arg.parse::<u8>() {
                        Ok(n) => {
                            let mut c = rec::Command::new(0, rec::cmd_op::SET_CONFIG);
                            c.patch.mask = rec::cfg::BEST_OF;
                            c.patch.best_of = n;
                            Ok(c)
                        }
                        Err(_) => Err("usage: BESTOF <1..31>".to_string()),
                    },
                    _ => {
                        let mut it = arg.split_whitespace();
                        let mode = match it.next().map(|m| m.to_ascii_lowercase()) {
                            Some(m) if m == "free" || m == "0" => Some(0u8),
                            Some(m) if m == "classes" || m == "1" => Some(1),
                            Some(m) if m == "custom" || m == "2" => Some(2),
                            _ => None,
                        };
                        let budget = it.next().and_then(|b| b.parse::<u16>().ok()).unwrap_or(100);
                        match mode {
                            Some(mode) => {
                                let mut c = rec::Command::new(0, rec::cmd_op::SET_CONFIG);
                                c.patch.mask = rec::cfg::KIT_RULES;
                                c.patch.kit_mode = mode;
                                c.patch.kit_budget = budget;
                                Ok(c)
                            }
                            None => Err("usage: KIT <free|classes|custom> [budget]".to_string()),
                        }
                    }
                };
                let line = match cmd {
                    Ok(cmd) => {
                        let r = server::run_command(&socket, &state, Actor::Rcon, &cmd).await;
                        format!("{} {}\n", if r.ok.get() { "OK" } else { "ERR" }, r.reason_text.lossy())
                    }
                    Err(usage) => format!("ERR {}\n", usage),
                };
                info!(%from, cmd = %cmd_line_for_log(verb, arg), reply = %line.trim(), "rcon");
                wr.write_all(line.as_bytes()).await?;
            }
            "ADMIN" => {
                // Admins are player keys, granted here or in --admins-file.
                let line = match server::rcon_admin(&socket, &state, rest).await {
                    Ok(s) => format!("OK {}\n", s),
                    Err(e) => format!("ERR {}\n", e),
                };
                info!(%from, cmd = %cmd_line_for_log(verb, rest), reply = %line.lines().next().unwrap_or("").trim(), "rcon");
                wr.write_all(line.as_bytes()).await?;
            }
            // REPORT: the newest 10 s stats report (stats.rs), its lines, then END.
            "REPORT" => {
                let mut out = crate::stats::last_report();
                out.push_str("
END
");
                wr.write_all(out.as_bytes()).await?;
            }
            "STATUS" => {
                let s = server::rcon_status(&state).await;
                wr.write_all(format!("OK {}\n", s).as_bytes()).await?;
            }
            "DEBUG" => {
                let mut it = rest.split_whitespace();
                let line = match (it.next().map(|s| s.to_ascii_uppercase()), it.next().and_then(|s| s.parse::<u8>().ok())) {
                    (Some(k), Some(seat)) if k == "KILL" => match server::rcon_debug_kill(&socket, &state, seat).await {
                        Ok(s) => format!("OK {}\n", s),
                        Err(e) => format!("ERR {}\n", e),
                    },
                    _ => "ERR usage: DEBUG KILL <seat>\n".to_string(),
                };
                info!(%from, cmd = %cmd_line_for_log(verb, rest), reply = %line.trim(), "rcon");
                wr.write_all(line.as_bytes()).await?;
            }
            "UNBAN" => {
                let ip: IpAddr = match rest.trim().parse() {
                    Ok(x) => x,
                    Err(_) => { wr.write_all(b"ERR bad ip\n").await?; continue; }
                };
                let removed = {
                    let mut inner = state.lock().lock().await;
                    inner.banned_ips.remove(&ip)
                };
                if removed {
                    server::save_banlist(&state).await;
                    let addrs = {
                        state.lock().lock().await.peers.keys().copied().collect::<Vec<_>>()
                    };
                    server::broadcast_admin_state(&socket, &state, &addrs).await;
                    wr.write_all(b"OK unbanned\n").await?;
                } else {
                    wr.write_all(b"ERR not banned\n").await?;
                }
            }
            "SAY" => {
                let text = rest.trim();
                if text.is_empty() {
                    wr.write_all(b"ERR empty\n").await?;
                    continue;
                }
                let addrs = {
                    state.lock().lock().await.peers.keys().copied().collect::<Vec<_>>()
                };
                server::broadcast_msg(&socket, &state, &addrs, server::server_chat_msg(text)).await;
                wr.write_all(b"OK broadcast\n").await?;
            }
            "SHUTDOWN" => {
                wr.write_all(b"OK stopping\n").await?;
                let _ = shutdown_tx.send(()).await;
                return Ok(Session::Ended);
            }
            "HELP" | "" => {
                wr.write_all(b"commands: LIST BANS KICK <id> BAN <id> UNBAN <ip> SAY <text> MAP <arena> START [FORCE] ABORT BESTOF <n> KIT <mode> [budget] STATUS ADMIN ADD|REMOVE <id|key> ADMIN LIST DEBUG KILL <seat> SHUTDOWN HELP\n").await?;
            }
            _ => {
                wr.write_all(format!("ERR unknown verb: {}\n", verb).as_bytes()).await?;
            }
        }
    }
}

/// Most BANS lines in one reply (the banlist itself is unbounded).
const MAX_BANS_LINES: usize = 1000;

/// LIST reply: "<id> <nick> <addr>" per peer, then "END".
fn list_reply<'a>(peers: impl Iterator<Item = (u32, &'a str, std::net::SocketAddr)>) -> String {
    let mut out = String::new();
    for (id, nick, addr) in peers {
        out.push_str(&format!("{} {} {}\n", id, nick, addr));
    }
    out.push_str("END\n");
    out
}

/// BANS reply: one IP per line (at most MAX_BANS_LINES, then "... N more"),
/// then "END".
fn bans_reply<'a>(ips: impl Iterator<Item = &'a IpAddr>) -> String {
    let mut out = String::new();
    let mut more = 0usize;
    for (i, ip) in ips.enumerate() {
        if i < MAX_BANS_LINES { out.push_str(&format!("{}\n", ip)); } else { more += 1; }
    }
    if more > 0 { out.push_str(&format!("... {} more\n", more)); }
    out.push_str("END\n");
    out
}

fn cmd_line_for_log(verb: &str, arg: &str) -> String {
    if arg.trim().is_empty() { verb.to_string() } else { format!("{} {}", verb, arg.trim()) }
}

fn split_once(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], &s[i+1..]),
        None => (s, ""),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut d: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) { d |= x ^ y; }
    d == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn start(lim: Limits) -> std::net::SocketAddr {
        start_with(lim, Arc::new(ServerState::new(8))).await
    }

    async fn start_with(lim: Limits, state: Arc<ServerState>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (tx, _rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _keep = _rx;
            let _ = serve(listener, "pw".into(), state, socket, tx, lim).await;
        });
        addr
    }

    /// Everything the server sends until it closes (a reset counts as
    /// closed). Panics if it keeps the connection open for 5 s.
    async fn read_all(s: &mut TcpStream) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await {
                Err(_) => panic!("server kept the connection open"),
                Ok(Ok(0)) | Ok(Err(_)) => break,
                Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn bounded_line_reader() {
        let data: &[u8] = b"AUTH pw\nxxxxxxxxxxxxxxxxxxxx\n";
        let mut rd = BufReader::with_capacity(4, data);
        let mut buf = Vec::new();
        assert!(read_line_bounded(&mut rd, &mut buf, 16).await.unwrap());
        assert_eq!(buf, b"AUTH pw");
        let e = read_line_bounded(&mut rd, &mut buf, 16).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(buf.len() <= 16, "never buffers past the limit");
        let mut rd = BufReader::new(&b""[..]);
        assert!(!read_line_bounded(&mut rd, &mut buf, 16).await.unwrap(), "EOF");
    }

    /// A pre-auth client streaming bytes without a newline is
    /// cut off at the line limit, not buffered without bound.
    #[tokio::test]
    async fn pre_auth_flood_without_newline_is_cut_off() {
        let addr = start(Limits { max_line: 1024, ..Limits::default() }).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        let junk = vec![b'A'; 64 * 1024];
        let _ = s.write_all(&junk).await; // the server may close mid-write
        // Closed (the "ERR line too long" may be lost to a reset), and the
        // listener still serves.
        let _ = read_all(&mut s).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"AUTH pw\nSHUTDOWN\n").await.unwrap();
        assert!(read_all(&mut s).await.starts_with("OK authenticated"));
    }

    #[tokio::test]
    async fn silent_client_hits_the_auth_timeout() {
        let addr = start(Limits { auth_timeout: Duration::from_millis(200), ..Limits::default() }).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        assert!(read_all(&mut s).await.contains("ERR auth timeout"));
    }

    #[tokio::test]
    async fn sessions_are_capped_and_failed_logins_throttled() {
        let addr = start(Limits { max_sessions: 2, max_per_ip: 2, max_failures: 2, ..Limits::default() }).await;
        let a = TcpStream::connect(addr).await.unwrap();
        let b = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        assert!(read_all(&mut c).await.starts_with("ERR"), "third session refused");
        drop((a, b));
        tokio::time::sleep(Duration::from_millis(100)).await;
        for _ in 0..2 {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(b"AUTH nope\n").await.unwrap();
            assert!(read_all(&mut s).await.contains("ERR bad password"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Refused before reading anything (even the right password).
        let mut s = TcpStream::connect(addr).await.unwrap();
        let r = read_all(&mut s).await;
        assert!(r.contains("too many failed logins") && !r.contains("OK authenticated"), "{r}");
    }

    /// A bad password is counted even when the client resets
    /// the connection right after sending it (the ERR write may then fail).
    #[tokio::test]
    async fn failed_login_counted_even_if_reply_write_fails() {
        let addr = start(Limits { max_failures: 3, ..Limits::default() }).await;
        for _ in 0..3 {
            let s = TcpStream::connect(addr).await.unwrap();
            let mut s = s;
            s.write_all(b"AUTH nope\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            #[allow(deprecated)] // blocking-on-drop is irrelevant for a 0 linger
            s.set_linger(Some(Duration::ZERO)).unwrap(); // RST on drop
            drop(s);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        let r = read_all(&mut s).await;
        assert!(r.contains("too many failed logins"), "{r}");
    }

    /// BANS is bounded and built before writing, so a client
    /// that never reads never holds the game lock.
    #[tokio::test]
    async fn bans_does_not_hold_the_game_lock_while_writing() {
        let state = Arc::new(ServerState::new(8));
        {
            let mut inner = state.lock().lock().await;
            for i in 0..50_000u32 {
                inner.banned_ips.insert(IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i)));
            }
        }
        let addr = start_with(Limits::default(), state.clone()).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        // Many BANS without ever reading: fills the socket buffers.
        let mut req = b"AUTH pw\n".to_vec();
        for _ in 0..400 { req.extend_from_slice(b"BANS\n"); }
        s.write_all(&req).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let got = tokio::time::timeout(Duration::from_secs(2), state.lock().lock()).await;
        assert!(got.is_ok(), "the game lock must be free while the RCON client is not reading");
    }

    #[test]
    fn bans_reply_is_bounded() {
        let ips: Vec<IpAddr> = (0..5000u32).map(|i| IpAddr::V4(std::net::Ipv4Addr::from(i))).collect();
        let r = bans_reply(ips.iter());
        assert_eq!(r.lines().count(), MAX_BANS_LINES + 2);
        assert!(r.ends_with("... 4000 more\nEND\n"));
        assert_eq!(bans_reply([].iter()), "END\n");
    }

    #[tokio::test]
    async fn good_password_still_works() {
        let addr = start(Limits::default()).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"AUTH pw\nSTATUS\n").await.unwrap();
        let mut buf = vec![0u8; 4096];
        let mut got = String::new();
        for _ in 0..20 {
            let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await.unwrap().unwrap();
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
            if got.contains("OK {") { break; }
        }
        assert!(got.starts_with("OK authenticated\n") && got.contains("OK {"), "{got}");
    }
}

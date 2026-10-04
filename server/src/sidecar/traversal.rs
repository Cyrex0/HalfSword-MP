//! Joining a host behind a NAT (docs/development/protocol.md, "NAT traversal").
//!
//! The direct handshake goes first. When the server has not answered a single datagram
//! after `DIRECT_WAIT`, the sidecar asks STUN for its own public endpoint (from the game
//! socket, so it is the mapping the host must aim at) and posts a punch request to the
//! server list. The host's server then probes that endpoint, which opens the host's NAT;
//! the sidecar's next Hello gets through. The link record's reason line tells the player
//! what is happening ("Connecting through your router...") and, when nothing worked,
//! what the host can do about it.

use hsmp_nat::{probe, stun};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

/// The direct handshake alone for this long first.
const DIRECT_WAIT: Duration = Duration::from_secs(3);
/// Punch requests while the first ones may still be in flight, this far apart.
const PUNCH_EVERY: Duration = Duration::from_secs(4);
const FAST_TRIES: u32 = 4;
/// After that, one more now and then (the host may fix its router meanwhile).
const SLOW_EVERY: Duration = Duration::from_secs(20);
/// From the start: no answer by now = tell the player the host's network blocks us.
const GIVE_UP_TEXT_AFTER: Duration = Duration::from_secs(22);
const STUN_WAIT: Duration = Duration::from_millis(1500);

pub const TEXT_TRYING: &str = "Connecting through your router...";

fn blocked_text(port: u16) -> String {
    format!("The host's network blocks incoming connections; ask them to forward UDP {port}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Punch only towards public addresses (not LAN / loopback servers).
    Auto,
    Off,
    /// Also towards private and loopback addresses (tests).
    Force,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Mode::Off,
            "force" => Mode::Force,
            _ => Mode::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub mode: Mode,
    /// Server lists (base URLs) that may relay the punch, in order.
    pub masters: Vec<String>,
    pub stun: Vec<String>,
}

/// `--stun`: "" = the public defaults, "off" = none, else a comma-separated list.
pub fn stun_servers(arg: &str) -> Vec<String> {
    let a = arg.trim();
    if a.eq_ignore_ascii_case("off") || a == "0" {
        return Vec::new();
    }
    if a.is_empty() {
        return stun::DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect();
    }
    a.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Punch probes received from the host (log / test evidence).
pub static PROBES: AtomicU32 = AtomicU32::new(0);

fn pending() -> &'static Mutex<HashMap<stun::TxId, tokio::sync::oneshot::Sender<SocketAddr>>> {
    static P: OnceLock<Mutex<HashMap<stun::TxId, tokio::sync::oneshot::Sender<SocketAddr>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A datagram on the game socket that did not come from the server.
pub(super) fn on_datagram(from: SocketAddr, data: &[u8]) {
    if let Some(stun::Parsed::Mapped { tx, addr }) = stun::parse(data) {
        if let Some(t) = pending().lock().unwrap_or_else(|e| e.into_inner()).remove(&tx) {
            let _ = t.send(addr);
        }
    } else if probe::is_probe(data) {
        let n = PROBES.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 {
            info!(%from, "punch probe from the host received: the path from the host is open");
            crate::events::emit("nat_probe_rx", serde_json::json!({"from": from.to_string()}));
        }
    }
}

/// Is `ip` an address no NAT stands in front of (same LAN / this machine)?
fn local_target(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => hsmp_nat::gateway::is_private_v4(v),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// This socket's public endpoint, from the first STUN server that answers.
async fn public_endpoint(sock: &UdpSocket, servers: &[String], v4: bool) -> Option<SocketAddr> {
    for s in servers {
        let Ok(it) = tokio::net::lookup_host(s.as_str()).await else { continue };
        let Some(addr) = it.into_iter().find(|a| a.is_ipv4() == v4) else { continue };
        let tx = stun::new_tx();
        let (otx, orx) = tokio::sync::oneshot::channel();
        pending().lock().unwrap_or_else(|e| e.into_inner()).insert(tx, otx);
        let req = stun::binding_request(&tx);
        let _ = sock.send_to(&req, addr).await;
        let again = async {
            tokio::time::sleep(STUN_WAIT / 2).await;
            let _ = sock.send_to(&req, addr).await;
            std::future::pending::<()>().await
        };
        let got = tokio::select! {
            r = tokio::time::timeout(STUN_WAIT, orx) => r.ok().and_then(|r| r.ok()),
            _ = again => None,
        };
        pending().lock().unwrap_or_else(|e| e.into_inner()).remove(&tx);
        if let Some(a) = got {
            return Some(SocketAddr::new(hsmp_master_core::ip::canonical(a.ip()), a.port()));
        }
    }
    None
}

/// Resolves host names to IPv4 only (the punch request must come from the address family
/// the game socket uses: the master checks the endpoint's IP against it).
struct Ipv4Only;

impl reqwest::dns::Resolve for Ipv4Only {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| a.is_ipv4()).collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no IPv4 address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// A master relayed it to the host.
    Sent,
    /// The server is not on any list, or does not take punches.
    NotPossible(String),
    /// Throttled / a transient error.
    Later(String),
}

async fn ask(http: &reqwest::Client, masters: &[String], server: SocketAddr, endpoint: SocketAddr) -> Asked {
    let nonce = format!("{:016x}", rand::random::<u64>());
    let body = hsmp_master_core::punch::PunchReq { host: server.ip().to_string(), port: server.port(), endpoint: endpoint.to_string(), nonce };
    let mut last = Asked::Later("no server list".into());
    for m in masters {
        let url = format!("{}/v1/punch", m.trim().trim_end_matches('/'));
        let r = match http.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                last = Asked::Later(format!("{m}: {e}"));
                continue;
            }
        };
        let st = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        match st {
            202 => return Asked::Sent,
            // not listed there / no relay to it: the next list may still have it
            404 | 409 => last = Asked::NotPossible(format!("{st} {}", text.trim())),
            _ => last = Asked::Later(format!("{st} {text}")),
        }
    }
    last
}

/// Show `text` as the link's reason line while still connecting.
fn say(text: &str) {
    let (connected, terminal) = super::net::link_now();
    if connected || terminal {
        return;
    }
    super::session_client::link_update(|l| {
        l.reason = hsmp_ipc::layout::Str::new(text);
        l.reason_code = 0;
    });
}

/// Run the traversal for this sidecar's first connect.
pub(super) fn spawn(sock: Arc<UdpSocket>, opts: Opts) -> Option<tokio::task::JoinHandle<()>> {
    let server = super::net::server_addr()?;
    if opts.mode == Mode::Off || opts.masters.is_empty() || opts.stun.is_empty() {
        return None;
    }
    if opts.mode == Mode::Auto && local_target(server.ip()) {
        debug!(%server, "LAN / loopback server: no NAT traversal");
        return None;
    }
    Some(tokio::spawn(run(sock, server, opts)))
}

async fn run(sock: Arc<UdpSocket>, server: SocketAddr, opts: Opts) {
    let start = Instant::now();
    tokio::time::sleep(DIRECT_WAIT).await;
    let heard = || super::net::HEARD_SERVER.load(Ordering::Relaxed);
    if heard() || super::net::link_now().0 {
        return; // the direct path works
    }
    info!(%server, "no answer from the server directly; trying NAT traversal through the server list");
    crate::events::emit("nat_traversal", serde_json::json!({"server": server.to_string(), "step": "start"}));
    say(TEXT_TRYING);
    let mut http = reqwest::Client::builder().timeout(Duration::from_secs(5));
    if opts.masters.iter().all(|m| m.contains("://127.0.0.1") || m.contains("://localhost") || m.contains("://[::1]")) {
        http = http.no_proxy();
    }
    if server.is_ipv4() {
        http = http.dns_resolver(Arc::new(Ipv4Only));
    }
    let http = http.build().unwrap_or_default();
    let mut endpoint = None;
    let mut tries = 0u32;
    let mut blocked_shown = false;
    loop {
        let (connected, terminal) = super::net::link_now();
        if connected || terminal {
            if connected {
                info!(after_ms = start.elapsed().as_millis() as u64, tries, "connected through NAT traversal");
                crate::events::emit("nat_traversal", serde_json::json!({"server": server.to_string(), "step": "connected", "tries": tries}));
            }
            return;
        }
        if endpoint.is_none() {
            endpoint = public_endpoint(&sock, &opts.stun, server.is_ipv4()).await;
            match endpoint {
                Some(e) => info!(public = %e, "STUN: this game socket's public endpoint"),
                None => warn!(servers = ?opts.stun, "STUN: no answer; cannot ask for a punch"),
            }
        }
        if let Some(ep) = endpoint {
            tries += 1;
            match ask(&http, &opts.masters, server, ep).await {
                Asked::Sent => {
                    info!(try_n = tries, endpoint = %ep, "punch request relayed to the host");
                    crate::events::emit("nat_traversal", serde_json::json!({"server": server.to_string(), "step": "relayed", "try": tries}));
                    // the host probes at once; the next Hello follows its first probes
                    super::net::retry_soon(Duration::from_millis(300));
                }
                Asked::NotPossible(why) => {
                    info!(%why, "the server list cannot relay a punch to this server");
                    if !blocked_shown {
                        say(&blocked_text(server.port()));
                        blocked_shown = true;
                    }
                }
                Asked::Later(why) => debug!(%why, "punch request not sent this time"),
            }
        }
        if !blocked_shown && start.elapsed() >= GIVE_UP_TEXT_AFTER {
            warn!(%server, "no answer directly or through the router; the host's network blocks incoming connections");
            crate::events::emit("nat_traversal", serde_json::json!({"server": server.to_string(), "step": "blocked"}));
            say(&blocked_text(server.port()));
            blocked_shown = true;
        }
        // wait for the next try, but notice a connect at once
        let until = Instant::now() + if tries < FAST_TRIES { PUNCH_EVERY } else { SLOW_EVERY };
        while Instant::now() < until {
            let (c, t) = super::net::link_now();
            if c || t { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_targets() {
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("force"), Mode::Force);
        assert_eq!(Mode::parse(""), Mode::Auto);
        for a in ["127.0.0.1", "192.168.1.5", "10.0.0.2", "100.64.3.3", "::1", "fd00::1", "fe80::1"] {
            assert!(local_target(a.parse().unwrap()), "{a}");
        }
        for a in ["203.0.113.5", "108.94.150.56", "2001:db8::1"] {
            assert!(!local_target(a.parse().unwrap()), "{a}");
        }
        assert_eq!(blocked_text(7777), "The host's network blocks incoming connections; ask them to forward UDP 7777");
    }

    #[test]
    fn stray_datagrams_are_sorted() {
        let from: SocketAddr = "198.51.100.1:7777".parse().unwrap();
        let before = PROBES.load(Ordering::Relaxed);
        on_datagram(from, &probe::encode(5));
        assert_eq!(PROBES.load(Ordering::Relaxed), before + 1);
        // a STUN answer nobody waits for, and garbage: ignored
        on_datagram(from, &stun::binding_success(&[1; 12], from));
        on_datagram(from, b"junk");
        assert_eq!(PROBES.load(Ordering::Relaxed), before + 1);
    }
}

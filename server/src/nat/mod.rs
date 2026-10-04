//! NAT traversal for a server behind a home router (docs/hosting/ports-and-firewall.md,
//! docs/development/protocol.md "NAT traversal"):
//!
//! 1. Port mapping: UPnP-IGD, PCP or NAT-PMP opens the UDP port on the router, with a
//!    lease that is renewed, and removed on a clean shutdown (`--port-map off` disables it).
//! 2. STUN from the game socket itself: the public endpoint and whether the NAT maps
//!    endpoint-independently. Without a router mapping the NAT's own mapping is kept alive
//!    by a STUN refresh every 25 s, and a port the NAT translated is what gets listed.
//! 3. Punch: the master relays a joiner's request (listen.rs); the server answers with a
//!    few small probes to the joiner's public endpoint, which opens its NAT for the
//!    joiner's handshake. The handshake and cookie are unchanged: a punched path is just
//!    the normal path.
//!
//! The listen host's game is told the outcome with a `notice` (NET_STATUS) to its owner.

pub mod emu;
pub mod listen;

use hsmp_nat::{gateway, portmap, probe, stun};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

/// Lease asked for (the router may give less).
const LEASE_S: u32 = 3600;
/// After a failed mapping attempt, try again this much later (a router whose UPnP was off
/// may have it switched on meanwhile).
const REMAP_AFTER: Duration = Duration::from_secs(600);
/// STUN refresh while the NAT's own mapping is all that keeps the port reachable (well
/// inside the 30-60 s UDP timeout of common routers).
const KEEPALIVE: Duration = Duration::from_secs(25);
/// STUN refresh otherwise (an address change is noticed within this).
const RECHECK: Duration = Duration::from_secs(300);
const STUN_WAIT: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone)]
pub struct Opts {
    pub port_map: bool,
    /// STUN servers (`host:port`); empty = off.
    pub stun: Vec<String>,
    pub punch: bool,
    /// Test only: drop inbound datagrams the way a NAT with address-and-port-dependent
    /// filtering would (emu.rs).
    pub emulate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortMap {
    Off,
    Trying,
    Mapped { method: &'static str, external: u16, external_ip: Option<Ipv4Addr>, double: bool },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Status {
    pub bind_port: u16,
    pub port_map: PortMap,
    /// Where STUN saw the game socket (None = no answer / STUN off).
    pub public: Option<SocketAddr>,
    pub mapping: Option<stun::Mapping>,
    pub stun_on: bool,
    pub punch_on: bool,
    /// The master's punch relay socket is open.
    pub listening: bool,
    /// The first port-mapping attempt and STUN round are done.
    pub settled: bool,
}

impl Default for Status {
    fn default() -> Status {
        Status { bind_port: 0, port_map: PortMap::Off, public: None, mapping: None, stun_on: false, punch_on: false, listening: false, settled: true }
    }
}

impl Status {
    /// The port players must use: the router's mapped port, else the port the NAT maps the
    /// socket to (only when that mapping is the same for everyone), else the bound port.
    pub fn advertised_port(&self) -> u16 {
        if let PortMap::Mapped { external, .. } = self.port_map {
            return external;
        }
        match (self.mapping, self.public) {
            (Some(stun::Mapping::EndpointIndependent), Some(p)) => p.port(),
            _ => self.bind_port,
        }
    }

    /// The listing's `nat` (hsmp_master_core::fields::NAT_KINDS); "" = nothing known.
    pub fn nat_kind(&self) -> &'static str {
        match (&self.port_map, self.mapping) {
            (_, Some(stun::Mapping::NoNat)) => "open",
            (PortMap::Mapped { double: true, .. }, _) => "double",
            (PortMap::Mapped { method, .. }, _) => method,
            (_, Some(m)) => m.as_str(),
            _ if self.stun_on => "unknown",
            _ => "",
        }
    }

    /// Joiners may need a punch: no clean router mapping and a NAT that punching can open.
    pub fn wants_punch(&self) -> bool {
        let mapped = matches!(self.port_map, PortMap::Mapped { double: false, .. });
        self.punch_on && !mapped && matches!(self.mapping, Some(stun::Mapping::EndpointIndependent) | Some(stun::Mapping::Unknown))
    }

    /// The owner's NET_STATUS notice: state, port, public address, NAT kind.
    pub fn notice_args(&self) -> [String; 4] {
        let state = match (&self.port_map, self.mapping) {
            (_, Some(stun::Mapping::NoNat)) => "open".to_string(),
            (PortMap::Mapped { double: true, .. }, _) => "double".to_string(),
            (PortMap::Mapped { method, .. }, _) => method.to_string(),
            (PortMap::Failed(_), _) => "failed".to_string(),
            (PortMap::Trying, _) => "trying".to_string(),
            (PortMap::Off, _) => "off".to_string(),
        };
        let public = match (&self.port_map, self.public) {
            (PortMap::Mapped { external, external_ip: Some(ip), double: false, .. }, _) => SocketAddr::from((*ip, *external)).to_string(),
            (_, Some(p)) => p.to_string(),
            _ => String::new(),
        };
        let kind = match (self.mapping, self.listening) {
            (Some(stun::Mapping::EndpointIndependent), true) => "cone+punch".to_string(),
            (Some(m), _) => m.as_str().to_string(),
            (None, _) => String::new(),
        };
        [state, self.advertised_port().to_string(), public, kind]
    }
}

fn cell() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Status::default()))
}

fn changes() -> &'static tokio::sync::watch::Sender<u64> {
    static W: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    W.get_or_init(|| tokio::sync::watch::channel(0).0)
}

pub fn status() -> Status {
    cell().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Bumped on every change of the status (master_client, the owner notice).
pub fn subscribe() -> tokio::sync::watch::Receiver<u64> {
    changes().subscribe()
}

fn update(f: impl FnOnce(&mut Status)) {
    let changed = {
        let mut s = cell().lock().unwrap_or_else(|e| e.into_inner());
        let before = (s.port_map.clone(), s.public, s.mapping, s.listening, s.settled);
        f(&mut s);
        before != (s.port_map.clone(), s.public, s.mapping, s.listening, s.settled)
    };
    if changed {
        changes().send_modify(|v| *v += 1);
    }
}

pub(crate) fn set_listening(on: bool) {
    update(|s| s.listening = on);
}

/// Wait until the first mapping attempt and STUN round are done (at most `limit`), so the
/// first registration lists the right port.
pub async fn settled(limit: Duration) {
    let mut rx = subscribe();
    let wait = async {
        while !status().settled {
            if rx.changed().await.is_err() {
                return;
            }
        }
    };
    let _ = tokio::time::timeout(limit, wait).await;
}

// ---- STUN on the game socket ---------------------------------------------------------------

fn pending() -> &'static Mutex<HashMap<stun::TxId, tokio::sync::oneshot::Sender<SocketAddr>>> {
    static P: OnceLock<Mutex<HashMap<stun::TxId, tokio::sync::oneshot::Sender<SocketAddr>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A STUN datagram arrived on the game socket (the receive loop filters them out first).
pub fn on_stun(from: SocketAddr, data: &[u8]) {
    match stun::parse(data) {
        Some(stun::Parsed::Mapped { tx, addr }) => {
            if let Some(tx) = pending().lock().unwrap_or_else(|e| e.into_inner()).remove(&tx) {
                let _ = tx.send(SocketAddr::new(hsmp_master_core::ip::canonical(addr.ip()), addr.port()));
            }
        }
        Some(stun::Parsed::Request { .. }) => debug!(%from, "STUN request on the game port ignored"),
        _ => {}
    }
}

/// One Binding exchange from the game socket (two sends, `STUN_WAIT` in all).
async fn binding(socket: &UdpSocket, server: SocketAddr) -> Option<SocketAddr> {
    let tx = stun::new_tx();
    let (otx, orx) = tokio::sync::oneshot::channel();
    pending().lock().unwrap_or_else(|e| e.into_inner()).insert(tx, otx);
    let req = stun::binding_request(&tx);
    emu::note_outbound(server);
    let resend = async {
        let _ = socket.send_to(&req, server).await;
        tokio::time::sleep(STUN_WAIT / 2).await;
        let _ = socket.send_to(&req, server).await;
        std::future::pending::<()>().await
    };
    let got = tokio::select! {
        r = tokio::time::timeout(STUN_WAIT, orx) => r.ok().and_then(|r| r.ok()),
        _ = resend => None,
    };
    pending().lock().unwrap_or_else(|e| e.into_inner()).remove(&tx);
    got
}

/// Resolve the STUN servers on the socket's address family.
async fn resolve(servers: &[String], v4: bool) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for s in servers {
        if let Ok(it) = tokio::net::lookup_host(s.as_str()).await {
            if let Some(a) = it.into_iter().find(|a| a.is_ipv4() == v4) {
                if !out.iter().any(|o: &SocketAddr| o.ip() == a.ip()) {
                    out.push(a);
                }
            }
        }
    }
    out
}

/// Ask STUN servers on different addresses until two answered (or all were tried).
async fn stun_round(socket: &UdpSocket, servers: &[SocketAddr], bind: SocketAddr, emulate: bool) -> (Option<SocketAddr>, Option<stun::Mapping>) {
    let mut seen = Vec::new();
    for s in servers {
        if let Some(a) = binding(socket, *s).await {
            seen.push(a);
            if seen.len() >= 2 {
                break;
            }
        }
    }
    if seen.is_empty() {
        return (None, None);
    }
    // The emulated NAT is port preserving: it must not read as "no NAT".
    let local = (!emulate)
        .then(|| match servers[0].ip() {
            IpAddr::V4(v) => gateway::local_ip_towards(v).map(|ip| SocketAddr::from((ip, bind.port()))),
            IpAddr::V6(_) => None,
        })
        .flatten();
    let mut kind = stun::classify(&seen, local);
    if emulate && kind == stun::Mapping::Unknown && seen.len() == 1 {
        kind = stun::Mapping::EndpointIndependent;
    }
    (seen.first().copied(), Some(kind))
}

// ---- punch probes ----------------------------------------------------------------------------

/// Host-side limits on probes, whatever the master asks: one burst per target per 2 s, at
/// most 30 bursts a minute in all.
struct ProbeLimits {
    last: HashMap<SocketAddr, Instant>,
    window: (Instant, u32),
}

fn probe_limits() -> &'static Mutex<ProbeLimits> {
    static L: OnceLock<Mutex<ProbeLimits>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(ProbeLimits { last: HashMap::new(), window: (Instant::now(), 0) }))
}

fn probe_allowed(to: SocketAddr, now: Instant) -> bool {
    let mut l = probe_limits().lock().unwrap_or_else(|e| e.into_inner());
    if now.duration_since(l.window.0) >= Duration::from_secs(60) {
        l.window = (now, 0);
        l.last.retain(|_, t| now.duration_since(*t) < Duration::from_secs(2));
    }
    if l.window.1 >= 30 || l.last.get(&to).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(2)) {
        return false;
    }
    l.window.1 += 1;
    l.last.insert(to, now);
    true
}

/// A punch request from the master: probe `to` (a joiner's public endpoint) a few times.
pub fn punch(socket: &Arc<UdpSocket>, to: SocketAddr, nonce: &str) {
    let to = SocketAddr::new(hsmp_master_core::ip::canonical(to.ip()), to.port());
    let bind_v4 = socket.local_addr().map(|a| a.is_ipv4()).unwrap_or(true);
    if to.port() < hsmp_master_core::punch::MIN_TARGET_PORT || to.is_ipv4() != bind_v4 || to.ip().is_unspecified() || to.ip().is_multicast() {
        warn!(%to, "punch target refused");
        return;
    }
    if !probe_allowed(to, Instant::now()) {
        debug!(%to, "punch over the host's probe budget; skipped");
        return;
    }
    info!(%to, nonce, "punch: probing the joiner's public endpoint");
    crate::events::emit("nat_punch", serde_json::json!({"to": to.to_string(), "nonce": nonce}));
    let socket = socket.clone();
    let n: u64 = rand::random();
    tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        for at in probe::SCHEDULE_MS {
            tokio::time::sleep_until(start + Duration::from_millis(at)).await;
            emu::note_outbound(to);
            let _ = socket.send_to(&probe::encode(n), to).await;
        }
    });
}

// ---- the task -----------------------------------------------------------------------------------

/// What keeps running: the router mapping (removed by `shutdown`).
pub struct Handle {
    targets: portmap::Targets,
    mapping: Arc<tokio::sync::Mutex<Option<portmap::Mapping>>>,
}

impl Handle {
    /// Clean shutdown: remove the router mapping (at most 3 s).
    pub async fn shutdown(&self) {
        let m = self.mapping.lock().await.take();
        if let Some(m) = m {
            let _ = tokio::time::timeout(Duration::from_secs(3), portmap::unmap(&self.targets, &m)).await;
            info!(method = m.method_name(), port = m.external, "router port mapping removed");
        }
    }
}

/// Start port mapping and STUN for the game socket bound at `bind`.
pub fn spawn(socket: Arc<UdpSocket>, bind: SocketAddr, opts: Opts) -> Handle {
    emu::set(opts.emulate);
    // A loopback-bound server is not reachable from outside: no router mapping. IPv6 has no
    // NAT to map through (the firewall still applies).
    let mappable = opts.port_map && bind.is_ipv4() && !bind.ip().is_loopback();
    let stun_on = !opts.stun.is_empty();
    update(|s| {
        s.bind_port = bind.port();
        s.port_map = if mappable { PortMap::Trying } else { PortMap::Off };
        s.stun_on = stun_on;
        s.punch_on = opts.punch && stun_on;
        s.settled = !(mappable || stun_on);
    });
    let targets = portmap::Targets::system();
    let mapping: Arc<tokio::sync::Mutex<Option<portmap::Mapping>>> = Default::default();
    let first_map = Arc::new(tokio::sync::Notify::new());
    if mappable {
        let (t, m, done) = (targets.clone(), mapping.clone(), first_map.clone());
        tokio::spawn(async move { map_loop(t, m, bind.port(), done).await });
    } else {
        first_map.notify_one();
    }
    let done = first_map.clone();
    tokio::spawn(async move {
        // The first STUN round after the mapping attempt (a fresh mapping may change what
        // STUN sees).
        let _ = tokio::time::timeout(Duration::from_secs(12), done.notified()).await;
        if stun_on {
            stun_loop(socket, bind, opts.stun, opts.emulate).await;
        } else {
            update(|s| s.settled = true);
        }
    });
    Handle { targets, mapping }
}

async fn map_loop(t: portmap::Targets, slot: Arc<tokio::sync::Mutex<Option<portmap::Mapping>>>, port: u16, first: Arc<tokio::sync::Notify>) {
    let mut first = Some(first);
    loop {
        update(|s| if !matches!(s.port_map, PortMap::Mapped { .. }) { s.port_map = PortMap::Trying });
        match portmap::map(&t, port, LEASE_S).await {
            Ok(mut m) => {
                info!(method = m.method_name(), external = m.external, external_ip = ?m.external_ip, lease_s = m.lease_s,
                      "router port opened automatically");
                if m.double_nat() {
                    warn!(external_ip = ?m.external_ip, "the router's own WAN address is private: another NAT (CGNAT?) is in front of it");
                }
                let st = PortMap::Mapped { method: method_static(&m), external: m.external, external_ip: m.external_ip, double: m.double_nat() };
                update(|s| s.port_map = st);
                *slot.lock().await = Some(m.clone());
                if let Some(f) = first.take() { f.notify_one(); }
                loop {
                    tokio::time::sleep(m.renew_after()).await;
                    match portmap::renew(&t, &mut m).await {
                        Ok(()) => {
                            debug!(method = m.method_name(), "router port mapping renewed");
                            *slot.lock().await = Some(m.clone());
                        }
                        Err(e) => {
                            warn!(error = %e, "router port mapping renewal failed; mapping again");
                            *slot.lock().await = None;
                            break;
                        }
                    }
                }
            }
            Err(why) => {
                info!(why = %why, port, "could not open the router port automatically (forward UDP {port} by hand for players outside your network)");
                update(|s| s.port_map = PortMap::Failed(why));
                if let Some(f) = first.take() { f.notify_one(); }
                tokio::time::sleep(REMAP_AFTER).await;
            }
        }
    }
}

fn method_static(m: &portmap::Mapping) -> &'static str {
    match m.method_name() {
        "upnp" => "upnp",
        "pcp" => "pcp",
        _ => "natpmp",
    }
}

async fn stun_loop(socket: Arc<UdpSocket>, bind: SocketAddr, servers: Vec<String>, emulate: bool) {
    loop {
        let addrs = resolve(&servers, bind.is_ipv4()).await;
        let (public, kind) = if addrs.is_empty() { (None, None) } else { stun_round(&socket, &addrs, bind, emulate).await };
        let before = status();
        if public != before.public || kind != before.mapping {
            match (public, kind) {
                (Some(p), Some(k)) => info!(public = %p, nat = k.as_str(), "STUN: public endpoint of the game port"),
                _ => info!(servers = ?servers, "STUN: no answer (public endpoint unknown)"),
            }
        }
        update(|s| {
            s.public = public;
            s.mapping = kind;
            s.settled = true;
        });
        let s = status();
        let mapped = matches!(s.port_map, PortMap::Mapped { double: false, .. });
        let behind_nat = !matches!(s.mapping, Some(stun::Mapping::NoNat));
        tokio::time::sleep(if !mapped && behind_nat && public.is_some() { KEEPALIVE } else { RECHECK }).await;
    }
}

/// The STUN servers from `--stun` / HSMP_STUN_SERVERS: "" = the public defaults, "off" = none.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn st(port_map: PortMap, public: Option<&str>, mapping: Option<stun::Mapping>) -> Status {
        Status { bind_port: 7777, port_map, public: public.map(|p| p.parse().unwrap()), mapping, stun_on: true, punch_on: true, listening: false, settled: true }
    }

    #[test]
    fn the_listed_port_is_the_one_that_works() {
        let mapped = PortMap::Mapped { method: "upnp", external: 7778, external_ip: Some(Ipv4Addr::new(203, 0, 113, 4)), double: false };
        // a router mapping wins
        let s = st(mapped.clone(), Some("203.0.113.4:40000"), Some(stun::Mapping::EndpointIndependent));
        assert_eq!((s.advertised_port(), s.nat_kind(), s.wants_punch()), (7778, "upnp", false));
        // a NAT that translates the port the same way for everyone: its port
        let s = st(PortMap::Failed("x".into()), Some("203.0.113.4:40000"), Some(stun::Mapping::EndpointIndependent));
        assert_eq!((s.advertised_port(), s.nat_kind(), s.wants_punch()), (40000, "cone", true));
        // symmetric: the port differs per destination, so only the bound one is meaningful
        let s = st(PortMap::Failed("x".into()), Some("203.0.113.4:40000"), Some(stun::Mapping::Symmetric));
        assert_eq!((s.advertised_port(), s.nat_kind(), s.wants_punch()), (7777, "symmetric", false));
        // a public address
        let s = st(PortMap::Failed("x".into()), Some("203.0.113.4:7777"), Some(stun::Mapping::NoNat));
        assert_eq!((s.advertised_port(), s.nat_kind(), s.wants_punch()), (7777, "open", false));
        // nothing known
        let s = Status { bind_port: 7777, ..Status::default() };
        assert_eq!((s.advertised_port(), s.nat_kind(), s.wants_punch()), (7777, "", false));
        // double NAT: the mapping alone is not enough, punching may still help
        let d = PortMap::Mapped { method: "upnp", external: 7777, external_ip: Some(Ipv4Addr::new(100, 64, 1, 2)), double: true };
        let s = st(d, Some("198.51.100.3:7777"), Some(stun::Mapping::EndpointIndependent));
        assert_eq!((s.nat_kind(), s.wants_punch()), ("double", true));
    }

    #[test]
    fn owner_notice_args() {
        let mapped = PortMap::Mapped { method: "upnp", external: 7777, external_ip: Some(Ipv4Addr::new(203, 0, 113, 4)), double: false };
        let s = st(mapped, Some("203.0.113.4:7777"), Some(stun::Mapping::EndpointIndependent));
        assert_eq!(s.notice_args(), ["upnp", "7777", "203.0.113.4:7777", "cone"].map(String::from));
        let mut s = st(PortMap::Failed("no UPnP".into()), Some("203.0.113.4:7777"), Some(stun::Mapping::EndpointIndependent));
        s.listening = true;
        assert_eq!(s.notice_args(), ["failed", "7777", "203.0.113.4:7777", "cone+punch"].map(String::from));
        let s = Status { bind_port: 7000, ..Status::default() };
        assert_eq!(s.notice_args(), ["off", "7000", "", ""].map(String::from));
    }

    #[test]
    fn stun_server_lists() {
        assert_eq!(stun_servers("off"), Vec::<String>::new());
        assert_eq!(stun_servers("").len(), stun::DEFAULT_SERVERS.len());
        assert_eq!(stun_servers(" a:1, b:2 ,"), vec!["a:1".to_string(), "b:2".to_string()]);
    }

    #[test]
    fn probe_budget() {
        let t = Instant::now();
        let a: SocketAddr = "198.51.100.7:40000".parse().unwrap();
        assert!(probe_allowed(a, t));
        assert!(!probe_allowed(a, t + Duration::from_millis(500)), "same target within 2 s");
        assert!(probe_allowed(a, t + Duration::from_millis(2100)));
        let n = (0..40u16).filter(|i| probe_allowed(SocketAddr::from(([192, 0, 2, 1], 2000 + i)), t + Duration::from_secs(3))).count();
        assert_eq!(n, 28, "30 bursts a minute in all");
    }

    #[tokio::test]
    async fn stun_on_the_game_socket() {
        // a STUN server that answers with the source address
        let srv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let srv_addr = srv.local_addr().unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 600];
            loop {
                let Ok((n, from)) = srv.recv_from(&mut b).await else { return };
                if let Some(stun::Parsed::Request { tx }) = stun::parse(&b[..n]) {
                    let _ = srv.send_to(&stun::binding_success(&tx, from), from).await;
                }
            }
        });
        let game = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let me = game.local_addr().unwrap();
        // the receive loop's part: STUN answers go to on_stun
        let rx = game.clone();
        tokio::spawn(async move {
            let mut b = [0u8; 600];
            loop {
                let Ok((n, from)) = rx.recv_from(&mut b).await else { return };
                if stun::is_stun(&b[..n]) { on_stun(from, &b[..n]); }
            }
        });
        assert_eq!(binding(&game, srv_addr).await, Some(me));
        let (public, kind) = stun_round(&game, &[srv_addr], me, true).await;
        assert_eq!((public, kind), (Some(me), Some(stun::Mapping::EndpointIndependent)));
        // nobody answering
        let dead = UdpSocket::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        assert_eq!(binding(&game, dead).await, None);
    }
}

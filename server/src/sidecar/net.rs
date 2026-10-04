//! Network: the wire-protocol client (docs/development/protocol.md).
//!
//! * One `hsmp_net::net::Client` behind a `std::sync::Mutex` (never held
//!   across an `.await`): `send_msg` queues a record message on its kind's channel and
//!   sends what is due; the receive task feeds datagrams in; the 5 ms transport
//!   task sends acks, retransmits, keepalives and handshake resends.
//! * Identity: a per-install Ed25519 seed in `%LOCALAPPDATA%\HSMP\identity`,
//!   or `$HSMP_STATE_DIR/identity` for test instances.
//! * Server key: `--server-key` (browser / master listing) is pinned and a
//!   mismatch fails closed. Otherwise trust on first use: the key is pinned
//!   for this process's reconnects and recorded in `known_servers.json`
//!   (a later mismatch there is logged loudly, not enforced).
//! * Reconnect: event driven. A lost connection (idle timeout, close,
//!   handshake timeout) re-handshakes with the same identity after 1, 2, 4,
//!   8 s; the server reclaims the seat by key. Kicked, rejected, replaced and
//!   server-closed are terminal.
//!
//! See docs/development/server-modules.md.

use super::*;
use hsmp_net::net::{close_code, Client, ClientConfig, ClientEvent, ConnConfig, ConnStats};
use std::sync::OnceLock;

/// Transport timer period.
const TRANSPORT_TICK: Duration = Duration::from_millis(5);
const MAX_BACKOFF_S: u64 = 8;

struct Params {
    seed: [u8; 32],
    nick: String,
    version_min: u16,
    version_max: u16,
    content_hash: [u8; 32],
    /// `--server-key`: enforced.
    explicit_key: Option<[u8; 32]>,
}

pub(super) struct NetClient {
    client: Client,
    t0: Instant,
    params: Params,
    /// Key learned on the first connect of this process (TOFU for reconnects).
    learned_key: Option<[u8; 32]>,
    reconnect_at: Option<Instant>,
    backoff_s: u64,
    /// Kicked / rejected / replaced / server closed: never reconnect.
    terminal: bool,
    server: String,
    known_path: PathBuf,
    /// Last datagram from the server (any: data, ack, keepalive).
    last_rx: Instant,
    /// Handshakes started since the link was last up (0 while up).
    attempts: u32,
    /// When the link went down (None while up).
    down_since: Option<Instant>,
    /// Unauthenticated (pre-cookie) rejects in a row since the last connect.
    unauth_rejects: u32,
    /// When the first of them arrived (they only become believable
    /// after a whole handshake timeout of nothing but rejects).
    unauth_since: Option<Instant>,
}

/// Unauthenticated rejects tolerated in a row on an automatic reconnect
/// before giving up (a real version change after a server update still
/// ends the session, a single forged packet does not).
const MAX_UNAUTH_REJECTS: u32 = 5;
/// ... and only once they have kept coming, with no authenticated
/// answer in between, for a whole handshake timeout (10 s).
const UNAUTH_TERMINAL_MS: u64 = 10_000;
/// An unauthenticated reject restarts the handshake after this long (the
/// first Hello resend interval), without consuming a backoff step.
const UNAUTH_RETRY: Duration = Duration::from_millis(250);

/// Is this reject the end of the session?
/// Authenticated rejects (sealed with the handshake keys) are always
/// believed. An unauthenticated PreReject can be forged by anyone who saw
/// the Hello: on the first connect only VERSION / CONTENT (the only codes a
/// real server sends before the cookie) end it; on an automatic reconnect
/// (we were connected before) it is retried, up to MAX_UNAUTH_REJECTS.
/// On a reconnect (and for every other code on the first connect)
/// unauthenticated rejects end the session only after they kept coming for
/// UNAUTH_TERMINAL_MS (a whole handshake timeout with no authenticated
/// answer); on the first connect VERSION / CONTENT need a second attempt
/// (250 ms later) to agree, so one forged packet never ends a session.
/// `unauth_for_ms`: since the first unauthenticated reject of this run.
pub(super) fn reject_is_terminal(code: u8, authenticated: bool, ever_connected: bool, unauth_in_a_row: u32, unauth_for_ms: u64) -> bool {
    use hsmp_net::net::handshake::reject_code;
    if authenticated { return true; }
    let sustained = unauth_in_a_row >= MAX_UNAUTH_REJECTS && unauth_for_ms >= UNAUTH_TERMINAL_MS;
    if ever_connected { return sustained; }
    ((code == reject_code::VERSION || code == reject_code::CONTENT) && unauth_in_a_row >= 2) || sustained
}

/// How often the transport timer re-evaluates the link record (published on change only).
const LINK_EVERY: Duration = Duration::from_millis(250);

/// The `link_state` code of the link record.
pub(super) fn link_state(connected: bool, terminal: bool, rx_age_ms: u64, stall_ms: u64) -> u8 {
    use hsmp_ipc::schema::session::link_state as ls;
    if terminal { ls::TERMINAL }
    else if !connected { ls::RECONNECTING }
    else if rx_age_ms > stall_ms { ls::STALLED }
    else { ls::UP }
}

/// Stall threshold reported in the link record (the Director's own is 3 s).
const STALL_MS: u64 = 2_500;

use std::time::Instant;

static NET: OnceLock<std::sync::Mutex<NetClient>> = OnceLock::new();

fn net() -> std::sync::MutexGuard<'static, NetClient> {
    NET.get().expect("net::init ran").lock().unwrap_or_else(|e| e.into_inner())
}

impl NetClient {
    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    fn build(&self) -> Client {
        let mut cfg = ClientConfig::new(self.params.seed, &self.params.nick);
        cfg.version_min = self.params.version_min;
        cfg.version_max = self.params.version_max;
        cfg.content_hash = self.params.content_hash;
        cfg.build = super::build_id::build_tag("hsmp-sidecar");
        cfg.pinned_server_key = self.params.explicit_key.or(self.learned_key);
        // Interaction channel (docs/development/subsystems/interact.md), offered on top of the base set.
        cfg.caps |= hsmp_net::net::caps::INTERACT;
        // Per-player RTTs (S2CPings) for the lobby / scoreboard ping column.
        cfg.caps |= hsmp_net::net::caps::PING;
        // Relayed skeletal frames carry the relay interval for this sender
        // (the pose record's aux field): poseplay sizes the buffer.
        cfg.caps |= hsmp_net::net::caps::POSE_RATE;
        // Combat hit effects: touch reports up, stand-in blood / wounds down.
        cfg.caps |= hsmp_net::net::caps::HIT_FX;
        // The owner's passport body for its stand-ins (body records), both ways.
        cfg.caps |= hsmp_net::net::caps::BODY;
        // Path-dead after 2 s of silence while sending: re-handshake
        // long before the 10 s idle timeout.
        let conn = ConnConfig { dead_after_ms: hsmp_net::net::conn::CLIENT_DEAD_AFTER_MS, ..ConnConfig::default() };
        Client::new(cfg, conn, self.now_ms(), &mut rand::rngs::OsRng)
    }

    /// Everything due now (handshake resends, acks, retransmits ...). The
    /// client's events go to the ONE event task, in order, while the NET lock
    /// is still held (the receive task and the transport timer both
    /// feed datagrams; on a multi-threaded runtime two tasks handling their
    /// own event batches could reorder e.g. Welcome and Session).
    fn drain(&mut self) -> Vec<Vec<u8>> {
        let now = self.now_ms();
        let mut out = Vec::new();
        while let Some(dg) = self.client.poll_transmit(now) {
            out.push(dg);
        }
        while let Some(e) = self.client.poll_event() {
            if let Some(q) = EVQ.get() { let _ = q.send(e); }
        }
        out
    }

    /// Feed one datagram from the server; returns what is due to send.
    fn ingest(&mut self, dg: &[u8]) -> Vec<Vec<u8>> {
        let now = self.now_ms();
        self.last_rx = Instant::now();
        self.client.handle(now, dg);
        self.drain()
    }
}

/// Where the per-install identity and known server keys live.
pub(super) fn identity_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HSMP_STATE_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    match std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("HSMP"),
        None => PathBuf::from("."),
    }
}

/// Load the Ed25519 seed, creating it (OS RNG) on first run.
pub(super) fn load_or_create_identity(dir: &Path) -> Result<[u8; 32]> {
    let path = dir.join("identity");
    if let Ok(b) = std::fs::read(&path) {
        if b.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&b);
            return Ok(k);
        }
        warn!(path = %path.display(), "identity file malformed; creating a new identity");
    }
    let mut k = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut k);
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, k)?;
    std::fs::rename(&tmp, &path)?;
    info!(path = %path.display(), "created a new player identity");
    Ok(k)
}

fn parse_key(hex_s: &str) -> Result<[u8; 32]> {
    let b = hex::decode(hex_s.trim()).context("hex")?;
    <[u8; 32]>::try_from(b.as_slice()).map_err(|_| anyhow::anyhow!("expected 32 bytes (64 hex chars)"))
}

/// File (in the sidecar's --state-dir) holding this player's public key, hex.
pub(super) const PLAYER_KEY_FILE: &str = ".player_key";

/// Write `<dir>/.player_key` (64 hex + LF, atomic). Not a secret: the public
/// half of the identity, as every server sees it in the handshake.
pub(super) fn write_player_key(dir: &Path, pk: &[u8; 32]) {
    let path = dir.join(PLAYER_KEY_FILE);
    let want = format!("{}\n", hex::encode(pk));
    if std::fs::read_to_string(&path).map_or(false, |s| s == want) { return; }
    let tmp = path.with_extension("tmp_pk");
    if std::fs::write(&tmp, want.as_bytes()).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
        warn!(path = %path.display(), "cannot write the player key file");
    }
}

/// `--print-player-key`: this install's player key (created on first use), hex.
pub(super) fn player_key_hex() -> Result<String> {
    let seed = load_or_create_identity(&identity_dir())?;
    Ok(hex::encode(ed25519_public(&seed)))
}

/// Build the client from the command line and identity. Call once, first.
pub(super) fn init(args: &Args) -> Result<()> {
    let dir = identity_dir();
    let seed = load_or_create_identity(&dir)?;
    let explicit_key = match args.server_key.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(h) => Some(parse_key(h).context("--server-key")?),
        None => None,
    };
    let content_hash = match args.content_hash.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(h) => parse_key(h).context("--content-hash")?,
        None => super::build_id::content_hash(),
    };
    let params = Params {
        seed,
        nick: args.nick.clone(),
        version_min: args.proto_min.unwrap_or(hsmp_net::net::VERSION_MIN),
        version_max: args.proto_max.unwrap_or(hsmp_net::net::VERSION_MAX),
        content_hash,
        explicit_key,
    };
    let pk = ed25519_public(&seed);
    // The public half for a listen server's --owner-key-file
    // (the host is admin by key, never by joining first). Written before the
    // first Hello, so the server finds it when this player's Auth arrives.
    write_player_key(&args.state_dir, &pk);
    info!(player = %hsmp_net::net::handshake::player_fingerprint(&pk),
          identity = %dir.join("identity").display(),
          proto = %format!("v{}..=v{}", params.version_min, params.version_max), "v5 identity");
    let mut nc = NetClient {
        client: Client::new(ClientConfig::new(seed, &args.nick), ConnConfig::default(), 0, &mut rand::rngs::OsRng),
        t0: Instant::now(),
        params,
        learned_key: None,
        reconnect_at: None,
        backoff_s: 1,
        terminal: false,
        server: args.server.clone(),
        known_path: dir.join("known_servers.json"),
        last_rx: Instant::now(),
        attempts: 1,
        down_since: Some(Instant::now()),
        unauth_rejects: 0,
        unauth_since: None,
    };
    nc.client = nc.build();
    let _ = NET.set(std::sync::Mutex::new(nc));
    Ok(())
}

fn ed25519_public(seed: &[u8; 32]) -> [u8; 32] {
    // A throw-away Client exposes the derived public key.
    Client::new(ClientConfig::new(*seed, ""), ConnConfig::default(), 0, &mut rand::rngs::OsRng).player_key()
}

/// The live connection's statistics (None while not connected).
pub(super) fn conn_stats() -> Option<ConnStats> {
    NET.get()?;
    let n = net();
    n.client.conn().filter(|_| n.client.is_connected()).map(|c| c.stats())
}

/// The server's address (set once by `bind_for`). The game socket is not connected: it
/// also talks to STUN servers (traversal.rs), so the receive path keeps only the server's
/// datagrams for the transport.
static SERVER_ADDR: OnceLock<std::net::SocketAddr> = OnceLock::new();

pub(super) fn server_addr() -> Option<std::net::SocketAddr> {
    SERVER_ADDR.get().copied()
}

/// Datagrams from the server have arrived (any; the traversal waits for none).
pub(super) static HEARD_SERVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A UDP socket for `server` (`host:port`; an IPv4 or `[IPv6]` literal or a host name),
/// bound on the address family of the address it resolves to. IPv4 addresses are tried
/// first (servers bind 0.0.0.0 by default), then IPv6. Returns the socket and the server
/// address it sends to.
pub(super) async fn bind_for(server: &str) -> Result<(UdpSocket, std::net::SocketAddr)> {
    let mut addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(server)
        .await
        .with_context(|| format!("resolve {server}"))?
        .collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    let mut last = None;
    for a in addrs {
        let local = if a.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        match UdpSocket::bind(local).await {
            Ok(s) => return Ok((s, a)),
            Err(e) => last = Some(anyhow::anyhow!("bind {local}: {e}")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("{server} has no address")))
}

/// `bind_for`, and remember the server address for every send and receive.
pub(super) async fn connect_udp(server: &str) -> Result<UdpSocket> {
    let (s, a) = bind_for(server).await?;
    let _ = SERVER_ADDR.set(a);
    Ok(s)
}

/// Where a datagram from `from` goes: the transport (the server), traversal (STUN answers,
/// punch probes) or nowhere.
fn from_server(from: std::net::SocketAddr, data: &[u8]) -> bool {
    // the host's punch probes come from the server's own address
    if hsmp_nat::probe::is_probe(data) {
        super::traversal::on_datagram(from, data);
        return false;
    }
    match server_addr() {
        Some(s) if s == from => {
            HEARD_SERVER.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        }
        Some(_) => {
            super::traversal::on_datagram(from, data);
            false
        }
        None => true,
    }
}

async fn send_all(sock: &UdpSocket, out: Vec<Vec<u8>>) {
    let Some(to) = server_addr() else { return };
    for dg in out {
        if let Err(e) = sock.send_to(&dg, to).await {
            debug!(error = %e, "udp send");
        }
    }
}

/// Queue one v6 record message (`hsmp_ipc::wire` framed: the record payload copied from
/// shared memory behind its 8-byte header) on its kind's channel and send what is due.
pub(super) async fn send_msg(sock: &UdpSocket, msg: Vec<u8>) -> Result<()> {
    let Ok((h, _)) = hsmp_ipc::wire::split(&msg) else { return Ok(()) };
    let Some(mode) = proto::record_mode(h.kind, h.peer) else {
        debug!(kind = h.kind, "v6: not a network record kind; dropped");
        return Ok(());
    };
    send_bytes(sock, mode, msg).await
}

/// Several v6 record messages at once (one ipc pump step): queued together, then one
/// transmit, so the transport packs them into as few datagrams as fit.
pub(super) async fn send_msgs(sock: &UdpSocket, msgs: Vec<Vec<u8>>) -> Result<()> {
    let out = {
        let mut n = net();
        let mut any = false;
        for msg in msgs {
            let Ok((h, _)) = hsmp_ipc::wire::split(&msg) else { continue };
            let Some(mode) = proto::record_mode(h.kind, h.peer) else {
                debug!(kind = h.kind, "v6: not a network record kind; dropped");
                continue;
            };
            any |= queue_bytes(&mut n, mode, msg);
        }
        if !any {
            return Ok(());
        }
        let now = n.now_ms();
        let mut out = Vec::new();
        if n.client.is_connected() {
            while let Some(dg) = n.client.poll_transmit(now) {
                out.push(dg);
            }
        }
        out
    };
    send_all(sock, out).await;
    Ok(())
}

/// Queue one message on the client; false if the connection is not up (dropped).
fn queue_bytes(n: &mut NetClient, mode: hsmp_net::net::SendMode, bytes: Vec<u8>) -> bool {
    // A channel-0 message too big for one datagram goes reliably (decided up front, so the
    // buffer is moved, never cloned for a retry).
    let mode = match mode {
        hsmp_net::net::SendMode::Latest { .. } if bytes.len() > hsmp_net::net::channel::MAX_UNRELIABLE => {
            hsmp_net::net::SendMode::Reliable
        }
        m => m,
    };
    match n.client.send(mode, bytes) {
        Ok(()) => true,
        Err(e) => {
            debug!(?e, "send before the connection is up; dropped");
            false
        }
    }
}

async fn send_bytes(sock: &UdpSocket, mode: hsmp_net::net::SendMode, bytes: Vec<u8>) -> Result<()> {
    let out = {
        let mut n = net();
        if !queue_bytes(&mut n, mode, bytes) {
            return Ok(());
        }
        let now = n.now_ms();
        let mut out = Vec::new();
        if n.client.is_connected() {
            while let Some(dg) = n.client.poll_transmit(now) {
                out.push(dg);
            }
        }
        out
    };
    send_all(sock, out).await;
    Ok(())
}

/// Client events in the order the transport produced them.
static EVQ: OnceLock<tokio::sync::mpsc::UnboundedSender<ClientEvent>> = OnceLock::new();
static EVRX: OnceLock<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ClientEvent>>>> = OnceLock::new();

/// Read every datagram already waiting in the socket without blocking
/// (at most `max`), handing each to `f`. Returns how many were read.
/// The transport timer calls this before any timer work, so replies
/// that queued up while the process was stalled (disk, AV, laptop resume)
/// count as heard BEFORE the path-dead / idle checks run.
pub(super) fn drain_socket(sock: &UdpSocket, buf: &mut [u8], max: usize, mut f: impl FnMut(&[u8])) -> usize {
    let (mut n, mut errs) = (0, 0);
    while n < max {
        match sock.try_recv_from(buf) {
            Ok((len, from)) => { if from_server(from, &buf[..len]) { f(&buf[..len]); } n += 1; }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            // Windows reports an ICMP port-unreachable from an earlier send
            // as a recv error (server down): skip it, a few per call.
            Err(_) => { errs += 1; if errs > 16 { break; } }
        }
    }
    n
}

pub(super) fn spawn_recv_task(sock: &Arc<UdpSocket>, shared: &Arc<Mutex<SharedState>>) -> tokio::task::JoinHandle<()> {
    // The one consumer of client events (ordered).
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = EVQ.set(tx);
    let _ = EVRX.set(std::sync::Mutex::new(Some(rx)));
    if let Some(mut rx) = EVRX.get().and_then(|m| m.lock().unwrap_or_else(|e| e.into_inner()).take()) {
        let shared = shared.clone();
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                let mut evs = vec![e];
                while let Ok(e) = rx.try_recv() { evs.push(e); }
                on_events(evs, &shared).await;
            }
        });
    }
    let sock_rx = sock.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match sock_rx.recv_from(&mut buf).await {
                Ok((n, from)) => {
                    if !from_server(from, &buf[..n]) { continue; }
                    let out = net().ingest(&buf[..n]);
                    send_all(&sock_rx, out).await;
                }
                // Windows reports an ICMP port-unreachable from an earlier
                // send as a recv error (server down); keep going.
                Err(e) => debug!(error = %e, "recv"),
            }
        }
    })
}

/// A transport-timer pass later than this after the previous one means the
/// process itself was stalled (logged; the queued datagrams are processed
/// first either way).
const LOCAL_STALL: Duration = Duration::from_millis(1000);

/// The 5 ms transport timer: handshake resends, acks, retransmits,
/// keepalives, CLOSE repeats, idle timeout and the reconnect schedule.
pub(super) fn spawn_transport_task(sock: &Arc<UdpSocket>, _shared: &Arc<Mutex<SharedState>>) -> tokio::task::JoinHandle<()> {
    let sock = sock.clone();
    tokio::spawn(async move {
        let mut iv = time::interval(TRANSPORT_TICK);
        iv.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut next_link = Instant::now();
        let mut last_pass = Instant::now();
        let mut buf = vec![0u8; 65536];
        loop {
            iv.tick().await;
            let gap = last_pass.elapsed();
            last_pass = Instant::now();
            // Everything the server already sent is processed before
            // any timer decision (a local stall must not read as a dead path).
            let mut out = Vec::new();
            let n = drain_socket(&sock, &mut buf, 4096, |dg| out.extend(net().ingest(dg)));
            if gap >= LOCAL_STALL {
                warn!(gap_ms = gap.as_millis() as u64, backlog = n, "sidecar was stalled; queued datagrams processed before the transport timer");
            }
            {
                let mut nc = net();
                if let Some(at) = nc.reconnect_at {
                    if Instant::now() >= at && !nc.terminal {
                        nc.reconnect_at = None;
                        nc.attempts += 1;
                        info!(server = %nc.server, attempt = nc.attempts, "reconnecting (new handshake, same identity)");
                        nc.client = nc.build();
                    }
                }
                out.extend(nc.drain());
            }
            send_all(&sock, out).await;
            if Instant::now() >= next_link {
                next_link = Instant::now() + LINK_EVERY;
                write_link();
            }
        }
    })
}

/// The link as the transport sees it (HSMPMatch shows "Reconnecting" from it long before
/// the 10 s idle timeout): the link record, republished only when the state or the attempt
/// changed (the ages are the values at that moment).
fn write_link() {
    let (st, attempt, rx_age, next, down) = {
        let nc = net();
        let connected = nc.client.is_connected();
        let rx_age = nc.last_rx.elapsed().as_millis() as u64;
        let next = nc.reconnect_at.map(|t| t.saturating_duration_since(Instant::now()).as_millis() as u64).unwrap_or(0);
        let down = nc.down_since.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
        (link_state(connected, nc.terminal, rx_age, STALL_MS), if connected { 0 } else { nc.attempts }, rx_age, next, down)
    };
    let c = |x: u64| x.min(u32::MAX as u64) as u32;
    super::session_client::link_update(|l| {
        l.state = st;
        l.attempt = attempt;
        l.rx_age_ms = c(rx_age);
        l.next_retry_ms = c(next);
        l.down_ms = c(down);
    });
}

/// Milliseconds since the last datagram from the server (the link record).
pub(super) fn rx_age_ms() -> u64 {
    if NET.get().is_none() { return 0; }
    net().last_rx.elapsed().as_millis() as u64
}

/// Restart the handshake UNAUTH_RETRY from now (never later than an
/// already scheduled attempt), leaving the backoff untouched.
fn retry_after_unauth_reject(nc: &mut NetClient) {
    if nc.down_since.is_none() {
        nc.down_since = Some(Instant::now());
    }
    if nc.terminal { return; }
    let at = Instant::now() + UNAUTH_RETRY;
    nc.reconnect_at = Some(nc.reconnect_at.map_or(at, |r| r.min(at)));
}

/// `immediate`: the server is known to have forgotten us (verified stateless
/// reset) or the path just died (2 s of silence): the first attempt goes out
/// now instead of after 1 s. Later attempts back off as usual.
fn schedule_reconnect(nc: &mut NetClient, immediate: bool) {
    if nc.down_since.is_none() {
        nc.down_since = Some(Instant::now());
    }
    if nc.terminal || nc.reconnect_at.is_some() {
        return;
    }
    let s = nc.backoff_s;
    let wait = if immediate && s == 1 { Duration::ZERO } else { Duration::from_secs(s) };
    nc.reconnect_at = Some(Instant::now() + wait);
    nc.backoff_s = (s * 2).min(MAX_BACKOFF_S);
}

async fn set_status(shared: &Arc<Mutex<SharedState>>, status: &'static str) {
    super::session_client::set_status(shared, status).await;
}

/// The reject reason (HSMPMatch / HUD show it) into the link record.
pub(super) async fn write_reject(reason: &str, code: u8) {
    super::session_client::set_reason(reason, code, 0);
}

async fn on_events(evs: Vec<ClientEvent>, shared: &Arc<Mutex<SharedState>>) {
    for ev in evs {
        match ev {
            ClientEvent::Message(d) => match proto::decode_msg(&d.data) {
                Ok((h, payload)) => {
                    if let Err(e) = handle_server_record(h, payload, shared).await {
                        warn!(error = %e, kind = h.kind, "handling server record");
                    }
                }
                Err(e) => debug!(error = %e, len = d.data.len(), "undecodable server message dropped"),
            },
            ClientEvent::Connected { server_key, version, caps, conn_id } => {
                let (server, known_path, down_ms, attempts) = {
                    let mut nc = net();
                    nc.backoff_s = 1;
                    nc.unauth_rejects = 0;
                    nc.unauth_since = None;
                    nc.learned_key = Some(server_key);
                    let down = nc.down_since.take().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
                    let a = std::mem::take(&mut nc.attempts);
                    (nc.server.clone(), nc.known_path.clone(), down, a)
                };
                if attempts > 1 {
                    info!(down_ms, attempts, "link resumed with the same identity (the server keeps the seat by key)");
                    crate::events::emit("link_resumed", serde_json::json!({"down_ms": down_ms, "attempts": attempts}));
                }
                info!(server_fp = %hsmp_net::net::handshake::player_fingerprint(&server_key),
                      version, caps, conn_id = %format!("{conn_id:016x}"), "v5 connected (encrypted)");
                // Disk I/O: off the receive path.
                std::thread::spawn(move || remember_server_key(&known_path, &server, &server_key));
                crate::interact_client::set_caps(caps);
            }
            ClientEvent::Rejected { code, text, authenticated } => {
                warn!(code, %text, authenticated, "relay rejected us");
                let terminal = {
                    let mut nc = net();
                    if !authenticated {
                        nc.unauth_rejects += 1;
                        nc.unauth_since.get_or_insert_with(Instant::now);
                    }
                    let for_ms = nc.unauth_since.map_or(0, |t| t.elapsed().as_millis() as u64);
                    let t = reject_is_terminal(code, authenticated, nc.learned_key.is_some(), nc.unauth_rejects, for_ms);
                    if t {
                        nc.terminal = true;
                    } else if !authenticated {
                        // A (possibly forged) pre-cookie reject aborted this
                        // handshake: start a new one shortly, no backoff step used.
                        retry_after_unauth_reject(&mut nc);
                    } else {
                        schedule_reconnect(&mut nc, false);
                    }
                    t
                };
                if terminal {
                    write_reject(&text, code).await;
                    set_status(shared, "rejected").await;
                } else {
                    info!(code, "unauthenticated reject during a reconnect: retrying (could be forged)");
                    set_status(shared, "reconnecting").await;
                }
            }
            ClientEvent::Closed { code, by_peer } => {
                let status = {
                    let mut nc = net();
                    if nc.terminal {
                        // Already decided (kick event, server closing, leave).
                        None
                    } else {
                        match (code, by_peer) {
                            (close_code::KICKED, true) => { nc.terminal = true; Some("kicked") }
                            (close_code::REPLACED, true) => { nc.terminal = true; Some("replaced") }
                            // A restart announcement already scheduled the reconnect.
                            (close_code::SERVER_CLOSING, true) if nc.reconnect_at.is_none() => {
                                nc.terminal = true;
                                Some("server_closed")
                            }
                            // Verified reset (server restarted) or a dead
                            // path: re-handshake at once.
                            (close_code::RESET, true) | (close_code::TIMEOUT, false) => {
                                schedule_reconnect(&mut nc, true);
                                Some("reconnecting")
                            }
                            _ => { schedule_reconnect(&mut nc, false); Some("reconnecting") }
                        }
                    }
                };
                warn!(code, by_peer, "connection lost; status {:?}", status);
                if let Some(st) = status {
                    if st == "kicked" && super::session_client::link_now().reason.is_empty() {
                        super::session_client::set_reason("kicked", 0, 0);
                    }
                    set_status(shared, st).await;
                }
            }
            ClientEvent::Failed(why) => {
                let fatal = why.contains("pinned");
                warn!(%why, "handshake failed");
                if fatal {
                    net().terminal = true;
                    write_reject(&format!("{why}: the server's identity changed"), 255).await;
                    set_status(shared, "rejected").await;
                } else {
                    let st = {
                        let mut nc = net();
                        if nc.terminal { None } else { schedule_reconnect(&mut nc, false); Some("reconnecting") }
                    };
                    if let Some(st) = st { set_status(shared, st).await; }
                }
            }
        }
    }
}

/// The server said it is closing: terminal unless it announced a restart.
pub(super) fn on_server_closing(reconnect_after_ms: u32) {
    if NET.get().is_none() { return; }
    let mut nc = net();
    if reconnect_after_ms > 0 {
        nc.reconnect_at = Some(Instant::now() + Duration::from_millis(reconnect_after_ms as u64));
    } else {
        nc.terminal = true;
    }
}

/// Mark the session terminal (kick event): no automatic rejoin.
pub(super) fn set_terminal() {
    if NET.get().is_none() { return; }
    net().terminal = true;
}

/// (connected, terminal) right now.
pub(super) fn link_now() -> (bool, bool) {
    if NET.get().is_none() { return (false, false); }
    let n = net();
    (n.client.is_connected(), n.terminal)
}

/// A punch was just relayed to the host (traversal.rs): start a fresh handshake shortly,
/// after the host's first probes, instead of waiting out the backoff.
pub(super) fn retry_soon(after: Duration) {
    if NET.get().is_none() { return; }
    let mut n = net();
    if n.terminal || n.client.is_connected() { return; }
    let at = Instant::now() + after;
    n.reconnect_at = Some(n.reconnect_at.map_or(at, |r| r.min(at)));
    n.backoff_s = 1;
}

/// TOFU record: `known_servers.json` maps "host:port" to the hex key. A
/// changed key is reported loudly (the browser's `--server-key` pin is the
/// enforced path).
fn remember_server_key(path: &Path, server: &str, key: &[u8; 32]) {
    let hex_key = hex::encode(key);
    let mut map: std::collections::BTreeMap<String, String> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    match map.get(server) {
        Some(k) if *k == hex_key => return,
        Some(k) => warn!(%server, old = %k, new = %hex_key,
            "SERVER IDENTITY CHANGED since the last visit (reinstalled server, or someone in the middle); \
             pass --server-key to enforce a key"),
        None => info!(%server, key = %hex_key, "first visit: remembering the server key"),
    }
    map.insert(server.to_string(), hex_key);
    if let Ok(s) = serde_json::to_string_pretty(&map) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Leave on purpose: a `leave` record on channel 2, then close the connection and
/// flush the CLOSE repeats (sidecar exit, `--parent-pid`).
pub(super) async fn leave(sock: &UdpSocket, shared: &Arc<Mutex<SharedState>>, reason: u8) {
    let connected = NET.get().is_some() && net().client.is_connected();
    if !connected {
        return;
    }
    let _ = shared;
    let leave = hsmp_ipc::schema::session::Leave { reason, _r: [0; 7] };
    let _ = send_msg(sock, hsmp_ipc::wire::encode(0, 0, &leave, &[])).await;
    let code = if reason == proto::v5::LeaveReason::GAME_EXITED { close_code::GAME_EXITED } else { close_code::NORMAL };
    {
        let mut nc = net();
        nc.terminal = true;
        let now = nc.now_ms();
        nc.client.close(now, code, "leave");
    }
    for _ in 0..4 {
        let out = net().drain();
        send_all(sock, out).await;
        tokio::time::sleep(Duration::from_millis(31)).await;
    }
}

// RTT ping task: a `ping` record every 2 s (RTT and clock offset for the link record).
pub(super) fn spawn_ping_task(sock: &Arc<UdpSocket>, shared: &Arc<Mutex<SharedState>>) -> tokio::task::JoinHandle<()> {
    let sock = sock.clone();
    let shared = shared.clone();
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            let connected = shared.lock().await.status == "connected";
            if !connected { continue; }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64).unwrap_or(0);
            let _ = send_msg(&sock, hsmp_ipc::wire::encode(0, 0, &hsmp_ipc::schema::session::Ping { client_time_ms: now }, &[])).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_state_order() {
        use hsmp_ipc::schema::session::link_state as ls;
        assert_eq!(link_state(true, false, 100, STALL_MS), ls::UP);
        assert_eq!(link_state(true, false, STALL_MS + 1, STALL_MS), ls::STALLED, "connected but silent = stalled");
        assert_eq!(link_state(false, false, 0, STALL_MS), ls::RECONNECTING);
        assert_eq!(link_state(false, true, 0, STALL_MS), ls::TERMINAL, "terminal wins over everything");
        assert_eq!(link_state(true, true, 0, STALL_MS), ls::TERMINAL);
    }

    /// One forged pre-cookie reject must not end an
    /// automatic reconnect for good.
    #[test]
    fn unauthenticated_rejects_are_retried_on_reconnect() {
        use hsmp_net::net::handshake::reject_code as rc;
        const T: u64 = UNAUTH_TERMINAL_MS;
        // Authenticated: always believed.
        assert!(reject_is_terminal(rc::BANNED, true, true, 0, 0));
        assert!(reject_is_terminal(rc::FULL, true, false, 0, 0));
        // First connect: a version / content mismatch ends it once a second
        // attempt agrees (one forged packet is not enough).
        assert!(!reject_is_terminal(rc::VERSION, false, false, 1, 0));
        assert!(reject_is_terminal(rc::VERSION, false, false, 2, 250));
        assert!(reject_is_terminal(rc::CONTENT, false, false, 2, 250));
        assert!(!reject_is_terminal(rc::BANNED, false, false, 1, 0), "codes a real server never sends pre-cookie");
        assert!(!reject_is_terminal(rc::BANNED, false, false, 40, T - 1));
        // Reconnect: five forged rejects, one per backoff step, do not end the
        // session; only a whole handshake timeout of nothing but rejects
        // does.
        assert!(!reject_is_terminal(rc::VERSION, false, true, 1, 0));
        assert!(!reject_is_terminal(rc::VERSION, false, true, MAX_UNAUTH_REJECTS, 1_000));
        assert!(!reject_is_terminal(rc::CONTENT, false, true, 40, T - 1));
        assert!(!reject_is_terminal(rc::CONTENT, false, true, MAX_UNAUTH_REJECTS - 1, T));
        assert!(reject_is_terminal(rc::VERSION, false, true, MAX_UNAUTH_REJECTS, T));
    }

    /// An unauthenticated reject restarts the handshake 250 ms later
    /// without consuming a backoff step; repeated ones keep the earliest time.
    #[test]
    fn unauthenticated_reject_retries_soon_without_backoff() {
        let seed = [9u8; 32];
        let mut nc = NetClient {
            client: Client::new(ClientConfig::new(seed, "t"), ConnConfig::default(), 0, &mut rand::rngs::OsRng),
            t0: Instant::now(),
            params: Params { seed, nick: "t".into(), version_min: 5, version_max: 5, content_hash: [0; 32], explicit_key: None },
            learned_key: Some([1; 32]), reconnect_at: None, backoff_s: 4, terminal: false,
            server: "127.0.0.1:1".into(), known_path: PathBuf::from("k.json"),
            last_rx: Instant::now(), attempts: 0, down_since: None, unauth_rejects: 0, unauth_since: None,
        };
        retry_after_unauth_reject(&mut nc);
        let at = nc.reconnect_at.expect("scheduled");
        assert!(at <= Instant::now() + UNAUTH_RETRY && at + UNAUTH_RETRY >= Instant::now() + UNAUTH_RETRY / 2);
        assert_eq!(nc.backoff_s, 4, "no backoff step consumed");
        retry_after_unauth_reject(&mut nc);
        assert!(nc.reconnect_at.unwrap() <= at + Duration::from_millis(5));
        nc.terminal = true;
        nc.reconnect_at = None;
        retry_after_unauth_reject(&mut nc);
        assert!(nc.reconnect_at.is_none());
    }

    /// The sidecar reaches an IPv6 server (it used to bind 0.0.0.0 and fail
    /// to connect to any IPv6 address) as well as an IPv4 one.
    #[tokio::test]
    async fn connects_on_the_server_address_family() {
        async fn round_trip(server: &UdpSocket, target: &str) {
            let (c, to) = bind_for(target).await.unwrap();
            c.send_to(b"hi", to).await.unwrap();
            let mut b = [0u8; 8];
            let (n, from) = tokio::time::timeout(Duration::from_secs(2), server.recv_from(&mut b)).await.unwrap().unwrap();
            assert_eq!(&b[..n], b"hi");
            server.send_to(b"ok", from).await.unwrap();
            let (n, _) = tokio::time::timeout(Duration::from_secs(2), c.recv_from(&mut b)).await.unwrap().unwrap();
            assert_eq!(&b[..n], b"ok");
        }
        let v4 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        round_trip(&v4, &format!("127.0.0.1:{}", v4.local_addr().unwrap().port())).await;
        round_trip(&v4, &format!("localhost:{}", v4.local_addr().unwrap().port())).await;
        match UdpSocket::bind("[::1]:0").await {
            Ok(v6) => round_trip(&v6, &format!("[::1]:{}", v6.local_addr().unwrap().port())).await,
            Err(e) => eprintln!("no IPv6 loopback here ({e}); the IPv6 case is skipped"),
        }
        assert!(bind_for("no-port").await.is_err());
    }

    /// Datagrams that queued while the process was stalled are all
    /// processed, without blocking, before the timer pass runs.
    #[tokio::test]
    async fn queued_datagrams_are_drained_without_blocking() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        a.connect(b.local_addr().unwrap()).await.unwrap();
        b.connect(a.local_addr().unwrap()).await.unwrap();
        let mut buf = vec![0u8; 2048];
        let t0 = Instant::now();
        assert_eq!(drain_socket(&a, &mut buf, 4096, |_| panic!("nothing queued")), 0, "empty socket: returns at once");
        for i in 0..200u32 { b.send(&i.to_le_bytes()).await.unwrap(); }
        tokio::time::sleep(Duration::from_millis(50)).await; // "stalled" while they queue
        let mut got = Vec::new();
        let n = drain_socket(&a, &mut buf, 4096, |d| got.push(u32::from_le_bytes([d[0], d[1], d[2], d[3]])));
        assert_eq!(n, 200);
        assert_eq!(got, (0..200).collect::<Vec<_>>(), "in arrival order");
        assert!(t0.elapsed() < Duration::from_millis(500));
        // The cap bounds one pass.
        for i in 0..10u32 { b.send(&i.to_le_bytes()).await.unwrap(); }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(drain_socket(&a, &mut buf, 4, |_| {}), 4);
        assert_eq!(drain_socket(&a, &mut buf, 4096, |_| {}), 6);
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_terminal_never_reconnects() {
        let seed = [7u8; 32];
        let mut nc = NetClient {
            client: Client::new(ClientConfig::new(seed, "t"), ConnConfig::default(), 0, &mut rand::rngs::OsRng),
            t0: Instant::now(),
            params: Params { seed, nick: "t".into(), version_min: 5, version_max: 5, content_hash: [0; 32], explicit_key: None },
            learned_key: None, reconnect_at: None, backoff_s: 1, terminal: false,
            server: "127.0.0.1:1".into(), known_path: PathBuf::from("k.json"),
            last_rx: Instant::now(), attempts: 0, down_since: None, unauth_rejects: 0, unauth_since: None,
        };
        // Verified reset / dead path: the first attempt goes out at once.
        schedule_reconnect(&mut nc, true);
        assert!(nc.reconnect_at.take().expect("scheduled") <= Instant::now(), "immediate first retry");
        nc.backoff_s = 1;
        let mut waits = vec![];
        for _ in 0..6 {
            schedule_reconnect(&mut nc, false);
            let at = nc.reconnect_at.take().expect("scheduled");
            waits.push(((at - Instant::now()).as_millis() as u64 + 500) / 1000);
        }
        assert_eq!(waits, vec![1, 2, 4, 8, 8, 8], "1, 2, 4, 8 s then capped (same identity every time)");
        assert!(nc.down_since.is_some(), "the outage start is recorded");
        nc.terminal = true;
        schedule_reconnect(&mut nc, true);
        assert!(nc.reconnect_at.is_none(), "kicked / replaced / closed / rejected never reconnect");
        // the rebuilt client keeps the player key (the server keeps the seat by key)
        let k1 = nc.client.player_key();
        nc.client = nc.build();
        assert_eq!(nc.client.player_key(), k1);
    }
}

//! hsmp-loadtest â€” spins up N simulated clients speaking the real protocol
//! v5 (handshake, encrypted channels) against a freshly spawned hsmp-server
//! and reports throughput, server-added relay latency, delivery ratio,
//! server CPU and decrypt failures.
//!
//!   hsmp-loadtest --server-bin target/release/hsmp-server.exe --clients 2,4,8,16,32
//!
//! Each client: handshakes, then sends root state at 30 Hz, a ~290 B
//! skeletal frame at 20 Hz, vitals at 5 Hz and a match ping at 1 Hz;
//! client 0 (the admin/host) also streams 12 world objects at 20 Hz.
//! Clients live in one process, so the `tick` field of root/skeletal carries
//! a shared microsecond clock and the receiver measures sendâ†’receive
//! latency, i.e. the latency the server adds (plus loopback).
//!
//!   hsmp-loadtest --v5-attack --target 127.0.0.1:7777
//!
//! Headless security probe against a running server: spoofed and replayed
//! handshake packets, replayed sealed datagrams, key material in cleartext,
//! amplification, old-protocol rejects. Prints `PASS <name>` / `FAIL <name>`
//! lines and exits 1 on any failure (run by scripts/e2e-test.sh).

mod proto;
mod query;
mod build_id;

use hsmp_net::net::{Client, ClientConfig, ClientEvent, ConnConfig};
use proto::PeerId;
use hsmp_ipc::schema::world::{self as wrec, WorldObj};
use hsmp_ipc::schema::session as rec;

/// One message the bot received: a v6 record.
enum Rx {
    /// (kind, peer from the wire header, payload)
    Rec(u16, u32, Vec<u8>),
}

/// The text of a received `chat_in` record.
fn chat_text(m: &Rx) -> Option<String> {
    match m {
        Rx::Rec(rec::K_CHAT_IN, _, p) => hsmp_ipc::record::view::<rec::ChatIn>(p).ok().map(|v| v.head.text.lossy().into_owned()),
        _ => None,
    }
}

/// The `leave` record (reason USER).
fn leave_msg() -> Vec<u8> {
    hsmp_ipc::wire::encode(0, 0, &rec::Leave { reason: 0, _r: [0; 7] }, &[])
}
use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

static T0: OnceLock<Instant> = OnceLock::new();
fn now_us() -> u32 { T0.get_or_init(Instant::now).elapsed().as_micros() as u32 }
fn now_ms() -> u64 { T0.get_or_init(Instant::now).elapsed().as_millis() as u64 }

#[derive(Default)]
struct Stats {
    lat_root_us: Vec<u32>,
    lat_skel_us: Vec<u32>,
    rx_root: u64,
    rx_skel: u64,
    rx_world: u64,
    rx_other: u64,
    rx_bytes: u64,
    dup_root: u64,
    aead_failed: u64,
    pkts_lost: u64,
    retransmits: u64,
    pair_root: HashMap<(usize, PeerId), u64>,
    pair_skel: HashMap<(usize, PeerId), u64>,
}

struct Args {
    server_bin: String,
    clients: Vec<usize>,
    secs: u64,
    port: u16,
    root_ts: bool,
    tick_hz: u32,
    spread: f32,
    v5_attack: bool,
    target: String,
    /// Bots connect here instead of the spawned server (an impairment proxy
    /// such as `hsmp-tools netsim --upstream 127.0.0.1:<port>`).
    via: Option<SocketAddr>,
}

fn parse_args() -> Args {
    let mut a = Args {
        server_bin: "target/release/hsmp-server.exe".into(),
        clients: vec![2, 4, 8, 16, 32],
        secs: 10,
        port: 47777,
        root_ts: false,
        tick_hz: 60,
        spread: 3000.0,
        v5_attack: false,
        target: "127.0.0.1:7777".into(),
        via: None,
    };
    let v: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < v.len() {
        let next = v.get(i + 1).cloned().unwrap_or_default();
        match v[i].as_str() {
            "--server-bin" => { a.server_bin = next; i += 1; }
            "--clients" => { a.clients = next.split(',').filter_map(|s| s.parse().ok()).collect(); i += 1; }
            "--secs" => { a.secs = next.parse().unwrap_or(10); i += 1; }
            "--port" => { a.port = next.parse().unwrap_or(47777); i += 1; }
            "--tick-hz" => { a.tick_hz = next.parse().unwrap_or(60); i += 1; }
            "--spread" => { a.spread = next.parse().unwrap_or(3000.0); i += 1; }
            "--root-ts" => { a.root_ts = true; }
            "--v5-attack" => { a.v5_attack = true; }
            "--target" => { a.target = next; i += 1; }
            "--via" => { a.via = next.parse().ok(); i += 1; }
            _ => {}
        }
        i += 1;
    }
    a
}

fn bind_socket() -> std::io::Result<UdpSocket> {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    let _ = s.set_recv_buffer_size(8 << 20);
    let _ = s.set_send_buffer_size(4 << 20);
    s.set_nonblocking(true)?;
    s.bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())?;
    UdpSocket::from_std(s.into())
}

fn cpu_ms(pid: u32) -> f64 {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command",
               &format!("(Get-Process -Id {}).TotalProcessorTime.TotalMilliseconds", pid)])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().replace(',', ".").parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

fn pct(v: &mut Vec<u32>, p: f64) -> f64 {
    if v.is_empty() { return f64::NAN; }
    v.sort_unstable();
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx] as f64 / 1000.0
}

// ---------------------------------------------------------------------------
// v5 bot transport
// ---------------------------------------------------------------------------

/// One simulated client: a v5 `Client` driven by a 5 ms timer. Every
/// datagram sent and received can be captured (attack probe).
struct Bot {
    sock: Arc<UdpSocket>,
    server: SocketAddr,
    client: Mutex<Client>,
    events: Mutex<Vec<ClientEvent>>,
    capture: Mutex<Option<Vec<(bool, Vec<u8>)>>>,
}

impl Bot {
    fn new(server: SocketAddr, seed: [u8; 32], nick: &str, versions: Option<(u16, u16)>) -> anyhow::Result<Arc<Bot>> {
        let mut cfg = ClientConfig::new(seed, nick);
        // Same build as the server under test: its content check lets the bots in.
        cfg.content_hash = build_id::content_hash();
        cfg.build = build_id::build_tag("hsmp-loadtest");
        if let Some((lo, hi)) = versions {
            cfg.version_min = lo;
            cfg.version_max = hi;
        }
        let client = Client::new(cfg, ConnConfig::default(), now_ms(), &mut rand::rngs::OsRng);
        Ok(Arc::new(Bot {
            sock: Arc::new(bind_socket()?),
            server,
            client: Mutex::new(client),
            events: Mutex::new(Vec::new()),
            capture: Mutex::new(None),
        }))
    }

    fn capture_start(&self) { *self.capture.lock().unwrap() = Some(Vec::new()); }
    fn captured(&self) -> Vec<(bool, Vec<u8>)> { self.capture.lock().unwrap().clone().unwrap_or_default() }
    fn record(&self, outbound: bool, dg: &[u8]) {
        if let Some(c) = self.capture.lock().unwrap().as_mut() { c.push((outbound, dg.to_vec())); }
    }

    async fn flush(&self, out: Vec<Vec<u8>>) {
        for dg in out {
            self.record(true, &dg);
            let _ = self.sock.send_to(&dg, self.server).await;
        }
    }

    fn drain(&self) -> Vec<Vec<u8>> {
        let mut c = self.client.lock().unwrap();
        let now = now_ms();
        let mut out = Vec::new();
        while let Some(dg) = c.poll_transmit(now) { out.push(dg); }
        let mut evs = self.events.lock().unwrap();
        while let Some(e) = c.poll_event() { evs.push(e); }
        out
    }

    /// Timer + receive loop until `stop_at`; messages go to `on_msg`.
    fn spawn_io(self: &Arc<Self>, stop_at: Instant, mut on_msg: impl FnMut(usize, Rx) + Send + 'static) -> tokio::task::JoinHandle<()> {
        let bot = self.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            let mut iv = tokio::time::interval(Duration::from_millis(5));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                if Instant::now() > stop_at { break; }
                tokio::select! {
                    _ = iv.tick() => {
                        let out = bot.drain();
                        bot.flush(out).await;
                    }
                    r = bot.sock.recv_from(&mut buf) => {
                        let Ok((n, _)) = r else { continue };
                        bot.record(false, &buf[..n]);
                        {
                            let mut c = bot.client.lock().unwrap();
                            c.handle(now_ms(), &buf[..n]);
                        }
                        let out = bot.drain();
                        bot.flush(out).await;
                        let evs: Vec<ClientEvent> = std::mem::take(&mut *bot.events.lock().unwrap());
                        let mut keep = Vec::new();
                        for e in evs {
                            match e {
                                ClientEvent::Message(d) => {
                                    if let Ok((h, p)) = proto::decode_msg(&d.data) {
                                        on_msg(n, Rx::Rec(h.kind, h.peer, p.to_vec()));
                                    }
                                }
                                other => keep.push(other),
                            }
                        }
                        bot.events.lock().unwrap().extend(keep);
                    }
                }
            }
        })
    }

    /// Wait for Connected (or a terminal event), up to `timeout`.
    async fn wait_event(&self, timeout: Duration, pred: impl Fn(&ClientEvent) -> bool) -> Option<ClientEvent> {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            {
                let evs = self.events.lock().unwrap();
                if let Some(e) = evs.iter().find(|e| pred(e)) { return Some(e.clone()); }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        None
    }

    /// Queue a v6 record message (`hsmp_ipc::wire` framed) on its kind's channel.
    async fn send_msg(&self, msg: Vec<u8>) -> Vec<Vec<u8>> {
        let Some(mode) = hsmp_ipc::wire::split(&msg).ok().and_then(|(h, _)| proto::record_mode(h.kind, h.peer)) else { return Vec::new() };
        self.send_raw(mode, msg).await
    }

    async fn send_raw(&self, mode: hsmp_net::net::SendMode, msg: Vec<u8>) -> Vec<Vec<u8>> {
        let out = {
            let mut c = self.client.lock().unwrap();
            if c.send(mode, msg).is_err() { return Vec::new(); }
            let now = now_ms();
            let mut out = Vec::new();
            if c.is_connected() { while let Some(dg) = c.poll_transmit(now) { out.push(dg); } }
            out
        };
        self.flush(out.clone()).await;
        out
    }

    /// Queue one v6 record message (`hsmp_ipc::wire` framed) on its kind's channel.
    async fn send_rec(&self, msg: Vec<u8>) -> Vec<Vec<u8>> {
        let out = {
            let mut c = self.client.lock().unwrap();
            let Ok((h, _)) = hsmp_ipc::wire::split(&msg) else { return Vec::new() };
            let Some(mode) = proto::record_mode(h.kind, h.peer) else { return Vec::new() };
            if c.send(mode, msg).is_err() { return Vec::new(); }
            let now = now_ms();
            let mut out = Vec::new();
            if c.is_connected() { while let Some(dg) = c.poll_transmit(now) { out.push(dg); } }
            out
        };
        self.flush(out.clone()).await;
        out
    }

    fn stats(&self) -> hsmp_net::net::ConnStats {
        self.client.lock().unwrap().conn().map(|c| c.stats()).unwrap_or_default()
    }
}

fn is_connected(e: &ClientEvent) -> bool {
    matches!(e, ClientEvent::Connected { .. } | ClientEvent::Rejected { .. } | ClientEvent::Failed(_))
}

async fn run_client(
    idx: usize, n: usize, server: SocketAddr, secs: u64, root_ts: bool, spread: f32,
    stats: Arc<Mutex<Stats>>, tx_bytes: Arc<AtomicU64>, start_at: Instant,
    ids: Arc<Mutex<HashMap<PeerId, usize>>>,
) -> anyhow::Result<()> {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&(idx as u64 + 1).to_le_bytes());
    seed[8..16].copy_from_slice(&(std::process::id() as u64).to_le_bytes());
    let bot = Bot::new(server, seed, &format!("lt{}", idx), None)?;
    let stop_at = start_at + Duration::from_secs(secs);

    // Receiver.
    let local = Arc::new(Mutex::new(Stats::default()));
    let my_id = Arc::new(AtomicU64::new(0));
    let io = {
        let local = local.clone();
        let my_id = my_id.clone();
        let mut seen: HashMap<(PeerId, u32), ()> = HashMap::new();
        bot.spawn_io(stop_at + Duration::from_millis(300), move |len, body| {
            let t = now_us();
            {
                let Rx::Rec(k, peer_id, p) = &body;
                let (k, peer_id, p) = (*k, *peer_id, p.as_slice());
                if k == rec::K_WELCOME {
                    if let Ok(w) = hsmp_ipc::record::view::<rec::Welcome>(p) { my_id.store(w.head.peer_id as u64, Ordering::Relaxed); }
                } else if k == hsmp_ipc::schema::pose::K_ROOT || k == hsmp_ipc::schema::pose::K_POSE {
                    let measuring = Instant::now() >= start_at && Instant::now() <= stop_at;
                    if !measuring { return; }
                    let mut local = local.lock().unwrap();
                    local.rx_bytes += len as u64;
                    if k == hsmp_ipc::schema::pose::K_ROOT {
                        let Ok(v) = hsmp_ipc::record::view::<hsmp_ipc::schema::pose::Root>(p) else { return };
                        let tick = v.head.tick;
                        if seen.insert((peer_id, tick), ()).is_some() { local.dup_root += 1; return; }
                        local.rx_root += 1;
                        *local.pair_root.entry((idx, peer_id)).or_insert(0) += 1;
                        local.lat_root_us.push(t.wrapping_sub(tick));
                    } else {
                        let Ok(v) = hsmp_ipc::record::view::<hsmp_ipc::schema::pose::PoseHead>(p) else { return };
                        local.rx_skel += 1;
                        *local.pair_skel.entry((idx, peer_id)).or_insert(0) += 1;
                        local.lat_skel_us.push(t.wrapping_sub(v.head.tick));
                    }
                    return;
                }
            }
            if Instant::now() >= start_at && Instant::now() <= stop_at {
                let mut l = local.lock().unwrap();
                if matches!(body, Rx::Rec(wrec::K_WORLD_STATE, _, _)) { l.rx_world += 1 } else { l.rx_other += 1 }
            }
        })
    };
    match bot.wait_event(Duration::from_secs(10), is_connected).await {
        Some(ClientEvent::Connected { .. }) => {}
        other => anyhow::bail!("bot {idx}: no connection ({other:?})"),
    }
    // The Welcome follows in the same packet; give it a moment.
    for _ in 0..200 {
        if my_id.load(Ordering::Relaxed) != 0 { break; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ids.lock().unwrap().insert(my_id.load(Ordering::Relaxed) as PeerId, idx);

    // Sender: 30 Hz root, 20 Hz skeletal, 5 Hz vitals, 1 Hz ping, host world 20 Hz.
    let angle = idx as f32 / n.max(1) as f32 * std::f32::consts::TAU;
    let base = [angle.cos() * spread * 0.5, angle.sin() * spread * 0.5, 100.0];
    let mut k: u64 = 0;
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    let _ = root_ts; // protocol v6: every root carries its sender ts
    let mut pose_frame = Vec::new();
    let mut wseq = 0u32;
    while Instant::now() < stop_at {
        ticker.tick().await;
        k += 1;
        let ms = k * 10;
        let wob = (ms as f32 / 1000.0).sin() * 50.0;
        let pos = [base[0] + wob, base[1], base[2]];
        if ms % 33 < 10 {
            let t = now_us();
            let r = hsmp_ipc::schema::pose::Root { tick: t, ts: t / 1000, send_wall_ms: 0, pos, rot: [0.0, 0.0, 0.0, 1.0], vel: [0.0; 3] };
            bot.send_msg(hsmp_ipc::wire::encode(0, 0, &r, &[])).await;
            tx_bytes.fetch_add(120, Ordering::Relaxed);
        }
        if ms % 50 == 0 {
            let t = now_us();
            synth_pose(pos, (t / 1000) as f64, &mut pose_frame);
            let head = hsmp_ipc::schema::pose::PoseHead { tick: t, n: 0, _r: 0 };
            bot.send_msg(hsmp_ipc::wire::encode(0, 0, &head, &pose_frame)).await;
            tx_bytes.fetch_add(340, Ordering::Relaxed);
            // World v2: every peer joins level 1 (epoch 1 = fresh
            // server); peer 0 registers 12 bodies and streams them at 20 Hz.
            if wseq == 0 || ms % 2000 == 0 {
                bot.send_msg(hsmp_ipc::wire::encode(0, 0, &wrec::WorldSync { level: 1, _r: 0 }, &[])).await;
            }
            if idx == 0 {
                if wseq % 20 == 0 {
                    let entries: Vec<wrec::ManifestEntry> = (0..12u32).map(|i| wrec::ManifestEntry {
                        id: 1000 + i, chash: i, pos: [i as f32 * 100.0, 0.0, 50.0], _r: 0,
                    }).collect();
                    let h = wrec::ManifestHead { level: 1, epoch: 1, req: wseq + 1, n: 0, _r: 0 };
                    bot.send_msg(hsmp_ipc::wire::encode(0, 0, &h, &entries)).await;
                }
                let objects: Vec<WorldObj> = (0..12).map(|i| WorldObj::from_parts(
                    1000 + i, [i as f32 * 100.0, 0.0, 50.0 + wob], [0.0; 3], [0.0; 3], 0,
                )).collect();
                let h = wrec::WorldStateHead { level: 1, epoch: 1, seq: wseq, ts: ms as u32, n: 0, _r: 0, _r2: 0 };
                bot.send_msg(hsmp_ipc::wire::encode(0, 0, &h, &objects)).await;
            }
            wseq += 1;
        }
        if ms % 200 == 0 {
            let mut v = proto::vitals::unknown();
            v.seq = k as u32;
            proto::vitals::set(&mut v, proto::vitals::I_HEALTH, 100.0);
            bot.send_msg(hsmp_ipc::wire::encode(0, 0, &v, &[])).await;
        }
        if ms % 1000 == 0 {
            // The game-status report (the old `ping:0:0` verb): liveness, nothing loaded.
            bot.send_rec(hsmp_ipc::wire::encode(0, 0, &rec::GameStatus::default(), &[])).await;
        }
    }
    let st = bot.stats();
    bot.send_rec(leave_msg()).await;
    let _ = io.await;
    let local = std::mem::take(&mut *local.lock().unwrap());
    let mut s = stats.lock().unwrap();
    s.lat_root_us.extend(local.lat_root_us);
    s.lat_skel_us.extend(local.lat_skel_us);
    s.rx_root += local.rx_root;
    s.rx_skel += local.rx_skel;
    s.rx_world += local.rx_world;
    s.rx_other += local.rx_other;
    s.rx_bytes += local.rx_bytes;
    s.dup_root += local.dup_root;
    s.aead_failed += st.auth_failed;
    s.pkts_lost += st.pkts_lost;
    s.retransmits += st.retransmits;
    for (k, v) in local.pair_root { *s.pair_root.entry(k).or_insert(0) += v; }
    for (k, v) in local.pair_skel { *s.pair_skel.entry(k).or_insert(0) += v; }
    Ok(())
}

fn spawn_server(bin: &str, port: u16, tick_hz: u32) -> anyhow::Result<Child> {
    let bans = std::env::temp_dir().join(format!("hsmp-lt-bans-{}.txt", port));
    let key = std::env::temp_dir().join(format!("hsmp-lt-key-{}.key", port));
    let child = Command::new(bin)
        .args(["--bind", &format!("127.0.0.1:{}", port), "--tick-hz", &tick_hz.to_string(),
               "--max-peers", "64", "--bans-file", bans.to_str().unwrap(), "--key-file", key.to_str().unwrap()])
        .env("RUST_LOG", "hsmp_server=warn,hsmp_server::perf=info,hsmp_server::server::dispatch=info,hsmp_server::net=info")
        .env("HSMP_PERF", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(child)
}

/// `aead_failed=N` from the server's newest "v5 transport stats" line.
fn server_aead_failed(lines: &[String]) -> Option<u64> {
    let l = strip_ansi(lines.iter().rev().find(|l| l.contains("v5 transport stats"))?);
    let i = l.find("aead_failed=")? + "aead_failed=".len();
    l[i..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Remove terminal colour codes (`ESC [ ... m`) from a log line.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            for d in it.by_ref() { if d.is_ascii_alphabetic() { break; } }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" { fn timeBeginPeriod(u_period: u32) -> u32; }

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Same 1 ms timer resolution as the server, so client send cadence and
    // receive wake-ups don't add 15.6 ms Windows timer quantisation.
    #[cfg(windows)]
    unsafe { timeBeginPeriod(1); }
    let a = parse_args();
    T0.get_or_init(Instant::now);
    if a.v5_attack {
        let ok = attack::run(&a.target).await;
        std::process::exit(if ok { 0 } else { 1 });
    }
    println!("server={} secs={} root_ts={} tick_hz={} spread={}", a.server_bin, a.secs, a.root_ts, a.tick_hz, a.spread);
    println!("{:>3} | {:>6} | {:>17} | {:>17} | {:>9} | {:>9} | {:>10} | {:>10}",
             "N", "srvCPU", "root lat p50/p99", "skel lat p50/p99", "root dlv", "skel dlv", "KB/s/clnt", "KB/s total");
    let mut any_decrypt_fail = false;
    for (round, &n) in a.clients.iter().enumerate() {
        let port = a.port + round as u16;
        let mut child = spawn_server(&a.server_bin, port, a.tick_hz)?;
        let pid = child.id();
        // Drain server output so its pipes never block; keep perf + stats lines.
        let perf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                     child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let perf = perf.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                    if line.contains("perf") || line.contains("v5 transport stats") { perf.lock().unwrap().push(line); }
                }
            });
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let server: SocketAddr = match a.via { Some(v) => v, None => format!("127.0.0.1:{}", port).parse()? };
        let stats = Arc::new(Mutex::new(Stats::default()));
        let tx = Arc::new(AtomicU64::new(0));
        let ids: Arc<Mutex<HashMap<PeerId, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        // Handshakes happen first; measurement window starts after a 2 s warm-up.
        let start_at = Instant::now() + Duration::from_millis(2000);
        let mut handles = Vec::new();
        for i in 0..n {
            handles.push(tokio::spawn(run_client(i, n, server, a.secs, a.root_ts, a.spread,
                                                 stats.clone(), tx.clone(), start_at, ids.clone())));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep_until((start_at).into()).await;
        let cpu0 = cpu_ms(pid);
        let t0 = Instant::now();
        tokio::time::sleep(Duration::from_secs(a.secs)).await;
        let cpu1 = cpu_ms(pid);
        let wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let perf_during = perf.lock().unwrap().iter().rev().find(|l| l.contains("perf")).cloned();
        let mut failed_bots = 0;
        for h in handles {
            if let Ok(Err(e)) = h.await { eprintln!("{e}"); failed_bots += 1; }
        }
        // One more stats line after the run (logged every 10 s).
        tokio::time::sleep(Duration::from_millis(10_500)).await;
        let _ = child.kill();
        let _ = child.wait();

        let mut s = stats.lock().unwrap();
        let cpu_pct = (cpu1 - cpu0) / wall_ms * 100.0;
        // Expected deliveries if every frame reached every other client.
        let exp_root = (n * (n - 1)) as f64 * 30.0 * a.secs as f64;
        let exp_skel = (n * (n - 1)) as f64 * 20.0 * a.secs as f64;
        let (r50, r99) = (pct(&mut s.lat_root_us, 0.50), pct(&mut s.lat_root_us, 0.99));
        let (k50, k99) = (pct(&mut s.lat_skel_us, 0.50), pct(&mut s.lat_skel_us, 0.99));
        let kbps_total = s.rx_bytes as f64 / 1024.0 / a.secs as f64;
        println!("{:>3} | {:>5.1}% | {:>7.2}/{:>7.2}ms | {:>7.2}/{:>7.2}ms | {:>8.1}% | {:>8.1}% | {:>10.1} | {:>10.1}",
                 n, cpu_pct, r50, r99, k50, k99,
                 s.rx_root as f64 / exp_root * 100.0, s.rx_skel as f64 / exp_skel * 100.0,
                 kbps_total / n as f64, kbps_total);
        // Fidelity: clients sit on a circle, so each one's 2 nearest players
        // are its ring neighbours (idx Â± 1). Rates are frames/s received.
        let ids = ids.lock().unwrap().clone();
        let (mut near_r, mut near_k, mut far_r, mut far_k) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (&src_pid, &j) in &ids {
            for i in 0..n {
                if i == j { continue; }
                let ring = ((i as i64 - j as i64).rem_euclid(n as i64)).min((j as i64 - i as i64).rem_euclid(n as i64));
                let r = *s.pair_root.get(&(i, src_pid)).unwrap_or(&0) as f64 / a.secs as f64;
                let k = *s.pair_skel.get(&(i, src_pid)).unwrap_or(&0) as f64 / a.secs as f64;
                if ring == 1 { near_r.push(r); near_k.push(k); } else { far_r.push(r); far_k.push(k); }
            }
        }
        let med = |v: &mut Vec<f64>| { if v.is_empty() { return f64::NAN; }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
        let mn = |v: &Vec<f64>| v.iter().cloned().fold(f64::INFINITY, f64::min);
        println!("      fidelity: nearest-2 root min {:.0} Hz / skel min {:.0} Hz | others root med {:.0} Hz / skel med {:.0} Hz",
                 mn(&near_r), mn(&near_k), med(&mut far_r), med(&mut far_k));
        let p = perf.lock().unwrap();
        let srv_aead = server_aead_failed(&p);
        println!("      v5: bots connected {}/{} | decrypt failures: clients {} server {} | client pkts lost {} retransmits {}",
                 n - failed_bots, n, s.aead_failed,
                 srv_aead.map(|v| v.to_string()).unwrap_or_else(|| "?".into()), s.pkts_lost, s.retransmits);
        if s.aead_failed > 0 || srv_aead != Some(0) || failed_bots > 0 { any_decrypt_fail = true; }
        if let Some(last) = perf_during { println!("      server: {}", strip_ansi(last.trim())); }
        if let Some(c) = p.iter().rev().find(|l| l.contains("v5 connection stats (per-connection totals)")) {
            println!("      server conns: {}", strip_ansi(c.trim()));
        }
        let _ = s.dup_root;
    }
    if any_decrypt_fail {
        println!("RESULT: FAIL (decrypt failures or bots that never connected)");
        std::process::exit(1);
    }
    println!("RESULT: PASS (0 decrypt failures)");
    Ok(())
}

// ---------------------------------------------------------------------------
// --v5-attack: headless security probe
// ---------------------------------------------------------------------------

mod attack {
    use super::*;

    struct R { ok: bool }
    impl R {
        fn check(&mut self, name: &str, ok: bool, detail: String) {
            if ok { println!("PASS {name}: {detail}"); } else { println!("FAIL {name}: {detail}"); self.ok = false; }
        }
    }

    /// Player count from the browser query (same port).
    async fn players(target: SocketAddr) -> Option<u32> {
        let s = bind_socket().ok()?;
        for nonce in 1..=5u64 {
            let _ = s.send_to(&query::build_request(nonce), target).await;
            let mut buf = [0u8; 2048];
            if let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(400), s.recv_from(&mut buf)).await {
                if let Some((_, info)) = query::parse_reply(&buf[..n]) { return Some(info.players); }
            }
        }
        None
    }

    /// Send datagrams from a fresh socket and collect every reply for `wait`.
    async fn probe(target: SocketAddr, dgs: &[Vec<u8>], wait: Duration) -> Vec<Vec<u8>> {
        let s = bind_socket().expect("bind");
        for d in dgs { let _ = s.send_to(d, target).await; }
        collect(&s, wait).await
    }

    async fn collect(s: &UdpSocket, wait: Duration) -> Vec<Vec<u8>> {
        let end = Instant::now() + wait;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 65536];
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match tokio::time::timeout(left, s.recv_from(&mut buf)).await {
                Ok(Ok((n, _))) => out.push(buf[..n].to_vec()),
                _ => break,
            }
        }
        out
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        needle.len() <= hay.len() && hay.windows(needle.len()).any(|w| w == needle)
    }

    fn seed(tag: u8) -> [u8; 32] {
        let mut s: [u8; 32] = rand::random();
        s[0] = tag;
        s
    }

    pub async fn run(target: &str) -> bool {
        let mut r = R { ok: true };
        let Ok(target) = target.parse::<SocketAddr>() else {
            println!("FAIL v5_args: bad --target {target}");
            return false;
        };
        let p0 = players(target).await;
        r.check("v5_query", p0.is_some(), format!("server answers the browser query (players={p0:?})"));
        let p0 = p0.unwrap_or(0);
        let stop = Instant::now() + Duration::from_secs(60);

        // 1. Unauthenticated garbage, short hellos, unknown-conn data and v4
        //    datagrams that are not a JOIN: no reply, no state.
        let mut junk: Vec<Vec<u8>> = Vec::new();
        for i in 0..1000usize {
            let len = 1 + (i * 37) % 1199;
            let mut d: Vec<u8> = (0..len).map(|_| rand::random::<u8>()).collect();
            d[0] = match i % 6 { 0 => 0xA1, 1 => 0xA3, 2 => 0xB0, 3 => 0xA2, 4 => 0x42, _ => 0x01 };
            if d[0] == 0x01 && d.len() >= 4 { d[1..4].copy_from_slice(&[0, 0, 0]); } // v4 Encrypted
            if d[0] == 0xA1 && d.len() >= 1200 { d.truncate(1199); }
            junk.push(d);
        }
        let bytes_in: usize = junk.iter().map(|d| d.len()).sum();
        let replies = probe(target, &junk, Duration::from_millis(700)).await;
        // A datagram for an unknown connection may draw a
        // stateless reset (25 B, never larger than the request, globally
        // limited to 50 burst + 50/s): the only reply junk may get. No
        // handshake state, no larger reply, no other kind.
        use hsmp_net::net::{PT_RESET, RESET_LEN};
        let only_resets = replies.iter().all(|d| d.len() == RESET_LEN && d[0] == PT_RESET);
        let bytes_out: usize = replies.iter().map(|d| d.len()).sum();
        r.check("v5_junk_no_reply", only_resets && replies.len() <= 100,
            format!("{} junk datagrams ({} B) -> {} replies ({} B, all stateless resets: {})",
                junk.len(), bytes_in, replies.len(), bytes_out, only_resets));

        // 2. A legitimate client with every datagram captured.
        let a = Bot::new(target, seed(0xA0), "probeA", None).expect("bot");
        a.capture_start();
        let got_a: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let io_a = { let g = got_a.clone(); a.spawn_io(stop, move |_, b| {
            if let Some(text) = chat_text(&b) { g.lock().unwrap().push(text); }
        }) };
        let ev = a.wait_event(Duration::from_secs(10), is_connected).await;
        r.check("v5_handshake", matches!(ev, Some(ClientEvent::Connected { .. })), format!("{ev:?}"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let cap = a.captured();
        let hello = cap.iter().find(|(o, d)| *o && d[0] == 0xA1).map(|(_, d)| d.clone()).unwrap_or_default();
        let challenge = cap.iter().find(|(o, d)| !*o && d[0] == 0xA2).map(|(_, d)| d.clone()).unwrap_or_default();
        let auth = cap.iter().find(|(o, d)| *o && d[0] == 0xA3).map(|(_, d)| d.clone()).unwrap_or_default();
        r.check("v5_hello_padded", hello.len() >= 1200, format!("hello {} B", hello.len()));
        r.check("v5_response_le_request", !challenge.is_empty() && challenge.len() <= hello.len(),
            format!("challenge {} B <= hello {} B", challenge.len(), hello.len()));

        // 3. No key material in any cleartext (or sealed) datagram.
        let secrets = a.client.lock().unwrap().debug_secrets().map(|s| (s.auth, s.c2s, s.s2c, s.resume, s.conn_id));
        match secrets {
            Some((auth_k, c2s, s2c, resume, _cid)) => {
                let keys = [("auth", auth_k), ("c2s", c2s), ("s2c", s2c), ("resume", resume)];
                let mut leaks = Vec::new();
                for (o, d) in &cap {
                    for (name, k) in &keys {
                        // Any 12-byte window of a key is a leak.
                        for w in k.windows(12) {
                            if contains(d, w) { leaks.push(format!("{name} in {} dg type {:#04x}", if *o { "out" } else { "in" }, d[0])); break; }
                        }
                    }
                }
                r.check("v5_no_key_in_clear", leaks.is_empty() && cap.len() >= 4,
                    format!("{} datagrams captured, leaks: {:?}", cap.len(), leaks));
            }
            None => r.check("v5_no_key_in_clear", false, "no secrets (handshake failed)".into()),
        }

        // 4. A captured Hello replayed from another address gets a cookie
        //    (one Challenge, <= the Hello) and allocates nothing.
        let p1 = players(target).await.unwrap_or(0);
        let rep = probe(target, &[hello.clone()], Duration::from_millis(500)).await;
        let ok = rep.len() == 1 && rep[0][0] == 0xA2 && rep[0].len() <= hello.len();
        r.check("v5_spoofed_hello", ok, format!("replies {:?} (sizes), hello {} B",
            rep.iter().map(|d| d.len()).collect::<Vec<_>>(), hello.len()));
        // 5. The captured Auth replayed from another address: cookie bound to
        //    the victim's address -> silently dropped.
        let rep = probe(target, &[auth.clone()], Duration::from_millis(500)).await;
        r.check("v5_replayed_auth_other_addr", rep.is_empty(), format!("{} replies", rep.len()));
        // ... and from the victim's own address: duplicate, dropped.
        let _ = a.sock.send_to(&auth, target).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let p2 = players(target).await.unwrap_or(0);
        r.check("v5_spoof_allocates_no_peer", p2 == p1 && p1 == p0 + 1,
            format!("players before {p0}, after connect {p1}, after replays {p2}"));

        // 6. A replayed sealed datagram is rejected by the replay window.
        let b = Bot::new(target, seed(0xB0), "probeB", None).expect("bot");
        let got_b: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let io_b = { let g = got_b.clone(); b.spawn_io(stop, move |_, body| {
            if let Some(text) = chat_text(&body) { g.lock().unwrap().push(text); }
        }) };
        let evb = b.wait_event(Duration::from_secs(10), is_connected).await;
        r.check("v5_second_client", matches!(evb, Some(ClientEvent::Connected { .. })), format!("{evb:?}"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let probe_text = format!("replay-probe-{}", rand::random::<u32>());
        let sent = a.send_rec(hsmp_ipc::wire::encode(0, 0, &rec::Chat { text: hsmp_ipc::layout::Str::new(&probe_text) }, &[])).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let other = bind_socket().expect("bind");
        for _ in 0..5 {
            for d in &sent {
                let _ = a.sock.send_to(d, target).await;     // same address: replay window
                let _ = other.send_to(d, target).await;      // other address: dropped
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        let n_b = got_b.lock().unwrap().iter().filter(|t| **t == probe_text).count();
        r.check("v5_replayed_data", !sent.is_empty() && n_b == 1,
            format!("{} sealed datagram(s) replayed 10x; B received the chat {} time(s)", sent.len(), n_b));
        let n_a = got_a.lock().unwrap().iter().filter(|t| **t == probe_text).count();
        r.check("v5_connection_survives_replays", n_a == 1, format!("A's own echo {} time(s)", n_a));

        // 7. Old protocol: a v5 handshake offering only v3..=v4 gets a
        //    readable PreReject (<= the Hello) before any cookie ...
        let old = Bot::new(target, seed(0xC0), "oldclient", Some((3, 4))).expect("bot");
        old.capture_start();
        let io_old = old.spawn_io(stop, |_, _| {});
        let ev = old.wait_event(Duration::from_secs(5), is_connected).await;
        let cap = old.captured();
        let pre = cap.iter().find(|(o, d)| !*o && d[0] == 0xA4).map(|(_, d)| d.len()).unwrap_or(usize::MAX);
        let ok = matches!(&ev, Some(ClientEvent::Rejected { authenticated: false, text, .. }) if text.contains("OUTDATED"))
            && pre <= 1200;
        r.check("v5_old_protocol_reject", ok, format!("{ev:?}, pre-reject {pre} B"));
        // ... and a v4 client's cleartext JOIN gets one bounded v4 S2CReject.
        let mut join = vec![0u8; 4 + 16 + 4];
        join.extend_from_slice(&4u32.to_le_bytes());
        join.extend_from_slice(&(10u64).to_le_bytes());
        join.extend_from_slice(b"OldWillie1");
        let rep = probe(target, &[join.clone()], Duration::from_millis(500)).await;
        let ok = rep.len() == 1 && rep[0].len() <= join.len() && rep[0].len() > 32
            && String::from_utf8_lossy(&rep[0][32..]).starts_with("OUTDATED");
        r.check("v4_join_outdated_reject", ok, format!("request {} B -> replies {:?}", join.len(),
            rep.iter().map(|d| (d.len(), String::from_utf8_lossy(d.get(32..).unwrap_or(&[])).to_string())).collect::<Vec<_>>()));

        // Leave cleanly.
        for bot in [&a, &b] {
            bot.send_rec(leave_msg()).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        io_a.abort(); io_b.abort(); io_old.abort();
        r.ok
    }
}

/// A codec v2 frame of a standing body (reference skeleton, identity rotations) with the
/// pelvis at `pos`: what the server's lag compensation accepts (tied to the root).
fn synth_pose(pos: [f32; 3], ts: f64, out: &mut Vec<u8>) {
    use hsmp_pose::posecodec::v2;
    let mut f = v2::Full { ts, ..Default::default() };
    f.bones[0].p = pos;
    for i in 1..v2::NB {
        let par = f.bones[v2::PARENT[i]].p;
        f.bones[i].p = [par[0] + v2::REF_T[i][0], par[1] + v2::REF_T[i][1], par[2] + v2::REF_T[i][2]];
    }
    v2::encode_into(&f, out);
}

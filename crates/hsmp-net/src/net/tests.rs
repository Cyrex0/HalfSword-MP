//! End-to-end tests: `Client` <-> `ServerEndpoint` over the mock link.

use std::collections::HashSet;
use std::net::SocketAddr;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::channel::{Delivery, SendMode, CH_ORDERED, CH_RELIABLE, CH_UNRELIABLE};
use super::conn::{close_code, ConnConfig};
use super::endpoint::{Client, ClientConfig, ClientEvent, DropReason, Incoming, PendingAuth, ServerConfig, ServerEndpoint};
use super::handshake::{reject_code, HsError};
use super::testlink::Link;
use super::{ConnId, HELLO_LEN, PT_AUTH, PT_DATA, PT_HELLO};

type Policy = Box<dyn Fn(&PendingAuth) -> Result<(), (u8, String)>>;

struct World {
    now: u64,
    server: ServerEndpoint,
    client: Client,
    up: Link,
    down: Link,
    caddr: SocketAddr,
    policy: Policy,
    cid: Option<ConnId>,
    server_in: Vec<Delivery>,
    client_in: Vec<Delivery>,
    client_ev: Vec<ClientEvent>,
    /// Every datagram the client put on the wire.
    up_log: Vec<Vec<u8>>,
    replies_ok: bool,
    migrations: Vec<(u64, SocketAddr)>,
}

impl World {
    fn new(seed: u64, up: Link, down: Link, ccfg: ClientConfig) -> World {
        let mut rng = StdRng::seed_from_u64(seed);
        let now = 1_000_000;
        let scfg = ServerConfig::new([42; 32]);
        let server = ServerEndpoint::new(scfg, ConnConfig::default(), now, Some(seed));
        let client = Client::new(ccfg, ConnConfig::default(), now, &mut rng);
        World {
            now,
            server,
            client,
            up,
            down,
            caddr: "203.0.113.7:50000".parse().unwrap(),
            policy: Box::new(|_| Ok(())),
            cid: None,
            server_in: vec![],
            client_in: vec![],
            client_ev: vec![],
            up_log: vec![],
            replies_ok: true,
            migrations: vec![],
        }
    }

    /// A world whose client uses `conn` (e.g. path-dead detection on).
    fn with_client_conn(seed: u64, up: Link, down: Link, conn: ConnConfig) -> World {
        let mut w = World::new(seed, up, down, ccfg());
        let mut rng = StdRng::seed_from_u64(seed ^ 0xC0FFEE);
        w.client = Client::new(ccfg(), conn, w.now, &mut rng);
        w
    }

    fn closed_with(&self, code: u8) -> bool {
        self.client_ev.iter().any(|e| matches!(e, ClientEvent::Closed { code: c, .. } if *c == code))
    }

    fn step(&mut self, dt: u64) {
        let now = self.now;
        while let Some(dg) = self.client.poll_transmit(now) {
            self.up_log.push(dg.clone());
            self.up.push(now, dg);
        }
        for (addr, dg) in self.server.poll_transmit(now) {
            // Sends to an address the client left (NAT rebind) are lost.
            if addr == self.caddr {
                self.down.push(now, dg);
            }
        }
        for dg in self.up.pop_due(now) {
            match self.server.handle(now, self.caddr, &dg) {
                Incoming::Reply(r) => {
                    self.replies_ok &= r.len() <= dg.len();
                    self.down.push(now, r);
                }
                Incoming::AuthRequest(p) => match (self.policy)(&p) {
                    Ok(()) => {
                        let cid = self.server.accept(now, p);
                        self.cid = Some(cid);
                        self.server.send(cid, SendMode::Ordered, b"welcome".to_vec()).unwrap();
                    }
                    Err((code, text)) => {
                        let r = self.server.reject(now, p, code, &text);
                        self.replies_ok &= r.len() <= dg.len();
                        self.down.push(now, r);
                    }
                },
                Incoming::Data { deliveries, migrated_from, probe, .. } => {
                    if let Some(p) = probe {
                        self.replies_ok &= p.len() <= dg.len();
                        self.down.push(now, p);
                    }
                    if let Some(old) = migrated_from {
                        self.migrations.push((now, old));
                    }
                    self.server_in.extend(deliveries)
                }
                Incoming::Dropped(_) => {}
            }
        }
        for dg in self.down.pop_due(now) {
            self.client.handle(now, &dg);
        }
        while let Some(ev) = self.client.poll_event() {
            match ev {
                ClientEvent::Message(d) => self.client_in.push(d),
                other => self.client_ev.push(other),
            }
        }
        self.now += dt;
    }

    fn run_until(&mut self, limit_ms: u64, mut done: impl FnMut(&World) -> bool) -> bool {
        let end = self.now + limit_ms;
        while self.now < end {
            self.step(2);
            if done(self) {
                return true;
            }
        }
        false
    }

    fn connected(&self) -> bool {
        self.client_ev.iter().any(|e| matches!(e, ClientEvent::Connected { .. }))
    }
}

fn ccfg() -> ClientConfig {
    let mut c = ClientConfig::new([11; 32], "Willie");
    c.build = "test-build".into();
    c
}

#[test]
fn handshake_round_trip_and_bidirectional_messages() {
    let mut w = World::new(1, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected() && !w.client_in.is_empty()));
    let skey = w.server.static_public();
    match &w.client_ev[0] {
        ClientEvent::Connected { server_key, version, .. } => {
            assert_eq!(*server_key, skey);
            assert_eq!(*version, crate::net::PROTOCOL_VERSION);
        }
        e => panic!("unexpected {e:?}"),
    }
    assert_eq!(
        w.client_in[0],
        Delivery {
            channel: CH_ORDERED,
            key: 0,
            data: b"welcome".to_vec()
        }
    );
    // Hello is padded; the client's first datagram is exactly HELLO_LEN.
    assert_eq!(w.up_log[0][0], PT_HELLO);
    assert_eq!(w.up_log[0].len(), HELLO_LEN);
    w.client.send(SendMode::Reliable, b"ready".to_vec()).unwrap();
    assert!(w.run_until(1_000, |w| !w.server_in.is_empty()));
    assert_eq!(w.server_in[0].data, b"ready");
    assert_eq!(w.server.conn_count(), 1);
    // Identity reached the server through the signed, encrypted Auth.
    let cid = w.cid.unwrap();
    assert!(w.server.conn(cid).unwrap().is_open());
}

#[test]
fn reliable_delivery_over_a_lossy_reordering_duplicating_link() {
    // 20 % loss, 10 % duplication, 30 ms +0..40 ms jitter, both directions,
    // over several independent schedules; plus one harsher 30 % loss run.
    for (seed, loss) in [(1u64, 0.20), (2, 0.20), (3, 0.20), (4, 0.20), (5, 0.30)] {
        lossy_run(seed, loss);
    }
}

fn lossy_run(seed: u64, loss: f64) {
    let up = Link::new(100 + seed, loss, 0.10, 30, 40);
    let down = Link::new(200 + seed, loss, 0.10, 30, 40);
    let mut w = World::new(seed, up, down, ccfg());
    assert!(w.run_until(15_000, |w| w.connected()), "handshake under loss");
    let cid = w.cid.unwrap();
    let mut rng = StdRng::seed_from_u64(99 + seed);
    const N: u32 = 300;
    let mut expect_big = Vec::new();
    for i in 0..N {
        // Every 25th ordered message is large enough to fragment (up to ~20 KB).
        let mut m = i.to_le_bytes().to_vec();
        if i % 25 == 0 {
            let extra: usize = rng.gen_range(2_000..20_000);
            m.extend((0..extra).map(|k| (k as u32 ^ i) as u8));
            expect_big.push(m.clone());
        }
        w.server.send(cid, SendMode::Ordered, m).unwrap();
        w.server
            .send(cid, SendMode::Reliable, (1_000_000 + i).to_le_bytes().to_vec())
            .unwrap();
        w.client.send(SendMode::Ordered, (2_000_000 + i).to_le_bytes().to_vec()).unwrap();
    }
    // A 60 Hz unreliable-latest stream from the client meanwhile.
    let mut stream_seq = 0u32;
    let mut next_sample = w.now;
    let done = |w: &World| {
        w.client_in.iter().filter(|d| d.channel == CH_ORDERED).count() == N as usize + 1
            && w.client_in.iter().filter(|d| d.channel == CH_RELIABLE).count() == N as usize
            && w.server_in.iter().filter(|d| d.channel == CH_ORDERED).count() == N as usize
    };
    let deadline = w.now + 120_000;
    while w.now < deadline && !done(&w) {
        if w.now >= next_sample {
            stream_seq += 1;
            w.client
                .send(SendMode::Latest { key: 0x0100_0001 }, stream_seq.to_le_bytes().to_vec())
                .unwrap();
            next_sample = w.now + 16;
        }
        w.step(2);
    }
    assert!(
        done(&w),
        "not everything delivered: client ordered {}, reliable {}, server ordered {}",
        w.client_in.iter().filter(|d| d.channel == CH_ORDERED).count(),
        w.client_in.iter().filter(|d| d.channel == CH_RELIABLE).count(),
        w.server_in.iter().filter(|d| d.channel == CH_ORDERED).count()
    );
    // Ordered: exactly once, in order (after the welcome).
    let ord: Vec<&Delivery> = w.client_in.iter().filter(|d| d.channel == CH_ORDERED).collect();
    assert_eq!(ord[0].data, b"welcome");
    let mut big = expect_big.iter();
    for (i, d) in ord[1..].iter().enumerate() {
        assert_eq!(&d.data[..4], &(i as u32).to_le_bytes(), "ordered message {i} out of order");
        if i % 25 == 0 {
            assert_eq!(&d.data, big.next().unwrap(), "fragmented message {i} corrupted");
        }
    }
    let sord: Vec<u32> = w
        .server_in
        .iter()
        .filter(|d| d.channel == CH_ORDERED)
        .map(|d| u32::from_le_bytes(d.data[..4].try_into().unwrap()))
        .collect();
    assert_eq!(sord, (0..N).map(|i| 2_000_000 + i).collect::<Vec<_>>());
    // Reliable-unordered: exactly once each.
    let rel: Vec<u32> = w
        .client_in
        .iter()
        .filter(|d| d.channel == CH_RELIABLE)
        .map(|d| u32::from_le_bytes(d.data[..4].try_into().unwrap()))
        .collect();
    let set: HashSet<u32> = rel.iter().copied().collect();
    assert_eq!(set.len(), rel.len(), "duplicate delivery on channel 1");
    assert_eq!(set, (0..N).map(|i| 1_000_000 + i).collect());
    // Unreliable-latest: never goes backwards, despite reordering and dups.
    let lat: Vec<u32> = w
        .server_in
        .iter()
        .filter(|d| d.channel == CH_UNRELIABLE)
        .map(|d| u32::from_le_bytes(d.data[..4].try_into().unwrap()))
        .collect();
    assert!(lat.len() > 10, "stream should mostly arrive");
    assert!(lat.windows(2).all(|p| p[1] > p[0]), "stale channel-0 sample delivered");
    // The link really did impair, and the transport really did recover.
    let st = w.server.conn(cid).unwrap().stats();
    assert!(st.retransmits > 0 && st.pkts_lost > 0, "{st:?}");
    assert!(st.srtt_ms > 30.0 && st.srtt_ms < 400.0, "srtt {}", st.srtt_ms);
    assert!(w.replies_ok);
}

#[test]
fn spoofed_and_replayed_handshake_packets_allocate_nothing() {
    let mut w = World::new(3, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let hello = w.up_log.iter().find(|d| d[0] == PT_HELLO).unwrap().clone();
    let auth = w.up_log.iter().find(|d| d[0] == PT_AUTH).unwrap().clone();
    let data = w.up_log.iter().find(|d| d[0] == PT_DATA).cloned();
    let attacker: SocketAddr = "198.51.100.66:4444".parse().unwrap();
    let now = w.now;
    // A captured Hello replayed from another address: a cookie, nothing else.
    match w.server.handle(now, attacker, &hello) {
        Incoming::Reply(r) => assert!(r.len() <= hello.len()),
        _ => panic!("expected a stateless challenge"),
    }
    assert_eq!(w.server.conn_count(), 1);
    // The captured Auth from another address: the cookie is bound to the address.
    assert!(matches!(
        w.server.handle(now, attacker, &auth),
        Incoming::Dropped(DropReason::Handshake(HsError::BadCookie))
    ));
    // The captured Auth replayed from the victim's own address: duplicate.
    assert!(matches!(
        w.server.handle(now, w.caddr, &auth),
        Incoming::Dropped(DropReason::DuplicateAuth)
    ));
    assert_eq!(w.server.conn_count(), 1);
    // A captured data packet replayed: rejected by the replay window.
    if let Some(d) = data {
        assert!(matches!(
            w.server.handle(now, w.caddr, &d),
            Incoming::Dropped(DropReason::Conn(super::conn::Drop::Duplicate))
        ));
        // ... and from another address too (path migration only follows
        // fresh authenticated packets).
        assert!(matches!(
            w.server.handle(now, attacker, &d),
            Incoming::Dropped(DropReason::Conn(super::conn::Drop::Duplicate))
        ));
    }
    // A fresh server never saw this client: the Auth alone gets nothing.
    let mut fresh = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), now, Some(5));
    assert!(matches!(
        fresh.handle(now, w.caddr, &auth),
        Incoming::Dropped(DropReason::Handshake(_))
    ));
    assert_eq!((fresh.conn_count(), fresh.preauth_state()), (0, 0));
}

/// Flushing the replay memory with other handshakes must not
/// re-open a captured Auth for replay (the rebuilt connection would reuse
/// the deterministic keys with next_seq = 0: keystream reuse). A full memory
/// fails closed instead of evicting.
#[test]
fn a_flooded_replay_memory_fails_closed() {
    let mut w = World::new(3, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let auth = w.up_log.iter().find(|d| d[0] == PT_AUTH).unwrap().clone();
    let now = w.now;
    let cid = w.server.conn_ids()[0];
    w.server.test_set_recent_cap(4);
    // The attacker completes other handshakes until the memory is full...
    for k in 1..=8u8 {
        w.server.test_remember(now, [k; 32]);
    }
    // ...and the victim's connection is gone (closed, reaped).
    w.server.test_drop_conn(cid);
    match w.server.handle(now, w.caddr, &auth) {
        Incoming::Dropped(DropReason::DuplicateAuth) | Incoming::Dropped(DropReason::RecentFull) => {}
        Incoming::AuthRequest(_) => panic!("captured Auth accepted again after a flood"),
        _ => panic!("unexpected outcome"),
    }
    assert_eq!(w.server.conn_count(), 0);
}

/// A fresh client's Hello → Challenge → Auth against `s` from `from`.
fn auth_from(s: &mut ServerEndpoint, now: u64, from: SocketAddr, seed: u8, rng: &mut StdRng) -> Option<Vec<u8>> {
    let mut c = Client::new(ClientConfig::new([seed; 32], "flood"), ConnConfig::default(), now, rng);
    let hello = c.poll_transmit(now)?;
    let Incoming::Reply(ch) = s.handle(now, from, &hello) else { return None };
    c.handle(now, &ch);
    c.poll_transmit(now)
}

/// One cookie-proven host running full handshakes as fast
/// as it can neither fills the fail-closed replay memory (per-source share)
/// nor gets more than its Auth budget through to the crypto; another
/// source's join still succeeds.
#[test]
fn one_source_flooding_auths_cannot_block_another_join() {
    let mut s = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), 0, Some(3));
    s.test_set_recent_cap(96);
    let mut rng = StdRng::seed_from_u64(5);
    let attacker: SocketAddr = "198.51.100.66:4444".parse().unwrap();
    let mut handed = 0usize;
    let mut limited = 0usize;
    let mut now = 1_000_000u64;
    // 20 Auths/s for 40 s from one address (each Auth is a complete,
    // valid handshake; the application accepts every one).
    for i in 0..800u32 {
        let Some(auth) = auth_from(&mut s, now, attacker, (i % 250) as u8, &mut rng) else { panic!("no auth") };
        match s.handle(now, attacker, &auth) {
            Incoming::AuthRequest(p) => {
                handed += 1;
                let cid = s.accept(now, p);
                s.test_drop_conn(cid);
            }
            Incoming::Dropped(DropReason::AuthRateLimited) => limited += 1,
            Incoming::Dropped(DropReason::RecentFull) => panic!("one source filled the replay memory"),
            Incoming::Dropped(DropReason::DuplicateAuth) => {}
            _ => panic!("unexpected outcome"),
        }
        now += 50;
    }
    // Budget: burst 10 + 5/s × 40 s, and never more than the per-source share live.
    assert!(handed <= 10 + 5 * 40 + 1, "{handed} Auths reached the crypto");
    assert!(limited > 500, "limited {limited}");
    // IPv6 neighbours in the same /64 share that budget.
    let v6a: SocketAddr = "[2001:db8:1:2::1]:1000".parse().unwrap();
    let v6b: SocketAddr = "[2001:db8:1:2::ffff]:1000".parse().unwrap();
    let mut v6_handed = 0;
    for i in 0..40u32 {
        let from = if i % 2 == 0 { v6a } else { v6b };
        let auth = auth_from(&mut s, now, from, 7, &mut rng).unwrap();
        if let Incoming::AuthRequest(p) = s.handle(now, from, &auth) {
            v6_handed += 1;
            let cid = s.accept(now, p);
            s.test_drop_conn(cid);
        }
    }
    assert_eq!(v6_handed, 10, "one /64 = one budget (burst only, same instant)");
    // A legitimate player from elsewhere still gets in.
    let legit: SocketAddr = "203.0.113.9:5000".parse().unwrap();
    let auth = auth_from(&mut s, now, legit, 99, &mut rng).unwrap();
    assert!(matches!(s.handle(now, legit, &auth), Incoming::AuthRequest(_)), "legit join blocked");
    // Pruning is by expiry order: after the memory window everything is gone.
    let later = now + 40_000;
    let auth = auth_from(&mut s, later, legit, 98, &mut rng).unwrap();
    let _ = s.handle(later, legit, &auth);
    let (map, queue) = s.test_recent_sizes();
    assert_eq!((map, queue), (0, 0), "expired entries popped off the queue front");
}

/// An idle server (no handshake traffic) still rotates its
/// ephemeral key from the timer path.
#[test]
fn idle_server_rotates_its_ephemeral_key() {
    let mut s = ServerEndpoint::new(ServerConfig::new([1; 32]), ConnConfig::default(), 0, Some(9));
    assert_eq!(s.test_rotated_at(), 0);
    let period = 120_000;
    let mut now = 0;
    while now <= 2 * period + 10 {
        let _ = s.poll_transmit(now);
        now += 5_000;
    }
    assert!(s.test_rotated_at() >= 2 * period, "rotated at {}", s.test_rotated_at());
}

#[test]
fn amplification_is_bounded_and_spoofed_floods_leave_no_state() {
    let mut s = ServerEndpoint::new(ServerConfig::new([1; 32]), ConnConfig::default(), 0, Some(9));
    let mut rng = StdRng::seed_from_u64(1234);
    let mut client_rng = StdRng::seed_from_u64(77);
    for i in 0..2_000u32 {
        let from: SocketAddr = format!("10.{}.{}.{}:{}", i % 200, (i / 200) % 200, i % 7, 1024 + i)
            .parse()
            .unwrap();
        let dg: Vec<u8> = match i % 5 {
            // Genuine-looking hellos from spoofed addresses.
            0 => {
                let mut cfg = ClientConfig::new([i as u8; 32], "x");
                if i % 10 == 0 {
                    cfg.version_min = 1;
                    cfg.version_max = 2; // provokes a (bounded) reject
                }
                let mut c = Client::new(cfg, ConnConfig::default(), 0, &mut client_rng);
                c.poll_transmit(0).unwrap()
            }
            // Random garbage with valid type bytes.
            1 => {
                let n = rng.gen_range(1..1500);
                let mut v: Vec<u8> = (0..n).map(|_| rng.gen()).collect();
                v[0] = [PT_HELLO, PT_AUTH, PT_DATA, 0xA2, 0xA4][rng.gen_range(0..5)];
                v
            }
            // Short hellos (would amplify if answered).
            2 => {
                let mut v = vec![PT_HELLO; rng.gen_range(1..HELLO_LEN)];
                v[1..].iter_mut().for_each(|b| *b = 0);
                v
            }
            // Data packets for unknown connections.
            3 => {
                let mut v = vec![PT_DATA; 60];
                rng.fill(&mut v[1..]);
                v
            }
            _ => (0..rng.gen_range(0..200)).map(|_| rng.gen()).collect(),
        };
        match s.handle(i as u64, from, &dg) {
            Incoming::Reply(r) => assert!(r.len() <= dg.len(), "reply {} > request {}", r.len(), dg.len()),
            Incoming::AuthRequest(_) => panic!("forged auth accepted"),
            Incoming::Data { .. } => panic!("forged data accepted"),
            Incoming::Dropped(_) => {}
        }
    }
    let st = s.stats();
    assert!(st.preauth_bytes_out <= st.preauth_bytes_in, "{st:?}");
    assert!(st.preauth_out > 0);
    assert_eq!((s.conn_count(), s.preauth_state()), (0, 0));
}

#[test]
fn rejected_client_gets_an_authenticated_reason() {
    let mut w = World::new(4, Link::perfect(), Link::perfect(), ccfg());
    w.policy = Box::new(|p| {
        assert_eq!(p.nick(), "Willie");
        Err((reject_code::BANNED, "banned: griefing".into()))
    });
    assert!(w.run_until(2_000, |w| !w.client_ev.is_empty()));
    assert_eq!(
        w.client_ev[0],
        ClientEvent::Rejected {
            code: reject_code::BANNED,
            text: "banned: griefing".into(),
            authenticated: true
        }
    );
    assert_eq!(w.server.conn_count(), 0);
    assert!(w.replies_ok);
    // A retransmitted Auth gets the same (cached) reject, a bounded number of times.
    let auth = w.up_log.iter().find(|d| d[0] == PT_AUTH).unwrap().clone();
    let mut replies = 0;
    for _ in 0..10 {
        if let Incoming::Reply(r) = w.server.handle(w.now, w.caddr, &auth) {
            assert!(r.len() <= auth.len());
            replies += 1;
        }
    }
    assert_eq!(replies, 3);
}

#[test]
fn version_mismatch_is_reported_before_the_cookie() {
    let mut c = ccfg();
    c.version_min = 3;
    c.version_max = 4;
    let mut w = World::new(5, Link::perfect(), Link::perfect(), c);
    assert!(w.run_until(2_000, |w| !w.client_ev.is_empty()));
    match &w.client_ev[0] {
        ClientEvent::Rejected { code, text, authenticated } => {
            assert_eq!(*code, reject_code::VERSION);
            assert!(!authenticated);
            assert!(text.starts_with("Server runs HalfSword-MP ") && text.contains("OUTDATED"), "{text}");
        }
        e => panic!("unexpected {e:?}"),
    }
    assert!(w.replies_ok);
    // A newer client with an overlapping range connects at this build's version.
    let mut c = ccfg();
    c.version_max = 8;
    let mut w = World::new(6, Link::perfect(), Link::perfect(), c);
    assert!(w.run_until(2_000, |w| w.connected()));
    assert!(matches!(w.client_ev[0], ClientEvent::Connected { version: crate::net::PROTOCOL_VERSION, .. }));
}

#[test]
fn pinned_key_mismatch_fails_closed() {
    let mut c = ccfg();
    c.pinned_server_key = Some([0xEE; 32]);
    let mut w = World::new(8, Link::perfect(), Link::perfect(), c);
    assert!(w.run_until(2_000, |w| !w.client_ev.is_empty()));
    assert!(matches!(w.client_ev[0], ClientEvent::Failed(_)));
    assert_eq!(w.server.conn_count(), 0);
}

#[test]
fn server_close_reaches_the_client_and_is_reaped() {
    let mut w = World::new(9, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    w.server.close(w.now, cid, close_code::SERVER_CLOSING, "host closed the server");
    assert!(w.run_until(1_000, |w| w.client_ev.iter().any(|e| matches!(e, ClientEvent::Closed { .. }))));
    assert!(w.client_ev.contains(&ClientEvent::Closed {
        code: close_code::SERVER_CLOSING,
        by_peer: true
    }));
    assert!(w.run_until(1_000, |w| w.server.conn(cid).is_none_or(|c| c.is_closed())));
    let reaped = w.server.reap_closed();
    assert_eq!(reaped.len(), 1);
    assert_eq!(w.server.conn_count(), 0);
}

#[test]
fn silent_peer_times_out() {
    let mut w = World::new(10, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    // Cut the uplink entirely.
    w.up.loss = 1.0;
    assert!(w.run_until(15_000, |w| w.server.conn_ids().iter().all(|c| w
        .server
        .conn(*c)
        .unwrap()
        .is_closed())));
    assert_eq!(w.server.reap_closed().len(), 1);
}

/// A NAT rebind (the client's source address changes
/// mid-session) recovers in well under 500 ms on the same connection: no
/// re-handshake, same conn id, data flows both ways.
#[test]
fn nat_rebind_migrates_the_connection_within_500_ms() {
    let up = Link::new(31, 0.0, 0.0, 40, 10);
    let down = Link::new(32, 0.0, 0.0, 40, 10);
    let mut w = World::new(31, up, down, ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    // A 30 Hz stream both ways.
    let mut k = 0u32;
    let mut next = w.now;
    let tick = |w: &mut World, k: &mut u32, next: &mut u64| {
        if w.now >= *next {
            *k += 1;
            w.client.send(SendMode::Latest { key: 1 }, k.to_le_bytes().to_vec()).unwrap();
            w.server.send(cid, SendMode::Latest { key: 2 }, k.to_le_bytes().to_vec()).unwrap();
            *next = w.now + 33;
        }
        w.step(2);
    };
    for _ in 0..250 {
        tick(&mut w, &mut k, &mut next);
    }
    let old = w.caddr;
    // The NAT rebinds: same client socket, new public port / address.
    w.caddr = "198.51.100.200:61000".parse().unwrap();
    let t_rebind = w.now;
    let (srv_before, cli_before) = (w.server_in.len(), w.client_in.len());
    let k_rebind = k;
    let mut srv_ok = None;
    let mut cli_ok = None;
    while w.now < t_rebind + 2_000 && (srv_ok.is_none() || cli_ok.is_none()) {
        tick(&mut w, &mut k, &mut next);
        if srv_ok.is_none() && w.server_in.len() > srv_before {
            srv_ok = Some(w.now - t_rebind);
        }
        // A server message sent after the rebind reached the client.
        if cli_ok.is_none()
            && w.client_in[cli_before..].iter().any(|d| d.channel == CH_UNRELIABLE && u32::from_le_bytes(d.data[..4].try_into().unwrap()) > k_rebind)
        {
            cli_ok = Some(w.now - t_rebind);
        }
    }
    let (srv_ok, cli_ok) = (srv_ok.expect("server heard the client"), cli_ok.expect("client heard the server"));
    assert!(srv_ok < 500 && cli_ok < 500, "recovery took {srv_ok} / {cli_ok} ms");
    assert_eq!(w.migrations.len(), 1, "one validated migration");
    assert_eq!(w.migrations[0].1, old);
    assert_eq!(w.server.conn_count(), 1);
    assert_eq!(w.server.addr_of(cid), Some(w.caddr), "same connection, new address");
    assert_eq!(w.client_ev.iter().filter(|e| matches!(e, ClientEvent::Connected { .. })).count(), 1, "no re-handshake");
    assert!(w.replies_ok, "probes never larger than the packet that caused them");
}

/// A captured packet re-sent from another address (spoofed) never moves the
/// connection: it is a replay, and a race winner only earns a probe that
/// the spoofed address cannot ack.
#[test]
fn a_spoofed_source_cannot_steal_the_path() {
    let mut w = World::new(33, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    // Capture a fresh client packet before it is delivered, and race it in
    // from the attacker's address.
    w.client.send(SendMode::Reliable, b"x".to_vec()).unwrap();
    let dg = w.client.poll_transmit(w.now).unwrap();
    let attacker: SocketAddr = "192.0.2.66:7777".parse().unwrap();
    match w.server.handle(w.now, attacker, &dg) {
        Incoming::Data { migrated_from, probe, .. } => {
            assert!(migrated_from.is_none());
            assert!(probe.is_some_and(|p| p.len() <= dg.len()));
        }
        _ => panic!("authentic packet should be delivered"),
    }
    assert_eq!(w.server.addr_of(cid), Some(w.caddr), "not moved without a probe ack");
    // The real client keeps talking from its address: the candidate is dropped.
    w.client.send(SendMode::Reliable, b"y".to_vec()).unwrap();
    assert!(w.run_until(500, |w| !w.server_in.is_empty()));
    assert_eq!(w.server.addr_of(cid), Some(w.caddr));
    assert!(w.migrations.is_empty());
}

/// After a server restart (same identity key) the client's next packet
/// draws a verified stateless reset and the client closes with RESET at
/// once (it re-handshakes), instead of waiting out its idle timeout. A
/// forged reset is ignored.
#[test]
fn server_restart_is_answered_with_a_verified_reset() {
    let mut w = World::new(34, Link::new(1, 0.0, 0.0, 30, 0), Link::new(2, 0.0, 0.0, 30, 0), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    assert!(w.run_until(1_000, |w| w.server.conn(cid).unwrap().reset_token_delivered()));
    assert!(w.client.conn().unwrap().reset_token().is_some());
    // A forged reset (right conn id, wrong token) does nothing.
    let mut forged = super::endpoint::reset_datagram(cid, &[0xAB; 16]);
    w.client.handle(w.now, &forged);
    forged[1] ^= 1;
    w.client.handle(w.now, &forged);
    assert!(w.client.is_connected());
    // The server restarts: same static key, no connections.
    w.server = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), w.now, Some(99));
    let t0 = w.now;
    w.client.send(SendMode::Reliable, b"hello?".to_vec()).unwrap();
    assert!(w.run_until(1_000, |w| w.closed_with(close_code::RESET)), "{:?}", w.client_ev);
    assert!(w.now - t0 < 200, "reset after {} ms", w.now - t0);
    assert!(w.replies_ok);
    // A server with another identity cannot reset this client.
    let mut w = World::new(35, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    assert!(w.run_until(500, |w| w.client.conn().is_some_and(|c| c.reset_token().is_some())));
    w.server = ServerEndpoint::new(ServerConfig::new([7; 32]), ConnConfig::default(), w.now, Some(98));
    w.client.send(SendMode::Reliable, b"hello?".to_vec()).unwrap();
    w.run_until(500, |_| false);
    assert!(!w.closed_with(close_code::RESET));
}

/// With path-dead detection on (clients: 2 s), a dead server is noticed
/// in about 2 s instead of the 10 s idle timeout, and an honest lossy link
/// does not trip it.
#[test]
fn client_notices_a_dead_path_in_two_seconds_without_false_alarms() {
    let conn = ConnConfig { dead_after_ms: super::conn::CLIENT_DEAD_AFTER_MS, ..ConnConfig::default() };
    // 10 % loss both ways, idle lobby (keepalives only) for 3 minutes.
    let mut w = World::with_client_conn(36, Link::new(3, 0.10, 0.0, 60, 30), Link::new(4, 0.10, 0.0, 60, 30), conn.clone());
    assert!(w.run_until(5_000, |w| w.connected()));
    assert!(!w.run_until(180_000, |w| w.closed_with(close_code::TIMEOUT)), "false path-dead under 10 % loss");
    // The server vanishes (no reset either: the downlink is cut).
    let mut w = World::with_client_conn(37, Link::new(5, 0.0, 0.0, 40, 0), Link::new(6, 0.0, 0.0, 40, 0), conn);
    assert!(w.run_until(2_000, |w| w.connected()));
    w.run_until(3_000, |_| false);
    w.down.loss = 1.0;
    let t0 = w.now;
    assert!(w.run_until(5_000, |w| w.closed_with(close_code::TIMEOUT)));
    let took = w.now - t0;
    // The server handed out a stateless-reset token, so path-dead waits
    // DEAD_AFTER_WITH_RESET_MS (honest Wi-Fi stalls of a few
    // seconds keep the connection; a restart is caught by the reset).
    let d = super::conn::DEAD_AFTER_WITH_RESET_MS;
    assert!((d - 1_000..d + 600).contains(&took), "path-dead after {took} ms ({d} ms after the last packet heard)");
}

#[path = "tests_transport.rs"]
mod transport;

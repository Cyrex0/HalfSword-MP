//! Transport regressions: RTO and loss detection, path migration, pacing
//! and congestion control, fragment reassembly limits, local stalls vs dead
//! paths, handshake/reset budgets and keyed ReliableLatest delivery.
//! Child of `tests.rs`: reuses its `World` harness.

use super::*;
use crate::net::conn::{parse_header, Conn, Side};
use crate::net::crypto;

/// (server lost, server retransmits, server packets sent, client lost,
/// client packets sent) for a clean link of `rtt` ms: one reliable message
/// every 100 ms from the server for `secs` seconds, plus optional 30 Hz
/// Latest streams both ways.
fn clean_link_run(seed: u64, rtt: u64, secs: u64, streams: bool) -> (u64, u64, u64, u64, u64) {
    let d = rtt / 2;
    let mut w = World::new(seed, Link::new(seed, 0.0, 0.0, d, 0), Link::new(seed + 1, 0.0, 0.0, d, 0), ccfg());
    assert!(w.run_until(3_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    w.run_until(1_000, |_| false);
    let s0 = w.server.conn(cid).unwrap().stats();
    let c0 = w.client.conn().unwrap().stats();
    let end = w.now + secs * 1_000;
    let (mut next_rel, mut next_stream, mut k) = (w.now, w.now, 0u32);
    while w.now < end {
        if w.now >= next_rel {
            w.server.send(cid, SendMode::Reliable, k.to_le_bytes().to_vec()).unwrap();
            next_rel += 100;
        }
        if streams && w.now >= next_stream {
            w.server.send(cid, SendMode::Latest { key: 2 }, vec![7; 300]).unwrap();
            w.client.send(SendMode::Latest { key: 1 }, vec![9; 300]).unwrap();
            next_stream += 33;
        }
        k += 1;
        w.step(2);
    }
    w.run_until(1_000, |_| false);
    let s = w.server.conn(cid).unwrap().stats();
    let c = w.client.conn().unwrap().stats();
    (
        s.pkts_lost - s0.pkts_lost,
        s.retransmits - s0.retransmits,
        s.pkts_sent - s0.pkts_sent,
        c.pkts_lost - c0.pkts_lost,
        c.pkts_sent - c0.pkts_sent,
    )
}

/// Since the ack delay is subtracted from RTT samples, the RTO must
/// add it back, or a clean link counts ~50 % of packets lost and
/// retransmits most reliable messages (293/300 at 40 ms).
#[test]
fn clean_link_has_no_spurious_losses_or_retransmits() {
    for (i, rtt) in [40u64, 80, 150].into_iter().enumerate() {
        let (lost, retx, sent, _, _) = clean_link_run(500 + i as u64, rtt, 30, false);
        println!("rtt {rtt} ms: server lost {lost}/{sent} packets, retransmits {retx}/300");
        assert!(retx <= 3, "rtt {rtt}: {retx} spurious retransmits");
        assert!(lost * 100 <= sent, "rtt {rtt}: {lost}/{sent} packets declared lost");
    }
    // 30 Hz streams in both directions as well.
    let (lost, retx, sent, clost, csent) = clean_link_run(510, 80, 30, true);
    println!("streams: server lost {lost}/{sent}, retransmits {retx}; client lost {clost}/{csent}");
    assert!(retx <= 3 && lost * 100 <= sent && clost * 100 <= csent, "{lost}/{sent} {retx} {clost}/{csent}");
}

/// Reordering by jitter widens the loss thresholds instead of
/// retransmitting spuriously forever; reliable data still gets through.
#[test]
fn jittery_link_adapts_reorder_threshold() {
    let up = Link::new(61, 0.0, 0.0, 40, 60);
    let down = Link::new(62, 0.0, 0.0, 40, 60);
    let mut w = World::new(61, up, down, ccfg());
    assert!(w.run_until(3_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    for k in 0..3_000u32 {
        if k % 5 == 0 {
            w.server.send(cid, SendMode::Reliable, k.to_le_bytes().to_vec()).unwrap();
            w.server.send(cid, SendMode::Latest { key: 2 }, vec![1; 200]).unwrap();
        }
        w.step(2);
    }
    w.run_until(3_000, |_| false);
    let got = w.client_in.iter().filter(|d| d.channel == CH_RELIABLE).count();
    let s = w.server.conn(cid).unwrap().stats();
    println!(
        "jitter 0..60 ms: lost {} spurious {} retransmits {} of {} sent",
        s.pkts_lost, s.spurious_lost, s.retransmits, s.pkts_sent
    );
    assert_eq!(got, 600, "reliable delivered {got}");
    assert!(s.spurious_lost * 10 <= s.pkts_sent, "{} spurious of {}", s.spurious_lost, s.pkts_sent);
}

/// An off-path observer races copies of the client's packets from
/// its own address and forwards the server's probes to the client. The real
/// client keeps talking from its address, so the path never moves.
#[test]
fn off_path_racer_cannot_steal_the_path() {
    let mut w = World::new(71, Link::new(71, 0.0, 0.0, 20, 0), Link::new(72, 0.0, 0.0, 20, 0), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    let real = w.caddr;
    let x: SocketAddr = "192.0.2.66:7777".parse().unwrap();
    let mut migrated = false;
    let mut probes = 0;
    for k in 0..90u32 {
        w.client.send(SendMode::Latest { key: 1 }, k.to_le_bytes().to_vec()).unwrap();
        let now = w.now;
        while let Some(dg) = w.client.poll_transmit(now) {
            // The attacker's copy wins the race ...
            if let Incoming::Data { probe, migrated_from, .. } = w.server.handle(now, x, &dg) {
                migrated |= migrated_from.is_some();
                // ... and it forwards the server's probe to the real client.
                if let Some(p) = probe {
                    probes += 1;
                    w.client.handle(now, &p);
                }
            }
            // The genuine copy arrives right after (a duplicate).
            let _ = w.server.handle(now + 1, real, &dg);
        }
        for (addr, dg) in w.server.poll_transmit(now) {
            if addr == real {
                w.client.handle(now, &dg);
            }
        }
        w.now += 33;
    }
    assert!(probes > 0, "the attack did run");
    assert!(!migrated, "racing copies moved the connection");
    assert_eq!(w.server.addr_of(cid), Some(real));
}

/// The key holder itself forges packets from a spoofed source V
/// (and goes silent on its real path). It never sees the PATH_CHALLENGE sent
/// to V, so guessed responses never move the path.
#[test]
fn owner_forged_path_response_cannot_redirect_the_stream() {
    let mut w = World::new(73, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    let v: SocketAddr = "198.51.100.77:9999".parse().unwrap();
    let mut rng = StdRng::seed_from_u64(73);
    let mut now = w.now;
    for _ in 0..60 {
        now += 33;
        let mut p = vec![crate::net::channel::CK_PATH_RESPONSE];
        p.extend_from_slice(&rng.gen::<[u8; 8]>());
        let dg = w.client.conn_mut().unwrap().seal_raw(now, &p);
        if let Incoming::Data { migrated_from, .. } = w.server.handle(now, v, &dg) {
            assert!(migrated_from.is_none(), "migrated to a spoofed address");
        }
        // The server's probes to V never reach the owner.
        let _ = w.server.poll_transmit(now);
    }
    assert_eq!(w.server.addr_of(cid), Some(w.caddr));
}

/// Migrations are rate-limited (≥ 1 s apart): a second rebind right
/// after the first waits for the gap.
#[test]
fn migrations_are_rate_limited() {
    let mut w = World::new(75, Link::new(75, 0.0, 0.0, 20, 0), Link::new(76, 0.0, 0.0, 20, 0), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let stream = |w: &mut World, ms: u64| {
        let end = w.now + ms;
        while w.now < end {
            if w.now % 30 < 2 {
                w.client.send(SendMode::Latest { key: 1 }, vec![1]).unwrap();
            }
            w.step(2);
        }
    };
    stream(&mut w, 500);
    w.caddr = "198.51.100.200:61000".parse().unwrap();
    stream(&mut w, 600);
    assert_eq!(w.migrations.len(), 1);
    let t1 = w.migrations[0].0;
    w.caddr = "198.51.100.201:62000".parse().unwrap();
    stream(&mut w, 2_000);
    assert_eq!(w.migrations.len(), 2);
    assert!(w.migrations[1].0 - t1 >= 1_000, "second migration after {} ms", w.migrations[1].0 - t1);
}

/// A validated path onto an address another live connection uses
/// never displaces that connection.
#[test]
fn migration_never_displaces_another_live_connection() {
    let mut s = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), 0, Some(9));
    let mut rng = StdRng::seed_from_u64(9);
    let a: SocketAddr = "203.0.113.1:1000".parse().unwrap();
    let b: SocketAddr = "203.0.113.2:2000".parse().unwrap();
    let mut now = 1_000_000u64;
    let mut connect = |s: &mut ServerEndpoint, from: SocketAddr, seed: u8, now: u64| {
        let mut c = Client::new(ClientConfig::new([seed; 32], "p"), ConnConfig::default(), now, &mut rng);
        let hello = c.poll_transmit(now).unwrap();
        let Incoming::Reply(ch) = s.handle(now, from, &hello) else { panic!("no challenge") };
        c.handle(now, &ch);
        let auth = c.poll_transmit(now).unwrap();
        let Incoming::AuthRequest(p) = s.handle(now, from, &auth) else { panic!("no auth request") };
        let cid = s.accept(now, p);
        s.send(cid, SendMode::Ordered, b"w".to_vec()).unwrap();
        for (_, dg) in s.poll_transmit_one(now, cid) {
            c.handle(now, &dg);
        }
        assert!(c.is_connected());
        (c, cid)
    };
    let (_ca, cid_a) = connect(&mut s, a, 1, now);
    let (mut cb, cid_b) = connect(&mut s, b, 2, now);
    // B's packets now come from A's address, and B even answers the probes.
    for _ in 0..60 {
        now += 30;
        cb.send(SendMode::Latest { key: 1 }, vec![1]).unwrap();
        while let Some(dg) = cb.poll_transmit(now) {
            if let Incoming::Data { probe: Some(p), .. } = s.handle(now, a, &dg) {
                cb.handle(now, &p);
            }
        }
    }
    assert_eq!(s.addr_of(cid_a), Some(a));
    assert_eq!(s.addr_of(cid_b), Some(b), "B must not take A's address");
}

/// A 500 KB reliable burst leaves paced (not 500 datagrams in one
/// poll), and Latest frames sent meanwhile are not starved behind it.
#[test]
fn reliable_burst_is_paced_and_latest_keeps_flowing() {
    let mut w = World::new(81, Link::new(81, 0.0, 0.0, 20, 0), Link::new(82, 0.0, 0.0, 20, 0), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    for i in 0..450u32 {
        let mut m = vec![0u8; 1100];
        m[..4].copy_from_slice(&i.to_le_bytes());
        w.server.send(cid, SendMode::Reliable, m).unwrap();
    }
    let first = w.server.poll_transmit_one(w.now, cid);
    assert!(first.len() <= 16, "{} datagrams in one poll", first.len());
    for (_, dg) in first {
        w.down.push(w.now, dg);
    }
    let mut latest_sent: Vec<(u32, u64)> = Vec::new();
    let mut latency = Vec::new();
    let t0 = w.now;
    let mut k = 0u32;
    let reliable = |w: &World| w.client_in.iter().filter(|d| d.channel == CH_RELIABLE).count();
    while reliable(&w) < 450 && w.now < t0 + 10_000 {
        if (w.now - t0) % 34 < 2 {
            k += 1;
            w.server.send(cid, SendMode::Latest { key: 2 }, k.to_le_bytes().to_vec()).unwrap();
            latest_sent.push((k, w.now));
        }
        let before = w.client_in.len();
        w.step(2);
        for d in &w.client_in[before..] {
            if d.channel == CH_UNRELIABLE {
                let n = u32::from_le_bytes(d.data[..4].try_into().unwrap());
                if let Some((_, at)) = latest_sent.iter().find(|(x, _)| *x == n) {
                    latency.push(w.now - at);
                }
            }
        }
    }
    let took = w.now - t0;
    assert_eq!(reliable(&w), 450, "burst delivered");
    latency.sort_unstable();
    let p95 = latency[latency.len() * 95 / 100];
    println!(
        "500 KB burst took {took} ms; latest p95 {p95} ms over {}/{k} frames; rate {:.0} B/s",
        latency.len(),
        w.server.conn(cid).unwrap().stats().cc_rate_bps
    );
    assert!(p95 <= 40, "Latest delayed behind the backlog: p95 {p95} ms");
    assert!(latency.len() as u32 * 10 >= k * 9, "Latest frames lost: {}/{k}", latency.len());
}

/// AIMD — loss lowers the pacing rate (never below the floor),
/// pacing-limited clean acks raise it again.
#[test]
fn congestion_rate_decreases_on_loss_and_recovers() {
    let mut w = World::new(83, Link::new(83, 0.3, 0.0, 20, 0), Link::new(84, 0.3, 0.0, 20, 0), ccfg());
    assert!(w.run_until(8_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    let r0 = w.server.conn(cid).unwrap().stats().cc_rate_bps;
    for i in 0..400u32 {
        w.server.send(cid, SendMode::Reliable, vec![i as u8; 1000]).unwrap();
    }
    w.run_until(4_000, |_| false);
    let r1 = w.server.conn(cid).unwrap().stats().cc_rate_bps;
    assert!(r1 < r0, "rate {r0} -> {r1} under 30 % loss");
    assert!(r1 >= crate::net::conn::CC_FLOOR_BPS, "below the floor: {r1}");
    w.up.loss = 0.0;
    w.down.loss = 0.0;
    w.run_until(10_000, |_| false);
    for i in 0..900u32 {
        w.server.send(cid, SendMode::Reliable, vec![i as u8; 1000]).unwrap();
    }
    w.run_until(3_000, |_| false);
    let r2 = w.server.conn(cid).unwrap().stats().cc_rate_bps;
    assert!(r2 > r1, "rate did not recover: {r1} -> {r2}");
}

/// 100 two-fragment Ordered messages with every other datagram
/// dropped complete without a protocol violation (instead of closing with
/// Frag(TooManyPartials)).
#[test]
fn fragmented_burst_with_alternate_loss_completes() {
    let sec = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let mut c = Conn::from_handshake(Side::Client, &sec, 0, ConnConfig::default());
    let mut s = Conn::from_handshake(Side::Server, &sec, 0, ConnConfig::default());
    for i in 0..100u32 {
        let mut m = vec![(i % 251) as u8; 2000];
        m[..4].copy_from_slice(&i.to_le_bytes());
        s.send(SendMode::Ordered, m).unwrap();
    }
    let mut got = Vec::new();
    let mut n = 0u64;
    let mut t = 0u64;
    while got.len() < 100 && t < 60_000 {
        while let Some(dg) = s.poll_transmit(t) {
            n += 1;
            if n.is_multiple_of(2) {
                continue; // a policer drops every other datagram
            }
            for d in c.recv(t, &dg).unwrap_or_default() {
                got.push(u32::from_le_bytes(d.data[..4].try_into().unwrap()));
            }
        }
        while let Some(a) = c.poll_transmit(t) {
            let _ = s.recv(t, &a);
        }
        assert!(c.violation().is_none(), "receiver closed: {:?}", c.violation());
        t += 5;
    }
    assert_eq!(got, (0..100).collect::<Vec<_>>(), "all delivered in order (t = {t} ms)");
    assert!(c.is_open() && s.is_open());
}

/// A peer that starts more fragmented messages than the reassembly
/// table holds (an older sender) gets its packet deferred — not acked,
/// retransmitted later — instead of a protocol-violation close.
#[test]
fn reassembly_overflow_defers_instead_of_closing() {
    use crate::net::channel::write_frag;
    let sec = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let mut c = Conn::from_handshake(Side::Client, &sec, 0, ConnConfig::default());
    let mut s = Conn::from_handshake(Side::Server, &sec, 0, ConnConfig::default());
    for id in 0..64u32 {
        let mut p = Vec::new();
        write_frag(&mut p, CH_ORDERED, id, 0, 2, b"a");
        let dg = s.seal_raw(0, &p);
        c.recv(0, &dg).unwrap();
    }
    let mut p = Vec::new();
    write_frag(&mut p, CH_ORDERED, 64, 0, 2, b"a");
    let deferred = s.seal_raw(0, &p);
    assert_eq!(c.recv(0, &deferred).unwrap(), vec![]);
    assert!(c.is_open() && c.violation().is_none());
    // The deferred packet (#64) is not acked.
    let ack = c.poll_transmit(100).unwrap();
    let h = parse_header(&ack).unwrap();
    assert_eq!(h.ack, 63, "highest acked is the last accepted packet");
    // Completing one message frees a slot; the retransmission then fits.
    let mut p = Vec::new();
    write_frag(&mut p, CH_ORDERED, 0, 1, 2, b"b");
    assert_eq!(c.recv(1, &s.seal_raw(1, &p)).unwrap().len(), 1);
    let mut p = Vec::new();
    write_frag(&mut p, CH_ORDERED, 64, 0, 2, b"a");
    assert!(c.recv(2, &s.seal_raw(2, &p)).is_ok());
    assert!(c.is_open());
}

/// The client's process stalls 2.5 s (disk, AV, resume) while the
/// server's replies queue in the socket. The timer runs before the receive
/// path; the connection must not be declared dead. A real dead path is
/// still noticed in about 2 s (client without the reset capability).
#[test]
fn a_local_stall_is_not_a_dead_path() {
    let conn = ConnConfig { dead_after_ms: crate::net::conn::CLIENT_DEAD_AFTER_MS, ..ConnConfig::default() };
    let mut cc = ccfg();
    cc.caps &= !crate::net::caps::RESET; // the strict 2 s rule applies
    let mut w = World::new(91, Link::new(91, 0.0, 0.0, 20, 0), Link::new(92, 0.0, 0.0, 20, 0), ccfg());
    let mut rng = StdRng::seed_from_u64(91);
    w.client = Client::new(cc, conn, w.now, &mut rng);
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    w.run_until(500, |_| false);
    // 2.5 s in which only the server runs; its datagrams queue for the client.
    let mut queued = Vec::new();
    let end = w.now + 2_500;
    while w.now < end {
        w.server.send(cid, SendMode::Latest { key: 2 }, vec![1; 50]).unwrap();
        for (_, dg) in w.server.poll_transmit(w.now) {
            queued.push(dg);
        }
        w.now += 33;
    }
    // The timer runs first, then the socket is drained.
    let _ = w.client.poll_transmit(w.now);
    for dg in &queued {
        w.client.handle(w.now, dg);
    }
    while let Some(ev) = w.client.poll_event() {
        if !matches!(ev, ClientEvent::Message(_)) {
            w.client_ev.push(ev);
        }
    }
    assert!(!w.closed_with(close_code::TIMEOUT), "local stall declared the path dead");
    assert!(w.client.is_connected());
    assert_eq!(w.client.conn().unwrap().stats().local_stalls, 1);
    w.run_until(1_000, |_| false);
    w.down.loss = 1.0;
    let t0 = w.now;
    assert!(w.run_until(5_000, |w| w.closed_with(close_code::TIMEOUT)));
    assert!((1_000..2_600).contains(&(w.now - t0)), "dead after {} ms", w.now - t0);
}

/// With the server's reset token held, an honest 4 s Wi-Fi stall
/// (both directions silent) keeps the connection — no re-handshake.
#[test]
fn a_four_second_wifi_stall_keeps_the_connection() {
    let conn = ConnConfig { dead_after_ms: crate::net::conn::CLIENT_DEAD_AFTER_MS, ..ConnConfig::default() };
    let mut w = World::with_client_conn(93, Link::new(93, 0.0, 0.0, 30, 5), Link::new(94, 0.0, 0.0, 30, 5), conn);
    assert!(w.run_until(2_000, |w| w.connected()));
    assert!(w.run_until(2_000, |w| w.client.conn().is_some_and(|c| c.reset_token().is_some())));
    w.up.loss = 1.0;
    w.down.loss = 1.0;
    w.run_until(4_000, |_| false);
    w.up.loss = 0.0;
    w.down.loss = 0.0;
    w.run_until(2_000, |_| false);
    assert!(!w.closed_with(close_code::TIMEOUT));
    assert_eq!(w.client_ev.iter().filter(|e| matches!(e, ClientEvent::Connected { .. })).count(), 1);
}

/// Resends of an already answered Auth do not charge the source's
/// Auth budget, so a fresh join from the same address right after still
/// reaches admission.
#[test]
fn auth_resends_do_not_drain_the_source_budget() {
    let mut s = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), 0, Some(4));
    let mut rng = StdRng::seed_from_u64(4);
    let from: SocketAddr = "198.51.100.10:4000".parse().unwrap();
    let now = 1_000_000;
    let auth = auth_from(&mut s, now, from, 1, &mut rng).unwrap();
    let Incoming::AuthRequest(p) = s.handle(now, from, &auth) else { panic!("no auth request") };
    s.accept(now, p);
    for _ in 0..30 {
        assert!(matches!(s.handle(now, from, &auth), Incoming::Dropped(DropReason::DuplicateAuth)));
    }
    let auth2 = auth_from(&mut s, now, from, 2, &mut rng).unwrap();
    assert!(matches!(s.handle(now, from, &auth2), Incoming::AuthRequest(_)), "resends drained the budget");
}

/// Accepting a second PendingAuth for a live conn id never replaces
/// the connection (fresh keys at seq 0 would reuse nonces).
#[test]
fn accept_never_overwrites_a_live_connection() {
    let mut s = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), 0, Some(5));
    let mut rng = StdRng::seed_from_u64(5);
    let from: SocketAddr = "198.51.100.11:4000".parse().unwrap();
    let now = 1_000_000;
    let auth = auth_from(&mut s, now, from, 1, &mut rng).unwrap();
    let Incoming::AuthRequest(p1) = s.handle(now, from, &auth) else { panic!("no auth request") };
    let Incoming::AuthRequest(p2) = s.handle(now, from, &auth) else { panic!("second copy before accept") };
    let cid = s.accept(now, p1);
    s.send(cid, SendMode::Ordered, b"x".to_vec()).unwrap();
    let _ = s.poll_transmit_one(now, cid);
    let sent = s.conn(cid).unwrap().stats().pkts_sent;
    assert_eq!(s.accept(now, p2), cid);
    assert_eq!(s.conn(cid).unwrap().stats().pkts_sent, sent, "connection replaced");
}

/// Stateless resets are budgeted per source: one junk source gets a
/// handful, another source still gets its reset.
#[test]
fn stateless_resets_are_shared_per_source() {
    let mut s = ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), 0, Some(6));
    let junk: SocketAddr = "192.0.2.1:1".parse().unwrap();
    let honest: SocketAddr = "203.0.113.50:5000".parse().unwrap();
    let mut dg = vec![PT_DATA; 60];
    let mut resets = 0;
    for i in 0..200u32 {
        dg[1..5].copy_from_slice(&i.to_le_bytes());
        if matches!(s.handle(1_000, junk, &dg), Incoming::Reply(_)) {
            resets += 1;
        }
    }
    assert!(resets <= 5, "{resets} resets to one source");
    assert!(matches!(s.handle(1_000, honest, &dg), Incoming::Reply(_)), "honest source starved");
}

/// A probe never consumes a packet number when it would be larger
/// than the packet that caused it.
#[test]
fn oversize_probe_is_never_sealed() {
    let mut w = World::new(95, Link::perfect(), Link::perfect(), ccfg());
    assert!(w.run_until(2_000, |w| w.connected()));
    let cid = w.cid.unwrap();
    w.run_until(200, |_| false);
    let sent = w.server.conn(cid).unwrap().stats().pkts_sent;
    // The smallest authenticated packet: an empty (ack-only) payload.
    let tiny = w.client.conn_mut().unwrap().seal_raw(w.now, &[]);
    let other: SocketAddr = "198.51.100.5:1".parse().unwrap();
    match w.server.handle(w.now, other, &tiny) {
        Incoming::Data { probe, .. } => assert!(probe.is_none()),
        _ => panic!("authentic packet not delivered"),
    }
    assert_eq!(w.server.conn(cid).unwrap().stats().pkts_sent, sent, "a packet number was consumed");
}

/// A superseded ReliableLatest copy already in flight can no longer
/// be delivered after the newer one (without the capability: ["NEW", "OLD"]).
#[test]
fn reliable_latest_never_delivers_an_older_copy_last() {
    let sec = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let run = |caps: u64| {
        let mut c = Conn::from_handshake(Side::Client, &sec, 0, ConnConfig::default());
        let mut s = Conn::from_handshake(Side::Server, &sec, 0, ConnConfig::default());
        c.set_caps(caps);
        s.set_caps(caps);
        s.send(SendMode::ReliableLatest { key: 7 }, b"OLD".to_vec()).unwrap();
        let old = s.poll_transmit(0).unwrap();
        s.send(SendMode::ReliableLatest { key: 7 }, b"NEW".to_vec()).unwrap();
        let new = s.poll_transmit(1).unwrap();
        let mut got: Vec<Vec<u8>> = Vec::new();
        got.extend(c.recv(2, &new).unwrap().into_iter().map(|d| d.data));
        got.extend(c.recv(3, &old).unwrap().into_iter().map(|d| d.data));
        got
    };
    assert_eq!(run(0), vec![b"NEW".to_vec(), b"OLD".to_vec()], "without the capability (old behaviour)");
    assert_eq!(run(crate::net::caps::REL_KEY), vec![b"NEW".to_vec()]);
    // Keyed fragmented messages round-trip too.
    let mut c = Conn::from_handshake(Side::Client, &sec, 0, ConnConfig::default());
    let mut s = Conn::from_handshake(Side::Server, &sec, 0, ConnConfig::default());
    c.set_caps(crate::net::caps::REL_KEY);
    s.set_caps(crate::net::caps::REL_KEY);
    let big: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
    s.send(SendMode::ReliableLatest { key: 9 }, big.clone()).unwrap();
    let mut got = Vec::new();
    while let Some(dg) = s.poll_transmit(0) {
        assert!(dg.len() <= crate::net::MAX_DATAGRAM);
        got.extend(c.recv(0, &dg).unwrap());
    }
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, big);
}

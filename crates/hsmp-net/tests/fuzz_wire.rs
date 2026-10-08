//! Seeded mutation tests of every datagram decoder an unauthenticated sender reaches, on both
//! sides of the handshake. Deterministic (xorshift over real seeds) so CI runs the same inputs
//! every time; HSMP_FUZZ_ITERS raises the iteration count for a local soak.
//!
//! Invariants checked beyond "no panic":
//! * the server never answers unauthenticated input with more bytes than it received;
//! * a mutated Auth never reaches admission (`AuthRequest`), only the genuine one does;
//! * no mutated datagram allocates a connection or pre-auth state on the server;
//! * a client never reports `Connected` from a mutated server reply.

use std::net::SocketAddr;

use hsmp_net::net::{
    handshake, Client, ClientConfig, ClientEvent, ConnConfig, Incoming, SendMode, ServerConfig, ServerEndpoint,
};
use rand::rngs::StdRng;
use rand::SeedableRng;

fn iters(ci: usize) -> usize {
    std::env::var("HSMP_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(ci)
}

struct Xs(u64);

impl Xs {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Bit flips, byte sets, boundary values in length-looking fields, truncation, growth, splices.
fn mutate(r: &mut Xs, seed: &[u8], corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut d = seed.to_vec();
    for _ in 0..1 + r.below(6) {
        match r.below(8) {
            0 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] ^= 1 << r.below(8);
            }
            1 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] = r.next() as u8;
            }
            2 if d.len() >= 2 => {
                let i = r.below(d.len() - 1);
                let v: u16 = [0, 1, 0x7F, 0x80, 0xFF, 0x100, 0x7FFF, 0xFFFF][r.below(8)];
                d[i..i + 2].copy_from_slice(&v.to_le_bytes());
            }
            3 if !d.is_empty() => {
                let n = r.below(d.len());
                d.truncate(n);
            }
            4 if d.len() < 1500 => {
                let n = 1 + r.below(64);
                let at = r.below(d.len() + 1);
                let fill: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
                d.splice(at..at, fill);
            }
            5 if !corpus.is_empty() => {
                let o = &corpus[r.below(corpus.len())];
                let cut = r.below(d.len() + 1);
                let from = r.below(o.len() + 1);
                d.truncate(cut);
                d.extend_from_slice(&o[from..]);
            }
            6 if !d.is_empty() => {
                // keep the type byte, randomise one field-sized run
                let i = 1.min(d.len() - 1) + r.below(d.len().saturating_sub(1).max(1));
                let n = (1 + r.below(8)).min(d.len() - i.min(d.len()));
                for k in 0..n {
                    d[i + k] = r.next() as u8;
                }
            }
            _ => {
                let i = r.below(d.len() + 1);
                d.insert(i, r.next() as u8);
            }
        }
    }
    d
}

const NOW: u64 = 1_000_000;

fn caddr() -> SocketAddr {
    "203.0.113.9:40000".parse().unwrap()
}

fn server() -> ServerEndpoint {
    ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), NOW, Some(5))
}

/// A real handshake: (hello, challenge, auth, the server's first data packet, a client data packet,
/// a sealed AuthReject for another client, a PreReject).
struct Seeds {
    hello: Vec<u8>,
    challenge: Vec<u8>,
    auth: Vec<u8>,
    s_data: Vec<u8>,
    c_data: Vec<u8>,
    auth_reject: Vec<u8>,
    pre_reject: Vec<u8>,
}

fn seeds() -> Seeds {
    let mut rng = StdRng::seed_from_u64(11);
    let mut ep = server();
    let mut c = Client::new(ClientConfig::new([3; 32], "fuzz"), ConnConfig::default(), NOW, &mut rng);
    let hello = c.poll_transmit(NOW).unwrap();
    let challenge = match ep.handle(NOW, caddr(), &hello) {
        Incoming::Reply(r) => r,
        _ => panic!("no challenge"),
    };
    c.handle(NOW + 1, &challenge);
    let auth = c.poll_transmit(NOW + 1).unwrap();
    let p = match ep.handle(NOW + 2, caddr(), &auth) {
        Incoming::AuthRequest(p) => p,
        _ => panic!("no auth request"),
    };
    let cid = ep.accept(NOW + 2, p);
    ep.send(cid, SendMode::Ordered, b"\x10\x02\0\0\0\0\0\0welcome".to_vec()).unwrap();
    let s_data = ep.poll_transmit(NOW + 3).into_iter().next().unwrap().1;
    c.handle(NOW + 4, &s_data);
    assert!(c.is_connected());
    c.send(SendMode::Ordered, b"\x13\x02\0\0\0\0\0\0hello".to_vec()).unwrap();
    let c_data = c.poll_transmit(NOW + 5).unwrap();

    // another client, rejected after admission: a sealed AuthReject
    let mut c2 = Client::new(ClientConfig::new([4; 32], "rej"), ConnConfig::default(), NOW, &mut rng);
    let h2 = c2.poll_transmit(NOW).unwrap();
    let ch2 = match ep.handle(NOW, caddr(), &h2) {
        Incoming::Reply(r) => r,
        _ => panic!(),
    };
    c2.handle(NOW + 1, &ch2);
    let a2 = c2.poll_transmit(NOW + 1).unwrap();
    let auth_reject = match ep.handle(NOW + 2, caddr(), &a2) {
        Incoming::AuthRequest(p) => ep.reject(NOW + 2, p, handshake::reject_code::FULL, "server full"),
        _ => panic!(),
    };

    // a server that enforces another content hash: PreReject
    let mut cfg = ServerConfig::new([42; 32]);
    cfg.content_hash = Some([0xEE; 32]);
    let mut strict = ServerEndpoint::new(cfg, ConnConfig::default(), NOW, Some(6));
    let pre_reject = match strict.handle(NOW, caddr(), &hello) {
        Incoming::Reply(r) => r,
        _ => panic!("no pre-reject"),
    };
    Seeds { hello, challenge, auth, s_data, c_data, auth_reject, pre_reject }
}

/// Every mutated client datagram into a fresh server: bounded replies, no state, and only the
/// genuine Auth is admitted.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn server_preauth_decoders_survive_mutation() {
    let s = seeds();
    let corpus = vec![s.hello.clone(), s.auth.clone(), s.c_data.clone()];
    let mut r = Xs(0x5EC0_0001);
    let n = iters(3_000);
    let (mut bytes_in, mut bytes_out) = (0usize, 0usize);
    for i in 0..n {
        let seed = &corpus[i % corpus.len()];
        let d = mutate(&mut r, seed, &corpus);
        let mut ep = server();
        // replay the real Hello first so the Auth's cookie and ephemeral generation are current
        let _ = ep.handle(NOW, caddr(), &s.hello);
        bytes_in += d.len();
        match ep.handle(NOW + 2, caddr(), &d) {
            Incoming::Reply(rep) => {
                assert!(rep.len() <= d.len(), "amplification: {} B reply to {} B", rep.len(), d.len());
                bytes_out += rep.len();
            }
            Incoming::AuthRequest(_) => assert_eq!(d, s.auth, "a mutated Auth reached admission"),
            Incoming::Data { .. } => panic!("data delivered without a connection"),
            Incoming::Dropped(_) => {}
        }
        assert_eq!(ep.conn_count(), 0);
        assert_eq!(ep.preauth_state(), 0, "pre-auth state from unauthenticated input");
    }
    assert!(bytes_out <= bytes_in);
}

/// Mutated datagrams into an established connection: no panic, nothing delivered unless it is
/// the genuine packet, the connection survives garbage.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn server_conn_decoders_survive_mutation() {
    let s = seeds();
    let corpus = vec![s.c_data.clone(), s.auth.clone()];
    let mut r = Xs(0x5EC0_0002);
    let n = iters(2_000);
    for i in 0..n {
        let mut rng = StdRng::seed_from_u64(11);
        let mut ep = server();
        let mut c = Client::new(ClientConfig::new([3; 32], "fuzz"), ConnConfig::default(), NOW, &mut rng);
        let hello = c.poll_transmit(NOW).unwrap();
        let Incoming::Reply(ch) = ep.handle(NOW, caddr(), &hello) else { panic!() };
        c.handle(NOW + 1, &ch);
        let auth = c.poll_transmit(NOW + 1).unwrap();
        let Incoming::AuthRequest(p) = ep.handle(NOW + 2, caddr(), &auth) else { panic!() };
        ep.accept(NOW + 2, p);
        let d = mutate(&mut r, &corpus[i % corpus.len()], &corpus);
        let from = if i % 5 == 0 { "198.51.100.1:5000".parse().unwrap() } else { caddr() };
        match ep.handle(NOW + 6, from, &d) {
            Incoming::Reply(rep) => assert!(rep.len() <= d.len(), "amplification"),
            Incoming::Data { deliveries, probe, .. } => {
                if let Some(p) = probe {
                    assert!(p.len() <= d.len(), "probe larger than its trigger");
                }
                assert!(deliveries.is_empty() || d == s.c_data, "a mutated packet delivered messages");
            }
            Incoming::AuthRequest(_) => panic!("auth from a data packet"),
            Incoming::Dropped(_) => {}
        }
        assert!(ep.conn_count() <= 1);
    }
}

/// Mutated server replies into a client at each handshake stage.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn client_decoders_survive_mutation() {
    let s = seeds();
    let corpus = vec![s.challenge.clone(), s.pre_reject.clone(), s.auth_reject.clone(), s.s_data.clone()];
    let mut r = Xs(0x5EC0_0003);
    let n = iters(3_000);
    for i in 0..n {
        let seed = &corpus[i % corpus.len()];
        let d = mutate(&mut r, seed, &corpus);
        let mut rng = StdRng::seed_from_u64(11);
        let mut c = Client::new(ClientConfig::new([3; 32], "fuzz"), ConnConfig::default(), NOW, &mut rng);
        let _ = c.poll_transmit(NOW);
        // half the inputs arrive while the client waits for the welcome (after the real Challenge)
        if i % 2 == 1 {
            c.handle(NOW + 1, &s.challenge);
            let _ = c.poll_transmit(NOW + 1);
        }
        c.handle(NOW + 4, &d);
        let _ = c.poll_transmit(NOW + 5);
        while let Some(ev) = c.poll_event() {
            if let ClientEvent::Connected { .. } = ev {
                assert_eq!(d, s.s_data, "Connected from a mutated datagram");
            }
        }
    }
}

/// Every raw handshake parser over random and mutated bytes.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn handshake_parsers_survive_mutation() {
    let s = seeds();
    let corpus = vec![s.hello.clone(), s.challenge.clone(), s.auth.clone(), s.pre_reject.clone(), s.auth_reject.clone()];
    let mut r = Xs(0x5EC0_0004);
    for i in 0..iters(10_000) {
        let d = if i % 7 == 0 {
            (0..r.below(1400)).map(|_| r.next() as u8).collect()
        } else {
            mutate(&mut r, &corpus[i % corpus.len()], &corpus)
        };
        let _ = handshake::parse_hello(&d);
        let _ = handshake::parse_challenge(&d);
        if let Ok(p) = handshake::parse_pre_reject(&d) {
            assert!(p.text.len() <= 512);
        }
        let _ = handshake::parse_auth(&d);
        let _ = handshake::AuthPayload::decode(&d);
        if d.len() > 1 {
            let _ = handshake::AuthPayload::decode(&d[1..]);
        }
    }
}

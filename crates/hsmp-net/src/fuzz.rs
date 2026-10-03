//! Panic-free entry points for `cargo-fuzz` (a `fuzz/` crate's targets are
//! one-liners calling one of these):
//!
//! ```ignore
//! fuzz_target!(|data: &[u8]| hsmp_net::fuzz::server_datagram(data));
//! ```
//!
//! Every function must return normally for any input. The unit test at the
//! bottom runs each one over random and mutated inputs as a smoke check.

use std::net::SocketAddr;

use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::net::conn::{Conn, ConnConfig, Side};
use crate::net::crypto;
use crate::net::endpoint::{Client, ClientConfig, Incoming, ServerConfig, ServerEndpoint};
use crate::net::frag::Reassembler;
use crate::net::handshake;
use crate::net::replay::{expand_seq, ReplayWindow};

fn addr() -> SocketAddr {
    "192.0.2.1:7777".parse().expect("static address")
}

/// Target `datagram`: arbitrary bytes into a fresh server endpoint
/// (classification, Hello, Auth and data paths). Any reply must be no larger
/// than the input.
pub fn server_datagram(data: &[u8]) {
    let mut ep = ServerEndpoint::new(ServerConfig::new([7; 32]), ConnConfig::default(), 1_000, Some(1));
    if let Incoming::Reply(r) = ep.handle(1_000, addr(), data) {
        assert!(r.len() <= data.len(), "amplification");
    }
    assert_eq!(ep.conn_count(), 0);
}

/// Target `handshake`: arbitrary bytes as the server's answer to a client
/// that just sent its Hello (Challenge, PreReject, AuthReject, Data paths).
pub fn client_handshake(data: &[u8]) {
    let mut rng = StdRng::seed_from_u64(2);
    let mut c = Client::new(ClientConfig::new([3; 32], "fuzz"), ConnConfig::default(), 0, &mut rng);
    let _ = c.poll_transmit(0);
    c.handle(1, data);
    let _ = c.poll_transmit(2);
    while c.poll_event().is_some() {}
    // Also the raw parsers.
    let _ = handshake::parse_hello(data);
    let _ = handshake::parse_challenge(data);
    let _ = handshake::parse_pre_reject(data);
    let _ = handshake::parse_auth(data);
    let _ = handshake::AuthPayload::decode(data);
}

/// Target `conn_payload`: arbitrary bytes as an authenticated payload
/// (bypasses the AEAD so the chunk parser, channels and reassembly are
/// reached). The first byte selects how many packets the input is split
/// into, exercising state across packets.
pub fn conn_payload(data: &[u8]) {
    let s = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let mut tx = Conn::from_handshake(Side::Client, &s, 0, ConnConfig::default());
    let mut rx = Conn::from_handshake(Side::Server, &s, 0, ConnConfig::default());
    let (parts, body) = match data.split_first() {
        Some((n, rest)) => ((*n as usize % 8) + 1, rest),
        None => (1, data),
    };
    let step = (body.len() / parts).max(1);
    for (i, chunk) in body.chunks(step).enumerate() {
        let dg = tx.seal_raw(i as u64, chunk);
        let _ = rx.recv(i as u64, &dg);
        let _ = rx.poll_transmit(i as u64);
    }
}

/// Target `datagram_conn`: arbitrary bytes straight into an established
/// connection (header parsing, replay window, AEAD rejection).
pub fn conn_datagram(data: &[u8]) {
    let s = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
    let mut rx = Conn::from_handshake(Side::Server, &s, 0, ConnConfig::default());
    let _ = rx.recv(0, data);
}

/// Target `reassembly`: the input is a sequence of 8-byte fragment records
/// `ch u8 | id u16 | idx u8 | count u8 | len u16 | fill u8`.
pub fn reassembly(data: &[u8]) {
    let mut r = Reassembler::new();
    for (i, rec) in data.chunks(8).filter(|c| c.len() == 8).enumerate() {
        let id = u16::from_le_bytes([rec[1], rec[2]]) as u32;
        let len = u16::from_le_bytes([rec[5], rec[6]]) as usize % 4096;
        let payload = vec![rec[7]; len];
        if let Ok(Some(msg)) = r.insert(i as u64, rec[0], id, rec[3], rec[4], &payload) {
            assert!(msg.len() <= crate::net::frag::MAX_MESSAGE);
        }
        assert!(r.bytes() <= crate::net::frag::MAX_REASM_BYTES);
        if i % 64 == 63 {
            r.expire(i as u64 * 1000);
        }
    }
}

/// Target `replay`: the input is a sequence of u32 wire sequence numbers.
pub fn replay(data: &[u8]) {
    let mut w = ReplayWindow::new();
    for b in data.chunks(4).filter(|c| c.len() == 4) {
        let wire = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let full = expand_seq(wire, w.expected());
        let fresh = w.mark(full);
        assert!(!w.mark(full) || !fresh, "double accept");
        let _ = w.ack_fields();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn entry_points_survive_random_and_mutated_input() {
        let mut rng = StdRng::seed_from_u64(0xF022);
        // Seeds: a real Hello, a real Challenge, real data packets.
        let mut c = Client::new(ClientConfig::new([3; 32], "seed"), ConnConfig::default(), 0, &mut rng);
        let hello = c.poll_transmit(0).expect("hello");
        let mut ep = ServerEndpoint::new(ServerConfig::new([7; 32]), ConnConfig::default(), 1_000, Some(1));
        let challenge = match ep.handle(1_000, addr(), &hello) {
            Incoming::Reply(r) => r,
            _ => panic!("expected challenge"),
        };
        let seeds = [
            hello,
            challenge,
            // a v6 record message: WireHdr (kind 0x0207, aux 0, peer 0) + an 8-byte payload
            vec![0x07, 0x02, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        ];
        for i in 0..3_000 {
            let mut d: Vec<u8> = if i % 3 == 0 {
                (0..rng.gen_range(0..1400)).map(|_| rng.gen()).collect()
            } else {
                seeds[i % seeds.len()].clone()
            };
            for _ in 0..rng.gen_range(0..8) {
                if !d.is_empty() {
                    let j = rng.gen_range(0..d.len());
                    d[j] = rng.gen();
                }
            }
            server_datagram(&d);
            client_handshake(&d);
            conn_payload(&d);
            conn_datagram(&d);
            reassembly(&d);
            replay(&d);
        }
    }
}

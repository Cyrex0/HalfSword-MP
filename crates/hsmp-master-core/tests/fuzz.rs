//! Seeded mutation tests of everything the server list reads from the internet: the signed
//! register / heartbeat / delete bodies and their signatures, the punch request and listen
//! path, and the bug-report zip check. Deterministic; HSMP_FUZZ_ITERS raises the count.
//!
//! Invariants beyond "no panic": a mutated signed body or path is never accepted, the list
//! never holds more than one entry per address here, and the dashboard never echoes markup.

use hsmp_master_core::{auth, dashboard, punch, reports, Config, Registry};
use std::net::IpAddr;

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

fn mutate(r: &mut Xs, seed: &[u8]) -> Vec<u8> {
    const TOKENS: &[&[u8]] = &[b"\"", b"\\", b"{", b"}", b"[", b"]", b":", b",", b"null", b"-1", b"1e999", b"99999999999999999999",
        b"<script>", b"\\u0000", b"\xff", b" ", b"0", b"true"];
    let mut d = seed.to_vec();
    for _ in 0..1 + r.below(4) {
        match r.below(5) {
            0 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] ^= 1 << r.below(8);
            }
            1 if !d.is_empty() => {
                let n = r.below(d.len());
                d.truncate(n);
            }
            2 if !d.is_empty() => {
                let i = r.below(d.len());
                d.remove(i);
            }
            _ => {
                let i = r.below(d.len() + 1);
                let t = TOKENS[r.below(TOKENS.len())];
                d.splice(i..i, t.iter().copied());
            }
        }
    }
    d
}

const T0: u64 = 1_800_000_000_000;

fn body(port: u16, ts: u64, key: &str, name: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "name": name, "host": "198.51.100.200", "port": port, "mode": "Best of 3",
        "map": "Map_Arena_Pit", "players": 1, "max_players": 8, "proto_ver": 6,
        "proto_min": 6, "proto_max": 6, "server_key": "cd".repeat(32), "pwd_protected": false,
        "version": "0.2.0", "region": "EU", "nonce": "n", "hmac": "", "nat": "cone", "punch": true,
        "listing_key": key, "ts": ts,
    }))
    .unwrap()
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn signed_writes_reject_every_mutation() {
    let sk = auth::listing_key(&[5; 32]);
    let pk = auth::public_hex(&sk);
    let from: IpAddr = "198.51.100.200".parse().unwrap();
    let mut r = Xs(0x3A57_0001);
    let mut reg = Registry::new(Config::public(120, 360, false), vec![], vec![], T0);
    let good = body(7777, T0, &pk, "EU <i>Duels</i>");
    let sig = auth::sign(&sk, "POST", "/v1/register", &good);
    let (rep, _) = reg.register(T0, from, &good, Some(&sig));
    assert_eq!(rep.status, 201, "{}", rep.body);
    let id: serde_json::Value = serde_json::from_str(&rep.body).unwrap();
    let id = id["server_id"].as_str().unwrap().to_string();
    let hb = serde_json::to_vec(&serde_json::json!({"players": 2, "map": "Map_Arena_Yard", "ts": T0 + 1})).unwrap();
    let hb_path = format!("/v1/heartbeat/{id}");
    let hb_sig = auth::sign(&sk, "POST", &hb_path, &hb);
    let del = serde_json::to_vec(&serde_json::json!({"ts": T0 + 2})).unwrap();
    let del_sig = auth::sign(&sk, "DELETE", &format!("/v1/servers/{id}"), &del);
    let lpath = punch::listen_path(&id, T0 + 3);
    let lsig = auth::sign(&sk, "GET", &lpath, b"");

    for i in 0..iters(4_000) {
        // a fresh list every 64 inputs, so the listing never expires under the mutated writes
        if i % 64 == 0 {
            reg = Registry::new(Config::public(120, 360, false), vec![], vec![], T0);
            assert_eq!(reg.register(T0, from, &good, Some(&sig)).0.status, 201);
        }
        let now = T0 + 10_000 + (i % 64) as u64 * 3_000; // past every throttle, inside the TTL
        match i % 5 {
            0 => {
                let d = mutate(&mut r, &good);
                let (rep, _) = reg.register(now, from, &d, Some(&sig));
                assert!(rep.status != 201 || d == good, "mutated register accepted: {}", String::from_utf8_lossy(&d));
            }
            1 => {
                let d = mutate(&mut r, &hb);
                let (rep, _) = reg.heartbeat(now, from, &id, &d, Some(&hb_sig));
                assert!(rep.status != 204 || d == hb, "mutated heartbeat accepted");
            }
            2 => {
                let d = mutate(&mut r, &del);
                let (rep, _) = reg.delete(now, &id, &d, Some(&del_sig));
                assert!(rep.status != 204 || d == del, "mutated delete accepted");
            }
            3 => {
                let p = String::from_utf8_lossy(&mutate(&mut r, lpath.as_bytes())).into_owned();
                if reg.listen(now, &p, Some(&lsig)).is_ok() {
                    assert_eq!(p, lpath, "mutated listen path accepted");
                }
            }
            _ => {
                let s = String::from_utf8_lossy(&mutate(&mut r, sig.as_bytes())).into_owned();
                let (rep, _) = reg.register(now, from, &good, Some(&s));
                assert!(rep.status != 201 || s.trim() == sig, "mutated signature accepted");
            }
        }
        assert!(reg.len() <= 1);
    }
    let html = dashboard::render(&reg.list(T0 + 10_000));
    assert!(!html.contains("<i>") && html.contains("Duels"), "markup from a listing reached the dashboard");
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn punch_requests_survive_mutation() {
    let seed = serde_json::to_vec(&serde_json::json!({
        "host": "203.0.113.5", "port": 7777, "endpoint": "198.51.100.20:40000", "nonce": "ab12"
    }))
    .unwrap();
    let me: IpAddr = "198.51.100.20".parse().unwrap();
    let mut reg = Registry::new(Config::public(120, 360, false), vec![], vec![], T0);
    let mut r = Xs(0x3A57_0002);
    for i in 0..iters(10_000) {
        let d = mutate(&mut r, &seed);
        let (rep, order) = reg.punch(T0 + i as u64, me, &d);
        assert!(order.is_none(), "punch relayed to a server that is not listed");
        assert!(rep.status >= 400);
        if let Ok(req) = serde_json::from_slice::<punch::PunchReq>(&d) {
            if let Ok(ep) = punch::check_request(&req, me) {
                assert_eq!(ep.ip(), me, "a punch aimed at someone else");
                assert!(ep.port() >= punch::MIN_TARGET_PORT);
            }
        }
        let _ = punch::parse_endpoint(&String::from_utf8_lossy(&d));
        let _ = punch::parse_listen_path(&String::from_utf8_lossy(&d));
    }
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn report_check_survives_mutation() {
    let manifest = br#"{"magic":"hsmp-report","format":1,"launcher":"0.1.0","hsmp":"0.1.0","sessions":["a"],"kind":"client"}"#;
    let good = reports::fixture_zip(&[(reports::MANIFEST_NAME, manifest), ("logs/a.txt", b"hello")]);
    assert!(reports::check(&good).is_ok());
    let mut r = Xs(0x3A57_0003);
    for _ in 0..iters(20_000) {
        let mut d = good.clone();
        for _ in 0..1 + r.below(4) {
            if d.is_empty() {
                break;
            }
            match r.below(4) {
                0 => {
                    let i = r.below(d.len());
                    d[i] ^= 1 << r.below(8);
                }
                1 => {
                    let i = r.below(d.len());
                    d[i] = [0, 0xFF, 0x7F, 0x80][r.below(4)];
                }
                2 if d.len() > 4 => {
                    // a 32-bit field (size, offset) at an extreme
                    let i = r.below(d.len() - 3);
                    let v: u32 = [0, 1, 0xFFFF, 0x7FFF_FFFF, 0xFFFF_FFFF][r.below(5)];
                    d[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
                _ => {
                    let n = r.below(d.len());
                    d.truncate(n);
                }
            }
        }
        if let Ok(c) = reports::check(&d) {
            assert!(c.entries >= 1 && c.entries <= reports::MAX_ENTRIES);
            assert_eq!(c.head.magic, reports::MAGIC);
        }
    }
    for id in ["", "20261004-", "../../etc", "20261004-zzzz", "20261004-00000000000000000000000000000000/x"] {
        assert!(reports::object_key(id).is_none() || reports::is_report_id(id), "{id}");
    }
}

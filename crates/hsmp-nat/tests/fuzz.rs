//! Seeded mutation tests of every decoder that reads bytes from the network: STUN answers (any
//! internet host can send them to the game port), punch probes, NAT-PMP / PCP answers and the
//! UPnP SSDP / description / SOAP texts (any LAN device can send them). Deterministic; set
//! HSMP_FUZZ_ITERS for a longer local run.

use hsmp_nat::{natpmp, pcp, probe, stun, upnp};
use std::net::{Ipv4Addr, SocketAddr};

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

fn mutate(r: &mut Xs, seed: &[u8], corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut d = seed.to_vec();
    for _ in 0..1 + r.below(6) {
        match r.below(7) {
            0 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] ^= 1 << r.below(8);
            }
            1 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] = r.next() as u8;
            }
            2 if d.len() >= 2 => {
                // big-endian length fields (STUN, PCP) at extremes
                let i = r.below(d.len() - 1);
                let v: u16 = [0, 1, 3, 4, 0x7FFF, 0xFFFC, 0xFFFF][r.below(7)];
                d[i..i + 2].copy_from_slice(&v.to_be_bytes());
            }
            3 if !d.is_empty() => {
                let n = r.below(d.len());
                d.truncate(n);
            }
            4 if d.len() < 4096 => {
                let at = r.below(d.len() + 1);
                let n = 1 + r.below(32);
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
            _ => {
                let i = r.below(d.len() + 1);
                // XML / HTTP structure characters
                d.insert(i, b"<>/:\r\n \"&="[r.below(10)]);
            }
        }
    }
    d
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn stun_and_probe_decoders_survive_mutation() {
    let tx = [7u8; 12];
    let v4 = stun::binding_success(&tx, "203.0.113.5:40000".parse().unwrap());
    let v6 = stun::binding_success(&tx, "[2001:db8::5]:40000".parse().unwrap());
    let corpus = vec![v4, v6, stun::binding_request(&tx).to_vec(), probe::encode(9).to_vec()];
    let mut r = Xs(0x57C0_0001);
    for i in 0..iters(20_000) {
        let d = mutate(&mut r, &corpus[i % corpus.len()], &corpus);
        let parsed = stun::parse(&d);
        // only well-framed STUN parses, and never as a probe
        if parsed.is_some() {
            assert!(stun::is_stun(&d));
            assert!(!probe::is_probe(&d));
        }
        if let Some(stun::Parsed::Mapped { tx: t, .. }) = parsed {
            assert_eq!(&t[..], &d[8..20]);
        }
        if probe::is_probe(&d) {
            assert!(probe::nonce(&d).is_some());
        }
    }
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn natpmp_and_pcp_decoders_survive_mutation() {
    let req = pcp::map_request(Ipv4Addr::new(192, 168, 1, 20), &[1; 12], 7777, 7777, 3600);
    let corpus = vec![
        natpmp::encode_external_reply(1, Ipv4Addr::new(203, 0, 113, 4)).to_vec(),
        natpmp::encode_map_reply(1, 7777, 7777, 3600, 0).to_vec(),
        pcp::encode_reply(&req, 7777, Ipv4Addr::new(203, 0, 113, 4), 3600, 0).to_vec(),
        req.to_vec(),
    ];
    let mut r = Xs(0x57C0_0002);
    for i in 0..iters(20_000) {
        let d = mutate(&mut r, &corpus[i % corpus.len()], &corpus);
        let _ = natpmp::parse(&d);
        if let Some(pcp::Reply::Mapped { .. }) = pcp::parse(&d) {
            assert!(d.len() >= pcp::LEN);
        }
    }
}

const DESC: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><URLBase>http://192.168.1.1:5000/</URLBase>
<device><deviceList><device><serviceList><service>
<serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
<controlURL>/ctl/IPConn</controlURL></service><service>
<serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
<controlURL>ppp</controlURL></service></serviceList></device></deviceList></device></root>"#;

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn upnp_text_parsers_survive_mutation() {
    let ssdp = "HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nLOCATION: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
    let fault = "<s:Envelope><s:Body><s:Fault><detail><UPnPError><errorCode>718</errorCode></UPnPError></detail></s:Fault></s:Body></s:Envelope>";
    let corpus: Vec<Vec<u8>> = vec![ssdp.into(), DESC.into(), fault.into()];
    let from: SocketAddr = "192.168.1.1:1900".parse().unwrap();
    let mut r = Xs(0x57C0_0003);
    for i in 0..iters(8_000) {
        let d = mutate(&mut r, &corpus[i % corpus.len()], &corpus);
        let t = String::from_utf8_lossy(&d);
        if let Some(loc) = upnp::ssdp_location(&t) {
            assert!(loc.to_ascii_lowercase().starts_with("http://"));
            // whatever the answer says, only the device that answered is ever contacted
            if let Some(dev) = upnp::device_location(&t, from.ip()) {
                assert_eq!(upnp::url_host(&dev).as_deref(), Some("192.168.1.1"));
            }
        }
        let base = "http://192.168.1.1:5000/rootDesc.xml";
        if let Some(svc) = upnp::find_wan_service(&t, base) {
            assert_eq!(upnp::url_host(&svc.control_url).as_deref(), Some("192.168.1.1"), "control URL off the device: {}", svc.control_url);
        }
        let _ = upnp::soap_error(&t);
        let _ = upnp::element(&t, "NewExternalIPAddress");
        let _ = upnp::join_url(&t, "x");
        let _ = upnp::join_url(base, &t);
    }
}

/// The description parser stays linear enough on hostile input of the largest accepted size.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn upnp_description_parse_is_bounded_on_hostile_input() {
    let unit = "<service></x>";
    let hostile = unit.repeat(upnp::MAX_BODY / unit.len());
    let t = std::time::Instant::now();
    assert!(upnp::find_wan_service(&hostile, "http://192.168.1.1/").is_none());
    let ms = t.elapsed().as_millis();
    println!("hostile {} B description: {ms} ms", hostile.len());
    assert!(ms < 5_000, "description parse took {ms} ms");
}

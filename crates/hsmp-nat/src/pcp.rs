//! PCP (RFC 6887) MAP for UDP over IPv4: one 60-byte request to the gateway's port 5351,
//! one 60-byte answer. A NAT-PMP-only gateway answers a PCP request with a NAT-PMP packet
//! (version 0, result UNSUPP_VERSION), which [`parse`] reports as [`Reply::NatPmpOnly`].

use std::net::{Ipv4Addr, Ipv6Addr};

pub const PORT: u16 = 5351;
const VERSION: u8 = 2;
const OP_MAP: u8 = 1;
const PROTO_UDP: u8 = 17;
pub const LEN: usize = 60;

pub type Nonce = [u8; 12];

fn mapped(ip: Ipv4Addr) -> [u8; 16] {
    ip.to_ipv6_mapped().octets()
}

/// MAP request. `client` is this host's LAN address (the PCP client IP). A suggested
/// external address of `::ffff:0.0.0.0` means "any IPv4". Lifetime 0 deletes.
pub fn map_request(client: Ipv4Addr, nonce: &Nonce, internal: u16, suggested_external: u16, lifetime_s: u32) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[0] = VERSION;
    b[1] = OP_MAP;
    b[4..8].copy_from_slice(&lifetime_s.to_be_bytes());
    b[8..24].copy_from_slice(&mapped(client));
    b[24..36].copy_from_slice(nonce);
    b[36] = PROTO_UDP;
    b[40..42].copy_from_slice(&internal.to_be_bytes());
    b[42..44].copy_from_slice(&suggested_external.to_be_bytes());
    b[44..60].copy_from_slice(&mapped(Ipv4Addr::UNSPECIFIED));
    b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Mapped { nonce: Nonce, internal: u16, external: u16, external_ip: Ipv4Addr, lifetime_s: u32 },
    /// A non-zero result code (2 NOT_AUTHORIZED, 8 NO_RESOURCES, 11 CANNOT_PROVIDE_EXTERNAL ...).
    Error(u8),
    /// The gateway speaks NAT-PMP only.
    NatPmpOnly,
}

pub fn parse(d: &[u8]) -> Option<Reply> {
    if d.len() >= 4 && d[0] == 0 && d[1] & 0x80 != 0 {
        // NAT-PMP's reply to a version it does not know (result 1 = UNSUPP_VERSION)
        return (u16::from_be_bytes([d[2], d[3]]) == 1).then_some(Reply::NatPmpOnly);
    }
    if d.len() < LEN || d[0] != VERSION || d[1] != 0x80 | OP_MAP {
        return None;
    }
    if d[3] != 0 {
        return Some(Reply::Error(d[3]));
    }
    let lifetime_s = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&d[24..36]);
    if d[36] != PROTO_UDP {
        return None;
    }
    let mut a = [0u8; 16];
    a.copy_from_slice(&d[44..60]);
    let external_ip = Ipv6Addr::from(a).to_ipv4_mapped()?;
    Some(Reply::Mapped {
        nonce,
        internal: u16::from_be_bytes([d[40], d[41]]),
        external: u16::from_be_bytes([d[42], d[43]]),
        external_ip,
        lifetime_s,
    })
}

/// A PCP server's answer to `req` (tests and `hsmp-tools natlab`).
pub fn encode_reply(req: &[u8; LEN], external: u16, external_ip: Ipv4Addr, lifetime_s: u32, result: u8) -> [u8; LEN] {
    let mut b = *req;
    b[1] = 0x80 | OP_MAP;
    b[2] = 0;
    b[3] = result;
    b[4..8].copy_from_slice(&lifetime_s.to_be_bytes());
    b[8..12].copy_from_slice(&1u32.to_be_bytes()); // epoch
    b[12..24].fill(0);
    b[42..44].copy_from_slice(&external.to_be_bytes());
    b[44..60].copy_from_slice(&mapped(external_ip));
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_layout() {
        let n = [9u8; 12];
        let r = map_request(Ipv4Addr::new(192, 168, 1, 20), &n, 7777, 7777, 7200);
        assert_eq!(r[0..4], [2, 1, 0, 0]);
        assert_eq!(u32::from_be_bytes(r[4..8].try_into().unwrap()), 7200);
        assert_eq!(r[8..24], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 192, 168, 1, 20]);
        assert_eq!(r[24..36], n);
        assert_eq!(r[36..40], [17, 0, 0, 0]);
        assert_eq!(r[40..44], [0x1E, 0x61, 0x1E, 0x61]);
        assert_eq!(r[44..60], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0]);
    }

    #[test]
    fn replies_parse() {
        let n = [3u8; 12];
        let req = map_request(Ipv4Addr::new(10, 0, 0, 2), &n, 7777, 7777, 3600);
        let ip = Ipv4Addr::new(198, 51, 100, 1);
        assert_eq!(parse(&encode_reply(&req, 7780, ip, 1800, 0)),
                   Some(Reply::Mapped { nonce: n, internal: 7777, external: 7780, external_ip: ip, lifetime_s: 1800 }));
        assert_eq!(parse(&encode_reply(&req, 0, ip, 0, 8)), Some(Reply::Error(8)));
        // NAT-PMP's UNSUPP_VERSION answer
        assert_eq!(parse(&[0, 0x80, 0, 1, 0, 0, 0, 0]), Some(Reply::NatPmpOnly));
        assert_eq!(parse(&req), None, "a request is not a reply");
        assert_eq!(parse(&req[..30]), None);
    }
}

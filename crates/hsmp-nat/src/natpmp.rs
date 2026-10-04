//! NAT-PMP (RFC 6886): UDP to the gateway, port 5351. Two messages are used: "external
//! address" (op 0) and "map UDP" (op 1). A lifetime of 0 removes a mapping.

use std::net::Ipv4Addr;

pub const PORT: u16 = 5351;
const VERSION: u8 = 0;
const OP_EXTERNAL: u8 = 0;
const OP_MAP_UDP: u8 = 1;

pub fn external_request() -> [u8; 2] {
    [VERSION, OP_EXTERNAL]
}

pub fn map_request(internal: u16, suggested_external: u16, lifetime_s: u32) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[0] = VERSION;
    b[1] = OP_MAP_UDP;
    b[4..6].copy_from_slice(&internal.to_be_bytes());
    b[6..8].copy_from_slice(&suggested_external.to_be_bytes());
    b[8..12].copy_from_slice(&lifetime_s.to_be_bytes());
    b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    External { ip: Ipv4Addr },
    Mapped { internal: u16, external: u16, lifetime_s: u32 },
    /// A non-zero result code (2 not authorized, 3 network failure, 4 out of resources ...).
    Error(u16),
}

pub fn parse(d: &[u8]) -> Option<Reply> {
    if d.len() < 8 || d[0] != VERSION || d[1] & 0x80 == 0 {
        return None;
    }
    let result = u16::from_be_bytes([d[2], d[3]]);
    if result != 0 {
        return Some(Reply::Error(result));
    }
    match d[1] & 0x7F {
        OP_EXTERNAL if d.len() >= 12 => Some(Reply::External { ip: Ipv4Addr::new(d[8], d[9], d[10], d[11]) }),
        OP_MAP_UDP if d.len() >= 16 => Some(Reply::Mapped {
            internal: u16::from_be_bytes([d[8], d[9]]),
            external: u16::from_be_bytes([d[10], d[11]]),
            lifetime_s: u32::from_be_bytes([d[12], d[13], d[14], d[15]]),
        }),
        _ => None,
    }
}

/// Answers of a NAT-PMP server (tests and `hsmp-tools natlab`).
pub fn encode_external_reply(epoch: u32, ip: Ipv4Addr) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[1] = 0x80 | OP_EXTERNAL;
    b[4..8].copy_from_slice(&epoch.to_be_bytes());
    b[8..12].copy_from_slice(&ip.octets());
    b
}

pub fn encode_map_reply(epoch: u32, internal: u16, external: u16, lifetime_s: u32, result: u16) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[1] = 0x80 | OP_MAP_UDP;
    b[2..4].copy_from_slice(&result.to_be_bytes());
    b[4..8].copy_from_slice(&epoch.to_be_bytes());
    b[8..10].copy_from_slice(&internal.to_be_bytes());
    b[10..12].copy_from_slice(&external.to_be_bytes());
    b[12..16].copy_from_slice(&lifetime_s.to_be_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_rfc_layout() {
        assert_eq!(external_request(), [0, 0]);
        assert_eq!(map_request(7777, 7777, 3600), [0, 1, 0, 0, 0x1E, 0x61, 0x1E, 0x61, 0, 0, 0x0E, 0x10]);
        // delete: lifetime 0, suggested external 0
        assert_eq!(&map_request(7777, 0, 0)[6..], &[0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn replies_parse() {
        let ip = Ipv4Addr::new(203, 0, 113, 7);
        assert_eq!(parse(&encode_external_reply(5, ip)), Some(Reply::External { ip }));
        assert_eq!(parse(&encode_map_reply(5, 7777, 7778, 3600, 0)),
                   Some(Reply::Mapped { internal: 7777, external: 7778, lifetime_s: 3600 }));
        assert_eq!(parse(&encode_map_reply(5, 7777, 0, 0, 2)), Some(Reply::Error(2)));
        // a request echoed back, a PCP packet, a short packet
        assert_eq!(parse(&map_request(1, 1, 1)), None);
        assert_eq!(parse(&[2, 0x81, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(parse(&[0, 0x80, 0]), None);
    }
}

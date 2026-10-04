//! STUN Binding (RFC 5389 / 8489), just enough to learn the public UDP endpoint of a socket.
//!
//! A request is the bare 20-byte header (no attributes): every public STUN server answers it.
//! The answer's XOR-MAPPED-ADDRESS (or the legacy MAPPED-ADDRESS) is where the server saw the
//! request come from. The game sockets demultiplex STUN from game traffic with [`is_stun`]:
//! STUN starts with two zero bits and carries the magic cookie at bytes 4..8, game packets
//! start with 0xA1..0xB0 or 0xFF.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const MAGIC_COOKIE: u32 = 0x2112_A442;
pub const HEADER_LEN: usize = 20;
pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS: u16 = 0x0101;
pub const BINDING_ERROR: u16 = 0x0111;

const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// Pre-RFC 5389 servers used this code for XOR-MAPPED-ADDRESS.
const ATTR_XOR_MAPPED_ADDRESS_OLD: u16 = 0x8020;

pub type TxId = [u8; 12];

/// A Binding request with transaction id `tx`.
pub fn binding_request(tx: &TxId) -> [u8; HEADER_LEN] {
    let mut b = [0u8; HEADER_LEN];
    b[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // length 0: no attributes
    b[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    b[8..20].copy_from_slice(tx);
    b
}

/// A fresh random transaction id.
pub fn new_tx() -> TxId {
    rand::random()
}

/// Cheap test for the hot receive path: does this datagram look like STUN?
#[inline]
pub fn is_stun(d: &[u8]) -> bool {
    d.len() >= HEADER_LEN
        && d[0] & 0xC0 == 0
        && d[4..8] == MAGIC_COOKIE.to_be_bytes()
        && u16::from_be_bytes([d[2], d[3]]) as usize + HEADER_LEN == d.len()
        && d.len() % 4 == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    /// A Binding success: the address the server saw.
    Mapped { tx: TxId, addr: SocketAddr },
    /// A Binding error response (or a success without an address attribute).
    Failed { tx: TxId },
    /// A Binding request from someone else (we never answer those).
    Request { tx: TxId },
}

/// Parse a STUN message. `None` for anything malformed or not a Binding message.
pub fn parse(d: &[u8]) -> Option<Parsed> {
    if !is_stun(d) {
        return None;
    }
    let ty = u16::from_be_bytes([d[0], d[1]]);
    let mut tx = [0u8; 12];
    tx.copy_from_slice(&d[8..20]);
    match ty {
        BINDING_REQUEST => return Some(Parsed::Request { tx }),
        BINDING_ERROR => return Some(Parsed::Failed { tx }),
        BINDING_SUCCESS => {}
        _ => return None,
    }
    let mut xor = None;
    let mut plain = None;
    let mut i = HEADER_LEN;
    while i + 4 <= d.len() {
        let at = u16::from_be_bytes([d[i], d[i + 1]]);
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        let v = d.get(i + 4..i + 4 + len)?;
        match at {
            ATTR_XOR_MAPPED_ADDRESS | ATTR_XOR_MAPPED_ADDRESS_OLD => xor = xor.or_else(|| address(v, Some(&tx))),
            ATTR_MAPPED_ADDRESS => plain = plain.or_else(|| address(v, None)),
            _ => {}
        }
        i += 4 + len.div_ceil(4) * 4;
    }
    Some(match xor.or(plain) {
        Some(addr) => Parsed::Mapped { tx, addr },
        None => Parsed::Failed { tx },
    })
}

/// A (XOR-)MAPPED-ADDRESS value; `tx` set = XOR'd.
fn address(v: &[u8], tx: Option<&TxId>) -> Option<SocketAddr> {
    if v.len() < 4 {
        return None;
    }
    let cookie = MAGIC_COOKIE.to_be_bytes();
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    if tx.is_some() {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }
    let ip = match (v[1], v.len()) {
        (1, 8) => {
            let mut o = [v[4], v[5], v[6], v[7]];
            if tx.is_some() {
                for (b, k) in o.iter_mut().zip(cookie) {
                    *b ^= k;
                }
            }
            IpAddr::V4(Ipv4Addr::from(o))
        }
        (2, 20) => {
            let mut o = [0u8; 16];
            o.copy_from_slice(&v[4..20]);
            if let Some(tx) = tx {
                let key: Vec<u8> = cookie.iter().chain(tx.iter()).copied().collect();
                for (b, k) in o.iter_mut().zip(key) {
                    *b ^= k;
                }
            }
            IpAddr::V6(Ipv6Addr::from(o))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// A Binding success for `tx` that reports `addr` (XOR-MAPPED-ADDRESS). Used by the test STUN
/// server (`hsmp-tools natlab stun`) and the tests.
pub fn binding_success(tx: &TxId, addr: SocketAddr) -> Vec<u8> {
    let cookie = MAGIC_COOKIE.to_be_bytes();
    let mut val = vec![0u8, 0];
    let xport = addr.port() ^ (MAGIC_COOKIE >> 16) as u16;
    match addr.ip() {
        IpAddr::V4(ip) => {
            val[1] = 1;
            val.extend_from_slice(&xport.to_be_bytes());
            val.extend(ip.octets().iter().zip(cookie).map(|(b, k)| b ^ k));
        }
        IpAddr::V6(ip) => {
            val[1] = 2;
            val.extend_from_slice(&xport.to_be_bytes());
            let key: Vec<u8> = cookie.iter().chain(tx.iter()).copied().collect();
            val.extend(ip.octets().iter().zip(key).map(|(b, k)| b ^ k));
        }
    }
    let mut m = Vec::with_capacity(HEADER_LEN + 4 + val.len());
    m.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    m.extend_from_slice(&((4 + val.len()) as u16).to_be_bytes());
    m.extend_from_slice(&cookie);
    m.extend_from_slice(tx);
    m.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    m.extend_from_slice(&(val.len() as u16).to_be_bytes());
    m.extend_from_slice(&val);
    m
}

/// How the NAT maps one socket, from the answers of two STUN servers on different addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapping {
    /// The public address is a local address of this host: no NAT on the path.
    NoNat,
    /// The same public endpoint towards every destination (endpoint-independent mapping):
    /// hole punching works.
    EndpointIndependent,
    /// A different public port per destination (symmetric NAT): punching will not work, a
    /// forwarded port is needed.
    Symmetric,
    /// Only one server answered.
    Unknown,
}

impl Mapping {
    pub fn as_str(self) -> &'static str {
        match self {
            Mapping::NoNat => "open",
            Mapping::EndpointIndependent => "cone",
            Mapping::Symmetric => "symmetric",
            Mapping::Unknown => "unknown",
        }
    }
}

/// Classify from the mapped endpoints seen by different STUN servers (`local` = the socket's
/// own address towards the internet, when known).
pub fn classify(seen: &[SocketAddr], local: Option<SocketAddr>) -> Mapping {
    let Some(first) = seen.first() else { return Mapping::Unknown };
    if local.is_some_and(|l| l.ip() == first.ip() && l.port() == first.port()) {
        return Mapping::NoNat;
    }
    if seen.len() < 2 {
        return Mapping::Unknown;
    }
    if seen.iter().all(|s| s == first) {
        Mapping::EndpointIndependent
    } else {
        Mapping::Symmetric
    }
}

/// The public STUN servers tried, in order (free, no account; IPv4 and IPv6).
pub const DEFAULT_SERVERS: &[&str] = &[
    "stun.cloudflare.com:3478",
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.nextcloud.com:443",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_layout() {
        let tx = [7u8; 12];
        let r = binding_request(&tx);
        assert_eq!(&r[0..4], &[0, 1, 0, 0]);
        assert_eq!(&r[4..8], &[0x21, 0x12, 0xA4, 0x42]);
        assert_eq!(&r[8..], &tx);
        assert!(is_stun(&r));
        assert_eq!(parse(&r), Some(Parsed::Request { tx }));
    }

    #[test]
    fn xor_mapped_v4_and_v6_round_trip() {
        let tx = [0x5a; 12];
        for a in ["203.0.113.9:40123", "[2001:db8::7]:7777"] {
            let addr: SocketAddr = a.parse().unwrap();
            let m = binding_success(&tx, addr);
            assert!(is_stun(&m), "{a}");
            assert_eq!(parse(&m), Some(Parsed::Mapped { tx, addr }), "{a}");
        }
    }

    /// RFC 5769 2.2: a real IPv4 response (with SOFTWARE, MESSAGE-INTEGRITY and FINGERPRINT,
    /// which are skipped) maps to 192.0.2.1:32853.
    #[test]
    fn rfc5769_sample_response() {
        let hex = concat!(
            "0101003c", "2112a442", "b7e7a701", "bc34d686", "fa87dfae",
            "8022000b", "74657374", "20766563", "746f7220",
            "00200008", "0001a147", "e112a643",
            "00080014", "2b91f599", "fd9e90c3", "8c7489f9", "2af9ba53", "f06be7d7",
            "80280004", "c07d4c96",
        );
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let tx: TxId = b[8..20].try_into().unwrap();
        assert_eq!(parse(&b), Some(Parsed::Mapped { tx, addr: "192.0.2.1:32853".parse().unwrap() }));
    }

    #[test]
    fn legacy_mapped_address_is_a_fallback() {
        let tx = [1u8; 12];
        let mut m = vec![0x01, 0x01, 0, 12];
        m.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        m.extend_from_slice(&tx);
        m.extend_from_slice(&[0, 1, 0, 8, 0, 1, 0x1E, 0x61, 198, 51, 100, 4]);
        assert_eq!(parse(&m), Some(Parsed::Mapped { tx, addr: "198.51.100.4:7777".parse().unwrap() }));
    }

    #[test]
    fn game_packets_and_garbage_are_not_stun() {
        let mut hello = vec![0xA1u8; 1200];
        hello[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        assert!(!is_stun(&hello), "a transport packet");
        assert!(!is_stun(b"\xFFHSMPQ1\x00aaaaaaaaaaaaaaaaaaaaaaaa"), "a browser query");
        assert!(!is_stun(&[0u8; 19]));
        // the length field must match the datagram
        let mut r = binding_request(&[0; 12]).to_vec();
        r.extend_from_slice(&[0; 4]);
        assert!(!is_stun(&r));
        // an attribute running past the end is refused, not panicked on
        let mut m = binding_success(&[2; 12], "1.2.3.4:5".parse().unwrap());
        m[22] = 0xFF;
        assert_eq!(parse(&m), None);
        // an error response
        let mut e = binding_request(&[3; 12]);
        e[0..2].copy_from_slice(&BINDING_ERROR.to_be_bytes());
        assert_eq!(parse(&e), Some(Parsed::Failed { tx: [3; 12] }));
    }

    #[test]
    fn classification() {
        let a: SocketAddr = "203.0.113.5:7777".parse().unwrap();
        let b: SocketAddr = "203.0.113.5:40001".parse().unwrap();
        assert_eq!(classify(&[a, a], None), Mapping::EndpointIndependent);
        assert_eq!(classify(&[a, b], None), Mapping::Symmetric);
        assert_eq!(classify(&[a], None), Mapping::Unknown);
        assert_eq!(classify(&[a, b], Some(a)), Mapping::NoNat);
        assert_eq!(classify(&[], None), Mapping::Unknown);
    }
}

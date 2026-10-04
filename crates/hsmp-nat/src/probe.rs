//! The punch probe: a small datagram a host's server sends to a joiner's public endpoint so
//! the host's own NAT opens a pinhole for that endpoint (docs/development/protocol.md, "NAT
//! traversal"). It carries nothing but a magic and a nonce, is never answered, and is far
//! smaller than the HTTPS request that asked for it (no amplification).
//!
//!   probe = MAGIC (8) | nonce u64 LE (8)            16 bytes

pub const MAGIC: &[u8; 8] = b"\xFFHSMPP1\x00";
pub const LEN: usize = 16;

/// Probes per punch request, and when (ms after the request arrived). A few, spread out, so a
/// single lost datagram does not fail the punch.
pub const SCHEDULE_MS: [u64; 4] = [0, 150, 400, 1000];

pub fn encode(nonce: u64) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[..8].copy_from_slice(MAGIC);
    b[8..].copy_from_slice(&nonce.to_le_bytes());
    b
}

#[inline]
pub fn is_probe(d: &[u8]) -> bool {
    d.len() == LEN && &d[..8] == MAGIC
}

pub fn nonce(d: &[u8]) -> Option<u64> {
    is_probe(d).then(|| u64::from_le_bytes(d[8..16].try_into().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rejects() {
        let p = encode(0x1122_3344_5566_7788);
        assert!(is_probe(&p));
        assert_eq!(nonce(&p), Some(0x1122_3344_5566_7788));
        assert!(!is_probe(&p[..15]));
        assert!(!is_probe(b"\xFFHSMPQ1\x00\0\0\0\0\0\0\0\0"), "the browser query magic differs");
        let mut q = p;
        q[1] = b'X';
        assert_eq!(nonce(&q), None);
        assert!(!crate::stun::is_stun(&p), "never mistaken for STUN");
    }
}

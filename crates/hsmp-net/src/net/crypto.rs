//! Key schedule and AEAD.
//!
//! ```text
//! th   = SHA-256("HSMP/5 transcript\0" || u16 len(hello_core) || hello_core || challenge_core)
//! prk  = HKDF-Extract(salt = th, ikm = DH(c_eph, s_eph) || DH(c_eph, s_static))
//! auth = HKDF-Expand(prk, "hsmp5 auth", 32)     C2SAuth payload key
//! c2s  = HKDF-Expand(prk, "hsmp5 c2s", 32)      client -> server traffic secret
//! s2c  = HKDF-Expand(prk, "hsmp5 s2c", 32)      server -> client traffic secret
//! cid  = HKDF-Expand(prk, "hsmp5 cid", 8)       conn_id (LE u64, 0 -> 1)
//! res  = HKDF-Expand(prk, "hsmp5 resume", 32)   reserved for resume tickets (1A)
//! next = HKDF-Expand(secret, "hsmp5 key update", 32)   key update chain
//! ```
//! AEAD: ChaCha20-Poly1305, nonce = full 64-bit packet number LE || 4 zero bytes.

use chacha20poly1305::aead::{Aead, AeadInPlace, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

pub type Secret = [u8; 32];

/// Nonce reserved for the sealed `C2SAuth` payload (key `auth`).
pub const NONCE_AUTH: u64 = u64::MAX;
/// Nonce reserved for the sealed `S2CAuthReject` (key `s2c`); data packets
/// never reach this packet number.
pub const NONCE_AUTH_REJECT: u64 = u64::MAX - 1;

pub fn cipher(secret: &Secret) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(secret))
}

pub fn nonce(seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&seq.to_le_bytes());
    n
}

pub fn seal(c: &ChaCha20Poly1305, seq: u64, aad: &[u8], pt: &[u8]) -> Vec<u8> {
    c.encrypt(Nonce::from_slice(&nonce(seq)), Payload { msg: pt, aad })
        .expect("ChaCha20-Poly1305 encryption cannot fail for datagram-sized input")
}

pub fn open(c: &ChaCha20Poly1305, seq: u64, aad: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    c.decrypt(Nonce::from_slice(&nonce(seq)), Payload { msg: ct, aad }).ok()
}

/// `buf` holds the associated data (`buf[..aad_len]`) followed by the
/// plaintext: encrypt the plaintext in place and append the tag. Same bytes
/// as `aad ‖ seal(..)`, without a second buffer.
pub fn seal_in_place(c: &ChaCha20Poly1305, seq: u64, aad_len: usize, buf: &mut Vec<u8>) {
    let (aad, pt) = buf.split_at_mut(aad_len);
    let tag = c
        .encrypt_in_place_detached(Nonce::from_slice(&nonce(seq)), aad, pt)
        .expect("ChaCha20-Poly1305 encryption cannot fail for datagram-sized input");
    buf.extend_from_slice(&tag);
}

/// `open` into a caller-owned buffer (cleared first, its capacity reused).
/// On failure `out` is left empty.
pub fn open_into(c: &ChaCha20Poly1305, seq: u64, aad: &[u8], ct: &[u8], out: &mut Vec<u8>) -> bool {
    out.clear();
    let Some(n) = ct.len().checked_sub(super::TAG_LEN) else { return false };
    out.extend_from_slice(&ct[..n]);
    if c.decrypt_in_place_detached(Nonce::from_slice(&nonce(seq)), aad, out, Tag::from_slice(&ct[n..])).is_ok() {
        return true;
    }
    out.clear();
    false
}

#[derive(Clone)]
pub struct HandshakeSecrets {
    pub auth: Secret,
    pub c2s: Secret,
    pub s2c: Secret,
    pub resume: Secret,
    pub conn_id: u64,
}

impl std::fmt::Debug for HandshakeSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HandshakeSecrets {{ conn_id: {:016x}, .. }}", self.conn_id)
    }
}

pub fn transcript(hello_core: &[u8], challenge_core: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"HSMP/5 transcript\0");
    h.update((hello_core.len() as u16).to_le_bytes());
    h.update(hello_core);
    h.update(challenge_core);
    h.finalize().into()
}

pub fn derive(th: &[u8; 32], ee: &[u8; 32], es: &[u8; 32]) -> HandshakeSecrets {
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(ee);
    ikm[32..].copy_from_slice(es);
    let hk = Hkdf::<Sha256>::new(Some(th), &ikm);
    let exp = |label: &[u8]| {
        let mut out = [0u8; 32];
        hk.expand(label, &mut out).expect("32 bytes is a valid HKDF-SHA256 length");
        out
    };
    let mut cid = [0u8; 8];
    hk.expand(b"hsmp5 cid", &mut cid).expect("8 bytes is a valid HKDF-SHA256 length");
    let conn_id = u64::from_le_bytes(cid).max(1);
    HandshakeSecrets {
        auth: exp(b"hsmp5 auth"),
        c2s: exp(b"hsmp5 c2s"),
        s2c: exp(b"hsmp5 s2c"),
        resume: exp(b"hsmp5 resume"),
        conn_id,
    }
}

/// Stateless-reset key of a server: derived from its long-term static
/// secret, so a restarted server (same identity file) computes the same
/// tokens for the connections it forgot.
pub fn reset_key(static_secret: &[u8; 32]) -> Secret {
    let hk = Hkdf::<Sha256>::new(Some(b"HSMP/5 stateless reset"), static_secret);
    let mut out = [0u8; 32];
    hk.expand(b"hsmp5 reset key", &mut out).expect("valid length");
    out
}

/// Stateless-reset token of `conn_id` (16 bytes): HMAC-SHA256(reset_key, cid).
pub fn reset_token(reset_key: &Secret, conn_id: u64) -> [u8; 16] {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(reset_key).expect("any key length");
    mac.update(b"HSMP/5 reset\0");
    mac.update(&conn_id.to_le_bytes());
    let tag = mac.finalize().into_bytes();
    let mut t = [0u8; 16];
    t.copy_from_slice(&tag[..16]);
    t
}

/// Constant-time equality of two tokens.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Next traffic secret in the key-update chain.
pub fn next_secret(s: &Secret) -> Secret {
    let hk = Hkdf::<Sha256>::from_prk(s).expect("32-byte PRK is valid");
    let mut out = [0u8; 32];
    hk.expand(b"hsmp5 key update", &mut out).expect("valid length");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aead_round_trip_and_nonce_binding() {
        let c = cipher(&[7u8; 32]);
        let ct = seal(&c, 42, b"hdr", b"payload");
        assert_eq!(open(&c, 42, b"hdr", &ct).as_deref(), Some(&b"payload"[..]));
        assert!(open(&c, 43, b"hdr", &ct).is_none(), "wrong nonce must fail");
        assert!(open(&c, 42, b"hdX", &ct).is_none(), "wrong AAD must fail");
        let other = cipher(&next_secret(&[7u8; 32]));
        assert!(open(&other, 42, b"hdr", &ct).is_none(), "next key must differ");
    }

    #[test]
    fn in_place_variants_match_the_allocating_ones() {
        let c = cipher(&[7u8; 32]);
        let mut buf = b"hdrpayload".to_vec();
        seal_in_place(&c, 42, 3, &mut buf);
        let ct = seal(&c, 42, b"hdr", b"payload");
        assert_eq!(&buf[3..], &ct[..]);
        let mut out = Vec::new();
        assert!(open_into(&c, 42, b"hdr", &ct, &mut out));
        assert_eq!(out, b"payload");
        assert!(!open_into(&c, 43, b"hdr", &ct, &mut out));
        assert!(out.is_empty());
        assert!(!open_into(&c, 42, b"hdr", &ct[..10], &mut out));
        // Empty plaintext (an ack-only packet).
        let mut buf = b"hdr".to_vec();
        seal_in_place(&c, 7, 3, &mut buf);
        assert_eq!(&buf[3..], &seal(&c, 7, b"hdr", b"")[..]);
        assert!(open_into(&c, 7, b"hdr", &buf[3..], &mut out) && out.is_empty());
    }

    #[test]
    fn derivation_separates_directions() {
        let s = derive(&[1; 32], &[2; 32], &[3; 32]);
        assert_ne!(s.c2s, s.s2c);
        assert_ne!(s.auth, s.c2s);
        assert_ne!(s.conn_id, 0);
        let t = derive(&[9; 32], &[2; 32], &[3; 32]);
        assert_ne!(s.c2s, t.c2s, "transcript must bind the keys");
    }
}

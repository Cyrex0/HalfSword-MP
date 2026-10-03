//! Listing ownership: every register, heartbeat and delete is signed with the server's
//! listing key.
//!
//! The server identity is an X25519 static key (it pins the encrypted transport), and X25519
//! cannot sign. The listing key is an Ed25519 key derived from the same 32-byte secret with
//! HKDF-SHA256, so a server keeps the same listing key for as long as it keeps its identity
//! file, and nothing new has to be stored.
//!
//! What is signed is the exact request: `"HSMP master v1\n" METHOD " " PATH "\n" BODY`, with
//! PATH the API path (`/v1/register`, `/v1/heartbeat/<id>`, `/v1/servers/<id>`) and BODY the
//! raw JSON bytes on the wire. The signature travels in the `x-hsmp-sig` header (128 hex), so
//! nothing has to be canonicalised and a master that predates signing ignores it. The body
//! carries `listing_key` (64 hex) and `ts` (unix ms); the master keeps the last `ts` it
//! accepted per listing and refuses anything not newer, which stops replays.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use sha2::Sha256;

/// Request header that carries the hex Ed25519 signature.
pub const SIG_HEADER: &str = "x-hsmp-sig";
const DOMAIN: &[u8] = b"HSMP master v1\n";
const KDF_SALT: &[u8] = b"HSMP/5 master listing";
const KDF_INFO: &[u8] = b"ed25519 seed";

/// The listing key of a server identity (`server_identity.key`, 32 bytes).
pub fn listing_key(static_secret: &[u8; 32]) -> SigningKey {
    let hk = Hkdf::<Sha256>::new(Some(KDF_SALT), static_secret);
    let mut seed = [0u8; 32];
    hk.expand(KDF_INFO, &mut seed).expect("32 bytes is a valid HKDF-SHA256 length");
    SigningKey::from_bytes(&seed)
}

/// Lower-case hex of the public half, as sent in `listing_key`.
pub fn public_hex(sk: &SigningKey) -> String {
    hex::encode(sk.verifying_key().as_bytes())
}

/// The bytes that are signed for one request.
pub fn signed_message(method: &str, path: &str, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(DOMAIN.len() + method.len() + path.len() + body.len() + 2);
    m.extend_from_slice(DOMAIN);
    m.extend_from_slice(method.as_bytes());
    m.push(b' ');
    m.extend_from_slice(path.as_bytes());
    m.push(b'\n');
    m.extend_from_slice(body);
    m
}

/// Hex signature for the `x-hsmp-sig` header.
pub fn sign(sk: &SigningKey, method: &str, path: &str, body: &[u8]) -> String {
    hex::encode(sk.sign(&signed_message(method, path, body)).to_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// `listing_key` is not 64 hex chars or not a valid Ed25519 point.
    BadKey,
    /// The header is missing or not 128 hex chars.
    BadSignatureShape,
    /// Well formed, but does not verify.
    BadSignature,
}

/// Parse a hex Ed25519 public key (64 hex chars).
pub fn parse_key(hex_key: &str) -> Result<VerifyingKey, AuthError> {
    let mut b = [0u8; 32];
    if hex_key.len() != 64 || hex::decode_to_slice(hex_key, &mut b).is_err() {
        return Err(AuthError::BadKey);
    }
    VerifyingKey::from_bytes(&b).map_err(|_| AuthError::BadKey)
}

/// Check `sig_hex` over the request with the hex public key. Strict verification: weak
/// (small-order) keys and non-canonical signatures are refused.
pub fn verify(key_hex: &str, sig_hex: Option<&str>, method: &str, path: &str, body: &[u8]) -> Result<(), AuthError> {
    let key = parse_key(key_hex)?;
    let sig_hex = sig_hex.map(str::trim).ok_or(AuthError::BadSignatureShape)?;
    let mut s = [0u8; 64];
    if sig_hex.len() != 128 || hex::decode_to_slice(sig_hex, &mut s).is_err() {
        return Err(AuthError::BadSignatureShape);
    }
    key.verify_strict(&signed_message(method, path, body), &Signature::from_bytes(&s))
        .map_err(|_| AuthError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_key_is_stable_and_distinct_per_identity() {
        let a = listing_key(&[7; 32]);
        assert_eq!(a.to_bytes(), listing_key(&[7; 32]).to_bytes());
        assert_ne!(a.to_bytes(), listing_key(&[8; 32]).to_bytes());
        // not the identity secret itself
        assert_ne!(a.to_bytes(), [7; 32]);
        assert_eq!(public_hex(&a).len(), 64);
    }

    #[test]
    fn sign_then_verify_and_every_tamper_fails() {
        let sk = listing_key(&[1; 32]);
        let pk = public_hex(&sk);
        let body = br#"{"players":2,"ts":5}"#;
        let sig = sign(&sk, "POST", "/v1/heartbeat/ab", body);
        assert_eq!(verify(&pk, Some(&sig), "POST", "/v1/heartbeat/ab", body), Ok(()));
        // another body, path, method or key
        assert_eq!(verify(&pk, Some(&sig), "POST", "/v1/heartbeat/ab", br#"{"players":3,"ts":5}"#), Err(AuthError::BadSignature));
        assert_eq!(verify(&pk, Some(&sig), "POST", "/v1/heartbeat/cd", body), Err(AuthError::BadSignature));
        assert_eq!(verify(&pk, Some(&sig), "DELETE", "/v1/heartbeat/ab", body), Err(AuthError::BadSignature));
        let other = public_hex(&listing_key(&[2; 32]));
        assert_eq!(verify(&other, Some(&sig), "POST", "/v1/heartbeat/ab", body), Err(AuthError::BadSignature));
        // a flipped bit in the signature
        let mut bad = sig.clone().into_bytes();
        bad[10] = if bad[10] == b'0' { b'1' } else { b'0' };
        assert_eq!(verify(&pk, Some(std::str::from_utf8(&bad).unwrap()), "POST", "/v1/heartbeat/ab", body), Err(AuthError::BadSignature));
    }

    #[test]
    fn malformed_inputs_are_refused_not_panicked() {
        let sk = listing_key(&[1; 32]);
        let pk = public_hex(&sk);
        assert_eq!(verify(&pk, None, "POST", "/", b""), Err(AuthError::BadSignatureShape));
        assert_eq!(verify(&pk, Some("zz"), "POST", "/", b""), Err(AuthError::BadSignatureShape));
        assert_eq!(verify(&pk, Some(&"g".repeat(128)), "POST", "/", b""), Err(AuthError::BadSignatureShape));
        assert_eq!(verify("", Some(&"0".repeat(128)), "POST", "/", b""), Err(AuthError::BadKey));
        assert_eq!(verify(&"x".repeat(64), Some(&"0".repeat(128)), "POST", "/", b""), Err(AuthError::BadKey));
        // the all-zero signature against a real key
        assert_eq!(verify(&pk, Some(&"0".repeat(128)), "POST", "/", b""), Err(AuthError::BadSignature));
        // a small-order key (the identity point) is refused by strict verification
        let mut id = [0u8; 32];
        id[0] = 1;
        assert!(verify(&hex::encode(id), Some(&"0".repeat(128)), "POST", "/", b"").is_err());
    }

    #[test]
    fn signed_message_layout_is_fixed() {
        assert_eq!(signed_message("POST", "/v1/register", b"{}"), b"HSMP master v1\nPOST /v1/register\n{}".to_vec());
    }
}

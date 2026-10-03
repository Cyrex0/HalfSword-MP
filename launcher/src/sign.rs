//! Ed25519 manifest signatures.
//!
//! `manifest.sig` (JSON) signs the exact bytes of `manifest.json`:
//!
//! ```json
//! { "alg": "ed25519", "key_id": "<16 hex>", "sig": "<128 hex>" }
//! ```
//!
//! `key_id` = first 16 hex chars of SHA-256(public key). The launcher trusts
//! the public keys compiled in from `launcher/trusted_keys.txt`
//! (one `<64 hex public key> <label>` per line). The private key never
//! touches the repo: `hsmp-release` reads it from `HSMP_RELEASE_SIGNING_KEY`.

use crate::util;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const ALG: &str = "ed25519";

/// Keys compiled into this launcher build.
pub const EMBEDDED_TRUSTED_KEYS: &str = include_str!("../trusted_keys.txt");

#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub id: String,
    pub key: VerifyingKey,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SigFile {
    pub alg: String,
    pub key_id: String,
    pub sig: String,
}

impl SigFile {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec_pretty(self).expect("sig serializes");
        v.push(b'\n');
        v
    }
    pub fn parse(bytes: &[u8]) -> Result<SigFile, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("manifest.sig is not valid: {e}"))
    }
}

pub fn key_id(vk: &VerifyingKey) -> String {
    util::sha256_hex(vk.as_bytes())[..16].to_string()
}

/// Parse `<hex public key> [label]` lines ('#' comments, blank lines ignored).
pub fn parse_trusted_keys(text: &str) -> Result<Vec<TrustedKey>, String> {
    let mut out = vec![];
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, label) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        let bytes: [u8; 32] = hex::decode(k)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| format!("trusted key line {}: not 64 hex chars", n + 1))?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|e| format!("trusted key line {}: {e}", n + 1))?;
        out.push(TrustedKey { id: key_id(&key), key, label: label.trim().to_string() });
    }
    Ok(out)
}

pub fn embedded_keys() -> Vec<TrustedKey> {
    parse_trusted_keys(EMBEDDED_TRUSTED_KEYS).expect("trusted_keys.txt is checked by a unit test")
}

/// Parse a signing key: 64 hex chars (the 32-byte seed), surrounding whitespace ignored.
pub fn parse_signing_key(text: &str) -> Result<SigningKey, String> {
    let t = text.trim();
    let bytes: [u8; 32] = hex::decode(t)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("signing key must be 64 hex characters (a 32-byte ed25519 seed)")?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn sign(manifest_bytes: &[u8], key: &SigningKey) -> SigFile {
    let sig = key.sign(manifest_bytes);
    SigFile { alg: ALG.into(), key_id: key_id(&key.verifying_key()), sig: hex::encode(sig.to_bytes()) }
}

/// Verify `manifest_bytes` against `sig` with one of `keys` (strict verification).
pub fn verify<'k>(manifest_bytes: &[u8], sig: &SigFile, keys: &'k [TrustedKey]) -> Result<&'k TrustedKey, String> {
    if sig.alg != ALG {
        return Err(format!("unsupported signature algorithm '{}'", sig.alg));
    }
    if keys.is_empty() {
        return Err("this launcher build has no trusted release keys (launcher/trusted_keys.txt is empty)".into());
    }
    let key = keys.iter().find(|k| k.id == sig.key_id).ok_or_else(|| {
        format!(
            "the release is signed with key {} which this launcher does not trust (trusted: {})",
            sig.key_id,
            keys.iter().map(|k| k.id.as_str()).collect::<Vec<_>>().join(", ")
        )
    })?;
    let bytes: [u8; 64] = hex::decode(&sig.sig).ok().and_then(|b| b.try_into().ok()).ok_or("signature is not 128 hex characters")?;
    let s = Signature::from_bytes(&bytes);
    key.key.verify_strict(manifest_bytes, &s).map_err(|_| "manifest signature is INVALID: the package was modified or corrupted".to_string())?;
    Ok(key)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    pub fn trusted(k: &SigningKey) -> Vec<TrustedKey> {
        parse_trusted_keys(&format!("# test\n{} test key\n", hex::encode(k.verifying_key().as_bytes()))).unwrap()
    }

    #[test]
    fn embedded_keys_parse() {
        let _ = embedded_keys();
    }

    #[test]
    fn sign_verify_roundtrip() {
        let k = test_key(7);
        let msg = b"{\"schema\":1}\n";
        let s = sign(msg, &k);
        assert_eq!(s.sig.len(), 128);
        let keys = trusted(&k);
        assert_eq!(verify(msg, &s, &keys).unwrap().label, "test key");
        // sig file survives serialization
        let s2 = SigFile::parse(&s.to_bytes()).unwrap();
        assert!(verify(msg, &s2, &keys).is_ok());
    }

    #[test]
    fn tampering_is_detected() {
        let k = test_key(7);
        let msg = b"manifest bytes";
        let s = sign(msg, &k);
        let keys = trusted(&k);
        assert!(verify(b"manifest byteS", &s, &keys).unwrap_err().contains("INVALID"));
        let mut bad = s.clone();
        bad.sig.replace_range(0..2, if &bad.sig[0..2] == "00" { "11" } else { "00" });
        assert!(verify(msg, &bad, &keys).is_err());
        let other = trusted(&test_key(8));
        assert!(verify(msg, &s, &other).unwrap_err().contains("does not trust"));
        assert!(verify(msg, &s, &[]).unwrap_err().contains("no trusted"));
        let mut alg = s.clone();
        alg.alg = "rsa".into();
        assert!(verify(msg, &alg, &keys).is_err());
        // a signature made by an untrusted key that claims a trusted key id
        let mut forged = sign(msg, &test_key(9));
        forged.key_id = s.key_id.clone();
        assert!(verify(msg, &forged, &keys).unwrap_err().contains("INVALID"));
    }

    #[test]
    fn key_parsing() {
        assert!(parse_signing_key(&"ab".repeat(32)).is_ok());
        assert!(parse_signing_key(&format!("  {}\r\n", "ab".repeat(32))).is_ok());
        assert!(parse_signing_key("abcd").is_err());
        assert!(parse_trusted_keys("nothex label").is_err());
        assert!(parse_trusted_keys("").unwrap().is_empty());
    }
}

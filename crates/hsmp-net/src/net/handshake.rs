//! v5 handshake: `C2SHello` -> `S2CChallenge` -> `C2SAuth`, then data.
//!
//! * The server answers a Hello **statelessly**: the Challenge carries a
//!   cookie `ts | HMAC(cookie_key, addr | H(hello_core) | s_eph | ts)[..16]`.
//!   Nothing is allocated until an Auth with a valid cookie from the same
//!   address arrives, so a spoofed source address costs the server one HMAC
//!   and gets the victim one reply no larger than the Hello (>= 1200 B).
//! * Keys: `ee = X25519(c_eph, s_eph)` (forward secrecy, `s_eph` rotates
//!   every 2 min) and `es = X25519(c_eph, s_static)` (server authentication;
//!   `s_static` is pinned from the master listing or trusted on first use).
//!   HKDF-SHA256 over both, salted with the transcript hash.
//! * The client proves its Ed25519 `player_key` by signing the transcript
//!   inside the encrypted Auth payload. Nothing secret is ever sent in clear.
//!
//! Layouts (all integers little-endian) are in `docs/development/protocol.md`.

use std::net::SocketAddr;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::crypto::{self, HandshakeSecrets};
use super::wire::{Put, Reader};
use super::{negotiate_version, HELLO_LEN, PT_AUTH, PT_AUTH_REJECT, PT_CHALLENGE, PT_HELLO, PT_PRE_REJECT, TAG_LEN};

pub const COOKIE_LEN: usize = 20;
pub const MAX_BUILD: usize = 32;
pub const MAX_NICK: usize = 32;
pub const MAX_REJECT_TEXT: usize = 512;
pub const CHALLENGE_LEN: usize = 1 + 8 + CHALLENGE_CORE_LEN;
pub const CHALLENGE_CORE_LEN: usize = 2 + 8 + 32 + 32 + 1 + 16 + COOKIE_LEN;
/// Challenge flag: the server requires `pwd_proof`.
pub const FLAG_NEEDS_PASSWORD: u8 = 0x01;

pub mod reject_code {
    pub const VERSION: u8 = 1;
    pub const CONTENT: u8 = 2;
    pub const BANNED: u8 = 3;
    pub const FULL: u8 = 4;
    pub const BAD_PASSWORD: u8 = 5;
    pub const BAD_IDENTITY: u8 = 6;
    pub const KICK_COOLDOWN: u8 = 7;
    pub const SERVER_CLOSING: u8 = 8;
    pub const DUPLICATE_PLAYER: u8 = 9;
    pub const RATE_LIMITED: u8 = 10;
    pub const INTERNAL: u8 = 255;
}

pub mod role {
    pub const FIGHTER: u8 = 0;
    pub const SPECTATOR: u8 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsError {
    WrongType,
    TooShort,
    Malformed,
    NoVersion,
    UnknownEph,
    BadCookie,
    StaleCookie,
    LowOrder,
    Decrypt,
    BadSignature,
    EchoMismatch,
    ServerKeyMismatch,
    /// The (cookie-proven) source exceeded its Auth budget: refused before
    /// any Diffie-Hellman or signature work.
    RateLimited,
    /// The Auth repeats a client ephemeral the server already answered
    /// (replay or resend): refused before charging the source budget or doing
    /// any Diffie-Hellman work.
    Replayed,
}

fn m<T>(_: T) -> HsError {
    HsError::Malformed
}

// ---------------------------------------------------------------------------
// Hello
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloCore {
    pub version_min: u16,
    pub version_max: u16,
    pub caps: u64,
    pub eph_pub: [u8; 32],
    pub content_hash: [u8; 32],
    pub build: String,
}

impl HelloCore {
    pub fn encode(&self) -> Vec<u8> {
        let build = super::channel::truncate_utf8(&self.build, MAX_BUILD);
        let mut w = Vec::with_capacity(77 + build.len());
        w.put_u16(self.version_min);
        w.put_u16(self.version_max);
        w.put_u64(self.caps);
        w.put(&self.eph_pub);
        w.put(&self.content_hash);
        w.put_u8(build.len() as u8);
        w.put(build.as_bytes());
        w
    }

    pub fn decode(r: &mut Reader) -> Result<Self, HsError> {
        let version_min = r.u16().map_err(m)?;
        let version_max = r.u16().map_err(m)?;
        let caps = r.u64().map_err(m)?;
        let eph_pub = r.array().map_err(m)?;
        let content_hash = r.array().map_err(m)?;
        let n = r.u8().map_err(m)? as usize;
        if n > MAX_BUILD {
            return Err(HsError::Malformed);
        }
        let build = std::str::from_utf8(r.bytes(n).map_err(m)?).map_err(m)?.to_string();
        Ok(HelloCore {
            version_min,
            version_max,
            caps,
            eph_pub,
            content_hash,
            build,
        })
    }

    pub fn echo(&self) -> [u8; 8] {
        let mut e = [0u8; 8];
        e.copy_from_slice(&self.eph_pub[..8]);
        e
    }
}

pub fn hello_datagram(core: &HelloCore) -> Vec<u8> {
    let mut dg = Vec::with_capacity(HELLO_LEN);
    dg.push(PT_HELLO);
    dg.extend_from_slice(&core.encode());
    dg.resize(HELLO_LEN, 0);
    dg
}

/// Returns the core and its exact encoded bytes.
pub fn parse_hello(dg: &[u8]) -> Result<(HelloCore, &[u8]), HsError> {
    if dg.first() != Some(&PT_HELLO) {
        return Err(HsError::WrongType);
    }
    if dg.len() < HELLO_LEN {
        return Err(HsError::TooShort);
    }
    let mut r = Reader::new(&dg[1..]);
    let core = HelloCore::decode(&mut r)?;
    let end = 1 + r.pos();
    Ok((core, &dg[1..end]))
}

// ---------------------------------------------------------------------------
// Challenge
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeCore {
    pub version: u16,
    pub server_caps: u64,
    pub static_pub: [u8; 32],
    pub eph_pub: [u8; 32],
    pub flags: u8,
    pub pwd_salt: [u8; 16],
    pub cookie: [u8; COOKIE_LEN],
}

impl ChallengeCore {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(CHALLENGE_CORE_LEN);
        w.put_u16(self.version);
        w.put_u64(self.server_caps);
        w.put(&self.static_pub);
        w.put(&self.eph_pub);
        w.put_u8(self.flags);
        w.put(&self.pwd_salt);
        w.put(&self.cookie);
        w
    }

    pub fn decode(r: &mut Reader) -> Result<Self, HsError> {
        Ok(ChallengeCore {
            version: r.u16().map_err(m)?,
            server_caps: r.u64().map_err(m)?,
            static_pub: r.array().map_err(m)?,
            eph_pub: r.array().map_err(m)?,
            flags: r.u8().map_err(m)?,
            pwd_salt: r.array().map_err(m)?,
            cookie: r.array().map_err(m)?,
        })
    }
}

pub fn challenge_datagram(echo: [u8; 8], core: &ChallengeCore) -> Vec<u8> {
    let mut dg = Vec::with_capacity(CHALLENGE_LEN);
    dg.push(PT_CHALLENGE);
    dg.extend_from_slice(&echo);
    dg.extend_from_slice(&core.encode());
    dg
}

pub fn parse_challenge(dg: &[u8]) -> Result<([u8; 8], ChallengeCore), HsError> {
    if dg.first() != Some(&PT_CHALLENGE) {
        return Err(HsError::WrongType);
    }
    let mut r = Reader::new(&dg[1..]);
    let echo = r.array().map_err(m)?;
    let core = ChallengeCore::decode(&mut r)?;
    Ok((echo, core))
}

// ---------------------------------------------------------------------------
// Pre-cookie reject (cleartext, unauthenticated, <= the Hello)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreReject {
    pub echo: [u8; 8],
    pub code: u8,
    pub server_min: u16,
    pub server_max: u16,
    pub text: String,
}

pub fn pre_reject_datagram(p: &PreReject, max_len: usize) -> Vec<u8> {
    let room = max_len.saturating_sub(1 + 8 + 1 + 2 + 2 + 2).min(MAX_REJECT_TEXT);
    let text = super::channel::truncate_utf8(&p.text, room);
    let mut dg = Vec::new();
    dg.put_u8(PT_PRE_REJECT);
    dg.put(&p.echo);
    dg.put_u8(p.code);
    dg.put_u16(p.server_min);
    dg.put_u16(p.server_max);
    dg.put_u16(text.len() as u16);
    dg.put(text.as_bytes());
    dg
}

pub fn parse_pre_reject(dg: &[u8]) -> Result<PreReject, HsError> {
    if dg.first() != Some(&PT_PRE_REJECT) {
        return Err(HsError::WrongType);
    }
    let mut r = Reader::new(&dg[1..]);
    let echo = r.array().map_err(m)?;
    let code = r.u8().map_err(m)?;
    let server_min = r.u16().map_err(m)?;
    let server_max = r.u16().map_err(m)?;
    let n = r.u16().map_err(m)? as usize;
    if n > MAX_REJECT_TEXT {
        return Err(HsError::Malformed);
    }
    let text = String::from_utf8_lossy(r.bytes(n).map_err(m)?).into_owned();
    Ok(PreReject {
        echo,
        code,
        server_min,
        server_max,
        text,
    })
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AuthPayload {
    pub player_key: [u8; 32],
    pub sig: Vec<u8>,
    pub want_role: u8,
    pub resume_token: Option<[u8; 16]>,
    pub pwd_proof: Option<[u8; 32]>,
    pub party_token: Option<[u8; 16]>,
    pub nick: String,
}

impl AuthPayload {
    pub fn encode(&self) -> Vec<u8> {
        let nick = super::channel::truncate_utf8(&self.nick, MAX_NICK);
        let mut w = Vec::with_capacity(200);
        w.put(&self.player_key);
        let mut sig = [0u8; 64];
        let n = self.sig.len().min(64);
        sig[..n].copy_from_slice(&self.sig[..n]);
        w.put(&sig);
        w.put_u8(self.want_role);
        let flags = self.resume_token.is_some() as u8 | (self.pwd_proof.is_some() as u8) << 1 | (self.party_token.is_some() as u8) << 2;
        w.put_u8(flags);
        if let Some(t) = &self.resume_token {
            w.put(t);
        }
        if let Some(p) = &self.pwd_proof {
            w.put(p);
        }
        if let Some(t) = &self.party_token {
            w.put(t);
        }
        w.put_u8(nick.len() as u8);
        w.put(nick.as_bytes());
        w
    }

    pub fn decode(b: &[u8]) -> Result<Self, HsError> {
        let mut r = Reader::new(b);
        let player_key = r.array().map_err(m)?;
        let sig = r.bytes(64).map_err(m)?.to_vec();
        let want_role = r.u8().map_err(m)?;
        let flags = r.u8().map_err(m)?;
        if flags & !0b111 != 0 {
            return Err(HsError::Malformed);
        }
        let resume_token = if flags & 1 != 0 { Some(r.array().map_err(m)?) } else { None };
        let pwd_proof = if flags & 2 != 0 { Some(r.array().map_err(m)?) } else { None };
        let party_token = if flags & 4 != 0 { Some(r.array().map_err(m)?) } else { None };
        let n = r.u8().map_err(m)? as usize;
        if n > MAX_NICK {
            return Err(HsError::Malformed);
        }
        let nick = std::str::from_utf8(r.bytes(n).map_err(m)?).map_err(m)?.to_string();
        if !r.is_empty() {
            return Err(HsError::Malformed);
        }
        Ok(AuthPayload {
            player_key,
            sig,
            want_role,
            resume_token,
            pwd_proof,
            party_token,
            nick,
        })
    }
}

pub fn sig_message(th: &[u8; 32]) -> Vec<u8> {
    let mut v = b"HSMP/5 auth\0".to_vec();
    v.extend_from_slice(th);
    v
}

/// `pwd_proof = HMAC-SHA256(password_key, "HSMP/5 pwd\0" || th)`; the
/// password key is `argon2(password, pwd_salt)` (computed by the caller).
pub fn password_proof(password_key: &[u8; 32], th: &[u8; 32]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(password_key).expect("any key length");
    mac.update(b"HSMP/5 pwd\0");
    mac.update(th);
    mac.finalize().into_bytes().into()
}

pub fn verify_password_proof(password_key: &[u8; 32], th: &[u8; 32], proof: &[u8; 32]) -> bool {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(password_key).expect("any key length");
    mac.update(b"HSMP/5 pwd\0");
    mac.update(th);
    mac.verify_slice(proof).is_ok()
}

pub struct ParsedAuth<'a> {
    pub hello: HelloCore,
    pub hello_bytes: &'a [u8],
    pub s_eph: [u8; 32],
    pub cookie: [u8; COOKIE_LEN],
    pub aad: &'a [u8],
    pub sealed: &'a [u8],
}

pub fn parse_auth(dg: &[u8]) -> Result<ParsedAuth<'_>, HsError> {
    if dg.first() != Some(&PT_AUTH) {
        return Err(HsError::WrongType);
    }
    let mut r = Reader::new(dg);
    r.u8().map_err(m)?;
    let n = r.u8().map_err(m)? as usize;
    let start = r.pos();
    let core_bytes = r.bytes(n).map_err(m)?;
    let mut cr = Reader::new(core_bytes);
    let hello = HelloCore::decode(&mut cr)?;
    if !cr.is_empty() {
        return Err(HsError::Malformed);
    }
    let s_eph = r.array().map_err(m)?;
    let cookie = r.array().map_err(m)?;
    let aad_end = r.pos();
    let sealed = r.rest();
    if sealed.len() < TAG_LEN {
        return Err(HsError::Malformed);
    }
    Ok(ParsedAuth {
        hello,
        hello_bytes: &dg[start..start + n],
        s_eph,
        cookie,
        aad: &dg[..aad_end],
        sealed,
    })
}

pub fn auth_datagram(
    hello_bytes: &[u8],
    s_eph: &[u8; 32],
    cookie: &[u8; COOKIE_LEN],
    secrets: &HandshakeSecrets,
    payload: &AuthPayload,
) -> Vec<u8> {
    let mut dg = Vec::with_capacity(400);
    dg.put_u8(PT_AUTH);
    dg.put_u8(hello_bytes.len() as u8);
    dg.put(hello_bytes);
    dg.put(s_eph);
    dg.put(cookie);
    let ct = crypto::seal(&crypto::cipher(&secrets.auth), crypto::NONCE_AUTH, &dg, &payload.encode());
    dg.extend_from_slice(&ct);
    dg
}

/// Sealed, stateless reject after a valid Auth (key `s2c`), <= `max_len`.
pub fn auth_reject_datagram(secrets: &HandshakeSecrets, code: u8, text: &str, max_len: usize) -> Vec<u8> {
    let room = max_len.saturating_sub(9 + 3 + TAG_LEN).min(MAX_REJECT_TEXT);
    let text = super::channel::truncate_utf8(text, room);
    let mut dg = Vec::new();
    dg.put_u8(PT_AUTH_REJECT);
    dg.put_u64(secrets.conn_id);
    let mut pt = Vec::new();
    pt.put_u8(code);
    pt.put_u16(text.len() as u16);
    pt.put(text.as_bytes());
    let ct = crypto::seal(&crypto::cipher(&secrets.s2c), crypto::NONCE_AUTH_REJECT, &dg, &pt);
    dg.extend_from_slice(&ct);
    dg
}

pub fn open_auth_reject(secrets: &HandshakeSecrets, dg: &[u8]) -> Option<(u8, String)> {
    if dg.len() < 9 + TAG_LEN || dg[0] != PT_AUTH_REJECT || dg[1..9] != secrets.conn_id.to_le_bytes() {
        return None;
    }
    let pt = crypto::open(&crypto::cipher(&secrets.s2c), crypto::NONCE_AUTH_REJECT, &dg[..9], &dg[9..])?;
    let mut r = Reader::new(&pt);
    let code = r.u8().ok()?;
    let n = r.u16().ok()? as usize;
    let text = String::from_utf8_lossy(r.bytes(n).ok()?).into_owned();
    Some((code, text))
}

fn dh(secret: &StaticSecret, public: &[u8; 32]) -> Result<[u8; 32], HsError> {
    let s = secret.diffie_hellman(&PublicKey::from(*public));
    if !s.was_contributory() {
        return Err(HsError::LowOrder);
    }
    Ok(*s.as_bytes())
}

// ---------------------------------------------------------------------------
// Client side
// ---------------------------------------------------------------------------

/// Output of a processed Challenge: the Auth datagram and the session keys.
pub struct ClientAuth {
    pub datagram: Vec<u8>,
    pub secrets: HandshakeSecrets,
    pub th: [u8; 32],
    pub version: u16,
    pub server_static: [u8; 32],
    pub flags: u8,
    pub pwd_salt: [u8; 16],
}

/// Client: turn a Challenge into an Auth. `make_payload` receives the
/// transcript hash and challenge (for the password proof) and returns the
/// payload without `player_key`/`sig`, which are filled in here.
pub fn client_on_challenge(
    eph: &StaticSecret,
    hello: &HelloCore,
    hello_bytes: &[u8],
    dg: &[u8],
    pinned: Option<[u8; 32]>,
    player: &SigningKey,
    make_payload: impl FnOnce(&[u8; 32], &ChallengeCore) -> AuthPayload,
) -> Result<ClientAuth, HsError> {
    let (echo, ch) = parse_challenge(dg)?;
    if echo != hello.echo() {
        return Err(HsError::EchoMismatch);
    }
    if negotiate_version(hello.version_min, hello.version_max, ch.version, ch.version) != Some(ch.version) {
        return Err(HsError::NoVersion);
    }
    if let Some(p) = pinned {
        if p != ch.static_pub {
            return Err(HsError::ServerKeyMismatch);
        }
    }
    let ee = dh(eph, &ch.eph_pub)?;
    let es = dh(eph, &ch.static_pub)?;
    let th = crypto::transcript(hello_bytes, &ch.encode());
    let secrets = crypto::derive(&th, &ee, &es);
    let mut payload = make_payload(&th, &ch);
    payload.player_key = player.verifying_key().to_bytes();
    payload.sig = player.sign(&sig_message(&th)).to_bytes().to_vec();
    let datagram = auth_datagram(hello_bytes, &ch.eph_pub, &ch.cookie, &secrets, &payload);
    Ok(ClientAuth {
        datagram,
        secrets,
        th,
        version: ch.version,
        server_static: ch.static_pub,
        flags: ch.flags,
        pwd_salt: ch.pwd_salt,
    })
}

// ---------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ServerHsConfig {
    pub static_secret: [u8; 32],
    pub version_min: u16,
    pub version_max: u16,
    pub caps: u64,
    /// `None` disables the content check (dev servers).
    pub content_hash: Option<[u8; 32]>,
    pub needs_password: bool,
    pub pwd_salt: [u8; 16],
    /// Ephemeral key and cookie key rotation period.
    pub rotate_ms: u64,
    /// Cookie lifetime.
    pub cookie_ttl_s: u32,
}

impl ServerHsConfig {
    pub fn new(static_secret: [u8; 32]) -> Self {
        ServerHsConfig {
            static_secret,
            version_min: super::VERSION_MIN,
            version_max: super::VERSION_MAX,
            caps: super::caps::SUPPORTED,
            content_hash: None,
            needs_password: false,
            pwd_salt: [0; 16],
            rotate_ms: 120_000,
            cookie_ttl_s: 30,
        }
    }
}

struct EphKey {
    secret: StaticSecret,
    public: [u8; 32],
}

impl EphKey {
    fn random<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let secret = StaticSecret::random_from_rng(rng);
        let public = PublicKey::from(&secret).to_bytes();
        EphKey { secret, public }
    }
}

pub enum HelloOutcome {
    Challenge(Vec<u8>),
    Reject(Vec<u8>),
}

pub struct AuthOk {
    pub secrets: HandshakeSecrets,
    pub th: [u8; 32],
    pub version: u16,
    /// Negotiated: `client_caps & server_caps`.
    pub caps: u64,
    pub hello: HelloCore,
    pub payload: AuthPayload,
}

pub struct ServerHandshake {
    cfg: ServerHsConfig,
    static_secret: StaticSecret,
    static_pub: [u8; 32],
    eph: EphKey,
    eph_prev: Option<EphKey>,
    cookie_key: [u8; 32],
    cookie_key_prev: Option<[u8; 32]>,
    rotated_at: u64,
}

impl ServerHandshake {
    pub fn new<R: RngCore + CryptoRng>(cfg: ServerHsConfig, now: u64, rng: &mut R) -> Self {
        let static_secret = StaticSecret::from(cfg.static_secret);
        let static_pub = PublicKey::from(&static_secret).to_bytes();
        let mut cookie_key = [0u8; 32];
        rng.fill_bytes(&mut cookie_key);
        ServerHandshake {
            static_secret,
            static_pub,
            eph: EphKey::random(rng),
            eph_prev: None,
            cookie_key,
            cookie_key_prev: None,
            rotated_at: now,
            cfg,
        }
    }

    pub fn config(&self) -> &ServerHsConfig {
        &self.cfg
    }

    pub fn static_public(&self) -> [u8; 32] {
        self.static_pub
    }

    /// When the ephemeral key (and cookie key) last rotated.
    pub fn rotated_at(&self) -> u64 {
        self.rotated_at
    }

    pub fn rotate_if_due<R: RngCore + CryptoRng>(&mut self, now: u64, rng: &mut R) {
        if now.saturating_sub(self.rotated_at) < self.cfg.rotate_ms {
            return;
        }
        let new = EphKey::random(rng);
        self.eph_prev = Some(std::mem::replace(&mut self.eph, new));
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        self.cookie_key_prev = Some(std::mem::replace(&mut self.cookie_key, k));
        self.rotated_at = now;
    }

    fn cookie_mac(key: &[u8; 32], from: &SocketAddr, hello_bytes: &[u8], s_eph: &[u8; 32], ts: u32) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
        mac.update(b"HSMP/5 cookie\0");
        match from {
            SocketAddr::V4(a) => {
                mac.update(&[4]);
                mac.update(&a.ip().octets());
            }
            SocketAddr::V6(a) => {
                mac.update(&[6]);
                mac.update(&a.ip().octets());
            }
        }
        mac.update(&from.port().to_le_bytes());
        mac.update(&Sha256::digest(hello_bytes));
        mac.update(s_eph);
        mac.update(&ts.to_le_bytes());
        mac
    }

    fn make_cookie(&self, from: &SocketAddr, hello_bytes: &[u8], now: u64) -> [u8; COOKIE_LEN] {
        let ts = (now / 1000) as u32;
        let tag = Self::cookie_mac(&self.cookie_key, from, hello_bytes, &self.eph.public, ts)
            .finalize()
            .into_bytes();
        let mut c = [0u8; COOKIE_LEN];
        c[..4].copy_from_slice(&ts.to_le_bytes());
        c[4..].copy_from_slice(&tag[..16]);
        c
    }

    fn check_cookie(
        &self,
        from: &SocketAddr,
        hello_bytes: &[u8],
        s_eph: &[u8; 32],
        cookie: &[u8; COOKIE_LEN],
        now: u64,
    ) -> Result<(), HsError> {
        let ts = u32::from_le_bytes([cookie[0], cookie[1], cookie[2], cookie[3]]);
        let ok = |key: &[u8; 32]| {
            Self::cookie_mac(key, from, hello_bytes, s_eph, ts)
                .verify_truncated_left(&cookie[4..])
                .is_ok()
        };
        let valid = ok(&self.cookie_key) || self.cookie_key_prev.as_ref().is_some_and(ok);
        if !valid {
            return Err(HsError::BadCookie);
        }
        let now_s = (now / 1000) as u32;
        if ts > now_s.saturating_add(1) || now_s.saturating_sub(ts) > self.cfg.cookie_ttl_s {
            return Err(HsError::StaleCookie);
        }
        Ok(())
    }

    fn challenge_core(&self, version: u16, s_eph: [u8; 32], cookie: [u8; COOKIE_LEN]) -> ChallengeCore {
        ChallengeCore {
            version,
            server_caps: self.cfg.caps,
            static_pub: self.static_pub,
            eph_pub: s_eph,
            flags: if self.cfg.needs_password { FLAG_NEEDS_PASSWORD } else { 0 },
            pwd_salt: self.cfg.pwd_salt,
            cookie,
        }
    }

    /// Stateless reply to a Hello. Every reply is <= the Hello's size.
    pub fn on_hello<R: RngCore + CryptoRng>(
        &mut self,
        now: u64,
        from: SocketAddr,
        dg: &[u8],
        rng: &mut R,
    ) -> Result<HelloOutcome, HsError> {
        self.rotate_if_due(now, rng);
        let (core, core_bytes) = parse_hello(dg)?;
        let reject = |code: u8, text: String| {
            let p = PreReject {
                echo: core.echo(),
                code,
                server_min: self.cfg.version_min,
                server_max: self.cfg.version_max,
                text,
            };
            HelloOutcome::Reject(pre_reject_datagram(&p, dg.len()))
        };
        let Some(version) = negotiate_version(core.version_min, core.version_max, self.cfg.version_min, self.cfg.version_max) else {
            let text = format!(
                "version mismatch: server speaks HSMP protocol v{}..=v{}, your client v{}..=v{}. {}",
                self.cfg.version_min,
                self.cfg.version_max,
                core.version_min,
                core.version_max,
                if core.version_max < self.cfg.version_min {
                    "Your mod is OUTDATED: update it."
                } else {
                    "The SERVER is outdated: ask the host to update."
                }
            );
            return Ok(reject(reject_code::VERSION, text));
        };
        if let Some(h) = self.cfg.content_hash {
            if h != core.content_hash {
                return Ok(reject(reject_code::CONTENT, "mods differ from the server: update HSMP".into()));
            }
        }
        let cookie = self.make_cookie(&from, core_bytes, now);
        let ch = self.challenge_core(version, self.eph.public, cookie);
        let out = challenge_datagram(core.echo(), &ch);
        debug_assert!(out.len() <= dg.len());
        Ok(HelloOutcome::Challenge(out))
    }

    /// Validate an Auth. Cookie first (cheap), then DH, AEAD and signature.
    pub fn on_auth<R: RngCore + CryptoRng>(&mut self, now: u64, from: SocketAddr, dg: &[u8], rng: &mut R) -> Result<AuthOk, HsError> {
        self.on_auth_gated(now, from, dg, rng, |_, _| Ok(()))
    }

    /// `on_auth` with an admission gate run once the cookie proved the
    /// source address and before any public-key work (per-source Auth
    /// budget). `gate` false = `HsError::RateLimited`.
    pub fn on_auth_gated<R: RngCore + CryptoRng>(
        &mut self,
        now: u64,
        from: SocketAddr,
        dg: &[u8],
        rng: &mut R,
        gate: impl FnOnce(&SocketAddr, &[u8; 32]) -> Result<(), HsError>,
    ) -> Result<AuthOk, HsError> {
        self.rotate_if_due(now, rng);
        let a = parse_auth(dg)?;
        let eph = if a.s_eph == self.eph.public {
            &self.eph
        } else {
            match &self.eph_prev {
                Some(p) if p.public == a.s_eph => p,
                _ => return Err(HsError::UnknownEph),
            }
        };
        self.check_cookie(&from, a.hello_bytes, &a.s_eph, &a.cookie, now)?;
        gate(&from, &a.hello.eph_pub)?;
        let version = negotiate_version(a.hello.version_min, a.hello.version_max, self.cfg.version_min, self.cfg.version_max)
            .ok_or(HsError::NoVersion)?;
        let ee = dh(&eph.secret, &a.hello.eph_pub)?;
        let es = dh(&self.static_secret, &a.hello.eph_pub)?;
        let ch = self.challenge_core(version, a.s_eph, a.cookie);
        let th = crypto::transcript(a.hello_bytes, &ch.encode());
        let secrets = crypto::derive(&th, &ee, &es);
        let pt = crypto::open(&crypto::cipher(&secrets.auth), crypto::NONCE_AUTH, a.aad, a.sealed).ok_or(HsError::Decrypt)?;
        let payload = AuthPayload::decode(&pt)?;
        let vk = VerifyingKey::from_bytes(&payload.player_key).map_err(|_| HsError::BadSignature)?;
        let sig: [u8; 64] = payload.sig.as_slice().try_into().map_err(|_| HsError::BadSignature)?;
        vk.verify_strict(&sig_message(&th), &Signature::from_bytes(&sig))
            .map_err(|_| HsError::BadSignature)?;
        Ok(AuthOk {
            secrets,
            th,
            version,
            caps: a.hello.caps & self.cfg.caps,
            hello: a.hello,
            payload,
        })
    }
}

/// Short display id of a player key: first 8 bytes of SHA-256, hex.
pub fn player_fingerprint(player_key: &[u8; 32]) -> String {
    Sha256::digest(player_key)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    struct Fixture {
        rng: StdRng,
        server: ServerHandshake,
        eph: StaticSecret,
        hello: HelloCore,
        player: SigningKey,
    }

    fn fixture() -> Fixture {
        let mut rng = StdRng::seed_from_u64(7);
        let server = ServerHandshake::new(ServerHsConfig::new([9; 32]), 1_000_000, &mut rng);
        let eph = StaticSecret::random_from_rng(&mut rng);
        let hello = HelloCore {
            version_min: crate::net::VERSION_MIN,
            version_max: crate::net::VERSION_MAX,
            caps: 0,
            eph_pub: PublicKey::from(&eph).to_bytes(),
            content_hash: [0; 32],
            build: "test".into(),
        };
        let player = SigningKey::from_bytes(&[3; 32]);
        Fixture {
            rng,
            server,
            eph,
            hello,
            player,
        }
    }

    fn payload(_: &[u8; 32], _: &ChallengeCore) -> AuthPayload {
        AuthPayload {
            nick: "Willie".into(),
            ..Default::default()
        }
    }

    #[test]
    fn handshake_round_trip_derives_matching_keys() {
        let mut f = fixture();
        let a = addr("1.2.3.4:5000");
        let hello = hello_datagram(&f.hello);
        assert_eq!(hello.len(), HELLO_LEN);
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello, &mut f.rng).unwrap() else {
            panic!("expected challenge")
        };
        assert!(ch.len() <= hello.len());
        let hb = f.hello.encode();
        let auth = client_on_challenge(&f.eph, &f.hello, &hb, &ch, Some(f.server.static_public()), &f.player, payload).unwrap();
        let ok = f.server.on_auth(1_000_100, a, &auth.datagram, &mut f.rng).unwrap();
        assert_eq!(ok.secrets.c2s, auth.secrets.c2s);
        assert_eq!(ok.secrets.s2c, auth.secrets.s2c);
        assert_eq!(ok.secrets.conn_id, auth.secrets.conn_id);
        assert_eq!(ok.payload.player_key, f.player.verifying_key().to_bytes());
        assert_eq!(ok.payload.nick, "Willie");
        assert_eq!(ok.th, auth.th);
        // No key material in any cleartext datagram.
        for secret in [auth.secrets.c2s, auth.secrets.s2c, auth.secrets.auth, auth.secrets.resume] {
            for dg in [&hello, &ch, &auth.datagram] {
                assert!(!dg.windows(8).any(|w| secret.windows(8).any(|s| s == w)), "secret bytes leaked");
            }
        }
    }

    #[test]
    fn cookie_binds_address_hello_and_time() {
        let mut f = fixture();
        let a = addr("1.2.3.4:5000");
        let hello = hello_datagram(&f.hello);
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello, &mut f.rng).unwrap() else {
            panic!()
        };
        let hb = f.hello.encode();
        let auth = client_on_challenge(&f.eph, &f.hello, &hb, &ch, None, &f.player, payload).unwrap();
        // Other address / other port.
        assert_eq!(
            f.server.on_auth(1_000_100, addr("1.2.3.5:5000"), &auth.datagram, &mut f.rng).err(),
            Some(HsError::BadCookie)
        );
        assert_eq!(
            f.server.on_auth(1_000_100, addr("1.2.3.4:5001"), &auth.datagram, &mut f.rng).err(),
            Some(HsError::BadCookie)
        );
        // Expired.
        assert_eq!(
            f.server.on_auth(1_000_000 + 32_000, a, &auth.datagram, &mut f.rng).err(),
            Some(HsError::StaleCookie)
        );
        // Tampered hello core inside the Auth (e.g. version range or caps).
        let mut t = auth.datagram.clone();
        t[2 + 4] ^= 1; // caps byte
        assert_eq!(f.server.on_auth(1_000_100, a, &t, &mut f.rng).err(), Some(HsError::BadCookie));
        // Tampered sealed payload.
        let mut t = auth.datagram.clone();
        let n = t.len();
        t[n - 20] ^= 1;
        assert_eq!(f.server.on_auth(1_000_100, a, &t, &mut f.rng).err(), Some(HsError::Decrypt));
        // Unknown server ephemeral (substituted challenge).
        let mut t = auth.datagram.clone();
        let off = 2 + hb.len();
        t[off] ^= 1;
        assert_eq!(f.server.on_auth(1_000_100, a, &t, &mut f.rng).err(), Some(HsError::UnknownEph));
        // The genuine one still works.
        assert!(f.server.on_auth(1_000_100, a, &auth.datagram, &mut f.rng).is_ok());
    }

    #[test]
    fn rotation_keeps_one_previous_generation() {
        let mut f = fixture();
        let a = addr("[::1]:7777");
        let hello = hello_datagram(&f.hello);
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello, &mut f.rng).unwrap() else {
            panic!()
        };
        let hb = f.hello.encode();
        let auth = client_on_challenge(&f.eph, &f.hello, &hb, &ch, None, &f.player, payload).unwrap();
        let mut cfg = f.server.config().clone();
        cfg.cookie_ttl_s = 1000;
        let mut s2 = ServerHandshake { cfg, ..f.server };
        // One rotation later the Auth is still accepted ...
        s2.rotate_if_due(1_000_000 + 120_000, &mut f.rng);
        assert!(s2.on_auth(1_000_000 + 120_001, a, &auth.datagram, &mut f.rng).is_ok());
        // ... two rotations later the ephemeral key is gone.
        s2.rotate_if_due(1_000_000 + 240_001, &mut f.rng);
        assert_eq!(
            s2.on_auth(1_000_000 + 240_002, a, &auth.datagram, &mut f.rng).err(),
            Some(HsError::UnknownEph)
        );
    }

    #[test]
    fn signature_and_low_order_points_are_checked() {
        let mut f = fixture();
        let a = addr("1.2.3.4:5000");
        let hello = hello_datagram(&f.hello);
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello, &mut f.rng).unwrap() else {
            panic!()
        };
        let hb = f.hello.encode();
        // A valid Auth whose signature was made by a different key.
        let mut auth = client_on_challenge(&f.eph, &f.hello, &hb, &ch, None, &f.player, payload).unwrap();
        let mut p = AuthPayload::decode(
            &crypto::open(
                &crypto::cipher(&auth.secrets.auth),
                crypto::NONCE_AUTH,
                &auth.datagram[..2 + hb.len() + 52],
                &auth.datagram[2 + hb.len() + 52..],
            )
            .unwrap(),
        )
        .unwrap();
        p.player_key = SigningKey::from_bytes(&[4; 32]).verifying_key().to_bytes();
        let (_, chc) = parse_challenge(&ch).unwrap();
        auth.datagram = auth_datagram(&hb, &chc.eph_pub, &chc.cookie, &auth.secrets, &p);
        assert_eq!(
            f.server.on_auth(1_000_100, a, &auth.datagram, &mut f.rng).err(),
            Some(HsError::BadSignature)
        );
        // An all-zero (low-order) client ephemeral is refused.
        let mut low = f.hello.clone();
        low.eph_pub = [0; 32];
        let lowb = low.encode();
        let HelloOutcome::Challenge(ch2) = f.server.on_hello(1_000_000, a, &hello_datagram(&low), &mut f.rng).unwrap() else {
            panic!()
        };
        let (_, c2) = parse_challenge(&ch2).unwrap();
        let fake = HandshakeSecrets {
            auth: [0; 32],
            c2s: [0; 32],
            s2c: [0; 32],
            resume: [0; 32],
            conn_id: 1,
        };
        let dg = auth_datagram(&lowb, &c2.eph_pub, &c2.cookie, &fake, &p);
        assert_eq!(f.server.on_auth(1_000_100, a, &dg, &mut f.rng).err(), Some(HsError::LowOrder));
    }

    #[test]
    fn version_and_content_rejects_fit_in_the_hello() {
        let mut f = fixture();
        let a = addr("1.2.3.4:5000");
        let mut old = f.hello.clone();
        old.version_min = 3;
        old.version_max = 4;
        let hello = hello_datagram(&old);
        let HelloOutcome::Reject(r) = f.server.on_hello(1_000_000, a, &hello, &mut f.rng).unwrap() else {
            panic!("expected reject")
        };
        assert!(r.len() <= hello.len());
        let pr = parse_pre_reject(&r).unwrap();
        assert_eq!((pr.code, pr.server_min, pr.server_max), (reject_code::VERSION, crate::net::VERSION_MIN, crate::net::VERSION_MAX));
        assert_eq!(pr.echo, old.echo());
        assert!(pr.text.contains("OUTDATED"));
        // Newer client, overlapping range: negotiated down to this build's version.
        let mut newer = f.hello.clone();
        newer.version_max = 9;
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello_datagram(&newer), &mut f.rng).unwrap() else {
            panic!()
        };
        assert_eq!(parse_challenge(&ch).unwrap().1.version, crate::net::PROTOCOL_VERSION);
        // Content mismatch.
        let mut cfg = ServerHsConfig::new([9; 32]);
        cfg.content_hash = Some([1; 32]);
        let mut s = ServerHandshake::new(cfg, 0, &mut f.rng);
        let HelloOutcome::Reject(r) = s.on_hello(0, a, &hello_datagram(&f.hello), &mut f.rng).unwrap() else {
            panic!()
        };
        assert_eq!(parse_pre_reject(&r).unwrap().code, reject_code::CONTENT);
        // Short hellos get nothing at all.
        let mut short = hello_datagram(&f.hello);
        short.truncate(HELLO_LEN - 1);
        assert_eq!(f.server.on_hello(0, a, &short, &mut f.rng).err(), Some(HsError::TooShort));
    }

    #[test]
    fn pinned_key_mismatch_and_echo_are_enforced() {
        let mut f = fixture();
        let a = addr("1.2.3.4:5000");
        let HelloOutcome::Challenge(ch) = f.server.on_hello(1_000_000, a, &hello_datagram(&f.hello), &mut f.rng).unwrap() else {
            panic!()
        };
        let hb = f.hello.encode();
        assert_eq!(
            client_on_challenge(&f.eph, &f.hello, &hb, &ch, Some([1; 32]), &f.player, payload).err(),
            Some(HsError::ServerKeyMismatch)
        );
        let mut bad = ch.clone();
        bad[1] ^= 1;
        assert_eq!(
            client_on_challenge(&f.eph, &f.hello, &hb, &bad, None, &f.player, payload).err(),
            Some(HsError::EchoMismatch)
        );
    }

    #[test]
    fn auth_reject_is_sealed_and_bounded() {
        let s = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
        let dg = auth_reject_datagram(&s, reject_code::BANNED, &"x".repeat(2000), 300);
        assert!(dg.len() <= 300);
        let (code, text) = open_auth_reject(&s, &dg).unwrap();
        assert_eq!(code, reject_code::BANNED);
        assert!(!text.is_empty());
        let mut t = dg.clone();
        t[12] ^= 1;
        assert!(open_auth_reject(&s, &t).is_none(), "forged reject must not verify");
    }

    #[test]
    fn password_proof_is_bound_to_the_transcript() {
        let k = [5u8; 32];
        let p = password_proof(&k, &[1; 32]);
        assert!(verify_password_proof(&k, &[1; 32], &p));
        assert!(!verify_password_proof(&k, &[2; 32], &p));
        assert!(!verify_password_proof(&[6; 32], &[1; 32], &p));
        assert_eq!(player_fingerprint(&[0; 32]).len(), 16);
    }
}

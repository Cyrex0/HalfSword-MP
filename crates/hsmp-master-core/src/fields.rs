//! Request bodies and field validation. Every string the list stores is trimmed, stripped of
//! control and invisible formatting characters, and length-capped; numbers are range-checked.

use serde::{Deserialize, Serialize};

pub const MAX_NAME: usize = 48;
pub const MAX_MODE: usize = 32;
pub const MAX_MAP: usize = 64;
pub const MAX_REGION: usize = 16;
pub const MAX_VERSION: usize = 24;
pub const MAX_NONCE: usize = 64;
pub const MAX_PLAYERS_CAP: u32 = 256;
/// Largest request body anything accepts; real ones are a few hundred bytes.
pub const MAX_BODY_BYTES: usize = 4096;
/// Game ports below this are refused: a listing must not point players' UDP probes at a
/// well-known service on the registrant's address.
pub const MIN_PORT: u16 = 1024;

/// Unicode format characters (category Cf) that render as nothing or reorder text: zero-width
/// spaces and joiners, bidi controls, BOM, tag characters. They allow look-alike names.
fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
        | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Drop control and invisible characters, trim, and refuse (None) anything over `max` bytes
/// so the caller can answer 400 instead of silently truncating.
pub fn clean(s: &str, max: usize) -> Option<String> {
    let c: String = s.chars().filter(|c| !c.is_control() && !is_invisible(*c)).collect();
    let c = c.trim();
    (c.len() <= max).then(|| c.to_string())
}

/// Arena map names are plain identifiers (the game loads them with OpenLevel).
fn map_ok(m: &str) -> bool {
    m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn version_ok(v: &str) -> bool {
    v.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
}

/// The hex X25519 server identity, when given.
fn hex32_ok(k: &str) -> bool {
    k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn nonce_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= MAX_NONCE && n.bytes().all(|b| b.is_ascii_graphic())
}

fn default_max_players() -> u32 {
    8
}

/// `POST /v1/register`. `host` is accepted and ignored: the listed address is always the
/// request's source address.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegisterReq {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// No default: the port is the one thing the registrant chooses.
    pub port: u16,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub players: u32,
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    #[serde(default)]
    pub proto_ver: u32,
    #[serde(default)]
    pub proto_min: u32,
    #[serde(default)]
    pub proto_max: u32,
    #[serde(default)]
    pub server_key: String,
    #[serde(default, alias = "password_required")]
    pub pwd_protected: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub region: String,
    /// Mod content hash the server enforces (64 hex); empty = accepts any content.
    #[serde(default)]
    pub content_hash: String,
    /// Legacy replay token (hsmp-master before signing). Ignored when the request is signed.
    #[serde(default)]
    pub nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
    /// Hex Ed25519 listing key (auth.rs). Absent = unsigned legacy registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_key: Option<String>,
    /// Unix ms; must be newer than the last one accepted for this listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResp {
    pub server_id: String,
    pub ttl_s: u64,
    /// Legacy shared secret (unsigned registrations only); empty for signed ones.
    pub secret: String,
    /// How often this master wants a heartbeat. Absent from old masters (10 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_s: Option<u64>,
}

/// `POST /v1/heartbeat/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatReq {
    #[serde(default)]
    pub players: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub hmac: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
}

/// `DELETE /v1/servers/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteReq {
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub hmac: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
}

/// The validated, cleaned fields of a registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub name: String,
    pub port: u16,
    pub mode: String,
    pub map: String,
    pub players: u32,
    pub max_players: u32,
    pub proto_ver: u32,
    pub proto_min: u32,
    pub proto_max: u32,
    pub server_key: String,
    pub pwd_protected: bool,
    pub version: String,
    pub region: String,
    pub content_hash: String,
}

/// Validate a registration. `min_port` is MIN_PORT on a public list (1 keeps the old
/// hsmp-master behaviour for unsigned local registrations).
pub fn validate_register(r: &RegisterReq, min_port: u16) -> Result<Fields, &'static str> {
    if r.port == 0 || r.port < min_port {
        return Err("port missing or below 1024");
    }
    let name = clean(&r.name, MAX_NAME).filter(|n| !n.is_empty()).ok_or("name missing or too long")?;
    let mode = clean(&r.mode, MAX_MODE).ok_or("mode too long")?;
    let map = clean(&r.map, MAX_MAP).filter(|m| map_ok(m)).ok_or("map too long or not an identifier")?;
    let region = clean(&r.region, MAX_REGION).ok_or("region too long")?;
    let version = clean(&r.version, MAX_VERSION).filter(|v| version_ok(v)).ok_or("version too long or malformed")?;
    let server_key = r.server_key.trim().to_ascii_lowercase();
    if !server_key.is_empty() && !hex32_ok(&server_key) {
        return Err("server_key must be 64 hex chars");
    }
    let content_hash = r.content_hash.trim().to_ascii_lowercase();
    if !content_hash.is_empty() && !hex32_ok(&content_hash) {
        return Err("content_hash must be 64 hex chars or empty");
    }
    if r.proto_ver > u16::MAX as u32 || r.proto_min > u16::MAX as u32 || r.proto_max > u16::MAX as u32
        || (r.proto_max != 0 && r.proto_min > r.proto_max)
    {
        return Err("bad protocol range");
    }
    let max_players = r.max_players.clamp(1, MAX_PLAYERS_CAP);
    Ok(Fields {
        name,
        port: r.port,
        mode,
        map,
        players: r.players.min(max_players),
        max_players,
        proto_ver: r.proto_ver,
        proto_min: r.proto_min,
        proto_max: r.proto_max,
        server_key,
        pwd_protected: r.pwd_protected,
        version,
        region,
        content_hash,
    })
}

/// A heartbeat's optional map / mode: Some(cleaned) when present and valid, None to keep
/// the stored value.
pub fn heartbeat_map(m: Option<&str>) -> Option<String> {
    m.and_then(|m| clean(m, MAX_MAP)).filter(|m| map_ok(m))
}

pub fn heartbeat_mode(m: Option<&str>) -> Option<String> {
    m.and_then(|m| clean(m, MAX_MODE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> RegisterReq {
        RegisterReq {
            name: "EU Duels".into(), port: 7777, mode: "Best of 3".into(), map: "Map_Arena_Pit".into(),
            players: 2, max_players: 8, proto_ver: 5, proto_min: 5, proto_max: 5,
            server_key: "AB".repeat(32), version: "0.1.0-beta.1".into(), region: "EU".into(),
            ..Default::default()
        }
    }

    #[test]
    fn clean_strips_controls_and_invisible_characters() {
        assert_eq!(clean("  Tab\tName\n ", 48).as_deref(), Some("TabName"));
        assert_eq!(clean("pay\u{200B}pal\u{202E}", 48).as_deref(), Some("paypal"));
        assert_eq!(clean("\u{FEFF}x\u{E0041}", 48).as_deref(), Some("x"));
        assert_eq!(clean("Kämpfer", 48).as_deref(), Some("Kämpfer"));
        assert_eq!(clean(&"x".repeat(49), 48), None);
        // the cap is in bytes: 24 two-byte characters fit 48
        assert!(clean(&"ä".repeat(24), 48).is_some());
        assert!(clean(&"ä".repeat(25), 48).is_none());
    }

    #[test]
    fn a_good_registration_is_cleaned_and_clamped() {
        let mut r = req();
        r.players = 99;
        let f = validate_register(&r, MIN_PORT).unwrap();
        assert_eq!(f.players, 8);
        assert_eq!(f.server_key, "ab".repeat(32));
        r.max_players = 100_000;
        assert_eq!(validate_register(&r, MIN_PORT).unwrap().max_players, MAX_PLAYERS_CAP);
        r.max_players = 0;
        assert_eq!(validate_register(&r, MIN_PORT).unwrap().max_players, 1);
    }

    #[test]
    fn every_bad_field_is_refused() {
        let bad: Vec<(&str, Box<dyn Fn(&mut RegisterReq)>)> = vec![
            ("port 0", Box::new(|r| r.port = 0)),
            ("port 53", Box::new(|r| r.port = 53)),
            ("blank name", Box::new(|r| r.name = " \t\u{200B} ".into())),
            ("long name", Box::new(|r| r.name = "n".repeat(MAX_NAME + 1))),
            ("long mode", Box::new(|r| r.mode = "m".repeat(MAX_MODE + 1))),
            ("map with a path", Box::new(|r| r.map = "../Map".into())),
            ("map with spaces", Box::new(|r| r.map = "Map Arena".into())),
            ("long region", Box::new(|r| r.region = "r".repeat(MAX_REGION + 1))),
            ("version with spaces", Box::new(|r| r.version = "1 0".into())),
            ("short server_key", Box::new(|r| r.server_key = "ab".into())),
            ("non-hex server_key", Box::new(|r| r.server_key = "zz".repeat(32))),
            ("proto range", Box::new(|r| { r.proto_min = 6; r.proto_max = 5; })),
            ("proto huge", Box::new(|r| r.proto_ver = 70_000)),
        ];
        for (what, f) in bad {
            let mut r = req();
            f(&mut r);
            assert!(validate_register(&r, MIN_PORT).is_err(), "{what} accepted");
        }
        // the legacy local master still takes any non-zero port
        let mut r = req();
        r.port = 80;
        assert!(validate_register(&r, 1).is_ok());
    }

    #[test]
    fn json_shapes_match_the_existing_protocol() {
        // what hsmp-server before signing and e2e-test.sh send
        let r: RegisterReq = serde_json::from_str(r#"{"name":"E2E Server","mode":"duel","host":"","port":7777,"players":1,"max_players":4,"password_required":false,"nonce":"e2e","hmac":""}"#).unwrap();
        assert_eq!(r.port, 7777);
        assert!(r.listing_key.is_none() && r.ts.is_none());
        // fields a newer server adds are accepted (and not stored)
        assert!(serde_json::from_str::<RegisterReq>(r#"{"name":"x","port":7777,"protocol":7,"content_hash":"ab","future":{"a":1}}"#).is_ok());
        // no port is a parse error, not a default
        assert!(serde_json::from_str::<RegisterReq>(r#"{"name":"x"}"#).is_err());
        assert!(serde_json::from_str::<RegisterReq>(r#"{"name":"x","port":70000}"#).is_err());
        // an old master's answer has no heartbeat_s
        let resp: RegisterResp = serde_json::from_str(r#"{"server_id":"a","ttl_s":30,"secret":"s"}"#).unwrap();
        assert_eq!(resp.heartbeat_s, None);
        let h: HeartbeatReq = serde_json::from_str(r#"{"players":2,"nonce":"n","hmac":"h"}"#).unwrap();
        assert_eq!(h.ts, None);
    }

    #[test]
    fn heartbeat_fields() {
        assert_eq!(heartbeat_map(Some("Map_Arena_Yard")).as_deref(), Some("Map_Arena_Yard"));
        assert_eq!(heartbeat_map(Some("bad map")), None);
        assert_eq!(heartbeat_mode(Some(" Best of 5 ")).as_deref(), Some("Best of 5"));
        assert_eq!(heartbeat_mode(None), None);
        assert!(nonce_ok("abc") && !nonce_ok("") && !nonce_ok(&"n".repeat(65)) && !nonce_ok("a b"));
    }
}

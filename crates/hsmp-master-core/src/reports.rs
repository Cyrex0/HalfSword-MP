//! Bug-report uploads (`POST /v1/reports` on the Cloudflare Worker): the format check both
//! sides share. The launcher writes the zip, the Worker refuses anything else.
//!
//! A report is a zip whose FIRST entry is `hsmp-report.json`, stored (not compressed), with
//! its sizes in the local header, holding `{"magic":"hsmp-report","format":1,...}`. The
//! Worker reads that header and the end-of-central-directory record only; it never
//! decompresses anything.

use serde::Deserialize;

pub const MANIFEST_NAME: &str = "hsmp-report.json";
pub const MAGIC: &str = "hsmp-report";
pub const FORMAT: u32 = 1;
/// The largest report accepted (and the launcher's cap).
pub const MAX_REPORT_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ManifestHead {
    pub magic: String,
    pub format: u32,
    #[serde(default)]
    pub launcher: String,
    #[serde(default)]
    pub hsmp: String,
    #[serde(default)]
    pub sessions: Vec<String>,
    /// "client" (the launcher) or "server" (`hsmp-server --report`).
    #[serde(default = "client")]
    pub kind: String,
}

fn client() -> String {
    "client".into()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub head: ManifestHead,
    pub entries: usize,
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
}
fn u32_at(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}

/// Is `body` a launcher report? `Err` is the reason (sent back as the 400 body).
pub fn check(body: &[u8]) -> Result<Checked, &'static str> {
    if body.len() > MAX_REPORT_BYTES {
        return Err("report too large");
    }
    if body.len() < 30 + MANIFEST_NAME.len() + 22 || &body[..4] != b"PK\x03\x04" {
        return Err("not a zip");
    }
    let flags = u16_at(body, 6).ok_or("truncated")?;
    let method = u16_at(body, 8).ok_or("truncated")?;
    let csize = u32_at(body, 18).ok_or("truncated")?;
    let usize_ = u32_at(body, 22).ok_or("truncated")?;
    let name_len = u16_at(body, 26).ok_or("truncated")?;
    let extra_len = u16_at(body, 28).ok_or("truncated")?;
    if body.get(30..30 + name_len) != Some(MANIFEST_NAME.as_bytes()) {
        return Err("the first entry is not hsmp-report.json");
    }
    if flags & 0x0009 != 0 || method != 0 || csize != usize_ || csize == 0 || csize > MAX_MANIFEST_BYTES {
        return Err("hsmp-report.json must be stored, unencrypted, with its size in the header");
    }
    let start = 30 + name_len + extra_len;
    let json = body.get(start..start + csize).ok_or("truncated")?;
    let head: ManifestHead = serde_json::from_slice(json).map_err(|_| "hsmp-report.json is not valid")?;
    if head.magic != MAGIC || head.format != FORMAT {
        return Err("not an HSMP report (magic / format)");
    }
    // end of central directory: in the last 22 + 65535 bytes
    let tail_from = body.len().saturating_sub(22 + 65_535);
    let eocd = (tail_from..=body.len() - 22).rev().find(|&i| &body[i..i + 4] == b"PK\x05\x06").ok_or("no zip directory")?;
    let entries = u16_at(body, eocd + 10).ok_or("truncated")?;
    let cd_size = u32_at(body, eocd + 12).ok_or("truncated")?;
    let cd_off = u32_at(body, eocd + 16).ok_or("truncated")?;
    if entries == 0 || entries > MAX_ENTRIES || cd_off.checked_add(cd_size).is_none_or(|e| e > eocd) || body.get(cd_off..cd_off + 4) != Some(b"PK\x01\x02") {
        return Err("bad zip directory");
    }
    Ok(Checked { head, entries })
}

/// `20261004-<32 hex>`: the day (UTC) and 16 random bytes.
pub fn is_report_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 41 && b[..8].iter().all(u8::is_ascii_digit) && b[8] == b'-' && b[9..].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// The R2 key of a report id (`r/<day>/<hex>.zip`).
pub fn object_key(id: &str) -> Option<String> {
    is_report_id(id).then(|| format!("r/{}/{}.zip", &id[..8], &id[9..]))
}

/// The id back from an R2 key.
pub fn id_of_key(key: &str) -> Option<String> {
    let rest = key.strip_prefix("r/")?.strip_suffix(".zip")?;
    let (day, hex) = rest.split_once('/')?;
    let id = format!("{day}-{hex}");
    is_report_id(&id).then_some(id)
}

/// `20261004` (UTC) for a Unix time in ms.
pub fn day_of(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

/// What a stored report keeps of the uploader's address: a per-day hash of it (an IPv6 /64),
/// enough to count uploads per address and day, not to list addresses.
pub fn ip_tag(day: &str, ip: std::net::IpAddr) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("hsmp-report|{day}|{}", crate::ip::host_key(ip)).as_bytes());
    hex::encode(&h[..8])
}

/// Constant-time comparison (admin token).
pub fn same_secret(a: &str, b: &str) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A minimal stored zip of `entries` in order (the CRCs are not set): what the e2e test and the
/// unit tests feed the format check (`hsmp-tools report-fixture`).
pub fn fixture_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![];
    let mut cd = vec![];
    for (name, data) in entries {
        let off = out.len() as u32;
        let mut h = vec![];
        h.extend_from_slice(b"PK\x03\x04");
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // flags
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&[0; 4]); // time, date
        h.extend_from_slice(&[0; 4]); // crc (not checked)
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        cd.extend_from_slice(b"PK\x01\x02");
        let at = cd.len();
        cd.extend_from_slice(&[0; 42]);
        cd[at + 24..at + 26].copy_from_slice(&(name.len() as u16).to_le_bytes());
        cd[at + 38..at + 42].copy_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fixture_zip as zip_of;

    const GOOD: &[u8] = br#"{"magic":"hsmp-report","format":1,"launcher":"0.1.0","hsmp":"0.1.0","sessions":["20261004_101500_p1"]}"#;

    #[test]
    fn accepts_a_report_and_rejects_everything_else() {
        let ok = check(&zip_of(&[(MANIFEST_NAME, GOOD), ("system.json", b"{}")])).unwrap();
        assert_eq!(ok.head.sessions, vec!["20261004_101500_p1".to_string()]);
        assert_eq!(ok.entries, 2);
        assert_eq!(check(b"hello world, this is not a zip at all, not even close"), Err("not a zip"));
        assert_eq!(check(&zip_of(&[("other.txt", GOOD)])), Err("the first entry is not hsmp-report.json"));
        assert_eq!(check(&zip_of(&[(MANIFEST_NAME, br#"{"magic":"nope","format":1}"#)])), Err("not an HSMP report (magic / format)"));
        assert_eq!(check(&zip_of(&[(MANIFEST_NAME, br#"{"magic":"hsmp-report","format":2}"#)])), Err("not an HSMP report (magic / format)"));
        assert_eq!(check(&zip_of(&[(MANIFEST_NAME, b"not json")])), Err("hsmp-report.json is not valid"));
        // compressed manifest
        let mut z = zip_of(&[(MANIFEST_NAME, GOOD)]);
        z[8] = 8;
        assert!(check(&z).unwrap_err().contains("stored"));
        // no directory at the end
        let z = zip_of(&[(MANIFEST_NAME, GOOD)]);
        assert_eq!(check(&z[..z.len() - 22]), Err("no zip directory"));
        // too large
        let big = vec![0u8; MAX_REPORT_BYTES + 1];
        assert_eq!(check(&big), Err("report too large"));
    }

    #[test]
    fn ids_and_keys() {
        let id = format!("20261004-{}", "0123456789abcdef".repeat(2));
        assert!(is_report_id(&id));
        assert_eq!(object_key(&id).unwrap(), format!("r/20261004/{}.zip", "0123456789abcdef".repeat(2)));
        assert_eq!(id_of_key(&object_key(&id).unwrap()).unwrap(), id);
        for bad in ["", "20261004-XYZ", "../../etc", &id.to_uppercase(), &format!("{id}0")] {
            assert!(!is_report_id(bad), "{bad}");
        }
        assert!(id_of_key("r/20261004/../x.zip").is_none());
        assert!(same_secret("abc", "abc") && !same_secret("abc", "abd") && !same_secret("", "") && !same_secret("a", "ab"));
    }
}

#[cfg(test)]
mod day_tests {
    use super::*;

    #[test]
    fn days_and_ip_tags() {
        assert_eq!(day_of(1_790_899_200_000), "20261002");
        assert_eq!(day_of(951_782_400_000), "20000229");
        let a: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let b: std::net::IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b2: std::net::IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        assert_eq!(ip_tag("20261002", a).len(), 16);
        assert_eq!(ip_tag("20261002", a), ip_tag("20261002", a));
        assert_ne!(ip_tag("20261002", a), ip_tag("20261003", a), "a new tag every day");
        assert_eq!(ip_tag("20261002", b), ip_tag("20261002", b2), "one IPv6 /64 is one uploader");
    }
}

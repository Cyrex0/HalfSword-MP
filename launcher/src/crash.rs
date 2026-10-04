//! Crash notices and redaction. This file
//!
//! 1. notices NEW crash folders under `Saved\Crashes` since the last run (the launcher's
//!    Bug report section says so and ticks the run that crashed),
//! 2. keeps the player's choice (`Consent`: tell me / don't) and the folders already seen,
//! 3. re-exports the `Redactor` (crates/hsmp-diag) and keeps `redact()`, its all-IPs form,
//!    and the older single-crash zip (`save_local_report`).
//!
//! The upload half is the `CrashSink` trait; report.rs implements it (`MasterSink`, the
//! master's `/v1/reports`), used only on the player's explicit click. Nothing in this file
//! talks to the network.
//!
//! Consent file: %LOCALAPPDATA%\HSMP\consent.json
//! `{ "crash_reports": "ask" | "never" | "always", "seen": ["UECC-...", ...] }`

use crate::util;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    /// Ask after each new crash (default).
    #[default]
    Ask,
    /// Never ask, never prepare reports.
    Never,
    /// Prepare (and, once an uploader exists, send) without asking.
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ConsentFile {
    pub crash_reports: Consent,
    /// Crash folder names already handled (shown or dismissed).
    pub seen: Vec<String>,
}

pub fn load(path: &Path) -> ConsentFile {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, c: &ConsentFile) -> Result<(), String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    util::atomic_write(path, &serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Crash {
    pub name: String,
    pub dir: PathBuf,
}

/// Crash folders not yet in `seen`, oldest name first.
pub fn new_crashes(crash_roots: &[PathBuf], c: &ConsentFile) -> Vec<Crash> {
    let mut out = vec![];
    for root in crash_roots {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() && !c.seen.contains(&name) && name.starts_with("UECC-") {
                    out.push(Crash { name, dir: e.path() });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub use hsmp_diag::redact::{IpMode, Redactor};

/// Remove player-identifying data from log text (every IP address, player names, the user
/// name, secrets). The crash-report path; bug reports use a [`Redactor`] with
/// [`IpMode::Public`].
pub fn redact(text: &str, user_name: Option<&str>) -> String {
    let r = Redactor { user: user_name.map(String::from), ip: Some(IpMode::All), ..Default::default() };
    r.redact(text)
}

/// Where uploads go (report::MasterSink: `<master>/v1/reports`).
pub trait CrashSink {
    fn submit(&self, report_zip: &[u8], meta: &serde_json::Value) -> Result<String, String>;
}

/// Build a redacted report zip and save it to `out_dir` (local only).
pub fn save_local_report(crash: &Crash, ue4ss_log: Option<&Path>, meta: &serde_json::Value, out_dir: &Path) -> Result<PathBuf, String> {
    use std::io::Write;
    let user = std::env::var("USERNAME").ok();
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let out = out_dir.join(format!("{}.zip", crash.name));
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut cur);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for e in std::fs::read_dir(&crash.dir).map_err(|e| e.to_string())?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            let bytes = std::fs::read(e.path()).map_err(|e| e.to_string())?;
            let data = if lower.ends_with(".dmp") {
                bytes
            } else if lower.ends_with(".log") || lower.ends_with(".xml") || lower.ends_with(".txt") {
                redact(&String::from_utf8_lossy(&bytes), user.as_deref()).into_bytes()
            } else {
                continue;
            };
            z.start_file(name, opts).map_err(|e| e.to_string())?;
            z.write_all(&data).map_err(|e| e.to_string())?;
        }
        if let Some(log) = ue4ss_log {
            if let Ok(t) = std::fs::read_to_string(log) {
                let lines: Vec<&str> = t.lines().collect();
                let tail = lines[lines.len().saturating_sub(2000)..].join("\n");
                z.start_file("UE4SS.tail.log", opts).map_err(|e| e.to_string())?;
                z.write_all(redact(&tail, user.as_deref()).as_bytes()).map_err(|e| e.to_string())?;
            }
        }
        z.start_file("hsmp-meta.json", opts).map_err(|e| e.to_string())?;
        z.write_all(&serde_json::to_vec_pretty(meta).unwrap_or_default()).map_err(|e| e.to_string())?;
        z.finish().map_err(|e| e.to_string())?;
    }
    util::atomic_write(&out, &cur.into_inner()).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction() {
        let t = "join nick=SirLancelot from 203.0.113.7:7777 ok\n{\"nick\":\"Bob\",\"x\":1}\nversion 5.4.4.2705 build 1.2.3.4.5\nC:\\Users\\alice\\AppData\\x name=Ann";
        let r = redact(t, Some("alice"));
        assert!(!r.contains("SirLancelot") && !r.contains("Bob") && !r.contains("Ann"), "{r}");
        assert!(!r.contains("203.0.113.7") && !r.contains("alice"), "{r}");
        assert!(r.contains("nick=<redacted> from <ip> ok"), "{r}");
        assert!(r.contains("5.4.4.2705"), "version numbers are not IPs: {r}");
        assert!(r.contains("1.2.3.4.5"), "five groups is not an IPv4: {r}");
        assert!(r.contains("\\Users\\<user>\\"), "{r}");
    }

    fn bug_report_redactor() -> Redactor {
        Redactor {
            user: Some("alice".into()),
            home: Some("C:\\Users\\alice".into()),
            computer: Some("ALICE-PC".into()),
            secrets: vec!["ab".repeat(32)],
            ..Default::default()
        }
        .with_ips(IpMode::Public, vec!["198.51.100.9".into()])
    }

    /// Bug reports: user names and home paths (every slash style), e-mails, keys, passwords
    /// and tokens go; peer IPs are numbered consistently, LAN / loopback addresses and version
    /// numbers stay, the player's own address is always removed.
    #[test]
    fn report_redaction_of_paths_keys_and_ips() {
        let r = bug_report_redactor();
        let t = [
            r"C:\Users\alice\AppData\Local\HSMP\logs",
            "C:/Users/alice/AppData/Local",
            r#"{"path":"C:\\Users\\alice\\x","nick":"Zed"}"#,
            r"D:\Users\bob\Documents (another account)",
            "machine ALICE-PC user alice is_admin=true",
            "mail me: alice.smith+hs@example.co.uk",
            "player key abababababababababababababababababababababababababababababababab",
            "server_key=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef content_hash=feedfeed",
            r#"args=Args { server: "203.0.113.7:7777", nick: "Bob", server_key: Some("c0ffee"), rcon_password: Some("hunter2") }"#,
            "--rcon-password cfpw --rcon-bind 127.0.0.1:27015",
            r#"{"admin_token":"t0k3n","password":"pw","x-hsmp-sig":"s"}"#,
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
            "JOIN: 203.0.113.7:7777 then 203.0.113.8:7777 then 203.0.113.7:7778",
            "lan 192.168.1.20:7777 loop 127.0.0.1 cgnat 100.70.1.2 my 198.51.100.9:5000",
            "v6 [2001:db8:85a3::8a2e:370:7334]:7777 lo [::1]:7777 ll fe80::1%12 time 12:34:56 path std::fs::write",
            "UE 5.4.4.2705 driver 32.0.15.6094 build 10.0.26100.1",
            "steam 76561198012345678",
        ]
        .join("\n");
        let out = r.redact(&t);
        for gone in ["alice", "ALICE-PC", "bob", "Zed", "Bob", "example.co.uk", "abababab", "0123456789abcdef", "c0ffee", "hunter2", "cfpw", "t0k3n", "\"pw\"", "eyJhbGci", "203.0.113", "198.51.100.9", "2001:db8", "76561198012345678"] {
            assert!(!out.contains(gone), "{gone} survived:\n{out}");
        }
        for kept in ["%USERPROFILE%\\AppData\\Local\\HSMP\\logs", "%USERPROFILE%/AppData/Local", "\\Users\\<user>\\Documents", "is_admin=true", "<email>", "content_hash=feedfeed",
            "192.168.1.20:7777", "127.0.0.1", "100.70.1.2", "<my-ip>:5000", "[::1]:7777", "fe80::1%12", "12:34:56", "std::fs::write", "5.4.4.2705", "32.0.15.6094", "10.0.26100.1", "<steam-id>", "--rcon-bind 127.0.0.1:27015"] {
            assert!(out.contains(kept), "{kept} missing:\n{out}");
        }
        assert!(out.contains("JOIN: <ip-1>:7777 then <ip-2>:7777 then <ip-1>:7778"), "{out}");
        assert!(out.contains("[<ip-3>]:7777"), "{out}");
        // OwnOnly: peers stay, the own address still goes
        let keep = Redactor::default().with_ips(IpMode::OwnOnly, vec!["198.51.100.9".into()]);
        let o = keep.redact("peer 203.0.113.7:7777 me 198.51.100.9");
        assert_eq!(o, "peer 203.0.113.7:7777 me <my-ip>");
    }

    #[test]
    fn redaction_of_non_ascii_text_never_panics() {
        let t = "ni漢字 nick=騎士 na日本 \"nick\":\"甲冑\" 名前 C:\\Users\\山田\\x 10.0.0.1";
        let r = redact(t, Some("山田"));
        assert!(!r.contains("騎士") && !r.contains("甲冑") && !r.contains("山田") && !r.contains("10.0.0.1"), "{r}");
        assert!(r.contains("ni漢字") && r.contains("名前"), "{r}");
    }

    #[test]
    fn new_crash_detection_and_consent_file() {
        let t = crate::testutil::TempDir::new("crash");
        let root = t.path().join("Crashes");
        std::fs::create_dir_all(root.join("UECC-Windows-AAA_0000")).unwrap();
        std::fs::create_dir_all(root.join("UECC-Windows-BBB_0000")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        let path = t.path().join("consent.json");
        let mut c = load(&path);
        assert_eq!(c.crash_reports, Consent::Ask);
        let n = new_crashes(&[root.clone(), t.path().join("missing")], &c);
        assert_eq!(n.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["UECC-Windows-AAA_0000", "UECC-Windows-BBB_0000"]);
        c.seen.push("UECC-Windows-AAA_0000".into());
        c.crash_reports = Consent::Never;
        save(&path, &c).unwrap();
        let c2 = load(&path);
        assert_eq!(c2, c);
        assert_eq!(new_crashes(std::slice::from_ref(&root), &c2).len(), 1);
        // local report
        std::fs::write(root.join("UECC-Windows-BBB_0000/UEMinidump.dmp"), b"MDMP").unwrap();
        std::fs::write(root.join("UECC-Windows-BBB_0000/HalfswordUE5.log"), b"nick=Zed 10.0.0.2").unwrap();
        let out = save_local_report(&n[1], None, &serde_json::json!({"v": 1}), &t.path().join("reports")).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(out).unwrap()).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("HalfswordUE5.log").unwrap(), &mut s).unwrap();
        assert_eq!(s, "nick=<redacted> <ip>");
    }
}

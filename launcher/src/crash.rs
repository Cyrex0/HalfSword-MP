//! Crash-report consent. Interface only:
//! this build makes no network calls. It
//!
//! 1. notices NEW crash folders under `Saved\Crashes` since the last run,
//! 2. asks once per crash, as the player chose (`Consent`),
//! 3. can build a REDACTED report and save it locally, so the player can
//!    attach it to a bug report themselves.
//!
//! The upload half is the `CrashSink` trait. A future `/v1/crash` uploader on
//! the HSMP master implements it; the launcher must only ever send to the
//! master URL from the signed manifest, and only with `Consent::Always` or an
//! explicit per-crash "Send". Nothing in this file talks to the network.
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

/// Remove player-identifying data from log text: `nick=...` values,
/// IPv4 addresses (with or without port) and the Windows user name in paths.
pub fn redact(text: &str, user_name: Option<&str>) -> String {
    let mut s = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // nick=<value> / "nick":"<value>"
        let rest = &text[i..];
        // `get` (not `[..n]`): byte n may fall inside a non-ASCII character
        let lower_starts = |p: &str| rest.get(..p.len()).is_some_and(|x| x.eq_ignore_ascii_case(p));
        if lower_starts("nick=") || lower_starts("\"nick\":\"") || lower_starts("name=") {
            let key_len = if lower_starts("nick=") || lower_starts("name=") { 5 } else { 8 };
            s.push_str(&rest[..key_len]);
            s.push_str("<redacted>");
            let mut j = i + key_len;
            while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\r' | b'\n' | b',' | b'"' | b'}' | b';') {
                j += 1;
            }
            i = j;
            continue;
        }
        // IPv4: four dot-separated 1-3 digit groups (optional :port)
        if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.')) {
            let mut j = i;
            let mut groups = 0;
            loop {
                let start = j;
                while j < b.len() && b[j].is_ascii_digit() && j - start < 3 {
                    j += 1;
                }
                if j == start {
                    break;
                }
                groups += 1;
                if groups == 4 || j >= b.len() || b[j] != b'.' {
                    break;
                }
                j += 1;
            }
            if groups == 4 && (j >= b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'.')) {
                if j < b.len() && b[j] == b':' {
                    let mut k = j + 1;
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k > j + 1 {
                        j = k;
                    }
                }
                s.push_str("<ip>");
                i = j;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        s.push(ch);
        i += ch.len_utf8();
    }
    match user_name.filter(|u| u.len() >= 2) {
        Some(u) => s.replace(&format!("\\Users\\{u}\\"), "\\Users\\<user>\\").replace(&format!("/Users/{u}/"), "/Users/<user>/"),
        None => s,
    }
}

/// What a report would contain (shown to the player BEFORE anything happens).
pub fn describe_contents() -> &'static [&'static str] {
    &[
        "the crash dump (UEMinidump.dmp: a snapshot of the game's memory stack, no screenshots)",
        "the crash context (CrashContext.runtime-xml) and the game log, redacted",
        "the last 2000 lines of UE4SS.log, redacted (player names and IP addresses removed)",
        "the HSMP version, protocol and the game build hash",
        "never: chat logs, .settings.json, your HSMP profile or your career save",
    ]
}

/// Where uploads would go. Not implemented in this build (no endpoint).
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

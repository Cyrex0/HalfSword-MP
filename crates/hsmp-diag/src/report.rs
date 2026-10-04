//! The bug-report zip both the launcher ("Create bug report") and `hsmp-server --report`
//! write: the entries exactly as they will be sent (text redacted, dumps unchanged), then
//! `hsmp-report.json` first in the zip (stored; every file's size and SHA-256), within the
//! upload cap. The Worker checks the format (hsmp_master_core::reports, the same constants).

use crate::redact::Redactor;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MANIFEST_NAME: &str = "hsmp-report.json";
pub const MAGIC: &str = "hsmp-report";
pub const FORMAT: u32 = 1;
/// The upload cap (the Worker refuses more).
pub const MAX_REPORT_BYTES: usize = 25 * 1024 * 1024;
/// Text files are redacted; these extensions are text.
pub const TEXT_EXT: &[&str] = &["log", "txt", "json", "jsonl", "xml", "runtime-xml", "ini", "csv"];

/// One file of the report, exactly as it goes into the zip.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Text that went through the redactor (else a binary, unchanged).
    pub redacted: bool,
}

impl Entry {
    pub fn is_text(&self) -> bool {
        self.redacted
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    /// "client" (the launcher) or "server" (`hsmp-server --report`).
    pub kind: String,
    /// The producing program and version ("hsmp-launcher 0.1.0").
    pub producer: String,
    pub entries: Vec<Entry>,
    /// What was left out and why (shown, and listed in the manifest).
    pub left_out: Vec<String>,
    pub sessions: Vec<String>,
    pub hsmp_version: String,
    pub options: Option<(bool, bool)>,
}

pub fn is_text(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    TEXT_EXT.iter().any(|e| l.ends_with(&format!(".{e}")))
}

/// Every file under `dir`, as (forward-slash path relative to `base`, path), sorted.
pub fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<_> = rd.flatten().collect();
    v.sort_by_key(|e| e.file_name());
    for e in v {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push((rel.to_string_lossy().replace('\\', "/"), p));
        }
    }
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}

impl Report {
    pub fn new(kind: &str, producer: &str) -> Report {
        Report { kind: kind.into(), producer: producer.into(), ..Default::default() }
    }

    /// A redacted text entry.
    pub fn text(&mut self, r: &Redactor, name: impl Into<String>, s: &str) {
        self.entries.push(Entry { name: name.into(), bytes: r.redact(s).into_bytes(), redacted: true });
    }

    /// Every file of `dir` under `prefix/`: text redacted, crash dumps as they are (when
    /// `dumps`), anything else left out (and said so). `filter` picks the files to take.
    pub fn add_dir(&mut self, r: &Redactor, dir: &Path, prefix: &str, dumps: bool, filter: &dyn Fn(&str) -> bool) {
        let mut files = vec![];
        walk(dir, dir, &mut files);
        for (rel, path) in files {
            if !filter(&rel) {
                continue;
            }
            let name = format!("{prefix}/{rel}");
            if is_text(&rel) {
                match std::fs::read(&path) {
                    Ok(b) => self.text(r, name, &String::from_utf8_lossy(&b)),
                    Err(e) => self.left_out.push(format!("{name}: {e}")),
                }
            } else if rel.to_ascii_lowercase().ends_with(".dmp") {
                if !dumps {
                    self.left_out.push(format!("{name}: crash dumps not included (option)"));
                    continue;
                }
                match std::fs::read(&path) {
                    Ok(b) => self.entries.push(Entry { name, bytes: b, redacted: false }),
                    Err(e) => self.left_out.push(format!("{name}: {e}")),
                }
            } else {
                self.left_out.push(format!("{name}: not a log or a dump"));
            }
        }
    }

    pub fn manifest(&self) -> Value {
        json!({
            "magic": MAGIC,
            "format": FORMAT,
            "kind": self.kind,
            "created_utc": crate::time::iso(crate::time::now_ms()),
            "launcher": self.producer,
            "hsmp": self.hsmp_version,
            "sessions": self.sessions,
            "options": self.options.map(|(ips, dumps)| json!({"hide_ips": ips, "include_dumps": dumps})),
            "files": self.entries.iter().map(|e| json!({"name": e.name, "size": e.bytes.len(), "sha256": sha256_hex(&e.bytes), "redacted": e.redacted})).collect::<Vec<_>>(),
            "left_out": self.left_out,
        })
    }

    pub fn raw_bytes(&self) -> usize {
        self.entries.iter().map(|e| e.bytes.len()).sum()
    }

    /// The zip: `hsmp-report.json` first (stored), then every entry (deflated).
    pub fn zip(&self) -> Result<Vec<u8>, String> {
        let mut cur = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut cur);
            let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file(MANIFEST_NAME, stored).map_err(|e| e.to_string())?;
            z.write_all(&serde_json::to_vec_pretty(&self.manifest()).unwrap_or_default()).map_err(|e| e.to_string())?;
            for e in &self.entries {
                z.start_file(e.name.as_str(), deflated).map_err(|e| e.to_string())?;
                z.write_all(&e.bytes).map_err(|e| e.to_string())?;
            }
            z.finish().map_err(|e| e.to_string())?;
        }
        Ok(cur.into_inner())
    }

    /// The zip within `cap`: crash dumps go first (largest first), then the largest text
    /// files keep only their end. Every cut is listed in `left_out`.
    pub fn zip_capped(&mut self, cap: usize) -> Result<Vec<u8>, String> {
        for _ in 0..40 {
            let z = self.zip()?;
            if z.len() <= cap {
                return Ok(z);
            }
            if let Some(i) = self.entries.iter().enumerate().filter(|(_, e)| !e.redacted).max_by_key(|(_, e)| e.bytes.len()).map(|(i, _)| i) {
                let e = self.entries.remove(i);
                self.left_out.push(format!("{}: left out to stay under {} MB", e.name, cap >> 20));
                continue;
            }
            let Some(e) = self.entries.iter_mut().max_by_key(|e| e.bytes.len()) else { break };
            let keep = e.bytes.len() / 2;
            if keep < 4096 {
                break;
            }
            let cut = e.bytes.len() - keep;
            e.bytes.drain(..cut);
            self.left_out.push(format!("{}: first {cut} bytes cut to stay under {} MB", e.name, cap >> 20));
        }
        let z = self.zip()?;
        if z.len() <= cap {
            Ok(z)
        } else {
            Err(format!("the report is still {} MB after cutting", z.len() >> 20))
        }
    }
}

/// The last `n` lines of a text.
pub fn tail_lines(t: &str, n: usize) -> String {
    let lines: Vec<&str> = t.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server report: the Worker's check accepts it and reads its kind; the constants agree.
    #[test]
    fn server_report_passes_the_upload_check() {
        assert_eq!((MAGIC, FORMAT, MANIFEST_NAME, MAX_REPORT_BYTES), (hsmp_master_core::reports::MAGIC, hsmp_master_core::reports::FORMAT, hsmp_master_core::reports::MANIFEST_NAME, hsmp_master_core::reports::MAX_REPORT_BYTES));
        let r = Redactor::default().with_ips(crate::redact::IpMode::Public, vec![]);
        let mut rep = Report::new("server", "hsmp-server 0.1.0");
        rep.text(&r, "logs/server-20261004.log", "peer joined from 203.0.113.7:5000 rcon_password=hunter2\n");
        let z = rep.zip_capped(MAX_REPORT_BYTES).unwrap();
        let c = hsmp_master_core::reports::check(&z).unwrap();
        assert_eq!(c.head.kind, "server");
        let t = String::from_utf8(rep.entries[0].bytes.clone()).unwrap();
        assert_eq!(t, "peer joined from <ip-1>:5000 rcon_password=<redacted>\n");
    }
}

//! `hsmp-server --report`: a bug-report zip of a dedicated server's recent logs, in the same
//! format as the launcher's (hsmp_diag::report, kind "server"): `system.json` (OS, this
//! build's identity), the log and events files of the last `--report-days` days, redacted
//! (user names and home paths, keys, passwords, tokens; player IP addresses numbered unless
//! `--report-keep-ips`; the server's own public address always removed). Saved next to the
//! logs, and with `--report-upload` sent to the master's `/v1/reports`.

use anyhow::{bail, Context, Result};
use hsmp_diag::redact::{IpMode, Redactor};
use hsmp_diag::report::{Report, MAX_REPORT_BYTES};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Opts {
    pub out: Option<PathBuf>,
    pub upload: bool,
    pub keep_ips: bool,
    pub days: u64,
}

const DEFAULT_MASTER: &str = "https://master.halfswordmp.workers.dev";

/// The first https master of HSMP_MASTER_URL, else the public one.
fn master() -> String {
    std::env::var("HSMP_MASTER_URL")
        .ok()
        .and_then(|v| v.split(',').map(str::trim).find(|u| u.starts_with("https://")).map(String::from))
        .unwrap_or_else(|| DEFAULT_MASTER.into())
        .trim_end_matches('/')
        .to_string()
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(120)).https_only(true).build().unwrap_or_default()
}

async fn own_ip(master: &str) -> Option<String> {
    let v: serde_json::Value = http().get(format!("{master}/v1/myaddr")).timeout(Duration::from_secs(5)).send().await.ok()?.json().await.ok()?;
    v["ip"].as_str().map(String::from)
}

/// Log files of the last `days` days (by modification time), newest included.
pub fn recent_logs(dir: &Path, days: u64, now_ms: u64) -> Vec<String> {
    let cutoff = now_ms.saturating_sub(days.max(1) * 86_400_000);
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_file())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    let ok = (n.ends_with(".log") || n.ends_with(".jsonl")) && hsmp_diag::time::mtime_ms(&e.path()).is_some_and(|t| t >= cutoff);
                    ok.then_some(n)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn build(dir: &Path, o: &Opts, own: Vec<String>) -> Report {
    let mut r = Redactor::system().with_ips(if o.keep_ips { IpMode::OwnOnly } else { IpMode::Public }, own);
    if r.home.is_none() {
        r.home = std::env::var("HOME").ok().filter(|h| h.len() > 1);
    }
    let mut rep = Report::new("server", &format!("hsmp-server {}", crate::build_id::VERSION));
    rep.hsmp_version = crate::build_id::VERSION.to_string();
    rep.options = Some((!o.keep_ips, false));
    let system = serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "build": serde_json::from_str::<serde_json::Value>(&crate::build_id::identity().to_json()).unwrap_or_default(),
        "in_docker": Path::new("/.dockerenv").exists(),
        "log_dir": dir.display().to_string(),
        "report_days": o.days,
    });
    rep.text(&r, "system.json", &serde_json::to_string_pretty(&system).unwrap_or_default());
    let files = recent_logs(dir, o.days, hsmp_diag::time::now_ms());
    rep.add_dir(&r, dir, "logs", false, &|rel| !rel.contains('/') && files.iter().any(|f| f == rel));
    rep
}

pub async fn run(dir: &Path, o: &Opts) -> Result<()> {
    if !dir.is_dir() {
        bail!("no log folder at {} (pass --log-dir)", dir.display());
    }
    let master = master();
    let own: Vec<String> = own_ip(&master).await.into_iter().collect();
    let mut rep = build(dir, o, own);
    let zip = rep.zip_capped(MAX_REPORT_BYTES).map_err(anyhow::Error::msg)?;
    println!("the report ({} KB zipped) contains:", zip.len() / 1024);
    for e in &rep.entries {
        println!("  {:<60} {:>9} bytes  {}", e.name, e.bytes.len(), if e.redacted { "redacted" } else { "unchanged" });
    }
    for l in &rep.left_out {
        println!("  left out: {l}");
    }
    let out = o.out.clone().unwrap_or_else(|| dir.join(format!("hsmp-server-report-{}.zip", hsmp_diag::time::stamp(hsmp_diag::time::now_ms()))));
    std::fs::write(&out, &zip).with_context(|| format!("write {}", out.display()))?;
    println!("saved {}", out.display());
    if o.upload {
        let r = http().post(format!("{master}/v1/reports")).header("content-type", "application/zip").body(zip).send().await.context("upload")?;
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("the master refused the report ({status}): {}", body.trim());
        }
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        println!("uploaded: report id {} (give us this id; reports are deleted after about 60 days)", v["id"].as_str().unwrap_or("?"));
    } else {
        println!("attach it to a GitHub issue (https://github.com/Cyrex0/HalfSword-MP/issues/new?template=server_host.yml), or run again with --report-upload");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_report_takes_recent_logs_redacted() {
        let d = std::env::temp_dir().join(format!("hsmp_srvreport_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("server-20261004.log"), "peer joined from=203.0.113.7:5000 nick=Bob rcon_password=pw123456\nstats peer 1 nick=\"Bob\" 203.0.113.7:5000: rtt 40 ms\n").unwrap();
        std::fs::write(d.join("server-events-20261004.jsonl"), "{\"message\":\"peer joined\",\"from\":\"203.0.113.7:5000\"}\n").unwrap();
        std::fs::write(d.join("bans.txt"), "203.0.113.9\n").unwrap();
        let o = Opts { out: None, upload: false, keep_ips: false, days: 2 };
        let mut rep = build(&d, &o, vec!["198.51.100.1".into()]);
        let names: Vec<&str> = rep.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["system.json", "logs/server-20261004.log", "logs/server-events-20261004.jsonl"]);
        let log = String::from_utf8(rep.entries[1].bytes.clone()).unwrap();
        assert!(!log.contains("203.0.113.7") && !log.contains("pw123456") && !log.contains("Bob"), "{log}");
        assert!(log.contains("<ip-1>:5000"), "{log}");
        let z = rep.zip_capped(MAX_REPORT_BYTES).unwrap();
        assert_eq!(hsmp_master_core::reports::check(&z).unwrap().head.kind, "server");
        let _ = std::fs::remove_dir_all(&d);
    }
}

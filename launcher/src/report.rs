//! Bug reports: "Create bug report" in the launcher and `hsmp-launcher report`.
//!
//! A report is a zip of the chosen session folders (%LOCALAPPDATA%\HSMP\logs, see
//! crates/hsmp-diag), the launcher log, the machine facts (`system.json`) and the crash
//! triage of those sessions (`triage.json`). Every text file is redacted (crash::Redactor)
//! BEFORE it is shown: the preview is the exact bytes that go into the zip. Crash dumps go in
//! unchanged (optional). The first entry, `hsmp-report.json` (stored), lists every file with
//! its size and SHA-256; the Worker checks it (hsmp_master_core::reports).
//!
//! Delivery: saved to %LOCALAPPDATA%\HSMP\bug_reports, then either attached to a GitHub issue
//! (opened pre-filled) or uploaded to `<master>/v1/reports`, which returns an opaque id.
//! Nothing is sent without an explicit click (or `--upload`).

use crate::crash::{IpMode, Redactor};
use hsmp_diag::sessions::{self, Session};
use hsmp_master_core::reports as fmt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const GITHUB_NEW_ISSUE: &str = "https://github.com/Cyrex0/HalfSword-MP/issues/new";
pub const DEFAULT_MASTER: &str = "https://master.halfswordmp.workers.dev";
/// The launcher log's last lines that go in.
const LAUNCHER_LOG_LINES: usize = 3000;

#[derive(Debug, Clone)]
pub struct Options {
    /// Peer IP addresses become `<ip-N>` (the player's own address is always removed).
    pub hide_ips: bool,
    pub include_dumps: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { hide_ips: true, include_dumps: true }
    }
}

pub use hsmp_diag::report::{Entry, Report};

/// What the report knows about this install, gathered once (it runs the installed sidecar
/// and asks the master for our public address: do it on a worker thread).
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub value: Value,
    pub own_ips: Vec<String>,
    pub secrets: Vec<String>,
    pub hsmp_version: String,
    pub game_build: String,
    pub master: String,
    /// The release says the master takes uploads (manifest `report_upload`).
    pub report_upload: bool,
}

/// What a player sees when the master cannot take the report (no endpoint yet, a server
/// error, or no connection).
pub const UPLOAD_UNAVAILABLE: &str = "Upload isn't available yet \u{2014} use Save report zip / Open GitHub issue";

/// The first https master of the release, else the public one.
pub fn master_url(pkg: Option<&crate::package::Package>) -> String {
    pkg.and_then(|p| p.manifest.master_urls.iter().find(|u| u.starts_with("https://")).cloned()).unwrap_or_else(|| DEFAULT_MASTER.to_string()).trim_end_matches('/').to_string()
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(true)
        .user_agent(&format!("hsmp-launcher/{}", crate::LAUNCHER_VERSION))
        .timeout_connect(Duration::from_secs(10))
        .timeout(timeout)
        .build()
}

/// The address the master sees us from (`/v1/myaddr`), so it can be removed from the logs.
pub fn public_ip(master: &str) -> Option<String> {
    let v: Value = agent(Duration::from_secs(4)).get(&format!("{master}/v1/myaddr")).call().ok()?.into_string().ok().and_then(|s| serde_json::from_str(&s).ok())?;
    v["ip"].as_str().map(String::from).filter(|s| s.parse::<std::net::IpAddr>().is_ok())
}

/// Run `exe args` (no window) and return its stdout, waiting at most `timeout`.
fn run_capture(exe: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut c = std::process::Command::new(exe);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = c.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill(); // our own child, by handle
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// Gather the facts. `env` is the chosen game folder, `pkg` the release the launcher holds.
pub fn gather(env: Option<&crate::install::Env>, pkg: Option<&crate::package::Package>, ask_master: bool) -> Facts {
    let master = master_url(pkg);
    let mut f = Facts { master: master.clone(), report_upload: pkg.is_some_and(|p| p.manifest.report_upload), ..Default::default() };
    let mut hsmp = json!({"launcher": crate::LAUNCHER_VERSION});
    let mut game = json!({});
    // NAT status: a listen host's server.log (in its session folder) has it in every `stats:` line
    // (port mapping, STUN type, punch relay) and the NET_STATUS notice in UE4SS.log
    let mut network = json!({"nat_upnp": "in the logs: every `stats:` line of sessions/*/server.log when hosting (port mapping, STUN type, punch relay) and the NET_STATUS lines of UE4SS.log", "master": master});
    if let Some(p) = pkg {
        let m = &p.manifest;
        hsmp["release"] = json!({"version": m.version, "protocol": m.protocol_version, "commit": m.git_commit, "channel": m.channel, "ue4ss": m.ue4ss.version});
        f.hsmp_version = m.version.clone();
    }
    if let Some(env) = env {
        let win64 = crate::game::win64(&env.game_root);
        match crate::install::status(env) {
            Ok(crate::install::Status::Installed { version, protocol, modified, missing }) => {
                hsmp["installed"] = json!({"version": version, "protocol": protocol, "modified": modified.len(), "missing": missing.len()});
                f.hsmp_version = version;
            }
            Ok(s) => hsmp["installed"] = json!(crate::ops::status_line(&s)),
            Err(e) => hsmp["installed"] = json!(format!("unknown: {e}")),
        }
        let sidecar = crate::careerguard::sidecar_path(env);
        if let Some(out) = run_capture(&sidecar, &["--build-info"], Duration::from_secs(10)) {
            hsmp["build_info"] = out.lines().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).unwrap_or(Value::Null);
        }
        let exe = crate::game::exe_path(&env.game_root);
        game["exe_sha256"] = json!(crate::util::sha256_file(&exe).ok().map(|(h, _)| h));
        let found = crate::steam::discover();
        let steam = found.iter().find(|g| crate::util::fold_path(&g.root) == crate::util::fold_path(&env.game_root));
        game["steam_buildid"] = json!(steam.and_then(|g| g.buildid.clone()));
        f.game_build = steam.and_then(|g| g.buildid.clone()).unwrap_or_default();
        if let (Some(p), Some(h)) = (pkg, game["exe_sha256"].as_str().map(String::from)) {
            game["supported_by_release"] = json!(p.manifest.game.builds.iter().any(|b| b.exe_sha256 == h));
            if let Some(b) = p.manifest.game.builds.iter().find(|b| b.exe_sha256 == h) {
                game["label"] = json!(b.label);
            }
        }
        if cfg!(windows) {
            network["firewall"] = json!(crate::firewall::status(&crate::firewall::server_exe(&env.game_root)).describe());
        }
        // this install's player key (an identifier): removed wherever it shows up
        for p in [win64.join("hsmp_state").join(".player_key")] {
            if let Ok(t) = std::fs::read_to_string(&p) {
                let k = t.trim().to_ascii_lowercase();
                if k.len() >= 32 {
                    f.secrets.push(k);
                }
            }
        }
    }
    if ask_master {
        if let Some(ip) = public_ip(&master) {
            f.own_ips.push(ip);
            network["own_public_ip"] = json!("found and removed from every file");
        } else {
            network["own_public_ip"] = json!("unknown (the master did not answer)");
        }
    }
    f.value = json!({"system": crate::sysinfo::collect(), "hsmp": hsmp, "game": game, "network": network});
    f
}

/// The redactor of a report: this machine's names, the facts' secrets and own address.
pub fn redactor(facts: &Facts, o: &Options) -> Redactor {
    let mut r = Redactor::system().with_ips(if o.hide_ips { IpMode::Public } else { IpMode::OwnOnly }, facts.own_ips.clone());
    r.secrets = facts.secrets.clone();
    r
}

/// The triage of a session: its outcome and each crash folder classified (no debugger).
pub fn triage_of(s: &Session, ue_saved: Option<&Path>) -> Value {
    let mut crashes = vec![];
    for name in &s.info.crash_dirs {
        let copy = s.dir.join("crashes").join(name);
        let dir = if copy.is_dir() { copy } else { ue_saved.map(|u| u.join("Crashes").join(name)).unwrap_or(copy) };
        if dir.is_dir() {
            crashes.push(hsmp_diag::triage::quick(&dir));
        }
    }
    json!({
        "session": s.info.id,
        "outcome": s.info.outcome.as_str(),
        "exit_code": s.info.exit_code.map(|c| format!("0x{c:08X} ({c})")),
        "started": hsmp_diag::time::iso(s.info.started_ms),
        "ended": s.info.ended_ms.map(hsmp_diag::time::iso),
        "ue4ss_dumps": s.info.ue4ss_dumps,
        "crashes": crashes,
        "notes": s.info.notes,
    })
}

/// Read, redact and assemble everything. `launcher_log` is launcher.log.
pub fn build(sessions: &[Session], launcher_log: Option<&Path>, ue_saved: Option<&Path>, facts: &Facts, o: &Options) -> Report {
    let r = redactor(facts, o);
    let mut rep = Report::new("client", &format!("hsmp-launcher {}", crate::LAUNCHER_VERSION));
    rep.hsmp_version = facts.hsmp_version.clone();
    rep.options = Some((o.hide_ips, o.include_dumps));
    rep.text(&r, "system.json", &serde_json::to_string_pretty(&facts.value).unwrap_or_default());
    let triage: Vec<Value> = sessions.iter().map(|s| triage_of(s, ue_saved)).collect();
    rep.text(&r, "triage.json", &serde_json::to_string_pretty(&triage).unwrap_or_default());
    for s in sessions {
        rep.sessions.push(s.info.id.clone());
        rep.add_dir(&r, &s.dir, &format!("sessions/{}", s.info.id), o.include_dumps, &|_| true);
    }
    if let Some(l) = launcher_log {
        if let Ok(b) = std::fs::read(l) {
            rep.text(&r, "launcher.log", &hsmp_diag::report::tail_lines(&String::from_utf8_lossy(&b), LAUNCHER_LOG_LINES));
        }
    }
    rep
}

/// %LOCALAPPDATA%\HSMP\bug_reports\hsmp-report-<UTC stamp>.zip
pub fn default_out(hsmp_home: &Path) -> PathBuf {
    hsmp_home.join("bug_reports").join(format!("hsmp-report-{}.zip", crate::util::stamp_utc(crate::util::now_unix())))
}

pub fn save(zip: &[u8], out: &Path) -> Result<(), String> {
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    crate::util::atomic_write(out, zip).map_err(|e| e.to_string())
}

/// Upload to `<master>/v1/reports`. Returns the report id the master gives back.
pub struct MasterSink {
    pub master: String,
}

impl crate::crash::CrashSink for MasterSink {
    fn submit(&self, report_zip: &[u8], _meta: &Value) -> Result<String, String> {
        upload(&self.master, report_zip)
    }
}

pub fn upload(master: &str, zip: &[u8]) -> Result<String, String> {
    if !master.starts_with("https://") {
        return Err(format!("reports are only sent over https (master {master})"));
    }
    post_report(&agent(Duration::from_secs(120)), &format!("{master}/v1/reports"), zip)
}

fn post_report(agent: &ureq::Agent, url: &str, zip: &[u8]) -> Result<String, String> {
    match agent.post(url).set("content-type", "application/zip").send_bytes(zip) {
        Ok(resp) => {
            let body = resp.into_string().map_err(|e| format!("bad answer from the master: {e}"))?;
            let v: Value = serde_json::from_str(&body).map_err(|e| format!("bad answer from the master: {e}"))?;
            v["id"].as_str().filter(|id| fmt::is_report_id(id)).map(String::from).ok_or_else(|| format!("bad answer from the master: {v}"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(match code {
                429 => format!("too many reports from your address today; try again later ({})", body.trim()),
                413 => "the report is too large for the upload; save it and attach it to a GitHub issue instead".into(),
                404 | 405 | 500..=599 => UPLOAD_UNAVAILABLE.into(),
                _ => format!("the master refused the report ({code}: {})", body.trim()),
            })
        }
        Err(ureq::Error::Transport(_)) => Err(UPLOAD_UNAVAILABLE.into()),
    }
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The GitHub "new issue" URL with the bug template's fields filled in (the player attaches
/// the zip themselves). Field ids are the ones in .github/ISSUE_TEMPLATE/bug_report.yml.
pub fn github_issue_url(facts: &Facts, zip_name: &str, upload_id: Option<&str>, outcome: Option<&str>) -> String {
    let os = facts.value["system"]["windows"].as_str().unwrap_or("").to_string();
    let mut logs = format!("Bug report zip from the launcher: {zip_name} (attached below).");
    if let Some(id) = upload_id {
        logs.push_str(&format!("\nAlso uploaded to the HSMP master: report id {id}"));
    }
    let title = match outcome {
        Some("crashed") => "Crash: ",
        _ => "Bug: ",
    };
    let q = [
        ("template", "bug_report.yml".to_string()),
        ("title", title.to_string()),
        ("hsmp-version", facts.hsmp_version.clone()),
        ("game-build", facts.game_build.clone()),
        ("os", os),
        ("logs", logs),
    ];
    let qs: Vec<String> = q.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}={}", enc(v))).collect();
    format!("{GITHUB_NEW_ISSUE}?{}", qs.join("&"))
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https links are opened".into());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (op, u) = (w("open"), w(url));
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
        let r = unsafe { ShellExecuteW(0, op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if r <= 32 {
            return Err(format!("could not open the browser (ShellExecute {r})"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// The sessions to report by default: the newest one that crashed, else the newest.
pub fn default_sessions(all: &[Session]) -> Vec<Session> {
    all.iter().find(|s| s.info.outcome == sessions::Outcome::Crashed).or(all.first()).cloned().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Read;

    fn facts() -> Facts {
        Facts {
            value: json!({"system": {"windows": "Windows 11 Pro 24H2 (build 26100.1)"}, "game": {"path": "C:\\Users\\zed\\Steam"}}),
            own_ips: vec!["198.51.100.9".into()],
            secrets: vec!["cd".repeat(32)],
            hsmp_version: "0.1.0-beta.3".into(),
            game_build: "24185754".into(),
            master: DEFAULT_MASTER.into(),
            report_upload: false,
        }
    }

    /// One-shot local HTTP server answering `status` once; returns its URL.
    fn answer_once(status: &str) -> String {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/reports", l.local_addr().unwrap());
        let status = status.to_string();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                // Read the whole request (headers + content-length body) first: closing a
                // socket with unread request bytes makes Windows send RST, and the client then
                // sees a transport error instead of this status (the 429 case failed 4 in 5).
                let _ = s.set_read_timeout(Some(Duration::from_millis(2000)));
                let (mut req, mut buf) = (Vec::new(), [0u8; 4096]);
                while let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
                    if n == 0 { break; }
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                    let Some(end) = text.find("\r\n\r\n") else { continue };
                    let len = text[..end].lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                    if req.len() >= end + 4 + len { break; }
                }
                let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-length: 9\r\nconnection: close\r\n\r\nNot Found");
            }
        });
        url
    }

    /// No /v1/reports yet (404), a server error or no connection: the player is pointed at
    /// the zip and the GitHub issue instead of a raw error.
    #[test]
    fn upload_degrades_when_the_master_has_no_reports_endpoint() {
        let a = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
        for status in ["404 Not Found", "405 Method Not Allowed", "500 Internal Server Error", "503 Service Unavailable"] {
            assert_eq!(post_report(&a, &answer_once(status), b"zip"), Err(UPLOAD_UNAVAILABLE.to_string()), "{status}");
        }
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/reports", closed.local_addr().unwrap());
        drop(closed);
        assert_eq!(post_report(&a, &url, b"zip"), Err(UPLOAD_UNAVAILABLE.to_string()));
        assert!(post_report(&a, &answer_once("429 Too Many Requests"), b"zip").unwrap_err().contains("too many reports"));
        assert!(!facts().report_upload);
    }

    fn session(root: &Path) -> Session {
        let mut s = sessions::create(root, 1_790_899_200_000, 77, "0.1.0-beta.3").unwrap();
        s.info.outcome = sessions::Outcome::Crashed;
        s.info.exit_code = Some(0xC000_0005);
        s.info.crash_dirs = vec!["UECC-Windows-X".into()];
        s.save().unwrap();
        std::fs::write(s.dir.join("UE4SS.log"), format!("[Lua] JOIN 203.0.113.7:7777 nick=Bob key {}\n", "cd".repeat(32))).unwrap();
        std::fs::write(s.dir.join("sidecar.log"), "link: rtt 40 ms | peer 198.51.100.9:5000\n").unwrap();
        let c = s.dir.join("crashes/UECC-Windows-X");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join("UEMinidump.dmp"), b"MDMP\x00\x01").unwrap();
        std::fs::write(c.join("CrashContext.runtime-xml"), "<ErrorMessage>EXCEPTION_ACCESS_VIOLATION</ErrorMessage><Threads><Thread><CallStack>HalfswordUE5-Win64-Shipping 0x00007ff700000000 + 4a23c64 \n</CallStack><IsCrashed>true</IsCrashed><ThreadName>GameThread</ThreadName></Thread></Threads>").unwrap();
        std::fs::write(s.dir.join("weird.bin"), b"\x00").unwrap();
        s
    }

    /// The zip the launcher writes passes the Worker's check; the manifest lists every file
    /// with its hash; text is redacted, dumps are byte-exact; the preview is the zip content.
    #[test]
    fn report_zip_manifest_and_redaction() {
        let t = TempDir::new("report_zip");
        let s = session(&t.path().join("logs"));
        std::fs::write(t.path().join("launcher.log"), "2026 started from C:\\Users\\zed\\Desktop\n").unwrap();
        let mut rep = build(std::slice::from_ref(&s), Some(&t.path().join("launcher.log")), None, &facts(), &Options::default());
        let names: Vec<&str> = rep.entries.iter().map(|e| e.name.as_str()).collect();
        let sid = &s.info.id;
        for want in ["system.json", "triage.json", "launcher.log"] {
            assert!(names.contains(&want), "{names:?}");
        }
        assert!(names.contains(&format!("sessions/{sid}/UE4SS.log").as_str()) && names.contains(&format!("sessions/{sid}/crashes/UECC-Windows-X/UEMinidump.dmp").as_str()), "{names:?}");
        assert!(rep.left_out.iter().any(|l| l.contains("weird.bin")), "{:?}", rep.left_out);
        let z = rep.zip_capped(fmt::MAX_REPORT_BYTES).unwrap();
        let checked = fmt::check(&z).expect("the Worker accepts the launcher's zip");
        assert_eq!(checked.head.sessions, vec![sid.clone()]);
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(&z)).unwrap();
        assert_eq!(a.by_index(0).unwrap().name(), fmt::MANIFEST_NAME);
        let mut text = |n: &str| {
            let mut s = String::new();
            a.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let log = text(&format!("sessions/{sid}/UE4SS.log"));
        assert_eq!(log, "[Lua] JOIN <ip-1>:7777 nick=<redacted> key <secret>\n");
        assert!(text(&format!("sessions/{sid}/sidecar.log")).contains("peer <my-ip>:5000"));
        assert!(text("launcher.log").contains("\\Users\\<user>\\Desktop"));
        assert!(!text("system.json").contains("zed"));
        let triage = text("triage.json");
        assert!(triage.contains("RVP_TASK_AFTER_TRAVEL") && triage.contains("0xC0000005"), "{triage}");
        let man: Value = serde_json::from_str(&text(fmt::MANIFEST_NAME)).unwrap();
        let files = man["files"].as_array().unwrap();
        assert_eq!(files.len(), rep.entries.len());
        for (f, e) in files.iter().zip(&rep.entries) {
            assert_eq!(f["sha256"], crate::util::sha256_hex(&e.bytes), "{}", e.name);
        }
        let mut dmp = vec![];
        a.by_name(&format!("sessions/{sid}/crashes/UECC-Windows-X/UEMinidump.dmp")).unwrap().read_to_end(&mut dmp).unwrap();
        assert_eq!(dmp, b"MDMP\x00\x01");
    }

    /// Incompressible bytes (xorshift).
    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    /// Over the cap: dumps go first, then the biggest text loses its beginning.
    #[test]
    fn report_cap_drops_dumps_then_cuts_text() {
        let mut rep = Report {
            entries: vec![
                Entry { name: "a.dmp".into(), bytes: noise(3_000_000, 1), redacted: false },
                Entry { name: "big.log".into(), bytes: noise(400_000, 2), redacted: true },
            ],
            ..Default::default()
        };
        let z = rep.zip_capped(200_000).unwrap();
        assert!(z.len() <= 200_000);
        assert!(rep.entries.iter().all(|e| e.name != "a.dmp"));
        assert!(rep.left_out.iter().any(|l| l.starts_with("a.dmp")), "{:?}", rep.left_out);
        fmt::check(&z).unwrap();
    }

    #[test]
    fn github_url_is_prefilled_and_encoded() {
        let u = github_issue_url(&facts(), "hsmp-report-20261004_101500.zip", Some("20261004-0123456789abcdef0123456789abcdef"), Some("crashed"));
        assert!(u.starts_with("https://github.com/Cyrex0/HalfSword-MP/issues/new?template=bug_report.yml&title=Crash%3A%20"), "{u}");
        assert!(u.contains("&hsmp-version=0.1.0-beta.3&game-build=24185754&os=Windows%2011%20Pro"), "{u}");
        assert!(u.contains("report%20id%2020261004-0123456789abcdef0123456789abcdef"), "{u}");
        assert!(!u.contains(' '));
    }

    #[test]
    fn default_session_prefers_the_newest_crash() {
        let t = TempDir::new("report_default");
        let root = t.path().join("logs");
        let mut a = sessions::create(&root, 1_790_899_200_000, 1, "v").unwrap();
        a.info.outcome = sessions::Outcome::Crashed;
        a.save().unwrap();
        sessions::create(&root, 1_790_899_300_000, 2, "v").unwrap();
        let all = sessions::list(&root);
        assert_eq!(default_sessions(&all)[0].info.id, a.info.id);
        assert!(default_sessions(&[]).is_empty());
    }
}

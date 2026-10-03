//! Updates from the GitHub releases of Cyrex0/HalfSword-MP.
//!
//! A release is tagged `v<version>` and carries `hsmp-<version>.zip` and
//! `hsmp-<version>.zip.sha256`. Nothing downloaded is trusted until it passes, in order: the
//! size cap, the published SHA-256, and the manifest signature against the keys compiled into
//! this launcher (`Package::open`, the same check as a zip opened by hand). Then a downgrade
//! (unless allowed) or a release that does not support the installed game build is refused,
//! and the normal journaled install runs (all-or-nothing, rolled back on failure).
//!
//! GitHub allows 60 unauthenticated API requests per hour per IP. The last answer is cached
//! with its ETag; a `304 Not Modified` does not count against that limit.

use crate::game::{self, BuildCheck};
use crate::install::{self, Env, InstallOpts, InstallReport, Status};
use crate::package::Package;
use crate::sign::TrustedKey;
use crate::util;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "Cyrex0/HalfSword-MP";
pub const API: &str = "https://api.github.com";
/// A release zip is ~40 MB; anything far larger is not ours.
pub const MAX_ZIP: u64 = 512 * 1024 * 1024;
const MAX_SHA_FILE: u64 = 4096;
const MAX_API_BODY: u64 = 8 * 1024 * 1024;
const ATTEMPTS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// The version from a `v<version>` tag; None for any other tag.
    pub fn version(&self) -> Option<&str> {
        self.tag_name.strip_prefix('v').filter(|v| valid_version(v))
    }
}

fn valid_version(v: &str) -> bool {
    v.len() <= 64 && v.starts_with(|c: char| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) && !v.contains("..")
}

/// The newest non-draft release with a `v<version>` tag. `stable_only` also skips
/// pre-releases (flagged on GitHub, or a `-` in the version).
pub fn pick_latest(releases: &[Release], stable_only: bool) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter(|r| r.version().is_some_and(|v| !stable_only || (!r.prerelease && !v.contains('-'))))
        .max_by(|a, b| util::cmp_version(a.version().unwrap(), b.version().unwrap()))
}

/// The release zip and its checksum file.
pub fn select_assets(r: &Release) -> Result<(&Asset, &Asset), String> {
    let v = r.version().ok_or_else(|| format!("release tag {} is not v<version>", r.tag_name))?;
    let (zip, sha) = (format!("hsmp-{v}.zip"), format!("hsmp-{v}.zip.sha256"));
    let find = |n: &str| r.assets.iter().find(|a| a.name.eq_ignore_ascii_case(n));
    let z = find(&zip).ok_or_else(|| format!("release {} has no {zip}", r.tag_name))?;
    let s = find(&sha).ok_or_else(|| format!("release {} has no {sha}, so its download cannot be checked", r.tag_name))?;
    Ok((z, s))
}

/// `<hex>` or `<hex>  <name>` (sha256sum style); the hash is lower-cased.
pub fn parse_sha256(text: &str) -> Result<String, String> {
    let t = text.trim_start_matches('\u{feff}').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if util::is_sha256_hex(&t) {
        Ok(t)
    } else {
        Err("the release's .sha256 file does not hold a SHA-256".into())
    }
}

/// Markdown release notes as plain text (headings, emphasis and code marks dropped).
pub fn plain_text(md: &str) -> String {
    md.lines()
        .map(|l| {
            let l = l.trim_end();
            let l = l.trim_start_matches('#').trim_start_matches(' ');
            let l = if let Some(rest) = l.strip_prefix("* ").or_else(|| l.strip_prefix("- ")) { format!("  - {rest}") } else { l.to_string() };
            l.replace("**", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the check found.
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub release: Release,
    pub version: String,
    /// The version compared against (installed HSMP, else this launcher's).
    pub current: String,
}

impl Available {
    pub fn ordering(&self) -> std::cmp::Ordering {
        util::cmp_version(&self.version, &self.current)
    }
    pub fn notes(&self) -> String {
        plain_text(self.release.body.as_deref().unwrap_or(""))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    url: String,
    etag: String,
    body: String,
}

pub struct Client {
    agent: ureq::Agent,
    api: String,
    /// between download attempts (0 in tests)
    pause: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

impl Client {
    pub fn new() -> Client {
        Client::with(API, true, Duration::from_secs(2))
    }

    fn with(api: &str, https_only: bool, pause: Duration) -> Client {
        let agent = ureq::AgentBuilder::new()
            .https_only(https_only)
            .user_agent(&format!("hsmp-launcher/{} (+https://github.com/{REPO})", crate::LAUNCHER_VERSION))
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .redirects(5)
            .build();
        Client { agent, api: api.trim_end_matches('/').to_string(), pause }
    }

    /// Releases to choose from: `/releases/latest` (stable only) or the newest `/releases`.
    /// `cache` is a file for the last answer and its ETag.
    pub fn releases(&self, stable_only: bool, cache: Option<&Path>) -> Result<Vec<Release>, String> {
        let url = if stable_only { format!("{}/repos/{REPO}/releases/latest", self.api) } else { format!("{}/repos/{REPO}/releases?per_page=30", self.api) };
        let cached: Cache = cache.and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).filter(|c: &Cache| c.url == url).unwrap_or_default();
        let mut req = self.agent.get(&url).set("Accept", "application/vnd.github+json").set("X-GitHub-Api-Version", "2022-11-28");
        if !cached.etag.is_empty() {
            req = req.set("If-None-Match", &cached.etag);
        }
        let body = match req.call() {
            Ok(r) if r.status() == 304 => cached.body,
            Ok(r) => {
                let etag = r.header("etag").unwrap_or("").to_string();
                let body = String::from_utf8(read_capped(r.into_reader(), MAX_API_BODY)?).map_err(|_| "GitHub's answer is not UTF-8".to_string())?;
                if let Some(p) = cache {
                    let c = Cache { url: url.clone(), etag, body: body.clone() };
                    let _ = p.parent().map(std::fs::create_dir_all);
                    let _ = util::atomic_write(p, &serde_json::to_vec(&c).unwrap_or_default());
                }
                body
            }
            Err(ureq::Error::Status(404, _)) if stable_only => return Ok(vec![]), // no stable release yet
            Err(ureq::Error::Status(code, r)) => return Err(status_text(code, &r)),
            Err(e) => return Err(format!("cannot reach GitHub to check for updates: {e}")),
        };
        if stable_only {
            serde_json::from_str::<Release>(&body).map(|r| vec![r]).map_err(|e| format!("unexpected answer from GitHub: {e}"))
        } else {
            serde_json::from_str::<Vec<Release>>(&body).map_err(|e| format!("unexpected answer from GitHub: {e}"))
        }
    }

    /// The newest release for the channel, compared with `current`.
    pub fn check(&self, current: &str, stable_only: bool, cache: Option<&Path>) -> Result<Option<Available>, String> {
        let all = self.releases(stable_only, cache)?;
        Ok(pick_latest(&all, stable_only).map(|r| Available { version: r.version().unwrap().to_string(), release: r.clone(), current: current.to_string() }))
    }

    fn get_small(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        match self.agent.get(url).call() {
            Ok(r) => read_capped(r.into_reader(), limit),
            Err(ureq::Error::Status(code, r)) => Err(status_text(code, &r)),
            Err(e) => Err(format!("download failed: {e}")),
        }
    }

    /// Download `url` to `dest` through `<dest>.part`, resuming a partial file and retrying
    /// dropped connections. Never more than `cap` bytes; `size` (0 = unknown) must match.
    pub fn download(&self, url: &str, dest: &Path, size: u64, cap: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        if size > cap {
            return Err(format!("the release zip is {size} bytes, more than the {cap} an HSMP release can be"));
        }
        let part = PathBuf::from(format!("{}.part", dest.display()));
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(self.pause * attempt);
            }
            let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if have > cap || (size > 0 && have > size) {
                let _ = std::fs::remove_file(&part);
                continue;
            }
            if size > 0 && have == size {
                break;
            }
            let mut req = self.agent.get(url);
            if have > 0 {
                req = req.set("Range", &format!("bytes={have}-"));
            }
            let resp = match req.call() {
                Ok(r) => r,
                Err(ureq::Error::Status(416, _)) => {
                    let _ = std::fs::remove_file(&part);
                    last = "the server refused to resume".into();
                    continue;
                }
                Err(ureq::Error::Status(code, r)) if code >= 500 || code == 429 => {
                    last = status_text(code, &r);
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => return Err(status_text(code, &r)),
                Err(e) => {
                    last = format!("download failed: {e}");
                    continue;
                }
            };
            let resumed = resp.status() == 206 && have > 0;
            let start = if resumed { have } else { 0 };
            if let Some(len) = resp.header("content-length").and_then(|v| v.parse::<u64>().ok()) {
                if start + len > cap || (size > 0 && start + len != size) {
                    let _ = std::fs::remove_file(&part);
                    return Err(format!("the server announced {} bytes, but the release lists {size}", start + len));
                }
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(resumed)
                .truncate(!resumed)
                .open(&part)
                .map_err(|e| format!("{}: {e}", part.display()))?;
            let mut got = start;
            let mut r = resp.into_reader();
            let mut buf = vec![0u8; 64 * 1024];
            let res: Result<(), String> = loop {
                match r.read(&mut buf) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        got += n as u64;
                        if got > cap || (size > 0 && got > size) {
                            drop(f);
                            let _ = std::fs::remove_file(&part);
                            return Err(format!("the download is larger than the {} bytes the release lists", if size > 0 { size } else { cap }));
                        }
                        f.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                        progress(got, size);
                    }
                    Err(e) => break Err(format!("the download broke off: {e}")),
                }
            };
            f.flush().map_err(|e| format!("{}: {e}", part.display()))?;
            drop(f);
            match res {
                Ok(()) if size == 0 || got == size => {
                    last.clear();
                    break;
                }
                Ok(()) => last = format!("the download ended after {got} of {size} bytes"),
                Err(e) => last = e,
            }
        }
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if !last.is_empty() || (size > 0 && have != size) {
            return Err(format!("{} (tried {ATTEMPTS} times; the partial download is kept and resumed next time)", if last.is_empty() { "incomplete download".to_string() } else { last }));
        }
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
    }

    /// Download the release zip into `dir` and check it against the published SHA-256. A zip
    /// already there with the right hash is reused. A mismatch deletes the file.
    pub fn fetch_verified(&self, r: &Release, dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
        let (zip, sha) = select_assets(r)?;
        let want = parse_sha256(&String::from_utf8_lossy(&self.get_small(&sha.browser_download_url, MAX_SHA_FILE)?))?;
        let dest = dir.join(&zip.name);
        if util::sha256_file(&dest).map(|(h, _)| h == want).unwrap_or(false) {
            return Ok(dest);
        }
        self.download(&zip.browser_download_url, &dest, zip.size, MAX_ZIP, progress)?;
        let (got, _) = util::sha256_file(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        if got != want {
            let _ = std::fs::remove_file(&dest);
            return Err(format!("the downloaded {} does not match its published SHA-256 (corrupted or tampered download). It was deleted; nothing was installed.", zip.name));
        }
        Ok(dest)
    }
}

fn read_capped(r: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    r.take(limit + 1).read_to_end(&mut v).map_err(|e| format!("download failed: {e}"))?;
    if v.len() as u64 > limit {
        return Err(format!("the answer is larger than {limit} bytes"));
    }
    Ok(v)
}

fn status_text(code: u16, r: &ureq::Response) -> String {
    if (code == 403 || code == 429) && r.header("x-ratelimit-remaining") == Some("0") {
        let reset = r.header("x-ratelimit-reset").and_then(|v| v.parse::<u64>().ok());
        return format!(
            "GitHub's limit of 60 update checks per hour from your network is used up; it resets at {}. Try again later.",
            reset.map(util::iso_utc).unwrap_or_else(|| "the top of the hour".into())
        );
    }
    format!("GitHub answered HTTP {code}")
}

/// Open the downloaded zip (signature against `keys`) and refuse what must not be installed:
/// a downgrade unless `allow_downgrade`, and a release that does not support this game build.
pub fn verify(env: &Env, zip: &Path, keys: &[TrustedKey], allow_downgrade: bool) -> Result<(Package, BuildCheck), String> {
    let pkg = Package::open(zip, keys)?;
    let new = pkg.manifest.version.clone();
    if let Status::Installed { version, .. } = install::status(env)? {
        if util::cmp_version(&new, &version) == std::cmp::Ordering::Less && !allow_downgrade {
            return Err(format!("HSMP {new} is older than the installed {version}; not installed (allow downgrades under Advanced to do it anyway)"));
        }
    }
    let build = game::check_build(&env.game_root, &pkg.manifest.game).map_err(|e| format!("cannot read the game exe: {e}"))?;
    if !build.is_supported() {
        return Err(format!("HSMP {new} does not support this Half Sword build (exe sha256 {}); nothing was installed", &build.sha256()[..16]));
    }
    Ok((pkg, build))
}

/// Verify and install a downloaded release. Refused while the game, a sidecar or a server
/// from this folder runs; the career guard recovers a crashed session first.
pub fn apply(env: &Env, zip: &Path, keys: &[TrustedKey], allow_downgrade: bool, opts: InstallOpts, log: &mut dyn FnMut(String)) -> Result<InstallReport, String> {
    if opts.check_processes {
        let running = crate::procs::blocking_processes(&env.game_root);
        if !running.is_empty() {
            return Err(format!("close Half Sword first ({} running); the launcher never closes it for you", running.join(", ")));
        }
    }
    let (pkg, build) = verify(env, zip, keys, allow_downgrade)?;
    log(format!("HSMP {} verified (SHA-256 and signature by {})", pkg.manifest.version, pkg.signed_by));
    match crate::careerguard::recover(env, log) {
        Ok(Some(r)) if !r.ok => return Err(crate::careerguard::failure_text(&r, "Update")),
        Ok(_) => {}
        Err(e) => log(format!("note: {e}; the new version runs the career save check after installing")),
    }
    let opts = InstallOpts { allow_downgrade, allow_unsupported: false, exe_sha256: Some(build.sha256().to_string()), ..opts };
    crate::ops::install_with(env, &pkg, opts, log)
}

/// Install options for an update on this PC.
pub fn default_opts(env: &Env) -> InstallOpts {
    InstallOpts { backup_saves: true, check_processes: true, steam_updating: crate::steam::is_updating(&crate::steam::discover(), &env.game_root), ..Default::default() }
}

/// Where downloads go.
pub fn download_dir(hsmp_home: &Path) -> PathBuf {
    hsmp_home.join("downloads")
}

pub fn cache_path(hsmp_home: &Path) -> PathBuf {
    hsmp_home.join("launcher").join("update_check.json")
}

/// Keep only `keep` among the downloaded release zips.
pub fn prune_downloads(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if p != keep && n.starts_with("hsmp-") && (n.ends_with(".zip") || n.ends_with(".zip.part")) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The installed launcher copy, when it is not the one running (an update brought a new one).
pub fn restart_target(hsmp_home: &Path) -> Option<PathBuf> {
    let p = crate::trust::installed_launcher_path(hsmp_home);
    let new = util::sha256_file(&p).ok()?.0;
    (Some(new) != crate::trust::self_sha256()).then_some(p)
}

/// Start `exe` detached (the caller exits next, so the new launcher takes over).
pub fn relaunch(exe: &Path) -> Result<(), String> {
    let mut c = std::process::Command::new(exe);
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        c.creation_flags(DETACHED_PROCESS);
    }
    c.spawn().map(|_| ()).map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::tests::{fake, package};
    use crate::manifest::Manifest;
    use crate::pack::{self, Item};
    use crate::package::Files;
    use crate::sign::tests::{test_key, trusted};
    use crate::testutil::{snapshot, TempDir};
    use std::collections::{BTreeMap, HashMap};
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn rel(tag: &str, pre: bool, draft: bool) -> Release {
        Release { tag_name: tag.into(), name: None, body: None, draft, prerelease: pre, html_url: String::new(), assets: vec![] }
    }

    #[test]
    fn version_ordering() {
        use std::cmp::Ordering::*;
        let order = ["0.1.0-alpha", "0.1.0-beta.1", "0.1.0-beta.2", "0.1.0-beta.10", "0.1.0-rc.1", "0.1.0", "0.1.1", "0.2.0", "0.10.0", "1.0.0"];
        for w in order.windows(2) {
            assert_eq!(util::cmp_version(w[0], w[1]), Less, "{} < {}", w[0], w[1]);
            assert_eq!(util::cmp_version(w[1], w[0]), Greater);
        }
        assert_eq!(util::cmp_version("1.0.0+build.5", "1.0.0"), Equal, "build metadata is ignored");
        assert_eq!(util::cmp_version("1.0.0-rc.1+x", "1.0.0-rc.1"), Equal);

        let all = vec![rel("v0.2.0-beta.1", true, false), rel("v0.1.0", false, false), rel("v0.3.0", false, true), rel("nightly", true, false), rel("v0.1.1", false, false), rel("v0.2.0-rc.1", false, false)];
        assert_eq!(pick_latest(&all, false).unwrap().tag_name, "v0.2.0-rc.1", "pre-releases count on the beta channel; drafts never");
        assert_eq!(pick_latest(&all, true).unwrap().tag_name, "v0.1.1", "stable skips flagged and dash pre-releases");
        assert!(pick_latest(&[rel("v0.3.0", false, true)], false).is_none());
        assert!(rel("v1.0;rm", false, false).version().is_none());
        assert!(rel("v..1", false, false).version().is_none());
    }

    #[test]
    fn asset_selection_and_checksum_file() {
        let a = |n: &str| Asset { name: n.into(), browser_download_url: format!("https://x/{n}"), size: 1 };
        let mut r = rel("v0.2.0", false, false);
        r.assets = vec![a("hsmp-0.1.0.zip"), a("hsmp-0.2.0.zip.sha256"), a("source.zip"), a("hsmp-0.2.0.zip")];
        let (z, s) = select_assets(&r).unwrap();
        assert_eq!((z.name.as_str(), s.name.as_str()), ("hsmp-0.2.0.zip", "hsmp-0.2.0.zip.sha256"));
        r.assets.retain(|x| !x.name.ends_with(".sha256"));
        assert!(select_assets(&r).unwrap_err().contains(".sha256"));
        r.assets.clear();
        assert!(select_assets(&r).unwrap_err().contains("hsmp-0.2.0.zip"));
        let h = "A".repeat(64);
        assert_eq!(parse_sha256(&format!("{h}  hsmp-0.2.0.zip\n")).unwrap(), "a".repeat(64));
        assert_eq!(parse_sha256(&format!("\u{feff}{h}")).unwrap(), "a".repeat(64));
        assert!(parse_sha256("not a hash").is_err());
        assert!(parse_sha256("").is_err());
        assert_eq!(plain_text("## Fixes\n* **bold** `code`\n"), "Fixes\n  - bold code");
    }

    // ---- a local HTTP server: canned answers, Range support, one cut connection ----

    #[derive(Clone)]
    struct Route {
        code: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        /// close the first full (non-Range) answer after this many body bytes
        cut: Option<usize>,
    }

    fn ok(body: impl Into<Vec<u8>>) -> Route {
        Route { code: 200, headers: vec![], body: body.into(), cut: None }
    }

    struct Mock {
        base: String,
        routes: Arc<Mutex<HashMap<String, Route>>>,
        /// (path, request head) of every request
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Mock {
        fn start() -> Mock {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
            let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
            let (r2, s2) = (routes.clone(), seen.clone());
            std::thread::spawn(move || {
                for s in l.incoming().flatten() {
                    let mut rd = std::io::BufReader::new(s.try_clone().unwrap());
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if rd.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    s2.lock().unwrap().push((path.clone(), head.clone()));
                    let range = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|v| v.trim_end_matches('-').parse::<usize>().unwrap_or(0)));
                    let mut routes = r2.lock().unwrap();
                    let Some(rt) = routes.get_mut(&path) else {
                        let _ = (&s).write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        continue;
                    };
                    let (code, body) = match range {
                        Some(n) if rt.code == 200 && n <= rt.body.len() => (206, rt.body[n..].to_vec()),
                        _ => (rt.code, rt.body.clone()),
                    };
                    let cut = if range.is_none() { rt.cut.take() } else { None };
                    let mut out = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                    for (k, v) in &rt.headers {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str("\r\n");
                    let mut w = &s;
                    let _ = w.write_all(out.as_bytes());
                    let _ = w.write_all(&body[..cut.unwrap_or(body.len()).min(body.len())]);
                    let _ = w.flush();
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            });
            Mock { base, routes, seen }
        }
        fn route(&self, path: &str, r: Route) {
            self.routes.lock().unwrap().insert(path.into(), r);
        }
        fn client(&self) -> Client {
            Client::with(&self.base, false, Duration::ZERO)
        }
        fn heads(&self, path: &str) -> Vec<String> {
            self.seen.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, h)| h.clone()).collect()
        }
    }

    fn release_json(m: &Mock, v: &str, zip: &[u8], pre: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": format!("v{v}"), "name": format!("HSMP {v}"), "body": "## Changes\n* better", "draft": false, "prerelease": pre,
            "assets": [
                { "name": format!("hsmp-{v}.zip"), "browser_download_url": format!("{}/dl/hsmp-{v}.zip", m.base), "size": zip.len() },
                { "name": format!("hsmp-{v}.zip.sha256"), "browser_download_url": format!("{}/dl/hsmp-{v}.zip.sha256", m.base), "size": 64 }
            ]
        })
    }

    fn publish(m: &Mock, v: &str, zip: &[u8], sha: &str) {
        m.route(&format!("/dl/hsmp-{v}.zip"), ok(zip.to_vec()));
        m.route(&format!("/dl/hsmp-{v}.zip.sha256"), ok(format!("{sha}  hsmp-{v}.zip\n")));
    }

    /// A release zip of `m`/`files`, signed by `key`.
    fn zip_of(m: Manifest, files: &Files, key: &ed25519_dalek::SigningKey) -> Vec<u8> {
        let items: BTreeMap<String, Item> = m.files.iter().map(|f| (f.path.clone(), Item { bytes: files[&f.path].clone(), role: f.role, install: f.install.clone() })).collect();
        let (mb, sb) = pack::seal(m.clone(), &items, key).unwrap();
        pack::zip_bytes(&format!("hsmp-{}", m.version), &items, &mb, &sb, 1_790_899_200).unwrap()
    }

    fn test_opts() -> InstallOpts {
        InstallOpts { backup_saves: true, ..Default::default() }
    }

    #[test]
    fn check_sends_user_agent_and_uses_the_etag() {
        let m = Mock::start();
        let list = serde_json::json!([release_json(&m, "0.2.0", b"z", false), release_json(&m, "0.3.0-beta.1", b"z", true)]);
        let path = format!("/repos/{REPO}/releases?per_page=30");
        m.route(&path, Route { headers: vec![("ETag".into(), "\"abc\"".into())], ..ok(list.to_string()) });
        let t = TempDir::new("upd_etag");
        let cache = t.path().join("update_check.json");
        let c = m.client();
        let a = c.check("0.1.0", false, Some(&cache)).unwrap().unwrap();
        assert_eq!(a.version, "0.3.0-beta.1");
        assert_eq!(a.ordering(), std::cmp::Ordering::Greater);
        assert!(a.notes().contains("  - better"));
        // the second check is answered 304 from the cache
        m.route(&path, Route { code: 304, headers: vec![], body: vec![], cut: None });
        assert_eq!(c.check("0.1.0", false, Some(&cache)).unwrap().unwrap().version, "0.3.0-beta.1");
        let heads = m.heads(&path);
        assert!(heads[0].to_ascii_lowercase().contains("user-agent: hsmp-launcher/"), "{}", heads[0]);
        assert!(heads[1].contains("If-None-Match: \"abc\"") || heads[1].to_ascii_lowercase().contains("if-none-match: \"abc\""), "{}", heads[1]);
        // stable channel: /releases/latest; none published yet is not an error
        assert_eq!(c.check("0.1.0", true, None).unwrap(), None);
    }

    #[test]
    fn rate_limit_is_explained() {
        let m = Mock::start();
        m.route(
            &format!("/repos/{REPO}/releases/latest"),
            Route { code: 403, headers: vec![("x-ratelimit-remaining".into(), "0".into()), ("x-ratelimit-reset".into(), "1790899200".into())], body: b"{}".to_vec(), cut: None },
        );
        let e = m.client().check("0.1.0", true, None).unwrap_err();
        assert!(e.contains("60 update checks per hour") && e.contains("resets at"), "{e}");
    }

    #[test]
    fn https_only_outside_tests() {
        let e = Client::new().get_small("http://127.0.0.1:9/x", 10).unwrap_err();
        assert!(e.to_ascii_lowercase().contains("https") || e.contains("insecure"), "{e}");
    }

    #[test]
    fn download_resumes_after_a_cut_and_honours_the_cap() {
        let m = Mock::start();
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        m.route("/big.zip", Route { cut: Some(100_000), ..ok(body.clone()) });
        let t = TempDir::new("upd_resume");
        let dest = t.path().join("hsmp-0.2.0.zip");
        let mut last = 0;
        m.client().download(&format!("{}/big.zip", m.base), &dest, body.len() as u64, MAX_ZIP, &mut |g, _| last = g).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(last, body.len() as u64);
        assert!(m.heads("/big.zip").iter().any(|h| h.to_ascii_lowercase().contains("range: bytes=")), "the second attempt resumed");
        // over the cap: refused before downloading
        let e = m.client().download(&format!("{}/big.zip", m.base), &t.path().join("x.zip"), body.len() as u64, 1000, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("more than"), "{e}");
        // the server sends more than the release lists
        let e = m.client().download(&format!("{}/big.zip", m.base), &t.path().join("y.zip"), 10, MAX_ZIP, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("release lists"), "{e}");
        assert!(!t.path().join("y.zip").exists() && !t.path().join("y.zip.part").exists());
    }

    #[test]
    fn sha_mismatch_is_rejected() {
        let m = Mock::start();
        let zip = b"PK not really".to_vec();
        publish(&m, "0.2.0", &zip, &"0".repeat(64));
        let r: Release = serde_json::from_value(release_json(&m, "0.2.0", &zip, false)).unwrap();
        let t = TempDir::new("upd_sha");
        let e = m.client().fetch_verified(&r, t.path(), &mut |_, _| {}).unwrap_err();
        assert!(e.contains("does not match its published SHA-256"), "{e}");
        assert!(!t.path().join("hsmp-0.2.0.zip").exists(), "the bad download is deleted");
        // the right hash passes, and a second fetch reuses the file
        publish(&m, "0.2.0", &zip, &util::sha256_hex(&zip));
        let p = m.client().fetch_verified(&r, t.path(), &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), zip);
        let n = m.heads("/dl/hsmp-0.2.0.zip").len();
        m.client().fetch_verified(&r, t.path(), &mut |_, _| {}).unwrap();
        assert_eq!(m.heads("/dl/hsmp-0.2.0.zip").len(), n);
    }

    /// The whole path: check, download, verify, install, against a fake game.
    #[test]
    fn end_to_end_update() {
        let f = fake(false);
        let key = test_key(1);
        let (m1, f1) = package(0);
        install::install(&f.env, &m1, &f1, &test_opts(), &mut |_| {}).unwrap();
        let (m2, f2) = package(1);
        let zip = zip_of(m2, &f2, &key);
        let mock = Mock::start();
        publish(&mock, "0.2.0", &zip, &util::sha256_hex(&zip));
        mock.route(&format!("/repos/{REPO}/releases/latest"), ok(release_json(&mock, "0.2.0", &zip, false).to_string()));
        let c = mock.client();
        let a = c.check("0.1.0", true, None).unwrap().unwrap();
        let p = c.fetch_verified(&a.release, &download_dir(&f.env.hsmp_home), &mut |_, _| {}).unwrap();
        let r = apply(&f.env, &p, &trusted(&key), false, test_opts(), &mut |_| {}).unwrap();
        assert_eq!(r.updated_from.as_deref(), Some("0.1.0"));
        assert!(matches!(install::status(&f.env).unwrap(), Status::Installed { version, .. } if version == "0.2.0"));
    }

    #[test]
    fn bad_signature_is_rejected() {
        let f = fake(false);
        let (m1, f1) = package(0);
        install::install(&f.env, &m1, &f1, &test_opts(), &mut |_| {}).unwrap();
        let before = snapshot(&f.env.game_root);
        let (m2, f2) = package(1);
        let t = TempDir::new("upd_sig");
        let zp = t.path().join("hsmp-0.2.0.zip");
        std::fs::write(&zp, zip_of(m2, &f2, &test_key(66))).unwrap();
        let e = apply(&f.env, &zp, &trusted(&test_key(1)), false, test_opts(), &mut |_| {}).unwrap_err();
        assert!(e.contains("does not trust"), "{e}");
        assert_eq!(snapshot(&f.env.game_root), before);
    }

    #[test]
    fn downgrade_and_unsupported_build_are_refused() {
        let f = fake(false);
        let key = test_key(1);
        let (m2, f2) = package(1);
        install::install(&f.env, &m2, &f2, &test_opts(), &mut |_| {}).unwrap();
        let before = snapshot(&f.env.game_root);
        let t = TempDir::new("upd_down");
        let (m1, f1) = package(0);
        let old = t.path().join("hsmp-0.1.0.zip");
        std::fs::write(&old, zip_of(m1.clone(), &f1, &key)).unwrap();
        let e = apply(&f.env, &old, &trusted(&key), false, test_opts(), &mut |_| {}).unwrap_err();
        assert!(e.contains("older than the installed 0.2.0"), "{e}");
        assert_eq!(snapshot(&f.env.game_root), before);
        // forced from the advanced menu
        apply(&f.env, &old, &trusted(&key), true, test_opts(), &mut |_| {}).unwrap();
        assert!(matches!(install::status(&f.env).unwrap(), Status::Installed { version, .. } if version == "0.1.0"));
        // a release for another game build
        let (mut m3, f3) = package(1);
        m3.version = "0.3.0".into();
        m3.game.builds[0].exe_sha256 = util::sha256_hex(b"another exe");
        let other = t.path().join("hsmp-0.3.0.zip");
        std::fs::write(&other, zip_of(m3, &f3, &key)).unwrap();
        let e = apply(&f.env, &other, &trusted(&key), false, test_opts(), &mut |_| {}).unwrap_err();
        assert!(e.contains("does not support this Half Sword build"), "{e}");
    }

    #[test]
    fn failed_update_rolls_back() {
        let f = fake(false);
        let key = test_key(1);
        let (m1, f1) = package(0);
        install::install(&f.env, &m1, &f1, &test_opts(), &mut |_| {}).unwrap();
        let before = snapshot(&f.env.game_root);
        let (m2, f2) = package(1);
        let t = TempDir::new("upd_rb");
        let zp = t.path().join("hsmp-0.2.0.zip");
        std::fs::write(&zp, zip_of(m2, &f2, &key)).unwrap();
        let o = InstallOpts { fail_after_writes: Some(2), ..test_opts() };
        apply(&f.env, &zp, &trusted(&key), false, o, &mut |_| {}).unwrap_err();
        assert_eq!(snapshot(&f.env.game_root), before);
        assert!(matches!(install::status(&f.env).unwrap(), Status::Installed { version, .. } if version == "0.1.0"));
    }

    #[test]
    fn prune_keeps_only_the_new_zip() {
        let t = TempDir::new("upd_prune");
        for n in ["hsmp-0.1.0.zip", "hsmp-0.2.0.zip", "hsmp-0.3.0.zip.part", "notes.txt"] {
            std::fs::write(t.path().join(n), b"x").unwrap();
        }
        prune_downloads(t.path(), &t.path().join("hsmp-0.2.0.zip"));
        let mut left: Vec<String> = std::fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec!["hsmp-0.2.0.zip", "notes.txt"]);
    }
}

//! Career save guard — layer 2 of career save isolation
//! (docs/players/career-saves.md).
//!
//! Layer 1 (`mods/shared/hsmp_saveguard.lua`) diverts the game's
//! save calls to `HSMP_<inst>_<slot>` while an MP session is live. This module
//! is the file-level safety net that does not depend on any hook working:
//!
//! * [`CareerGuard::enter`] (entering MP): copies every career `*.sav` in
//!   `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames` (everything except
//!   `HSMP_*.sav`) into `%LOCALAPPDATA%\HSMP\save_backups\<timestamp>\files\`
//!   with SHA-256s in `manifest.json` (state `open`), then prunes the backup
//!   root to the newest [`GuardConfig::keep`] (default 5) backups.
//! * [`CareerGuard::heartbeat`] (every [`HEARTBEAT`] while in MP): stamps `alive_ms`
//!   into the open manifest, so a later crash recovery knows when the MP
//!   window really ended.
//! * [`CareerGuard::leave`] (leaving MP): re-hashes the career files; any file
//!   that changed or disappeared is restored from the backup (the MP-modified
//!   copy is kept as `mp_modified\<name>` for `hsmp-tools gvas-diff`), any new
//!   career `.sav` is moved to `quarantine\`. The manifest is closed.
//! * [`CareerGuard::startup_check`] (sidecar startup): finds `open` manifests
//!   left by a sidecar that died (no heartbeat for [`GuardConfig::stale_ms`])
//!   and performs the same comparison, except that a career file modified
//!   *after* that session's last heartbeat (+ grace) is legitimate post-crash
//!   career play and is left alone (`skipped_newer`).
//!
//! All writes are atomic: copy/write to a temp file in the destination
//! directory, `sync_all`, then rename over the target (MoveFileEx
//! REPLACE_EXISTING on Windows), retried while the game holds the file.
//! A backup without a manifest is incomplete and is never used to restore.
//!
//! The guard never reads or writes `HSMP_*.sav` (the redirected MP slots) and
//! never descends into sub-directories of SaveGames.
//!
//! Everything here is blocking std I/O: call it from `spawn_blocking` (or
//! before the tokio runtime starts).

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

pub const MANIFEST: &str = "manifest.json";
pub const MANIFEST_VERSION: u32 = 1;
pub const MP_PREFIX: &str = "HSMP_";

#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// The game's SaveGames directory.
    pub save_dir: PathBuf,
    /// Root of the timestamped backups.
    pub backup_root: PathBuf,
    /// Backups to keep (newest first). Open (unrecovered) backups are never pruned.
    pub keep: usize,
    /// HSMP instance id, recorded in the manifest (diagnostics only).
    pub inst: String,
    /// An open manifest whose `alive_ms` is older than this belongs to a dead sidecar.
    pub stale_ms: u64,
    /// Crash recovery: a career file modified later than `alive_ms + grace_ms`
    /// is treated as legitimate post-session career play and not restored.
    pub grace_ms: u64,
}

impl GuardConfig {
    /// `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames` and `%LOCALAPPDATA%\HSMP\save_backups`.
    /// `None` when LOCALAPPDATA is not set (non-Windows dev boxes): the caller skips the guard.
    pub fn from_env(inst: &str) -> Option<Self> {
        let la = std::env::var_os("LOCALAPPDATA")?;
        let la = PathBuf::from(la);
        Some(Self::with_dirs(
            la.join("HalfSwordUE5").join("Saved").join("SaveGames"),
            la.join("HSMP").join("save_backups"),
            inst,
        ))
    }

    pub fn with_dirs(save_dir: PathBuf, backup_root: PathBuf, inst: &str) -> Self {
        GuardConfig { save_dir, backup_root, keep: 5, inst: inst.to_string(), stale_ms: 90_000, grace_ms: GRACE_MS }
    }
}

/// Heartbeat period of an open manifest (the sidecar's heartbeat task).
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// Crash-recovery grace after the last heartbeat: two heartbeats. A hard kill
/// ends the MP window at most HEARTBEAT after `alive_ms`; career play later
/// than this is never "restored".
pub const GRACE_MS: u64 = 10_000;

/// The sidecar that wrote an open manifest is still running: its PID is
/// alive AND that process was created before the manifest (a reused PID is
/// a different process). Injectable for tests.
pub type OwnerAlive = fn(pid: u32, created_ms: u64) -> bool;

fn owner_alive_default(pid: u32, created_ms: u64) -> bool {
    pid != 0 && crate::parent::alive_since(pid, created_ms)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub created_ms: u64,
    pub inst: String,
    pub pid: u32,
    /// "open" while the MP window is in progress, "closed" once verified.
    pub state: String,
    pub alive_ms: u64,
    #[serde(default)]
    pub closed_ms: Option<u64>,
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub result: Option<Report>,
}

/// Outcome of one comparison (leave or crash recovery).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub backup: String,
    /// "leave" or "recover".
    pub kind: String,
    pub checked: usize,
    pub unchanged: usize,
    /// Changed during MP, restored from the backup.
    pub restored: Vec<String>,
    /// Deleted during MP, restored from the backup.
    pub recreated: Vec<String>,
    /// New career `.sav` created during MP, moved to `quarantine\`.
    pub quarantined: Vec<String>,
    /// Recovery only: changed after the dead session ended; left alone.
    pub skipped_newer: Vec<String>,
    pub errors: Vec<String>,
}

impl Report {
    pub fn changed(&self) -> usize {
        self.restored.len() + self.recreated.len() + self.quarantined.len()
    }
    pub fn is_clean(&self) -> bool {
        self.changed() == 0 && self.errors.is_empty()
    }
}

/// The gate's view of a report: one `(action, file, why)` per thing the
/// guard did, so a layer-2 restore can never hide a layer-1 failure. Actions: `restored`,
/// `recreated`, `quarantined` (each a gate FAIL: a career file was touched during MP),
/// `error`, `skipped_newer` (crash recovery left post-session career play alone) and
/// `clean` (nothing changed).
pub fn report_actions(r: &Report) -> Vec<(&'static str, Option<String>, String)> {
    let mut v = Vec::new();
    for f in &r.restored {
        v.push(("restored", Some(f.clone()), "changed during the MP session; restored from the backup (MP copy kept in mp_modified)".to_string()));
    }
    for f in &r.recreated {
        v.push(("recreated", Some(f.clone()), "deleted during the MP session; recreated from the backup".to_string()));
    }
    for f in &r.quarantined {
        v.push(("quarantined", Some(f.clone()), "new career .sav created during the MP session; moved to quarantine".to_string()));
    }
    for f in &r.skipped_newer {
        v.push(("skipped_newer", Some(f.clone()), "changed after the dead session's last heartbeat: post-session career play, left alone".to_string()));
    }
    for e in &r.errors {
        v.push(("error", None, e.clone()));
    }
    if r.changed() == 0 && r.errors.is_empty() {
        v.push(("clean", None, format!("{} career file(s) unchanged", r.unchanged)));
    }
    v
}

#[derive(Debug, Clone)]
pub struct EnterReport {
    pub backup: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub pruned: Vec<String>,
}

pub struct CareerGuard {
    cfg: GuardConfig,
    session: Option<PathBuf>,
    owner_alive: OwnerAlive,
}

impl CareerGuard {
    pub fn new(cfg: GuardConfig) -> Self {
        CareerGuard { cfg, session: None, owner_alive: owner_alive_default }
    }

    /// Tests: replace the owner-process liveness check.
    pub fn with_owner_check(mut self, f: OwnerAlive) -> Self {
        self.owner_alive = f;
        self
    }

    pub fn config(&self) -> &GuardConfig {
        &self.cfg
    }

    /// The open backup of the current MP window, if any.
    pub fn session(&self) -> Option<&Path> {
        self.session.as_deref()
    }

    /// Sidecar startup: recover every open manifest whose sidecar stopped
    /// heartbeating (crash / kill). Fresh open manifests (another live
    /// instance on this machine) are left alone. Returns one report per
    /// recovered backup.
    pub fn startup_check(&mut self) -> io::Result<Vec<Report>> {
        let now = now_ms();
        let mut out = Vec::new();
        for dir in list_backups(&self.cfg.backup_root)? {
            if Some(dir.as_path()) == self.session.as_deref() {
                continue;
            }
            let Ok(m) = read_manifest(&dir) else { continue };
            if m.state != "open" {
                continue;
            }
            // Fresh heartbeat AND its sidecar still runs: another live instance.
            // A dead owner (killed with the game) is recovered at once,
            // whatever its last heartbeat.
            let fresh = now.saturating_sub(m.alive_ms) < self.cfg.stale_ms;
            if fresh && (self.owner_alive)(m.pid, m.created_ms) {
                info!(backup = %dir.display(), inst = %m.inst, "career guard: open backup is still heartbeating; not touching it");
                continue;
            }
            if fresh {
                info!(backup = %dir.display(), pid = m.pid, "career guard: open backup's sidecar is gone; recovering now");
            }
            let cutoff = m.alive_ms.saturating_add(self.cfg.grace_ms);
            let r = self.compare_and_restore(&dir, m, "recover", Some(cutoff))?;
            out.push(r);
        }
        Ok(out)
    }

    /// Entering MP: back up and hash every career `.sav`, write an open manifest,
    /// prune old backups. Calling it again while a session is open is a no-op
    /// that returns the existing backup.
    pub fn enter(&mut self) -> io::Result<EnterReport> {
        if let Some(dir) = &self.session {
            let m = read_manifest(dir)?;
            return Ok(EnterReport { backup: dir.clone(), files: m.files.len(), bytes: m.files.iter().map(|f| f.size).sum(), pruned: vec![] });
        }
        fs::create_dir_all(&self.cfg.backup_root)?;
        let dir = unique_backup_dir(&self.cfg.backup_root)?;
        let files_dir = dir.join("files");
        fs::create_dir_all(&files_dir)?;
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        for name in career_files(&self.cfg.save_dir)? {
            let src = self.cfg.save_dir.join(&name);
            let dst = files_dir.join(&name);
            // Copy first, then hash the COPY and verify it against the source,
            // so a save landing mid-copy cannot produce a manifest that lies.
            let mut tries = 0;
            let entry = loop {
                tries += 1;
                atomic_copy(&src, &dst)?;
                let h_copy = sha256_file(&dst)?;
                let h_src = sha256_file(&src)?;
                if h_copy == h_src || tries >= 5 {
                    if h_copy != h_src {
                        warn!(file = %name, "career guard: source kept changing while backing up; using the last copy");
                    }
                    let md = fs::metadata(&dst)?;
                    break FileEntry { name: name.clone(), size: md.len(), sha256: h_copy, mtime_ms: mtime_ms(&src) };
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            bytes += entry.size;
            entries.push(entry);
        }
        let now = now_ms();
        let m = Manifest {
            version: MANIFEST_VERSION,
            created_ms: now,
            inst: self.cfg.inst.clone(),
            pid: std::process::id(),
            state: "open".into(),
            alive_ms: now,
            closed_ms: None,
            files: entries,
            result: None,
        };
        write_manifest(&dir, &m)?; // last: a backup without a manifest is never used
        self.session = Some(dir.clone());
        let pruned = self.prune()?;
        info!(backup = %dir.display(), files = m.files.len(), bytes, "career guard: career saves backed up for the MP session");
        Ok(EnterReport { backup: dir, files: m.files.len(), bytes, pruned })
    }

    /// Stamp `alive_ms` into the open manifest (call every ~30 s while in MP).
    pub fn heartbeat(&self) -> io::Result<()> {
        let Some(dir) = &self.session else { return Ok(()) };
        let mut m = read_manifest(dir)?;
        m.alive_ms = now_ms();
        write_manifest(dir, &m)
    }

    /// Leaving MP: compare against the session backup, restore on mismatch,
    /// close the manifest. `Ok(None)` when no session was open.
    pub fn leave(&mut self) -> io::Result<Option<Report>> {
        let Some(dir) = self.session.take() else { return Ok(None) };
        let m = read_manifest(&dir)?;
        let r = self.compare_and_restore(&dir, m, "leave", None)?;
        Ok(Some(r))
    }

    fn compare_and_restore(&self, dir: &Path, mut m: Manifest, kind: &str, cutoff: Option<u64>) -> io::Result<Report> {
        let mut r = Report { backup: dir.display().to_string(), kind: kind.into(), ..Default::default() };
        let current = career_files(&self.cfg.save_dir)?;
        for f in &m.files {
            r.checked += 1;
            let path = self.cfg.save_dir.join(&f.name);
            let backup = dir.join("files").join(&f.name);
            if path.exists() {
                let h = match sha256_file(&path) {
                    Ok(h) => h,
                    Err(e) => {
                        r.errors.push(format!("{}: hash failed: {e}", f.name));
                        continue;
                    }
                };
                if h == f.sha256 {
                    r.unchanged += 1;
                    continue;
                }
                if let Some(c) = cutoff {
                    if mtime_ms(&path) > c {
                        r.skipped_newer.push(f.name.clone());
                        continue;
                    }
                }
                // keep the MP-modified copy for diagnosis (hsmp-tools gvas-diff)
                let keep_dir = dir.join("mp_modified");
                if let Err(e) = fs::create_dir_all(&keep_dir).and_then(|_| atomic_copy(&path, &keep_dir.join(&f.name))) {
                    r.errors.push(format!("{}: could not keep the MP copy: {e}", f.name));
                }
                match restore_file(&backup, &path, &f.sha256) {
                    Ok(()) => r.restored.push(f.name.clone()),
                    Err(e) => r.errors.push(format!("{}: restore failed: {e}", f.name)),
                }
            } else {
                match restore_file(&backup, &path, &f.sha256) {
                    Ok(()) => r.recreated.push(f.name.clone()),
                    Err(e) => r.errors.push(format!("{}: recreate failed: {e}", f.name)),
                }
            }
        }
        for name in current {
            if m.files.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let path = self.cfg.save_dir.join(&name);
            if let Some(c) = cutoff {
                if mtime_ms(&path) > c {
                    r.skipped_newer.push(name);
                    continue;
                }
            }
            let q = dir.join("quarantine");
            match fs::create_dir_all(&q).and_then(|_| move_file(&path, &q.join(&name))) {
                Ok(()) => r.quarantined.push(name),
                Err(e) => r.errors.push(format!("{name}: quarantine failed: {e}")),
            }
        }
        m.state = "closed".into();
        m.closed_ms = Some(now_ms());
        m.result = Some(r.clone());
        if let Err(e) = write_manifest(dir, &m) {
            r.errors.push(format!("manifest close failed: {e}"));
        }
        if r.is_clean() {
            info!(backup = %r.backup, kind, checked = r.checked, skipped_newer = r.skipped_newer.len(), "career guard: career saves unchanged");
        } else {
            warn!(
                backup = %r.backup, kind,
                restored = ?r.restored, recreated = ?r.recreated, quarantined = ?r.quarantined, errors = ?r.errors,
                "career guard: career save restored ({} files)", r.changed()
            );
        }
        Ok(r)
    }

    /// Keep the newest `keep` backups; delete older closed or manifest-less
    /// ones. Open backups and the current session are never deleted.
    fn prune(&self) -> io::Result<Vec<String>> {
        let mut pruned = Vec::new();
        let dirs = list_backups(&self.cfg.backup_root)?; // newest first
        for dir in dirs.into_iter().skip(self.cfg.keep) {
            if Some(dir.as_path()) == self.session.as_deref() {
                continue;
            }
            let deletable = match read_manifest(&dir) {
                Ok(m) => m.state != "open",
                Err(_) => true,
            };
            if deletable {
                match fs::remove_dir_all(&dir) {
                    Ok(()) => pruned.push(dir.display().to_string()),
                    Err(e) => warn!(backup = %dir.display(), "career guard: prune failed: {e}"),
                }
            }
        }
        Ok(pruned)
    }
}

// --- helpers -------------------------------------------------------------------

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime_ms(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Career save files: regular `*.sav` files directly in `save_dir`, except `HSMP_*`.
pub fn career_files(save_dir: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(save_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for ent in rd {
        let ent = ent?;
        if !ent.file_type()?.is_file() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".sav") || name.starts_with(MP_PREFIX) || name.starts_with('.') {
            continue;
        }
        out.push(name);
    }
    out.sort();
    Ok(out)
}

pub fn sha256_file(p: &Path) -> io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn tmp_sibling(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    dst.with_file_name(format!(".{name}.{}.hsmp_tmp", std::process::id()))
}

/// Rename with retries (the game may briefly hold the target open).
fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut last = None;
    for i in 0..20 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(25 * (i + 1)));
            }
        }
    }
    let _ = fs::remove_file(from);
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::Other, "rename failed")))
}

/// Copy `src` to `dst` atomically: temp sibling, fsync, rename over `dst`.
pub fn atomic_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let tmp = tmp_sibling(dst);
    let res = (|| {
        fs::copy(src, &tmp)?;
        fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        rename_retry(&tmp, dst)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

pub fn atomic_write(dst: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = tmp_sibling(dst);
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        rename_retry(&tmp, dst)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Restore `backup` over `target` and verify the result hashes to `sha`.
fn restore_file(backup: &Path, target: &Path, sha: &str) -> io::Result<()> {
    let hb = sha256_file(backup)?;
    if hb != sha {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("backup copy is corrupt (sha {hb} != manifest {sha})")));
    }
    atomic_copy(backup, target)?;
    let ht = sha256_file(target)?;
    if ht != sha {
        return Err(io::Error::new(io::ErrorKind::Other, format!("restored file hashes to {ht}, expected {sha}")));
    }
    Ok(())
}

/// Move a file (rename, or copy + delete across volumes).
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    atomic_copy(from, to)?;
    fs::remove_file(from)
}

fn read_manifest(dir: &Path) -> io::Result<Manifest> {
    let s = fs::read_to_string(dir.join(MANIFEST))?;
    serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn write_manifest(dir: &Path, m: &Manifest) -> io::Result<()> {
    let s = serde_json::to_string_pretty(m).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write(&dir.join(MANIFEST), s.as_bytes())
}

/// Backup dirs (named `YYYYMMDD-HHMMSS-mmm[-n]`), newest first.
fn list_backups(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    let rd = match fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(v),
        Err(e) => return Err(e),
    };
    for ent in rd {
        let ent = ent?;
        if !ent.file_type()?.is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        if is_backup_name(&name) {
            v.push(ent.path());
        }
    }
    v.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    Ok(v)
}

fn is_backup_name(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() >= 19
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'-'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'-'
        && b[16..19].iter().all(u8::is_ascii_digit)
}

fn unique_backup_dir(root: &Path) -> io::Result<PathBuf> {
    let base = timestamp_name(now_ms());
    for i in 0..1000u32 {
        let name = if i == 0 { base.clone() } else { format!("{base}-{i}") };
        let p = root.join(&name);
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free backup dir name"))
}

/// UTC `YYYYMMDD-HHMMSS-mmm` (sorts chronologically).
pub fn timestamp_name(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, mi, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{:03}", ms % 1000)
}

/// Days since 1970-01-01 -> (year, month, day). H. Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// --- tests -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// Throw-away temp tree: <tmp>/SaveGames and <tmp>/backups. Never the real dirs.
    struct T {
        root: PathBuf,
    }
    impl T {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hsmp_cg_test_{}_{}_{}",
                std::process::id(),
                now_ms(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("SaveGames")).unwrap();
            T { root }
        }
        fn saves(&self) -> PathBuf {
            self.root.join("SaveGames")
        }
        fn cfg(&self) -> GuardConfig {
            GuardConfig::with_dirs(self.saves(), self.root.join("backups"), "1")
        }
        fn put(&self, name: &str, data: &[u8]) {
            fs::write(self.saves().join(name), data).unwrap();
        }
        fn get(&self, name: &str) -> Option<Vec<u8>> {
            fs::read(self.saves().join(name)).ok()
        }
    }
    impl Drop for T {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn career(t: &T) {
        t.put("GameProgress.sav", b"career-progress-v1");
        t.put("Settings.sav", b"settings-v1");
        t.put("SG Gauntlet Progress.sav", b"gauntlet");
        t.put("HSMP_1_GameProgress.sav", b"mp-slot");
        t.put("GameProgress.sav.hsmp_aside", b"not a sav");
        fs::create_dir_all(t.saves().join("backup")).unwrap();
        fs::write(t.saves().join("backup").join("old.sav"), b"nested").unwrap();
    }

    #[test]
    fn career_file_selection() {
        let t = T::new();
        career(&t);
        let v = career_files(&t.saves()).unwrap();
        assert_eq!(v, vec!["GameProgress.sav", "SG Gauntlet Progress.sav", "Settings.sav"]);
    }

    #[test]
    fn enter_backs_up_with_hashes() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        assert_eq!(er.files, 3);
        let m = read_manifest(&er.backup).unwrap();
        assert_eq!(m.state, "open");
        assert_eq!(m.inst, "1");
        let gp = m.files.iter().find(|f| f.name == "GameProgress.sav").unwrap();
        assert_eq!(gp.sha256, hex::encode(Sha256::digest(b"career-progress-v1")));
        assert_eq!(fs::read(er.backup.join("files").join("GameProgress.sav")).unwrap(), b"career-progress-v1");
        assert!(!er.backup.join("files").join("HSMP_1_GameProgress.sav").exists(), "MP slots are not career");
        // entering twice is idempotent
        let er2 = g.enter().unwrap();
        assert_eq!(er2.backup, er.backup);
        // no temp files left anywhere
        for d in [t.saves(), er.backup.clone(), er.backup.join("files")] {
            for e in fs::read_dir(d).unwrap() {
                assert!(!e.unwrap().file_name().to_string_lossy().ends_with(".hsmp_tmp"));
            }
        }
    }

    #[test]
    fn leave_clean_session() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        g.enter().unwrap();
        t.put("HSMP_1_GameProgress.sav", b"mp wrote its own slot"); // allowed
        let r = g.leave().unwrap().unwrap();
        assert!(r.is_clean(), "{r:?}");
        assert_eq!(r.checked, 3);
        assert_eq!(r.unchanged, 3);
        assert_eq!(t.get("HSMP_1_GameProgress.sav").unwrap(), b"mp wrote its own slot");
        let m = read_manifest(Path::new(&r.backup)).unwrap();
        assert_eq!(m.state, "closed");
        assert!(m.result.unwrap().is_clean());
        assert!(g.leave().unwrap().is_none(), "second leave is a no-op");
    }

    #[test]
    fn leave_restores_changed_deleted_and_quarantines_new() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        t.put("GameProgress.sav", b"MP CLOBBERED THE CAREER");
        fs::remove_file(t.saves().join("Settings.sav")).unwrap();
        t.put("Save_Crash.sav", b"created during MP");
        let r = g.leave().unwrap().unwrap();
        assert_eq!(r.restored, vec!["GameProgress.sav"]);
        assert_eq!(r.recreated, vec!["Settings.sav"]);
        assert_eq!(r.quarantined, vec!["Save_Crash.sav"]);
        assert!(r.errors.is_empty(), "{r:?}");
        assert_eq!(t.get("GameProgress.sav").unwrap(), b"career-progress-v1");
        assert_eq!(t.get("Settings.sav").unwrap(), b"settings-v1");
        assert!(t.get("Save_Crash.sav").is_none());
        assert_eq!(fs::read(er.backup.join("mp_modified").join("GameProgress.sav")).unwrap(), b"MP CLOBBERED THE CAREER");
        assert_eq!(fs::read(er.backup.join("quarantine").join("Save_Crash.sav")).unwrap(), b"created during MP");
        assert_eq!(r.changed(), 3);
        // the gate sees every action (a restore is a gate failure, never silent)
        let acts: Vec<(&str, Option<String>)> = report_actions(&r).into_iter().map(|(a, f, _)| (a, f)).collect();
        assert_eq!(acts, vec![
            ("restored", Some("GameProgress.sav".to_string())),
            ("recreated", Some("Settings.sav".to_string())),
            ("quarantined", Some("Save_Crash.sav".to_string())),
        ]);
    }

    #[test]
    fn clean_report_is_one_clean_action() {
        let r = Report { checked: 3, unchanged: 3, ..Default::default() };
        let a = report_actions(&r);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, "clean");
        let r = Report { errors: vec!["x: hash failed".into()], ..Default::default() };
        assert_eq!(report_actions(&r)[0].0, "error");
    }

    #[test]
    fn corrupt_backup_is_never_restored() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        fs::write(er.backup.join("files").join("GameProgress.sav"), b"bit rot").unwrap();
        t.put("GameProgress.sav", b"changed");
        let r = g.leave().unwrap().unwrap();
        assert!(r.restored.is_empty());
        assert_eq!(r.errors.len(), 1, "{r:?}");
        assert_eq!(t.get("GameProgress.sav").unwrap(), b"changed", "a bad backup must not overwrite anything");
    }

    #[test]
    fn startup_recovers_a_crashed_session() {
        let t = T::new();
        career(&t);
        let dir = {
            let mut g = CareerGuard::new(t.cfg());
            let er = g.enter().unwrap();
            // the sidecar dies here; fake an old heartbeat
            let mut m = read_manifest(&er.backup).unwrap();
            m.alive_ms = now_ms() - 10 * 60_000;
            write_manifest(&er.backup, &m).unwrap();
            er.backup
        };
        // a write during the MP window: mtime well before the cutoff
        t.put("GameProgress.sav", b"clobbered during MP");
        set_mtime_ms(&t.saves().join("GameProgress.sav"), now_ms() - 20 * 60_000);
        let mut g2 = CareerGuard::new(t.cfg());
        let rs = g2.startup_check().unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].kind, "recover");
        assert_eq!(rs[0].restored, vec!["GameProgress.sav"]);
        assert_eq!(t.get("GameProgress.sav").unwrap(), b"career-progress-v1");
        assert_eq!(read_manifest(&dir).unwrap().state, "closed");
        assert!(g2.startup_check().unwrap().is_empty(), "closed manifests are not re-processed");
    }

    #[test]
    fn startup_keeps_career_play_after_the_crash() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        let mut m = read_manifest(&er.backup).unwrap();
        m.alive_ms = now_ms() - 60 * 60_000; // session died an hour ago
        write_manifest(&er.backup, &m).unwrap();
        // then the user played career normally (fresh mtime) and made a new slot
        t.put("GameProgress.sav", b"legit career progress");
        t.put("New Career Slot.sav", b"legit");
        let rs = CareerGuard::new(t.cfg()).startup_check().unwrap();
        assert_eq!(rs.len(), 1);
        assert!(rs[0].restored.is_empty() && rs[0].quarantined.is_empty(), "{:?}", rs[0]);
        assert_eq!(rs[0].skipped_newer.len(), 2);
        assert_eq!(t.get("GameProgress.sav").unwrap(), b"legit career progress");
        assert!(t.get("New Career Slot.sav").is_some());
    }

    #[test]
    fn startup_ignores_a_live_session() {
        let t = T::new();
        career(&t);
        let mut a = CareerGuard::new(t.cfg());
        a.enter().unwrap();
        a.heartbeat().unwrap();
        t.put("GameProgress.sav", b"changed");
        let mut b = CareerGuard::new(GuardConfig { inst: "2".into(), ..t.cfg() });
        assert!(b.startup_check().unwrap().is_empty(), "another instance's fresh session is not ours to judge");
        assert_eq!(t.get("GameProgress.sav").unwrap(), b"changed");
        // ...but the owner's leave() restores it
        let r = a.leave().unwrap().unwrap();
        assert_eq!(r.restored, vec!["GameProgress.sav"]);
    }

    /// A game quit kills the sidecar (taskkill /F); its manifest has a
    /// fresh heartbeat but a dead PID. The next sidecar recovers it at once
    /// instead of waiting 90 s (and backing up MP-modified saves meanwhile).
    #[test]
    fn startup_recovers_a_fresh_manifest_whose_sidecar_is_dead() {
        let t = T::new();
        career(&t);
        let dir = {
            let mut g = CareerGuard::new(t.cfg());
            let er = g.enter().unwrap();
            let mut m = read_manifest(&er.backup).unwrap();
            m.pid = 0x7FFF_FFF0; // no such process
            m.alive_ms = now_ms() - 2_000; // heartbeat 2 s ago
            write_manifest(&er.backup, &m).unwrap();
            er.backup
        };
        t.put("GameProgress.sav", b"clobbered during MP");
        set_mtime_ms(&t.saves().join("GameProgress.sav"), now_ms() - 30_000);
        let rs = CareerGuard::new(t.cfg()).startup_check().unwrap();
        assert_eq!(rs.len(), 1, "dead owner: recovered despite the fresh heartbeat");
        assert_eq!(rs[0].restored, vec!["GameProgress.sav"]);
        assert_eq!(read_manifest(&dir).unwrap().state, "closed");
    }

    #[test]
    fn startup_treats_a_reused_pid_as_dead() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        let mut m = read_manifest(&er.backup).unwrap();
        m.pid = std::process::id(); // alive...
        m.created_ms = 1; // ...but started long after this manifest was written
        write_manifest(&er.backup, &m).unwrap();
        let rs = CareerGuard::new(t.cfg()).startup_check().unwrap();
        assert_eq!(rs.len(), 1, "a younger process with the old pid is not the owner");
        // the injected check is what decides
        let t2 = T::new();
        career(&t2);
        CareerGuard::new(t2.cfg()).enter().unwrap();
        let rs = CareerGuard::new(t2.cfg()).with_owner_check(|_, _| true).startup_check().unwrap();
        assert!(rs.is_empty());
    }

    #[test]
    fn heartbeat_and_grace_are_tight() {
        assert!(HEARTBEAT <= Duration::from_secs(5));
        assert!(GRACE_MS <= 2 * HEARTBEAT.as_millis() as u64);
        assert_eq!(GuardConfig::with_dirs(PathBuf::new(), PathBuf::new(), "0").grace_ms, GRACE_MS);
    }

    #[test]
    fn heartbeat_updates_alive() {
        let t = T::new();
        career(&t);
        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        let mut m = read_manifest(&er.backup).unwrap();
        m.alive_ms = 1;
        write_manifest(&er.backup, &m).unwrap();
        g.heartbeat().unwrap();
        assert!(read_manifest(&er.backup).unwrap().alive_ms > 1);
        CareerGuard::new(t.cfg()).heartbeat().unwrap(); // no session: no-op
    }

    #[test]
    fn keeps_five_newest_and_never_prunes_open() {
        let t = T::new();
        career(&t);
        let root = t.root.join("backups");
        fs::create_dir_all(&root).unwrap();
        // 6 old closed backups, 1 old open one, 1 junk dir without manifest
        for i in 0..6 {
            let d = root.join(format!("2026010{}-000000-000", i + 1));
            fs::create_dir_all(&d).unwrap();
            let m = Manifest {
                version: 1, created_ms: 0, inst: "1".into(), pid: 0, state: "closed".into(),
                alive_ms: 0, closed_ms: Some(0), files: vec![], result: None,
            };
            write_manifest(&d, &m).unwrap();
        }
        let open = root.join("20250101-000000-000");
        fs::create_dir_all(&open).unwrap();
        let mo = Manifest {
            version: 1, created_ms: 0, inst: "9".into(), pid: 0, state: "open".into(),
            alive_ms: now_ms(), closed_ms: None, files: vec![], result: None,
        };
        write_manifest(&open, &mo).unwrap();
        fs::create_dir_all(root.join("20240101-000000-000")).unwrap(); // incomplete
        fs::create_dir_all(root.join("not-a-backup")).unwrap();

        let mut g = CareerGuard::new(t.cfg());
        let er = g.enter().unwrap();
        let left: Vec<String> = list_backups(&root).unwrap().iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(left.len(), 6, "{left:?}"); // 5 newest + the open one
        assert_eq!(left[0], er.backup.file_name().unwrap().to_string_lossy());
        assert!(left.contains(&"20250101-000000-000".to_string()), "open backup survives");
        assert!(!left.contains(&"20240101-000000-000".to_string()), "incomplete backup pruned");
        assert!(!left.contains(&"20260101-000000-000".to_string()), "oldest closed pruned");
        assert!(root.join("not-a-backup").exists(), "foreign dirs untouched");
        assert_eq!(er.pruned.len(), 3);
    }

    #[test]
    fn missing_save_dir_is_empty_not_an_error() {
        let t = T::new();
        let cfg = GuardConfig::with_dirs(t.root.join("nope"), t.root.join("backups"), "0");
        let mut g = CareerGuard::new(cfg);
        assert_eq!(g.enter().unwrap().files, 0);
        assert!(g.leave().unwrap().unwrap().is_clean());
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let t = T::new();
        let p = t.saves().join("x.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read_dir(t.saves()).unwrap().count(), 1);
    }

    #[test]
    fn timestamp_names() {
        assert_eq!(timestamp_name(0), "19700101-000000-000");
        assert_eq!(timestamp_name(1_759_340_000_123), "20251001-173320-123");
        assert_eq!(timestamp_name(951_782_400_000), "20000229-000000-000");
        assert!(is_backup_name("20261001-120000-001-2"));
        assert!(!is_backup_name("hsmp_backup_20261001_192535"));
    }

    fn set_mtime_ms(p: &Path, ms: u64) {
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_millis(ms)).unwrap();
    }
}

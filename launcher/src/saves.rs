//! Career save backups.
//!
//! Half Sword keeps its saves in %LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames
//! (GameProgress.sav = the career, Settings.sav, ...). The launcher copies the
//! whole folder (except HSMP's own `hsmp_*` backup folders) to
//! %LOCALAPPDATA%\HSMP\save_backups\<stamp>\ before the first install and on
//! request, verifies every copy by SHA-256 and NEVER deletes a backup.
//! Restoring first takes a safety backup of the current saves.

use crate::util;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const INDEX: &str = "backup.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedFile {
    pub rel: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub created_utc: String,
    pub reason: String,
    pub source: String,
    pub files: Vec<SavedFile>,
    /// The files live in this sub-folder of the backup (the career guard's MP backups keep
    /// them in `files\`); None = directly in the backup folder (the launcher's own backups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    /// A career-guard MP backup (the sidecar's `manifest.json`), not one the launcher made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub guard: bool,
}

impl Backup {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn has_career(&self) -> bool {
        self.files.iter().any(|f| f.rel.eq_ignore_ascii_case("GameProgress.sav"))
    }
}

fn skip_top(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    rel.contains('/') && first.to_ascii_lowercase().starts_with("hsmp_")
}

/// Copy every save file to a new backup folder under `backups_root`.
/// Returns Ok(None) when there is nothing to back up (no save folder / empty).
pub fn backup(save_dir: &Path, backups_root: &Path, reason: &str) -> Result<Option<Backup>, String> {
    let rels: Vec<String> = util::list_files_rel(save_dir).map_err(|e| format!("cannot list {}: {e}", save_dir.display()))?.into_iter().filter(|r| !skip_top(r)).collect();
    if rels.is_empty() {
        return Ok(None);
    }
    let now = util::now_unix();
    let mut id = util::stamp_utc(now);
    let mut n = 1;
    while backups_root.join(&id).exists() {
        n += 1;
        id = format!("{}_{n}", util::stamp_utc(now));
    }
    let dir = backups_root.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut files = vec![];
    for rel in rels {
        let src = util::join_rel(save_dir, &rel);
        let bytes = std::fs::read(&src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        let sha = util::sha256_hex(&bytes);
        let dst = util::join_rel(&dir, &rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::write(&dst, &bytes).map_err(|e| format!("cannot write {}: {e}", dst.display()))?;
        let (check, _) = util::sha256_file(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        if check != sha {
            return Err(format!("backup copy of {rel} does not match the original (disk problem?)"));
        }
        files.push(SavedFile { rel, sha256: sha, size: bytes.len() as u64 });
    }
    let b = Backup { id: id.clone(), created_utc: util::iso_utc(now), reason: reason.into(), source: save_dir.display().to_string(), files, payload: None, guard: false };
    let json = serde_json::to_vec_pretty(&b).map_err(|e| e.to_string())?;
    util::atomic_write(&dir.join(INDEX), &json).map_err(|e| format!("cannot write backup index: {e}"))?;
    Ok(Some(b))
}

/// The sidecar career guard's backup index (server/src/sidecar/career_guard.rs `Manifest`).
pub const GUARD_INDEX: &str = "manifest.json";

#[derive(Deserialize)]
struct GuardFile {
    name: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct GuardManifest {
    created_ms: u64,
    #[serde(default)]
    inst: String,
    #[serde(default)]
    state: String,
    files: Vec<GuardFile>,
}

/// A career-guard MP backup as a restorable [`Backup`], so Restore lists the guard's copies of
/// a career an MP crash damaged next to the launcher's own backups. Plain file names only
/// (the guard never descends into sub-folders).
fn guard_backup(dir: &Path) -> Option<Backup> {
    let m: GuardManifest = serde_json::from_slice(&std::fs::read(dir.join(GUARD_INDEX)).ok()?).ok()?;
    if m.files.iter().any(|f| f.name.contains(['/', '\\']) || !util::is_safe_rel_all([f.name.as_str()].into_iter())) {
        return None;
    }
    let id = dir.file_name()?.to_str()?.to_string();
    let open = if m.state == "open" { "; that MP session never finished its save check" } else { "" };
    Some(Backup {
        id,
        created_utc: util::iso_utc(m.created_ms / 1000),
        reason: format!("career guard: saves before an MP session (instance {}){open}", if m.inst.is_empty() { "?" } else { &m.inst }),
        source: String::new(),
        files: m.files.into_iter().map(|f| SavedFile { rel: f.name, sha256: f.sha256.to_ascii_lowercase(), size: f.size }).collect(),
        payload: Some("files".into()),
        guard: true,
    })
}

/// Where a backup's files are.
fn file_dir(dir: &Path, b: &Backup) -> PathBuf {
    match &b.payload {
        Some(p) => util::join_rel(dir, p),
        None => dir.to_path_buf(),
    }
}

/// All valid backups (the launcher's and the career guard's), newest first.
pub fn list(backups_root: &Path) -> Vec<(PathBuf, Backup)> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(backups_root) {
        for e in rd.flatten() {
            let idx = e.path().join(INDEX);
            if let Some(b) = std::fs::read(&idx).ok().and_then(|t| serde_json::from_slice::<Backup>(&t).ok()) {
                out.push((e.path(), b));
            } else if let Some(b) = guard_backup(&e.path()) {
                out.push((e.path(), b));
            }
        }
    }
    out.sort_by(|a, b| b.1.created_utc.cmp(&a.1.created_utc).then_with(|| b.1.id.cmp(&a.1.id)));
    out
}

/// Check every file of a backup against its recorded hash.
pub fn verify(dir: &Path, b: &Backup) -> Result<(), String> {
    for f in &b.files {
        let p = util::join_rel(&file_dir(dir, b), &f.rel);
        let (h, _) = util::sha256_file(&p).map_err(|e| format!("backup file {} unreadable: {e}", f.rel))?;
        if h != f.sha256 {
            return Err(format!("backup file {} is damaged (hash mismatch); not restoring", f.rel));
        }
    }
    Ok(())
}

/// Restore a backup over `save_dir`. Takes a safety backup of the current
/// saves first (returned). Files that are not in the backup are left alone.
pub fn restore(dir: &Path, b: &Backup, save_dir: &Path, backups_root: &Path) -> Result<Option<Backup>, String> {
    if !util::is_safe_rel_all(b.files.iter().map(|f| f.rel.as_str())) {
        return Err("backup index has unsafe paths".into());
    }
    verify(dir, b)?;
    let safety = backup(save_dir, backups_root, &format!("automatic safety copy before restoring {}", b.id))?;
    for f in &b.files {
        let src = util::join_rel(&file_dir(dir, b), &f.rel);
        let dst = util::join_rel(save_dir, &f.rel);
        let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        if util::sha256_hex(&bytes) != f.sha256 {
            return Err(format!("backup file {} changed while restoring", f.rel));
        }
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        util::atomic_write(&dst, &bytes).map_err(|e| format!("cannot write {}: {e} (is the game running?)", dst.display()))?;
    }
    Ok(safety)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{snapshot, TempDir};

    #[test]
    fn backup_and_restore() {
        let t = TempDir::new("saves");
        let saves = t.path().join("SaveGames");
        let root = t.path().join("backups");
        std::fs::create_dir_all(saves.join("backup")).unwrap();
        std::fs::create_dir_all(saves.join("hsmp_backup_1")).unwrap();
        std::fs::write(saves.join("GameProgress.sav"), b"career v1").unwrap();
        std::fs::write(saves.join("Settings.sav"), b"settings").unwrap();
        std::fs::write(saves.join("backup/old.sav"), b"old").unwrap();
        std::fs::write(saves.join("hsmp_backup_1/GameProgress.sav"), b"ours").unwrap();
        let before = snapshot(&saves);

        let b = backup(&saves, &root, "test").unwrap().unwrap();
        assert!(b.has_career());
        assert_eq!(b.files.len(), 3, "hsmp_* folders skipped: {:?}", b.files);
        let list1 = list(&root);
        assert_eq!(list1.len(), 1);

        // the career gets overwritten, a new slot appears
        std::fs::write(saves.join("GameProgress.sav"), b"career CLOBBERED").unwrap();
        std::fs::write(saves.join("HSMP_0_GameProgress.sav"), b"mp").unwrap();
        let (dir, b) = &list1[0];
        let safety = restore(dir, b, &saves, &root).unwrap().unwrap();
        assert!(safety.files.iter().any(|f| f.rel == "HSMP_0_GameProgress.sav"));
        assert_eq!(std::fs::read(saves.join("GameProgress.sav")).unwrap(), b"career v1");
        std::fs::remove_file(saves.join("HSMP_0_GameProgress.sav")).unwrap();
        assert_eq!(snapshot(&saves), before);
        assert_eq!(list(&root).len(), 2, "safety backup kept; nothing deleted");

        // damaged backup is refused
        std::fs::write(dir.join("GameProgress.sav"), b"bitrot").unwrap();
        assert!(restore(dir, b, &saves, &root).unwrap_err().contains("damaged"));
    }

    /// The career guard's MP backups (sidecar manifest.json + files\) are listed
    /// next to the launcher's and can be restored; a damaged or unsafe one is not.
    #[test]
    fn career_guard_backups_are_listed_and_restorable() {
        let t = TempDir::new("saves_guard");
        let saves = t.path().join("SaveGames");
        let root = t.path().join("save_backups");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("GameProgress.sav"), b"MP damaged").unwrap();
        let g = root.join("20261002-120000-123");
        std::fs::create_dir_all(g.join("files")).unwrap();
        std::fs::write(g.join("files/GameProgress.sav"), b"career").unwrap();
        let man = serde_json::json!({"version": 1, "created_ms": 1_790_942_400_123u64, "inst": "2", "pid": 77, "state": "open", "alive_ms": 1_790_942_401_000u64,
            "files": [{"name": "GameProgress.sav", "size": 6, "sha256": util::sha256_hex(b"career").to_uppercase(), "mtime_ms": 1}]});
        std::fs::write(g.join("manifest.json"), serde_json::to_vec(&man).unwrap()).unwrap();
        // an unsafe guard manifest is not listed
        let bad = root.join("20261002-120000-124");
        std::fs::create_dir_all(bad.join("files")).unwrap();
        std::fs::write(bad.join("manifest.json"), br#"{"created_ms": 1, "files": [{"name": "..\\x.sav", "size": 1, "sha256": "00"}]}"#).unwrap();
        let l = list(&root);
        assert_eq!(l.len(), 1, "{l:?}");
        let (dir, b) = &l[0];
        assert!(b.guard && b.has_career());
        assert!(b.reason.contains("instance 2") && b.reason.contains("never finished"), "{}", b.reason);
        restore(dir, b, &saves, &root).unwrap();
        assert_eq!(std::fs::read(saves.join("GameProgress.sav")).unwrap(), b"career");
        // the safety copy of the damaged file is a launcher backup; both listed
        assert_eq!(list(&root).len(), 2);
        // a damaged guard copy is refused
        std::fs::write(g.join("files/GameProgress.sav"), b"bitrot").unwrap();
        assert!(restore(dir, b, &saves, &root).unwrap_err().contains("damaged"));
    }

    #[test]
    fn nothing_to_back_up() {
        let t = TempDir::new("saves_empty");
        assert!(backup(&t.path().join("missing"), &t.path().join("b"), "x").unwrap().is_none());
    }
}

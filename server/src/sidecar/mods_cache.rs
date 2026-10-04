//! Server mods: the sidecar's content-addressed cache (docs/hosting/server-mods.md).
//!
//! `<cache>/<mod hash hex>/<path inside the mod>`, with `<cache>` = `--mods-cache`
//! (HSMPMenu passes `hsmp_mods`, next to the game's `Win64` folder: outside `ue4ss/Mods`, so
//! UE4SS never loads a server mod by itself on a later start). Only this module writes there.
//!
//! * **Nothing unverified is written.** [`commit`] checks every file's size and SHA-256
//!   against the validated manifest first; one mismatch and nothing is written at all. Then
//!   it writes into a fresh `.tmp-<hash>-<random>` folder and renames it into place, so a
//!   cache folder is always complete.
//! * **Every path is the manifest's**, checked by `manifest::check_path` (relative, no `..`,
//!   no drive or device names), joined component by component and checked again to stay
//!   inside the folder being written.
//! * **A cached mod is verified before each use** ([`verify`]): the exact file list, sizes
//!   and hashes. Anything else (a changed, missing or extra file) removes the folder and the
//!   mod is downloaded again.

use super::server_mods::manifest::{self, ModEntry};
use std::path::{Path, PathBuf};

/// Why a commit failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CacheError {
    /// A file's bytes did not match the manifest (nothing was written).
    Mismatch(String),
    /// The disk refused (permissions, space).
    Disk(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Mismatch(p) => write!(f, "{p} does not match the server's announced hash"),
            CacheError::Disk(e) => write!(f, "cannot write the mod cache: {e}"),
        }
    }
}

/// The folder of a mod in the cache.
pub(crate) fn mod_dir(root: &Path, m: &ModEntry) -> PathBuf {
    root.join(m.hash_hex())
}

/// `base` + a manifest path, refusing anything that would leave `base`.
fn join_inside(base: &Path, rel: &str) -> Result<PathBuf, CacheError> {
    manifest::check_path(rel).map_err(|e| CacheError::Mismatch(format!("{rel}: {e}")))?;
    let mut p = base.to_path_buf();
    for c in rel.split('/') {
        p.push(c);
    }
    if !p.starts_with(base) || p.components().count() != base.components().count() + rel.split('/').count() {
        return Err(CacheError::Mismatch(format!("{rel}: leaves the mod folder")));
    }
    Ok(p)
}

/// Every regular file under `dir`, relative with '/'. None on a symlink or an unreadable
/// folder (the caller treats the folder as broken).
fn list_files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Option<()> {
    for e in std::fs::read_dir(dir).ok()? {
        let e = e.ok()?;
        let name = e.file_name().to_str()?.to_string();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let md = std::fs::symlink_metadata(e.path()).ok()?;
        if md.file_type().is_symlink() {
            return None;
        } else if md.is_dir() {
            list_files(&e.path(), &r, out)?;
        } else if md.is_file() {
            out.push(r);
        } else {
            return None;
        }
    }
    Some(())
}

/// Is `m` in the cache, exactly (the file list, every size and hash)?
pub(crate) fn verify(root: &Path, m: &ModEntry) -> bool {
    let dir = mod_dir(root, m);
    if !dir.is_dir() {
        return false;
    }
    let mut files = Vec::new();
    if list_files(&dir, "", &mut files).is_none() {
        return false;
    }
    files.sort();
    let mut want: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
    want.sort_unstable();
    if files != want {
        return false;
    }
    m.files.iter().all(|f| {
        join_inside(&dir, &f.path).ok().and_then(|p| std::fs::read(p).ok())
            .is_some_and(|b| b.len() as u64 == f.size && manifest::sha256(&b) == f.sha256)
    })
}

/// Remove a mod's folder (broken or stale); best effort.
pub(crate) fn remove(root: &Path, m: &ModEntry) {
    let _ = std::fs::remove_dir_all(mod_dir(root, m));
}

/// Check `data` (one buffer per file, in the mod's file order) against the manifest and
/// write the mod into the cache. Nothing is written unless every file matches.
pub(crate) fn commit(root: &Path, m: &ModEntry, data: &[Vec<u8>]) -> Result<PathBuf, CacheError> {
    if data.len() != m.files.len() {
        return Err(CacheError::Mismatch(format!("mod {}: {} files expected", m.name, m.files.len())));
    }
    for (f, b) in m.files.iter().zip(data) {
        if b.len() as u64 != f.size || manifest::sha256(b) != f.sha256 {
            return Err(CacheError::Mismatch(format!("mod {}: {}", m.name, f.path)));
        }
        join_inside(root, &f.path)?;
    }
    let dest = mod_dir(root, m);
    if verify(root, m) {
        return Ok(dest);
    }
    std::fs::create_dir_all(root).map_err(|e| CacheError::Disk(format!("{}: {e}", root.display())))?;
    let tmp = root.join(format!(".tmp-{}-{:08x}", m.hash_hex(), rand::random::<u32>()));
    let write = || -> Result<(), CacheError> {
        for (f, b) in m.files.iter().zip(data) {
            let p = join_inside(&tmp, &f.path)?;
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| CacheError::Disk(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&p, b).map_err(|e| CacheError::Disk(format!("{}: {e}", p.display())))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    // A broken copy in the way is replaced (verify said it is not this mod).
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_dir_all(&tmp);
        // Another sidecar (a second game instance) may have committed the same mod meanwhile.
        if verify(root, m) {
            return Ok(dest);
        }
        return Err(CacheError::Disk(format!("{}: {e}", dest.display())));
    }
    Ok(dest)
}

/// Remove `.tmp-*` folders a crash left behind (older than an hour: another instance may be
/// writing one right now).
pub(crate) fn sweep(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 3600);
        if name.starts_with(".tmp-") && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{seal, FileEntry};

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("hsmp-cache-{}-{}", std::process::id(), rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn one_mod(files: &[(&str, &[u8])]) -> ModEntry {
        let files = files.iter().map(|(p, b)| FileEntry { path: p.to_string(), size: b.len() as u64, sha256: manifest::sha256(b) }).collect();
        seal(vec![ModEntry { name: "arena".into(), files, ..Default::default() }]).mods.remove(0)
    }

    /// Verify-and-commit: a tampered byte is rejected with nothing written anywhere; good
    /// bytes land in `<root>/<hash>/` exactly; the copy verifies; a changed or extra file
    /// fails verification.
    #[test]
    fn tampered_bytes_are_rejected_and_nothing_is_written() {
        let root = tmp();
        let m = one_mod(&[("Scripts/main.lua", b"print('hi')"), ("Scripts/lib/u.lua", b"return 1"), ("mod.json", b"{}")]);
        let data: Vec<Vec<u8>> = m.files.iter().map(|f| match f.path.as_str() {
            "Scripts/main.lua" => b"print('hi')".to_vec(),
            "Scripts/lib/u.lua" => b"return 1".to_vec(),
            _ => b"{}".to_vec(),
        }).collect();
        // one flipped byte in any file: refused, the root stays empty
        for i in 0..data.len() {
            let mut bad = data.clone();
            bad[i][0] ^= 1;
            assert!(matches!(commit(&root, &m, &bad), Err(CacheError::Mismatch(_))));
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "nothing written");
        }
        // a short file, a missing file
        let mut short = data.clone();
        short[0].pop();
        assert!(commit(&root, &m, &short).is_err());
        assert!(commit(&root, &m, &data[..2]).is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        // the real bytes: committed, verified
        let dir = commit(&root, &m, &data).unwrap();
        assert_eq!(dir, root.join(m.hash_hex()));
        assert_eq!(std::fs::read(dir.join("Scripts").join("main.lua")).unwrap(), b"print('hi')");
        assert!(verify(&root, &m));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "no temp folder left");
        // a committed mod is not rewritten
        assert!(commit(&root, &m, &data).is_ok());
        // tampering in the cache is noticed
        std::fs::write(dir.join("mod.json"), b"{ }").unwrap();
        assert!(!verify(&root, &m));
        std::fs::write(dir.join("mod.json"), b"{}").unwrap();
        assert!(verify(&root, &m));
        std::fs::write(dir.join("Scripts").join("extra.lua"), b"evil()").unwrap();
        assert!(!verify(&root, &m), "an extra file fails verification");
        // a broken copy is replaced by a commit
        assert!(commit(&root, &m, &data).is_ok());
        assert!(verify(&root, &m));
        assert!(!dir.join("Scripts").join("extra.lua").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A manifest path that would leave the folder never reaches the disk (defence in depth:
    /// the manifest refuses it first).
    #[test]
    fn paths_never_leave_the_mod_folder() {
        let base = Path::new("cache").join("abc");
        for bad in ["../x.lua", "/etc/x.lua", "C:/x.lua", "a/../../x.lua", "a\\..\\x.lua", "\\\\srv\\x.lua", "con.lua"] {
            assert!(join_inside(&base, bad).is_err(), "{bad}");
        }
        assert_eq!(join_inside(&base, "Scripts/main.lua").unwrap(), base.join("Scripts").join("main.lua"));
        let root = tmp();
        let mut m = one_mod(&[("Scripts/main.lua", b"x")]);
        m.files.push(FileEntry { path: "../../evil.lua".into(), size: 1, sha256: manifest::sha256(b"y") });
        assert!(commit(&root, &m, &[b"x".to_vec(), b"y".to_vec()]).is_err());
        assert!(!root.parent().unwrap().join("evil.lua").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}

//! The content hash: SHA-256 over the files that decide gameplay on both ends.
//!
//! The set (repo-relative paths, '/' separated):
//!   * `mods/mods.release.txt`
//!   * `mods/<Mod>/Scripts/*.lua` for every HSMP mod the template enables (`Name : 1`)
//!     that has a `Scripts/main.lua` (C++ mods such as HSMPNative have none)
//!   * `mods/shared/*.lua`
//!   * `server/data/maps/*.json`
//!
//! CR bytes are dropped before hashing, so a CRLF checkout hashes like the git objects.
//! This file uses only std and sha2: `server/build.rs` includes it with `#[path]`.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const TEMPLATE: &str = "mods/mods.release.txt";
const DOMAIN: &[u8] = b"hsmp-content-v1\0";

/// Where the files come from: a working tree, or git objects (the release tool).
pub trait Source {
    /// Repo-relative paths of the files directly inside `dir` (no recursion).
    fn files(&self, dir: &str) -> Vec<String>;
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// A checkout on disk.
pub struct FsSource(pub PathBuf);

impl Source for FsSource {
    fn files(&self, dir: &str) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.0.join(dir)) else { return vec![] };
        let mut v: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(|n| format!("{dir}/{n}")))
            .collect();
        v.sort();
        v
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

/// HSMP mods the template enables in every profile (`Name : 1`).
pub fn enabled_mods(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in template.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some((name, val)) = t.split_once(':') {
            let (name, val) = (name.trim(), val.trim());
            if val == "1" && name.starts_with("HSMP") && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The hashed files, sorted by path. `None` when the template is missing.
pub fn collect(src: &dyn Source) -> Option<Vec<String>> {
    let template = String::from_utf8_lossy(&src.read(TEMPLATE)?).into_owned();
    let mut paths = vec![TEMPLATE.to_string()];
    let lua = |p: &String| p.ends_with(".lua");
    for m in enabled_mods(&template) {
        let dir = format!("mods/{m}/Scripts");
        let files = src.files(&dir);
        if files.iter().any(|f| f.ends_with("/main.lua")) {
            paths.extend(files.into_iter().filter(lua));
        }
    }
    paths.extend(src.files("mods/shared").into_iter().filter(lua));
    paths.extend(src.files("server/data/maps").into_iter().filter(|p| p.ends_with(".json")));
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// Hash `(path, bytes)` entries (any order).
pub fn hash_entries(entries: &mut [(String, Vec<u8>)]) -> [u8; 32] {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    h.update(DOMAIN);
    for (path, bytes) in entries.iter() {
        let body: Vec<u8> = bytes.iter().copied().filter(|b| *b != b'\r').collect();
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update((body.len() as u64).to_le_bytes());
        h.update(&body);
    }
    h.finalize().into()
}

/// The content hash of a source, plus the paths it covered. `None`: no template.
pub fn content_hash(src: &dyn Source) -> Option<([u8; 32], Vec<String>)> {
    let paths = collect(src)?;
    let mut entries: Vec<(String, Vec<u8>)> = paths.iter().map(|p| (p.clone(), src.read(p).unwrap_or_default())).collect();
    Some((hash_entries(&mut entries), paths))
}

/// The content hash of a checkout.
pub fn content_hash_of_dir(root: &Path) -> Option<[u8; 32]> {
    content_hash(&FsSource(root.to_path_buf())).map(|(h, _)| h)
}

//! Server mods: the manifest (docs/hosting/server-mods.md, docs/development/protocol.md
//! "Server mods"). Shared by `hsmp-server` (builds it from `--mods-dir`) and `hsmp-sidecar`
//! (rebuilds it from the `mod_manifest` / `mod_files` records and checks every rule again:
//! the client trusts nothing the server says).
//!
//! The rules, the same on both ends:
//! * a set has at most [`Limits::max_mods`] mods, [`Limits::max_files`] files and
//!   [`Limits::max_total`] bytes; one file at most [`Limits::max_file`] bytes;
//! * a mod name is 1-32 of `[A-Za-z0-9_-]`, never `HSMP*` (any case: HSMP* Lua states get the
//!   native API and must never be impersonated or overwritten), never a Windows device name,
//!   unique without regard to case;
//! * a file path is relative and `/`-separated: 1-8 components of `[A-Za-z0-9._-]`, none
//!   starting or ending with `.`, no device names, at most 128 bytes, unique without regard to
//!   case, no file where another file needs a folder; its extension is one of
//!   [`ALLOWED_EXT`] (Lua and plain data; no DLL, EXE, PAK or script host files);
//! * every mod has `Scripts/main.lua`;
//! * `mod_hash` = SHA-256 over the name, the metadata and every (path, size, SHA-256) in path
//!   order; `set_hash` = SHA-256 over the mod hashes in name order. Both are recomputed by the
//!   client and must equal what the server announced.

// Shared by two binaries; each uses a different subset.
#![allow(dead_code)]

use hsmp_ipc::layout::Str;
use hsmp_ipc::schema::mods as rec;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Extensions a server mod may contain (v1: Lua and plain data only).
pub const ALLOWED_EXT: &[&str] = &["lua", "json", "txt", "csv", "ini", "md"];
/// The entry point every mod must have.
pub const MAIN: &str = "Scripts/main.lua";
/// Optional metadata file at the mod's root.
pub const META_FILE: &str = "mod.json";
const MAX_COMPONENTS: usize = 8;
const MAX_COMPONENT: usize = 64;
const MAX_NAME: usize = 32;
const MAX_VERSION: usize = 16;
const MAX_AUTHOR: usize = 48;
const MAX_DESCRIPTION: usize = 160;

/// Windows device names (a file named `con.txt` opens the console).
const DEVICES: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$", "conout$", "clock$"];

/// Size limits of one mod set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_total: u64,
    pub max_file: u64,
    pub max_files: usize,
    pub max_mods: usize,
}

impl Limits {
    /// The protocol's ceiling (and the client's limits): 64 MiB, 16 MiB per file.
    pub const PROTOCOL: Limits = Limits { max_total: 64 << 20, max_file: 16 << 20, max_files: rec::MAX_FILES, max_mods: rec::MAX_MODS };

    /// A server's `--mods-max-mb`: lower than the protocol's, never higher.
    pub fn with_total_mb(mb: u64) -> Limits {
        let total = (mb << 20).clamp(1, Self::PROTOCOL.max_total);
        Limits { max_total: total, max_file: Self::PROTOCOL.max_file.min(total), ..Self::PROTOCOL }
    }
}

/// One file of a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative, `/`-separated (`Scripts/main.lua`).
    pub path: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// One mod: its name (the folder), metadata and files (in path order).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModEntry {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub files: Vec<FileEntry>,
    pub hash: [u8; 32],
}

impl ModEntry {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash)
    }
}

/// A mod set in canonical order (mods by lower-case name, files by path).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub mods: Vec<ModEntry>,
    pub set_hash: [u8; 32],
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.mods.iter().map(ModEntry::bytes).sum()
    }
    pub fn file_count(&self) -> usize {
        self.mods.iter().map(|m| m.files.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }
    /// Every file in transfer order (the `mod_files` rows): (mod index, file).
    pub fn flat(&self) -> Vec<(usize, &FileEntry)> {
        self.mods.iter().enumerate().flat_map(|(i, m)| m.files.iter().map(move |f| (i, f))).collect()
    }
    /// The first `mod_files` row of each mod.
    pub fn first_files(&self) -> Vec<usize> {
        let mut at = 0;
        self.mods.iter().map(|m| { let f = at; at += m.files.len(); f }).collect()
    }
    pub fn set_hash_hex(&self) -> String {
        hex::encode(self.set_hash)
    }
}

// ---- rules ------------------------------------------------------------------------------

fn is_device(component: &str) -> bool {
    let base = component.split('.').next().unwrap_or("").to_ascii_lowercase();
    DEVICES.contains(&base.as_str())
}

/// A mod (folder) name: 1-32 of `[A-Za-z0-9_-]`, not `HSMP*` in any case, not a device name.
pub fn check_mod_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(format!("mod name {name:?}: 1 to {MAX_NAME} characters"));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("mod name {name:?}: only letters, digits, '_' and '-'"));
    }
    if name.to_ascii_lowercase().starts_with("hsmp") {
        return Err(format!("mod name {name:?}: names starting with HSMP are reserved for HalfSword-MP's own mods"));
    }
    if is_device(name) {
        return Err(format!("mod name {name:?} is a reserved Windows device name"));
    }
    Ok(())
}

/// The extension of a path's last component, lower-cased ("" when none).
pub fn extension(path: &str) -> String {
    let last = path.rsplit('/').next().unwrap_or(path);
    match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// A file path inside a mod (see the module docs).
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > rec::MAX_PATH {
        return Err(format!("path {path:?}: 1 to {} bytes", rec::MAX_PATH));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_COMPONENTS {
        return Err(format!("path {path:?}: more than {MAX_COMPONENTS} levels"));
    }
    for c in &parts {
        if c.is_empty() {
            return Err(format!("path {path:?}: must be relative, without empty parts (no leading '/', no '//')"));
        }
        if c.len() > MAX_COMPONENT {
            return Err(format!("path {path:?}: a part is longer than {MAX_COMPONENT} characters"));
        }
        if !c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
            return Err(format!("path {path:?}: only letters, digits, '_', '-', '.' and '/' (no '\\\\', ':', spaces)"));
        }
        if c.starts_with('.') || c.ends_with('.') {
            return Err(format!("path {path:?}: no part may start or end with '.' (no '..', no hidden files)"));
        }
        if is_device(c) {
            return Err(format!("path {path:?}: {c:?} is a reserved Windows device name"));
        }
    }
    let ext = extension(path);
    if !ALLOWED_EXT.contains(&ext.as_str()) {
        return Err(format!("path {path:?}: file type {:?} is not allowed (allowed: {})",
            if ext.is_empty() { "(none)" } else { ext.as_str() }, ALLOWED_EXT.join(", ")));
    }
    Ok(())
}

/// Metadata text: control and invisible characters dropped, trimmed, at most `max` bytes.
fn clean_meta(what: &str, s: &str, max: usize) -> Result<String, String> {
    hsmp_master_core::fields::clean(s, max).ok_or_else(|| format!("{what}: at most {max} bytes"))
}

/// One mod: name, metadata, files (paths valid, unique without regard to case, no file in
/// the place of a folder, `Scripts/main.lua` present, sizes within `limits`).
fn check_mod(m: &ModEntry, limits: &Limits) -> Result<(), String> {
    check_mod_name(&m.name)?;
    for (what, s, max) in [("version", &m.version, MAX_VERSION), ("author", &m.author, MAX_AUTHOR), ("description", &m.description, MAX_DESCRIPTION)] {
        if clean_meta(what, s, max)? != *s {
            return Err(format!("mod {}: {what} has control or invisible characters", m.name));
        }
    }
    if !m.files.iter().any(|f| f.path == MAIN) {
        return Err(format!("mod {}: {MAIN} is missing", m.name));
    }
    let mut lower: std::collections::HashSet<String> = std::collections::HashSet::new();
    for w in m.files.windows(2) {
        if w[0].path >= w[1].path {
            return Err(format!("mod {}: files are not in path order", m.name));
        }
    }
    for f in &m.files {
        check_path(&f.path).map_err(|e| format!("mod {}: {e}", m.name))?;
        if f.size > limits.max_file {
            return Err(format!("mod {}: {} is {} bytes (the limit is {})", m.name, f.path, f.size, limits.max_file));
        }
        if !lower.insert(f.path.to_ascii_lowercase()) {
            return Err(format!("mod {}: {} differs from another file only by case", m.name, f.path));
        }
    }
    for f in &m.files {
        let l = f.path.to_ascii_lowercase();
        let mut at = 0;
        while let Some(i) = l[at..].find('/') {
            if lower.contains(&l[..at + i]) {
                return Err(format!("mod {}: {} is both a file and a folder", m.name, &f.path[..at + i]));
            }
            at += i + 1;
        }
    }
    Ok(())
}

// ---- hashes -------------------------------------------------------------------------------

fn put_str(h: &mut Sha256, s: &str) {
    h.update((s.len() as u32).to_le_bytes());
    h.update(s.as_bytes());
}

/// The hash of one mod (its cache directory name).
pub fn mod_hash(m: &ModEntry) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"hsmp-server-mod-v1\0");
    for s in [&m.name, &m.version, &m.author, &m.description] {
        put_str(&mut h, s);
    }
    h.update((m.files.len() as u32).to_le_bytes());
    for f in &m.files {
        put_str(&mut h, &f.path);
        h.update(f.size.to_le_bytes());
        h.update(f.sha256);
    }
    h.finalize().into()
}

/// The hash of a set: the mod hashes in canonical (name) order.
pub fn set_hash(mods: &[ModEntry]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"hsmp-server-mod-set-v1\0");
    h.update((mods.len() as u32).to_le_bytes());
    for m in mods {
        h.update(m.hash);
    }
    h.finalize().into()
}

pub fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

/// Check a whole set against `limits`, including its canonical order and both hash levels.
pub fn validate(man: &Manifest, limits: &Limits) -> Result<(), String> {
    if man.mods.is_empty() {
        return Err("the set has no mods".into());
    }
    if man.mods.len() > limits.max_mods {
        return Err(format!("{} mods (the limit is {})", man.mods.len(), limits.max_mods));
    }
    if man.file_count() > limits.max_files {
        return Err(format!("{} files (the limit is {})", man.file_count(), limits.max_files));
    }
    let total = man.total_bytes();
    if total > limits.max_total {
        return Err(format!("{total} bytes in all (the limit is {})", limits.max_total));
    }
    for w in man.mods.windows(2) {
        let (a, b) = (w[0].name.to_ascii_lowercase(), w[1].name.to_ascii_lowercase());
        if a == b {
            return Err(format!("two mods named {:?} (names are compared without regard to case)", w[1].name));
        }
        if a > b {
            return Err("mods are not in name order".into());
        }
    }
    for m in &man.mods {
        check_mod(m, limits)?;
        if mod_hash(m) != m.hash {
            return Err(format!("mod {}: its hash does not match its contents", m.name));
        }
    }
    if set_hash(&man.mods) != man.set_hash {
        return Err("the set hash does not match the mods".into());
    }
    Ok(())
}

/// Sort into canonical order and compute both hash levels.
pub fn seal(mut mods: Vec<ModEntry>) -> Manifest {
    mods.sort_by_key(|m| m.name.to_ascii_lowercase());
    for m in &mut mods {
        m.files.sort_by(|a, b| a.path.cmp(&b.path));
        m.hash = mod_hash(m);
    }
    let set_hash = set_hash(&mods);
    Manifest { mods, set_hash }
}

// ---- records ------------------------------------------------------------------------------

/// The two S2C messages (`mod_manifest`, `mod_files`), framed.
pub fn to_messages(man: &Manifest, max_chunk: u32, timeout_s: u32) -> (Vec<u8>, Vec<u8>) {
    let firsts = man.first_files();
    let rows: Vec<rec::ModRow> = man.mods.iter().zip(firsts).map(|(m, first)| rec::ModRow {
        mod_hash: m.hash,
        bytes: m.bytes(),
        first_file: first as u16,
        files: m.files.len() as u16,
        _r: 0,
        name: Str::new(&m.name),
        version: Str::new(&m.version),
        author: Str::new(&m.author),
        description: Str::new(&m.description),
    }).collect();
    let head = rec::ModManifestHead {
        set_hash: man.set_hash,
        total_bytes: man.total_bytes(),
        max_chunk,
        timeout_s,
        n: 0,
        files: man.file_count() as u16,
        _r: 0,
    };
    let manifest = hsmp_ipc::wire::encode(0, 0, &head, &rows);
    let files: Vec<rec::ModFileRow> = man.flat().into_iter().map(|(i, f)| rec::ModFileRow {
        sha256: f.sha256,
        size: f.size,
        mod_index: i as u16,
        _r: [0; 6],
        path: Str::new(&f.path),
    }).collect();
    let fh = rec::ModFilesHead { set_hash: man.set_hash, n: 0, _r: [0; 6] };
    (manifest, hsmp_ipc::wire::encode(0, 0, &fh, &files))
}

/// What the client learns from the two records (validated as a whole).
#[derive(Debug, Clone)]
pub struct Announced {
    pub manifest: Manifest,
    pub max_chunk: u32,
    pub timeout_s: u32,
}

/// Rebuild a manifest from the `mod_manifest` and `mod_files` payloads and check every rule
/// (limits, names, paths, extensions, order, hashes). The client acts on nothing else.
pub fn from_payloads(manifest: &[u8], files: &[u8], limits: &Limits) -> Result<Announced, String> {
    let mv = hsmp_ipc::record::view::<rec::ModManifestHead>(manifest).map_err(|e| format!("mod_manifest: {e}"))?;
    let fv = hsmp_ipc::record::view::<rec::ModFilesHead>(files).map_err(|e| format!("mod_files: {e}"))?;
    let (mh, fh) = (mv.head(), fv.head());
    if mh.set_hash != fh.set_hash {
        return Err("mod_manifest and mod_files describe different sets".into());
    }
    if mh.files as usize != fv.rows.len() {
        return Err("mod_files has a different number of files than announced".into());
    }
    let s = |x: &Str<32>| x.as_str().map(str::to_string);
    let mut mods = Vec::with_capacity(mv.rows.len());
    let mut next = 0usize;
    for (i, r) in mv.rows.iter().enumerate() {
        if r.first_file as usize != next {
            return Err(format!("mod {i}: its files do not follow the previous mod's"));
        }
        let end = next + r.files as usize;
        if end > fv.rows.len() {
            return Err(format!("mod {i}: more files than mod_files has"));
        }
        let mut files = Vec::with_capacity(r.files as usize);
        for f in &fv.rows[next..end] {
            if f.mod_index as usize != i {
                return Err(format!("mod {i}: a file row names another mod"));
            }
            files.push(FileEntry { path: f.path.as_str().unwrap_or("").to_string(), size: f.size, sha256: f.sha256 });
        }
        next = end;
        let m = ModEntry {
            name: s(&r.name).unwrap_or_default(),
            version: r.version.as_str().unwrap_or("").to_string(),
            author: r.author.as_str().unwrap_or("").to_string(),
            description: r.description.as_str().unwrap_or("").to_string(),
            files,
            hash: r.mod_hash,
        };
        if m.bytes() != r.bytes {
            return Err(format!("mod {}: its size does not add up", m.name));
        }
        mods.push(m);
    }
    if next != fv.rows.len() {
        return Err("mod_files has files that belong to no mod".into());
    }
    let man = Manifest { mods, set_hash: mh.set_hash };
    if man.total_bytes() != mh.total_bytes {
        return Err("the announced total size does not add up".into());
    }
    validate(&man, limits)?;
    Ok(Announced { manifest: man, max_chunk: mh.max_chunk, timeout_s: mh.timeout_s })
}

// ---- building from a folder (server) --------------------------------------------------

/// A built set: the manifest and every file's bytes in transfer order (`Manifest::flat`).
#[derive(Debug, Clone, Default)]
pub struct Built {
    pub manifest: Manifest,
    pub data: Vec<std::sync::Arc<[u8]>>,
}

#[derive(serde::Deserialize, Default)]
struct MetaJson {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Read `dir` (one sub-folder per mod) into a sealed, validated set. Any rule broken is an
/// error naming the mod and the file; nothing is skipped silently except entries whose name
/// starts with '.' (`.git`) and loose files next to the mod folders (a README), which are
/// returned as notes. Symbolic links are refused.
pub fn build_from_dir(dir: &Path, limits: &Limits) -> Result<(Built, Vec<String>), String> {
    let mut notes = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        let md = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if name.starts_with('.') {
            notes.push(format!("skipped {} (hidden)", p.display()));
            continue;
        }
        if md.file_type().is_symlink() {
            return Err(format!("{}: symbolic links are not allowed in the mods folder", p.display()));
        }
        if md.is_file() {
            notes.push(format!("skipped {} (a file next to the mod folders; each mod is a folder)", p.display()));
            continue;
        }
        entries.push((name, p));
    }
    entries.sort();
    if entries.len() > limits.max_mods {
        return Err(format!("{}: {} mod folders (the limit is {})", dir.display(), entries.len(), limits.max_mods));
    }
    let mut mods = Vec::new();
    let mut blobs: Vec<(String, Vec<(String, std::sync::Arc<[u8]>)>)> = Vec::new();
    let mut total: u64 = 0;
    for (name, p) in entries {
        check_mod_name(&name).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut files = Vec::new();
        let mut data = Vec::new();
        walk(&p, "", &mut |rel, path| {
            check_path(rel).map_err(|e| format!("mod {name}: {e}"))?;
            let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
            if size > limits.max_file {
                return Err(format!("mod {name}: {rel} is {size} bytes (the limit is {})", limits.max_file));
            }
            total += size;
            if total > limits.max_total {
                return Err(format!("the mods are larger than {} bytes in all", limits.max_total));
            }
            let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if b.len() as u64 != size {
                return Err(format!("{}: changed while it was read", path.display()));
            }
            files.push(FileEntry { path: rel.to_string(), size, sha256: sha256(&b) });
            data.push((rel.to_string(), std::sync::Arc::<[u8]>::from(b)));
            if files.len() > limits.max_files {
                return Err(format!("mod {name}: more than {} files", limits.max_files));
            }
            Ok(())
        })?;
        let mut m = ModEntry { name: name.clone(), files, ..Default::default() };
        if let Some(meta) = data.iter().find(|(r, _)| r == META_FILE).map(|(_, b)| b.clone()) {
            let j: MetaJson = serde_json::from_slice(&meta).map_err(|e| format!("mod {name}: {META_FILE}: {e}"))?;
            m.version = clean_meta(&format!("mod {name}: version"), j.version.as_deref().unwrap_or(""), MAX_VERSION)?;
            m.author = clean_meta(&format!("mod {name}: author"), j.author.as_deref().unwrap_or(""), MAX_AUTHOR)?;
            m.description = clean_meta(&format!("mod {name}: description"), j.description.as_deref().unwrap_or(""), MAX_DESCRIPTION)?;
        }
        mods.push(m);
        blobs.push((name, data));
    }
    if mods.is_empty() {
        return Err(format!("{}: no mod folders (each mod is a folder with {MAIN})", dir.display()));
    }
    let manifest = seal(mods);
    validate(&manifest, limits)?;
    // The bytes in transfer order.
    let mut data = Vec::with_capacity(manifest.file_count());
    for m in &manifest.mods {
        let (_, files) = blobs.iter().find(|(n, _)| *n == m.name).ok_or("internal: mod lost")?;
        for f in &m.files {
            let (_, b) = files.iter().find(|(r, _)| *r == f.path).ok_or("internal: file lost")?;
            data.push(b.clone());
        }
    }
    Ok((Built { manifest, data }, notes))
}

/// Every regular file under `dir` (relative path with '/', absolute path). Symbolic links
/// are refused; folders are walked in name order.
fn walk(dir: &Path, rel: &str, f: &mut dyn FnMut(&str, &Path) -> Result<(), String>) -> Result<(), String> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    v.sort();
    for (name, p) in v {
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let md = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if md.file_type().is_symlink() {
            return Err(format!("{}: symbolic links are not allowed in a mod", p.display()));
        }
        if md.is_dir() {
            walk(&p, &r, f)?;
        } else if md.is_file() {
            f(&r, &p)?;
        } else {
            return Err(format!("{}: not a regular file", p.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A temp folder of mods: (mod, path, bytes).
    pub(crate) fn mods_dir(files: &[(&str, &str, &[u8])]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hsmp-mods-{}-{}", std::process::id(), rand::random::<u64>()));
        for (m, p, b) in files {
            let f = d.join(m).join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b).unwrap();
        }
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file(path: &str, b: &[u8]) -> FileEntry {
        FileEntry { path: path.into(), size: b.len() as u64, sha256: sha256(b) }
    }

    pub(crate) fn sample() -> Manifest {
        seal(vec![
            ModEntry { name: "Zeta".into(), files: vec![file("Scripts/main.lua", b"print('z')")], ..Default::default() },
            ModEntry { name: "arena_tweaks".into(), version: "1.2".into(), author: "Ann".into(), description: "Faster rounds".into(),
                files: vec![file("Scripts/main.lua", b"print('a')"), file("Scripts/util.lua", b"return {}"), file("mod.json", b"{}")],
                ..Default::default() },
        ])
    }

    #[test]
    fn names_and_paths_follow_the_rules() {
        for ok in ["Arena_Tweaks", "a", "x-1", "Hsm", "MyHSMP"] {
            assert!(check_mod_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "HSMPMenu", "hsmpmenu", "HsMp", "hsmp_x", "a b", "a.b", "../x", "CON", "nul", "lpt1", "x".repeat(33).as_str(), "é"] {
            assert!(check_mod_name(bad).is_err(), "{bad}");
        }
        for ok in ["Scripts/main.lua", "mod.json", "Scripts/lib/a_b-c.lua", "data/table.csv", "README.md", "cfg/x.INI"] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
        let long = format!("{}.lua", "a".repeat(130));
        for bad in [
            "", "/Scripts/main.lua", "Scripts//main.lua", "../main.lua", "Scripts/../../x.lua", "./a.lua", "Scripts/.hidden.lua",
            "a./b.lua", "C:/x.lua", "C:x.lua", "\\\\server\\share\\x.lua", "Scripts\\main.lua", "a b.lua", "Scripts/con.lua", "AUX.txt",
            "x.dll", "x.exe", "x.pak", "x.bat", "x.ps1", "x.cmd", "x.vbs", "x.luac", "x", "Scripts/", "Scripts/main.lua/", "a/b/c/d/e/f/g/h/i.lua",
            "naïve.lua", "x.lua:stream", long.as_str(),
        ] {
            assert!(check_path(bad).is_err(), "{bad:?} accepted");
        }
        assert_eq!(extension("a/b.LUA"), "lua");
        assert_eq!(extension("a/.lua"), "");
    }

    #[test]
    fn a_sealed_set_validates_and_hashes_are_canonical() {
        let m = sample();
        assert!(validate(&m, &Limits::PROTOCOL).is_ok());
        assert_eq!(m.mods[0].name, "arena_tweaks", "mods sorted by lower-case name");
        assert_eq!(m.mods[0].files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["Scripts/main.lua", "Scripts/util.lua", "mod.json"]);
        // the same mods in another order seal to the same hashes
        let again = seal(sample().mods.into_iter().rev().collect());
        assert_eq!(again.set_hash, m.set_hash);
        // any change to a file, a size or the metadata changes the set
        let mut x = m.clone();
        x.mods[0].author = "Bob".into();
        x.mods[0].hash = mod_hash(&x.mods[0]);
        assert_ne!(set_hash(&x.mods), m.set_hash);
        assert!(validate(&x, &Limits::PROTOCOL).is_err(), "stale set hash");
        let mut y = m.clone();
        y.mods[1].files[0].sha256[0] ^= 1;
        assert!(validate(&y, &Limits::PROTOCOL).is_err(), "stale mod hash");
    }

    #[test]
    fn limits_hsmp_names_and_missing_main_are_refused() {
        let m = sample();
        assert!(validate(&m, &Limits { max_total: 20, ..Limits::PROTOCOL }).is_err());
        assert!(validate(&m, &Limits { max_file: 9, ..Limits::PROTOCOL }).is_err());
        assert!(validate(&m, &Limits { max_files: 3, ..Limits::PROTOCOL }).is_err());
        assert!(validate(&m, &Limits { max_mods: 1, ..Limits::PROTOCOL }).is_err());
        let s = |name: &str, files: Vec<FileEntry>| seal(vec![ModEntry { name: name.into(), files, ..Default::default() }]);
        assert!(validate(&s("HSMPSync", vec![file(MAIN, b"x")]), &Limits::PROTOCOL).is_err());
        assert!(validate(&s("ok", vec![file("Scripts/other.lua", b"x")]), &Limits::PROTOCOL).is_err(), "no main.lua");
        assert!(validate(&s("ok", vec![file(MAIN, b"x"), file("scripts/MAIN.lua", b"y")]), &Limits::PROTOCOL).is_err(), "case clash");
        assert!(validate(&s("ok", vec![file(MAIN, b"x"), file("a.lua", b"1"), file("a.lua/b.lua", b"2")]), &Limits::PROTOCOL).is_err(),
            "file and folder");
        let two = seal(vec![
            ModEntry { name: "Dup".into(), files: vec![file(MAIN, b"1")], ..Default::default() },
            ModEntry { name: "dup".into(), files: vec![file(MAIN, b"2")], ..Default::default() },
        ]);
        assert!(validate(&two, &Limits::PROTOCOL).is_err(), "case-insensitive duplicate mod names");
        let mut bad_meta = s("ok", vec![file(MAIN, b"x")]);
        bad_meta.mods[0].author = "evil\u{202E}".into();
        bad_meta.mods[0].hash = mod_hash(&bad_meta.mods[0]);
        bad_meta.set_hash = set_hash(&bad_meta.mods);
        assert!(validate(&bad_meta, &Limits::PROTOCOL).is_err(), "invisible characters in metadata");
        assert_eq!(Limits::with_total_mb(1000), Limits::PROTOCOL);
        assert_eq!(Limits::with_total_mb(4).max_total, 4 << 20);
        assert_eq!(Limits::with_total_mb(4).max_file, 4 << 20);
    }

    #[test]
    fn records_round_trip_and_tampering_is_caught() {
        let m = sample();
        let (mm, fm) = to_messages(&m, 32768, 300);
        let (_, mp) = hsmp_ipc::wire::split(&mm).unwrap();
        let (_, fp) = hsmp_ipc::wire::split(&fm).unwrap();
        let a = from_payloads(mp, fp, &Limits::PROTOCOL).unwrap();
        assert_eq!(a.manifest, m);
        assert_eq!((a.max_chunk, a.timeout_s), (32768, 300));
        // flip any byte of either record: refused or (padding) the very same set, never another
        for (which, msg) in [(0, &mm), (1, &fm)] {
            let (_, p) = hsmp_ipc::wire::split(msg).unwrap();
            for i in 0..p.len() {
                let mut b = p.to_vec();
                b[i] ^= 0x41;
                let r = if which == 0 { from_payloads(&b, fp, &Limits::PROTOCOL) } else { from_payloads(mp, &b, &Limits::PROTOCOL) };
                if let Ok(x) = r {
                    assert_eq!(x.manifest, m, "byte {i} of record {which} changed the set without failing");
                }
            }
        }
        // a server announcing an HSMP* name, a traversal path or a DLL is refused even with
        // self-consistent hashes
        for (name, path) in [("HSMPMenu", MAIN), ("ok", "../../ue4ss/Mods/HSMPMenu/Scripts/main.lua"), ("ok", "Scripts/x.dll")] {
            let mut files = vec![file(MAIN, b"x")];
            if path != MAIN { files.push(file(path, b"y")); }
            let bad = Manifest { mods: vec![ModEntry { name: name.into(), files, ..Default::default() }], set_hash: [0; 32] };
            let mut bad = bad;
            bad.mods[0].files.sort_by(|a, b| a.path.cmp(&b.path));
            bad.mods[0].hash = mod_hash(&bad.mods[0]);
            bad.set_hash = set_hash(&bad.mods);
            let (mm, fm) = to_messages(&bad, 32768, 300);
            let r = from_payloads(hsmp_ipc::wire::split(&mm).unwrap().1, hsmp_ipc::wire::split(&fm).unwrap().1, &Limits::PROTOCOL);
            assert!(r.is_err(), "{name} {path} accepted");
        }
    }

    #[test]
    fn build_from_dir_reads_mods_and_refuses_bad_ones() {
        let d = mods_dir(&[
            ("arena", "Scripts/main.lua", b"print('hi')"),
            ("arena", "Scripts/lib/util.lua", b"return 1"),
            ("arena", "mod.json", br#"{"version":"1.0","author":"Ann","description":"Arena rules","extra":5}"#),
            ("chat", "Scripts/main.lua", b"-- chat"),
        ]);
        std::fs::write(d.join("README.txt"), b"notes").unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        let (b, notes) = build_from_dir(&d, &Limits::PROTOCOL).unwrap();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(b.manifest.mods.len(), 2);
        assert_eq!(b.manifest.mods[0].name, "arena");
        assert_eq!((b.manifest.mods[0].version.as_str(), b.manifest.mods[0].author.as_str()), ("1.0", "Ann"));
        assert_eq!(b.data.len(), b.manifest.file_count());
        for ((_, f), bytes) in b.manifest.flat().into_iter().zip(&b.data) {
            assert_eq!(sha256(bytes), f.sha256);
        }
        // each broken rule stops the server with a message naming it
        let cases: Vec<(Vec<(&str, &str, &[u8])>, &str)> = vec![
            (vec![("HSMPWorld", "Scripts/main.lua", b"x")], "reserved"),
            (vec![("m", "Scripts/x.dll", b"x"), ("m", "Scripts/main.lua", b"x")], "not allowed"),
            (vec![("m", "Scripts/other.lua", b"x")], "missing"),
            (vec![("m", "Scripts/main.lua", b"x"), ("m", "my file.lua", b"x")], "only letters"),
            (vec![("m", "Scripts/main.lua", b"x"), ("m", "mod.json", b"{not json")], "mod.json"),
            (vec![("m", "Scripts/main.lua", &[0u8; 2048])], "limit"),
        ];
        for (files, want) in cases {
            let d = mods_dir(&files);
            let e = build_from_dir(&d, &Limits { max_total: 1024, max_file: 1024, ..Limits::PROTOCOL }).unwrap_err();
            assert!(e.contains(want), "{e:?} should mention {want:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
        let empty = mods_dir(&[]);
        assert!(build_from_dir(&empty, &Limits::PROTOCOL).unwrap_err().contains("no mod folders"));
        #[cfg(unix)]
        {
            let s = mods_dir(&[("m", "Scripts/main.lua", b"x")]);
            std::os::unix::fs::symlink("/etc/passwd", s.join("m").join("Scripts").join("p.txt")).unwrap();
            assert!(build_from_dir(&s, &Limits::PROTOCOL).unwrap_err().contains("symbolic"));
            let _ = std::fs::remove_dir_all(&s);
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&empty);
    }
}

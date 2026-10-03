//! A release package: a folder or a .zip holding `manifest.json`,
//! `manifest.sig`, the launcher and `payload/...`.
//!
//! `Package::open` verifies the signature before it even parses the manifest
//! (nothing unsigned is interpreted). `Package::load_files` then reads every
//! manifest file and checks size and SHA-256; install works only from those
//! verified in-memory bytes, so a file swapped on disk after verification is
//! never installed.

use crate::manifest::{Manifest, Role};
use crate::sign::{self, SigFile, TrustedKey};
use crate::util;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "manifest.json";
pub const SIGNATURE: &str = "manifest.sig";

enum Source {
    Dir(PathBuf),
    Zip { path: PathBuf, prefix: String },
}

pub struct Package {
    src: Source,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub signed_by: String,
}

/// Verified file contents keyed by package path.
pub type Files = BTreeMap<String, Vec<u8>>;

/// Read limits. The manifest and signature are read BEFORE anything is
/// verified, so they get small fixed caps; a payload file may not exceed the
/// size the signed manifest declares. The size a zip header declares is
/// never trusted (not even for the buffer capacity).
pub const MAX_MANIFEST: u64 = 8 * 1024 * 1024;
pub const MAX_SIGNATURE: u64 = 64 * 1024;

fn read_capped(mut r: impl Read, limit: u64, name: &str) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    r.by_ref().take(limit + 1).read_to_end(&mut v).map_err(|e| format!("{name}: {e}"))?;
    if v.len() as u64 > limit {
        return Err(format!("{name} is larger than {limit} bytes: wrong size (corrupted download, or not an HSMP release)"));
    }
    Ok(v)
}

fn read_zip_entry(path: &Path, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{} is not a valid zip: {e}", path.display()))?;
    let e = match z.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(format!("zip entry {name}: {e}")),
    };
    read_capped(e, limit, &format!("zip entry {name}")).map(Some)
}

fn zip_prefix(path: &Path) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let z = zip::ZipArchive::new(f).map_err(|e| format!("{} is not a valid zip: {e}", path.display()))?;
    let mut best: Option<String> = None;
    for name in z.file_names() {
        if let Some(pre) = name.strip_suffix(MANIFEST) {
            if (pre.is_empty() || (pre.ends_with('/') && pre.matches('/').count() == 1)) && best.as_ref().map(|b| pre.len() < b.len()).unwrap_or(true) {
                best = Some(pre.to_string());
            }
        }
    }
    best.ok_or_else(|| format!("{} has no {MANIFEST} (is it an HSMP release zip?)", path.display()))
}

impl Package {
    /// Open a package folder or zip and verify the manifest signature.
    pub fn open(path: &Path, keys: &[TrustedKey]) -> Result<Package, String> {
        let src = if path.is_dir() {
            if !path.join(MANIFEST).is_file() {
                return Err(format!(
                    "{} has no {MANIFEST}. Extract the WHOLE release zip to a folder and run hsmp-launcher.exe from there (running it from inside the zip does not work).",
                    path.display()
                ));
            }
            Source::Dir(path.to_path_buf())
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false) {
            Source::Zip { path: path.to_path_buf(), prefix: zip_prefix(path)? }
        } else {
            return Err(format!("{} is neither a release folder nor a .zip", path.display()));
        };
        let mut p = Package { src, manifest: crate::manifest::Manifest::placeholder(), manifest_bytes: vec![], signed_by: String::new() };
        let mb = p.read_raw(MANIFEST, MAX_MANIFEST)?.ok_or_else(|| format!("{MANIFEST} is missing"))?;
        let sb = p.read_raw(SIGNATURE, MAX_SIGNATURE)?.ok_or_else(|| format!("{SIGNATURE} is missing: the release is not signed"))?;
        let sig = SigFile::parse(&sb)?;
        let key = sign::verify(&mb, &sig, keys)?;
        p.signed_by = format!("{} ({})", key.id, if key.label.is_empty() { "release key" } else { &key.label });
        p.manifest = Manifest::parse(&mb)?;
        p.manifest_bytes = mb;
        Ok(p)
    }

    pub fn describe(&self) -> String {
        match &self.src {
            Source::Dir(d) => d.display().to_string(),
            Source::Zip { path, .. } => path.display().to_string(),
        }
    }

    fn read_raw(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
        if !util::is_safe_rel(rel) {
            return Err(format!("unsafe package path {rel}"));
        }
        match &self.src {
            Source::Dir(d) => match std::fs::File::open(util::join_rel(d, rel)) {
                Ok(f) => read_capped(f, limit, rel).map(Some),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("{rel}: {e}")),
            },
            Source::Zip { path, prefix } => read_zip_entry(path, &format!("{prefix}{rel}"), limit),
        }
    }

    /// Read and verify every file the manifest lists (except the launcher
    /// itself, unless `include_launcher`).
    pub fn load_files(&self, include_launcher: bool) -> Result<Files, String> {
        let mut out = Files::new();
        for f in &self.manifest.files {
            if f.role == Role::Launcher && !include_launcher {
                continue;
            }
            // never read more than the signed size (+1 to detect a longer file)
            let b = self.read_raw(&f.path, f.size)?.ok_or_else(|| format!("package file {} is missing (incomplete download or extraction?)", f.path))?;
            if b.len() as u64 != f.size {
                return Err(format!("package file {} has size {} but the manifest says {} (corrupted download?)", f.path, b.len(), f.size));
            }
            let h = util::sha256_hex(&b);
            if h != f.sha256 {
                return Err(format!("package file {} fails its SHA-256 check (corrupted or modified)", f.path));
            }
            out.insert(f.path.clone(), b);
        }
        Ok(out)
    }
}

impl Manifest {
    fn placeholder() -> Manifest {
        Manifest {
            schema: 0,
            product: String::new(),
            version: String::new(),
            channel: String::new(),
            git_commit: String::new(),
            source_date_epoch: 0,
            protocol_version: 0,
            toolchain: String::new(),
            game: crate::manifest::GameCompat { steam_appid: 0, builds: vec![] },
            ue4ss: crate::manifest::Ue4ssInfo { version: String::new(), dll_sha256: String::new(), proxy_sha256: String::new() },
            master_urls: vec![],
            launch_args: vec![],
            ini_settings: vec![],
            mods_template: String::new(),
            files: vec![],
        }
    }
}

/// Default package location: the folder holding the running launcher exe.
pub fn default_package_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

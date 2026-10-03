//! Writing release packages (used by `hsmp-release` and by the tests).
//!
//! Deterministic by construction: the manifest lists files sorted by path,
//! the zip entries are written in that order with one fixed timestamp
//! (the commit time), fixed permissions and fixed compression settings.
//! Same inputs -> byte-identical manifest.json, manifest.sig and zip.

use crate::manifest::{FileEntry, Manifest, Role};
use crate::package::{MANIFEST, SIGNATURE};
use crate::sign;
use crate::util;
use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// One file going into the package.
#[derive(Debug, Clone)]
pub struct Item {
    pub bytes: Vec<u8>,
    pub role: Role,
    pub install: Option<String>,
}

/// Fill `manifest.files` from `items` (sorted by package path), validate,
/// serialize and sign. Returns (manifest bytes, signature bytes).
pub fn seal(mut manifest: Manifest, items: &BTreeMap<String, Item>, key: &SigningKey) -> Result<(Vec<u8>, Vec<u8>), String> {
    manifest.files = items
        .iter()
        .map(|(path, it)| FileEntry { path: path.clone(), sha256: util::sha256_hex(&it.bytes), size: it.bytes.len() as u64, role: it.role, install: it.install.clone() })
        .collect();
    manifest.validate()?;
    let mb = manifest.to_bytes();
    let sb = sign::sign(&mb, key).to_bytes();
    Ok((mb, sb))
}

/// Write the package as a folder (manifest, signature, every item).
pub fn write_dir(out: &Path, items: &BTreeMap<String, Item>, manifest_bytes: &[u8], sig_bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join(MANIFEST), manifest_bytes)?;
    std::fs::write(out.join(SIGNATURE), sig_bytes)?;
    for (path, it) in items {
        let p = util::join_rel(out, path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, &it.bytes)?;
    }
    Ok(())
}

/// Deterministic zip bytes with every entry under `top/`.
pub fn zip_bytes(top: &str, items: &BTreeMap<String, Item>, manifest_bytes: &[u8], sig_bytes: &[u8], epoch: u64) -> Result<Vec<u8>, String> {
    let (y, mo, d, h, mi, s) = util::civil_utc(epoch.max(315_532_800)); // zip time starts in 1980
    let dt = zip::DateTime::from_date_and_time(y.clamp(1980, 2107) as u16, mo as u8, d as u8, h as u8, mi as u8, s as u8).map_err(|e| format!("zip time: {e}"))?;
    let mut entries: Vec<(String, &[u8])> = vec![(MANIFEST.into(), manifest_bytes), (SIGNATURE.into(), sig_bytes)];
    for (p, it) in items {
        entries.push((p.clone(), &it.bytes));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cur);
        for (p, b) in entries {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(6))
                .last_modified_time(dt)
                .unix_permissions(0o644)
                .large_file(false);
            zw.start_file(format!("{top}/{p}"), opts).map_err(|e| format!("zip {p}: {e}"))?;
            zw.write_all(b).map_err(|e| format!("zip {p}: {e}"))?;
        }
        zw.finish().map_err(|e| format!("zip finish: {e}"))?;
    }
    Ok(cur.into_inner())
}

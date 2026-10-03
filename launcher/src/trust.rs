//! Which keys the launcher trusts, and the launcher copy used for updates.
//!
//! The release signature can only be as trustworthy as the launcher that
//! checks it. The launcher ships inside the zip it verifies, so for a FIRST
//! download the signature detects corruption, not a re-packed zip (anyone
//! can rebuild the launcher with their own key). The trust root for a first
//! download is the SHA-256 published on the official release page.
//!
//! After that, trust is anchored OUTSIDE the package:
//!
//! * keys are only ever taken from this launcher binary (compiled in from
//!   `launcher/trusted_keys.txt`) and from an optional, user-managed
//!   `%LOCALAPPDATA%\HSMP\trusted_keys.txt`. Nothing inside a package (a
//!   `trusted_keys.txt`, a key in the manifest, the package's own launcher)
//!   is ever used as a key;
//! * after a successful install, the verified launcher of that release is
//!   kept at `%LOCALAPPDATA%\HSMP\bin\hsmp-launcher.exe`. Updates are opened
//!   with THAT launcher ("Open another release..."), so a later zip is checked
//!   by keys you already trust, and a re-signed mirror zip is refused.

use crate::manifest::{Manifest, Role};
use crate::package::Files;
use crate::sign::{self, TrustedKey};
use crate::util;
use std::path::{Path, PathBuf};

/// Optional extra keys, pinned by the player (never written by the launcher).
pub fn pinned_keys_path(hsmp_home: &Path) -> PathBuf {
    hsmp_home.join("trusted_keys.txt")
}

/// Compiled-in keys plus the pinned file's keys. A pinned file that does not
/// parse is an error (never silently ignored).
pub fn effective_keys(hsmp_home: Option<&Path>) -> Result<Vec<TrustedKey>, String> {
    effective_keys_from(sign::EMBEDDED_TRUSTED_KEYS, hsmp_home)
}

pub fn effective_keys_from(embedded: &str, hsmp_home: Option<&Path>) -> Result<Vec<TrustedKey>, String> {
    let mut keys = sign::parse_trusted_keys(embedded)?;
    if let Some(h) = hsmp_home {
        let p = pinned_keys_path(h);
        if let Some(b) = util::read_opt(&p).map_err(|e| format!("{}: {e}", p.display()))? {
            let extra = sign::parse_trusted_keys(&String::from_utf8_lossy(&b)).map_err(|e| format!("{}: {e}", p.display()))?;
            for k in extra {
                if !keys.iter().any(|x| x.id == k.id) {
                    keys.push(TrustedKey { label: format!("{} (pinned in {})", k.label, p.display()), ..k });
                }
            }
        }
    }
    Ok(keys)
}

/// Where the launcher for updates lives.
pub fn installed_launcher_path(hsmp_home: &Path) -> PathBuf {
    hsmp_home.join("bin").join("hsmp-launcher.exe")
}

/// Keep the release's launcher (verified: its bytes are in `files` only after
/// the signature and SHA-256 checks) outside the package for later updates.
/// Returns the path, or None when the package has no launcher entry.
pub fn install_launcher_copy(hsmp_home: &Path, manifest: &Manifest, files: &Files) -> Result<Option<PathBuf>, String> {
    let Some(entry) = manifest.files.iter().find(|f| f.role == Role::Launcher) else { return Ok(None) };
    let Some(bytes) = files.get(&entry.path) else { return Ok(None) };
    if util::sha256_hex(bytes) != entry.sha256 {
        return Err("launcher bytes do not match the signed manifest".into());
    }
    let dst = installed_launcher_path(hsmp_home);
    if util::sha256_file(&dst).map(|(h, _)| h == entry.sha256).unwrap_or(false) {
        return Ok(Some(dst));
    }
    std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| format!("{}: {e}", hsmp_home.display()))?;
    if let Err(e) = util::atomic_write(&dst, bytes) {
        // a running exe cannot be replaced, but it can be renamed away
        let old = dst.with_file_name("hsmp-launcher.old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&dst, &old).map_err(|_| format!("{}: {e}", dst.display()))?;
        util::atomic_write(&dst, bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    Ok(Some(dst))
}

/// Remove the copy a self-update renamed away (it was still running then).
pub fn cleanup_old_copy(hsmp_home: &Path) {
    let _ = std::fs::remove_file(installed_launcher_path(hsmp_home).with_file_name("hsmp-launcher.old.exe"));
}

/// SHA-256 of the running launcher (shown so it can be compared with the
/// hash published on the release page).
pub fn self_sha256() -> Option<String> {
    util::sha256_file(&std::env::current_exe().ok()?).ok().map(|(h, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::sample;
    use crate::pack::{self, Item};
    use crate::package::Package;
    use crate::sign::tests::test_key;
    use crate::testutil::TempDir;
    use std::collections::BTreeMap;

    fn line(k: &ed25519_dalek::SigningKey, label: &str) -> String {
        format!("{} {label}\n", hex::encode(k.verifying_key().as_bytes()))
    }

    fn items(attacker_keys: &str) -> BTreeMap<String, Item> {
        let mut m = BTreeMap::new();
        m.insert("payload/mods.release.txt".into(), Item { bytes: b"HSMPMenu : 1\n".to_vec(), role: Role::Template, install: None });
        m.insert("payload/Win64/dwmapi.dll".into(), Item { bytes: b"proxy".to_vec(), role: Role::Ue4ss, install: Some("HalfswordUE5/Binaries/Win64/dwmapi.dll".into()) });
        m.insert("hsmp-launcher.exe".into(), Item { bytes: b"MZ launcher built by someone".to_vec(), role: Role::Launcher, install: None });
        // a package that ships "its own" trust anchors
        m.insert("trusted_keys.txt".into(), Item { bytes: attacker_keys.as_bytes().to_vec(), role: Role::Doc, install: None });
        m.insert("launcher/trusted_keys.txt".into(), Item { bytes: attacker_keys.as_bytes().to_vec(), role: Role::Doc, install: None });
        m
    }

    /// A mirror re-signs a modified package with its own key and ships
    /// that key inside the package: it is refused. Only keys compiled into
    /// the launcher (or pinned outside the package) count.
    #[test]
    fn keys_inside_a_package_are_never_trusted() {
        let t = TempDir::new("trust_pkg");
        let real = test_key(1);
        let attacker = test_key(66);
        let it = items(&line(&attacker, "attacker"));
        let (mb, sb) = pack::seal(sample(), &it, &attacker).unwrap();
        let dir = t.path().join("mirror");
        pack::write_dir(&dir, &it, &mb, &sb).unwrap();
        let zip = t.path().join("mirror.zip");
        std::fs::write(&zip, pack::zip_bytes("hsmp-0.1.0", &it, &mb, &sb, 1_790_899_200).unwrap()).unwrap();
        let home = t.path().join("HSMP");
        let keys = effective_keys_from(&line(&real, "release"), Some(&home)).unwrap();
        assert_eq!(keys.len(), 1);
        for p in [&dir, &zip] {
            let e = Package::open(p, &keys).err().unwrap();
            assert!(e.contains("does not trust"), "{e}");
        }
        // the genuine release opens with the same keys
        let it2 = items("");
        let (mb, sb) = pack::seal(sample(), &it2, &real).unwrap();
        let good = t.path().join("good");
        pack::write_dir(&good, &it2, &mb, &sb).unwrap();
        assert!(Package::open(&good, &keys).is_ok());
    }

    #[test]
    fn pinned_key_file_outside_the_package() {
        let t = TempDir::new("trust_pin");
        let home = t.path().join("HSMP");
        let real = test_key(1);
        let fork = test_key(2);
        assert_eq!(effective_keys_from(&line(&real, "r"), Some(&home)).unwrap().len(), 1, "no pinned file");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(pinned_keys_path(&home), line(&fork, "my fork")).unwrap();
        let k = effective_keys_from(&line(&real, "r"), Some(&home)).unwrap();
        assert_eq!(k.len(), 2);
        assert!(k[1].label.contains("pinned"));
        std::fs::write(pinned_keys_path(&home), "not a key\n").unwrap();
        assert!(effective_keys_from(&line(&real, "r"), Some(&home)).is_err(), "a broken pinned file is an error, not ignored");
        assert_eq!(effective_keys_from(&line(&real, "r"), None).unwrap().len(), 1);
    }

    /// The verified launcher is kept outside the package for updates.
    #[test]
    fn launcher_copy_for_updates() {
        let t = TempDir::new("trust_copy");
        let home = t.path().join("HSMP");
        let key = test_key(1);
        let it = items("");
        let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
        let dir = t.path().join("rel");
        pack::write_dir(&dir, &it, &mb, &sb).unwrap();
        let p = Package::open(&dir, &effective_keys_from(&line(&key, "r"), None).unwrap()).unwrap();
        let files = p.load_files(true).unwrap();
        let dst = install_launcher_copy(&home, &p.manifest, &files).unwrap().unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"MZ launcher built by someone");
        assert_eq!(dst, installed_launcher_path(&home));
        // files without the launcher (load_files(false)) -> nothing copied
        let none = p.load_files(false).unwrap();
        assert!(install_launcher_copy(&t.path().join("H2"), &p.manifest, &none).unwrap().is_none());
    }
}

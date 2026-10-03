//! The release manifest (`manifest.json`), written by `hsmp-release` and
//! verified by the launcher before anything is installed.
//!
//! The manifest is signed as raw bytes (see `sign.rs`); this module only
//! defines the schema and the structural checks that run after the
//! signature has been verified.

use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SCHEMA: u32 = 1;
pub const PRODUCT: &str = "hsmp";
/// Every install target must live under this game-root-relative prefix.
pub const INSTALL_PREFIX: &str = "HalfswordUE5/Binaries/Win64/";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameBuild {
    /// Human label ("Early Access, Steam build 24185754").
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_buildid: Option<String>,
    /// SHA-256 of HalfswordUE5/Binaries/Win64/HalfSwordUE5-Win64-Shipping.exe.
    pub exe_sha256: String,
    /// Size in bytes (0 = not checked).
    #[serde(default)]
    pub exe_size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameCompat {
    pub steam_appid: u32,
    pub builds: Vec<GameBuild>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ue4ssInfo {
    /// Free-form pin label (e.g. "RE-UE4SS experimental e3ba1016").
    pub version: String,
    /// SHA-256 of ue4ss/UE4SS.dll and dwmapi.dll as pinned in release.json.
    pub dll_sha256: String,
    pub proxy_sha256: String,
}

/// One `[Section] Key=Value` the launcher merges into an ini under
/// %LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows (journaled, reverted on uninstall).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IniSetting {
    /// "Engine" (the only ini the launcher accepts today).
    pub file: String,
    pub section: String,
    pub key: String,
    pub value: String,
    /// Why (shown in the launcher log).
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// UE4SS proxy, dll, settings, its runtime Lua mods.
    Ue4ss,
    /// HSMP Lua mods.
    Mod,
    /// hsmp-server / hsmp-sidecar / hsmp-master / hsmp-query.
    Bin,
    /// The mods.txt template (not installed; the launcher renders mods.txt from it).
    Template,
    /// The launcher itself (not installed).
    Launcher,
    /// Documentation shipped in the zip (not installed).
    Doc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path inside the package, forward slashes ("payload/Win64/dwmapi.dll").
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub role: Role,
    /// Game-root-relative install target, if the file is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub version: String,
    pub channel: String,
    pub git_commit: String,
    /// Commit time (seconds since epoch); also the zip entry timestamp.
    pub source_date_epoch: u64,
    /// hsmp-net PROTOCOL_VERSION the binaries and mods speak.
    pub protocol_version: u32,
    /// `rustc -V` of the toolchain that built the binaries (reproducibility).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub toolchain: String,
    pub game: GameCompat,
    pub ue4ss: Ue4ssInfo,
    /// Written into hsmp.cfg `master_url` (primary first).
    pub master_urls: Vec<String>,
    /// Extra game command-line arguments for every launch.
    pub launch_args: Vec<String>,
    pub ini_settings: Vec<IniSetting>,
    /// Package path of the mods.txt template.
    pub mods_template: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("manifest.json is not valid: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec_pretty(self).expect("manifest serializes");
        v.push(b'\n');
        v
    }

    pub fn installed_files(&self) -> impl Iterator<Item = (&FileEntry, &str)> {
        self.files.iter().filter_map(|f| f.install.as_deref().map(|i| (f, i)))
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Structural checks: schema, safe paths, hashes, unique targets, roles.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("manifest schema {} is not supported by this launcher (expects {SCHEMA}); use the launcher from the same release", self.schema));
        }
        if self.product != PRODUCT {
            return Err(format!("manifest is for '{}', not '{PRODUCT}'", self.product));
        }
        if self.version.trim().is_empty() {
            return Err("manifest has no version".into());
        }
        if self.game.builds.is_empty() {
            return Err("manifest lists no supported game builds".into());
        }
        for b in &self.game.builds {
            if !util::is_sha256_hex(&b.exe_sha256) {
                return Err(format!("bad exe_sha256 for game build '{}'", b.label));
            }
        }
        for u in &self.master_urls {
            if !safe_url(u) {
                return Err(format!("master url '{u}' is not a plain http(s) URL"));
            }
            if !u.starts_with("https://") && !loopback_url(u) {
                return Err(format!("master url '{u}' must use https (plain http only for this machine)"));
            }
        }
        for a in &self.launch_args {
            if a.is_empty() || a.contains('"') || a.contains(char::is_whitespace) || !a.starts_with('-') {
                return Err(format!("launch argument '{a}' is not allowed"));
            }
        }
        for s in &self.ini_settings {
            if s.file != "Engine" {
                return Err(format!("ini setting for '{}' is not allowed (only Engine)", s.file));
            }
            let ok = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
            if !ok(&s.section) || !ok(&s.key) || !ok(&s.value) {
                return Err(format!("ini setting [{}] {}={} has unsafe characters", s.section, s.key, s.value));
            }
        }
        let mut paths = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for f in &self.files {
            if !util::is_safe_rel(&f.path) {
                return Err(format!("unsafe package path '{}'", f.path));
            }
            if !paths.insert(f.path.to_ascii_lowercase()) {
                return Err(format!("duplicate package path '{}'", f.path));
            }
            if !util::is_sha256_hex(&f.sha256) {
                return Err(format!("bad sha256 for '{}'", f.path));
            }
            let installs = matches!(f.role, Role::Ue4ss | Role::Mod | Role::Bin);
            match (&f.install, installs) {
                (Some(t), true) => {
                    if !util::is_safe_rel(t) || !t.starts_with(INSTALL_PREFIX) || t.len() <= INSTALL_PREFIX.len() {
                        return Err(format!("install target '{t}' is outside {INSTALL_PREFIX}"));
                    }
                    let lower = t.to_ascii_lowercase();
                    for reserved in ["hsmp.cfg", "ue4ss/mods/mods.txt", "halfswordue5-win64-shipping.exe", "hsmp_install.json"] {
                        if lower == format!("{}{reserved}", INSTALL_PREFIX.to_ascii_lowercase()) {
                            return Err(format!("install target '{t}' is reserved (the launcher writes it)"));
                        }
                    }
                    if !targets.insert(lower) {
                        return Err(format!("duplicate install target '{t}'"));
                    }
                }
                (None, false) => {}
                (Some(t), false) => return Err(format!("{:?} file '{}' must not have an install target ({t})", f.role, f.path)),
                (None, true) => return Err(format!("{:?} file '{}' has no install target", f.role, f.path)),
            }
        }
        match self.file(&self.mods_template) {
            Some(f) if f.role == Role::Template => {}
            _ => return Err(format!("mods template '{}' is not a template file in the manifest", self.mods_template)),
        }
        Ok(())
    }
}

/// http(s)://localhost or 127.x (a master on this machine).
fn loopback_url(u: &str) -> bool {
    let rest = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.")
}

/// Same rule as shared/hsmp_cfg.lua `safe_url`: plain http(s)://host[:port][/path]
/// with cmd-safe characters (the URL ends up inside os.execute command lines).
pub fn safe_url(u: &str) -> bool {
    let rest = if let Some(r) = u.strip_prefix("http://") {
        r
    } else if let Some(r) = u.strip_prefix("https://") {
        r
    } else {
        return false;
    };
    let (host, tail) = match rest.find([':', '/']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    !host.is_empty()
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && tail.chars().all(|c| c.is_ascii_alphanumeric() || ":./-_".contains(c))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Manifest {
        let h = |b: &[u8]| util::sha256_hex(b);
        Manifest {
            schema: SCHEMA,
            product: PRODUCT.into(),
            version: "0.1.0".into(),
            channel: "test".into(),
            git_commit: "0".repeat(40),
            source_date_epoch: 1_790_899_200,
            protocol_version: 5,
            toolchain: String::new(),
            game: GameCompat { steam_appid: 2397300, builds: vec![GameBuild { label: "test".into(), steam_buildid: None, exe_sha256: h(b"exe"), exe_size: 3 }] },
            ue4ss: Ue4ssInfo { version: "pin".into(), dll_sha256: h(b"dll"), proxy_sha256: h(b"proxy") },
            master_urls: vec!["http://127.0.0.1:7778".into()],
            launch_args: vec!["-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0".into()],
            ini_settings: vec![IniSetting { file: "Engine".into(), section: "SystemSettings".into(), key: "r.HairStrands.Streaming".into(), value: "0".into(), reason: "hair-strand streaming workaround".into() }],
            mods_template: "payload/mods.release.txt".into(),
            files: vec![
                FileEntry { path: "payload/mods.release.txt".into(), sha256: h(b"t"), size: 1, role: Role::Template, install: None },
                FileEntry { path: "payload/Win64/dwmapi.dll".into(), sha256: h(b"proxy"), size: 5, role: Role::Ue4ss, install: Some("HalfswordUE5/Binaries/Win64/dwmapi.dll".into()) },
            ],
        }
    }

    #[test]
    fn sample_is_valid_and_roundtrips() {
        let m = sample();
        m.validate().unwrap();
        assert_eq!(Manifest::parse(&m.to_bytes()).unwrap(), m);
    }

    #[test]
    fn rejects_bad_manifests() {
        type Mutate = Box<dyn Fn(&mut Manifest)>;
        let cases: Vec<(&str, Mutate)> = vec![
            ("schema", Box::new(|m| m.schema = 99)),
            ("traversal", Box::new(|m| m.files[1].install = Some("HalfswordUE5/Binaries/Win64/../../../evil.dll".into()))),
            ("outside", Box::new(|m| m.files[1].install = Some("HalfswordUE5/Content/x.pak".into()))),
            ("reserved", Box::new(|m| m.files[1].install = Some("HalfswordUE5/Binaries/Win64/hsmp.cfg".into()))),
            ("exe", Box::new(|m| m.files[1].install = Some("HalfswordUE5/Binaries/Win64/HalfSwordUE5-Win64-Shipping.exe".into()))),
            ("abs pkg path", Box::new(|m| m.files[1].path = "C:/x".into())),
            ("hash", Box::new(|m| m.files[1].sha256 = "zz".into())),
            ("dup", Box::new(|m| { let f = m.files[1].clone(); m.files.push(f) })),
            ("template install", Box::new(|m| m.files[0].install = Some("HalfswordUE5/Binaries/Win64/x".into()))),
            ("no template", Box::new(|m| m.mods_template = "payload/none".into())),
            ("bad url", Box::new(|m| m.master_urls = vec!["http://x & calc".into()])),
            ("http internet master", Box::new(|m| m.master_urls = vec!["http://master.example.net:7778".into()])),
            ("bad arg", Box::new(|m| m.launch_args = vec!["-a b".into()])),
            ("ini file", Box::new(|m| m.ini_settings[0].file = "Game".into())),
            ("ini value", Box::new(|m| m.ini_settings[0].value = "0\n[X]".into())),
            ("no builds", Box::new(|m| m.game.builds.clear())),
        ];
        for (name, f) in cases {
            let mut m = sample();
            f(&mut m);
            assert!(m.validate().is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn urls() {
        assert!(safe_url("http://127.0.0.1:7778"));
        assert!(safe_url("https://master.example.net/hsmp"));
        assert!(!safe_url("ftp://x"));
        assert!(!safe_url("http://"));
        assert!(!safe_url("http://a b"));
        assert!(!safe_url("http://x/\"&calc"));
    }
}

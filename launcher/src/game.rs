//! The Half Sword install: layout, build verification, conflicting installs.

use crate::manifest::{GameBuild, GameCompat};
use crate::util;
use std::path::{Path, PathBuf};

/// Binaries folder, relative to the game root (the folder holding HalfSwordUE5.exe).
pub const WIN64_REL: &str = "HalfswordUE5/Binaries/Win64";
/// The shipping exe inside WIN64_REL (hashed for the build check).
pub const EXE_NAME: &str = "HalfSwordUE5-Win64-Shipping.exe";
/// Shipping exe as a game-root-relative path.
pub fn exe_rel() -> String {
    format!("{WIN64_REL}/{EXE_NAME}")
}

pub fn win64(root: &Path) -> PathBuf {
    util::join_rel(root, WIN64_REL)
}
pub fn exe_path(root: &Path) -> PathBuf {
    win64(root).join(EXE_NAME)
}

/// A game root has the shipping exe at the expected place.
pub fn is_game_root(root: &Path) -> bool {
    exe_path(root).is_file()
}

/// Accept the game root, the Win64 folder, the shipping exe, or anything up to
/// four levels below the root (what people pick in a browse dialog).
pub fn normalize_game_root(p: &Path) -> Option<PathBuf> {
    let start = if p.is_file() { p.parent()? } else { p };
    let mut cur = Some(start);
    for _ in 0..5 {
        let c = cur?;
        if is_game_root(c) {
            return Some(c.to_path_buf());
        }
        cur = c.parent();
    }
    None
}

/// Result of hashing the shipping exe against the manifest's supported list.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildCheck {
    Supported { label: String, sha256: String },
    Unsupported { sha256: String, size: u64 },
}

impl BuildCheck {
    pub fn is_supported(&self) -> bool {
        matches!(self, BuildCheck::Supported { .. })
    }
    pub fn sha256(&self) -> &str {
        match self {
            BuildCheck::Supported { sha256, .. } | BuildCheck::Unsupported { sha256, .. } => sha256,
        }
    }
}

/// Compare a known (sha256, size) against the supported builds.
pub fn classify(sha256: &str, size: u64, compat: &GameCompat) -> BuildCheck {
    match compat.builds.iter().find(|b: &&GameBuild| b.exe_sha256.eq_ignore_ascii_case(sha256) && (b.exe_size == 0 || b.exe_size == size)) {
        Some(b) => BuildCheck::Supported { label: b.label.clone(), sha256: sha256.to_string() },
        None => BuildCheck::Unsupported { sha256: sha256.to_string(), size },
    }
}

/// Hash the shipping exe (about 150 MB: run it off the UI thread).
pub fn check_build(root: &Path, compat: &GameCompat) -> std::io::Result<BuildCheck> {
    let (sha, size) = util::sha256_file(&exe_path(root))?;
    Ok(classify(&sha, size, compat))
}

/// Files that mean an old-layout UE4SS (UE4SS.dll directly in Win64) is
/// installed. The pinned proxy would pick that up instead of ue4ss/UE4SS.dll,
/// so installing over it is refused rather than half-working.
pub fn old_layout_ue4ss(root: &Path) -> Vec<PathBuf> {
    let w = win64(root);
    ["UE4SS.dll", "UE4SS-settings.ini", "xinput1_3.dll"]
        .iter()
        .map(|n| w.join(n))
        .filter(|p| p.is_file())
        .filter(|p| !p.ends_with("xinput1_3.dll") || w.join("UE4SS.dll").is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{GameBuild, GameCompat};

    fn compat() -> GameCompat {
        GameCompat {
            steam_appid: 2397300,
            builds: vec![GameBuild { label: "EA 2026-10".into(), steam_buildid: Some("24185754".into()), exe_sha256: "ab".repeat(32), exe_size: 10 }],
        }
    }

    #[test]
    fn classify_builds() {
        assert!(classify(&"ab".repeat(32), 10, &compat()).is_supported());
        assert!(classify(&"AB".repeat(32), 10, &compat()).is_supported());
        assert!(!classify(&"ab".repeat(32), 11, &compat()).is_supported());
        assert!(!classify(&"cd".repeat(32), 10, &compat()).is_supported());
    }

    #[test]
    fn normalize_from_subpaths() {
        let t = crate::testutil::TempDir::new("game_norm");
        let root = t.path().join("Half Sword");
        std::fs::create_dir_all(win64(&root)).unwrap();
        std::fs::write(exe_path(&root), b"x").unwrap();
        assert_eq!(normalize_game_root(&root).as_deref(), Some(root.as_path()));
        assert_eq!(normalize_game_root(&win64(&root)).as_deref(), Some(root.as_path()));
        assert_eq!(normalize_game_root(&exe_path(&root)).as_deref(), Some(root.as_path()));
        assert_eq!(normalize_game_root(t.path()), None);
    }

    #[test]
    fn old_layout_detection() {
        let t = crate::testutil::TempDir::new("game_old");
        let w = win64(t.path());
        std::fs::create_dir_all(&w).unwrap();
        assert!(old_layout_ue4ss(t.path()).is_empty());
        std::fs::write(w.join("xinput1_3.dll"), b"x").unwrap();
        assert!(old_layout_ue4ss(t.path()).is_empty(), "xinput alone is not UE4SS");
        std::fs::write(w.join("UE4SS.dll"), b"x").unwrap();
        assert_eq!(old_layout_ue4ss(t.path()).len(), 2);
    }
}

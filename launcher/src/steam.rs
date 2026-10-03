//! Steam discovery: Steam root(s) from the registry, library folders from
//! `libraryfolders.vdf`, and the Half Sword install from
//! `appmanifest_2397300.acf`.
//!
//! The per-library `apps` list in libraryfolders.vdf is NOT trusted (it is
//! often stale); each library's `steamapps/appmanifest_<id>.acf` is the source
//! of truth, with a plain `steamapps/common/Half Sword` probe as a fallback.

use crate::game;
use crate::vdf;
use std::path::{Path, PathBuf};

/// Half Sword on Steam.
pub const HALF_SWORD_APPID: u32 = 2397300;

/// The fields of an `appmanifest_<id>.acf` the launcher needs.
#[derive(Debug, Clone, PartialEq)]
pub struct AppManifest {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub buildid: Option<String>,
    pub state_flags: Option<u32>,
}

impl AppManifest {
    /// StateFlags bit 2 (value 4) = fully installed; anything else set
    /// (update required, updating, ...) means Steam is not done with it.
    pub fn fully_installed(&self) -> bool {
        self.state_flags.map(|f| f == 4).unwrap_or(true)
    }
}

/// A Half Sword install found through Steam.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundGame {
    pub library: PathBuf,
    pub root: PathBuf,
    pub buildid: Option<String>,
    pub fully_installed: bool,
    /// How it was found ("appmanifest", "common-dir probe", "uninstall key").
    pub via: &'static str,
}

/// Library paths listed in a libraryfolders.vdf text (new and old formats).
///
/// New format (2021+): `"libraryfolders" { "0" { "path" "C:\\..." ... } }`.
/// Old format: `"LibraryFolders" { "TimeNextStatsReport" "..." "1" "D:\\SteamLibrary" }`.
pub fn parse_library_folders(text: &str) -> Vec<PathBuf> {
    let Ok(root) = vdf::parse(text) else { return vec![] };
    let Some(lf) = root.get("libraryfolders") else { return vec![] };
    let mut out = vec![];
    for (k, v) in lf.children() {
        if !k.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let p = match v {
            vdf::Vdf::Str(s) => Some(s.as_str()),
            vdf::Vdf::Obj(_) => v.str_of("path"),
        };
        if let Some(p) = p.map(str::trim).filter(|p| !p.is_empty()) {
            out.push(PathBuf::from(p));
        }
    }
    out
}

/// Parse an appmanifest_<id>.acf text.
pub fn parse_app_manifest(text: &str) -> Option<AppManifest> {
    let root = vdf::parse(text).ok()?;
    let st = root.get("AppState")?;
    Some(AppManifest {
        appid: st.str_of("appid")?.trim().parse().ok()?,
        name: st.str_of("name").unwrap_or("").to_string(),
        installdir: st.str_of("installdir").map(str::to_string).filter(|s| !s.trim().is_empty())?,
        buildid: st.str_of("buildid").map(str::to_string),
        state_flags: st.str_of("StateFlags").and_then(|s| s.trim().parse().ok()),
    })
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    crate::util::fold_path(a) == crate::util::fold_path(b)
}

/// Every library of a Steam root: the root itself plus the libraries listed in
/// `steamapps/libraryfolders.vdf` (and the legacy `config/libraryfolders.vdf`).
pub fn libraries_of(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    for f in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        if let Ok(t) = std::fs::read_to_string(steam_root.join(f)) {
            for p in parse_library_folders(&t) {
                if !libs.iter().any(|l| same_path(l, &p)) {
                    libs.push(p);
                }
            }
        }
    }
    libs
}

/// Look for `appid` in each library. Only installs whose game exe exists are
/// returned (a stale acf for an uninstalled game is ignored).
pub fn find_in_libraries(libraries: &[PathBuf], appid: u32) -> Vec<FoundGame> {
    let mut out: Vec<FoundGame> = vec![];
    for lib in libraries {
        let steamapps = lib.join("steamapps");
        let acf = steamapps.join(format!("appmanifest_{appid}.acf"));
        let mut found = None;
        if let Some(m) = std::fs::read_to_string(&acf).ok().and_then(|t| parse_app_manifest(&t)) {
            if m.appid == appid {
                let root = steamapps.join("common").join(&m.installdir);
                if game::is_game_root(&root) {
                    found = Some(FoundGame {
                        library: lib.clone(),
                        root,
                        fully_installed: m.fully_installed(),
                        buildid: m.buildid.clone(),
                        via: "appmanifest",
                    });
                }
            }
        }
        if found.is_none() && appid == HALF_SWORD_APPID {
            let root = steamapps.join("common").join("Half Sword");
            if game::is_game_root(&root) {
                found = Some(FoundGame { library: lib.clone(), root, buildid: None, fully_installed: true, via: "common-dir probe" });
            }
        }
        if let Some(f) = found {
            if !out.iter().any(|o| same_path(&o.root, &f.root)) {
                out.push(f);
            }
        }
    }
    out
}

/// Discover the game from a list of Steam roots (registry results in
/// production, fixture dirs in tests).
pub fn discover_from_roots(steam_roots: &[PathBuf], appid: u32) -> Vec<FoundGame> {
    let mut libs: Vec<PathBuf> = vec![];
    for r in steam_roots {
        for l in libraries_of(r) {
            if !libs.iter().any(|x| same_path(x, &l)) {
                libs.push(l);
            }
        }
    }
    find_in_libraries(&libs, appid)
}

/// Steam roots from the registry (HKCU SteamPath, HKLM InstallPath).
#[cfg(windows)]
pub fn registry_steam_roots() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let mut out: Vec<PathBuf> = vec![];
    let mut push = |s: Option<String>| {
        if let Some(s) = s.filter(|s| !s.trim().is_empty()) {
            let p = PathBuf::from(s.replace('/', "\\"));
            if p.is_dir() && !out.iter().any(|o| same_path(o, &p)) {
                out.push(p);
            }
        }
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    push(hkcu.open_subkey("Software\\Valve\\Steam").and_then(|k| k.get_value::<String, _>("SteamPath")).ok());
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for key in ["SOFTWARE\\WOW6432Node\\Valve\\Steam", "SOFTWARE\\Valve\\Steam"] {
        push(hklm.open_subkey(key).and_then(|k| k.get_value::<String, _>("InstallPath")).ok());
    }
    out
}

#[cfg(not(windows))]
pub fn registry_steam_roots() -> Vec<PathBuf> {
    vec![]
}

/// The game folder from Steam's per-app uninstall key, if present.
#[cfg(windows)]
pub fn registry_uninstall_location(appid: u32) -> Option<PathBuf> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for base in [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    ] {
        if let Ok(k) = hklm.open_subkey(format!("{base}\\Steam App {appid}")) {
            if let Ok(s) = k.get_value::<String, _>("InstallLocation") {
                let p = PathBuf::from(s);
                if game::is_game_root(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn registry_uninstall_location(_appid: u32) -> Option<PathBuf> {
    None
}

/// Steam is still downloading / updating the install at `root` (its
/// appmanifest StateFlags is not plain "fully installed").
pub fn is_updating(found: &[FoundGame], root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    found.iter().any(|f| !f.fully_installed && same_path(&canon(&f.root), &canon(root)))
}

/// Full discovery on this machine: registry Steam roots, then the uninstall key.
pub fn discover() -> Vec<FoundGame> {
    let mut found = discover_from_roots(&registry_steam_roots(), HALF_SWORD_APPID);
    if let Some(p) = registry_uninstall_location(HALF_SWORD_APPID) {
        if !found.iter().any(|f| same_path(&f.root, &p)) {
            found.push(FoundGame { library: p.clone(), root: p, buildid: None, fully_installed: true, via: "uninstall key" });
        }
    }
    found
}

/// steam.exe: HKCU SteamExe, else <root>/steam.exe of the first registry root.
#[cfg(windows)]
pub fn steam_exe() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(s) = hkcu.open_subkey("Software\\Valve\\Steam").and_then(|k| k.get_value::<String, _>("SteamExe")) {
        let p = PathBuf::from(s.replace('/', "\\"));
        if p.is_file() {
            return Some(p);
        }
    }
    registry_steam_roots().into_iter().map(|r| r.join("steam.exe")).find(|p| p.is_file())
}

#[cfg(not(windows))]
pub fn steam_exe() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    #[test]
    fn library_folders_new_format() {
        let libs = parse_library_folders(&fixture("libraryfolders_new.vdf"));
        assert_eq!(
            libs,
            vec![PathBuf::from("C:\\Program Files (x86)\\Steam"), PathBuf::from("D:\\SteamLibrary"), PathBuf::from("R:\\Games\\Steam Lib")]
        );
    }

    #[test]
    fn library_folders_old_format() {
        let libs = parse_library_folders(&fixture("libraryfolders_old.vdf"));
        assert_eq!(libs, vec![PathBuf::from("D:\\SteamLibrary"), PathBuf::from("E:\\Games\\Steam")]);
    }

    #[test]
    fn library_folders_garbage() {
        assert!(parse_library_folders("").is_empty());
        assert!(parse_library_folders("\"libraryfolders\" {").is_empty());
        assert!(parse_library_folders("\"other\" { \"0\" { \"path\" \"x\" } }").is_empty());
    }

    #[test]
    fn app_manifest() {
        let m = parse_app_manifest(&fixture("appmanifest_2397300.acf")).unwrap();
        assert_eq!(m.appid, 2397300);
        assert_eq!(m.name, "Half Sword");
        assert_eq!(m.installdir, "Half Sword");
        assert_eq!(m.buildid.as_deref(), Some("24185754"));
        assert!(m.fully_installed());
        let upd = parse_app_manifest(&fixture("appmanifest_updating.acf")).unwrap();
        assert!(!upd.fully_installed());
        assert!(parse_app_manifest("\"AppState\" { \"appid\" \"1\" }").is_none(), "no installdir");
    }

    fn fake_game(root: &Path) {
        let w = root.join(game::WIN64_REL);
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join(game::EXE_NAME), b"exe").unwrap();
    }

    #[test]
    fn discovery_over_fake_steam_tree() {
        let t = crate::testutil::TempDir::new("steam");
        let steam = t.path().join("Steam");
        let lib2 = t.path().join("Lib Two");
        let lib3 = t.path().join("Lib3");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib2.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib3.join("steamapps/common/Half Sword")).unwrap();
        let vdf_text = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\" {{ }}\n\t}}\n\t\"2\" {{ \"path\" \"{}\" }}\n}}\n",
            steam.display().to_string().replace('\\', "\\\\"),
            lib2.display().to_string().replace('\\', "\\\\"),
            lib3.display().to_string().replace('\\', "\\\\"),
        );
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), vdf_text).unwrap();
        // the game lives in lib2 under a custom installdir; lib3 has an empty folder (no exe)
        std::fs::write(lib2.join("steamapps/appmanifest_2397300.acf"), fixture("appmanifest_2397300.acf").replace("\"installdir\"\t\t\"Half Sword\"", "\"installdir\"\t\t\"HS Custom\"")).unwrap();
        fake_game(&lib2.join("steamapps/common/HS Custom"));
        let found = discover_from_roots(std::slice::from_ref(&steam), HALF_SWORD_APPID);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].root, lib2.join("steamapps").join("common").join("HS Custom"));
        assert_eq!(found[0].buildid.as_deref(), Some("24185754"));
        assert_eq!(found[0].via, "appmanifest");

        // no acf anywhere, but the default folder exists in lib3 -> probe finds it
        std::fs::remove_file(lib2.join("steamapps/appmanifest_2397300.acf")).unwrap();
        fake_game(&lib3.join("steamapps/common/Half Sword"));
        let found = discover_from_roots(&[steam], HALF_SWORD_APPID);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].via, "common-dir probe");
        assert!(found[0].root.starts_with(&lib3));
    }

    #[test]
    fn updating_install_is_detected_for_its_folder_only() {
        let t = crate::testutil::TempDir::new("steam_upd");
        let lib = t.path().join("Lib");
        std::fs::create_dir_all(lib.join("steamapps")).unwrap();
        std::fs::write(lib.join("steamapps/appmanifest_2397300.acf"), fixture("appmanifest_updating.acf")).unwrap();
        let m = parse_app_manifest(&fixture("appmanifest_updating.acf")).unwrap();
        let root = lib.join("steamapps").join("common").join(&m.installdir);
        fake_game(&root);
        let found = find_in_libraries(std::slice::from_ref(&lib), HALF_SWORD_APPID);
        assert_eq!(found.len(), 1);
        assert!(is_updating(&found, &root));
        assert!(is_updating(&found, Path::new(&root.to_string_lossy().to_uppercase())), "case-insensitive");
        assert!(!is_updating(&found, &t.path().join("elsewhere")));
    }

    #[test]
    fn discovery_nothing_installed() {
        let t = crate::testutil::TempDir::new("steam_empty");
        std::fs::create_dir_all(t.path().join("steamapps")).unwrap();
        assert!(discover_from_roots(&[t.path().to_path_buf()], HALF_SWORD_APPID).is_empty());
        assert!(discover_from_roots(&[t.path().join("missing")], HALF_SWORD_APPID).is_empty());
    }
}

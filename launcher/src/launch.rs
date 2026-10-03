//! Starting the game.
//!
//! * `Via::Direct`: the shipping exe with the working directory set to
//!   Binaries\Win64 and `HSMP_CFG` pointing at the absolute hsmp.cfg. This
//!   is the path every HSMP test has used (scripts/play.ps1, mp_test.ps1):
//!   hsmp.cfg, `bin_dir = hsmp` and `hsmp_state` are resolved relative to the
//!   working directory. Steam must be running (the game still talks to it).
//! * `Via::Steam`: `steam.exe -applaunch 2397300 <args>` (starts Steam when it
//!   is not running; Steam may show a one-time "launch with these arguments?"
//!   prompt).
//! * `Via::Auto` (the Play button): Direct when Steam is running, else Steam.
//!
//! Every route passes the manifest's launch arguments, e.g.
//! `-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0` (the hair-strand
//! streaming workaround), which works even after the game rewrote Engine.ini.

use crate::game;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Auto,
    Steam,
    Direct,
}

/// Resolve `Auto` against whether Steam is running.
pub fn resolve(via: Via, steam_running: bool) -> Via {
    match via {
        Via::Auto if steam_running => Via::Direct,
        Via::Auto => Via::Steam,
        v => v,
    }
}

/// Resolve the route for THIS game folder: `steam -applaunch` starts Steam's own
/// install of the app, which is not the folder the player chose with Browse (another copy,
/// maybe without HSMP or with another version). The Steam route is used only when the chosen
/// folder IS Steam's install; otherwise Play goes direct and needs Steam already running.
pub fn resolve_for(via: Via, steam_running: bool, is_steam_install: bool) -> Result<Via, String> {
    match resolve(via, steam_running) {
        Via::Steam if !is_steam_install => Err(if via == Via::Steam {
            "this Half Sword folder is not Steam's own install: launching through Steam would start Steam's copy instead. Use the direct route (start Steam first).".into()
        } else {
            "this Half Sword folder is not Steam's own install, so Play cannot go through Steam (that would start Steam's copy). Start Steam, then use Play again.".into()
        }),
        v => Ok(v),
    }
}

/// `root` is one of Steam's installs of the game (canonical paths compared).
pub fn is_steam_install(found: &[crate::steam::FoundGame], root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    found.iter().any(|f| crate::steam::same_path(&canon(&f.root), &canon(root)))
}

/// The command that would be run (for display and tests). `via` must be resolved.
pub fn command_line(via: Via, steam_exe: Option<&Path>, game_root: &Path, appid: u32, args: &[String]) -> Result<(std::path::PathBuf, Vec<String>), String> {
    match via {
        Via::Steam | Via::Auto => {
            let exe = steam_exe.ok_or("Steam was not found on this PC (is it installed?). Start Steam, then use Play again.")?;
            let mut a = vec!["-applaunch".to_string(), appid.to_string()];
            a.extend(args.iter().cloned());
            Ok((exe.to_path_buf(), a))
        }
        Via::Direct => {
            let exe = game::exe_path(game_root);
            if !exe.is_file() {
                return Err(format!("{} not found", exe.display()));
            }
            Ok((exe, args.to_vec()))
        }
    }
}

pub fn launch(via: Via, steam_exe: Option<&Path>, game_root: &Path, appid: u32, args: &[String]) -> Result<u32, String> {
    let (exe, a) = command_line(via, steam_exe, game_root, appid, args)?;
    let mut c = Command::new(&exe);
    c.args(&a);
    if via == Via::Direct {
        let w = game::win64(game_root);
        c.current_dir(&w); // hsmp.cfg, bin_dir and hsmp_state are cwd-relative
        c.env("HSMP_CFG", w.join("hsmp.cfg")); // first lookup of shared/hsmp_cfg.lua
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        c.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = c.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_command() {
        let args = vec!["-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0".to_string()];
        let (exe, a) = command_line(Via::Steam, Some(Path::new("C:\\Steam\\steam.exe")), Path::new("X:\\nowhere"), 2397300, &args).unwrap();
        assert_eq!(exe, Path::new("C:\\Steam\\steam.exe"));
        assert_eq!(a, vec!["-applaunch", "2397300", "-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0"]);
        assert!(command_line(Via::Steam, None, Path::new("X:\\"), 1, &args).is_err());
        assert!(command_line(Via::Direct, None, Path::new("X:\\nowhere"), 1, &args).is_err());
        assert_eq!(resolve(Via::Auto, true), Via::Direct);
        assert_eq!(resolve(Via::Auto, false), Via::Steam);
        assert_eq!(resolve(Via::Steam, true), Via::Steam);
        assert_eq!(resolve(Via::Direct, false), Via::Direct);
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use crate::steam::FoundGame;

    /// Steam's -applaunch only for Steam's own install of the game.
    #[test]
    fn steam_route_only_for_steams_install() {
        assert_eq!(resolve_for(Via::Auto, false, true), Ok(Via::Steam));
        assert_eq!(resolve_for(Via::Auto, true, false), Ok(Via::Direct));
        assert!(resolve_for(Via::Auto, false, false).unwrap_err().contains("Start Steam"));
        assert!(resolve_for(Via::Steam, true, false).unwrap_err().contains("Steam's copy"));
        assert_eq!(resolve_for(Via::Steam, true, true), Ok(Via::Steam));
        assert_eq!(resolve_for(Via::Direct, false, false), Ok(Via::Direct));
        let t = crate::testutil::TempDir::new("launch_route");
        let (steam_copy, other) = (t.path().join("steamapps/common/Half Sword"), t.path().join("Games/Half Sword"));
        std::fs::create_dir_all(&steam_copy).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let found = vec![FoundGame { library: t.path().to_path_buf(), root: steam_copy.clone(), buildid: None, fully_installed: true, via: "appmanifest" }];
        assert!(is_steam_install(&found, &steam_copy));
        assert!(is_steam_install(&found, Path::new(&steam_copy.to_string_lossy().to_uppercase())));
        assert!(!is_steam_install(&found, &other));
        assert!(!is_steam_install(&[], &steam_copy));
    }
}

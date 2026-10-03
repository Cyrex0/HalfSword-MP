//! Starting the game through Steam.
//!
//! `Via::Steam` runs `steam.exe -applaunch 2397300 <args>`. Steam starts itself when it is
//! not running, and may show a one-time "launch with these arguments?" prompt.
//!
//! The arguments are the manifest's launch arguments, e.g.
//! `-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0` (the hair-strand streaming
//! workaround), which works even after the game rewrote Engine.ini.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Steam,
}

/// `steam -applaunch` starts Steam's own install of the app, which is not the folder the
/// player chose with Browse (another copy, maybe without HSMP or with another version).
pub fn check_route(via: Via, is_steam_install: bool) -> Result<(), String> {
    match via {
        Via::Steam if !is_steam_install => Err("this Half Sword folder is not Steam's own install: launching through Steam would start Steam's copy instead. Start the game from that folder yourself.".into()),
        Via::Steam => Ok(()),
    }
}

/// `root` is one of Steam's installs of the game (canonical paths compared).
pub fn is_steam_install(found: &[crate::steam::FoundGame], root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    found.iter().any(|f| crate::steam::same_path(&canon(&f.root), &canon(root)))
}

/// The command that would be run (for display and tests).
pub fn command_line(via: Via, steam_exe: Option<&Path>, appid: u32, args: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    match via {
        Via::Steam => {
            let exe = steam_exe.ok_or("Steam was not found on this PC (is it installed?). Start Steam, then try again.")?;
            let mut a = vec!["-applaunch".to_string(), appid.to_string()];
            a.extend(args.iter().cloned());
            Ok((exe.to_path_buf(), a))
        }
    }
}

pub fn launch(via: Via, steam_exe: Option<&Path>, appid: u32, args: &[String]) -> Result<u32, String> {
    let (exe, a) = command_line(via, steam_exe, appid, args)?;
    let mut c = Command::new(&exe);
    c.args(&a);
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
    use crate::steam::FoundGame;

    #[test]
    fn steam_command() {
        let args = vec!["-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0".to_string()];
        let (exe, a) = command_line(Via::Steam, Some(Path::new("C:\\Steam\\steam.exe")), 2397300, &args).unwrap();
        assert_eq!(exe, Path::new("C:\\Steam\\steam.exe"));
        assert_eq!(a, vec!["-applaunch", "2397300", "-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0"]);
        let e = command_line(Via::Steam, None, 1, &args).unwrap_err();
        assert!(e.contains("Steam was not found"), "{e}");
    }

    /// Steam's -applaunch only for Steam's own install of the game.
    #[test]
    fn steam_route_only_for_steams_install() {
        assert_eq!(check_route(Via::Steam, true), Ok(()));
        assert!(check_route(Via::Steam, false).unwrap_err().contains("Steam's copy"));
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

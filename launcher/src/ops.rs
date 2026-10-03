//! High-level operations shared by the CLI and the GUI.

use crate::game::{self, BuildCheck};
use crate::install::{self, Env, InstallOpts, InstallReport, Status};
use crate::launch::{self, Via};
use crate::package::{self, Package};
use crate::steam;
use crate::util;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub game_root: Option<String>,
}

/// %LOCALAPPDATA%\HSMP
pub fn hsmp_home() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("HSMP")).ok_or_else(|| "LOCALAPPDATA is not set".to_string())
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(hsmp_home()?.join("launcher").join("settings.json"))
}

pub fn load_settings() -> Settings {
    settings_path().ok().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(p) = settings_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = util::atomic_write(&p, &serde_json::to_vec_pretty(s).unwrap_or_default());
    }
}

/// Append a line to %LOCALAPPDATA%\HSMP\launcher\launcher.log (best effort).
pub fn log_line(line: &str) {
    if let Ok(h) = hsmp_home() {
        let p = h.join("launcher").join("launcher.log");
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{} {line}", util::iso_utc(util::now_unix()));
        }
    }
}

/// Pick the game folder: explicit path, then the remembered one, then Steam
/// discovery (exactly one hit, or an error listing the candidates).
pub fn choose_game(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return game::normalize_game_root(p).ok_or_else(|| format!("{} is not a Half Sword folder ({} not found)", p.display(), game::exe_rel()));
    }
    if let Some(p) = load_settings().game_root.map(PathBuf::from) {
        if game::is_game_root(&p) {
            return Ok(p);
        }
    }
    let found = steam::discover();
    match found.len() {
        0 => Err("Half Sword was not found through Steam. Pass --game \"<folder with HalfSwordUE5.exe>\"".into()),
        1 => Ok(found[0].root.clone()),
        _ => Err(format!(
            "Half Sword was found in several places; pick one with --game:\n{}",
            found.iter().map(|f| format!("  {}", f.root.display())).collect::<Vec<_>>().join("\n")
        )),
    }
}

/// Open and signature-check the package (default: the launcher's own folder).
pub fn open_package(path: Option<&Path>) -> Result<Package, String> {
    let p = match path {
        Some(p) => p.to_path_buf(),
        None => package::default_package_dir().ok_or("cannot locate the launcher folder")?,
    };
    // only keys compiled into THIS launcher (+ a key file pinned outside any
    // package) are trusted; nothing inside the package is
    let keys = crate::trust::effective_keys(hsmp_home().ok().as_deref())?;
    Package::open(&p, &keys)
}

pub fn check_build(root: &Path, pkg: &Package) -> Result<BuildCheck, String> {
    game::check_build(root, &pkg.manifest.game).map_err(|e| format!("cannot read the game exe: {e}"))
}

/// Verify every package file, then install/update (journaled, all-or-nothing).
pub fn install(env: &Env, pkg: &Package, build: &BuildCheck, allow_unsupported: bool, backup_saves: bool, log: &mut dyn FnMut(String)) -> Result<InstallReport, String> {
    install_ex(env, pkg, build, allow_unsupported, backup_saves, false, log)
}

pub fn install_ex(
    env: &Env,
    pkg: &Package,
    build: &BuildCheck,
    allow_unsupported: bool,
    backup_saves: bool,
    allow_downgrade: bool,
    log: &mut dyn FnMut(String),
) -> Result<InstallReport, String> {
    log(format!("verifying {} package files...", pkg.manifest.files.len()));
    let files = pkg.load_files(true)?;
    log("package verified (signature and SHA-256 of every file)".into());
    let opts = InstallOpts {
        backup_saves,
        allow_unsupported,
        check_processes: true,
        allow_downgrade,
        steam_updating: steam::is_updating(&steam::discover(), &env.game_root),
        exe_sha256: Some(build.sha256().to_string()),
        fail_after_writes: None,
    };
    let r = install::install(env, &pkg.manifest, &files, &opts, log)?;
    // keep the verified launcher of this release outside the package: later
    // updates are checked by a launcher you already trust
    match crate::trust::install_launcher_copy(&env.hsmp_home, &pkg.manifest, &files) {
        Ok(Some(p)) => log(format!("for updates, start {} and choose \"Open another release...\"", p.display())),
        Ok(None) => {}
        Err(e) => log(format!("note: could not keep a launcher copy for updates: {e}")),
    }
    Ok(r)
}

/// Launch arguments of the installed release.
pub fn launch_args(env: &Env) -> Result<Vec<String>, String> {
    Ok(install::load_state(env)?.map(|s| s.launch_args).unwrap_or_default())
}

pub fn launch(env: &Env, via: Via, log: &mut dyn FnMut(String)) -> Result<u32, String> {
    match install::status(env)? {
        Status::Installed { modified, missing, .. } => {
            if !missing.is_empty() {
                return Err(format!("{} HSMP file(s) are missing ({}...). Run Repair first.", missing.len(), missing[0]));
            }
            if !modified.is_empty() {
                log(format!("note: {} HSMP file(s) differ from the release ({}); Repair restores them", modified.len(), modified.join(", ")));
            }
        }
        Status::Interrupted => return Err("an install was interrupted; run Install again first".into()),
        Status::UninstallIncomplete { .. } => return Err("an uninstall has not finished; run Uninstall again (or Install to repair)".into()),
        Status::NotInstalled { .. } => return Err("HSMP is not installed in this game folder yet".into()),
    }
    if crate::procs::game_running() {
        return Err("Half Sword is already running".into());
    }
    // An MP session that crashed left its career-guard backup open; recover it now,
    // BEFORE any career play could make the damaged file look like newer legitimate play
    match crate::careerguard::recover(env, log)? {
        Some(r) if !r.ok => return Err(crate::careerguard::failure_text(&r, "Play")),
        _ => {}
    }
    install::reapply_ini(env, log)?;
    let args = launch_args(env)?;
    let via = launch::resolve_for(via, crate::procs::steam_running(), launch::is_steam_install(&steam::discover(), &env.game_root))?;
    let steam_exe = steam::steam_exe();
    let (exe, a) = launch::command_line(via, steam_exe.as_deref(), &env.game_root, steam::HALF_SWORD_APPID, &args)?;
    log(format!("starting: \"{}\" {}", exe.display(), a.join(" ")));
    launch::launch(via, steam_exe.as_deref(), &env.game_root, steam::HALF_SWORD_APPID, &args)
}

/// Uninstall, after recovering any career-guard session a crash left open (the
/// sidecar that could recover it is about to be removed). A failed recovery refuses the
/// uninstall; a sidecar that cannot be found or started does not (its backups stay listed
/// under Restore), and is said in the log.
pub fn uninstall(env: &Env, opts: &install::UninstallOpts, log: &mut dyn FnMut(String)) -> Result<install::UninstallReport, String> {
    let rec = crate::careerguard::recover(env, log);
    uninstall_after_recovery(rec, env, opts, log)
}

fn uninstall_after_recovery(
    rec: Result<Option<crate::careerguard::Recovery>, String>,
    env: &Env,
    opts: &install::UninstallOpts,
    log: &mut dyn FnMut(String),
) -> Result<install::UninstallReport, String> {
    match rec {
        Ok(Some(r)) if !r.ok => return Err(crate::careerguard::failure_text(&r, "Uninstall")),
        Ok(_) => {}
        Err(e) => log(format!("note: {e}; check Saves > Restore for the career guard's MP backups")),
    }
    install::uninstall_with(env, opts, log)
}

/// One-line human status.
pub fn status_line(st: &Status) -> String {
    match st {
        Status::NotInstalled { foreign_hsmp: true, .. } => "Not installed by the launcher. HSMP/UE4SS files from a manual install were found; they are backed up and put back on uninstall.".into(),
        Status::NotInstalled { foreign_ue4ss: true, .. } => "Not installed. UE4SS is already present (another mod); it is backed up and put back on uninstall.".into(),
        Status::NotInstalled { .. } => "Not installed.".into(),
        Status::Interrupted => "An install was interrupted. Install again (it rolls back first).".into(),
        Status::UninstallIncomplete { remaining } => {
            format!("Uninstall did not finish: {} file(s) still to restore. Click Uninstall again (see the log for what blocked it).", remaining.len())
        }
        Status::Installed { version, protocol, modified, missing } if modified.is_empty() && missing.is_empty() => format!("Installed: HSMP {version} (protocol {protocol})."),
        Status::Installed { version, modified, missing, .. } => {
            format!("Installed: HSMP {version}, but {} file(s) changed and {} missing. Use Repair.", modified.len(), missing.len())
        }
    }
}

/// "Back up now" / `backup-saves`: a career save backup under the launcher's operation lock
/// (taken so these two cannot race an install's or a restore's
/// writes into save_backups).
pub fn backup_saves(hsmp_home: &Path, save_dir: &Path, backups_root: &Path, reason: &str) -> Result<Option<crate::saves::Backup>, String> {
    let _lock = install::lock(hsmp_home)?;
    crate::saves::backup(save_dir, backups_root, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn backup_saves_takes_the_operation_lock() {
        let t = TempDir::new("ops_backup_lock");
        let (home, saves, root) = (t.path().join("HSMP"), t.path().join("SaveGames"), t.path().join("HSMP/save_backups"));
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("GameProgress.sav"), b"career").unwrap();
        {
            let _held = install::lock(&home).unwrap();
            if cfg!(windows) {
                let e = backup_saves(&home, &saves, &root, "manual").unwrap_err();
                assert!(e.contains("another HSMP launcher"), "{e}");
                assert!(crate::saves::list(&root).is_empty());
            }
        }
        let b = backup_saves(&home, &saves, &root, "manual").unwrap().unwrap();
        assert!(b.has_career());
    }

    /// A failed career recovery refuses Uninstall; a sidecar that cannot be started
    /// does not (the normal uninstall path runs; here the folder has no install).
    #[test]
    fn uninstall_policy_after_career_recovery() {
        let t = TempDir::new("ops_uninstall_cg");
        let env = Env::new(&t.path().join("game"), t.path().join("la/HSMP"), t.path().join("la/HalfSwordUE5/Saved"));
        let failed = crate::careerguard::parse("{\"ev\":\"career_guard\",\"action\":\"error\",\"file\":null,\"why\":\"restore failed: denied\",\"kind\":\"recover\",\"backup\":\"B\"}\n", Some(1));
        let e = uninstall_after_recovery(Ok(Some(failed)), &env, &install::UninstallOpts::default(), &mut |_| {}).unwrap_err();
        assert!(e.contains("restore failed: denied") && e.contains("Uninstall is blocked"), "{e}");
        let mut logs = vec![];
        let r = uninstall_after_recovery(Err("cannot start x".into()), &env, &install::UninstallOpts::default(), &mut |s| logs.push(s));
        if let Err(e) = r {
            assert!(!e.contains("career save check"), "{e}");
        }
        assert!(logs.iter().any(|l| l.contains("cannot start x")), "{logs:?}");
    }
}

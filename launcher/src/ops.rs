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
#[serde(default)]
pub struct Settings {
    pub game_root: Option<String>,
    /// update from stable releases only (default: pre-releases too)
    pub stable_only: bool,
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

/// Change one setting, keeping the others.
pub fn update_settings(f: impl FnOnce(&mut Settings)) {
    let mut s = load_settings();
    f(&mut s);
    save_settings(&s);
}

/// Append a line to %LOCALAPPDATA%\HSMP\launcher\launcher.log (best effort).
pub fn log_line(line: &str) {
    if let Ok(h) = hsmp_home() {
        let p = h.join("launcher").join("launcher.log");
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        // one old copy above 4 MB
        if std::fs::metadata(&p).is_ok_and(|m| m.len() > 4 << 20) {
            let _ = std::fs::rename(&p, p.with_file_name("launcher.1.log"));
        }
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
    match Package::open(&p, &keys) {
        // the launcher copy in HSMP\bin has no release next to it: use the downloaded one
        Err(e) if path.is_none() => match downloaded_release(hsmp_home().ok().as_deref()) {
            Some(z) => Package::open(&z, &keys),
            None => Err(e),
        },
        r => r,
    }
}

/// The downloaded zip of this launcher's own release, if an update brought it.
fn downloaded_release(hsmp_home: Option<&Path>) -> Option<PathBuf> {
    let z = crate::update::download_dir(hsmp_home?).join(format!("hsmp-{}.zip", crate::LAUNCHER_VERSION));
    z.is_file().then_some(z)
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
    let opts = InstallOpts {
        backup_saves,
        allow_unsupported,
        check_processes: true,
        allow_downgrade,
        steam_updating: steam::is_updating(&steam::discover(), &env.game_root),
        exe_sha256: Some(build.sha256().to_string()),
        fail_after_writes: None,
    };
    install_with(env, pkg, opts, log)
}

/// Verify every package file, install, keep the launcher copy, then run the start-up check.
pub fn install_with(env: &Env, pkg: &Package, opts: InstallOpts, log: &mut dyn FnMut(String)) -> Result<InstallReport, String> {
    log(format!("verifying {} package files...", pkg.manifest.files.len()));
    let files = pkg.load_files(true)?;
    log("package verified (signature and SHA-256 of every file)".into());
    let mut r = install::install(env, &pkg.manifest, &files, &opts, log)?;
    // keep the verified launcher of this release outside the package: later
    // updates are checked by a launcher you already trust
    match crate::trust::install_launcher_copy(&env.hsmp_home, &pkg.manifest, &files) {
        Ok(Some(p)) => log(format!("launcher for updates: {}", p.display())),
        Ok(None) => {}
        Err(e) => log(format!("note: could not keep a launcher copy for updates: {e}")),
    }
    // the freshly installed sidecar recovers what an older one left open
    if let Err(e) = startup_check(env, log) {
        r.notes.push(e);
    }
    Ok(r)
}

/// Runs at launcher start, after every install/update and before "Launch through Steam"
/// (players may also start the game from Steam directly): finish the career save check of an
/// MP session that crashed, and put back ini settings the game dropped. Skipped while the
/// game or an HSMP binary from this folder runs: a live session's guard backup is open,
/// not crashed. The game's own sidecar repeats the check at every MP session start.
pub fn startup_check(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    let running = crate::procs::blocking_processes(&env.game_root);
    if !running.is_empty() {
        log(format!("career save check skipped while {} runs; it runs the next time the launcher opens", running.join(", ")));
        return Ok(());
    }
    rescue_session_logs(env, log);
    let rec = crate::careerguard::recover(env, log);
    if let Err(e) = install::reapply_ini(env, log) {
        log(format!("note: ini settings not re-applied: {e}"));
    }
    startup_verdict(rec)
}

/// Where the per-run log folders are: `HSMP_LOGS_DIR`, else `<HSMP home>ogs`.
pub fn logs_root(hsmp_home: &Path) -> PathBuf {
    std::env::var_os("HSMP_LOGS_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| hsmp_home.join("logs"))
}

/// A run whose log watcher never finished (PC shut down, watcher stopped): copy its
/// UE4SS.log into its session folder before the next game start overwrites it. Runs with
/// the start-up check, so never while the game runs.
pub fn rescue_session_logs(env: &Env, log: &mut dyn FnMut(String)) {
    let root = logs_root(&env.hsmp_home);
    if !root.is_dir() {
        return;
    }
    let win64 = game::win64(&env.game_root);
    let src = hsmp_diag::collect::Sources {
        ue4ss_log: Some(win64.join("ue4ss").join("UE4SS.log")),
        ue4ss_dir: Some(win64.join("ue4ss")),
        ue_saved: Some(env.ue_saved.clone()),
        state_dir: Some(win64.join("hsmp_state")),
    };
    hsmp_diag::sessions::close_stale(&root, &hsmp_diag::proc::alive);
    if let Some(id) = hsmp_diag::collect::rescue(&root, &src, hsmp_diag::time::now_ms()) {
        log(format!("logs: kept UE4SS.log of the unfinished game run {id}"));
    }
}

fn startup_verdict(rec: Result<Option<crate::careerguard::Recovery>, String>) -> Result<(), String> {
    match rec {
        Ok(Some(r)) if !r.ok => Err(format!(
            "{}. Do not play your career until this is fixed: restore it from Saves > Restore (the guard's MP backups are listed there), or reopen the launcher to try again.",
            crate::careerguard::check_failed_text(&r)
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("the career save check could not run: {e}. Check Saves > Restore for the career guard's MP backups.")),
    }
}

/// Launch arguments of the installed release.
pub fn launch_args(env: &Env) -> Result<Vec<String>, String> {
    Ok(install::load_state(env)?.map(|s| s.launch_args).unwrap_or_default())
}

/// The `steam -applaunch` command for this game folder.
pub fn steam_command(args: &[String], steam_exe: Option<&Path>, is_steam_install: bool) -> Result<(PathBuf, Vec<String>), String> {
    launch::check_route(Via::Steam, is_steam_install)?;
    launch::command_line(Via::Steam, steam_exe, steam::HALF_SWORD_APPID, args)
}

/// "Launch through Steam" / `launch`: the start-up check, then `steam -applaunch` with the
/// release's launch options. Steam starts itself when it is not running.
pub fn launch(env: &Env, log: &mut dyn FnMut(String)) -> Result<u32, String> {
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
    // recover a crashed MP session BEFORE career play could make the damaged file look newer
    startup_check(env, log)?;
    let args = launch_args(env)?;
    let steam_exe = steam::steam_exe();
    let (exe, a) = steam_command(&args, steam_exe.as_deref(), launch::is_steam_install(&steam::discover(), &env.game_root))?;
    log(format!("starting: \"{}\" {}", exe.display(), a.join(" ")));
    launch::launch(Via::Steam, Some(&exe), steam::HALF_SWORD_APPID, &args)
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

/// After an install or update: make sure Windows Firewall lets hsmp-server.exe in (one UAC
/// prompt when it does not). Never fails the install; returns the warning to show, if any.
pub fn ensure_firewall(env: &Env, log: &mut dyn FnMut(String)) -> Option<String> {
    use crate::firewall::{self, Outcome};
    if !cfg!(windows) {
        return None;
    }
    let exe = firewall::server_exe(&env.game_root);
    match firewall::ensure(&exe) {
        Outcome::AlreadyAllowed => {
            log("firewall: hsmp-server.exe is already allowed".into());
            None
        }
        Outcome::Added => {
            log(format!("firewall: added the inbound UDP rule \"{}\" for {}", firewall::RULE_NAME, exe.display()));
            None
        }
        Outcome::Declined => Some(format!("the Windows prompt was declined. {}", firewall::NOT_ALLOWED_WARNING)),
        Outcome::Failed(e) => Some(format!("{e}. {}", firewall::NOT_ALLOWED_WARNING)),
        Outcome::Removed | Outcome::NothingToRemove => None,
    }
}

/// After an uninstall: remove the firewall rule (one UAC prompt when there is one). Returns
/// a note when it stays.
pub fn remove_firewall(env: &Env, log: &mut dyn FnMut(String)) -> Option<String> {
    use crate::firewall::{self, Outcome};
    if !cfg!(windows) {
        return None;
    }
    let left = |why: String| Some(format!("{why}; the firewall rule \"{}\" was left in place (remove it in Windows Defender Firewall, or run hsmp-launcher firewall-remove as administrator)", firewall::RULE_NAME));
    match firewall::remove(&firewall::server_exe(&env.game_root)) {
        Outcome::Removed => {
            log(format!("firewall: removed the rule \"{}\"", firewall::RULE_NAME));
            None
        }
        Outcome::Declined => left("the Windows prompt was declined".into()),
        Outcome::Failed(e) => left(e),
        _ => None,
    }
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

    /// At launcher start a failed or unrunnable recovery is reported; a clean one or a
    /// missing sidecar is not.
    #[test]
    fn startup_check_verdicts() {
        let failed = crate::careerguard::parse("{\"ev\":\"career_guard\",\"action\":\"error\",\"file\":null,\"why\":\"restore failed: denied\",\"kind\":\"recover\",\"backup\":\"B\"}\n", Some(1));
        let e = startup_verdict(Ok(Some(failed))).unwrap_err();
        assert!(e.contains("restore failed: denied") && e.contains("Do not play your career"), "{e}");
        assert!(startup_verdict(Err("cannot start x".into())).unwrap_err().contains("cannot start x"));
        assert!(startup_verdict(Ok(None)).is_ok());
        assert!(startup_verdict(Ok(Some(crate::careerguard::parse("", Some(0))))).is_ok());
    }

    /// Nothing installed: the start-up check is a quiet no-op.
    #[test]
    fn startup_check_without_install() {
        let t = TempDir::new("ops_startup_none");
        let env = Env::new(&t.path().join("game"), t.path().join("la/HSMP"), t.path().join("la/HalfSwordUE5/Saved"));
        let mut logs = vec![];
        startup_check(&env, &mut |s| logs.push(s)).unwrap();
        assert!(logs.iter().all(|l| !l.contains("not re-applied")), "{logs:?}");
    }

    /// The shipped release's launch options go to `steam -applaunch` unchanged, the IO-1
    /// hair cvars included; a folder that is not Steam's install is refused.
    #[test]
    fn steam_launch_command_of_the_release() {
        let rel: serde_json::Value = serde_json::from_str(include_str!("../../tools/release/release.json")).unwrap();
        let args: Vec<String> = rel["launch_args"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let steam_exe = Path::new("C:\\Program Files (x86)\\Steam\\steam.exe");
        let (exe, a) = steam_command(&args, Some(steam_exe), true).unwrap();
        assert_eq!(exe, steam_exe);
        assert_eq!(a, vec!["-applaunch", "2397300", "-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0",
                           "-ini:Engine:[SystemSettings]:r.HairStrands.UseCardsInsteadOfStrands=1"]);
        assert!(steam_command(&args, Some(steam_exe), false).unwrap_err().contains("Steam's copy"));
        assert!(steam_command(&args, None, true).unwrap_err().contains("Steam was not found"));
    }

    /// Nothing installed: launch refuses before touching Steam.
    #[test]
    fn launch_without_install() {
        let t = TempDir::new("ops_launch_none");
        let env = Env::new(&t.path().join("game"), t.path().join("la/HSMP"), t.path().join("la/HalfSwordUE5/Saved"));
        let e = launch(&env, &mut |_| {}).unwrap_err();
        assert!(e.contains("not installed"), "{e}");
        assert!(launch_args(&env).unwrap().is_empty());
    }
}

//! hsmp-launcher: double-click for the window; or use the command line.
//!
//!     hsmp-launcher                          open the launcher window
//!     hsmp-launcher status        [--game DIR] [--package DIR|ZIP]
//!     hsmp-launcher verify        [--package DIR|ZIP]
//!     hsmp-launcher install       [--game DIR] [--package DIR|ZIP] [--allow-unsupported] [--no-save-backup]
//!     hsmp-launcher uninstall     [--game DIR]
//!     hsmp-launcher launch        [--game DIR]
//!     hsmp-launcher check-update  [--stable-only]
//!     hsmp-launcher update        [--game DIR] [--stable-only] [--allow-downgrade]
//!     hsmp-launcher backup-saves
//!     hsmp-launcher list-backups
//!     hsmp-launcher restore-saves <backup id>
//!     hsmp-launcher find-game
//!     hsmp-launcher firewall-status [--game DIR]
//!
//! `firewall-allow <exe>` and `firewall-remove [<exe>]` are the helper the launcher starts
//! through UAC to change Windows Firewall; they are not meant to be typed.
//!
//! Exit codes: 0 ok, 1 failed, 2 bad usage.

#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
mod gui;

use hsmp_launcher::install::{self, Env};
use hsmp_launcher::{firewall, ops, saves, steam, update};
use std::path::PathBuf;

const USAGE: &str = "hsmp-launcher [status|verify|install|uninstall|launch|check-update|update|backup-saves|list-backups|restore-saves <id>|find-game|firewall-status]
  --game DIR          Half Sword folder (default: remembered, else found through Steam)
  --package DIR|ZIP   release folder or zip (default: the launcher's own folder)
  --allow-unsupported install on a Half Sword build this release does not list
  --no-save-backup    skip the automatic career save backup before the first install
  --allow-downgrade   install a release older than the installed one
  --stable-only       check-update / update: stable releases only (default: the saved setting)
  --forget-missing    uninstall: give up on backed-up originals that are gone for good
                      (an antivirus deleted them); HSMP's version of those files is removed
launch: start Half Sword through Steam (steam -applaunch) with the HSMP launch options
firewall-status: is hsmp-server.exe allowed through Windows Firewall (install adds the rule)
Without arguments the launcher window opens.";

#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: plain Win32 call; failure (no parent console) is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
fn attach_console() {}

struct Args {
    cmd: String,
    pos: Vec<String>,
    game: Option<PathBuf>,
    package: Option<PathBuf>,
    allow_unsupported: bool,
    allow_downgrade: bool,
    forget_missing: bool,
    no_save_backup: bool,
    stable_only: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args { cmd: String::new(), pos: vec![], game: None, package: None, allow_unsupported: false, allow_downgrade: false, forget_missing: false, no_save_backup: false, stable_only: false };
    let mut it = argv.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--game" => a.game = Some(PathBuf::from(it.next().ok_or("--game needs a folder")?)),
            "--package" => a.package = Some(PathBuf::from(it.next().ok_or("--package needs a folder or zip")?)),
            "--allow-unsupported" => a.allow_unsupported = true,
            "--allow-downgrade" => a.allow_downgrade = true,
            "--forget-missing" => a.forget_missing = true,
            "--no-save-backup" => a.no_save_backup = true,
            "--stable-only" => a.stable_only = true,
            "-h" | "--help" | "help" => a.cmd = "help".into(),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if a.cmd.is_empty() => a.cmd = s.to_string(),
            s => a.pos.push(s.to_string()),
        }
    }
    Ok(a)
}

fn say(s: String) {
    println!("{s}");
    ops::log_line(&s);
}

fn env_for(a: &Args) -> Result<Env, String> {
    let root = ops::choose_game(a.game.as_deref())?;
    Env::system(&root)
}

fn run_cli(a: Args) -> Result<(), String> {
    let mut log = |s: String| say(format!("  {s}"));
    match a.cmd.as_str() {
        "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "find-game" => {
            let found = steam::discover();
            if found.is_empty() {
                return Err("Half Sword was not found through Steam".into());
            }
            for f in found {
                say(format!("{}  (build {}, via {}{})", f.root.display(), f.buildid.unwrap_or_else(|| "?".into()), f.via, if f.fully_installed { "" } else { ", Steam is still updating it" }));
            }
            Ok(())
        }
        "verify" => {
            let p = ops::open_package(a.package.as_deref())?;
            say(format!("signature OK: {} signed by {}", p.describe(), p.signed_by));
            say(format!(
                "note: the signature proves the files are intact and signed by a key THIS launcher trusts (sha256 {}). For a first download, compare the zip's SHA-256 with the official release page.",
                hsmp_launcher::trust::self_sha256().unwrap_or_else(|| "?".into())
            ));
            let files = p.load_files(true)?;
            say(format!("all {} files match the manifest (HSMP {}, protocol {}, commit {})", files.len(), p.manifest.version, p.manifest.protocol_version, p.manifest.git_commit));
            Ok(())
        }
        "status" => {
            let env = env_for(&a)?;
            say(format!("game:    {}", env.game_root.display()));
            match ops::open_package(a.package.as_deref()) {
                Ok(p) => {
                    say(format!("package: HSMP {} (protocol {}), signed by {}", p.manifest.version, p.manifest.protocol_version, p.signed_by));
                    match ops::check_build(&env.game_root, &p) {
                        Ok(b) if b.is_supported() => say("build:   supported".into()),
                        Ok(b) => say(format!("build:   NOT supported by this release (exe sha256 {})", b.sha256())),
                        Err(e) => say(format!("build:   {e}")),
                    }
                }
                Err(e) => say(format!("package: {e}")),
            }
            say(format!("hsmp:    {}", ops::status_line(&install::status(&env)?)));
            let b = saves::list(&env.save_backups());
            say(format!("saves:   {} backup(s) in {}", b.len(), env.save_backups().display()));
            Ok(())
        }
        "install" => {
            let env = env_for(&a)?;
            let p = ops::open_package(a.package.as_deref())?;
            say(format!("installing HSMP {} into {}", p.manifest.version, env.game_root.display()));
            let build = ops::check_build(&env.game_root, &p)?;
            if !build.is_supported() {
                if !a.allow_unsupported {
                    return Err(format!(
                        "this Half Sword build (exe sha256 {}) is not supported by HSMP {}. Wait for an HSMP update, or pass --allow-unsupported to try anyway.",
                        build.sha256(),
                        p.manifest.version
                    ));
                }
                say("WARNING: unsupported Half Sword build; installing anyway as requested".into());
            }
            let r = ops::install_ex(&env, &p, &build, a.allow_unsupported, !a.no_save_backup, a.allow_downgrade, &mut log)?;
            ops::update_settings(|s| s.game_root = Some(env.game_root.to_string_lossy().to_string()));
            match &r.updated_from {
                Some(v) => say(format!("updated HSMP {v} -> {} ({} change(s), {} file(s) removed)", r.version, r.files_written, r.files_removed)),
                None => say(format!("installed HSMP {} ({} file(s) written)", r.version, r.files_written)),
            }
            for n in r.notes {
                say(format!("note: {n}"));
            }
            if let Some(w) = ops::ensure_firewall(&env, &mut log) {
                say(format!("WARNING: {w}"));
            }
            Ok(())
        }
        "uninstall" => {
            let env = env_for(&a)?;
            let r = ops::uninstall(&env, &install::UninstallOpts { check_processes: true, forget_missing: a.forget_missing }, &mut log)?;
            say(format!("uninstalled: {} file(s) restored, {} removed", r.restored, r.removed));
            for n in r.notes {
                say(format!("note: {n}"));
            }
            if let Some(w) = ops::remove_firewall(&env, &mut log) {
                say(format!("note: {w}"));
            }
            Ok(())
        }
        "launch" => {
            let env = env_for(&a)?;
            let pid = ops::launch(&env, &mut log)?;
            say(format!("started through Steam (pid {pid})"));
            Ok(())
        }
        "check-update" | "update" => {
            let home = ops::hsmp_home()?;
            let stable = a.stable_only || ops::load_settings().stable_only;
            // installed HSMP if there is one, else this launcher's version
            let env = env_for(&a).ok();
            let cur = match env.as_ref().map(install::status) {
                Some(Ok(install::Status::Installed { version, .. })) => version,
                _ => hsmp_launcher::LAUNCHER_VERSION.to_string(),
            };
            let c = update::Client::new();
            let Some(av) = c.check(&cur, stable, Some(&update::cache_path(&home)))? else {
                say("no HSMP release is published for this channel yet".into());
                return Ok(());
            };
            let newer = av.ordering() == std::cmp::Ordering::Greater;
            say(match av.ordering() {
                std::cmp::Ordering::Greater => format!("update available: {cur} -> {}", av.version),
                std::cmp::Ordering::Equal => format!("HSMP {cur} is the latest release"),
                std::cmp::Ordering::Less => format!("HSMP {cur} is newer than the latest release ({})", av.version),
            });
            if a.cmd == "check-update" {
                if newer {
                    say(av.notes());
                }
                return Ok(());
            }
            if !newer && !a.allow_downgrade {
                return Ok(());
            }
            let env = env.ok_or("Half Sword was not found; pass --game")?;
            let dir = update::download_dir(&home);
            let zip = c.fetch_verified(&av.release, &dir, &mut |_, _| {})?;
            let keys = hsmp_launcher::trust::effective_keys(Some(&home))?;
            let r = update::apply(&env, &zip, &keys, a.allow_downgrade, update::default_opts(&env), &mut log)?;
            update::prune_downloads(&dir, &zip);
            say(format!("installed HSMP {}", r.version));
            for n in r.notes {
                say(format!("note: {n}"));
            }
            if let Some(w) = ops::ensure_firewall(&env, &mut log) {
                say(format!("WARNING: {w}"));
            }
            if update::restart_target(&home).is_some() {
                say(format!("the new launcher is {}", hsmp_launcher::trust::installed_launcher_path(&home).display()));
            }
            Ok(())
        }
        "backup-saves" => {
            let env = Env::system(&PathBuf::new())?;
            // under the operation lock (ops::backup_saves)
            match ops::backup_saves(&env.hsmp_home, &env.save_dir(), &env.save_backups(), "manual backup (command line)")? {
                Some(b) => say(format!("backed up {} file(s) to {}", b.files.len(), env.save_backups().join(&b.id).display())),
                None => say(format!("no saves found in {}", env.save_dir().display())),
            }
            Ok(())
        }
        "list-backups" => {
            let env = Env::system(&PathBuf::new())?;
            for (_, b) in saves::list(&env.save_backups()) {
                say(format!("{}  {}  {} file(s), {} KB{}  {}", b.id, b.created_utc, b.files.len(), b.total_bytes() / 1024, if b.has_career() { ", career" } else { "" }, b.reason));
            }
            Ok(())
        }
        "restore-saves" => {
            let id = a.pos.first().ok_or("restore-saves needs a backup id (see list-backups)")?;
            let env = Env::system(&PathBuf::new())?;
            let _lock = install::lock(&env.hsmp_home)?;
            if hsmp_launcher::procs::game_running() {
                return Err("close Half Sword before restoring saves".into());
            }
            let all = saves::list(&env.save_backups());
            let (dir, b) = all.iter().find(|(_, b)| &b.id == id).ok_or_else(|| format!("no backup with id {id}"))?;
            let safety = saves::restore(dir, b, &env.save_dir(), &env.save_backups())?;
            say(format!("restored {} file(s) from {id}", b.files.len()));
            if let Some(s) = safety {
                say(format!("your previous saves were kept as backup {}", s.id));
            }
            Ok(())
        }
        "firewall-status" => {
            let env = env_for(&a)?;
            let exe = firewall::server_exe(&env.game_root);
            say(format!("server:  {}{}", exe.display(), if exe.is_file() { "" } else { " (not installed)" }));
            say(firewall::status(&exe).describe());
            Ok(())
        }
        // the elevated helper (started through UAC by ops::ensure_firewall / remove_firewall)
        "firewall-allow" => {
            let exe = PathBuf::from(a.pos.first().ok_or("firewall-allow needs the path of hsmp-server.exe")?);
            if !exe.is_file() {
                return Err(format!("{} does not exist", exe.display()));
            }
            firewall::apply(&firewall::allow_commands(&exe)?, &mut log)?;
            say(format!("firewall: inbound UDP allowed for {}", exe.display()));
            Ok(())
        }
        "firewall-remove" => {
            let exe = a.pos.first().map(PathBuf::from);
            firewall::apply(&firewall::remove_commands(exe.as_deref())?, &mut log)?;
            say(format!("firewall: rule \"{}\" removed", firewall::RULE_NAME));
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n{USAGE}")),
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        #[cfg(feature = "gui")]
        {
            if let Err(e) = gui::run() {
                ops::log_line(&format!("gui error: {e}"));
                std::process::exit(1);
            }
            return;
        }
    }
    attach_console();
    let a = match parse(&argv) {
        Ok(a) if !a.cmd.is_empty() => a,
        Ok(_) => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    ops::log_line(&format!("cli: {}", argv.join(" ")));
    if let Err(e) = run_cli(a) {
        eprintln!("ERROR: {e}");
        ops::log_line(&format!("error: {e}"));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn launch_command_parses() {
        let a = parse(&argv("launch --game D:\\HS")).unwrap();
        assert_eq!(a.cmd, "launch");
        assert_eq!(a.game, Some(PathBuf::from("D:\\HS")));
        assert_eq!(parse(&argv("launch --help")).unwrap().cmd, "help");
        assert!(parse(&argv("launch --direct")).is_err(), "the direct route is gone");
        assert!(USAGE.contains("|launch|") && USAGE.contains("steam -applaunch"));
    }
}

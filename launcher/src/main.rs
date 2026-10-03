//! hsmp-launcher: double-click for the window; or use the command line.
//!
//!     hsmp-launcher                          open the launcher window
//!     hsmp-launcher status        [--game DIR] [--package DIR|ZIP]
//!     hsmp-launcher verify        [--package DIR|ZIP]
//!     hsmp-launcher install       [--game DIR] [--package DIR|ZIP] [--allow-unsupported] [--no-save-backup]
//!     hsmp-launcher uninstall     [--game DIR]
//!     hsmp-launcher launch        [--game DIR] [--direct | --steam]
//!     hsmp-launcher backup-saves
//!     hsmp-launcher list-backups
//!     hsmp-launcher restore-saves <backup id>
//!     hsmp-launcher find-game
//!
//! Exit codes: 0 ok, 1 failed, 2 bad usage.

#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
mod gui;

use hsmp_launcher::install::{self, Env};
use hsmp_launcher::launch::Via;
use hsmp_launcher::{ops, saves, steam};
use std::path::PathBuf;

const USAGE: &str = "hsmp-launcher [status|verify|install|uninstall|launch|backup-saves|list-backups|restore-saves <id>|find-game]
  --game DIR          Half Sword folder (default: remembered, else found through Steam)
  --package DIR|ZIP   release folder or zip (default: the launcher's own folder)
  --allow-unsupported install on a Half Sword build this release does not list
  --no-save-backup    skip the automatic career save backup before the first install
  --allow-downgrade   install a release older than the installed one
  --forget-missing    uninstall: give up on backed-up originals that are gone for good
                      (an antivirus deleted them); HSMP's version of those files is removed
  --direct / --steam  launch the exe directly / through Steam (default: direct when Steam runs, else Steam)
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
    direct: bool,
    steam: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args { cmd: String::new(), pos: vec![], game: None, package: None, allow_unsupported: false, allow_downgrade: false, forget_missing: false, no_save_backup: false, direct: false, steam: false };
    let mut it = argv.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--game" => a.game = Some(PathBuf::from(it.next().ok_or("--game needs a folder")?)),
            "--package" => a.package = Some(PathBuf::from(it.next().ok_or("--package needs a folder or zip")?)),
            "--allow-unsupported" => a.allow_unsupported = true,
            "--allow-downgrade" => a.allow_downgrade = true,
            "--forget-missing" => a.forget_missing = true,
            "--no-save-backup" => a.no_save_backup = true,
            "--direct" => a.direct = true,
            "--steam" => a.steam = true,
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
            ops::save_settings(&ops::Settings { game_root: Some(env.game_root.to_string_lossy().to_string()) });
            match &r.updated_from {
                Some(v) => say(format!("updated HSMP {v} -> {} ({} change(s), {} file(s) removed)", r.version, r.files_written, r.files_removed)),
                None => say(format!("installed HSMP {} ({} file(s) written)", r.version, r.files_written)),
            }
            for n in r.notes {
                say(format!("note: {n}"));
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
            Ok(())
        }
        "launch" => {
            let env = env_for(&a)?;
            let pid = ops::launch(&env, if a.direct { Via::Direct } else if a.steam { Via::Steam } else { Via::Auto }, &mut log)?;
            say(format!("started (pid {pid})"));
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

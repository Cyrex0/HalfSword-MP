//! The launcher window (egui/eframe). All slow work (hashing the 150 MB game
//! exe, verifying and installing) runs on a worker thread; the window only
//! shows state and messages.

use eframe::egui::{self, Color32, RichText};
use hsmp_launcher::crash::{self, Consent, ConsentFile, Crash};
use hsmp_launcher::game::{self, BuildCheck};
use hsmp_launcher::install::{self, Env, Status};
use hsmp_launcher::package::Package;
use hsmp_launcher::saves::{self, Backup};
use hsmp_launcher::steam::{self, FoundGame};
use hsmp_launcher::{ops, update, util, LAUNCHER_VERSION};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

const OK: Color32 = Color32::from_rgb(80, 200, 120);
const WARN: Color32 = Color32::from_rgb(230, 180, 60);
const BAD: Color32 = Color32::from_rgb(235, 90, 80);

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Info,
    Ok,
    Warn,
    Err,
}

enum Msg {
    Log(Level, String),
    /// (game root, build-check generation, result)
    Build(PathBuf, u64, Result<BuildCheck, String>),
    Done(Result<String, String>),
    UpdateCheck(Result<Option<update::Available>, String>),
    /// download progress (bytes so far, total)
    Progress(u64, u64),
    /// an update was installed from this zip
    Updated(PathBuf),
}

#[derive(Default)]
struct Updates {
    checking: bool,
    result: Option<Result<Option<update::Available>, String>>,
    progress: Option<(u64, u64)>,
    stable_only: bool,
    allow_downgrade: bool,
    /// the new launcher copy to restart into after a self-update
    restart: Option<PathBuf>,
}

#[derive(PartialEq, Clone, Copy)]
enum Confirm {
    None,
    Uninstall,
    Restore,
}

struct App {
    hsmp_home: PathBuf,
    ue_saved: PathBuf,
    pkg: Result<Arc<Package>, String>,
    found: Vec<FoundGame>,
    game_root: Option<PathBuf>,
    manual: String,
    build: Option<Result<BuildCheck, String>>,
    /// bumped by every new build check (another game folder, another release): a result of
    /// an older check is dropped
    build_gen: u64,
    allow_unsupported: bool,
    allow_downgrade: bool,
    forget_missing: bool,
    backup_saves: bool,
    status: Option<Result<Status, String>>,
    backups: Vec<(PathBuf, Backup)>,
    selected_backup: Option<String>,
    consent_path: PathBuf,
    consent: ConsentFile,
    crashes: Vec<Crash>,
    confirm: Confirm,
    busy: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    log: Vec<(Level, String)>,
    updates: Updates,
    frames: u64,
    shot_requested: bool,
}

pub fn run() -> Result<(), String> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 780.0]).with_min_inner_size([620.0, 520.0]).with_title(format!("Half Sword Multiplayer - launcher {LAUNCHER_VERSION}")),
        ..Default::default()
    };
    eframe::run_native("hsmp-launcher", opts, Box::new(|_cc| Ok(Box::new(App::new())))).map_err(|e| e.to_string())
}

impl App {
    fn new() -> App {
        let (tx, rx) = channel();
        let la = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
        let hsmp_home = la.join("HSMP");
        let consent_path = hsmp_home.join("consent.json");
        let consent = crash::load(&consent_path);
        let mut app = App {
            ue_saved: la.join("HalfSwordUE5").join("Saved"),
            hsmp_home,
            pkg: ops::open_package(None).map(Arc::new),
            found: vec![],
            game_root: None,
            manual: String::new(),
            build: None,
            build_gen: 0,
            allow_unsupported: false,
            allow_downgrade: false,
            forget_missing: false,
            backup_saves: true,
            status: None,
            backups: vec![],
            selected_backup: None,
            consent_path,
            consent,
            crashes: vec![],
            confirm: Confirm::None,
            busy: None,
            tx,
            rx,
            log: vec![],
            updates: Updates { stable_only: ops::load_settings().stable_only, ..Default::default() },
            frames: 0,
            shot_requested: false,
        };
        hsmp_launcher::trust::cleanup_old_copy(&app.hsmp_home);
        match &app.pkg {
            Ok(p) => app.push(Level::Ok, format!("release HSMP {} verified (signed by {})", p.manifest.version, p.signed_by)),
            Err(e) => app.push(Level::Err, format!("release package: {e}")),
        }
        app.found = steam::discover();
        let remembered = ops::load_settings().game_root.map(PathBuf::from).filter(|p| game::is_game_root(p));
        let pick = remembered.or_else(|| app.found.first().map(|f| f.root.clone()));
        match &pick {
            Some(p) => app.push(Level::Info, format!("Half Sword: {}", p.display())),
            None => app.push(Level::Warn, "Half Sword was not found through Steam: choose its folder below".into()),
        }
        if let Some(p) = pick {
            app.set_game(p);
        }
        app.refresh_saves();
        app.start_check();
        // the smoke test renders offline
        if std::env::var_os("HSMP_LAUNCHER_SMOKE_FRAMES").is_none() {
            app.check_updates();
        }
        app.crashes = crash::new_crashes(&[app.ue_saved.join("Crashes")], &app.consent);
        app
    }

    fn push(&mut self, l: Level, s: String) {
        ops::log_line(&s);
        self.log.push((l, s));
        if self.log.len() > 400 {
            self.log.drain(..100);
        }
    }

    fn env(&self) -> Option<Env> {
        self.game_root.as_ref().map(|r| Env::new(r, self.hsmp_home.clone(), self.ue_saved.clone()))
    }

    fn set_game(&mut self, root: PathBuf) {
        self.game_root = Some(root.clone());
        self.manual = root.display().to_string();
        self.build = None;
        self.allow_unsupported = false;
        ops::update_settings(|s| s.game_root = Some(root.to_string_lossy().to_string()));
        self.refresh_status();
        self.start_build_check();
    }

    /// (Re-)check the game build against the CURRENT package. Runs whenever the game folder
    /// or the package changes, so "Open another release..." never shows the previous
    /// release's verdict (or none at all when the launcher had no package of its own).
    fn start_build_check(&mut self) {
        self.build_gen += 1;
        self.build = None;
        self.allow_unsupported = false;
        let (Some(root), Ok(p)) = (self.game_root.clone(), &self.pkg) else { return };
        let p = p.clone();
        let tx = self.tx.clone();
        let gen = self.build_gen;
        std::thread::spawn(move || {
            let r = ops::check_build(&root, &p);
            let _ = tx.send(Msg::Build(root, gen, r));
        });
    }

    fn refresh_status(&mut self) {
        self.status = self.env().map(|e| install::status(&e));
    }

    fn refresh_saves(&mut self) {
        self.backups = saves::list(&self.hsmp_home.join("save_backups"));
    }

    /// Run `f` on a worker thread; log lines stream back to the window.
    fn work(&mut self, what: &str, f: impl FnOnce(&mut dyn FnMut(String)) -> Result<String, String> + Send + 'static) {
        self.busy = Some(what.to_string());
        self.push(Level::Info, format!("{what}..."));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut log = move |s: String| {
                let _ = tx2.send(Msg::Log(Level::Info, s));
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut log))).unwrap_or_else(|_| Err("internal error (panic); see launcher.log".into()));
            let _ = tx.send(Msg::Done(r));
        });
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(l, s) => self.push(l, s),
                Msg::Build(root, gen, r) => {
                    if build_result_is_current(gen, self.build_gen, &root, self.game_root.as_deref()) {
                        match &r {
                            Ok(b) if b.is_supported() => self.push(Level::Ok, "game build: supported".into()),
                            Ok(b) => self.push(Level::Warn, format!("game build: NOT in this release's supported list (exe sha256 {})", &b.sha256()[..16])),
                            Err(e) => self.push(Level::Err, e.clone()),
                        }
                        self.build = Some(r);
                    }
                }
                Msg::Done(r) => {
                    self.busy = None;
                    match r {
                        Ok(s) => self.push(Level::Ok, s),
                        Err(e) => {
                            for l in e.lines() {
                                self.push(Level::Err, l.to_string());
                            }
                        }
                    }
                    self.updates.progress = None;
                    self.refresh_status();
                    self.refresh_saves();
                }
                Msg::UpdateCheck(r) => {
                    self.updates.checking = false;
                    match &r {
                        Ok(Some(a)) if a.ordering() == std::cmp::Ordering::Greater => self.push(Level::Ok, format!("update available: HSMP {} -> {}", a.current, a.version)),
                        Ok(_) => {}
                        Err(e) => self.push(Level::Warn, format!("update check: {e}")),
                    }
                    self.updates.result = Some(r);
                }
                Msg::Progress(got, total) => self.updates.progress = Some((got, total)),
                Msg::Updated(zip) => {
                    self.pkg = ops::open_package(Some(&zip)).map(Arc::new);
                    self.start_build_check();
                    self.updates.result = None;
                    self.updates.restart = update::restart_target(&self.hsmp_home);
                }
            }
        }
    }

    /// Current version for the update check: the installed HSMP, else this launcher's.
    fn current_version(&self) -> String {
        match &self.status {
            Some(Ok(Status::Installed { version, .. })) => version.clone(),
            _ => LAUNCHER_VERSION.to_string(),
        }
    }

    fn check_updates(&mut self) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        let (tx, cur, stable, cache) = (self.tx.clone(), self.current_version(), self.updates.stable_only, update::cache_path(&self.hsmp_home));
        std::thread::spawn(move || {
            let r = update::Client::new().check(&cur, stable, Some(&cache));
            let _ = tx.send(Msg::UpdateCheck(r));
        });
    }

    fn start_update(&mut self, rel: update::Release) {
        let Some(env) = self.env() else { return };
        let (tx, home, allow) = (self.tx.clone(), self.hsmp_home.clone(), self.updates.allow_downgrade);
        self.updates.progress = Some((0, 0));
        self.work("Updating HSMP", move |log| {
            let dir = update::download_dir(&home);
            let mut shown = u64::MAX;
            let zip = update::Client::new().fetch_verified(&rel, &dir, &mut |got, total| {
                if got >> 20 != shown {
                    shown = got >> 20;
                    let _ = tx.send(Msg::Progress(got, total));
                }
            })?;
            log(format!("downloaded {} (SHA-256 matches the release)", zip.display()));
            let keys = hsmp_launcher::trust::effective_keys(Some(&home))?;
            let r = update::apply(&env, &zip, &keys, allow, update::default_opts(&env), log)?;
            for n in &r.notes {
                log(format!("note: {n}"));
            }
            update::prune_downloads(&dir, &zip);
            let _ = tx.send(Msg::Updated(zip));
            Ok(match r.updated_from {
                Some(v) if v != r.version => format!("updated HSMP {v} -> {}", r.version),
                _ => format!("HSMP {} is installed", r.version),
            })
        });
    }

    fn ui_updates(&mut self, ui: &mut egui::Ui) {
        ui.heading("Updates");
        let idle = self.busy.is_none();
        ui.horizontal(|ui| {
            if ui.add_enabled(idle && !self.updates.checking, egui::Button::new("Check for updates")).clicked() {
                self.check_updates();
            }
            if ui.checkbox(&mut self.updates.stable_only, "Stable releases only").changed() {
                let v = self.updates.stable_only;
                ops::update_settings(|s| s.stable_only = v);
                self.updates.result = None;
                self.check_updates();
            }
            if self.updates.checking {
                ui.spinner();
            }
        });
        if let Some((got, total)) = self.updates.progress {
            let frac = if total > 0 { got as f32 / total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(frac).text(format!("{:.1} / {:.1} MB", got as f64 / 1048576.0, total as f64 / 1048576.0)));
        }
        if let Some(p) = self.updates.restart.clone() {
            ui.colored_label(OK, "The update brought a new launcher.");
            if ui.button(RichText::new("Restart the launcher").strong()).clicked() {
                match update::relaunch(&p) {
                    Ok(()) => std::process::exit(0),
                    Err(e) => self.push(Level::Err, e),
                }
            }
        }
        let mut go: Option<update::Release> = None;
        match &self.updates.result {
            None => {}
            Some(Err(e)) => {
                ui.colored_label(WARN, e);
            }
            Some(Ok(None)) => {
                ui.label("No HSMP release is published for this channel yet.");
            }
            Some(Ok(Some(a))) => {
                use std::cmp::Ordering::*;
                match a.ordering() {
                    Greater => {
                        ui.colored_label(OK, RichText::new(format!("Update available: {} -> {}", a.current, a.version)).strong());
                    }
                    Equal => {
                        ui.label(format!("HSMP {} is the latest release.", a.version));
                    }
                    Less => {
                        ui.label(format!("HSMP {} is newer than the latest release ({}).", a.current, a.version));
                    }
                }
                let notes = a.notes();
                if a.ordering() == Greater && !notes.trim().is_empty() {
                    egui::CollapsingHeader::new("Release notes").default_open(true).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt("notes").max_height(140.0).show(ui, |ui| ui.label(notes));
                    });
                }
                let can = idle && self.game_root.is_some();
                if a.ordering() == Greater && ui.add_enabled(can, egui::Button::new(RichText::new(format!("Update to {}", a.version)).strong()).fill(Color32::from_rgb(40, 90, 60))).on_hover_text("Close Half Sword first. Downloads the release, checks its SHA-256 and signature, then installs it").clicked() {
                    go = Some(a.release.clone());
                }
                egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                    ui.checkbox(&mut self.updates.allow_downgrade, "Allow installing an older release (downgrade)");
                    if a.ordering() == Less && self.updates.allow_downgrade && ui.add_enabled(can, egui::Button::new(format!("Install {} (downgrade)", a.version))).clicked() {
                        go = Some(a.release.clone());
                    }
                });
            }
        }
        if let Some(r) = go {
            self.start_update(r);
        }
    }

    fn ui_release(&mut self, ui: &mut egui::Ui) {
        match &self.pkg {
            Ok(p) => {
                let m = &p.manifest;
                ui.label(RichText::new(format!("HSMP {}  -  protocol {}  -  commit {}", m.version, m.protocol_version, &m.git_commit[..m.git_commit.len().min(10)])).strong());
                ui.label(format!("Release verified: signed by {}. UE4SS {}.", p.signed_by, m.ue4ss.version));
            }
            Err(e) => {
                ui.colored_label(BAD, RichText::new("This launcher cannot use its release files.").strong());
                ui.colored_label(BAD, e);
                ui.label("Download the release zip again, extract ALL of it to a folder, and run hsmp-launcher.exe from that folder.");
            }
        }
        ui.horizontal(|ui| {
            // updates: open the new zip with the launcher you already trust
            if ui.add_enabled(self.busy.is_none(), egui::Button::new("Open another release...")).on_hover_text("Check and use a newer HSMP release zip with THIS launcher's keys (the safe way to update)").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Choose the new HSMP release zip").add_filter("HSMP release", &["zip"]).pick_file() {
                    self.pkg = ops::open_package(Some(&p)).map(Arc::new);
                    match &self.pkg {
                        Ok(p) => self.push(Level::Ok, format!("release HSMP {} verified (signed by {})", p.manifest.version, p.signed_by)),
                        Err(e) => self.push(Level::Err, format!("release package: {e}")),
                    }
                    self.refresh_status();
                    self.start_build_check();
                }
            }
            ui.label(RichText::new(format!("Launcher for updates: {}", hsmp_launcher::trust::installed_launcher_path(&self.hsmp_home).display())).small());
        });
    }

    fn ui_game(&mut self, ui: &mut egui::Ui) {
        ui.heading("1. Half Sword");
        if !self.found.is_empty() {
            let mut pick: Option<PathBuf> = None;
            for f in &self.found {
                let sel = self.game_root.as_deref() == Some(f.root.as_path());
                let label = format!("{}   (Steam build {}{})", f.root.display(), f.buildid.as_deref().unwrap_or("?"), if f.fully_installed { "" } else { ", Steam is updating it" });
                if ui.radio(sel, label).clicked() {
                    pick = Some(f.root.clone());
                }
            }
            if let Some(p) = pick {
                self.set_game(p);
            }
        }
        ui.horizontal(|ui| {
            ui.label("Folder:");
            ui.add(egui::TextEdit::singleline(&mut self.manual).desired_width(430.0));
            if ui.button("Browse...").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Choose the Half Sword folder (the one with HalfSwordUE5.exe)").pick_folder() {
                    self.manual = p.display().to_string();
                }
            }
            if ui.button("Use").clicked() {
                let p = PathBuf::from(self.manual.trim());
                match game::normalize_game_root(&p) {
                    Some(r) => self.set_game(r),
                    None => self.push(Level::Err, format!("{} is not a Half Sword folder ({} not found)", p.display(), game::exe_rel())),
                }
            }
        });
        match (&self.game_root, &self.build) {
            (None, _) => {
                ui.colored_label(WARN, "Choose the Half Sword folder.");
            }
            (Some(_), None) if self.pkg.is_err() => {
                ui.colored_label(WARN, "No release opened yet: the game build is checked once a release is open (\"Open another release...\").");
            }
            (Some(_), None) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the game version...");
                });
            }
            (Some(_), Some(Ok(b))) => match b {
                BuildCheck::Supported { label, .. } => {
                    ui.colored_label(OK, format!("Supported Half Sword build ({label})."));
                }
                BuildCheck::Unsupported { .. } => {
                    let v = self.pkg.as_ref().map(|p| p.manifest.version.clone()).unwrap_or_default();
                    ui.colored_label(BAD, format!("Half Sword was updated. HSMP {v} does not support this game build yet; please wait for an HSMP update."));
                    ui.checkbox(&mut self.allow_unsupported, "Try anyway (unsupported: menus or matches may break)");
                }
            },
            (Some(_), Some(Err(e))) => {
                ui.colored_label(BAD, e);
            }
        }
    }

    fn ui_install(&mut self, ui: &mut egui::Ui) {
        ui.heading("2. Multiplayer mod");
        let st = self.status.clone();
        match &st {
            Some(Ok(s)) => {
                let col = match s {
                    Status::Installed { modified, missing, .. } if modified.is_empty() && missing.is_empty() => OK,
                    Status::Installed { .. } | Status::Interrupted | Status::UninstallIncomplete { .. } => WARN,
                    Status::NotInstalled { .. } => Color32::GRAY,
                };
                ui.colored_label(col, ops::status_line(s));
            }
            Some(Err(e)) => {
                ui.colored_label(BAD, e);
            }
            None => {}
        }
        let idle = self.busy.is_none();
        let build_ok = matches!(&self.build, Some(Ok(b)) if b.is_supported()) || (matches!(&self.build, Some(Ok(_))) && self.allow_unsupported);
        let installed = matches!(&st, Some(Ok(Status::Installed { .. })));
        let half_uninstalled = matches!(&st, Some(Ok(Status::UninstallIncomplete { .. })));
        let pkg_ok = self.pkg.is_ok();
        let mut older = false;
        let (label, upd) = match (&st, &self.pkg) {
            (Some(Ok(Status::Installed { version, modified, missing, .. })), Ok(p)) => {
                older = util::cmp_version(&p.manifest.version, version) == std::cmp::Ordering::Less;
                if older {
                    (format!("Downgrade {version} -> {}", p.manifest.version), false)
                } else if *version != p.manifest.version {
                    (format!("Update {version} -> {}", p.manifest.version), true)
                } else if !modified.is_empty() || !missing.is_empty() {
                    ("Repair".to_string(), true)
                } else {
                    ("Reinstall".to_string(), false)
                }
            }
            _ => ("Install".to_string(), true),
        };
        if !installed && !half_uninstalled {
            ui.checkbox(&mut self.backup_saves, "Back up my career saves first (recommended)");
        }
        if older {
            ui.colored_label(WARN, "This release is OLDER than the installed HSMP.");
            ui.checkbox(&mut self.allow_downgrade, "Yes, downgrade to it");
        }
        ui.horizontal(|ui| {
            let b = egui::Button::new(RichText::new(&label).strong());
            let can = idle && build_ok && pkg_ok && self.game_root.is_some() && (!older || self.allow_downgrade);
            let b = if upd && can { b.fill(Color32::from_rgb(40, 90, 60)) } else { b };
            if ui.add_enabled(can, b).clicked() {
                if let (Some(env), Ok(p), Some(Ok(build))) = (self.env(), self.pkg.clone(), self.build.clone()) {
                    let (allow, backup, down) = (self.allow_unsupported, self.backup_saves, self.allow_downgrade);
                    self.work(&label, move |log| {
                        let r = ops::install_ex(&env, &p, &build, allow, backup, down, log)?;
                        let mut s = match r.updated_from {
                            Some(v) if v != r.version => format!("updated HSMP {v} -> {}", r.version),
                            _ => format!("HSMP {} is installed ({} change(s))", r.version, r.files_written),
                        };
                        if let Some(id) = r.save_backup {
                            s.push_str(&format!("; career saves backed up as {id}"));
                        }
                        for n in r.notes {
                            log(format!("note: {n}"));
                        }
                        Ok(s)
                    });
                }
            }
            if ui.add_enabled(idle && (installed || half_uninstalled) && self.confirm != Confirm::Uninstall, egui::Button::new("Uninstall...")).clicked() {
                self.confirm = Confirm::Uninstall;
            }
        });
        if !pkg_ok {
            ui.colored_label(BAD, "Install is disabled: the release files could not be verified (see above).");
        }
        if installed {
            ui.label(RichText::new("Start Half Sword from Steam as usual. The main menu's Multiplayer ribbon opens the server browser.").small());
        }
        if self.confirm == Confirm::Uninstall {
            ui.group(|ui| {
                ui.label("Uninstall puts back every file HSMP changed (UE4SS, mods.txt, Engine.ini) and removes everything it added. Your career saves are NOT touched. HSMP settings and logs are moved to %LOCALAPPDATA%\\HSMP\\uninstalled.");
                if half_uninstalled {
                    ui.label("The last uninstall stopped because some backed-up originals could not be read (often an antivirus quarantine). Restore them from your antivirus first if you can.");
                    ui.checkbox(&mut self.forget_missing, "Give up on backups that are gone for good (removes HSMP's version of those files; Steam's \"Verify integrity\" can restore game files)");
                }
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Yes, uninstall HSMP").strong()).fill(Color32::from_rgb(110, 40, 40))).clicked() {
                        self.confirm = Confirm::None;
                        let forget = half_uninstalled && self.forget_missing;
                        if let Some(env) = self.env() {
                            self.work("Uninstall", move |log| {
                                let r = ops::uninstall(&env, &install::UninstallOpts { check_processes: true, forget_missing: forget }, log)?;
                                for n in &r.notes {
                                    log(n.clone());
                                }
                                Ok(format!("HSMP removed: {} file(s) restored, {} removed. The game is back to how it was.", r.restored, r.removed))
                            });
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm = Confirm::None;
                    }
                });
            });
        }
    }

    /// Career save check of a crashed MP session (and the ini re-apply) at launcher start.
    fn start_check(&mut self) {
        if !matches!(&self.status, Some(Ok(Status::Installed { .. }))) {
            return;
        }
        if let Some(env) = self.env() {
            self.work("Checking career saves", move |log| ops::startup_check(&env, log).map(|_| "career saves checked".to_string()));
        }
    }

    fn ui_saves(&mut self, ui: &mut egui::Ui) {
        ui.heading("Career saves");
        let root = self.hsmp_home.join("save_backups");
        if self.backups.is_empty() {
            ui.label("No backups yet. One is made automatically before the first install.");
        } else {
            ui.label(format!("{} backup(s) in {} (never deleted by HSMP):", self.backups.len(), root.display()));
            egui::ScrollArea::vertical().id_salt("backups").max_height(110.0).show(ui, |ui| {
                for (_, b) in &self.backups {
                    let sel = self.selected_backup.as_deref() == Some(b.id.as_str());
                    let text = format!("{}  -  {} file(s), {} KB{}  -  {}", b.created_utc, b.files.len(), b.total_bytes() / 1024, if b.has_career() { ", career" } else { "" }, b.reason);
                    if ui.radio(sel, text).clicked() {
                        self.selected_backup = Some(b.id.clone());
                    }
                }
            });
        }
        let idle = self.busy.is_none();
        ui.horizontal(|ui| {
            if ui.add_enabled(idle, egui::Button::new("Back up now")).clicked() {
                let (saves_dir, root) = (self.ue_saved.join("SaveGames"), root.clone());
                let home = self.hsmp_home.clone();
                self.work("Backing up saves", move |_| {
                    // under the operation lock: never two writers in save_backups
                    match ops::backup_saves(&home, &saves_dir, &root, "manual backup")? {
                        Some(b) => Ok(format!("saves backed up: {} file(s) as {}", b.files.len(), b.id)),
                        None => Err(format!("no saves found in {}", saves_dir.display())),
                    }
                });
            }
            if ui.add_enabled(idle && self.selected_backup.is_some(), egui::Button::new("Restore selected...")).clicked() {
                self.confirm = Confirm::Restore;
            }
            if ui.button("Open folder").clicked() {
                let _ = std::fs::create_dir_all(&root);
                open_folder(&root);
            }
        });
        if self.confirm == Confirm::Restore {
            let id = self.selected_backup.clone().unwrap_or_default();
            ui.group(|ui| {
                ui.label(format!("Restore backup {id}? Close Half Sword first. Your current saves are backed up automatically before they are replaced. Steam Cloud may sync the restored files on the next game start."));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Yes, restore").strong()).fill(Color32::from_rgb(110, 80, 30))).clicked() {
                        self.confirm = Confirm::None;
                        let (saves_dir, root) = (self.ue_saved.join("SaveGames"), root.clone());
                        let home = self.hsmp_home.clone();
                        self.work("Restoring saves", move |log| {
                            let _lock = install::lock(&home)?;
                            if hsmp_launcher::procs::game_running() {
                                return Err("close Half Sword before restoring saves".into());
                            }
                            let all = saves::list(&root);
                            let (dir, b) = all.iter().find(|(_, b)| b.id == id).ok_or("backup not found")?;
                            if let Some(s) = saves::restore(dir, b, &saves_dir, &root)? {
                                log(format!("your previous saves were kept as backup {}", s.id));
                            }
                            Ok(format!("restored {} file(s) from backup {id}", b.files.len()))
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm = Confirm::None;
                    }
                });
            });
        }
    }

    fn ui_crash(&mut self, ui: &mut egui::Ui) {
        ui.heading("Crash reports");
        let before = self.consent.crash_reports;
        ui.horizontal(|ui| {
            ui.label("After a crash:");
            ui.radio_value(&mut self.consent.crash_reports, Consent::Ask, "ask me");
            ui.radio_value(&mut self.consent.crash_reports, Consent::Never, "never ask");
        });
        if before != self.consent.crash_reports {
            let _ = crash::save(&self.consent_path, &self.consent);
        }
        ui.label(RichText::new("Nothing is ever sent from this version: a report can only be saved to a folder for you to attach to a bug report.").small());
        if self.consent.crash_reports == Consent::Never || self.crashes.is_empty() {
            return;
        }
        ui.colored_label(WARN, format!("Half Sword crashed {} time(s) since the launcher last looked.", self.crashes.len()));
        ui.label("A report would contain:");
        for c in crash::describe_contents() {
            ui.label(format!("  - {c}"));
        }
        ui.horizontal(|ui| {
            if ui.button("Save a redacted report").clicked() {
                let out = self.hsmp_home.join("crash_reports");
                let log = self.game_root.as_ref().map(|r| game::win64(r).join("ue4ss").join("UE4SS.log"));
                let meta = serde_json::json!({
                    "hsmp": self.pkg.as_ref().map(|p| p.manifest.version.clone()).unwrap_or_default(),
                    "protocol": self.pkg.as_ref().map(|p| p.manifest.protocol_version).unwrap_or(0),
                    "game_exe_sha256": self.build.as_ref().and_then(|b| b.as_ref().ok()).map(|b| b.sha256().to_string()),
                    "saved_utc": util::iso_utc(util::now_unix()),
                });
                let mut saved = 0;
                for c in self.crashes.clone() {
                    match crash::save_local_report(&c, log.as_deref(), &meta, &out) {
                        Ok(p) => {
                            saved += 1;
                            self.push(Level::Ok, format!("crash report saved: {}", p.display()));
                        }
                        Err(e) => self.push(Level::Err, format!("crash report {}: {e}", c.name)),
                    }
                }
                if saved > 0 {
                    open_folder(&out);
                }
                self.dismiss_crashes();
            }
            if ui.button("Dismiss").clicked() {
                self.dismiss_crashes();
            }
        });
    }

    fn dismiss_crashes(&mut self) {
        for c in self.crashes.drain(..) {
            self.consent.seen.push(c.name);
        }
        let _ = crash::save(&self.consent_path, &self.consent);
    }
}

/// 24-bit bottom-up BMP (smoke-test screenshots; no image codec dependency).
fn bmp(img: &egui::ColorImage) -> Vec<u8> {
    let (w, h) = (img.size[0], img.size[1]);
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut v = Vec::with_capacity(size);
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&(size as u32).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(w as i32).to_le_bytes());
    v.extend_from_slice(&(h as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let start = v.len();
        for x in 0..w {
            let c = img.pixels[y * w + x];
            v.extend_from_slice(&[c.b(), c.g(), c.r()]);
        }
        v.resize(start + row, 0);
    }
    v
}

fn open_folder(p: &Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        // CI/dev smoke test: HSMP_LAUNCHER_SMOKE_FRAMES=n renders n frames, then closes;
        // with HSMP_LAUNCHER_SMOKE_SHOT=<file.bmp> it saves a screenshot first.
        if let Some(n) = std::env::var("HSMP_LAUNCHER_SMOKE_FRAMES").ok().and_then(|v| v.parse::<u64>().ok()) {
            self.frames += 1;
            let ready = self.busy.is_none() && (self.build.is_some() || self.pkg.is_err() || self.game_root.is_none());
            let cap = n.saturating_mul(50);
            let mut close = (self.frames >= n && ready) || self.frames >= cap;
            if let Ok(shot) = std::env::var("HSMP_LAUNCHER_SMOKE_SHOT") {
                if self.frames >= n && ready && !self.shot_requested {
                    self.shot_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                }
                let img = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                close = self.frames >= cap;
                if let Some(img) = img {
                    let _ = std::fs::write(&shot, bmp(&img));
                    close = true;
                }
            }
            if close {
                ops::log_line(&format!("gui smoke: {} frames rendered, build check {:?}, status {:?}", self.frames, self.build.as_ref().map(|b| b.as_ref().map(|x| x.is_supported())), self.status.as_ref().map(|s| s.as_ref().map(ops::status_line))));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        if self.busy.is_some() || self.build.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        egui::TopBottomPanel::bottom("log").resizable(true).default_height(150.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Messages").strong());
                if let Some(b) = &self.busy {
                    ui.spinner();
                    ui.label(format!("{b}..."));
                }
            });
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for (l, s) in &self.log {
                    let c = match l {
                        Level::Info => ui.visuals().text_color(),
                        Level::Ok => OK,
                        Level::Warn => WARN,
                        Level::Err => BAD,
                    };
                    ui.colored_label(c, s);
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(RichText::new("Half Sword Multiplayer").size(24.0).strong());
                self.ui_release(ui);
                ui.separator();
                self.ui_updates(ui);
                ui.separator();
                self.ui_game(ui);
                ui.separator();
                self.ui_install(ui);
                ui.separator();
                self.ui_saves(ui);
                ui.separator();
                self.ui_crash(ui);
            });
        });
    }
}

/// A build-check result is used only when it belongs to the latest check (the package and game
/// folder it was started for are still the current ones).
fn build_result_is_current(gen: u64, cur_gen: u64, root: &Path, cur_root: Option<&Path>) -> bool {
    gen == cur_gen && Some(root) == cur_root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_build_results_are_dropped() {
        let a = Path::new("C:/Games/Half Sword");
        let b = Path::new("D:/Other/Half Sword");
        assert!(build_result_is_current(3, 3, a, Some(a)));
        // a check started for the previous release (gen 2) finished after "Open another release"
        assert!(!build_result_is_current(2, 3, a, Some(a)));
        // a check for a game folder the player moved away from
        assert!(!build_result_is_current(3, 3, b, Some(a)));
        assert!(!build_result_is_current(3, 3, a, None));
    }
}

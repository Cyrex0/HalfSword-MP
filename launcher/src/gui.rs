//! The launcher window (egui/eframe). All slow work (hashing the 150 MB game
//! exe, verifying and installing) runs on a worker thread; the window only
//! shows state and messages.

use eframe::egui::{self, Color32, RichText};
use hsmp_launcher::crash::{self, Consent, ConsentFile, Crash};
use hsmp_launcher::report;
use hsmp_launcher::game::{self, BuildCheck};
use hsmp_launcher::install::{self, Env, Status};
use hsmp_launcher::package::Package;
use hsmp_launcher::saves::{self, Backup};
use hsmp_launcher::steam::{self, FoundGame};
use hsmp_launcher::{firewall, ops, update, util, LAUNCHER_VERSION};
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
    /// (game root, firewall rule state)
    Firewall(PathBuf, firewall::State),
    /// a bug report was prepared
    Report(Box<Result<Built, String>>),
    /// a bug report upload finished (the report id)
    Uploaded(Result<String, String>),
}

/// A prepared bug report: what the player previews is `rep`, what is saved or sent is `zip`.
struct Built {
    rep: report::Report,
    facts: report::Facts,
    zip: Vec<u8>,
}

struct BugUi {
    runs: Vec<hsmp_diag::sessions::Session>,
    selected: Vec<bool>,
    hide_ips: bool,
    include_dumps: bool,
    preparing: bool,
    built: Option<Built>,
    view: Option<usize>,
    saved: Option<PathBuf>,
    uploading: bool,
    confirm_upload: bool,
    upload_id: Option<String>,
}

impl Default for BugUi {
    fn default() -> Self {
        BugUi { runs: vec![], selected: vec![], hide_ips: true, include_dumps: true, preparing: false, built: None, view: None, saved: None, uploading: false, confirm_upload: false, upload_id: None }
    }
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
    /// the hsmp-server.exe firewall rule (read on a worker thread)
    firewall: Option<firewall::State>,
    backups: Vec<(PathBuf, Backup)>,
    selected_backup: Option<String>,
    consent_path: PathBuf,
    consent: ConsentFile,
    crashes: Vec<Crash>,
    bug: BugUi,
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
    let override_value = std::env::var("HSMP_LAUNCHER_RENDERER").ok();
    let backend = crate::render::Backend::from_override(override_value.as_deref())?;
    let explicit = override_value.as_deref().is_some_and(|v| !v.trim().is_empty());
    let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
    match run_backend(backend, created.clone()) {
        Ok(()) => Ok(()),
        Err(first) => {
            let Some(fallback) = backend.fallback(explicit, created.load(std::sync::atomic::Ordering::Relaxed)) else { return Err(first) };
            ops::log_line(&format!("gui startup failed: {first}; retrying {}", fallback.name()));
            // eframe's run-and-return path reuses its event loop for another window.
            run_backend(fallback, created).map_err(|second| format!("{first}\n{second}"))
        }
    }
}

fn run_backend(backend: crate::render::Backend, created: Arc<std::sync::atomic::AtomicBool>) -> Result<(), String> {
    ops::log_line(&format!("gui starting: renderer={}", backend.name()));
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::default();
    if backend == crate::render::Backend::Dx12 {
        // Explicitly avoid OpenGL and Vulkan driver initialization on Windows.
        // This affects the launcher window only, never Half Sword's renderer.
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    let opts = eframe::NativeOptions {
        run_and_return: true,
        renderer: match backend {
            crate::render::Backend::Glow => eframe::Renderer::Glow,
            _ => eframe::Renderer::Wgpu,
        },
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration { wgpu_setup: setup.into(), ..Default::default() },
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 780.0]).with_min_inner_size([620.0, 520.0]).with_title(format!("Half Sword Multiplayer - launcher {LAUNCHER_VERSION}")),
        ..Default::default()
    };
    eframe::run_native("hsmp-launcher", opts, Box::new(move |cc| {
        // Smoke-only fault injection exercises the real event-loop retry after
        // graphics initialization, before any app settings or game operations.
        if backend == crate::render::Backend::Glow && std::env::var_os("HSMP_LAUNCHER_SMOKE_FRAMES").is_some() && std::env::var_os("HSMP_LAUNCHER_SMOKE_FAIL_GLOW").is_some() {
            return Err("injected OpenGL startup failure (smoke test)".into());
        }
        if let Some(state) = &cc.wgpu_render_state {
            ops::log_line(&format!("gui adapter: {}", eframe::egui_wgpu::adapter_info_summary(&state.adapter.get_info())));
        } else if let Some(gl) = &cc.gl {
            use eframe::glow::HasContext;
            // SAFETY: eframe made this GL context current before calling the creator.
            let info = unsafe { format!("{} / {} / {}", gl.get_parameter_string(eframe::glow::VENDOR), gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
            ops::log_line(&format!("gui adapter: OpenGL {info}"));
        }
        created.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(Box::new(App::new()))
    })).map_err(|e| format!("{} startup failed: {e}", backend.name()))
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
            firewall: None,
            backups: vec![],
            selected_backup: None,
            consent_path,
            consent,
            crashes: vec![],
            bug: BugUi::default(),
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
        app.refresh_runs();
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
        self.refresh_firewall();
    }

    fn refresh_firewall(&mut self) {
        self.firewall = None;
        let Some(root) = self.game_root.clone() else { return };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let s = firewall::status(&firewall::server_exe(&root));
            let _ = tx.send(Msg::Firewall(root, s));
        });
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
                // the workers' log lines are Info; their warnings say so
                Msg::Log(l, s) => {
                    let l = if s.starts_with("WARNING:") { Level::Warn } else { l };
                    self.push(l, s)
                }
                Msg::Firewall(root, s) => {
                    if self.game_root.as_deref() == Some(root.as_path()) {
                        self.firewall = Some(s);
                    }
                }
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
                Msg::Report(r) => {
                    self.bug.preparing = false;
                    match *r {
                        Ok(b) => {
                            self.push(Level::Ok, format!("bug report ready: {} files, {} KB; check them below before you save or send it", b.rep.entries.len(), b.zip.len() / 1024));
                            self.bug.built = Some(b);
                        }
                        Err(e) => self.push(Level::Err, format!("bug report: {e}")),
                    }
                }
                Msg::Uploaded(r) => {
                    self.bug.uploading = false;
                    match r {
                        Ok(id) => {
                            self.push(Level::Ok, format!("bug report uploaded: report id {id}"));
                            self.bug.upload_id = Some(id);
                        }
                        Err(e) => self.push(Level::Err, format!("bug report upload: {e}")),
                    }
                }
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
            if let Some(w) = ops::ensure_firewall(&env, log) {
                log(format!("WARNING: {w}"));
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
                        if let Some(w) = ops::ensure_firewall(&env, log) {
                            log(format!("WARNING: {w}"));
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
        if installed && cfg!(windows) {
            self.ui_firewall(ui, idle);
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
                                if let Some(w) = ops::remove_firewall(&env, log) {
                                    log(format!("WARNING: {w}"));
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

    /// The hsmp-server.exe firewall rule, with "Fix firewall" when it is not right.
    fn ui_firewall(&mut self, ui: &mut egui::Ui, idle: bool) {
        let Some(s) = self.firewall.clone() else {
            ui.label(RichText::new("Firewall: checking...").small());
            return;
        };
        if s.is_allowed() {
            ui.colored_label(OK, s.describe());
            return;
        }
        ui.colored_label(WARN, s.describe());
        ui.horizontal(|ui| {
            let fix = ui
                .add_enabled(idle, egui::Button::new("Fix firewall"))
                .on_hover_text("Adds an inbound UDP allow rule for hsmp-server.exe (all networks). Windows asks for permission once.");
            if fix.clicked() {
                if let Some(env) = self.env() {
                    self.work("Fix firewall", move |log| match ops::ensure_firewall(&env, log) {
                        None => Ok("Windows Firewall now allows hsmp-server.exe".into()),
                        Some(w) => Err(w),
                    });
                }
            }
            ui.label(RichText::new("Needed to host games for players outside your network (they also need the UDP port forwarded on your router).").small());
        });
    }

    fn ui_play(&mut self, ui: &mut egui::Ui) {
        ui.heading("3. Launch");
        let ok = matches!(&self.status, Some(Ok(Status::Installed { missing, .. })) if missing.is_empty()) && self.busy.is_none();
        let b = egui::Button::new(RichText::new("  Launch through Steam  ").strong());
        let b = if ok { b.fill(Color32::from_rgb(40, 70, 110)) } else { b };
        if ui.add_enabled(ok, b).on_hover_text("steam -applaunch with the HSMP launch options (starts Steam if it is not running)").clicked() {
            if let Some(env) = self.env() {
                self.work("Launch", move |log| ops::launch(&env, log).map(|pid| format!("Half Sword is starting through Steam (pid {pid}). Have fun.")));
            }
        }
        ui.label(RichText::new("Recommended: it checks your career saves first and passes the hair-streaming crash workaround. Steam may ask once to allow these launch options. Starting Half Sword from Steam directly also works. In the game, the main menu's Multiplayer ribbon opens the server browser.").small());
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

    fn refresh_runs(&mut self) {
        let runs = hsmp_diag::sessions::list(&ops::logs_root(&self.hsmp_home));
        let crashed_new = !self.crashes.is_empty();
        let keep: Vec<String> = self.bug.runs.iter().zip(&self.bug.selected).filter(|(_, s)| **s).map(|(r, _)| r.info.id.clone()).collect();
        self.bug.selected = runs.iter().map(|r| keep.contains(&r.info.id)).collect();
        if keep.is_empty() {
            // default: the newest crashed run (when the game crashed since the launcher last looked), else the newest
            let pick = if crashed_new { runs.iter().position(|r| r.info.outcome == hsmp_diag::sessions::Outcome::Crashed) } else { None }.or(if runs.is_empty() { None } else { Some(0) });
            if let Some(i) = pick {
                self.bug.selected[i] = true;
            }
        }
        self.bug.runs = runs;
    }

    fn start_report(&mut self) {
        let chosen: Vec<_> = self.bug.runs.iter().zip(&self.bug.selected).filter(|(_, s)| **s).map(|(r, _)| r.clone()).collect();
        let (tx, env, pkg) = (self.tx.clone(), self.env(), self.pkg.as_ref().ok().cloned());
        let opts = report::Options { hide_ips: self.bug.hide_ips, include_dumps: self.bug.include_dumps };
        let log = self.hsmp_home.join("launcher").join("launcher.log");
        let saved_dir = self.ue_saved.clone();
        self.bug.preparing = true;
        self.bug.built = None;
        self.bug.view = None;
        self.bug.saved = None;
        self.bug.upload_id = None;
        std::thread::spawn(move || {
            let facts = report::gather(env.as_ref(), pkg.as_deref(), true);
            let mut rep = report::build(&chosen, Some(&log), Some(&saved_dir), &facts, &opts);
            let r = rep.zip_capped(hsmp_master_core::reports::MAX_REPORT_BYTES).map(|zip| Built { rep, facts, zip });
            let _ = tx.send(Msg::Report(Box::new(r)));
        });
    }

    fn save_report(&mut self) -> Option<PathBuf> {
        if let Some(p) = &self.bug.saved {
            return Some(p.clone());
        }
        let b = self.bug.built.as_ref()?;
        let out = report::default_out(&self.hsmp_home);
        match report::save(&b.zip, &out) {
            Ok(()) => {
                self.push(Level::Ok, format!("bug report saved: {}", out.display()));
                self.bug.saved = Some(out.clone());
                Some(out)
            }
            Err(e) => {
                self.push(Level::Err, format!("bug report not saved: {e}"));
                None
            }
        }
    }

    fn ui_report(&mut self, ui: &mut egui::Ui) {
        ui.heading("Bug report");
        let before = self.consent.crash_reports;
        ui.horizontal(|ui| {
            ui.label("After a crash:");
            ui.radio_value(&mut self.consent.crash_reports, Consent::Ask, "tell me");
            ui.radio_value(&mut self.consent.crash_reports, Consent::Never, "don't");
        });
        if before != self.consent.crash_reports {
            let _ = crash::save(&self.consent_path, &self.consent);
        }
        if self.consent.crash_reports != Consent::Never && !self.crashes.is_empty() {
            ui.horizontal(|ui| {
                ui.colored_label(WARN, format!("Half Sword crashed {} time(s) since the launcher last looked. Create a bug report below to send us the logs.", self.crashes.len()));
                if ui.button("Dismiss").clicked() {
                    self.dismiss_crashes();
                }
            });
        }
        ui.label(RichText::new(format!("Your last game runs (newest first), kept in {}:", ops::logs_root(&self.hsmp_home).display())).small());
        if self.bug.runs.is_empty() {
            ui.label("No game runs recorded yet. They are recorded from the next time you start Half Sword with HSMP.");
        }
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt("runs").max_height(120.0).show(ui, |ui| {
            for (i, r) in self.bug.runs.iter().enumerate() {
                let o = r.info.outcome;
                let col = match o {
                    hsmp_diag::sessions::Outcome::Crashed => BAD,
                    hsmp_diag::sessions::Outcome::Clean | hsmp_diag::sessions::Outcome::Running => ui.visuals().text_color(),
                    _ => WARN,
                };
                let when = hsmp_diag::time::iso(r.info.started_ms).replace('T', " ").replace('Z', " UTC");
                let what = match o {
                    hsmp_diag::sessions::Outcome::Running => "running now".to_string(),
                    hsmp_diag::sessions::Outcome::Unfinished => "unfinished (logs may be missing)".to_string(),
                    other => other.as_str().to_string(),
                };
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut self.bug.selected[i], "").changed();
                    ui.colored_label(col, format!("{when}  -  {what}{}", r.info.crash_dirs.first().map(|c| format!("  ({c})")).unwrap_or_default()));
                });
            }
        });
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut self.bug.hide_ips, "Hide other players' IP addresses").on_hover_text("Your own public address is always removed.").changed();
            changed |= ui.checkbox(&mut self.bug.include_dumps, "Include crash dumps").on_hover_text("A crash dump is a snapshot of the game's memory stack when it crashed. It helps us find the cause. It holds no screenshots.").changed();
        });
        if changed {
            self.bug.built = None;
        }
        ui.horizontal(|ui| {
            let idle = !self.bug.preparing && !self.bug.uploading;
            if ui.add_enabled(idle, egui::Button::new(RichText::new("Create bug report").strong())).on_hover_text("Collects the chosen runs' logs, removes personal data and shows you every file first. Nothing is sent yet.").clicked() {
                self.refresh_runs();
                self.start_report();
            }
            if ui.button("Refresh").clicked() {
                self.refresh_runs();
            }
            if self.bug.preparing {
                ui.spinner();
                ui.label("collecting and removing personal data...");
            }
        });
        let Some(b) = &self.bug.built else { return };
        ui.label(format!(
            "The report is {} KB and contains exactly these {} files (user names, home folders, e-mail addresses, keys, passwords{} removed). Click a file to see it:",
            b.zip.len() / 1024,
            b.rep.entries.len(),
            if self.bug.hide_ips { ", IP addresses" } else { ", your own IP address" }
        ));
        let mut view = self.bug.view;
        egui::ScrollArea::vertical().id_salt("report_files").max_height(120.0).show(ui, |ui| {
            for (i, e) in b.rep.entries.iter().enumerate() {
                let label = format!("{}  ({} bytes{})", e.name, e.bytes.len(), if e.redacted { "" } else { ", binary, unchanged" });
                if ui.selectable_label(view == Some(i), label).clicked() {
                    view = Some(i);
                }
            }
            for l in &b.rep.left_out {
                ui.label(RichText::new(format!("left out: {l}")).small());
            }
        });
        self.bug.view = view;
        if let Some(e) = view.and_then(|i| b.rep.entries.get(i)) {
            let shown = if e.redacted {
                let t = String::from_utf8_lossy(&e.bytes);
                let cut = t.char_indices().nth(200_000).map(|(i, _)| i).unwrap_or(t.len());
                format!("{}{}", &t[..cut], if cut < t.len() { "\n... (the rest is in the zip)" } else { "" })
            } else {
                format!("{} is a binary crash dump ({} bytes). It goes into the zip unchanged.", e.name, e.bytes.len())
            };
            egui::ScrollArea::both().id_salt("report_view").max_height(200.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(shown).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
            });
        }
        let mut save = false;
        let mut issue = false;
        let mut upload = false;
        ui.horizontal(|ui| {
            save = ui.button("Save report zip").on_hover_text("Saves it to %LOCALAPPDATA%\\HSMP\\bug_reports and opens the folder").clicked();
            issue = ui.button("Open GitHub issue").on_hover_text("Saves the zip and opens a pre-filled bug report on GitHub: attach the zip there (drag it into the page)").clicked();
            if b.facts.report_upload {
                let can = !self.bug.uploading && self.bug.upload_id.is_none();
                upload = ui.add_enabled(can, egui::Button::new("Upload to HSMP...")).on_hover_text("Sends the zip to the HSMP developers (kept about 60 days, never public)").clicked();
                if self.bug.uploading {
                    ui.spinner();
                }
            }
        });
        if upload {
            self.bug.confirm_upload = true;
        }
        if self.bug.confirm_upload {
            ui.group(|ui| {
                ui.label("Send this report to the HSMP developers? It goes to the HSMP server list service (Cloudflare), is readable only by the developers, and is deleted after about 60 days. You get a report id to quote.");
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Yes, upload").strong()).fill(Color32::from_rgb(40, 70, 110))).clicked() {
                        self.bug.confirm_upload = false;
                        self.bug.uploading = true;
                        let (tx, master, zip) = (self.tx.clone(), b.facts.master.clone(), b.zip.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(Msg::Uploaded(report::upload(&master, &zip)));
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        self.bug.confirm_upload = false;
                    }
                });
            });
        }
        if let Some(id) = self.bug.upload_id.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(OK, format!("Uploaded. Report id: {id}"));
                if ui.button("Copy id").clicked() {
                    ui.ctx().copy_text(id.clone());
                }
            });
        }
        if save {
            if let Some(p) = self.save_report() {
                if let Some(d) = p.parent() {
                    open_folder(d);
                }
            }
        }
        if issue {
            if let Some(p) = self.save_report() {
                let b = self.bug.built.as_ref().expect("built");
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let outcome = b.rep.sessions.first().and_then(|id| self.bug.runs.iter().find(|r| &r.info.id == id)).map(|r| r.info.outcome.as_str());
                let url = report::github_issue_url(&b.facts, &name, self.bug.upload_id.as_deref(), outcome);
                if let Some(d) = p.parent() {
                    open_folder(d);
                }
                if let Err(e) = report::open_url(&url) {
                    self.push(Level::Err, e);
                }
            }
        }
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
                self.ui_play(ui);
                ui.separator();
                self.ui_saves(ui);
                ui.separator();
                self.ui_report(ui);
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

//! Journald install / update / uninstall.
//!
//! Every change the launcher makes is recorded before it is made:
//!
//! * `State.files[key]` remembers each game file's ORIGINAL bytes (a
//!   content-addressed blob in the launcher store, or "absent") the first
//!   time HSMP writes it. Uninstall puts those bytes back (under the
//!   original file name, case included) or deletes the file. Keys are the
//!   case-folded relative path, because Windows file names are
//!   case-insensitive: `Menu_ui.lua` and `menu_ui.lua` are ONE file and one
//!   record, whose `original` is carried forward across releases.
//! * `State.dirs_created` lists folders HSMP created; uninstall removes them
//!   (moving any runtime leftovers into an archive folder, never deleting
//!   unknown data).
//! * `State.ini_edits` holds the exact Engine.ini edits (see `ini.rs`).
//! * A running install/update is a transaction (`Tx`) persisted in
//!   `State.pending` before each write. Any failure (or a crash, found on the
//!   next start) rolls every step back to the previous state.
//!
//! Store layout: `%LOCALAPPDATA%\HSMP\launcher\<install id>\state.json` +
//! `blobs\<sha256>.hsmpblob` (masked, so an antivirus does not see the
//! backed-up UE4SS proxy as a PE file). The install id is written into the
//! game folder (`Win64\hsmp_install.json`, with the Steam app id and the
//! game exe hash), so the journal is found again after Steam moves the
//! library, through a junction, a subst drive or an 8.3 path. A journal is
//! only used for a folder whose files match it (verified before acting).
//!
//! Career saves are never touched here (see `saves.rs`), except for the
//! automatic backup before the first install (read-only on the saves).

use crate::cfgfile;
use crate::game;
use crate::ini::{self, IniEdit, Revert};
use crate::manifest::Manifest;
use crate::modstxt;
use crate::package::Files;
use crate::saves;
use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const STATE_SCHEMA: u32 = 2;
pub const MODS_TXT_REL: &str = "HalfswordUE5/Binaries/Win64/ue4ss/Mods/mods.txt";
pub const CFG_REL: &str = "HalfswordUE5/Binaries/Win64/hsmp.cfg";
pub const MODS_DIR_REL: &str = "HalfswordUE5/Binaries/Win64/ue4ss/Mods";
/// Install-identity marker in the game folder (journaled like any HSMP file).
pub const MARKER_REL: &str = "HalfswordUE5/Binaries/Win64/hsmp_install.json";
const BLOB_EXT: &str = "hsmpblob";

/// Where things live. Production: `Env::system(root)`; tests: `Env::new` with temp dirs.
#[derive(Debug, Clone)]
pub struct Env {
    pub game_root: PathBuf,
    /// %LOCALAPPDATA%\HSMP
    pub hsmp_home: PathBuf,
    /// %LOCALAPPDATA%\HalfSwordUE5\Saved
    pub ue_saved: PathBuf,
    /// The resolved store key (see `resolve_key`), computed once per Env.
    key: OnceLock<String>,
}

/// `Win64\hsmp_install.json`: which journal belongs to this game folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub product: String,
    pub install_id: String,
    pub steam_appid: u32,
    pub exe_sha256: String,
}

/// Stable identity of the game install HSMP was put into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub steam_appid: u32,
    /// Shipping exe hash at the last install/update (changes when Steam patches the game).
    pub exe_sha256: String,
}

fn is_key(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Store key used by older launchers: hash of the literal path.
fn legacy_key(root: &Path) -> String {
    let norm = root.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    util::sha256_hex(norm.as_bytes())[..16].to_string()
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Store key of a fresh install: hash of the canonical (junctions, subst and
/// 8.3 names resolved), case-folded path.
fn canonical_key(root: &Path) -> String {
    util::sha256_hex(util::fold_path(&canonical(root)).as_bytes())[..16].to_string()
}

fn same_root(a: &Path, b: &Path) -> bool {
    util::fold_path(&canonical(a)) == util::fold_path(&canonical(b))
}

pub fn read_marker(root: &Path) -> Option<Marker> {
    let b = std::fs::read(util::join_rel(root, MARKER_REL)).ok()?;
    serde_json::from_slice::<Marker>(&b).ok().filter(|m| m.product == crate::manifest::PRODUCT && is_key(&m.install_id))
}

fn read_state_file(p: &Path) -> Result<Option<State>, String> {
    match util::read_opt(p).map_err(|e| format!("cannot read install state: {e}"))? {
        None => Ok(None),
        Some(b) => serde_json::from_slice::<State>(&b).map(Some).map_err(|e| format!("install state {} is damaged: {e}", p.display())),
    }
}

impl Env {
    pub fn new(game_root: &Path, hsmp_home: PathBuf, ue_saved: PathBuf) -> Env {
        Env { game_root: game_root.to_path_buf(), hsmp_home, ue_saved, key: OnceLock::new() }
    }
    pub fn system(game_root: &Path) -> Result<Env, String> {
        let la = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).filter(|p| p.is_dir()).ok_or("LOCALAPPDATA is not set")?;
        Ok(Env::new(game_root, la.join("HSMP"), la.join("HalfSwordUE5").join("Saved")))
    }
    pub fn ini_path(&self, file: &str) -> PathBuf {
        self.ue_saved.join("Config").join("Windows").join(format!("{file}.ini"))
    }
    pub fn save_dir(&self) -> PathBuf {
        self.ue_saved.join("SaveGames")
    }
    pub fn crash_dirs(&self) -> Vec<PathBuf> {
        vec![self.ue_saved.join("Crashes"), util::join_rel(&self.game_root, "HalfswordUE5/Saved/Crashes")]
    }
    pub fn save_backups(&self) -> PathBuf {
        self.hsmp_home.join("save_backups")
    }
    pub fn launcher_dir(&self) -> PathBuf {
        self.hsmp_home.join("launcher")
    }
    /// This install's store folder (journal + backed-up originals).
    pub fn store(&self) -> PathBuf {
        self.launcher_dir().join(self.store_key())
    }
    pub fn store_key(&self) -> String {
        self.key.get_or_init(|| self.resolve_key()).clone()
    }
    fn has_state(&self, key: &str) -> bool {
        self.launcher_dir().join(key).join("state.json").is_file()
    }
    /// Which journal belongs to this game folder:
    /// 1. the install id in `Win64\hsmp_install.json` (survives a move), unless
    ///    that journal's folder still exists with the same marker (a COPY of
    ///    the game: the journal stays with the original);
    /// 2. the canonical-path key, then the legacy literal-path key;
    /// 3. a journal whose game folder no longer exists and whose files are
    ///    all here (a move done before markers existed);
    /// 4. else a new canonical-path key.
    fn resolve_key(&self) -> String {
        let ck = canonical_key(&self.game_root);
        if let Some(m) = read_marker(&self.game_root) {
            if self.has_state(&m.install_id) && !self.belongs_elsewhere(&m.install_id) {
                return m.install_id;
            }
        }
        if self.has_state(&ck) {
            return ck;
        }
        let lk = legacy_key(&self.game_root);
        if self.has_state(&lk) {
            return lk;
        }
        self.find_moved().unwrap_or(ck)
    }
    fn belongs_elsewhere(&self, key: &str) -> bool {
        let Ok(Some(st)) = read_state_file(&self.launcher_dir().join(key).join("state.json")) else { return false };
        let old = PathBuf::from(&st.game_root);
        !st.game_root.is_empty() && !same_root(&old, &self.game_root) && game::is_game_root(&old) && read_marker(&old).is_some_and(|m| m.install_id == key)
    }
    fn find_moved(&self) -> Option<String> {
        let mut hits = vec![];
        for e in std::fs::read_dir(self.launcher_dir()).ok()?.flatten() {
            let key = e.file_name().to_string_lossy().to_string();
            if !is_key(&key) {
                continue;
            }
            let Ok(Some(mut st)) = read_state_file(&e.path().join("state.json")) else { continue };
            let old = PathBuf::from(&st.game_root);
            if st.game_root.is_empty() || game::is_game_root(&old) {
                continue; // that install is still where it was
            }
            migrate(&mut st);
            if verify_files(&st, &self.game_root).is_ok() {
                hits.push(key);
            }
        }
        if hits.len() == 1 {
            hits.pop()
        } else {
            None
        }
    }
    fn state_path(&self) -> PathBuf {
        self.store().join("state.json")
    }
    fn blob_path(&self, sha: &str) -> PathBuf {
        self.store().join("blobs").join(format!("{sha}.{BLOB_EXT}"))
    }
    fn legacy_blob_path(&self, sha: &str) -> PathBuf {
        self.store().join("blobs").join(sha)
    }
    pub fn abs(&self, rel: &str) -> PathBuf {
        util::join_rel(&self.game_root, rel)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRec {
    /// Game-root-relative path, exactly as HSMP last wrote it (its case).
    #[serde(default)]
    pub path: String,
    /// Blob sha of the bytes before HSMP first wrote this file; None = it did not exist.
    pub original: Option<String>,
    /// The original file's own relative path (its on-disk case), when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    /// sha256 of what HSMP wrote last.
    pub installed: String,
    /// "copy" (package file), "merged" (mods.txt, hsmp.cfg) or "marker".
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IniEditRec {
    pub file: String,
    pub section: String,
    pub key: String,
    pub edit: IniEdit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TxStep {
    pub target: String,
    /// Blob sha of the bytes before this step; None = file did not exist.
    pub prev: Option<String>,
    /// On-disk file name (exact case) before this step, when the file existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Tx {
    pub started_utc: String,
    pub steps: Vec<TxStep>,
    /// Absolute paths of folders created by this transaction (parents first).
    pub created_dirs: Vec<String>,
    /// The committed state before this transaction (None = was not installed).
    pub prev_state: Option<Box<State>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct State {
    pub schema: u32,
    pub game_root: String,
    pub version: String,
    pub channel: String,
    pub git_commit: String,
    pub protocol_version: u32,
    pub installed_utc: String,
    /// Keyed by `util::fold(rel)` (case-insensitive, like Windows).
    pub files: BTreeMap<String, FileRec>,
    /// Game-root-relative folders HSMP created (parents first).
    pub dirs_created: Vec<String>,
    /// Absolute folders created outside the game (e.g. Saved\Config\Windows).
    pub ext_dirs_created: Vec<String>,
    pub ini_edits: Vec<IniEditRec>,
    /// Runtime artefact names in Win64 that existed before the first install.
    pub runtime_preexisting: Vec<String>,
    pub master_url_written: Option<String>,
    pub save_backup_id: Option<String>,
    pub launch_args: Vec<String>,
    pub ini_settings: Vec<crate::manifest::IniSetting>,
    /// The store key (= the id in the game folder's marker).
    #[serde(default)]
    pub install_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// Folders this install was found in before (Steam library moves).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_roots: Vec<String>,
    /// An uninstall started and could not finish yet (see `uninstall`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uninstalling: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Tx>,
}

/// Files/folders the game + HSMP create at runtime in Win64 (archived on uninstall).
fn is_runtime_artefact(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.starts_with("hsmp_state") || l == "imgui.ini"
}

fn runtime_names(win64: &Path) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> =
        std::fs::read_dir(win64).map(|rd| rd.flatten().map(|e| e.file_name()).filter(|n| is_runtime_artefact(&n.to_string_lossy())).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Re-key a journal written before case-insensitive keys (schema 1). Two
/// records that differ only in case are ONE file: the true original is the
/// one whose `original` is not HSMP's own bytes; the path and installed hash
/// come from the later record (the one that saw HSMP's bytes as "original").
fn migrate(st: &mut State) {
    let needs = st.files.iter().any(|(k, r)| r.path.is_empty() || *k != util::fold(&r.path));
    if !needs {
        return;
    }
    let old = std::mem::take(&mut st.files);
    let ours: std::collections::HashSet<String> = old.values().map(|r| r.installed.clone()).collect();
    let is_ours = |r: &FileRec| r.original.as_ref().is_some_and(|o| ours.contains(o));
    for (k, mut r) in old {
        if r.path.is_empty() {
            r.path = k.clone();
        }
        let key = util::fold(&r.path);
        match st.files.remove(&key) {
            None => {
                st.files.insert(key, r);
            }
            Some(e) => {
                let (first, later) = if is_ours(&r) && !is_ours(&e) { (e, r) } else if is_ours(&e) && !is_ours(&r) { (r, e) } else { (e, r) };
                let merged = FileRec {
                    path: later.path,
                    original_path: first.original_path.or_else(|| first.original.as_ref().map(|_| first.path.clone())),
                    original: first.original,
                    installed: later.installed,
                    kind: later.kind,
                };
                st.files.insert(key, merged);
            }
        }
    }
}

/// Is `root` the game folder this journal describes? At least half of the
/// files HSMP copied (and at least one) must be there with HSMP's exact bytes.
pub fn verify_files(st: &State, root: &Path) -> Result<(), String> {
    let copies: Vec<&FileRec> = st.files.values().filter(|r| r.kind == "copy").collect();
    if copies.is_empty() {
        return Err("the record lists no HSMP files to compare".into());
    }
    let matched = copies.iter().filter(|r| util::sha256_file(&util::join_rel(root, &r.path)).map(|(h, _)| h == r.installed).unwrap_or(false)).count();
    if matched == 0 || matched * 2 < copies.len() {
        return Err(format!("only {matched} of {} HSMP files match", copies.len()));
    }
    if let (Some(m), Some(id)) = (read_marker(root), Some(&st.install_id).filter(|s| !s.is_empty())) {
        if &m.install_id != id {
            return Err(format!("the folder's install id {} is not {id}", m.install_id));
        }
    }
    Ok(())
}

fn rebase(s: &str, old: &Path, new: &Path) -> String {
    let p = Path::new(s);
    match p.strip_prefix(old) {
        Ok(rest) => new.join(rest).to_string_lossy().to_string(),
        Err(_) => s.to_string(),
    }
}

/// The journal was made for another path of the same install (a moved Steam
/// library): point it at this folder, including any pending transaction.
fn adopt(st: &mut State, root: &Path) {
    let old = PathBuf::from(&st.game_root);
    let new_root = root.to_string_lossy().to_string();
    if let Some(tx) = st.pending.as_mut() {
        for s in &mut tx.steps {
            s.target = rebase(&s.target, &old, root);
        }
        for d in &mut tx.created_dirs {
            *d = rebase(d, &old, root);
        }
    }
    if !st.previous_roots.contains(&st.game_root) {
        st.previous_roots.push(st.game_root.clone());
    }
    st.game_root = new_root;
}

pub fn load_state(env: &Env) -> Result<Option<State>, String> {
    let Some(mut st) = read_state_file(&env.state_path())? else { return Ok(None) };
    migrate(&mut st);
    if st.install_id.is_empty() {
        st.install_id = env.store_key();
    }
    if !st.game_root.is_empty() && !same_root(Path::new(&st.game_root), &env.game_root) {
        // verify before acting: never apply a journal to a folder it does not describe
        verify_files(&st, &env.game_root).map_err(|e| {
            format!(
                "HSMP's install record {} was made for {}, and {} does not match it ({e}). Nothing was changed. If you moved Half Sword, check that the move finished; otherwise install HSMP again.",
                st.install_id,
                st.game_root,
                env.game_root.display()
            )
        })?;
        adopt(&mut st, &env.game_root);
        save_state(env, &st)?;
    }
    Ok(Some(st))
}

fn save_state(env: &Env, st: &State) -> Result<(), String> {
    let p = env.state_path();
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("cannot create {}: {e}", env.store().display()))?;
    let b = serde_json::to_vec_pretty(st).map_err(|e| e.to_string())?;
    util::atomic_write(&p, &b).map_err(|e| format!("cannot write install state: {e}"))
}

/// Blobs are stored XOR-masked: a backed-up `dwmapi.dll` is then not a PE
/// file an antivirus would quarantine.
fn mask(bytes: &[u8]) -> Vec<u8> {
    let k = sha2::Sha256::digest(b"HSMP blob mask v1");
    bytes.iter().enumerate().map(|(i, b)| b ^ k[i % 32]).collect()
}
use sha2::Digest;

fn put_blob(env: &Env, bytes: &[u8]) -> Result<String, String> {
    let sha = util::sha256_hex(bytes);
    let p = env.blob_path(&sha);
    let ok = std::fs::read(&p).map(|b| util::sha256_hex(&mask(&b)) == sha).unwrap_or(false);
    if !ok {
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("blob store: {e}"))?;
        util::atomic_write(&p, &mask(bytes)).map_err(|e| format!("blob store: {e}"))?;
    }
    Ok(sha)
}

/// A backed-up original (masked blob, or an unmasked one from older launchers).
/// Transient locks (antivirus scans) are retried.
fn get_blob(env: &Env, sha: &str) -> Result<Vec<u8>, String> {
    let masked = env.blob_path(sha);
    let b = match util::retry_io(|| std::fs::read(&masked)) {
        Ok(b) => mask(&b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let legacy = env.legacy_blob_path(sha);
            util::retry_io(|| std::fs::read(&legacy)).map_err(|e| format!("the backup copy {} is unreadable: {}", masked.display(), util::explain_io(&e)))?
        }
        Err(e) => return Err(format!("the backup copy {} is unreadable: {}", masked.display(), util::explain_io(&e))),
    };
    if util::sha256_hex(&b) != sha {
        return Err(format!("the backup copy {} is damaged", masked.display()));
    }
    Ok(b)
}

/// Delete blobs that no committed record needs.
fn gc_blobs(env: &Env, st: Option<&State>) {
    let keep: std::collections::HashSet<String> = st.map(|s| s.files.values().filter_map(|r| r.original.clone()).collect()).unwrap_or_default();
    if let Ok(rd) = std::fs::read_dir(env.store().join("blobs")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let sha = n.strip_suffix(&format!(".{BLOB_EXT}")).unwrap_or(&n);
            if !keep.contains(sha) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Write `prev` back to `target` (None = delete). Transient locks are retried.
fn restore_bytes(target: &Path, prev: Option<&[u8]>) -> Result<(), String> {
    match prev {
        Some(b) => {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            util::retry_io(|| util::atomic_write(target, b)).map_err(|e| format!("cannot restore {}: {}", target.display(), util::explain_io(&e)))
        }
        None => match util::retry_io(|| std::fs::remove_file(target)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {}: {}", target.display(), util::explain_io(&e))),
        },
    }
}

/// A temp file `atomic_write` left behind for `target` by a crash.
fn remove_stale_tmp(target: &Path) {
    if let Some(t) = util::atomic_tmp_path(target) {
        let _ = std::fs::remove_file(t);
    }
}

/// Undo an interrupted transaction (steps in reverse, created folders, state).
pub fn rollback(env: &Env, tx: &Tx) -> Result<(), String> {
    let mut errs = vec![];
    for s in tx.steps.iter().rev() {
        let target = Path::new(&s.target);
        remove_stale_tmp(target);
        let prev = match &s.prev {
            Some(sha) => match get_blob(env, sha) {
                Ok(b) => Some(b),
                Err(e) => {
                    errs.push(e);
                    continue;
                }
            },
            None => None,
        };
        if let Err(e) = restore_bytes(target, prev.as_deref()) {
            errs.push(e);
            continue;
        }
        if let (Some(n), Some(parent)) = (&s.prev_name, target.parent()) {
            if let Err(e) = util::set_name_case(&parent.join(n)) {
                errs.push(format!("cannot rename {} back to {n}: {e}", target.display()));
            }
        }
    }
    for d in tx.created_dirs.iter().rev() {
        let _ = std::fs::remove_dir(d); // only if empty
    }
    if !errs.is_empty() {
        return Err(format!("rollback incomplete: {}", errs.join("; ")));
    }
    match &tx.prev_state {
        Some(prev) => {
            let mut p = (**prev).clone();
            p.pending = None;
            save_state(env, &p)?;
            gc_blobs(env, Some(&p));
        }
        None => {
            let _ = std::fs::remove_dir_all(env.store());
        }
    }
    Ok(())
}

/// Roll back a transaction left behind by a crash. Returns true if one was found.
pub fn recover(env: &Env) -> Result<bool, String> {
    match load_state(env)? {
        Some(st) => match &st.pending {
            Some(tx) => rollback(env, tx).map(|_| true),
            None => Ok(false),
        },
        None => Ok(false),
    }
}

/// Exclusive lock for install / uninstall / save restore: the launcher
/// window and the command line never run one at the same time. Held as an
/// open file with no sharing, so the OS releases it when the process ends
/// (a crash cannot leave a stale lock).
pub struct OpLock {
    _f: std::fs::File,
}

pub fn lock(hsmp_home: &Path) -> Result<OpLock, String> {
    let p = hsmp_home.join("launcher").join(".lock");
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", hsmp_home.display()))?;
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    match o.open(&p) {
        Ok(f) => Ok(OpLock { _f: f }),
        Err(e) if matches!(e.raw_os_error(), Some(32) | Some(33)) => {
            Err("another HSMP launcher (a launcher window or the command line) is installing, uninstalling or restoring saves right now. Wait for it to finish, then try again.".into())
        }
        Err(e) => Err(format!("cannot take the launcher lock {}: {e}", p.display())),
    }
}

struct Txn<'a> {
    env: &'a Env,
    st: State,
    tx: Tx,
    writes: usize,
    fail_after: Option<usize>,
    /// The player's own edits of release files this install overwrites or removes
    /// (rel path, their bytes); archived to changed_by_you once the install succeeded
    player_edits: Vec<(String, Vec<u8>)>,
}

impl<'a> Txn<'a> {
    fn save(&mut self) -> Result<(), String> {
        self.st.pending = Some(self.tx.clone());
        save_state(self.env, &self.st)
    }

    fn ensure_dir(&mut self, dir: &Path) -> Result<(), String> {
        let mut missing = vec![];
        let mut cur = Some(dir);
        while let Some(c) = cur {
            if c.is_dir() {
                break;
            }
            missing.push(c.to_path_buf());
            cur = c.parent();
        }
        for d in missing.into_iter().rev() {
            self.tx.created_dirs.push(d.to_string_lossy().to_string());
            match d.strip_prefix(&self.env.game_root) {
                Ok(rel) => {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !self.st.dirs_created.iter().any(|x| util::fold(x) == util::fold(&rel)) {
                        self.st.dirs_created.push(rel);
                    }
                }
                Err(_) => {
                    let s = d.to_string_lossy().to_string();
                    if !self.st.ext_dirs_created.contains(&s) {
                        self.st.ext_dirs_created.push(s);
                    }
                }
            }
            self.save()?;
            std::fs::create_dir(&d).map_err(|e| format!("cannot create folder {}: {e}", d.display()))?;
        }
        Ok(())
    }

    /// Record the previous content (and name case), then write (None =
    /// delete) and give the file exactly the name `target` spells.
    /// Returns (previous blob, previous on-disk name).
    fn put(&mut self, target: &Path, bytes: Option<&[u8]>) -> Result<(Option<String>, Option<String>), String> {
        if let Some(n) = self.fail_after {
            if self.writes >= n {
                return Err(format!("simulated failure before writing {}", target.display()));
            }
        }
        let prev = util::retry_io(|| util::read_opt(target)).map_err(|e| format!("cannot read {}: {}", target.display(), util::explain_io(&e)))?;
        let prev_name = prev.as_ref().and_then(|_| util::actual_file_name(target)).map(|n| n.to_string_lossy().to_string());
        let prev_blob = match &prev {
            Some(b) => Some(put_blob(self.env, b)?),
            None => None,
        };
        let case_ok = bytes.is_none() || prev_name.as_deref().map(|n| Some(std::ffi::OsStr::new(n)) == target.file_name()).unwrap_or(true);
        if prev.as_deref() == bytes && case_ok {
            return Ok((prev_blob, prev_name));
        }
        self.tx.steps.push(TxStep { target: target.to_string_lossy().to_string(), prev: prev_blob.clone(), prev_name: prev_name.clone() });
        if let (Some(_), Some(parent)) = (bytes, target.parent()) {
            self.ensure_dir(parent)?;
        }
        self.save()?;
        self.writes += 1;
        match bytes {
            Some(b) => {
                if prev.as_deref() != Some(b) {
                    util::retry_io(|| util::atomic_write(target, b)).map_err(|e| format!("cannot write {}: {}", target.display(), util::explain_io(&e)))?;
                }
                util::set_name_case(target).map_err(|e| format!("cannot rename {}: {e}", target.display()))?;
            }
            None => restore_bytes(target, None)?,
        }
        Ok((prev_blob, prev_name))
    }

    /// Install a game file; on first touch keep its original (bytes and name).
    fn install_file(&mut self, rel: &str, bytes: &[u8], kind: &str) -> Result<(), String> {
        let abs = self.env.abs(rel);
        self.note_player_edit(&util::fold(rel), Some(bytes));
        let (prev_blob, prev_name) = self.put(&abs, Some(bytes))?;
        let key = util::fold(rel);
        let installed = util::sha256_hex(bytes);
        match self.st.files.get_mut(&key) {
            // same file (any case): the original is carried forward
            Some(rec) => {
                rec.path = rel.to_string();
                rec.installed = installed;
                rec.kind = kind.into();
            }
            None => {
                let original_path = match (&prev_blob, &prev_name) {
                    (Some(_), Some(n)) => Some(match rel.rsplit_once('/') {
                        Some((dir, _)) => format!("{dir}/{n}"),
                        None => n.clone(),
                    }),
                    _ => None,
                };
                self.st.files.insert(key, FileRec { path: rel.to_string(), original: prev_blob, original_path, installed, kind: kind.into() });
            }
        }
        Ok(())
    }

    /// A release file ("copy") the player changed after HSMP wrote it, about to be
    /// overwritten with `next` (or removed: None): keep their bytes (Uninstall already did).
    fn note_player_edit(&mut self, key: &str, next: Option<&[u8]>) {
        let Some(rec) = self.st.files.get(key) else { return };
        if rec.kind != "copy" {
            return; // merged files (mods.txt, hsmp.cfg) carry the player's lines forward
        }
        let Ok(Some(cur)) = util::read_opt(&self.env.abs(&rec.path)) else { return };
        let h = util::sha256_hex(&cur);
        if h != rec.installed && rec.original.as_deref() != Some(h.as_str()) && next != Some(cur.as_slice()) {
            self.player_edits.push((rec.path.clone(), cur));
        }
    }

    /// Put a file back to its original (update: a file the new release no longer ships).
    fn restore_original(&mut self, key: &str) -> Result<(), String> {
        self.note_player_edit(key, None);
        let rec = self.st.files.get(key).cloned().ok_or("no record")?;
        let (target, orig) = match &rec.original {
            Some(sha) => (rec.original_path.clone().unwrap_or_else(|| rec.path.clone()), Some(get_blob(self.env, sha)?)),
            None => (rec.path.clone(), None),
        };
        let abs = self.env.abs(&target);
        self.put(&abs, orig.as_deref())?;
        self.st.files.remove(key);
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Back up the career saves before the first install (on by default in the UI/CLI).
    pub backup_saves: bool,
    /// The player chose "install anyway" for an unknown game build.
    pub allow_unsupported: bool,
    /// Refuse while the game or HSMP binaries run (off only in tests).
    pub check_processes: bool,
    /// Install a release older than the installed one.
    pub allow_downgrade: bool,
    /// Steam reports the game is still downloading / updating.
    pub steam_updating: bool,
    /// SHA-256 of the shipping exe as the caller checked it (the window/CLI build check). The
    /// install always re-hashes the exe itself and classifies it against THIS manifest's
    /// supported builds: a caller's stale verdict (another release, an exe Steam
    /// updated since) is never trusted. A mismatch with this value refuses the install.
    pub exe_sha256: Option<String>,
    #[doc(hidden)]
    pub fail_after_writes: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    pub version: String,
    pub updated_from: Option<String>,
    pub files_written: usize,
    pub files_removed: usize,
    pub save_backup: Option<String>,
    pub notes: Vec<String>,
}

/// Things that block an install or uninstall right now.
pub fn blockers(env: &Env, check_processes: bool) -> Vec<String> {
    let mut b = vec![];
    if !game::is_game_root(&env.game_root) {
        b.push(format!("{} is not a Half Sword folder ({} not found)", env.game_root.display(), game::exe_rel()));
        return b;
    }
    if env.game_root.to_str().is_none() {
        b.push(format!("the game folder path {} contains characters that are not valid Unicode; move Half Sword to another folder with Steam", env.game_root.display()));
    }
    if check_processes {
        for p in crate::procs::blocking_processes(&env.game_root) {
            b.push(format!("{p} is running: close it first (the launcher never kills processes)"));
        }
    }
    b
}

/// HSMP mod folders present in ue4ss/Mods plus the ones the package installs.
fn hsmp_mod_dirs(env: &Env, manifest: &Manifest) -> Vec<String> {
    // a folder name that is not valid Unicode cannot be an HSMP mod and cannot
    // be written into mods.txt faithfully: skip it (never a lossy name)
    let mut v: Vec<String> = std::fs::read_dir(env.abs(MODS_DIR_REL))
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    let prefix = format!("{MODS_DIR_REL}/");
    for (_, t) in manifest.installed_files() {
        if let Some(rest) = t.strip_prefix(&prefix) {
            if let Some((d, _)) = rest.split_once('/') {
                if !v.iter().any(|x| x.eq_ignore_ascii_case(d)) {
                    v.push(d.to_string());
                }
            }
        }
    }
    v.retain(|d| modstxt::is_hsmp(d));
    v.sort();
    v
}

/// Install or update from a verified package. All-or-nothing.
pub fn install(env: &Env, manifest: &Manifest, files: &Files, opts: &InstallOpts, log: &mut dyn FnMut(String)) -> Result<InstallReport, String> {
    let _lock = lock(&env.hsmp_home)?;
    // nothing (not even a rollback) touches the game folder while something holds it
    let blk = blockers(env, opts.check_processes);
    if !blk.is_empty() {
        return Err(blk.join("\n"));
    }
    if opts.steam_updating {
        return Err("Steam is still downloading or updating Half Sword. Wait until Steam shows it as ready to play, then install.".into());
    }
    if recover(env)? {
        log("an interrupted install was found and rolled back".into());
    }
    // The build verdict is computed here, against the manifest being installed
    let (exe_sha, exe_size) = util::sha256_file(&game::exe_path(&env.game_root)).map_err(|e| format!("cannot read the game exe: {e}"))?;
    if let Some(seen) = &opts.exe_sha256 {
        if !seen.eq_ignore_ascii_case(&exe_sha) {
            return Err("the Half Sword exe changed since it was checked (Steam updated the game?). Check the game build again, then install.".into());
        }
    }
    if !game::classify(&exe_sha, exe_size, &manifest.game).is_supported() && !opts.allow_unsupported {
        return Err(format!("this Half Sword build (exe sha256 {exe_sha}) is not supported by HSMP {} (choose \"install anyway\" to override)", manifest.version));
    }
    let old_layout = game::old_layout_ue4ss(&env.game_root);
    if !old_layout.is_empty() {
        return Err(format!(
            "an older UE4SS install is in the game folder ({}). Remove it (or verify the game files in Steam) and run the launcher again.",
            old_layout.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    // verify that every installed file is present in the verified set
    for (f, _) in manifest.installed_files() {
        if !files.contains_key(&f.path) {
            return Err(format!("package file {} was not verified", f.path));
        }
    }
    let template_bytes = files.get(&manifest.mods_template).ok_or("mods template missing from the verified files")?;
    let template = modstxt::parse_template(&String::from_utf8_lossy(template_bytes))?;

    let old = load_state(env)?;
    if let Some(o) = &old {
        if util::cmp_version(&manifest.version, &o.version) == std::cmp::Ordering::Less && !opts.allow_downgrade {
            return Err(format!(
                "HSMP {} is installed; this release ({}) is OLDER. Use the newer release, or choose \"downgrade\" (--allow-downgrade) if you really want the older one.",
                o.version, manifest.version
            ));
        }
    }
    let mut report = InstallReport { version: manifest.version.clone(), updated_from: old.as_ref().map(|s| s.version.clone()), ..Default::default() };
    if let Some(o) = &old {
        if let Some(prev) = o.previous_roots.last() {
            report.notes.push(format!("Half Sword was moved from {prev}; HSMP's install record followed it"));
        }
    }

    // career saves first: nothing is written to the game before this succeeded
    let mut save_backup_id = old.as_ref().and_then(|s| s.save_backup_id.clone());
    if old.is_none() && opts.backup_saves {
        match saves::backup(&env.save_dir(), &env.save_backups(), "automatic backup before the first HSMP install")? {
            Some(b) => {
                log(format!("career saves backed up ({} files) to {}", b.files.len(), env.save_backups().join(&b.id).display()));
                save_backup_id = Some(b.id.clone());
                report.save_backup = Some(b.id);
            }
            None => log("no career saves found to back up (new install of Half Sword?)".into()),
        }
    }

    let win64 = game::win64(&env.game_root);
    let mut st = old.clone().unwrap_or_else(|| State { runtime_preexisting: runtime_names(&win64).iter().map(|n| n.to_string_lossy().to_string()).collect(), ..Default::default() });
    st.schema = STATE_SCHEMA;
    st.game_root = env.game_root.to_string_lossy().to_string();
    st.save_backup_id = save_backup_id;
    st.install_id = env.store_key();
    st.identity = Some(Identity { steam_appid: manifest.game.steam_appid, exe_sha256: exe_sha.clone() });
    st.uninstalling = false;
    let tx = Tx { started_utc: util::iso_utc(util::now_unix()), steps: vec![], created_dirs: vec![], prev_state: old.clone().map(Box::new) };
    let mut t = Txn { env, st, tx, writes: 0, fail_after: opts.fail_after_writes, player_edits: vec![] };

    let result = (|| -> Result<(), String> {
        t.save()?;
        let mut planned: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (f, target) in manifest.installed_files() {
            t.install_file(target, &files[&f.path], "copy")?;
            planned.insert(util::fold(target));
        }
        // mods.txt from the template + whatever is there now
        let cur = util::read_opt(&env.abs(MODS_TXT_REL)).map_err(|e| e.to_string())?;
        let mods_txt = modstxt::render_bytes(&template, cur.as_deref(), &hsmp_mod_dirs(env, manifest), &manifest.version);
        t.install_file(MODS_TXT_REL, &mods_txt, "merged")?;
        planned.insert(util::fold(MODS_TXT_REL));
        // hsmp.cfg (other lines kept byte-for-byte, in the file's own encoding)
        let cur = util::read_opt(&env.abs(CFG_REL)).map_err(|e| e.to_string())?;
        let (cur_text, enc) = cur.as_deref().map(util::decode_text).map(|(t, e)| (Some(t), e)).unwrap_or((None, util::TextEnc::Utf8));
        let merged = cfgfile::merge(cur_text.as_deref(), &manifest.version, &manifest.master_urls, t.st.master_url_written.as_deref());
        if merged.kept_user_master {
            log(format!("hsmp.cfg: kept your master_url ({})", merged.master_url));
        }
        t.install_file(CFG_REL, &util::encode_text(&merged.text, enc), "merged")?;
        t.st.master_url_written = Some(merged.master_url);
        planned.insert(util::fold(CFG_REL));
        // the install identity marker (lets the journal follow a moved library)
        let marker = Marker { product: crate::manifest::PRODUCT.into(), install_id: t.st.install_id.clone(), steam_appid: manifest.game.steam_appid, exe_sha256: exe_sha.clone() };
        let mut mb = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
        mb.push(b'\n');
        t.install_file(MARKER_REL, &mb, "marker")?;
        planned.insert(util::fold(MARKER_REL));
        // files the previous version installed that this one does not ship
        let stale: Vec<String> = t.st.files.keys().filter(|k| !planned.contains(*k)).cloned().collect();
        for key in stale {
            t.restore_original(&key)?;
            report.files_removed += 1;
        }
        // Engine.ini (hair-strand streaming workaround and any other manifest ini setting)
        for s in &manifest.ini_settings {
            let path = env.ini_path(&s.file);
            let (text, enc) = read_text(&path)?;
            let (new_text, edit) = ini::apply_setting(text.as_deref(), &s.section, &s.key, &s.value);
            if let Some(nt) = new_text {
                t.put(&path, Some(&util::encode_text(&nt, enc)))?;
                log(format!("{}.ini: [{}] {}={} ({})", s.file, s.section, s.key, s.value, s.reason));
            }
            merge_ini_record(&mut t.st.ini_edits, &s.file, &s.section, &s.key, edit);
        }
        Ok(())
    })();

    if let Err(e) = result {
        let rb = rollback(env, &t.tx);
        return Err(match rb {
            Ok(()) => format!("install failed: {e}\nEverything was rolled back; the game folder is as it was."),
            Err(r) => format!("install failed: {e}\nROLLBACK PROBLEM: {r}\nRun the launcher again to retry the rollback."),
        });
    }
    report.files_written = t.tx.steps.len();
    t.st.version = manifest.version.clone();
    t.st.channel = manifest.channel.clone();
    t.st.git_commit = manifest.git_commit.clone();
    t.st.protocol_version = manifest.protocol_version;
    t.st.installed_utc = util::iso_utc(util::now_unix());
    t.st.launch_args = manifest.launch_args.clone();
    t.st.ini_settings = manifest.ini_settings.clone();
    t.st.pending = None;
    save_state(env, &t.st)?;
    gc_blobs(env, Some(&t.st));
    if !t.player_edits.is_empty() {
        let dir = env.hsmp_home.join("changed_by_you").join(util::stamp_utc(util::now_unix()));
        for (rel, bytes) in &t.player_edits {
            let dst = util::join_rel(&dir, rel);
            match std::fs::create_dir_all(dst.parent().unwrap()).and_then(|_| std::fs::write(&dst, bytes)) {
                Ok(()) => report.notes.push(format!("you had changed {rel}; the release version replaced it and your version was kept as {}", dst.display())),
                Err(e) => report.notes.push(format!("you had changed {rel}; it was replaced, and keeping your copy failed: {e}")),
            }
        }
    }
    for w in enabled_txt_warnings(env, &template) {
        report.notes.push(w);
    }
    Ok(report)
}

/// A text file decoded without loss (None = absent), plus its encoding.
fn read_text(path: &Path) -> Result<(Option<String>, util::TextEnc), String> {
    Ok(match util::read_opt(path).map_err(|e| format!("{}: {e}", path.display()))? {
        Some(b) => {
            let (t, e) = util::decode_text(&b);
            (Some(t), e)
        }
        None => (None, util::TextEnc::Utf8),
    })
}

fn merge_ini_record(recs: &mut Vec<IniEditRec>, file: &str, section: &str, key: &str, edit: IniEdit) {
    let pos = recs.iter().position(|r| r.file.eq_ignore_ascii_case(file) && r.section.eq_ignore_ascii_case(section) && r.key.eq_ignore_ascii_case(key));
    match (pos, &edit) {
        (Some(_), IniEdit::AlreadyPresent) => {} // keep how we got there
        // The player's ORIGINAL line is kept whatever a re-apply did later (the game
        // dropped our line, we inserted it again): uninstall puts the original value back
        // (revert finds our line in the section and writes the original over it)
        (Some(i), _) if matches!(recs[i].edit, IniEdit::ReplacedLine { .. }) => {}
        (Some(i), _) => recs[i].edit = edit,
        (None, _) => recs.push(IniEditRec { file: file.into(), section: section.into(), key: key.into(), edit }),
    }
}

/// UE4SS also starts any mod with an `enabled.txt`, whatever mods.txt says.
pub fn enabled_txt_warnings(env: &Env, template: &[modstxt::Entry]) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(env.abs(MODS_DIR_REL)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().join("enabled.txt").is_file() {
                let off = template.iter().any(|t| t.name.eq_ignore_ascii_case(&name) && t.value != "1");
                if off {
                    out.push(format!("ue4ss/Mods/{name}/enabled.txt makes UE4SS load {name} although the release disables it; delete that enabled.txt"));
                }
            }
        }
    }
    out
}

/// Re-apply the manifest's ini settings (the game may rewrite Engine.ini).
/// Called before every launch; journaled into the install state.
pub fn reapply_ini(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    let _lock = lock(&env.hsmp_home)?;
    let Some(mut st) = load_state(env)? else { return Ok(()) };
    if st.pending.is_some() {
        return Err("an install is still pending; run the launcher's install again".into());
    }
    if st.uninstalling {
        return Err("an uninstall has not finished; run Uninstall again".into());
    }
    let mut changed = false;
    for s in st.ini_settings.clone() {
        let path = env.ini_path(&s.file);
        let (text, enc) = read_text(&path)?;
        let (new_text, edit) = ini::apply_setting(text.as_deref(), &s.section, &s.key, &s.value);
        if let Some(nt) = new_text {
            if let Some(p) = path.parent() {
                if !p.is_dir() {
                    std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
                }
            }
            util::atomic_write(&path, &util::encode_text(&nt, enc)).map_err(|e| format!("{}: {e}", path.display()))?;
            log(format!("{}.ini: re-applied [{}] {}={} (the game had removed it)", s.file, s.section, s.key, s.value));
            changed = true;
        }
        merge_ini_record(&mut st.ini_edits, &s.file, &s.section, &s.key, edit);
    }
    if changed {
        save_state(env, &st)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct UninstallOpts {
    /// Refuse while the game or HSMP binaries run (off only in tests).
    pub check_processes: bool,
    /// Give up on originals whose backup copy is gone for good (antivirus
    /// deleted it): HSMP's version of such a file is removed instead.
    pub forget_missing: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UninstallReport {
    pub restored: usize,
    pub removed: usize,
    pub archived_to: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Undo everything the launcher did. Resumable: progress is saved per file.
pub fn uninstall(env: &Env, check_processes: bool, log: &mut dyn FnMut(String)) -> Result<UninstallReport, String> {
    uninstall_with(env, &UninstallOpts { check_processes, forget_missing: false }, log)
}

/// Uninstall. A file that cannot be restored (its backup copy is missing or
/// locked, typically by an antivirus) is skipped after retries: everything
/// else is undone, the journal keeps exactly the files still to do (so the
/// state is never half-known), and the error says what to do. Running it
/// again finishes the job; `forget_missing` gives up on lost backups.
pub fn uninstall_with(env: &Env, o: &UninstallOpts, log: &mut dyn FnMut(String)) -> Result<UninstallReport, String> {
    let _lock = lock(&env.hsmp_home)?;
    if o.check_processes {
        let blk = crate::procs::blocking_processes(&env.game_root);
        if !blk.is_empty() {
            return Err(blk.iter().map(|p| format!("{p} is running: close it first")).collect::<Vec<_>>().join("\n"));
        }
    }
    if recover(env)? {
        log("an interrupted install was found and rolled back".into());
    }
    let Some(mut st) = load_state(env)? else {
        return Err("HSMP is not installed in this game folder by the launcher (nothing to undo)".into());
    };
    if !st.uninstalling {
        st.uninstalling = true;
        save_state(env, &st)?;
    }
    let mut rep = UninstallReport::default();
    let archive = env.hsmp_home.join("uninstalled").join(util::stamp_utc(util::now_unix()));
    let mut archived = false;
    let mut skipped: Vec<String> = vec![];
    // deepest paths first
    let mut keys: Vec<String> = st.files.keys().cloned().collect();
    keys.sort_by(|a, b| b.matches('/').count().cmp(&a.matches('/').count()).then(b.cmp(a)));
    for key in keys {
        let rec = st.files[&key].clone();
        let abs = env.abs(&rec.path);
        remove_stale_tmp(&abs);
        // a file the player changed after HSMP wrote it: keep their version
        if let Ok(Some(cur)) = util::read_opt(&abs) {
            let h = util::sha256_hex(&cur);
            if h != rec.installed && rec.original.as_deref() != Some(h.as_str()) {
                let dst = util::join_rel(&archive.join("changed_by_you"), &rec.path);
                std::fs::create_dir_all(dst.parent().unwrap()).and_then(|_| std::fs::write(&dst, &cur)).map_err(|e| format!("cannot keep your copy of {}: {e}", rec.path))?;
                archived = true;
                rep.notes.push(format!("you had changed {}; your version was kept as {}", rec.path, dst.display()));
            }
        }
        let outcome = match &rec.original {
            Some(sha) => match get_blob(env, sha) {
                Ok(b) => {
                    let target = env.abs(rec.original_path.as_deref().unwrap_or(&rec.path));
                    restore_bytes(&target, Some(&b)).and_then(|_| util::set_name_case(&target).map_err(|e| format!("cannot rename {}: {e}", target.display()))).map(|_| rep.restored += 1)
                }
                Err(e) if o.forget_missing => {
                    // the original is gone for good: remove HSMP's copy (unless the player changed it)
                    let ours = util::sha256_file(&abs).map(|(h, _)| h == rec.installed).unwrap_or(false);
                    if ours {
                        restore_bytes(&abs, None)?;
                    }
                    rep.notes.push(format!("{}: the original could not be restored ({e}); {}", rec.path, if ours { "HSMP's version was removed" } else { "the file was left as it is" }));
                    Ok(())
                }
                Err(e) => Err(e),
            },
            None => restore_bytes(&abs, None).map(|_| rep.removed += 1),
        };
        match outcome {
            Ok(()) => {
                st.files.remove(&key);
                save_state(env, &st)?;
            }
            Err(e) => {
                log(format!("could not restore {}: {e}", rec.path));
                skipped.push(format!("{}: {e}", rec.path));
            }
        }
    }
    for r in st.ini_edits.clone() {
        let path = env.ini_path(&r.file);
        let (text, enc) = read_text(&path)?;
        match ini::revert_setting(text.as_deref(), &r.edit) {
            Revert::Keep => {}
            Revert::Write(t) => util::retry_io(|| util::atomic_write(&path, &util::encode_text(&t, enc))).map_err(|e| format!("{}: {}", path.display(), util::explain_io(&e)))?,
            Revert::Delete => util::retry_io(|| std::fs::remove_file(&path)).map_err(|e| format!("{}: {}", path.display(), util::explain_io(&e)))?,
        }
        st.ini_edits.retain(|x| x != &r);
        save_state(env, &st)?;
    }
    if !skipped.is_empty() {
        // stop before the folder cleanup: those folders still hold files to restore
        save_state(env, &st)?;
        return Err(format!(
            "HSMP was partly removed. {} file(s) could not be put back yet:\n  {}\n\
             What to do: if your antivirus quarantined or locked a file in {}, restore it from the antivirus quarantine (or allow it) and click Uninstall again. \
             Uninstall resumes where it stopped; everything else is already undone.\n\
             If the backup is gone for good, run `hsmp-launcher uninstall --forget-missing` (or use Steam's \"Verify integrity of game files\" afterwards).",
            skipped.len(),
            skipped.join("\n  "),
            env.store().join("blobs").display()
        ));
    }
    // runtime leftovers: archived, never deleted
    let mut archive_move = |src: &Path, dst: &Path| -> Result<(), String> {
        util::move_path(src, dst).map_err(|e| format!("cannot archive {}: {e}", src.display()))?;
        archived = true;
        Ok(())
    };
    let win64 = game::win64(&env.game_root);
    for name in runtime_names(&win64) {
        let n = name.to_string_lossy().to_string();
        if !st.runtime_preexisting.iter().any(|p| p.eq_ignore_ascii_case(&n)) {
            archive_move(&win64.join(&name), &util::join_rel(&archive, game::WIN64_REL).join(&name))?;
        }
    }
    for rel in st.dirs_created.iter().rev() {
        let d = env.abs(rel);
        if d.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let child_rel = format!("{rel}/{}", e.file_name().to_string_lossy());
                    // a child folder we created ourselves is handled by its own entry
                    if e.path().is_dir() && st.dirs_created.iter().any(|x| util::fold(x) == util::fold(&child_rel)) {
                        continue;
                    }
                    archive_move(&e.path(), &util::join_rel(&archive, rel).join(e.file_name()))?;
                }
            }
            util::retry_io(|| std::fs::remove_dir(&d)).map_err(|e| format!("cannot remove folder {}: {}", d.display(), util::explain_io(&e)))?;
        }
    }
    for d in st.ext_dirs_created.iter().rev() {
        let _ = std::fs::remove_dir(d); // only if empty (the game may use it)
    }
    if archived {
        log(format!("runtime files HSMP created (settings, logs) were moved to {}", archive.display()));
        rep.archived_to = Some(archive);
    }
    std::fs::remove_dir_all(env.store()).map_err(|e| format!("cannot remove install state: {e}"))?;
    if let Some(id) = &st.save_backup_id {
        rep.notes.push(format!("your career save backup {id} is kept in {}", env.save_backups().display()));
    }
    Ok(rep)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    NotInstalled { foreign_hsmp: bool, foreign_ue4ss: bool },
    Interrupted,
    /// An uninstall stopped part-way (files listed still need restoring).
    UninstallIncomplete { remaining: Vec<String> },
    Installed { version: String, protocol: u32, modified: Vec<String>, missing: Vec<String> },
}

pub fn status(env: &Env) -> Result<Status, String> {
    let Some(st) = load_state(env)? else {
        let w = game::win64(&env.game_root);
        return Ok(Status::NotInstalled {
            foreign_hsmp: w.join("hsmp.cfg").is_file() || w.join("ue4ss/Mods/HSMPMenu").is_dir(),
            foreign_ue4ss: w.join("ue4ss").is_dir() || w.join("dwmapi.dll").is_file(),
        });
    };
    if st.pending.is_some() {
        return Ok(Status::Interrupted);
    }
    if st.uninstalling {
        return Ok(Status::UninstallIncomplete { remaining: st.files.values().map(|r| r.path.clone()).collect() });
    }
    let mut modified = vec![];
    let mut missing = vec![];
    for rec in st.files.values() {
        match util::sha256_file(&env.abs(&rec.path)) {
            Ok((h, _)) if h == rec.installed => {}
            Ok(_) => modified.push(rec.path.clone()),
            Err(_) => missing.push(rec.path.clone()),
        }
    }
    Ok(Status::Installed { version: st.version, protocol: st.protocol_version, modified, missing })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{tests::sample, FileEntry, Role};
    use crate::testutil::{snapshot, TempDir};

    const W: &str = "HalfswordUE5/Binaries/Win64";

    struct Fake {
        _t: TempDir,
        env: Env,
    }

    fn fake(with_ue4ss: bool) -> Fake {
        let t = TempDir::new("install");
        let root = t.path().join("Half Sword");
        let env = Env::new(&root, t.path().join("LocalAppData/HSMP"), t.path().join("LocalAppData/HalfSwordUE5/Saved"));
        let w = game::win64(&root);
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join(game::EXE_NAME), b"exe").unwrap();
        std::fs::write(w.join("tbb.dll"), b"game dll").unwrap();
        std::fs::create_dir_all(root.join("HalfswordUE5/Content/Paks")).unwrap();
        std::fs::write(root.join("HalfswordUE5/Content/Paks/pakchunk0.pak"), b"pak").unwrap();
        if with_ue4ss {
            // a player who already runs UE4SS with a third-party mod
            std::fs::write(w.join("dwmapi.dll"), b"their proxy").unwrap();
            std::fs::create_dir_all(w.join("ue4ss/Mods/TheirMod/Scripts")).unwrap();
            std::fs::write(w.join("ue4ss/UE4SS.dll"), b"their ue4ss").unwrap();
            std::fs::write(w.join("ue4ss/Mods/TheirMod/Scripts/main.lua"), b"print(1)").unwrap();
            std::fs::write(w.join("ue4ss/Mods/mods.txt"), b"TheirMod : 1\r\nConsoleEnablerMod : 1\r\n\r\n; Built-in keybinds, do not move up!\r\nKeybinds : 1\r\n").unwrap();
            std::fs::write(w.join("imgui.ini"), b"[Window]").unwrap();
        }
        let cfg = env.ue_saved.join("Config/Windows");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("Engine.ini"), b"[/Script/Engine.RendererSettings]\r\nr.X=1\r\n").unwrap();
        std::fs::write(cfg.join("GameUserSettings.ini"), b"[x]\r\n").unwrap();
        std::fs::create_dir_all(env.save_dir()).unwrap();
        std::fs::write(env.save_dir().join("GameProgress.sav"), b"career").unwrap();
        Fake { _t: t, env }
    }

    /// A manifest + verified file set. `variant` changes the payload a bit (for updates).
    fn package(variant: u8) -> (Manifest, Files) {
        let mut m = sample();
        let mut files = Files::new();
        let mut add = |m: &mut Manifest, path: &str, bytes: &[u8], role: Role, install: Option<&str>| {
            m.files.push(FileEntry { path: path.into(), sha256: util::sha256_hex(bytes), size: bytes.len() as u64, role, install: install.map(|s| format!("{W}/{s}")) });
            files.insert(path.into(), bytes.to_vec());
        };
        m.files.clear();
        add(&mut m, "payload/mods.release.txt", b"BPModLoaderMod : 1\nConsoleEnablerMod : 0\nHSMPMenu : 1\nHSMPWorld : 1\nHSMPDiag : dev\nKeybinds : dev\n", Role::Template, None);
        add(&mut m, "payload/Win64/dwmapi.dll", b"pinned proxy", Role::Ue4ss, Some("dwmapi.dll"));
        add(&mut m, "payload/Win64/ue4ss/UE4SS.dll", b"pinned ue4ss", Role::Ue4ss, Some("ue4ss/UE4SS.dll"));
        add(&mut m, "payload/Win64/ue4ss/Mods/BPModLoaderMod/Scripts/main.lua", b"-- bpml", Role::Ue4ss, Some("ue4ss/Mods/BPModLoaderMod/Scripts/main.lua"));
        add(&mut m, "payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua", if variant == 0 { b"-- menu v1" } else { b"-- menu v2" }, Role::Mod, Some("ue4ss/Mods/HSMPMenu/Scripts/main.lua"));
        if variant == 0 {
            add(&mut m, "payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/old.lua", b"-- dropped in v2", Role::Mod, Some("ue4ss/Mods/HSMPMenu/Scripts/old.lua"));
        } else {
            add(&mut m, "payload/Win64/ue4ss/Mods/HSMPWorld/Scripts/main.lua", b"-- world (new in v2)", Role::Mod, Some("ue4ss/Mods/HSMPWorld/Scripts/main.lua"));
        }
        add(&mut m, "payload/Win64/hsmp/hsmp-server.exe", b"server", Role::Bin, Some("hsmp/hsmp-server.exe"));
        m.version = format!("0.{}.0", variant + 1);
        m.validate().unwrap();
        (m, files)
    }

    fn opts() -> InstallOpts {
        InstallOpts { backup_saves: true, ..Default::default() }
    }

    fn snap_all(env: &Env) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        (snapshot(&env.game_root), snapshot(&env.ue_saved))
    }

    fn quiet() -> impl FnMut(String) {
        |_s: String| {}
    }

    fn check_roundtrip(with_ue4ss: bool) {
        let f = fake(with_ue4ss);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        let rep = install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        assert!(rep.save_backup.is_some());
        let w = game::win64(&f.env.game_root);
        assert_eq!(std::fs::read(w.join("dwmapi.dll")).unwrap(), b"pinned proxy");
        let mods = std::fs::read_to_string(w.join("ue4ss/Mods/mods.txt")).unwrap();
        assert!(mods.contains("HSMPMenu : 1"));
        assert!(mods.contains("ConsoleEnablerMod : 0"));
        if with_ue4ss {
            assert!(mods.contains("TheirMod : 1"), "{mods}");
        }
        let cfg = std::fs::read_to_string(w.join("hsmp.cfg")).unwrap();
        assert_eq!(cfgfile::get(&cfg, "bin_dir").as_deref(), Some("hsmp"));
        let eng = std::fs::read_to_string(f.env.ini_path("Engine")).unwrap();
        assert!(eng.contains("[SystemSettings]\r\nr.HairStrands.Streaming=0\r\n"));
        assert!(matches!(status(&f.env).unwrap(), Status::Installed { ref modified, ref missing, .. } if modified.is_empty() && missing.is_empty()));
        // the game runs: runtime files appear
        std::fs::create_dir_all(w.join("hsmp_state")).unwrap();
        std::fs::write(w.join("hsmp_state/.settings.json"), b"{}").unwrap();
        std::fs::write(w.join("ue4ss/UE4SS.log"), b"log").unwrap();
        std::fs::write(w.join("hsmp/server.log"), b"log").unwrap();
        if !with_ue4ss {
            std::fs::write(w.join("imgui.ini"), b"[Window]").unwrap();
        }
        // a save is written by the game: uninstall must not touch saves
        std::fs::write(f.env.save_dir().join("GameProgress.sav"), b"career progressed").unwrap();
        let rep = uninstall(&f.env, false, &mut quiet()).unwrap();
        assert!(rep.archived_to.is_some());
        std::fs::write(f.env.save_dir().join("GameProgress.sav"), b"career").unwrap();
        let after = snap_all(&f.env);
        if with_ue4ss {
            // the pre-existing ue4ss dir keeps the runtime log UE4SS itself wrote
            let mut a = after.0.clone();
            a.remove("HalfswordUE5/Binaries/Win64/ue4ss/UE4SS.log");
            assert_eq!(a, before.0, "game folder restored byte-identically");
        } else {
            assert_eq!(after.0, before.0, "game folder restored byte-identically");
        }
        assert_eq!(after.1, before.1, "Engine.ini restored byte-identically, saves untouched");
        assert!(!f.env.store().exists(), "install state removed");
        assert_eq!(saves::list(&f.env.save_backups()).len(), 1, "career backup kept");
        let arch = rep.archived_to.unwrap();
        assert!(arch.join(W).join("hsmp_state/.settings.json").is_file());
        assert!(arch.join(W).join("hsmp/server.log").is_file());
    }

    #[test]
    fn install_uninstall_clean_game() {
        check_roundtrip(false);
    }

    #[test]
    fn install_uninstall_over_existing_ue4ss() {
        check_roundtrip(true);
    }

    #[test]
    fn update_then_uninstall() {
        let f = fake(true);
        let before = snap_all(&f.env);
        let (m1, f1) = package(0);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        let w = game::win64(&f.env.game_root);
        assert!(w.join("ue4ss/Mods/HSMPMenu/Scripts/old.lua").is_file());
        let (m2, f2) = package(1);
        let rep = install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        assert_eq!(rep.updated_from.as_deref(), Some("0.1.0"));
        assert_eq!(rep.files_removed, 1);
        assert!(rep.save_backup.is_none(), "saves are backed up before the FIRST install only");
        assert!(!w.join("ue4ss/Mods/HSMPMenu/Scripts/old.lua").exists());
        assert_eq!(std::fs::read(w.join("ue4ss/Mods/HSMPMenu/Scripts/main.lua")).unwrap(), b"-- menu v2");
        assert!(std::fs::read_to_string(w.join("ue4ss/Mods/mods.txt")).unwrap().contains("HSMPWorld : 1"));
        // re-install of the same version is a no-op
        let rep = install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        assert_eq!(rep.files_written, 0);
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
    }

    #[test]
    fn failure_mid_install_rolls_back() {
        for n in [0, 1, 3, 6, 8] {
            let f = fake(true);
            let before = snap_all(&f.env);
            let (m, files) = package(0);
            let o = InstallOpts { fail_after_writes: Some(n), ..opts() };
            let e = install(&f.env, &m, &files, &o, &mut quiet()).unwrap_err();
            assert!(e.contains("rolled back"), "{e}");
            assert_eq!(snap_all(&f.env), before, "rollback after {n} writes");
            assert!(!f.env.store().join("state.json").exists());
        }
    }

    #[test]
    fn failed_update_returns_to_previous_version() {
        let f = fake(false);
        let (m1, f1) = package(0);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        let installed = snap_all(&f.env);
        let state1 = load_state(&f.env).unwrap();
        let (m2, f2) = package(1);
        let o = InstallOpts { fail_after_writes: Some(2), ..opts() };
        install(&f.env, &m2, &f2, &o, &mut quiet()).unwrap_err();
        assert_eq!(snap_all(&f.env), installed);
        assert_eq!(load_state(&f.env).unwrap(), state1);
    }

    #[test]
    fn crash_recovery_from_pending_tx() {
        let f = fake(false);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        // simulate a crash: failure without rollback by writing a pending state by hand
        let o = InstallOpts { fail_after_writes: Some(4), ..opts() };
        let mut t = Txn { env: &f.env, st: State::default(), tx: Tx::default(), writes: 0, fail_after: o.fail_after_writes, player_edits: vec![] };
        for (fe, target) in m.installed_files().take(3) {
            t.install_file(target, &files[&fe.path], "copy").unwrap();
        }
        drop(t); // "power loss": state.json has pending steps, files are half written
        // ...and the crash left atomic_write's temp file next to a target
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join(".dwmapi.dll.hsmp_tmp"), b"half").unwrap();
        assert_eq!(status(&f.env).unwrap(), Status::Interrupted);
        assert!(recover(&f.env).unwrap());
        assert_eq!(snap_all(&f.env), before);
    }

    #[test]
    fn modified_and_missing_detected() {
        let f = fake(false);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join("ue4ss/UE4SS.dll"), b"tampered").unwrap();
        std::fs::remove_file(w.join("hsmp/hsmp-server.exe")).unwrap();
        match status(&f.env).unwrap() {
            Status::Installed { modified, missing, .. } => {
                assert_eq!(modified, vec![format!("{W}/ue4ss/UE4SS.dll")]);
                assert_eq!(missing, vec![format!("{W}/hsmp/hsmp-server.exe")]);
            }
            s => panic!("{s:?}"),
        }
        // repair = install again
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        assert!(matches!(status(&f.env).unwrap(), Status::Installed { ref modified, ref missing, .. } if modified.is_empty() && missing.is_empty()));
    }

    #[test]
    fn engine_ini_reapplied_after_game_rewrite() {
        let f = fake(false);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        // the game rewrites Engine.ini without our line
        std::fs::write(f.env.ini_path("Engine"), b"[/Script/Engine.RendererSettings]\r\nr.X=1\r\n").unwrap();
        reapply_ini(&f.env, &mut quiet()).unwrap();
        assert!(std::fs::read_to_string(f.env.ini_path("Engine")).unwrap().contains("r.HairStrands.Streaming=0"));
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
    }

    /// The build verdict comes from the manifest being installed, never from the
    /// caller (a launcher that checked the build against an OLDER release, then opened a new one).
    #[test]
    fn build_check_is_against_the_installed_manifest() {
        let f = fake(false);
        let (mut m, files) = package(0);
        let exe_sha = util::sha256_hex(b"exe");
        // the new release dropped support for the player's build; the caller's check was against
        // the old release (it matched): still refused
        m.game.builds[0].exe_sha256 = "cd".repeat(32);
        let o = InstallOpts { exe_sha256: Some(exe_sha.clone()), ..opts() };
        let e = install(&f.env, &m, &files, &o, &mut quiet()).unwrap_err();
        assert!(e.contains("not supported"), "{e}");
        assert!(load_state(&f.env).unwrap().is_none(), "nothing installed");
        // supported by this manifest: fine
        let (m, files) = package(0);
        install(&f.env, &m, &files, &o, &mut quiet()).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap();
        // the exe changed after the caller hashed it: refused, even with "install anyway"
        let o = InstallOpts { exe_sha256: Some("ef".repeat(32)), allow_unsupported: true, ..opts() };
        assert!(install(&f.env, &m, &files, &o, &mut quiet()).unwrap_err().contains("changed since it was checked"));
    }

    /// The player had r.HairStrands.Streaming=1; the game then dropped our line and
    /// the launcher re-applied it. Uninstall must still put the player's =1 back.
    #[test]
    fn reapply_keeps_the_players_original_ini_line() {
        let f = fake(false);
        std::fs::write(f.env.ini_path("Engine"), b"[SystemSettings]\r\nr.HairStrands.Streaming=1\r\nr.Y=2\r\n").unwrap();
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        assert!(std::fs::read_to_string(f.env.ini_path("Engine")).unwrap().contains("r.HairStrands.Streaming=0"));
        // the game rewrites Engine.ini without the line
        std::fs::write(f.env.ini_path("Engine"), b"[SystemSettings]\r\nr.Y=2\r\n").unwrap();
        reapply_ini(&f.env, &mut quiet()).unwrap();
        assert!(std::fs::read_to_string(f.env.ini_path("Engine")).unwrap().contains("r.HairStrands.Streaming=0"));
        uninstall(&f.env, false, &mut quiet()).unwrap();
        let eng = std::fs::read_to_string(f.env.ini_path("Engine")).unwrap();
        assert!(eng.contains("r.HairStrands.Streaming=1"), "{eng}");
        assert!(!eng.contains("r.HairStrands.Streaming=0"), "{eng}");
    }

    /// An update overwrites (or drops) a release file the player edited: their
    /// version is archived to changed_by_you first, like Uninstall does. An unedited file is not.
    #[test]
    fn update_archives_player_edits() {
        let f = fake(false);
        let (m0, files0) = package(0);
        install(&f.env, &m0, &files0, &opts(), &mut quiet()).unwrap();
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join("ue4ss/Mods/HSMPMenu/Scripts/main.lua"), b"-- my tweak").unwrap();
        std::fs::write(w.join("ue4ss/Mods/HSMPMenu/Scripts/old.lua"), b"-- my old tweak").unwrap();
        let (m1, files1) = package(1);
        let rep = install(&f.env, &m1, &files1, &opts(), &mut quiet()).unwrap();
        assert_eq!(std::fs::read(w.join("ue4ss/Mods/HSMPMenu/Scripts/main.lua")).unwrap(), b"-- menu v2");
        let notes = rep.notes.join("\n");
        assert!(notes.contains("HSMPMenu/Scripts/main.lua") && notes.contains("changed_by_you"), "{notes}");
        assert!(notes.contains("HSMPMenu/Scripts/old.lua"), "{notes}");
        assert!(!notes.contains("dwmapi.dll"), "unedited files are not archived: {notes}");
        let kept: Vec<PathBuf> = std::fs::read_dir(f.env.hsmp_home.join("changed_by_you")).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(util::join_rel(&kept[0], &format!("{W}/ue4ss/Mods/HSMPMenu/Scripts/main.lua"))).unwrap(), b"-- my tweak");
        assert_eq!(std::fs::read(util::join_rel(&kept[0], &format!("{W}/ue4ss/Mods/HSMPMenu/Scripts/old.lua"))).unwrap(), b"-- my old tweak");
        // a repair with nothing edited archives nothing new
        let rep = install(&f.env, &m1, &files1, &opts(), &mut quiet()).unwrap();
        assert!(!rep.notes.iter().any(|n| n.contains("changed_by_you")), "{:?}", rep.notes);
    }

    #[test]
    fn refusals() {
        let f = fake(false);
        let (m, files) = package(0);
        // an exe this manifest does not list
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join(game::EXE_NAME), b"updated exe").unwrap();
        assert!(install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap_err().contains("not supported"));
        let o = InstallOpts { allow_unsupported: true, ..opts() };
        install(&f.env, &m, &files, &o, &mut quiet()).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap();
        std::fs::write(w.join(game::EXE_NAME), b"exe").unwrap();
        // old-layout UE4SS
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join("UE4SS.dll"), b"old").unwrap();
        assert!(install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap_err().contains("older UE4SS"));
        std::fs::remove_file(w.join("UE4SS.dll")).unwrap();
        // not a game folder
        let mut bad = f.env.clone();
        bad.game_root = f.env.game_root.join("nope");
        assert!(install(&bad, &m, &files, &opts(), &mut quiet()).unwrap_err().contains("not a Half Sword folder"));
        // uninstall without install
        assert!(uninstall(&f.env, false, &mut quiet()).unwrap_err().contains("not installed"));
        // a file listed in the manifest but not verified
        let mut partial = files.clone();
        partial.remove("payload/Win64/dwmapi.dll");
        assert!(install(&f.env, &m, &partial, &opts(), &mut quiet()).unwrap_err().contains("not verified"));
    }

    /// CJK mod folders, a CJK mods.txt line, a GBK line, a CJK game path and
    /// a UTF-16 Engine.ini: install works and uninstall is byte-exact.
    #[test]
    fn non_ascii_mods_paths_and_encodings() {
        let t = TempDir::new("install_cjk");
        let root = t.path().join("游戏库").join("Half Sword ÄÖ");
        let env = Env::new(&root, t.path().join("LocalAppData/HSMP"), t.path().join("LocalAppData/HalfSwordUE5/Saved"));
        let w = game::win64(&root);
        std::fs::create_dir_all(w.join("ue4ss/Mods/漢字モッド/Scripts")).unwrap();
        std::fs::create_dir_all(w.join("ue4ss/Mods/HSM漢")).unwrap();
        std::fs::write(w.join(game::EXE_NAME), b"exe").unwrap();
        std::fs::write(w.join("dwmapi.dll"), b"their proxy").unwrap();
        let mut mods = "漢字モッド : 1\r\n".as_bytes().to_vec();
        mods.extend_from_slice(b"\xba\xba\xd7\xd6 : 1\r\nKeybinds : 1\r\n");
        std::fs::write(w.join("ue4ss/Mods/mods.txt"), &mods).unwrap();
        let cfg = env.ue_saved.join("Config/Windows");
        std::fs::create_dir_all(&cfg).unwrap();
        let mut eng = vec![0xFF, 0xFE];
        for u in "[Core.Log]\r\nPath=C:\\Users\\山田\\x\r\n".encode_utf16() {
            eng.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(cfg.join("Engine.ini"), &eng).unwrap();
        let before = snap_all(&env);
        let (m, files) = package(0);
        install(&env, &m, &files, &opts(), &mut quiet()).unwrap();
        let txt = std::fs::read(w.join("ue4ss/Mods/mods.txt")).unwrap();
        assert!(txt.windows("漢字モッド : 1".len()).any(|x| x == "漢字モッド : 1".as_bytes()));
        assert!(txt.windows(5).any(|x| x == b"\xba\xba\xd7\xd6 "), "GBK name kept byte-for-byte");
        let (e, enc) = util::decode_text(&std::fs::read(cfg.join("Engine.ini")).unwrap());
        assert_eq!(enc, util::TextEnc::Utf16Le, "Engine.ini stays UTF-16");
        assert!(e.contains("山田") && e.contains("r.HairStrands.Streaming=0"), "{e}");
        assert!(matches!(status(&env).unwrap(), Status::Installed { ref modified, .. } if modified.is_empty()));
        uninstall(&env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&env), before);
    }

    /// A small package: the template, the proxy, and `files` (Win64-relative path, bytes).
    fn pkg(version: &str, files: &[(&str, &[u8])]) -> (Manifest, Files) {
        let mut m = sample();
        m.files.clear();
        let mut fs = Files::new();
        let mut add = |m: &mut Manifest, path: String, bytes: &[u8], role: Role, install: Option<String>| {
            m.files.push(FileEntry { path: path.clone(), sha256: util::sha256_hex(bytes), size: bytes.len() as u64, role, install });
            fs.insert(path, bytes.to_vec());
        };
        add(&mut m, "payload/mods.release.txt".into(), b"HSMPMenu : 1\nKeybinds : dev\n", Role::Template, None);
        for (rel, b) in files {
            add(&mut m, format!("payload/Win64/{rel}"), b, if rel.contains("Mods/") { Role::Mod } else { Role::Ue4ss }, Some(format!("{W}/{rel}")));
        }
        m.version = version.into();
        m.validate().unwrap();
        (m, fs)
    }

    /// Names as they are on disk (case-sensitive), for exact-case checks.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    /// Release v1 ships Menu_ui.lua, v2 menu_ui.lua, v3 drops it.
    #[test]
    fn case_only_rename_between_releases() {
        let f = fake(false);
        let before = snap_all(&f.env);
        let s = game::win64(&f.env.game_root).join("ue4ss/Mods/HSMPMenu/Scripts");
        let (m1, f1) = pkg("0.1.0", &[("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- main"), ("ue4ss/Mods/HSMPMenu/Scripts/Menu_ui.lua", b"-- ui v1")]);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        let (m2, f2) = pkg("0.2.0", &[("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- main"), ("ue4ss/Mods/HSMPMenu/Scripts/menu_ui.lua", b"-- ui v2")]);
        install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        assert_eq!(names(&s), vec!["main.lua", "menu_ui.lua"], "the on-disk name follows the release");
        let st = load_state(&f.env).unwrap().unwrap();
        let rec = &st.files[&util::fold(&format!("{W}/ue4ss/Mods/HSMPMenu/Scripts/menu_ui.lua"))];
        assert_eq!(rec.original, None, "the original (absent) is carried forward, not HSMP's v1 bytes");
        assert_eq!(st.files.len(), 5, "one record per file: main.lua, menu_ui.lua, mods.txt, hsmp.cfg, marker");
        assert!(matches!(status(&f.env).unwrap(), Status::Installed { ref modified, ref missing, .. } if modified.is_empty() && missing.is_empty()), "not 'modified forever'");
        let (m3, f3) = pkg("0.3.0", &[("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- main")]);
        let rep = install(&f.env, &m3, &f3, &opts(), &mut quiet()).unwrap();
        assert_eq!(rep.files_removed, 1);
        assert_eq!(names(&s), vec!["main.lua"], "dropped file removed, HSMP's v1 bytes NOT restored");
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
    }

    /// The player's own DWMAPI.dll (upper case) survives any case the releases use.
    #[test]
    fn preexisting_file_in_other_case_is_restored_exactly() {
        let f = fake(false);
        let w = game::win64(&f.env.game_root);
        std::fs::write(w.join("DWMAPI.dll"), b"their proxy").unwrap();
        let before = snap_all(&f.env);
        let (m1, f1) = pkg("0.1.0", &[("dwmapi.dll", b"pinned proxy"), ("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- m")]);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        assert!(names(&w).contains(&"dwmapi.dll".to_string()));
        let (m2, f2) = pkg("0.2.0", &[("DWMAPI.dll", b"pinned proxy 2"), ("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- m")]);
        install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        let st = load_state(&f.env).unwrap().unwrap();
        let rec = &st.files[&util::fold(&format!("{W}/dwmapi.dll"))];
        assert_eq!(rec.original.as_deref(), Some(util::sha256_hex(b"their proxy").as_str()));
        assert_eq!(rec.original_path.as_deref(), Some(format!("{W}/DWMAPI.dll").as_str()));
        // a failed update rolls the name case back too
        let (m3, f3) = pkg("0.3.0", &[("dwmapi.dll", b"pinned proxy 3"), ("ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- m")]);
        let o = InstallOpts { fail_after_writes: Some(1), ..opts() };
        install(&f.env, &m3, &f3, &o, &mut quiet()).unwrap_err();
        assert!(names(&w).contains(&"DWMAPI.dll".to_string()), "rollback restores the name case: {:?}", names(&w));
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
        assert!(names(&w).contains(&"DWMAPI.dll".to_string()), "byte-exact AND name-exact: {:?}", names(&w));
    }

    /// A journal written by the old launcher with a case-split pair is merged correctly.
    #[test]
    fn v1_journal_case_split_is_migrated() {
        let f = fake(false);
        let before = snap_all(&f.env);
        let (m1, f1) = pkg("0.1.0", &[("ue4ss/Mods/HSMPMenu/Scripts/Menu_ui.lua", b"-- ui v1")]);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        // what the v1 launcher wrote after an update shipping menu_ui.lua
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(f.env.state_path()).unwrap()).unwrap();
        let upper = format!("{W}/ue4ss/Mods/HSMPMenu/Scripts/Menu_ui.lua");
        let lower = format!("{W}/ue4ss/Mods/HSMPMenu/Scripts/menu_ui.lua");
        let v1_blob = put_blob(&f.env, b"-- ui v1").unwrap();
        let files = v["files"].as_object_mut().unwrap();
        let old: Vec<(String, serde_json::Value)> = files.iter().map(|(k, x)| (k.clone(), x.clone())).collect();
        files.clear();
        for (_, mut rec) in old {
            let p = rec["path"].as_str().unwrap().to_string();
            rec.as_object_mut().unwrap().remove("path");
            files.insert(p, rec);
        }
        files.insert(lower.clone(), serde_json::json!({ "original": v1_blob, "installed": util::sha256_hex(b"-- ui v2"), "kind": "copy" }));
        v["schema"] = 1.into();
        std::fs::write(f.env.state_path(), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        std::fs::write(f.env.abs(&upper), b"-- ui v2").unwrap();
        let st = load_state(&f.env).unwrap().unwrap();
        let rec = &st.files[&util::fold(&upper)];
        assert_eq!(rec.original, None, "the true original (absent) wins");
        assert_eq!(rec.path, lower, "path of the later write");
        assert_eq!(rec.installed, util::sha256_hex(b"-- ui v2"));
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before, "HSMP's v1 bytes are not 'restored'");
    }

    fn move_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(from, to).unwrap();
    }

    /// Steam "Move install folder": the journal follows (marker), uninstall is exact.
    #[test]
    fn journal_follows_a_moved_library() {
        let f = fake(true);
        let before = snapshot(&f.env.game_root);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let marker = read_marker(&f.env.game_root).unwrap();
        assert_eq!(marker.steam_appid, 2397300);
        assert_eq!(marker.exe_sha256, util::sha256_hex(b"exe"));
        let new_root = f._t.path().join("Other Library").join("steamapps/common/Half Sword");
        move_dir(&f.env.game_root, &new_root);
        let env2 = Env::new(&new_root, f.env.hsmp_home.clone(), f.env.ue_saved.clone());
        assert_eq!(env2.store_key(), marker.install_id);
        assert!(matches!(status(&env2).unwrap(), Status::Installed { ref modified, ref missing, .. } if modified.is_empty() && missing.is_empty()));
        let st = load_state(&env2).unwrap().unwrap();
        assert!(same_root(Path::new(&st.game_root), &new_root));
        assert_eq!(st.previous_roots.len(), 1);
        let st = st.identity.unwrap();
        assert_eq!((st.steam_appid, st.exe_sha256.as_str()), (2397300, util::sha256_hex(b"exe").as_str()));
        // an update in the new place reports the move
        let (m2, f2) = package(1);
        let rep = install(&env2, &m2, &f2, &opts(), &mut quiet()).unwrap();
        assert!(rep.notes.iter().any(|n| n.contains("moved")), "{:?}", rep.notes);
        uninstall(&env2, false, &mut quiet()).unwrap();
        assert_eq!(snapshot(&new_root), before, "uninstall after the move is byte-exact");
        assert!(!env2.store().exists());
    }

    /// A move done before markers existed (or a deleted marker) is found by
    /// scanning journals whose folder is gone, and verified by file hashes.
    #[test]
    fn moved_install_without_marker_is_found_and_verified() {
        let f = fake(false);
        let before = snapshot(&f.env.game_root);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        std::fs::remove_file(f.env.abs(MARKER_REL)).unwrap();
        let new_root = f._t.path().join("D_drive").join("Half Sword");
        move_dir(&f.env.game_root, &new_root);
        let env2 = Env::new(&new_root, f.env.hsmp_home.clone(), f.env.ue_saved.clone());
        assert!(matches!(status(&env2).unwrap(), Status::Installed { .. }));
        uninstall(&env2, false, &mut quiet()).unwrap();
        assert_eq!(snapshot(&new_root), before);
    }

    /// A COPY of the game keeps the journal with the original folder.
    #[test]
    fn copied_game_folder_does_not_take_the_journal() {
        let f = fake(false);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let copy = f._t.path().join("Copy").join("Half Sword");
        util::copy_tree(&f.env.game_root, &copy).unwrap();
        let before_copy = snapshot(&copy);
        let env2 = Env::new(&copy, f.env.hsmp_home.clone(), f.env.ue_saved.clone());
        assert_ne!(env2.store_key(), f.env.store_key());
        assert!(matches!(status(&env2).unwrap(), Status::NotInstalled { foreign_hsmp: true, .. }));
        assert!(uninstall(&env2, false, &mut quiet()).unwrap_err().contains("not installed"));
        // installing into the copy gets its own journal; uninstalling it restores the copy exactly
        install(&env2, &m, &files, &opts(), &mut quiet()).unwrap();
        uninstall(&env2, false, &mut quiet()).unwrap();
        assert_eq!(snapshot(&copy), before_copy);
        assert!(matches!(status(&f.env).unwrap(), Status::Installed { .. }), "the original install is untouched");
    }

    /// Verify before acting: a journal is never applied to a folder it does not describe.
    #[test]
    fn journal_for_a_different_folder_is_refused() {
        let f = fake(false);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let new_root = f._t.path().join("Moved").join("Half Sword");
        move_dir(&f.env.game_root, &new_root);
        // the files there are not HSMP's (e.g. Steam re-downloaded a clean game into the folder)
        for rec in load_state_raw(&f.env).files.values().filter(|r| r.kind == "copy") {
            std::fs::write(util::join_rel(&new_root, &rec.path), b"something else").unwrap();
        }
        let env2 = Env::new(&new_root, f.env.hsmp_home.clone(), f.env.ue_saved.clone());
        let snap = snapshot(&new_root);
        let e = uninstall(&env2, false, &mut quiet()).unwrap_err();
        assert!(e.contains("does not match") && e.contains("Nothing was changed"), "{e}");
        assert_eq!(snapshot(&new_root), snap, "nothing was changed");
    }

    fn load_state_raw(env: &Env) -> State {
        let mut st: State = serde_json::from_slice(&std::fs::read(env.state_path()).unwrap()).unwrap();
        migrate(&mut st);
        st
    }

    /// Journals of the old launcher (literal-path key) are still found.
    #[test]
    fn legacy_literal_path_store_is_found() {
        let t = TempDir::new("install_legacy");
        let root = t.path().join("Half Sword ÄÖ");
        let env = Env::new(&root, t.path().join("LocalAppData/HSMP"), t.path().join("LocalAppData/HalfSwordUE5/Saved"));
        let w = game::win64(&root);
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join(game::EXE_NAME), b"exe").unwrap();
        let before = snapshot(&root);
        let (m, files) = package(0);
        install(&env, &m, &files, &opts(), &mut quiet()).unwrap();
        let lk = legacy_key(&root);
        assert_ne!(lk, env.store_key(), "non-ASCII upper case: the keys differ");
        std::fs::rename(env.store(), env.launcher_dir().join(&lk)).unwrap();
        std::fs::remove_file(env.abs(MARKER_REL)).unwrap();
        let env2 = Env::new(&root, env.hsmp_home.clone(), env.ue_saved.clone());
        assert_eq!(env2.store_key(), lk);
        uninstall(&env2, false, &mut quiet()).unwrap();
        assert_eq!(snapshot(&root), before);
    }

    fn proxy_blob(env: &Env) -> (PathBuf, Vec<u8>) {
        let st = load_state_raw(env);
        let sha = st.files[&util::fold(&format!("{W}/dwmapi.dll"))].original.clone().unwrap();
        let p = env.blob_path(&sha);
        let b = std::fs::read(&p).unwrap();
        (p, b)
    }

    /// The antivirus quarantined the backed-up dwmapi.dll: uninstall does
    /// everything else, keeps exactly that file in the journal, explains, and
    /// finishes once the backup is back.
    #[test]
    fn quarantined_backup_is_skipped_then_resumed() {
        let f = fake(true);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let (blob, bytes) = proxy_blob(&f.env);
        assert!(!bytes.windows(b"their proxy".len()).any(|x| x == b"their proxy"), "blobs are masked (not scannable as the original file)");
        std::fs::remove_file(&blob).unwrap(); // quarantined
        let e = uninstall(&f.env, false, &mut quiet()).unwrap_err();
        assert!(e.contains("dwmapi.dll") && e.contains("Uninstall again") && e.contains("--forget-missing") && e.contains("quarantine"), "{e}");
        match status(&f.env).unwrap() {
            Status::UninstallIncomplete { remaining } => assert_eq!(remaining, vec![format!("{W}/dwmapi.dll")]),
            s => panic!("{s:?}"),
        }
        let w = game::win64(&f.env.game_root);
        assert_eq!(std::fs::read(w.join("ue4ss/UE4SS.dll")).unwrap(), b"their ue4ss", "everything else is already undone");
        assert!(!f.env.ini_path("Engine").exists() || !std::fs::read_to_string(f.env.ini_path("Engine")).unwrap().contains("HairStrands"));
        assert!(reapply_ini(&f.env, &mut quiet()).is_err(), "launch is refused while half uninstalled");
        // the second run without the backup: same answer, nothing breaks
        assert!(uninstall(&f.env, false, &mut quiet()).is_err());
        std::fs::write(&blob, &bytes).unwrap(); // restored from quarantine
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
    }

    /// The backup is gone for good: --forget-missing finishes the uninstall.
    #[test]
    fn forget_missing_finishes_the_uninstall() {
        let f = fake(true);
        let (before_game, before_saved) = snap_all(&f.env);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let (blob, _) = proxy_blob(&f.env);
        std::fs::remove_file(&blob).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap_err();
        let rep = uninstall_with(&f.env, &UninstallOpts { check_processes: false, forget_missing: true }, &mut quiet()).unwrap();
        assert!(rep.notes.iter().any(|n| n.contains("dwmapi.dll") && n.contains("removed")), "{:?}", rep.notes);
        let mut want = before_game.clone();
        want.remove(&format!("{W}/dwmapi.dll"));
        assert_eq!(snapshot(&f.env.game_root), want, "all but the lost file restored; HSMP's proxy removed");
        assert_eq!(snapshot(&f.env.ue_saved), before_saved);
        assert!(!f.env.store().exists());
    }

    /// Blobs from older launchers (unmasked) still restore.
    #[test]
    fn unmasked_legacy_blob_still_restores() {
        let f = fake(true);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let (blob, masked) = proxy_blob(&f.env);
        let sha = blob.file_stem().unwrap().to_string_lossy().to_string();
        std::fs::remove_file(&blob).unwrap();
        std::fs::write(f.env.legacy_blob_path(&sha), mask(&masked)).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(snap_all(&f.env), before);
    }

    /// Low: a mods.txt the player edited after install is kept (archived) on uninstall.
    #[test]
    fn user_edits_are_kept_on_uninstall() {
        let f = fake(true);
        let before = snap_all(&f.env);
        let (m, files) = package(0);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        let mods = f.env.abs(MODS_TXT_REL);
        let mut t = std::fs::read(&mods).unwrap();
        t.extend_from_slice(b"MyNewMod : 1\r\n");
        std::fs::write(&mods, &t).unwrap();
        let rep = uninstall(&f.env, false, &mut quiet()).unwrap();
        let note = rep.notes.iter().find(|n| n.contains("mods.txt")).unwrap_or_else(|| panic!("{:?}", rep.notes));
        assert!(note.contains("changed_by_you"), "{note}");
        let kept = rep.archived_to.unwrap().join("changed_by_you").join(W).join("ue4ss/Mods/mods.txt");
        assert_eq!(std::fs::read(kept).unwrap(), t);
        assert_eq!(snap_all(&f.env), before);
    }

    /// Low: downgrade protection and Steam "updating" block the install.
    #[test]
    fn downgrade_and_steam_updating_are_refused() {
        let f = fake(false);
        let (m2, f2) = package(1); // 0.2.0
        install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        let snap = snap_all(&f.env);
        let (m1, f1) = package(0); // 0.1.0
        assert!(install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap_err().contains("OLDER"));
        assert_eq!(snap_all(&f.env), snap);
        let o = InstallOpts { steam_updating: true, ..opts() };
        assert!(install(&f.env, &m2, &f2, &o, &mut quiet()).unwrap_err().contains("Steam is still"));
        let o = InstallOpts { allow_downgrade: true, ..opts() };
        install(&f.env, &m1, &f1, &o, &mut quiet()).unwrap();
        assert_eq!(load_state(&f.env).unwrap().unwrap().version, "0.1.0");
    }

    /// Low: one install/uninstall at a time (GUI and CLI share the lock).
    #[test]
    fn operations_are_mutually_exclusive() {
        let f = fake(false);
        let held = lock(&f.env.hsmp_home).unwrap();
        let (m, files) = package(0);
        if cfg!(windows) {
            assert!(install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap_err().contains("another HSMP launcher"));
            assert!(lock(&f.env.hsmp_home).is_err());
        }
        drop(held);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
    }

    /// Low: a pending transaction is not rolled back while something blocks
    /// the install (recover runs after the checks).
    #[test]
    fn recovery_waits_for_blockers() {
        let f = fake(false);
        let (m, files) = package(0);
        let mut t = Txn { env: &f.env, st: State::default(), tx: Tx::default(), writes: 0, fail_after: None, player_edits: vec![] };
        for (fe, target) in m.installed_files().take(2) {
            t.install_file(target, &files[&fe.path], "copy").unwrap();
        }
        drop(t);
        let exe = game::exe_path(&f.env.game_root);
        std::fs::rename(&exe, exe.with_extension("away")).unwrap(); // "not a game folder" blocker
        let snap = snapshot(&f.env.game_root);
        install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap_err();
        assert_eq!(snapshot(&f.env.game_root), snap, "nothing touched while blocked");
        assert_eq!(status(&f.env).unwrap(), Status::Interrupted);
    }

    /// Career saves: no install / update / failure / uninstall path writes them
    /// (bytes AND modification times are unchanged).
    #[test]
    fn career_saves_are_never_written() {
        let f = fake(true);
        std::fs::write(f.env.save_dir().join("Settings.sav"), b"settings").unwrap();
        let stamp = |env: &Env| -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
            let mut v: Vec<_> = std::fs::read_dir(env.save_dir())
                .unwrap()
                .flatten()
                .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap(), e.metadata().unwrap().modified().unwrap()))
                .collect();
            v.sort();
            v
        };
        let s0 = stamp(&f.env);
        let (m1, f1) = package(0);
        let (m2, f2) = package(1);
        install(&f.env, &m1, &f1, &opts(), &mut quiet()).unwrap();
        install(&f.env, &m2, &f2, &InstallOpts { fail_after_writes: Some(2), ..opts() }, &mut quiet()).unwrap_err();
        install(&f.env, &m2, &f2, &opts(), &mut quiet()).unwrap();
        reapply_ini(&f.env, &mut quiet()).unwrap();
        let (blob, bytes) = proxy_blob(&f.env);
        std::fs::remove_file(&blob).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap_err();
        std::fs::write(&blob, &bytes).unwrap();
        uninstall(&f.env, false, &mut quiet()).unwrap();
        assert_eq!(stamp(&f.env), s0);
        // and the only code that restores saves is the explicit, confirmed restore
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut callers = vec![];
        let needle = format!("{}::{}(", "saves", "restore");
        for e in std::fs::read_dir(&src).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            if text.contains(&needle) {
                callers.push(e.file_name().to_string_lossy().to_string());
            }
        }
        callers.sort();
        assert_eq!(callers, vec!["gui.rs", "main.rs"], "restore-saves (CLI command) and the GUI's confirmed Restore only");
    }

    #[test]
    fn foreign_install_detected() {
        let f = fake(true);
        match status(&f.env).unwrap() {
            Status::NotInstalled { foreign_hsmp, foreign_ue4ss } => {
                assert!(!foreign_hsmp);
                assert!(foreign_ue4ss);
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn enabled_txt_warning() {
        let f = fake(true);
        let w = game::win64(&f.env.game_root);
        std::fs::create_dir_all(w.join("ue4ss/Mods/HSMPDiag")).unwrap();
        std::fs::write(w.join("ue4ss/Mods/HSMPDiag/enabled.txt"), b"").unwrap();
        let (m, files) = package(0);
        let rep = install(&f.env, &m, &files, &opts(), &mut quiet()).unwrap();
        assert!(rep.notes.iter().any(|n| n.contains("HSMPDiag")), "{:?}", rep.notes);
    }
}

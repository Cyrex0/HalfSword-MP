//! Server mods, the sidecar's half (docs/hosting/server-mods.md, docs/development/protocol.md
//! "Server mods").
//!
//! ```text
//! server                        sidecar (this module)                        game
//! welcome (caps SERVER_MODS) -> session records held back from the game
//! mod_manifest + mod_files   -> rebuilt and checked (manifest::from_payloads);
//!                               cached mods verified                      -> mod_offer, mod_entry x N,
//!                                                                            mod_progress OFFER
//!                                                                         <- mod_decision ACCEPT / DECLINE
//! mod_chunk_req (window 4)   <- pull the missing files, resumable by (file, offset)
//! mod_chunk                  -> file complete: SHA-256 checked; mod complete:
//!                               mods_cache::commit (verify, temp, rename)  -> mod_progress DOWNLOADING
//!                                                                         -> mod_progress READY
//!                                                                         <- mod_loaded (HSMPModHost)
//! mod_ready LOADED           <- the held session reaches the game          -> mod_progress JOINED
//! ```
//!
//! The consent is the game's (HSMPMenu): the sidecar downloads nothing before a
//! `mod_decision` ACCEPT for exactly the offered set hash, and the game loads nothing before
//! `mod_progress` READY. Only names, sizes, hashes and text cross into shared memory; paths
//! and bytes stay between the server, this module and the cache.

use super::server_mods::manifest::{self, Announced, Limits};
use super::*;
use hsmp_ipc::layout::Str;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::mods::{self as rec, mod_error, mod_result, mod_state};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Chunk requests in flight (32 KiB each: 128 KiB, well inside the transport's 256 KiB).
pub(crate) const WINDOW: usize = 4;
/// A request unanswered this long is asked again (the server may have refused it).
const REQ_TIMEOUT_MS: u64 = 10_000;
/// A server that offered SERVER_MODS must send its manifest within this long.
const MANIFEST_WAIT_MS: u64 = 15_000;
const TICK: Duration = Duration::from_millis(50);
const PROGRESS_EVERY_MS: u64 = 250;

// ---- the transfer (sans-IO) -----------------------------------------------------------------

/// Why a transfer failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum XferError {
    /// A complete file did not match its SHA-256 (path).
    HashMismatch(String),
    /// The server answered a request with other bytes than asked for.
    Protocol(String),
}

struct FileX {
    size: u64,
    sha256: [u8; 32],
    mod_idx: usize,
    path: String,
    /// Allocated at the first request of the file.
    buf: Vec<u8>,
    got: Vec<bool>,
    n_got: usize,
    verified: bool,
}

struct Outstanding {
    file: usize,
    idx: usize,
    at: u64,
}

/// Pulls the files of the mods that are not cached, chunk by chunk, with at most [`WINDOW`]
/// requests in flight; checks each file's SHA-256 as soon as it is complete; hands over a
/// mod's files when all of them are verified. A reconnect only forgets the requests in flight:
/// the next ones resume at the first missing (file, offset).
pub(crate) struct Transfer {
    /// The set (the tests read it back).
    #[cfg_attr(not(test), allow(dead_code))]
    pub ann: Announced,
    chunk: u64,
    /// One entry per `mod_files` row; None = its mod is cached.
    files: Vec<Option<FileX>>,
    out: HashMap<u32, Outstanding>,
    next_id: u32,
    done: u64,
    total: u64,
    mod_done: Vec<bool>,
    mod_taken: Vec<bool>,
}

impl Transfer {
    pub(crate) fn new(ann: Announced, cached: &[bool]) -> Result<Transfer, XferError> {
        let chunk = (ann.max_chunk.clamp(1, rec::MAX_CHUNK as u32)) as u64;
        let mut files = Vec::new();
        let mut total = 0;
        for (mi, f) in ann.manifest.flat() {
            if cached.get(mi).copied().unwrap_or(false) {
                files.push(None);
                continue;
            }
            total += f.size;
            let n = f.size.div_ceil(chunk) as usize;
            let mut x = FileX { size: f.size, sha256: f.sha256, mod_idx: mi, path: f.path.clone(), buf: Vec::new(), got: vec![false; n], n_got: 0, verified: false };
            if n == 0 {
                if manifest::sha256(&[]) != f.sha256 {
                    return Err(XferError::HashMismatch(f.path.clone()));
                }
                x.verified = true;
            }
            files.push(Some(x));
        }
        let n_mods = ann.manifest.mods.len();
        let mut t = Transfer { ann, chunk, files, out: HashMap::new(), next_id: 1, done: 0, total, mod_done: vec![false; n_mods], mod_taken: vec![false; n_mods] };
        for m in 0..n_mods {
            t.mod_done[m] = t.mod_complete(m);
        }
        Ok(t)
    }

    fn mod_complete(&self, m: usize) -> bool {
        self.files.iter().flatten().filter(|f| f.mod_idx == m).all(|f| f.verified)
    }

    /// The next requests: stale ones again, then the first missing chunks, up to the window.
    pub(crate) fn requests(&mut self, now: u64) -> Vec<rec::ModChunkReq> {
        self.out.retain(|_, o| now.saturating_sub(o.at) < REQ_TIMEOUT_MS);
        let mut v = Vec::new();
        'files: for fi in 0..self.files.len() {
            let Some(f) = self.files[fi].as_mut() else { continue };
            if f.verified {
                continue;
            }
            for idx in 0..f.got.len() {
                if self.out.len() >= WINDOW {
                    break 'files;
                }
                if f.got[idx] || self.out.values().any(|o| o.file == fi && o.idx == idx) {
                    continue;
                }
                if f.buf.is_empty() {
                    f.buf = vec![0; f.size as usize];
                }
                let offset = idx as u64 * self.chunk;
                let len = self.chunk.min(f.size - offset) as u32;
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                self.out.insert(id, Outstanding { file: fi, idx, at: now });
                v.push(rec::ModChunkReq { offset, req_id: id, len, file: fi as u16, _r: [0; 6] });
            }
        }
        v
    }

    /// One `mod_chunk`. Returns the mods that just became complete (every file verified).
    pub(crate) fn on_chunk(&mut self, h: &rec::ModChunkHead, bytes: &[u8]) -> Result<Vec<usize>, XferError> {
        // A late or duplicate answer (an old connection's) is ignored.
        let Some(o) = self.out.remove(&h.req_id) else { return Ok(Vec::new()) };
        let Some(f) = self.files.get_mut(o.file).and_then(|f| f.as_mut()) else {
            return Err(XferError::Protocol("a chunk for a cached file".into()));
        };
        let offset = o.idx as u64 * self.chunk;
        let want = self.chunk.min(f.size - offset) as usize;
        if h.file as usize != o.file || h.offset != offset || bytes.len() != want {
            return Err(XferError::Protocol(format!("{}: the server answered with other bytes than asked for", f.path)));
        }
        if !f.got[o.idx] {
            f.buf[offset as usize..offset as usize + want].copy_from_slice(bytes);
            f.got[o.idx] = true;
            f.n_got += 1;
            self.done += want as u64;
        }
        let mut newly = Vec::new();
        if f.n_got == f.got.len() && !f.verified {
            if manifest::sha256(&f.buf) != f.sha256 {
                let p = f.path.clone();
                f.buf = Vec::new();
                return Err(XferError::HashMismatch(p));
            }
            f.verified = true;
            let m = f.mod_idx;
            if !self.mod_done[m] && self.mod_complete(m) {
                self.mod_done[m] = true;
                newly.push(m);
            }
        }
        Ok(newly)
    }

    /// The connection was replaced: forget the requests in flight (they are asked again).
    pub(crate) fn on_reconnect(&mut self) {
        self.out.clear();
    }

    /// The verified files of a complete mod, in its file order (moved out; once).
    pub(crate) fn take_mod(&mut self, m: usize) -> Option<Vec<Vec<u8>>> {
        if !self.mod_done.get(m).copied().unwrap_or(false) || self.mod_taken[m] {
            return None;
        }
        self.mod_taken[m] = true;
        Some(self.files.iter_mut().flatten().filter(|f| f.mod_idx == m).map(|f| std::mem::take(&mut f.buf)).collect())
    }

    /// Mods complete before any download (cached, or only empty files).
    pub(crate) fn complete_mods(&self) -> Vec<usize> {
        (0..self.mod_done.len()).filter(|m| self.mod_done[*m] && !self.mod_taken[*m]).collect()
    }

    pub(crate) fn progress(&self) -> (u64, u64) {
        (self.done, self.total)
    }

    pub(crate) fn complete(&self) -> bool {
        self.mod_done.iter().all(|d| *d)
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.out.len()
    }
}

// ---- the client --------------------------------------------------------------------------

/// Session records are held back from the game while the server's mods are not loaded.
static HOLD: AtomicBool = AtomicBool::new(false);

pub(crate) fn holds_session() -> bool {
    HOLD.load(Ordering::Acquire)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No server mods (or not connected yet).
    Idle,
    /// The welcome offered SERVER_MODS: the manifest comes next.
    AwaitManifest,
    /// The offer is with the player.
    Offered,
    Downloading,
    /// In the cache: HSMPModHost loads the set.
    Ready,
    /// The server knows: the session runs.
    Loaded,
    /// Failed, declined or blocked: the game leaves.
    Failed,
}

enum Input {
    Connected([u8; 32]),
    LinkDown,
    Welcome { epoch: u64, caps: u64 },
    Server(u16, Vec<u8>),
    Game(u16, Vec<u8>),
}

static INPUT: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<Input>> = std::sync::OnceLock::new();

fn send_input(i: Input) {
    if let Some(tx) = INPUT.get() {
        let _ = tx.send(i);
    }
}

/// The transport connected (the server identity for the consent key).
pub(crate) fn on_connected(server_key: [u8; 32]) {
    // Welcome may be delivered before the transfer task handles Connected.
    // The fast-path epoch is meaningful only for the same authenticated server.
    static KEY: std::sync::Mutex<[u8;32]> = std::sync::Mutex::new([0;32]);
    let mut key=KEY.lock().unwrap_or_else(|e|e.into_inner());
    if *key != server_key { LOADED_EPOCH.set(None); *key=server_key; }
    send_input(Input::Connected(server_key));
}

/// The connection ended (a reconnect follows).
pub(crate) fn on_link_down() {
    send_input(Input::LinkDown);
}

/// A `welcome`. Called on the event task before any later record, so the hold is in place
/// before the first session snapshot of this connection.
pub(crate) fn on_welcome(epoch: u64, caps: u64) {
    if caps & hsmp_net::net::caps::SERVER_MODS != 0 && !LOADED_EPOCH.with(|e| e == Some(epoch)) {
        HOLD.store(true, Ordering::Release);
    }
    send_input(Input::Welcome { epoch, caps });
}

/// The set loaded on this server instance (epoch): a resume keeps it without a new offer.
struct LoadedEpoch(std::sync::Mutex<Option<u64>>);
impl LoadedEpoch {
    fn with<R>(&self, f: impl FnOnce(Option<u64>) -> R) -> R {
        f(*self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
    fn set(&self, v: Option<u64>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = v;
    }
}
static LOADED_EPOCH: LoadedEpoch = LoadedEpoch(std::sync::Mutex::new(None));

/// A server record of the mods domain (S2C).
pub(crate) fn on_server_record(kind: u16, payload: &[u8]) {
    send_input(Input::Server(kind, payload.to_vec()));
}

/// A game record of the mods domain (G2S): the decision, the loaded report.
pub(crate) fn on_game_record(kind: u16, payload: &[u8]) {
    send_input(Input::Game(kind, payload.to_vec()));
}

struct Client {
    root: PathBuf,
    /// The session hold and the loaded epoch (statics; tests use their own).
    hold: &'static AtomicBool,
    loaded_epoch: &'static LoadedEpoch,
    phase: Phase,
    since: Instant,
    server_key: [u8; 32],
    epoch: u64,
    pend_manifest: Option<Vec<u8>>,
    pend_files: Option<Vec<u8>>,
    ann: Option<Announced>,
    cached: Vec<bool>,
    xfer: Option<Transfer>,
    loaded: Option<[u8; 32]>,
    progress_at: Option<Instant>,
    last_progress: Option<(u8, u64)>,
    error: (u8, String),
}

/// What the client wants done (the task performs it).
#[derive(Debug, Clone, PartialEq)]
enum Act {
    /// A framed message to the server.
    Net(Vec<u8>),
    /// A record for the game (S2G).
    Game(u16, Vec<u8>),
    /// The held session snapshot reaches the game.
    Release,
}

fn now_ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

impl Client {
    fn new(root: PathBuf, hold: &'static AtomicBool, loaded_epoch: &'static LoadedEpoch) -> Client {
        Client {
            root, hold, loaded_epoch, phase: Phase::Idle, since: Instant::now(), server_key: [0; 32], epoch: 0, pend_manifest: None, pend_files: None,
            ann: None, cached: Vec::new(), xfer: None, loaded: None, progress_at: None, last_progress: None,
            error: (mod_error::NONE, String::new()),
        }
    }

    fn set_hash(&self) -> [u8; 32] {
        self.ann.as_ref().map_or([0; 32], |a| a.manifest.set_hash)
    }

    fn progress(&self, state: u8) -> Act {
        let (done, total) = match (&self.xfer, &self.ann) {
            (Some(x), _) => x.progress(),
            (None, Some(a)) => (0, a.manifest.total_bytes()),
            _ => (0, 0),
        };
        let mut p = rec::ModProgress { set_hash: self.set_hash(), done_bytes: done, total_bytes: total, state, code: 0, _r: [0; 6], text: Str::default() };
        if state == mod_state::FAILED {
            p.code = self.error.0;
            p.text = Str::new(&self.error.1);
        }
        Act::Game(rec::K_MOD_PROGRESS, hsmp_ipc::bytemuck::bytes_of(&p).to_vec())
    }

    fn ready_msg(&self, result: u8, failed: u8) -> Act {
        Act::Net(hsmp_ipc::wire::encode(0, 0, &rec::ModReady { set_hash: self.set_hash(), result, failed, _r: [0; 6] }, &[]))
    }

    fn fail(&mut self, code: u8, text: String, out: &mut Vec<Act>) {
        warn!(code, %text, "server mods failed");
        events::emit("server_mods", serde_json::json!({"state": "failed", "code": code, "text": text}));
        self.error = (code, text);
        self.phase = Phase::Failed;
        self.xfer = None;
        let result = if code == mod_error::DECLINED || code == mod_error::BLOCKED { mod_result::DECLINED } else { mod_result::FAILED };
        if self.ann.is_some() {
            out.push(self.ready_msg(result, 0));
        }
        out.push(self.progress(mod_state::FAILED));
    }

    /// The offer for the game: the set, one entry per mod, the OFFER state.
    fn offer(&self, out: &mut Vec<Act>) {
        let Some(a) = &self.ann else { return };
        let m = &a.manifest;
        let cached_bytes: u64 = m.mods.iter().zip(&self.cached).filter(|(_, c)| **c).map(|(m, _)| m.bytes()).sum();
        let o = rec::ModOffer {
            set_hash: m.set_hash, server_key: self.server_key, total_bytes: m.total_bytes(), cached_bytes,
            n: m.mods.len() as u16, files: m.file_count() as u16, _r: 0,
        };
        out.push(Act::Game(rec::K_MOD_OFFER, hsmp_ipc::bytemuck::bytes_of(&o).to_vec()));
        for (i, md) in m.mods.iter().enumerate() {
            let e = rec::ModEntry {
                set_hash: m.set_hash, mod_hash: md.hash, bytes: md.bytes(), index: i as u16, n: m.mods.len() as u16,
                files: md.files.len() as u16, cached: self.cached.get(i).copied().unwrap_or(false).into(), _r: 0,
                name: Str::new(&md.name), version: Str::new(&md.version), author: Str::new(&md.author), description: Str::new(&md.description),
            };
            out.push(Act::Game(rec::K_MOD_ENTRY, hsmp_ipc::bytemuck::bytes_of(&e).to_vec()));
        }
    }

    fn state_code(&self) -> u8 {
        match self.phase {
            Phase::Offered => mod_state::OFFER,
            Phase::Downloading => mod_state::DOWNLOADING,
            Phase::Ready => mod_state::READY,
            Phase::Loaded => mod_state::JOINED,
            Phase::Failed => mod_state::FAILED,
            Phase::Idle | Phase::AwaitManifest => mod_state::CLEAR,
        }
    }

    fn handle(&mut self, i: Input, now: u64, out: &mut Vec<Act>) {
        match i {
            Input::Connected(k) => {
                if self.server_key != k {
                    // Consent belongs to server identity AND content set. Identical
                    // bytes on a different server still require its own decision.
                    if self.phase != Phase::Idle { out.push(self.progress(mod_state::CLEAR)); }
                    self.phase=Phase::Idle;
                    self.ann=None;self.xfer=None;self.loaded=None;
                    self.cached.clear();self.pend_manifest=None;self.pend_files=None;
                    self.loaded_epoch.set(None);
                    self.error=(mod_error::NONE,String::new());
                    self.progress_at=None;self.last_progress=None;
                }
                self.server_key=k;
            },
            Input::LinkDown => {
                if let Some(x) = self.xfer.as_mut() {
                    x.on_reconnect();
                }
            }
            Input::Welcome { epoch, caps } => {
                let has = caps & hsmp_net::net::caps::SERVER_MODS != 0;
                let same_server = epoch == self.epoch;
                self.epoch = epoch;
                if !has {
                    // A server without mods: nothing to hold; a loaded set is unloaded.
                    if self.loaded.is_some() || self.phase != Phase::Idle {
                        info!("this server has no mods: the loaded server mods are unloaded");
                        self.phase = Phase::Idle;
                        self.ann = None;
                        self.xfer = None;
                        self.loaded = None;
                        self.loaded_epoch.set(None);
                        out.push(self.progress(mod_state::CLEAR));
                    }
                    self.hold.store(false, Ordering::Release);
                    out.push(Act::Release);
                    return;
                }
                match self.phase {
                    Phase::Loaded if same_server => {}
                    Phase::Offered | Phase::Downloading | Phase::Ready | Phase::Failed => {
                        // A resume: the server sends the manifest again; the transfer goes on.
                        if let Some(x) = self.xfer.as_mut() { x.on_reconnect(); }
                    }
                    _ => {
                        self.phase = Phase::AwaitManifest;
                        self.since = Instant::now();
                    }
                }
                self.pend_manifest = None;
                self.pend_files = None;
            }
            Input::Server(kind, payload) => self.on_server(kind, payload, now, out),
            Input::Game(kind, payload) => self.on_game(kind, &payload, now, out),
        }
    }

    fn on_server(&mut self, kind: u16, payload: Vec<u8>, now: u64, out: &mut Vec<Act>) {
        match kind {
            rec::K_MOD_MANIFEST => self.pend_manifest = Some(payload),
            rec::K_MOD_FILES => self.pend_files = Some(payload),
            rec::K_MOD_CHUNK => {
                let Some(x) = self.xfer.as_mut() else { return };
                let v = match view::<rec::ModChunkHead>(&payload) {
                    Ok(v) => v,
                    Err(e) => { debug!(error = %e, "mod_chunk refused"); return; }
                };
                match x.on_chunk(&v.head(), &v.rows) {
                    Ok(mods) => {
                        for m in mods {
                            self.commit(m, out);
                        }
                        if self.phase == Phase::Downloading && self.xfer.as_ref().is_some_and(|x| x.complete()) {
                            self.go_ready(out);
                        }
                        self.pump(now, out);
                    }
                    Err(XferError::HashMismatch(p)) => self.fail(mod_error::HASH_MISMATCH, format!("{p} did not match the server's announced hash; nothing was written"), out),
                    Err(XferError::Protocol(e)) => self.fail(mod_error::BAD_MANIFEST, e, out),
                }
                return;
            }
            _ => return,
        }
        let (Some(m), Some(f)) = (&self.pend_manifest, &self.pend_files) else { return };
        let ann = match manifest::from_payloads(m, f, &Limits::PROTOCOL) {
            Ok(a) => a,
            Err(e) => {
                self.pend_manifest = None;
                self.pend_files = None;
                // Without a valid offer there is no set hash to answer with.
                self.ann = None;
                return self.fail(mod_error::BAD_MANIFEST, format!("the server's mod list breaks a rule: {e}"), out);
            }
        };
        self.pend_manifest = None;
        self.pend_files = None;
        let set = ann.manifest.set_hash;
        // The set the game runs already (the same server after a restart, a resume).
        if self.loaded == Some(set) {
            self.ann = Some(ann);
            self.phase = Phase::Loaded;
            self.loaded_epoch.set(Some(self.epoch));
            out.push(self.ready_msg(mod_result::LOADED, 0));
            self.hold.store(false, Ordering::Release);
            out.push(Act::Release);
            return;
        }
        // The same offer again (a resume): keep the decision and the transfer.
        if self.ann.as_ref().is_some_and(|a| a.manifest.set_hash == set) && matches!(self.phase, Phase::Offered | Phase::Downloading | Phase::Ready) {
            if self.phase == Phase::Ready {
                out.push(self.progress(mod_state::READY));
            }
            self.pump(now, out);
            return;
        }
        let m = &ann.manifest;
        info!(mods = m.mods.len(), files = m.file_count(), bytes = m.total_bytes(), set = %m.set_hash_hex(), "the server offers mods");
        self.cached = m.mods.iter().map(|md| super::mods_cache::verify(&self.root, md)).collect();
        for (md, c) in m.mods.iter().zip(&self.cached) {
            if !c && super::mods_cache::mod_dir(&self.root, md).exists() {
                warn!(name = %md.name, "a cached copy of this mod does not verify; it is removed and downloaded again");
                super::mods_cache::remove(&self.root, md);
            }
        }
        self.ann = Some(ann);
        self.xfer = None;
        self.phase = Phase::Offered;
        self.offer(out);
        out.push(self.progress(mod_state::OFFER));
    }

    fn on_game(&mut self, kind: u16, payload: &[u8], now: u64, out: &mut Vec<Act>) {
        match kind {
            rec::K_MOD_DECISION => {
                let Ok(d) = view::<rec::ModDecision>(payload).map(|v| v.head()) else { return };
                if d.op == rec::mod_op::RESEND {
                    if self.ann.is_some() {
                        self.offer(out);
                    }
                    out.push(self.progress(self.state_code()));
                    return;
                }
                if self.phase != Phase::Offered || d.set_hash != self.set_hash() {
                    debug!(op = d.op, "a decision for no current offer; ignored");
                    return;
                }
                if d.op == rec::mod_op::DECLINE {
                    info!("the player declined the server's mods");
                    return self.fail(mod_error::DECLINED, "You declined the server's mods".into(), out);
                }
                let Some(ann) = self.ann.clone() else { return };
                match Transfer::new(ann, &self.cached) {
                    Ok(x) => self.xfer = Some(x),
                    Err(XferError::HashMismatch(p)) | Err(XferError::Protocol(p)) => {
                        return self.fail(mod_error::BAD_MANIFEST, format!("{p}: an empty file with a wrong hash"), out)
                    }
                }
                info!("the player accepted the server's mods");
                events::emit("server_mods", serde_json::json!({"state": "accepted"}));
                self.phase = Phase::Downloading;
                let done: Vec<usize> = self.xfer.as_ref().map(|x| x.complete_mods()).unwrap_or_default();
                for m in done {
                    if !self.cached.get(m).copied().unwrap_or(false) {
                        self.commit(m, out);
                    }
                }
                if self.phase == Phase::Downloading && self.xfer.as_ref().is_some_and(|x| x.complete()) {
                    self.go_ready(out);
                } else {
                    out.push(self.progress(mod_state::DOWNLOADING));
                    self.pump(now, out);
                }
            }
            rec::K_MOD_LOADED => {
                let Ok(l) = view::<rec::ModLoaded>(payload).map(|v| v.head()) else { return };
                if self.phase != Phase::Ready || l.set_hash != self.set_hash() {
                    debug!("a loaded report for no ready set; ignored");
                    return;
                }
                if !l.ok.get() {
                    return self.fail(mod_error::LOAD_FAILED, format!("The game could not load the server's mods: {}", l.text.lossy()), out);
                }
                info!(failed = l.failed, "the game loaded the server's mods");
                events::emit("server_mods", serde_json::json!({"state": "loaded", "failed": l.failed}));
                self.phase = Phase::Loaded;
                self.loaded = Some(self.set_hash());
                self.loaded_epoch.set(Some(self.epoch));
                out.push(self.ready_msg(mod_result::LOADED, l.failed));
                out.push(self.progress(mod_state::JOINED));
                self.hold.store(false, Ordering::Release);
                out.push(Act::Release);
            }
            _ => {}
        }
    }

    /// Write a complete mod into the cache.
    fn commit(&mut self, m: usize, out: &mut Vec<Act>) {
        let (Some(x), Some(ann)) = (self.xfer.as_mut(), self.ann.as_ref()) else { return };
        let Some(data) = x.take_mod(m) else { return };
        let md = &ann.manifest.mods[m];
        match super::mods_cache::commit(&self.root, md, &data) {
            Ok(dir) => info!(name = %md.name, dir = %dir.display(), "server mod verified and cached"),
            Err(super::mods_cache::CacheError::Mismatch(p)) => {
                self.fail(mod_error::HASH_MISMATCH, format!("{p} did not match the server's announced hash; nothing was written"), out)
            }
            Err(super::mods_cache::CacheError::Disk(e)) => self.fail(mod_error::DISK, e, out),
        }
    }

    fn go_ready(&mut self, out: &mut Vec<Act>) {
        if self.phase == Phase::Failed {
            return;
        }
        info!("every server mod is in the cache: the game loads them");
        self.phase = Phase::Ready;
        out.push(self.progress(mod_state::READY));
    }

    /// Requests due now.
    fn pump(&mut self, now: u64, out: &mut Vec<Act>) {
        if self.phase != Phase::Downloading {
            return;
        }
        let Some(x) = self.xfer.as_mut() else { return };
        for r in x.requests(now) {
            out.push(Act::Net(hsmp_ipc::wire::encode(0, 0, &r, &[])));
        }
    }

    /// The 50 ms timer: requests, the manifest deadline, throttled progress.
    fn tick(&mut self, now: u64, out: &mut Vec<Act>) {
        if self.phase == Phase::AwaitManifest && self.since.elapsed() >= Duration::from_millis(MANIFEST_WAIT_MS) {
            return self.fail(mod_error::TIMEOUT, "The server offered mods but did not send its mod list".into(), out);
        }
        self.pump(now, out);
        if self.phase == Phase::Downloading {
            let due = self.progress_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(PROGRESS_EVERY_MS));
            let done = self.xfer.as_ref().map_or(0, |x| x.progress().0);
            if due && self.last_progress != Some((mod_state::DOWNLOADING, done)) {
                self.progress_at = Some(Instant::now());
                self.last_progress = Some((mod_state::DOWNLOADING, done));
                out.push(self.progress(mod_state::DOWNLOADING));
            }
        }
    }
}

/// Start the task (once, from main). `root` is the cache (`--mods-cache`).
pub(crate) fn spawn(sock: Arc<UdpSocket>, root: PathBuf) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
    if INPUT.set(tx).is_err() {
        return;
    }
    super::mods_cache::sweep(&root);
    info!(cache = %root.display(), "server mods cache");
    tokio::spawn(async move {
        let t0 = Instant::now();
        let mut c = Client::new(root, &HOLD, &LOADED_EPOCH);
        let mut iv = time::interval(TICK);
        iv.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            let mut out = Vec::new();
            tokio::select! {
                i = rx.recv() => match i {
                    Some(i) => c.handle(i, now_ms(t0), &mut out),
                    None => return,
                },
                _ = iv.tick() => c.tick(now_ms(t0), &mut out),
            }
            perform(&sock, out).await;
        }
    });
}

async fn perform(sock: &UdpSocket, out: Vec<Act>) {
    let mut msgs = Vec::new();
    for a in out {
        match a {
            Act::Net(m) => msgs.push(m),
            Act::Game(kind, p) => {
                if let Some(l) = ipc_shm::link() {
                    l.push_record(kind, 0, 0, &p);
                }
            }
            Act::Release => session_client::release_held_session(),
        }
    }
    if !msgs.is_empty() {
        let _ = net::send_msgs(sock, msgs).await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::server_mods::{manifest::tests::mods_dir, Config, Host};
    use super::*;
    use hsmp_net::net::testlink::Link;
    use hsmp_net::net::{ClientConfig, ClientEvent, ConnConfig, Incoming, SendMode, ServerConfig, ServerEndpoint};

    /// A server set: `n` files of the given sizes in one mod, plus a second small mod.
    fn host(sizes: &[usize]) -> Host {
        let names = ["Scripts/main.lua", "Scripts/a.lua", "Scripts/b.lua", "data/c.csv"];
        let blobs: Vec<Vec<u8>> = sizes.iter().enumerate().map(|(i, n)| (0..*n).map(|x| (x * 7 + i) as u8).collect()).collect();
        let mut files: Vec<(&str, &str, &[u8])> = sizes.iter().enumerate().map(|(i, _)| ("big", names[i], blobs[i].as_slice())).collect();
        files.push(("small", "Scripts/main.lua", b"print('small')"));
        let d = mods_dir(&files);
        let (b, _) = manifest::build_from_dir(&d, &Limits::PROTOCOL).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        Host::new(Config::default(), b)
    }

    fn announced(h: &Host) -> Announced {
        let (_, m) = hsmp_ipc::wire::split(&h.manifest_msg).unwrap();
        let (_, f) = hsmp_ipc::wire::split(&h.files_msg).unwrap();
        manifest::from_payloads(m, f, &Limits::PROTOCOL).unwrap()
    }

    /// Server endpoint + client over an impaired link; the server answers chunk requests
    /// through `Host` (its queue and rates), the client pulls with `Transfer`.
    struct World {
        now: u64,
        server: ServerEndpoint,
        client: hsmp_net::net::Client,
        up: Link,
        down: Link,
        caddr: std::net::SocketAddr,
        cid: Option<hsmp_net::net::ConnId>,
        host: Host,
        xfer: Transfer,
        committed: Vec<usize>,
        chunks: u64,
        seed: u64,
    }

    impl World {
        fn new(seed: u64, loss: f64, h: Host, cached: &[bool]) -> World {
            let now = 1_000_000;
            let ann = announced(&h);
            World {
                now,
                server: ServerEndpoint::new(ServerConfig::new([42; 32]), ConnConfig::default(), now, Some(seed)),
                client: Self::client(seed, now),
                up: Link::new(100 + seed, loss, 0.1, 30, 40),
                down: Link::new(200 + seed, loss, 0.1, 30, 40),
                caddr: "203.0.113.9:50000".parse().unwrap(),
                cid: None,
                host: h,
                xfer: Transfer::new(ann, cached).unwrap(),
                committed: Vec::new(),
                chunks: 0,
                seed,
            }
        }
        fn client(seed: u64, now: u64) -> hsmp_net::net::Client {
            let mut cfg = ClientConfig::new([seed as u8 + 1; 32], "w");
            cfg.caps |= hsmp_net::net::caps::SERVER_MODS;
            let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(seed);
            hsmp_net::net::Client::new(cfg, ConnConfig::default(), now, &mut rng)
        }
        /// A new connection (sidecar reconnect): the old one is gone mid-transfer.
        fn reconnect(&mut self) {
            self.seed += 1000;
            self.client = Self::client(self.seed, self.now);
            self.cid = None;
            self.host.clear_queue(1);
            self.xfer.on_reconnect();
        }
        fn step(&mut self) -> Result<(), XferError> {
            let now = self.now;
            if self.cid.is_some() && self.client.is_connected() {
                for r in self.xfer.requests(now) {
                    self.client.send(SendMode::Ordered, hsmp_ipc::wire::encode(0, 0, &r, &[])).unwrap();
                }
            }
            while let Some(dg) = self.client.poll_transmit(now) { self.up.push(now, dg); }
            if let Some(cid) = self.cid {
                for (_, r, msg) in self.host.take_due(now, 64) {
                    if self.server.send(cid, SendMode::Reliable, msg).is_err() { self.host.retry(1, r); }
                }
            }
            for (a, dg) in self.server.poll_transmit(now) {
                if a == self.caddr { self.down.push(now, dg); }
            }
            for dg in self.up.pop_due(now) {
                match self.server.handle(now, self.caddr, &dg) {
                    Incoming::Reply(r) => self.down.push(now, r),
                    Incoming::AuthRequest(p) => self.cid = Some(self.server.accept(now, p)),
                    Incoming::Data { deliveries, .. } => {
                        for d in deliveries {
                            let (h, p) = hsmp_ipc::wire::split(&d.data).unwrap();
                            assert_eq!(h.kind, rec::K_MOD_CHUNK_REQ);
                            let r = view::<rec::ModChunkReq>(p).unwrap().head();
                            assert_eq!(self.host.request(1, &r, now), super::super::server_mods::Req::Queued);
                        }
                    }
                    _ => {}
                }
            }
            for dg in self.down.pop_due(now) { self.client.handle(now, &dg); }
            while let Some(ev) = self.client.poll_event() {
                if let ClientEvent::Message(d) = ev {
                    let (h, p) = hsmp_ipc::wire::split(&d.data).unwrap();
                    if h.kind == rec::K_MOD_CHUNK {
                        let v = view::<rec::ModChunkHead>(p).unwrap();
                        self.chunks += 1;
                        self.committed.extend(self.xfer.on_chunk(&v.head(), &v.rows)?);
                    }
                }
            }
            self.now += 2;
            Ok(())
        }
    }

    /// The whole set over a link with 20 % loss, 10 % duplication and 30-70 ms of jitter
    /// (reordering), with a reconnect in the middle: every file arrives complete and
    /// verified, the in-flight window is never exceeded, and the resume does not start over.
    #[test]
    fn transfer_survives_loss_reorder_and_a_reconnect() {
        for (seed, loss) in [(1u64, 0.0), (2, 0.2), (3, 0.3)] {
            let mut w = World::new(seed, loss, host(&[150_000, 40_000, 1, 0]), &[]);
            let total = w.xfer.progress().1;
            assert_eq!(total, 150_000 + 40_000 + 1 + 14);
            let mut reconnected = false;
            let mut before = 0;
            let deadline = w.now + 300_000;
            while w.now < deadline && !w.xfer.complete() {
                w.step().unwrap();
                assert!(w.xfer.in_flight() <= WINDOW);
                if !reconnected && w.xfer.progress().0 > total / 3 {
                    before = w.xfer.progress().0;
                    w.reconnect();
                    reconnected = true;
                }
            }
            assert!(w.xfer.complete(), "seed {seed}: {:?} of {total}", w.xfer.progress());
            assert!(reconnected && before > 0);
            assert_eq!(w.xfer.progress().0, total, "every byte counted once");
            let mut c = w.committed.clone();
            c.sort_unstable();
            assert_eq!(c, vec![0, 1], "both mods complete once");
            // the bytes are the server's
            let flat = w.xfer.ann.manifest.flat().into_iter().map(|(_, f)| f.sha256).collect::<Vec<_>>();
            let mut got = Vec::new();
            for m in 0..2 { got.extend(w.xfer.take_mod(m).unwrap()); }
            for (b, sha) in got.iter().zip(flat) { assert_eq!(manifest::sha256(b), sha); }
            // resumed, not restarted: at most the window was asked twice
            let chunk_count: u64 = w.xfer.ann.manifest.flat().iter().map(|(_, f)| f.size.div_ceil(32768)).sum();
            assert!(w.chunks <= chunk_count + WINDOW as u64 + 2, "seed {seed}: {} chunks for {chunk_count}", w.chunks);
        }
    }

    /// Cached mods are not downloaded; a server that answers with other bytes than the
    /// announced hash fails the transfer before anything is handed over.
    #[test]
    fn cached_mods_are_skipped_and_bad_bytes_fail() {
        let h = host(&[50_000]);
        let ann = announced(&h);
        let big = ann.manifest.mods.iter().position(|m| m.name == "big").unwrap();
        let mut cached = vec![false; 2];
        cached[big] = true;
        let mut x = Transfer::new(ann.clone(), &cached).unwrap();
        assert_eq!(x.progress().1, 14, "only the small mod is downloaded");
        let rs = x.requests(0);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].file as usize, ann.manifest.first_files()[1 - big]);
        // a tampered answer: the file fails its hash, the mod is never complete
        let mut bytes = b"print('small')".to_vec();
        bytes[0] ^= 1;
        let head = rec::ModChunkHead { offset: 0, req_id: rs[0].req_id, n: 0, file: rs[0].file, _r: [0; 6] };
        assert!(matches!(x.on_chunk(&head, &bytes), Err(XferError::HashMismatch(_))));
        assert!(!x.complete());
        assert!(x.take_mod(1 - big).is_none());
        // an answer of the wrong size or offset is a protocol error; an unknown req_id is ignored
        let mut y = Transfer::new(ann, &cached).unwrap();
        let r = y.requests(0)[0];
        assert!(y.on_chunk(&rec::ModChunkHead { req_id: 999, ..head }, b"x").unwrap().is_empty());
        assert!(matches!(y.on_chunk(&rec::ModChunkHead { req_id: r.req_id, offset: 5, ..head }, b"print('small')"), Err(XferError::Protocol(_))));
        // stale requests are asked again
        let mut z = Transfer::new(announced(&host(&[200_000])), &[false, false]).unwrap();
        let first: Vec<u32> = z.requests(0).iter().map(|r| r.req_id).collect();
        assert_eq!(first.len(), WINDOW);
        assert!(z.requests(1000).is_empty(), "the window is full");
        assert_eq!(z.requests(REQ_TIMEOUT_MS + 1).len(), WINDOW, "asked again after the timeout");
    }

    static T_HOLD: AtomicBool = AtomicBool::new(false);
    static T_EPOCH: LoadedEpoch = LoadedEpoch(std::sync::Mutex::new(None));

    fn feed(c: &mut Client, i: Input) -> Vec<Act> {
        let mut out = Vec::new();
        c.handle(i, 0, &mut out);
        out
    }

    fn decision(c: &Client, op: u8) -> Input {
        Input::Game(rec::K_MOD_DECISION, hsmp_ipc::bytemuck::bytes_of(&rec::ModDecision { set_hash: c.set_hash(), op, _r: [0; 7] }).to_vec())
    }

    fn progress_states(out: &[Act]) -> Vec<u8> {
        out.iter().filter_map(|a| match a {
            Act::Game(k, p) if *k == rec::K_MOD_PROGRESS => view::<rec::ModProgress>(p).ok().map(|v| v.head().state),
            _ => None,
        }).collect()
    }

    /// The consent rule: nothing is requested before ACCEPT for exactly the offered set; a
    /// decline answers DECLINED; the offer reaching the game carries only data; an invalid
    /// manifest fails without an offer; the session is held until the game loaded the set.
    #[test]
    fn nothing_downloads_without_consent() {
        let root = std::env::temp_dir().join(format!("hsmp-mc-{}-{}", std::process::id(), rand::random::<u64>()));
        let h = host(&[10_000]);
        let mut c = Client::new(root.clone(), &T_HOLD, &T_EPOCH);
        feed(&mut c, Input::Connected([7; 32]));
        let caps = hsmp_net::net::caps::SERVER_MODS;
        T_HOLD.store(true, Ordering::Release); // what on_welcome does for a SERVER_MODS welcome
        assert!(feed(&mut c, Input::Welcome { epoch: 77, caps }).is_empty());
        assert!(feed(&mut c, Input::Server(rec::K_MOD_MANIFEST, hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec())).is_empty());
        let out = feed(&mut c, Input::Server(rec::K_MOD_FILES, hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec()));
        // offer + 2 entries + OFFER progress, and nothing for the network
        assert!(out.iter().all(|a| matches!(a, Act::Game(..))), "{out:?}");
        assert_eq!(out.iter().filter(|a| matches!(a, Act::Game(k, _) if *k == rec::K_MOD_ENTRY)).count(), 2);
        let o = out.iter().find_map(|a| match a { Act::Game(k, p) if *k == rec::K_MOD_OFFER => Some(view::<rec::ModOffer>(p).unwrap().head()), _ => None }).unwrap();
        assert_eq!((o.server_key, o.n, o.set_hash), ([7; 32], 2, h.set_hash()));
        assert_eq!(progress_states(&out), vec![mod_state::OFFER]);
        // ticks never request before consent
        let mut t = Vec::new();
        c.tick(10, &mut t);
        assert!(t.is_empty());
        // a decision for another set is ignored
        let wrong = Input::Game(rec::K_MOD_DECISION, hsmp_ipc::bytemuck::bytes_of(&rec::ModDecision { set_hash: [1; 32], op: rec::mod_op::ACCEPT, _r: [0; 7] }).to_vec());
        assert!(feed(&mut c, wrong).is_empty());
        // accept: requests go out, DOWNLOADING
        let out = { let i = decision(&c, rec::mod_op::ACCEPT); feed(&mut c, i) };
        assert_eq!(out.iter().filter(|a| matches!(a, Act::Net(m) if hsmp_ipc::wire::kind_of(m) == rec::K_MOD_CHUNK_REQ)).count(), 2);
        assert!(progress_states(&out).contains(&mod_state::DOWNLOADING));
        // serve the requests from the host: READY, the cache holds both mods
        let mut acts = out;
        for _ in 0..10 {
            let reqs: Vec<rec::ModChunkReq> = acts.iter().filter_map(|a| match a {
                Act::Net(m) if hsmp_ipc::wire::kind_of(m) == rec::K_MOD_CHUNK_REQ => Some(view::<rec::ModChunkReq>(hsmp_ipc::wire::split(m).unwrap().1).unwrap().head()),
                _ => None,
            }).collect();
            acts.clear();
            for r in reqs { h.request(1, &r, 0); }
            for (_, _, msg) in h.take_due(0, 64) {
                acts.extend(feed(&mut c, Input::Server(rec::K_MOD_CHUNK, hsmp_ipc::wire::split(&msg).unwrap().1.to_vec())));
            }
            if c.phase == Phase::Ready { break; }
        }
        assert_eq!(c.phase, Phase::Ready);
        assert!(T_HOLD.load(Ordering::Acquire), "still held: the game has not loaded the set");
        for m in &c.ann.as_ref().unwrap().manifest.mods { assert!(super::super::mods_cache::verify(&root, m), "{} cached", m.name); }
        // the game loaded it: mod_ready LOADED, JOINED, released
        let loaded = rec::ModLoaded { set_hash: c.set_hash(), ok: true.into(), failed: 0, _r: [0; 6], text: Str::default() };
        let out = feed(&mut c, Input::Game(rec::K_MOD_LOADED, hsmp_ipc::bytemuck::bytes_of(&loaded).to_vec()));
        assert!(out.iter().any(|a| matches!(a, Act::Net(m) if hsmp_ipc::wire::kind_of(m) == rec::K_MOD_READY)));
        assert!(out.contains(&Act::Release));
        assert!(!T_HOLD.load(Ordering::Acquire));
        assert_eq!(progress_states(&out), vec![mod_state::JOINED]);
        assert!(T_EPOCH.with(|e| e == Some(77)), "a resume of this server instance needs no new offer");
        // a server restart with the same set: answered at once from the loaded set
        T_HOLD.store(true, Ordering::Release);
        feed(&mut c, Input::Welcome { epoch: 78, caps });
        feed(&mut c, Input::Server(rec::K_MOD_MANIFEST, hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec()));
        let out = feed(&mut c, Input::Server(rec::K_MOD_FILES, hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec()));
        assert!(out.iter().any(|a| matches!(a, Act::Net(m) if hsmp_ipc::wire::kind_of(m) == rec::K_MOD_READY)) && out.contains(&Act::Release));

        // a new player: decline answers DECLINED and never requests
        let mut d = Client::new(root.clone(), &T_HOLD, &T_EPOCH);
        feed(&mut d, Input::Welcome { epoch: 1, caps });
        feed(&mut d, Input::Server(rec::K_MOD_MANIFEST, hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec()));
        feed(&mut d, Input::Server(rec::K_MOD_FILES, hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec()));
        let out = { let i = decision(&d, rec::mod_op::DECLINE); feed(&mut d, i) };
        let ready = out.iter().find_map(|a| match a { Act::Net(m) => hsmp_ipc::wire::decode::<rec::ModReady>(m).ok().map(|(_, v)| v.head()), _ => None }).unwrap();
        assert_eq!(ready.result, mod_result::DECLINED);
        assert!(!out.iter().any(|a| matches!(a, Act::Net(m) if hsmp_ipc::wire::kind_of(m) == rec::K_MOD_CHUNK_REQ)));
        assert_eq!(d.phase, Phase::Failed);
        // an accept after the decline does nothing
        assert!({ let i = decision(&d, rec::mod_op::ACCEPT); feed(&mut d, i) }.is_empty());

        // a manifest that breaks a rule: FAILED, no offer
        let mut e = Client::new(root.clone(), &T_HOLD, &T_EPOCH);
        feed(&mut e, Input::Welcome { epoch: 1, caps });
        let mut bad = hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec();
        bad[0] ^= 1; // the set hash of mod_files
        feed(&mut e, Input::Server(rec::K_MOD_MANIFEST, hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec()));
        let out = feed(&mut e, Input::Server(rec::K_MOD_FILES, bad));
        assert_eq!(progress_states(&out), vec![mod_state::FAILED]);
        assert!(!out.iter().any(|a| matches!(a, Act::Game(k, _) if *k == rec::K_MOD_OFFER)));
        // a server without mods: nothing held, a loaded set is cleared
        let out = feed(&mut c, Input::Welcome { epoch: 5, caps: 0 });
        assert!(!T_HOLD.load(Ordering::Acquire));
        assert_eq!(progress_states(&out), vec![mod_state::CLEAR]);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn consent_does_not_follow_identical_mods_to_another_server() {
        let root=std::env::temp_dir().join(format!("hsmp-mod-identity-{}",rand::random::<u64>()));
        let hold=Box::leak(Box::new(AtomicBool::new(true)));
        let epoch=Box::leak(Box::new(LoadedEpoch(std::sync::Mutex::new(None))));
        let mut c=Client::new(root,hold,epoch);
        let h=host(&[10_000]);
        let caps=hsmp_net::net::caps::SERVER_MODS;
        feed(&mut c,Input::Connected([7;32]));
        feed(&mut c,Input::Welcome{epoch:77,caps});
        feed(&mut c,Input::Server(rec::K_MOD_MANIFEST,hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec()));
        feed(&mut c,Input::Server(rec::K_MOD_FILES,hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec()));
        let accepted=decision(&c,rec::mod_op::ACCEPT);feed(&mut c,accepted);
        assert_eq!(c.phase,Phase::Downloading);
        feed(&mut c,Input::Connected([7;32]));
        assert_eq!(c.phase,Phase::Downloading,"same-key reconnect preserves consent and transfer");
        c.loaded=Some(h.set_hash());c.phase=Phase::Loaded;epoch.set(Some(77));
        let cleared=feed(&mut c,Input::Connected([8;32]));
        assert_eq!(progress_states(&cleared),vec![mod_state::CLEAR]);
        assert!(c.loaded.is_none() && epoch.with(|e|e.is_none()));
        feed(&mut c,Input::Welcome{epoch:77,caps});
        feed(&mut c,Input::Server(rec::K_MOD_MANIFEST,hsmp_ipc::wire::split(&h.manifest_msg).unwrap().1.to_vec()));
        let offered=feed(&mut c,Input::Server(rec::K_MOD_FILES,hsmp_ipc::wire::split(&h.files_msg).unwrap().1.to_vec()));
        assert_eq!(c.phase,Phase::Offered);
        assert_eq!(progress_states(&offered),vec![mod_state::OFFER]);
        assert!(offered.iter().all(|a|matches!(a,Act::Game(..))),"new identity must neither download nor release before consent");
    }

}

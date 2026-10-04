//! Server-served mods (kinds `0x09xx`, net capability `SERVER_MODS`; docs/hosting/server-mods.md,
//! docs/development/protocol.md "Server mods").
//!
//! A server with a `--mods-dir` announces its mods right after the `welcome`; the player's
//! game asks for consent; the sidecar downloads the files over the game connection, checks
//! every byte against the announced SHA-256, writes them into its content-addressed cache and
//! the game's HSMPModHost loads them. Every record is data: names, sizes, hashes and text.
//! No path, code or Lua source ever enters the shared-memory segment; file paths and bytes
//! travel only between the server and the sidecar.
//!
//! - Network: `mod_manifest` and `mod_files` (S2C: the mod list and the file list),
//!   `mod_chunk_req` (C2S, pull) and `mod_chunk` (S2C, at most [`MAX_CHUNK`] bytes),
//!   `mod_ready` (C2S: the mods are loaded, or the player declined / loading failed).
//! - Sidecar -> game (S2G ring): `mod_offer`, one `mod_entry` per mod, `mod_progress`.
//! - Game -> sidecar (G2S ring): `mod_decision` (accept / decline / resend the offer),
//!   `mod_loaded` (HSMPModHost loaded the set).

use crate::layout::{Bool, Str};
use crate::record::Invalid;

/// No legacy (codec) kinds.
pub const KINDS: &[super::KindInfo] = &[];

// ---- limits (protocol constants; a server may configure lower ones) -------------------------

/// Mods in one set.
pub const MAX_MODS: usize = 16;
/// Files in one set (all mods together).
pub const MAX_FILES: usize = 256;
/// Bytes of one `mod_chunk`.
pub const MAX_CHUNK: usize = 32 * 1024;
/// Bytes of a file path inside its mod (`Scripts/main.lua`).
pub const MAX_PATH: usize = 128;

// ---- kinds ----------------------------------------------------------------------------------

pub const K_MOD_MANIFEST: u16 = 0x0901;
pub const K_MOD_FILES: u16 = 0x0902;
pub const K_MOD_CHUNK_REQ: u16 = 0x0903;
pub const K_MOD_CHUNK: u16 = 0x0904;
pub const K_MOD_READY: u16 = 0x0905;
pub const K_MOD_OFFER: u16 = 0x0906;
pub const K_MOD_ENTRY: u16 = 0x0907;
pub const K_MOD_PROGRESS: u16 = 0x0908;
pub const K_MOD_DECISION: u16 = 0x0909;
pub const K_MOD_LOADED: u16 = 0x090A;

/// A kind of this domain.
pub fn is_mods_kind(kind: u16) -> bool {
    (K_MOD_MANIFEST..=K_MOD_LOADED).contains(&kind)
}

// ---- codes ----------------------------------------------------------------------------------

/// `mod_ready.result`.
pub mod mod_result {
    /// The game loaded the set: the player may play.
    pub const LOADED: u8 = 1;
    /// Downloading, verifying or loading failed (the sidecar leaves).
    pub const FAILED: u8 = 2;
    /// The player declined (the sidecar leaves).
    pub const DECLINED: u8 = 3;
}

/// `mod_progress.state`.
pub mod mod_state {
    /// The offer waits for the player's decision.
    pub const OFFER: u8 = 1;
    pub const DOWNLOADING: u8 = 2;
    pub const VERIFYING: u8 = 3;
    /// Every mod of the set is in the cache: HSMPModHost loads it now.
    pub const READY: u8 = 4;
    /// The server knows the set is loaded: the session continues.
    pub const JOINED: u8 = 5;
    /// Terminal: `code` and `text` say why (the sidecar leaves the server).
    pub const FAILED: u8 = 6;
    /// This server has no mods (or a new server without the loaded set): unload everything.
    pub const CLEAR: u8 = 7;
}

/// `mod_progress.code`.
pub mod mod_error {
    pub const NONE: u8 = 0;
    /// A downloaded file did not match its announced SHA-256 (nothing was written).
    pub const HASH_MISMATCH: u8 = 1;
    /// The set is over this client's limits.
    pub const TOO_LARGE: u8 = 2;
    /// The manifest broke a rule (path, name, extension, hash).
    pub const BAD_MANIFEST: u8 = 3;
    /// The connection to the server ended.
    pub const DISCONNECTED: u8 = 4;
    /// The cache could not be written.
    pub const DISK: u8 = 5;
    /// The server did not answer in time.
    pub const TIMEOUT: u8 = 6;
    /// HSMPModHost could not load the set.
    pub const LOAD_FAILED: u8 = 7;
    pub const DECLINED: u8 = 8;
    /// "Never allow server mods" is set.
    pub const BLOCKED: u8 = 9;
}

/// `mod_decision.op`.
pub mod mod_op {
    pub const ACCEPT: u8 = 1;
    pub const DECLINE: u8 = 2;
    /// Push the offer, its entries and the progress again (a mod's cursor fell behind).
    pub const RESEND: u8 = 3;
}

// ---- records --------------------------------------------------------------------------------

crate::ipc_pod! {
    /// `mod_manifest` (S2C): the server's mod set. Head + one [`ModRow`] per mod.
    pub struct ModManifestHead {
        /// SHA-256 over the canonical manifest (every mod hash, sorted by name).
        pub set_hash: [u8; 32],
        /// Bytes of every file of every mod.
        pub total_bytes: u64,
        /// Largest `mod_chunk_req.len` the server serves.
        pub max_chunk: u32,
        /// Seconds the server waits for `mod_ready` before it disconnects the player.
        pub timeout_s: u32,
        /// Mods (rows).
        pub n: u16,
        /// Files (the rows of the `mod_files` that follows).
        pub files: u16,
        pub _r: u32,
    }

    /// One mod of a set.
    pub struct ModRow {
        /// SHA-256 over the mod's name, metadata and files (its cache directory name).
        pub mod_hash: [u8; 32],
        pub bytes: u64,
        /// Its files are `mod_files` rows `first_file .. first_file + files`.
        pub first_file: u16,
        pub files: u16,
        pub _r: u32,
        /// The mod folder name (`[A-Za-z0-9_-]`, never `HSMP*`).
        pub name: Str<32>,
        pub version: Str<16>,
        pub author: Str<48>,
        pub description: Str<160>,
    }

    /// `mod_files` (S2C): every file of the set. Head + one [`ModFileRow`] per file.
    pub struct ModFilesHead {
        pub set_hash: [u8; 32],
        pub n: u16,
        pub _r: [u8; 6],
    }

    /// One file: where it goes inside its mod, how big it is, its SHA-256.
    pub struct ModFileRow {
        pub sha256: [u8; 32],
        pub size: u64,
        /// Row of its mod in `mod_manifest`.
        pub mod_index: u16,
        pub _r: [u8; 6],
        /// Relative, `/`-separated (`Scripts/main.lua`).
        pub path: Str<128>,
    }

    /// `mod_chunk_req` (C2S): send `len` bytes of file `file` from `offset`.
    pub struct ModChunkReq {
        pub offset: u64,
        /// Echoed in the answer.
        pub req_id: u32,
        pub len: u32,
        /// Row of the file in `mod_files`.
        pub file: u16,
        pub _r: [u8; 6],
    }

    /// `mod_chunk` (S2C): the bytes of one request. Head + `n` byte rows.
    pub struct ModChunkHead {
        pub offset: u64,
        pub req_id: u32,
        /// Bytes that follow.
        pub n: u32,
        pub file: u16,
        pub _r: [u8; 6],
    }

    /// `mod_ready` (C2S): the outcome for this set.
    pub struct ModReady {
        pub set_hash: [u8; 32],
        /// `mod_result` code.
        pub result: u8,
        /// Mods whose code raised an error while loading (they are unloaded; the rest run).
        pub failed: u8,
        pub _r: [u8; 6],
    }

    /// `mod_offer` (S2G): a server offers a mod set; the entries follow.
    pub struct ModOffer {
        pub set_hash: [u8; 32],
        /// The server's identity (X25519 public key): consent is remembered per (key, set).
        pub server_key: [u8; 32],
        pub total_bytes: u64,
        /// Bytes already in the local cache (verified): nothing to download for those.
        pub cached_bytes: u64,
        /// Mods (one `mod_entry` each).
        pub n: u16,
        pub files: u16,
        pub _r: u32,
    }

    /// `mod_entry` (S2G): one mod of the offer, as the player sees it.
    pub struct ModEntry {
        pub set_hash: [u8; 32],
        /// The cache directory (`<hsmp_mods>/<hex>`), the only thing HSMPModHost loads from.
        pub mod_hash: [u8; 32],
        pub bytes: u64,
        /// 0-based position in the set, and the set's size.
        pub index: u16,
        pub n: u16,
        pub files: u16,
        pub cached: Bool,
        pub _r: u8,
        pub name: Str<32>,
        pub version: Str<16>,
        pub author: Str<48>,
        pub description: Str<160>,
    }

    /// `mod_progress` (S2G): where the set stands.
    pub struct ModProgress {
        pub set_hash: [u8; 32],
        pub done_bytes: u64,
        pub total_bytes: u64,
        /// `mod_state` code.
        pub state: u8,
        /// `mod_error` code (FAILED).
        pub code: u8,
        pub _r: [u8; 6],
        pub text: Str<128>,
    }

    /// `mod_decision` (G2S): the player's answer to an offer.
    pub struct ModDecision {
        pub set_hash: [u8; 32],
        /// `mod_op` code.
        pub op: u8,
        pub _r: [u8; 7],
    }

    /// `mod_loaded` (G2S): HSMPModHost ran the set.
    pub struct ModLoaded {
        pub set_hash: [u8; 32],
        pub ok: Bool,
        /// Mods that raised an error while loading (unloaded again).
        pub failed: u8,
        pub _r: [u8; 6],
        pub text: Str<128>,
    }
}

fn check_req(r: &ModChunkReq) -> Result<(), Invalid> {
    if r.len == 0 || r.len as usize > MAX_CHUNK {
        return Err(Invalid::Range("len"));
    }
    if r.file as usize >= MAX_FILES {
        return Err(Invalid::Range("file"));
    }
    Ok(())
}

fn check_manifest(h: &ModManifestHead) -> Result<(), Invalid> {
    if h.files as usize > MAX_FILES {
        return Err(Invalid::Range("files"));
    }
    if h.max_chunk as usize > MAX_CHUNK || h.max_chunk == 0 {
        return Err(Invalid::Range("max_chunk"));
    }
    Ok(())
}

fn check_mod_row(_h: &ModManifestHead, r: &ModRow) -> Result<(), Invalid> {
    if r.first_file as usize + r.files as usize > MAX_FILES {
        return Err(Invalid::Range("files"));
    }
    Ok(())
}

fn check_file_row(_h: &ModFilesHead, r: &ModFileRow) -> Result<(), Invalid> {
    if r.mod_index as usize >= MAX_MODS {
        return Err(Invalid::Range("mod_index"));
    }
    Ok(())
}

fn check_chunk(h: &ModChunkHead) -> Result<(), Invalid> {
    if h.file as usize >= MAX_FILES {
        return Err(Invalid::Range("file"));
    }
    Ok(())
}

fn check_ready(r: &ModReady) -> Result<(), Invalid> {
    if !(mod_result::LOADED..=mod_result::DECLINED).contains(&r.result) {
        return Err(Invalid::Range("result"));
    }
    Ok(())
}

fn check_offer(o: &ModOffer) -> Result<(), Invalid> {
    if o.n as usize > MAX_MODS || o.files as usize > MAX_FILES {
        return Err(Invalid::Range("n"));
    }
    Ok(())
}

fn check_entry(e: &ModEntry) -> Result<(), Invalid> {
    if e.n as usize > MAX_MODS || e.index >= e.n {
        return Err(Invalid::Range("index"));
    }
    Ok(())
}

fn check_progress(p: &ModProgress) -> Result<(), Invalid> {
    if !(mod_state::OFFER..=mod_state::CLEAR).contains(&p.state) {
        return Err(Invalid::Range("state"));
    }
    if p.code > mod_error::BLOCKED {
        return Err(Invalid::Range("code"));
    }
    Ok(())
}

fn check_decision(d: &ModDecision) -> Result<(), Invalid> {
    if !(mod_op::ACCEPT..=mod_op::RESEND).contains(&d.op) {
        return Err(Invalid::Range("op"));
    }
    Ok(())
}

crate::record!(ModManifestHead, kind = K_MOD_MANIFEST, name = "mod_manifest", rows = ModRow, count = n, max = MAX_MODS,
    check = check_manifest, check_row = check_mod_row);
crate::record!(ModFilesHead, kind = K_MOD_FILES, name = "mod_files", rows = ModFileRow, count = n, max = MAX_FILES,
    check_row = check_file_row);
crate::record!(ModChunkReq, kind = K_MOD_CHUNK_REQ, name = "mod_chunk_req", check = check_req);
crate::record!(ModChunkHead, kind = K_MOD_CHUNK, name = "mod_chunk", rows = u8, count = n, max = MAX_CHUNK, check = check_chunk);
crate::record!(ModReady, kind = K_MOD_READY, name = "mod_ready", check = check_ready);
crate::record!(ModOffer, kind = K_MOD_OFFER, name = "mod_offer", check = check_offer);
crate::record!(ModEntry, kind = K_MOD_ENTRY, name = "mod_entry", check = check_entry);
crate::record!(ModProgress, kind = K_MOD_PROGRESS, name = "mod_progress", check = check_progress);
crate::record!(ModDecision, kind = K_MOD_DECISION, name = "mod_decision", check = check_decision);
crate::record!(ModLoaded, kind = K_MOD_LOADED, name = "mod_loaded");

use super::flow::{C2S, G2S, S2C, S2G};
use super::Chan;

/// Record kinds of this domain (wire + shared memory). Shared-memory capability 0: the game
/// always takes them; the network side is gated by `hsmp_net::net::caps::SERVER_MODS`.
pub const RECORDS: &[super::RecordInfo] = &[
    crate::record_info!(ModManifestHead, cap = 0, flow = S2C, chan = Chan::Ordered,
        doc = "the server's mod set (one row per mod), sent right after the welcome; only with caps::SERVER_MODS"),
    crate::record_info!(ModFilesHead, cap = 0, flow = S2C, chan = Chan::Ordered,
        doc = "every file of the set: path inside its mod, size, SHA-256"),
    crate::record_info!(ModChunkReq, cap = 0, flow = C2S, chan = Chan::Ordered,
        doc = "pull one chunk of one file (bounded window, resumable by file and offset)"),
    crate::record_info!(ModChunkHead, cap = 0, flow = S2C, chan = Chan::Reliable,
        doc = "the bytes of one chunk request (at most 32 KiB); reliable channel 1, behind the game traffic"),
    crate::record_info!(ModReady, cap = 0, flow = C2S, chan = Chan::Ordered,
        doc = "the set is loaded (the server lets the player in), or declined / failed"),
    crate::record_info!(ModOffer, cap = 0, flow = S2G, chan = Chan::None,
        doc = "a server offers a mod set: set hash, server key, sizes (data only: no paths, no code)"),
    crate::record_info!(ModEntry, cap = 0, flow = S2G, chan = Chan::None,
        doc = "one mod of the offer: name, version, author, description, size, cache hash"),
    crate::record_info!(ModProgress, cap = 0, flow = S2G, chan = Chan::None,
        doc = "download / verify / ready / failed, with bytes done and a reason"),
    crate::record_info!(ModDecision, cap = 0, flow = G2S, chan = Chan::None,
        doc = "the player's decision on an offer (accept / decline / resend)"),
    crate::record_info!(ModLoaded, cap = 0, flow = G2S, chan = Chan::None,
        doc = "HSMPModHost loaded the set (or failed)"),
];

/// Named record slots of this domain: none (events only).
pub const SLOTS: &[super::SlotInfo] = &[];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[super::EnumInfo] = &[
    super::EnumInfo {
        name: "mod_result",
        values: &[("LOADED", mod_result::LOADED as u32), ("FAILED", mod_result::FAILED as u32), ("DECLINED", mod_result::DECLINED as u32)],
    },
    super::EnumInfo {
        name: "mod_state",
        values: &[
            ("OFFER", mod_state::OFFER as u32), ("DOWNLOADING", mod_state::DOWNLOADING as u32),
            ("VERIFYING", mod_state::VERIFYING as u32), ("READY", mod_state::READY as u32),
            ("JOINED", mod_state::JOINED as u32), ("FAILED", mod_state::FAILED as u32), ("CLEAR", mod_state::CLEAR as u32),
        ],
    },
    super::EnumInfo {
        name: "mod_error",
        values: &[
            ("NONE", mod_error::NONE as u32), ("HASH_MISMATCH", mod_error::HASH_MISMATCH as u32),
            ("TOO_LARGE", mod_error::TOO_LARGE as u32), ("BAD_MANIFEST", mod_error::BAD_MANIFEST as u32),
            ("DISCONNECTED", mod_error::DISCONNECTED as u32), ("DISK", mod_error::DISK as u32),
            ("TIMEOUT", mod_error::TIMEOUT as u32), ("LOAD_FAILED", mod_error::LOAD_FAILED as u32),
            ("DECLINED", mod_error::DECLINED as u32), ("BLOCKED", mod_error::BLOCKED as u32),
        ],
    },
    super::EnumInfo {
        name: "mod_op",
        values: &[("ACCEPT", mod_op::ACCEPT as u32), ("DECLINE", mod_op::DECLINE as u32), ("RESEND", mod_op::RESEND as u32)],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::view;

    #[test]
    fn layouts_fit_the_wire_and_the_rings() {
        use core::mem::size_of;
        assert_eq!(size_of::<ModRow>(), 304);
        assert_eq!(size_of::<ModFileRow>(), 176);
        // the largest manifest and file list fit one reliable message (64 KiB) with the header
        assert!(8 + size_of::<ModManifestHead>() + MAX_MODS * size_of::<ModRow>() <= 64 * 1024);
        assert!(8 + size_of::<ModFilesHead>() + MAX_FILES * size_of::<ModFileRow>() <= 64 * 1024);
        assert!(8 + size_of::<ModChunkHead>() + MAX_CHUNK <= 64 * 1024);
        // every game <-> sidecar record fits one ring slot
        for r in RECORDS.iter().filter(|r| r.flow & (G2S | S2G) != 0) {
            assert!(r.head.size() <= crate::ring::MAX_PAYLOAD, "{} does not fit a ring slot", r.name);
            assert_eq!(r.max_rows, 0, "{} must be a fixed record (ring)", r.name);
        }
        for r in RECORDS {
            assert!(is_mods_kind(r.kind) && r.kind >> 8 == 0x09, "{}", r.name);
        }
    }

    #[test]
    fn chunk_round_trip_and_bounds() {
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let m = crate::wire::encode(0, 0, &ModChunkHead { offset: 4096, req_id: 7, n: 0, file: 3, _r: [0; 6] }, &data);
        let (h, v) = crate::wire::decode::<ModChunkHead>(&m).unwrap();
        assert_eq!(h.kind, K_MOD_CHUNK);
        assert_eq!((v.head.offset, v.head.req_id, v.head.file, v.rows.len()), (4096, 7, 3, 1000));
        assert_eq!(&v.rows[..], &data[..]);
        // a count that disagrees with the bytes is refused
        let mut bad = m.clone();
        bad[8 + 12..8 + 16].copy_from_slice(&999u32.to_le_bytes());
        assert!(crate::wire::decode::<ModChunkHead>(&bad).is_err());
        // more than MAX_CHUNK bytes is refused
        let full = vec![0u8; MAX_CHUNK];
        let mut m = crate::wire::encode(0, 0, &ModChunkHead { offset: 0, req_id: 1, n: 0, file: 0, _r: [0; 6] }, &full);
        assert!(crate::wire::decode::<ModChunkHead>(&m).is_ok());
        m.push(0);
        m[8 + 12..8 + 16].copy_from_slice(&(MAX_CHUNK as u32 + 1).to_le_bytes());
        assert!(crate::wire::decode::<ModChunkHead>(&m).is_err());
        // requests: 1..=MAX_CHUNK bytes, a file row inside the protocol limit
        let ok = ModChunkReq { offset: 0, req_id: 1, len: MAX_CHUNK as u32, file: 0, _r: [0; 6] };
        assert!(view::<ModChunkReq>(bytemuck::bytes_of(&ok)).is_ok());
        for bad in [ModChunkReq { len: 0, ..ok }, ModChunkReq { len: MAX_CHUNK as u32 + 1, ..ok },
                    ModChunkReq { file: MAX_FILES as u16, ..ok }] {
            assert!(view::<ModChunkReq>(bytemuck::bytes_of(&bad)).is_err());
        }
    }

    #[test]
    fn codes_are_range_checked() {
        let p = ModProgress { state: mod_state::READY, ..Default::default() };
        assert!(view::<ModProgress>(bytemuck::bytes_of(&p)).is_ok());
        for bad in [ModProgress { state: 0, ..p }, ModProgress { state: 8, ..p }, ModProgress { code: 10, ..p }] {
            assert!(view::<ModProgress>(bytemuck::bytes_of(&bad)).is_err());
        }
        let d = ModDecision { op: mod_op::ACCEPT, ..Default::default() };
        assert!(view::<ModDecision>(bytemuck::bytes_of(&d)).is_ok());
        assert!(view::<ModDecision>(bytemuck::bytes_of(&ModDecision { op: 4, ..d })).is_err());
        let r = ModReady { result: mod_result::LOADED, ..Default::default() };
        assert!(view::<ModReady>(bytemuck::bytes_of(&r)).is_ok());
        assert!(view::<ModReady>(bytemuck::bytes_of(&ModReady { result: 0, ..r })).is_err());
        let e = ModEntry { index: 1, n: 2, ..Default::default() };
        assert!(view::<ModEntry>(bytemuck::bytes_of(&e)).is_ok());
        assert!(view::<ModEntry>(bytemuck::bytes_of(&ModEntry { index: 2, ..e })).is_err());
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let mut x = 0x2545F4914F6CDD1Du64;
        for r in RECORDS {
            for len in [0usize, 1, 7, 24, 40, 56, 88, 168, 184, 336, 360, 4096, 40_000] {
                let mut b = vec![0u8; len];
                for v in b.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *v = x as u8;
                }
                let _ = (r.check)(&b);
            }
        }
    }
}

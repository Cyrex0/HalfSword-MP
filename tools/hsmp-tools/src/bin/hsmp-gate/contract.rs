//! `check_events` (G0 check `events`, `hsmp-gate check-events`): the event contract between
//! the REAL emitters and the gate's rules.
//!
//! Without it the gate can read fields nobody writes (`spawn_verified.dist_m`, `z_ok`,
//! `min_peer_m`, a `hitch` no code emits) while its own fixtures and fake game synthesise
//! exactly those shapes, so every self-test stays green. This lint ties the three
//! together statically:
//!
//! * emitters are extracted from the code that writes them:
//!   - Lua (release-set mods + `shared/`): `ev("name", {..})`, `self:ev(..)`, `ctx.ev(..)`,
//!     `pcall(api.ev, "name", f)`, `HL.event/emit(..)` and the `hsmp_log` wrappers
//!     (`HL.hitch(ms, {..})`, `HL.frame(..)`, ...), with a table literal or a local table
//!     (`local f = {..}` + `f.k = ..`) as the fields;
//!   - Rust `events::emit("name", json!({..}))` in `server/src` (hsmp-server / hsmp-sidecar
//!     `--events`), the sidecar tap's S2G `cmd_result` record (`session_client::result_line` payload),
//!     the netsim proxy's `--events`, and the gate's own fake game;
//! * [`CONTRACT`] lists every event the emitters write and, for the events the gate judges,
//!   the fields it reads and the fields it knows but ignores;
//! * failures: an emitter writes an event (or, for a judged event, a field) the contract does
//!   not know; the gate reads a field no real emitter writes (unless a [`PENDING`] request
//!   records who must add it); the rules' source reads a field literal the contract does not
//!   declare; a Lua event name outside `hsmp_log`'s vocabulary (written as `_bad_event` at
//!   runtime) unless recorded in [`PENDING_VOCAB`].
//!
//! A PENDING entry is a note, never a pass: the rule that reads the field reports
//! INCOMPLETE until the emitter exists.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Src {
    /// a game mod through shared/hsmp_log.lua (hsmp_events.jsonl)
    Lua,
    /// hsmp-server --events (server.jsonl)
    Server,
    /// hsmp-sidecar --events
    Sidecar,
    /// the S2G cmd_result records of the sidecar's tap (inst<N>/ipc_tap.jsonl)
    Results,
    /// hsmp-tools netsim --events (netsim<i>.jsonl)
    Netsim,
    /// hsmp-gate fake-game (must use the real shapes too)
    Fake,
}

pub struct Entry {
    pub ev: &'static str,
    pub src: &'static [Src],
    /// fields the gate's rules read (each needs a real emitter or a PENDING request)
    pub read: &'static [&'static str],
    /// fields an emitter writes that the gate knows and ignores
    pub known: &'static [&'static str],
    /// the event is optional evidence: a missing emitter for a read field is a note
    pub optional: bool,
}

const L: &[Src] = &[Src::Lua, Src::Fake];
const SV: &[Src] = &[Src::Server];
const SC: &[Src] = &[Src::Sidecar];

const fn e(ev: &'static str, src: &'static [Src], read: &'static [&'static str], known: &'static [&'static str]) -> Entry {
    Entry { ev, src, read, known, optional: false }
}

/// Envelope fields every emitter writes (hsmp_log / events.rs / netsim).
pub const ENVELOPE: &[&str] = &["v", "ev", "inst", "mod", "seq", "t_ms", "wall_ms", "_missing", "listen"];

/// The contract. `read` = what rules.rs / soak.rs / wait.rs judge; `known` = written, ignored.
/// Events with an empty `read` are known by name only (their fields are not checked).
pub const CONTRACT: &[Entry] = &[
    // --- game (Lua) ---------------------------------------------------------------------
    e("lobby_ready", L, &["epoch", "notice"], &["peer_id", "role", "arena"]),
    e("cmd_sent", L, &["cmd", "cmd_id"], &["arg", "backend", "via"]),
    e("cmd_result", &[Src::Lua, Src::Fake, Src::Server, Src::Sidecar, Src::Results],
      &["cmd", "cmd_id", "ok", "reason", "source", "player"],
      &["state", "ms", "tries", "seat", "peer_id", "code", "config_rev", "reason_code", "reason_text"]),
    e("travel", L, &["to", "reason"], &["from", "by", "round", "match_id"]),
    e("world_ready", L, &["arena"], &["world_key", "round", "server_arena", "match_id"]),
    e("spawn_verified", L, &["ok", "vitals_ok", "gi_ok", "dist_cm", "dest_cm", "offset_cm", "tol_cm", "x", "y", "z", "snap_z", "load_error"],
      &["round", "arena", "world_key", "spawn_id", "steps", "ms"]),
    e("kit_verified", L, &["who", "armour_n", "r_class", "l_class", "ok"],
      &["exp_armour_n", "round", "kit", "tries", "error", "r_visible", "l_visible"]),
    e("willie_census", L, &["visible", "expected", "at"], &["extras", "missing", "round", "detail"]),
    e("save_redirected", L, &["fn", "slot", "to_slot", "op", "ok"], &["how"]),
    e("x_save_guard", L, &["active"], &["why", "session", "ok"]),
    e("x_career_recover", L, &["code", "ms"], &[]),   // HSMPMenu boot-time career-guard recovery
    e("native_travel_rewritten", L, &["from", "to", "soft"], &["phase", "n"]),
    // shared/hsmp_rvp.lua let a travel go while the Runtime Vertex Paint queue was still busy
    // (MAX_HOLD_S elapsed): that travel carries the RVP crash risk (docs/development/crash-rr.md).
    e("rvp_hold_timeout", L, &[], &["why", "waited_s", "paint", "detect"]),
    // Stall probe (release profile; docs/development/testing.md "Stall probe").
    e("hitch", L, &["ms", "travel", "world_key"], &[]),
    e("frame_hb", L, &["max_ms"], &["n"]),
    e("ready_report", L, &[], &["round", "arena", "load_error"]),
    e("pose_quality", L, &["peer", "arm_p95_uu", "tip_p95_uu", "latency_ms", "jitter_ratio", "foot_slide_p95", "idle_rms"], &["round"]),
    // SMOOTH-1 (replication.md "Rubber banding"): HSMPAvatars per stand-in every 5 s; HSMPSync spawn_place / HSMPAvatars launch clamp
    e("netfeel", L, &["peer", "snaps_per_min", "rigid_snaps", "clock_resets", "jump_max_uu", "window_s"], &["frames", "buffer_ms", "jitter_ms", "round"]),
    e("pawn_correction", L, &["why", "live", "dist_cm"], &["speed", "round"]),
    // SPAWN-1: HSMPAvatars, 3 s after a stand-in starts being driven / is re-posed, and after our pawn spawns
    e("spawn_stretch", L, &["who", "peer", "max_uu", "bone", "why"], &["frames", "round"]),
    // PAWN-1 (docs/development/subsystems/spawns.md): HSMPSync spawn_place emit_state + HSMPLoadout kit.lua
    e("pawn_state", L, &["at", "protected", "downed", "consciousness", "dist_cm", "weapon_r", "weapon_l", "reason"],
      &["round", "pawn", "fallen", "health", "live", "who"]),
    // WORLD-1 (docs/development/subsystems/world-replication.md): HSMPWorld (Lua) and the server's pairing
    e("world_consistency", &[Src::Lua, Src::Server], &["compared", "mismatched", "hash_match", "level"],
      &["mismatched_n", "hash_equal", "peer", "other", "epoch", "world", "kinds", "f_seq"]),
    // WORLD-2 (world-replication.md "Measuring sync"): HSMPWorld, harness runs (tracks) and every 5 s (quality)
    e("world_track", L, &["nid", "t", "x", "y", "z", "rest", "level", "epoch"], &["qx", "qy", "qz", "qw", "mode", "owner"]),
    e("world_sync_quality", L, &["hard_snaps"], &["max_off_cm", "lost_races", "takeovers", "poked", "follow_ticks", "window_s", "level", "epoch"]),
    // COMBAT-1 (docs/development/subsystems/combat.md): HSMPCombat every 5 s of combat during Live
    e("combat_quality", L, &["claims", "accepted", "pending", "rejected_by_reason"], &["confirmed", "clashes", "round", "window_s"]),
    e("x_combat_quality", L, &["claims", "accepted", "pending", "rejected_by_reason"], &["confirmed", "clashes", "round", "window_s"]),
    e("x_pose_contact", L, &[], &[]),   // stand-in contact impulses (diagnostic, not gated)
    // DoD-8: attributes a career-file mtime change to a pre-session native write (guard off)
    e("x_save_call", L, &["fn", "slot", "active"], &["err"]),
    e("x_autotest_cmd", L, &[], &[]),
    e("conn_state", L, &[], &[]),
    e("travel_reason", L, &[], &[]),
    e("resume", L, &[], &[]),
    // hsmp_log `lua_error` events are extra SOAK-LUAERR evidence; UE4SS.log is the primary one.
    Entry { ev: "lua_error", src: L, read: &["mod", "error", "msg"], known: &[], optional: true },
    e("_open", L, &[], &[]),
    e("_rotated", L, &[], &[]),
    e("_bad_event", L, &[], &[]),
    // --- hsmp-server --events ---------------------------------------------------------------
    e("phase", &[Src::Server, Src::Sidecar], &["from", "to", "round", "match_id", "arena", "frozen_arena", "epoch"], &[]),
    e("arena_picked", SV, &["arena"], &["by"]),
    // who is admin changed; the no-admin lobby's auto start
    // (state armed | cancelled | started | refused).
    e("admin_changed", SV, &["admin_peer_id"], &["admins"]),
    e("auto_start", SV, &["state"], &["players", "delay_s", "arena", "detail"]),
    e("seat_restored", SV, &["same_seat", "same_wins"], &["player_key", "seat", "old_seat", "wins", "match_id", "how"]),
    e("load_failed", SV, &["round", "nick", "error"], &["peer_id", "seat", "match_id"]),
    e("round_void", SV, &["round"], &["fighters", "streak", "match_id"]),
    e("server_start", SV, &[], &[]),
    e("server_stop", SV, &[], &[]),
    e("link_stall", SV, &[], &[]),
    e("forfeit", SV, &[], &[]),
    e("cmd_dup", SV, &[], &[]),
    e("debug_kill", SV, &[], &[]),
    e("death_ignored", SV, &[], &[]),
    // session resume, a paused round resuming, a drop with no
    // pause left losing the round. Known by name; not judged yet.
    e("session_resumed", &[Src::Server, Src::Sidecar], &[], &[]),
    e("round_resumed", SV, &[], &[]),
    e("drop_forfeit_round", SV, &[], &[]),
    // NAT traversal (server/src/nat): a relayed punch answered with probes. Not judged.
    e("nat_punch", SV, &[], &["to", "nonce"]),
    // server mods (docs/hosting/server-mods.md): a joining player's set loaded / declined / failed
    e("server_mods", SV, &[], &["peer_id", "result", "ms", "failed"]),
    // --- hsmp-sidecar --events -----------------------------------------------------------------
    e("cmd_sent", SC, &[], &[]),
    e("cmd_timeout", SC, &["cmd", "cmd_id", "tries"], &[]),
    // which game process the sidecar watches (exits with it); informational.
    e("parent_watch", SC, &["target"], &[]),
    e("session_epoch", SC, &[], &[]),
    e("notice", SC, &[], &[]),
    e("leave_request", SC, &[], &[]),
    e("link_resumed", SC, &[], &[]),
    e("parent_exit", SC, &[], &[]),
    e("sidecar_exit", SC, &[], &[]),
    // NAT traversal on join (sidecar/traversal.rs): start, relayed, connected, blocked; the
    // host's first probe arriving. Not judged.
    e("nat_traversal", SC, &[], &["server", "step", "try", "tries"]),
    e("nat_probe_rx", SC, &[], &["from"]),
    // attach / refusal of the game's segment (docs/development/ipc-shared-memory.md).
    e("ipc_attached", SC, &["name", "abi", "caps"], &[]),
    e("ipc_refused", SC, &["code", "detail"], &[]),
    // DoD-8: the career file guard's actions; --events and <state>/.career_guard.jsonl
    // (collected into inst<i>/; the game launches the sidecar without --events).
    e("career_guard", SC, &["action", "file", "why", "kind"], &["backup", "pid"]),
    // --- netsim proxy ---------------------------------------------------------------------------
    e("netsim_start", &[Src::Netsim], &["profile", "loss"],
      &["upstream", "pid", "delay", "jitter", "dup", "spike_every", "spike_ms", "spike_len", "loss_up", "loss_down", "loss_burst",
        "reorder_pct", "reorder_ms", "jitter_tau_ms", "jitter_model", "rate_up_kbps", "rate_down_kbps", "queue_ms", "fifo"]),
    e("netsim_mode", &[Src::Netsim], &[], &[]),
    // game traffic through the proxy (cumulative counters every 5 s); the proxy's own
    // scheduling lateness per window (late_*), and the measured loss vs the profile
    e("netsim_stats", &[Src::Netsim], &["in", "out", "clients", "lost", "late_n", "late_p99_ms", "late_max_ms"],
      &["listen", "dup", "blackout_dropped", "queued", "late_p50_ms", "up", "down", "lost_up", "lost_down", "queue_dropped", "reordered"]),
    e("netsim_exit", &[Src::Netsim], &[], &[]),
];

pub struct Pending {
    pub ev: &'static str,
    pub field: &'static str,
    pub owner: &'static str,
    pub why: &'static str,
}

/// Fields the gate reads that no emitter writes YET. Each is a recorded request; the rule
/// reading it reports INCOMPLETE until the emitter lands. Remove an entry once it is emitted
/// (a stale entry is a note).
pub const PENDING: &[Pending] = &[
    Pending { ev: "hitch", field: "ms", owner: "shared/hsmp_log.lua stall probe", why: "DoD-2 / SOAK-HITCH: hitch{ms,world_key,travel} for a frame > 2 s, release profile" },
    Pending { ev: "hitch", field: "travel", owner: "shared/hsmp_log.lua", why: "DoD-2: travel=false > 2 s fails" },
    Pending { ev: "hitch", field: "world_key", owner: "shared/hsmp_log.lua", why: "DoD-2 report: which world stalled" },
    Pending { ev: "frame_hb", field: "max_ms", owner: "shared/hsmp_log.lua", why: "DoD-2: frame_hb{n,max_ms} every 10 s proves the probe ran" },
    Pending { ev: "spawn_verified", field: "snap_z", owner: "HSMPMatch director.lua", why: "DoD-7 Z check: the slot's ground-snapped Z (HSMPSync .spawn_status.json z, the Director's p.placed[3])" },
];

/// Lua event names outside hsmp_log's M.EVENTS (and without the x_ prefix): `HL.event`
/// writes them as `_bad_event`, so they never reach the gate under their name.
pub const PENDING_VOCAB: &[(&str, &str)] = &[
    ("conn_state", "hsmp_log / director.lua: add to hsmp_log M.EVENTS or rename x_conn_state (director.lua set_conn)"),
    ("travel_reason", "hsmp_log / director.lua: add to M.EVENTS or rename x_travel_reason (director.lua)"),
    ("resume", "hsmp_log / director.lua: add to M.EVENTS or rename x_resume (director.lua)"),
    ("frame_hb", "hsmp_log: add frame_hb{n,max_ms} to M.EVENTS with the stall probe"),
    ("combat_quality", "HSMPCombat falls back to x_combat_quality while combat_quality is not in M.EVENTS"),
];

/// Field literals the gate source reads that are not event fields (harness marks / RCON
/// replies, observer samples, memwatch rows, the normaliser's own keys).
pub const GATE_INTERNAL: &[&str] = &[
    "name", "pick", "reply", "accepted", "step", "detail", "file", "state", "status", "role", "private_bytes",
    "working_set", "handles", "inst", "seq", "t_ms", "time_ms", "ts", "mod",
];

// ------------------------------------------------------------------------------------------
// extraction
// ------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Emit {
    pub src: Src,
    pub file: String,
    pub line: usize,
    pub ev: String,
    pub fields: BTreeSet<String>,
    /// the payload may carry fields the extractor could not see (merged / dynamic object)
    pub open: bool,
}

/// Lua source with comments removed and strings KEPT (same length per line is not needed;
/// newlines are kept so line numbers stay exact).
pub fn lua_strip_comments(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let long_level = |i: usize| -> Option<usize> {
        if i < n && c[i] == '[' {
            let mut j = i + 1;
            while j < n && c[j] == '=' {
                j += 1;
            }
            if j < n && c[j] == '[' {
                return Some(j - i - 1);
            }
        }
        None
    };
    let find_close = |from: usize, level: usize| -> usize {
        let mut k = from;
        while k < n {
            if c[k] == ']' {
                let mut j = k + 1;
                let mut eq = 0;
                while j < n && c[j] == '=' {
                    eq += 1;
                    j += 1;
                }
                if eq == level && j < n && c[j] == ']' {
                    return j + 1;
                }
            }
            k += 1;
        }
        n
    };
    while i < n {
        let ch = c[i];
        if ch == '-' && i + 1 < n && c[i + 1] == '-' {
            if let Some(level) = long_level(i + 2) {
                let end = find_close(i + 2 + level + 2, level);
                for k in i..end {
                    if c[k] == '\n' {
                        out.push('\n');
                    }
                }
                i = end;
            } else {
                while i < n && c[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            out.push(ch);
            i += 1;
            while i < n && c[i] != ch && c[i] != '\n' {
                if c[i] == '\\' && i + 1 < n {
                    out.push(c[i]);
                    i += 1;
                }
                out.push(c[i]);
                i += 1;
            }
            if i < n {
                out.push(c[i]);
                i += 1;
            }
            continue;
        }
        if let Some(level) = long_level(i) {
            let end = find_close(i + level + 2, level);
            for k in i..end {
                out.push(c[k]);
            }
            i = end;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Index just past the bracket that closes the one at `open` (`{`/`(`/`[`), skipping strings.
fn match_close(t: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < t.len() {
        match t[i] {
            b'"' | b'\'' => {
                let q = t[i];
                i += 1;
                while i < t.len() && t[i] != q {
                    if t[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split the inside of a bracket at top-level commas/semicolons.
fn split_top(inner: &str) -> Vec<String> {
    let t = inner.as_bytes();
    let mut parts = vec![];
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'"' | b'\'' => {
                let q = t[i];
                i += 1;
                while i < t.len() && t[i] != q {
                    if t[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b',' | b';' if depth == 0 => {
                parts.push(inner[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < inner.len() {
        parts.push(inner[start..].to_string());
    }
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// Keys of a Lua table constructor `{ a = 1, ["b"] = 2, ... }` (text including the braces).
pub fn lua_table_keys(table: &str) -> BTreeSet<String> {
    let inner = &table[1..table.len().saturating_sub(1)];
    let key = Regex::new(r#"^\s*(?:([A-Za-z_]\w*)|\[\s*"([^"]+)"\s*\])\s*=[^=]"#).unwrap();
    split_top(inner)
        .iter()
        .filter_map(|p| {
            let p = format!("{p} ");
            key.captures(&p).map(|c| c.get(1).or(c.get(2)).unwrap().as_str().to_string())
        })
        .collect()
}

/// Keys of a `json!({ "a": .., "b": .. })` object (text including the braces).
pub fn json_object_keys(obj: &str) -> BTreeSet<String> {
    let inner = &obj[1..obj.len().saturating_sub(1)];
    let key = Regex::new(r#"^\s*"([^"]+)"\s*:"#).unwrap();
    split_top(inner).iter().filter_map(|p| key.captures(p).map(|c| c[1].to_string())).collect()
}

fn line_of(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].bytes().filter(|b| *b == b'\n').count() + 1
}

/// The argument list of a call whose `(` is at `open`: (arguments, end index).
fn call_args(text: &str, open: usize) -> Option<(Vec<String>, usize)> {
    let end = match_close(text.as_bytes(), open)?;
    Some((split_top(&text[open + 1..end - 1]).into_iter().map(|s| s.trim().to_string()).collect(), end))
}

/// Fields of a Lua table argument: a literal, or a local table built before `pos`
/// (`local f = {..}` plus `f.k = ..` / `f.a, f.b = ..` assignments up to the call).
fn lua_fields(text: &str, pos: usize, arg: &str) -> Option<(BTreeSet<String>, bool)> {
    let arg = arg.trim();
    if arg.starts_with('{') {
        return Some((lua_table_keys(arg), false));
    }
    let ident = arg.split(" or ").next().unwrap_or(arg).trim();
    if !Regex::new(r"^[A-Za-z_]\w*$").unwrap().is_match(ident) {
        return None;
    }
    let before = &text[..pos];
    let decl = Regex::new(&format!(r"(?:local\s+)?\b{ident}\s*=\s*(?:{ident}\s+or\s+)?\{{")).unwrap();
    let m = decl.find_iter(before).last()?;
    let mut keys = BTreeSet::new();
    let open = m.end() - 1;
    let from = match_close(text.as_bytes(), open)?;
    keys.extend(lua_table_keys(&text[open..from]));
    let assign = Regex::new(&format!(r"\b{ident}\.([A-Za-z_]\w*)\s*(?:,|=[^=])")).unwrap();
    for c in assign.captures_iter(&text[from..pos]) {
        keys.insert(c[1].to_string());
    }
    Some((keys, false))
}

/// Event name argument: a string literal, or a local whose initialiser has literals.
fn lua_names(text: &str, pos: usize, arg: &str) -> Vec<String> {
    let lit = Regex::new(r#"^"([A-Za-z_][\w]*)"$"#).unwrap();
    if let Some(c) = lit.captures(arg.trim()) {
        return vec![c[1].to_string()];
    }
    let ident = arg.trim();
    if !Regex::new(r"^[A-Za-z_]\w*$").unwrap().is_match(ident) {
        return vec![];
    }
    let decl = Regex::new(&format!(r"local\s+{ident}\s*=\s*([^\n]+)")).unwrap();
    let Some(c) = decl.captures_iter(&text[..pos]).last() else { return vec![] };
    let strs = Regex::new(r#""([A-Za-z_]\w*)""#).unwrap();
    // only names that look like event names (a literal used as a table key test is skipped)
    strs.captures_iter(&c[1]).map(|m| m[1].to_string()).collect()
}

/// hsmp_log's vocabulary: M.EVENTS name -> required fields.
pub fn hsmp_log_vocab(repo: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Ok(src) = std::fs::read_to_string(repo.join("mods/shared/hsmp_log.lua")) else { return out };
    let text = lua_strip_comments(&src);
    let Some(p) = text.find("M.EVENTS") else { return out };
    let Some(open) = text[p..].find('{').map(|o| o + p) else { return out };
    let Some(close) = match_close(text.as_bytes(), open) else { return out };
    let row = Regex::new(r#"([A-Za-z_]\w*)\s*=\s*\{([^}]*)\}"#).unwrap();
    let s = Regex::new(r#""([^"]+)""#).unwrap();
    for c in row.captures_iter(&text[open + 1..close - 1]) {
        out.insert(c[1].to_string(), s.captures_iter(&c[2]).map(|m| m[1].to_string()).collect());
    }
    out
}

/// hsmp_log wrapper functions (`function M.<name>(...)`) -> the emissions inside each,
/// resolved recursively (M.frame -> M.hitch -> M.event("hitch", f)).
fn hsmp_log_wrappers(repo: &Path) -> BTreeMap<String, Vec<(String, BTreeSet<String>)>> {
    let mut out: BTreeMap<String, Vec<(String, BTreeSet<String>)>> = BTreeMap::new();
    let Ok(src) = std::fs::read_to_string(repo.join("mods/shared/hsmp_log.lua")) else { return out };
    let text = lua_strip_comments(&src);
    let fdef = Regex::new(r"(?m)^function\s+M\.([A-Za-z_]\w*)\s*\(").unwrap();
    let starts: Vec<(usize, String)> = fdef.captures_iter(&text).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
    let mut bodies: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (k, (s, name)) in starts.iter().enumerate() {
        let e = starts.get(k + 1).map(|x| x.0).unwrap_or(text.len());
        bodies.insert(name.clone(), (*s, e));
    }
    // direct emissions: M.event("lit", tbl) / M.emit("lit", tbl)
    let direct = Regex::new(r"\bM\.(event|emit)\s*\(").unwrap();
    let call = Regex::new(r"\bM\.([A-Za-z_]\w*)\s*\(").unwrap();
    fn resolve(name: &str, text: &str, bodies: &BTreeMap<String, (usize, usize)>, direct: &Regex, call: &Regex, depth: usize) -> Vec<(String, BTreeSet<String>)> {
        let mut v = vec![];
        let Some((s, e)) = bodies.get(name).copied() else { return v };
        if depth > 4 {
            return v;
        }
        let body = &text[s..e];
        for m in direct.find_iter(body) {
            let open = s + m.end() - 1;
            let Some((args, _)) = call_args(text, open) else { continue };
            if args.len() < 2 {
                continue;
            }
            for n in lua_names(text, open, &args[0]) {
                let f = lua_fields(text, open, &args[1]).map(|x| x.0).unwrap_or_default();
                v.push((n, f));
            }
        }
        for c in call.captures_iter(body) {
            let callee = c[1].to_string();
            if callee == "event" || callee == "emit" || callee == name || !bodies.contains_key(&callee) {
                continue;
            }
            let open = s + c.get(0).unwrap().end() - 1;
            let extra = call_args(text, open).and_then(|(a, _)| a.last().filter(|l| l.starts_with('{')).map(|l| lua_table_keys(l))).unwrap_or_default();
            for (n, mut f) in resolve(&callee, text, bodies, direct, call, depth + 1) {
                f.extend(extra.iter().cloned());
                v.push((n, f));
            }
        }
        v
    }
    for name in bodies.keys() {
        if matches!(name.as_str(), "event" | "emit" | "init" | "_reset" | "path" | "inst" | "echo" | "echo_from_env" | "wall_ms") {
            continue;
        }
        let r = resolve(name, &text, &bodies, &direct, &call, 0);
        if !r.is_empty() {
            out.insert(name.clone(), r);
        }
    }
    out
}

/// Release-set mods (`: 1` or `: dev` in mods/mods.release.txt) + shared/.
pub fn release_mods(repo: &Path) -> Vec<String> {
    let mut v = vec![];
    if let Ok(t) = std::fs::read_to_string(repo.join("mods/mods.release.txt")) {
        let rx = Regex::new(r"^\s*(HSMP\w+)\s*:\s*(1|dev)\s*$").unwrap();
        for l in t.lines() {
            if let Some(c) = rx.captures(l) {
                v.push(c[1].to_string());
            }
        }
    }
    v
}

fn lua_files(repo: &Path) -> Vec<PathBuf> {
    let base = repo.join("mods");
    let mut dirs: Vec<PathBuf> = release_mods(repo).iter().map(|m| hsmp_tools::paths::mod_dir(repo, m).join("Scripts")).collect();
    dirs.push(base.join("shared"));
    let mut out = vec![];
    for d in dirs {
        let mut stack = vec![d];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().and_then(|n| n.to_str()) != Some("tests") {
                        stack.push(p);
                    }
                } else if p.extension().and_then(|x| x.to_str()) == Some("lua") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn rel(repo: &Path, p: &Path) -> String {
    p.strip_prefix(repo).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

pub fn lua_emits(repo: &Path) -> Vec<Emit> {
    let wrappers = hsmp_log_wrappers(repo);
    let generic = Regex::new(r"(?:^|[^\w.:])((?:[A-Za-z_]\w*[.:])*(?:ev|event|emit))\s*\(").unwrap();
    let pcall = Regex::new(r"\bpcall\s*\(\s*((?:[A-Za-z_]\w*[.:])*(?:ev|event|emit))\s*,").unwrap();
    let hl_bind = Regex::new(r#"(?:local\s+)?([A-Za-z_][\w.]*)\s*=\s*(?:load_module|load_shared|require)\s*\(\s*"hsmp_log""#).unwrap();
    let mut out = vec![];
    for p in lua_files(repo) {
        let file = rel(repo, &p);
        if file.ends_with("shared/hsmp_log.lua") {
            continue; // the library: its wrappers count where a mod calls them
        }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        let text = lua_strip_comments(&src);
        let push = |pos: usize, args: &[String], name_idx: usize, out: &mut Vec<Emit>| {
            if args.len() <= name_idx {
                return;
            }
            let names = lua_names(&text, pos, &args[name_idx]);
            let fields = args.get(name_idx + 1).map(|a| lua_fields(&text, pos, a));
            for n in names {
                let (f, open) = match &fields {
                    Some(Some((f, o))) => (f.clone(), *o),
                    Some(None) => (BTreeSet::new(), true),
                    None => (BTreeSet::new(), false),
                };
                out.push(Emit { src: Src::Lua, file: file.clone(), line: line_of(&text, pos), ev: n, fields: f, open });
            }
        };
        for c in generic.captures_iter(&text) {
            let m = c.get(1).unwrap();
            // skip definitions (`function M.emit(`, `local function ev(`)
            let pre = &text[..m.start()];
            if pre.trim_end().ends_with("function") {
                continue;
            }
            let open = c.get(0).unwrap().end() - 1;
            if let Some((args, _)) = call_args(&text, open) {
                push(open, &args, 0, &mut out);
            }
        }
        for c in pcall.captures_iter(&text) {
            let open = c.get(0).unwrap().start() + c.get(0).unwrap().as_str().find('(').unwrap();
            if let Some((args, _)) = call_args(&text, open) {
                push(open, &args, 1, &mut out);
            }
        }
        // hsmp_log wrappers through the bound module name: HL.hitch(ms, {..}), HL.frame(...)
        let names: BTreeSet<String> = hl_bind.captures_iter(&text).map(|c| c[1].to_string()).collect();
        for hl in names {
            let w = Regex::new(&format!(r"(?:^|[^\w.]){}\.([A-Za-z_]\w*)\s*\(", regex::escape(&hl))).unwrap();
            for c in w.captures_iter(&text) {
                let Some(ems) = wrappers.get(&c[1]) else { continue };
                if matches!(&c[1], "event" | "emit") {
                    continue;
                }
                let open = c.get(0).unwrap().end() - 1;
                let extra = call_args(&text, open).and_then(|(a, _)| a.last().filter(|l| l.starts_with('{')).map(|l| lua_table_keys(l))).unwrap_or_default();
                for (n, f) in ems {
                    let mut f = f.clone();
                    f.extend(extra.iter().cloned());
                    out.push(Emit { src: Src::Lua, file: file.clone(), line: line_of(&text, open), ev: n.clone(), fields: f, open: false });
                }
            }
        }
    }
    out
}

/// Rust json!-payload emitters: `<callee>("name", json!({..}) | ident)`.
fn rust_emits_in(repo: &Path, path: &Path, callee: &Regex, src: Src) -> Vec<Emit> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    // drop `//` line comments (keeps strings intact enough for event payloads)
    let text: String = text.lines().map(|l| match l.find("//") { Some(i) if !l[..i].contains('"') => &l[..i], _ => l }).collect::<Vec<_>>().join("\n");
    // the test module is not an emitter (a lone #[cfg(test)] helper above real code is)
    let test_mod = Regex::new(r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s").unwrap();
    let text = match test_mod.find(&text) { Some(m) => text[..m.start()].to_string(), None => text };
    let file = rel(repo, path);
    let mut out = vec![];
    let quoted = Regex::new(r#"^"([A-Za-z_]\w*)"$"#).unwrap();
    for m in callee.find_iter(&text) {
        let open = m.end() - 1;
        let Some((args, _)) = call_args(&text, open) else { continue };
        if args.len() < 2 {
            continue;
        }
        let Some(name) = quoted.captures(&args[0]).map(|c| c[1].to_string()) else { continue };
        let (fields, open_payload) = rust_payload(&text, open, &args[1]);
        out.push(Emit { src, file: file.clone(), line: line_of(&text, open), ev: name, fields, open: open_payload });
    }
    out
}

fn rust_payload(text: &str, pos: usize, arg: &str) -> (BTreeSet<String>, bool) {
    let jm = Regex::new(r"^(?:serde_json::)?json!\s*\(").unwrap();
    if let Some(m) = jm.find(arg) {
        let rest = arg[m.end()..].trim_start();
        if rest.starts_with('{') {
            if let Some(end) = match_close(rest.as_bytes(), 0) {
                return (json_object_keys(&rest[..end]), false);
            }
        }
        return (BTreeSet::new(), true);
    }
    // a local built with json!({..}) (and maybe extended: open)
    let ident = arg.trim().trim_end_matches(".clone()");
    if Regex::new(r"^[A-Za-z_]\w*$").unwrap().is_match(ident) {
        let decl = Regex::new(&format!(r"let\s+(?:mut\s+)?{ident}\s*=\s*(?:serde_json::)?json!\s*\(\s*\{{")).unwrap();
        if let Some(m) = decl.find_iter(&text[..pos]).last() {
            let open = m.end() - 1;
            if let Some(close) = match_close(text.as_bytes(), open) {
                let extended = text[close..pos].contains(&format!("{ident}.as_object_mut()")) || text[close..pos].contains(".extend(");
                return (json_object_keys(&text[open..close]), extended);
            }
        }
    }
    (BTreeSet::new(), true)
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if !n.starts_with("target") {
                    stack.push(p);
                }
            } else if n.ends_with(".rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn rust_emits(repo: &Path) -> Vec<Emit> {
    let mut out = vec![];
    let emit = Regex::new(r"(?:crate::)?events::emit\s*\(").unwrap();
    for p in rs_files(&repo.join("server").join("src")) {
        let r = rel(repo, &p);
        let src = if r.contains("/sidecar/") || r.ends_with("_client.rs") { Src::Sidecar } else { Src::Server };
        out.extend(rust_emits_in(repo, &p, &emit, src));
    }
    // the sidecar's S2G cmd_result payload
    let sc = repo.join("server/src/sidecar/session_client.rs");
    if let Ok(text) = std::fs::read_to_string(&sc) {
        if let Some(p) = text.find("fn result_line") {
            if let Some(o) = text[p..].find("json!(") {
                let open = p + o + "json!(".len();
                let open = open + text[open..].find('{').unwrap_or(0);
                if let Some(close) = match_close(text.as_bytes(), open) {
                    out.push(Emit { src: Src::Results, file: rel(repo, &sc), line: line_of(&text, open), ev: "cmd_result".into(),
                                    fields: json_object_keys(&text[open..close]), open: false });
                }
            }
        }
    }
    // netsim --events
    let ns = repo.join("tools/hsmp-tools/src/cmd/netsim.rs");
    out.extend(rust_emits_in(repo, &ns, &Regex::new(r"\bev\.emit\s*\(").unwrap(), Src::Netsim));
    // the gate's fake game
    let fg = repo.join("tools/hsmp-tools/src/bin/hsmp-gate/fake_game.rs");
    out.extend(rust_emits_in(repo, &fg, &Regex::new(r"\bg\.ev\s*\(").unwrap(), Src::Fake));
    out
}

/// Field literals the gate's rules read: `sv(e, "x")`, `s(..)`, `bv(..)`, `f64v(..)`,
/// `i64v(..)`, `contains_key("x")`, plus the POSE-1 / kit loops' literal arrays.
pub fn gate_reads(repo: &Path) -> BTreeMap<String, Vec<String>> {
    let dir = repo.join("tools/hsmp-tools/src/bin/hsmp-gate");
    let call = Regex::new(r#"\b(?:sv|s|bv|f64v|i64v)\(\s*[&\w*]+\s*,\s*"([a-z_0-9]+)"\s*\)"#).unwrap();
    let ck = Regex::new(r#"contains_key\(\s*"([a-z_0-9]+)"\s*\)"#).unwrap();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in ["rules.rs", "soak.rs", "wait.rs", "events.rs"] {
        let Ok(t) = std::fs::read_to_string(dir.join(f)) else { continue };
        let t = match t.find("#[cfg(test)]") { Some(i) => t[..i].to_string(), None => t };
        for (n, line) in t.lines().enumerate() {
            for c in call.captures_iter(line).chain(ck.captures_iter(line)) {
                out.entry(c[1].to_string()).or_default().push(format!("{f}:{}", n + 1));
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// the check
// ------------------------------------------------------------------------------------------

#[derive(Default)]
pub struct Report {
    pub fails: Vec<String>,
    pub notes: Vec<String>,
    pub emits: usize,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.fails.is_empty()
    }
    pub fn summary(&self) -> String {
        let mut s = format!("{} emitter call sites; {} violation(s)", self.emits, self.fails.len());
        if !self.fails.is_empty() {
            s += &format!(": {}", self.fails.iter().take(8).cloned().collect::<Vec<_>>().join(" | "));
        }
        if !self.notes.is_empty() {
            s += &format!("; {} pending request(s)", self.notes.len());
        }
        s
    }
}

fn entry_for(ev: &str, src: Src) -> Option<&'static Entry> {
    CONTRACT.iter().find(|e| e.ev == ev && e.src.contains(&src))
}

pub fn evaluate(emits: &[Emit], vocab: &BTreeMap<String, Vec<String>>, reads: &BTreeMap<String, Vec<String>>) -> Report {
    let mut r = Report { emits: emits.len(), ..Default::default() };
    let env: BTreeSet<&str> = ENVELOPE.iter().copied().collect();
    // (a) every emitted event / judged field is known to the gate
    for m in emits {
        let at = format!("{}:{}", m.file, m.line);
        let Some(ent) = entry_for(&m.ev, m.src) else {
            r.fails.push(format!("{at}: {:?} emits {:?}, which the gate's contract does not know (add it to contract::CONTRACT)", m.src, m.ev));
            continue;
        };
        if !ent.read.is_empty() {
            for f in &m.fields {
                if !env.contains(f.as_str()) && !ent.read.contains(&f.as_str()) && !ent.known.contains(&f.as_str()) {
                    r.fails.push(format!("{at}: {}.{f} is not in the contract (read or known)", m.ev));
                }
            }
        }
        if m.src == Src::Lua && !m.ev.starts_with("x_") && !m.ev.starts_with('_') && !vocab.is_empty() && !vocab.contains_key(&m.ev) {
            match PENDING_VOCAB.iter().find(|(n, _)| *n == m.ev) {
                Some((_, who)) => r.notes.push(format!("{at}: {} is not in hsmp_log M.EVENTS (written as _bad_event): {who}", m.ev)),
                None => r.fails.push(format!("{at}: {} is not in hsmp_log M.EVENTS and has no x_ prefix: HL.event writes it as _bad_event", m.ev)),
            }
        }
    }
    // (b) every field the gate reads has a real emitter (the fake game does not count)
    for ent in CONTRACT {
        for f in ent.read {
            let real = ent.src.iter().filter(|s| **s != Src::Fake);
            let mut emitted = false;
            for s in real {
                if emits.iter().any(|m| m.src == *s && m.ev == ent.ev && (m.fields.contains(*f) || m.open)) {
                    emitted = true;
                }
            }
            let pend = PENDING.iter().find(|p| p.ev == ent.ev && p.field == *f);
            match (emitted, pend) {
                (true, Some(p)) => r.notes.push(format!("{}.{f} is now emitted: remove the PENDING request ({})", ent.ev, p.owner)),
                (true, None) => {}
                (false, Some(p)) => r.notes.push(format!("PENDING {}.{f}: {} - {}", ent.ev, p.owner, p.why)),
                (false, None) if ent.optional => r.notes.push(format!("optional evidence {}.{f} has no emitter", ent.ev)),
                (false, None) => r.fails.push(format!("the gate reads {}.{f} but no real emitter writes it (fix the rule or record a PENDING request)", ent.ev)),
            }
        }
    }
    // (c) every field literal the rules read is declared
    let declared: BTreeSet<&str> = CONTRACT.iter().flat_map(|e| e.read.iter().copied()).chain(GATE_INTERNAL.iter().copied()).chain(ENVELOPE.iter().copied()).collect();
    for (f, at) in reads {
        if !declared.contains(f.as_str()) {
            r.fails.push(format!("the gate reads field {f:?} ({}) that no contract entry declares", at.join(", ")));
        }
    }
    r
}

/// Every event in the gate's fixtures (scripts/fixtures/*): hsmp_events*.jsonl and UE4SS.log
/// `[hsmp_ev]` lines (Lua), server.jsonl (Server), netsim*.jsonl (Netsim),
/// inst*/ipc_tap.jsonl S2G cmd_result payloads (Results). Fixtures may only use contract shapes; they never
/// count as an emitter.
pub fn fixture_emits(repo: &Path) -> Vec<Emit> {
    let mut out = vec![];
    let root = repo.join("scripts").join("fixtures");
    let Ok(rd) = std::fs::read_dir(&root) else { return out };
    let mut cases: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    cases.sort();
    let mut files: Vec<(PathBuf, Src)> = vec![];
    for c in cases {
        for e in std::fs::read_dir(&c).into_iter().flatten().flatten() {
            let p = e.path();
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if p.is_dir() && n.starts_with("inst") {
                for f in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                    let fp = f.path();
                    let fname = fp.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    if fname.starts_with("hsmp_events") && fname.ends_with(".jsonl") {
                        files.push((fp, Src::Lua));
                    } else if fname == crate::events::TAP_FILE {
                        files.push((fp, Src::Results));
                    } else if fname == ".career_guard.jsonl" {
                        files.push((fp, Src::Sidecar));
                    }
                }
            } else if n == "server.jsonl" {
                files.push((p, Src::Server));
            } else if n.starts_with("netsim") && n.ends_with(".jsonl") {
                files.push((p, Src::Netsim));
            } else if n == "UE4SS.log" {
                files.push((p, Src::Lua));
            }
        }
    }
    for (p, src) in files {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let file = rel(repo, &p);
        for (k, line) in text.lines().enumerate() {
            let body = match line.find("[hsmp_ev]") {
                Some(i) => line[i + 9..].trim(),
                None if p.extension().and_then(|x| x.to_str()) == Some("log") => continue,
                None => line.trim(),
            };
            let Ok(serde_json::Value::Object(mut o)) = serde_json::from_str::<serde_json::Value>(body) else { continue };
            if src == Src::Results {
                // a tap line: only S2G cmd_result records count, judged by their payload
                if o.get("ev").and_then(|v| v.as_str()) != Some("s2g") || o.get("kind").and_then(|v| v.as_str()) != Some("cmd_result") {
                    continue;
                }
                let Some(serde_json::Value::Object(v)) = o.remove("v") else { continue };
                o = v;
            }
            let ev = match (src, o.get("ev").and_then(|v| v.as_str())) {
                (Src::Results, _) => "cmd_result".to_string(),
                (_, Some(e)) => e.to_string(),
                _ => continue,
            };
            out.push(Emit { src, file: file.clone(), line: k + 1, ev, fields: o.keys().cloned().collect(), open: false });
        }
    }
    out
}

/// (a) for fixtures: every event and judged field is a contract shape.
fn check_fixtures(fx: &[Emit], r: &mut Report) {
    let env: BTreeSet<&str> = ENVELOPE.iter().copied().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for m in fx {
        let at = format!("{}:{}", m.file, m.line);
        let Some(ent) = entry_for(&m.ev, m.src) else {
            if seen.insert(format!("ev {} {:?}", m.ev, m.src)) {
                r.fails.push(format!("{at}: fixture uses {:?} event {:?} outside the contract", m.src, m.ev));
            }
            continue;
        };
        if ent.read.is_empty() {
            continue;
        }
        for f in &m.fields {
            if !env.contains(f.as_str()) && !ent.read.contains(&f.as_str()) && !ent.known.contains(&f.as_str()) && seen.insert(format!("{}.{f}", m.ev)) {
                r.fails.push(format!("{at}: fixture field {}.{f} is not a contract field (invented shape)", m.ev));
            }
        }
    }
}

pub fn check(repo: &Path) -> Report {
    if !hsmp_tools::paths::has_mods(repo) {
        return Report { notes: vec!["no mods: skipped".into()], ..Default::default() };
    }
    let mut emits = lua_emits(repo);
    emits.extend(rust_emits(repo));
    let mut r = evaluate(&emits, &hsmp_log_vocab(repo), &gate_reads(repo));
    check_fixtures(&fixture_emits(repo), &mut r);
    r
}

/// `hsmp-gate check-events [--verbose]`
pub fn run_cli(repo: &Path, verbose: bool) -> i32 {
    let mut emits = lua_emits(repo);
    emits.extend(rust_emits(repo));
    if verbose {
        for m in &emits {
            println!("{:?} {}:{} {} {:?}{}", m.src, m.file, m.line, m.ev, m.fields, if m.open { " +open" } else { "" });
        }
    }
    let mut rep = evaluate(&emits, &hsmp_log_vocab(repo), &gate_reads(repo));
    check_fixtures(&fixture_emits(repo), &mut rep);
    for n in &rep.notes {
        println!("note: {n}");
    }
    for f in &rep.fails {
        println!("FAIL: {f}");
    }
    println!("check-events: {}", rep.summary());
    if rep.ok() { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_tables_and_locals() {
        let src = "local f = {\n a = 1, b = { c = 2 }, [\"d\"] = 3, -- e = 4\n}\nf.g = 5\nf.h, f.i = 1, 2\nself:ev(\"pawn_state\", f)\nctx.ev(\"cmd_sent\", { cmd = x, cmd_id = y, arg = (a ~= \"\" and a or nil) })\n";
        let t = lua_strip_comments(src);
        assert!(!t.contains("e = 4"));
        let p = t.find("self:ev").unwrap();
        let (k, _) = lua_fields(&t, p, "f").unwrap();
        assert_eq!(k.into_iter().collect::<Vec<_>>(), vec!["a", "b", "d", "g", "h", "i"]);
        let q = t.find("ctx.ev").unwrap();
        let open = q + "ctx.ev".len();
        let (args, _) = call_args(&t, open).unwrap();
        assert_eq!(lua_table_keys(&args[1]).into_iter().collect::<Vec<_>>(), vec!["arg", "cmd", "cmd_id"]);
    }

    #[test]
    fn json_keys() {
        let k = json_object_keys(r#"{ "from": phase_label(from), "to": x, "nested": json!({"a": 1}), "round": r }"#);
        assert_eq!(k.into_iter().collect::<Vec<_>>(), vec!["from", "nested", "round", "to"]);
    }

    fn em(src: Src, ev: &str, f: &[&str]) -> Emit {
        Emit { src, file: "x".into(), line: 1, ev: ev.into(), fields: f.iter().map(|s| s.to_string()).collect(), open: false }
    }

    #[test]
    fn contract_violations() {
        let vocab: BTreeMap<String, Vec<String>> = [("spawn_verified".to_string(), vec![]), ("travel".to_string(), vec![])].into_iter().collect();
        // an emitter renames dist_cm -> dist_m: unknown field AND the read field loses its emitter
        let emits = vec![em(Src::Lua, "spawn_verified", &["ok", "vitals_ok", "gi_ok", "dist_m", "x", "y", "z", "load_error"])];
        let r = evaluate(&emits, &vocab, &BTreeMap::new());
        assert!(r.fails.iter().any(|f| f.contains("spawn_verified.dist_m is not in the contract")), "{:?}", r.fails);
        assert!(r.fails.iter().any(|f| f.contains("reads spawn_verified.dist_cm")), "{:?}", r.fails);
        // an unknown event name
        let r = evaluate(&[em(Src::Server, "brand_new", &[])], &vocab, &BTreeMap::new());
        assert!(r.fails.iter().any(|f| f.contains("\"brand_new\"")));
        // the gate reads an undeclared literal
        let reads: BTreeMap<String, Vec<String>> = [("min_peer_m".to_string(), vec!["rules.rs:1".to_string()])].into_iter().collect();
        let r = evaluate(&[], &vocab, &reads);
        assert!(r.fails.iter().any(|f| f.contains("min_peer_m")));
        // a Lua name outside the vocabulary
        let r = evaluate(&[em(Src::Lua, "pawn_state", &[])], &vocab, &BTreeMap::new());
        assert!(r.fails.iter().any(|f| f.contains("pawn_state is not in hsmp_log M.EVENTS")));
        // PENDING fields are notes, not failures
        let r = evaluate(&[], &vocab, &BTreeMap::new());
        assert!(r.notes.iter().any(|n| n.contains("PENDING hitch.ms")));
        assert!(!r.fails.iter().any(|f| f.contains("hitch.")), "{:?}", r.fails);
    }

    #[test]
    fn fixtures_cannot_invent_shapes() {
        let mut r = Report::default();
        check_fixtures(&[em(Src::Lua, "spawn_verified", &["v", "ev", "ok", "dist_m", "z_ok"]), em(Src::Server, "made_up", &[])], &mut r);
        assert!(r.fails.iter().any(|f| f.contains("spawn_verified.dist_m")), "{:?}", r.fails);
        assert!(r.fails.iter().any(|f| f.contains("spawn_verified.z_ok")));
        assert!(r.fails.iter().any(|f| f.contains("\"made_up\"")));
        let mut ok = Report::default();
        check_fixtures(&[em(Src::Lua, "spawn_verified", &["ok", "dist_cm", "snap_z"]), em(Src::Results, "cmd_result", &["cmd_id", "reason_text"])], &mut ok);
        assert!(ok.fails.is_empty(), "{:?}", ok.fails);
    }

    /// The real tree satisfies the contract (the same check G0 runs).
    #[test]
    fn tree_satisfies_contract() {
        let repo = hsmp_tools::paths::repo_root().unwrap();
        let r = check(&repo);
        assert!(r.ok(), "{}", r.fails.join("\n"));
        assert!(r.emits > 30, "extractor found only {} emitters", r.emits);
    }
}

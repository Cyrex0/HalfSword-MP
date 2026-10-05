//! Offline tests of the HSMPHud mod (in-match HUD) under Lua 5.4 (mlua), with
//! a mocked UE4SS + UMG (`tests/mock_ue.lua`). No game needed.
//!
//!     cargo test -p hsmp-hud-test
//!
//! * Loads the real `HSMPHud/Scripts/main.lua` (which loads hud_kit, hud_state,
//!   hud_model and hud_view) against the mock.
//! * Drives every state through the typed session records (`link`, `session`, the peer
//!   directory, bus `conn_state` / `spectate`, `notice` events) and the typed combat records
//!   (my `vitals`, each opponent's `peer_vitals`, `death` events) and checks what the HUD shows: round banner + timer, score
//!   strip, own and opponents' HP/ST bars, kill feed (+ fade), TAB scoreboard
//!   (hold and toggle fallback), net indicator (good / fair / bad / stale /
//!   no link) and every centre phase message.
//! * Hiding: menu world, no MP session, native pause menu, lobby state.
//! * Hosting: inside UI_HUD_C when it exists (no viewport widget), viewport
//!   fallback otherwise (z 990, HitTestInvisible), native HUD lost mid-world,
//!   resolution change, and the world guard (no freed widget touched after a
//!   level change).
//! * Input: no Button / CheckBox / EditableTextBox, no widget ever Visible
//!   (hit-testable), no input-mode / cursor calls, TAB the only key, no "< >".
//! * Layout at 1280x720 (canvas and DPI-scaled), 1920x1080, 2560x1440,
//!   2560x1080, 3840x2160 (DPI 1 and 2): inside the canvas, no text overlap,
//!   texts fit (ui_kit CHAR_W / LINE_H), fonts >= 9, HUD regions disjoint.

use mlua::{FromLuaMulti, Lua, Table};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const MOCK: &str = include_str!("mock_ue.lua");

/// The sidecar-written peer directory (`HUDT.peers`: id, nick, rtt_ms; slot = id, like the
/// file mock's peer slots), typed S2G events (`HUDT.ev`), and no sidecar heartbeat yet.
const PEER_DIR_SHIM: &str = r#"
HUDT = { peers = {}, ev = {} }
HSMPNative._st.hb_age = 1e9
HSMPNative.peers = function(out)
    local n = 0
    for i, e in ipairs(HUDT.peers) do
        n = i
        local x = out[i] or {}
        out[i] = x
        for k in pairs(x) do x[k] = nil end
        x.id, x.slot, x.gen, x.nick, x.rtt_ms = e.id, e.id, 1, e.nick, e.rtt_ms or 0
        x.play_seq, x.root_seq, x.vitals_seq, x.kit_seq = 0, 0, 0, 0
    end
    for i = n + 1, #out do out[i] = nil end
    return n
end
local base_poll = HSMPNative.poll
HSMPNative.poll = function(max, out)
    local n = base_poll(max, out)
    while #HUDT.ev > 0 and n < max do
        n = n + 1
        local e = out[n] or {}
        out[n] = e
        local r = table.remove(HUDT.ev, 1)
        e.kind, e.req_id, e.data, e.peer, e.aux = r.kind, 0, r.data, 0, 0
    end
    return n
end
"#;

/// A Lua string literal.
fn lstr(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/HSMPHud/Scripts")
}

fn lua_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// --- check collector -------------------------------------------------------------------------

#[derive(Default)]
struct C {
    pass: usize,
    fails: Vec<String>,
}

impl C {
    fn check(&mut self, cond: bool, msg: impl Into<String>) {
        if cond {
            self.pass += 1;
        } else {
            let m = msg.into();
            eprintln!("FAIL: {m}");
            self.fails.push(m);
        }
    }
    fn finish(self, what: &str) {
        eprintln!("{what}: {} checks passed, {} failed", self.pass, self.fails.len());
        assert!(self.fails.is_empty(), "{what}: {} failure(s):\n{}", self.fails.len(), self.fails.join("\n"));
    }
}

// --- the HUD under test ----------------------------------------------------------------------

#[derive(Clone)]
struct Sidecar {
    status: String,
    my: u32,
    peers: Vec<(u32, String)>,
    ping: BTreeMap<u32, u32>,
    reason: String,
}

/// The match id every `mt` session and `mode` record carries.
const TEST_MATCH: u64 = 9001;

struct Mt {
    state: &'static str,
    rnd: u32,
    cd: u32,
    best_of: u32,
    score: Vec<(u32, u32, u32)>,
    waiting: Vec<u32>,
    winner: u32,
    reason: &'static str,
    arena: &'static str,
    /// Live with a round time limit: seconds left (the session deadline).
    round_left: Option<u32>,
}

impl Default for Mt {
    fn default() -> Self {
        Mt { state: "live", rnd: 1, cd: 0, best_of: 3, score: vec![(1, 0, 1), (2, 0, 1)], waiting: vec![],
             winner: 0, reason: "", arena: "Map_Arena_Pit", round_left: None }
    }
}

#[derive(Debug, Clone)]
struct W {
    cls: String,
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    fs: f64,
    anchor: String,
}

static DIR_N: AtomicU32 = AtomicU32::new(0);

struct Hud {
    lua: Lua,
    sd: PathBuf,
    cw: f64,
    ch: f64,
    last_tick: u32,
    sidecar: Option<Sidecar>,
    /// Metrics fields of the link record (Lua table fields), None = no sample yet.
    metrics: Option<String>,
    /// The vitals context of the last `mt` (round, countdown): vitals belong to the current life.
    ctx: std::cell::Cell<(u32, bool)>,
}

impl Drop for Hud {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.sd);
    }
}

impl Hud {
    fn new(vw: f64, vh: f64, scale: Option<f64>, banner: bool, hud_present: bool) -> Hud {
        Hud::new_env(vw, vh, scale, Some(if banner { "1" } else { "0" }), hud_present)
    }

    /// `banner_env`: HSMP_HUD_BANNER as set in the environment (None = unset).
    fn new_env(vw: f64, vh: f64, scale: Option<f64>, banner_env: Option<&str>, hud_present: bool) -> Hud {
        let sd = std::env::temp_dir().join(format!("hsmp_hud_{}_{}", std::process::id(),
                                                   DIR_N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&sd);
        fs::create_dir_all(&sd).unwrap();
        let lua = Lua::new();
        lua.load(MOCK).set_name("@mock_ue.lua").exec().expect("mock loads");
        let (cw, ch) = match scale {
            Some(s) => ((vw / s).round(), (vh / s).round()),
            None => ((vw / (vh / 1080.0)).round(), 1080.0),
        };
        {
            let m: Table = lua.globals().get("M").unwrap();
            m.set("vw", vw).unwrap();
            m.set("vh", vh).unwrap();
            match scale {
                Some(s) => m.set("scale", s).unwrap(),
                None => m.set("scale", mlua::Value::Nil).unwrap(),
            }
            m.set("cw", cw).unwrap();
            m.set("ch", ch).unwrap();
            m.set("hud_present", hud_present).unwrap();
            let env: Table = m.get("env").unwrap();
            env.set("HSMP_STATE_DIR", lua_path(&sd)).unwrap();
            match banner_env { Some(v) => env.set("HSMP_HUD_BANNER", v).unwrap(), None => env.set("HSMP_HUD_BANNER", false).unwrap() }
        }
        let sp = lua_path(&scripts_dir());
        // + shared/ (hsmp_wg.lua, the world guard; deploy copies it into Scripts/)
        lua.load(format!("package.path = \"{sp}/?.lua;{sp}/../../shared/?.lua;\" .. package.path")).exec().unwrap();
        // Shared memory is the only IPC; the file-backed HSMPNative mock
        // (tools/hsmp-tools/lua-tests/lib) stands in for the sidecar's side
        lua.load(format!("HSMP_TEST_LIB = \"{sp}/../../../tools/hsmp-tools/lua-tests/lib\"; HSMPNative = dofile(HSMP_TEST_LIB .. \"/hsmp_native_filemock.lua\").new()")).exec().unwrap();
        // The session domain is typed records (docs/development/ipc-shared-memory.md): the link / session slots and the
        // bus keys live in the records mixin (sc_put / bus_put); the peer directory and the
        // typed S2G events are served from HUDT here; the header heartbeat is _st.hb_age.
        lua.load(PEER_DIR_SHIM).exec().unwrap();
        let main = fs::read_to_string(scripts_dir().join("main.lua")).expect("main.lua");
        lua.load(&main).set_name(format!("@{sp}/main.lua")).exec().expect("main.lua runs");
        Hud { lua, sd, cw, ch, last_tick: 0, sidecar: None, metrics: None, ctx: std::cell::Cell::new((0, false)) }
    }

    fn ev<T: FromLuaMulti>(&self, expr: &str) -> T {
        self.lua.load(format!("return {expr}")).eval::<T>()
            .unwrap_or_else(|e| panic!("eval `{expr}` failed: {e}"))
    }
    fn s(&self, expr: &str) -> String { self.ev::<String>(&format!("tostring({expr})")) }
    fn b(&self, expr: &str) -> bool { self.ev::<bool>(&format!("(({expr}) and true or false)")) }
    fn n(&self, expr: &str) -> f64 { self.ev::<f64>(&format!("tonumber({expr}) or -1")) }
    fn exec(&self, code: &str) { self.lua.load(code).exec().unwrap_or_else(|e| panic!("exec `{code}`: {e}")) }

    // --- config files (.settings.json) ---
    fn write(&self, name: &str, text: &str) {
        let p = self.sd.join(name);
        let tmp = self.sd.join(format!("{name}.tmp"));
        fs::write(&tmp, text).unwrap();
        fs::rename(&tmp, &p).unwrap();
    }

    // --- typed records (the mock's sidecar side, lua-tests/lib/hsmp_native_records.lua) ---
    /// My own `vitals` record, as HSMPCombat writes it (another Lua state, the same segment).
    fn own_vitals(&self, seq: u32, flags: u32, vals: &[(usize, f64)]) {
        self.exec(&format!("assert(HSMPNative.put('vitals', {{ seq = {seq}, flags = {flags}, {}, v = {} }}))", self.life_ctx(), vrec(vals)));
    }
    /// The match / round / life a vitals record carries: the current life of the last `mt`
    /// (a countdown's records already belong to the next round's first life).
    fn life_ctx(&self) -> String {
        let (round, countdown) = self.ctx.get();
        format!("match_id = {TEST_MATCH}, round = {}, life = 1", if countdown { round + 1 } else { round })
    }
    /// Peer `id`'s `peer_vitals` record, as its sidecar writes it.
    fn peer_vitals(&self, id: u32, seq: u32, flags: u32, vals: &[(usize, f64)]) {
        self.exec(&format!("HSMPNative.sc_put('peer_vitals', {{ seq = {seq}, flags = {flags}, {}, v = {} }}, {id})", self.life_ctx(), vrec(vals)));
    }
    /// One S2G `death` record (wall_ms / match_id filled by the sidecar).
    fn death(&self, peer: u32, round: u32, killer: u32, cause: u32, wall_ms: u64) {
        self.exec(&format!("HSMPNative.sc_rec_event('death', {{ peer_id = {peer}, round = {round}, killer = {killer}, \
                            cause = {cause}, wall_ms = {wall_ms} }})"));
    }
}

/// A `vitals` v[19] Lua constructor: the listed 1-based scalars quantised (round(x * 64)),
/// every other one unknown (65535).
fn vrec(vals: &[(usize, f64)]) -> String {
    let mut s = String::from("(function() local v = {} for i = 1, 19 do v[i] = 65535 end ");
    for (i, x) in vals {
        s += &format!("v[{i}] = math.floor({x} * 64 + 0.5) ");
    }
    s + "return v end)()"
}

/// The 19 scalars of a full frame (comma-separated, VITALS order) as (index, value).
fn full(csv: &str) -> Vec<(usize, f64)> {
    csv.split(',').enumerate().map(|(i, x)| (i + 1, x.trim().parse::<f64>().unwrap())).collect()
}

/// Health / stamina only (no body parts known): the HUD shows HP / ST.
fn hp_st(hp: f64, st: f64) -> Vec<(usize, f64)> { vec![(1, hp), (14, st)] }

impl Hud {

    fn set_sidecar(&mut self, status: &str, peers: &[(u32, &str)], ping: &[(u32, u32)]) {
        self.sidecar = Some(Sidecar {
            status: status.into(), my: 1,
            peers: peers.iter().map(|(i, n)| (*i, n.to_string())).collect(),
            ping: ping.iter().cloned().collect(),
            reason: String::new(),
        });
        self.write_sidecar();
    }
    fn status(&mut self, status: &str) {
        if let Some(s) = self.sidecar.as_mut() { s.status = status.into(); }
        self.write_sidecar();
    }
    /// A rejection: the link's reason + status "rejected".
    fn reject(&mut self, reason: &str) {
        if let Some(s) = self.sidecar.as_mut() { s.reason = reason.into(); }
        self.status("rejected");
    }
    /// The link record's metrics (Lua fields, e.g. "rtt_ms = 42, metrics_wall_ms = 1"); None = no sample.
    fn metrics(&mut self, fields: Option<&str>) {
        self.metrics = fields.map(|f| f.to_string());
        self.write_sidecar();
    }
    /// The `link` record (republished on change, like the sidecar) and the peer directory.
    fn write_sidecar(&mut self) {
        let Some(s) = self.sidecar.clone() else { return };
        self.last_tick += 1;
        let ps: Vec<String> = s.peers.iter().map(|(i, n)| format!("{{ id = {i}, nick = {}, rtt_ms = {} }}",
            lstr(n), s.ping.get(i).copied().unwrap_or(0))).collect();
        let state = match s.status.as_str() { "connected" => "UP", "reconnecting" => "RECONNECTING", "connecting" => "CONNECTING", _ => "TERMINAL" };
        self.exec(&format!(
            "HUDT.peers = {{ {} }}; local E = HSMP_IPC.S.ENUMS; HSMPNative.sc_put('link', {{ status = E.sidecar_status[{}], \
             state = E.link_state.{state}, my_peer_id = {}, is_admin = true, reason = {}, {} }})",
            ps.join(", "), lstr(&s.status.to_uppercase()), s.my, lstr(&s.reason),
            self.metrics.clone().unwrap_or_else(|| "metrics_wall_ms = 0".into())));
    }

    /// The session record for a v4-style match description.
    fn mt(&self, m: Mt) {
        let phase = match m.state {
            "lobby" => "LOBBY", "countdown" if !m.waiting.is_empty() => "LOADING", "countdown" => "COUNTDOWN",
            "live" => "LIVE", "roundover" => "ROUND_OVER", "match_over" => "MATCH_OVER", "paused" => "PAUSED", _ => "LOBBY",
        };
        let mut ids: Vec<(u32, u32, u32)> = m.score.clone();
        for w in &m.waiting { if !ids.iter().any(|x| x.0 == *w) { ids.push((*w, 0, 1)); } }
        let rows: Vec<String> = ids.iter().enumerate().map(|(i, (id, w, a))| format!(
            "{{ seat = {}, peer_id = {id}, connected = true, wins = {w}, alive = {}, waiting = {}, spawn_pos = {{ 0, 0, 0 }} }}",
            i + 1, *a == 1, m.waiting.contains(id))).collect();
        let winner_seat = ids.iter().position(|x| x.0 == m.winner && m.winner != 0).map_or(255, |i| i + 1);
        let reason = match m.reason { "kill" => "KILL", "draw" => "DRAW", "forfeit" => "FORFEIT", "opponent_left" => "OPPONENT_LEFT",
                                      "load_failed" => "LOAD_FAILED", "" if m.winner != 0 => "KILL", _ => "NONE" };
        self.ctx.set((m.rnd, m.state == "countdown"));
        let left = if m.state == "live" { m.round_left.unwrap_or(0) } else { m.cd };
        let deadline = if left > 0 { 1_000_000 + left as u64 * 1000 } else { 0 };
        self.exec(&format!(
            "local E = HSMP_IPC.S.ENUMS; HSMPNative.sc_put('session', {{ epoch = 77, seq = {}, match_id = {TEST_MATCH}, round = {}, phase = E.phase.{phase}, \
             winner_seat = {winner_seat}, result_reason = E.result_reason.{reason}, server_time_ms = 1000000, phase_deadline_ms = {deadline}, \
             config = {{ arena = {}, best_of = {}, countdown_s = {} }}, rows = {{ {} }} }})",
            self.last_tick + 1, m.rnd, lstr(m.arena), m.best_of, m.cd, rows.join(", ")));
        // The mode record: every listed player on its first life (vitals are scoped to it).
        let mrows: Vec<String> = ids.iter().enumerate().map(|(i, (id, _, a))| format!(
            "{{ seat = {}, peer_id = {id}, life = 1, alive = {} }}", i + 1, *a == 1)).collect();
        self.exec(&format!("HSMPNative.sc_put('mode', {{ seq = {}, match_id = {TEST_MATCH}, round = {}, rows = {{ {} }} }})",
            self.last_tick + 1, m.rnd, mrows.join(", ")));
    }

    /// A Director connection state (Lua fields of the `conn_state` bus record); None = cleared.
    fn conn(&self, fields: Option<&str>) {
        match fields {
            Some(f) => self.exec(&format!("HSMP_IPC.bus_put('conn_state', {{ {f} }})")),
            None => self.exec("HSMP_IPC.bus_clear('conn_state')"),
        }
    }
    /// HSMPMatch's spectate target; None = cleared.
    fn spectate(&self, fields: Option<&str>) {
        match fields {
            Some(f) => self.exec(&format!("HSMP_IPC.bus_put('spectate', {{ {f} }})")),
            None => self.exec("HSMP_IPC.bus_clear('spectate')"),
        }
    }
    /// One typed S2G notice event.
    fn notice(&self, event_id: u32, code: u32, args: &[&str]) {
        let a: Vec<String> = args.iter().map(|x| lstr(x)).collect();
        self.exec(&format!("HUDT.ev[#HUDT.ev + 1] = {{ kind = 'notice', data = {{ event_id = {event_id}, code = {code}, args = {{ {} }} }} }}",
                           a.join(", ")));
    }
    /// The last `ui_request` the HUD wrote (bus record field).
    fn ui_req(&self, field: &str) -> String { self.s(&format!("(HSMPNative.sc_get('ui_request') or {{}}).{field}")) }

    /// Advance the fake clock; a live sidecar beats (header heartbeat) all along.
    fn run(&mut self, ms: u32) { self.run_ex(ms, true) }
    fn run_ex(&mut self, ms: u32, fresh: bool) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(200);
            if fresh && self.sidecar.is_some() {
                self.exec("HSMPNative._st.hb_age = 0.05");
            } else {
                self.exec(&format!("HSMPNative._st.hb_age = (HSMPNative._st.hb_age or 0) + {}", step as f64 / 1000.0));
            }
            self.exec(&format!("M.run({step})"));
            left -= step;
        }
    }

    fn world(&self, name: &str) { self.exec(&format!("M.new_world('{name}')")); }

    fn set_canvas(&mut self, vw: f64, vh: f64) {
        self.exec(&format!("M.vw, M.vh, M.cw, M.ch = {vw}, {vh}, {vw}, {vh}"));
        self.cw = vw;
        self.ch = vh;
    }

    fn logs(&self) -> String { self.s("M.logtext()") }

    fn live(&self) -> Vec<W> {
        let t: Table = self.ev("M.live()");
        let mut out = vec![];
        for e in t.sequence_values::<Table>() {
            let e = e.unwrap();
            out.push(W {
                cls: e.get("cls").unwrap(), text: e.get("text").unwrap(),
                x: e.get("x").unwrap(), y: e.get("y").unwrap(), w: e.get("w").unwrap(), h: e.get("h").unwrap(),
                fs: e.get("fs").unwrap(), anchor: e.get("anchor").unwrap(),
            });
        }
        out
    }
}

// --- generic checks ----------------------------------------------------------------------------

fn overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 < b.0 + b.2 - 0.5 && b.0 < a.0 + a.2 - 0.5 && a.1 < b.1 + b.3 - 0.5 && b.1 < a.1 + a.3 - 0.5
}

fn group(anchor: &str) -> &str {
    match anchor {
        "0.0,0.0" => "top-left",
        "0.5,0.0" => "top-centre",
        "1.0,0.0" => "top-right",
        "0.5,0.3" => "centre",
        "0.5,0.5" => "scoreboard",
        "0.0,1.0" => "bottom-left",
        "1.0,1.0" => "bottom-right",
        other => other,
    }
}

fn layout_checks(c: &mut C, h: &Hud, tag: &str) {
    let live = h.live();
    c.check(live.len() > 5, format!("{tag}: HUD has visible widgets ({})", live.len()));
    if live.is_empty() { return; }
    let (cw, ch) = (h.cw, h.ch);
    let out: Vec<&W> = live.iter()
        .filter(|w| w.x < -0.5 || w.y < -0.5 || w.x + w.w > cw + 0.5 || w.y + w.h > ch + 0.5).collect();
    c.check(out.is_empty(), format!("{tag}: {} widget(s) outside the {cw}x{ch} canvas, e.g. {:?}", out.len(),
                                    out.iter().take(2).collect::<Vec<_>>()));
    let texts: Vec<&W> = live.iter().filter(|w| w.cls == "TextBlock" && !w.text.trim().is_empty()).collect();
    let mut tov = vec![];
    for (i, a) in texts.iter().enumerate() {
        for b in &texts[i + 1..] {
            if overlap((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h)) { tov.push((a.text.clone(), b.text.clone())); }
        }
    }
    c.check(tov.is_empty(), format!("{tag}: {} overlapping text pair(s), e.g. {:?}", tov.len(), &tov[..tov.len().min(3)]));
    let (mut short, mut wide, mut tiny) = (vec![], vec![], vec![]);
    for t in &texts {
        if t.h + 1.0 < t.fs * 1.4 - 0.01 { short.push((t.text.clone(), t.h, t.fs)); }
        if t.text.len() as f64 * t.fs * 0.6 > t.w + 1.0 { wide.push((t.text.clone(), t.w, t.fs)); }
        if t.fs < 9.0 { tiny.push((t.text.clone(), t.fs)); }
    }
    c.check(short.is_empty(), format!("{tag}: text(s) taller than their box: {:?}", &short[..short.len().min(3)]));
    c.check(wide.is_empty(), format!("{tag}: text(s) wider than their slot: {:?}", &wide[..wide.len().min(3)]));
    c.check(tiny.is_empty(), format!("{tag}: text(s) under 9 pt: {:?}", &tiny[..tiny.len().min(3)]));
    // HUD regions never overlap each other
    let mut boxes: BTreeMap<String, (f64, f64, f64, f64)> = BTreeMap::new();
    for w in &live {
        let g = group(&w.anchor).to_string();
        let e = boxes.entry(g).or_insert((w.x, w.y, w.x + w.w, w.y + w.h));
        e.0 = e.0.min(w.x);
        e.1 = e.1.min(w.y);
        e.2 = e.2.max(w.x + w.w);
        e.3 = e.3.max(w.y + w.h);
    }
    let names: Vec<&String> = boxes.keys().collect();
    let mut clash = vec![];
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let (p, q) = (boxes[*a], boxes[*b]);
            if overlap((p.0, p.1, p.2 - p.0, p.3 - p.1), (q.0, q.1, q.2 - q.0, q.3 - q.1)) {
                clash.push(((*a).clone(), (*b).clone()));
            }
        }
    }
    c.check(clash.is_empty(), format!("{tag}: HUD regions overlap: {clash:?}"));
    let cyc: Vec<&String> = texts.iter().map(|t| &t.text).filter(|t| t.contains('<') || t.contains('>')).collect();
    c.check(cyc.is_empty(), format!("{tag}: no '< >' cycler text ({cyc:?})"));
}

/// The HUD takes no input. Only the panels (hud_panel.lua: connection modal,
/// MP pause, match result) build Buttons, make widgets Visible and switch the
/// input mode, and only while one is open: `panels` says the run opened one.
fn input_checks(c: &mut C, h: &Hud, tag: &str) {
    let panels = h.b("M.constructed['Button'] ~= nil");
    for cls in ["CheckBox", "EditableTextBox", "ScrollBox", "Slider", "ComboBoxString"] {
        c.check(!h.b(&format!("M.constructed['{cls}']")), format!("{tag}: no {cls} constructed"));
    }
    if panels {
        c.check(h.b("HSMPHUD.panel_ui() == nil"), format!("{tag}: every panel closed at the end"));
        c.check(h.s("M.input_calls[#M.input_calls] or 'none'") == "SetShowMouseCursor"
                && h.s("M.input_calls[#M.input_calls - 1] or 'none'") == "SetInputMode_GameOnly",
                format!("{tag}: input handed back to the game when the last panel closed ({})",
                        h.s("table.concat(M.input_calls, ',')")));
        c.check(!h.b("M.vis_set[4]"), format!("{tag}: no widget ever set SelfHitTestInvisible"));
    } else {
        c.check(!h.b("M.vis_set[0]") && !h.b("M.vis_set[4]"),
                format!("{tag}: no widget ever set Visible / SelfHitTestInvisible"));
        c.check(h.n("#M.input_calls") == 0.0, format!("{tag}: no input-mode / cursor / focus calls"));
    }
    let keys = h.s("(function() local k = {} for n in pairs(M.keys) do k[#k+1] = n end table.sort(k) return table.concat(k, ',') end)()");
    c.check(keys == "TAB" && !h.b("M.keys_async"), format!("{tag}: TAB is the only key bound, not async ({keys})"));
    let logs = h.logs();
    let errs: Vec<&str> = logs.lines().filter(|l| l.contains("ERR") || l.to_lowercase().contains("error")).collect();
    c.check(errs.is_empty(), format!("{tag}: no loop / tick errors ({:?})", &errs[..errs.len().min(3)]));
}

const V: &str = "HSMPHUD.view()";

fn vt(h: &Hud, path: &str) -> String { h.s(&format!("{V}.{path}.last")) }
fn vshown(h: &Hud, path: &str) -> bool { h.b(&format!("{V}.{path}.shown")) }

// --- 1. parsers ------------------------------------------------------------------------------------

#[test]
fn parse_and_parsers() {
    let mut c = C::default();
    let lua = Lua::new();
    for e in fs::read_dir(scripts_dir()).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map_or(false, |x| x == "lua") {
            let src = fs::read_to_string(&p).unwrap();
            let r = lua.load(&src).set_name(format!("@{}", p.display())).into_function();
            c.check(r.is_ok(), format!("parse {}: {:?}", p.display(), r.err()));
        }
    }
    let sp = lua_path(&scripts_dir());
    lua.load(format!("package.path = \"{sp}/?.lua;{sp}/../../shared/?.lua;\" .. package.path")).exec().unwrap();
    let t = |e: &str| -> bool { lua.load(format!("return (({e}) and true or false)")).eval::<bool>().unwrap() };
    lua.load(r#"S = require("hud_state"); Mo = require("hud_model")"#).exec().unwrap();
    // the `vitals` record (quantised once by the owner: round(x * 64), 65535 unknown)
    lua.load(r#"vr = S.vitals_from_record({ seq = 1, flags = 5, v = (function() local v = {} v[1] = 65535 v[14] = 2848
                    for i = 2, 13 do v[i] = (i - 1) * 64 end for i = 15, 19 do v[i] = 0 end return v end)() })"#)
        .exec().unwrap();
    c.check(t("vr.hp == nil and vr.st == 44.5 and vr.dead and not vr.down"), "vitals record: unknown hp, stamina v[14], DEAD flag");
    lua.load(format!("vr2 = S.vitals_from_record({{ seq = 1, flags = 2, v = {} }})", vrec(&[(1, 61.0)]))).exec().unwrap();
    c.check(t("vr2.hp == 61 and vr2.down and not vr2.dead and vr2.st == nil and vr2.body == nil"),
            "vitals record: FALLEN -> down, only Health known");
    lua.load(format!("vr3 = S.vitals_from_record({{ seq = 2, flags = 0, v = {} }})", vrec(&[(1, 0.0), (14, 20.0)]))).exec().unwrap();
    c.check(t("vr3.dead and vr3.hp == 0 and vr3.st == 20"), "vitals record: Health 0 is dead (the contract's dead rule)");
    c.check(t("S.vitals_from_record(nil) == nil"), "vitals record: none -> nil");
    lua.load(r#"sc = S.sidecar_from({ my_peer_id = 3, status = 1 }, { { id = 1, nick = 'A "q"', rtt_ms = 0 }, { id = 3, nick = 'Me', rtt_ms = 31 }, { id = 4, nick = '' } })"#).exec().unwrap();
    c.check(t(r#"sc.my_id == 3 and sc.nicks[1] == 'A "q"' and sc.ping[3] == 31 and sc.ping[1] == nil and sc.nicks[4] == 'P4' and #sc.ids == 3"#),
            "sidecar view: peer directory nicks, server-measured RTT (0 = unknown)");
    lua.load(r#"mt = S.match_from({ state = 'live', round = 2, best_of = 5, arena = 'Map_Arena_Pit', seq = 7, countdown_s = 0, last_winner = 0, reason = '', result_reason = 0, scoreboard = { { 1, 1, 1 }, { 2, 0, 0 } }, waiting_on = { 3 }, ready = { 1 } })"#).exec().unwrap();
    c.check(t("mt.state == 'live' and mt.round == 2 and mt.best_of == 5 and mt.alive[2] == false and mt.wins[1] == 1 and mt.waiting[1] == 3 and mt.ready[1] and mt.round_left == nil"),
            "match view: scoreboard + fields from the session view");
    c.check(t("S.match_from(nil) == nil and S.sidecar_from(nil, {}) == nil and S.metrics_from(nil) == nil and S.metrics_from({ metrics_wall_ms = 0 }) == nil and S.conn_from({ state = '' }) == nil and S.spectate_from({ target = 0 }) == nil"),
            "views tolerate missing / cleared records");
    lua.load(r#"m2 = S.metrics_from({ rtt_ms = 42, clock_offset_ms = -3, metrics_wall_ms = 1, loss_pct = 0.4, loss_pct_10s = -1, jitter_ms = 3, rx_age_ms = 17 })"#).exec().unwrap();
    c.check(t("m2.rtt == 42 and m2.loss == 0.4 and m2.jitter == 3 and m2.last_rx == 17 and m2.ts == 1 and S.metrics_from({ metrics_wall_ms = 1, loss_pct = 9, loss_pct_10s = 0.5 }).loss == 0.5"), "metrics: all fields, the 10 s window wins");
    c.check(t("Mo.arena_label('Map_Arena_EastTower') == 'East Tower' and Mo.arena_label('Map_Arena_LordsHall') == 'Lords Hall'"),
            "arena label");
    c.check(t("Mo.mmss(0) == '0:00' and Mo.mmss(95) == '1:35' and Mo.mmss(-3) == '0:00'"), "mm:ss");
    // the deaths: typed S2G `death` records (this Lua state's cursor starts at the first
    // poll), resends deduped by match + victim + round
    let dir = std::env::temp_dir().join(format!("hsmp_hud_tail_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    lua.load(format!("HSMP_TEST_LIB = \"{sp}/../../../tools/hsmp-tools/lua-tests/lib\"; HSMPNative = dofile(HSMP_TEST_LIB .. \"/hsmp_native_filemock.lua\").new(); \n        require('hsmp_ipc').init({{ mod = 'HudTest', state_dir = '{}' }})", lua_path(&dir))).exec().unwrap();
    let d = |v: u32, r: u32, ts: u32, mid: u32| {
        lua.load(format!("HSMPNative.sc_rec_event('death', {{ peer_id = {v}, round = {r}, killer = 1, cause = 1, wall_ms = {ts}, match_id = {mid} }})"))
            .exec().unwrap();
    };
    d(9, 1, 1, 0);
    lua.load("tail = S.new_tail('death')").exec().unwrap();
    c.check(t("#S.poll_tail(tail) == 0"), "tail: deaths from before the first poll are skipped");
    d(2, 3, 2, 0);
    d(2, 3, 3, 0);
    lua.load("d1 = S.poll_tail(tail)").exec().unwrap();
    c.check(t("#d1 == 1 and d1[1].victim == 2 and d1[1].killer == 1 and d1[1].round == 3 and d1[1].t == 2 and d1[1].match_id == nil"),
            "tail: new death, resend deduped, wall_ms = t, match_id 0 = none");
    // match 2 reuses (victim, round)
    lua.load("tail = S.new_tail('death'); S.poll_tail(tail)").exec().unwrap();
    lua.load(r#"live = { state = 'live', round = 1, order = {} }
                lobby = { state = 'lobby', round = 0, order = {} }"#).exec().unwrap();
    d(2, 1, 10, 0);
    d(2, 2, 11, 0);
    c.check(t("#S.poll_tail(tail, live) == 2"), "match 1 deaths (2,r1) (2,r2)");
    lua.load("S.poll_tail(tail, lobby)").exec().unwrap();
    d(2, 1, 20, 0);
    c.check(t("#S.poll_tail(tail, live) == 1"), "match 2 (2,r1) after the lobby is shown (was swallowed)");
    d(2, 1, 21, 0);
    c.check(t("#S.poll_tail(tail, live) == 0"), "a resend inside match 2 is still deduped");
    d(2, 2, 22, 0);
    c.check(t("#S.poll_tail(tail, live) == 1"), "match 2 (2,r2) shown");
    // no lobby seen: the round going down starts a new match context
    d(2, 1, 30, 0);
    c.check(t("#S.poll_tail(tail, live) == 1"), "round goes down without a lobby -> new match context");
    lua.load("S.poll_tail(tail, nil)").exec().unwrap();
    d(3, 2, 31, 0);
    c.check(t("#S.poll_tail(tail, nil) == 1"), "a missing .match.json is no evidence");
    // match_id from the sidecar wins
    d(4, 1, 40, 7);
    d(4, 1, 41, 7);
    d(4, 1, 42, 8);
    c.check(t("#S.poll_tail(tail, live) == 2"), "match_id keys the dedup when present");
    // read_json_line (real files: .settings.json): a complete line, an absent file
    let sc = dir.join(".settings.json");
    fs::write(&sc, "{\"nick\":\"connected\"}\n").unwrap();
    c.check(t(&format!("S.read_json_line('{}'):find('connected') ~= nil", lua_path(&sc))), "read_json_line: complete line");
    c.check(t("S.parse_match == nil and S.parse_sidecar == nil and S.parse_metrics == nil and S.parse_conn == nil and S.parse_spectate == nil"), "the text parsers are gone");
    let _ = fs::remove_file(&sc);
    c.check(t(&format!("S.read_json_line('{}') == nil", lua_path(&sc))), "read_json_line: absent file -> nil");
    c.check(t("S.match_from({ state = 'roundover', last_winner = 0, result_reason = 0, scoreboard = {} }).reason == 'pending' and S.match_from({ state = 'live', deadline_in_ms = 94100, scoreboard = {} }).round_left == 95"), "settling round = pending; a live deadline = time left");
    // HUD switches (.settings.json)
    c.check(t("(function() local p = S.parse_hud_prefs(nil); return p.hud and p.killfeed and p.net == 'always' end)()"),
            "hud prefs: defaults without a file");
    c.check(t(r#"(function() local p = S.parse_hud_prefs('{"hud":false,"hud_killfeed":false,"hud_net":"when_bad"}'); return p.hud == false and p.killfeed == false and p.net == 'bad' end)()"#),
            "hud prefs: hud / hud_killfeed / hud_net (when_bad alias)");
    c.check(t(r#"(function() local p = S.parse_hud_prefs('{"hud":"yes","hud_net":"sometimes"}'); return p.hud == true and p.net == 'always' end)()"#),
            "hud prefs: unknown values fall back to the defaults");
    let _ = fs::remove_dir_all(&dir);
    c.finish("parsers");
}

// --- 2. every state, hosting and the world guard ------------------------------------------------------

#[test]
fn states_hosting_world_guard() {
    let mut c = C::default();
    let tag = "states";
    let mut h = Hud::new(1920.0, 1080.0, Some(1.0), true, true);
    c.check(!h.logs().contains("raw LoopAsync must be shimmed"), "loops go through the game-thread shim");
    c.check(h.b("M.premap ~= nil and M.openlevel_hook ~= nil"), "world guard hooks OpenLevel + LoadMap");

    // menu world: hidden, nothing built
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "countdown", rnd: 0, cd: 3, ..Default::default() });
    h.run(1000);
    c.check(h.b("HSMPHUD.host() == nil") && h.s("HSMPHUD.model().why") == "not in an arena",
            format!("{tag}: hidden in the menu world ({})", h.s("HSMPHUD.model().why")));

    // arena, sidecar file never changes: no MP session
    h.world("Map_Arena_Pit");
    h.run_ex(16000, false);     // > MP_ALIVE_S (15 s) without a sidecar rewrite
    c.check(h.live().is_empty() && h.s("HSMPHUD.model().why") == "no MP session",
            format!("{tag}: no MP session -> hidden ({})", h.s("HSMPHUD.model().why")));

    // countdown, a player still loading
    h.mt(Mt { state: "countdown", rnd: 0, cd: 5, waiting: vec![2], ..Default::default() });
    h.run(600);
    c.check(h.b("HSMPHUD.model().visible"), format!("{tag}: visible once the sidecar is live in an arena"));
    c.check(h.s("HSMPHUD.host() and HSMPHUD.host().kind") == "native", format!("{tag}: hosted inside the native UI_HUD_C"));
    c.check(h.b("rawget(HSMPHUD.host().canvas, 'parent') == M.hud_root"),
            format!("{tag}: our canvas is a child of UI_HUD_C's root canvas"));
    c.check(h.b("(function() local s = rawget(HSMPHUD.host().canvas, 'slotobj'); return s.anchors.Minimum.X == 0 and s.anchors.Maximum.X == 1 and s.anchors.Maximum.Y == 1 and s.z == 500 and s.offsets.Right == 0 end)()"),
            format!("{tag}: native canvas fills the HUD (anchors 0..1, offsets 0, z 500)"));
    c.check(h.b("next(M.viewport) == nil"), format!("{tag}: no viewport widget when hosted natively"));
    c.check(h.n("rawget(HSMPHUD.host().canvas, 'vis')") == 3.0, format!("{tag}: HUD root is HitTestInvisible"));
    c.check(h.b("rawget(HSMPHUD.host().canvas, 'outer') == M.hud.WidgetTree"), format!("{tag}: widgets outered to the HUD's WidgetTree"));
    c.check(vt(&h, "title") == "ROUND 1  /  BEST OF 3" && vt(&h, "timer") == "LOADING",
            format!("{tag}: banner while loading ({} | {})", vt(&h, "title"), vt(&h, "timer")));
    c.check(vt(&h, "c_title") == "WAITING FOR PLAYERS" && vt(&h, "c_sub").contains("Mate"),
            format!("{tag}: WAITING FOR PLAYERS names the loader ({})", vt(&h, "c_sub")));
    c.check(vt(&h, "cells[1].tx") == "Willie  0" && vt(&h, "cells[2].tx") == "Mate  0" && !vshown(&h, "cells[3].tx"),
            format!("{tag}: score strip, one cell per player"));
    c.check(!vshown(&h, "me.name") && !vshown(&h, "board.bg"), format!("{tag}: no vitals yet, no scoreboard"));
    layout_checks(&mut c, &h, &format!("{tag} waiting"));

    // countdown running
    h.mt(Mt { state: "countdown", rnd: 0, cd: 3, ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "ROUND 1" && vt(&h, "c_sub").contains("fight in 3") && vt(&h, "timer") == "0:03",
            format!("{tag}: countdown ({} | {} | {})", vt(&h, "c_title"), vt(&h, "c_sub"), vt(&h, "timer")));

    // vitals + live: FIGHT! flash
    // my `vitals` record with only Health / Stamina known (HP / ST bars); peer 2's full record
    h.own_vitals(1, 0, &hp_st(87.0, 61.0));
    let vv = [vec!["55.0"; 13], vec!["33.0"], vec!["0"; 5]].concat().join(",");
    h.peer_vitals(2, 9, 0, &full(&vv));
    h.mt(Mt { state: "live", rnd: 1, ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "FIGHT!" && vshown(&h, "c_title"), format!("{tag}: FIGHT! on countdown -> live ({})", vt(&h, "c_title")));
    c.check(vt(&h, "me.hp.label") == "HP 87" && vt(&h, "me.vals") == "ST 61"
            && h.b(&format!("{V}.me.hp.fill.rect[3] == math.floor({V}.me.hp.w * 0.87)"))
            && h.b(&format!("{V}.me.st.fill.rect[3] == math.floor({V}.me.st.w * 0.61)")),
            format!("{tag}: own HP/ST bars ({} {})", vt(&h, "me.hp.label"), vt(&h, "me.vals")));
    c.check(vt(&h, "opps[1].name") == "Mate" && vt(&h, "opps[1].vals") == "CON 55 BODY 55%" && !vshown(&h, "opps[2].name"),
            format!("{tag}: opponent CON / BODY from its peer_vitals record ({})", vt(&h, "opps[1].vals")));
    c.check(h.b(&format!("{V}.opps[1].hp.fill.color == package.loaded.hud_kit.C.ok")), format!("{tag}: 55 % BODY is amber"));
    // my own full frame through the same parser: CON / BODY / BLEED, raw
    // Health secondary; an opponent sending the same frame reads identically.
    let own = "100.0,100.0,40.0,0.0,0.0,100.0,0.0,100.0,100.0,100.0,62.0,75.0,75.0,61.0,0.0,2.5,1.5,7.5,1.0";
    h.own_vitals(5, 0, &full(own));
    h.peer_vitals(2, 10, 0, &full(own));
    h.run(400);
    // BODY = (3*min(100, crush 62) + 3*40 + 2*0 + 1.5*0 + .5*100 + .75*(0+100+100+100)) / 13 = 44.69 %:
    // the head takes the lower of its health and the native skull-crush channel (v[11]).
    c.check(vt(&h, "me.hp.label") == "BODY 45%" && vt(&h, "me.vals") == "CON 75  hp 100  BLEED",
            format!("{tag}: own CON / BODY / BLEED ({} | {})", vt(&h, "me.hp.label"), vt(&h, "me.vals")));
    c.check(vt(&h, "opps[1].vals") == "CON 75 BODY 45%" && h.b(&format!("{V}.opps[1].vals.color == package.loaded.hud_kit.C.bad")),
            format!("{tag}: the same frame on the opponent row reads the same ({})", vt(&h, "opps[1].vals")));
    h.own_vitals(6, 0, &hp_st(87.0, 61.0));
    layout_checks(&mut c, &h, &format!("{tag} fight"));
    h.run(2000);
    let timer = vt(&h, "timer");
    c.check(!vshown(&h, "c_title"), format!("{tag}: FIGHT! clears after 1.5 s"));
    c.check(timer == "0:02" || timer == "0:03", format!("{tag}: live timer counts up ({timer})"));
    h.mt(Mt { round_left: Some(95), ..Default::default() });
    h.run(400);
    c.check(vt(&h, "timer") == "1:35", format!("{tag}: round_left_s shows time left ({})", vt(&h, "timer")));
    h.mt(Mt::default());
    h.peer_vitals(2, 11, 2, &hp_st(70.0, 40.0));   // FALLEN, no body parts known
    h.run(400);
    c.check(vt(&h, "opps[1].vals") == "DOWN  HP 70  ST 40", format!("{tag}: HP / ST record with FALLEN -> DOWN ({})", vt(&h, "opps[1].vals")));

    // kill feed
    h.death(2, 1, 1, 1, 5);
    h.death(2, 1, 1, 1, 6);   // the server's resend
    h.mt(Mt { score: vec![(1, 0, 1), (2, 0, 0)], ..Default::default() });
    h.run(400);
    c.check(vt(&h, "feed[1].tx") == "You slew Mate" && vshown(&h, "feed[1].tx") && !vshown(&h, "feed[2].tx"),
            format!("{tag}: kill feed line, resend deduped ({})", vt(&h, "feed[1].tx")));
    c.check(vt(&h, "opps[1].vals") == "DEAD", format!("{tag}: dead opponent shown as DEAD"));
    c.check(h.b(&format!("rawget({V}.cells[2].tx.tb, 'color') == package.loaded.hud_kit.C.dim")),
            format!("{tag}: dead player's score cell dimmed"));
    h.death(1, 1, 0, 0, 99);
    h.death(3, 1, 0, 3, 7);
    h.run(400);
    let feed: Vec<String> = (1..=5).filter(|i| vshown(&h, &format!("feed[{i}].tx")))
        .map(|i| vt(&h, &format!("feed[{i}].tx"))).collect();
    c.check(feed == ["P3 left the fight", "You died", "You slew Mate"], format!("{tag}: feed newest first ({feed:?})"));
    h.death(2, 2, 3, 1, 8);
    h.run(400);
    c.check(vt(&h, "feed[1].tx") == "P3 slew Mate", format!("{tag}: a later death record goes on top ({})", vt(&h, "feed[1].tx")));
    h.run(6400);
    let op = h.n(&format!("rawget({V}.feed[4].tx.tb, 'opacity')"));
    c.check((0.0..1.0).contains(&op), format!("{tag}: old feed lines fade ({op})"));
    h.run(2000);
    c.check(!vshown(&h, "feed[1].tx"), format!("{tag}: feed lines expire"));

    // I die: YOU DIED, then SPECTATING; late joiner
    h.own_vitals(7, 0, &hp_st(0.0, 50.0));   // Health 0: dead (the record's dead rule)
    h.mt(Mt { score: vec![(1, 0, 0), (2, 0, 1)], ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "YOU DIED", format!("{tag}: YOU DIED ({})", vt(&h, "c_title")));
    c.check(vt(&h, "me.hp.label") == "DEAD", format!("{tag}: own bar says DEAD"));
    h.run(3200);
    c.check(vt(&h, "c_title") == "SPECTATING MATE" && vt(&h, "c_sub").contains("TAB scores"),
            format!("{tag}: SPECTATING <name> after 3 s ({} | {})", vt(&h, "c_title"), vt(&h, "c_sub")));
    // HSMPMatch says who the camera really follows (and that Q / E can switch)
    h.spectate(Some("target = 3, nick = \"Peasant\", round = 1, alive = 2"));
    h.run(600);
    c.check(vt(&h, "c_title") == "SPECTATING PEASANT" && vt(&h, "c_sub").contains("Q / E switch"),
            format!("{tag}: spectate target from the spectate bus key ({} | {})", vt(&h, "c_title"), vt(&h, "c_sub")));
    h.spectate(None);
    h.own_vitals(8, 0, &hp_st(100.0, 100.0));
    h.run(400);
    c.check(vt(&h, "c_title").starts_with("SPECTATING") && vt(&h, "c_sub").contains("next round"),
            format!("{tag}: late joiner SPECTATING ({})", vt(&h, "c_sub")));
    layout_checks(&mut c, &h, &format!("{tag} spectating"));

    // round over: pending, winner, draw, me
    h.mt(Mt { state: "roundover", cd: 4, score: vec![(1, 0, 0), (2, 1, 1)], reason: "pending", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "ROUND OVER", format!("{tag}: ROUND OVER while settling"));
    h.mt(Mt { state: "roundover", cd: 4, score: vec![(1, 0, 0), (2, 1, 1)], winner: 2, reason: "kill", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "ROUND OVER" && vt(&h, "c_sub").contains("Mate wins the round") && vt(&h, "c_sub").contains("next round in 4"),
            format!("{tag}: round result ({})", vt(&h, "c_sub")));
    c.check(vt(&h, "cells[2].tx") == "Mate  1" && vt(&h, "timer") == "0:04", format!("{tag}: strip follows wins, timer = countdown"));
    h.mt(Mt { state: "roundover", cd: 4, reason: "draw", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_sub").starts_with("draw"), format!("{tag}: draw ({})", vt(&h, "c_sub")));
    h.mt(Mt { state: "roundover", rnd: 2, cd: 4, score: vec![(1, 2, 1), (2, 1, 0)], winner: 1, reason: "kill", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_sub").contains("You win the round"), format!("{tag}: you win the round"));

    // paused, match over
    h.mt(Mt { state: "paused", rnd: 2, cd: 25, ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "OPPONENT DISCONNECTED" && vt(&h, "timer") == "PAUSED 0:25",
            format!("{tag}: paused ({} | {})", vt(&h, "c_title"), vt(&h, "timer")));
    h.mt(Mt { state: "match_over", rnd: 3, cd: 6, score: vec![(1, 2, 1), (2, 1, 0)], winner: 1, reason: "kill", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "VICTORY" && vt(&h, "c_sub").contains("back to lobby in 6") && vt(&h, "timer") == "END",
            format!("{tag}: VICTORY ({})", vt(&h, "c_sub")));
    h.mt(Mt { state: "match_over", rnd: 3, cd: 6, score: vec![(1, 1, 1), (2, 2, 0)], winner: 2, reason: "forfeit", ..Default::default() });
    h.run(400);
    c.check(vt(&h, "c_title") == "MATE WINS THE MATCH" && vt(&h, "c_sub").contains("forfeited"),
            format!("{tag}: defeat ({})", vt(&h, "c_title")));
    // the match result buttons: REMATCH / BACK TO LOBBY (a row, not modal)
    c.check(h.s("HSMPHUD.panel() and HSMPHUD.panel().kind") == "result"
            && h.s("HSMPHUD.panel().buttons[1].label") == "REMATCH"
            && h.s("HSMPHUD.panel().buttons[2].label") == "BACK TO LOBBY"
            && h.b("HSMPHUD.panel_ui() ~= nil and #HSMPHUD.panel_ui().btns == 2"),
            format!("{tag}: match result panel REMATCH / BACK TO LOBBY"));
    c.check(vshown(&h, "c_title"), format!("{tag}: the result banner stays visible with the buttons"));
    h.exec("HSMPHUD.on_click('result:rematch')");
    h.run(300);
    let rq = h.ui_req("want");
    c.check(rq == "rematch", format!("{tag}: REMATCH -> ui_request want=rematch ({rq})"));
    c.check(h.s("HSMPHUD.panel().buttons[1].label").starts_with("REMATCH:") && h.b("HSMPHUD.panel().buttons[1].disabled"),
            format!("{tag}: REMATCH turns into a waiting state ({})", h.s("HSMPHUD.panel().buttons[1].label")));

    // net indicator
    h.mt(Mt { rnd: 3, ..Default::default() });
    let net = |h: &mut Hud, f: &str| { h.metrics(Some(f)); h.run(400); vt(h, "net.tx") };
    let dot = |h: &Hud, col: &str| h.b(&format!("{V}.net.dot.color == package.loaded.hud_kit.C.{col}"));
    let t = net(&mut h, "rtt_ms = 42, clock_offset_ms = 1, loss_pct_10s = -1, metrics_wall_ms = 1");
    c.check(t == "NET GOOD  42 ms  0.0%  +-0" && dot(&h, "good"), format!("{tag}: net good ({t})"));
    let t = net(&mut h, "rtt_ms = 116, loss_pct = 0.0, loss_pct_10s = -1, jitter_ms = 8, metrics_wall_ms = 2");
    c.check(t == "NET OK  116 ms  0.0%  +-8" && dot(&h, "ok"), format!("{tag}: a clean 116 ms link is NET OK, not BAD ({t})"));
    let t = net(&mut h, "rtt_ms = 150, loss_pct = 0.5, loss_pct_10s = -1, jitter_ms = 7, metrics_wall_ms = 3");
    c.check(t == "NET POOR  150 ms  0.5%  +-7" && dot(&h, "warn"), format!("{tag}: net poor on RTT >= 150 ({t})"));
    let t = net(&mut h, "rtt_ms = 60, loss_pct = 3.0, loss_pct_10s = -1, metrics_wall_ms = 4");
    c.check(t.starts_with("NET POOR"), format!("{tag}: net poor on loss >= 3 % ({t})"));
    let t = net(&mut h, "rtt_ms = 250, loss_pct = 0.0, loss_pct_10s = -1, metrics_wall_ms = 5");
    c.check(t.starts_with("NET BAD") && dot(&h, "bad"), format!("{tag}: net bad on RTT >= 250 ({t})"));
    let t = net(&mut h, "rtt_ms = 40, loss_pct = 9.0, loss_pct_10s = -1, metrics_wall_ms = 6");
    c.check(t.starts_with("NET BAD"), format!("{tag}: net bad on loss > 8 % ({t})"));
    let t = net(&mut h, "rtt_ms = 40, loss_pct = 0.0, loss_pct_10s = -1, rx_age_ms = 1600, metrics_wall_ms = 7");
    c.check(t.starts_with("NET BAD"), format!("{tag}: net bad on no RX for 1.5 s ({t})"));
    h.metrics(Some("rtt_ms = 40, metrics_wall_ms = 8"));
    h.run(5600);
    c.check(vt(&h, "net.tx") == "NET BAD  NO DATA", format!("{tag}: stale metrics -> NO DATA ({})", vt(&h, "net.tx")));
    h.metrics(None);
    h.run(400);
    c.check(vt(&h, "net.tx") == "NET --" && dot(&h, "off"), format!("{tag}: no metrics yet ({})", vt(&h, "net.tx")));

    // TAB scoreboard (hold)
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[(2, 88)]);
    h.metrics(Some("rtt_ms = 42, metrics_wall_ms = 8"));
    h.mt(Mt { rnd: 3, score: vec![(1, 1, 1), (2, 2, 1)], arena: "Map_Arena_LordsHall", ..Default::default() });
    h.exec("M.tab_down = true; M.keys.TAB()");
    h.run(400);
    c.check(vshown(&h, "board.bg") && vt(&h, "board.title") == "SCOREBOARD  -  BEST OF 3  -  ROUND 3  -  LORDS HALL",
            format!("{tag}: TAB opens the scoreboard ({})", vt(&h, "board.title")));
    let row = |h: &Hud, r: u32| -> Vec<String> { (1..=6).map(|i| vt(h, &format!("board.rows[{r}].cells[{i}]"))).collect() };
    let (r1, r2) = (row(&h, 1), row(&h, 2));
    c.check(r1 == ["1", "Mate", "2", "0 / 0", "88 ms", "ALIVE"] && r2 == ["2", "Willie  (you)", "1", "0 / 0", "42 ms", "ALIVE"],
            format!("{tag}: rows sorted by wins, with ping ({r1:?} {r2:?})"));
    c.check(!vshown(&h, "board.rows[3].bg"), format!("{tag}: unused rows hidden"));
    c.check(!vshown(&h, "c_title"), format!("{tag}: centre message hidden under the scoreboard"));
    c.check(h.s("M.last_key") == "Tab", format!("{tag}: release read through IsInputKeyDown(Tab)"));
    layout_checks(&mut c, &h, &format!("{tag} scoreboard"));
    h.exec("M.tab_down = false");
    h.run(300);
    c.check(!vshown(&h, "board.bg"), format!("{tag}: releasing TAB closes the scoreboard"));
    h.exec("M.tab_unreadable = true; M.keys.TAB()");
    h.run(300);
    c.check(vshown(&h, "board.bg"), format!("{tag}: toggle fallback opens"));
    h.exec("M.keys.TAB()");
    h.run(300);
    c.check(!vshown(&h, "board.bg"), format!("{tag}: toggle fallback: second press closes"));
    h.exec("M.keys.TAB()");
    h.run(8500);
    c.check(!vshown(&h, "board.bg"), format!("{tag}: toggle fallback auto-closes"));
    h.exec("M.tab_unreadable = false");

    // native pause menu hides the HUD
    // Esc in an MP arena: the native pause is suppressed, the MP pause opens
    h.exec("M.pause_open = true; rawset(M.pause, 'inviewport', true); HSMPHUD.on_esc()");   // the Esc edge
    h.run(800);
    c.check(h.b("rawget(M.pause, 'removed') == true"), format!("{tag}: native pause menu removed"));
    c.check(h.s("HSMPHUD.panel() and HSMPHUD.panel().kind") == "pause"
            && h.s("HSMPHUD.panel().buttons[1].label") == "RESUME" && h.s("HSMPHUD.panel().buttons[3].label") == "LEAVE MATCH",
            format!("{tag}: MP pause RESUME / SETTINGS / LEAVE MATCH / QUIT GAME"));
    c.check(h.b("HSMPHUD.model().visible") && !vshown(&h, "c_title"), format!("{tag}: HUD stays, centre hidden under the modal"));
    h.exec("HSMPHUD.on_click('pause:leave')");
    h.run(200);
    c.check(h.s("HSMPHUD.panel().buttons[3].label") == "CONFIRM: LEAVE MATCH", format!("{tag}: LEAVE asks to confirm"));
    h.run(4500);
    c.check(h.s("HSMPHUD.panel().buttons[3].label") == "LEAVE MATCH", format!("{tag}: the confirm times out"));
    h.exec("HSMPHUD.on_click('pause:resume')");
    h.exec("M.pause_open = false");
    h.run(400);
    c.check(h.b("HSMPHUD.panel() == nil and HSMPHUD.panel_ui() == nil"), format!("{tag}: RESUME closes the MP pause"));
    // a native menu we do not replace (photo mode, settings) still hides the HUD
    h.exec("M.photo_open = true");
    h.run(1000);   // one menu class per tick (9 ticks a cycle)
    c.check(!h.b("HSMPHUD.model().visible") && h.s("HSMPHUD.model().why") == "menu open" && h.live().is_empty(),
            format!("{tag}: hidden while a native menu is open"));
    h.exec("M.photo_open = false");
    h.run(1000);
    c.check(h.b("HSMPHUD.model().visible") && !h.live().is_empty(), format!("{tag}: back after the pause menu"));

    // connection lost / rejected
    h.status("reconnecting");
    h.run(1400);
    c.check(vt(&h, "c_title") == "CONNECTION LOST" && vt(&h, "c_sub").contains("reconnecting"),
            format!("{tag}: CONNECTION LOST ({})", vt(&h, "c_sub")));
    c.check(vt(&h, "net.tx") == "NO LINK - RECONNECTING" && dot(&h, "bad"), format!("{tag}: net says no link"));
    h.status("connected");
    h.run(600);
    c.check(vt(&h, "c_title") != "CONNECTION LOST" || !vshown(&h, "c_title"), format!("{tag}: link back clears CONNECTION LOST"));
    h.reject("protocol mismatch (server v5, client v4)");
    h.status("rejected");
    h.run(600);
    c.check(vt(&h, "c_title") == "CONNECTION REJECTED" && vt(&h, "c_sub").contains("protocol mismatch"),
            format!("{tag}: CONNECTION REJECTED ({})", vt(&h, "c_sub")));
    h.status("connected");
    // the Director's connection state (bus conn_state) wins over the raw sidecar status
    h.mt(Mt { rnd: 3, ..Default::default() });
    h.conn(Some("seq = 4, state = \"reconnecting\", reason = \"stalled\", remaining_s = 27, window_s = 35, in_match = true"));
    h.run(600);
    c.check(vt(&h, "c_title") == "RECONNECTING..." && vt(&h, "c_sub").contains("27 s left"),
            format!("{tag}: Reconnecting overlay with the window ({} | {})", vt(&h, "c_title"), vt(&h, "c_sub")));
    c.check(vt(&h, "net.tx").starts_with("NO LINK - RECONNECTING"), format!("{tag}: net says no link ({})", vt(&h, "net.tx")));
    h.conn(Some("seq = 5, state = \"lost\", reason = \"server_closed\", title = \"SERVER CLOSED\", text = \"Host closed the server\", latched = true, actions = { \"menu\" }"));
    h.run(600);
    c.check(h.s("HSMPHUD.panel() and HSMPHUD.panel().kind") == "conn" && h.s("HSMPHUD.panel().title") == "SERVER CLOSED"
            && h.s("HSMPHUD.panel().text") == "Host closed the server" && h.s("HSMPHUD.panel().buttons[1].label") == "BACK TO MENU",
            format!("{tag}: connection modal with the reason and BACK TO MENU"));
    h.exec("HSMPHUD.on_click('conn:menu')");
    h.run(400);
    let rq = h.ui_req("want");
    c.check(rq == "dismiss" && h.b("HSMPHUD.panel() == nil"),
            format!("{tag}: BACK TO MENU -> want=dismiss, modal closed at once ({rq})"));
    h.conn(Some("seq = 6, state = \"ok\""));
    h.mt(Mt { state: "lobby", rnd: 0, ..Default::default() });
    h.run(600);
    c.check(!h.b("HSMPHUD.model().visible") && h.live().is_empty(), format!("{tag}: hidden in the lobby state"));

    // world change: refs dropped untouched, rebuilt in the new world
    h.mt(Mt { state: "countdown", rnd: 3, cd: 3, ..Default::default() });
    h.run(600);
    c.check(!h.live().is_empty(), format!("{tag}: shown before the level change"));
    h.exec("M.openlevel_hook(); M.kill_all()");
    h.world("Map_Arena_Pit");
    h.run(400);
    c.check(h.b("HSMPHUD.host() == nil or HSMPHUD.host().canvas.parent == M.hud_root"),
            format!("{tag}: old host dropped on the level change"));
    h.run(3000);
    let dead = h.s("table.concat(M.dead_touch, ', ')");
    c.check(dead.is_empty(), format!("{tag}: no freed widget touched after the level change ({dead})"));
    c.check(h.b("HSMPHUD.host() and rawget(HSMPHUD.host().canvas, 'parent') == M.hud_root"),
            format!("{tag}: rebuilt inside the NEW world's UI_HUD_C"));

    // native HUD leaves the viewport mid-world -> fallback
    h.exec("M.hud_inviewport = false");
    h.run(1200);
    c.check(h.s("HSMPHUD.host() and HSMPHUD.host().kind") == "viewport", format!("{tag}: native HUD gone -> viewport fallback"));
    c.check(h.s("(function() local z = {} for _, v in pairs(M.viewport) do z[#z+1] = v end return table.concat(z, ',') end)()") == "990",
            format!("{tag}: fallback at viewport z 990"));
    c.check(h.n("rawget(HSMPHUD.host().uw, 'vis')") == 3.0, format!("{tag}: fallback HitTestInvisible"));
    let dead = h.s("table.concat(M.dead_touch, ', ')");
    c.check(dead.is_empty(), format!("{tag}: no freed widget touched ({dead})"));

    // resolution change -> rebuild
    let before = h.n("HSMPHUD.stats.rebuilds");
    h.set_canvas(2560.0, 1440.0);
    h.run(1200);
    c.check(h.n("HSMPHUD.stats.rebuilds") == before + 1.0 && h.n("HSMPHUD.host().cw") == 2560.0,
            format!("{tag}: resolution change rebuilds (rebuilds {} -> {}, host cw {})", before,
                    h.n("HSMPHUD.stats.rebuilds"), h.n("HSMPHUD.host() and HSMPHUD.host().cw")));
    layout_checks(&mut c, &h, &format!("{tag} after resize"));
    input_checks(&mut c, &h, tag);
    c.finish(tag);
}

/// UI scale: in the game GameViewportClient:GetViewportSize is C++-only, so the
/// HUD takes the viewport size from the layout library (a broken GVC probe must
/// not leave it at 1920x1080). An open panel (the reconnect / connection modal)
/// follows a resolution or DPI change.
#[test]
fn ui_scale_probe_and_panel_resize() {
    let mut c = C::default();
    let tag = "ui-scale";
    let mut h = Hud::new(1280.0, 720.0, Some(720.0 / 1080.0), true, true);
    h.exec("M.gvc_broken = true");
    h.world("Map_Arena_Alley");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "countdown", rnd: 1, cd: 3, ..Default::default() });
    h.run(4000);
    c.check(h.n("HSMPHUD.host() and HSMPHUD.host().cw") == 1920.0 && h.n("HSMPHUD.host().ch") == 1080.0,
            format!("{tag}: 1280x720 at DPI 0.667 is a 1920x1080 canvas (host {}x{})",
                    h.n("HSMPHUD.host() and HSMPHUD.host().cw"), h.n("HSMPHUD.host() and HSMPHUD.host().ch")));
    c.check(h.logs().contains("viewport 1280x720 dpi 0.667 [wll]"), format!("{tag}: the probe source is logged"));
    layout_checks(&mut c, &h, &format!("{tag} 1280x720"));
    // the connection modal is open; the window goes to 5120x1440 at DPI 1.333
    h.conn(Some("seq = 5, state = \"lost\", reason = \"timeout\", title = \"CONNECTION LOST\", text = \"The server stopped answering\", latched = true, actions = { \"reconnect\", \"menu\" }"));
    h.run(600);
    c.check(h.n("HSMPHUD.panel_ui() and HSMPHUD.panel_ui().cw") == 1920.0, format!("{tag}: modal built for the 1920x1080 canvas"));
    h.exec("M.vw, M.vh, M.scale, M.cw, M.ch = 5120, 1440, 1440 / 1080, 3840, 1080");
    h.cw = 3840.0;
    h.ch = 1080.0;
    h.run(1500);
    c.check(h.n("HSMPHUD.panel_ui() and HSMPHUD.panel_ui().cw") == 3840.0 && h.logs().contains("rebuilt"),
            format!("{tag}: the open modal is rebuilt for 3840x1080 ({})", h.n("HSMPHUD.panel_ui() and HSMPHUD.panel_ui().cw")));
    c.check(h.n("HSMPHUD.host() and HSMPHUD.host().cw") == 3840.0, format!("{tag}: the HUD follows too"));
    let dead = h.s("table.concat(M.dead_touch, ', ')");
    c.check(dead.is_empty(), format!("{tag}: no freed widget touched ({dead})"));
    c.finish(tag);
}

#[test]
fn centre_banner_on_by_default() {
    // HSMPHud is the only mod that draws the centre banner.
    let mut c = C::default();
    let tag = "banner default";
    let mut h = Hud::new_env(1920.0, 1080.0, Some(1.0), None, true);
    h.world("Map_Arena_Alley");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "countdown", rnd: 0, cd: 3, ..Default::default() });
    h.run(4000);
    c.check(vshown(&h, "c_title") && vt(&h, "c_title") == "ROUND 1",
            format!("{tag}: centre messages on without HSMP_HUD_BANNER ({})", vt(&h, "c_title")));
    input_checks(&mut c, &h, tag);
    c.finish(tag);
}

#[test]
fn centre_banner_off_with_env_0() {
    let mut c = C::default();
    let tag = "banner off";
    let mut h = Hud::new(1920.0, 1080.0, Some(1.0), false, true);
    h.world("Map_Arena_Alley");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "countdown", rnd: 0, cd: 3, ..Default::default() });
    h.run(4000);
    h.mt(Mt::default());
    h.run(400);
    c.check(h.b(&format!("{V} ~= nil")) && !vshown(&h, "c_title") && !vshown(&h, "c_sub"),
            format!("{tag}: no centre message with HSMP_HUD_BANNER=0"));
    c.check(vshown(&h, "title") && vt(&h, "title").starts_with("ROUND 1"), format!("{tag}: top banner still shown"));
    input_checks(&mut c, &h, tag);
    c.finish(tag);
}

// --- 3. layout at every resolution, native and fallback ---------------------------------------------

fn layout_run(c: &mut C, vw: f64, vh: f64, scale: Option<f64>, tag: &str, hud_present: bool) {
    let mut h = Hud::new(vw, vh, scale, true, hud_present);
    h.world("Map_Arena_Yard");
    let names = ["Willie", "Mate", "Peasant", "Sir Reginald", "Bob", "Xx_Slayer_xX", "Lady Agnes", "Grub"];
    let peers: Vec<(u32, &str)> = names.iter().enumerate().map(|(i, n)| (i as u32 + 1, *n)).collect();
    let ping: Vec<(u32, u32)> = (2..=8).map(|i| (i, 30 + 17 * i)).collect();
    h.set_sidecar("connected", &peers, &ping);
    h.metrics(Some("rtt_ms = 142, loss_pct = 1.2, loss_pct_10s = -1, jitter_ms = 9, metrics_wall_ms = 1"));
    h.own_vitals(1, 0, &hp_st(64.0, 23.0));
    for i in 2..=8u32 {
        h.peer_vitals(i, 1, 0, &hp_st((100 - 9 * i) as f64, (10 * i) as f64));
    }
    let score: Vec<(u32, u32, u32)> = (1..=8).map(|i| (i, (9 - i) % 3, if i % 3 != 0 { 1 } else { 0 })).collect();
    h.mt(Mt { state: "countdown", rnd: 4, cd: 3, best_of: 7, score: score.clone(), waiting: vec![5, 6, 7], ..Default::default() });
    h.run(3600);
    let kind = h.s("HSMPHUD.host() and HSMPHUD.host().kind");
    c.check(kind == if hud_present { "native" } else { "viewport" }, format!("{tag}: host kind {kind}"));
    if !hud_present {
        c.check(h.logs().contains("no UI_HUD_C in the viewport"), format!("{tag}: fallback is logged"));
        c.check(h.b("rawget(HSMPHUD.host().uw, 'inviewport')"), format!("{tag}: fallback added to the viewport"));
    }
    layout_checks(c, &h, &format!("{tag} 8 players waiting"));
    h.mt(Mt { state: "live", rnd: 5, best_of: 7, score: score.clone(), ..Default::default() });
    for i in 1..=5u32 {
        h.death(i + 1, 5, (i % 7) + 2, 1, i as u64);
    }
    h.run(400);
    let n_opps = (1..=7).filter(|i| vshown(&h, &format!("opps[{i}].name"))).count();
    c.check(n_opps == 7, format!("{tag}: 7 opponents listed ({n_opps})"));
    let n_feed = (1..=5).filter(|i| vshown(&h, &format!("feed[{i}].tx"))).count();
    c.check(n_feed == 5, format!("{tag}: 5 kill-feed lines ({n_feed})"));
    let n_cells = (1..=8).filter(|i| vshown(&h, &format!("cells[{i}].tx"))).count();
    c.check(n_cells == 8, format!("{tag}: 8 score cells ({n_cells})"));
    layout_checks(c, &h, &format!("{tag} 8 players fight"));
    h.exec("M.tab_down = true; M.keys.TAB()");
    h.run(300);
    let rows = (1..=8).filter(|r| vshown(&h, &format!("board.rows[{r}].bg"))).count();
    c.check(rows == 8, format!("{tag}: scoreboard 8 rows ({rows})"));
    layout_checks(c, &h, &format!("{tag} 8 players scoreboard"));
    h.exec("M.tab_down = false");
    h.mt(Mt { state: "roundover", rnd: 5, cd: 4, best_of: 7, score, winner: 6, reason: "kill", ..Default::default() });
    h.run(400);
    layout_checks(c, &h, &format!("{tag} round over"));
    // two players: the strip shrinks back
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "countdown", rnd: 1, cd: 2, ..Default::default() });
    h.run(400);
    layout_checks(c, &h, &format!("{tag} 2 players"));
    input_checks(c, &h, tag);
}

#[test]
fn layout_all_resolutions() {
    let mut c = C::default();
    for (vw, vh, sc, tg) in [(1280.0, 720.0, Some(1.0), "1280x720 canvas"), (1280.0, 720.0, None, "1280x720 dpi-default"),
                             (1920.0, 1080.0, Some(1.0), "1920x1080"), (2560.0, 1440.0, None, "2560x1440 dpi-default"),
                             (2560.0, 1080.0, Some(1.0), "2560x1080 ultrawide"), (3840.0, 2160.0, Some(1.0), "3840x2160 dpi-1"),
                             (3840.0, 2160.0, Some(2.0), "3840x2160 dpi-2")] {
        layout_run(&mut c, vw, vh, sc, &format!("{tg} native"), true);
    }
    layout_run(&mut c, 1280.0, 720.0, Some(1.0), "1280x720 fallback", false);
    layout_run(&mut c, 3840.0, 2160.0, Some(1.0), "3840x2160 fallback", false);
    c.finish("layout");
}

/// The SETTINGS screen's HUD switches (.settings.json hud / hud_killfeed /
/// hud_net, docs/development/subsystems/menu-ui.md): taken while no match runs (from the next
/// match), the round banner and server notices always stay.
#[test]
fn hud_switches_from_settings() {
    let mut c = C::default();
    let tag = "switches";
    let mut h = Hud::new_env(1920.0, 1080.0, Some(1.0), None, true);
    h.world("Map_Arena_Alley");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.metrics(Some("rtt_ms = 40, loss_pct = 0.0, loss_pct_10s = -1, metrics_wall_ms = 1"));
    // lobby: the switches are read now
    h.write(".settings.json", "{\"nick\":\"Willie\",\"hud\":true,\"hud_killfeed\":false,\"hud_net\":\"bad\"}\n");
    h.mt(Mt { state: "lobby", rnd: 0, ..Default::default() });
    h.run(600);
    h.mt(Mt { state: "live", rnd: 1, ..Default::default() });
    // vitals of the live round's first life (records are scoped to the current life)
    h.own_vitals(1, 0, &hp_st(87.0, 61.0));
    h.peer_vitals(2, 1, 0, &hp_st(70.0, 40.0));
    h.run(600);
    h.death(2, 1, 1, 1, 5);
    h.run(600);
    c.check(vshown(&h, "title") && vshown(&h, "me.name"), format!("{tag}: banner and vitals on"));
    c.check(!vshown(&h, "feed[1].tx"), format!("{tag}: hud_killfeed=false hides the kill line"));
    c.check(!vshown(&h, "net.tx"), format!("{tag}: hud_net=bad hides a good link"));
    h.metrics(Some("rtt_ms = 350, loss_pct = 9.0, loss_pct_10s = -1, metrics_wall_ms = 2"));
    h.run(600);
    c.check(vshown(&h, "net.tx") && vt(&h, "net.tx").starts_with("NET BAD"),
            format!("{tag}: hud_net=bad shows a bad link ({})", vt(&h, "net.tx")));
    // mid-match change: not applied until the next match
    h.write(".settings.json", "{\"nick\":\"Willie\",\"hud\":false,\"hud_killfeed\":true,\"hud_net\":\"always\"}\n");
    h.run(600);
    c.check(vshown(&h, "me.name"), format!("{tag}: a change during the match waits for the next match"));
    h.mt(Mt { state: "lobby", rnd: 0, ..Default::default() });
    h.run(600);
    h.mt(Mt { state: "countdown", rnd: 0, cd: 3, ..Default::default() });
    h.run(600);
    c.check(vshown(&h, "title") && vt(&h, "title").starts_with("ROUND"), format!("{tag}: hud=false keeps the round banner"));
    c.check(vshown(&h, "c_title"), format!("{tag}: hud=false keeps the centre phase message"));
    c.check(!vshown(&h, "me.name") && !vshown(&h, "opps[1].name") && !vshown(&h, "net.tx"),
            format!("{tag}: hud=false hides vitals, opponents and net"));
    h.notice(91, 99, &["Host closed the server"]);
    h.run(600);
    c.check(vshown(&h, "feed[1].tx") && vt(&h, "feed[1].tx").contains("Host closed"),
            format!("{tag}: server notices stay with hud=false ({})", vt(&h, "feed[1].tx")));
    c.finish(tag);
}

/// The panels' UI input mode belongs to the world it was applied in.
/// A level change with a panel still open (the Director travels before the
/// HUD closes it) must not run SetInputMode_GameOnly / hide the cursor in the
/// menu world, and a panel rebuilt in the next arena re-applies UI input.
/// QUIT GAME quits although the leave travel changed the world first.
#[test]
fn panel_input_and_quit_across_worlds() {
    let mut c = C::default();
    let tag = "input/quit";
    let mut h = Hud::new(1920.0, 1080.0, Some(1.0), true, true);
    h.world("Map_Arena_Pit");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "live", rnd: 1, ..Default::default() });
    h.run(1200);
    // Esc: the MP pause opens -> UI input on
    h.exec("M.input_calls = {}; M.pause_open = true; rawset(M.pause, 'inviewport', true); HSMPHUD.on_esc()");
    h.run(800);
    c.check(h.s("HSMPHUD.panel() and HSMPHUD.panel().kind") == "pause", format!("{tag}: MP pause open"));
    c.check(h.s("table.concat(M.input_calls, ',')").contains("SetInputMode_GameAndUI"),
            format!("{tag}: UI input applied in the arena ({})", h.s("table.concat(M.input_calls, ',')")));
    // the Director travels to the menu before the HUD closes the panel
    h.exec("M.input_calls = {}; M.pause_open = false; M.openlevel_hook(); M.kill_all()");
    h.world("Map_Menu_Startup");
    h.run(2000);
    let calls = h.s("table.concat(M.input_calls, ',')");
    c.check(!calls.contains("SetInputMode_GameOnly") && !calls.contains("SetShowMouseCursor"),
            format!("{tag}: no GameOnly / cursor call in the menu world ({calls})"));
    let dead = h.s("table.concat(M.dead_touch, ', ')");
    c.check(dead.is_empty(), format!("{tag}: no freed object touched ({dead})"));
    // back in an arena (round reset): a pause panel there re-applies UI input
    h.exec("M.openlevel_hook(); M.kill_all()");
    h.world("Map_Arena_Pit");
    h.run(1500);
    h.exec("M.input_calls = {}; M.pause_open = true; rawset(M.pause, 'inviewport', true)");
    h.run(800);
    c.check(h.s("HSMPHUD.panel() and HSMPHUD.panel().kind") == "pause"
            && h.s("table.concat(M.input_calls, ',')").contains("SetInputMode_GameAndUI"),
            format!("{tag}: UI input re-applied in the new arena ({})", h.s("table.concat(M.input_calls, ',')")));
    // QUIT GAME (confirmed); the Director's leave travel lands before the quit fires
    h.exec(r#"
        local sfo = StaticFindObject
        StaticFindObject = function(p)
            if p == "/Script/Engine.Default__KismetSystemLibrary" then
                return { IsValid = function() return true end,
                         QuitGame = function(_, w) M.quit_n = (M.quit_n or 0) + 1; M.quit_world = w end }
            end
            return sfo(p)
        end"#);
    h.exec("HSMPHUD.on_click('pause:quit')");
    h.run(100);
    h.exec("HSMPHUD.on_click('pause:quit')");
    let rq = h.ui_req("want");
    c.check(rq == "leave", format!("{tag}: QUIT GAME asks the Director to leave first ({rq})"));
    h.run(250);
    h.exec("M.pause_open = false; M.openlevel_hook(); M.kill_all()");
    h.world("Map_Menu_Startup");
    h.run(1500);
    c.check(h.n("M.quit_n") == 1.0 && h.b("M.quit_world == M.world"),
            format!("{tag}: the game quits after the world changed (quits {})", h.n("M.quit_n")));
    c.check(h.logs().contains("QUIT GAME: QuitGame"), format!("{tag}: quit logged"));
    c.finish(tag);
}

/// The CONNECTION REJECTED banner belongs to a rejection that just
/// happened, not to every later single-player arena with the leftover file.
#[test]
fn stale_rejected_file_shows_nothing() {
    let mut c = C::default();
    let tag = "rejected";
    let mut h = Hud::new(1920.0, 1080.0, Some(1.0), true, true);
    h.world("Map_Arena_Pit");
    h.set_sidecar("connected", &[(1, "Willie")], &[]);
    h.mt(Mt { state: "lobby", rnd: 0, ..Default::default() });
    h.run(600);
    h.reject("wrong password");
    h.status("rejected");
    h.run(600);
    c.check(vt(&h, "c_title") == "CONNECTION REJECTED", format!("{tag}: shown right after the rejection ({})", vt(&h, "c_title")));
    // the sidecar is gone (no heartbeat); its "rejected" link stays in the segment
    h.run_ex(16000, false);
    c.check(!h.b("HSMPHUD.model().visible") || vt(&h, "c_title") != "CONNECTION REJECTED" || !vshown(&h, "c_title"),
            format!("{tag}: a stale rejected file shows no banner ({} / {})", h.s("HSMPHUD.model().why"), vt(&h, "c_title")));

    // A fresh game start in single-player with a leftover "rejected"
    // link (its sidecar is not beating): the banner needs a live sidecar
    // (header heartbeat), so it never shows, not even for one read.
    let mut h2 = Hud::new(1920.0, 1080.0, Some(1.0), true, true);
    h2.set_sidecar("rejected", &[], &[]);
    if let Some(s) = h2.sidecar.as_mut() { s.reason = "wrong password".into(); }  h2.write_sidecar();
    h2.world("Map_Arena_Pit");
    let mut ever = false;
    for _ in 0..20 {
        h2.run_ex(200, false);
        if h2.b("HSMPHUD.model() and HSMPHUD.model().centre and HSMPHUD.model().centre.title == 'CONNECTION REJECTED'") { ever = true; }
    }
    c.check(!ever, format!("{tag}: a leftover rejected file at game start never shows the banner"));
    c.check(!h2.b("HSMPHUD.T and HSMPHUD.T.connected == true"),
            format!("{tag}: a leftover file is not a connected session"));
    c.finish(tag);
}

/// In an MP arena the HUD walks at most one native menu class per tick
/// (round-robin over the 9) and the pause classes only after an Esc press
/// (plus a 1 Hz backstop).
#[test]
fn native_menu_scans_are_spread_and_pause_scans_follow_esc() {
    let mut c = C::default();
    let tag = "scans";
    let mut h = Hud::new(1920.0, 1080.0, Some(1.0), true, true);
    h.world("Map_Arena_Pit");
    h.set_sidecar("connected", &[(1, "Willie"), (2, "Mate")], &[]);
    h.mt(Mt { state: "live", rnd: 1, ..Default::default() });
    h.run(1200);
    h.exec(r#"
        SCANS = { menu = 0, pause = 0, max_tick = 0, cur = 0 }
        local menu = { UI_PhotoMode_C = 1, UI_Gallery_C = 1, UI_KeyBinds_C = 1, UI_GameSettings_C = 1,
                       UI_DisplaySettings2_C = 1, UI_AudioSettings_C = 1, UI_Controls_C = 1 }
        local f = FindAllOf
        FindAllOf = function(cls)
            if menu[cls] then SCANS.menu = SCANS.menu + 1; SCANS.cur = SCANS.cur + 1 end
            if cls == "UI_Pause_C" or cls == "UI_Pause_Eng_C" then SCANS.pause = SCANS.pause + 1 end
            return f(cls)
        end
        local t = HSMPHUD.tick
    "#);
    // per-tick maximum: sample the counter around every 100 ms tick
    let mut max_tick = 0.0f64;
    for _ in 0..20 {
        h.exec("SCANS.cur = 0");
        h.run(100);
        max_tick = max_tick.max(h.n("SCANS.cur"));
    }
    c.check(max_tick <= 1.0, format!("{tag}: at most one native menu class walked per tick ({max_tick})"));
    c.check(h.n("SCANS.menu") >= 14.0, format!("{tag}: the round-robin keeps scanning ({})", h.n("SCANS.menu")));
    c.check(h.n("SCANS.pause") <= 10.0,
            format!("{tag}: no Esc: pause classes only on the 1 Hz backstop (pause/menu walks {})", h.n("SCANS.pause")));
    h.exec("SCANS.pause = 0; HSMPHUD.on_esc()");
    h.run(300);
    c.check(h.n("SCANS.pause") >= 4.0, format!("{tag}: an Esc press scans the pause classes at once ({})", h.n("SCANS.pause")));
    c.finish(tag);
}

//! One client: a game (HSMPWorld's main.lua in its own Lua state on `sim_mock.lua`, its
//! physics world and pawn) and its sidecar (the real `world_rx` tables). The host side of
//! the shared-memory records: Lua tables in the native module's shapes, converted to and
//! from the `#[repr(C)]` records here (the only conversion in the path, as the native
//! module's marshalling is in game).

use crate::phys::{self, Body, Phys, Pusher, V3};
use crate::world_rx::{self as wrx, Prop, WorldRx};
use hsmp_ipc::layout::{Bool, Str};
use hsmp_ipc::record::to_payload;
use hsmp_ipc::schema::world::{self as rec, DynEntry, HashRow, ManifestEntry, WorldObj};
use mlua::{Function, Lua, Table, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

pub fn repo() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..") }

/// What a client's game wrote for its sidecar (the slots and the G2S ring).
#[derive(Default)]
pub struct Out {
    /// G2S records (claims, syncs) in order: (kind, payload).
    pub g2s: Vec<(u16, Vec<u8>)>,
    pub world_out: Option<(u32, Vec<u8>)>,
    pub manifest_out: Option<(u32, u32, Vec<ManifestEntry>)>,
    pub dyn_out: Option<(u32, u32, Vec<DynEntry>)>,
    pub hash: Option<((u32, u32, u32), Vec<u8>)>,
}

#[derive(Default)]
pub struct Host {
    /// The game's clock (ms since its process start).
    pub clock_ms: f64,
    pub pawn: V3,
    pub puppets: HashMap<u32, V3>,
    pub out: Out,
    pub events: Vec<(f64, String, String)>,
    pub logs: Vec<String>,
    pub keep_logs: bool,
    pub violations: Vec<String>,
    pub held_bus: Vec<(u32, u32, u8)>,
}

pub struct Client {
    pub id: u32,
    pub lua: Lua,
    pub phys: Rc<RefCell<Phys>>,
    pub host: Rc<RefCell<Host>>,
    pub rx: WorldRx,
    /// Lua handles.
    tick: Function,
    pub mock: Table,
    pub t: Table,
    /// Sidecar: last world_out seq framed, last hash sent, last forced publish.
    last_seq: Option<(u32, u32)>,
    last_hash: Option<(u32, u32, u32)>,
    last_force: f64,
    /// Clock offset: game clock = true time + offset.
    pub clock_off: f64,
}

fn num(t: &Table, k: &str) -> f64 {
    match t.get::<Value>(k) {
        Ok(Value::Integer(i)) => i as f64,
        Ok(Value::Number(n)) => n,
        Ok(Value::Boolean(true)) => 1.0,
        _ => 0.0,
    }
}
fn numi(t: &Table, i: usize) -> f64 {
    match t.get::<Value>(i) {
        Ok(Value::Integer(v)) => v as f64,
        Ok(Value::Number(n)) => n,
        _ => 0.0,
    }
}
fn arr3(t: &Table, k: &str) -> [f64; 3] {
    match t.get::<Value>(k) {
        Ok(Value::Table(a)) => [numi(&a, 1), numi(&a, 2), numi(&a, 3)],
        _ => [0.0; 3],
    }
}
fn u32of(t: &Table, k: &str) -> u32 { num(t, k) as i64 as u32 }

/// A WorldObj row as HSMPWorld's W2.qobj builds it.
pub fn obj_from(t: &Table) -> WorldObj {
    let p = arr3(t, "pos");
    let r = arr3(t, "rot");
    let v = arr3(t, "vel");
    WorldObj {
        id: u32of(t, "id"),
        pos: [p[0] as f32, p[1] as f32, p[2] as f32],
        rot: [r[0] as i16, r[1] as i16, r[2] as i16],
        vel: [v[0] as i16, v[1] as i16, v[2] as i16],
        flags: num(t, "flags") as u8,
        _r: [0; 3],
    }
}

fn rows(t: &Table) -> Vec<Table> {
    match t.get::<Value>("rows") {
        Ok(Value::Table(a)) => a.sequence_values::<Table>().filter_map(|x| x.ok()).collect(),
        _ => Vec::new(),
    }
}

fn obj_to(lua: &Lua, o: &WorldObj) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", o.id as i64)?;
    t.set("pos", lua.create_sequence_from(o.pos.iter().map(|v| *v as f64))?)?;
    t.set("rot", lua.create_sequence_from(o.rot.iter().map(|v| *v as i64))?)?;
    t.set("vel", lua.create_sequence_from(o.vel.iter().map(|v| *v as i64))?)?;
    t.set("flags", o.flags as i64)?;
    Ok(t)
}

/// A sidecar-published slot record (bytes) as the Lua table the native module hands over.
pub fn slot_table(lua: &Lua, kind: u16, payload: &[u8]) -> mlua::Result<Table> {
    use hsmp_ipc::record::view;
    let t = lua.create_table()?;
    let rows_t = lua.create_table()?;
    match kind {
        rec::K_WORLD_OWNERS => {
            let v = view::<rec::OwnersHead>(payload).expect("owners");
            t.set("level", v.head.level as i64)?;
            t.set("epoch", v.head.epoch as i64)?;
            t.set("sync", v.head.sync.get())?;
            t.set("manifest_len", v.head.manifest_len as i64)?;
            for (i, o) in v.rows.iter().enumerate() {
                let r = lua.create_table()?;
                r.set("id", o.id as i64)?;
                r.set("owner", o.owner as i64)?;
                r.set("ver", o.ver as i64)?;
                r.set("mode", o.mode as i64)?;
                rows_t.set(i + 1, r)?;
            }
        }
        rec::K_WORLD_MANIFEST => {
            let v = view::<rec::ManifestHead>(payload).expect("manifest");
            t.set("level", v.head.level as i64)?;
            t.set("epoch", v.head.epoch as i64)?;
            for (i, e) in v.rows.iter().enumerate() {
                let r = lua.create_table()?;
                r.set("id", e.id as i64)?;
                r.set("chash", e.chash as i64)?;
                r.set("pos", lua.create_sequence_from(e.pos.iter().map(|v| *v as f64))?)?;
                rows_t.set(i + 1, r)?;
            }
        }
        rec::K_WORLD_DYN => {
            let v = view::<rec::DynHead>(payload).expect("dyn");
            t.set("level", v.head.level as i64)?;
            t.set("epoch", v.head.epoch as i64)?;
            for (i, e) in v.rows.iter().enumerate() {
                let r = lua.create_table()?;
                r.set("id", e.id as i64)?;
                r.set("chash", e.chash as i64)?;
                r.set("pos", lua.create_sequence_from(e.pos.iter().map(|v| *v as f64))?)?;
                r.set("dyn_owner", e.dyn_owner as i64)?;
                r.set("class_path", e.class_path.as_str())?;
                rows_t.set(i + 1, r)?;
            }
        }
        rec::K_WORLD_REMOTE => {
            let v = view::<rec::RemoteHead>(payload).expect("remote");
            t.set("level", v.head.level as i64)?;
            t.set("epoch", v.head.epoch as i64)?;
            for (i, s) in v.rows.iter().enumerate() {
                let r = lua.create_table()?;
                r.set("sender", s.sender as i64)?;
                r.set("seq", s.seq as i64)?;
                r.set("ts", s.ts as i64)?;
                r.set("obj", obj_to(lua, &s.obj)?)?;
                rows_t.set(i + 1, r)?;
            }
        }
        rec::K_WORLD_VERDICT => {
            let v = view::<rec::VerdictHead>(payload).expect("verdict");
            let h = v.head;
            t.set("level", h.level as i64)?;
            t.set("epoch", h.epoch as i64)?;
            t.set("seq", h.seq as i64)?;
            t.set("other", h.other as i64)?;
            t.set("compared", h.compared as i64)?;
            t.set("mismatched_n", h.mismatched_n as i64)?;
            t.set("hash_match", h.hash_match.get())?;
            t.set("hash_equal", h.hash_equal.get())?;
            for (i, m) in v.rows.iter().enumerate() {
                let r = lua.create_table()?;
                r.set("id", m.id as i64)?;
                r.set("kind", m.kind as i64)?;
                r.set("dpos", m.dpos as f64)?;
                r.set("dang", m.dang as f64)?;
                rows_t.set(i + 1, r)?;
            }
        }
        k => panic!("unexpected slot kind {k:#x}"),
    }
    t.set("rows", rows_t)?;
    Ok(t)
}

/// One scene body every client creates at the same spawn pose.
#[derive(Clone, Debug)]
pub struct SceneBody {
    pub name: String,
    /// Weapon class short name, or None for a prop.
    pub weapon: Option<String>,
    pub pos: V3,
    pub yaw: f64,
    pub r: f64,
    pub h: f64,
    pub mass: f64,
}

impl Client {
    pub fn new(id: u32, rng: &mut crate::Rng, scene: &[SceneBody], clock_off: f64) -> Client {
        let lua = Lua::new();
        let phys = Rc::new(RefCell::new(Phys::new(rng)));
        let host = Rc::new(RefCell::new(Host::default()));
        install_host(&lua, &phys, &host).expect("host bridge");
        let root = repo();
        let mock_src = std::fs::read_to_string(root.join("tests/hsmpworld-sim/sim_mock.lua")).expect("sim_mock.lua");
        let mock: Table = lua.load(&mock_src).set_name("@sim_mock.lua").eval().expect("mock loads");
        lua.globals().set("M", mock.clone()).unwrap();
        lua.load("M.install(_G)").exec().unwrap();
        let shared = root.join("mods/shared").to_string_lossy().replace('\\', "/");
        lua.load(format!("package.path = \"{shared}/?.lua;\" .. package.path")).exec().unwrap();
        let scripts = root.join("mods/HSMPWorld/Scripts").to_string_lossy().replace('\\', "/");
        lua.load(format!("package.path = \"{scripts}/?.lua;\" .. package.path")).exec().unwrap();
        lua.load("HSMP_WORLD_TEST = true").exec().unwrap();
        // HSMPWORLD_MAIN: another main.lua (a before/after comparison against an older version)
        let main_path = std::env::var("HSMPWORLD_MAIN").map(PathBuf::from).unwrap_or_else(|_| root.join("mods/HSMPWorld/Scripts/main.lua"));
        let main = std::fs::read_to_string(&main_path).expect("main.lua");
        let t: Table = lua.load(&main).set_name("@HSMPWorld/Scripts/main.lua").eval().expect("main.lua loads");
        lua.globals().set("T", t.clone()).unwrap();
        lua.load(format!(r#"
            HSMP_IPC = M.ipc
            M.ipc.peer_dir = function() return {{ by_id = SIM_PEER_RTT or {{}} }} end
            local S = T.sess
            S.HSM = {{
                link = function() return {{ my_peer_id = {id}, rtt_ms = SIM_RTT or 0 }} end,
                status_name = function() return "connected" end,
                view = function() return {{ state = SIM_MATCH_STATE or "live" }} end,
            }}
            S.HS = nil
            -- Host helpers: what the game's own input would do, and read-only queries.
            SIMH = {{}}
            function SIMH.objs()
                local W = T.W(); local out = {{}}
                if not W then return out end
                for _, o in ipairs(W.objs) do
                    if o.nid and not o.dead and o.body and o.body._idx and o.body._valid then
                        local r = W.owners[o.nid]
                        out[#out + 1] = {{ o.nid, o.body._idx, r and r.owner or 0, o.kin and 1 or 0 }}
                    end
                end
                return out
            end
            function SIMH.ready() local W = T.W(); return W ~= nil and W.ready == true end
            function SIMH.weapon_at(idx)
                for _, a in ipairs(M.weapons) do if a._valid and a._root._idx == idx then return a end end
            end
            function SIMH.hold(idx, side)
                local a = SIMH.weapon_at(idx)
                if a then M.hold(a, side); return true end
                return false
            end
            function SIMH.release(side)
                local a = M.pawn["Weapon " .. side]
                if not a then return nil end
                M.release(a, side)
                return a._root._idx
            end
            function SIMH.set_puppets(peers)
                local rows = {{}}
                for _, p in ipairs(peers) do
                    local a = M.puppets[p] or M.add_puppet(p)
                    rows[#rows + 1] = {{ peer = p, name = a._name }}
                end
                M.bus.puppets = {{ rows = rows }}
            end
            function SIMH.fw(nid)
                local W = T.W(); local o = W and W.by_nid[nid]
                if not o then return "none" end
                local f = o.fw or {{}}
                local off = f.off and math.sqrt(f.off.X ^ 2 + f.off.Y ^ 2 + f.off.Z ^ 2) or -1
                local bs = {{}}
                for _, s in ipairs(o.buf or {{}}) do bs[#bs + 1] = string.format("%d:(%.0f,%.0f)f%d", s.t, s.pos.X, s.pos.Y, s.flags) end
                local ap = o.applied_pos and string.format("(%.0f,%.0f)", o.applied_pos.X, o.applied_pos.Y) or "-"
                return string.format("kin=%s settled=%s u=%.2f off=%.1f shown=%s sim=%s applied=%s buf=%s", tostring(o.kin), tostring(o.settled), f.u or -1, off, tostring(f.shown), tostring(o.sim), ap, table.concat(bs, " "))
            end
            function SIMH.stats() local W = T.W(); return W and W.stats or {{}} end
        "#)).exec().unwrap();
        let tick: Function = t.get("on_tick").unwrap();
        let mut c = Client { id, lua, phys, host, rx: WorldRx::default(), tick, mock, t, last_seq: None, last_hash: None,
                             last_force: -1e9, clock_off };
        c.build_scene(scene);
        c
    }

    /// (Re)create the scene's bodies and actors (level load / arena reload).
    pub fn build_scene(&mut self, scene: &[SceneBody]) {
        {
            let mut p = self.phys.borrow_mut();
            p.bodies.clear();
            for s in scene {
                p.add(Body::new(s.pos, s.yaw, s.r, s.h, s.mass, true));
            }
        }
        for (i, s) in scene.iter().enumerate() {
            let f: Function = if s.weapon.is_some() { self.mock.get("add_weapon").unwrap() } else { self.mock.get("add_prop").unwrap() };
            match &s.weapon {
                Some(cls) => f.call::<()>((i as i64, cls.as_str(), s.name.as_str())).unwrap(),
                None => f.call::<()>((i as i64, s.name.as_str())).unwrap(),
            }
        }
    }

    pub fn reload(&mut self, scene: &[SceneBody]) {
        let f: Function = self.mock.get("reload").unwrap();
        f.call::<()>(()).unwrap();
        self.build_scene(scene);
    }

    pub fn set_clock(&self, true_ms: f64) { self.host.borrow_mut().clock_ms = true_ms + self.clock_off; }

    /// One game frame: HSMPWorld's tick (it runs every frame in game), then physics.
    pub fn frame(&mut self, true_ms: f64, dt_s: f64, pushers: &[Pusher], hand: Option<(V3, phys::Q)>) {
        self.set_clock(true_ms);
        if let Err(e) = self.tick.call::<()>(()) {
            panic!("client {} tick error: {e}", self.id);
        }
        let mut p = self.phys.borrow_mut();
        // a held weapon follows the hand
        if let Some((pos, q)) = hand {
            if let Some(i) = self.held_idx("R") {
                let b = &mut p.bodies[i];
                b.attached = true;
                b.sim = false;
                b.pos = pos;
                b.q = q;
            }
        }
        p.step(dt_s, pushers);
    }

    pub fn held_idx(&self, side: &str) -> Option<usize> {
        let f: Function = self.mock.get("held_idx").unwrap();
        f.call::<Option<i64>>(side).unwrap().map(|i| i as usize)
    }

    /// The sidecar's 25 ms pass: (messages to send up: (reliable, framed bytes)).
    pub fn sidecar(&mut self, true_ms: f64) -> Vec<(bool, Vec<u8>)> {
        let t = true_ms as u64;
        let mut up = Vec::new();
        if let Some(level) = wrx::resync_due(&mut self.rx, t) {
            up.push((true, hsmp_ipc::wire::encode(0, 0, &rec::WorldSync { level, _r: 0 }, &[])));
        }
        {
            let h = self.host.borrow();
            if let Some((l, e, rows)) = &h.out.manifest_out {
                wrx::add_proposals(&mut self.rx, *l, *e, rows.iter().map(|x| Prop::Static(*x)));
            }
            if let Some((l, e, rows)) = &h.out.dyn_out {
                wrx::add_proposals(&mut self.rx, *l, *e, rows.iter().map(|x| Prop::Dyn(*x)));
            }
        }
        for m in wrx::due_proposals(&mut self.rx, t) { up.push((true, m)); }
        let wo = self.host.borrow().out.world_out.clone();
        if let Some((epoch, buf)) = wo {
            let seq = u32::from_le_bytes(buf[8..12].try_into().unwrap());
            if self.last_seq != Some((seq, epoch)) {
                self.last_seq = Some((seq, epoch));
                up.push((false, hsmp_ipc::wire::message(rec::K_WORLD_STATE, 0, 0, &buf)));
            }
        }
        let hs = self.host.borrow().out.hash.clone();
        if let Some((key, buf)) = hs {
            if self.rx.synced && key.0 == self.rx.level && key.1 == self.rx.epoch && self.last_hash != Some(key) {
                self.last_hash = Some(key);
                up.push((true, hsmp_ipc::wire::message(rec::K_WORLD_HASH, 0, 0, &buf)));
            }
        }
        // inbound tables -> the game's slots
        let force = true_ms - self.last_force > 1000.0;
        let publish = wrx::take_publish(&mut self.rx, t, force);
        if publish.iter().any(|(s, _, _)| *s == "world_remote") { self.last_force = true_ms; }
        let set: Function = self.mock.get("slot_set").unwrap();
        for (slot, kind, payload) in publish {
            let tb = slot_table(&self.lua, kind, &payload).unwrap();
            set.call::<()>((slot, tb)).unwrap();
        }
        up
    }

    /// The G2S ring: claims and syncs written since the last call (a sync resets the tables).
    pub fn take_g2s(&mut self) -> Vec<Vec<u8>> {
        let g = std::mem::take(&mut self.host.borrow_mut().out.g2s);
        let mut out = Vec::new();
        for (kind, payload) in g {
            if kind == rec::K_WORLD_SYNC {
                let level = u32::from_le_bytes(payload[0..4].try_into().unwrap());
                wrx::on_sync_request(&mut self.rx, level);
            }
            out.push(hsmp_ipc::wire::message(kind, 0, 0, &payload));
        }
        out
    }

    /// A framed server message arrived (`peer` = the wire header's peer).
    pub fn deliver(&mut self, true_ms: f64, msg: &[u8]) {
        let (h, payload) = hsmp_ipc::wire::split(msg).expect("wire header");
        wrx::on_record(&mut self.rx, h.kind, h.peer, payload, true_ms as u64).expect("valid server record");
    }

    pub fn call<A: mlua::IntoLuaMulti, R: mlua::FromLuaMulti>(&self, f: &str, a: A) -> R {
        let func: Function = self.lua.globals().get::<Table>("SIMH").unwrap().get(f).unwrap_or_else(|_| panic!("SIMH.{f}"));
        func.call::<R>(a).unwrap_or_else(|e| panic!("SIMH.{f}: {e}"))
    }
}

fn install_host(lua: &Lua, phys: &Rc<RefCell<Phys>>, host: &Rc<RefCell<Host>>) -> mlua::Result<()> {
    let g = lua.globals();
    let p = lua.create_table()?;
    macro_rules! pf {
        ($name:expr, $f:expr) => {{
            let ph = phys.clone();
            p.set($name, lua.create_function(move |_, a| { let mut w = ph.borrow_mut(); Ok($f(&mut *w, a)) })?)?;
        }};
    }
    pf!("alive", |w: &mut Phys, i: i64| w.bodies.get(i as usize).map_or(false, |b| b.alive));
    pf!("is_sim", |w: &mut Phys, i: i64| { let b = &w.bodies[i as usize]; b.sim && !b.attached });
    pf!("set_sim", |w: &mut Phys, (i, on): (i64, bool)| {
        let b = &mut w.bodies[i as usize];
        if b.attached { return; }
        if b.sim != on {
            b.sim = on;
            b.vel = [0.0; 3];
            b.w = [0.0; 3];
            if on { b.wake(); }
        }
    });
    pf!("pos", |w: &mut Phys, i: i64| { let b = &w.bodies[i as usize]; (b.pos[0], b.pos[1], b.pos[2]) });
    pf!("rot", |w: &mut Phys, i: i64| { let r = phys::quat_to_rot(w.bodies[i as usize].q); (r[0], r[1], r[2]) });
    pf!("vel", |w: &mut Phys, i: i64| { let b = &w.bodies[i as usize]; if b.sim { (b.vel[0], b.vel[1], b.vel[2]) } else { (0.0, 0.0, 0.0) } });
    pf!("set_tf", |w: &mut Phys, (i, x, y, z, pi, ya, ro): (i64, f64, f64, f64, f64, f64, f64)| {
        let b = &mut w.bodies[i as usize];
        if b.attached { return; }
        b.pos = [x, y, z];
        b.q = phys::rot_to_quat(pi, ya, ro);
        if b.sim { b.wake(); }
    });
    pf!("place_q", |w: &mut Phys, (i, x, y, z, qx, qy, qz, qw): (i64, f64, f64, f64, f64, f64, f64, f64)| {
        let b = &mut w.bodies[i as usize];
        b.pos = [x, y, z];
        b.q = phys::qnorm([qx, qy, qz, qw]);
    });
    pf!("set_vel", |w: &mut Phys, (i, x, y, z): (i64, f64, f64, f64)| {
        let b = &mut w.bodies[i as usize];
        if b.sim { b.vel = [x, y, z]; if phys::len(b.vel) > 0.5 { b.wake(); } }
    });
    pf!("set_angvel", |w: &mut Phys, (i, x, y, z): (i64, f64, f64, f64)| {
        let b = &mut w.bodies[i as usize];
        if b.sim { b.w = [x.to_radians(), y.to_radians(), z.to_radians()]; }
    });
    pf!("impulse", |w: &mut Phys, (i, x, y, z): (i64, f64, f64, f64)| {
        let b = &mut w.bodies[i as usize];
        if b.sim { b.vel = phys::add(b.vel, phys::mul([x, y, z], 1.0 / b.mass)); b.wake(); }
    });
    pf!("sleep", |w: &mut Phys, i: i64| {
        let b = &mut w.bodies[i as usize];
        if b.sim { b.sleeping = true; b.vel = [0.0; 3]; b.w = [0.0; 3]; }
    });
    pf!("kill", |w: &mut Phys, i: i64| { w.bodies[i as usize].alive = false; });
    {
        let ph = phys.clone();
        p.set("spawn", lua.create_function(move |_, (x, y, z, qx, qy, qz, qw): (f64, f64, f64, f64, f64, f64, f64)| {
            let mut w = ph.borrow_mut();
            let mut b = Body::new([x, y, z], 0.0, 8.0, 3.0, 2.0, false);
            b.q = phys::qnorm([qx, qy, qz, qw]);
            Ok(w.add(b) as i64)
        })?)?;
    }
    g.set("PHYS", p)?;

    let s = lua.create_table()?;
    macro_rules! hf {
        ($name:expr, $f:expr) => {{
            let h = host.clone();
            s.set($name, lua.create_function(move |lua, a| { let mut st = h.borrow_mut(); $f(lua, &mut *st, a) })?)?;
        }};
    }
    hf!("clock_s", |_: &Lua, h: &mut Host, ()| Ok(h.clock_ms / 1000.0));
    hf!("pawn_pos", |_: &Lua, h: &mut Host, ()| Ok((h.pawn[0], h.pawn[1], h.pawn[2])));
    hf!("puppet_pos", |_: &Lua, h: &mut Host, peer: i64| {
        let p = h.puppets.get(&(peer as u32)).copied().unwrap_or([0.0, 0.0, -5000.0]);
        Ok((p[0], p[1], p[2]))
    });
    hf!("log", |_: &Lua, h: &mut Host, msg: String| {
        if h.keep_logs { h.logs.push(msg.trim_end().to_string()); }
        if msg.contains("tick error") || msg.contains("update error") { h.violations.push(msg.trim_end().to_string()); }
        Ok(())
    });
    hf!("violation", |_: &Lua, h: &mut Host, msg: String| { h.violations.push(msg); Ok(()) });
    hf!("event", |_: &Lua, h: &mut Host, (name, f): (String, Table)| {
        let mut parts: Vec<String> = Vec::new();
        for (k, v) in f.pairs::<String, Value>().flatten() {
            let s = match v {
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => format!("{n:.3}"),
                Value::Boolean(b) => b.to_string(),
                Value::String(s) => s.to_string_lossy().to_string(),
                Value::Table(_) => "[..]".into(),
                _ => "?".into(),
            };
            parts.push(format!("{k}={s}"));
        }
        parts.sort();
        let t = h.clock_ms;
        h.events.push((t, name, parts.join(" ")));
        Ok(())
    });
    hf!("bus_put", |_: &Lua, h: &mut Host, (key, t): (String, Table)| {
        if key == "world_held" {
            h.held_bus = rows(&t).iter().map(|r| (u32of(r, "peer"), u32of(r, "nid"), num(r, "hand") as u8)).collect();
        }
        Ok(())
    });
    hf!("g2s", |_: &Lua, h: &mut Host, (kind, t): (String, Table)| {
        match kind.as_str() {
            "world_claim" => {
                let has_rest = t.get::<bool>("has_rest").unwrap_or(false);
                let rest = match t.get::<Value>("rest") { Ok(Value::Table(r)) if has_rest => obj_from(&r), _ => WorldObj::default() };
                let c = rec::WorldClaim {
                    level: u32of(&t, "level"), epoch: u32of(&t, "epoch"), req: u32of(&t, "req"), id: u32of(&t, "id"),
                    mode: num(&t, "mode") as u8, has_rest: Bool::from(has_rest), _r: 0, _r2: 0, rest,
                };
                h.out.g2s.push((rec::K_WORLD_CLAIM, to_payload(&c, &[])));
            }
            "world_sync" => {
                let s = rec::WorldSync { level: u32of(&t, "level"), _r: 0 };
                h.out.g2s.push((rec::K_WORLD_SYNC, to_payload(&s, &[])));
            }
            k => panic!("unexpected G2S {k}"),
        }
        Ok(())
    });
    hf!("put", |_: &Lua, h: &mut Host, (slot, t): (String, Table)| {
        let (level, epoch) = (u32of(&t, "level"), u32of(&t, "epoch"));
        match slot.as_str() {
            "world_out" => {
                let objs: Vec<WorldObj> = rows(&t).iter().map(obj_from).collect();
                let head = rec::WorldStateHead { level, epoch, seq: u32of(&t, "seq"), ts: u32of(&t, "ts"), n: 0, _r: 0, _r2: 0 };
                h.out.world_out = Some((epoch, to_payload(&head, &objs)));
            }
            "world_manifest_out" => {
                let r: Vec<ManifestEntry> = rows(&t).iter().map(|e| {
                    let p = arr3(e, "pos");
                    ManifestEntry { id: u32of(e, "id"), chash: u32of(e, "chash"), pos: [p[0] as f32, p[1] as f32, p[2] as f32], _r: 0 }
                }).collect();
                h.out.manifest_out = Some((level, epoch, r));
            }
            "world_dyn_out" => {
                let r: Vec<DynEntry> = rows(&t).iter().map(|e| {
                    let p = arr3(e, "pos");
                    let cp: String = e.get("class_path").unwrap_or_default();
                    DynEntry { id: u32of(e, "id"), chash: u32of(e, "chash"), pos: [p[0] as f32, p[1] as f32, p[2] as f32],
                               dyn_owner: u32of(e, "dyn_owner"), class_path: Str::new(&cp), ..Default::default() }
                }).collect();
                h.out.dyn_out = Some((level, epoch, r));
            }
            "world_hash" => {
                let r: Vec<HashRow> = rows(&t).iter().map(|e| {
                    let p = arr3(e, "pos");
                    HashRow { id: u32of(e, "id"), ver: u32of(e, "ver"), pos: [p[0] as f32, p[1] as f32, p[2] as f32],
                              q: num(e, "q") as i64 as u32, status: num(e, "status") as u8, _r: [0; 3], _r2: 0 }
                }).collect();
                let seq = u32of(&t, "seq");
                let head = rec::HashHead { level, epoch, seq, hash: num(&t, "hash") as i64 as u32, n: 0, _r: 0, _r2: 0 };
                h.out.hash = Some(((level, epoch, seq), to_payload(&head, &r)));
            }
            _ => {}
        }
        Ok(())
    });
    g.set("SIM", s)?;
    Ok(())
}

//! HSMP-SHM from the tools' side: JSON views of every region, JSON -> slot/blob/message
//! writes in the game role, and [`GameHost`], a game-process emulator that creates the
//! segment, spawns the REAL sidecar with `--ipc shm:<name>` and pumps the game side
//! (heartbeat, doorbell, S2G drain, state blobs, peer slots).
//!
//! Users: `hsmp-tools ipc-dump` / `ipc-put` / `ipc-ctl` / `ipc-game`, `hsmp-gate fake-game`
//! (HSMP_IPC=shm) and scripts/e2e-test.sh (shm block).
//!
//! ABI-2 records (docs/development/ipc-shared-memory.md) are handled generically by the
//! schema: every record slot (`schema::slots()` + `Segment::slot_ref` + `RawSlot`) and every
//! ring record is rendered with `hsmp_ipc::debug_json` (the native module's Lua-table shape), and [`put_record_json`] writes
//! a record slot / G2S record / `dev_cmd` from that shape. No per-record code here: a domain
//! that declares its records and slot arms gets ipc-dump / ipc-put / ipc-game --view for free.
//! Legacy kinds keep the JSON shapes of the files they replaced.

use anyhow::{anyhow, bail, Context, Result};
use hsmp_ipc::header::{ctr, SideBlock, SideState};
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::pose::{
    PeerDir, PeerPlay, PeerRoot, PoseBuf, PoseHead, PoseLead, Root, Weapon, K_POSE, K_ROOT, K_WEAPON, MAX_PEER_SLOTS, NB,
    PEER_PLAY_HAS_CONTROL, PEER_PLAY_HAS_ROOT, PEER_PLAY_V2, PLAY_MODES, PLAY_SLOTS,
};
use hsmp_ipc::schema::{self, Dir, KindInfo, RawSlot, SlotMeta};
use hsmp_ipc::segment::Segment;
use hsmp_ipc::shm::{self, Access, Doorbell, Mapping};
use hsmp_ipc::{RefuseCode, LAYOUT_HASH};
use serde_json::{json, Value as J};
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Codec-v2 bone names (HSMPSync POSE_BONES) for the named-bone input form.
pub const POSE_BONES: [&str; NB] = [
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head", "clavicle_l",
    "upperarm_l", "lowerarm_l", "hand_l", "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r", "thigh_l", "calf_l",
    "foot_l", "thigh_r", "calf_r", "foot_r",
];

pub fn kind(name: &str) -> Result<&'static KindInfo> {
    schema::kind_by_name(name).ok_or_else(|| anyhow!("unknown kind {name:?} (see mods/shared/hsmp_ipc_schema.lua)"))
}

pub fn kind_name(id: u16) -> String {
    schema::kind_info(id).map_or_else(|| format!("0x{id:04x}"), |k| k.name.to_string())
}

// ---- opening -----------------------------------------------------------------------------

/// Open a running game's mapping by PID (name from PID + process creation time).
pub fn open_pid(pid: u32, access: Access) -> Result<Mapping> {
    let ct = shm::process_create_time(pid).with_context(|| format!("process {pid} not found"))?;
    let name = shm::mapping_name(pid, ct);
    Mapping::open(&name, access).with_context(|| format!("open {name}"))
}

pub fn open_name(name: &str, access: Access) -> Result<Mapping> {
    Mapping::open(name, access).with_context(|| format!("open {name}"))
}

pub fn segment(m: &Mapping) -> Result<&Segment> {
    let s = m.segment().ok_or_else(|| anyhow!("mapping too small ({} bytes)", m.len()))?;
    hsmp_ipc::handshake::check_prefix(s, m.len()).map_err(|r| anyhow!("segment refused: {r}"))?;
    Ok(s)
}

// ---- JSON helpers ------------------------------------------------------------------------

fn f(v: &J) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn arr<const N: usize>(v: &J) -> [f64; N] {
    let mut a = [0.0; N];
    for (i, x) in v.as_array().into_iter().flatten().take(N).enumerate() {
        a[i] = f(x);
    }
    a
}
fn num(x: f32) -> J {
    if x.is_finite() {
        json!((x as f64 * 1e4).round() / 1e4)
    } else {
        J::Null
    }
}
fn nums(xs: &[f32]) -> J {
    J::Array(xs.iter().map(|x| num(*x)).collect())
}

/// Monotonic µs from the QPC (the same clock in every process on the machine).
pub fn now_us() -> u64 {
    let f = shm::qpc_freq().max(1);
    (shm::qpc() as u128 * 1_000_000 / f as u128) as u64
}

pub fn game_meta(s: &Segment, sample_seq: u32) -> SlotMeta {
    SlotMeta {
        writer_epoch: s.header.game.epoch.load(Ordering::Acquire),
        world_epoch: s.header.world_epoch.load(Ordering::Acquire),
        session_epoch: s.header.session_epoch.load(Ordering::Acquire),
        valid: 1,
        sample_seq,
        t_us: now_us(),
    }
}

fn meta_json(m: &SlotMeta) -> J {
    json!({"writer_epoch": format!("{:016x}", m.writer_epoch), "world_epoch": m.world_epoch, "session_epoch": m.session_epoch,
           "valid": m.valid, "sample_seq": m.sample_seq, "t_us": m.t_us})
}

// ---- JSON -> the hot records (built exactly as HSMPNative builds them) -------------------

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `put_root`'s record from the `me_<PID>.json` shape `{"tick","ts","pos":[3],"rot":[pitch,yaw,roll],"vel":[3]}`.
pub fn local_root_from(v: &J) -> Root {
    let (p, r, w) = (arr::<3>(&v["pos"]), arr::<3>(&v["rot"]), arr::<3>(&v["vel"]));
    let mut root=hsmp_pose::sample::root(&[f(&v["tick"]), f(&v["ts"]), p[0], p[1], p[2], r[0], r[1], r[2], w[0], w[1], w[2]], wall_ms());
    root.match_id=v["match_id"].as_u64().unwrap_or(0);
    root.round=v["round"].as_u64().and_then(|v|u32::try_from(v).ok()).unwrap_or(0);
    root.life=v["life"].as_u64().and_then(|v|u16::try_from(v).ok()).unwrap_or(0);
    root
}

/// `put_weapon`'s record from the `.weapon.json` shape (`held` defaults to 1).
pub fn local_weapon_from(v: &J) -> Weapon {
    let (p, r, w) = (arr::<3>(&v["pos"]), arr::<3>(&v["rot"]), arr::<3>(&v["vel"]));
    let held = v["held"].as_f64().unwrap_or(1.0);
    hsmp_pose::sample::weapon(&[f(&v["tick"]), f(&v["ts"]), f(&v["weapon_id"]), held, p[0], p[1], p[2], r[0], r[1], r[2], w[0], w[1], w[2]])
}

/// `put_pose`'s record (the codec v2 frame, encoded once) from the `.skeletal.json` v2 shape:
/// `{"tick","ts","dt","b":[23*13],"w":[[21]...],"c":{"f","gr","gl","s":[16],"aim":[3],"cr":[2],"ik":[12]}}`,
/// or with `"bones":{"pelvis":[7 or 13], ...}` (POSE_BONES names; missing bones are zero, a
/// missing rotation is the identity). `None` if a number is not finite.
pub fn local_pose_from(v: &J) -> Option<Box<PoseBuf>> {
    let mut b = [0f64; hsmp_pose::sample::BONE_NUMS];
    for i in 0..NB {
        b[i * 13 + 6] = 1.0;
    }
    if let Some(a) = v["b"].as_array() {
        let flat: Vec<f64> = a.iter().flat_map(|x| match x.as_array() {
            Some(a) => a.iter().map(f).collect::<Vec<_>>(),
            None => vec![f(x)],
        }).collect();
        for (i, x) in flat.iter().take(NB * 13).enumerate() {
            b[i] = *x;
        }
    }
    if let Some(bones) = v["bones"].as_object() {
        for (name, xf) in bones {
            if let Some(i) = POSE_BONES.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                for (k, x) in xf.as_array().into_iter().flatten().take(13).enumerate() {
                    b[i * 13 + k] = f(x);
                }
            }
        }
    }
    let mut ws: Vec<[f64; 21]> = Vec::new();
    for w in v["w"].as_array().into_iter().flatten().take(2) {
        let a: Vec<f64> = w.as_array().into_iter().flatten().map(f).collect();
        if a.len() >= 21 {
            let mut x = [0f64; 21];
            x.copy_from_slice(&a[..21]);
            ws.push(x);
        }
    }
    let c = &v["c"];
    let mut cv = [0f64; hsmp_pose::sample::CONTROL_NUMS];
    if c.is_object() {
        cv[0] = f(&c["f"]);
        cv[1] = f(&c["gr"]);
        cv[2] = f(&c["gl"]);
        cv[3..19].copy_from_slice(&arr::<16>(&c["s"]));
        cv[19..22].copy_from_slice(&arr::<3>(&c["aim"]));
        cv[22..24].copy_from_slice(&arr::<2>(&c["cr"]));
        cv[24..36].copy_from_slice(&arr::<12>(&c["ik"]));
    }
    let a = hsmp_pose::sample::PoseArgs { tick: f(&v["tick"]), ts: f(&v["ts"]), dt: f(&v["dt"]), b: &b, w: &ws, c: c.is_object().then_some(&cv) };
    let mut out = PoseBuf::new_boxed();
    hsmp_pose::sample::encode_pose(&a, &mut hsmp_pose::sample::Scratch::default(), &mut out).then_some(out)
}

/// Read a hot record slot: (meta, version, payload).
pub fn read_slot(slot: &dyn RawSlot) -> Option<(SlotMeta, u64, Vec<u8>)> {
    let (mut sc, mut out) = (Vec::new(), Vec::new());
    let (m, _, ver) = slot.get(&mut sc, &mut out)?;
    Some((m, ver, out))
}

// ---- records -> JSON ----------------------------------------------------------------------

fn quat_json(q: &[f32; 4]) -> J {
    json!({"q": nums(q), "yaw": num(hsmp_pose::sample::quat_yaw(*q))})
}
pub fn root_json(r: &Root) -> J {
    json!({"tick": r.tick, "ts": r.ts, "send_wall_ms": r.send_wall_ms, "pos": nums(&r.pos), "rot": quat_json(&r.rot), "vel": nums(&r.vel),
        "match_id":r.match_id,"round":r.round,"life":r.life})
}
pub fn weapon_json(w: &Weapon) -> J {
    json!({"tick": w.tick, "ts": w.ts, "weapon_id": w.weapon_id, "held": w.held, "pos": nums(&w.pos), "rot": quat_json(&w.rot), "vel": nums(&w.vel)})
}
/// A `pose` record (head + codec v2 frame), decoded for display.
pub fn pose_json(payload: &[u8], full: bool) -> J {
    let Ok(v) = hsmp_ipc::record::view::<PoseHead>(payload) else { return json!({"_err": "invalid pose record", "_len": payload.len()}) };
    let mut j = json!({"tick": v.head.tick, "bytes": v.rows.len()});
    match hsmp_pose::posecodec::v2::decode(&v.rows) {
        Some(d) => {
            let p = d.bones[0];
            j["ts"] = json!(d.ts);
            j["k"] = num(d.k);
            j["step"] = num(d.step);
            j["has_control"] = json!(d.control.is_some());
            j["weapons"] = json!(d.weapons.len());
            j["context"] = d.context.map(|c|json!({"match_id":c.match_id,"round":c.round,"life":c.life})).unwrap_or(J::Null);
            j["has_body_strikers"] = json!(d.strikers.is_some());
            j["overrides"] = json!(d.overrides.count_ones());
            j["pelvis"] = nums(&[p.p[0], p.p[1], p.p[2], p.q[0], p.q[1], p.q[2], p.q[3]]);
            if full {
                j["body_strikers"] = J::Array(d.strikers.iter().flatten().map(|s|{
                    let b=d.bones[s.bone().unwrap()];
                    let offset=hsmp_pose::posecodec::v2::qrot(b.q,s.p);
                    let p: [f32;3]=std::array::from_fn(|i|b.p[i]+offset[i]);
                    json!({"part":s.part,"component":s.component,"kind":s.kind,"local_center":nums(&s.p),"local_rotation":nums(&s.q),"half":nums(&s.half),"world_center":nums(&p)})
                }).collect());
                j["weapon_frames"] = J::Array(d.weapons.iter().map(|w| {
                    let (base, tip) = hsmp_pose::posecodec::v2::blade_world(w);
                    json!({"hands": w.hands, "id": w.id, "pos": nums(&w.p), "rot": quat_json(&w.q),
                        "vel": nums(&w.v), "angular_vel": nums(&w.w),
                        "local_base": nums(&w.base), "local_tip": nums(&w.tip),
                        "world_base": nums(&base), "world_tip": nums(&tip),
                        "boxes": w.boxes.iter().map(|b| json!({"component":b.component, "center": nums(&b.p), "rotation": nums(&b.q), "half": nums(&b.half)})).collect::<Vec<_>>()})
                }).collect());
                j["bones"] = J::Object(POSE_BONES.iter().enumerate().map(|(i, n)| {
                    let x = d.bones[i];
                    (n.to_string(), nums(&[x.p[0], x.p[1], x.p[2], x.q[0], x.q[1], x.q[2], x.q[3]]))
                }).collect());
            }
        }
        None => j["_err"] = json!("frame does not decode"),
    }
    j
}
pub fn peer_root_json(r: &PeerRoot) -> J {
    let mut j = root_json(&r.root);
    j["peer_id"] = json!(r.peer_id);
    j
}
pub fn play_mode(m: u32) -> &'static str {
    PLAY_MODES.iter().find(|(_, v)| *v == m).map_or("?", |(n, _)| n)
}
pub fn peer_play_json(p: &PeerPlay, full: bool) -> J {
    let slots: Vec<usize> = (0..PLAY_SLOTS).filter(|i| p.mask & (1 << i) != 0).collect();
    let mut j = json!({"meta": meta_json(&p.meta), "peer_id": p.peer_id, "mode": play_mode(p.mode), "cut": p.cut,
        "play_seq": p.play_seq, "pt": p.pt, "age": num(p.age), "delay": num(p.delay), "jit": num(p.jit), "lead": num(p.lead),
        "st": num(p.st), "iv": num(p.iv), "k": num(p.k), "v2": p.flags & PEER_PLAY_V2 != 0,
        "root": if p.flags & PEER_PLAY_HAS_ROOT != 0 { nums(&p.root) } else { J::Null },
        "has_control": p.flags & PEER_PLAY_HAS_CONTROL != 0, "m": p.mask, "vm": p.vmask, "slots": slots.len(),
        "weapons": p.w.iter().filter(|w| w.present != 0).count()});
    if p.mask & 1 != 0 {
        j["pelvis"] = nums(&p.b[0][..7]);
    }
    if full {
        j["B"] = J::Object(slots.iter().map(|&i| {
            let n = if i < NB { POSE_BONES[i].to_string() } else if i == NB { "weapon_r".into() } else { "weapon_l".into() };
            (n, nums(&p.b[i]))
        }).collect());
    }
    j
}
pub fn peer_dir_json(d: &PeerDir) -> J {
    let e: Vec<J> = d.entries.iter().enumerate().filter(|(_, e)| e.active == 1)
        .map(|(slot, e)| json!({"slot": slot, "peer_id": e.peer_id, "gen": e.gen, "nick": e.nick()})).collect();
    json!({"meta": meta_json(&d.meta), "count": d.count, "dir_gen": d.dir_gen, "entries": e})
}
/// A payload of `kind` as JSON: a record through `hsmp_ipc::debug_json` (generic, by the
/// schema); an unknown kind as its length.
pub fn payload_json(kind: u16, payload: &[u8]) -> J {
    if payload.is_empty() {
        return J::Null;
    }
    if schema::record_info(kind).is_some() {
        hsmp_ipc::debug_json::record_to_json(kind, payload)
    } else {
        json!({"_unknown_kind": kind, "_len": payload.len()})
    }
}

/// The name of a record kind or a legacy kind (`0x....` when unknown).
pub fn any_kind_name(id: u16) -> String {
    match schema::record_info(id) {
        Some(r) => r.name.to_string(),
        None => kind_name(id),
    }
}

/// One ring record (G2S / S2G / DevCtl) as JSON: the header fields and the payload
/// rendered by [`payload_json`].
pub fn record_json(r: &Record) -> J {
    json!({"seq": r.hdr.seq, "kind": any_kind_name(r.hdr.kind), "kind_id": r.hdr.kind, "aux": r.hdr.aux, "peer": r.hdr.peer,
           "flags": r.hdr.flags, "req_id": r.hdr.req_id,
           "producer_epoch": format!("{:016x}", r.hdr.producer_epoch), "v": payload_json(r.hdr.kind, r.payload())})
}

/// One named record slot as JSON (`{"version", "kind", "meta", "len", "v"}`), read through
/// [`hsmp_ipc::schema::RawSlot`]. `peek` = never write the segment (read-only mapping; blobs
/// show the buffer their reader holds); otherwise a blob is taken (the game is its reader).
pub fn slot_json(raw: &dyn schema::RawSlot, peek: bool, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<J> {
    let (meta, kind, ver) = if peek { raw.peek(scratch, out)? } else { raw.get(scratch, out)? };
    if out.is_empty() && meta.valid == 0 {
        return Some(json!({"version": ver, "meta": meta_json(&meta), "len": 0, "v": J::Null}));
    }
    Some(json!({"version": ver, "kind": any_kind_name(kind), "meta": meta_json(&meta), "len": out.len(), "v": payload_json(kind, out)}))
}

/// Active peer-table slots and their peer ids.
fn active_peers(s: &Segment) -> Vec<(usize, u32)> {
    let mut dir = Box::<PeerDir>::default();
    if s.peers.dir.read_into(&mut dir).is_err() {
        return Vec::new();
    }
    dir.entries.iter().enumerate().take(MAX_PEER_SLOTS).filter(|(_, e)| e.active == 1).map(|(i, e)| (i, e.peer_id)).collect()
}

/// Every named record slot of the schema (`schema::slots()`, resolved with
/// `Segment::slot_ref`) as JSON, read-only. Per-peer forms: `{"<peer id>": {...}}`. Bus keys
/// are listed under `bus` (resolved through the bus directory).
pub fn record_slots_json(s: &Segment) -> J {
    let mut m = serde_json::Map::new();
    let peers = active_peers(s);
    let (mut scratch, mut out) = (Vec::new(), Vec::new());
    for info in schema::slots() {
        match info.form {
            schema::SlotForm::Bus => {}
            schema::SlotForm::Slot | schema::SlotForm::Blob => {
                if let Some(raw) = s.slot_ref(info.name, 0) {
                    m.insert(info.name.into(), slot_json(raw, true, &mut scratch, &mut out).unwrap_or(J::Null));
                }
            }
            schema::SlotForm::PeerSlot | schema::SlotForm::PeerBlob => {
                let mut pm = serde_json::Map::new();
                for &(slot, id) in &peers {
                    if let Some(raw) = s.slot_ref(info.name, slot) {
                        let mut v = slot_json(raw, true, &mut scratch, &mut out).unwrap_or(J::Null);
                        if v.is_object() {
                            v["slot"] = json!(slot);
                        }
                        pm.insert(id.to_string(), v);
                    }
                }
                m.insert(info.name.into(), J::Object(pm));
            }
        }
    }
    J::Object(m)
}

fn side_json(b: &SideBlock, now_qpc: u64, freq: u64) -> J {
    let counters: serde_json::Map<String, J> = ctr::NAMES.iter().enumerate()
        .map(|(i, n)| (n.to_string(), json!(b.counter(i)))).filter(|(_, v)| v != &json!(0)).collect();
    json!({"pid": b.pid.load(Ordering::Acquire), "state": b.state().map_or("?", |s| s.as_str()),
           "epoch": format!("{:016x}", b.epoch.load(Ordering::Acquire)), "create_time": format!("{:x}", b.create_time.load(Ordering::Acquire)),
           "caps": format!("0x{:x}", b.caps.load(Ordering::Acquire)), "hb_count": b.hb_count.load(Ordering::Acquire),
           "hb_age_s": b.hb_age_s(now_qpc, freq), "build_id": b.build_id(), "counters": counters})
}

fn caps_names(c: u64) -> Vec<&'static str> {
    schema::CAPS.iter().filter(|(_, v)| c & v != 0).map(|(n, _)| *n).collect()
}

pub fn header_json(s: &Segment) -> J {
    let h = &s.header;
    let freq = h.qpc_freq.load(Ordering::Acquire);
    let now = shm::qpc();
    let eff = h.caps_effective.load(Ordering::Acquire);
    json!({
        "abi": format!("{}.{}", hsmp_ipc::ABI_MAJOR, hsmp_ipc::ABI_MINOR), "layout_hash": format!("{:016x}", LAYOUT_HASH),
        "refuse": h.prefix.refuse_code().as_str(), "refuse_detail": h.prefix.refuse_detail(),
        "caps_effective": format!("0x{:x}", eff), "caps_effective_names": caps_names(eff),
        "world_epoch": h.world_epoch.load(Ordering::Acquire), "game_lua_gen": h.game_lua_gen.load(Ordering::Acquire),
        "session_epoch": h.session_epoch.load(Ordering::Acquire), "resync_req": h.resync_req.load(Ordering::Acquire),
        "attach_count": h.attach_count.load(Ordering::Acquire),
        "game": side_json(&h.game, now, freq), "sidecar": side_json(&h.sidecar, now, freq),
        "g2s": {"head": s.g2s().head(), "tail": s.g2s().tail()}, "s2g": {"head": s.s2g().head(), "tail": s.s2g().tail()},
        "devctl": {"head": s.devctl().head(), "tail": s.devctl().tail()},
    })
}

/// Everything in the segment as one JSON object (`ipc-dump --json`). Read-only.
pub fn dump_json(s: &Segment, records: usize, full: bool) -> J {
    let go = &s.game_out;
    let mut j = json!({"header": header_json(s)});
    let mut slots = serde_json::Map::new();
    if let Some((m, seq, b)) = read_slot(&go.local_root) {
        let v = hsmp_ipc::record::view::<Root>(&b).map(|v| root_json(&v.head)).unwrap_or_else(|e| json!({"_err": e.to_string()}));
        slots.insert("local_root".into(), json!({"seq": seq, "meta": meta_json(&m), "v": v}));
    }
    if let Some((m, seq, b)) = read_slot(&go.local_weapon) {
        let v = hsmp_ipc::record::view::<Weapon>(&b).map(|v| weapon_json(&v.head)).unwrap_or_else(|e| json!({"_err": e.to_string()}));
        slots.insert("local_weapon".into(), json!({"seq": seq, "meta": meta_json(&m), "v": v}));
    }
    if let Some((m, seq, b)) = read_slot(&go.local_pose) {
        slots.insert("local_pose".into(), json!({"seq": seq, "meta": meta_json(&m), "v": pose_json(&b, full)}));
    }
    if let Ok((v, seq)) = go.pose_lead.read() { slots.insert("pose_lead".into(), json!({"seq": seq, "lead_ms": v.lead_ms})); }
    j["game_out"] = J::Object(slots);
    let gb = &s.game_blobs;
    j["game_blobs"] = json!({"loadout": {"published_gen": gb.loadout.published_gen()}});
    j["state"] = json!({});
    let mut dir = Box::<PeerDir>::default();
    let mut peers = Vec::new();
    if s.peers.dir.read_into(&mut dir).is_ok() {
        j["peer_dir"] = peer_dir_json(&dir);
        for (slot, e) in dir.entries.iter().enumerate().take(MAX_PEER_SLOTS) {
            if e.active != 1 {
                continue;
            }
            let ps = &s.peers.slots[slot];
            let mut pj = json!({"slot": slot, "peer_id": e.peer_id, "nick": e.nick()});
            let mut pp = Box::<PeerPlay>::default();
            if let Ok(seq) = ps.play.read_into(&mut pp) { pj["play"] = json!({"seq": seq, "v": peer_play_json(&pp, full)}); }
            if let Some((m, seq, b)) = read_slot(&ps.root) {
                if let Ok(v) = hsmp_ipc::record::view::<PeerRoot>(&b) { pj["root"] = json!({"seq": seq, "meta": meta_json(&m), "v": peer_root_json(&v.head)}); }
            }
            if let Some((m, seq, b)) = read_slot(&ps.vitals) {
                if let Ok(v) = hsmp_ipc::record::view::<hsmp_ipc::schema::combat::Vitals>(&b) {
                    pj["vitals"] = json!({"seq": seq, "meta": meta_json(&m), "v": {"seq": v.head.seq, "flags": v.head.flags, "dism": v.head.dism, "v": v.head.v.to_vec()}});
                }
            }
            pj["loadout_gen"] = json!(s.peer_loadouts.slots[slot].published_gen());
            peers.push(pj);
        }
    }
    j["peers"] = J::Array(peers);
    let mut bus = serde_json::Map::new();
    if let Ok((d, _)) = s.bus.dir.read() {
        for i in 0..(d.count as usize).min(schema::bus::BUS_KEYS) {
            let Some(name) = d.name(i) else { continue };
            let sl = &s.bus.keys[i];
            let (mut sc, mut out) = (Vec::new(), Vec::new());
            let v = match sl.get(&mut sc, &mut out) {
                Some((m, kind, seq)) => json!({"gen": seq, "kind": any_kind_name(kind), "valid": m.valid, "v": payload_json(kind, &out)}),
                None => json!({"err": "busy"}),
            };
            bus.insert(name, v);
        }
    }
    j["bus"] = J::Object(bus);
    j["records"] = record_slots_json(s);
    j["g2s_last"] = J::Array(last_records(s.g2s().tail(), records, |q, r| s.g2s().peek(q, r)));
    j["s2g_last"] = J::Array(last_records(s.s2g().tail(), records, |q, r| s.s2g().peek(q, r)));
    j["devctl_last"] = J::Array(last_records(s.devctl().tail(), records, |q, r| s.devctl().peek(q, r)));
    j
}

pub fn last_records(tail: u64, n: usize, peek: impl Fn(u64, &mut Record) -> bool) -> Vec<J> {
    let mut out = Vec::new();
    let mut r = Record::default();
    for q in tail.saturating_sub(n as u64)..tail {
        if peek(q, &mut r) {
            out.push(record_json(&r));
        }
    }
    out
}

// ---- game-role writes --------------------------------------------------------------------

/// The game's req_id: `(game epoch as u32) << 32 | counter`.
pub fn next_req_id(s: &Segment) -> u64 {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let e = s.header.game.epoch.load(Ordering::Acquire) as u32 as u64;
    // separate short-lived processes (ipc-put) must not reuse ids: mix in the clock
    let c = N.fetch_add(1, Ordering::Relaxed).wrapping_add((shm::qpc() & 0xffff_ffff) as u32);
    (e << 32) | c as u64
}

/// Write `v` as `name` in the game role: a hot record, a record slot, a G2S record or `pose_lead`.
/// Returns the req_id for messages.
pub fn put_json(s: &Segment, name: &str, v: &J) -> Result<Option<u64>> {
    let go = &s.game_out;
    // The hot records: built as HSMPNative builds them, published into their slot.
    let hot: Option<(&dyn RawSlot, u16, Vec<u8>)> = match name {
        "local_root" => Some((&go.local_root, K_ROOT, bytemuck::bytes_of(&local_root_from(v)).to_vec())),
        "local_weapon" => Some((&go.local_weapon, K_WEAPON, bytemuck::bytes_of(&local_weapon_from(v)).to_vec())),
        "local_pose" => Some((&go.local_pose, K_POSE, local_pose_from(v).ok_or_else(|| anyhow!("local_pose: a number is not finite"))?.payload().to_vec())),
        _ => None,
    };
    if let Some((slot, kind, payload)) = hot {
        if !slot.put(game_meta(s, 0), kind, &payload, &mut Vec::new()) {
            bail!("{name}: record over its slot capacity");
        }
        s.header.game.count(ctr::SLOT_WRITES, 1);
        return Ok(None);
    }
    if let Some(r) = put_record_json(s, name, v)? {
        return Ok(r);
    }
    let k = kind(name)?;
    let go = &s.game_out;
    if k.dir != Dir::GameToSidecar {
        bail!("{name} is not written by the game");
    }
    match k.name {
        "pose_lead" => go.pose_lead.write(&PoseLead { meta: game_meta(s, 0), lead_ms: v.as_f64().unwrap_or_else(|| f(&v["lead_ms"])) as f32, _r: 0 }),
        other => bail!("put of {other} is not supported"),
    }
    s.header.game.count(ctr::SLOT_WRITES, 1);
    Ok(None)
}

/// The ABI-2 half of [`put_json`]: `name` is a game-written record slot (`schema::SlotInfo`,
/// written through `Segment::slot_ref` + `RawSlot::put`), a G2S record kind (pushed into the
/// G2S ring as it is) or `dev_cmd` (DevCtl ring). The JSON is the record's Lua-table shape
/// (`hsmp_ipc::debug_json::json_to_record`; per-peer slots are not game-written). `None` =
/// not a record name (legacy kinds); `Some(req_id)` for ring records.
pub fn put_record_json(s: &Segment, name: &str, v: &J) -> Result<Option<Option<u64>>> {
    if let Some(info) = schema::slot_by_name(name) {
        if info.dir != Dir::GameToSidecar || matches!(info.form, schema::SlotForm::Bus | schema::SlotForm::PeerSlot | schema::SlotForm::PeerBlob) {
            bail!("{name} is not a game-written slot ({:?}, {:?})", info.form, info.dir);
        }
        let r = schema::record_info(info.kind).ok_or_else(|| anyhow!("slot {name}: unknown record kind"))?;
        let (kind, payload) = hsmp_ipc::debug_json::json_to_record(r.name, v).map_err(|e| anyhow!("{e}"))?;
        let raw = s.slot_ref(name, 0).ok_or_else(|| anyhow!("slot {name} has no home in this segment (Segment::slot_ref)"))?;
        let mut scratch = Vec::new();
        if !raw.put(game_meta(s, 0), kind, &payload, &mut scratch) {
            bail!("too_big: {} bytes > {}", payload.len(), raw.cap());
        }
        s.header.game.count(ctr::SLOT_WRITES, 1);
        return Ok(Some(None));
    }
    let Some(r) = schema::record_by_name(name) else { return Ok(None) };
    let (kind, payload) = hsmp_ipc::debug_json::json_to_record(r.name, v).map_err(|e| anyhow!("{e}"))?;
    if kind == schema::dev::K_DEV_CMD {
        return Ok(Some(Some(devctl_push_bytes(s, &payload)?)));
    }
    if r.flow & schema::flow::G2S == 0 {
        bail!("{name} is not written by the game (flow {:#x})", r.flow);
    }
    let id = next_req_id(s);
    let epoch = s.header.game.epoch.load(Ordering::Acquire);
    s.g2s().push(epoch, kind, 0, id, &payload).map_err(|e| anyhow!("g2s push: {e:?}"))?;
    s.header.game.count(ctr::MSGS_OUT, 1);
    Ok(Some(Some(id)))
}

/// Push one `dev_cmd` record into the DevCtl ring (dev tools; the game accepts any producer
/// epoch on this ring). Returns the ring req_id.
pub fn devctl_push(s: &Segment, c: &schema::dev::DevCmd) -> Result<u64> {
    devctl_push_bytes(s, hsmp_ipc::bytemuck::bytes_of(c))
}

fn devctl_push_bytes(s: &Segment, payload: &[u8]) -> Result<u64> {
    schema::check_payload(schema::dev::K_DEV_CMD, payload).map_err(|e| anyhow!("dev_cmd: {e}"))?;
    let id = shm::random_u64();
    s.devctl().push(id, schema::dev::K_DEV_CMD, 0, id, payload).map_err(|e| anyhow!("devctl push: {e:?}"))?;
    Ok(id)
}

// ---- session records as the game reads them (hsmp_session.lua rules) ----------------------

/// `link.status == CONNECTED`.
pub fn link_connected(l: &J) -> bool {
    l["status"].as_u64() == Some(schema::session::sidecar_status::CONNECTED as u64)
}

/// The sidecar status name of a `link` record ("connected", "kicked", ...).
pub fn link_status_name(l: &J) -> String {
    let code = l["status"].as_u64().unwrap_or(u64::MAX) as u32;
    schema::enum_by_name("sidecar_status").and_then(|e| e.name_of(code)).map_or("unknown".into(), |n| n.to_ascii_lowercase())
}

/// The legacy v4 match-state name of a session phase (lobby, countdown (loading too), live,
/// roundover, match_over, paused), as `hsmp_session.lua` STATE_OF_PHASE.
pub fn legacy_state(phase: u64) -> &'static str {
    match phase {
        1 | 2 => "countdown",
        3 => "live",
        4 => "roundover",
        5 => "match_over",
        7 => "paused",
        _ => "lobby",
    }
}

/// The arena in force of a session record: the frozen one in a match, else the lobby config's.
pub fn session_arena(s: &J) -> Option<String> {
    let in_match = s["phase"].as_u64().unwrap_or(0) != 0 && s["has_frozen"] == true;
    let a = if in_match { &s["frozen"]["arena"] } else { &s["config"]["arena"] };
    a.as_str().filter(|x| !x.is_empty()).map(String::from)
}

// ---- the game emulator ---------------------------------------------------------------------

/// A game process stand-in: owns the segment (role = game), its doorbells and, optionally,
/// the sidecar child.
pub struct GameHost {
    map: Mapping,
    g2s_bell: Option<Doorbell>,
    _s2g_bell: Option<Doorbell>,
    pub pid: u32,
    pub create_time: u64,
    pub epoch: u64,
    last_sidecar_epoch: u64,
    last_dir_seq: u64,
    peer_seen: [(u64, u64, u64, u64); MAX_PEER_SLOTS],
    play_logged: [Option<Instant>; MAX_PEER_SLOTS],

    session: Option<J>,
    link: Option<J>,
    rec: RecSlots,
}

/// One thing the game side observed during [`GameHost::pump`].
pub type GameEvent = J;

impl GameHost {
    /// Create the segment for this process (`Local\HSMP.ipc.<abi>.<pid>.<ctime>`, or `name`).
    pub fn create(name: Option<&str>, caps: u64) -> Result<GameHost> {
        let pid = shm::current_pid();
        let ct = shm::process_create_time(pid).unwrap_or(1);
        let name = name.map(String::from).unwrap_or_else(|| shm::mapping_name(pid, ct));
        let (map, _existed) = Mapping::create(&name).with_context(|| format!("create {name}"))?;
        let epoch = shm::random_u64();
        let p = hsmp_ipc::handshake::SideParams { pid, create_time: ct, epoch, caps, build_id: "hsmp-tools fake game" };
        // SAFETY: freshly created mapping of SEGMENT_SIZE owned by this process.
        unsafe { hsmp_ipc::handshake::init_game(map.segment_ptr(), map.len(), &p, shm::qpc_freq()) }.map_err(|r| anyhow!("init_game: {r}"))?;
        let (g, sc) = shm::doorbell_names(&name);
        Ok(GameHost {
            g2s_bell: Doorbell::create(&g).ok(),
            _s2g_bell: Doorbell::create(&sc).ok(),
            map,
            pid,
            create_time: ct,
            epoch,
            last_sidecar_epoch: 0,
            last_dir_seq: 0,
            peer_seen: [(0, 0, 0, 0); MAX_PEER_SLOTS],
            play_logged: [None; MAX_PEER_SLOTS],

            session: None,
            link: None,
            rec: RecSlots::default(),
        })
    }

    /// Anonymous (heap) segment: in-process tests without a sidecar.
    pub fn anonymous(caps: u64) -> Result<GameHost> {
        let map = Mapping::anonymous();
        let epoch = shm::random_u64();
        let p = hsmp_ipc::handshake::SideParams { pid: shm::current_pid(), create_time: 1, epoch, caps, build_id: "test" };
        // SAFETY: fresh anonymous mapping owned here.
        unsafe { hsmp_ipc::handshake::init_game(map.segment_ptr(), map.len(), &p, shm::qpc_freq()) }.map_err(|r| anyhow!("init_game: {r}"))?;
        Ok(GameHost { map, g2s_bell: None, _s2g_bell: None, pid: shm::current_pid(), create_time: 1, epoch, last_sidecar_epoch: 0,
            last_dir_seq: 0, peer_seen: [(0, 0, 0, 0); MAX_PEER_SLOTS], play_logged: [None; MAX_PEER_SLOTS],
            session: None, link: None, rec: RecSlots::default() })
    }

    pub fn name(&self) -> &str {
        self.map.name()
    }
    pub fn seg(&self) -> &Segment {
        self.map.segment().expect("segment")
    }
    pub fn mapping(&self) -> &Mapping {
        &self.map
    }

    /// The sidecar arguments that attach it to this segment.
    pub fn sidecar_args(&self) -> Vec<String> {
        vec!["--parent-pid".into(), self.pid.to_string(), "--ipc".into(), format!("shm:{}", self.name())]
    }

    pub fn caps_effective(&self) -> u64 {
        self.seg().header.caps_effective.load(Ordering::Acquire)
    }
    /// Is the sidecar attached and has it negotiated `cap`?
    pub fn has(&self, cap: u64) -> bool {
        let h = &self.seg().header;
        h.sidecar.state() == Some(SideState::Ready) && self.caps_effective() & cap != 0
    }
    pub fn sidecar_ready(&self) -> bool {
        self.seg().header.sidecar.state() == Some(SideState::Ready)
    }

    /// Write `v` as kind `name` (see [`put_json`]) and ring the G2S doorbell.
    pub fn put(&self, name: &str, v: &J) -> Result<Option<u64>> {
        let r = put_json(self.seg(), name, v)?;
        if let Some(b) = &self.g2s_bell {
            b.ring();
        }
        Ok(r)
    }

    /// Ring the G2S doorbell (after a direct `put_json`).
    pub fn ring(&self) {
        if let Some(b) = &self.g2s_bell {
            b.ring();
        }
    }

    /// The latest session snapshot record (`session` slot) as JSON.
    pub fn session(&self) -> Option<&J> {
        self.session.as_ref()
    }

    /// The latest `link` record (the sidecar's connection view) as JSON.
    pub fn link(&self) -> Option<&J> {
        self.link.as_ref()
    }

    /// The sidecar says it is connected (`link.status == CONNECTED`).
    pub fn connected(&self) -> bool {
        self.link.as_ref().is_some_and(link_connected)
    }

    /// Push one typed G2S record (`name` = a record kind with the g2s flow) from its JSON shape
    /// and ring the doorbell. Returns the ring req_id.
    pub fn send_record(&self, name: &str, v: &J) -> Result<u64> {
        let r = put_record_json(self.seg(), name, v)?.flatten().ok_or_else(|| anyhow!("{name}: not a G2S record"))?;
        self.ring();
        Ok(r)
    }

    /// One game frame: heartbeat, attach/refuse detection, S2G drain, state blobs, peer slots.
    pub fn pump(&mut self) -> Vec<GameEvent> {
        let mut ev = Vec::new();
        let s: &Segment = self.map.segment().expect("segment");
        let h = &s.header;
        h.game.beat(shm::qpc());
        let se = h.sidecar.epoch.load(Ordering::Acquire);
        if se != self.last_sidecar_epoch && h.sidecar.state() == Some(SideState::Ready) {
            self.last_sidecar_epoch = se;
            ev.push(json!({"ev": "attach", "sidecar_pid": h.sidecar.pid.load(Ordering::Acquire), "sidecar_epoch": format!("{se:016x}"),
                           "caps_effective": caps_names(h.caps_effective.load(Ordering::Acquire)), "attach_count": h.attach_count.load(Ordering::Acquire)}));
        }
        let rc = h.prefix.refuse_code();
        if rc != RefuseCode::None {
            ev.push(json!({"ev": "refuse", "code": rc.as_str(), "detail": h.prefix.refuse_detail()}));
        }
        // S2G: the game is the only consumer.
        let mut r = Record::default();
        for _ in 0..4096 {
            match s.s2g().pop(se, &mut r) {
                Pop::Empty => break,
                Pop::Record => {
                    h.game.count(ctr::MSGS_IN, 1);
                    let mut j = record_json(&r);
                    j["ev"] = json!("s2g");
                    ev.push(j);
                }
                Pop::StaleEpoch => h.game.count(ctr::STALE_EPOCH, 1),
                Pop::Bad => h.game.count(ctr::BAD_RECORD, 1),
                Pop::Resynced => h.game.count(ctr::RESYNC, 1),
            }
        }
        // DevCtl (tools -> game; any producer epoch).
        for _ in 0..64 {
            match s.devctl().pop(0, &mut r) {
                Pop::Empty => break,
                Pop::Record => {
                    let mut j = record_json(&r);
                    j["ev"] = json!("devctl");
                    ev.push(j);
                }
                _ => h.game.count(ctr::BAD_RECORD, 1),
            }
        }
        // Sidecar-written record slots (schema::slots(), generic): one event per new version.
        self.rec.pump(s, &mut ev);
        // The session snapshot record (GameHost::session(): the latest one, as JSON).
        if let Some(e) = ev.iter().rev().find(|e| e["ev"] == "slot" && e["slot"] == "session") {
            self.session = Some(e["v"].clone());
        }
        if let Some(e) = ev.iter().rev().find(|e| e["ev"] == "slot" && e["slot"] == "link") {
            self.link = Some(e["v"].clone());
        }
        // Peer directory and per-peer slots.
        let dseq = s.peers.dir.seq();
        let mut dir = Box::<PeerDir>::default();
        let have_dir = s.peers.dir.read_into(&mut dir).is_ok();
        if have_dir && dseq != self.last_dir_seq {
            self.last_dir_seq = dseq;
            let mut j = peer_dir_json(&dir);
            j["ev"] = json!("peer_dir");
            ev.push(j);
        }
        if have_dir {
            for slot in 0..MAX_PEER_SLOTS {
                let e = &dir.entries[slot];
                if e.active != 1 {
                    continue;
                }
                let ps = &s.peers.slots[slot];
                let (pl, ro, vi, ki) = (ps.play.seq(), ps.root.version(), ps.vitals.version(), ps.kit.seq());
                let seen = self.peer_seen[slot];
                if ro != seen.1 {
                    if let Some((_, _, b)) = read_slot(&ps.root) {
                        if let Ok(v) = hsmp_ipc::record::view::<PeerRoot>(&b) {
                            ev.push(json!({"ev": "peer_root", "slot": slot, "peer": e.peer_id, "seq": ro, "v": peer_root_json(&v.head)}));
                        }
                    }
                }
                // Play changes every ~2 ms: report the first one and then at most 5 Hz.
                if pl != seen.0 && self.play_logged[slot].map_or(true, |t| t.elapsed().as_millis() >= 200) {
                    let mut pp = Box::<PeerPlay>::default();
                    if ps.play.read_into(&mut pp).is_ok() {
                        self.play_logged[slot] = Some(Instant::now());
                        ev.push(json!({"ev": "peer_play", "slot": slot, "peer": e.peer_id, "seq": pl, "v": peer_play_json(&pp, true)}));
                    }
                }
                self.peer_seen[slot] = (if self.play_logged[slot].is_some() { pl } else { seen.0 }, ro, vi, ki);
            }
        }
        ev
    }
}

/// [`GameHost`]'s view of the sidecar-written record slots.
#[derive(Default)]
struct RecSlots {
    rec_seen: std::collections::HashMap<(&'static str, usize), u64>,
    scratch: Vec<u64>,
    scratch_out: Vec<u8>,
}

impl RecSlots {
    /// Every sidecar-written record slot (`SlotInfo::dir == SidecarToGame`) whose version
    /// changed: `{"ev":"slot","slot":<name>,"peer":<id>?,"version","kind","meta","v"}`.
    /// Blobs are taken (the game is their reader).
    fn pump(&mut self, s: &Segment, ev: &mut Vec<GameEvent>) {
        let mut peers: Option<Vec<(usize, u32)>> = None;
        for info in schema::slots().filter(|i| i.dir == Dir::SidecarToGame) {
            let targets: Vec<(usize, Option<u32>)> = match info.form {
                schema::SlotForm::Slot | schema::SlotForm::Blob => vec![(0, None)],
                schema::SlotForm::PeerSlot | schema::SlotForm::PeerBlob => {
                    peers.get_or_insert_with(|| active_peers(s)).iter().map(|&(sl, id)| (sl, Some(id))).collect()
                }
                schema::SlotForm::Bus => continue,
            };
            for (slot, peer) in targets {
                let Some(raw) = s.slot_ref(info.name, slot) else { continue };
                let ver = raw.version();
                let seen = self.rec_seen.entry((info.name, slot)).or_insert(0);
                if ver == 0 || ver == *seen {
                    continue;
                }
                if let Some(mut j) = slot_json(raw, false, &mut self.scratch, &mut self.scratch_out) {
                    *seen = ver;
                    j["ev"] = json!("slot");
                    j["slot"] = json!(info.name);
                    if let Some(id) = peer {
                        j["peer"] = json!(id);
                        j["peer_slot"] = json!(slot);
                    }
                    ev.push(j);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::handshake::{attach_sidecar, SideParams};
    use hsmp_ipc::schema::{CAP_POSE, CAP_QUEUES, CAP_STATE};

    #[test]
    fn root_tools_preserve_original_generation_without_float_conversion() {
        let original=0xfedc_ba98_7654_3210u64;
        let r=local_root_from(&json!({"tick":17,"ts":1234,"pos":[1,2,3],"rot":[0,0,0],"vel":[0,0,0],
            "match_id":original,"round":29,"life":513}));
        assert_eq!((r.match_id,r.round,r.life),(original,29,513));
        let out=root_json(&r);
        assert_eq!(out["match_id"].as_u64(),Some(original));
        assert_eq!(out["round"],29);
        assert_eq!(out["life"],513);
        let legacy=local_root_from(&json!({"tick":17,"ts":1234,"pos":[1,2,3],"rot":[0,0,0],"vel":[0,0,0]}));
        assert_eq!((legacy.match_id,legacy.round,legacy.life),(0,0,0),"tools never restamp missing context from a current session");
    }

    /// The generic ABI-2 machinery: records by name into the G2S / DevCtl rings, rendered back
    /// by the schema; record slots through RawSlot (peek never takes a blob).
    #[test]
    fn records_put_and_render_generically() {
        let mut g = GameHost::anonymous(CAP_POSE | CAP_QUEUES | schema::CAP_DEVCTL).unwrap();
        let s = g.seg();
        let w = json!({"tick": 3, "ts": 9, "weapon_id": 4, "held": 1, "pos": [1, 2, 3], "rot": [0, 0, 0, 1], "vel": [0, 0, 1]});
        let id = put_json(s, "weapon", &w).unwrap().unwrap();
        assert!(put_json(s, "weapon", &json!({"held": 1})).is_err(), "rot (0,0,0,0) fails the record check");
        let mut r = Record::default();
        assert_eq!(s.g2s().pop(g.epoch, &mut r), Pop::Record);
        assert_eq!((r.hdr.kind, r.hdr.req_id), (schema::pose::K_WEAPON, id));
        let j = record_json(&r);
        assert_eq!(j["kind"], "weapon");
        assert_eq!(j["v"]["weapon_id"], 4);
        assert_eq!(j["v"]["rot"][3], 1.0);
        assert!(j["v"].get("_r").is_none() && j["v"].get("_invalid").is_none(), "{j}");
        // a variable record: rows set the count
        let mut rows = vec![0u8; 24];
        rows[0] = 2;
        put_json(s, "pose", &json!({"tick": 1, "rows": rows})).unwrap();
        assert_eq!(s.g2s().pop(g.epoch, &mut r), Pop::Record);
        assert_eq!(record_json(&r)["v"]["n"], 24);
        assert_eq!(record_json(&r)["v"]["rows"].as_array().unwrap().len(), 24);
        // dev_cmd -> DevCtl ring -> the game's pump
        put_json(s, "dev_cmd", &json!({"id": 5, "op": 1, "key": "join", "arg": "127.0.0.1:7777"})).unwrap();
        assert!(put_json(s, "dev_cmd", &json!({"op": 9})).is_err());
        let ev = g.pump();
        let d = ev.iter().find(|e| e["ev"] == "devctl").expect("devctl event");
        assert_eq!(d["kind"], "dev_cmd");
        assert_eq!(d["v"]["key"], "join");
        assert_eq!(d["v"]["arg"], "127.0.0.1:7777");
        // S2C-only names are refused for the game
        assert!(put_json(g.seg(), "nonexistent_record", &json!({})).is_err());

        // RawSlot rendering: a seqlock slot and a blob (peek does not take)
        let sl = hsmp_ipc::seqlock::SeqSlot::<schema::Stamped<schema::pose::Root>>::new_boxed();
        let tb = hsmp_ipc::triple::TripleBuf::<schema::Stamped<schema::pose::Root>>::new_boxed();
        let (kind, payload) = hsmp_ipc::debug_json::json_to_record("root", &json!({"tick": 8, "rot": [0, 0, 0, 1], "pos": [5, 6, 7], "match_id": 3, "round": 1, "life": 1})).unwrap();
        let (mut sc, mut out) = (Vec::new(), Vec::new());
        let meta = SlotMeta { valid: 1, ..Default::default() };
        assert!(schema::RawSlot::put(&*sl, meta, kind, &payload, &mut sc));
        assert!(schema::RawSlot::put(&*tb, meta, kind, &payload, &mut sc));
        let v = slot_json(&*sl, true, &mut sc, &mut out).unwrap();
        assert_eq!((v["kind"].as_str(), v["v"]["tick"].as_u64()), (Some("root"), Some(8)));
        assert!(slot_json(&*tb, true, &mut sc, &mut out).is_none(), "peek before the first take");
        let v = slot_json(&*tb, false, &mut sc, &mut out).unwrap();
        assert_eq!(v["v"]["pos"][2], 7.0);
        let v = slot_json(&*tb, true, &mut sc, &mut out).unwrap();
        assert_eq!(v["v"]["pos"][0], 5.0);
        // the dump carries the (schema-driven) record slot map
        assert!(dump_json(g.seg(), 4, false)["records"].is_object());
    }

    #[test]
    fn put_dump_and_pump_roundtrip() {
        let mut g = GameHost::anonymous(CAP_POSE | CAP_STATE | CAP_QUEUES).unwrap();
        let s = g.seg();
        // a "sidecar" attaches in-process
        let sp = SideParams { pid: 7, create_time: 7, epoch: 0x55, caps: CAP_POSE | CAP_QUEUES, build_id: "t" };
        attach_sidecar(s, g.mapping().len(), g.pid, None, &sp).unwrap();
        let ev = g.pump();
        assert!(ev.iter().any(|e| e["ev"] == "attach"), "{ev:?}");
        assert!(g.has(CAP_POSE) && !g.has(CAP_STATE));

        g.put("local_root", &json!({"tick": 42, "ts": 5.5, "pos": [500.0, 0.0, 100.0], "rot": [0, 90, 0], "vel": [1, 2, 3], "match_id": 3, "round": 1, "life": 1})).unwrap();
        let mut b = vec![0.0; NB * 13];
        b[2] = 100.0;
        b[6] = 1.0;
        g.put("local_pose", &json!({"tick": 3, "ts": 10.25, "dt": 8.3, "b": b, "w": [[1, 7, 0,0,0,0,0,0,1, 0,0,0,0,0,0, 1,2,3, 4,5,6]],
                                    "c": {"f": 5, "gr": 1, "gl": 0, "s": [0.5], "aim": [1, 0, 0], "cr": [10, 20], "ik": [1,2,3]}})).unwrap();
        g.put("vitals", &json!({"seq": 1, "flags": 2, "v": [5600]})).unwrap();
        g.put("world_out", &json!({"level": 5, "epoch": 1, "seq": 1, "ts": 9,
            "rows": [{"id": 7, "pos": [1, 2, 3], "rot": [0, 0, 0], "vel": [0, 0, 0], "flags": 192}]})).unwrap();
        assert!(g.put("world_out", &json!({"rows": [{"id": 0}]})).is_err(), "an invalid record is refused");
        let id = g.put("death_report", &json!({"death_id": 12, "round": 1})).unwrap().unwrap();
        assert_eq!(id >> 32, g.epoch as u32 as u64);
        assert!(g.put("peer_play", &json!({})).is_err());

        let s = g.seg();
        let (m, _, b) = read_slot(&s.game_out.local_root).unwrap();
        let r = hsmp_ipc::record::view::<Root>(&b).unwrap().head();
        assert_eq!(r.pos, [500.0, 0.0, 100.0]);
        assert!((hsmp_pose::sample::quat_yaw(r.rot) - 90.0).abs() < 1e-3);
        assert_eq!(m.writer_epoch, g.epoch);
        let (_, _, b) = read_slot(&s.game_out.local_pose).unwrap();
        let v = hsmp_ipc::record::view::<PoseHead>(&b).unwrap();
        assert_eq!(v.head.tick, 3);
        let d = hsmp_pose::posecodec::v2::decode(&v.rows).expect("a v2 frame");
        assert!((d.bones[0].p[2] - 100.0).abs() < 0.01);
        assert_eq!(d.weapons[0].id, 7);
        assert!((d.control.as_ref().unwrap().ctrl_yaw - 20.0).abs() < 0.01);
        let mut rec = Record::default();
        assert_eq!(s.g2s().pop(g.epoch, &mut rec), Pop::Record);
        assert_eq!(rec.hdr.kind, hsmp_ipc::schema::combat::K_DEATH_REPORT);
        assert_eq!(hsmp_ipc::record::view::<hsmp_ipc::schema::combat::DeathReport>(rec.payload()).unwrap().head().death_id, 12);

        // sidecar side writes: session + peer dir + peer root + s2g event
        let mut sc = Vec::new();
        for (name, v) in [("session", json!({"epoch": 9, "seq": 1, "phase": 0, "winner_seat": 255, "config": {"arena": "Map_Arena_Alley"}, "rows": [{"seat": 1, "peer_id": 1, "connected": true, "nick": "A"}]})),
                          ("link", json!({"status": 1, "state": 1, "my_peer_id": 1}))] {
            let (kind, payload) = hsmp_ipc::debug_json::json_to_record(name, &v).unwrap();
            assert!(s.slot_ref(name, 0).unwrap().put(SlotMeta { valid: 1, ..Default::default() }, kind, &payload, &mut sc));
        }
        let mut d = Box::<PeerDir>::default();
        d.count = 1;
        d.entries[3].active = 1;
        d.entries[3].peer_id = 2;
        d.entries[3].set_nick("Bravo");
        s.peers.dir.write(&d);
        let mut pr = PeerRoot::default();
        pr.peer_id = 2;
        pr.root = local_root_from(&json!({"pos": [1.0, 2.0, 3.0], "match_id": 3, "round": 1, "life": 1}));
        let mut m = game_meta(s, 0);
        m.writer_epoch = 0x55;
        assert!(s.peers.slots[3].root.put(m, hsmp_ipc::schema::pose::K_PEER_ROOT, bytemuck::bytes_of(&pr), &mut Vec::new()));
        let mut pp = Box::<PeerPlay>::default();
        pp.peer_id = 2;
        pp.mask = 1;
        pp.b[0][2] = 99.0;
        s.peers.slots[3].play.write(&pp);
        let (_, res) = hsmp_ipc::debug_json::json_to_record("cmd_result", &json!({"cmd_id": 12, "ok": true, "op": 1})).unwrap();
        s.s2g().push(0x55, schema::session::K_CMD_RESULT, 0, id, &res).unwrap();
        s.s2g().push(0x44, schema::session::K_CMD_RESULT, 0, 1, &[0]).unwrap(); // stale epoch: dropped
        let ev = g.pump();
        let kinds: Vec<&str> = ev.iter().filter_map(|e| e["ev"].as_str()).collect();
        assert!(kinds.contains(&"slot") && kinds.contains(&"peer_dir") && kinds.contains(&"peer_root") && kinds.contains(&"peer_play"), "{kinds:?}");
        assert_eq!(g.session().unwrap()["rows"][0]["peer_id"], 1);
        assert!(g.connected() && g.link().unwrap()["my_peer_id"] == 1);
        assert_eq!(legacy_state(g.session().unwrap()["phase"].as_u64().unwrap()), "lobby");
        assert_eq!(session_arena(g.session().unwrap()).as_deref(), Some("Map_Arena_Alley"));
        let res: Vec<&J> = ev.iter().filter(|e| e["ev"] == "s2g").collect();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0]["kind"], "cmd_result");
        assert_eq!(res[0]["v"]["ok"], true);
        assert_eq!(g.seg().header.game.counter(ctr::STALE_EPOCH), 1);
        // nothing new: quiet frame (play not re-reported)
        let ev = g.pump();
        assert!(ev.is_empty(), "{ev:?}");

        // dump
        let d = dump_json(g.seg(), 8, true);
        assert_eq!(d["game_out"]["local_root"]["v"]["tick"], 42);
        assert_eq!(d["peers"][0]["nick"], "Bravo");
        assert_eq!(d["peers"][0]["play"]["v"]["pelvis"][2], 99.0);
        assert_eq!(d["records"]["session"]["v"]["config"]["arena"], "Map_Arena_Alley");
        assert_eq!(d["g2s_last"].as_array().unwrap().len(), 1);
        assert_eq!(d["header"]["sidecar"]["state"], "ready");
    }
}

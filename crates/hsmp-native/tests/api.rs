//! End-to-end test of the Lua API against an in-process fake sidecar that
//! attaches to the real named mapping. Lua comes from mlua's vendored 5.4 (ffi only).
//!
//! One test function on purpose: the native state is process-global (like in the game).
//! Correctness and the 10k-frame allocation regression run by default. Timing-only
//! loops require `HSMP_NATIVE_BENCH=1 cargo test -p hsmp-native --release --test api -- --nocapture`
//! (PowerShell: set `$env:HSMP_NATIVE_BENCH='1'` before the command, remove it afterward).

use std::ffi::{CStr, CString};

use hsmp_ipc::handshake::{attach_sidecar, SideParams};
use hsmp_ipc::schema::pose::{PeerDir, PeerPlay, PeerRoot, PoseHead, Root, PEER_PLAY_HAS_CONTROL, PEER_PLAY_HAS_ROOT, PEER_PLAY_V2};
use hsmp_ipc::schema::{self as sc, SlotMeta};
use hsmp_ipc::segment::Segment;
use hsmp_ipc::schema::RawSlot;
use hsmp_ipc::shm::{self, Access, Mapping};
use mlua::ffi;

type L = *mut ffi::lua_State;

fn new_state(name: &str) -> L {
    unsafe {
        let l = ffi::luaL_newstate();
        ffi::luaL_openlibs(l);
        let n = CString::new(name).unwrap();
        assert_eq!(hsmp_native::hsmp_native_open(l as *mut _, n.as_ptr()), 1);
        l
    }
}

fn run(l: L, code: &str) {
    unsafe {
        let c = CString::new(code).unwrap();
        let rc = ffi::luaL_loadstring(l, c.as_ptr());
        let rc = if rc == 0 { ffi::lua_pcall(l, 0, 0, 0) } else { rc };
        if rc != 0 {
            let msg = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
            ffi::lua_settop(l, 0);
            panic!("lua error: {}\n--- chunk ---\n{}", msg, code);
        }
    }
}

fn eval_str(l: L, expr: &str) -> String {
    unsafe {
        let c = CString::new(format!("return tostring({})", expr)).unwrap();
        assert_eq!(ffi::luaL_loadstring(l, c.as_ptr()), 0);
        assert_eq!(ffi::lua_pcall(l, 0, 1, 0), 0, "eval {}", expr);
        let s = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
        ffi::lua_settop(l, 0);
        s
    }
}

struct FakeSidecar {
    map: Mapping,
    epoch: u64,
}

impl FakeSidecar {
    fn attach(name: &str, epoch: u64) -> FakeSidecar {
        let map = Mapping::open(name, Access::ReadWrite).expect("open mapping");
        let pid = shm::current_pid();
        let ct = shm::process_create_time(pid).unwrap();
        let p = SideParams { pid, create_time: ct, epoch, caps: u64::MAX, build_id: "fake-sidecar" };
        attach_sidecar(map.segment().unwrap(), map.len(), pid, Some(ct), &p).expect("attach");
        FakeSidecar { map, epoch }
    }
    fn seg(&self) -> &Segment {
        self.map.segment().unwrap()
    }
    fn meta(&self) -> SlotMeta {
        SlotMeta { writer_epoch: self.epoch, valid: 1, ..Default::default() }
    }
    fn set_peers(&self, n: usize) {
        let mut d = hsmp_ipc::layout::boxed_zeroed::<PeerDir>();
        d.meta = self.meta();
        d.count = n as u32;
        for i in 0..n {
            let e = &mut d.entries[i];
            e.active = 1;
            e.peer_id = 100 + i as u32;
            e.gen = 1;
            e.set_nick(if i == 0 { "Grüße" } else { "peer" });
        }
        self.seg().peers.dir.write(&d);
    }
    fn write_play(&self, slot: usize, play_seq: u64) {
        let mut p = hsmp_ipc::layout::boxed_zeroed::<PeerPlay>();
        p.meta = self.meta();
        p.peer_id = 100 + slot as u32;
        p.play_seq = play_seq;
        p.mode = 1;
        p.pt = 1234.5;
        p.flags = PEER_PLAY_V2 | PEER_PLAY_HAS_ROOT | PEER_PLAY_HAS_CONTROL;
        p.mask = (1 << 25) - 1;
        p.vmask = p.mask;
        p.root = [1.0, 2.0, 3.0, 90.0];
        for i in 0..25 {
            for j in 0..13 {
                p.b[i][j] = (i * 13 + j) as f32;
            }
        }
        p.w[0].present = 1;
        p.w[0].hands = 1;
        p.w[0].class_id = 7;
        p.w[0].tip = [0.0, 0.0, 100.0];
        p.control.flags = 5;
        p.control.ik_world = 3;
        self.seg().peers.slots[slot].play.write(&p);
    }
    fn write_root(&self, slot: usize) {
        let root = Root { tick: 9, ts: 77, send_wall_ms: 1, pos: [1.0, 2.0, 3.0], rot: hsmp_pose::sample::rotator_to_quat(0.0, 90.0, 0.0), vel: [0.0; 3], match_id: 1, round: 1, life: 1, _r: 0 };
        let r = PeerRoot { peer_id: 100 + slot as u32, _r: 0, root };
        assert!(self.seg().peers.slots[slot].root.put(self.meta(), sc::pose::K_PEER_ROOT, bytemuck::bytes_of(&r), &mut Vec::new()));
    }
    /// A sidecar-written record blob (world_owners, one owner row) plus a session record (its
    /// slot seq drives SESSION_CHANGED).
    fn publish_session(&self) {
        let oh = sc::world::OwnersHead { level: 3, epoch: 1, manifest_len: 1, n: 0, sync: hsmp_ipc::layout::Bool::TRUE, _r: 0 };
        let op = hsmp_ipc::record::to_payload(&oh, &[sc::world::OwnerRec::new(9, 2, 1, 1)]);
        let ow = self.seg().slot_ref("world_owners", 0).unwrap();
        assert!(ow.put(self.meta(), sc::world::K_WORLD_OWNERS, &op, &mut Vec::new()));
        let head = sc::session::SessionHead { epoch: 1, seq: 1, ..Default::default() };
        let payload = hsmp_ipc::record::to_payload(&head, &[]);
        let slot = self.seg().slot_ref("session", 0).unwrap();
        assert!(slot.put(self.meta(), sc::session::K_SESSION, &payload, &mut Vec::new()));
    }
    /// A typed S2G record (the wire header fields + the record bytes).
    fn push_rec(&self, kind: u16, peer: u32, payload: &[u8]) {
        self.seg().s2g().push_msg(self.epoch, kind, 0, peer, 0, 42, payload).unwrap();
    }
    fn pop_g2s(&self) -> Option<(u16, Vec<u8>)> {
        let mut r = hsmp_ipc::ring::Record::default();
        match self.seg().g2s().pop(0, &mut r) {
            hsmp_ipc::ring::Pop::Record => Some((r.hdr.kind, r.payload().to_vec())),
            _ => None,
        }
    }
}

#[test]
fn lua_api_end_to_end() {
    let a = new_state("HSMPSync");
    let b = new_state("HSMPAvatars");
    let c = new_state("HSMPHud");

    // Before open: nothing raises.
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        assert(N._impl == "F")
        local ma, mi, h = N.abi(); assert(ma == 2 and mi == 0 and #h == 16)
        assert(N.frame("w1") == 0)
        local r, e = N.send("death_report", {}); assert(r == nil and e == "not open", tostring(e))
        local info = N.ipc_info(); assert(info.state == "closed")
        assert(N.thread_ok() == true)
    "#);
    let name = eval_str(a, "HSMPNative.ipc_open()");
    assert!(name.starts_with(&format!("Local\\HSMP.ipc.{}.", hsmp_ipc::ABI_MAJOR)), "{}", name);
    assert_eq!(eval_str(a, "HSMPNative.ipc_open()"), name, "idempotent");

    // Game writes before any sidecar exists.
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        assert(N.put_root(1, 10.5, 1, 2, 3, 0, 90, 0, 4, 5, 6, root_context))
        local _,scoped=N.get("local_root",-1)
        assert(scoped.match_id==1 and scoped.round==1 and scoped.life==1)
        assert(not N.put_root(2,11,1,2,3,0,0,0,0,0,0),"missing original context refused")
        root_context.match_id=9007199254740993
        assert(N.put_root(2,11,1,2,3,0,0,0,0,0,0,root_context))
        local _,wide=N.get("local_root",-1)
        assert(wide.match_id==9007199254740993,"full integer identity preserved")
        root_context.match_id=1
        assert(N.put_root(1,10.5,1,2,3,0,90,0,4,5,6,root_context))
        assert(N.put_weapon(1, 10.5, 77, 1, 1, 2, 3, 0, 90, 0, 0, 0, 0))
        local b = {}
        for i = 1, 23 * 13 do b[i] = i * 0.5 end
        local w = {}
        for i = 1, 21 do w[i] = i end
        local c = {}
        for i = 1, 37 do c[i] = i end
        assert(N.put_pose(5, 11.25, 8.3, 1.0, b, w, c))
        local r, e = N.put_pose(5, 11.25, 8.3, 1.0, {1, 2, 3}); assert(r == nil and e == "bad")
        r, e = N.put_root("x", root_context); assert(r == nil and e == "bad")
        assert(N.put_lead(12.5))
        assert(N.put("vitals", {seq = 1, flags = 1, v = {5760}}))
        r, e = N.put("session", {}); assert(r == nil and e == "bad")
        r, e = N.send("death_report", {}); assert(r == nil and e == "cap", tostring(e))
        r, e = N.put("nope", {}); assert(r == nil and e == "bad")
    "#);

    // The fake sidecar reads the game's slots.
    let sc1 = FakeSidecar::attach(&name, 0xA1);
    {
        let s = sc1.seg();
        let (mut sc_, mut out) = (Vec::new(), Vec::new());
        let (m, k, _) = s.game_out.local_root.get(&mut sc_, &mut out).unwrap();
        assert_eq!((m.writer_epoch != 0, m.valid, k), (true, 1, sc::pose::K_ROOT));
        let root = hsmp_ipc::record::view::<Root>(&out).unwrap().head();
        assert_eq!((root.tick, root.ts), (1, 10));
        assert_eq!(root.pos, [1.0, 2.0, 3.0]);
        assert_eq!(root.vel, [4.0, 5.0, 6.0]);
        assert_eq!(root.rot, hsmp_pose::sample::rotator_to_quat(0.0, 90.0, 0.0), "rotator -> quaternion once, in the native module");
        assert!(root.send_wall_ms > 1_600_000_000_000, "wall clock at sampling");
        let (_, k, _) = s.game_out.local_weapon.get(&mut sc_, &mut out).unwrap();
        assert_eq!(k, sc::pose::K_WEAPON);
        let w = hsmp_ipc::record::view::<sc::pose::Weapon>(&out).unwrap().head();
        assert_eq!((w.tick, w.weapon_id, w.held), (1, 77, 1));
        // The pose record is the codec v2 frame of exactly these Lua numbers (hsmp_pose::sample).
        let (_, k, _) = s.game_out.local_pose.get(&mut sc_, &mut out).unwrap();
        assert_eq!(k, sc::pose::K_POSE);
        let b: [f64; 23 * 13] = std::array::from_fn(|i| (i + 1) as f64 * 0.5);
        let wv: [f64; 21] = std::array::from_fn(|i| (i + 1) as f64);
        let cv: [f64; 37] = std::array::from_fn(|i| (i + 1) as f64);
        let a = hsmp_pose::sample::PoseArgs { tick: 5.0, ts: 11.25, dt: 8.3, b: &b, w: &[wv], c: Some(&cv) };
        let mut want = sc::pose::PoseBuf::new_boxed();
        assert!(hsmp_pose::sample::encode_pose(&a, &mut Default::default(), &mut want));
        assert_eq!(&out[..], want.payload(), "put_pose bytes == the reference encoder");
        let v = hsmp_ipc::record::view::<PoseHead>(&out).unwrap();
        let d = hsmp_pose::posecodec::v2::decode(&v.rows).expect("v2 frame");
        assert!((d.bones[0].p[0] - 0.5).abs() < 0.01 && d.weapons.len() == 1 && d.control.is_some());
        let (_, k, _) = s.game_out.local_vitals.get(&mut sc_, &mut out).unwrap();
        assert_eq!(k, sc::combat::K_VITALS);
        let vt = hsmp_ipc::record::view::<sc::combat::Vitals>(&out).unwrap().head();
        assert_eq!((vt.seq, vt.flags, vt.v[0]), (1, 1, 5760));
        assert_eq!(s.game_out.pose_lead.read().unwrap().0.lead_ms, 12.5);
    }

    // frame: SIDECAR_RESET, peers, play, session, events.
    sc1.set_peers(7);
    for slot in 0..7 {
        sc1.write_play(slot, 1);
    }
    sc1.write_root(0);
    sc1.publish_session();
    let verdict = |cid: u32| hsmp_ipc::record::to_payload(&sc::combat::DamageVerdict::new(5, cid, sc::combat::VERDICT_FINAL, true, ""), &[]);
    let death = |peer: u32| hsmp_ipc::record::to_payload(&sc::combat::Death::new(peer, 2, 1, 1), &[]);
    sc1.push_rec(sc::combat::K_DAMAGE_VERDICT, 0, &verdict(12));
    sc1.push_rec(sc::combat::K_DEATH, 101, &death(101));
    run(a, r#"
        local N, F = HSMPNative, HSMPNative.frame
        local f = N.frame("w1")
        assert(f & 0x8 ~= 0, "SIDECAR_RESET " .. f)
        assert(f & 0x4 ~= 0, "PEERS_CHANGED")
        assert(f & 0x1 ~= 0, "SESSION_CHANGED")
        assert(f & 0x2 ~= 0, "EVENTS")
        assert(N.flags() == f)
        local out = {}
        assert(N.peers(out) == 7)
        assert(out[1].id == 100 and out[1].slot == 0 and out[1].nick == "Grüße" and out[1].play_seq > 0)
        local p = {}
        local seq = N.peer_play(0, p, -1)
        assert(seq and p.peer_id == 100 and p.seq == 1 and p.mode == "extrap" and p.v2 == true and p.pt == 1234.5)
        assert(#p.B == 25 * 13 and p.B[1] == 0 and p.B[325] == 324)
        assert(#p.W == 8 and p.W[1] == 1 and p.W[2] == 7 and p.W[8] == 100)
        assert(#p.C == 37 and p.C[1] == 5 and p.C[37] == 3)
        assert(p.root[4] == 90)
        assert(N.peer_play(0, p, seq) == nil, "unchanged")
        local r = N.peer_play(31, p, -1); assert(r == nil)
        local r2, e2 = N.peer_play(99, p); assert(r2 == nil and e2 == "bad")
        -- peer_root is a record slot (the relayed root record + its owner), read generically.
        local rs, rt = N.peer("peer_root", 0, -1)
        assert(rs and rt.peer_id == 100 and rt.root.ts == 77 and rt.root.pos[3] == 3, tostring(rt))
        local g, st = N.get("world_owners", -1)
        assert(g and st.level == 3 and st.sync == true and st.n == 1 and st.rows[1].id == 9 and st.rows[1].owner == 2)
        assert(N.get("world_owners", g) == g)
        local g2, st2 = N.get("world_owners", g); assert(st2 == nil)
        local ev = {}
        assert(N.poll(10, ev) == 2)
        assert(ev[1].kind == "damage_verdict" and ev[1].data.cid == 12 and ev[1].req_id == 42)
        assert(ev[2].kind == "death" and ev[2].data.peer_id == 101 and ev[2].peer == 101)
        assert(N.poll(10, ev) == 0 and ev[1] == nil)
        local id = N.send("death_report", {death_id = 7, round = 3})
        assert(math.type(id) == "integer" and id > 0)
    "#);
    let (k, v) = sc1.pop_g2s().expect("g2s record");
    assert_eq!(k, sc::combat::K_DEATH_REPORT);
    let dr = hsmp_ipc::record::view::<sc::combat::DeathReport>(&v).unwrap().head();
    assert_eq!((dr.death_id, dr.round), (7, 3));

    // Fan-out to 3 Lua states (b, c registered before the events; c filters).
    run(c, r#"assert(HSMPNative.subscribe({"death"}))"#);
    sc1.push_rec(sc::combat::K_DAMAGE_VERDICT, 0, &verdict(1));
    sc1.push_rec(sc::combat::K_DEATH, 102, &death(102));
    run(a, r#"local ev = {}; assert(HSMPNative.poll(10, ev) == 2)"#);
    run(b, r#"local ev = {}; local n = HSMPNative.poll(10, ev); assert(n == 4, n)"#);
    run(c, r#"local ev = {}; local n = HSMPNative.poll(10, ev); assert(n == 2 and ev[1].kind == "death" and ev[2].data.peer_id == 102, n)"#);

    // Bus round trip (typed records only: UTF-8, rows, world-scoped clear).
    run(b, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        assert(N.bus_put("conn_state", {wall = 2.5, seq = 7, attempt = -1, in_match = true, state = "fight", reason = "Grüße"}))
        assert(N.bus_put("puppets", {rows = {{peer = 101, name = "Willie_1"}}}))
        local r, e = N.bus_put("dev_notes", {phase = "fight"}); assert(r == nil and e == "bad", "a key that is not a typed record")
    "#);
    run(c, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        local g, t = N.bus_get("conn_state", -1)
        assert(g > 0 and t.state == "fight" and t.reason == "Grüße" and t.wall == 2.5 and t.attempt == -1 and t.in_match == true)
        assert(math.type(t.seq) == "integer" and math.type(t.wall) == "float")
        assert(N.bus_get("conn_state", g) == g)
        assert(N.bus_get("spectate", -1) == 0, "a typed key never written")
        local pg, pt = N.bus_get("puppets", -1); assert(pt.n == 1 and pt.rows[1].peer == 101 and pt.rows[1].name == "Willie_1")
        local r, e = N.bus_get(string.rep("k", 33), -1); assert(r == nil and e == "bad")
    "#);

    // World leave / ready.
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        local we = N.ipc_info().world_epoch
        assert(N.world_leaving())
        assert(N.ipc_info().world_epoch == we + 1 and N.ipc_info().state == "loading")
        local g, t = N.bus_get("puppets", -1); assert(g == 0 and t == nil, "world-scoped key cleared (no valid record)")
        local g2, t2 = N.bus_get("conn_state", -1); assert(t2 and t2.state == "fight", "not world-scoped")
        assert(N.world_ready("w2"))
        local f = N.frame("w2"); assert(f & 0x10 == 0, "no second bump for the key the ready set")
        assert(N.ipc_info().world_epoch == we + 1 and N.ipc_info().state == "ready")
        f = N.frame("w3"); assert(f & 0x10 ~= 0)
        assert(N.ipc_info().world_epoch == we + 2)
    "#);
    {
        let (m, _, _) = sc1.seg().game_out.local_root.get(&mut Vec::new(), &mut Vec::new()).unwrap();
        assert_eq!(m.valid, 0, "republished invalid at world leave");
    }

    // Wrong thread.
    std::thread::spawn(|| {
        let l = new_state("HSMPOther");
        run(l, r#"
            local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
            local r, e = N.send("death_report", {}); assert(r == nil and e == "wrong thread", tostring(e))
            r, e = N.frame("x"); assert(r == nil and e == "wrong thread")
            assert(N.thread_ok() == false)
            local ma = N.abi(); assert(ma == 2)
        "#);
        unsafe { ffi::lua_close(l) };
    })
    .join()
    .unwrap();
    assert!(eval_str(a, "HSMPNative.ipc_info().counters.wrong_thread").parse::<u64>().unwrap() >= 2);

    // Sidecar restart: new epoch -> SIDECAR_RESET; data of the old epoch is ignored.
    drop(sc1);
    let sc2 = FakeSidecar::attach(&name, 0xB2);
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        local f = N.frame("w3")
        assert(f & 0x8 ~= 0, "SIDECAR_RESET on restart")
        local out = {}
        assert(N.peers(out) == 0, "old epoch directory ignored")
        local p = {}
        assert(N.peer_play(0, p, -1) == nil, "old epoch play ignored")
        assert(N.get("world_owners", -1) == nil, "old epoch record blob dropped")
    "#);
    sc2.set_peers(7);
    for slot in 0..7 {
        sc2.write_play(slot, 2);
    }
    sc2.publish_session();

    // Allocation-free hot path always runs; redundant timing loops are opt-in.
    let timings = std::env::var("HSMP_NATIVE_BENCH").as_deref() == Ok("1");
    run(a, &format!("BENCH_TIMINGS = {timings}"));
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        local out, plays, ev, b, w, c = {}, {}, {}, {}, {}, {}
        for i = 1, 7 do plays[i] = {} end
        for i = 1, 23 * 13 do b[i] = i * 0.25 end
        for i = 1, 21 do w[i] = i end
        for i = 1, 37 do c[i] = i end
        local g = N.get("world_owners", -1)
        local seqs = {}
        for i = 1, 7 do seqs[i] = N.peer_play(i - 1, plays[i], -1) end
        local function one()
            N.frame("w3")
            N.put_root(1, 2, 1, 2, 3, 0, 0, 0, 0, 0, 0, root_context)
            N.put_pose(1, 2, 3, 1, b, w, c)
            N.peers(out)
            for i = 1, 7 do N.peer_play(i - 1, plays[i], -1) end
            N.poll(16, ev)
            N.get("world_owners", g)
        end
        for i = 1, 100 do one() end
        collectgarbage("collect"); collectgarbage("stop")
        local k0 = collectgarbage("count")
        for i = 1, 10000 do one() end
        local grew = collectgarbage("count") - k0
        collectgarbage("restart")
        assert(grew < 1, "hot path allocated " .. grew .. " KiB over 10k frames")
        print(string.format("ALLOCATION hot path: %.3f KiB over 10k frames", grew))
        if BENCH_TIMINGS then
        local function bench(label, n, fn)
            local t0 = N.now_us()
            for i = 1, n do fn() end
            local us = (N.now_us() - t0) / n
            print(string.format("TIMING %-28s %8.3f us", label, us))
        end
        bench("frame", 20000, function() N.frame("w3") end)
        bench("put_root", 20000, function() N.put_root(1, 2, 1, 2, 3, 0, 0, 0, 0, 0, 0, root_context) end)
        bench("put_pose (23x13+w+c)", 20000, function() N.put_pose(1, 2, 3, 1, b, w, c) end)
        bench("peers (7)", 20000, function() N.peers(out) end)
        bench("peer_play x7 (changed)", 5000, function() for i = 1, 7 do N.peer_play(i - 1, plays[i], -1) end end)
        bench("peer_play x7 (unchanged)", 20000, function() for i = 1, 7 do N.peer_play(i - 1, plays[i], seqs[i]) end end)
        bench("poll (empty)", 20000, function() N.poll(16, ev) end)
        bench("get unchanged (blob)", 20000, function() N.get("world_owners", g) end)
        bench("send death_report", 400, function() N.send("death_report", {death_id = 1, round = 1}) end)
        bench("bus_put small", 20000, function() N.bus_put("playback", {rows = {{peer = 2, body_ts = 1.5, arm_ts = 2.5, local_ms = 3}}}) end)
        local bg = N.bus_get("playback", -2)
        bench("bus_get unchanged", 20000, function() N.bus_get("playback", bg) end)
        bench("bus_get changed (decode)", 20000, function() N.bus_get("playback", -2) end)
        bench("full frame (8p budget)", 5000, one)
        end
    "#);
    // Drain what the bench sent so the ring is clean.
    while sc2.pop_g2s().is_some() {}

    // G2S full -> "full", never raises.
    run(a, r#"
        local N = HSMPNative; local root_context={match_id=1,round=1,life=1}
        local full
        for i = 1, 600 do
            local r, e = N.send("death_report", {death_id = i, round = i})
            if not r then full = e break end
        end
        assert(full == "full", tostring(full))
    "#);

    // Sidecar death: SIDECAR_LOST via the process handle needs another process; covered by
    // the CMake harness (fake-sidecar exe). Here: refusal flag.
    hsmp_ipc::handshake::write_refusal(
        sc2.seg(),
        &hsmp_ipc::handshake::Refusal { code: hsmp_ipc::RefuseCode::AbiMismatch, detail: "test".into() },
    );
    run(a, r#"local f = HSMPNative.frame("w3"); assert(f & 0x40 ~= 0, "REFUSED")"#);

    unsafe {
        ffi::lua_close(a);
        ffi::lua_close(b);
        ffi::lua_close(c);
    }
}

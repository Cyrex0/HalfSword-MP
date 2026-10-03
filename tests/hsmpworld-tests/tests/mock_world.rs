//! HSMPWorld against a mocked UE4SS (ue4ss_mock.lua): dynamic-item spawn,
//! follow, round-reset restore, physics-safety guards and the drop filter.
//! Each test runs a Lua scenario in a fresh Lua 5.4 state; a scenario fails
//! by raising an error. Every scenario also asserts that no call touched an
//! invalid (pending-kill) object and no pooled Willie was destroyed.

use mlua::Lua;
use std::path::PathBuf;

fn dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }

fn read(p: PathBuf) -> String {
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Fresh state: mock installed as `M`, HSMPWorld's test table as `T`, a
/// level `W` in Map_Arena_Alley with me = peer 2, and `H` helpers.
fn setup() -> Lua {
    let lua = Lua::new();
    let mock = read(dir().join("ue4ss_mock.lua"));
    let main = read(dir().join("../../mods/HSMPWorld/Scripts/main.lua"));
    lua.load(r#"HSMP_WORLD_TEST = true"#).exec().unwrap();
    let m: mlua::Table = lua.load(&mock).set_name("@ue4ss_mock.lua").eval().unwrap();
    lua.globals().set("M", m).unwrap();
    lua.load("M.install(_G)").exec().unwrap();
    // shared/hsmp_wg.lua (the world guard HSMPWorld requires); deploy copies it into Scripts/
    let shared = dir().join("../../mods/shared").to_string_lossy().replace('\\', "/");
    lua.load(format!("package.path = \"{shared}/?.lua;\" .. package.path")).exec().unwrap();
    // the file-backed HSMPNative mock (tools/hsmp-tools/lua-tests/lib)
    lua.load(format!("HSMP_TEST_LIB = \"{shared}/../../tools/hsmp-tools/lua-tests/lib\"; HSMPNative = dofile(HSMP_TEST_LIB .. \"/hsmp_native_filemock.lua\").new()")).exec().unwrap();
    let t: mlua::Table = lua.load(&main).set_name("@HSMPWorld/Scripts/main.lua").eval()
        .expect("main.lua loads in test mode");
    lua.globals().set("T", t).unwrap();
    lua.load(r#"
        T.set_name_none(FName("None"))
        T.sess.my_id, T.sess.connected = 2, true
        T.new_level("World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley", "key", 0)
        W = T.W()
        W.synced, W.epoch, W.mine_awake = true, 5, {}
        -- arena bounds as finish_scan would set them
        W.bounds = T.bounds_of({ {X=-2800,Y=-700,Z=10}, {X=1700,Y=800,Z=770}, {X=0,Y=0,Z=100} })
        H = {}
        LONG = "/Game/Assets/Weapons/Blueprints/Built_Weapons/ModularWeaponBP_LongSword_T3.ModularWeaponBP_LongSword_T3_C"
        function H.weapon_class()
            local c = M.make_class(LONG, true)
            M.loadable[LONG] = c          -- not loaded until LoadAsset (as in game)
            return c
        end
        function H.entry(id, dyn, x, y, z)
            return { id = id, chash = T.fnv1a("ModularWeaponBP_LongSword_T3_C|W"), pos = {X=x,Y=y,Z=z},
                     dyn = dyn, class = LONG }
        end
        function H.no_violations()
            if #M.violations > 0 then error("violations:\n" .. table.concat(M.violations, "\n")) end
        end
        function H.expect(cond, msg) if not cond then error(msg .. "\nlog:\n" .. table.concat(M.logs, "")) end end
        function H.sample(o, sender, ts, x, y, z, flags)
            -- what ingest_remote does with one streamed row
            o.buf_sender = sender
            table.insert(o.buf, { t = ts, pos = {X=x,Y=y,Z=z}, q = {0,0,0,1}, vel = {X=0,Y=0,Z=0}, flags = flags })
            W.render[sender] = { ms = ts, latest = ts }
            o.settled = false
        end
    "#).exec().unwrap();
    lua
}

fn run(lua: &Lua, name: &str, src: &str) {
    if let Err(e) = lua.load(src).set_name(name).exec() {
        panic!("{name} failed:\n{e}");
    }
    lua.load("H.no_violations()").exec().unwrap_or_else(|e| panic!("{name}: {e}"));
}

#[test]
fn peer_drop_spawns_finished_copy_with_passport_and_follows() {
    let lua = setup();
    run(&lua, "spawn_and_follow", r#"
        local cls = H.weapon_class()
        -- Peer 1's stand-in still holds its copy of the kit weapon.
        local wil = M.make_willie("Willie_BP_C_2147482000")
        local held = M.make_weapon(cls, "ModularWeaponBP_LongSword_T3_C_1", {X=100,Y=0,Z=100}, false)
        held._props["Weapon Passport"] = M.passport(7)
        wil._props["Weapon R"] = held
        W.puppets = { { id = 1, actor = wil } }
        -- Newest streamed pose of the item, seen before it is spawned here.
        local id = 0x80000000 | (1 << 16) | 1
        W.unbound_rows[id] = { id = id, sender = 1, ts = 500, pos = {X=120,Y=10,Z=40}, q = {0,0,0.7071,0.7071},
                               vel = {X=0,Y=0,Z=0}, flags = 0 }

        T.bind_dyn({ H.entry(id, 1, 110, 0, 90) }, 1000)
        T.service_dyn(1000)

        local o = W.by_nid[id]
        H.expect(o ~= nil, "copy bound to the dynamic id")
        local a = o.actor
        H.expect(rawget(a, "_finished"), "FinishSpawningActor ran (3 parameters)")
        H.expect(rawget(a, "_finish_scale") == 1 and rawget(a, "_begin_scale") == 1, "MultiplyWithRoot both calls")
        local t = rawget(a, "_finish_t")
        H.expect(t.Translation.X == 120 and t.Translation.Z == 40, "spawned at the newest streamed pose")
        H.expect(math.abs(t.Rotation.W - 0.7071) < 1e-6, "FQuat rotation")
        H.expect(rawget(a, "_simphys_at_finish") == true, "'Simulates Physics' set before construction")
        local pp = rawget(a, "_passport_at_finish")
        H.expect(pp.ID_70_C02CF656483647A1933EEA96314B78A6 == 7, "peer's passport copied before construction")
        H.expect(pp.HeadSize_21_2D425E61473B8F64FBAB51B223459D57.X == 7, "struct members copied by value")
        H.expect(pp.Name_57_3729B51148E846FE8DD336B9419BCEE1:ToString() == "Blade7", "FName copied")
        H.expect(#M.spawned == 1, "exactly one actor spawned")
        H.expect(M.log_has("spawned local copy %(peer passport%)"), "log")

        -- Peer 1 owns it (touch lease) and streams the fall: we follow kinematically.
        W.owners[id] = { owner = 1, ver = 3, mode = 1 }
        H.sample(o, 1, 600, 130, 10, 30, 8)
        T.update_body(o, 1100, false)
        local b = o.body
        H.expect(rawget(b, "_sim") == false, "follower is kinematic")
        H.expect(rawget(b, "_pos").X == 130 and rawget(b, "_pos").Z == 30, "follows the stream")
        H.sample(o, 1, 700, 140, 10, 5, 8 | 1)   -- asleep frame
        T.update_body(o, 1200, false)
        H.expect(rawget(b, "_pos").X == 140, "reaches the rest frame")

        -- Released: physics back on AFTER the rest-frame teleport, then asleep.
        W.owners[id] = { owner = 0, ver = 4, mode = 0 }
        M.events = {}
        T.update_body(o, 1300, false)
        local ev = M.events_of(rawget(b, "_name"))
        local tp, sim, sleep
        for i, e in ipairs(ev) do
            if e:find("^tp") and not tp then tp = i end
            if e:find("^sim .* true") and not sim then sim = i end
            if e:find("^sleep") then sleep = i end
        end
        H.expect(rawget(b, "_sim") == true, "free again: simulating (pick-up-able)")
        H.expect(tp and sim and tp < sim, "teleport while kinematic, then physics on: " .. table.concat(ev, " | "))
        H.expect(sleep and sleep > sim, "put to sleep at rest")
    "#);
}

#[test]
fn finish_failure_retires_half_spawned_actor_and_retries_with_backoff() {
    let lua = setup();
    run(&lua, "finish_failure", r#"
        H.weapon_class()
        M.finish_fails = true
        local id = 0x80000000 | (1 << 16) | 9
        T.bind_dyn({ H.entry(id, 1, 300, 0, 50) }, 0)
        local now = 0
        for i = 1, 200 do now = now + 16; T.service_dyn(now) end   -- ~3.2 s of ticks
        H.expect(W.by_nid[id] == nil, "nothing bound")
        local n = #M.spawned
        H.expect(n >= 3 and n <= 5, "retried with backoff, not every tick (" .. n .. " tries)")
        for _, a in ipairs(M.spawned) do
            H.expect(rawget(a, "_valid") == false, "half-spawned actor destroyed")
            H.expect(rawget(a, "_hidden") == true and rawget(a, "_collision") == false, "made inert first")
        end
        for i = 1, 2000 do now = now + 16; T.service_dyn(now) end
        H.expect(#M.spawned == 5, "gives up after 5 tries")
        H.expect(M.log_has("FinishSpawningActor"), "failure reason logged")
        -- A later success path still works for other items.
        M.finish_fails = false
        local id2 = id + 1
        T.bind_dyn({ H.entry(id2, 1, 300, 0, 50) }, now)
        T.service_dyn(now)
        H.expect(W.by_nid[id2] ~= nil, "next item spawns")
    "#);
}

#[test]
fn spawn_refuses_pawns_and_non_weapon_classes() {
    let lua = setup();
    run(&lua, "class_filter", r#"
        local willie = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C"
        M.classes[willie] = M.make_class(willie, false)
        local door = "/Game/Props/BP_Door.BP_Door_C"
        M.classes[door] = M.make_class(door, false)
        local e1 = { id = 0x80010001, chash = 1, pos = {X=10,Y=0,Z=0}, dyn = 1, class = willie }
        local e2 = { id = 0x80010002, chash = 1, pos = {X=10,Y=0,Z=0}, dyn = 1, class = door }
        local e3 = { id = 0x80010003, chash = 1, pos = {X=10,Y=0,Z=0}, dyn = 1, class = "C:/evil" }
        T.bind_dyn({ e1, e2, e3 }, 0)
        for i = 1, 400 do T.service_dyn(i * 16) end
        H.expect(#M.spawned == 0, "nothing spawned")
        H.expect(T.weapon_class_path_ok(LONG) and not T.weapon_class_path_ok(willie), "path filter")
    "#);
}

#[test]
fn round_reset_restore_is_physics_safe() {
    let lua = setup();
    run(&lua, "restore", r#"
        local cls = H.weapon_class()
        -- (a) a spawned copy lying free, (b) a spawned copy a stand-in holds natively,
        -- (c) a moved scene body, (d) an untouched scene body.
        local id = 0x80000000 | (1 << 16) | 1
        T.bind_dyn({ H.entry(id, 1, 200, 0, 50), H.entry(id + 1, 1, 220, 0, 50) }, 0)
        T.service_dyn(0); T.service_dyn(1)
        local free_copy, held_copy = W.by_nid[id], W.by_nid[id + 1]
        H.expect(free_copy and held_copy, "both copies spawned")
        held_copy.actor._props["Is Held"] = true

        local function scene(name, x, moved)
            local a = M.make_weapon(cls, name, {X=x,Y=0,Z=20}, true)
            local o = { lid = x, nid = x, chash = 1, cls = "X", comp = "W", kind = "weapon", actor = a, actor_addr = a:GetAddress(),
                        body = a._props.BaseMesh, sim0 = true, sim = true, spawn_pos = {X=x,Y=0,Z=20},
                        spawn_rot = {Pitch=0,Yaw=0,Roll=0}, anchor_pos = {X=x,Y=0,Z=20}, anchor_rot = {Pitch=0,Yaw=0,Roll=0},
                        pos = {X=x + (moved and 300 or 0),Y=0,Z=20}, rot = {Pitch=0,Yaw=0,Roll=0}, dyn_owner = 0, buf = {},
                        probed_at = os.clock() * 1000 }
            if moved then rawset(a._props.BaseMesh, "_pos", {X=x+300,Y=0,Z=20}) end
            W.objs[#W.objs + 1] = o
            return o
        end
        local moved = scene("Rake_1", 500, true)
        local still = scene("Rake_2", 900, false)

        M.events = {}
        T.restore_all("round reset")
        H.expect(rawget(free_copy.actor or {}, "_valid") == nil, "copy reference dropped")
        local fa = M.spawned[1]
        H.expect(rawget(fa, "_valid") == false and rawget(fa, "_hidden") == true, "free copy retired (inert, destroyed)")
        local ha = M.spawned[2]
        H.expect(rawget(ha, "_valid") == true, "natively held copy left alone")
        local mb = moved.body
        H.expect(rawget(mb, "_pos").X == 500 and rawget(mb, "_sim") == true, "moved body back at spawn, simulating")
        local ev = M.events_of(rawget(mb, "_name"))
        H.expect(ev[1]:find("^sim .* false") and ev[2]:find("^tp .* kin"), "kinematic before the teleport: " .. table.concat(ev, " | "))
        H.expect(#M.events_of(rawget(still.body, "_name")) == 0, "untouched body not kicked")
        H.expect(W.by_nid[id] == nil and #W.objs == 2, "dynamic records dropped")
    "#);
}

#[test]
fn nothing_is_driven_when_held_dead_or_travelling() {
    let lua = setup();
    run(&lua, "guards", r#"
        local cls = H.weapon_class()
        local id = 0x80000000 | (1 << 16) | 1
        T.bind_dyn({ H.entry(id, 1, 200, 0, 50) }, 0)
        T.service_dyn(0)
        local o = W.by_nid[id]
        W.owners[id] = { owner = 1, ver = 1, mode = 2 }

        -- natively held here (a stand-in grabbed it): don't fight the holder
        o.actor._props["Is Held"] = true
        H.sample(o, 1, 100, 250, 0, 60, 8)
        M.events = {}
        T.update_body(o, 100, false)
        H.expect(#M.events_of(rawget(o.body, "_name")) == 0, "held: not touched")
        o.actor._props["Is Held"] = false

        -- level change pending: nothing moves
        T.WG.travel_from = "old-world"
        T.update_body(o, 120, false)
        H.expect(#M.events_of(rawget(o.body, "_name")) == 0, "travel: not touched")
        T.WG.travel_from = nil

        -- bad stream pose (origin) is refused
        o.buf = {}
        H.sample(o, 1, 200, 0, 0, 0, 8)
        T.update_body(o, 200, false)
        H.expect(rawget(o.body, "_pos").X ~= 0 or rawget(o.body, "_pos").Z ~= 0, "never moved to the origin")
        H.expect(M.log_has("refused to move"), "logged")

        -- destroyed by the game: noticed once, never touched again
        M.kill(o.actor)
        T.update_body(o, 300, false)
        H.expect(o.dead and o.actor == nil and o.body == nil, "dead: references dropped")
        T.update_body(o, 400, false)
        T.restore_all("round reset")
    "#);
}

#[test]
fn destroyed_or_swapped_kit_weapon_is_not_a_drop() {
    // Regression: a kit weapon that leaves my hand because it was destroyed or
    // swapped reads back at (0,0,0). Registered as a world item, it would be released
    // at rest and anchored at the origin on every client.
    let lua = setup();
    run(&lua, "origin_drop", r#"
        local cls = H.weapon_class()
        local me = M.make_willie("Willie_BP_C_0")
        rawset(me._props.BaseMesh, "_pos", {X=300,Y=100,Z=20})
        local sword = M.make_weapon(cls, "ModularWeaponBP_LongSword_T3_C_5", {X=320,Y=100,Z=120}, false)
        me._props["Weapon R"] = sword
        M.pc._props.Pawn = me
        -- Track it in hand, then it leaves the hand and reads back at the origin
        -- (destroyed by the fists re-arm / kit re-apply).
        local hands = T.hands
        hands.pawn, hands.pawn_loc = me, {X=300,Y=100,Z=20}
        hands.R, hands.Ra = sword:GetAddress(), sword
        T.track_my_items(0)
        hands.R, hands.Ra = nil, nil
        rawset(sword._props.BaseMesh, "_pos", {X=0,Y=0,Z=0})
        for t = 16, 1700, 16 do T.track_my_items(t) end
        for _, o in ipairs(W.objs) do H.expect(o.dyn_owner ~= 2, "no world item registered for an origin pose") end
        H.expect(M.log_has("not a drop %(origin"), "logged as not a drop")

        -- A real drop (lands next to me after a few frames at the origin) IS an item.
        local sword2 = M.make_weapon(cls, "ModularWeaponBP_LongSword_T3_C_6", {X=320,Y=100,Z=120}, false)
        hands.R, hands.Ra = sword2:GetAddress(), sword2
        T.track_my_items(2000)
        hands.R, hands.Ra = nil, nil
        rawset(sword2._props.BaseMesh, "_pos", {X=0,Y=0,Z=0})       -- detach frame(s): bogus pose
        T.track_my_items(2016)
        T.track_my_items(2150)                                        -- confirm time: still bogus -> wait
        for _, o in ipairs(W.objs) do H.expect(o.dyn_owner ~= 2, "not registered from a bogus pose") end
        rawset(sword2._props.BaseMesh, "_pos", {X=340,Y=120,Z=30})   -- later frames: on the floor
        rawset(sword2._props.BaseMesh, "_sim", true)
        T.track_my_items(2200)
        local got
        for _, o in ipairs(W.objs) do if o.dyn_owner == 2 then got = o end end
        H.expect(got and got.spawn_pos.X == 340, "real drop registered at its floor pose")

        -- Pure verdicts.
        H.expect(T.drop_verdict({X=0,Y=0,Z=0}, false, nil, nil) == "origin", "origin")
        H.expect(T.drop_verdict({X=9000,Y=0,Z=0}, false, nil, W.bounds) == "out-of-arena", "bounds")
        H.expect(T.drop_verdict({X=900,Y=0,Z=50}, false, {X=0,Y=0,Z=50}, W.bounds) == "far-from-me", "far")
        H.expect(T.drop_verdict({X=90,Y=0,Z=50}, true, nil, nil) == "attached", "attached")
        H.expect(T.drop_verdict({X=90,Y=0,Z=50}, false, {X=0,Y=0,Z=50}, W.bounds) == "ok", "ok")
    "#);
}

#[test]
fn origin_anchor_is_ignored() {
    let lua = setup();
    run(&lua, "origin_anchor", r#"
        H.expect(not T.pose_ok({X=0.2,Y=-0.4,Z=0.1}), "origin")
        H.expect(not T.pose_ok({X=0/0,Y=0,Z=5}), "NaN")
        H.expect(not T.pose_ok({X=5,Y=5,Z=-9000}, W.bounds), "below the arena")
        H.expect(T.pose_ok({X=5,Y=5,Z=50}, W.bounds), "inside")
    "#);
}

/// restore_all never touches a body that was not probed recently (it
/// may have been destroyed AND garbage-collected: IsValid would read freed
/// memory); unbound bodies are probed on a slow cadence so they stay fresh.
#[test]
fn restore_skips_unprobed_bodies() {
    let lua = setup();
    run(&lua, "m7", r#"
        local cls = H.weapon_class()
        local function scene(name, x, probed)
            local a = M.make_weapon(cls, name, {X=x,Y=0,Z=20}, true)
            local o = { lid = x, chash = 1, cls = "X", comp = "W", kind = "weapon", actor = a, actor_addr = a:GetAddress(),
                        body = a._props.BaseMesh, sim0 = true, sim = true, spawn_pos = {X=x,Y=0,Z=20},
                        spawn_rot = {Pitch=0,Yaw=0,Roll=0}, anchor_pos = {X=x,Y=0,Z=20}, anchor_rot = {Pitch=0,Yaw=0,Roll=0},
                        pos = {X=x + 300,Y=0,Z=20}, rot = {Pitch=0,Yaw=0,Roll=0}, dyn_owner = 0, buf = {},
                        probed_at = probed }
            rawset(a._props.BaseMesh, "_pos", {X=x+300,Y=0,Z=20})
            W.objs[#W.objs + 1] = o
            return o, a
        end
        local now = os.clock() * 1000
        -- refused by the server (no nid), destroyed and GC'd 10 s ago, never probed since
        local gone, ga = scene("Rake_gone", 500, now - 10000)
        M.free(ga)
        local fresh = scene("Rake_fresh", 900, now)
        T.restore_all("round reset")
        H.expect(rawget(fresh.body, "_pos").X == 900, "a freshly probed moved body is restored")
        -- the freed one was never touched (H.no_violations runs after the scenario)
    "#);
}

/// The session ends while the arena stays loaded: bodies followed
/// kinematically get their physics back, local copies of peers' dropped items
/// are retired, nothing is teleported.
#[test]
fn session_end_releases_the_world() {
    let lua = setup();
    run(&lua, "h17", r#"
        local cls = H.weapon_class()
        local id = 0x80000000 | (1 << 16) | 1
        T.bind_dyn({ H.entry(id, 1, 200, 0, 50) }, 0)
        T.service_dyn(0); T.service_dyn(1)
        local copy = W.by_nid[id]
        H.expect(copy and copy.spawned_by_us, "peer's dropped item spawned locally")
        copy.probed_at = os.clock() * 1000
        local a = M.make_weapon(cls, "Rake_1", {X=500,Y=0,Z=20}, true)
        local o = { lid = 500, nid = 500, chash = 1, cls = "X", comp = "W", kind = "weapon", actor = a, actor_addr = a:GetAddress(),
                    body = a._props.BaseMesh, sim0 = true, sim = false, kin = true, spawn_pos = {X=500,Y=0,Z=20},
                    spawn_rot = {Pitch=0,Yaw=0,Roll=0}, pos = {X=800,Y=0,Z=20}, rot = {Pitch=0,Yaw=0,Roll=0},
                    dyn_owner = 0, buf = {}, probed_at = os.clock() * 1000 }
        rawset(a._props.BaseMesh, "_pos", {X=800,Y=0,Z=20})
        rawset(a._props.BaseMesh, "_sim", false)
        W.objs[#W.objs + 1] = o
        T.release_world("session ended")
        H.expect(rawget(o.body, "_sim") == true and o.kin == false, "followed body simulates again")
        H.expect(rawget(o.body, "_pos").X == 800, "and stays where it is (no teleport)")
        local fa = M.spawned[1]
        H.expect(rawget(fa, "_valid") == false, "the local copy of the peer's dropped item is retired")
    "#);
}

/// Protocol v6: what HSMPWorld hands the native module are typed records (the typed mock,
/// lua-tests/lib/hsmp_native_records.lua, marshals them by the generated schema exactly as
/// the native module does): a claim with its quantised rest pose, the world_held bus key.
#[test]
fn claims_and_held_items_are_typed_records() {
    let lua = setup();
    run(&lua, "typed_records", r#"
        local N = HSMPNative
        local W2 = T.W2
        local rest = W2.qobj(77, {X=10.04,Y=20,Z=30}, {Pitch=0,Yaw=90,Roll=0}, {X=1.4,Y=-2.6,Z=1e9}, 1 | 8)
        W2.claim(77, 0, rest)
        local m = N._rec.sends[#N._rec.sends]
        H.expect(m and m.kind == "world_claim", "a world_claim G2S record")
        local d = m.data
        H.expect(d.id == 77 and d.mode == 0 and d.has_rest == true and d.level == W.level and d.epoch == 5, "claim head")
        H.expect(math.abs(d.rest.pos[1] - 10.0) < 1e-4 and d.rest.rot[3] == 32767 and d.rest.vel[1] == 1
            and d.rest.vel[2] == -3 and d.rest.vel[3] == 32767, "rest: 0.1 cm, smallest-three, i16 velocities")
        H.expect(d.rest.flags == (1 | 8 | (2 << 6)), "flags: WF_* + the dropped quaternion index")
        W2.put_held({ { peer = 3, nid = 77, hand = 1, actor = "Sword_1" } })
        local h = N.sc_get("world_held")
        H.expect(h and h.n == 1 and h.rows[1].hand == 1 and h.rows[1].actor == "Sword_1", "world_held bus record")
    "#);
}

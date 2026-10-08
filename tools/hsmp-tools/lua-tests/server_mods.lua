-- HSMPMenu's server-mods consent (mods/HSMPMenu/Scripts/server_mods.lua):
--
--     hsmp-tools lua-test server_mods
--
--   * nothing is accepted by default: an offer waits for the player;
--   * "never" declines at once; a remembered (server key, set hash) accepts at once, any
--     other set or server asks again; FORGET empties the store.

local T = T
local SM = dofile(T.path("mods/HSMPMenu/Scripts/server_mods.lua"))
local J = dofile(T.path("mods/HSMPMenu/Scripts/jsonlite.lua"))
local dir = T.tmpdir("hsmp_smods_")

local function raw(c) local t = {}; for i = 1, 32 do t[i] = c end; return t end
local queues, sent = { mod_offer = {}, mod_entry = {}, mod_progress = {} }, {}
local ipc = {
    S = { ENUMS = { mod_op = { ACCEPT = 1, DECLINE = 2, RESEND = 3 } } },
    events = function(k) local q = queues[k] or {}; queues[k] = {}; return q end,
    send = function(kind, t) sent[#sent + 1] = { kind = kind, op = t.op, set = SM.hex(t.set_hash) }; return 1 end,
}
local policy, declined, opened = "ask", nil, nil
SM.attach({ json = J, state_dir = dir, ipc = function() return ipc end, policy = function() return policy end,
    decline = function(why) declined = why end })

local function offer(set_c, key_c)
    queues.mod_offer = { { data = { set_hash = raw(set_c), server_key = raw(key_c), n = 1, total_bytes = 2048, cached_bytes = 0 } } }
    queues.mod_entry = { { data = { set_hash = raw(set_c), index = 0, n = 1, name = "Arena", version = "1.0", author = "Ann", bytes = 2048 } } }
    queues.mod_progress = { { data = { set_hash = raw(set_c), state = 1 } } }
    SM.reset()
    sent, opened = {}, nil
    SM.tick(nil, function(k) opened = k end)
end

offer(1, 7)
T.check(#sent == 0 and opened == "mods", "an offer waits for the player (nothing sent, the warning opens)")
T.check(SM.cur and SM.cur.mods[1].name == "Arena" and SM.cur.remember == true, "the mod list is shown; remember is on by default")
SM.accept()
T.check(#sent == 1 and sent[1].op == 1 and sent[1].set == SM.hex(raw(1)), "ACCEPT names exactly the offered set")
T.check(SM.remembered(SM.hex(raw(7)), SM.hex(raw(1))), "remembered for (server key, set hash)")

offer(1, 7)
T.check(#sent == 1 and sent[1].op == 1 and SM.cur.auto == "remembered", "the same server and set: accepted at once")
offer(2, 7)
T.check(#sent == 0, "another mod set from the same server asks again")
offer(1, 8)
T.check(#sent == 0, "the same set from another server asks again")
SM.decline()
T.check(sent[1] and sent[1].op == 2 and declined ~= nil, "DECLINE answers and leaves")

policy = "never"
offer(1, 7)
T.check(#sent == 1 and sent[1].op == 2 and SM.cur.state == "failed", "NEVER declines at once, even a remembered set")
policy = "ask"
T.check(SM.count() == 1 and SM.forget_all() and SM.count() == 0, "FORGET empties the store")
offer(1, 7)
T.check(#sent == 0, "after FORGET it asks again")
T.write(dir .. "/" .. SM.FILE, "garbage{")
T.check(SM.count() == 0 and not SM.remembered(SM.hex(raw(7)), SM.hex(raw(1))), "a broken file remembers nothing")

offer(1,7)
SM.cur.remember=false
SM.accept()
local before=#sent
queues.mod_offer={{data={set_hash=raw(1),server_key=raw(7),n=1,total_bytes=2048}}}
queues.mod_progress={{data={set_hash=raw(1),state=1}}}
SM.tick("mods",function(k)opened=k end)
T.check(SM.cur.decided=="accept" and #sent==before,"same-identity resume preserves explicit consent without duplicate send")
queues.mod_offer={{data={set_hash=raw(1),server_key=raw(8),n=1,total_bytes=2048}}}
queues.mod_entry={{data={set_hash=raw(1),index=0,name="Arena",bytes=2048}}}
queues.mod_progress={{data={set_hash=raw(1),state=1}}}
SM.tick("mods",function(k)opened=k end)
T.check(SM.cur.key==SM.hex(raw(8)) and SM.cur.decided==nil and #sent==before,"different server identity with same bytes needs fresh user decision without a manual menu reset")
SM.accept()
T.check(#sent==before+1 and sent[#sent].op==1,"new identity warning remains actionable")

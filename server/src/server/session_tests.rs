//! Session unit + property tests (session.rs): snapshot building, command dedup
//! and results, phase transitions, seats by key, nick dedup, the empty-server
//! and solo-leave rules. Commands are `command` records (built from the test
//! shorthand `C` below); snapshots are `session` records (read through `tv`).

use super::*;
use crate::server::match_core::round_tests::peer;
use crate::server::tick::advance;
use proptest::prelude::*;

/// Test shorthand for a `command` record (the old v5 command shapes).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum C {
    Ready(bool),
    Start { force: bool },
    Abort,
    PickArena(String),
    SetConfig(Patch),
    Kick { peer_id: PeerId, reason: String },
    Promote { peer_id: PeerId, admin_role: u8 },
    SetTeam(u8),
}

/// Test shorthand for a config patch (`None` = unchanged).
#[derive(Debug, Clone, PartialEq, Default)]
struct Patch {
    best_of: Option<u8>,
    mode: Option<u8>,
    kit_rules: Option<(u8, u16)>,
}

impl C {
    /// The `command` record (cmd_id 0 = RCON / internal).
    fn rec(&self, cmd_id: u32, expected_rev: u32) -> rec::Command {
        let mut c = rec::Command { cmd_id, expected_rev, ..Default::default() };
        match self {
            C::Ready(v) => { c.op = cmd_op::READY; c.flag = Bool::from(*v); }
            C::Start { force } => { c.op = cmd_op::START; c.flag = Bool::from(*force); }
            C::Abort => c.op = cmd_op::ABORT,
            C::PickArena(a) => { c.op = cmd_op::PICK_ARENA; c.text = Str::new(a); }
            C::SetConfig(p) => {
                c.op = cmd_op::SET_CONFIG;
                if let Some(b) = p.best_of { c.patch.mask |= cfg::BEST_OF; c.patch.best_of = b; }
                if let Some(m) = p.mode { c.patch.mask |= cfg::MODE; c.patch.mode = m; }
                if let Some((m, b)) = p.kit_rules { c.patch.mask |= cfg::KIT_RULES; c.patch.kit_mode = m; c.patch.kit_budget = b; }
            }
            C::Kick { peer_id, reason } => { c.op = cmd_op::KICK; c.peer_id = *peer_id; c.text = Str::new(reason); }
            C::Promote { peer_id, admin_role } => { c.op = cmd_op::PROMOTE; c.peer_id = *peer_id; c.role = *admin_role; }
            C::SetTeam(t) => { c.op = cmd_op::SET_TEAM; c.role = *t; }
        }
        c
    }
}

/// A `cmd_result` record, for assertions.
#[derive(Debug, Clone, PartialEq)]
struct R {
    cmd_id: u32,
    ok: bool,
    reason_code: u16,
    reason_text: String,
    config_rev: u32,
}

fn r_of(r: &rec::CmdResult) -> R {
    R { cmd_id: r.cmd_id, ok: r.ok.get(), reason_code: r.reason_code, reason_text: r.reason_text.lossy().into_owned(), config_rev: r.config_rev }
}

/// `command_locked` with the test shorthand.
fn cl(i: &mut Inner, actor: Actor, id: Option<u32>, rev: u32, c: &C, fx: &mut CmdEffects) -> (R, bool) {
    let (r, fresh) = command_locked(i, actor, &c.rec(id.unwrap_or(0), rev), fx);
    (r_of(&r), fresh)
}

/// One roster row of a snapshot, for assertions.
#[derive(Debug, Clone, PartialEq)]
struct TRow {
    seat: u8,
    peer_id: PeerId,
    nick: String,
    role: u8,
    admin_role: u8,
    ready: bool,
    alive: bool,
    connected: bool,
    waiting: bool,
    wins: u32,
    loaded_round: u32,
    player_id: [u8; 8],
    spawn: Option<TSpawn>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TSpawn {
    spawn_id: u32,
    pos: [f32; 3],
    yaw: f32,
    protect_ms: u32,
    slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TResult {
    winner_seat: u8,
    reason: u8,
}

/// A `session` record, for assertions (the head's fields + typed rows).
#[derive(Debug, Clone)]
struct TView {
    raw: SessionSnap,
    epoch: u64,
    seq: u32,
    phase: u8,
    phase_deadline_ms: u64,
    match_id: u64,
    round: u32,
    config: rec::SessionConfig,
    frozen: Option<rec::SessionConfig>,
    last_result: TResult,
    roster: Vec<TRow>,
    /// Seats the load barrier waits on.
    waiting_on: Vec<u8>,
}

fn tv(s: &SessionSnap) -> TView {
    let h = &s.head;
    // The record as a client sees it: through the validator.
    let bytes = session_msg(s);
    let (_, v) = hsmp_ipc::wire::decode::<rec::SessionHead>(&bytes).expect("a valid session record");
    assert_eq!(v.rows.len(), s.rows.len());
    let roster: Vec<TRow> = s.rows.iter().map(|r| TRow {
        seat: r.seat, peer_id: r.peer_id, nick: r.nick.lossy().into_owned(), role: r.role, admin_role: r.admin_role,
        ready: r.ready.get(), alive: r.alive.get(), connected: r.connected.get(), waiting: r.waiting.get(),
        wins: r.wins, loaded_round: r.loaded_round, player_id: r.player_id,
        spawn: (r.spawn_id != 0).then_some(TSpawn { spawn_id: r.spawn_id, pos: r.spawn_pos, yaw: r.spawn_yaw,
            protect_ms: r.spawn_protect_ms, slot: r.spawn_slot }),
    }).collect();
    TView {
        raw: s.clone(), epoch: h.epoch, seq: h.seq, phase: h.phase, phase_deadline_ms: h.phase_deadline_ms,
        match_id: h.match_id, round: h.round, config: h.config,
        frozen: h.has_frozen.get().then_some(h.frozen),
        last_result: TResult { winner_seat: h.winner_seat, reason: h.result_reason },
        waiting_on: roster.iter().filter(|r| r.waiting).map(|r| r.seat).collect(),
        roster,
    }
}

/// An `admin_state` record: (admin peer, masked bans).
fn adm(msg: &[u8]) -> (PeerId, Vec<String>) {
    let (_, v) = hsmp_ipc::wire::decode::<rec::AdminStateHead>(msg).expect("admin_state");
    (v.head.admin_peer, v.rows.iter().map(|r| r.entry.lossy().into_owned()).collect())
}

/// Notices of `code` queued for everyone (their args).
fn queued_notices(i: &Inner, code: u16) -> Vec<Vec<String>> {
    i.out_msgs.iter().filter_map(|(_, m)| hsmp_ipc::wire::decode::<rec::Notice>(m).ok())
        .filter(|(_, v)| v.head.code == code)
        .map(|(_, v)| {
            let mut a: Vec<String> = v.head.args.iter().map(|s| s.lossy().into_owned()).collect();
            while a.last().is_some_and(|s| s.is_empty()) { a.pop(); }
            a
        }).collect()
}

fn addr(n: u16) -> SocketAddr {
    format!("127.0.0.1:{}", 20000 + n).parse().unwrap()
}

/// What dispatch.rs does on a JOIN (dedup, insert, admin from the policy,
/// mid-match joiners spectate, on_joined). Fixture: these tests run a
/// LISTEN server whose host is the first player the test joins (its key is
/// made the owner when no owner is set yet), unless `dedicated` is called.
fn join(i: &mut Inner, port: u16, nick: &str) -> SocketAddr {
    let a = addr(port);
    let nick = dedup_nick(i, a, nick);
    let id = i.next_peer_id;
    i.next_peer_id += 1;
    let mut p = peer(id, &nick);
    p.ready = false;
    p.alive = i.match_state == "lobby";
    if i.admins.owner.is_none() && !DEDICATED.with(|d| d.get()) { i.admins.owner = Some(peer_key(&p)); }
    i.peers.insert(a, p);
    refresh_admins(i);
    on_joined(i, a);
    a
}

thread_local! {
    /// The fixture `join` makes no owner (a dedicated server).
    static DEDICATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A dedicated-server state for this test (thread): `join` makes no owner.
fn dedicated() -> ServerState {
    DEDICATED.with(|d| d.set(true));
    ServerState::new(8)
}

/// What peer_leave does to the state (removal + admin recompute).
fn leave(i: &mut Inner, a: SocketAddr) {
    let Some(p) = i.peers.remove(&a) else { return };
    admin_left(i, &p);
}

fn cmd(i: &mut Inner, a: SocketAddr, id: u32, c: C) -> R {
    cl(i, Actor::Peer(a), Some(id), 0, &c, &mut CmdEffects::default()).0
}

/// Advance n ticks. Every connected client's transport keeps talking (keepalive
/// PINGs refresh `last_seen_tick` in dispatch), as on a real server.
fn ticks(i: &mut Inner, n: u32) {
    ticks_except(i, n, &[]);
}

/// Advance n ticks while the peers at `silent` send nothing (a blackout).
fn ticks_except(i: &mut Inner, n: u32, silent: &[SocketAddr]) {
    for _ in 0..n {
        let t = i.server_tick;
        for (a, p) in i.peers.iter_mut() {
            if !silent.contains(a) { p.last_seen_tick = t; }
        }
        advance(i);
    }
}

fn new_state() -> ServerState {
    DEDICATED.with(|d| d.set(false));
    ServerState::new(8)
}

fn seat(i: &Inner, a: SocketAddr) -> u8 {
    i.sess.seats[&peer_key(&i.peers[&a])]
}

// ---- root acceptance -----------------------------------------------------------

#[tokio::test]
async fn nan_root_is_refused_and_never_disables_the_speed_cap() {
    let st = Arc::new(new_state());
    let a = { let mut i = st.inner.lock().await; join(&mut i, 1, "A") };
    let body = hsmp_ipc::schema::pose::Root::default();
    assert!(accept_root(&st, a, [f32::NAN, 0.0, 0.0], body.clone()).await.is_none(), "NaN as the first packet");
    assert!(accept_root(&st, a, [f32::INFINITY, 0.0, 0.0], body.clone()).await.is_none());
    assert!(accept_root(&st, a, [100.0, 0.0, 0.0], body.clone()).await.is_some());
    // Alternate NaN -> teleport: the NaN is refused, the teleport is measured
    // from the last good position and refused too.
    assert!(accept_root(&st, a, [f32::NAN, 0.0, 0.0], body.clone()).await.is_none());
    assert!(accept_root(&st, a, [90_000.0, 0.0, 0.0], body.clone()).await.is_none());
    let i = st.inner.lock().await;
    assert_eq!(i.peers[&a].last_valid_pos, Some([100.0, 0.0, 0.0]));
}

// ---- host admin across a reconnect ---------------------------------------------

/// Listen server: the host (owner key) drops; nobody is promoted in its
/// place, and the host is admin again by its key whenever it returns (no
/// time window: the key is the proof).
#[test]
fn listen_host_regains_admin_by_key_and_nobody_else_inherits_it() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let host = join(&mut i, 1, "Host");
    let b = join(&mut i, 2, "B");
    let _c = join(&mut i, 3, "C");
    let host_key = peer_key(&i.peers[&host]);
    assert!(i.peers[&host].is_admin);
    assert_eq!(i.admins.role(&host_key), AdminRole::OWNER);
    assert!(!i.peers[&b].is_admin);
    // NAT rebind / timeout: the host drops. No one is promoted.
    assert!(admin_left_after(&mut i, host), "the owner leaving is reported");
    ticks(&mut i, 30);
    assert_eq!(i.admin_peer_id, 0);
    assert!(i.peers.values().all(|p| !p.is_admin), "no one inherits admin");
    // The host is back from a new address: admin again.
    let host2 = join(&mut i, 9, "Host");
    assert_eq!(peer_key(&i.peers[&host2]), host_key);
    assert_eq!(i.admin_peer_id, i.peers[&host2].id);
    assert!(i.peers[&host2].is_admin && !i.peers[&b].is_admin);
    // Also much later (the key, not a window, decides).
    leave(&mut i, host2);
    ticks(&mut i, 10 * 60 * 30);
    let host3 = join(&mut i, 10, "Host");
    assert!(i.peers[&host3].is_admin);
    // A stranger whose nick equals the host's does not get it (different key).
    let mut s = peer(500, "Host");
    s.player_key = [5; 32];
    i.peers.insert(addr(11), s);
    refresh_admins(&mut i);
    assert!(!i.peers[&addr(11)].is_admin);
}

fn admin_left_after(i: &mut Inner, a: SocketAddr) -> bool {
    let p = i.peers.remove(&a).unwrap();
    admin_left(i, &p)
}

/// A grant (Promote) adds an admin; the granting admin keeps admin and the
/// returning host is admin too.
#[test]
fn promote_grants_admin_without_revoking_anyone() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let host = join(&mut i, 1, "Host");
    let b = join(&mut i, 2, "B");
    let c = join(&mut i, 3, "C");
    let c_id = i.peers[&c].id;
    // Not an admin: refused.
    assert_eq!(cmd(&mut i, b, 1, C::Promote { peer_id: c_id, admin_role: AdminRole::ADMIN }).reason_code, CmdReason::NOT_ADMIN);
    assert!(cmd(&mut i, host, 2, C::Promote { peer_id: c_id, admin_role: AdminRole::ADMIN }).ok);
    assert!(i.peers[&host].is_admin && i.peers[&c].is_admin && !i.peers[&b].is_admin);
    leave(&mut i, host);
    assert_eq!(i.admin_peer_id, c_id, "the granted admin is still admin");
    let back = join(&mut i, 9, "Host");
    assert!(i.peers[&back].is_admin && i.peers[&c].is_admin);
    let s = tv(&build_session(&i, 0));
    let role = |id: PeerId| s.roster.iter().find(|r| r.peer_id == id).unwrap().admin_role;
    assert_eq!(role(i.peers[&back].id), AdminRole::OWNER);
    assert_eq!(role(c_id), AdminRole::ADMIN);
    assert_eq!(role(i.peers[&b].id), AdminRole::NONE);
}

// ---- admin assignment on a dedicated server ------------------------------------

#[test]
fn dedicated_server_first_joiner_is_not_admin() {
    let st = dedicated();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Stranger");
    let b = join(&mut i, 2, "Other");
    assert!(!i.peers[&a].is_admin && !i.peers[&b].is_admin);
    assert_eq!(i.admin_peer_id, 0);
    for (n, c) in [C::PickArena("Pit".into()), C::Start { force: true }, C::Kick { peer_id: i.peers[&b].id, reason: String::new() },
                   C::Promote { peer_id: i.peers[&a].id, admin_role: AdminRole::ADMIN }].into_iter().enumerate() {
        assert_eq!(cmd(&mut i, a, 10 + n as u32, c).reason_code, CmdReason::NOT_ADMIN);
    }
    // The admin state a stranger gets names no admin and carries no bans.
    i.banned_ips.insert("203.0.113.7".parse().unwrap());
    let (admin_peer_id, banned_ips) = adm(&admin_state_for(&i, a));
    assert_eq!(admin_peer_id, 0);
    assert!(banned_ips.is_empty());
}

#[test]
fn configured_admin_key_is_admin_and_strangers_are_not() {
    let st = dedicated();
    let mut i = st.inner.try_lock().unwrap();
    let s1 = join(&mut i, 1, "Stranger");
    // The admin's key is configured (--admin-key / --admins-file).
    let (keys, bad) = parse_admins_text(&format!("# admins\n{}  # alice\n\nnot-a-key\n", hex::encode(provisional_key("Alice"))));
    assert_eq!(bad, vec!["not-a-key".to_string()]);
    i.admins.grant(*keys.iter().next().unwrap());
    let alice = join(&mut i, 2, "Alice");
    assert!(i.peers[&alice].is_admin && !i.peers[&s1].is_admin);
    assert_eq!(i.admin_peer_id, i.peers[&alice].id);
    assert!(cmd(&mut i, alice, 1, C::PickArena("Pit".into())).ok);
    // The admin's own S2CAdminState names itself; bans masked.
    i.banned_ips.insert("203.0.113.7".parse().unwrap());
    let me = i.peers[&alice].id;
    {
        let (admin_peer_id, banned_ips) = adm(&admin_state_for(&i, alice));
            assert_eq!(admin_peer_id, me);
            assert_eq!(banned_ips.len(), 1);
            assert!(banned_ips[0].starts_with("203.0.x.x#") && !banned_ips[0].contains("113.7"), "{:?}", banned_ips);
            // ...and the masked entry unbans exactly that IP.
            assert_eq!(resolve_ban(&i, &banned_ips[0]), Some("203.0.113.7".parse().unwrap()));
    }
    let (admin_peer_id, banned_ips) = adm(&admin_state_for(&i, s1));
    assert_eq!(admin_peer_id, me);
    assert!(banned_ips.is_empty());
    // The admin leaves: nobody is promoted.
    leave(&mut i, alice);
    assert_eq!(i.admin_peer_id, 0);
    assert!(!i.peers[&s1].is_admin);
}

#[test]
fn admin_key_parsing_and_masking() {
    assert!(parse_key_hex(&"ab".repeat(32)).is_some());
    assert!(parse_key_hex(&"ab".repeat(31)).is_none());
    assert!(parse_key_hex(&"zz".repeat(32)).is_none());
    assert!(parse_key_hex(&"00".repeat(32)).is_none(), "the all-zero key is never an admin");
    let salt = [3u8; 16];
    let v6: IpAddr = "2001:db8::1".parse().unwrap();
    let m = mask_ip(&salt, &v6);
    assert!(m.starts_with("2001:db8:x#") && m.len() == "2001:db8:x#".len() + 8, "{m}");
    assert_ne!(ban_token(&salt, &v6), ban_token(&[4u8; 16], &v6), "tokens are salted per run");
    let p = AdminPolicy::default();
    assert_eq!(p.role(&[0u8; 32]), AdminRole::NONE);
}

/// No-admin liveness: with no admin a server still runs matches: all
/// ready (>= 2 players) arms a countdown, an unready cancels it, and the
/// match starts by itself.
#[test]
fn no_admin_lobby_auto_starts_when_everyone_is_ready() {
    let st = dedicated();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    // One ready player alone never auto-starts.
    cmd(&mut i, a, 1, C::Ready(true));
    ticks(&mut i, 10 * 30);
    assert_eq!(i.match_state, "lobby");
    assert!(i.sess.auto_start_at.is_none());
    let b = join(&mut i, 2, "B");
    ticks(&mut i, 3);
    assert!(i.sess.auto_start_at.is_none(), "B is not ready");
    cmd(&mut i, b, 1, C::Ready(true));
    ticks(&mut i, 2);
    assert!(i.sess.auto_start_at.is_some(), "everyone ready: armed");
    update_deadline(&mut i, 1_000);
    let s = tv(&build_session(&i, 1_000));
    assert_eq!(s.phase, Phase::LOBBY);
    assert!(s.phase_deadline_ms > 1_000, "the lobby shows when it starts");
    // B changes its mind: cancelled.
    cmd(&mut i, b, 2, C::Ready(false));
    ticks(&mut i, 1);
    assert!(i.sess.auto_start_at.is_none());
    cmd(&mut i, b, 3, C::Ready(true));
    ticks(&mut i, AUTO_START_S * 30 + 2);
    assert_ne!(i.match_state, "lobby", "started by itself");
    assert_eq!(i.participants.len(), 2);
    assert!(i.sess.match_id != 0);
}

/// With an admin present the admin starts; READY alone does not.
#[test]
fn an_admin_present_disables_the_auto_start() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let host = join(&mut i, 1, "Host");
    let b = join(&mut i, 2, "B");
    cmd(&mut i, host, 1, C::Ready(true));
    cmd(&mut i, b, 1, C::Ready(true));
    ticks(&mut i, AUTO_START_S * 30 + 30);
    assert_eq!(i.match_state, "lobby");
    // The host drops: the remaining ready... only one player: still lobby.
    leave(&mut i, host);
    ticks(&mut i, AUTO_START_S * 30 + 30);
    assert_eq!(i.match_state, "lobby");
    let c = join(&mut i, 3, "C");
    cmd(&mut i, c, 1, C::Ready(true));
    ticks(&mut i, AUTO_START_S * 30 + 2);
    assert_ne!(i.match_state, "lobby", "no admin left: the ready players start");
}

// ---- per-peer state is dropped on leave, and budgets ----------------------------

#[test]
fn forget_peer_drops_every_per_peer_table() {
    use crate::validate::{cheat, rate::Kind};
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    i.next_peer_id = 7_001; // ids unique to this test (the tables are global)
    let a = join(&mut i, 1, "A");
    let id = i.peers[&a].id;
    i.match_peers.entry(id).or_default().aware = true;
    i.sess.placed.insert(id, (1, 1));
    i.sess.load_errors.insert(id, (1, "x".into(), 1));
    cheat::bump(id, cheat::Kind::DamageClamped);
    assert!(cheat::get(id).damage_clamped > 0);
    // Chat is budgeted per peer.
    let sent = (0..20).filter(|_| super::dispatch::within_budget(&st, id, Kind::Chat, 1.0)).count();
    assert_eq!(sent, 5, "chat burst");
    let p = i.peers.remove(&a).unwrap();
    forget_peer_locked(&mut i, p.id);
    assert!(!i.match_peers.contains_key(&id));
    assert!(!i.sess.placed.contains_key(&id) && !i.sess.load_errors.contains_key(&id));
    assert_eq!(cheat::get(id).damage_clamped, 0);
    assert!(super::dispatch::within_budget(&st, id, Kind::Chat, 1.0), "budget forgotten");
}

// ---- identity ---------------------------------------------------------------

#[test]
fn nick_dedup() {
    assert_eq!(dedup_nick_among([], "Willie"), "Willie");
    assert_eq!(dedup_nick_among(["Willie"], "Willie"), "Willie (2)");
    assert_eq!(dedup_nick_among(["willie", "WILLIE (2)"], "Willie"), "Willie (3)");
    assert_eq!(dedup_nick_among([], "  "), "Willie");
    assert_eq!(dedup_nick_among([], "a\u{7}b\n"), "ab");
    let long = "x".repeat(40);
    let first = dedup_nick_among([], &long);
    assert_eq!(first.len(), 32);
    let second = dedup_nick_among([first.as_str()], &long);
    assert!(second.ends_with(" (2)") && second.len() <= 32, "{second}");
    // Multi-byte nicks are clipped on a char boundary.
    let n = dedup_nick_among([], &"é".repeat(20));
    assert!(n.len() <= 32 && n.chars().all(|c| c == 'é'));
}

#[test]
fn two_willies_get_distinct_nicks_keys_and_seats() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Willie");
    let b = join(&mut i, 2, "Willie");
    assert_eq!(i.peers[&a].nick, "Willie");
    assert_eq!(i.peers[&b].nick, "Willie (2)");
    assert_ne!(peer_key(&i.peers[&a]), peer_key(&i.peers[&b]));
    assert_eq!((seat(&i, a), seat(&i, b)), (1, 2));
    // The same address re-joining keeps its own nick (sidecar reconnect).
    assert_eq!(dedup_nick(&i, b, "Willie (2)"), "Willie (2)");
}

#[test]
fn seats_follow_the_handshake_player_key_not_the_nick() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Willie");
    let b = join(&mut i, 2, "Bob");
    i.peers.get_mut(&a).unwrap().player_key = [7; 32];
    i.peers.get_mut(&b).unwrap().player_key = [9; 32];
    reconcile_seats(&mut i);
    for p in i.peers.values_mut() { p.ready = true; }
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    ticks(&mut i, 91);
    let seat_b = seat(&i, b);
    assert_eq!(peer_key(&i.peers[&b]), [9; 32]);
    // B reconnects from a new address under another nick: same key, same seat.
    leave(&mut i, b);
    ticks(&mut i, 1);
    let b2 = addr(50);
    let mut p = peer(42, "Bobby");
    p.player_key = [9; 32];
    p.alive = false;
    i.peers.insert(b2, p);
    on_joined(&mut i, b2);
    assert_eq!(seat(&i, b2), seat_b);
    // A stranger reusing B's old nick gets no seat of B's.
    let c = join(&mut i, 3, "Bob");
    i.peers.get_mut(&c).unwrap().player_key = [11; 32];
    reconcile_seats(&mut i);
    assert_ne!(seat(&i, c), seat_b);
}

#[test]
fn lobby_seat_is_freed_and_reused() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let _b = join(&mut i, 2, "B");
    leave(&mut i, a);
    reconcile_seats(&mut i);
    let c = join(&mut i, 3, "C");
    assert_eq!(seat(&i, c), 1, "lowest free seat");
}

// ---- snapshot ---------------------------------------------------------------

#[test]
fn lobby_snapshot_is_built_from_server_state() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Alpha");
    let b = join(&mut i, 2, "Bravo");
    i.peers.get_mut(&b).unwrap().ready = true;
    let s = tv(&build_session(&i, 1000));
    assert_eq!(s.phase, Phase::LOBBY);
    assert_eq!(s.epoch, i.sess.epoch);
    assert!(s.epoch > 0 && s.epoch < (1 << 53));
    assert!(s.frozen.is_none());
    assert_eq!(s.match_id, 0);
    assert_eq!(s.phase_deadline_ms, 0);
    assert_eq!(s.config.arena, DEFAULT_ARENA, "'default' shows the arena START would lock");
    assert_eq!(s.roster.len(), 2);
    assert_eq!(s.roster[0].seat, 1);
    assert_eq!(s.roster[0].nick, "Alpha");
    assert_eq!(s.roster[0].admin_role, AdminRole::OWNER, "the fixture's listen host");
    assert_eq!(s.roster[1].admin_role, AdminRole::NONE);
    assert!(!s.roster[0].ready && s.roster[1].ready);
    assert!(s.roster.iter().all(|r| r.connected && r.role == Role::FIGHTER && r.spawn.is_none()));
    assert_eq!(s.roster[0].player_id, player_id(&peer_key(&i.peers[&a])));
    assert_eq!(s.last_result.winner_seat, 0xFF);
}

#[test]
fn match_snapshot_has_frozen_config_spawns_and_seat_ordered_plan() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    i.match_arena = "Map_Arena_Pit".into();
    for p in i.peers.values_mut() { p.ready = true; }
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    let s = tv(&build_session(&i, 5000));
    assert_eq!(s.phase, Phase::LOADING);
    assert_ne!(s.match_id, 0);
    let f = s.frozen.clone().expect("frozen in a match");
    assert_eq!(f.arena, "Map_Arena_Pit");
    assert_eq!(f, i.sess.frozen.clone().unwrap());
    // The spawn plan is ordered by seat: seat 1 gets creation index 0.
    let sa = s.roster.iter().find(|r| r.seat == seat(&i, a)).unwrap().spawn.unwrap();
    let sb = s.roster.iter().find(|r| r.seat == seat(&i, b)).unwrap().spawn.unwrap();
    assert_eq!(sa.spawn_id, 1 << 8);
    assert_eq!(sb.spawn_id, (1 << 8) | 1);
    assert_ne!(sa.pos, sb.pos);
    assert!(s.phase_deadline_ms > 5000, "loading has the barrier deadline");
}

#[test]
fn snapshot_cadence_and_seq() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let s1 = session_due(&mut i, 100).map(|s| tv(&s)).expect("first snapshot");
    assert_eq!(s1.seq, 1);
    assert!(session_due(&mut i, 150).map(|s| tv(&s)).is_none(), "nothing changed, lobby 1 Hz");
    i.peers.get_mut(&a).unwrap().ready = true;
    let s2 = session_due(&mut i, 160).map(|s| tv(&s)).expect("on change");
    assert_eq!(s2.seq, 2);
    assert!(session_due(&mut i, 900).map(|s| tv(&s)).is_none());
    assert_eq!(session_due(&mut i, 1161).map(|s| tv(&s)).unwrap().seq, 3, "1 Hz in the lobby");
    // In a match: 3 Hz.
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    let s4 = session_due(&mut i, 1170).map(|s| tv(&s)).unwrap();
    assert_eq!(s4.phase, Phase::LOADING);
    ticks(&mut i, 1);
    let s5 = session_due(&mut i, 1200).map(|s| tv(&s)).unwrap();
    assert_eq!(s5.phase, Phase::COUNTDOWN);
    assert!(session_due(&mut i, 1400).map(|s| tv(&s)).is_none());
    assert!(session_due(&mut i, 1534).map(|s| tv(&s)).is_some(), "333 ms in a match");
}

#[test]
fn disconnected_participant_keeps_its_seat_and_wins() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    for p in i.peers.values_mut() { p.ready = true; }
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    ticks(&mut i, 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    let pid_a = i.peers[&a].id;
    let pid_b = i.peers[&b].id;
    assert!(declare_death(&mut i, pid_b, pid_a, DEATH_REPORTED));
    ticks(&mut i, 12);
    assert_eq!(i.peers[&a].wins, 1);
    let seat_a = seat(&i, a);
    // A drops: the duel pauses, the roster keeps the seat (connected=false).
    leave(&mut i, a);
    ticks(&mut i, 1);
    let s = tv(&build_session(&i, 0));
    let ra = s.roster.iter().find(|r| r.seat == seat_a).unwrap();
    assert!(!ra.connected);
    assert_eq!(ra.wins, 1);
    assert_eq!(ra.nick, "A");
    // A comes back from another address: same seat, same wins.
    let a2 = join(&mut i, 9, "A");
    assert_eq!(seat(&i, a2), seat_a);
    assert_eq!(i.peers[&a2].wins, 1);
    assert_ne!(i.peers[&a2].id, pid_a, "new peer id, same seat");
}

// ---- commands ---------------------------------------------------------------

#[test]
fn command_results() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Host");
    let b = join(&mut i, 2, "Guest");
    let r = cmd(&mut i, b, 1, C::Start { force: false });
    assert!(!r.ok);
    assert_eq!(r.reason_code, CmdReason::NOT_ADMIN);
    let r = cmd(&mut i, b, 2, C::PickArena("Pit".into()));
    assert_eq!(r.reason_code, CmdReason::NOT_ADMIN);
    let r = cmd(&mut i, a, 3, C::Start { force: false });
    assert_eq!(r.reason_code, CmdReason::NOT_ALL_READY);
    assert_eq!(r.reason_text, "start blocked: 0 of 2 peers ready");
    assert_eq!(phase_code(&i), Phase::LOBBY, "no instance travels");
    let r = cmd(&mut i, a, 4, C::PickArena("nowhere".into()));
    assert_eq!(r.reason_code, CmdReason::UNKNOWN_ARENA);
    for (name, want) in [("Pit", "Map_Arena_Pit"), ("map_arena_yard", "Map_Arena_Yard"),
                         ("/Game/Maps/Arenas/Map_Arena_Cellar.Map_Arena_Cellar", "Map_Arena_Cellar"),
                         ("EastTower", "Map_Arena_EastTower")] {
        let r = cl(&mut i, Actor::Rcon, None, 0, &C::PickArena(name.into()), &mut CmdEffects::default()).0;
        assert!(r.ok, "{name}");
        assert_eq!(i.match_arena, want);
    }
    // Config edits: rev check, value checks, unsupported fields.
    let rev = i.sess.config_rev;
    let patch = |p: Patch| C::SetConfig(p);
    let r = cl(&mut i, Actor::Peer(a), Some(10), rev + 5,
        &patch(Patch { best_of: Some(5), ..Default::default() }), &mut CmdEffects::default()).0;
    assert_eq!(r.reason_code, CmdReason::REV_MISMATCH);
    let r = cmd(&mut i, a, 11, patch(Patch { best_of: Some(0), ..Default::default() }));
    assert_eq!(r.reason_code, CmdReason::INVALID_VALUE);
    let r = cmd(&mut i, a, 12, patch(Patch { mode: Some(v5::Mode::FFA), ..Default::default() }));
    assert_eq!(r.reason_code, CmdReason::UNSUPPORTED);
    let before = i.sess.config_rev;
    let r = cmd(&mut i, a, 13, patch(Patch { best_of: Some(19), ..Default::default() }));
    assert!(r.ok);
    assert_eq!(i.best_of, 19);
    assert_eq!(r.config_rev, before + 1, "config_rev bumps on a change");
    let mut fx = CmdEffects::default();
    let r = cl(&mut i, Actor::Peer(a), Some(14), 0, &patch(Patch {
        kit_rules: Some((1, 100)), ..Default::default() }), &mut fx).0;
    assert!(r.ok);
    assert_eq!(fx.kit_rules, Some((1, 100)), "kit rules applied after the lock");
    // Unsupported commands are answered too.
    assert_eq!(cmd(&mut i, b, 15, C::SetTeam(1)).reason_code, CmdReason::UNSUPPORTED);
    // Ready, then START by the admin.
    assert!(cmd(&mut i, a, 16, C::Ready(true)).ok);
    assert!(cmd(&mut i, b, 17, C::Ready(true)).ok);
    let r = cmd(&mut i, a, 18, C::Start { force: false });
    assert!(r.ok, "{:?}", r);
    assert_eq!(phase_code(&i), Phase::LOADING);
    // Mid-match: config is frozen, START is the wrong phase, ABORT works.
    assert_eq!(cmd(&mut i, a, 19, C::PickArena("Pit".into())).reason_code, CmdReason::WRONG_PHASE);
    assert_eq!(cmd(&mut i, a, 20, patch(Patch { best_of: Some(3), ..Default::default() })).reason_code,
               CmdReason::WRONG_PHASE);
    assert_eq!(cmd(&mut i, a, 21, C::Start { force: false }).reason_code, CmdReason::WRONG_PHASE);
    assert!(cmd(&mut i, a, 22, C::Abort).ok);
    assert_eq!(phase_code(&i), Phase::LOBBY);
    assert!(i.sess.frozen.is_none() && i.sess.match_id == 0);
    assert_eq!(tv(&build_session(&i, 0)).last_result.reason, ResultReason::ABORTED);
    // Kick / promote targets must exist.
    assert_eq!(cmd(&mut i, a, 23, C::Kick { peer_id: 99, reason: String::new() }).reason_code, CmdReason::UNKNOWN_PLAYER);
    assert_eq!(cmd(&mut i, a, 24, C::Promote { peer_id: 99, admin_role: AdminRole::ADMIN }).reason_code,
               CmdReason::UNKNOWN_PLAYER);
    let mut fx = CmdEffects::default();
    let bid = i.peers[&b].id;
    assert!(cl(&mut i, Actor::Peer(a), Some(25), 0, &C::Kick { peer_id: bid, reason: String::new() }, &mut fx).0.ok);
    assert_eq!(fx.kicks.len(), 1);
    assert_eq!(fx.kicks[0].0, b);
}

#[test]
fn force_start_and_rcon_start_refusal() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let r = cl(&mut i, Actor::Rcon, None, 0, &C::Start { force: true }, &mut CmdEffects::default()).0;
    assert_eq!(r.reason_code, CmdReason::NOT_ENOUGH_PLAYERS);
    join(&mut i, 1, "A");
    join(&mut i, 2, "B");
    let r = cl(&mut i, Actor::Rcon, None, 0, &C::Start { force: false }, &mut CmdEffects::default()).0;
    assert!(!r.ok, "START with unready players is refused");
    assert!(r.reason_text.starts_with("start blocked"));
    let r = cl(&mut i, Actor::Rcon, None, 0, &C::Start { force: true }, &mut CmdEffects::default()).0;
    assert!(r.ok);
}

#[test]
fn duplicate_cmd_id_gets_the_cached_result_and_is_not_reapplied() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let r1 = cmd(&mut i, a, 7, C::Ready(true));
    assert!(r1.ok && i.peers[&a].ready);
    assert!(cmd(&mut i, a, 8, C::Ready(false)).ok);
    assert!(!i.peers[&a].ready);
    // A resend of #7 (lost result, reconnect) is answered from the cache.
    let (r1b, fresh) = cl(&mut i, Actor::Peer(a), Some(7), 0, &C::Ready(true), &mut CmdEffects::default());
    assert!(!fresh);
    assert_eq!(r1b, r1);
    assert!(!i.peers[&a].ready, "not applied twice");
    // The cache keeps the last 64 per player.
    for id in 100..(100 + CMD_CACHE as u32) { cmd(&mut i, a, id, C::Ready(id % 2 == 0)); }
    let (_, fresh) = cl(&mut i, Actor::Peer(a), Some(7), 0, &C::Ready(true), &mut CmdEffects::default());
    assert!(fresh, "#7 fell out of the 64-entry cache");
    // Dedup is per player: another player's #100 is its own command.
    let b = join(&mut i, 2, "B");
    let (_, fresh) = cl(&mut i, Actor::Peer(b), Some(100), 0, &C::Ready(true), &mut CmdEffects::default());
    assert!(fresh);
}

// ---- phases -----------------------------------------------------------------

#[test]
fn phase_transitions_through_a_match() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    for p in i.peers.values_mut() { p.ready = true; }
    i.best_of = 3;
    let mut seen = vec![phase_code(&i)];
    fn note(i: &Inner, seen: &mut Vec<u8>) {
        let p = phase_code(i);
        if *seen.last().unwrap() != p { seen.push(p); }
        assert_eq!(i.sess.last_phase, p, "observe_phase ran");
    }
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    note(&i, &mut seen);
    let (pa, pb) = (i.peers[&a].id, i.peers[&b].id);
    for round in 1..=2u32 {
        for _ in 0..400 {
            advance(&mut i);
            note(&i, &mut seen);
            if phase_code(&i) == Phase::LIVE { break; }
        }
        assert_eq!(i.match_round, round);
        assert!(declare_death(&mut i, pb, pa, DEATH_DAMAGE));
        note(&i, &mut seen);
        ticks(&mut i, 13);
        note(&i, &mut seen);
    }
    for _ in 0..400 { advance(&mut i); note(&i, &mut seen); }
    use Phase::*;
    assert_eq!(seen, vec![LOBBY, LOADING, COUNTDOWN, LIVE, ROUND_OVER, LOADING, COUNTDOWN, LIVE, ROUND_OVER,
                          MATCH_OVER, LOBBY]);
    assert_eq!(phase_label(MATCH_OVER), "MatchOver");
    assert_eq!(phase_label(ROUND_OVER), "RoundOver");
}

#[test]
fn solo_match_whose_player_leaves_returns_to_lobby() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Solo");
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok, "solo start needs no ready");
    ticks(&mut i, 1 + 90);
    assert_eq!(i.match_state, "live");
    leave(&mut i, a);
    ticks(&mut i, 1);
    assert_eq!(i.match_state, "lobby");
    assert_eq!(phase_code(&i), Phase::LOBBY);
    assert!(i.sess.frozen.is_none());
    assert!(i.sess.seats.is_empty());
}

#[test]
fn empty_server_returns_to_lobby_from_any_match_phase() {
    for steps in [0u32, 1, 40, 91, 95] {
        let st = new_state();
        let mut i = st.inner.try_lock().unwrap();
        let a = join(&mut i, 1, "A");
        let b = join(&mut i, 2, "B");
        for p in i.peers.values_mut() { p.ready = true; }
        assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
        ticks(&mut i, steps);
        leave(&mut i, a);
        leave(&mut i, b);
        ticks(&mut i, 1);
        assert_eq!(i.match_state, "lobby", "after {steps} ticks");
    }
}

#[test]
fn loaded_counts_only_on_the_frozen_arena() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    i.match_arena = "Map_Arena_Pit".into();
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    let id = i.peers[&a].id;
    // A game that reports the wrong world is not loaded: the barrier holds.
    game_status_in(&mut i, id, true, 1, Some("Map_Arena_Yard"), false);
    assert_eq!(i.match_peers[&id].loaded_round, 0);
    ticks(&mut i, 5);
    assert_eq!(phase_code(&i), Phase::LOADING);
    assert_eq!(tv(&build_session(&i, 0)).waiting_on, vec![1]);
    game_status_in(&mut i, id, true, 1, Some("/Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit"), false);
    assert_eq!(i.match_peers[&id].loaded_round, 1);
    ticks(&mut i, 1);
    assert_eq!(phase_code(&i), Phase::COUNTDOWN);
    // An old client without an arena field is trusted (headless tools).
    assert!(arena_matches(&i, None) && arena_matches(&i, Some("")));
}

#[test]
fn debug_kill_ends_the_round_by_seat() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    for p in i.peers.values_mut() { p.ready = true; }
    assert!(debug_kill(&mut i, 2).is_err(), "not live");
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    ticks(&mut i, 1 + 90);
    assert!(debug_kill(&mut i, 9).is_err());
    let sb = seat(&i, b);
    assert!(debug_kill(&mut i, sb).is_ok());
    assert!(debug_kill(&mut i, sb).is_err(), "already dead");
    observe_phase(&mut i);
    assert_eq!(phase_code(&i), Phase::ROUND_OVER);
    ticks(&mut i, 13);
    assert_eq!(i.peers[&a].wins, 1);
    assert_eq!(tv(&build_session(&i, 0)).last_result.winner_seat, seat(&i, a));
}

// ---- load failures and spawn protection (user bug: "guy insta died and now
// it's stuck waiting for Mate") ------------------------------------------------

/// A duel (or more) in Loading, every game client aware (pinging).
fn loading_match(nicks: &[&str]) -> (ServerState, Vec<SocketAddr>) {
    let st = new_state();
    let mut v = Vec::new();
    {
        let mut i = st.inner.try_lock().unwrap();
        for (k, n) in nicks.iter().enumerate() { v.push(join(&mut i, k as u16 + 1, n)); }
        for p in i.peers.values_mut() { p.ready = true; }
        i.match_arena = "Map_Arena_Pit".into();
        let a = v[0];
        assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
        for a in &v {
            let id = i.peers[a].id;
            game_status_in(&mut i, id, true, 0, None, false);
        }
    }
    (st, v)
}

fn load(i: &mut Inner, a: SocketAddr, round: u32) {
    let id = i.peers[&a].id;
    game_status_in(i, id, true, round, Some("Map_Arena_Pit"), false);
}

fn notices(i: &Inner) -> Vec<Vec<String>> {
    queued_notices(i, v5::Notice::LOAD_FAILED)
}

#[test]
fn load_error_gets_one_retry_window_then_the_round_is_void_not_stuck() {
    let (st, v) = loading_match(&["Willie", "Mate"]);
    let mut i = st.inner.try_lock().unwrap();
    let (a, m) = (v[0], v[1]);
    load(&mut i, a, 1);
    let mid = i.peers[&m].id;
    note_load_error(&mut i, mid, "spawn_timeout");
    ticks(&mut i, LOAD_RETRY_TICKS - 1);
    assert_eq!(phase_code(&i), Phase::LOADING, "inside the retry window: still waiting");
    note_load_error(&mut i, mid, "spawn_timeout"); // repeated pings do not restart the window
    ticks(&mut i, 1);
    assert_eq!(phase_code(&i), Phase::COUNTDOWN, "window over: the barrier releases");
    assert_eq!(i.sess.sat_out.len(), 1);
    let n = notices(&i);
    assert_eq!(n.len(), 1);
    assert_eq!(n[0][0], "Mate");
    assert_eq!(n[0][1], "spawn_timeout");
    // Only one fighter loaded in a duel: the round is void (no winner, no
    // loss), the match moves on to the next round instead of stalling.
    ticks(&mut i, 95);
    assert_eq!(i.match_state, "roundover");
    assert_eq!(i.match_reason, "load_failed");
    assert_eq!(i.last_winner, 0);
    assert!(i.peers.values().all(|p| p.wins == 0));
    assert!(i.round_deaths.is_empty(), "nobody died");
    assert_eq!(i.sess.void_streak, 1);
    // Next round: Mate loads this time and the duel is played.
    ticks(&mut i, ROUNDOVER_TICKS);
    assert_eq!(phase_code(&i), Phase::LOADING);
    load(&mut i, a, 2);
    load(&mut i, m, 2);
    ticks(&mut i, 92);
    assert_eq!(phase_code(&i), Phase::LIVE);
    assert_eq!(i.match_round, 2);
    assert!(i.peers.values().all(|p| p.alive));
    assert_eq!(i.sess.void_streak, 0);
}

#[test]
fn a_client_that_recovers_inside_the_window_plays() {
    let (st, v) = loading_match(&["A", "B"]);
    let mut i = st.inner.try_lock().unwrap();
    load(&mut i, v[0], 1);
    let bid = i.peers[&v[1]].id;
    note_load_error(&mut i, bid, "no_pawn");
    ticks(&mut i, 100);
    load(&mut i, v[1], 1);
    ticks(&mut i, 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    assert!(i.peers.values().all(|p| p.alive));
    assert!(i.sess.sat_out.is_empty() && notices(&i).is_empty());
}

#[test]
fn three_players_one_fails_the_other_two_fight_and_it_spectates() {
    let (st, v) = loading_match(&["A", "B", "C"]);
    let mut i = st.inner.try_lock().unwrap();
    load(&mut i, v[0], 1);
    load(&mut i, v[1], 1);
    let cid = i.peers[&v[2]].id;
    note_load_error(&mut i, cid, "wrong_world");
    ticks(&mut i, LOAD_RETRY_TICKS + 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    assert!(i.peers[&v[0]].alive && i.peers[&v[1]].alive);
    assert!(!i.peers[&v[2]].alive, "C spectates this round");
    assert!(i.round_deaths.is_empty(), "C is not dead and lost nothing");
    // The round is decided between A and B only.
    let (pa, pb) = (i.peers[&v[0]].id, i.peers[&v[1]].id);
    assert!(declare_death_unprotected(&mut i, pb, pa, DEATH_DAMAGE));
    ticks(&mut i, 13);
    assert_eq!(i.last_winner, pa);
}

#[test]
fn barrier_never_waits_more_than_45_s() {
    let (st, v) = loading_match(&["A", "Mate"]);
    let mut i = st.inner.try_lock().unwrap();
    load(&mut i, v[0], 1);
    // Mate pings but never loads and reports no error.
    ticks(&mut i, BARRIER_TIMEOUT_TICKS - 1);
    assert_eq!(phase_code(&i), Phase::LOADING);
    ticks(&mut i, 2);
    assert_eq!(phase_code(&i), Phase::COUNTDOWN, "45 s: released");
    assert_eq!(notices(&i)[0][1], "timeout");
    // A late error report cannot extend it either.
    let (st2, v2) = loading_match(&["A", "Mate"]);
    let mut j = st2.inner.try_lock().unwrap();
    load(&mut j, v2[0], 1);
    ticks(&mut j, BARRIER_TIMEOUT_TICKS - 5);
    let mid = j.peers[&v2[1]].id;
    note_load_error(&mut j, mid, "spawn_timeout");
    ticks(&mut j, 6);
    assert_eq!(phase_code(&j), Phase::COUNTDOWN, "the 10 s window is capped by the 45 s deadline");
}

#[test]
fn repeated_void_rounds_end_the_match() {
    let (st, v) = loading_match(&["A", "Mate"]);
    let mut i = st.inner.try_lock().unwrap();
    let mid = i.peers[&v[1]].id;
    for round in 1..=MAX_VOID_ROUNDS {
        load(&mut i, v[0], round);
        note_load_error(&mut i, mid, "spawn_timeout");
        ticks(&mut i, LOAD_RETRY_TICKS + 1 + 90);
        if round < MAX_VOID_ROUNDS {
            assert_eq!(i.match_reason, "load_failed", "round {round}");
            ticks(&mut i, ROUNDOVER_TICKS);
        }
    }
    assert_eq!(i.match_state, "lobby", "no endless loop of void rounds");
}

/// Spawn protection covers placement → Live only (the client ends
/// it at Live too). During the countdown nothing can kill a placed player;
/// from Live on there is no protected attacker or victim.
#[test]
fn spawn_protection_covers_placement_to_live_only() {
    let (st, v) = loading_match(&["A", "Mate"]);
    let mut i = st.inner.try_lock().unwrap();
    let (pa, pm) = (i.peers[&v[0]].id, i.peers[&v[1]].id);
    assert!(i.spawn_plan.iter().all(|s| s.protect_ms >= SPAWN_PROTECT_MS), "orders carry the protection");
    load(&mut i, v[0], 1);
    load(&mut i, v[1], 1);
    note_placed(&mut i, pm, 1); // Mate placed on its order during loading
    ticks(&mut i, 1);
    assert_eq!(phase_code(&i), Phase::COUNTDOWN);
    // Countdown: Mate is protected, A (no placement report) is not, and no
    // death or damage counts for anyone (no open round).
    assert!(spawn_protected(&i, pm) && !spawn_protected(&i, pa));
    assert!(!declare_death(&mut i, pm, pa, DEATH_DAMAGE));
    assert!(!declare_death(&mut i, pa, pm, DEATH_DAMAGE));
    ticks(&mut i, 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    // Live: protection is over for both, at once. A kill right after Live counts.
    assert!(!spawn_protected(&i, pm) && !spawn_protected(&i, pa));
    assert!(declare_death(&mut i, pm, pa, DEATH_DAMAGE));
    assert!(!i.peers[&v[1]].alive);
    // DEBUG KILL (admin) is never blocked: next round, inside the window.
    ticks(&mut i, 13 + ROUNDOVER_TICKS);
    load(&mut i, v[0], 2);
    load(&mut i, v[1], 2);
    note_placed(&mut i, pm, 2);
    ticks(&mut i, 92);
    assert_eq!(phase_code(&i), Phase::LIVE);
    let seat_m = seat(&i, v[1]);
    assert!(debug_kill(&mut i, seat_m).is_ok());
}

/// The legacy C2SKitRules path cannot change the kit rules mid-match (they
/// are part of the frozen config; SetConfig refuses them outside the lobby).
#[tokio::test]
async fn legacy_kit_rules_are_frozen_during_a_match() {
    let st = Arc::new(new_state());
    let sock = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let a = {
        let mut i = st.inner.lock().await;
        let a = join(&mut i, 1, "Host");
        i.match_state = "live".into();
        a
    };
    let before = crate::loadout::session_view().await;
    let other = if before.mode == 2 { 1 } else { 2 };
    crate::loadout::on_kit_rules(&sock, &st, a, other, 77).await;
    assert_eq!(crate::loadout::session_view().await.mode, before.mode);
}

/// S2CPings: every connected peer with its seat and rounded srtt (0 = no
/// sample), and the lag-comp samples only for peers that have one.
#[test]
fn pings_map_transport_rtt_to_peers() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    let (ia, ib) = (i.peers[&a].id, i.peers[&b].id);
    let (samples, p) = super::tick::build_pings(&i, &[(a, 47.6)], 9, 100);
    assert_eq!(samples, vec![(ia, 47.6)]);
    let (_, p) = hsmp_ipc::wire::decode::<rec::PingsHead>(&p).expect("pings record");
    assert_eq!((p.head.epoch, p.head.server_time_ms), (9, 100));
    assert_eq!(p.rows.to_vec(), vec![
        rec::PingRow { peer_id: ia, seat: 1, rtt_ms: 48, _r: 0 },
        rec::PingRow { peer_id: ib, seat: 2, rtt_ms: 0, _r: 0 },
    ]);
}

/// One public address cannot fill every seat; LAN / loopback
/// (local tests, LAN parties) are not limited.
#[test]
fn per_ip_admission_cap() {
    use super::dispatch::{per_ip_ok, MAX_PER_PUBLIC_IP};
    let public: std::net::IpAddr = "203.0.113.7".parse().unwrap();
    assert!((0..MAX_PER_PUBLIC_IP).all(|n| per_ip_ok(public, n, 16)));
    assert!(!per_ip_ok(public, MAX_PER_PUBLIC_IP, 16));
    for local in ["127.0.0.1", "192.168.1.20", "10.0.0.5", "::1", "fe80::1"] {
        assert!(per_ip_ok(local.parse().unwrap(), 15, 16), "{local}");
    }
}

/// An IPv6 host rotating addresses inside its /64 counts as
/// one host for the per-IP seat cap.
#[test]
fn per_ip_cap_counts_an_ipv6_slash_64_as_one_host() {
    use super::dispatch::{per_ip_ok, same_host_count, MAX_PER_PUBLIC_IP};
    let peers: Vec<SocketAddr> = (1..=MAX_PER_PUBLIC_IP as u16)
        .map(|h| format!("[2001:db8:5:5::{h:x}]:7000").parse().unwrap())
        .collect();
    let newcomer: SocketAddr = "[2001:db8:5:5:dead:beef:0:1]:7000".parse().unwrap();
    let n = same_host_count(peers.iter(), newcomer);
    assert_eq!(n, MAX_PER_PUBLIC_IP);
    assert!(!per_ip_ok(newcomer.ip(), n, 16), "fifth seat from one /64 refused");
    let other: SocketAddr = "[2001:db8:5:6::1]:7000".parse().unwrap();
    assert_eq!(same_host_count(peers.iter(), other), 0, "another /64 is another host");
    // IPv4 stays per exact address.
    let v4: Vec<SocketAddr> = vec!["203.0.113.7:1".parse().unwrap(), "203.0.113.8:1".parse().unwrap()];
    assert_eq!(same_host_count(v4.iter(), "203.0.113.7:2".parse().unwrap()), 1);
}

/// A placement report sent mid-round grants no protection.
#[test]
fn placement_reported_mid_round_grants_no_protection() {
    let (st, v) = loading_match(&["A", "Mate"]);
    let mut i = st.inner.try_lock().unwrap();
    let (pa, pm) = (i.peers[&v[0]].id, i.peers[&v[1]].id);
    load(&mut i, v[0], 1);
    load(&mut i, v[1], 1);
    ticks(&mut i, 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    ticks(&mut i, 60); // 2 s into the round
    note_placed(&mut i, pm, 1);
    assert!(!spawn_protected(&i, pm), "no protection on demand");
    assert!(declare_death(&mut i, pm, pa, DEATH_DAMAGE));
}

/// The match winner leaving during MATCH OVER does not make the
/// result name whoever is still connected.
#[test]
fn match_result_names_the_winner_even_after_it_left() {
    let (st, v) = loading_match(&["A", "B", "C"]);
    let mut i = st.inner.try_lock().unwrap();
    i.best_of = 1;
    let (pa, pb, pc) = (i.peers[&v[0]].id, i.peers[&v[1]].id, i.peers[&v[2]].id);
    for a in &v { load(&mut i, *a, 1); }
    ticks(&mut i, 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    ticks(&mut i, 120); // past spawn protection
    assert!(declare_death(&mut i, pb, pa, DEATH_DAMAGE));
    assert!(declare_death(&mut i, pc, pa, DEATH_DAMAGE));
    ticks(&mut i, 20); // settle
    assert_eq!(i.match_state, "match_over");
    leave(&mut i, v[0]); // the winner leaves the result screen
    let mut result = None;
    for _ in 0..MATCH_OVER_TICKS + 5 {
        let t = i.server_tick;
        for p in i.peers.values_mut() { p.last_seen_tick = t; }
        if let Some(b) = advance(&mut i) { result = Some(b); }
    }
    match result {
        Some(r) => assert_eq!((r.winner_peer_id, r.winner_nick.as_str()), (pa, "A")),
        None => panic!("no match result"),
    }
}

/// The proptest seed (proptest-regressions/server/session_tests.txt), as a
/// plain test: a forced duel start, the opponent drops during loading, the
/// 30 s reconnect pause runs out (forfeit -> result screen), then the last
/// player leaves the result screen. One tick later the server is in the
/// lobby (match_step supervises the result screen as well).
#[test]
fn last_player_leaving_the_result_screen_returns_to_the_lobby() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let ann = join(&mut i, 1, "Ann");
    let willie = join(&mut i, 2, "Willie");
    assert!(cmd(&mut i, ann, 1, C::Start { force: true }).ok);
    leave(&mut i, willie);
    for _ in 0..900 { advance(&mut i); }
    assert_eq!(i.match_state, "match_over", "forfeit after the reconnect window");
    leave(&mut i, ann);
    advance(&mut i);
    assert_eq!(i.match_state, "lobby");
    assert_eq!(i.sess.last_phase, Phase::LOBBY, "the transition was observed");
    assert!(tv(&build_session(&i, 0)).frozen.is_none());
}

// ---- property test ----------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Join(u8),
    Leave(u8),
    Ready(u8, bool),
    Start(u8, bool),
    Abort(u8),
    Pick(u8, u8),
    BestOf(u8, u8),
    Kill(u8),
    Tick(u16),
    /// Re-send the most recent (player, cmd_id) of this player.
    Dup(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..5).prop_map(Op::Join),
        (0u8..5).prop_map(Op::Leave),
        (0u8..5, any::<bool>()).prop_map(|(a, b)| Op::Ready(a, b)),
        (0u8..5, any::<bool>()).prop_map(|(a, b)| Op::Start(a, b)),
        (0u8..5).prop_map(Op::Abort),
        (0u8..5, 0u8..9).prop_map(|(a, b)| Op::Pick(a, b)),
        (0u8..5, 0u8..40).prop_map(|(a, b)| Op::BestOf(a, b)),
        (1u8..6).prop_map(Op::Kill),
        (1u16..400).prop_map(Op::Tick),
        (0u8..5).prop_map(Op::Dup),
    ]
}

const PICKS: [&str; 9] = ["Pit", "Yard", "Map_Arena_Alley", "Slums", "Cellar", "LordsHall", "EastTower", "bogus", "default"];
const NICKS: [&str; 5] = ["Willie", "Willie", "Ann", "Bob", "Willie"];

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    #[test]
    fn session_invariants(ops in proptest::collection::vec(op(), 1..60)) {
        let st = new_state();
        let mut i = st.inner.try_lock().unwrap();
        let mut slots: [Option<SocketAddr>; 5] = [None; 5];
        let mut port = 0u16;
        let mut next_cmd = 1u32;
        let mut last_cmd: HashMap<SocketAddr, (u32, C, R)> = HashMap::new();
        let mut now = 0u64;
        let mut last_seq = 0u32;
        let mut round_in_match: Option<(u64, u32)> = None;
        let mut frozen_at_start: Option<rec::SessionConfig> = None;
        for o in ops {
            let mut issued: Option<(SocketAddr, u32, C)> = None;
            match o {
                Op::Join(k) => if slots[k as usize].is_none() {
                    port += 1;
                    slots[k as usize] = Some(join(&mut i, port, NICKS[k as usize]));
                },
                Op::Leave(k) => if let Some(a) = slots[k as usize].take() { leave(&mut i, a); },
                Op::Ready(k, v) => if let Some(a) = slots[k as usize] { issued = Some((a, next_cmd, C::Ready(v))); },
                Op::Start(k, f) => if let Some(a) = slots[k as usize] { issued = Some((a, next_cmd, C::Start { force: f })); },
                Op::Abort(k) => if let Some(a) = slots[k as usize] { issued = Some((a, next_cmd, C::Abort)); },
                Op::Pick(k, m) => if let Some(a) = slots[k as usize] {
                    issued = Some((a, next_cmd, C::PickArena(PICKS[m as usize].into())));
                },
                Op::BestOf(k, n) => if let Some(a) = slots[k as usize] {
                    issued = Some((a, next_cmd, C::SetConfig(Patch { best_of: Some(n), ..Default::default() })));
                },
                Op::Kill(s) => { let _ = debug_kill(&mut i, s); observe_phase(&mut i); }
                Op::Tick(n) => for _ in 0..n { advance(&mut i); },
                Op::Dup(k) => if let Some(a) = slots[k as usize] {
                    if let Some((id, c, r)) = last_cmd.get(&a).cloned() {
                        if i.peers.contains_key(&a) {
                            let before = tv(&build_session(&i, now));
                            let (r2, fresh) = cl(&mut i, Actor::Peer(a), Some(id), 0, &c, &mut CmdEffects::default());
                            prop_assert!(!fresh, "a duplicate is never re-applied");
                            prop_assert_eq!(&r2, &r, "a duplicate gets the same result");
                            prop_assert!(same_content(&before.raw, &build_session(&i, now)), "a duplicate changes nothing");
                        }
                    }
                },
            }
            if let Some((a, id, c)) = issued {
                next_cmd += 1;
                let (r, fresh) = cl(&mut i, Actor::Peer(a), Some(id), 0, &c, &mut CmdEffects::default());
                prop_assert!(fresh);
                prop_assert_eq!(r.cmd_id, id, "exactly one result, for this cmd_id");
                prop_assert_eq!(r.ok, r.reason_code == CmdReason::OK);
                last_cmd.insert(a, (id, c, r));
            }

            // ---- invariants ----
            let p = phase_code(&i);
            prop_assert!(p <= Phase::PAUSED);
            prop_assert_eq!(i.sess.last_phase, p, "every transition was observed");
            let s = tv(&build_session(&i, now));
            // Seats: unique, every connected peer seated.
            let seats: HashSet<u8> = s.roster.iter().map(|r| r.seat).collect();
            prop_assert_eq!(seats.len(), s.roster.len());
            prop_assert_eq!(s.roster.iter().filter(|r| r.connected).count(), i.peers.len());
            let nicks: HashSet<String> = i.peers.values().map(|p| p.nick.to_lowercase()).collect();
            prop_assert_eq!(nicks.len(), i.peers.len(), "nicks are unique");
            // Admin iff the listen host's key is connected;
            // nobody else ever holds it.
            let owner_here = i.peers.values().any(|p| Some(peer_key(p)) == i.admins.owner);
            prop_assert_eq!(i.admin_peer_id != 0, owner_here);
            for p in i.peers.values() {
                prop_assert_eq!(p.is_admin, Some(peer_key(p)) == i.admins.owner);
            }
            // Config: frozen while a match runs and never changed under it.
            if p == Phase::LOBBY {
                prop_assert!(s.frozen.is_none() && s.match_id == 0);
                round_in_match = None;
                frozen_at_start = None;
            } else {
                let f = s.frozen.clone().unwrap();
                prop_assert_eq!(f.arena.lossy().into_owned(), i.match_arena.clone());
                prop_assert!(f.arena.lossy().starts_with("Map_Arena_"));
                prop_assert_eq!(f.best_of, i.best_of);
                match &frozen_at_start {
                    Some(f0) => prop_assert_eq!(f0, &f, "frozen config unchanged within the match"),
                    None => frozen_at_start = Some(f),
                }
                // round never decreases within a match.
                if let Some((m, r)) = round_in_match {
                    if m == s.match_id { prop_assert!(s.round >= r); }
                }
                round_in_match = Some((s.match_id, s.round));
            }
            // Wins never exceed what the best-of needs.
            let needed = (i.best_of as u32 + 1) / 2;
            prop_assert!(i.peers.values().all(|p| p.wins <= needed));
            // Snapshot seq strictly increases.
            now += 40;
            if let Some(snap) = session_due(&mut i, now) {
                prop_assert!(snap.head.seq > last_seq);
                last_seq = snap.head.seq;
            }
            // An empty server is in the lobby after one tick.
            if i.peers.is_empty() {
                advance(&mut i);
                prop_assert_eq!(i.match_state.as_str(), "lobby");
            }
        }
    }
}

// ---- resume / seat retention ---------------------------------------------------

/// A live duel between aware game clients A and B where A already won round 1.
/// Returns (state, [a, b]) with round 2 live.
fn live_duel_round2() -> (ServerState, Vec<SocketAddr>) {
    let (st, v) = loading_match(&["A", "B"]);
    {
        let mut i = st.inner.try_lock().unwrap();
        load(&mut i, v[0], 1);
        load(&mut i, v[1], 1);
        ticks(&mut i, 1 + 90);
        assert_eq!(phase_code(&i), Phase::LIVE);
        let sb = seat(&i, v[1]);
        // past the spawn protection, then A wins round 1
        ticks(&mut i, SPAWN_PROTECT_MS as u32 * 30 / 1000 + 1);
        debug_kill(&mut i, sb).unwrap();
        ticks(&mut i, 13 + ROUNDOVER_TICKS);
        load(&mut i, v[0], 2);
        load(&mut i, v[1], 2);
        ticks(&mut i, 92);
        assert_eq!(phase_code(&i), Phase::LIVE);
        assert_eq!(i.match_round, 2);
        assert_eq!(i.peers[&v[0]].wins, 1);
    }
    (st, v)
}

#[test]
fn blackout_8s_pauses_the_duel_and_resumes_into_the_same_seat_and_wins() {
    let (st, v) = live_duel_round2();
    let mut i = st.inner.try_lock().unwrap();
    let (a, b) = (v[0], v[1]);
    let (seat_a, seat_b) = (seat(&i, a), seat(&i, b));
    let id_b = i.peers[&b].id;
    // A's network dies for 8 s: its connection stays open (transport idle
    // timeout 10 s), it just goes silent.
    ticks_except(&mut i, LINK_STALL_TICKS - 2, &[a]);
    assert_eq!(i.match_state, "live", "3 s of silence is tolerated");
    ticks_except(&mut i, 4, &[a]);
    assert_eq!(i.match_state, "paused", "then the duel pauses for A's return");
    assert_eq!(i.match_reason, "opponent_left");
    assert!(i.sess.stalled.contains(&i.peers[&a].id));
    let s = tv(&build_session(&i, 0));
    assert_eq!(s.phase, Phase::PAUSED);
    assert!(s.roster.iter().all(|r| r.connected), "the seat never left the roster");
    ticks_except(&mut i, 8 * 30 - LINK_STALL_TICKS - 2, &[a]);
    assert_eq!(i.match_state, "paused", "still waiting inside the 30 s window");
    // A is back on the same connection: the SAME round resumes (a
    // drop never buys a replay from full health).
    load(&mut i, a, 2);
    ticks(&mut i, 1);
    assert!(i.sess.stalled.is_empty());
    assert_eq!(phase_code(&i), Phase::LIVE, "participant back: the round resumes");
    assert_eq!(i.match_round, 2, "same round, not replayed");
    assert_eq!((seat(&i, a), seat(&i, b)), (seat_a, seat_b), "same seats");
    assert_eq!(i.peers[&a].wins, 1, "same wins");
    assert_eq!(i.peers[&b].id, id_b);
    assert!(i.peers.values().all(|p| p.alive));
    // A second drop by A in this match has no pause left: A loses the round.
    ticks_except(&mut i, LINK_STALL_TICKS + 2, &[a]);
    assert_eq!(i.match_state, "roundover", "no second pause");
    ticks(&mut i, 13);
    assert_eq!(i.peers[&b].wins, 1, "B wins the round A dropped out of");
}

#[test]
fn reconnect_with_a_new_handshake_inside_the_window_keeps_seat_and_wins() {
    let (st, v) = live_duel_round2();
    let mut i = st.inner.try_lock().unwrap();
    let (a, b) = (v[0], v[1]);
    let seat_a = seat(&i, a);
    let old_id = i.peers[&a].id;
    // A's transport timed out (or a new socket): the old peer is removed ...
    leave(&mut i, a);
    ticks(&mut i, 1);
    assert_eq!(i.match_state, "paused");
    let s = tv(&build_session(&i, 0));
    let ra = s.roster.iter().find(|r| r.seat == seat_a).unwrap();
    assert!(!ra.connected && ra.wins == 1, "seat kept, disconnected, wins shown");
    ticks(&mut i, 20 * 30);
    assert_eq!(i.match_state, "paused", "20 s later: still inside the window");
    // ... and the same player key comes back with a new peer id.
    let a2 = join(&mut i, 9, "A");
    assert_ne!(i.peers[&a2].id, old_id, "new peer id");
    assert_eq!(seat(&i, a2), seat_a, "same seat by key");
    assert_eq!(i.peers[&a2].wins, 1, "same wins by key");
    // Its old session (ledger, health) is gone, so it does not come back to
    // full health in the interrupted round: the round resumes
    // without it and B wins it; A fights again from the next round.
    assert!(!i.peers[&a2].alive);
    // (B's game kept pinging through the pause, as a real client does.)
    load(&mut i, b, 2);
    load(&mut i, a2, 2);
    ticks(&mut i, 1);
    assert_eq!(i.match_round, 2, "not replayed");
    ticks(&mut i, 13);
    assert_eq!(i.peers[&b].wins, 1, "B takes the interrupted round");
    assert_eq!(i.match_state, "roundover");
    ticks(&mut i, ROUNDOVER_TICKS);
    load(&mut i, a2, 3);
    load(&mut i, b, 3);
    ticks(&mut i, 92);
    assert_eq!(phase_code(&i), Phase::LIVE);
    assert!(i.peers[&a2].alive, "the returning player fights the next round");
}

#[test]
fn resume_window_expired_forfeits_to_the_player_who_stayed() {
    let (st, v) = live_duel_round2();
    let mut i = st.inner.try_lock().unwrap();
    let (a, b) = (v[0], v[1]);
    let id_b = i.peers[&b].id;
    leave(&mut i, a);
    ticks(&mut i, 1);
    assert_eq!(i.match_state, "paused");
    ticks(&mut i, 30 * 30 + 2);
    assert_eq!(i.match_state, "match_over");
    assert_eq!(i.match_reason, "forfeit");
    assert_eq!(i.last_winner, id_b);
}

#[test]
fn a_deliberate_leave_forfeits_at_once_and_says_left() {
    let (st, v) = live_duel_round2();
    let mut i = st.inner.try_lock().unwrap();
    let (a, b) = (v[0], v[1]);
    let (id_a, id_b) = (i.peers[&a].id, i.peers[&b].id);
    on_deliberate_leave(&mut i, a, v5::LeaveReason::USER);
    assert!(i.sess.leaving.contains(&id_a), "peer_leave words the notice 'left'");
    assert_eq!(i.match_state, "match_over", "no 30 s pause for a player who chose to leave");
    assert_eq!((i.match_reason.as_str(), i.last_winner), ("forfeit", id_b));
    leave(&mut i, a);
    ticks(&mut i, 1);
    assert_eq!(i.match_state, "match_over", "stays on the result screen");
    // A leave in the lobby changes nothing but the notice.
    let st2 = new_state();
    let mut j = st2.inner.try_lock().unwrap();
    let x = join(&mut j, 1, "X");
    let _y = join(&mut j, 2, "Y");
    on_deliberate_leave(&mut j, x, v5::LeaveReason::USER);
    assert_eq!(j.match_state, "lobby");
}

#[test]
fn listen_host_leaving_closes_the_server_only_in_listen_mode() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "Host");
    let b = join(&mut i, 2, "Guest");
    std::env::remove_var("HSMP_LISTEN_HOST");
    assert!(!host_closes_on_leave(&i, a), "a dedicated server keeps running when its admin leaves");
    std::env::set_var("HSMP_LISTEN_HOST", "1");
    assert!(host_closes_on_leave(&i, a), "listen host leaving -> Host closed the server");
    assert!(!host_closes_on_leave(&i, b), "a guest leaving never closes it");
    std::env::remove_var("HSMP_LISTEN_HOST");
}

#[test]
fn stalls_count_only_for_game_clients_in_a_live_round() {
    // Headless peers (no pings) are never judged by silence.
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let a = join(&mut i, 1, "A");
    let _b = join(&mut i, 2, "B");
    for p in i.peers.values_mut() { p.ready = true; }
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    ticks(&mut i, 1 + 90);
    assert_eq!(phase_code(&i), Phase::LIVE);
    ticks_except(&mut i, 10 * 30, &[a]);
    assert_eq!(i.match_state, "live", "a headless client is not paused for silence");
    // Game clients: silence while loading is the barrier's business.
    let (st2, v) = loading_match(&["A", "B"]);
    let mut j = st2.inner.try_lock().unwrap();
    load(&mut j, v[1], 1);
    ticks_except(&mut j, 10 * 30, &[v[0]]);
    assert_eq!(phase_code(&j), Phase::LOADING, "no pause while loading (a level load blocks the game)");
}

fn notice_args(i: &Inner, want: u16) -> Vec<Vec<String>> {
    queued_notices(i, want)
}

#[test]
fn join_notices_for_the_hud() {
    let st = new_state();
    let mut i = st.inner.try_lock().unwrap();
    let _a = join(&mut i, 1, "A");
    let b = join(&mut i, 2, "B");
    assert_eq!(notice_args(&i, v5::Notice::PLAYER_JOINED), vec![vec!["B".to_string(), "joined".to_string()]],
        "the first player gets no notice; B does");
    i.out_msgs.clear();
    // B's seat is kept while a match runs: B coming back says "rejoined".
    for p in i.peers.values_mut() { p.ready = true; }
    let a = i.peers.iter().find(|(_, p)| p.nick == "A").map(|(a, _)| *a).unwrap();
    assert!(cmd(&mut i, a, 1, C::Start { force: false }).ok);
    leave(&mut i, b);
    ticks(&mut i, 1);
    i.out_msgs.clear();
    let _b2 = join(&mut i, 3, "B");
    assert_eq!(notice_args(&i, v5::Notice::PLAYER_JOINED), vec![vec!["B".to_string(), "rejoined".to_string()]]);
}

/// Measurement (printed with --nocapture): bytes of one session snapshot record and the
/// encode cost of one broadcast (the record is framed once and copied per destination).
#[test]
fn snapshot_bytes_and_encode_cost() {
    use std::time::Instant;
    for n in [2usize, 8, 16] {
        let st = new_state();
        let mut i = st.inner.try_lock().unwrap();
        for k in 0..n { join(&mut i, k as u16 + 1, &format!("Willie ({k})")); }
        for p in i.peers.values_mut() { p.ready = true; }
        let host = *i.peers.iter().find(|(_, p)| p.is_admin).unwrap().0;
        assert!(cmd(&mut i, host, 1, C::Start { force: false }).ok);
        let snap = build_session(&i, 1000);
        let rec_msg = session_msg(&snap);
        let iters = 2000u32;
        let t = Instant::now();
        for _ in 0..iters {
            let m = session_msg(&snap);
            for _ in 0..n { std::hint::black_box(m.clone()); }
        }
        let rec_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
        println!("session snapshot, {n:>2} players: record {:>5} B (wire msg); one broadcast to {n} peers: {rec_us:.2} us",
            rec_msg.len());
        assert_eq!(rec_msg.len(), 8 + 184 + 96 * n);
    }
}

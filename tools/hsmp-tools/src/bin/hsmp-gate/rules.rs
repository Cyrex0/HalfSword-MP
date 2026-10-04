//! DoD evaluation -> report.json + junit.xml.
//! Verdict: fail if any check fails; else incomplete if evidence is missing (or the
//! run is a STOPGAP); else pass. Exit code 0 pass, 1 fail, 2 incomplete.

use crate::events;
use crate::g0;
use crate::lint;
use crate::scenario;
use crate::soak;
use crate::util::{self, bv, ev_name, f64v, inst, is_menu_world, norm_map, s, sv, wall, Ev};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const PASS: &str = "pass";
pub const FAIL: &str = "fail";
pub const INCOMPLETE: &str = "incomplete";

#[derive(Clone, Debug)]
pub struct Check {
    pub rule: String,
    pub status: &'static str,
    pub msg: String,
    pub round: Option<usize>,
    pub inst: Option<String>,
}

#[derive(Default)]
pub struct Checks {
    pub items: Vec<Check>,
}

impl Checks {
    fn add(&mut self, rule: &str, status: &'static str, msg: impl Into<String>, round: Option<usize>, inst: Option<&str>) {
        self.items.push(Check { rule: rule.into(), status, msg: msg.into(), round, inst: inst.map(String::from) });
    }
    pub fn rule_status(&self, rule: &str) -> &'static str {
        let st: Vec<&str> = self.items.iter().filter(|c| c.rule == rule).map(|c| c.status).collect();
        if st.is_empty() {
            INCOMPLETE
        } else if st.contains(&FAIL) {
            FAIL
        } else if st.contains(&INCOMPLETE) {
            INCOMPLETE
        } else {
            PASS
        }
    }
}

#[derive(Clone, Debug)]
pub struct Round {
    pub idx: usize,
    pub round: Option<i64>,
    pub match_id: Option<String>,
    pub arena: Option<String>,
    pub load_ms: Option<i64>,
    pub live_ms: i64,
    pub end_ms: Option<i64>,
    pub epoch: Option<i64>,
    pub aborted: bool,
    pub resumed: bool,
    pub win_lo: i64,
    pub win_hi: i64,
}

fn rl(r: &Round) -> String {
    format!("#{} {} r{}", r.idx, r.arena.clone().unwrap_or_else(|| "?".into()), r.round.map(|x| x.to_string()).unwrap_or("?".into()))
}

fn in_round(e: &Ev, r: &Round) -> bool {
    let w = wall(e);
    r.win_lo < w && w <= r.win_hi
}

fn is_client(e: &Ev) -> bool {
    matches!(util::src(e), "inst" | "ue4ss") && !matches!(inst(e), "server" | "harness")
}

fn client<'a>(evs: &'a [Ev], name: &str, i: Option<&str>) -> Vec<&'a Ev> {
    evs.iter().filter(|e| ev_name(e) == name && is_client(e) && i.map(|x| inst(e) == x).unwrap_or(true)).collect()
}

fn harness(evs: &[Ev]) -> impl Iterator<Item = &Ev> {
    evs.iter().filter(|e| util::src(e) == "harness")
}

fn marks<'a>(evs: &'a [Ev], name: &str) -> Vec<&'a Ev> {
    harness(evs).filter(|e| ev_name(e) == "mark" && s(e, "name") == Some(name)).collect()
}

fn server_phases(evs: &[Ev]) -> Vec<&Ev> {
    evs.iter().filter(|e| ev_name(e) == "phase" && inst(e) == "server").collect()
}

pub fn build_rounds(evs: &[Ev]) -> Vec<Round> {
    let mut rounds: Vec<Round> = vec![];
    let mut arena: Option<String> = None;
    let mut load_ms: Option<i64> = None;
    let mut pending_pick: Option<String> = None;
    let mut sel: Vec<&Ev> = evs
        .iter()
        .filter(|e| {
            (ev_name(e) == "phase" && inst(e) == "server")
                || (ev_name(e) == "arena_picked" && inst(e) == "server")
                || (util::src(e) == "harness" && e.contains_key("pick"))
        })
        .collect();
    sel.sort_by_key(|e| wall(e));
    for e in sel {
        if ev_name(e) == "arena_picked" || util::src(e) == "harness" {
            pending_pick = sv(e, "arena").or_else(|| sv(e, "pick"));
            continue;
        }
        let to = s(e, "to").unwrap_or("");
        let fa = sv(e, "frozen_arena").or_else(|| sv(e, "arena"));
        if fa.is_some() && matches!(to, "Loading" | "Countdown" | "Live") {
            arena = fa.clone();
        }
        match to {
            "Loading" => {
                load_ms = Some(wall(e));
                if fa.is_none() && pending_pick.is_some() && arena.is_none() {
                    arena = pending_pick.clone();
                }
            }
            "Live" => {
                if arena.is_none() {
                    arena = pending_pick.clone();
                }
                if let Some(cur) = rounds.last_mut() {
                    if cur.end_ms.is_none() && cur.round.is_some() && cur.round == util::i64v(e, "round") {
                        cur.resumed = true; // Paused -> Live of the same round
                        continue;
                    }
                }
                rounds.push(Round {
                    idx: rounds.len(),
                    round: util::i64v(e, "round"),
                    match_id: sv(e, "match_id"),
                    arena: arena.clone(),
                    load_ms,
                    live_ms: wall(e),
                    end_ms: None,
                    epoch: util::i64v(e, "epoch"),
                    aborted: false,
                    resumed: false,
                    win_lo: 0,
                    win_hi: 0,
                });
            }
            "RoundOver" => {
                if let Some(cur) = rounds.last_mut() {
                    if cur.end_ms.is_none() {
                        cur.end_ms = Some(wall(e));
                    }
                }
            }
            "Lobby" | "PostMatch" | "MatchOver" | "Menu" => {
                if let Some(cur) = rounds.last_mut() {
                    if cur.end_ms.is_none() {
                        cur.end_ms = Some(wall(e));
                        cur.aborted = true;
                    }
                }
                arena = None;
            }
            _ => {}
        }
    }
    let mut prev_end = -1;
    for r in rounds.iter_mut() {
        r.win_lo = prev_end;
        r.win_hi = r.end_ms.unwrap_or(i64::MAX / 4);
        prev_end = r.win_hi;
    }
    rounds
}

struct Ctx<'a> {
    evs: &'a [Ev],
    rounds: &'a [Round],
    cfg: &'a Value,
    sc: &'a Value,
    insts: Vec<String>,
    baseline: Option<Value>,
    fin: Option<Value>,
    g0: Option<Value>,
    repo: &'a Path,
    run: &'a Path,
    /// `hsmp-tools crash-triage` over the run window (soak::triage_report)
    triage: Option<Value>,
}

impl Ctx<'_> {
    fn soak(&self) -> soak::SoakCtx<'_> {
        soak::SoakCtx { run: self.run, evs: self.evs, sc: self.sc, fin: self.fin.as_ref(), triage: self.triage.as_ref(), insts: &self.insts }
    }
}

fn dod1(c: &mut Checks, x: &Ctx) {
    let want = x.sc["rounds"].as_u64().unwrap_or(1) as usize;
    if x.sc["stopgap"].as_bool().unwrap_or(false) {
        c.add("DoD-1", INCOMPLETE, format!("STOPGAP run: rounds are not driven (saw {} Live phases)", x.rounds.len()), None, None);
        return;
    }
    if x.rounds.is_empty() {
        c.add("DoD-1", INCOMPLETE, "no server phase events (Live/RoundOver) in server.jsonl/server.log", None, None);
        return;
    }
    let mut done: HashMap<String, usize> = HashMap::new();
    for r in x.rounds {
        if r.end_ms.is_some() && !r.aborted {
            *done.entry(norm_map(r.arena.as_deref())).or_default() += 1;
        }
    }
    for a in x.sc["arenas"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        let got = *done.get(&norm_map(Some(a))).unwrap_or(&0);
        c.add("DoD-1", if got >= want { PASS } else { FAIL }, format!("{a}: {got}/{want} Live->RoundOver rounds"), None, None);
    }
    // a client that failed to load, or a round voided for it, breaks "10 consecutive rounds"
    for name in ["load_failed", "round_void"] {
        let v: Vec<String> = x.evs.iter().filter(|e| ev_name(e) == name && util::src(e) != "harness")
            .map(|e| format!("round {} {}", sv(e, "round").unwrap_or("?".into()), sv(e, "nick").or_else(|| sv(e, "error")).unwrap_or_default()))
            .collect();
        if !v.is_empty() {
            c.add("DoD-1", FAIL, format!("{} {name} event(s): {}", v.len(), v.iter().take(5).cloned().collect::<Vec<_>>().join("; ")), None, None);
        }
    }
    let epochs: HashSet<i64> = x.rounds.iter().filter_map(|r| r.epoch).collect();
    if epochs.len() > 1 {
        c.add("DoD-1", FAIL, format!("server restarted during the session (epochs {:?})", epochs), None, None);
    }
    if let Some(f) = &x.fin {
        let restarted: Vec<&String> = f["restarted"].as_object().into_iter().flatten().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k).collect();
        if !restarted.is_empty() {
            c.add("DoD-1", FAIL, format!("process restarted during the session: {restarted:?}"), None, None);
        }
    }
}

fn dod2(c: &mut Checks, x: &Ctx) {
    match &x.fin {
        None => c.add("DoD-2", INCOMPLETE, "final.json missing (crash dir / liveness not captured)", None, None),
        Some(f) => {
            let crashes: Vec<String> = f["new_crashes"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
            c.add("DoD-2", if crashes.is_empty() { PASS } else { FAIL }, format!("new crash dirs: {}", if crashes.is_empty() { "none".into() } else { format!("{crashes:?}") }), None, None);
            // every new dump classified by hsmp-tools crash-triage over the run window
            match &x.triage {
                Some(t) => {
                    let new = soak::triage_new(t);
                    c.add("DoD-2", if new.is_empty() { PASS } else { FAIL },
                          format!("crash-triage {}: {} new dump(s){}", soak::triage_window(t), new.len(), if new.is_empty() { String::new() } else { format!(": {}", new.join("; ")) }), None, None);
                }
                None => c.add("DoD-2", INCOMPLETE, "crash_triage.json missing (hsmp-tools crash-triage not run for the run window)", None, None),
            }
            match f["alive_at_end"].as_object() {
                Some(a) if !a.is_empty() => {
                    let dead: Vec<&String> = a.iter().filter(|(_, v)| v.as_bool() != Some(true)).map(|(k, _)| k).collect();
                    c.add("DoD-2", if dead.is_empty() { PASS } else { FAIL }, format!("processes dead before teardown: {}", if dead.is_empty() { "none".into() } else { format!("{dead:?}") }), None, None);
                }
                _ => c.add("DoD-2", INCOMPLETE, "alive_at_end not recorded", None, None),
            }
        }
    }
    // Game-thread stalls (docs/development/testing.md "Stall probe contract"): per instance, the
    // frame_hb heartbeat proves the probe ran; without it the rule is incomplete, never
    // pass. A hitch > 2 s with travel=false fails, and so does a heartbeat gap or a
    // frame_hb max_ms > 2 s outside every travel window.
    for i in &x.insts {
        let (st, msg) = soak::stall_check(x.evs, i, 2000.0);
        c.add("DoD-2", st, msg, None, Some(i));
    }
}

fn dod3(c: &mut Checks, x: &Ctx) {
    let wr_all = client(x.evs, "world_ready", None);
    if x.rounds.is_empty() {
        c.add("DoD-3", INCOMPLETE, "no rounds (server phase evidence missing)", None, None);
        return;
    }
    if wr_all.is_empty() {
        c.add("DoD-3", INCOMPLETE, "no world_ready events from any instance (Director not instrumented?)", None, None);
    }
    for r in x.rounds {
        for i in &x.insts {
            if !wr_all.is_empty() {
                let wr: Vec<&&Ev> = wr_all.iter().filter(|e| inst(e) == i && in_round(e, r)).collect();
                if wr.is_empty() {
                    c.add("DoD-3", FAIL, format!("{}: no world_ready", rl(r)), Some(r.idx), Some(i));
                } else if r.arena.is_none() {
                    c.add("DoD-3", INCOMPLETE, format!("{}: server frozen arena unknown", rl(r)), Some(r.idx), Some(i));
                } else {
                    let exp = r.arena.as_deref();
                    let names: Vec<String> = wr.iter().map(|e| sv(e, "arena").unwrap_or_default()).collect();
                    let wrong = wr.iter().any(|e| norm_map(s(e, "arena")) != norm_map(exp));
                    c.add("DoD-3", if wrong { FAIL } else { PASS }, format!("{}: world_ready {names:?} vs frozen {}", rl(r), exp.unwrap()), Some(r.idx), Some(i));
                }
            }
            let lo = r.load_ms.unwrap_or(r.live_ms);
            let mut menu = vec![];
            for e in client(x.evs, "travel", Some(i)).into_iter().chain(client(x.evs, "world_ready", Some(i))) {
                let w = wall(e);
                let target = s(e, "to").or_else(|| s(e, "arena"));
                if lo <= w && w <= r.win_hi && is_menu_world(target) && !r.aborted && r.end_ms.map(|end| w <= end).unwrap_or(true) {
                    menu.push(format!("({}, {})", ev_name(e), target.unwrap_or("")));
                }
            }
            if !menu.is_empty() {
                c.add("DoD-3", FAIL, format!("{}: menu/tavern world during the match: {}", rl(r), menu.join(" ")), Some(r.idx), Some(i));
            }
        }
    }
    // native_travel_rewritten (director.lua on_native_open_level / on_soft_travel_post):
    // * soft=true: the Director re-issues the travel itself -> a `travel` within 10 s;
    // * otherwise the native OpenLevel was rewritten IN PLACE (no travel event): the
    //   instance must arrive in the rewritten destination: a world_ready for that arena
    //   within 30 s (a menu destination: a travel or a lobby_ready within 30 s).
    for e in client(x.evs, "native_travel_rewritten", None) {
        let i = inst(e);
        let to = s(e, "to");
        let near = |name: &str, max_ms: i64| client(x.evs, name, Some(i)).into_iter().find(|t| (0..=max_ms).contains(&(wall(t) - wall(e))));
        let (ok, how) = if bv(e, "soft") == Some(true) {
            (near("travel", 10_000).is_some(), "re-issued travel within 10 s")
        } else if is_menu_world(to) {
            (near("travel", 30_000).is_some() || near("lobby_ready", 30_000).is_some(), "menu reached within 30 s")
        } else {
            let wr = client(x.evs, "world_ready", Some(i)).into_iter()
                .find(|t| (0..=30_000).contains(&(wall(t) - wall(e))) && norm_map(s(t, "arena")) == norm_map(to));
            // a travel only counts when it goes to the rewritten arena
            let tr = client(x.evs, "travel", Some(i)).into_iter()
                .find(|t| (0..=10_000).contains(&(wall(t) - wall(e))) && norm_map(s(t, "to")) == norm_map(to));
            (wr.is_some() || tr.is_some(), "world_ready in (or a travel to) the rewritten arena")
        };
        c.add("DoD-3", if ok { PASS } else { FAIL },
              format!("native_travel_rewritten {}->{}{}: {} {how}", sv(e, "from").unwrap_or_default(), sv(e, "to").unwrap_or_default(),
                      if bv(e, "soft") == Some(true) { " (soft)" } else { " (in place)" }, if ok { "has" } else { "has NO" }),
              None, Some(i));
    }
}

fn dod4(c: &mut Checks, x: &Ctx) {
    // check_travel (with its allow-list), the same check G0 runs; the simple built-in
    // lint::travel_lint knows no allow-list and would flag legacy_travel.lua.
    if hsmp_tools::paths::has_mods(x.repo) {
        let (st, summary) = g0::check_travel(x.repo);
        c.add("DoD-4", if st == "pass" { PASS } else { FAIL }, format!("check_travel: {summary}"), None, None);
    }
    let starts = marks(x.evs, "start");
    let refused = marks(x.evs, "start_refused");
    if starts.is_empty() && refused.is_empty() {
        c.add("DoD-4", INCOMPLETE, "no START marks in harness.jsonl", None, None);
    }
    let picks: Vec<&Ev> = harness(x.evs).filter(|e| e.contains_key("pick")).collect();
    let wr_any = client(x.evs, "world_ready", None);
    for m in &starts {
        let exp = picks.iter().rfind(|p| wall(p) <= wall(m)).and_then(|p| sv(p, "pick")).or_else(|| sv(m, "arena"));
        let hi = starts.iter().chain(refused.iter()).map(|x| wall(x)).filter(|w| *w > wall(m)).min().unwrap_or(i64::MAX);
        let exp_s = exp.clone().unwrap_or_else(|| "?".into());
        if wr_any.is_empty() {
            c.add("DoD-4", INCOMPLETE, format!("START@{exp_s}: no world_ready events to verify"), None, None);
            continue;
        }
        for i in &x.insts {
            let first = client(x.evs, "world_ready", Some(i))
                .into_iter()
                .find(|e| wall(e) >= wall(m) - 1000 && wall(e) < hi && !is_menu_world(s(e, "arena")));
            match first {
                None => c.add("DoD-4", FAIL, format!("START@{exp_s}: instance never reached an arena"), None, Some(i)),
                Some(f) => {
                    let ok = norm_map(s(f, "arena")) == norm_map(exp.as_deref());
                    c.add("DoD-4", if ok { PASS } else { FAIL }, format!("START after picking {exp_s}: loaded {}", sv(f, "arena").unwrap_or_default()), None, Some(i));
                }
            }
        }
    }
    for m in &refused {
        let (lo, hi) = (wall(m) - 500, wall(m) + 25000);
        let moved = client(x.evs, "travel", None)
            .into_iter()
            .chain(client(x.evs, "world_ready", None))
            .filter(|e| lo <= wall(e) && wall(e) <= hi && !is_menu_world(s(e, "to").or_else(|| s(e, "arena"))))
            .count();
        let loads = server_phases(x.evs)
            .into_iter()
            .filter(|e| matches!(s(e, "to"), Some("Loading" | "Countdown" | "Live")) && wall(m) <= wall(e) && wall(e) <= hi)
            .count();
        c.add("DoD-4", if moved + loads > 0 { FAIL } else { PASS }, format!("START with an unready player: {moved} client travels, {loads} server loads"), None, None);
        if let Some(rc) = harness(x.evs).find(|e| ev_name(e) == "rcon" && sv(e, "cmd").map(|c| c.to_uppercase()) == Some("START".into()) && wall(e) >= wall(m) - 10) {
            let accepted = bv(rc, "accepted").unwrap_or_else(|| sv(rc, "reply").map(|r| r.starts_with("OK")).unwrap_or(false));
            c.add("DoD-4", if accepted { FAIL } else { PASS }, format!("server reply to START: {:?}", sv(rc, "reply").unwrap_or_default()), None, None);
        }
    }
}

/// kit.lua's kit_verified carries `ok` (its own comparison against the kit, incl. the
/// stability window). Without it the event cannot be judged.
fn kit_ok(e: &Ev) -> (Option<bool>, String) {
    match bv(e, "ok") {
        Some(ok) => (Some(ok), "ok flag".into()),
        None => (None, "no ok field".into()),
    }
}

fn dod5(c: &mut Checks, x: &Ctx) {
    let kv_all = client(x.evs, "kit_verified", None);
    if kv_all.is_empty() || x.rounds.is_empty() {
        c.add("DoD-5", INCOMPLETE, if kv_all.is_empty() { "no kit_verified events" } else { "no rounds" }, None, None);
        return;
    }
    let n = x.insts.len();
    // Puppet evidence (who="peer:<id>") has no emitter yet (only kit.lua who="self"):
    // with none anywhere in the run the puppet half is incomplete, not a false FAIL.
    let any_peer = kv_all.iter().any(|e| sv(e, "who").map(|w| w.starts_with("peer:")).unwrap_or(false));
    if !any_peer && n > 1 {
        c.add("DoD-5", INCOMPLETE, "no kit_verified{who=\"peer:<id>\"} anywhere in the run (puppet kit not instrumented: HSMPLoadout apply_remote)", None, None);
    }
    for r in x.rounds {
        for i in &x.insts {
            let kv: Vec<&&Ev> = kv_all.iter().filter(|e| inst(e) == i && in_round(e, r)).collect();
            let selfs = kv.iter().filter(|e| sv(e, "who").as_deref() == Some("self")).count();
            let peers: HashSet<String> = kv.iter().filter_map(|e| sv(e, "who")).filter(|w| w != "self").collect();
            if selfs == 0 {
                c.add("DoD-5", FAIL, format!("{}: no kit_verified for own pawn", rl(r)), Some(r.idx), Some(i));
            }
            if any_peer && peers.len() < n - 1 {
                c.add("DoD-5", FAIL, format!("{}: kit_verified for {}/{} puppets", rl(r), peers.len(), n - 1), Some(r.idx), Some(i));
            }
            for e in kv {
                let (ok, how) = kit_ok(e);
                let detail = format!("who={} armour={} R={} L={} ({how})", sv(e, "who").unwrap_or_default(), sv(e, "armour_n").unwrap_or_default(), sv(e, "r_class").unwrap_or("null".into()), sv(e, "l_class").unwrap_or("null".into()));
                match ok {
                    None => c.add("DoD-5", INCOMPLETE, format!("{}: kit_verified {detail}", rl(r)), Some(r.idx), Some(i)),
                    Some(g) => c.add("DoD-5", if g { PASS } else { FAIL }, format!("{}: {detail}", rl(r)), Some(r.idx), Some(i)),
                }
            }
        }
    }
}

fn dod6(c: &mut Checks, x: &Ctx) {
    let ce = client(x.evs, "willie_census", None);
    if ce.is_empty() || x.rounds.is_empty() {
        c.add("DoD-6", INCOMPLETE, if ce.is_empty() { "no willie_census events" } else { "no rounds" }, None, None);
        return;
    }
    // The Director samples at Ready (at="ready"); the every-5-s Live samples need a Live
    // emitter. With no non-Ready sample anywhere in the run that half is incomplete.
    let live_emitter = ce.iter().any(|e| s(e, "at") != Some("ready"));
    if !live_emitter {
        c.add("DoD-6", INCOMPLETE, "only at=\"ready\" willie_census samples in the run (no Live census emitter: every 5 s during Live is unchecked)", None, None);
    }
    for r in x.rounds {
        for i in &x.insts {
            let smp: Vec<&&Ev> = ce.iter().filter(|e| inst(e) == i && in_round(e, r)).collect();
            if smp.is_empty() {
                c.add("DoD-6", FAIL, format!("{}: no census sample", rl(r)), Some(r.idx), Some(i));
                continue;
            }
            // a sample without visible/expected cannot be judged (two missing fields
            // compare equal): incomplete, never a silent pass.
            let unjudged = smp.iter().filter(|e| sv(e, "visible").is_none() || sv(e, "expected").is_none()).count();
            let bad: Vec<String> = smp.iter().filter(|e| sv(e, "visible").is_some() && sv(e, "expected").is_some() && sv(e, "visible") != sv(e, "expected"))
                .map(|e| format!("({}, {})", sv(e, "visible").unwrap_or_default(), sv(e, "expected").unwrap_or_default())).take(5).collect();
            let st = if !bad.is_empty() { FAIL } else if unjudged > 0 { INCOMPLETE } else { PASS };
            c.add("DoD-6", st, format!("{}: {} samples, mismatches [{}]{}", rl(r), smp.len(), bad.join(", "),
                                        if unjudged > 0 { format!(", {unjudged} without visible/expected (not judged)") } else { String::new() }), Some(r.idx), Some(i));
            if !live_emitter {
                continue;
            }
            if let Some(end) = r.end_ms {
                let live_s = (end - r.live_ms) as f64 / 1000.0;
                let during = smp.iter().filter(|e| r.live_ms <= wall(e) && wall(e) <= end).count();
                if live_s > 12.0 && (during as i64) < (live_s / 5.0) as i64 - 1 {
                    c.add("DoD-6", FAIL, format!("{}: only {during} census samples in {live_s:.0} s of Live (want one per 5 s)", rl(r)), Some(r.idx), Some(i));
                }
            }
        }
    }
}

fn dod7(c: &mut Checks, x: &Ctx) {
    let sv_all = client(x.evs, "spawn_verified", None);
    if sv_all.is_empty() || x.rounds.is_empty() {
        c.add("DoD-7", INCOMPLETE, if sv_all.is_empty() { "no spawn_verified events" } else { "no rounds" }, None, None);
        return;
    }
    for r in x.rounds {
        for i in &x.insts {
            let Some(e) = sv_all.iter().rfind(|e| inst(e) == i && in_round(e, r)) else {
                c.add("DoD-7", FAIL, format!("{}: no spawn_verified", rl(r)), Some(r.idx), Some(i));
                continue;
            };
            // Fields of director.lua's spawn_verified (the real emitter): ok, vitals_ok, gi_ok,
            // x/y/z (pawn, cm), dist_cm (horizontal, pawn -> server spawn order), snap_z (the
            // slot's ground-snapped Z; not emitted yet). Every sub-check needs its field:
            // a missing one is incomplete, never a silent pass.
            let mut probs = vec![];
            let mut missing = vec![];
            match bv(e, "ok") {
                Some(false) => probs.push(format!("ok=false (load_error={})", sv(e, "load_error").unwrap_or("none".into()))),
                Some(true) => {}
                None => missing.push("ok"),
            }
            match bv(e, "vitals_ok") {
                Some(false) => probs.push("vitals_ok=false (health != Willie CDO)".to_string()),
                Some(true) => {}
                None => missing.push("vitals_ok"),
            }
            if bv(e, "gi_ok") == Some(false) {
                probs.push("gi_ok=false".to_string());
            }
            // Placement accuracy is judged against the placer's DESTINATION (dest_cm): the
            // server's order, or the clear spiral point next to it when the order was blocked
            // (docs/development/subsystems/spawns.md). The spiral offset itself (offset_cm) is bounded
            // by the outer ring. Older emitters without dest_cm are judged on dist_cm (to the
            // order). The placer's own acceptance radius (tol_cm) must equal this limit.
            match (f64v(e, "dest_cm"), f64v(e, "dist_cm")) {
                (Some(d), _) if d > SPAWN_DIST_MAX_CM => probs.push(format!("dest_cm={d:.0}>{SPAWN_DIST_MAX_CM:.0} from the placement destination (dist_cm={:.0} from the order)", f64v(e, "dist_cm").unwrap_or(-1.0))),
                (Some(_), _) => {}
                (None, Some(d)) if d > SPAWN_DIST_MAX_CM => probs.push(format!("dist_cm={d:.0}>{SPAWN_DIST_MAX_CM:.0} from the spawn order")),
                (None, Some(_)) => {}
                (None, None) => missing.push("dist_cm"),
            }
            if let Some(off) = f64v(e, "offset_cm") {
                if off > SPAWN_OFFSET_MAX_CM {
                    probs.push(format!("offset_cm={off:.0}>{SPAWN_OFFSET_MAX_CM:.0}: the destination is beyond the clearance spiral"));
                }
            }
            match f64v(e, "tol_cm") {
                Some(t) if (t - SPAWN_DIST_MAX_CM).abs() > 0.5 => probs.push(format!("placer tol_cm={t:.0} disagrees with the gate's {SPAWN_DIST_MAX_CM:.0}")),
                Some(_) => {}
                None if f64v(e, "dest_cm").is_some() => missing.push("tol_cm"),
                None => {}
            }
            match (f64v(e, "z"), f64v(e, "snap_z")) {
                (Some(z), Some(sz)) if z < sz - SPAWN_Z_TOL_CM => probs.push(format!("z={z:.0} < snap_z {sz:.0} - {SPAWN_Z_TOL_CM:.0}")),
                (Some(_), Some(_)) => {}
                (None, _) => missing.push("z"),
                (_, None) => missing.push("snap_z"),
            }
            // peer spacing: the other instances' own spawn_verified positions in this round
            match (f64v(e, "x"), f64v(e, "y")) {
                (Some(px), Some(py)) => {
                    let others: Vec<(String, f64)> = x.insts.iter().filter(|j| *j != i)
                        .filter_map(|j| sv_all.iter().rfind(|o| inst(o) == j && in_round(o, r)).map(|o| (j, o)))
                        .filter_map(|(j, o)| Some((j.clone(), ((f64v(o, "x")? - px).powi(2) + (f64v(o, "y")? - py).powi(2)).sqrt())))
                        .collect();
                    if others.is_empty() && x.insts.len() > 1 {
                        missing.push("peer x/y");
                    }
                    for (j, d) in others {
                        if d < SPAWN_PEER_MIN_CM {
                            probs.push(format!("{d:.0} cm from inst {j}'s pawn (< {SPAWN_PEER_MIN_CM:.0})"));
                        }
                    }
                }
                _ => missing.push("x/y"),
            }
            let (st, msg) = if !probs.is_empty() {
                (FAIL, probs.join("; "))
            } else if !missing.is_empty() {
                (INCOMPLETE, format!("spawn_verified lacks {} (not checked)", missing.join(", ")))
            } else {
                (PASS, format!("clean spawn ({:.0} cm from the order)", f64v(e, "dist_cm").unwrap_or(0.0)))
            };
            c.add("DoD-7", st, format!("{}: {msg}", rl(r)), Some(r.idx), Some(i));
        }
    }
}

/// DoD-7 limits: within 1 m of the placement destination (the server's
/// spawn order, or its clearance-spiral point when the order was blocked), Z at least the
/// slot's ground-snap Z - 50 cm, no two pawns within 1.5 m. HSMPSync's verify_tol_cm and the
/// Director's place_tol_cm are the same 100 cm (spawn_verified.tol_cm is checked against it).
const SPAWN_DIST_MAX_CM: f64 = 100.0;
/// The outer clearance-spiral ring (HSMPSync spawn_place.lua T.rings).
const SPAWN_OFFSET_MAX_CM: f64 = 226.0;
const SPAWN_Z_TOL_CM: f64 = 50.0;
const SPAWN_PEER_MIN_CM: f64 = 150.0;

/// Native save calls that write a slot (shared/hsmp_saveguard.lua FUNCTIONS, kind write/delete).
const SAVE_WRITE_FNS: [&str; 3] = ["SaveGameToSlot", "AsyncSaveGameToSlot", "DeleteGameInSlot"];
/// A career-file mtime is attributed to a logged native write within this distance.
const SAVE_ATTRIB_MS: i64 = 3000;

fn dod8(c: &mut Checks, x: &Ctx) {
    match (&x.baseline, &x.fin) {
        (Some(b), Some(f)) if f.get("save_hashes").is_some() => {
            let (b, a) = (b["save_hashes"].as_object().cloned().unwrap_or_default(), f["save_hashes"].as_object().cloned().unwrap_or_default());
            // HSMP_*.sav are the save guard's own slots, created on purpose: not career data
            let mut keys: Vec<&String> = b.keys().chain(a.keys()).filter(|k| !k.starts_with("HSMP_")).collect();
            keys.sort();
            keys.dedup();
            let changed: Vec<&&String> = keys.iter().filter(|k| b.get(**k) != a.get(**k)).collect();
            let n = b.keys().filter(|k| !k.starts_with("HSMP_")).count();
            // no career save at all proves nothing (wrong path, fresh profile)
            let st = if !changed.is_empty() { FAIL } else if n == 0 { INCOMPLETE } else { PASS };
            c.add("DoD-8", st,
                  format!("{n} career .sav files; changed/added/removed: {}{}", if changed.is_empty() { "none".into() } else { format!("{changed:?}") },
                          if n == 0 { " (the baseline has no career files: nothing was protected, nothing proven)" } else { "" }), None, None);
        }
        _ => c.add("DoD-8", INCOMPLETE, "baseline/final save hashes missing", None, None),
    }
    dod8_writes(c, x);
    dod8_career_guard(c, x);
    // hsmp_saveguard.lua save_redirected{fn, slot, to_slot, op, ok, how}: ok=false means the
    // rewrite failed and the native call hit the career slot; a safe block is
    // to_slot="<blocked>" with ok=true. Missing `ok` = an emitter the gate cannot judge.
    let sr = client(x.evs, "save_redirected", None);
    let mut bad: Vec<String> = vec![];
    let mut unknown = 0;
    for e in &sr {
        let to = sv(e, "to_slot").unwrap_or_default();
        let what = format!("{}({}) -> {to:?} op={}", sv(e, "fn").unwrap_or_default(), sv(e, "slot").unwrap_or_default(), sv(e, "op").unwrap_or("?".into()));
        match bv(e, "ok") {
            Some(false) => bad.push(format!("{what} ok=false (NOT diverted)")),
            Some(true) if !(to.starts_with("HSMP_") || to == "<blocked>") => bad.push(format!("{what}: not an HSMP_ slot")),
            Some(true) => {}
            None => unknown += 1,
        }
    }
    let st = if !bad.is_empty() { FAIL } else if unknown > 0 { INCOMPLETE } else { PASS };
    c.add("DoD-8", st, format!("save calls during session: {} redirected/blocked, {} failed{}{}", sr.len() - bad.len() - unknown, bad.len(),
        if unknown > 0 { format!(", {unknown} without an ok flag") } else { String::new() },
        if bad.is_empty() { String::new() } else { format!(": {}", bad.iter().take(5).cloned().collect::<Vec<_>>().join("; ")) }), None, None);
    // The guard must have been ACTIVE in each instance (x_save_guard{active=true} on every
    // session start): zero save_redirected from a guard that never armed proves nothing.
    for i in &x.insts {
        let armed = client(x.evs, "x_save_guard", Some(i)).iter().any(|e| bv(e, "active") == Some(true));
        if !armed {
            c.add("DoD-8", INCOMPLETE, "save guard never reported active (no x_save_guard{active=true})", None, Some(i));
        }
    }
}

/// A career file WRITTEN during the run, even with the same bytes (the hash cannot see it)
/// or restored afterwards. mp_test.ps1 records `save_files{name: {size, mtime_ms, sha256}}` in
/// baseline.json and final.json. Any size/mtime change of a non-HSMP_ file fails, unless an
/// instance logged the native write itself while its own guard was off (x_save_call{fn=a write,
/// slot, active=false} within 3 s of the new mtime, and no x_save_guard{active=true} of that
/// instance before it): that is vanilla pre-session behaviour (the main menu saves Settings on
/// load), reported as a note.
fn dod8_writes(c: &mut Checks, x: &Ctx) {
    let files = |v: &Option<Value>| v.as_ref().and_then(|v| v["save_files"].as_object().cloned());
    let (Some(b), Some(a)) = (files(&x.baseline), files(&x.fin)) else {
        c.add("DoD-8", INCOMPLETE, "baseline/final have no save_files (size + mtime) record: a career write of the same bytes is invisible", None, None);
        return;
    };
    let lo = x.baseline.as_ref().and_then(|v| v["wall_ms"].as_i64()).unwrap_or(0);
    let calls = client(x.evs, "x_save_call", None);
    let guards = client(x.evs, "x_save_guard", None);
    // the exemption holds only BEFORE that instance's FIRST arm. A write while the guard was
    // off again later (session end, a mid-session sidecar drop) is not vanilla pre-session
    // behaviour: it is a career write the guard let through.
    let first_arm = |i: &str| guards.iter().filter(|g| inst(g) == i && bv(g, "active") == Some(true)).map(|g| wall(g)).min();
    let guard_on = |i: &str, t: i64| first_arm(i).map(|a| a <= t).unwrap_or(false);
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).filter(|k| !k.starts_with("HSMP_")).collect();
    keys.sort();
    keys.dedup();
    let (mut bad, mut notes) = (vec![], vec![]);
    for k in keys {
        let meta = |m: Option<&Value>| m.map(|m| (m["size"].as_i64(), m["mtime_ms"].as_i64()));
        let (mb, ma) = (meta(b.get(k)), meta(a.get(k)));
        if mb == ma {
            continue;
        }
        let what = match (mb, ma) {
            (None, Some(_)) => "created during the run".to_string(),
            (Some(_), None) => "deleted during the run".to_string(),
            (Some((sb, tb)), Some((sa, ta))) => format!("written during the run (size {} -> {}, mtime {} -> {})",
                sb.map(|v| v.to_string()).unwrap_or("?".into()), sa.map(|v| v.to_string()).unwrap_or("?".into()),
                tb.map(util::ms_to_iso).unwrap_or("?".into()), ta.map(util::ms_to_iso).unwrap_or("?".into())),
            (None, None) => continue,
        };
        // attribution: the native write of that slot, logged with the guard off
        let slot = k.strip_suffix(".sav").or_else(|| k.strip_suffix(".SAV")).unwrap_or(k);
        let mt = ma.and_then(|m| m.1);
        let who = mt.and_then(|t| calls.iter().find(|e| {
            SAVE_WRITE_FNS.contains(&s(e, "fn").unwrap_or("")) && s(e, "slot").map(|x| x.eq_ignore_ascii_case(slot)).unwrap_or(false)
                && bv(e, "active") == Some(false) && (wall(e) - t).abs() <= SAVE_ATTRIB_MS && !guard_on(inst(e), wall(e))
        }));
        match who {
            Some(e) if mt.map(|t| t > lo).unwrap_or(false) => notes.push(format!("{k}: {what}: pre-session native write by inst {} ({} with its guard off, {} ms from the mtime)",
                inst(e), sv(e, "fn").unwrap_or_default(), (wall(e) - mt.unwrap_or(0)).abs())),
            _ => bad.push(format!("{k}: {what}")),
        }
    }
    // report every interval in which an armed guard went off again (until it re-armed, or
    // to the end of the run). A career write inside one is a FAIL above; the interval itself
    // is evidence the reviewer must see.
    let mut gaps = vec![];
    for i in &x.insts {
        let Some(arm) = first_arm(i) else { continue };
        let mine: Vec<&&Ev> = guards.iter().filter(|g| inst(g) == i.as_str() && wall(g) >= arm).collect();
        let mut off: Option<(i64, String)> = None;
        for g in mine {
            match (bv(g, "active") == Some(true), &off) {
                (false, None) => off = Some((wall(g), sv(g, "why").unwrap_or_default())),
                (true, Some((t, why))) => {
                    gaps.push(format!("inst {i} {} ms from {} ({why})", wall(g) - t, util::ms_to_iso(*t)));
                    off = None;
                }
                _ => {}
            }
        }
        if let Some((t, why)) = off {
            gaps.push(format!("inst {i} off from {} to the end ({why})", util::ms_to_iso(t)));
        }
    }
    let st = if bad.is_empty() { PASS } else { FAIL };
    let mut msg = format!("career .sav size/mtime: {}", if bad.is_empty() { "no unexplained write".to_string() } else { bad.join("; ") });
    if !gaps.is_empty() {
        msg += &format!("; guard off after arming: {}", gaps.join("; "));
    }
    if !notes.is_empty() {
        msg += &format!("; note: {}", notes.join("; "));
    }
    c.add("DoD-8", st, msg, None, None);
}

/// The sidecar's career file guard (layer 2) restores a career file changed during MP at
/// leave/recovery, which hides a layer-1 failure from the before/after hashes. Its actions
/// (career_guard{action,file,why}, <state>/.career_guard.jsonl) are judged: any restore,
/// recreate, quarantine or guard error fails; an instance without any guard report is
/// incomplete (the guard's evidence was not collected).
fn dod8_career_guard(c: &mut Checks, x: &Ctx) {
    let cg: Vec<&Ev> = x.evs.iter().filter(|e| ev_name(e) == "career_guard" && util::src(e) == "sidecar").collect();
    for i in &x.insts {
        let mine: Vec<&&Ev> = cg.iter().filter(|e| inst(e) == i).collect();
        if mine.is_empty() {
            c.add("DoD-8", INCOMPLETE, "no career_guard report (.career_guard.jsonl): a layer-2 restore would be invisible", None, Some(i));
            continue;
        }
        let bad: Vec<String> = mine.iter().filter(|e| matches!(s(e, "action"), Some("restored" | "recreated" | "quarantined" | "error")))
            .map(|e| format!("{} {} ({}): {}", sv(e, "action").unwrap_or_default(), sv(e, "file").unwrap_or_default(), sv(e, "kind").unwrap_or("?".into()), sv(e, "why").unwrap_or_default()))
            .collect();
        let count = |a: &str| mine.iter().filter(|e| s(e, "action") == Some(a)).count();
        if bad.is_empty() {
            c.add("DoD-8", PASS, format!("career guard: {} backup(s), {} clean check(s), {} skipped_newer; no restore", count("backup"), count("clean"), count("skipped_newer")), None, Some(i));
        } else {
            c.add("DoD-8", FAIL, format!("career guard touched career saves (layer 1 missed a write): {}", bad.iter().take(6).cloned().collect::<Vec<_>>().join("; ")), None, Some(i));
        }
    }
}

fn dod10(c: &mut Checks, x: &Ctx) {
    let sent = client(x.evs, "cmd_sent", None);
    let res = client(x.evs, "cmd_result", None);
    // the sidecar gave up resending a command (sidecar cmd_timeout): never "exactly one result"
    let to: Vec<String> = x.evs.iter().filter(|e| ev_name(e) == "cmd_timeout")
        .map(|e| format!("{} #{} ({} tries)", sv(e, "cmd").unwrap_or_default(), sv(e, "cmd_id").unwrap_or_default(), sv(e, "tries").unwrap_or("?".into())))
        .collect();
    if !to.is_empty() {
        c.add("DoD-10", FAIL, format!("cmd_timeout: {}", to.join(", ")), None, None);
    }
    // The server applied a command twice: two server cmd_result lines for one (player, cmd_id)
    // (session.rs command_locked caches the result per player key; a cached resend is cmd_dup).
    let mut srv: BTreeMap<(String, String), usize> = BTreeMap::new();
    for e in x.evs.iter().filter(|e| ev_name(e) == "cmd_result" && inst(e) == "server" && s(e, "source") == Some("command")) {
        *srv.entry((sv(e, "player").unwrap_or_default(), sv(e, "cmd_id").unwrap_or_default())).or_default() += 1;
    }
    for ((p, id), n) in srv.iter().filter(|(_, n)| **n > 1) {
        c.add("DoD-10", FAIL, format!("server applied command #{id} of {p:?} {n} times (dedup cache missed)"), None, None);
    }
    if sent.is_empty() {
        c.add("DoD-10", INCOMPLETE, "no cmd_sent events (client commands not instrumented)", None, None);
        return;
    }
    let key = |e: &Ev| (inst(e).to_string(), sv(e, "cmd_id").unwrap_or_default());
    let mut by: BTreeMap<(String, String), Vec<&Ev>> = BTreeMap::new();
    for e in &res {
        by.entry(key(e)).or_default().push(e);
    }
    // The server's answer as the sidecar received it: an S2G cmd_result record in the sidecar's
    // tap (one per fresh S2CCommandResult; events.rs reads it as src=sidecar ev=cmd_result).
    let mut answers: BTreeMap<(String, String), Vec<&Ev>> = BTreeMap::new();
    for e in x.evs.iter().filter(|e| ev_name(e) == "cmd_result" && util::src(e) == "sidecar") {
        answers.entry(key(e)).or_default().push(e);
    }
    // per instance: was that instance's sidecar tap collected at all?
    let have_answers_file = |i: &str| x.evs.iter().any(|e| util::src(e) == "sidecar" && inst(e) == i && s(e, "file") == Some(crate::events::TAP_FILE))
        || answers.keys().any(|(ai, _)| ai == i);
    let sent_keys: HashSet<(String, String)> = sent.iter().map(|e| key(e)).collect();
    for e in &sent {
        let k = key(e);
        let what = format!("{} #{}", sv(e, "cmd").unwrap_or_default(), k.1);
        let rs = by.get(&k).cloned().unwrap_or_default();
        let detail: Vec<String> = rs.iter().map(|r| format!("({}, {}, {})", sv(r, "ok").unwrap_or_default(), sv(r, "source").unwrap_or("?".into()), sv(r, "reason").unwrap_or_default())).collect();
        // The menu shows exactly one result per command.
        if rs.len() != 1 {
            c.add("DoD-10", FAIL, format!("{what}: {} menu results [{}]", rs.len(), detail.join(", ")), None, Some(inst(e)));
            continue;
        }
        // ... and it must be the SERVER's answer, shown as such: a menu timeout / local /
        // inferred result is the client guessing, never a result. Both halves are required:
        // the sidecar's answer line (exactly one) AND the menu's source="server". A late
        // answer that arrived after the menu already gave up ("no answer from the server") is
        // what the player saw: a refusal, so it fails.
        let ans_v = answers.get(&k).cloned().unwrap_or_default();
        let ans = ans_v.len();
        let src = sv(rs[0], "source");
        let menu_server = src.as_deref() == Some("server");
        let has_file = have_answers_file(inst(e));
        let (st, why) = if ans > 1 {
            (FAIL, format!("{ans} server answers"))
        } else if ans == 1 && menu_server {
            (PASS, "server answered".to_string())
        } else if ans == 1 {
            let late = wall(ans_v[0]) - wall(rs[0]);
            (FAIL, format!("the server answered, but the menu resolved it by {} first (answer arrived {late} ms after the menu gave up): the player saw no server result",
                           src.unwrap_or("?".into())))
        } else if menu_server && has_file {
            (FAIL, "the menu claims a server answer, but the sidecar's tap has no answer for this command".to_string())
        } else if menu_server {
            (INCOMPLETE, "menu source=server but no sidecar tap (ipc_tap.jsonl) for this instance: the server answer is not corroborated".to_string())
        } else if src.is_none() && !has_file {
            (INCOMPLETE, "no source field and no sidecar tap: cannot tell a server answer from a local guess".to_string())
        } else {
            (FAIL, format!("no server answer; the menu resolved it by {}", src.unwrap_or("?".into())))
        };
        c.add("DoD-10", st, format!("{what}: {why} [{}]", detail.join(", ")), None, Some(inst(e)));
    }
    let orphan: Vec<&(String, String)> = by.keys().filter(|k| !sent_keys.contains(*k)).collect();
    if !orphan.is_empty() {
        c.add("DoD-10", FAIL, format!("cmd_result without cmd_sent: {orphan:?}"), None, None);
    }
}

fn dod11(c: &mut Checks, x: &Ctx) {
    let name = x.sc["name"].as_str().unwrap_or("");
    let phases = server_phases(x.evs);
    match name {
        "reconnect" => {
            let Some(m) = marks(x.evs, "blackout").first().map(|e| wall(e)) else {
                c.add("DoD-11", INCOMPLETE, "no blackout mark", None, None);
                return;
            };
            let t1 = marks(x.evs, "blackout_end").first().map(|e| wall(e)).unwrap_or(m + 8000);
            let paused = phases.iter().any(|e| s(e, "to") == Some("Paused") && m <= wall(e) && wall(e) <= t1 + 5000);
            let back = phases.iter().any(|e| s(e, "to") == Some("Live") && wall(e) > m);
            c.add("DoD-11", if paused { PASS } else { FAIL }, format!("duel paused during the blackout: {paused}"), None, None);
            c.add("DoD-11", if back { PASS } else { FAIL }, format!("back to Live after the blackout: {back}"), None, None);
            let seat: Vec<&Ev> = x.evs.iter().filter(|e| ev_name(e) == "seat_restored" && wall(e) > m).collect();
            if let Some(first) = seat.first() {
                let same = seat.iter().all(|e| bv(e, "same_seat") != Some(false) && bv(e, "same_wins") != Some(false));
                c.add("DoD-11", if same { PASS } else { FAIL }, format!("seat/wins restored by key: {}", Value::Object((*first).clone())), None, None);
            } else {
                c.add("DoD-11", INCOMPLETE, "no seat_restored evidence (server --events roster)", None, None);
            }
        }
        "host_leave" => {
            let Some(t0) = marks(x.evs, "host_leave").first().map(|e| wall(e)) else {
                c.add("DoD-11", INCOMPLETE, "no host_leave mark", None, None);
                return;
            };
            for i in x.insts.iter().skip(1) {
                let tr = client(x.evs, "travel", Some(i)).into_iter().find(|e| wall(e) >= t0 && is_menu_world(s(e, "to")));
                let Some(tr) = tr else {
                    c.add("DoD-11", FAIL, "joiner never returned to the menu", None, Some(i));
                    continue;
                };
                let dt = wall(tr) - t0;
                c.add("DoD-11", if dt <= 5000 { PASS } else { FAIL }, format!("joiner in the menu after {dt} ms"), None, Some(i));
                let reason = sv(tr, "reason").unwrap_or_default();
                c.add("DoD-11", if reason.to_lowercase().contains("host") { PASS } else { FAIL }, format!("menu notice/reason: {reason:?} (want 'Host closed the server')"), None, Some(i));
            }
        }
        "server_restart" => {
            let Some(t0) = marks(x.evs, "server_restart").first().map(|e| wall(e)) else {
                c.add("DoD-11", INCOMPLETE, "no server_restart mark", None, None);
                return;
            };
            for i in &x.insts {
                let lr = client(x.evs, "lobby_ready", Some(i));
                let before = lr.iter().rfind(|e| wall(e) < t0);
                let Some(after) = lr.iter().find(|e| wall(e) >= t0) else {
                    c.add("DoD-11", FAIL, "no lobby_ready after the restart", None, Some(i));
                    continue;
                };
                let eb = before.and_then(|e| sv(e, "epoch"));
                match sv(after, "epoch") {
                    None => c.add("DoD-11", INCOMPLETE, "lobby_ready has no epoch field", None, Some(i)),
                    // without the epoch before the restart, any epoch after it "changed"
                    Some(ea) if eb.is_none() => c.add("DoD-11", INCOMPLETE, format!("no lobby_ready with an epoch before the restart: the epoch change (-> {ea}) is not proven"), None, Some(i)),
                    Some(ea) => c.add("DoD-11", if Some(&ea) != eb.as_ref() { PASS } else { FAIL }, format!("epoch {} -> {ea}", eb.unwrap_or("none".into())), None, Some(i)),
                }
                if let Some(msg) = sv(after, "notice") {
                    c.add("DoD-11", PASS, format!("lobby message: {msg:?}"), None, Some(i));
                }
            }
        }
        _ => c.add("DoD-11", INCOMPLETE, format!("DoD-11 not defined for scenario {name}"), None, None),
    }
}

fn dod12(c: &mut Checks, x: &Ctx) {
    match x.fin.as_ref().and_then(|f| f.get("orphans")) {
        None => c.add("DoD-12", INCOMPLETE, "final.json has no orphan scan", None, None),
        Some(o) => {
            let list = o.as_array().cloned().unwrap_or_default();
            c.add("DoD-12", if list.is_empty() { PASS } else { FAIL }, format!("orphan hsmp-* processes after the scenario: {}", if list.is_empty() { "none".into() } else { Value::Array(list).to_string() }), None, None);
        }
    }
    // the games must quit the user's way (the menu's quit, then WM_CLOSE). A game the
    // harness had to kill by PID fails: its quit path hangs, and a kill also skips the menu's
    // session teardown, so the orphan scan above no longer tests that the server a game
    // launched exits with it. Evidence: final.json game_force_killed / quit_path, else the
    // harness.jsonl game_force_killed events.
    let mut killed: Vec<String> = x.fin.as_ref().and_then(|f| f["game_force_killed"].as_array().cloned()).unwrap_or_default()
        .iter().filter_map(|v| v.as_str().map(String::from)).collect();
    for e in harness(x.evs).filter(|e| ev_name(e) == "game_force_killed") {
        if let Some(r) = sv(e, "role") {
            if !killed.contains(&r) {
                killed.push(r);
            }
        }
    }
    let paths = x.fin.as_ref().and_then(|f| f["quit_path"].as_object().cloned());
    // a game that was already dead when quit_all came (dead_before_quit) crashed or exited
    // on its own, unless a scenario step ended it on purpose (harness expected_exit / proc_killed).
    let expected = expected_exits(x);
    let dead: Vec<String> = paths.iter().flatten()
        .filter(|(k, v)| v.as_str() == Some("dead_before_quit") && !expected.contains(k.as_str()))
        .map(|(k, _)| k.clone()).collect();
    if !dead.is_empty() {
        c.add("DoD-12", FAIL, format!("game(s) dead before quit_all (crashed or exited on their own; no scenario step ended them): {}", dead.join(", ")), None, None);
    }
    if !killed.is_empty() {
        killed.sort();
        c.add("DoD-12", FAIL, format!("game(s) force-killed by PID at quit_all (neither the menu quit nor WM_CLOSE ended them): {}", killed.join(", ")), None, None);
    } else if let Some(p) = paths {
        let desc: Vec<String> = p.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or("?"))).collect();
        let wm: Vec<&String> = p.iter().filter(|(_, v)| v.as_str() == Some("wm_close")).map(|(k, _)| k).collect();
        let menu = p.iter().filter(|(_, v)| v.as_str() == Some("menu_quit")).count();
        if p.is_empty() {
            c.add("DoD-12", INCOMPLETE, "final.json quit_path is empty (no game was quit by quit_all)", None, None);
        } else if !wm.is_empty() {
            c.add("DoD-12", INCOMPLETE, format!("the menu quit did not end {wm:?} (WM_CLOSE did): the user's quit path is not proven [{}]", desc.join(", ")), None, None);
        } else if menu == 0 {
            c.add("DoD-12", INCOMPLETE, format!("no game quit through the menu path: the user's quit path is not proven [{}]", desc.join(", ")), None, None);
        } else if dead.is_empty() {
            c.add("DoD-12", PASS, format!("games quit through the menu path [{}]", desc.join(", ")), None, None);
        }
    } else if x.fin.is_some() {
        c.add("DoD-12", INCOMPLETE, "final.json has no quit_path (old harness): how the games quit is unknown", None, None);
    }
    let hits = lint::image_name_kills(x.repo);
    c.add("DoD-12", if hits.is_empty() { PASS } else { FAIL }, format!("kill-by-image-name in the tree: {}", if hits.is_empty() { "none".into() } else { format!("{hits:?}") }), None, None);
}

/// Roles a scenario step ended on purpose: harness `expected_exit{role}` (mp_test.ps1 writes it
/// for a step's `expect_exit`, e.g. host_leave's listen server) and `proc_killed{role}` (kill /
/// restart steps). Everything else that died before quit_all crashed or exited on its own.
fn expected_exits(x: &Ctx) -> HashSet<String> {
    harness(x.evs)
        .filter(|e| matches!(ev_name(e), "expected_exit" | "proc_killed"))
        .filter_map(|e| sv(e, "role"))
        .collect()
}

/// CRASH: the crash and liveness half of DoD-2, judged in EVERY scenario that does not
/// already judge DoD-2 (reconnect, host_leave, server_restart, map_change, start_refused): no new
/// crash dir, no new dump in crash-triage over the run window, and every tracked game / server /
/// master / netsim alive at quit_all unless a scenario step ended it on purpose.
fn crash_rule(c: &mut Checks, x: &Ctx) {
    let Some(f) = &x.fin else {
        c.add("CRASH", INCOMPLETE, "final.json missing (crash dir / liveness not captured)", None, None);
        return;
    };
    let crashes: Vec<String> = f["new_crashes"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    c.add("CRASH", if crashes.is_empty() { PASS } else { FAIL }, format!("new crash dirs: {}", if crashes.is_empty() { "none".into() } else { format!("{crashes:?}") }), None, None);
    match &x.triage {
        Some(t) => {
            let new = soak::triage_new(t);
            c.add("CRASH", if new.is_empty() { PASS } else { FAIL },
                  format!("crash-triage {}: {} new dump(s){}", soak::triage_window(t), new.len(), if new.is_empty() { String::new() } else { format!(": {}", new.join("; ")) }), None, None);
        }
        None => c.add("CRASH", INCOMPLETE, "crash_triage.json missing (hsmp-tools crash-triage not run for the run window)", None, None),
    }
    let expected = expected_exits(x);
    match f["alive_at_end"].as_object() {
        Some(a) if !a.is_empty() => {
            let dead: Vec<&String> = a.iter().filter(|(k, v)| v.as_bool() != Some(true) && !expected.contains(k.as_str())).map(|(k, _)| k).collect();
            let gone: Vec<&String> = a.iter().filter(|(k, v)| v.as_bool() != Some(true) && expected.contains(k.as_str())).map(|(k, _)| k).collect();
            c.add("CRASH", if dead.is_empty() { PASS } else { FAIL },
                  format!("processes dead before teardown: {}{}", if dead.is_empty() { "none".into() } else { format!("{dead:?}") },
                          if gone.is_empty() { String::new() } else { format!(" (ended on purpose by a scenario step: {gone:?})") }), None, None);
        }
        _ => c.add("CRASH", INCOMPLETE, "alive_at_end not recorded", None, None),
    }
}

/// One-way delay (ms) of the hsmp-tools netsim profiles.
fn profile_one_way(p: &str) -> Option<f64> {
    Some(match p {
        "none" | "lan" => 1.0,
        "good" => 20.0,
        "typical" => 50.0,
        "wifi" => 35.0,
        "bad" => 110.0,
        "intl" => 88.0,
        "far" => 150.0,
        "awful" => 180.0,
        _ => return None,
    })
}

/// POSE-1: every `pose_quality` sample during Live, per observing
/// instance's netsim profile. typical (and better): arm_p95_uu <= 5, tip_p95_uu <= 8;
/// wifi: arm_p95_uu <= 10 (tip not bounded); both: latency_ms <= one-way + 40.
/// bad/awful are not specified -> incomplete. No samples at all -> incomplete.
fn pose1(c: &mut Checks, x: &Ctx) {
    let pq = client(x.evs, "pose_quality", None);
    if pq.is_empty() || x.rounds.is_empty() {
        c.add("POSE-1", INCOMPLETE, if pq.is_empty() { "no pose_quality events (pose sampler not instrumented?)" } else { "no rounds" }, None, None);
        return;
    }
    // A peer's pose crosses both instances' links (a listen host has none of its own), so the
    // limits come from the most impaired profile in the run.
    const ORDER: [&str; 9] = ["none", "lan", "good", "typical", "wifi", "intl", "bad", "far", "awful"];
    let worst = x.insts
        .iter()
        .filter_map(|i| x.cfg["netsim"].get(i.as_str()).and_then(|v| v.as_str()))
        .max_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(ORDER.len()))
        .unwrap_or("typical")
        .to_string();
    for i in &x.insts {
        let prof = worst.clone();
        let Some(one_way) = profile_one_way(&prof) else {
            c.add("POSE-1", INCOMPLETE, format!("netsim profile {prof:?} has no POSE-1 limits"), None, Some(i));
            continue;
        };
        // (jitter_ratio, foot_slide_p95 uu/s, idle_rms uu); idle_rms has no wifi limit (none specified)
        let (jit_max, foot_max, idle_max): (f64, f64, Option<f64>) = if prof == "wifi" { (1.5, 8.0, None) } else { (1.2, 5.0, Some(0.5)) };
        let (arm_max, tip_max) = match prof.as_str() {
            "wifi" => (10.0, None),
            "intl" | "bad" | "far" | "awful" => {
                c.add("POSE-1", INCOMPLETE, format!("POSE-1 limits are defined for typical and wifi, not {prof}"), None, Some(i));
                continue;
            }
            _ => (5.0, Some(8.0)),
        };
        let lat_max = one_way + 40.0;
        let mut measured: HashSet<&str> = HashSet::new();
        let mut sampled = false;
        for r in x.rounds {
            let end = r.end_ms.unwrap_or(i64::MAX);
            let smp: Vec<&&Ev> = pq.iter().filter(|e| inst(e) == i && r.live_ms <= wall(e) && wall(e) <= end).collect();
            if smp.is_empty() {
                if r.end_ms.map(|e| e - r.live_ms > 6000).unwrap_or(true) {
                    c.add("POSE-1", FAIL, format!("{}: no pose_quality sample during Live", rl(r)), Some(r.idx), Some(i));
                }
                continue;
            }
            sampled = true;
            let mut bad = vec![];
            let mut missing: HashSet<&str> = HashSet::new();
            for e in &smp {
                let peer = sv(e, "peer").unwrap_or_default();
                let arm = f64v(e, "arm_p95_uu").unwrap_or(f64::INFINITY);
                let tip = f64v(e, "tip_p95_uu").unwrap_or(f64::INFINITY);
                let lat = f64v(e, "latency_ms").unwrap_or(f64::INFINITY);
                if arm > arm_max {
                    bad.push(format!("peer {peer} arm_p95_uu={arm}>{arm_max}"));
                }
                if let Some(tm) = tip_max {
                    if tip > tm {
                        bad.push(format!("peer {peer} tip_p95_uu={tip}>{tm}"));
                    }
                }
                if lat > lat_max {
                    bad.push(format!("peer {peer} latency_ms={lat}>{lat_max}"));
                }
                // stand-in quality: missing fields make the rule incomplete, never pass
                for (k, max) in [("jitter_ratio", Some(jit_max)), ("foot_slide_p95", Some(foot_max)), ("idle_rms", idle_max)] {
                    match f64v(e, k) {
                        None => missing.insert(k),
                        // -1 = not applicable in that Live window (HSMPAvatars: nothing moved / no
                        // planted foot / the owner was never idle). Never a pass on its own: a
                        // metric that is n/a in every sample of the run is incomplete (below).
                        Some(v) if v < 0.0 => false,
                        Some(v) => {
                            measured.insert(k);
                            if let Some(m) = max {
                                if v > m {
                                    bad.push(format!("peer {peer} {k}={v}>{m}"));
                                }
                            }
                            false
                        }
                    };
                }
            }
            if !missing.is_empty() {
                let mut m: Vec<&str> = missing.iter().copied().collect();
                m.sort();
                c.add("POSE-1", INCOMPLETE, format!("{}: pose_quality lacks {} (pose sampler out of date)", rl(r), m.join(", ")), Some(r.idx), Some(i));
            }
            let peers: HashSet<String> = smp.iter().filter_map(|e| sv(e, "peer")).collect();
            let msg = if bad.is_empty() {
                format!("{}: {} samples, {} peer(s) within limits ({prof}: arm<={arm_max}{} lat<={lat_max} jitter<={jit_max} foot<={foot_max}{})", rl(r), smp.len(), peers.len(),
                        tip_max.map(|t| format!(" tip<={t}")).unwrap_or_default(), idle_max.map(|t| format!(" idle<={t}")).unwrap_or_default())
            } else {
{
                let mut uniq: Vec<String> = vec![];
                for b in &bad {
                    if !uniq.contains(b) {
                        uniq.push(b.clone());
                    }
                }
                let more = if uniq.len() > 12 { format!(" (+{} more)", uniq.len() - 12) } else { String::new() };
                format!("{}: {}{more} ({prof})", rl(r), uniq.iter().take(12).cloned().collect::<Vec<_>>().join("; "))
            }
            };
            c.add("POSE-1", if bad.is_empty() { PASS } else { FAIL }, msg, Some(r.idx), Some(i));
            if peers.len() < x.insts.len() - 1 {
                c.add("POSE-1", FAIL, format!("{}: pose_quality for {}/{} peers", rl(r), peers.len(), x.insts.len() - 1), Some(r.idx), Some(i));
            }
        }
        if sampled {
            for k in ["jitter_ratio", "foot_slide_p95", "idle_rms"] {
                if !measured.contains(k) {
                    c.add("POSE-1", INCOMPLETE, format!("{k} was not applicable (-1) in every Live sample of the run: never measured"), None, Some(i));
                }
            }
        }
    }
}


/// SMOOTH-1 limits per netsim profile: stand-in snaps (frames whose motion
/// jumps > 3 uu beyond its targets') and rigid snaps per minute of Live, per peer.
/// From the netfeel model (hsmp-pose feelsim) with headroom; awful is not specified.
fn netfeel_limits(p: &str) -> Option<(f64, f64)> {
    Some(match p {
        "none" | "lan" | "good" | "typical" => (10.0, 0.5),
        "intl" => (20.0, 0.5),
        "far" => (30.0, 0.5),
        "wifi" => (40.0, 1.0),
        "bad" => (80.0, 1.0),
        _ => return None,
    })
}

/// SMOOTH-1 (docs/development/subsystems/replication.md "Rubber banding"):
/// - no correction of an honest local pawn during Live (`pawn_correction` with
///   live = true, other than a fall below the floor);
/// - per peer, stand-in snaps and rigid snaps per minute of Live within the
///   profile's limits, and at most one playback-clock reset per minute.
fn netfeel1(c: &mut Checks, x: &Ctx) {
    let prof = worst_profile(x);
    let Some((snap_max, rigid_max)) = netfeel_limits(&prof) else {
        c.add("SMOOTH-1", INCOMPLETE, format!("SMOOTH-1 has no limits for netsim profile {prof:?}"), None, None);
        return;
    };
    for i in &x.insts {
        let corr: Vec<&Ev> = client(x.evs, "pawn_correction", Some(i)).into_iter()
            .filter(|e| e.get("live").and_then(|v| v.as_bool()) == Some(true) && s(e, "why") != Some("fell")).collect();
        if corr.is_empty() {
            c.add("SMOOTH-1", PASS, "no correction of the local pawn during Live", None, Some(i));
        } else {
            let list: Vec<String> = corr.iter().take(8).map(|e| format!("{} {:.0} cm", s(e, "why").unwrap_or("?"), f64v(e, "dist_cm").unwrap_or(-1.0))).collect();
            c.add("SMOOTH-1", FAIL, format!("{} correction(s) of the local pawn during Live: {}", corr.len(), list.join(", ")), None, Some(i));
        }
        let nf = client(x.evs, "netfeel", Some(i));
        let live: Vec<&&Ev> = nf.iter().filter(|e| x.rounds.iter().any(|r| r.live_ms <= wall(e) && wall(e) <= r.end_ms.unwrap_or(i64::MAX))).collect();
        if live.is_empty() {
            c.add("SMOOTH-1", INCOMPLETE, "no netfeel samples during Live", None, Some(i));
            continue;
        }
        let mut per: std::collections::BTreeMap<String, (f64, f64, f64, f64, f64)> = Default::default();
        for e in &live {
            let w = f64v(e, "window_s").unwrap_or(5.0) / 60.0;
            let a = per.entry(sv(e, "peer").unwrap_or_default()).or_default();
            a.0 += w;
            a.1 += f64v(e, "snaps_per_min").unwrap_or(0.0) * w;
            a.2 += f64v(e, "rigid_snaps").unwrap_or(0.0);
            a.3 += f64v(e, "clock_resets").unwrap_or(0.0);
            a.4 = a.4.max(f64v(e, "jump_max_uu").unwrap_or(0.0));
        }
        for (peer, (mins, snaps, rigid, clk, jmax)) in per {
            let m = mins.max(1e-6);
            let (sr, rr, cr) = (snaps / m, rigid / m, clk / m);
            let ok = sr <= snap_max && rr <= rigid_max && cr <= 1.0;
            c.add("SMOOTH-1", if ok { PASS } else { FAIL },
                format!("peer {peer}: {sr:.1} snaps/min (<= {snap_max}), {rr:.2} rigid snaps/min (<= {rigid_max}), {cr:.2} clock resets/min (<= 1), max jump {jmax:.1} uu over {:.1} min of Live ({prof})", mins),
                None, Some(i));
        }
    }
}

/// SPAWN-1: the largest joint stretch (bone length vs the reference skeleton) of
/// every stand-in in the 3 s after it starts being driven or is re-posed after a
/// teleport, and of the local pawn after it spawns, is at most SPAWN_STRETCH_MAX_UU.
const SPAWN_STRETCH_MAX_UU: f64 = 10.0;
fn spawn1(c: &mut Checks, x: &Ctx) {
    for i in &x.insts {
        let ev = client(x.evs, "spawn_stretch", Some(i));
        if ev.is_empty() {
            c.add("SPAWN-1", INCOMPLETE, "no spawn_stretch samples", None, Some(i));
            continue;
        }
        let bad: Vec<String> = ev.iter().filter(|e| f64v(e, "max_uu").unwrap_or(f64::INFINITY) > SPAWN_STRETCH_MAX_UU)
            .take(8).map(|e| format!("{} {} {:.1} uu ({}, {})", s(e, "who").unwrap_or("?"), sv(e, "peer").unwrap_or_default(),
                f64v(e, "max_uu").unwrap_or(-1.0), s(e, "bone").unwrap_or("?"), s(e, "why").unwrap_or("?"))).collect();
        let worst = ev.iter().filter_map(|e| f64v(e, "max_uu")).fold(0.0, f64::max);
        if bad.is_empty() {
            c.add("SPAWN-1", PASS, format!("{} spawn(s), worst stretch {worst:.1} uu (<= {SPAWN_STRETCH_MAX_UU})", ev.len()), None, Some(i));
        } else {
            c.add("SPAWN-1", FAIL, format!("stretched on spawn (> {SPAWN_STRETCH_MAX_UU} uu): {}", bad.join("; ")), None, Some(i));
        }
    }
}
/// The most impaired netsim profile among the run's instances (a peer's traffic crosses both links).
fn worst_profile(x: &Ctx) -> String {
    const ORDER: [&str; 9] = ["none", "lan", "good", "typical", "wifi", "intl", "bad", "far", "awful"];
    x.insts
        .iter()
        .filter_map(|i| x.cfg["netsim"].get(i.as_str()).and_then(|v| v.as_str()))
        .max_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(ORDER.len()))
        .unwrap_or("typical")
        .to_string()
}

/// An empty hand for PAWN-1: nothing, or the game's bare-hand / feet "weapons".
fn empty_hand(c: Option<&str>) -> bool {
    match c {
        None => true,
        Some(c) => {
            let l = c.to_ascii_lowercase();
            l.is_empty() || l == "none" || l == "null" || l.contains("weapon_fists") || l.contains("weapon_feet")
        }
    }
}

/// PAWN-1 limits (docs/development/subsystems/spawns.md): a protected pawn is standing, conscious, on its spot
/// (the same 1 m as DoD-7's SPAWN_DIST_MAX_CM; pawn_state.dist_cm is measured from the
/// placement destination) and armed as its kit says.
const PAWN_CONSCIOUSNESS_MIN: f64 = 95.0;
const PAWN_PROTECTED_AT: [&str; 4] = ["placed", "ready", "protect", "live"];

/// PAWN-1 ("knocked down at spawn", docs/development/subsystems/spawns.md). Every `pawn_state` with
/// protected=true and at in placed/ready/protect/live: downed=false, consciousness >= 95,
/// dist_cm <= 100, weapon_r/weapon_l not fists/None when the instance's kit has a weapon in that
/// hand (its own kit_verified{who=self, ok=true}). No pawn_state{at=rearm} after at=ready in a
/// round. Names the first offending event. No pawn_state at all: incomplete.
fn pawn1(c: &mut Checks, x: &Ctx) {
    let ps = client(x.evs, "pawn_state", None);
    if ps.is_empty() || x.rounds.is_empty() {
        c.add("PAWN-1", INCOMPLETE, if ps.is_empty() { "no pawn_state events (HSMPSync spawn_place / HSMPLoadout not instrumented?)" } else { "no rounds" }, None, None);
        return;
    }
    let kits = client(x.evs, "kit_verified", None);
    for r in x.rounds {
        for i in &x.insts {
            let mine: Vec<&&Ev> = ps.iter().filter(|e| inst(e) == i && in_round(e, r)).collect();
            let prot: Vec<&&&Ev> = mine.iter().filter(|e| bv(e, "protected") == Some(true) && PAWN_PROTECTED_AT.contains(&s(e, "at").unwrap_or(""))).collect();
            if prot.is_empty() {
                let seen = mine.first().map(|e| format!("; first pawn_state: at={} protected={} dist_cm={} @{}", sv(e, "at").unwrap_or_default(),
                    sv(e, "protected").unwrap_or("?".into()), sv(e, "dist_cm").unwrap_or("?".into()), util::ms_to_iso(wall(e)))).unwrap_or_default();
                c.add("PAWN-1", FAIL, format!("{}: no pawn_state{{protected=true}} (the pawn was not placed / protected){seen}", rl(r)), Some(r.idx), Some(i));
                continue;
            }
            // the kit's hands: this instance's own verified kit (this round, else the latest before it)
            let kit = kits.iter().filter(|k| inst(k) == i && s(k, "who") == Some("self") && bv(k, "ok") == Some(true) && wall(k) <= r.win_hi).rfind(|k| in_round(k, r))
                .or_else(|| kits.iter().rfind(|k| inst(k) == i && s(k, "who") == Some("self") && bv(k, "ok") == Some(true) && wall(k) <= r.win_hi));
            let kit_hand = |k: &str| kit.map(|kv| !empty_hand(s(kv, k)));
            let mut probs: Vec<String> = vec![];
            let mut missing: HashSet<&str> = HashSet::new();
            for e in &prot {
                let at = sv(e, "at").unwrap_or_default();
                let mut why = vec![];
                match bv(e, "downed") {
                    Some(true) => why.push("downed=true".to_string()),
                    Some(false) => {}
                    None => { missing.insert("downed"); }
                }
                match f64v(e, "consciousness") {
                    Some(v) if v < PAWN_CONSCIOUSNESS_MIN => why.push(format!("consciousness={v} (< {PAWN_CONSCIOUSNESS_MIN})")),
                    Some(_) => {}
                    None => { missing.insert("consciousness"); }
                }
                match f64v(e, "dist_cm") {
                    Some(d) if d > SPAWN_DIST_MAX_CM => why.push(format!("dist_cm={d:.0} (> {SPAWN_DIST_MAX_CM:.0} from the placement)")),
                    Some(_) => {}
                    None => { missing.insert("dist_cm"); }
                }
                for (hand, kk) in [("weapon_r", "r_class"), ("weapon_l", "l_class")] {
                    match kit_hand(kk) {
                        Some(true) if empty_hand(s(e, hand)) => why.push(format!("{hand}={} (the kit has a weapon there)", sv(e, hand).unwrap_or("None".into()))),
                        Some(_) => {}
                        None => { missing.insert("the kit's hands (no kit_verified ok=true)"); }
                    }
                }
                if !why.is_empty() {
                    probs.push(format!("pawn_state at={at} @{}: {}", util::ms_to_iso(wall(e)), why.join(", ")));
                }
            }
            // a re-arm after Ready: something still knocks the idle pawns' weapons out before Live
            if let Some(ready) = mine.iter().filter(|e| s(e, "at") == Some("ready")).map(|e| wall(e)).min() {
                if let Some(e) = mine.iter().find(|e| s(e, "at") == Some("rearm") && wall(e) > ready) {
                    probs.push(format!("pawn_state at=rearm @{} after at=ready ({})", util::ms_to_iso(wall(e)), sv(e, "reason").unwrap_or_default()));
                }
            }
            let (st, msg) = if !probs.is_empty() {
                (FAIL, format!("{}: {} offending pawn_state; first: {}{}", rl(r), probs.len(), probs[0],
                               if probs.len() > 1 { format!(" (then: {})", probs[1..].iter().take(3).cloned().collect::<Vec<_>>().join("; ")) } else { String::new() }))
            } else if !missing.is_empty() {
                let mut m: Vec<&str> = missing.into_iter().collect();
                m.sort();
                (INCOMPLETE, format!("{}: pawn_state lacks {} (not checked)", rl(r), m.join(", ")))
            } else {
                (PASS, format!("{}: {} protected pawn_state samples: standing, conscious, on the spot, armed", rl(r), prot.len()))
            };
            c.add("PAWN-1", st, msg, Some(r.idx), Some(i));
        }
    }
}

/// WORLD-1 and COMBAT-1 judge Live only, at a 5 s emitter cadence: a round whose Live is
/// shorter than this cannot carry a verdict (p0_gate's DEBUG KILL ends Live after about 1 s).
const LIVE_JUDGE_MIN_MS: i64 = 10_000;

fn mismatched_ids(e: &Ev) -> Vec<String> {
    // hsmp_log writes an empty Lua table as {}
    e.get("mismatched").and_then(|v| v.as_array()).map(|a| a.iter().map(|v| v.to_string()).collect()).unwrap_or_default()
}

/// WORLD-1 (docs/development/subsystems/world-replication.md): per round and instance,
/// during Live: (1) at least one world_consistency verdict with compared >= 1 (none:
/// incomplete); (2) no id in `mismatched` of two consecutive verdicts of that instance (a
/// single mismatch is healed and allowed): fail; (3) the round's last verdict has
/// hash_match=true: fail otherwise. Rounds with Live < 10 s are not judged.
fn world1(c: &mut Checks, x: &Ctx) {
    let wc = client(x.evs, "world_consistency", None);
    if wc.is_empty() || x.rounds.is_empty() {
        c.add("WORLD-1", INCOMPLETE, if wc.is_empty() { "no world_consistency events (HSMPWorld not instrumented?)" } else { "no rounds" }, None, None);
        return;
    }
    let mut judged = 0;
    let mut short = 0;
    for r in x.rounds {
        let Some(end) = r.end_ms else { continue };
        if end - r.live_ms < LIVE_JUDGE_MIN_MS {
            short += 1;
            continue;
        }
        judged += 1;
        let live_s = (end - r.live_ms) as f64 / 1000.0;
        for i in &x.insts {
            let all: Vec<&&Ev> = wc.iter().filter(|e| inst(e) == i).collect();
            let during: Vec<usize> = (0..all.len()).filter(|k| r.live_ms <= wall(all[*k]) && wall(all[*k]) <= end).collect();
            let compared: Vec<&usize> = during.iter().filter(|k| f64v(all[**k], "compared").unwrap_or(0.0) >= 1.0).collect();
            if compared.is_empty() {
                c.add("WORLD-1", INCOMPLETE, format!("{}: no world_consistency verdict with compared >= 1 in {live_s:.0} s of Live", rl(r)), Some(r.idx), Some(i));
                continue;
            }
            let mut probs = vec![];
            for k in &during {
                if *k == 0 {
                    continue;
                }
                let (a, b) = (all[k - 1], all[*k]);
                if sv(a, "level") != sv(b, "level") {
                    continue; // another world: ids are not comparable
                }
                let prev = mismatched_ids(a);
                let rep: Vec<String> = mismatched_ids(b).into_iter().filter(|id| prev.contains(id)).collect();
                if !rep.is_empty() {
                    probs.push(format!("id(s) {} mismatched in two consecutive verdicts (@{} and @{}): a real divergence", rep.join(","), util::ms_to_iso(wall(a)), util::ms_to_iso(wall(b))));
                }
            }
            let last = all[*during.last().unwrap()];
            match bv(last, "hash_match") {
                Some(true) => {}
                Some(false) => probs.push(format!("the round's last verdict has hash_match=false ({} mismatched)", mismatched_ids(last).len())),
                None => {
                    c.add("WORLD-1", INCOMPLETE, format!("{}: the last world_consistency has no hash_match", rl(r)), Some(r.idx), Some(i));
                    continue;
                }
            }
            if probs.is_empty() {
                c.add("WORLD-1", PASS, format!("{}: {} verdicts in {live_s:.0} s of Live, no repeated mismatch, last hash_match=true", rl(r), during.len()), Some(r.idx), Some(i));
            } else {
                c.add("WORLD-1", FAIL, format!("{}: {}", rl(r), probs.join("; ")), Some(r.idx), Some(i));
            }
        }
    }
    if judged == 0 {
        c.add("WORLD-1", INCOMPLETE, format!("no round had Live >= {} s ({short} shorter: the scenario ends Live at once), so no world_consistency verdict could be judged", LIVE_JUDGE_MIN_MS / 1000), None, None);
    } else if short > 0 {
        c.add("WORLD-1", PASS, format!("{short} round(s) with Live < {} s not judged", LIVE_JUDGE_MIN_MS / 1000), None, None);
    }
}

/// WORLD-2's limit on the p95 divergence of moving bodies between two screens (cm), by the
/// most impaired profile: the world-sync simulator's results with margin
/// (tests/hsmpworld-sim, world_sync.rs `world2_limit`). None: no limit defined (incomplete).
fn world2_limit(profile: &str) -> Option<f64> {
    match profile {
        "none" | "lan" | "good" => Some(80.0),
        "typical" | "wifi" => Some(130.0),
        "intl" => Some(170.0),
        "bad" | "far" => Some(260.0),
        _ => None,
    }
}

/// Two screens' final rest poses of a body may differ by this much (cm): WORLD-1's pose tolerance
/// (world.rs POS_TOL; a hinged or chained body is not pinned exactly).
const REST_TOL_CM: f64 = 5.0;

/// One instance's world_track of one body: (server ms, position, rest).
type Track = Vec<(f64, [f64; 3], bool)>;

/// `b`'s position at host-clock time t: linear between its samples (at most 250 ms apart).
fn track_at(b: &Track, t: f64) -> Option<[f64; 3]> {
    let i = b.iter().position(|s| s.0 >= t)?;
    if i == 0 {
        return (b[0].0 - t < 1.0).then_some(b[0].1);
    }
    let (p, q) = (b[i - 1], b[i]);
    if q.0 - p.0 > 250.0 {
        return None;
    }
    let k = (t - p.0) / (q.0 - p.0).max(1e-9);
    Some([p.1[0] + (q.1[0] - p.1[0]) * k, p.1[1] + (q.1[1] - p.1[1]) * k, p.1[2] + (q.1[2] - p.1[2]) * k])
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// WORLD-2 (docs/development/subsystems/world-replication.md, "Measuring sync"): per round
/// whose Live lasted >= 10 s, HSMPWorld's world_track samples (host clock: one machine) of every two
/// instances are paired per body (same nid, level, epoch): (1) the p95 distance of moving
/// samples to the other screen's track at the same moment <= world2_limit(profile); (2) every
/// body both screens tracked ends at the same rest pose (last rest samples within REST_TOL_CM);
/// (3) no hard snap (world_sync_quality hard_snaps) during Live. Nothing paired in a judged
/// round (nothing moved: the scenario pokes props) is incomplete.
fn world2(c: &mut Checks, x: &Ctx) {
    let tr = client(x.evs, "world_track", None);
    if tr.is_empty() || x.rounds.is_empty() {
        c.add("WORLD-2", INCOMPLETE, if tr.is_empty() { "no world_track events (HSMPWorld tracks only in harness runs: HSMP_AUTOTEST)" } else { "no rounds" }, None, None);
        return;
    }
    let profile = worst_profile(x);
    let Some(limit) = world2_limit(&profile) else {
        c.add("WORLD-2", INCOMPLETE, format!("no WORLD-2 limit for profile {profile}"), None, None);
        return;
    };
    let quality = client(x.evs, "world_sync_quality", None);
    let mut judged = 0;
    for r in x.rounds {
        let Some(end) = r.end_ms else { continue };
        if end - r.live_ms < LIVE_JUDGE_MIN_MS {
            continue;
        }
        judged += 1;
        // (instance, level, epoch, nid) -> track
        let mut tracks: BTreeMap<(String, String, String, String), Track> = BTreeMap::new();
        for e in tr.iter().filter(|e| r.live_ms <= wall(e) && wall(e) <= end) {
            let (Some(t), Some(px), Some(py), Some(pz)) = (f64v(e, "t"), f64v(e, "x"), f64v(e, "y"), f64v(e, "z")) else { continue };
            let key = (inst(e).to_string(), sv(e, "level").unwrap_or_default(), sv(e, "epoch").unwrap_or_default(), sv(e, "nid").unwrap_or_default());
            tracks.entry(key).or_default().push((t, [px, py, pz], bv(e, "rest") == Some(true)));
        }
        for v in tracks.values_mut() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let (mut moving, mut rest_bad, mut rest_n) = (Vec::new(), Vec::new(), 0);
        let keys: Vec<_> = tracks.keys().cloned().collect();
        for a in &keys {
            for b in &keys {
                if !(a.0 < b.0 && a.1 == b.1 && a.2 == b.2 && a.3 == b.3) {
                    continue;
                }
                let (ta, tb) = (&tracks[a], &tracks[b]);
                for s in ta.iter().filter(|s| !s.2) {
                    if let Some(p) = track_at(tb, s.0) {
                        moving.push(dist3(s.1, p));
                    }
                }
                if let (Some(la), Some(lb)) = (ta.iter().rev().find(|s| s.2), tb.iter().rev().find(|s| s.2)) {
                    rest_n += 1;
                    let d = dist3(la.1, lb.1);
                    if d > REST_TOL_CM {
                        rest_bad.push(format!("body {} rests {d:.1} cm apart (inst {} vs {})", a.3, a.0, b.0));
                    }
                }
            }
        }
        if moving.is_empty() {
            c.add("WORLD-2", INCOMPLETE, format!("{}: no body moved on two screens at once (no paired world_track samples)", rl(r)), Some(r.idx), None);
            continue;
        }
        moving.sort_by(|a, b| a.total_cmp(b));
        let p95 = moving[((moving.len() - 1) as f64 * 0.95).round() as usize];
        let p50 = moving[(moving.len() - 1) / 2];
        let snaps: f64 = quality.iter().filter(|e| r.live_ms <= wall(e) && wall(e) <= end).filter_map(|e| f64v(e, "hard_snaps")).sum();
        let mut probs = Vec::new();
        if p95 > limit {
            probs.push(format!("moving bodies p95 {p95:.0} cm apart > {limit:.0} ({profile})"));
        }
        probs.extend(rest_bad.iter().take(4).cloned());
        if snaps > 0.0 {
            probs.push(format!("{snaps:.0} hard snap(s)"));
        }
        let msg = format!("{}: {} paired samples p50 {p50:.0} / p95 {p95:.0} cm (limit {limit:.0}, {profile}), {rest_n} rest poses compared, {snaps:.0} hard snaps",
                          rl(r), moving.len());
        if probs.is_empty() {
            c.add("WORLD-2", PASS, msg, Some(r.idx), None);
        } else {
            c.add("WORLD-2", FAIL, format!("{msg}: {}", probs.join("; ")), Some(r.idx), None);
        }
    }
    if judged == 0 {
        c.add("WORLD-2", INCOMPLETE, format!("no round had Live >= {} s", LIVE_JUDGE_MIN_MS / 1000), None, None);
    }
}

/// COMBAT-1 acceptance floors: honest acceptance in docs/development/subsystems/combat.md (G0 sim thresholds):
/// >= 99 % loopback, >= 97 % typical, >= 93 % wifi. None for bad/awful (not documented).
fn combat_floor(profile: &str) -> Option<f64> {
    match profile {
        "none" | "lan" => Some(0.99),
        "good" | "typical" => Some(0.97),
        "wifi" => Some(0.93),
        _ => None,
    }
}
/// The acceptance ratio is only judged over at least this many decided claims (a handful of
/// claims cannot tell 97 % from 80 %).
const COMBAT_MIN_CLAIMS: f64 = 20.0;
/// `pending` (claims without a final answer) may rise by this much over a round (in flight).
const COMBAT_PENDING_SLACK: f64 = 5.0;

/// COMBAT-1 (HSMPCombat `combat_quality` every 5 s of combat during Live, per-window
/// counts, `pending` carried): per round and instance whose Live lasted >= 10 s, at least one
/// sample (none: incomplete; the emitter is silent without combat); `pending` not growing
/// across the round (last <= first + 5); accepted / (claims - parried) >= the profile's
/// documented floor when >= 20 claims were decided. The same ratio over all of an instance's
/// Live samples in the run. Parried is a legitimate outcome, not a rejection of an honest hit.
fn combat1(c: &mut Checks, x: &Ctx) {
    let mut cq = client(x.evs, "combat_quality", None);
    cq.extend(client(x.evs, "x_combat_quality", None));
    cq.sort_by_key(|e| wall(e));
    if cq.is_empty() || x.rounds.is_empty() {
        c.add("COMBAT-1", INCOMPLETE, if cq.is_empty() { "no combat_quality events (no combat during Live, or HSMPCombat not instrumented)" } else { "no rounds" }, None, None);
        return;
    }
    let prof = worst_profile(x);
    let floor = combat_floor(&prof);
    let n = |e: &Ev, k: &str| f64v(e, k).unwrap_or(0.0);
    let parried = |e: &Ev| e.get("rejected_by_reason").and_then(|v| v.get("parried")).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ratio_check = |claims: f64, accepted: f64, par: f64| -> (Option<bool>, String) {
        let decided = claims - par;
        match floor {
            None => (None, format!("no documented acceptance floor for {prof}")),
            Some(_) if decided < COMBAT_MIN_CLAIMS => (Some(true), format!("acceptance not judged ({decided:.0} decided claims < {COMBAT_MIN_CLAIMS:.0})")),
            Some(f) => {
                let q = (accepted / decided).min(1.0);
                (Some(q >= f), format!("accepted {accepted:.0}/{decided:.0} = {:.1} % (floor {:.0} % under {prof}; {par:.0} parried excluded)", q * 100.0, f * 100.0))
            }
        }
    };
    let (mut judged, mut short) = (0, 0);
    for r in x.rounds {
        let Some(end) = r.end_ms else { continue };
        if end - r.live_ms < LIVE_JUDGE_MIN_MS {
            short += 1;
            continue;
        }
        judged += 1;
        let live_s = (end - r.live_ms) as f64 / 1000.0;
        for i in &x.insts {
            let smp: Vec<&&Ev> = cq.iter().filter(|e| inst(e) == i && r.live_ms <= wall(e) && wall(e) <= end).collect();
            if smp.is_empty() {
                c.add("COMBAT-1", INCOMPLETE, format!("{}: no combat_quality in {live_s:.0} s of Live (no combat sampled)", rl(r)), Some(r.idx), Some(i));
                continue;
            }
            let mut probs = vec![];
            match (f64v(smp[0], "pending"), f64v(smp[smp.len() - 1], "pending")) {
                (Some(a), Some(b)) if b > a + COMBAT_PENDING_SLACK => probs.push(format!("pending grew {a:.0} -> {b:.0} (claims never answered: the claim pipeline leaks)")),
                (Some(_), Some(_)) => {}
                _ => {
                    c.add("COMBAT-1", INCOMPLETE, format!("{}: combat_quality has no pending field", rl(r)), Some(r.idx), Some(i));
                    continue;
                }
            }
            let (cl, ac, pa) = smp.iter().fold((0.0, 0.0, 0.0), |t, e| (t.0 + n(e, "claims"), t.1 + n(e, "accepted"), t.2 + parried(e)));
            let (ok, why) = ratio_check(cl, ac, pa);
            if ok == Some(false) {
                probs.push(why.clone());
            }
            if !probs.is_empty() {
                c.add("COMBAT-1", FAIL, format!("{}: {}", rl(r), probs.join("; ")), Some(r.idx), Some(i));
            } else if ok.is_none() {
                c.add("COMBAT-1", INCOMPLETE, format!("{}: {why}", rl(r)), Some(r.idx), Some(i));
            } else {
                c.add("COMBAT-1", PASS, format!("{}: {} samples, {cl:.0} claims; {why}; pending stable", rl(r), smp.len()), Some(r.idx), Some(i));
            }
        }
    }
    // the whole run per instance: every combat_quality sample during any Live
    for i in &x.insts {
        let live: Vec<&&Ev> = cq.iter().filter(|e| inst(e) == i && x.rounds.iter().any(|r| r.live_ms <= wall(e) && wall(e) <= r.end_ms.unwrap_or(i64::MAX))).collect();
        let (cl, ac, pa) = live.iter().fold((0.0, 0.0, 0.0), |t, e| (t.0 + n(e, "claims"), t.1 + n(e, "accepted"), t.2 + parried(e)));
        if let (Some(false), why) = ratio_check(cl, ac, pa) {
            c.add("COMBAT-1", FAIL, format!("whole run: {why}"), None, Some(i));
        }
    }
    if judged == 0 {
        c.add("COMBAT-1", INCOMPLETE, format!("no round had Live >= {} s ({short} shorter: the scenario ends Live at once), so combat quality could not be judged per round", LIVE_JUDGE_MIN_MS / 1000), None, None);
    } else if short > 0 {
        c.add("COMBAT-1", PASS, format!("{short} round(s) with Live < {} s not judged", LIVE_JUDGE_MIN_MS / 1000), None, None);
    }
}

/// The commit this gate binary was built from (build.rs) and whether its package
/// tree was dirty: {"commit", "dirty"} with dirty "0", "1" or "unknown".
pub fn gate_build() -> Value {
    json!({"commit": env!("HSMP_GIT_SHA"), "dirty": env!("HSMP_GIT_DIRTY")})
}

/// The judge's identity for a run: this binary's build, except in synthetic fixtures (run.json
/// `fixture: true`, written only by `make-fixtures`), which carry the gate identity they model.
fn judge_identity(cfg: &Value) -> Value {
    if cfg["fixture"].as_bool() == Some(true) {
        if let Some(g) = cfg.get("fixture_gate") {
            return g.clone();
        }
    }
    gate_build()
}

/// Release mod set (mods/mods.release.txt): name -> "0" | "1" | "dev".
fn release_template(repo: &Path) -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(repo.join("mods").join("mods.release.txt")).ok()?;
    let mut m = BTreeMap::new();
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Some(m)
}

/// DoD-13 certifies the BUILD UNDER TEST, not a stamp. The deploy stamp
/// (hsmp_deploy.json) records the profile, -SkipBuild, and the SHA-256 of every deployed file
/// (`hashes`, incl. the binaries copied into Win64\hsmp); mp_test.ps1 re-hashes them when the
/// run starts (run.json `deploy_check`) and hashes the binaries it actually runs. And the
/// gate that judges must be built from the deployed commit.
fn dod13_build(c: &mut Checks, x: &Ctx) {
    let dep = &x.cfg["deploy"];
    if dep.is_null() {
        return; // reported below ("no deploy stamp")
    }
    // profile: the release mod set; "dev" = release + the template's dev-only diagnostics
    match dep["profile"].as_str() {
        Some("release") => c.add("DoD-13", PASS, "deploy profile: release", None, None),
        Some("dev") => {
            // the dev profile may only ADD the template's "dev" entries; every shipped mod must be on
            let mut bad = vec![];
            let (tpl, mods) = (release_template(x.repo), dep["mods"].as_object());
            if tpl.is_none() || mods.is_none() {
                bad.push(format!("cannot compare ({})", if tpl.is_none() { "mods/mods.release.txt not found" } else { "the stamp has no mods table" }));
            }
            if let (Some(tpl), Some(mods)) = (tpl, mods) {
                for (k, v) in &tpl {
                    let on = mods.get(k).and_then(|m| m.as_str()) == Some("1");
                    match v.as_str() {
                        "1" if !on => bad.push(format!("{k} (shipped) was off")),
                        "0" if on => bad.push(format!("{k} (not shipped) was on")),
                        _ => {}
                    }
                }
            }
            if bad.is_empty() {
                c.add("DoD-13", PASS, "deploy profile: dev (release mods + the template's dev-only diagnostics)", None, None);
            } else {
                c.add("DoD-13", INCOMPLETE, format!("deploy profile dev differs from the release mod set: {}", bad.join(", ")), None, None);
            }
        }
        p => c.add("DoD-13", INCOMPLETE, format!("deploy profile {:?}: not the release configuration", p.unwrap_or("none")), None, None),
    }
    if dep["skip_build"].as_bool() == Some(true) && dep["bins_match_commit"].as_bool() != Some(true) {
        c.add("DoD-13", INCOMPLETE, format!("deployed with -SkipBuild: the binaries (built from {}) are not proven to match the deployed commit {}",
                                            short(dep["bins_commit"].as_str().unwrap_or("unknown")), short(dep["commit"].as_str().unwrap_or("?"))), None, None);
    }
    let hashes = dep["hashes"].as_object();
    match hashes {
        None => c.add("DoD-13", INCOMPLETE, "the deploy stamp has no file hashes (old build-and-deploy.ps1): the binaries under test are unknown; redeploy", None, None),
        Some(h) if h.is_empty() => c.add("DoD-13", INCOMPLETE, "the deploy stamp lists no deployed files", None, None),
        Some(h) => {
            let chk = &x.cfg["deploy_check"];
            if chk.is_null() {
                c.add("DoD-13", INCOMPLETE, "run.json has no deploy_check: the deployed files were not re-hashed when the run started", None, None);
            } else {
                let list = |k: &str| -> Vec<String> { chk[k].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect() };
                let (mism, miss) = (list("mismatched"), list("missing"));
                let n = chk["checked"].as_u64().unwrap_or(0);
                if !mism.is_empty() || !miss.is_empty() {
                    c.add("DoD-13", FAIL, format!("deployed files changed since the deploy stamp: {} changed {:?}, {} missing {:?}", mism.len(), mism.iter().take(8).collect::<Vec<_>>(), miss.len(), miss.iter().take(8).collect::<Vec<_>>()), None, None);
                } else if (n as usize) < h.len() {
                    c.add("DoD-13", INCOMPLETE, format!("deploy_check re-hashed {n} of {} stamped files", h.len()), None, None);
                } else {
                    c.add("DoD-13", PASS, format!("{n} deployed files re-hashed at run start: identical to the deploy stamp"), None, None);
                }
                // the binaries this run actually executed (bin_dir) must be the stamped ones
                match chk["bins_used"].as_object() {
                    Some(used) if !used.is_empty() => {
                        let stamped: HashMap<String, String> = h.iter()
                            .filter_map(|(k, v)| Some((k.rsplit(['/', '\\']).next()?.to_ascii_lowercase(), v.as_str()?.to_ascii_uppercase()))).collect();
                        let bad: Vec<String> = used.iter().filter(|(k, v)| stamped.get(&k.to_ascii_lowercase()) != v.as_str().map(|s| s.to_ascii_uppercase()).as_ref())
                            .map(|(k, _)| k.clone()).collect();
                        if bad.is_empty() {
                            c.add("DoD-13", PASS, format!("the run executed the stamped binaries ({})", used.keys().cloned().collect::<Vec<_>>().join(", ")), None, None);
                        } else {
                            c.add("DoD-13", FAIL, format!("the run executed binaries that are not the deployed ones: {bad:?} (bin_dir {})", chk["bin_dir"].as_str().unwrap_or("?")), None, None);
                        }
                    }
                    _ => c.add("DoD-13", INCOMPLETE, "deploy_check has no bins_used: which server/sidecar binaries ran is unknown", None, None),
                }
            }
        }
    }
    // the judge itself
    let gate = judge_identity(x.cfg);
    let gc = gate["commit"].as_str().unwrap_or("unknown");
    match dep["commit"].as_str() {
        Some(dc) if gc == dc && gate["dirty"].as_str() == Some("0") => c.add("DoD-13", PASS, format!("judged by hsmp-gate built from the deployed commit {}", short(dc)), None, None),
        Some(dc) if gc == dc => c.add("DoD-13", INCOMPLETE, format!("hsmp-gate was built from {} with uncommitted tools changes (dirty={}): the rules that judged this run are not the commit's", short(dc), gate["dirty"].as_str().unwrap_or("?")), None, None),
        Some(dc) => c.add("DoD-13", INCOMPLETE, format!("hsmp-gate was built from {} but the deployed build is {}: the run was judged by other rules (rebuild the tools at the deployed commit)", short(gc), short(dc)), None, None),
        None => {}
    }
}

fn dod13(c: &mut Checks, x: &Ctx) {
    dod13_build(c, x);
    let Some(g0) = &x.g0 else {
        c.add("DoD-13", INCOMPLETE, "g0.json missing (run: hsmp-gate g0 --json <run>/g0.json)", None, None);
        return;
    };
    // Provenance: a g0.json only certifies the build under test if it ran the full check set
    // on a clean tree of the deployed commit. The run's own g0 is `--quick` (no cargo): the
    // cargo half comes from the deploy stamp's full-G0 record for the same commit.
    let checks = g0["checks"].as_object().cloned().unwrap_or_default();
    let commit = g0["commit"].as_str().unwrap_or("");
    let dep = &x.cfg["deploy"];
    // the deploy stamp's full G0 (hsmp-g0.json: every check incl. cargo) for the deployed commit
    let full_for_deployed = dep["g0_ok"].as_bool() == Some(true)
        && dep["dirty"].as_bool() == Some(false)
        && dep["commit"].as_str().is_some_and(|dc| dep["g0"]["commit"].as_str() == Some(dc) && dc == commit);
    for k in g0::REQUIRED_CHECKS {
        if !checks.contains_key(*k) {
            c.add("DoD-13", INCOMPLETE, format!("{k}: not in g0.json (partial or old G0)"), None, None);
        }
    }
    for (k, v) in &checks {
        let st = match v["status"].as_str() {
            Some("pass") => PASS,
            Some("fail") => FAIL,
            // `--quick` skips cargo: covered only by a full G0 of the same, deployed commit
            Some("skip") if full_for_deployed => PASS,
            _ => INCOMPLETE,
        };
        c.add("DoD-13", st, format!("{k}: {} {}", v["status"].as_str().unwrap_or("?"), v["summary"].as_str().unwrap_or("")), None, None);
    }
    if commit.is_empty() {
        c.add("DoD-13", INCOMPLETE, "g0.json has no commit", None, None);
    }
    if g0["tree_dirty"].as_bool() != Some(false) {
        c.add("DoD-13", INCOMPLETE, "g0.json ran on a dirty (or unknown) tree", None, None);
    }
    match dep["commit"].as_str() {
        None => c.add("DoD-13", INCOMPLETE, "run.json has no deploy stamp (hsmp_deploy.json): the tested build is unknown", None, None),
        Some(dc) => {
            if dc != commit {
                c.add("DoD-13", INCOMPLETE, format!("g0.json is for {} but the deployed build is {}", short(commit), short(dc)), None, None);
            }
            let full = dep["g0_ok"].as_bool() == Some(true) && dep["g0"]["commit"].as_str() == Some(dc) && dep["dirty"].as_bool() == Some(false);
            if g0["quick"].as_bool() != Some(false) && !full {
                c.add("DoD-13", INCOMPLETE, format!("quick G0 only: no full G0 (cargo) pass recorded for the deployed commit {} on a clean tree", short(dc)), None, None);
            } else if full {
                c.add("DoD-13", PASS, format!("full G0 passed for the deployed commit {}", short(dc)), None, None);
            }
        }
    }
}

fn short(c: &str) -> &str {
    &c[..c.len().min(10)]
}

/// NETSIM: the run was impaired the way its scenario requires. `-Netsim` may only make it worse
/// (a weaker profile proves nothing about the scenario's DoD), and each impaired instance's
/// proxy must have started with that profile (`netsim<i>.jsonl` netsim_start). The listen host
/// talks to its own server (no proxy by design).
fn netsim_rule(c: &mut Checks, x: &Ctx) {
    const ORDER: [&str; 9] = ["none", "lan", "good", "typical", "wifi", "intl", "bad", "far", "awful"];
    let rank = |p: &str| ORDER.iter().position(|o| *o == p);
    let want_spec = x.sc["netsim"].as_str().unwrap_or("typical").to_string();
    let wants: Vec<&str> = want_spec.split(',').map(str::trim).collect();
    let listen = x.sc["topology"].as_str() == Some("listen");
    for (k, i) in x.insts.iter().enumerate() {
        if listen && i == "1" {
            continue;
        }
        let want = wants.get(k).or(wants.last()).copied().unwrap_or("typical");
        let got = x.cfg["netsim"].get(i.as_str()).and_then(|v| v.as_str());
        let Some(got) = got else {
            c.add("NETSIM", INCOMPLETE, format!("run.json has no netsim profile (scenario needs {want})"), None, Some(i));
            continue;
        };
        match (rank(got), rank(want)) {
            (Some(g), Some(w)) if g < w => {
                c.add("NETSIM", INCOMPLETE, format!("ran under {got}, weaker than the scenario's {want}: this run cannot certify it"), None, Some(i));
                continue;
            }
            (None, _) | (_, None) => {
                c.add("NETSIM", INCOMPLETE, format!("unknown profile {got:?} (scenario needs {want})"), None, Some(i));
                continue;
            }
            _ => {}
        }
        if got == "none" {
            c.add("NETSIM", PASS, "no impairment required", None, Some(i));
            continue;
        }
        let start = x.evs.iter().rfind(|e| util::src(e) == "netsim" && inst(e) == i && ev_name(e) == "netsim_start");
        match start {
            None => c.add("NETSIM", INCOMPLETE, format!("no netsim_start in netsim{i}.jsonl (proxy not evidenced; profile {got})"), None, Some(i)),
            Some(e) if s(e, "profile") == Some(got) => c.add("NETSIM", PASS, format!("proxy ran {got} (scenario {want})"), None, Some(i)),
            Some(e) => c.add("NETSIM", FAIL, format!("proxy ran {:?} but run.json says {got}", sv(e, "profile").unwrap_or_default()), None, Some(i)),
        }
        netsim_traffic(c, x, i);
        let st: Vec<&Ev> = x.evs.iter().filter(|e| util::src(e) == "netsim" && inst(e) == i && ev_name(e) == "netsim_stats").collect();
        if !st.is_empty() {
            netsim_quality(c, x, i, &st, start);
        }
    }
}

/// A proxy that started is not a proxy the game used. The client may reach the server
/// directly (a master listing, a stale .settings.json, a reconnect fallback) and the run is then
/// not impaired at all. netsim's `netsim_stats{in,out,clients}` (cumulative, every 5 s) must show
/// game traffic: in and out growing across the run with clients >= 1, and growing across every
/// round (the last sample at or before Live to the first at or after the round's end).
fn netsim_traffic(c: &mut Checks, x: &Ctx, i: &str) {
    let st: Vec<&Ev> = x.evs.iter().filter(|e| util::src(e) == "netsim" && inst(e) == i && ev_name(e) == "netsim_stats").collect();
    if st.is_empty() {
        c.add("NETSIM", INCOMPLETE, format!("no netsim_stats in netsim{i}.jsonl: game traffic through the proxy is not evidenced"), None, Some(i));
        return;
    }
    let v = |e: &Ev, k: &str| f64v(e, k).unwrap_or(0.0);
    let (first, last) = (st[0], st[st.len() - 1]);
    let max_clients = st.iter().map(|e| v(e, "clients")).fold(0.0, f64::max);
    let (din, dout) = (v(last, "in") - v(first, "in"), v(last, "out") - v(first, "out"));
    if din <= 0.0 || dout <= 0.0 || max_clients < 1.0 {
        c.add("NETSIM", FAIL, format!("the proxy carried no game traffic (in +{din:.0}, out +{dout:.0}, clients max {max_clients:.0} over {} samples): the client bypassed the impairment", st.len()), None, Some(i));
        return;
    }
    let mut judged = 0;
    let mut idle: Vec<String> = vec![];
    for r in x.rounds {
        let Some(end) = r.end_ms else { continue };
        let before = st.iter().rfind(|e| wall(e) <= r.live_ms);
        let after = st.iter().find(|e| wall(e) >= end);
        let (Some(b), Some(a)) = (before, after) else { continue };
        judged += 1;
        if !(v(a, "in") > v(b, "in") && v(a, "out") > v(b, "out") && v(a, "clients") >= 1.0) {
            idle.push(format!("{} (in {:.0}->{:.0}, out {:.0}->{:.0}, clients {:.0})", rl(r), v(b, "in"), v(a, "in"), v(b, "out"), v(a, "out"), v(a, "clients")));
        }
    }
    if !idle.is_empty() {
        c.add("NETSIM", FAIL, format!("no traffic through the proxy across {}/{judged} round(s): {}", idle.len(), idle.join("; ")), None, Some(i));
    } else {
        c.add("NETSIM", PASS, format!("game traffic through the proxy: in +{din:.0}, out +{dout:.0}, clients {max_clients:.0}; growing across {judged} round(s)"), None, Some(i));
    }
}


/// The proxy's own scheduling error (actual send time - due time, hsmp-tools netsim
/// `netsim_stats.late_*`, per 5 s window) must stay below this p99, or the delay the run added
/// was not the profile's (a starved delivery thread adds tens of ms the profile does not model).
const NETSIM_LATE_P99_MAX_MS: f64 = 2.0;
/// Measured loss is compared with the profile only over at least this many packets.
const NETSIM_LOSS_MIN_PACKETS: f64 = 5000.0;

/// The impairment the proxy really applied. (1) Every `netsim_stats` window that
/// overlaps a Live phase has `late_p99_ms` <= 2 ms (no lateness fields at all: an old netsim, the
/// delay it added is unmeasured -> incomplete; a window above the limit -> incomplete, the run
/// cannot certify the profile). (2) The measured loss (`lost` / `in`, cumulative) is not below
/// half the profile's `loss` (a proxy that dropped less impaired less: incomplete).
fn netsim_quality(c: &mut Checks, x: &Ctx, i: &str, st: &[&Ev], start: Option<&Ev>) {
    let measured: Vec<&Ev> = st.iter().copied().filter(|e| e.contains_key("late_n")).collect();
    if measured.is_empty() {
        c.add("NETSIM", INCOMPLETE, format!("netsim{i}.jsonl has no scheduling-lateness fields (late_*): an old netsim; the delay the proxy itself added is unmeasured"), None, Some(i));
    } else {
        let live = |e: &Ev| {
            let w = wall(e);
            x.rounds.is_empty() || x.rounds.iter().any(|r| r.live_ms < w && w <= r.end_ms.unwrap_or(i64::MAX).saturating_add(5000))
        };
        let judged: Vec<&Ev> = measured.iter().copied().filter(|e| f64v(e, "late_n").unwrap_or(0.0) > 0.0 && live(e)).collect();
        let p99 = |e: &Ev| f64v(e, "late_p99_ms");
        let over: Vec<&Ev> = judged.iter().copied().filter(|e| p99(e).map(|v| v > NETSIM_LATE_P99_MAX_MS).unwrap_or(false)).collect();
        let worst = judged.iter().filter_map(|e| p99(e)).fold(0.0, f64::max);
        let max = judged.iter().filter_map(|e| f64v(e, "late_max_ms")).fold(0.0, f64::max);
        if judged.is_empty() {
            c.add("NETSIM", INCOMPLETE, "no netsim_stats window with delivered packets during Live: the proxy's scheduling error is unmeasured", None, Some(i));
        } else if !over.is_empty() {
            let first = over[0];
            c.add("NETSIM", INCOMPLETE, format!("the proxy's own scheduling error exceeded the profile: late_p99_ms > {NETSIM_LATE_P99_MAX_MS} in {}/{} Live window(s) (worst p99 {worst:.1} ms, max {max:.1} ms; first @{}): the path was not the profile's (box overloaded?)",
                                                over.len(), judged.len(), util::ms_to_iso(wall(first))), None, Some(i));
        } else {
            c.add("NETSIM", PASS, format!("proxy scheduling error p99 <= {worst:.2} ms (max {max:.1} ms) in {} Live window(s)", judged.len()), None, Some(i));
        }
    }
    // measured loss vs the profile's long-run mean
    let want = start.and_then(|s| f64v(s, "loss"));
    let last = st.last();
    let (inp, lost) = (last.and_then(|e| f64v(e, "in")).unwrap_or(0.0), last.and_then(|e| f64v(e, "lost")).unwrap_or(0.0));
    match want {
        Some(w) if w > 0.0 && inp >= NETSIM_LOSS_MIN_PACKETS => {
            let got = lost / inp * 100.0;
            if got < 0.5 * w {
                c.add("NETSIM", INCOMPLETE, format!("measured loss {got:.2} % over {inp:.0} packets is below half the profile's {w} %: the run was impaired less than its profile"), None, Some(i));
            } else {
                c.add("NETSIM", PASS, format!("measured loss {got:.2} % over {inp:.0} packets (profile {w} %)"), None, Some(i));
            }
        }
        Some(w) if w > 0.0 => c.add("NETSIM", PASS, format!("measured loss not judged ({inp:.0} packets < {NETSIM_LOSS_MIN_PACKETS:.0}; profile {w} %)"), None, Some(i)),
        _ => {}
    }
}

/// STATE-1: after a shared-memory run, every instance's state dir holds only
/// allow-listed files (config, identity, logs; see statefiles.rs), judged with the list this
/// gate was built with, from `inst<N>/state_dir_listing.txt` (mp_test.ps1 Collect-State).
/// A run without `ipc = "shm"` in run.json is not judged. Returns whether the rule was
/// judged.
fn state1(c: &mut Checks, x: &Ctx) -> bool {
    if x.cfg["ipc"].as_str() != Some("shm") {
        return false;
    }
    let allow = crate::statefiles::Allow::parse(crate::statefiles::ALLOW_TEXT);
    for i in &x.insts {
        let p = x.run.join(format!("inst{i}")).join("state_dir_listing.txt");
        let Ok(text) = std::fs::read_to_string(&p) else {
            c.add("STATE-1", INCOMPLETE, format!("no inst{i}/state_dir_listing.txt: the state dir was not listed"), None, Some(i.as_str()));
            continue;
        };
        let names: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let bad: Vec<&str> = names.iter().copied().filter(|n| !allow.allows_file(n)).collect();
        if bad.is_empty() {
            c.add("STATE-1", PASS, format!("{} file(s) in the state dir, all allow-listed", names.len()), None, Some(i.as_str()));
        } else {
            c.add("STATE-1", FAIL, format!("files outside the state-dir allow-list (IPC belongs in shared memory): {}", bad.join(" ")), None, Some(i.as_str()));
        }
    }
    true
}

pub fn evaluate(run: &Path, repo: &Path, scenario_override: Option<&str>) -> anyhow::Result<Value> {
    let cfg = events::load_run(run);
    let name = scenario_override.map(String::from).or_else(|| cfg["scenario"].as_str().map(String::from)).unwrap_or("p0_gate".into());
    let n = cfg["instances"].as_u64().unwrap_or(2) as usize;
    let arenas = match &cfg["arenas"] {
        Value::Array(a) => Some(a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(",")),
        Value::String(x) => Some(x.clone()),
        _ => None,
    };
    let mode = if cfg["mode"].as_str() == Some("autotest") { "autotest" } else { "rcon" };
    let sc = scenario::expand(&name, n, arenas.as_deref(), cfg["rounds"].as_u64().map(|r| r as u32), mode)?;
    let evs = events::gather(run, &cfg);
    let rounds = build_rounds(&evs);
    let fin = util::read_json(&run.join("final.json"));
    // every scenario judges crashes (DoD-2, or the CRASH rule where DoD-2 is not listed)
    let triage = soak::triage_report(run, &cfg, fin.as_ref());
    let x = Ctx {
        evs: &evs,
        rounds: &rounds,
        cfg: &cfg,
        sc: &sc,
        insts: (1..=n).map(|i| i.to_string()).collect(),
        baseline: util::read_json(&run.join("baseline.json")),
        fin,
        g0: util::read_json(&run.join("g0.json")),
        repo,
        run,
        triage,
    };
    let _ = x.cfg;
    let mut c = Checks::default();
    let mut dod: Vec<u64> = sc["dod"].as_array().unwrap().iter().filter_map(|v| v.as_u64()).collect();
    let has14 = dod.contains(&14);
    if has14 {
        for d in 1..=7 {
            if !dod.contains(&d) {
                dod.push(d);
            }
        }
        dod.sort();
    }
    for d in &dod {
        match d {
            1 => dod1(&mut c, &x),
            2 => dod2(&mut c, &x),
            3 => dod3(&mut c, &x),
            4 => dod4(&mut c, &x),
            5 => dod5(&mut c, &x),
            6 => dod6(&mut c, &x),
            7 => dod7(&mut c, &x),
            8 => dod8(&mut c, &x),
            9 => c.add("DoD-9", INCOMPLETE, "headless: covered by e2e-test.sh, not by the game gate", None, None),
            10 => dod10(&mut c, &x),
            11 => dod11(&mut c, &x),
            12 => dod12(&mut c, &x),
            13 => dod13(&mut c, &x),
            _ => {}
        }
    }
    let mut rules = serde_json::Map::new();
    for d in &dod {
        if *d != 14 {
            rules.insert(format!("DoD-{d}"), json!(c.rule_status(&format!("DoD-{d}"))));
        }
    }
    for extra in sc["extra_rules"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        let judged = match extra {
            "POSE-1" => { pose1(&mut c, &x); true }
            "SMOOTH-1" => { netfeel1(&mut c, &x); true }
            "SPAWN-1" => { spawn1(&mut c, &x); true }
            "PAWN-1" => { pawn1(&mut c, &x); true }
            "WORLD-1" => { world1(&mut c, &x); true }
            "WORLD-2" => { world2(&mut c, &x); true }
            "COMBAT-1" => { combat1(&mut c, &x); true }
            _ => false,
        };
        if judged {
            rules.insert(extra.into(), json!(c.rule_status(extra)));
        }
        let soak_out = match extra {
            "SOAK-CRASH" => Some(soak::crash(&x.soak())),
            "SOAK-MEM" => Some(soak::mem(&x.soak())),
            "SOAK-LUAERR" => Some(soak::luaerr(&x.soak())),
            "SOAK-HITCH" => Some(soak::hitch(&x.soak())),
            _ => None,
        };
        if let Some(out) = soak_out {
            for (st, msg, i) in out {
                c.add(extra, st, msg, None, i.as_deref());
            }
            rules.insert(extra.into(), json!(c.rule_status(extra)));
        }
    }
    netsim_rule(&mut c, &x);
    rules.insert("NETSIM".into(), json!(c.rule_status("NETSIM")));
    if state1(&mut c, &x) {
        rules.insert("STATE-1".into(), json!(c.rule_status("STATE-1")));
    }
    if !dod.contains(&2) {
        crash_rule(&mut c, &x);
        rules.insert("CRASH".into(), json!(c.rule_status("CRASH")));
    }
    if has14 {
        let sub: Vec<&str> = (1..=7).map(|d| rules[&format!("DoD-{d}")].as_str().unwrap()).collect();
        let st = if sub.contains(&FAIL) { FAIL } else if sub.contains(&INCOMPLETE) { INCOMPLETE } else { PASS };
        rules.insert("DoD-14".into(), json!(st));
    }
    let failed_steps: Vec<&Ev> = harness(&evs).filter(|e| ev_name(e) == "step_failed").collect();
    for e in &failed_steps {
        c.add("harness", FAIL, format!("step failed: {} {}", sv(e, "step").unwrap_or_default(), sv(e, "detail").unwrap_or_default()), None, None);
    }
    let torn = evs.iter().filter(|e| ev_name(e) == "_torn_line").count();
    if torn > 0 {
        c.add("harness", FAIL, format!("{torn} torn/invalid JSONL lines"), None, None);
    }
    if !failed_steps.is_empty() || torn > 0 {
        rules.insert("harness".into(), json!(FAIL));
    }
    let vals: Vec<&str> = rules.values().filter_map(|v| v.as_str()).collect();
    let stopgap = sc["stopgap"].as_bool().unwrap_or(false);
    let verdict = if vals.contains(&FAIL) { FAIL } else if vals.contains(&INCOMPLETE) || stopgap { INCOMPLETE } else { PASS };
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for e in &evs {
        *counts.entry(format!("{}:{}", util::src(e), ev_name(e))).or_default() += 1;
    }
    let rounds_v: Vec<Value> = rounds.iter().map(|r| json!({
        "idx": r.idx, "round": r.round, "match_id": r.match_id, "arena": r.arena, "load_ms": r.load_ms,
        "live_ms": r.live_ms, "end_ms": r.end_ms, "epoch": r.epoch, "aborted": r.aborted, "resumed": r.resumed,
    })).collect();
    let checks_v: Vec<Value> = c.items.iter().map(|ch| json!({
        "rule": ch.rule, "status": ch.status, "msg": ch.msg, "round": ch.round, "inst": ch.inst,
    })).collect();
    Ok(json!({
        "run": run.display().to_string(),
        "scenario": sc["name"], "mode": mode, "stopgap": stopgap, "verdict": verdict,
        "rules": rules, "rounds": rounds_v, "checks": checks_v, "event_counts": counts,
        "instances": n, "generated": util::ms_to_iso(util::now_ms()),
        "gate": judge_identity(&cfg),
    }))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn junit(rep: &Value) -> String {
    let checks = rep["checks"].as_array().cloned().unwrap_or_default();
    let mut suites: Vec<(String, Vec<Value>)> = vec![];
    for ch in checks.iter() {
        let rule = ch["rule"].as_str().unwrap_or("?").to_string();
        match suites.iter_mut().find(|(r, _)| *r == rule) {
            Some((_, v)) => v.push(ch.clone()),
            None => suites.push((rule, vec![ch.clone()])),
        }
    }
    let count = |v: &[Value], st: &str| v.iter().filter(|c| c["status"] == st).count();
    let mut out = vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
        format!(r#"<testsuites name="hsmp-gate {}" tests="{}" failures="{}" skipped="{}">"#, xml(rep["scenario"].as_str().unwrap_or("")), checks.len(), count(&checks, FAIL), count(&checks, INCOMPLETE)),
    ];
    for (rule, chs) in &suites {
        out.push(format!(r#"  <testsuite name="{}" tests="{}" failures="{}" skipped="{}">"#, xml(rule), chs.len(), count(chs, FAIL), count(chs, INCOMPLETE)));
        for (k, ch) in chs.iter().enumerate() {
            let mut nm = format!("{rule}#{k}");
            if let Some(r) = ch["round"].as_u64() {
                nm += &format!(" r{r}");
            }
            if let Some(i) = ch["inst"].as_str() {
                nm += &format!(" inst{i}");
            }
            let msg = xml(ch["msg"].as_str().unwrap_or(""));
            out.push(format!(r#"    <testcase classname="hsmp.{}" name="{}">"#, xml(rule), xml(&nm)));
            match ch["status"].as_str() {
                Some(FAIL) => out.push(format!(r#"      <failure message="{msg}"/>"#)),
                Some(INCOMPLETE) => out.push(format!(r#"      <skipped message="{msg}"/>"#)),
                _ => out.push(format!("      <system-out>{msg}</system-out>")),
            }
            out.push("    </testcase>".into());
        }
        out.push("  </testsuite>".into());
    }
    out.push("</testsuites>".into());
    out.join("\n") + "\n"
}

pub fn exit_code(verdict: &str) -> i32 {
    match verdict {
        PASS => 0,
        FAIL => 1,
        _ => 2,
    }
}

pub fn print_summary(rep: &Value, report_path: &Path) {
    println!("scenario {}  mode {}  rounds {}{}", rep["scenario"].as_str().unwrap_or(""), rep["mode"].as_str().unwrap_or(""),
             rep["rounds"].as_array().map(|a| a.len()).unwrap_or(0), if rep["stopgap"] == true { "  [STOPGAP]" } else { "" });
    for (k, v) in rep["rules"].as_object().into_iter().flatten() {
        println!("  {k:8} {}", v.as_str().unwrap_or("").to_uppercase());
        for ch in rep["checks"].as_array().into_iter().flatten() {
            if ch["rule"] == k.as_str() && ch["status"] != PASS {
                let mut wh = String::new();
                if let Some(r) = ch["round"].as_u64() {
                    wh += &format!(" r{r}");
                }
                if let Some(i) = ch["inst"].as_str() {
                    wh += &format!(" inst{i}");
                }
                println!("           - {}{wh}: {}", ch["status"].as_str().unwrap_or(""), ch["msg"].as_str().unwrap_or(""));
            }
        }
    }
    println!("VERDICT: {}   ({})", rep["verdict"].as_str().unwrap_or("").to_uppercase(), report_path.display());
}

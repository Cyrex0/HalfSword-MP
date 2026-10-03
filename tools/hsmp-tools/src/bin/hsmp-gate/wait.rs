//! Live predicates for mp_test.ps1: `wait` (until a matching event exists) and
//! `expect_none` (no matching event within a window). Events come from the same
//! collector the assertions use, so a wait can never disagree with the verdict.

use crate::events;
use crate::scenario::resolve_instances;
use crate::util::{self, ev_name, inst, is_menu_world, norm_map, s, sv, wall, Ev};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

fn where_ok(e: &Ev, wh: &Value) -> bool {
    let Some(o) = wh.as_object() else { return true };
    for (k, want) in o {
        let got = sv(e, k);
        let want_s = want.as_str().map(String::from).unwrap_or_else(|| want.to_string());
        if let Some(rest) = want_s.strip_prefix('~') {
            if rest == "menu" {
                if !is_menu_world(got.as_deref()) {
                    return false;
                }
            } else if !got.unwrap_or_default().to_lowercase().contains(&rest.to_lowercase()) {
                return false;
            }
        } else if matches!(k.as_str(), "arena" | "to" | "from") && ev_name(e) != "phase" {
            if norm_map(got.as_deref()) != norm_map(Some(&want_s)) {
                return false; // map names: Map_Arena_X == X
            }
        } else if got.as_deref() != Some(want_s.as_str()) {
            return false;
        }
    }
    true
}

fn phase_of_obs(state: &str) -> Option<&'static str> {
    Some(match state {
        "lobby" => "Lobby",
        "countdown" => "Countdown",
        "live" => "Live",
        "roundover" => "RoundOver",
        "match_over" => "PostMatch",
        "paused" => "Paused",
        _ => return None,
    })
}

/// Events matching `step` since `since_ms`; for who=each/others/<n> every listed
/// instance must have one. Fallbacks for events not yet instrumented:
/// lobby_ready <- observed session status "connected"; phase <- observed session match (sidecar tap).
pub fn match_events(evs: &[Ev], step: &Value, n: usize, since_ms: i64) -> Option<Vec<Ev>> {
    let names: Vec<String> = match &step["ev"] {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Value::String(x) => vec![x.clone()],
        _ => vec![],
    };
    let who = step["who"].as_str().map(String::from).unwrap_or_else(|| step["who"].as_u64().map(|x| x.to_string()).unwrap_or("each".into()));
    let has_server_phase = evs.iter().any(|e| ev_name(e) == "phase" && inst(e) == "server" && util::src(e) != "observed");
    // The observed session status "connected" stands in for lobby_ready ONLY for an
    // instance that never emitted a real lobby_ready in the run (not instrumented). Once it
    // has, a sidecar reconnect (server_restart) must not satisfy the wait before the Director
    // is back in the lobby (same rule as the phase fallback and has_server_phase).
    let real_lobby: std::collections::HashSet<String> = evs
        .iter()
        .filter(|e| ev_name(e) == "lobby_ready" && !matches!(util::src(e), "observed" | "harness"))
        .map(|e| inst(e).to_string())
        .collect();
    let mut hits: Vec<Ev> = vec![];
    for e in evs {
        let src = util::src(e);
        let slack = if matches!(src, "inst" | "ue4ss" | "observed") { 1000 } else { 0 };
        if wall(e) < since_ms - slack {
            continue;
        }
        let name = ev_name(e);
        let mut cand: Option<Ev> = None;
        if names.iter().any(|x| x == name) && src != "harness" {
            cand = Some(e.clone());
        } else if names.iter().any(|x| x == "lobby_ready") && name == "obs_sidecar" && s(e, "status") == Some("connected")
            && !real_lobby.contains(inst(e))
        {
            cand = Some(e.clone());
        } else if names.iter().any(|x| x == "phase") && name == "obs_match" && !has_server_phase {
            if let Some(ph) = s(e, "state").and_then(phase_of_obs) {
                let mut x = e.clone();
                x.insert("ev".into(), json!("phase"));
                x.insert("to".into(), json!(ph));
                x.insert("inst".into(), json!("server"));
                cand = Some(x);
            }
        }
        let Some(c) = cand else { continue };
        if !where_ok(&c, &step["where"]) {
            continue;
        }
        if who == "server" && inst(&c) != "server" {
            continue;
        }
        hits.push(c);
    }
    if who == "server" || who == "any" {
        return hits.into_iter().next().map(|h| vec![h]);
    }
    let want = resolve_instances(&who, n);
    let mut got: Vec<Ev> = vec![];
    for w in &want {
        match hits.iter().find(|e| inst(e) == w) {
            Some(e) => got.push(e.clone()),
            None => return None,
        }
    }
    Some(got)
}

/// Blocking wait. Returns (exit code, message): 0 matched / none seen, 1 timeout / unexpected.
pub fn run(run_dir: &Path, step: &Value, since_ms: i64) -> (i32, String) {
    let cfg = events::load_run(run_dir);
    let n = cfg["instances"].as_u64().unwrap_or(2) as usize;
    let negate = step["do"].as_str() == Some("expect_none");
    let timeout = step["timeout_s"].as_f64().or_else(|| step["window_s"].as_f64()).unwrap_or(60.0);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let evs = events::gather(run_dir, &cfg);
        if negate {
            let mut st = step.clone();
            st["who"] = json!("any");
            if let Some(hit) = match_events(&evs, &st, n, since_ms) {
                return (1, json!({"unexpected": hit}).to_string());
            }
        } else if let Some(hit) = match_events(&evs, step, n, since_ms) {
            return (0, json!({"matched": hit}).to_string());
        }
        if Instant::now() >= deadline {
            return if negate {
                (0, json!({"none_seen": step["ev"], "window_s": timeout}).to_string())
            } else {
                (1, json!({"timeout": step, "since_ms": since_ms}).to_string())
            };
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev(v: Value) -> Ev {
        v.as_object().unwrap().clone()
    }
    #[test]
    fn each_needs_every_instance() {
        let evs = vec![
            ev(json!({"ev":"lobby_ready","inst":"1","src":"inst","wall_ms":1000})),
            ev(json!({"ev":"obs_sidecar","inst":"2","src":"observed","status":"connected","wall_ms":1500})),
        ];
        let st = json!({"do":"wait","ev":"lobby_ready","who":"each"});
        assert!(match_events(&evs, &st, 2, 0).is_some());
        assert!(match_events(&evs, &st, 3, 0).is_none());
        assert!(match_events(&evs, &st, 2, 5000).is_none());
    }
    /// server_restart: after a restart the sidecar reconnects (`obs_sidecar connected`)
    /// before the Director is back in the lobby. For an instance that has emitted a real
    /// lobby_ready in the run, only a new lobby_ready satisfies the wait.
    #[test]
    fn observed_connected_is_no_lobby_ready_for_an_instrumented_instance() {
        let evs = vec![
            ev(json!({"ev":"lobby_ready","inst":"1","src":"inst","wall_ms":1000})),
            ev(json!({"ev":"lobby_ready","inst":"2","src":"inst","wall_ms":1100})),
            ev(json!({"ev":"obs_sidecar","inst":"1","src":"observed","status":"connected","wall_ms":9000})),
            ev(json!({"ev":"obs_sidecar","inst":"2","src":"observed","status":"connected","wall_ms":9100})),
        ];
        let st = json!({"do":"wait","ev":"lobby_ready","who":"each"});
        assert!(match_events(&evs, &st, 2, 8000).is_none(), "a reconnect is not the lobby");
        let mut more = evs.clone();
        more.push(ev(json!({"ev":"lobby_ready","inst":"1","src":"inst","wall_ms":12000})));
        more.push(ev(json!({"ev":"lobby_ready","inst":"2","src":"inst","wall_ms":12500})));
        assert!(match_events(&more, &st, 2, 8000).is_some());
    }
    #[test]
    fn where_maps_and_menu() {
        let evs = vec![ev(json!({"ev":"world_ready","inst":"2","src":"inst","arena":"Map_Arena_Cellar","wall_ms":10})),
                       ev(json!({"ev":"travel","inst":"2","src":"inst","to":"Map_Menu_Startup","wall_ms":20}))];
        assert!(match_events(&evs, &json!({"ev":"world_ready","who":"2","where":{"arena":"Cellar"}}), 2, 0).is_some());
        assert!(match_events(&evs, &json!({"ev":"world_ready","who":"2","where":{"arena":"Pit"}}), 2, 0).is_none());
        assert!(match_events(&evs, &json!({"ev":"travel","who":"others","where":{"to":"~menu"}}), 2, 0).is_some());
    }
    #[test]
    fn phase_falls_back_to_observed_only_without_server_events() {
        let obs = ev(json!({"ev":"obs_match","inst":"1","src":"observed","state":"live","wall_ms":50}));
        let st = json!({"ev":"phase","who":"server","where":{"to":"Live"}});
        assert!(match_events(&[obs.clone()], &st, 2, 0).is_some());
        let srv = ev(json!({"ev":"phase","inst":"server","src":"serverlog","to":"Lobby","wall_ms":40}));
        assert!(match_events(&[obs, srv], &st, 2, 0).is_none());
    }
}

//! Scenario definitions (data in `scenarios.json`) and expansion into flat steps.
//!
//! Placeholders: {arena} {seat} {best_of} {round}. `foreach_arena` / `foreach_round`
//! blocks are unrolled. Instance selectors: "each", "any", "server", "last", "others", "<n>".

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

pub const SCENARIOS_JSON: &str = include_str!("scenarios.json");

pub fn data() -> Value {
    serde_json::from_str(SCENARIOS_JSON).expect("scenarios.json is valid JSON")
}

pub fn arenas_all() -> Vec<String> {
    data()["arenas_all"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

pub fn resolve_instances(spec: &str, n: usize) -> Vec<String> {
    match spec {
        "each" | "all" | "*" => (1..=n).map(|i| i.to_string()).collect(),
        "last" => vec![n.to_string()],
        "others" => (2..=n).map(|i| i.to_string()).collect(),
        other => vec![other.to_string()],
    }
}

fn subst(v: &Value, ctx: &[(String, String)]) -> Value {
    match v {
        Value::String(s) => {
            let mut out = s.clone();
            for (k, val) in ctx {
                out = out.replace(&format!("{{{k}}}"), val);
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, ctx)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), subst(x, ctx))).collect()),
        other => other.clone(),
    }
}

fn with(ctx: &[(String, String)], k: &str, v: String) -> Vec<(String, String)> {
    let mut c: Vec<(String, String)> = ctx.iter().filter(|(a, _)| a != k).cloned().collect();
    c.push((k.to_string(), v));
    c
}

/// Expand a scenario. `arenas`: None = scenario default, "all", or a comma list.
pub fn expand(name: &str, instances: usize, arenas: Option<&str>, rounds: Option<u32>, mode: &str) -> Result<Value> {
    let d = data();
    let mut name = name.to_string();
    if mode == "autotest" {
        if let Some(sg) = d["stopgap_for"].get(&name).and_then(|v| v.as_str()) {
            name = sg.to_string();
        }
    }
    let scs = d["scenarios"].as_object().unwrap();
    let sc = scs.get(&name).ok_or_else(|| {
        anyhow!("unknown scenario {name}; known: {}", scs.keys().cloned().collect::<Vec<_>>().join(", "))
    })?;
    let steps_v = match &sc["steps"] {
        Value::String(other) => scs[other.as_str()]["steps"].clone(),
        v => v.clone(),
    };
    let arena_list: Vec<String> = match arenas.filter(|a| !a.is_empty() && *a != "default") {
        Some("all") => arenas_all(),
        Some(list) => list.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        None => match &sc["arenas"] {
            Value::String(a) if a == "all" => arenas_all(),
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            _ => vec!["Alley".into()],
        },
    };
    let rounds = rounds.filter(|r| *r > 0).unwrap_or_else(|| sc["rounds"].as_u64().unwrap_or(1) as u32);
    let mut flat: Vec<Value> = vec![];

    fn walk(list: &Value, ctx: &[(String, String)], arenas: &[String], rounds: u32, n: usize, out: &mut Vec<Value>) {
        for st in list.as_array().into_iter().flatten() {
            if let Some(inner) = st.get("foreach_arena") {
                for a in arenas {
                    walk(inner, &with(ctx, "arena", a.clone()), arenas, rounds, n, out);
                }
            } else if let Some(inner) = st.get("foreach_round") {
                for r in 1..=rounds {
                    let seat = ((r as usize - 1) % n) + 1;
                    let c = with(&with(ctx, "round", r.to_string()), "seat", seat.to_string());
                    walk(inner, &c, arenas, rounds, n, out);
                }
            } else {
                let mut s = subst(st, ctx);
                for k in ["inst", "who"] {
                    if s.get(k).and_then(|v| v.as_str()) == Some("last") {
                        s[k] = json!(n.to_string());
                    }
                }
                out.push(s);
            }
        }
    }
    let best_of = std::cmp::max(1, 2 * rounds as i64 - 1).to_string();
    walk(&steps_v, &[("best_of".into(), best_of)], &arena_list, rounds, instances, &mut flat);

    let blocked: Vec<Value> = if mode == "autotest" {
        flat.iter()
            .filter(|s| matches!(s["do"].as_str(), Some("rcon" | "client_cmd" | "netsim" | "restart")))
            .map(|s| s.get("cmd").cloned().unwrap_or_else(|| s["do"].clone()))
            .collect()
    } else {
        vec![]
    };
    let mut env = Map::new();
    if let Some(e) = sc.get("env").and_then(|v| v.as_object()) {
        for (key, kv) in e {
            let targets = if ["last", "others", "each", "*"].contains(&key.as_str()) {
                resolve_instances(key, instances)
            } else {
                vec![key.clone()]
            };
            for t in targets {
                let slot = env.entry(t).or_insert_with(|| json!({}));
                for (k, v) in kv.as_object().into_iter().flatten() {
                    slot[k] = v.clone();
                }
            }
        }
    }
    Ok(json!({
        "name": name,
        "doc": sc.get("doc").cloned().unwrap_or(json!("")),
        "dod": sc["dod"].clone(),
        "extra_rules": sc.get("extra_rules").cloned().unwrap_or(json!([])),
        "topology": sc["topology"].clone(),
        "netsim": sc.get("netsim").cloned().unwrap_or(json!("typical")),
        "arenas": arena_list,
        "rounds": rounds,
        "requires": sc.get("requires").cloned().unwrap_or(json!([])),
        "stopgap": sc.get("stopgap").and_then(|v| v.as_bool()).unwrap_or(false),
        // soak limits (SOAK-* rules) and the memory sampler interval; null for other scenarios
        "soak": sc.get("soak").cloned().unwrap_or(Value::Null),
        "env": env,
        "steps": flat,
        "unsupported_in_mode": blocked,
    }))
}

pub fn list() -> Vec<(String, String, String)> {
    let d = data();
    d["scenarios"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v["dod"].to_string(), v["doc"].as_str().unwrap_or("").to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_scenario_ends_with_quit() {
        for (name, _, _) in list() {
            let sc = expand(&name, 2, None, None, "rcon").unwrap();
            assert_eq!(sc["steps"].as_array().unwrap().last().unwrap()["do"], "quit_all", "{name}");
        }
    }
    #[test]
    fn p0_gate_has_70_alternating_kills() {
        let sc = expand("p0_gate", 2, Some("all"), Some(10), "rcon").unwrap();
        let kills: Vec<String> = sc["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["cmd"].as_str())
            .filter(|c| c.starts_with("DEBUG KILL"))
            .map(String::from)
            .collect();
        assert_eq!(kills.len(), 70);
        assert_eq!(&kills[..4], &["DEBUG KILL 1", "DEBUG KILL 2", "DEBUG KILL 1", "DEBUG KILL 2"]);
        assert!(sc["steps"].as_array().unwrap().iter().any(|s| s["cmd"] == "BESTOF 19"));
    }
    #[test]
    fn soak_scenarios_expand() {
        let sc = expand("soak_60m", 2, None, None, "rcon").unwrap();
        let steps = sc["steps"].as_array().unwrap();
        let kills = steps.iter().filter(|s| s["cmd"].as_str().map(|c| c.starts_with("DEBUG KILL")).unwrap_or(false)).count();
        assert_eq!(kills, 90);
        let blackouts: Vec<&str> = steps.iter().filter(|s| s["do"] == "netsim" && s["mode"] == "blackout").map(|s| s["inst"].as_str().unwrap()).collect();
        assert_eq!(blackouts.len(), 90);
        assert_eq!(&blackouts[..3], &["1", "2", "1"]);
        assert_eq!(sc["soak"]["mem_sample_s"], 30);
        assert_eq!(sc["soak"]["mem_slope_max_mb_per_h"], 150);
        assert_eq!(sc["soak"]["mem_growth_max_mb"], 400);
        assert!(sc["extra_rules"].as_array().unwrap().iter().any(|r| r == "SOAK-CRASH"));
        // every arena at least once
        let arenas: std::collections::HashSet<&str> = sc["arenas"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        for a in arenas_all() {
            assert!(arenas.contains(a.as_str()), "{a}");
        }
        let q = expand("soak_10m", 2, None, None, "rcon").unwrap();
        let qk = q["steps"].as_array().unwrap().iter().filter(|s| s["cmd"].as_str().map(|c| c.starts_with("DEBUG KILL")).unwrap_or(false)).count();
        assert_eq!(qk, 14);
        assert_eq!(q["soak"]["mem_warmup_min"], 3);
        assert!(expand("p0_gate", 2, None, None, "rcon").unwrap()["soak"].is_null());
    }
    /// scripts/scenarios/<name>.json must match the compiled-in scenarios.json.
    #[test]
    fn scenario_spec_files_match_compiled_data() {
        let repo = hsmp_tools::paths::repo_root().unwrap();
        let dir = repo.join("scripts").join("scenarios");
        let d = data();
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            for (name, sc) in v["scenarios"].as_object().into_iter().flatten() {
                assert_eq!(&d["scenarios"][name], sc, "{} differs from scenarios.json[{name}]", p.display());
                n += 1;
            }
        }
        assert!(n >= 2, "no scenario spec files under {}", dir.display());
    }
    #[test]
    fn autotest_falls_back_to_stopgap() {
        let sc = expand("p0_gate", 2, None, None, "autotest").unwrap();
        assert_eq!(sc["name"], "p0_smoke_autotest");
        assert!(sc["unsupported_in_mode"].as_array().unwrap().is_empty());
        assert_eq!(sc["env"]["1"]["HSMP_AUTOTEST"], "1");
        assert_eq!(sc["env"]["2"]["HSMP_AUTOTEST"], "join");
        let r = expand("reconnect", 2, None, None, "autotest").unwrap();
        assert!(!r["unsupported_in_mode"].as_array().unwrap().is_empty());
    }
}

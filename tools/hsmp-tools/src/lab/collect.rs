//! Incremental line readers retain partial writes and byte offsets. Source lines
//! accompany every sample. Legacy probe pairing is deliberately conservative:
//! ambiguous input signatures are excluded, never assigned by arrival order.
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Evidence {
    pub file: String,
    pub line: u64,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sample {
    pub value: f64,
    pub evidence: Evidence,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Attacker {
    pub accepted: u64,
    pub rejected: u64,
    pub reasons: BTreeMap<String, u64>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Summary {
    pub schema: u32,
    pub recipe: String,
    pub run: String,
    pub commit: String,
    pub metrics: BTreeMap<String, Vec<Sample>>,
    pub attackers: BTreeMap<String, Attacker>,
    pub reasons: BTreeMap<String, Vec<Evidence>>,
    pub pairs: BTreeMap<String, Vec<[f64; 2]>>,
    pub pair_evidence: Vec<[Evidence; 2]>,
    pub counters: BTreeMap<String, u64>,
    pub limitations: Vec<String>,
}
impl Summary {
    pub fn values(&self, key: &str) -> Vec<f64> {
        self.metrics
            .get(key)
            .into_iter()
            .flatten()
            .map(|s| s.value)
            .collect()
    }
    pub fn sample(&mut self, key: impl Into<String>, value: f64, e: &Evidence) {
        if value.is_finite() {
            self.metrics.entry(key.into()).or_default().push(Sample {
                value,
                evidence: e.clone(),
            });
        }
    }
    pub fn count(&mut self, key: &str) {
        *self.counters.entry(key.into()).or_default() += 1;
    }
}

#[derive(Default)]
struct Cursor {
    offset: u64,
    line: u64,
}
#[derive(Clone)]
struct Blow {
    e: Evidence,
    fields: BTreeMap<String, f64>,
    claim: Option<String>,
}
pub struct Collector {
    cursors: BTreeMap<PathBuf, Cursor>,
    decisions: BTreeSet<String>,
    peer_attackers: BTreeMap<u32, BTreeSet<u32>>,
    events_seen: BTreeSet<String>,
    probes: BTreeMap<String, Vec<Blow>>,
    replays: BTreeMap<String, Vec<Blow>>,
    exact_probes: BTreeMap<String, Vec<Blow>>,
    exact_replays: BTreeMap<String, Vec<Blow>>,
    arena: BTreeMap<String, String>,
    decision: Regex,
    field: Regex,
    blow: Regex,
    replay_id: Regex,
    delta: Regex,
    pub summary: Summary,
}
impl Collector {
    pub fn new(run: &Path, recipe: &str, commit: &str) -> Self {
        Self { cursors:BTreeMap::new(),decisions:BTreeSet::new(),peer_attackers:BTreeMap::new(),events_seen:BTreeSet::new(),probes:BTreeMap::new(),replays:BTreeMap::new(),exact_probes:BTreeMap::new(),exact_replays:BTreeMap::new(),arena:BTreeMap::new(),
            decision:Regex::new(r"attacker(?:_id)?=(\d+) target=(\d+) hit_id=(\d+)").unwrap(),
            field:Regex::new(r"([a-z_]+)=(-?\d+(?:\.\d+)?)").unwrap(),
            blow:Regex::new(r"(?:PROBE native on peer (\d+)|HIT from peer (\d+) \(#\d+\)) bone=(\S+) vel=(-?\d+) rig=(-?[\d.]+) cut=(-?\d+) stab=(-?[\d.]+): (.*)").unwrap(),
            replay_id:Regex::new(r"HIT from peer (\d+) \(#(\d+)\)").unwrap(),
            delta:Regex::new(r"([A-Za-z_][A-Za-z_ ]*?)([+-]\d+(?:\.\d+)?)").unwrap(),
            summary:Summary{schema:1,recipe:recipe.into(),run:run.display().to_string(),commit:commit.into(),limitations:vec!["Legacy PROBE pairing uses unique rounded input signatures in a two-player run; ambiguous signatures are excluded. It cannot prove causality or continuation parity.".into(),"Per-hit bootstrap intervals are exploratory: blows within a round are correlated. Use independent repeated experiments for acceptance.".into()],..Default::default()}
        }
    }
    /// Prime offsets before the Live experiment boundary; old session evidence
    /// must not leak into a later recipe.
    pub fn prime(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.poll(paths)?;
        let s = &self.summary;
        let fresh = Self::new(Path::new(&s.run), &s.recipe, &s.commit);
        self.summary = fresh.summary;
        self.decisions.clear();
        self.peer_attackers.clear();
        self.probes.clear();
        self.replays.clear();
        self.exact_probes.clear();
        self.exact_replays.clear();
        Ok(())
    }
    pub fn poll(&mut self, paths: &[PathBuf]) -> Result<usize> {
        let mut lines = Vec::new();
        for path in paths {
            if !path.exists() {
                continue;
            }
            let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
            let cur = self.cursors.entry(path.clone()).or_default();
            if f.metadata()?.len() < cur.offset {
                cur.offset = 0;
                cur.line = 0;
                self.summary.count("log_truncated");
                self.summary.limitations.push(format!(
                    "{} truncated; previously collected samples retained",
                    path.display()
                ));
            }
            let mut r = BufReader::new(f);
            r.seek(SeekFrom::Start(cur.offset))?;
            loop {
                let mut line = String::new();
                let n = r.read_line(&mut line)?;
                if n == 0 || !line.ends_with('\n') {
                    break;
                }
                cur.offset += n as u64;
                cur.line += 1;
                lines.push(Evidence {
                    file: path.display().to_string(),
                    line: cur.line,
                    text: line.trim_end().into(),
                });
            }
        }
        let n = lines.len();
        for e in lines {
            self.ingest(e);
        }
        Ok(n)
    }
    pub fn ingest(&mut self, e: Evidence) {
        let text = &e.text;
        // Shared UE4SS echoes JSONL too. Collect structured events only from
        // instance files, never both sinks (would double pose samples).
        if text.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(text) {
                self.event(&v, &e);
            } else {
                self.summary.count("malformed_json");
            }
            return;
        }
        if let Some(c) = self.decision.captures(text) {
            let (a, t, h) = (&c[1], &c[2], &c[3]);
            let accept = text.contains("damage accepted");
            let reject = text.contains("damage rejected");
            if let (Ok(attacker), Ok(victim)) = (a.parse::<u32>(), t.parse::<u32>()) {
                self.peer_attackers
                    .entry(victim)
                    .or_default()
                    .insert(attacker);
            }
            let delivery = text.contains("stage=\"delivery\"");
            if (accept || reject) && !delivery && self.decisions.insert(format!("{a}:{t}:{h}")) {
                let row = self.summary.attackers.entry(a.into()).or_default();
                if accept {
                    row.accepted += 1;
                } else {
                    row.rejected += 1;
                    let reason = text
                        .split("reason=")
                        .nth(1)
                        .unwrap_or("reason missing")
                        .trim_matches('"')
                        .to_string();
                    *row.reasons.entry(reason.clone()).or_default() += 1;
                    self.summary
                        .reasons
                        .entry(reason)
                        .or_default()
                        .push(e.clone());
                }
                self.summary
                    .sample("accept", if accept { 1.0 } else { 0.0 }, &e);
                self.summary.sample(
                    format!("attacker.{a}.accept"),
                    if accept { 1.0 } else { 0.0 },
                    &e,
                );
                if accept
                    || text.split("reason=").nth(1).unwrap_or("").trim_matches('"') != "parried"
                {
                    self.summary
                        .sample("honest_accept", if accept { 1.0 } else { 0.0 }, &e);
                }
            }
        }
        let f: BTreeMap<_, _> = self
            .field
            .captures_iter(text)
            .filter_map(|c| Some((c[1].to_string(), c[2].parse::<f64>().ok()?)))
            .collect();
        if text.contains("LAB_PROBE ") || text.contains("LAB_REPLAY ") {
            if let (Some(a), Some(cid)) = (f.get("attacker"), f.get("cid")) {
                let key = format!("{}:{}", *a as u32, *cid as u32);
                let b = Blow {
                    e: e.clone(),
                    fields: self.changes(text),
                    claim: Some(key.clone()),
                };
                let probe = text.contains("LAB_PROBE ");
                self.summary.count(if probe {
                    "exact_native_samples"
                } else {
                    "exact_replay_samples"
                });
                if !b.fields.contains_key("Health") {
                    self.summary.count("exact_health_unavailable");
                }
                if probe {
                    self.exact_probes.entry(key).or_default().push(b);
                } else {
                    self.exact_replays.entry(key).or_default().push(b);
                }
            } else {
                self.summary.count("malformed_exact_probe");
            }
            return;
        }
        if text.contains("proxy box frame differs") {
            if let Some(&v) = f.get("center_cm") {
                self.summary.sample("proxy.center_cm", v, &e);
            }
            if let Some(&v) = f.get("rotation_dot") {
                self.summary.sample(
                    "proxy.rotation_deg",
                    2.0 * v.abs().clamp(0.0, 1.0).acos().to_degrees(),
                    &e,
                );
            }
        }
        if text.contains("impact rescale") {
            if let Some(&v) = f.get("factor") {
                self.summary.sample("calibration.factor", v, &e);
            }
            self.summary.count("calibration_samples");
        }
        for (needle, key) in [
            ("INSIDE_JOURNAL", "inside.native"),
            ("INSIDE forwarded", "inside.forwarded"),
            ("no claimed parent", "inside.orphaned"),
            ("stuck blade accepted", "inside.accepted"),
            ("stuck blade delivered", "inside.delivered"),
            ("stuck blade rejected", "inside.rejected"),
            ("pose peer", "pose.diagnostic"),
            ("Lua Error", "lua_errors"),
            ("twisted against a joint", "stall"),
            ("clkr", "clock_reset_log"),
        ] {
            if text.contains(needle) {
                self.summary.count(key);
                self.summary
                    .reasons
                    .entry(key.into())
                    .or_default()
                    .push(e.clone());
            }
        }
        if text.contains("CACHED HIT") {
            return;
        }
        if let Some(c) = self.blow.captures(text) {
            let probe = c.get(1).is_some();
            let peer = c
                .get(1)
                .or_else(|| c.get(2))
                .unwrap()
                .as_str()
                .parse::<u32>()
                .unwrap_or(0);
            if peer == 0 {
                self.summary.count("pair_unsupported_peer");
                return;
            }
            let key = format!("{peer}:{}:{}:{}:{}:{}", &c[3], &c[4], &c[5], &c[6], &c[7]);
            let b = Blow {
                e: e.clone(),
                fields: self.changes(&c[8]),
                claim: if probe {
                    None
                } else {
                    self.replay_id
                        .captures(text)
                        .map(|id| format!("{}:{}", &id[1], &id[2]))
                },
            };
            if probe {
                self.probes.entry(key).or_default().push(b);
            } else {
                self.replays.entry(key).or_default().push(b);
            }
            self.summary
                .count(if probe { "probe_blows" } else { "replay_blows" });
        }
    }
    fn changes(&self, text: &str) -> BTreeMap<String, f64> {
        let mut fields = BTreeMap::new();
        if let Some(h) = text
            .split("dmg Health ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
        {
            fields.insert("Health".into(), h);
        }
        for d in self
            .delta
            .captures_iter(text.split('[').nth(1).unwrap_or(""))
        {
            let name = d[1].trim();
            if name != "Health" {
                if let Ok(v) = d[2].parse() {
                    fields.insert(name.into(), v);
                }
            }
        }
        fields
    }
    fn event(&mut self, v: &Value, e: &Evidence) {
        if v.get("seq").is_some()
            && !self.events_seen.insert(format!(
                "{}:{}:{}:{}:{}",
                v["inst"], v["mod"], v["seq"], v["wall_ms"], v["ev"]
            ))
        {
            return;
        }
        let inst = v["inst"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v["inst"].to_string());
        let ev = v["ev"].as_str().unwrap_or("");
        if ev == "world_ready" {
            if let Some(a) = v["arena"].as_str() {
                self.arena.insert(inst.clone(), a.into());
            }
        }
        if ev == "phase" {
            if let Some(a) = v["arena"].as_str() {
                self.arena.insert("server".into(), a.into());
            }
        }
        let arena = self
            .arena
            .get(&inst)
            .or_else(|| self.arena.get("server"))
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let peer = v["peer"].to_string();
        if matches!(
            ev,
            "pose_quality"
                | "netfeel"
                | "hitch"
                | "spawn_stretch"
                | "ipc_stats"
                | "pose_sender"
                | "x_pose_clock_reset"
        ) {
            for (k, val) in v.as_object().into_iter().flatten() {
                if matches!(
                    k.as_str(),
                    "wall_ms" | "t_ms" | "seq" | "v" | "peer" | "inst"
                ) {
                    continue;
                }
                if let Some(n) = val
                    .as_f64()
                    .filter(|n| *n >= 0.0 || ev == "x_pose_clock_reset")
                {
                    self.summary.sample(format!("{ev}.{k}"), n, e);
                    self.summary.sample(
                        format!("arena.{arena}.inst.{inst}.peer.{peer}.{ev}.{k}"),
                        n,
                        e,
                    );
                }
            }
            if ev == "netfeel" {
                if let (Some(n), Some(s)) = (
                    v["clock_resets"].as_f64(),
                    v["window_s"].as_f64().filter(|s| *s > 0.0),
                ) {
                    self.summary.sample("clock_resets_per_min", 60.0 * n / s, e);
                }
            }
        }
        if ev == "x_pose_clock_reset" {
            self.summary.count(ev);
            self.summary
                .reasons
                .entry(ev.into())
                .or_default()
                .push(e.clone());
        }
        if matches!(
            ev,
            "pawn_state"
                | "hit_probe"
                | "combat_quality"
                | "respawn"
                | "x_server_mods_loaded"
                | "x_server_mod_error"
        ) {
            self.summary.count(ev);
            self.summary
                .reasons
                .entry(ev.into())
                .or_default()
                .push(e.clone());
        }
    }
    pub fn finish(&mut self) {
        self.summary.pairs.clear();
        self.summary.pair_evidence.clear();
        let mut ambiguous = 0;
        let mut exact_pairs = 0;
        for (key, ps) in &self.exact_probes {
            let Some(rs) = self.exact_replays.get(key) else {
                continue;
            };
            if ps.len() != 1 || rs.len() != 1 {
                ambiguous += 1;
                continue;
            }
            let (p, r) = (&ps[0], &rs[0]);
            for (field, &n) in &p.fields {
                if let Some(&v) = r.fields.get(field) {
                    self.summary
                        .pairs
                        .entry(field.clone())
                        .or_default()
                        .push([n, v]);
                }
            }
            self.summary.pair_evidence.push([p.e.clone(), r.e.clone()]);
            exact_pairs += 1;
        }
        self.summary
            .counters
            .insert("exact_claim_pairs".into(), exact_pairs);
        for (k, ps) in &self.probes {
            let Some((target, signature)) = k.split_once(':') else {
                continue;
            };
            let Some(attackers) = target
                .parse::<u32>()
                .ok()
                .and_then(|t| self.peer_attackers.get(&t))
            else {
                continue;
            };
            if attackers.len() != 1 {
                ambiguous += 1;
                continue;
            }
            let key = format!("{}:{signature}", attackers.first().unwrap());
            let Some(rs) = self.replays.get(&key) else {
                continue;
            };
            if ps.len() != 1 || rs.len() != 1 {
                ambiguous += 1;
                continue;
            }
            let (p, r) = (&ps[0], &rs[0]);
            if r.claim
                .as_ref()
                .is_some_and(|k| self.exact_probes.contains_key(k))
            {
                continue;
            }
            // Only fields actually observed on both screens are paired.
            for (field, &n) in &p.fields {
                if let Some(&v) = r.fields.get(field) {
                    self.summary
                        .pairs
                        .entry(field.clone())
                        .or_default()
                        .push([n, v]);
                }
            }
            self.summary.pair_evidence.push([p.e.clone(), r.e.clone()]);
        }
        self.summary
            .counters
            .insert("pair_ambiguous_signatures".into(), ambiguous);
        let claims: Vec<_> = self
            .summary
            .attackers
            .values()
            .map(|a| a.accepted + a.rejected)
            .collect();
        if claims.len() == 2 {
            let hi = *claims.iter().max().unwrap();
            let lo = *claims.iter().min().unwrap();
            self.summary
                .counters
                .insert("claim_asymmetry_max".into(), hi);
            self.summary
                .counters
                .insert("claim_asymmetry_min".into(), lo);
        }
    }
}

pub fn sources(run: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![run.join("server.log"), run.join("server.jsonl")];
    let config = run.join("run.json");
    if config.exists() {
        let v: Value = serde_json::from_slice(&std::fs::read(config)?)?;
        if let Some(dirs) = v["state_dirs"].as_object() {
            for dir in dirs.values().filter_map(Value::as_str) {
                out.extend(event_files(Path::new(dir))?);
            }
        }
        if let Some(game) = v["game"].as_str() {
            out.push(Path::new(game).join("HalfswordUE5/Binaries/Win64/ue4ss/UE4SS.log"));
        }
    }
    let archived = run.join("UE4SS.log");
    if archived.exists() {
        out.retain(|p| !p.ends_with("UE4SS.log"));
        out.push(archived);
    }
    for entry in std::fs::read_dir(run)? {
        let p = entry?.path();
        if p.is_dir()
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("inst"))
        {
            out.extend(event_files(&p)?);
        }
    }
    out.sort_by(|a, b| {
        a.parent().cmp(&b.parent()).then_with(|| {
            let an = a.file_name().unwrap().to_string_lossy();
            let bn = b.file_name().unwrap().to_string_lossy();
            if an.starts_with("hsmp_events") && bn.starts_with("hsmp_events") {
                bn.cmp(&an)
            } else {
                an.cmp(&bn)
            }
        })
    });
    out.dedup();
    Ok(out)
}
fn event_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut files = vec![];
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        let n = p.file_name().unwrap().to_string_lossy();
        if n.starts_with("hsmp_events") && n.ends_with(".jsonl") {
            files.push(p);
        }
    }
    // oldest rotation first, current last, independent of filesystem order.
    files.sort_by_key(|p| std::cmp::Reverse(p.file_name().unwrap().to_string_lossy().to_string()));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev(text: &str, line: u64) -> Evidence {
        Evidence {
            file: "fixture".into(),
            line,
            text: text.into(),
        }
    }
    #[test]
    fn duplicate_cached_and_ambiguous_blows_do_not_inflate_parity() {
        let mut c = Collector::new(Path::new("fixture"), "sword-cloth", "abc");
        c.ingest(ev("damage accepted attacker_id=1 target=2 hit_id=3", 1));
        c.ingest(ev("damage accepted attacker_id=1 target=2 hit_id=3", 2));
        c.ingest(ev("PROBE native on peer 2 bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -2.00 [Health-2.0 Pain+1.0]",3));
        c.ingest(ev("HIT from peer 1 (#3) bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -1.00 [Health-1.0 Pain+0.5]",4));
        c.ingest(ev("CACHED HIT from peer 1 (#3) bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -1.00 [Health-1.0]",5));
        c.finish();
        assert_eq!(c.summary.attackers["1"].accepted, 1);
        assert_eq!(c.summary.pairs["Health"], vec![[-2.0, -1.0]]);
        c.ingest(ev("PROBE native on peer 2 bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -2.00 [Health-2.0]",6));
        c.finish();
        assert!(c.summary.pairs.is_empty());
        assert_eq!(c.summary.counters["pair_ambiguous_signatures"], 1);
    }
    #[test]
    fn exact_claim_pairs_take_precedence_and_preserve_tiny_health_changes() {
        let mut c = Collector::new(Path::new("fixture"), "sword-cloth", "abc");
        c.ingest(ev("damage accepted attacker_id=15 target=212 hit_id=37", 1));
        c.ingest(ev("PROBE native on peer 212 bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -0.00 [none]",2));
        c.ingest(ev("HIT from peer 15 (#37) bone=head vel=450 rig=0.50 cut=20 stab=0.00: dmg Health -0.00 [none]",3));
        c.ingest(ev(
            "LAB_PROBE attacker=15 cid=37 parent_cid=9 bone=head dmg Health -0.000120 [none]",
            4,
        ));
        c.ingest(ev(
            "LAB_REPLAY attacker=15 cid=37 parent_cid=9 bone=head dmg Health -0.000120 [none]",
            5,
        ));
        c.finish();
        assert_eq!(c.summary.pairs["Health"], vec![[-0.000120, -0.000120]]);
        assert_eq!(c.summary.counters["exact_claim_pairs"], 1);
        c.ingest(ev(
            "LAB_PROBE attacker=15 cid=37 parent_cid=9 bone=head dmg Health -0.000120 [none]",
            6,
        ));
        c.finish();
        assert!(
            c.summary.pairs.is_empty(),
            "ambiguous exact claims must not fall back to rounded matching"
        );
    }
    #[test]
    fn partial_line_is_retained_and_offsets_do_not_recount() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("hsmp-lab-{}", hsmp_ipc::shm::random_u64()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("server.log");
        std::fs::write(&p, "damage accepted attacker_id=1 target=2 hit_id=7").unwrap();
        let mut c = Collector::new(&dir, "fixture", "abc");
        assert_eq!(c.poll(std::slice::from_ref(&p)).unwrap(), 0);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f).unwrap();
        drop(f);
        assert_eq!(c.poll(std::slice::from_ref(&p)).unwrap(), 1);
        assert_eq!(c.poll(std::slice::from_ref(&p)).unwrap(), 0);
        assert_eq!(c.summary.attackers["1"].accepted, 1);
        std::fs::remove_file(p).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

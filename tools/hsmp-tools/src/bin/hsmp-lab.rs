//! Live lab owner/controller and portable evidence analysis.
use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use hsmp_tools::lab::{
    collect::{self, Collector, Summary},
    control::{self, Live},
    stats, Recipe,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(about = "Incremental native combat experiments; test results never certify solo parity")]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    /// Start one harness and keep both games alive. Run exp/ab in another terminal.
    Session {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 14400)]
        seconds: u64,
        #[arg(long)]
        fake_game: bool,
        #[arg(long, default_value = "[]")]
        server_args_json: String,
    },
    Stop {
        #[arg(long)]
        session: PathBuf,
    },
    /// Apply a committed recipe in a live session without restarting games.
    Exp {
        recipe: String,
        #[arg(long)]
        session: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Analyse existing raw evidence (no game required).
    Analyse {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        recipe: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Compare sample distributions with deterministic bootstrap delta intervals.
    Compare {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        current: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Append traceable regressions and suspected causes to the lab journal.
    Review {
        #[arg(long)]
        summary: PathBuf,
        #[arg(long)]
        baseline: Option<PathBuf>,
        #[arg(long, default_value = "test-results/lab/journal.jsonl")]
        journal: PathBuf,
    },
    /// Promote a measured summary; explicit acceptance is required. Never auto-promote.
    Baseline {
        #[arg(long)]
        summary: PathBuf,
        #[arg(long)]
        accept: bool,
    },
    /// In-session A/B/A/B recipe runs with one knob changed; compare same session noise.
    Ab {
        knob: String,
        recipe: String,
        #[arg(long)]
        session: PathBuf,
        #[arg(long)]
        a: f64,
        #[arg(long)]
        b: f64,
        #[arg(long, default_value_t = 2)]
        repeats: usize,
    },
}
fn load<T: serde::de::DeserializeOwned>(p: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(p).with_context(|| format!("read {}", p.display()))?)
        .context("parse JSON")
}
fn recipe(name: &str) -> Result<Recipe> {
    let p = if name.ends_with(".json") {
        PathBuf::from(name)
    } else {
        PathBuf::from("tools/hsmp-tools/lab").join(format!("{name}.json"))
    };
    let r: Recipe = load(&p)?;
    r.validate()?;
    Ok(r)
}
fn compare(a: &Summary, b: &Summary) -> Value {
    let keys: BTreeSet<_> = a.metrics.keys().chain(b.metrics.keys()).collect();
    let mut rows = vec![];
    for k in keys {
        let aa = a.values(k);
        let bb = b.values(k);
        rows.push(json!({"metric":k,"baseline_n":aa.len(),"current_n":bb.len(),"mean_delta":if k.ends_with("accept"){stats::difference(&aa,&bb,-1.0)}else{None},"median_delta":stats::difference(&aa,&bb,0.5),"p90_delta":stats::difference(&aa,&bb,0.9)}));
    }
    let paired: Vec<_> = b
        .pairs
        .iter()
        .map(|(k, p)| json!({"field":k,"ratio":stats::ratio(p)}))
        .collect();
    json!({"recipe":b.recipe,"baseline_commit":a.commit,"current_commit":b.commit,"metrics":rows,"paired_replay_over_native":paired,"limitations":b.limitations})
}
fn review(s: &Summary, baseline: Option<&Summary>, r: &Recipe) -> Value {
    let mut bounds = vec![];
    for (key, b) in &r.bounds {
        let xs = s.values(key);
        let pairs = key
            .strip_prefix("paired.")
            .and_then(|field| s.pairs.get(field));
        let n = pairs.map_or(xs.len(), Vec::len);
        let value = if let Some(ps) = pairs {
            stats::ratio(ps).map(|v| v.estimate)
        } else if key.ends_with("accept") {
            if n == 0 {
                None
            } else {
                Some(xs.iter().sum::<f64>() / n as f64)
            }
        } else {
            stats::quantile(&xs, 0.9)
        };
        let verdict = if n < b.min_samples || value.is_none() {
            "incomplete"
        } else if value
            .is_some_and(|v| b.min.is_some_and(|min| v < min) || b.max.is_some_and(|max| v > max))
        {
            "outside_bound"
        } else {
            "within_bound"
        };
        bounds.push(json!({"metric":key,"statistic":if pairs.is_some(){"paired_ratio"}else if key=="accept"{"mean"}else{"p90"},"value":value,"n":n,"bound":b,"verdict":verdict}));
    }
    let mut causes:Vec<_>=s.reasons.iter().map(|(reason,evs)|json!({"reason":reason,"count":evs.len(),"evidence":evs.iter().take(3).collect::<Vec<_>>()})).collect();
    causes.sort_by_key(|v| std::cmp::Reverse(v["count"].as_u64().unwrap_or(0)));
    causes.truncate(3);
    let new_reasons: Vec<_> = s
        .reasons
        .keys()
        .filter(|k| baseline.is_some_and(|a| !a.reasons.contains_key(*k)))
        .collect();
    let hi = s.counters.get("claim_asymmetry_max").copied().unwrap_or(0);
    let lo = s.counters.get("claim_asymmetry_min").copied().unwrap_or(0);
    json!({"at":control::now_ms(),"recipe":s.recipe,"commit":s.commit,"run":s.run,"bounds":bounds,"comparison":baseline.map(|a|compare(a,s)),"new_reasons":new_reasons,"top_suspected_causes":causes,"claim_asymmetry": {"max":hi,"min":lo,"ratio":if lo>0{Some(hi as f64/lo as f64)}else{None},"status":if hi>2*lo{"investigate_downed_time_before_attribution"}else{"no_large_count_asymmetry"}},"paired_fields":s.pairs.iter().map(|(k,p)|json!({"field":k,"n":p.len(),"ratio":stats::ratio(p)})).collect::<Vec<_>>(),"limitations":s.limitations,"live_parity_certified":false})
}
fn main() {
    if let Err(e) = run() {
        eprintln!("hsmp-lab: {e:#}");
        std::process::exit(2);
    }
}
fn run() -> Result<()> {
    match Args::parse().command {
        Cmd::Session {
            dir,
            seconds,
            fake_game,
            server_args_json,
        } => control::session(
            &std::env::current_dir()?,
            &dir,
            seconds,
            fake_game,
            &server_args_json,
        )?,
        Cmd::Stop { session } => {
            ensure!(session.join("ready.json").exists(), "unknown session");
            fs::write(session.join("stop"), b"owner requested stop\n")?;
        }
        Cmd::Exp {
            recipe: name,
            session,
            out,
        } => {
            let r = recipe(&name)?;
            let live = Live::attach(&session)?;
            let out = out.unwrap_or_else(|| {
                live.run
                    .join(format!("lab-{}-{}.json", r.name, control::now_ms()))
            });
            live.experiment(&r, &out)?;
            println!("{}", out.display());
        }
        Cmd::Analyse {
            run,
            recipe: name,
            out,
        } => {
            let _ = recipe(&name)?;
            let mut c = Collector::new(&run, &name, "unknown archived build (no deploy stamp)");
            if let Ok(v) = load::<Value>(&run.join("run.json")) {
                if let Some(h) = v["deploy"]["commit"].as_str() {
                    c.summary.commit = h.into();
                }
            }
            let start = std::time::Instant::now();
            let n = c.poll(&collect::sources(&run)?)?;
            c.finish();
            control::save(&out, &c.summary)?;
            println!(
                "{} lines analysed in {:.3}s: {}",
                n,
                start.elapsed().as_secs_f64(),
                out.display()
            );
        }
        Cmd::Compare {
            baseline,
            current,
            out,
        } => {
            let a: Summary = load(&baseline)?;
            let b: Summary = load(&current)?;
            ensure!(
                a.schema == b.schema && a.recipe == b.recipe,
                "incompatible baseline/recipe"
            );
            let v = compare(&a, &b);
            if let Some(p) = out {
                control::save(&p, &v)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        Cmd::Review {
            summary,
            baseline,
            journal,
        } => {
            let s: Summary = load(&summary)?;
            let r = recipe(&s.recipe)?;
            let b = baseline.as_deref().map(load::<Summary>).transpose()?;
            let v = review(&s, b.as_ref(), &r);
            control::journal(&journal, &v)?;
            control::save(&summary.with_extension("review.json"), &v)?;
            println!(
                "Review: {}",
                summary.with_extension("review.json").display()
            );
        }
        Cmd::Baseline { summary, accept } => {
            ensure!(
                accept,
                "promoting a baseline requires --accept after evidence review"
            );
            let s: Summary = load(&summary)?;
            let r = recipe(&s.recipe)?;
            let v = review(&s, None, &r);
            ensure!(
                v["bounds"].as_array().is_some_and(
                    |rs| !rs.is_empty() && rs.iter().all(|r| r["verdict"] == "within_bound")
                ),
                "incomplete/outside-bound evidence cannot become an accepted baseline"
            );
            let dest =
                PathBuf::from("test-results/lab/baseline").join(format!("{}.json", s.recipe));
            control::save(&dest, &s)?;
            println!(
                "Accepted baseline {} (stage explicitly; never includes credentials)",
                dest.display()
            );
        }
        Cmd::Ab {
            knob,
            recipe: name,
            session,
            a,
            b,
            repeats,
        } => {
            ensure!(
                a.is_finite() && b.is_finite() && (1..=10).contains(&repeats),
                "invalid A/B parameters"
            );
            let mut r = recipe(&name)?;
            let live = Live::attach(&session)?;
            let original = *r
                .tune
                .get(&knob)
                .context("A/B recipe must declare this knob's baseline value")?;
            let mut outs = vec![];
            let mut groups = [Vec::<Summary>::new(), Vec::<Summary>::new()];
            for i in 0..repeats {
                let order = if i % 2 == 0 {
                    [("a", a, 0), ("b", b, 1)]
                } else {
                    [("b", b, 1), ("a", a, 0)]
                };
                for (label, value, group) in order {
                    r.tune.insert(knob.clone(), value);
                    let p = live.run.join(format!(
                        "lab-ab-{}-{}-{i}-{label}-{}.json",
                        r.name,
                        knob,
                        control::now_ms()
                    ));
                    groups[group].push(live.experiment(&r, &p)?);
                    outs.push(p);
                }
            }
            for i in 0..2 {
                live.dev(i, &["tune", &knob, &original.to_string()])?;
            }
            let keys: BTreeSet<_> = groups
                .iter()
                .flatten()
                .flat_map(|s| s.metrics.keys())
                .collect();
            let comparisons:Vec<_>=keys.into_iter().map(|k|{
                let per_run=|g:&Vec<Summary>|g.iter().filter_map(|s|{
                    let xs=s.values(k);if k.ends_with("accept"){if xs.is_empty(){None}else{Some(xs.iter().sum::<f64>()/xs.len() as f64)}}else{stats::quantile(&xs,0.5)}
                }).collect::<Vec<_>>();
                json!({"metric":k,"independent_run_median_delta":stats::difference(&per_run(&groups[0]),&per_run(&groups[1]),0.5)})
            }).collect();
            control::save(
                &live.run.join("lab-ab.json"),
                &json!({"recipe":r.name,"knob":knob,"a":a,"b":b,"runs":outs,"comparisons":comparisons,"restored":original,"note":"AB then BA order; run-level bootstrap. Small repeat counts remain exploratory."}),
            )?;
            println!("{} A/B experiments complete", outs.len());
        }
    }
    Ok(())
}

//! `hsmp-tools lua-test [suite...] [--verbose] [-- suite args]`
//! Runs tools/hsmp-tools/lua-tests/<suite>.lua under mlua (Lua 5.4) with the
//! `T` harness (src/luatest.rs). No suite named = all suites.

use hsmp_tools::{luatest, paths};

#[derive(clap::Args)]
pub struct Args {
    /// Suites to run (file stems in lua-tests/); default: all
    suites: Vec<String>,
    /// Print every passing check too
    #[arg(short, long)]
    verbose: bool,
    /// List the suites and exit
    #[arg(long)]
    list: bool,
    /// Write suite names, timings and assertion counts to this JSON report
    #[arg(long)]
    json: Option<std::path::PathBuf>,
    /// Extra args handed to the suite(s) as `...` (e.g. pose: recorded pose_play files)
    #[arg(last = true)]
    extra: Vec<String>,
}

pub fn run(a: Args) -> anyhow::Result<i32> {
    let root = paths::repo_root()?;
    let all = luatest::suites(&root);
    if a.list {
        for s in &all {
            println!("{s}");
        }
        return Ok(0);
    }
    let mut chosen = if a.suites.is_empty() { all.clone() } else { a.suites.clone() };
    let mut seen=std::collections::HashSet::new();
    chosen.retain(|s|seen.insert(s.clone()));
    for s in &chosen {
        if !all.contains(s) { anyhow::bail!("no suite {s:?} (have: {})",all.join(", ")); }
    }
    let (mut tp, mut tf, mut failed_suites) = (0, 0, 0);
    let mut reports = Vec::new();
    for s in &chosen {
        let t0 = std::time::Instant::now();
        let c = luatest::run_suite(&root, s, &a.extra, a.verbose);
        let elapsed = t0.elapsed().as_secs_f64();
        println!(
            "{s}: {} checks passed, {} failed  ({:.1} s)",
            c.pass,
            c.fail,
            elapsed
        );
        failed_suites += usize::from(c.fail > 0);
        reports.push(serde_json::json!({"name":s,"assertions_passed":c.pass,"assertions_failed":c.fail,
            "seconds":elapsed,"failures":c.failures}));
        tp += c.pass;
        tf += c.fail;
    }
    let passed_suites = chosen.len() - failed_suites;
    println!("Lua suites: {passed_suites} passed, {failed_suites} failed; assertions: {tp} passed, {tf} failed");
    if let Some(path) = a.json {
        let report=serde_json::json!({"evidence_level":"offline Lua suites (mock engine)","suites":reports,
            "suites_passed":passed_suites,"suites_failed":failed_suites,"assertions_passed":tp,"assertions_failed":tf});
        if let Some(parent)=path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent)?; }
        std::fs::write(path,serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(if tf > 0 { 1 } else { 0 })
}

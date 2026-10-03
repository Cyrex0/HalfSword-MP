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
    let chosen = if a.suites.is_empty() { all.clone() } else { a.suites.clone() };
    let (mut tp, mut tf) = (0, 0);
    for s in &chosen {
        if !all.contains(s) {
            anyhow::bail!("no suite {s:?} (have: {})", all.join(", "));
        }
        let t0 = std::time::Instant::now();
        let c = luatest::run_suite(&root, s, &a.extra, a.verbose);
        println!(
            "{s}: {} checks passed, {} failed  ({:.1} s)",
            c.pass,
            c.fail,
            t0.elapsed().as_secs_f64()
        );
        tp += c.pass;
        tf += c.fail;
    }
    if chosen.len() > 1 {
        println!("total: {tp} checks passed, {tf} failed");
    }
    Ok(if tf > 0 { 1 } else { 0 })
}

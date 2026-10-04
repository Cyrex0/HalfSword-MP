//! `hsmp-gate`: the HSMP test gate.
//! Built with hsmp-tools: `cargo build --release -p hsmp-tools`
//! -> target/release/hsmp-gate.exe. Driven by scripts/mp_test.ps1 and
//! .githooks/pre-push; every subcommand also works standalone (docs/development/testing.md).
//!
//! * scenario     - scenario definitions (data: scenarios.json) and expansion into steps
//! * events       - evidence collection (hsmp_log JSONL, server events/log, UE4SS.log, harness)
//! * rules        - DoD-1..14 -> report.json + junit.xml; exit code = verdict
//! * wait         - live predicates for mp_test.ps1 (wait / expect_none)
//! * rcon         - RCON line-protocol client
//! * observe      - samples each sidecar tap's session status / match (observed.jsonl)
//! * memwatch     - soak memory sampler (private bytes / working set / handles) -> mem.jsonl
//! * soak         - SOAK-* rules (soak_60m / soak_10m) + the crash-triage evidence DoD-2 uses
//! * lint         - travel lint + kill-by-image-name grep
//! * contract     - check_events: real emitters vs the fields the gate reads (G0 `events`)
//! * g0           - the pre-push gate
//! * fixtures     - mocked runs + self-test
//! * fake_game    - shared-memory game stand-in driving the real sidecar/server (mp_test.ps1 -FakeGame)
//! * statefiles   - G0 state_files (check_no_state_files) + the STATE-1 matcher
//! * abdiff       - A/B comparison of two runs (baseline vs candidate)

mod abdiff;
mod contract;
mod events;
mod fake_game;
mod fixtures;
mod g0;
mod lint;
mod memwatch;
mod observe;
mod rcon;
mod rules;
mod scenario;
mod soak;
mod statefiles;
mod util;
mod wait;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "hsmp-gate", about = "HSMP gate: scenarios, waits, RCON, DoD assertions, G0")]
struct Cli {
    /// Repo root (default: HSMP_ROOT, else discovered from the cwd / exe).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Evaluate a run dir -> report.json + junit.xml. Exit 0 pass, 1 fail, 2 incomplete.
    Assert {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        junit: Option<PathBuf>,
        #[arg(long)]
        quiet: bool,
    },
    /// Print the expanded steps of a scenario as JSON.
    Scenario {
        name: String,
        #[arg(long, default_value_t = 2)]
        instances: usize,
        #[arg(long)]
        arenas: Option<String>,
        #[arg(long)]
        rounds: Option<u32>,
        #[arg(long, default_value = "rcon")]
        mode: String,
    },
    /// List scenarios.
    Scenarios,
    /// Block until a step's predicate holds (wait) or a window passes clean (expect_none).
    Wait {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        step: Option<String>,
        /// JSON step in a file (PowerShell 5.1 mangles quotes in native arguments)
        #[arg(long)]
        step_file: Option<PathBuf>,
        #[arg(long)]
        since_ms: i64,
    },
    /// Send one RCON command. Exit 0 OK, 1 ERR reply, 3 unreachable.
    Rcon {
        #[arg(long)]
        addr: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value_t = 5.0)]
        timeout: f64,
        #[arg(required = true)]
        words: Vec<String>,
    },
    /// Sample each instance's sidecar tap into <run>/observed.jsonl until stopped.
    Observe {
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value_t = 0)]
        parent_pid: u32,
    },
    /// Sample the harness-tracked processes' memory into <run>/mem.jsonl until stopped (soak).
    Memwatch {
        #[arg(long)]
        run: PathBuf,
        /// mp_test.ps1's pidfile ([{role,pid,name,start_ticks}]), re-read every tick
        #[arg(long)]
        pids: PathBuf,
        #[arg(long, default_value_t = 30.0)]
        interval_s: f64,
        #[arg(long, default_value_t = 0)]
        parent_pid: u32,
    },
    /// Evaluate scripts/fixtures/* against their expected.json.
    Selftest,
    /// Regenerate scripts/fixtures/*.
    MakeFixtures,
    /// Travel lint (level changes only in HSMPMatch).
    TravelLint {
        #[arg(long)]
        quiet: bool,
    },
    /// G0 pre-push gate.
    G0 {
        #[arg(long)]
        quick: bool,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,
        /// Report these checks as SKIP without running them (no G0 stamp is written)
        #[arg(long, value_delimiter = ',')]
        skip: Vec<String>,
        #[arg(long)]
        no_stamp: bool,
        /// Fail (instead of skip) the checks that need the game's UE4SS object dump
        /// (bp_names, unsafe U1/U2) when no dump is found.
        #[arg(long)]
        strict: bool,
    },
    /// Game stand-in for mp_test.ps1 -FakeGame (environment-driven).
    FakeGame,
    /// Event contract lint (G0 `events`): real emitters vs the fields the gate reads.
    CheckEvents {
        /// list every extracted emitter call site
        #[arg(long)]
        verbose: bool,
    },
    /// G0 `state_files` lint: list / check state-dir names and file writers; --update drops unused entries.
    StateFiles {
        #[arg(long)]
        update: bool,
        #[arg(long)]
        verbose: bool,
    },
    /// Compare two runs (baseline vs candidate): frame cost, pose sender, pose quality, hitches, ipc_stats.
    AbDiff {
        /// the baseline run dir
        #[arg(long)]
        a: PathBuf,
        /// the candidate run dir
        #[arg(long)]
        b: PathBuf,
        /// also write the comparison as JSON here
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Print the commit this gate was built from (JSON {commit, dirty}).
    Version,
}

fn main() {
    let cli = Cli::parse();
    // `--repo .` must be made absolute and normalised (no `.` / `..` components), or
    // paths built from it double up (`lua-tests\./tools/...`, os error 3) in G0's checks.
    // std::path::absolute, not canonicalize: no `\\?\` prefix for cargo / Lua / git.
    let repo = match cli.repo.clone().map(|p| std::path::absolute(&p).map_err(anyhow::Error::from)).unwrap_or_else(hsmp_tools::paths::repo_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };
    let code = match cli.cmd {
        Cmd::Assert { run, scenario, report, junit, quiet } => match rules::evaluate(&run, &repo, scenario.as_deref()) {
            Ok(rep) => {
                let rp = report.unwrap_or_else(|| run.join("report.json"));
                let jp = junit.unwrap_or_else(|| run.join("junit.xml"));
                let _ = util::write_json(&rp, &rep);
                let _ = std::fs::write(&jp, rules::junit(&rep));
                if !quiet {
                    rules::print_summary(&rep, &rp);
                }
                rules::exit_code(rep["verdict"].as_str().unwrap_or(""))
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Cmd::Scenario { name, instances, arenas, rounds, mode } => match scenario::expand(&name, instances, arenas.as_deref(), rounds, &mode) {
            Ok(v) => {
                println!("{v}");
                0
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Cmd::Scenarios => {
            for (n, dod, doc) in scenario::list() {
                println!("{n:20} DoD {dod:28} {doc}");
            }
            0
        }
        Cmd::Wait { run, step, step_file, since_ms } => {
            let text = match (step, step_file) {
                (Some(s), _) => s,
                (None, Some(f)) => util::read_text(&f).unwrap_or_default(),
                _ => {
                    eprintln!("error: --step or --step-file required");
                    std::process::exit(2);
                }
            };
            match serde_json::from_str(&text) {
                Ok(st) => {
                    let (code, msg) = wait::run(&run, &st, since_ms);
                    println!("{msg}");
                    code
                }
                Err(e) => {
                    eprintln!("error: bad step json: {e}");
                    2
                }
            }
        }
        Cmd::Rcon { addr, password, timeout, words } => match rcon::send(&addr, &password, &words.join(" "), Duration::from_secs_f64(timeout)) {
            Ok((ok, reply)) => {
                println!("{reply}");
                if ok { 0 } else { 1 }
            }
            Err(e) => {
                println!("ERR rcon unreachable: {e}");
                3
            }
        },
        Cmd::Observe { run, parent_pid } => {
            observe::run(&run, parent_pid);
            0
        }
        Cmd::Memwatch { run, pids, interval_s, parent_pid } => {
            memwatch::run(&run, &pids, interval_s, parent_pid);
            0
        }
        Cmd::Selftest => {
            let (n, bad) = fixtures::selftest(&repo, true);
            println!("selftest: {n} fixtures, {} failing", bad.len());
            if bad.is_empty() { 0 } else { 1 }
        }
        Cmd::MakeFixtures => {
            let d = fixtures::fixtures_dir(&repo);
            fixtures::make(&d);
            println!("fixtures written to {}", d.display());
            0
        }
        Cmd::TravelLint { quiet } => {
            let (ok, summary, detail) = lint::travel_lint(&repo);
            if !quiet {
                for d in &detail {
                    println!("{d}");
                }
            }
            println!("travel lint: {summary}");
            if ok { 0 } else { 1 }
        }
        Cmd::G0 { quick, json, only, skip, no_stamp, strict } => g0::run_g0(&repo, &g0::Opts { quick, json, only, skip, no_stamp, strict }),
        Cmd::FakeGame => fake_game::run(),
        Cmd::StateFiles { update, verbose } => statefiles::run_cli(&repo, update, verbose),
        Cmd::AbDiff { a, b, json } => abdiff::run(&a, &b, json.as_deref()),
        Cmd::CheckEvents { verbose } => contract::run_cli(&repo, verbose),
        Cmd::Version => {
            println!("{}", rules::gate_build());
            0
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    /// The evaluator agrees with every fixture's expected.json.
    #[test]
    fn fixtures_selftest() {
        let repo = hsmp_tools::paths::repo_root().unwrap();
        let (n, bad) = super::fixtures::selftest(&repo, true);
        assert!(n >= 10, "fixtures missing ({n})");
        assert!(bad.is_empty(), "{bad:?}");
    }
}

//! hsmpworld-sim [--profile NAME|all] [--seeds N] [--logs CLIENT]
//! Runs the mixed world-sync scenario and prints the report per profile.

use hsmpworld_sim::{net::Profile, report, sim};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let profiles: Vec<String> = match get("--profile").as_deref() {
        None | Some("all") => ["lan", "typical", "intl", "far", "bad"].iter().map(|s| s.to_string()).collect(),
        Some(p) => vec![p.to_string()],
    };
    let seeds: u64 = get("--seeds").and_then(|s| s.parse().ok()).unwrap_or(3);
    let logs: Option<usize> = get("--logs").and_then(|s| s.parse().ok());
    for p in profiles {
        let prof = Profile::get(&p);
        let runs: Vec<sim::Metrics> = (0..seeds).map(|seed| {
            sim::Sim::new(prof, sim::mixed(), 1000 + seed, logs.is_some()).run()
        }).collect();
        if let Some(c) = logs {
            if let Some(l) = runs[0].logs.get(c) { for line in l { println!("[c{}] {line}", c + 1); } }
        }
        report::print(&report::summarize(&p, &runs));
    }
}

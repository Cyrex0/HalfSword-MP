//! Netfeel table: how a remote fighter looks over each netsim profile
//! (`cargo test -p hsmp-pose --release --test netfeel -- --nocapture`).
//! `NETFEEL_OLD_LUA=1` models the game driver as it was before the smoothing change.

use hsmp_pose::feelsim::{run, Config, Report};

const PROFILES: [&str; 7] = ["lan", "typical", "intl", "bad", "far", "wifi", "awful"];

fn cfg(p: &str, secs: f64, seed: u64) -> Config {
    let mut c = Config::profile(p, secs, seed);
    if std::env::var("NETFEEL_OLD_LUA").is_ok_and(|v| v == "1") { c.lua_v2 = false; }
    c
}

fn table(seeds: u64, secs: f64) -> Vec<Report> {
    println!("{}", Report::header());
    let mut out = Vec::new();
    for p in PROFILES {
        for s in 1..=seeds {
            let r = run(&cfg(p, secs, s));
            println!("{}", r.row());
            out.push(r);
        }
    }
    out
}

#[test]
fn netfeel_table() {
    let _ = table(2, 60.0);
}

/// One profile, one seed: `NETFEEL_ONE=far FEELSIM_DEBUG=3 cargo test ... netfeel_one -- --nocapture`.
#[test]
fn netfeel_one() {
    let Ok(p) = std::env::var("NETFEEL_ONE") else { return };
    let r = run(&cfg(&p, 30.0, 1));
    println!("{}\n{}", Report::header(), r.row());
}

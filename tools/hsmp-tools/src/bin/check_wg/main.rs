//! check_wg: world-guard lint.
//!
//!     cargo run --release -p hsmp-tools --bin check_wg [-- --strict] [--sites]
//!
//! Rules W1 (guard present), W2 (module-level UObject cache reset on drop) and
//! W3 (deferred callback captures a UObject without a world check): see wg.rs.
//!
//! Exit 1 when a mod that uses the shared guard (`require "hsmp_wg"`) has a
//! finding. Findings in mods that still carry a legacy per-mod block or no
//! guard are printed as TODO notes; `--strict` makes them
//! failures too (the DoD-13 target once every mod is converted).
//! `--sites` lists the legacy call sites still to convert.
//! `--release-set` limits the scan to the mods `mods/mods.release.txt` enables; G0 runs
//! `--strict --release-set`.

mod wg;

use hsmp_tools::{lint, paths};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let strict = args.iter().any(|a| a == "--strict");
    let show_sites = args.iter().any(|a| a == "--sites");
    let root = match paths::repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("check_wg: {e:#}");
            std::process::exit(2);
        }
    };
    let files = match lint::LuaFile::load_all(&root, &paths::mod_script_files(&root)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("check_wg: {e:#}");
            std::process::exit(2);
        }
    };
    // --release-set: only the mods mods/mods.release.txt enables (`1` / `dev`);
    // G0 runs `check_wg --strict --release-set` so every shipped mod's TODO is a failure
    let release_set = args.iter().any(|a| a == "--release-set");
    let status = lint::release_mod_status(&root);
    if release_set && status.is_empty() {
        eprintln!("check_wg: --release-set but mods/mods.release.txt lists no mods");
        std::process::exit(2);
    }
    let all: Vec<(String, String, String)> = files.iter().map(|f| (f.mod_name(), f.rel.clone(), f.src.clone())).collect();
    let (input, skipped) = if release_set { release_filter(all, &status) } else { (all, vec![]) };
    let reps = wg::scan_mods(&input, strict);
    let mut rep = lint::Report::new("check_wg");
    if release_set {
        rep.note(format!("release set (mods/mods.release.txt){}; disabled mods skipped: {}", if strict { ", strict" } else { "" },
                         if skipped.is_empty() { "none".into() } else { skipped.join(",") }));
    }
    for m in &reps {
        if !m.touches && m.findings.is_empty() {
            continue;
        }
        let todo = m.findings.iter().filter(|f| !f.error).count();
        rep.note(format!(
            "{:<16} guard={:<7} findings={} todo={}{}",
            m.name,
            m.guard.label(),
            m.findings.len() - todo,
            todo,
            if m.guard == wg::Guard::Shared { "" } else { "   (convert to shared/hsmp_wg.lua: owner)" }
        ));
    }
    for m in &reps {
        for f in &m.findings {
            if f.error {
                rep.push_at(f.file.clone(), f.line, format!("[{}] {}", f.rule, f.msg));
            } else {
                rep.note(format!("TODO {}:{}  [{}] {}", f.file, f.line, f.rule, f.msg));
            }
        }
    }
    if show_sites {
        for m in &reps {
            if m.sites.is_empty() {
                continue;
            }
            rep.note(format!("-- {} ({}): conversion sites", m.name, m.guard.label()));
            for (file, line, code) in &m.sites {
                rep.note(format!("   {file}:{line}  {}", code.chars().take(110).collect::<String>()));
            }
        }
    }
    std::process::exit(rep.finish("every converted mod registers its UObject caches with the world guard"));
}

/// Keep the files of mods `mods.release.txt` enables; returns (kept, skipped mod names).
fn release_filter(all: Vec<(String, String, String)>, status: &std::collections::BTreeMap<String, bool>) -> (Vec<(String, String, String)>, Vec<String>) {
    let mut skipped: Vec<String> = vec![];
    let mut kept = vec![];
    for f in all {
        if status.get(&f.0).copied().unwrap_or(false) {
            kept.push(f);
        } else if !skipped.contains(&f.0) {
            skipped.push(f.0.clone());
        }
    }
    (kept, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_set_keeps_enabled_and_strict_fails_their_todo() {
        let status: std::collections::BTreeMap<String, bool> =
            [("On".to_string(), true), ("Dev".to_string(), true), ("Off".to_string(), false)].into_iter().collect();
        let src = "local hud = nil\nRegisterHook(\"/Script/A.B:C\", function() hud = FindFirstOf(\"X\") end)\n".to_string();
        let f = |m: &str| (m.to_string(), format!("mods/{m}/Scripts/main.lua"), src.clone());
        let (kept, skipped) = release_filter(vec![f("On"), f("Dev"), f("Off"), f("Unlisted")], &status);
        assert_eq!(kept.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(), vec!["On", "Dev"]);
        assert_eq!(skipped, vec!["Off".to_string(), "Unlisted".to_string()]);
        // an enabled mod without the shared guard that touches UObjects: strict -> an error
        let reps = wg::scan_mods(&kept, true);
        assert!(reps.iter().all(|r| r.findings.iter().any(|x| x.error)), "strict must fail enabled mods' findings");
        let lax = wg::scan_mods(&kept, false);
        assert!(lax.iter().all(|r| r.findings.iter().all(|x| !x.error)), "non-strict: TODO notes only");
    }
}

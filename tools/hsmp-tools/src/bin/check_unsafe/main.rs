//! check_unsafe: static lint for UE4SS/Half Sword APIs that crashed the game
//! (wired into G0 as `unsafe`).
//!
//!     cargo run --release -p hsmp-tools --bin check_unsafe
//!         [-- --strict] [--write-baseline] [--dump <UE4SS_ObjectDump.txt>] [--json <out>] [--list-soft]
//!
//! | rule | flags | crash it prevents |
//! |------|-------|-------------------|
//! | U1 | `:get()` / `.x` / `[..]` on a RegisterHook callback param whose UFunction param type is SoftObject/SoftClass (types from UE4SS_ObjectDump.txt) | null memcpy in UE4SS push_softobjectproperty (uncatchable AV; seen in the HSMPMatch soft-travel hook) |
//! | U2 | `obj.Name` / `obj["Name"]` / `obj:GetPropertyValue("Name")` / `SetPropertyValue("Name", ..)` where Name is a Soft* property on every class that declares it, or on any `/Game/` class (mark a known plain owner `-- soft: ok <class>`) | same (HSMPDiag property dump) |
//! | U3 | `SetLeaderPoseComponent` / `SetMasterPoseComponent`; every `K2_DestroyActor` unless marked | leader pose on a stand-in's SK_Skeleton crashed; K2_DestroyActor on pooled Willie_BP_C is a no-op that snaps it to the origin |
//! | U4 | `ExecuteWithDelay` / `LoopAsync` / `RegisterKeyBindAsync` / `ExecuteAsync` not routed through the mod's game-thread shim | off-thread Lua corrupted the Lua VM (lua_next / __index on garbage) |
//! | U5 | a hook callback (RegisterHook, NotifyOnNewObject, BeginPlay hooks) that uses a module-level UObject cache without a world-guard check | stale UObject written after OpenLevel (UStruct::FindProperty <- __newindex) |
//! | U6 | `ProcessConsoleExec` / `ConsoleCommand` / `ExecuteConsoleCommand` outside HSMPMatch/Scripts/director.lua | travel / quit behind the Director's back |
//!
//! Scope: `mods/<Mod>/Scripts/*.lua` of every mod that
//! `mods/mods.release.txt` enables (`1` or `dev`), plus `shared/*.lua`.
//! Disabled mods are counted in a note only.
//!
//! Suppression: `-- unsafe: ok <reason>` on the flagged line (justify the reason).
//!
//! Baseline: `tools/hsmp-tools/check_unsafe.baseline` lists known findings in
//! mods that have an entry in `OWNERS` (rule, file, trimmed code line; no line
//! numbers, so it survives edits elsewhere). They print as `PENDING(owner)` and
//! do not fail; any finding NOT in the baseline fails (exit 1). Delete a line
//! when its finding is fixed; stale lines are reported. `--strict` ignores
//! the baseline (the target). `--write-baseline` rewrites it from the current
//! findings (owned-mod findings only; mods without an owner are fixed instead).
//!
//! Exit: 0 clean, 1 new finding(s), 2 tool error (no object dump: U1/U2 cannot
//! run, so the lint fails instead of passing vacuously; `--no-dump` skips them).

mod lex;
mod rules;
mod sig;

use hsmp_tools::{lint, paths};
use rules::{Finding, SrcFile};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Mod -> owner label for reporting (mods not listed are unowned: their findings cannot be
/// baselined).
const OWNERS: &[(&str, &str)] = &[
    ("HSMPAvatars", "pose"),
    ("HSMPSync", "pose"),
    ("HSMPMatch", "match"),
    ("HSMPLoadout", "loadout"),
    ("HSMPMenu", "menu"),
    ("HSMPBrowser", "menu"),
    ("shared", "shared"),
];

fn owner(module: &str) -> Option<&'static str> {
    OWNERS.iter().find(|(m, _)| *m == module).map(|(_, o)| *o)
}

const BASELINE: &str = "tools/hsmp-tools/check_unsafe.baseline";

/// `mods.release.txt`: name -> enabled ("1" / "dev").
fn mod_status(root: &Path) -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(root.join("mods").join("mods.release.txt")) {
        for l in t.lines() {
            let l = l.trim();
            if l.starts_with('#') {
                continue;
            }
            if let Some((n, v)) = l.split_once(':') {
                let v = v.trim();
                m.insert(n.trim().to_string(), v == "1" || v == "dev");
            }
        }
    }
    m
}

fn key(f: &Finding) -> (String, String, String) {
    (f.rule.to_string(), f.file.clone(), f.code.clone())
}

fn load_baseline(root: &Path) -> Vec<(String, String, String)> {
    let Ok(t) = std::fs::read_to_string(root.join(BASELINE)) else { return vec![] };
    t.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some((p.next()?.to_string(), p.next()?.to_string(), p.next()?.trim().to_string()))
        })
        .collect()
}

fn write_baseline(root: &Path, fs: &[(Finding, String)]) -> std::io::Result<usize> {
    let mut s = String::from(
        "# check_unsafe baseline: known findings in mods listed in check_unsafe's OWNERS table.\n\
         # Format: RULE<TAB>file<TAB>trimmed code line. Delete a line when its finding is fixed.\n\
         # Never add findings in unowned mods (fix them instead). Regenerate: check_unsafe --write-baseline\n",
    );
    // one line per finding: identical lines are a COUNT, so a second copy of a baselined
    // line is a new finding, not a free pass
    let mut n = 0;
    for (f, module) in fs {
        if owner(module).is_none() {
            continue;
        }
        s.push_str(&format!("{}\t{}\t{}\n", f.rule, f.file, f.code));
        n += 1;
    }
    std::fs::write(root.join(BASELINE), s)?;
    Ok(n)
}

/// Match findings against the baseline as a multiset: each baseline line covers
/// exactly ONE finding with that (rule, file, code) key, so a duplicated identical hit beyond
/// the baselined count fails. Returns (per finding: baselined?, unused baseline lines).
fn match_baseline(all: &[(Finding, String)], base: &[(String, String, String)]) -> (Vec<bool>, Vec<(String, String, String)>) {
    let mut left: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for b in base {
        *left.entry(b.clone()).or_default() += 1;
    }
    let matched = all
        .iter()
        .map(|(f, _)| match left.get_mut(&key(f)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        })
        .collect();
    let mut stale = vec![];
    for (k, n) in left {
        stale.extend(std::iter::repeat_n(k, n));
    }
    (matched, stale)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let opt = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let strict = flag("--strict");
    let root = match paths::repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("check_unsafe: {e:#}");
            std::process::exit(2);
        }
    };
    let sigs = if flag("--no-dump") {
        None
    } else {
        let dump = opt("--dump").map(PathBuf::from).or_else(|| {
            paths::game_dir(&root).ok().map(|g| paths::ue4ss_dir(&g).join("UE4SS_ObjectDump.txt"))
        });
        match dump.as_ref().and_then(|d| std::fs::read(d).ok()) {
            Some(b) => Some(sig::Sigs::parse(&String::from_utf8_lossy(&b))),
            None => {
                eprintln!(
                    "check_unsafe: UE4SS_ObjectDump.txt not found ({}); set HSMP_GAME_DIR or --dump (or --no-dump to skip U1/U2)",
                    dump.map(|d| d.display().to_string()).unwrap_or_else(|| "no game dir".into())
                );
                std::process::exit(2);
            }
        }
    };
    let soft_only = sigs.as_ref().map(|s| s.soft_only()).unwrap_or_default();
    if flag("--list-soft") {
        if let Some(s) = &sigs {
            println!("soft-only member names ({}): {}", soft_only.len(), soft_only.iter().cloned().collect::<Vec<_>>().join(", "));
            let amb = s.soft_ambiguous();
            println!("ambiguous (soft on some classes) ({}): {}", amb.len(), amb.into_iter().collect::<Vec<_>>().join(", "));
            let ga = s.game_ambiguous();
            println!("  of which soft on a /Game/ class (U2-linted; `-- soft: ok <class>` to mark a plain owner) ({}): {}", ga.len(), ga.into_iter().collect::<Vec<_>>().join(", "));
            println!("functions with soft params: {}", s.funcs.values().filter(|v| v.iter().any(|p| p.1)).count());
        }
    }

    let status = mod_status(&root);
    let mut files: Vec<PathBuf> = paths::mod_script_files(&root);
    files.extend(paths::glob(&root.join("mods").join("shared"), "*.lua"));
    let loaded = match lint::LuaFile::load_all(&root, &files) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("check_unsafe: {e:#}");
            std::process::exit(2);
        }
    };
    // per-mod shim from main.lua
    let mut shims: BTreeMap<String, rules::Shim> = BTreeMap::new();
    for f in &loaded {
        if f.rel.ends_with("/Scripts/main.lua") {
            shims.insert(f.mod_name(), rules::find_shim(&f.src));
        }
    }
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    let mut all: Vec<(Finding, String)> = vec![];
    let mut scanned = 0;
    for f in &loaded {
        let module = if f.rel.starts_with("mods/shared/") { "shared".to_string() } else { f.mod_name() };
        if module != "shared" && !status.get(&module).copied().unwrap_or(false) {
            skipped.insert(module);
            continue;
        }
        scanned += 1;
        let src = SrcFile { module: module.clone(), rel: f.rel.clone(), src: f.src.clone() };
        let shim = shims.get(&module).cloned().unwrap_or_default();
        for x in rules::scan(&src, sigs.as_ref(), &soft_only, &shim) {
            all.push((x, module.clone()));
        }
    }

    if flag("--write-baseline") {
        match write_baseline(&root, &all) {
            Ok(n) => println!("check_unsafe: wrote {n} owned-mod finding(s) to {BASELINE}"),
            Err(e) => {
                eprintln!("check_unsafe: {e}");
                std::process::exit(2);
            }
        }
    }
    let base = if strict { vec![] } else { load_baseline(&root) };
    let mut rep = lint::Report::new("check_unsafe");
    rep.note(format!(
        "{scanned} file(s) scanned; disabled mods skipped: {}; soft-only names {}; dump {}",
        if skipped.is_empty() { "none".into() } else { skipped.iter().cloned().collect::<Vec<_>>().join(",") },
        soft_only.len(),
        if sigs.is_some() { "loaded" } else { "SKIPPED (U1/U2 off)" }
    ));
    let (matched, stale) = match_baseline(&all, &base);
    let mut pending = 0;
    let mut json_rows = vec![];
    for ((f, module), in_base) in all.iter().zip(matched) {
        json_rows.push(serde_json::json!({
            "rule": f.rule, "file": f.file, "line": f.line, "msg": f.msg, "code": f.code,
            "module": module, "owner": owner(module).unwrap_or("unowned"), "baselined": in_base,
        }));
        if in_base {
            pending += 1;
            rep.note(format!("PENDING({}) {}:{}  [{}] {}", owner(module).unwrap_or("?"), f.file, f.line, f.rule, f.msg));
        } else {
            rep.push_at(f.file.clone(), f.line, format!("[{}] {}{}", f.rule, f.msg, owner(module).map(|o| format!("  (owner: {o})")).unwrap_or_default()));
        }
    }
    for b in &stale {
        rep.note(format!("stale baseline entry (fixed or edited? remove it): {}\t{}\t{}", b.0, b.1, b.2));
    }
    if pending > 0 {
        rep.note(format!("{pending} baselined finding(s) pending their owners (see {BASELINE}; --strict fails on them)"));
    }
    if let Some(j) = opt("--json") {
        let doc = serde_json::json!({ "tool": "check_unsafe", "strict": strict, "findings": json_rows });
        let _ = std::fs::write(j, serde_json::to_string_pretty(&doc).unwrap_or_default());
    }
    std::process::exit(rep.finish("no unsafe UE4SS API use outside the baseline"));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(line: usize, code: &str) -> (Finding, String) {
        (Finding { file: "mods/HSMPLoadout/Scripts/main.lua".into(), line, rule: "U3", msg: "m".into(), code: code.into() }, "HSMPLoadout".into())
    }
    fn b(code: &str) -> (String, String, String) {
        ("U3".into(), "mods/HSMPLoadout/Scripts/main.lua".into(), code.into())
    }
    #[test]
    fn baseline_is_counted() {
        let x = "pcall(function() cur:K2_DestroyActor() end)";
        // one baseline line, two identical findings: the second is NEW
        let (m, stale) = match_baseline(&[f(1, x), f(9, x)], &[b(x)]);
        assert_eq!(m, vec![true, false]);
        assert!(stale.is_empty());
        // two lines cover two
        let (m, _) = match_baseline(&[f(1, x), f(9, x)], &[b(x), b(x)]);
        assert_eq!(m, vec![true, true]);
        // a fixed copy leaves one stale line
        let (m, stale) = match_baseline(&[f(1, x)], &[b(x), b(x)]);
        assert_eq!(m, vec![true]);
        assert_eq!(stale.len(), 1);
    }
}

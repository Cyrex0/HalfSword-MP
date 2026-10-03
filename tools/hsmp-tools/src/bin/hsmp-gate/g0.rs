//! G0 gate, run by .githooks/pre-push before every push.
//!
//! Checks:
//!   bp_names      hsmp-tools check-bp-names (needs the game's UE4SS_ObjectDump.txt: without
//!                 one it is SKIPPED, or fails under --strict)
//!   lua_check     hsmp-tools lua-check (every mod Lua file compiles under Lua 5.4)
//!   lua_test      hsmp-tools lua-test (every lua-tests/*.lua suite, incl. hsmp_log)
//!   travel        check_travel: level changes only in HSMPMatch/Scripts/director.lua
//!                 (+ the allow-listed, opt-in HSMPMenu legacy_travel.lua shim)
//!   wg            check_wg: world-guard lint (all mods, then --strict on the release set)
//!   image_kill    no kill-by-image-name anywhere in the tree (DoD-12)
//!   instant_sub   no new `Instant::now() - Duration` (baselined known hits)
//!   events        the event contract: real emitters vs the fields the gate reads (contract.rs)
//!   gate_selftest the gate evaluator on scripts/fixtures
//!   ipc_schema    generated IPC files current (gen-ipc --check) + header compiles (cl /W4 /WX)
//!   state_files   check_no_state_files: Lua names + server/src file writers vs state_files.allow
//!   cargo        cargo test --workspace (skipped with --quick)
//! The game dir is HSMP_GAME_DIR, else <repo>/game, else the main worktree's game/ (see
//! hsmp_tools::paths::game_dir). Without the game's UE4SS object dump, bp_names and the dump
//! rules of `unsafe` (U1/U2) are reported as SKIP; `--strict` turns that into a failure.
//! On a full pass with nothing skipped, on a clean tree, it writes
//! <git-common-dir>/hsmp-g0/<commit>.json (and the legacy hsmp-g0.json), which
//! build-and-deploy.ps1 copies into the deploy stamp.

use crate::{contract, fixtures, lint, util};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub struct Opts {
    pub quick: bool,
    pub json: Option<PathBuf>,
    pub only: Option<Vec<String>>,
    pub no_stamp: bool,
    /// fail, instead of skip, the checks that need the game's UE4SS object dump
    pub strict: bool,
}

/// What a skipped dump-dependent check reports.
pub const NO_DUMP: &str = "skipped: no game dump (set HSMP_GAME_DIR)";

/// The game's `UE4SS_ObjectDump.txt`, when the game dir (hsmp_tools::paths::game_dir) has one.
fn object_dump(repo: &Path) -> Option<PathBuf> {
    let g = hsmp_tools::paths::game_dir(repo).ok()?;
    Some(hsmp_tools::paths::ue4ss_dir(&g).join("UE4SS_ObjectDump.txt")).filter(|p| p.is_file())
}

fn run(cmd: &mut Command) -> (i32, String, f64) {
    let t = Instant::now();
    match cmd.output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s += &String::from_utf8_lossy(&o.stderr);
            (o.status.code().unwrap_or(-1), s, t.elapsed().as_secs_f64())
        }
        Err(e) => (127, e.to_string(), t.elapsed().as_secs_f64()),
    }
}

fn last_lines(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    v[v.len().saturating_sub(n)..].join(" | ")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let (code, out, _) = run(Command::new("git").arg("-C").arg(repo).args(args));
    if code == 0 { out.trim().to_string() } else { String::new() }
}

fn cargo() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO") {
        return PathBuf::from(p);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let p = Path::new(&home).join(".cargo").join("bin").join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    if p.exists() { p } else { PathBuf::from("cargo") }
}

/// hsmp-tools.exe: next to this binary (same target dir), else the workspace target dir.
fn hsmp_tools(repo: &Path) -> Option<PathBuf> {
    sibling_bin(repo, "hsmp-tools")
}

/// A binary of the hsmp-tools package (`hsmp-tools`, `check_travel`, ...): next to this
/// binary (same target dir), else `$CARGO_TARGET_DIR` or `<repo>/target`, `{release,debug}`.
fn sibling_bin(repo: &Path, name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    if let Ok(me) = std::env::current_exe() {
        if let Some(p) = me.parent().map(|d| d.join(&exe)).filter(|p| p.exists()) {
            return Some(p);
        }
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| repo.join("target"));
    ["release", "debug"].iter().map(|prof| target.join(prof).join(&exe)).find(|p| p.exists())
}

/// travel: `check_travel` (the authoritative rule, with its allow-list:
/// HSMPMatch/Scripts/director.lua and the temporary HSMPMenu/Scripts/legacy_travel.lua shim).
pub fn check_travel(repo: &Path) -> (&'static str, String) {
    let Some(t) = sibling_bin(repo, "check_travel") else {
        return ("fail", "check_travel not built (cargo build --release -p hsmp-tools)".into());
    };
    let (code, out, dt) = run(Command::new(t).current_dir(repo).env("HSMP_ROOT", repo));
    let notes: Vec<&str> = out.lines().filter(|l| l.starts_with("allowed")).map(|l| l.split(" (").next().unwrap_or(l)).collect();
    let st = if code == 0 { "pass" } else { "fail" };
    let extra = if notes.is_empty() { String::new() } else { format!(" [{}]", notes.join("; ")) };
    (st, format!("{}{extra} ({dt:.1}s)", last_lines(&out, if code == 0 { 1 } else { 6 })))
}

fn tools_check(repo: &Path, args: &[&str], tail: usize) -> (&'static str, String) {
    let Some(t) = hsmp_tools(repo) else {
        return ("fail", "hsmp-tools not built (cargo build --release -p hsmp-tools)".into());
    };
    let (code, out, dt) = run(Command::new(t).args(args).current_dir(repo).env("HSMP_ROOT", repo));
    (if code == 0 { "pass" } else { "fail" }, format!("{} ({dt:.1}s)", last_lines(&out, tail)))
}

/// wg: the `check_wg` binary (converted mods must register UObject
/// caches with the world guard; unconverted mods are advisory TODO notes).
fn check_wg(repo: &Path) -> (&'static str, String) {
    let Some(t) = sibling_bin(repo, "check_wg") else {
        return ("fail", "check_wg not built (cargo build --release -p hsmp-tools)".into());
    };
    let (code, out, dt) = run(Command::new(&t).current_dir(repo).env("HSMP_ROOT", repo));
    // every mod the release enables must also pass --strict (its TODOs fail)
    let (scode, sout, sdt) = run(Command::new(&t).args(["--strict", "--release-set"]).current_dir(repo).env("HSMP_ROOT", repo));
    let ok = code == 0 && scode == 0;
    (if ok { "pass" } else { "fail" }, format!("all mods: {} | --strict --release-set: {} ({:.1}s)",
        last_lines(&out, if code == 0 { 1 } else { 6 }), last_lines(&sout, if scode == 0 { 1 } else { 6 }), dt + sdt))
}

/// The checks a full G0 runs (DoD-13 rejects a g0.json that lacks any of them).
pub const REQUIRED_CHECKS: &[&str] = &[
    "bp_names", "lua_check", "lua_test", "travel", "wg", "unsafe", "image_kill", "instant_sub", "events", "gate_selftest",
    "ipc_schema", "state_files", "cargo",
];

/// The MSVC environment script (vcvars64.bat) of the newest Visual Studio, if installed.
fn vcvars64() -> Option<PathBuf> {
    let pf = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let vswhere = Path::new(&pf).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let (code, out, _) = run(Command::new(vswhere).args(["-latest", "-products", "*", "-property", "installationPath"]));
    let root = out.lines().next().map(str::trim).filter(|l| code == 0 && !l.is_empty())?;
    Some(Path::new(root).join(r"VC\Auxiliary\Build\vcvars64.bat")).filter(|p| p.is_file())
}

/// ipc_schema: the committed generated files (C header, Lua schema)
/// equal a fresh `gen-ipc`, and the header's static_asserts hold under `cl /W4 /WX` (C11 and
/// C++17) when MSVC is installed (otherwise noted, not failed).
fn check_ipc_schema(repo: &Path) -> (&'static str, String) {
    let bad = hsmp_ipc::gen::check_all(repo);
    if !bad.is_empty() {
        return ("fail", format!("stale generated IPC files {bad:?}: run `hsmp-tools gen-ipc`"));
    }
    let hash = format!("layout {:016x}", hsmp_ipc::LAYOUT_HASH);
    if !cfg!(windows) {
        return ("pass", format!("{hash}; generated files current; header compile: not on Windows"));
    }
    let Some(vc) = vcvars64() else {
        return ("pass", format!("{hash}; generated files current; header compile skipped (MSVC not found)"));
    };
    let tmp = std::env::temp_dir().join(format!("hsmp-g0-ipc-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let hdr = repo.join(hsmp_ipc::gen::C_HEADER_PATH).display().to_string().replace('\\', "/");
    let _ = std::fs::write(tmp.join("t.c"), format!("#include \"{hdr}\"\nint main(void) {{ return 0; }}\n"));
    let _ = std::fs::write(tmp.join("t.cpp"), format!("#include \"{hdr}\"\nint main() {{ return 0; }}\n"));
    let mut c = Command::new("cmd");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.raw_arg(format!(
            "/C \"call \"{}\" >nul 2>nul && cl /nologo /W4 /WX /std:c11 /c t.c && cl /nologo /W4 /WX /std:c++17 /EHsc /c t.cpp\"",
            vc.display()
        ));
    }
    let (code, out, dt) = run(c.current_dir(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
    if code == 0 {
        ("pass", format!("{hash}; generated files current; header compiles under cl /W4 /WX (C11, C++17) ({dt:.1}s)"))
    } else {
        ("fail", format!("generated header does not compile: {}", last_lines(&out, 6)))
    }
}

/// cargo: `cargo test --workspace --locked` over the root workspace (every package: server,
/// launcher, tools, crates and the Lua test harnesses under tests/).
fn check_cargo(repo: &Path) -> (&'static str, String) {
    let manifest = repo.join("Cargo.toml");
    let (code, out, dt) =
        run(Command::new(cargo()).current_dir(repo).args(["test", "--workspace", "--locked", "--quiet", "--manifest-path"]).arg(&manifest));
    let results: Vec<(u64, u64)> = out
        .lines()
        .filter_map(|l| l.strip_prefix("test result: "))
        .map(|l| {
            let num = |key: &str| {
                l.split(';').find(|p| p.trim().ends_with(key)).and_then(|p| p.split_whitespace().next()).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0)
            };
            (l.split(';').next().and_then(|x| x.split_whitespace().nth(1)).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0), num("failed"))
        })
        .collect();
    let passed: u64 = results.iter().map(|r| r.0).sum();
    let failed: u64 = results.iter().map(|r| r.1).sum();
    let summary = format!("workspace: {passed} passed, {failed} failed in {} test binaries ({dt:.0}s)", results.len());
    if code == 0 {
        ("pass", summary)
    } else {
        let errs: Vec<&str> = out.lines().filter(|l| l.contains("FAILED") || l.starts_with("error")).take(6).collect();
        ("fail", format!("{summary} || {}", errs.join(" | ")))
    }
}

pub fn run_g0(repo: &Path, o: &Opts) -> i32 {
    type CheckFn = Box<dyn Fn(&Path) -> (&'static str, String)>;
    let strict = o.strict;
    // Without the game's object dump these checks cannot run: SKIP (visible, and no G0
    // stamp is written), or FAIL under --strict. Never a vacuous PASS.
    let no_dump = move || if strict { ("fail", format!("{NO_DUMP}; --strict")) } else { ("skip", NO_DUMP.to_string()) };
    let checks: Vec<(&str, CheckFn)> = vec![
        ("bp_names", Box::new(move |r: &Path| {
            match object_dump(r) {
                Some(d) => tools_check(r, &["check-bp-names", "--dump", &d.to_string_lossy()], 2),
                None => no_dump(),
            }
        })),
        ("lua_check", Box::new(|r: &Path| tools_check(r, &["lua-check"], 2))),
        ("lua_test", Box::new(|r: &Path| tools_check(r, &["lua-test"], 3))),
        ("travel", Box::new(check_travel)),
        ("wg", Box::new(check_wg)),
        // unsafe UE4SS API lint (soft params/props, leader pose, pooled destroy,
        // off-thread delays, unguarded hook captures, console outside the Director)
        ("unsafe", Box::new(move |r: &Path| {
            let Some(t) = sibling_bin(r, "check_unsafe") else {
                return ("fail", "check_unsafe not built (cargo build --release -p hsmp-tools)".into());
            };
            // without a dump the source-only rules still run (--no-dump skips U1/U2)
            let dump = object_dump(r);
            let mut cmd = Command::new(t);
            match &dump {
                Some(d) => cmd.arg("--dump").arg(d),
                None => cmd.arg("--no-dump"),
            };
            let (code, out, dt) = run(cmd.current_dir(r).env("HSMP_ROOT", r));
            let summary = format!("{} ({dt:.1}s)", last_lines(&out, if code == 0 { 2 } else { 6 }));
            match (code == 0, dump.is_some()) {
                (false, _) => ("fail", summary),
                (true, true) => ("pass", summary),
                (true, false) => {
                    let (st, why) = no_dump();
                    (st, format!("U1/U2 {why}; other rules: {summary}"))
                }
            }
        })),
        ("image_kill", Box::new(|r: &Path| {
            let hits = lint::image_name_kills(r);
            (if hits.is_empty() { "pass" } else { "fail" }, format!("kill-by-image-name: {}", if hits.is_empty() { "none".into() } else { format!("{hits:?}") }))
        })),
        // `Instant::now() - Duration` panics on a PC booted less than that long ago
        ("instant_sub", Box::new(|r: &Path| {
            let (new, old, stale) = lint::instant_sub_hits(r);
            let mut s = format!("Instant::now() minus a Duration: {}", if new.is_empty() { "no new hits".into() } else { format!("NEW {new:?}") });
            if !old.is_empty() {
                s += &format!("; baselined (owner must fix) {old:?}");
            }
            if !stale.is_empty() {
                s += &format!("; note: stale baseline entries {stale:?}");
            }
            (if new.is_empty() { "pass" } else { "fail" }, s)
        })),
        // every event/field a real emitter writes is known to the gate, and every field the
        // gate reads has a real emitter (or a recorded PENDING request)
        ("events", Box::new(|r: &Path| {
            let rep = contract::check(r);
            (if rep.ok() { "pass" } else { "fail" }, rep.summary())
        })),
        ("gate_selftest", Box::new(|r: &Path| {
            let (n, bad) = fixtures::selftest(r, false);
            (if bad.is_empty() { "pass" } else { "fail" }, format!("{n} fixtures, {} failing {:?}", bad.len(), bad))
        })),
        ("ipc_schema", Box::new(check_ipc_schema)),
        // no state-dir files outside the allow-list: the IPC is shared memory (statefiles.rs)
        ("state_files", Box::new(crate::statefiles::g0)),
        ("cargo", Box::new(check_cargo)),
    ];
    let mut res = Map::new();
    let mut failed = vec![];
    let mut skipped = vec![];
    for (name, f) in &checks {
        if let Some(only) = &o.only {
            if !only.iter().any(|x| x == name) {
                continue;
            }
        }
        let (st, summary) = if o.quick && *name == "cargo" { ("skip", "--quick".to_string()) } else { f(repo) };
        println!("[G0] {name:13} {:5} {summary}", st.to_uppercase());
        match st {
            "fail" => failed.push(name.to_string()),
            "skip" => skipped.push(name.to_string()),
            _ => {}
        }
        res.insert(name.to_string(), json!({"status": st, "summary": summary}));
    }
    let doc = json!({
        "commit": git(repo, &["rev-parse", "HEAD"]),
        "tree_dirty": !git(repo, &["status", "--porcelain"]).is_empty(),
        "time": util::ms_to_iso(util::now_ms()),
        "quick": o.quick,
        "ok": failed.is_empty(),
        "checks": Value::Object(res),
    });
    if let Some(j) = &o.json {
        let _ = util::write_json(j, &doc);
    }
    // The stamp lives in the git dir every worktree shares. Keyed by commit
    // (hsmp-g0/<commit>.json), so a G0 in one worktree never replaces another worktree's record
    // for its own commit; written only for a clean tree (a dirty run certifies nothing, DoD-13).
    // hsmp-g0.json (the last clean pass) stays for older deploy scripts. A run that skipped a
    // check (no game dump) certifies less than a full G0, so it writes no stamp either.
    let clean = doc["tree_dirty"].as_bool() == Some(false);
    if failed.is_empty() && skipped.is_empty() && !o.no_stamp && o.only.is_none() && !o.quick && clean {
        let common = git(repo, &["rev-parse", "--git-common-dir"]);
        let commit = doc["commit"].as_str().unwrap_or("").to_string();
        if !common.is_empty() && commit.len() == 40 {
            let p = if Path::new(&common).is_absolute() { PathBuf::from(&common) } else { repo.join(&common) };
            let _ = std::fs::create_dir_all(p.join("hsmp-g0"));
            let _ = util::write_json(&p.join("hsmp-g0").join(format!("{commit}.json")), &doc);
            let _ = util::write_json(&p.join("hsmp-g0.json"), &doc);
        }
    }
    let verdict = match (failed.is_empty(), skipped.is_empty()) {
        (false, _) => format!("FAIL: {}", failed.join(", ")),
        (true, true) => "PASS".to_string(),
        (true, false) => format!("PASS (skipped: {}; no G0 stamp written)", skipped.join(", ")),
    };
    println!("[G0] {verdict}");
    if failed.is_empty() { 0 } else { 1 }
}

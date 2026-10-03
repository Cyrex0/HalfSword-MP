//! G0 `state_files`: the lint `check_no_state_files` (see
//! docs/development/ipc-shared-memory.md), and the matcher behind gate rule STATE-1 (rules.rs).
//!
//! Shared memory carries every game<->sidecar and Lua<->Lua contract, the native module's
//! process handles and pipes replace the pid / caps / tool-output files, and the DevCtl ring
//! replaces the dev knob files. A state dir holds ONLY persistent config, the
//! identity, and logs / dev output: an allow-list category other than config / identity /
//! log (plus `runtime:` globs, `fs:` writers and `debt:` contracts not yet moved) fails G0.
//! Two checks:
//! * **Lua names**: every string literal in `mods/**/*.lua` that names a state-dir file (a
//!   dot-file name or prefix such as `".skeletal.json"`, `".pose_play"`, `".pose_play{}.json"`,
//!   or the `me_` root-file family) must be on the allow-list. `io.open` / `os.rename` /
//!   `os.remove` targets are built from those literals, so an unlisted literal is an unlisted
//!   file. The generated schema (`mods/shared/hsmp_ipc_schema.lua`) is skipped: its
//!   `replaces` strings document the old files.
//! * **Rust writers**: in `server/src` (outside `#[cfg(test)]`), `std::fs` / `tokio::fs` writes
//!   (`write`, `create_dir*`, `rename`, `remove_*`, `copy`, `File::create`, `OpenOptions::new`)
//!   may only appear in the allow-listed modules (`fs:<path>` entries): logging, panic guard,
//!   career guard, identity, the tap and hsmp-server's own persistence.
//!
//! The allow-list (`state_files.allow`, next to this file) groups entries under `[category]`
//! headers; a `debt: <owner>` category names a contract not yet moved and who removes it.
//! Entries no longer used are notes; `hsmp-gate state-files --update` drops them (the list
//! only ever shrinks). The gate binary embeds the list ([`ALLOW_TEXT`]) for STATE-1, so a run
//! is judged by the list of the gate's own commit (DoD-13 ties that to the deploy).

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const ALLOW_REL: &str = "tools/hsmp-tools/src/bin/hsmp-gate/state_files.allow";
/// The allow-list this gate was built with (STATE-1).
pub const ALLOW_TEXT: &str = include_str!("state_files.allow");
/// Generated, documents the old files in `replaces` strings: not code that names files.
const SKIP_LUA: &[&str] = &["mods/shared/hsmp_ipc_schema.lua"];

/// The parsed allow-list.
#[derive(Debug, Default, Clone)]
pub struct Allow {
    /// (entry, category): Lua-nameable state-dir names / prefixes / patterns.
    pub names: Vec<(String, String)>,
    /// (glob, category): run-time files (STATE-1 only).
    pub runtime: Vec<(String, String)>,
    /// (module path, category): server/src modules allowed to write files.
    pub fs: Vec<(String, String)>,
}

impl Allow {
    pub fn parse(text: &str) -> Allow {
        let mut a = Allow::default();
        let mut cat = String::from("uncategorised");
        for raw in text.lines() {
            let l = raw.split('#').next().unwrap_or("").trim();
            if l.is_empty() {
                continue;
            }
            if let Some(rest) = l.strip_prefix('[') {
                cat = rest.split(']').next().unwrap_or("").trim().to_string();
                continue;
            }
            if let Some(p) = l.strip_prefix("fs:") {
                a.fs.push((p.trim().to_string(), cat.clone()));
            } else if is_runtime(&cat) {
                a.runtime.push((l.to_string(), cat.clone()));
            } else {
                a.names.push((l.to_string(), cat.clone()));
            }
        }
        a
    }
    pub fn name_set(&self) -> BTreeSet<String> {
        self.names.iter().map(|(n, _)| n.clone()).collect()
    }
    pub fn category_of(&self, name: &str) -> Option<&str> {
        self.names.iter().find(|(n, _)| n == name).map(|(_, c)| c.as_str())
    }
    /// STATE-1: may a file called `name` be in a state dir after a run? A name entry matches
    /// exactly, as a prefix, or with `{}` / `<id>` standing for digits; a runtime entry is a
    /// `*` glob.
    pub fn allows_file(&self, name: &str) -> bool {
        self.names.iter().any(|(e, _)| entry_matches(e, name)) || self.runtime.iter().any(|(g, _)| glob(g, name))
    }
}

/// `[runtime]` / `[runtime: <kind>]`: globs of files written at run time (STATE-1 only).
fn is_runtime(cat: &str) -> bool {
    cat == "runtime" || cat.starts_with("runtime:")
}

/// The only kinds of state-dir file there may be: persistent config, the
/// identity, logs / dev output; plus `fs:` writer modules and `debt: <owner>` (a contract not
/// yet moved). A category outside these fails G0, so plumbing / tool / knob files cannot
/// come back under a new heading.
pub const CATEGORY_KINDS: &[&str] = &["config", "identity", "log"];

pub fn category_allowed(cat: &str) -> bool {
    if cat.starts_with("debt:") || cat.starts_with("fs:") {
        return true;
    }
    let base = cat.strip_prefix("runtime:").map(str::trim).unwrap_or(cat);
    let head = base.split([' ', ':']).next().unwrap_or("");
    CATEGORY_KINDS.contains(&head)
}

fn entry_matches(entry: &str, name: &str) -> bool {
    if entry == name || name.starts_with(entry) {
        return true;
    }
    if entry.contains("{}") || entry.contains('<') {
        let pat = Regex::new(r"\{\}|<[a-z]+>").unwrap();
        let re = format!("^{}", pat.split(entry).map(regex::escape).collect::<Vec<_>>().join(r"\d+"));
        return Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(false);
    }
    false
}

fn glob(g: &str, name: &str) -> bool {
    let re = format!("^{}$", g.split('*').map(regex::escape).collect::<Vec<_>>().join(".*"));
    Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(false)
}

pub fn read_allow_text(repo: &Path) -> Option<String> {
    std::fs::read_to_string(repo.join(ALLOW_REL)).ok()
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if p.is_dir() {
            if !name.starts_with("target") && name != "proptest-regressions" {
                walk(&p, ext, out);
            }
        } else if name.ends_with(ext) {
            out.push(p);
        }
    }
}

fn is_state_name(s: &str) -> bool {
    static DOT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static ME: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let dot = DOT.get_or_init(|| Regex::new(r"^\.[a-z][A-Za-z0-9_\-]*(\{\}|<[a-z]+>)?[A-Za-z0-9_\-]*(\.[A-Za-z0-9_\-{}]+)*$").unwrap());
    let me = ME.get_or_init(|| Regex::new(r"^me_[A-Za-z_]*(\{\})?(\.json)?$").unwrap());
    if s.len() < 3 || s.len() > 64 {
        return false;
    }
    // bare extensions and Rust/Lua method-ish strings are not file names
    if matches!(s, ".lua" | ".dll" | ".exe" | ".pdb" | ".txt" | ".json" | ".jsonl" | ".log" | ".tmp" | ".bin" | ".sav" | ".md" | ".toml" | ".rs" | ".ps1" | ".sh" | ".zip" | ".pak" | ".ucas" | ".utoc" | ".sig" | ".hpp" | ".bak" | ".old" | ".new") {
        return false;
    }
    dot.is_match(s) || me.is_match(s)
}

/// `name -> locations` of every state-file literal in `mods/**/*.lua`.
pub fn inventory(repo: &Path) -> BTreeMap<String, Vec<String>> {
    let lit = Regex::new(r#""((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'"#).unwrap();
    let ph = Regex::new(r"\{[^}]*\}").unwrap();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut files = vec![];
    walk(&repo.join("mods"), ".lua", &mut files);
    for f in &files {
        let rel = f.strip_prefix(repo).unwrap_or(f).to_string_lossy().replace('\\', "/");
        if SKIP_LUA.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for (ln, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("--") {
                continue;
            }
            // drop a trailing Lua comment (good enough: state names never contain "--")
            let code = line.split(" --").next().unwrap_or(line);
            for c in lit.captures_iter(code) {
                let s = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
                // `STATE .. "/.skeletal.json"`: the file name only
                let s = s.rsplit(['/', '\\']).next().unwrap_or(s);
                let n = ph.replace_all(s, "{}").to_string();
                if is_state_name(&n) {
                    out.entry(n).or_default().push(format!("{rel}:{}", ln + 1));
                }
            }
        }
    }
    out
}

/// `module -> lines` of every file write in `server/src` outside `#[cfg(test)]`.
pub fn fs_writers(repo: &Path) -> BTreeMap<String, Vec<usize>> {
    let re = Regex::new(
        r"\b(?:std::fs|tokio::fs|fs)::(?:write|create_dir_all|create_dir|rename|remove_file|remove_dir_all|remove_dir|copy|hard_link)\b|\bFile::create\b|\bOpenOptions::new\b",
    )
    .unwrap();
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut files = vec![];
    walk(&repo.join("server").join("src"), ".rs", &mut files);
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let rel = f.strip_prefix(repo).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        for (ln, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            // a `#[cfg(test)] mod ...` starts the tests (they write fixtures, not IPC); an
            // item-level #[cfg(test)] does not end the scan
            if t.starts_with("#[cfg(test)]") {
                let next = lines[ln + 1..].iter().map(|n| n.trim_start()).find(|n| !n.is_empty() && !n.starts_with("#["));
                if next.is_some_and(|n| {
                    let n = n.strip_prefix("pub(crate) ").or_else(|| n.strip_prefix("pub(super) ")).or_else(|| n.strip_prefix("pub ")).unwrap_or(n);
                    n.starts_with("mod ")
                }) {
                    break;
                }
            }
            if t.starts_with("//") {
                continue;
            }
            if re.is_match(line) {
                out.entry(rel.clone()).or_default().push(ln + 1);
            }
        }
    }
    out
}

/// The outcome of the lint.
#[derive(Debug, Default)]
pub struct Report {
    /// Lua names not on the list, with their locations.
    pub new: Vec<(String, Vec<String>)>,
    /// Name entries no longer used.
    pub stale: Vec<String>,
    /// server/src modules that write files without an `fs:` entry, with their lines.
    pub bad_fs: Vec<(String, Vec<usize>)>,
    /// `fs:` entries no longer writing.
    pub stale_fs: Vec<String>,
    /// (category, used names) of every name category.
    pub by_cat: BTreeMap<String, usize>,
    /// Categories that are not config / identity / log / runtime / fs / debt.
    pub bad_cats: Vec<String>,
}

pub fn check(repo: &Path) -> Report {
    let allow = Allow::parse(&read_allow_text(repo).unwrap_or_default());
    let inv = inventory(repo);
    let names = allow.name_set();
    let mut r = Report {
        new: inv.iter().filter(|(n, _)| !names.contains(*n)).map(|(n, l)| (n.clone(), l.clone())).collect(),
        stale: names.iter().filter(|a| !inv.contains_key(*a)).cloned().collect(),
        ..Report::default()
    };
    for n in inv.keys() {
        if let Some(c) = allow.category_of(n) {
            *r.by_cat.entry(c.to_string()).or_default() += 1;
        }
    }
    let cats: BTreeSet<&str> = allow.names.iter().chain(&allow.runtime).chain(&allow.fs).map(|(_, c)| c.as_str()).collect();
    r.bad_cats = cats.into_iter().filter(|c| !category_allowed(c)).map(String::from).collect();
    let fsw = fs_writers(repo);
    let fs_ok: BTreeSet<&str> = allow.fs.iter().map(|(p, _)| p.as_str()).collect();
    r.bad_fs = fsw.iter().filter(|(m, _)| !fs_ok.contains(m.as_str())).map(|(m, l)| (m.clone(), l.clone())).collect();
    r.stale_fs = allow.fs.iter().filter(|(p, _)| !fsw.contains_key(p)).map(|(p, _)| p.clone()).collect();
    r
}

pub fn g0(repo: &Path) -> (&'static str, String) {
    if !repo.join(ALLOW_REL).is_file() {
        return ("fail", format!("{ALLOW_REL} missing"));
    }
    let r = check(repo);
    let ok = r.new.is_empty() && r.bad_fs.is_empty() && r.bad_cats.is_empty();
    let debt: usize = r.by_cat.iter().filter(|(c, _)| c.starts_with("debt")).map(|(_, n)| n).sum();
    let used: usize = r.by_cat.values().sum();
    let mut s = if ok {
        format!("no state-dir file outside the allow-list ({used} names in use, {debt} of them debt; file writes only in allow-listed modules)")
    } else {
        let mut parts = vec![];
        if !r.new.is_empty() {
            parts.push(format!("NEW state-dir files (use shared memory, docs/development/ipc-shared-memory.md): {}",
                r.new.iter().map(|(n, l)| format!("{n} @ {}", l.first().cloned().unwrap_or_default())).collect::<Vec<_>>().join("; ")));
        }
        if !r.bad_fs.is_empty() {
            parts.push(format!("file writes in modules not on the fs: allow-list: {}",
                r.bad_fs.iter().map(|(m, l)| format!("{m}:{}", l.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join("; ")));
        }
        if !r.bad_cats.is_empty() {
            parts.push(format!("allow-list categories other than config / identity / log (+ runtime: / fs: / debt:): {}", r.bad_cats.join("; ")));
        }
        parts.join(" | ")
    };
    let debt_cats: Vec<String> = r.by_cat.iter().filter(|(c, _)| c.starts_with("debt")).map(|(c, n)| format!("{c} x{n}")).collect();
    if !debt_cats.is_empty() {
        s += &format!("; {}", debt_cats.join(", "));
    }
    if !r.stale.is_empty() || !r.stale_fs.is_empty() {
        let all: Vec<String> = r.stale.iter().cloned().chain(r.stale_fs.iter().map(|p| format!("fs:{p}"))).collect();
        s += &format!("; note: {} allow-list entries no longer used (hsmp-gate state-files --update drops them): {}", all.len(), all.join(" "));
    }
    (if ok { "pass" } else { "fail" }, s)
}

/// Drop unused name / fs entries from the allow-list, keeping its categories and comments.
/// Never adds: a new name belongs in shared memory, not on the list.
pub fn shrink(repo: &Path) -> std::io::Result<usize> {
    let text = read_allow_text(repo).unwrap_or_default();
    let r = check(repo);
    let stale: BTreeSet<&str> = r.stale.iter().map(String::as_str).collect();
    let stale_fs: BTreeSet<String> = r.stale_fs.iter().map(|p| format!("fs:{p}")).collect();
    let mut cat = String::new();
    let mut dropped = 0;
    let mut out = String::new();
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if let Some(rest) = l.strip_prefix('[') {
            cat = rest.split(']').next().unwrap_or("").trim().to_string();
        } else if !l.is_empty() && !is_runtime(&cat) && (stale.contains(l) || stale_fs.contains(l)) {
            dropped += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(repo.join(ALLOW_REL), out)?;
    Ok(dropped)
}

pub fn run_cli(repo: &Path, update: bool, verbose: bool) -> i32 {
    if update {
        match shrink(repo) {
            Ok(n) => println!("state-files: dropped {n} unused entries from {ALLOW_REL}"),
            Err(e) => {
                eprintln!("state-files: {e}");
                return 2;
            }
        }
    }
    if verbose {
        let allow = Allow::parse(&read_allow_text(repo).unwrap_or_default());
        for (n, l) in inventory(repo) {
            println!("{n:34} [{}] {}", allow.category_of(&n).unwrap_or("NOT LISTED"), l.join(" "));
        }
        for (m, l) in fs_writers(repo) {
            println!("fs:{m:31} {}", l.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","));
        }
    }
    let (st, s) = g0(repo);
    println!("{s}");
    if st == "pass" { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_state_names() {
        for s in [".skeletal.json", ".pose_play", ".pose_play{}.json", ".control.outbox.jsonl", "me_", "me_remote", "me_{}.json", ".tdiag_on", ".world_sync.req"] {
            assert!(is_state_name(s), "{s}");
        }
        for s in [".json", ".lua", "..", ".5", "hello", "a.json", ".0f", "me", "Map_Arena_Alley"] {
            assert!(!is_state_name(s), "{s}");
        }
    }

    #[test]
    fn the_committed_list_parses_and_categorises() {
        let a = Allow::parse(ALLOW_TEXT);
        assert!(a.names.iter().any(|(n, c)| n == ".settings.json" && c.starts_with("config")));
        assert!(a.names.iter().all(|(_, c)| c != "uncategorised"), "every entry has a category");
        assert!(a.fs.iter().any(|(p, _)| p == "server/src/sidecar/career_guard.rs"));
        // STATE-1 matching
        assert!(a.allows_file("hsmp_events.jsonl") && a.allows_file("hsmp_events.3.jsonl"));
        assert!(a.allows_file(".settings.json") && a.allows_file(".player_key") && a.allows_file("identity"));
        assert!(a.allows_file(".diag_frame.txt"), "prefix entry");
        assert!(!a.allows_file("random.bin") && !a.allows_file(".brand_new.json"));
        // no process plumbing, tool output files or dev knobs
        for gone in [".pid.sidecar.json", ".pid.master.json", ".caps.server.txt", ".caps.query.txt", ".game_pid.txt",
                     ".autotest.cmd.jsonl", ".pose_tune.json", ".tdiag_on", ".browser_result.tsv",
                     ".servers.json", ".master_probe.txt", ".settings_test1.body", ".hsmp_pids.json"] {
            assert!(!a.allows_file(gone), "{gone} must not be allowed");
        }
        // every category is config / identity / log (+ runtime / fs / debt)
        let cats: Vec<&str> = a.names.iter().chain(&a.runtime).chain(&a.fs).map(|(_, c)| c.as_str()).collect();
        assert!(cats.iter().all(|c| category_allowed(c)), "{cats:?}");
        // every contract is a typed record; no debt names are left on the list.
        assert!(!cats.iter().any(|c| c.starts_with("debt")), "debt entries left: {cats:?}");
    }

    #[test]
    fn categories_are_config_identity_log_only() {
        for ok in ["config", "config: persistent", "identity", "log", "log: dev output", "runtime: log", "runtime: identity",
                   "fs: logs", "debt: F-LUA"] {
            assert!(category_allowed(ok), "{ok}");
        }
        for bad in ["tool", "dev", "runtime", "runtime: tool", "uncategorised", "plumbing"] {
            assert!(!category_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn new_names_and_writers_fail_stale_entries_shrink() {
        let d = std::env::temp_dir().join(format!("hsmp-sf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("mods/HSMPX/Scripts")).unwrap();
        std::fs::create_dir_all(d.join("mods/shared")).unwrap();
        std::fs::create_dir_all(d.join("server/src/sidecar")).unwrap();
        std::fs::create_dir_all(d.join(Path::new(ALLOW_REL).parent().unwrap())).unwrap();
        std::fs::write(d.join("mods/HSMPX/Scripts/main.lua"), "local A = STATE .. \"/.old.json\"\nlocal B = '.brand_new.json' -- \".comment.json\"\n").unwrap();
        std::fs::write(d.join("mods/shared/hsmp_ipc_schema.lua"), "replaces = \".skeletal.json\"\n").unwrap();
        std::fs::write(d.join("server/src/sidecar/ok.rs"), "fn f() { std::fs::write(p, b\"x\"); }\n").unwrap();
        std::fs::write(d.join("server/src/sidecar/bad.rs"), "// std::fs::write in a comment\nfn g() { let _ = tokio::fs::write(p, s); }\n#[cfg(test)]\nmod tests { fn t() { std::fs::write(p, b\"\"); } }\n").unwrap();
        std::fs::write(d.join(ALLOW_REL), "# c\n[config] x\n.old.json\n[debt: Y] z\n.gone.json\n[runtime: identity]\nidentity\n[fs: logs]\nfs:server/src/sidecar/ok.rs\nfs:server/src/sidecar/gone.rs\n").unwrap();
        let r = check(&d);
        assert_eq!(r.new.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec![".brand_new.json"]);
        assert_eq!(r.stale, vec![".gone.json".to_string()]);
        assert_eq!(r.bad_fs, vec![("server/src/sidecar/bad.rs".to_string(), vec![2])]);
        assert_eq!(r.stale_fs, vec!["server/src/sidecar/gone.rs".to_string()]);
        assert_eq!(g0(&d).0, "fail");
        assert_eq!(shrink(&d).unwrap(), 2);
        let text = std::fs::read_to_string(d.join(ALLOW_REL)).unwrap();
        assert!(text.contains("[debt: Y] z") && !text.contains(".gone.json") && !text.contains("gone.rs") && text.contains("identity"));
        // fix the two offenders: pass
        std::fs::write(d.join("mods/HSMPX/Scripts/main.lua"), "local A = STATE .. \"/.old.json\"\n").unwrap();
        std::fs::remove_file(d.join("server/src/sidecar/bad.rs")).unwrap();
        assert_eq!(g0(&d).0, "pass", "{}", g0(&d).1);
        // a new category (plumbing / tool output / knobs) fails, even with no Lua user
        let text = std::fs::read_to_string(d.join(ALLOW_REL)).unwrap();
        std::fs::write(d.join(ALLOW_REL), format!("{text}[tool] child outputs\n.servers.json\n")).unwrap();
        let (st, msg) = g0(&d);
        assert!(st == "fail" && msg.contains("categories"), "{msg}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

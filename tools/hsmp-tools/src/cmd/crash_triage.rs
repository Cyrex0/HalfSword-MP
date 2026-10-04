//! `hsmp-tools crash-triage`: scan Half Sword's crash dumps, symbolise them with
//! cdb (UE4SS.pdb), classify each one against the known HSMP crash signatures
//! and write a JSON report.
//!
//!     hsmp-tools crash-triage                              # new since the last run (state file)
//!     hsmp-tools crash-triage --since 1790000000000 --out run/crash_triage.json   # gate run
//!     hsmp-tools crash-triage --all --out triage.json      # re-triage every dump
//!     hsmp-tools crash-triage --no-symbolise               # CrashContext xml only (fast)
//!
//! Crash dirs: `%LOCALAPPDATA%\HalfSwordUE5\Saved\Crashes\UECC-*` (`--dir`), each with
//! `UEMinidump.dmp` + `CrashContext.runtime-xml` (+ any `*.log`, whose tail is classified too).
//!
//! "New": with `--since <unix ms | ISO-8601>`, a crash whose dump is newer than that time
//! (and not newer than `--until`, if given: the gate passes its run window);
//! otherwise a crash dir not listed in the state file (`--state`, default
//! `<Crashes>/.hsmp_triage_seen.json`, updated after each run unless `--no-state-update`).
//! Only new crashes are triaged, unless `--all`.
//!
//! Exit code: 0 = no new crash, 1 = new crash(es), 2 = tool error.
//!
//! cdb: `C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe` (`--cdb`), symbol path
//! `<game>/.../Win64/ue4ss;<game>/.../Win64` (local only; `--symsrv` adds the Microsoft symbol
//! server for OS modules). One cdb per dump, killed (by its own PID) after `--timeout-s`.
//! The game exe ships without a PDB, so engine frames resolve to the nearest export
//! (`IAntiLag2Module::operator=` etc.); signatures key on UE4SS/Lua frames, the crashing
//! thread's name and the faulting module instead.

use anyhow::{bail, Context, Result};
use hsmp_tools::paths;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// The signature table, the classifier and the xml parser live in crates/hsmp-diag (the
// launcher classifies bug reports with them).
use hsmp_diag::triage::{classify, frame_module, is_game_thread, parse_xml, Evidence};
pub use hsmp_diag::triage::sig_json;

const DEFAULT_CDB: &str = r"C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe";
const VERSION: u32 = 1;

#[derive(clap::Args)]
pub struct Args {
    /// Crashes dir (default %LOCALAPPDATA%\HalfSwordUE5\Saved\Crashes)
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Seen-state file (default <dir>/.hsmp_triage_seen.json)
    #[arg(long)]
    state: Option<PathBuf>,
    /// Only crashes newer than this (unix ms, or ISO-8601 like 2030-01-31T22:00:00Z) are new
    #[arg(long)]
    since: Option<String>,
    /// With --since: only crashes up to this time (same formats) are new, e.g. the end of a gate run
    #[arg(long)]
    until: Option<String>,
    /// Triage every crash dir (the exit code still counts only new ones)
    #[arg(long)]
    all: bool,
    /// Write the JSON report here (default: stdout)
    #[arg(long)]
    out: Option<PathBuf>,
    /// cdb.exe path
    #[arg(long)]
    cdb: Option<PathBuf>,
    /// Extra symbol dirs (';'-separated), prepended to the UE4SS + Win64 dirs
    #[arg(long)]
    sympath: Option<String>,
    /// Also use the Microsoft public symbol server (network; slow on first use)
    #[arg(long)]
    symsrv: bool,
    /// Per-dump cdb timeout, seconds
    #[arg(long, default_value_t = 90)]
    timeout_s: u64,
    /// Do not run cdb: classify from CrashContext.runtime-xml + logs only
    #[arg(long)]
    no_symbolise: bool,
    /// Do not record triaged dirs in the state file
    #[arg(long)]
    no_state_update: bool,
    /// No human summary on stderr
    #[arg(long)]
    quiet: bool,
}

// ------------------------------------------------------------------------------------------
// cdb
// ------------------------------------------------------------------------------------------

#[derive(Default, Debug, Clone)]
pub struct CdbInfo {
    pub code: String,
    pub desc: String,
    pub addr: String,
    pub module: String,
    pub thread_id: String,
    pub thread: String,
    pub frames: Vec<String>,
    /// Other non-game threads executing Lua at crash time ("#<n> <tid> <name>").
    pub lua_threads: Vec<String>,
}

/// A stack that is executing Lua (not just a LuaMod thread sleeping in update_async).
const LUA_RUNNING: &str = r"(?i)luaV_execute|luaD_precall|luaD_pcall|lua_pcallk|LuaMadeSimple::Lua::call_function|process_lua_function";

pub fn parse_cdb(out: &str) -> CdbInfo {
    let mut c = CdbInfo::default();
    let exc = Regex::new(r"(?m)^\(([0-9a-fA-F]+)\.([0-9a-fA-F]+)\): (.+?) - code ([0-9a-fA-F]{8})").unwrap();
    if let Some(m) = exc.captures(out) {
        c.thread_id = m[2].to_ascii_lowercase();
        c.desc = m[3].trim().to_string();
        c.code = format!("0x{}", m[4].to_ascii_lowercase());
    }
    let rip = Regex::new(r"(?m)\brip=([0-9a-fA-F]{16})").unwrap();
    // Context after ".ecxr": the last rip= before "Call Site".
    let cs_at = out.find("Call Site").unwrap_or(out.len());
    if let Some(m) = rip.captures_iter(&out[..cs_at]).last() {
        c.addr = format!("0x{}", m[1].to_ascii_lowercase());
    }
    // the marker on a line of its own (the echoed command line also contains it)
    let thr_mark = Regex::new(r"(?m)^===THREADS===").unwrap().find(out).map(|m| m.start()).unwrap_or(out.len());
    if cs_at < thr_mark {
        for l in out[cs_at..thr_mark].lines().skip(1) {
            let l = l.trim();
            if l.is_empty() || l.starts_with("0:") || l.starts_with("===") || l.contains("quit:") {
                break;
            }
            if l.contains('!') || Regex::new(r"^[\w.\-]+\+0x").unwrap().is_match(l) || l.starts_with("0x") {
                c.frames.push(l.to_string());
            }
        }
    }
    c.module = c.frames.first().map(|f| frame_module(f)).unwrap_or_default();
    // "   0  Id: 10260.107a0 Suspend: 0 Teb: ... Unfrozen "GameThread""
    let th = Regex::new(r#"^[ .#]\s*(\d+)\s+Id: [0-9a-fA-F]+\.([0-9a-fA-F]+) .*?(?:"([^"]*)")?\s*$"#).unwrap();
    let running = Regex::new(LUA_RUNNING).unwrap();
    // (num, tid, name, frames)
    let mut threads: Vec<(String, String, String, String)> = vec![];
    for l in out[thr_mark.min(out.len())..].lines() {
        let l = l.trim_end();
        if let Some(m) = th.captures(l) {
            threads.push((m[1].to_string(), m[2].to_ascii_lowercase(), m.get(3).map(|x| x.as_str().to_string()).unwrap_or_default(), String::new()));
        } else if let Some(t) = threads.last_mut() {
            t.3.push_str(l);
            t.3.push('\n');
        }
    }
    for (num, tid, name, frames) in &threads {
        if tid.eq_ignore_ascii_case(&c.thread_id) {
            c.thread = name.clone();
        } else if !is_game_thread(name) && running.is_match(frames) {
            c.lua_threads.push(format!("#{num} {tid} {}", if name.is_empty() { "<unnamed>" } else { name }).trim().to_string());
        }
    }
    c
}


fn run_cdb(cdb: &Path, dump: &Path, sympath: &str, timeout: Duration) -> Result<String> {
    let mut child = Command::new(cdb)
        .arg("-z")
        .arg(dump)
        .arg("-y")
        .arg(sympath)
        .arg("-c")
        .arg(".ecxr; kc 60; .echo ===THREADS===; ~*kc 30; q")
        .env_remove("_NT_SYMBOL_PATH")
        .env_remove("_NT_ALT_SYMBOL_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", cdb.display()))?;
    let mut so = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let t0 = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if t0.elapsed() > timeout {
            let _ = child.kill(); // our own child, by handle/PID
            let _ = child.wait();
            let _ = reader.join();
            bail!("cdb timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let v = reader.join().unwrap_or_default();
    Ok(String::from_utf8_lossy(&v).into_owned())
}

// ------------------------------------------------------------------------------------------
// Time helpers (no chrono)
// ------------------------------------------------------------------------------------------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn ms_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86400));
    let s = secs.rem_euclid(86400);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", s / 3600, s / 60 % 60, s % 60, ms.rem_euclid(1000))
}

/// Unix ms, or ISO-8601 `YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM]` (no zone = UTC).
pub fn parse_since(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        // seconds if it looks like seconds
        return Ok(if n < 100_000_000_000 { n * 1000 } else { n });
    }
    let re = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?)?\s*(Z|[+-]\d{2}:?\d{2})?$").unwrap();
    let c = re.captures(s).with_context(|| format!("--since: not unix ms or ISO-8601: {s:?}"))?;
    let n = |i: usize| c.get(i).map(|m| m.as_str().parse::<i64>().unwrap_or(0)).unwrap_or(0);
    let mut ms = (days_from_civil(n(1), n(2), n(3)) * 86400 + n(4) * 3600 + n(5) * 60 + n(6)) * 1000;
    if let Some(f) = c.get(7) {
        let mut f = f.as_str().to_string();
        f.truncate(3);
        while f.len() < 3 {
            f.push('0');
        }
        ms += f.parse::<i64>().unwrap_or(0);
    }
    if let Some(z) = c.get(8).map(|m| m.as_str()).filter(|z| *z != "Z") {
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let digits: String = z[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        let (h, m) = (digits[..2].parse::<i64>().unwrap_or(0), digits[2..].parse::<i64>().unwrap_or(0));
        ms -= sign * (h * 3600 + m * 60) * 1000;
    }
    Ok(ms)
}

fn mtime_ms(p: &Path) -> Option<i64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------
// Scan + report
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CrashDir {
    pub name: String,
    pub path: PathBuf,
    pub dump: Option<PathBuf>,
    pub time_ms: i64,
}

pub fn scan(dir: &Path) -> Vec<CrashDir> {
    let mut v = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return v };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let dump = std::fs::read_dir(&p).ok().and_then(|r| {
            r.flatten().map(|e| e.path()).find(|f| f.extension().map(|x| x.eq_ignore_ascii_case("dmp")).unwrap_or(false))
        });
        let time_ms = dump.as_deref().and_then(mtime_ms).or_else(|| mtime_ms(&p)).unwrap_or(0);
        v.push(CrashDir { name: e.file_name().to_string_lossy().into_owned(), path: p, dump, time_ms });
    }
    v.sort_by(|a, b| a.time_ms.cmp(&b.time_ms).then(a.name.cmp(&b.name)));
    v
}

fn load_seen(p: &Path) -> Map<String, Value> {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("seen").and_then(|s| s.as_object().cloned()))
        .unwrap_or_default()
}

/// Which crash dirs are new: newer than `since` (and not newer than `until`) if given,
/// else not in `seen`.
pub fn is_new_in(c: &CrashDir, since: Option<i64>, until: Option<i64>, seen: &Map<String, Value>) -> bool {
    match since {
        Some(s) => c.time_ms > s && until.map(|u| c.time_ms <= u).unwrap_or(true),
        None => !seen.contains_key(&c.name),
    }
}

fn log_tail(dir: &Path) -> String {
    let mut s = String::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("log")).unwrap_or(false) {
                if let Ok(b) = std::fs::read(&p) {
                    let t = String::from_utf8_lossy(&b);
                    let lines: Vec<&str> = t.lines().collect();
                    s += &lines[lines.len().saturating_sub(60)..].join("\n");
                    s.push('\n');
                }
            }
        }
    }
    s
}

pub struct TriageOpts {
    pub cdb: Option<PathBuf>,
    pub sympath: String,
    pub timeout: Duration,
}

/// Triage one crash dir -> its JSON record.
pub fn triage(c: &CrashDir, new: bool, o: &TriageOpts) -> Value {
    let xml = std::fs::read(c.path.join("CrashContext.runtime-xml")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let xi = parse_xml(&xml);
    let mut error: Option<String> = None;
    let mut cdb_ok = false;
    let mut ci = CdbInfo::default();
    if let (Some(cdb), Some(dump)) = (&o.cdb, &c.dump) {
        match run_cdb(cdb, dump, &o.sympath, o.timeout) {
            Ok(out) => {
                ci = parse_cdb(&out);
                // a walk that failed yields bare "0x0" frames: fall back to the xml stack
                cdb_ok = ci.frames.iter().any(|f| f.contains('!'));
                if !cdb_ok {
                    error = Some("cdb produced no symbolised stack (xml stack used)".into());
                }
            }
            Err(e) => error = Some(format!("{e:#}")),
        }
    } else if c.dump.is_none() {
        error = Some("no .dmp in crash dir".into());
    }
    let frames = if cdb_ok { ci.frames.clone() } else { xi.frames.clone() };
    let thread = if cdb_ok && !ci.thread.is_empty() { ci.thread.clone() } else { xi.crashed_thread.clone() };
    let module = if cdb_ok { ci.module.clone() } else { frames.first().map(|f| frame_module(f)).unwrap_or_default() };
    let text = format!(
        "{} {}\n{}\nCrashType:{}\nIsAssert:{}\nIsEnsure:{}\nIsStall:{}\n{}",
        ci.desc,
        ci.code,
        xi.error_message,
        xi.crash_type,
        xi.is_assert,
        xi.is_ensure,
        xi.is_stall,
        log_tail(&c.path)
    );
    let ev = Evidence {
        frames: frames.clone(),
        rva_frames: xi.frames.clone(),
        module: module.clone(),
        thread: thread.clone(),
        text,
        lua_threads: ci.lua_threads.clone(),
    };
    let sig = classify(&ev);
    let mut rec = json!({
        "dir": c.name,
        "time": ms_to_iso(c.time_ms),
        "time_ms": c.time_ms,
        "new": new,
        "dump": c.dump.as_ref().map(|d| d.to_string_lossy().into_owned()),
        "exception": {
            "code": if ci.code.is_empty() { Value::Null } else { json!(ci.code) },
            "desc": if ci.desc.is_empty() { Value::Null } else { json!(ci.desc) },
            "addr": if ci.addr.is_empty() { Value::Null } else { json!(ci.addr) },
            "module": module,
        },
        "error_message": xi.error_message,
        "crash_type": xi.crash_type,
        "seconds_since_start": xi.seconds_since_start,
        "thread": thread,
        "xml_thread": xi.crashed_thread,
        "other_lua_threads": ci.lua_threads,
        "frames":frames.iter().take(40).cloned().collect::<Vec<_>>(),
        "rva_frames": xi.frames.iter().take(16).cloned().collect::<Vec<_>>(),
        "callstack_hash": xi.callstack_hash,
        "signature": sig_json(sig),
        "cdb_ok": cdb_ok,
    });
    if let Some(e) = error {
        rec["error"] = json!(e);
    }
    rec
}

pub fn default_crashes_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("HalfSwordUE5").join("Saved").join("Crashes"))
}

fn default_sympath(extra: Option<&str>, symsrv: bool) -> String {
    let game = paths::repo_root().ok().and_then(|r| paths::game_dir(&r).ok());
    let mut parts: Vec<String> = vec![];
    if let Some(e) = extra {
        parts.extend(e.split(';').filter(|s| !s.is_empty()).map(String::from));
    }
    if let Some(g) = game {
        let ue = paths::ue4ss_dir(&g);
        parts.push(ue.to_string_lossy().into_owned());
        if let Some(w) = ue.parent() {
            parts.push(w.to_string_lossy().into_owned());
        }
    }
    if symsrv {
        let cache = std::env::temp_dir().join("hsmp_symcache");
        parts.push(format!("srv*{}*https://msdl.microsoft.com/download/symbols", cache.display()));
    }
    parts.join(";")
}

pub fn summary(records: &[Value], total: usize) -> Value {
    let mut by: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_new: BTreeMap<String, u64> = BTreeMap::new();
    let mut new = 0;
    for r in records {
        let id = r["signature"]["id"].as_str().unwrap_or("?").to_string();
        *by.entry(id.clone()).or_default() += 1;
        if r["new"].as_bool() == Some(true) {
            new += 1;
            *by_new.entry(id).or_default() += 1;
        }
    }
    json!({"total": total, "triaged": records.len(), "new": new, "by_signature": by, "new_by_signature": by_new})
}

pub fn run(a: Args) -> Result<i32> {
    let dir = match a.dir.clone().or_else(default_crashes_dir) {
        Some(d) => d,
        None => bail!("no --dir and LOCALAPPDATA is not set"),
    };
    let since = a.since.as_deref().map(parse_since).transpose()?;
    let until = a.until.as_deref().map(parse_since).transpose()?;
    if until.is_some() && since.is_none() {
        bail!("--until needs --since");
    }
    let state = a.state.clone().unwrap_or_else(|| dir.join(".hsmp_triage_seen.json"));
    let mut seen = load_seen(&state);
    let crashes = if dir.is_dir() { scan(&dir) } else { vec![] };

    let cdb = if a.no_symbolise {
        None
    } else {
        let p = a.cdb.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_CDB));
        if p.is_file() {
            Some(p)
        } else {
            if !a.quiet {
                eprintln!("crash-triage: cdb not found at {} - classifying from CrashContext xml only", p.display());
            }
            None
        }
    };
    let o = TriageOpts { cdb, sympath: default_sympath(a.sympath.as_deref(), a.symsrv), timeout: Duration::from_secs(a.timeout_s) };

    let mut records = vec![];
    for c in &crashes {
        let new = is_new_in(c, since, until, &seen);
        if !(new || a.all) {
            continue;
        }
        let rec = triage(c, new, &o);
        if !a.quiet {
            eprintln!(
                "{} {} {:<28} {:<10} {}",
                if new { "NEW" } else { "   " },
                rec["time"].as_str().unwrap_or(""),
                rec["signature"]["id"].as_str().unwrap_or(""),
                rec["thread"].as_str().unwrap_or(""),
                c.name
            );
        }
        seen.insert(c.name.clone(), json!(rec["signature"]["id"]));
        records.push(rec);
    }
    let sum = summary(&records, crashes.len());
    let report = json!({
        "tool": "hsmp-tools crash-triage",
        "version": VERSION,
        "crashes_dir": dir.to_string_lossy(),
        "generated": ms_to_iso(now_ms()),
        "since": since.map(ms_to_iso),
        "until": until.map(ms_to_iso),
        "symbolised": o.cdb.is_some(),
        "sympath": o.sympath,
        "crashes": records,
        "summary": sum,
    });
    let text = serde_json::to_string_pretty(&report)?;
    match &a.out {
        Some(p) => {
            if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, text.as_bytes()).with_context(|| format!("write {}", p.display()))?;
        }
        None => println!("{text}"),
    }
    if !a.no_state_update && dir.is_dir() {
        let doc = json!({"tool": "hsmp-tools crash-triage", "updated": ms_to_iso(now_ms()), "seen": Value::Object(seen)});
        if let Err(e) = std::fs::write(&state, serde_json::to_string_pretty(&doc)?.as_bytes()) {
            if !a.quiet {
                eprintln!("crash-triage: cannot write state {}: {e}", state.display());
            }
        }
    }
    let new = sum["new"].as_u64().unwrap_or(0);
    if !a.quiet {
        eprintln!("crash-triage: {} crash dir(s), {} triaged, {} new; {}", crashes.len(), records.len(), new, sum["by_signature"]);
    }
    Ok(if new > 0 { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_diag::triage::{strip_dispatch_frames, SIGS};

    fn ev(thread: &str, module: &str, frames: &[&str], text: &str) -> Evidence {
        Evidence { frames: frames.iter().map(|s| s.to_string()).collect(), rva_frames: vec![], module: module.into(), thread: thread.into(), text: text.into(), lua_threads: vec![] }
    }

    #[test]
    fn classifies_softobject() {
        let e = ev(
            "GameThread",
            "VCRUNTIME140",
            &[
                "VCRUNTIME140!_NLG_Return2",
                "UE4SS!RC::Unreal::FString::operator=",
                "UE4SS!RC::Unreal::FSoftObjectPath::operator=",
                "UE4SS!RC::LuaType::push_softobjectproperty",
                "UE4SS!luaD_precall",
                "UE4SS!RC::lua_unreal_script_function_hook_pre",
            ],
            "Access violation c0000005",
        );
        assert_eq!(classify(&e).id, "SOFTOBJECT_PARAM_READ");
    }

    #[test]
    fn classifies_offthread_lua() {
        // unnamed thread (UE4SS async threads have no name) = not the game thread
        let e = ev("", "UE4SS", &["UE4SS!lua_next", "UE4SS!luaB_next", "UE4SS!luaD_precall", "UE4SS!luaV_execute"], "Access violation");
        assert_eq!(classify(&e).id, "LUA_VM_OFFTHREAD");
        let e = ev("Thread 7", "UE4SS", &["UE4SS!lua_next", "UE4SS!luaB_next", "UE4SS!luaV_execute"], "Access violation");
        assert_eq!(classify(&e).id, "LUA_VM_OFFTHREAD");
        // game thread crashed while an unnamed UE4SS thread was running Lua too
        let mut e = ev("GameThread", "UE4SS", &["UE4SS!RC::LuaMadeSimple::Lua::new_metatable", "UE4SS!luaD_precall", "UE4SS!RC::process_simple_actions"], "");
        assert_eq!(classify(&e).id, "LUA_UOBJECT_ACCESS");
        e.lua_threads = vec!["#191 13d54 <unnamed>".into()];
        assert_eq!(classify(&e).id, "LUA_VM_OFFTHREAD");
        // VM internals at the top of a game-thread stack
        let e = ev("GameThread", "UE4SS", &["UE4SS!lua_getiuservalue", "UE4SS!RC::LuaMadeSimple::Lua::get_userdata<X>", "UE4SS!RC::LuaType::push_nameproperty", "UE4SS!luaV_execute"], "");
        assert_eq!(classify(&e).id, "LUA_VM_CORRUPTION");
        let e = ev("GameThread", "UE4SS", &["UE4SS!traverseLclosure", "UE4SS!propagatemark", "UE4SS!luaC_step", "UE4SS!luaV_execute"], "");
        assert_eq!(classify(&e).id, "LUA_VM_CORRUPTION");
        let e = ev("", "windows.storage", &["windows.storage+0x6be4d940", "HalfSwordUE5-Win64-Shipping+0x39afac4"], "Unhandled Exception: EXCEPTION_ACCESS_VIOLATION 0x00000180c09cd940\n");
        assert_eq!(classify(&e).id, "EXEC_BAD_ADDRESS");
    }

    #[test]
    fn classifies_stale_uobject() {
        let e = ev(
            "GameThread",
            "UE4SS",
            &[
                "UE4SS!RC::Unreal::UStruct::FindProperty",
                "UE4SS!RC::LuaType::UObjectBase<RC::Unreal::UObject,RC::LuaType::UObjectName>::setup_metamethods::__l2::<lambda_2>::operator()",
                "UE4SS!luaV_finishset",
                "UE4SS!luaV_execute",
                "UE4SS!RC::LuaMod::process_simple_actions",
            ],
            "",
        );
        assert_eq!(classify(&e).id, "STALE_UOBJECT_AFTER_TRAVEL");
    }

    #[test]
    fn classifies_leader_pose_and_physics_and_misc() {
        let e = ev("GameThread", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!USkinnedMeshComponent::SetLeaderPoseComponent", "UE4SS!luaV_execute"], "");
        assert_eq!(classify(&e).id, "LEADER_POSE_SKSKELETON");
        let e = ev(
            "Foreground Worker #1",
            "HalfswordUE5_Win64_Shipping",
            &["HalfswordUE5_Win64_Shipping!IAntiLag2Module::operator=", "HalfswordUE5_Win64_Shipping!nsvgRasterizeFull", "kernel32!BaseThreadInitThunk"],
            "Access violation - code c0000005",
        );
        assert_eq!(classify(&e).id, "PHYSICS_DYING_WORLD");
        let e = ev("RHIThread", "d3d12core", &["d3d12core!x"], "DXGI_ERROR_DEVICE_REMOVED");
        assert_eq!(classify(&e).id, "GPU_DEVICE_REMOVED");
        let e = ev("RenderThread 0", "HalfSwordUE5-Win64-Shipping", &["HalfSwordUE5-Win64-Shipping+0x1234"], "Access violation");
        assert_eq!(classify(&e).id, "RENDER_THREAD_NATIVE");
        // an RVP worker thread is the RVP family; other plugin threads stay generic
        let e = ev("Runtime Vertex Paint and Detection Plugin Thread #10", "HalfSwordUE5-Win64-Shipping", &["HalfSwordUE5-Win64-Shipping+0x1"], "Access violation");
        assert_eq!(classify(&e).id, "RVP_WORKER_AFTER_TRAVEL");
        let e = ev("Some Other Plugin Thread", "HalfSwordUE5-Win64-Shipping", &["HalfSwordUE5-Win64-Shipping+0x1"], "Access violation");
        assert_eq!(classify(&e).id, "PLUGIN_THREAD_NATIVE");
        // the exact RVP game-thread site
        let mut e = ev("GameThread", "HalfswordUE5_Win64_Shipping",
            &["HalfswordUE5_Win64_Shipping!src_strerror", "HalfswordUE5_Win64_Shipping!ffxPrintMessage",
              "UE4SS!RC::Unreal::Hook::Internal::TDetourInstance<9007199254740992,std::function<void __cdecl(RC::Unreal::UEngine *,float,bool)> >::Invoke"],
            "Unhandled Exception: EXCEPTION_ACCESS_VIOLATION reading address 0x000001e9b78ed010");
        e.rva_frames = vec!["HalfswordUE5-Win64-Shipping+0x4a23c64".into(), "HalfswordUE5-Win64-Shipping+0x112e775".into()];
        assert_eq!(classify(&e).id, "RVP_TASK_AFTER_TRAVEL");
        assert_eq!(classify(&e).confidence, "high");
        e.rva_frames = vec!["HalfswordUE5-Win64-Shipping+0x4a23c640".into()];
        assert_ne!(classify(&e).id, "RVP_TASK_AFTER_TRAVEL");
        // the IoDispatcher family, then the exact ReadRequest site via the xml RVA frames
        let mut e = ev("IoDispatcher", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!src_strerror", "kernel32!BaseThreadInitThunk"], "EXCEPTION_ACCESS_VIOLATION reading address 0xffffffffffffffff");
        assert_eq!(classify(&e).id, "PAK_ASYNC_READ_OOB");
        assert_eq!(classify(&e).confidence, "medium");
        e.rva_frames = vec!["HalfswordUE5-Win64-Shipping+0x21e5ca4".into(), "HalfswordUE5-Win64-Shipping+0x1444293".into()];
        assert_eq!(classify(&e).id, "PAK_ASYNC_READ_OOB");
        assert_eq!(classify(&e).confidence, "high");
        // another RVA prefix must not match the exact site
        e.rva_frames = vec!["HalfswordUE5-Win64-Shipping+0x21e5ca40".into()];
        assert_eq!(classify(&e).confidence, "medium");
        // a worker thread with the same fault stays PHYSICS_DYING_WORLD
        let e = ev("Background Worker #3", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!x"], "Access violation");
        assert_eq!(classify(&e).id, "PHYSICS_DYING_WORLD");
        let e = ev("GameThread", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!x"], "Ran out of memory allocating 123 bytes");
        assert_eq!(classify(&e).id, "OUT_OF_MEMORY");
        let e = ev("GameThread", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!x"], "Assertion failed: IsValid(Obj)");
        assert_eq!(classify(&e).id, "ASSERTION");
        let e = ev("GameThread", "HSMPNative", &["HSMPNative!foo"], "Access violation");
        assert_eq!(classify(&e).id, "HSMP_NATIVE_DLL");
        let e = ev("GameThread", "UE4SS", &["UE4SS!PLH::x64Detour::hook", "UE4SS!RC::Unreal::Hook::Internal::TDetourInstance<1>::Invoke"], "");
        assert_eq!(classify(&e).id, "UE4SS_HOOK_CRASH");
        let e = ev("GameThread", "UE4SS", &["UE4SS!RC::something", "UE4SS!RC::other"], "");
        assert_eq!(classify(&e).id, "UE4SS_FAULT_OTHER");
        let e = ev("GameThread", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!a", "UE4SS!luaV_execute", "UE4SS!luaD_precall"], "");
        assert_eq!(classify(&e).id, "NATIVE_FROM_LUA_CALL");
        let e = ev("GameThread", "HalfswordUE5_Win64_Shipping", &["HalfswordUE5_Win64_Shipping!a"], "");
        assert_eq!(classify(&e).id, "GAME_THREAD_NATIVE");
        let e = ev("GameThread", "", &[], "");
        assert_eq!(classify(&e).id, "UNKNOWN");
        let e = ev("GameThread", "x", &[], "IsStall:true");
        assert_eq!(classify(&e).id, "HANG_STALL");
    }

    #[test]
    fn every_signature_regex_compiles_and_ids_unique() {
        let mut ids = std::collections::BTreeSet::new();
        for s in SIGS {
            for r in [s.frames, s.text, s.module, s.not_frames].into_iter().flatten() {
                Regex::new(r).unwrap();
            }
        }
        // an id may repeat only as consecutive variants of one signature
        let mut last = "";
        for s in SIGS {
            if s.id != last {
                assert!(ids.insert(s.id), "non-consecutive duplicate {}", s.id);
            }
            last = s.id;
        }
    }

    const CDB_SAMPLE: &str = r#"Debug session time: Thu Oct  1 22:39:16.000 2026 (UTC + 1:00)
(10260.107a0): Access violation - code c0000005 (first/second chance not available)
For analysis of this file, run !analyze -v
ntdll!NtWaitForSingleObject+0x14:
00007ffd`f23c0e44 c3              ret
0:000> cdb: Reading initial command '.ecxr; kc 60; .echo ===THREADS===; ~; q'
rax=000002d36f889c80 rbx=000000ec7cd7b9f0 rcx=000002d36f889c80
rip=00007ffdd189cba8 rsp=000000ec7cd7b908 rbp=000000ec7cd7ba70
VCRUNTIME140!_NLG_Return2+0xf78:
00007ffd`d189cba8 0fb70a          movzx   ecx,word ptr [rdx] ds:00000000`00000000=????
Call Site
VCRUNTIME140!_NLG_Return2
UE4SS!RC::Unreal::FSoftObjectPath::operator=
UE4SS!RC::LuaType::push_softobjectproperty
UE4SS!luaD_precall
===THREADS===
.  0  Id: 10260.107a0 Suspend: 0 Teb: 000000ec`7c04a000 Unfrozen "GameThread"
Call Site
ntdll!NtWaitForSingleObject

   1  Id: 10260.1234 Suspend: 0 Teb: 000000ec`7c04c000 Unfrozen "Foreground Worker #0"
Call Site
ntdll!NtWaitForSingleObject

 185  Id: 10260.13b44 Suspend: 0 Teb: 00000070`bbc42000 Unfrozen
Call Site
KERNELBASE!SleepEx
UE4SS!RC::LuaMod::update_async

 191  Id: 10260.13d54 Suspend: 0 Teb: 00000070`bbc42000 Unfrozen
Call Site
UE4SS!luaV_execute
UE4SS!RC::LuaMod::update_async
quit:
"#;

    #[test]
    fn parses_cdb_output() {
        let c = parse_cdb(CDB_SAMPLE);
        assert_eq!(c.code, "0xc0000005");
        assert_eq!(c.thread_id, "107a0");
        assert_eq!(c.thread, "GameThread");
        assert_eq!(c.addr, "0x00007ffdd189cba8");
        assert_eq!(c.frames.len(), 4);
        assert_eq!(c.module, "VCRUNTIME140");
        assert_eq!(c.lua_threads, vec!["#191 13d54 <unnamed>"]);
    }

    #[test]
    fn strips_crash_reporter_frames() {
        let f: Vec<String> = ["ntdll+0x1", "KERNELBASE+0x2", "Game+0x3", "VCRUNTIME140+0x4", "ntdll+0x5", "ntdll+0x6", "UE4SS+0x7", "UE4SS+0x8"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(strip_dispatch_frames(f), vec!["UE4SS+0x7", "UE4SS+0x8"]);
    }

    const XML_SAMPLE: &str = "<FGenericCrashContext><RuntimeProperties><IsEnsure>false</IsEnsure><IsStall>false</IsStall><IsAssert>false</IsAssert><CrashType>Crash</CrashType>\
<ErrorMessage>Unhandled Exception: EXCEPTION_ACCESS_VIOLATION reading address 0x0000000000000000</ErrorMessage><SecondsSinceStart>10</SecondsSinceStart>\
<Threads><Thread><CallStack>ntdll 0x00007ffdf2260000 + 160e44\n</CallStack><IsCrashed>false</IsCrashed><ThreadName>RHIThread</ThreadName></Thread>\
<Thread><CallStack>VCRUNTIME140                 0x00007ffdd1880000 + 1cba8           \nUE4SS                        0x00007ffd2c7f0000 + 234560          \n</CallStack><IsCrashed>true</IsCrashed><ThreadName>GameThread</ThreadName></Thread></Threads>\
</RuntimeProperties></FGenericCrashContext>";

    #[test]
    fn parses_xml() {
        let x = parse_xml(XML_SAMPLE);
        assert_eq!(x.crashed_thread, "GameThread");
        assert_eq!(x.frames, vec!["VCRUNTIME140+0x1cba8", "UE4SS+0x234560"]);
        assert_eq!(x.seconds_since_start, Some(10));
        assert!(x.error_message.contains("ACCESS_VIOLATION"));
        assert_eq!(x.callstack_hash, "");
        let x = parse_xml(&XML_SAMPLE.replace("<SecondsSinceStart>", "<PCallStackHash>44106A9D</PCallStackHash><SecondsSinceStart>"));
        assert_eq!(x.callstack_hash, "44106A9D");
    }

    #[test]
    fn time_parsing() {
        assert_eq!(parse_since("1790000000000").unwrap(), 1790000000000);
        assert_eq!(parse_since("1790000000").unwrap(), 1790000000000);
        assert_eq!(parse_since("1970-01-02T00:00:00Z").unwrap(), 86_400_000);
        assert_eq!(parse_since("1970-01-01T01:00:00+01:00").unwrap(), 0);
        assert_eq!(ms_to_iso(86_400_123), "1970-01-02T00:00:00.123Z");
        let t = parse_since("2026-10-01T22:39:16.5Z").unwrap();
        assert_eq!(ms_to_iso(t), "2026-10-01T22:39:16.500Z");
        assert!(parse_since("yesterday").is_err());
    }

    fn is_new(c: &CrashDir, since: Option<i64>, seen: &Map<String, Value>) -> bool {
        is_new_in(c, since, None, seen)
    }

    #[test]
    fn new_detection_and_report_shape_without_cdb() {
        let tmp = paths::make_temp_dir("hsmp_triage_test_").unwrap();
        for n in ["UECC-A_0000", "UECC-B_0000"] {
            let d = tmp.join(n);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("UEMinidump.dmp"), b"MDMP").unwrap();
            std::fs::write(d.join("CrashContext.runtime-xml"), XML_SAMPLE).unwrap();
        }
        let crashes = scan(&tmp);
        assert_eq!(crashes.len(), 2);
        let mut seen = Map::new();
        seen.insert("UECC-A_0000".into(), json!("X"));
        assert!(!is_new(&crashes.iter().find(|c| c.name == "UECC-A_0000").unwrap().clone(), None, &seen));
        assert!(is_new(&crashes.iter().find(|c| c.name == "UECC-B_0000").unwrap().clone(), None, &seen));
        assert!(is_new(&crashes[0], Some(0), &seen));
        assert!(!is_new(&crashes[0], Some(i64::MAX), &seen));
        // a gate run window: (since, until]
        let t = crashes[0].time_ms;
        assert!(is_new_in(&crashes[0], Some(t - 1), Some(t), &seen));
        assert!(!is_new_in(&crashes[0], Some(t - 10), Some(t - 1), &seen));

        let o = TriageOpts { cdb: None, sympath: String::new(), timeout: Duration::from_secs(1) };
        let recs: Vec<Value> = crashes.iter().map(|c| triage(c, c.name.ends_with("B_0000"), &o)).collect();
        for r in &recs {
            for k in ["dir", "time", "dump", "exception", "thread", "frames", "signature", "cdb_ok"] {
                assert!(r.get(k).is_some(), "missing {k}");
            }
            assert_eq!(r["thread"], "GameThread");
            assert_eq!(r["cdb_ok"], false);
            assert!(r["signature"]["id"].is_string());
        }
        let s = summary(&recs, 2);
        assert_eq!(s["total"], 2);
        assert_eq!(s["new"], 1);

        // full run: state file written, second run finds nothing new
        let out = tmp.join("r.json");
        let args = |all: bool| Args {
            dir: Some(tmp.clone()),
            state: None,
            since: None,
            until: None,
            all,
            out: Some(out.clone()),
            cdb: None,
            sympath: None,
            symsrv: false,
            timeout_s: 1,
            no_symbolise: true,
            no_state_update: false,
            quiet: true,
        };
        assert_eq!(run(args(false)).unwrap(), 1);
        let rep: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(rep["summary"]["new"], 2);
        assert_eq!(run(args(false)).unwrap(), 0);
        assert_eq!(run(args(true)).unwrap(), 0);
        let rep: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(rep["summary"]["triaged"], 2);
        assert_eq!(rep["summary"]["new"], 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

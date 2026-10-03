//! G0 lints (Lua syntax is `hsmp-tools lua-check`):
//! * travel lint (DoD-4): only HSMPMatch (the Director) may change the level;
//! * image-name kill grep (DoD-12): processes are killed by recorded PID only;
//!
//! `travel_lint` is the simple built-in rule behind `hsmp-gate lint`; G0 runs the
//! `check_travel` binary instead, which also knows the allow-list.

use regex::Regex;
use std::path::{Path, PathBuf};

/// Mods that may change the level.
pub const TRAVEL_OWNERS: &[&str] = &["HSMPMatch"];
/// Retired mods: never deployed, not linted.
pub const RETIRED: &[&str] = &["HSMPLobby", "HSMPAdmin", "HSMPCharacter", "HSMPSettings", "HSMPChat"];

fn walk(dir: &Path, exts: &[&str], skip_dirs: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if p.is_dir() {
            if !skip_dirs.contains(&name.as_str()) && !name.starts_with("target") {
                walk(&p, exts, skip_dirs, out);
            }
        } else if exts.iter().any(|e| name.ends_with(e)) {
            out.push(p);
        }
    }
}

pub fn mod_lua_files(repo: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    walk(&repo.join("mods"), &[".lua"], &[], &mut v);
    v
}

pub use hsmp_tools::lualex::code_lines;

/// Raw lines with only comments removed (strings kept) - for console "open <map>" strings.
fn lines_without_comments(src: &str) -> Vec<String> {
    let code = code_lines(src);
    let raw: Vec<&str> = src.split('\n').collect();
    raw.iter()
        .zip(code.iter())
        .map(|(r, c)| {
            // keep the raw text up to where the code line ends (comment start)
            let r = r.trim_end_matches('\r');
            if c.trim().is_empty() { String::new() } else { r.to_string() }
        })
        .collect()
}

pub struct Hit {
    pub file: String,
    pub line: usize,
    pub what: String,
    pub code: String,
}

pub fn travel_hits(repo: &Path) -> Option<Vec<Hit>> {
    if !hsmp_tools::paths::has_mods(repo) {
        return None;
    }
    let call = Regex::new(r"[:.]\s*(OpenLevel|OpenLevelBySoftObjectPtr|ServerTravel|ClientTravel)\s*\(").unwrap();
    // console level change: a string starting with "open "/"travel " on a line that builds an
    // FString or calls a console-command function (case-sensitive: the armour catalogue has
    // display names like "Open Sallet A1")
    let console = Regex::new(r#"["'](open|travel|servertravel)\s"#).unwrap();
    let console_call = Regex::new(r"(ExecuteConsoleCommand|ConsoleCommand|ExecConsole|FString)\s*\(").unwrap();
    let mut hits = vec![];
    for p in mod_lua_files(repo) {
        let rel = p.strip_prefix(repo).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let module = hsmp_tools::paths::mod_of_rel(&rel);
        if TRAVEL_OWNERS.contains(&module) || RETIRED.contains(&module) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        let code = code_lines(&src);
        let nocomment = lines_without_comments(&src);
        for (n, line) in code.iter().enumerate() {
            let raw = nocomment.get(n).cloned().unwrap_or_default();
            if let Some(m) = call.captures(line) {
                hits.push(Hit { file: rel.clone(), line: n + 1, what: format!("{}()", &m[1]), code: raw.trim().to_string() });
            } else if console_call.is_match(line) {
                // strings are blanked in `line`; look at the comment-free raw text
                let raw_code = raw.split("--").next().unwrap_or("");
                if console.is_match(raw_code) {
                    hits.push(Hit { file: rel.clone(), line: n + 1, what: "console open/travel".into(), code: raw.trim().to_string() });
                }
            }
        }
    }
    Some(hits)
}

/// (ok, one-line summary, detail lines). "skipped" summary when the repo has no mods.
pub fn travel_lint(repo: &Path) -> (bool, String, Vec<String>) {
    let Some(hits) = travel_hits(repo) else { return (true, "skipped".into(), vec![]) };
    let detail: Vec<String> = hits.iter().map(|h| format!("{}:{}: {} outside HSMPMatch: {}", h.file, h.line, h.what, h.code.chars().take(140).collect::<String>())).collect();
    if hits.is_empty() {
        (true, "ok - no level change outside HSMPMatch".into(), detail)
    } else {
        let mut mods: Vec<String> = hits.iter().map(|h| hsmp_tools::paths::mod_of_rel(&h.file).to_string()).collect();
        mods.sort();
        mods.dedup();
        (false, format!("FAIL - {} level change(s) outside HSMPMatch ({})", hits.len(), mods.join(", ")), detail)
    }
}

/// File types the kill-by-name lint reads (.py too, though the repo has none).
const KILL_EXTS: &[&str] = &[".lua", ".ps1", ".psm1", ".rs", ".sh", ".bash", ".cmd", ".bat", ".cs", ".py", "pre-push"];

/// The kill-by-name patterns. Every literal is split so this file never flags itself.
struct KillRx {
    tk_im: Regex,
    tk_image: Regex,
    tk_pid: Regex,
    stop_name: Regex,
    getproc_stop: Regex,
    getproc_kill: Regex,
    procname_sel: Regex,
    stops: Regex,
    posix: Regex,
    tk_word: Regex,
    im_lit: Regex,
    wmic: Regex,
    cim_name: Regex,
    terminate: Regex,
}

fn kill_rx() -> KillRx {
    let tk = ["task", "kill"].concat();
    let sp = ["Stop", "-Process"].concat();
    let gp = ["Get", "-Process"].concat();
    let pn = ["Process", "Name"].concat();
    let r = |s: String| Regex::new(&s).unwrap();
    KillRx {
        // taskkill ... /IM <image>
        tk_im: r(format!(r"(?i)\b{tk}\b[^\n]*?/IM\b")),
        // taskkill /FI "IMAGENAME eq x" without /PID selects every process of that image
        tk_image: r(format!(r"(?i)\b{tk}\b[^\n]*?IMAGENAME\s+eq\b")),
        tk_pid: r(r"(?i)/PID\b".to_string()),
        // Stop-Process / spps / kill ... -Name / -ProcessName / any unique prefix of -Name (-N, -Na, -Nam)
        stop_name: r(format!(r"(?i)(\b{sp}\b|\bspps\b|(^|[\s;|({{])kill\b)[^\n]*?\s-({pn}|N|Na|Nam|Name)\b")),
        // Get-Process <name> | Stop-Process (a -Id lookup starts with '-', not a name)
        getproc_stop: r(format!(r#"(?i)\b{gp}\s+(-(Process)?Name\s+)?["']?[A-Za-z*$][^|\n]*\|[^\n]*(\b{sp}\b|\.Kill\(\))"#)),
        // (Get-Process <name>).Kill()
        getproc_kill: r(format!(r#"(?i)\(\s*{gp}\s+(-(Process)?Name\s+)?["']?[A-Za-z*$][^)\n]*\)\s*\.\s*Kill\s*\("#)),
        // a process selected by its name: .ProcessName -match / -like / -eq / -in / -contains
        procname_sel: r(format!(r"(?i)\b{pn}\s*-c?i?(match|like|eq|in|contains)\b")),
        stops: r(format!(r"(?i)(\b{sp}\b|\bspps\b|\.Kill\(\)|\b{tk}\b|\bTerminate\b)")),
        // POSIX: pkill / killall / kill $(pgrep|pidof ...)
        posix: r([r"(?i)(\b(p", "kill|kill", r"all)\b|\bkill\b[^\n]*\$\(\s*(pg", "rep|pid", r"of)\b)"].concat()),
        tk_word: r(format!(r"(?i)\b{tk}\b")),
        // a quoted /IM argument (held in a variable, or Rust `.args(["/IM", ..])` next to a
        // Command::new("taskkill") on another line)
        im_lit: r(r#"(?i)["']\s*/IM\b"#.to_string()),
        // wmic process where name=... delete | call terminate
        wmic: r([r"(?i)\bwm", r"ic\b[^\n]*\bprocess\b[^\n]*\bname\b[^\n]*\b(delete|terminate)\b"].concat()),
        // CIM / WMI process selected by its name ... Terminate
        cim_name: r(r#"(?i)\bWin32_Process\b[^\n]*\bName\s*(=|like)"#.to_string()),
        terminate: r(r"(?i)(\bTerminate\b)".to_string()),
    }
}

fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("--") || t.starts_with('#') || t.starts_with("//") || t.to_ascii_lowercase().starts_with("rem ") || t.starts_with("::")
}

/// Physical lines joined across continuations (PowerShell backtick, cmd caret, shell
/// backslash): (first physical line index, logical line).
fn logical_lines(lines: &[&str]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = vec![];
    let mut cont = false;
    for (n, l) in lines.iter().enumerate() {
        let t = l.trim_end();
        let (body, more) = match t.chars().last() {
            Some('`') | Some('^') | Some('\\') if !is_comment_line(t) => (&t[..t.len() - 1], true),
            _ => (t, false),
        };
        if cont {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(body.trim_start());
            }
        } else {
            out.push((n, body.to_string()));
        }
        cont = more;
    }
    out
}

/// Kill-by-name hits in one file's text: (1-based line, what).
fn kill_hits_in(text: &str, rx: &KillRx) -> Vec<(usize, &'static str)> {
    let phys: Vec<&str> = text.lines().collect();
    let lines = logical_lines(&phys);
    let file_has_tk = phys.iter().any(|l| !is_comment_line(l) && rx.tk_word.is_match(l));
    let mut hits = vec![];
    for (k, (n, line)) in lines.iter().enumerate() {
        if is_comment_line(line) {
            continue;
        }
        let near = |w: usize| lines[k.saturating_sub(w)..(k + w + 1).min(lines.len())].iter().filter(|(_, l)| !is_comment_line(l));
        let what = if rx.tk_im.is_match(line) {
            Some("kill tool with /IM")
        } else if rx.tk_image.is_match(line) && !rx.tk_pid.is_match(line) {
            Some("kill tool IMAGENAME filter without a PID")
        } else if rx.stop_name.is_match(line) {
            Some("Stop-Process by name")
        } else if rx.getproc_stop.is_match(line) {
            Some("Get-Process <name> | Stop-Process")
        } else if rx.getproc_kill.is_match(line) {
            Some("(Get-Process <name>).Kill()")
        } else if rx.procname_sel.is_match(line) && lines[k..(k + 3).min(lines.len())].iter().any(|(_, l)| !is_comment_line(l) && rx.stops.is_match(l)) {
            Some("ProcessName filter -> stop")
        } else if rx.posix.is_match(line) {
            Some("posix kill by name")
        } else if file_has_tk && rx.im_lit.is_match(line) {
            Some("a quoted image switch (IM) for the kill tool")
        } else if rx.wmic.is_match(line) {
            Some("wm-ic process selected by name -> delete or end")
        } else if rx.terminate.is_match(line) && (rx.cim_name.is_match(line) || near(3).any(|(_, l)| rx.cim_name.is_match(l))) {
            Some("a CIM process selected by name -> its end method")
        } else {
            None
        };
        if let Some(w) = what {
            hits.push((n + 1, w));
        }
    }
    hits
}

/// Lines that kill processes by image name (DoD-12): taskkill /IM, taskkill IMAGENAME without
/// /PID, Stop-Process -Name/-ProcessName/-N, Get-Process <name> | Stop-Process,
/// (Get-Process <name>).Kill(), a ProcessName filter feeding a stop, pkill/killall, a quoted
/// /IM in a file that runs the kill tool, wmic process-by-name delete/terminate, a CIM/WMI
/// Win32_Process name filter feeding Terminate; lines joined across ` ^ \ continuations
/// Scans the repo root (top level) and mod/, scripts/, server/src, tools/
/// (src/bin included), crates/, .githooks.
pub fn image_name_kills(repo: &Path) -> Vec<String> {
    let rx = kill_rx();
    let mut files = vec![];
    if let Ok(rd) = std::fs::read_dir(repo) {
        let mut top: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).map(|n| KILL_EXTS.iter().any(|e| n.ends_with(e))).unwrap_or(false))
            .collect();
        top.sort();
        files.extend(top);
    }
    for root in ["mods", "scripts", "server/src", "tools", "crates", "launcher", "tests", ".githooks"] {
        walk(&repo.join(root), KILL_EXTS, &["fixtures", "obj", "node_modules"], &mut files);
    }
    let mut hits = vec![];
    for p in files {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        for (n, what) in kill_hits_in(&text, &rx) {
            hits.push(format!("{}:{} ({what})", p.strip_prefix(repo).unwrap_or(&p).to_string_lossy().replace('\\', "/"), n));
        }
    }
    hits
}

/// `Instant::now() - <Duration>` panics when the result would be
/// before the monotonic clock's zero (Windows: boot time), so a PC booted < the duration
/// ago kills the process. Flags, in server/src, crates and tools:
/// * `Instant::now() - x` and `Instant::now().sub(x)`, also wrapped over two lines
///   (`Instant::now()` ending a line, the next one starting with `-` / `.sub(`);
/// * the two-statement form: `let t = Instant::now();` (or a `t: Instant` parameter) and later
///   `t - <a Duration>` / `t.sub(..)` in the same file, where the right side looks like a
///   Duration (`Duration::..`, `from_secs/millis/..`, an UPPER_CASE constant, or a name with
///   dur/timeout/window/interval/ttl/grace/period/delay/age in it). `t - other_instant` is a
///   saturating Duration and is not flagged.
/// Known hits are baselined in tools/hsmp-tools/instant_sub.baseline as `path: trimmed line`
/// (line-number free). Each baseline line covers ONE hit in that file (a count), and a stale
/// entry FAILS (a fixed hit must leave the baseline, or the line could come back unseen).
/// Returns (new hits, baselined hits, stale baseline entries).
pub fn instant_sub_hits(repo: &Path) -> (Vec<String>, Vec<String>, Vec<String>) {
    let base_text = std::fs::read_to_string(repo.join("tools/hsmp-tools/instant_sub.baseline")).unwrap_or_default();
    let baseline: Vec<String> = base_text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
    let mut files = vec![];
    for root in ["server/src", "crates", "tools", "launcher", "tests"] {
        walk(&repo.join(root), &[".rs"], &["obj", "node_modules"], &mut files);
    }
    let (mut new, mut old) = (vec![], vec![]);
    let mut used = vec![false; baseline.len()];
    for p in files {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let rel = p.strip_prefix(repo).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        for (n, line) in instant_sub_in(&text) {
            let key = format!("{rel}: {}", line.trim());
            match baseline.iter().enumerate().position(|(i, b)| *b == key && !used[i]) {
                Some(i) => {
                    used[i] = true;
                    old.push(format!("{rel}:{n}"));
                }
                None => new.push(format!("{rel}:{n}: {}", line.trim())),
            }
        }
    }
    let stale: Vec<String> = baseline.iter().zip(used).filter(|(_, u)| !u).map(|(b, _)| b.clone()).collect();
    // a stale entry would silently re-admit the same line later: it fails G0
    new.extend(stale.iter().map(|s| format!("stale baseline entry (its hit is gone: delete it from instant_sub.baseline): {s}")));
    (new, old, stale)
}

/// `(1-based line, line text)` of every instant_sub hit in one Rust source.
fn instant_sub_in(text: &str) -> Vec<(usize, String)> {
    let direct = Regex::new(&[r"Instant::now\(\)\s*(", "-", r"[^=>]|\.\s*sub\s*\()"].concat()).unwrap();
    let ends_now = Regex::new(&[r"Instant::now\(\)\s*$"].concat()).unwrap();
    let starts_sub = Regex::new(&[r"^\s*(", "-", r"[^=>]|\.\s*sub\s*\()"].concat()).unwrap();
    let bind = Regex::new(r"\blet\s+(mut\s+)?([a-z_][a-z0-9_]*)\s*(:\s*[A-Za-z_:]*Instant\s*)?=\s*(std::time::|time::)?Instant::now\(\)\s*;").unwrap();
    let param = Regex::new(r"[(,]\s*([a-z_][a-z0-9_]*)\s*:\s*(std::time::)?Instant\s*[,)]").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let code = |l: &str| !l.trim_start().starts_with("//");
    let mut vars: Vec<String> = vec![];
    for l in lines.iter().filter(|l| code(l)) {
        for c in bind.captures_iter(l) {
            vars.push(c[2].to_string());
        }
        for c in param.captures_iter(l) {
            vars.push(c[1].to_string());
        }
    }
    vars.sort();
    vars.dedup();
    let dur_rhs = Regex::new(r"^\s*(Duration\b|std::time::Duration\b|[A-Za-z_:]*from_(secs|millis|micros|nanos|secs_f32|secs_f64)\b|[A-Z][A-Z0-9_]{2,}\b|[a-z_][a-z0-9_.]*(dur|timeout|window|interval|ttl|grace|period|delay|age)[a-z0-9_]*\b)").unwrap();
    let var_sub: Vec<(Regex, Regex)> = vars
        .iter()
        .map(|v| {
            let e = regex::escape(v);
            (Regex::new(&[r"(^|[^A-Za-z0-9_.])", &e, r"\s*", "-", r"([^=>].*)$"].concat()).unwrap(), Regex::new(&[r"(^|[^A-Za-z0-9_.])", &e, r"\s*\.\s*sub\s*\("].concat()).unwrap())
        })
        .collect();
    let mut hits = vec![];
    for (n, line) in lines.iter().enumerate() {
        if !code(line) {
            continue;
        }
        let wrapped = ends_now.is_match(line) && lines.get(n + 1).map(|nx| starts_sub.is_match(nx)).unwrap_or(false);
        let two = var_sub.iter().any(|(minus, sub)| sub.is_match(line) || minus.captures(line).map(|c| dur_rhs.is_match(&c[2])).unwrap_or(false));
        if direct.is_match(line) || wrapped || two {
            hits.push((n + 1, line.to_string()));
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn travel_rules() {
        let tmp = std::env::temp_dir().join(format!("hsmp_gate_lint_{}", std::process::id()));
        let m = tmp.join("mods/HSMPMenu/Scripts");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::create_dir_all(tmp.join("mods/HSMPMatch/Scripts")).unwrap();
        std::fs::create_dir_all(tmp.join("mods/shared")).unwrap();
        std::fs::write(tmp.join("mods/HSMPMatch/Scripts/main.lua"), "gs:OpenLevel(w, n)\n").unwrap();
        std::fs::write(m.join("main.lua"), concat!(
            "Log(\"x: OpenLevel('%s') OK\", a) -- string, not a call\n",
            "A(\"h\", \"Open Sallet A1\")\n",
            "RegisterHook(\"/Script/Engine.GameplayStatics:OpenLevel\", f)\n",
            "-- gs:OpenLevel(w)\n",
            "gs:OpenLevel(w, FName(x))\n",
            "local cmd = FString(\"open \" .. path)\n")).unwrap();
        let hits = travel_hits(&tmp).unwrap();
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![5, 6], "{:?}", hits.iter().map(|h| &h.code).collect::<Vec<_>>());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Placeholders keep the samples from flagging this file: SP Stop-Process, TK taskkill,
    /// GP Get-Process, PN ProcessName, PK pkill, KA killall, XSPPSX spps, KILL kill, PGR pgrep.
    fn sample(s: &str) -> String {
        s.replace("XSPPSX", &["sp", "ps"].concat())
            .replace("KILL", &["ki", "ll"].concat())
            .replace("PGR", &["pg", "rep"].concat())
            .replace("SP", &["Stop", "-Process"].concat())
            .replace("TK", &["task", "kill"].concat())
            .replace("GP", &["Get", "-Process"].concat())
            .replace("PN", &["Process", "Name"].concat())
            .replace("PK", &["p", "kill"].concat())
            .replace("KA", &["kill", "all"].concat())
    }

    #[test]
    fn kill_by_name_patterns() {
        let rx = kill_rx();
        let bad = [
            "os.execute('TK /F /IM hsmp-sidecar.exe')",
            "TK /F /T /FI \"IMAGENAME eq hsmp-server.exe\"",
            "SP -Name HalfSwordUE5-Win64-Shipping -Force",
            "XSPPSX -Name hsmp-server",
            "KILL -Name hsmp-sidecar",
            "GP hsmp-* | SP -Force",
            "GP -Name hsmp-server | ForEach-Object { $_.Kill() }",
            "GP | Where-Object { $_.PN -match \"HalfSword|hsmp-\" } | SP -Force",
            "GP | Where-Object { $_.PN -like 'hsmp-*' } | ForEach-Object {\n    try { SP -Id $_.Id -Force } catch { }\n}",
            "PK -f hsmp-server",
            "KA hsmp-sidecar",
            "KILL $(PGR hsmp-server)",
        ];
        for b in bad {
            let t = sample(b);
            assert!(!kill_hits_in(&t, &rx).is_empty(), "not flagged: {t}");
        }
        let good = [
            "TK /F /T /PID 1234 /FI \"IMAGENAME eq hsmp-server.exe\" >nul 2>nul",
            "SP -Id $p.Id -Force -ErrorAction Stop",
            "if ($p.PN -ne $e.name) { return $null }",
            "$p = GP -Id ([int]$e.pid) -ErrorAction SilentlyContinue",
            "GP -Id $pid | SP -Force",
            "-- TK /IM hsmp-server.exe (comment)",
            "# SP -Name x (comment)",
            "// KA (comment)",
            "let killer = peer.kill_count;",
            "$e = [ordered]@{ name = $proc.PN }",
        ];
        for g in good {
            let t = sample(g);
            assert!(kill_hits_in(&t, &rx).is_empty(), "false positive: {t} -> {:?}", kill_hits_in(&t, &rx));
        }
    }

    /// Evasive kill-by-name forms (short -Name prefixes, .Kill(), CIM/WMI, continuations,
    /// quoted /IM), and their PID-based twins that must not fire.
    #[test]
    fn kill_by_name_evasive_forms() {
        let rx = kill_rx();
        let wm = |s: &str| sample(s).replace("WMI_C", &["wm", "ic"].concat()).replace("W32P", &["Win32", "_Process"].concat())
            .replace("TERMN", &["Termi", "nate"].concat()).replace("SLASHIM", &["/", "IM"].concat());
        let bad = [
            "SP -PN hsmp-server -Force",
            "SP -N hsmp-server",
            "(GP hsmp-server).Kill()",
            "(GP -Name 'hsmp-sidecar').Kill()",
            "Get-CimInstance W32P -Filter \"Name='hsmp-server.exe'\" | Invoke-CimMethod -MethodName TERMN",
            "$p = Get-WmiObject W32P -Filter \"name like 'hsmp-%'\"\n$p | ForEach-Object { $_.TERMN() }",
            "TK /F `\n    SLASHIM hsmp-server.exe",
            "$im = \"SLASHIM\"\nTK /F $im hsmp-server.exe",
            "WMI_C process where name=\"hsmp-server.exe\" delete",
            "WMI_C process where \"name='hsmp-server.exe'\" call terminate",
            "let mut c = Command::new(\"TK\");\nc.args([\"/F\",\n    \"SLASHIM\", \"hsmp-server.exe\"]);",
        ];
        for b in bad {
            let t = wm(b);
            assert!(!kill_hits_in(&t, &rx).is_empty(), "not flagged: {t}");
        }
        let good = [
            "(GP -Id $pid).Kill()",
            "SP -Id $p.Id -Force",
            "Get-CimInstance W32P -Filter \"Name='hsmp-server.exe'\" | ForEach-Object { $_.ProcessId }",
            "Invoke-CimMethod -InputObject (Get-CimInstance W32P -Filter \"ProcessId=$pid\") -MethodName TERMN",
            "TK /F `\n    /PID 1234",
            "WMI_C process where \"name='hsmp-server.exe'\" get processid /format:value",
            "error(\"unterminated string\")",
            "let a = [\"SLASHIM\"]; // no kill tool anywhere in this file",
            "# TK SLASHIM x (comment)",
        ];
        for g in good {
            let t = wm(g);
            assert!(kill_hits_in(&t, &rx).is_empty(), "false positive: {t} -> {:?}", kill_hits_in(&t, &rx));
        }
    }

    #[test]
    fn kill_lint_reads_py() {
        let tmp = std::env::temp_dir().join(format!("hsmp_gate_killpy_{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("scripts")).unwrap();
        std::fs::write(tmp.join("scripts/x.py"), sample("import os\nos.system(\"TK /F /IM hsmp-server.exe\")\n")).unwrap();
        std::fs::write(tmp.join("scripts/y.txt"), sample("TK /F /IM hsmp-server.exe\n")).unwrap();
        let hits = image_name_kills(&tmp);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].starts_with("scripts/x.py:2"), "{hits:?}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn kill_lint_scans_root_and_src_bin() {
        let tmp = std::env::temp_dir().join(format!("hsmp_gate_kill_{}", std::process::id()));
        let bin = tmp.join("tools/x/src/bin/y");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(tmp.join("tools/x/target/release")).unwrap();
        std::fs::write(tmp.join("demo.ps1"), sample("GP | Where-Object { $_.PN -match 'hsmp-' } | SP\n")).unwrap();
        std::fs::write(bin.join("main.rs"), sample("fn f() { run(\"TK /IM a.exe\"); }\n")).unwrap();
        std::fs::write(tmp.join("tools/x/target/release/z.rs"), sample("run(\"TK /IM a.exe\");\n")).unwrap();
        let hits = image_name_kills(&tmp);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits[0].starts_with("demo.ps1:1"), "{hits:?}");
        assert!(hits[1].starts_with("tools/x/src/bin/y/main.rs:1"), "{hits:?}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn instant_sub() {
        let tmp = std::env::temp_dir().join(format!("hsmp_gate_instant_{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("server/src")).unwrap();
        std::fs::create_dir_all(tmp.join("tools/hsmp-tools")).unwrap();
        let minus = ["Instant::now() ", "- "].concat();
        std::fs::write(tmp.join("server/src/a.rs"), format!(
            "let a = {minus}Duration::from_secs(3600);\nlet b = {minus}Duration::from_secs(9);\n// let c = {minus}d;\nlet e = t.checked_sub(d);\nlet f = Instant::now() >= t;\n")).unwrap();
        std::fs::write(tmp.join("tools/hsmp-tools/instant_sub.baseline"), format!(
            "# known\nserver/src/a.rs: let a = {minus}Duration::from_secs(3600);\nserver/src/gone.rs: let z = {minus}d;\n")).unwrap();
        let (new, old, stale) = instant_sub_hits(&tmp);
        // the new hit, and the stale entry (it fails too)
        assert_eq!(new.len(), 2, "{new:?}");
        assert!(new[0].starts_with("server/src/a.rs:2:"), "{new:?}");
        assert!(new[1].contains("stale baseline entry") && new[1].contains("gone.rs"), "{new:?}");
        assert_eq!(old, vec!["server/src/a.rs:1".to_string()]);
        assert_eq!(stale.len(), 1);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The two-statement, wrapped and .sub() forms; a baseline line covers one hit.
    #[test]
    fn instant_sub_forms_and_counts() {
        let m = "-";
        let bad = [
            format!("let now = Instant::now();\nlet t = now {m} Duration::from_secs(5);"),
            format!("let now = std::time::Instant::now();\nlet t = now {m} TIMEOUT;"),
            format!("fn f(now: Instant, d: Duration) {{\n    let t = now {m} retry_delay;\n}}"),
            format!("let t = Instant::now().{s}(Duration::from_secs(1));", s = "sub"),
            format!("let n = Instant::now();\nlet t = n.{s}(d);", s = "sub"),
            format!("let t = Instant::now()\n    {m} Duration::from_secs(1);"),
        ];
        for b in &bad {
            assert!(!instant_sub_in(b).is_empty(), "not flagged:\n{b}");
        }
        let good = [
            format!("let now = Instant::now();\nlet age = now {m} last_seen;"),
            format!("let now = Instant::now();\nlet left = now.checked_sub(d);"),
            format!("let now = SystemTime::now();\nlet cutoff = now {m} Duration::from_secs(600);"),
            format!("let now = Instant::now();\nlet ok = now {m}= x;"),
            format!("// let t = Instant::now() {m} d;"),
            format!("let x = Instant::now();\nlet d = x.duration_since(start);"),
        ];
        for g in &good {
            assert!(instant_sub_in(g).is_empty(), "false positive:\n{g}\n{:?}", instant_sub_in(g));
        }
        // counts: one baseline line, two identical hits in the file -> one new
        let tmp = std::env::temp_dir().join(format!("hsmp_gate_instant2_{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("server/src")).unwrap();
        std::fs::create_dir_all(tmp.join("tools/hsmp-tools")).unwrap();
        let line = format!("let a = Instant::now() {m} Duration::from_secs(3600);");
        std::fs::write(tmp.join("server/src/a.rs"), format!("{line}\n{line}\n")).unwrap();
        std::fs::write(tmp.join("tools/hsmp-tools/instant_sub.baseline"), format!("server/src/a.rs: {line}\n")).unwrap();
        let (new, old, _) = instant_sub_hits(&tmp);
        assert_eq!((new.len(), old.len()), (1, 1), "{new:?} {old:?}");
        // the same line in ANOTHER file is not covered by a.rs's entry
        std::fs::write(tmp.join("server/src/b.rs"), format!("{line}\n")).unwrap();
        std::fs::write(tmp.join("server/src/a.rs"), format!("{line}\n")).unwrap();
        let (new, old, _) = instant_sub_hits(&tmp);
        assert_eq!((new.len(), old.len()), (1, 1), "{new:?} {old:?}");
        assert!(new[0].starts_with("server/src/b.rs:1"), "{new:?}");
        std::fs::remove_dir_all(&tmp).ok();
    }
}

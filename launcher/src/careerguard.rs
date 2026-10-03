//! Career-guard crash recovery before Play and Uninstall.
//!
//! The sidecar's career guard (server/src/sidecar/career_guard.rs) backs up the career saves
//! when an MP session starts and checks them when it ends. A session that crashed is left
//! `open`; the guard itself recovers it only at the next sidecar start (Host/Join). Without
//! this module, a player who crashed in MP and then clicked Play for career would make the
//! MP-damaged career file look like legitimate post-crash play (`skipped_newer`), and
//! Uninstall would remove the sidecar without ever recovering. So the launcher runs the
//! installed sidecar's recover-only mode
//!
//! ```text
//! hsmp-sidecar.exe --career-recover --career-save-dir <SaveGames> --career-backup-root <HSMP\save_backups>
//! ```
//!
//! which runs the guard's own `startup_check` (one implementation, no copy here), prints one
//! JSON line per action and exits 0 (nothing to do / recovered) or 1 (a guard error).

use crate::game;
use crate::install::Env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The installed sidecar, relative to Win64 (the launcher installs the binaries to `hsmp\`).
pub const SIDECAR_REL: &str = "hsmp/hsmp-sidecar.exe";
/// Recovery copies at most a few career files; anything slower than this is stuck.
pub const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// restored | recreated | quarantined | skipped_newer | clean | error | none
    pub action: String,
    pub file: Option<String>,
    pub why: String,
    pub backup: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recovery {
    pub actions: Vec<Action>,
    /// exit code 0 and no `error` action
    pub ok: bool,
}

impl Recovery {
    /// Career files the recovery put back (restored / recreated) or moved away (quarantined).
    pub fn changed(&self) -> Vec<&Action> {
        self.actions.iter().filter(|a| matches!(a.action.as_str(), "restored" | "recreated" | "quarantined")).collect()
    }
    pub fn errors(&self) -> Vec<&Action> {
        self.actions.iter().filter(|a| a.action == "error").collect()
    }
}

pub fn sidecar_path(env: &Env) -> PathBuf {
    crate::util::join_rel(&game::win64(&env.game_root), SIDECAR_REL)
}

/// Parse the sidecar's stdout: JSON lines with `ev = "career_guard"`; anything else (its log
/// output) is ignored. `ok` needs exit code 0 AND no error line.
pub fn parse(stdout: &str, code: Option<i32>) -> Recovery {
    let mut actions = vec![];
    for l in stdout.lines() {
        let l = l.trim();
        if !l.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        if v["ev"] != "career_guard" {
            continue;
        }
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        actions.push(Action { action: s("action"), file: v["file"].as_str().map(String::from), why: s("why"), backup: s("backup") });
    }
    let ok = code == Some(0) && !actions.iter().any(|a| a.action == "error");
    Recovery { actions, ok }
}

/// Run `exe --career-recover ...` and wait at most `timeout` (then the child, and only it, is
/// killed through its own handle).
pub fn run(exe: &Path, save_dir: &Path, backup_root: &Path, timeout: Duration) -> Result<Recovery, String> {
    let mut c = Command::new(exe);
    c.arg("--career-recover").arg("--career-save-dir").arg(save_dir).arg("--career-backup-root").arg(backup_root);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = c.spawn().map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let mut out = child.stdout.take().ok_or("no stdout pipe")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the career save check ({}) did not finish within {} s", exe.display(), timeout.as_secs()));
            }
            Err(e) => return Err(format!("career save check: {e}")),
        }
    };
    let text = reader.join().unwrap_or_default();
    Ok(parse(&text, status.code()))
}

/// Recover career-guard sessions a crash left open. `Ok(None)`: the sidecar is not installed
/// (nothing of HSMP to run; the guard's backups stay listed under Restore).
pub fn recover(env: &Env, log: &mut dyn FnMut(String)) -> Result<Option<Recovery>, String> {
    recover_with(&sidecar_path(env), env, TIMEOUT, log)
}

pub fn recover_with(exe: &Path, env: &Env, timeout: Duration, log: &mut dyn FnMut(String)) -> Result<Option<Recovery>, String> {
    if !exe.is_file() {
        return Ok(None);
    }
    let r = run(exe, &env.save_dir(), &env.save_backups(), timeout)?;
    for a in r.changed() {
        log(format!(
            "career guard: {} {} from the MP backup {} (an earlier multiplayer session ended without its save check)",
            a.action,
            a.file.as_deref().unwrap_or("?"),
            a.backup
        ));
    }
    for a in r.actions.iter().filter(|a| a.action == "skipped_newer") {
        log(format!("career guard: {} changed after the crashed MP session ended; left as it is", a.file.as_deref().unwrap_or("?")));
    }
    Ok(Some(r))
}

/// The refusal text when recovery did not succeed.
pub fn failure_text(r: &Recovery, what: &str) -> String {
    let errs: Vec<String> = r.errors().iter().map(|a| a.why.clone()).collect();
    format!(
        "the career save check after an earlier multiplayer crash failed ({}). {what} is blocked so your career is not changed further; restore your career from Saves > Restore (the guard's MP backups are listed there), or try again.",
        if errs.is_empty() { "the check exited with an error".to_string() } else { errs.join("; ") }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn parse_lines() {
        let out = "2026-10-02T00:00:00Z  INFO hsmp_sidecar: career guard: something\n\
                   {\"ev\":\"career_guard\",\"action\":\"restored\",\"file\":\"GameProgress.sav\",\"why\":\"changed\",\"kind\":\"recover\",\"backup\":\"B\"}\n\
                   {\"ev\":\"other\"}\nnot json {\n";
        let r = parse(out, Some(0));
        assert!(r.ok);
        assert_eq!(r.actions.len(), 1);
        assert_eq!(r.changed()[0].file.as_deref(), Some("GameProgress.sav"));
        assert!(!parse(out, Some(1)).ok, "exit 1 is a failure");
        assert!(!parse(out, None).ok, "killed is a failure");
        let e = parse("{\"ev\":\"career_guard\",\"action\":\"error\",\"file\":null,\"why\":\"startup_check: denied\",\"kind\":\"recover\",\"backup\":\"\"}\n", Some(0));
        assert!(!e.ok);
        assert!(failure_text(&e, "Play").contains("startup_check: denied"));
    }

    fn env(t: &TempDir) -> Env {
        Env::new(&t.path().join("game"), t.path().join("la/HSMP"), t.path().join("la/HalfSwordUE5/Saved"))
    }

    #[test]
    fn missing_sidecar_is_not_an_error() {
        let t = TempDir::new("cg_missing");
        let e = env(&t);
        assert_eq!(recover(&e, &mut |_| {}).unwrap(), None);
    }

    /// A stand-in sidecar (a .cmd script): the launcher passes the env's save dir and backup
    /// root, reads its JSON lines and reports what was restored.
    #[cfg(windows)]
    #[test]
    fn runs_the_installed_sidecar() {
        let t = TempDir::new("cg_run");
        let e = env(&t);
        let exe = t.path().join("fake-sidecar.cmd");
        let line = r#"{"ev":"career_guard","action":"restored","file":"GameProgress.sav","why":"changed","kind":"recover","backup":"B1"}"#;
        std::fs::write(&exe, format!("@echo off\r\necho %*> \"%~dp0args.txt\"\r\necho {line}\r\nexit /b 0\r\n")).unwrap();
        let mut logs = vec![];
        let r = recover_with(&exe, &e, TIMEOUT, &mut |s| logs.push(s)).unwrap().unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.changed().len(), 1);
        assert!(logs[0].contains("restored GameProgress.sav from the MP backup B1"), "{logs:?}");
        let args = std::fs::read_to_string(t.path().join("args.txt")).unwrap();
        assert!(args.contains("--career-recover"), "{args}");
        assert!(args.contains("--career-save-dir") && args.contains("SaveGames"), "{args}");
        assert!(args.contains("--career-backup-root") && args.contains("save_backups"), "{args}");

        // exit code 1: not ok
        std::fs::write(&exe, "@echo off\r\nexit /b 1\r\n").unwrap();
        assert!(!recover_with(&exe, &e, TIMEOUT, &mut |_| {}).unwrap().unwrap().ok);

        // hangs: killed after the timeout, an error
        std::fs::write(&exe, "@echo off\r\nping -n 4 127.0.0.1 >nul\r\n").unwrap();
        let err = recover_with(&exe, &e, Duration::from_millis(500), &mut |_| {}).unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
    }
}

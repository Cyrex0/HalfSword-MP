//! Windows Firewall: an inbound allow rule for the installed `hsmp-server.exe`.
//!
//! A hosted game is unreachable from outside when Windows blocks the server: the player
//! dismissed the first-run firewall prompt (Windows then adds its own block rules for the
//! exe) or the network is on the "Public" profile. Install and update add one rule
//! (program-based, UDP, every profile); uninstall removes it.
//!
//! Changing the firewall needs admin rights, so the launcher starts a copy of itself
//! through UAC with `firewall-allow <exe>` or `firewall-remove [<exe>]` and waits for
//! it; the launcher itself never runs elevated. Reading the rule needs no rights.
//! Tests only build command lines and parse output; they never touch the firewall.

use std::path::{Path, PathBuf};

/// The rule's name in Windows Defender Firewall.
pub const RULE_NAME: &str = "Half Sword MP server";
const RULE_DESC: &str = "Lets players reach Half Sword MP games hosted on this PC (added by hsmp-launcher)";
pub const SERVER_EXE: &str = "hsmp-server.exe";

/// What the user sees when the rule could not be added.
pub const NOT_ALLOWED_WARNING: &str =
    "Windows Firewall does not allow hsmp-server.exe: players outside your network may not reach games you host. Click \"Fix firewall\" (or run the launcher's install again) and accept the Windows prompt.";

/// The installed server: `<game>\HalfswordUE5\Binaries\Win64\hsmp\hsmp-server.exe`.
pub fn server_exe(game_root: &Path) -> PathBuf {
    crate::game::win64(game_root).join("hsmp").join(SERVER_EXE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// exactly one rule with our name, for this exe, enabled
    Allowed,
    /// no rule with our name
    Missing,
    /// our rule exists but for another exe (the game folder moved)
    WrongProgram,
    /// our rule exists but is turned off
    Disabled,
    /// more than one rule with our name
    Duplicates(usize),
    /// netsh could not be asked (or this is not Windows)
    Unknown(String),
}

impl State {
    pub fn is_allowed(&self) -> bool {
        *self == State::Allowed
    }

    pub fn describe(&self) -> String {
        match self {
            State::Allowed => format!("Firewall: hsmp-server.exe is allowed (rule \"{RULE_NAME}\", UDP, all networks)."),
            State::Missing => "Firewall: no rule for hsmp-server.exe yet. Players outside your network may not reach games you host.".into(),
            State::WrongProgram => "Firewall: the HSMP rule points at another hsmp-server.exe (the game folder moved). Fix it so hosting works.".into(),
            State::Disabled => "Firewall: the HSMP rule is turned off. Players outside your network may not reach games you host.".into(),
            State::Duplicates(n) => format!("Firewall: {n} rules named \"{RULE_NAME}\"; Fix firewall replaces them with one."),
            State::Unknown(e) => format!("Firewall: could not read the rules ({e})."),
        }
    }
}

/// A path that can go into a netsh command line and is really our server.
fn check_exe(exe: &Path) -> Result<String, String> {
    let s = exe.to_string_lossy().to_string();
    if !exe.is_absolute() || s.contains('"') || s.contains('\n') || s.contains('\r') {
        return Err(format!("not a usable server path: {s}"));
    }
    if !exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(SERVER_EXE)) {
        return Err(format!("the firewall rule is only for {SERVER_EXE}, not {s}"));
    }
    Ok(s)
}

fn name_arg() -> String {
    format!("name=\"{RULE_NAME}\"")
}

/// netsh argument lists that replace every inbound rule for `exe` (the block rules Windows
/// added when its prompt was dismissed, an older rule of ours) with one allow rule. Each
/// argument is passed raw: netsh wants `program="C:\a b\x.exe"`, not `"program=C:\a b\x.exe"`.
/// The two deletes fail harmlessly when there is nothing to delete.
pub fn allow_commands(exe: &Path) -> Result<Vec<Vec<String>>, String> {
    let p = check_exe(exe)?;
    let base = || vec!["advfirewall".to_string(), "firewall".into()];
    let mut del_ours = base();
    del_ours.extend(["delete".into(), "rule".into(), name_arg()]);
    let mut del_prog = base();
    del_prog.extend(["delete".into(), "rule".into(), "name=all".into(), "dir=in".into(), format!("program=\"{p}\"")]);
    let mut add = base();
    add.extend([
        "add".into(),
        "rule".into(),
        name_arg(),
        "dir=in".into(),
        "action=allow".into(),
        format!("program=\"{p}\""),
        "protocol=UDP".into(),
        "profile=any".into(),
        "enable=yes".into(),
        format!("description=\"{RULE_DESC}\""),
    ]);
    Ok(vec![del_ours, del_prog, add])
}

/// netsh argument lists that remove our rule (and, given the exe, every inbound rule for it).
pub fn remove_commands(exe: Option<&Path>) -> Result<Vec<Vec<String>>, String> {
    let mut out = vec![vec!["advfirewall".to_string(), "firewall".into(), "delete".into(), "rule".into(), name_arg()]];
    if let Some(exe) = exe {
        let p = check_exe(exe)?;
        out.push(vec!["advfirewall".into(), "firewall".into(), "delete".into(), "rule".into(), "name=all".into(), "dir=in".into(), format!("program=\"{p}\"")]);
    }
    Ok(out)
}

pub fn show_command() -> Vec<String> {
    vec!["advfirewall".into(), "firewall".into(), "show".into(), "rule".into(), name_arg(), "verbose".into()]
}

/// The rule's state from `netsh advfirewall firewall show rule name=... verbose`.
/// netsh's labels are localized, so this goes by our rule name and the exe path, and only
/// reads "Enabled: No" where Windows is in English.
pub fn parse_show(exit_ok: bool, out: &str, exe: &Path) -> State {
    let count = out.lines().filter(|l| l.trim_end().ends_with(RULE_NAME)).count();
    if !exit_ok || count == 0 {
        return State::Missing;
    }
    if count > 1 {
        return State::Duplicates(count);
    }
    let want = exe.to_string_lossy().to_lowercase();
    // netsh prints in the console code page: a non-ASCII path cannot be compared reliably
    if want.is_ascii() && !out.to_lowercase().contains(&want) {
        return State::WrongProgram;
    }
    let disabled = out.lines().any(|l| {
        let l = l.trim();
        l.starts_with("Enabled:") && l["Enabled:".len()..].trim().eq_ignore_ascii_case("no")
    });
    if disabled {
        State::Disabled
    } else {
        State::Allowed
    }
}

/// The command line for the elevated copy of the launcher.
pub fn elevated_params(verb: &str, exe: Option<&Path>) -> Result<String, String> {
    match exe {
        Some(e) => Ok(format!("{verb} \"{}\"", check_exe(e)?)),
        None => Ok(verb.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    AlreadyAllowed,
    Added,
    Removed,
    NothingToRemove,
    /// the user said no to the UAC prompt
    Declined,
    Failed(String),
}

/// Add the rule unless it is already right. `status` reads the rule, `elevate(params)` runs
/// the launcher elevated and returns its exit code. Injected so tests never touch the system.
pub fn ensure_with(exe: &Path, status: &mut dyn FnMut(&Path) -> State, elevate: &mut dyn FnMut(&str) -> Result<u32, Elevate>) -> Outcome {
    if let Err(e) = check_exe(exe) {
        return Outcome::Failed(e);
    }
    if !exe.is_file() {
        return Outcome::Failed(format!("{} is not installed", exe.display()));
    }
    if status(exe).is_allowed() {
        return Outcome::AlreadyAllowed;
    }
    let params = match elevated_params("firewall-allow", Some(exe)) {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e),
    };
    match elevate(&params) {
        Err(Elevate::Declined) => Outcome::Declined,
        Err(Elevate::Failed(e)) => Outcome::Failed(e),
        Ok(code) if code != 0 => Outcome::Failed(format!("the firewall helper exited with code {code} (see launcher.log)")),
        Ok(_) => match status(exe) {
            State::Allowed => Outcome::Added,
            // the rule went in but cannot be read back: trust the helper's exit code
            State::Unknown(_) => Outcome::Added,
            s => Outcome::Failed(s.describe()),
        },
    }
}

/// Remove the rule if there is one.
pub fn remove_with(exe: &Path, status: &mut dyn FnMut(&Path) -> State, elevate: &mut dyn FnMut(&str) -> Result<u32, Elevate>) -> Outcome {
    if status(exe) == State::Missing {
        return Outcome::NothingToRemove;
    }
    let params = match elevated_params("firewall-remove", check_exe(exe).ok().map(|_| exe)) {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e),
    };
    match elevate(&params) {
        Err(Elevate::Declined) => Outcome::Declined,
        Err(Elevate::Failed(e)) => Outcome::Failed(e),
        Ok(0) => Outcome::Removed,
        Ok(code) => Outcome::Failed(format!("the firewall helper exited with code {code} (see launcher.log)")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevate {
    Declined,
    Failed(String),
}

/// Read the rule's state from the system.
pub fn status(exe: &Path) -> State {
    match netsh(&show_command()) {
        Ok((ok, out)) => parse_show(ok, &out, exe),
        Err(e) => State::Unknown(e),
    }
}

/// Add (or replace) the rule, with one UAC prompt, unless it is already right.
pub fn ensure(exe: &Path) -> Outcome {
    ensure_with(exe, &mut status, &mut elevate)
}

/// Remove the rule (uninstall), with one UAC prompt when there is one.
pub fn remove(exe: &Path) -> Outcome {
    remove_with(exe, &mut status, &mut elevate)
}

/// Run netsh commands in order; the elevated helper's job. A failed delete is normal
/// (nothing to delete); the last command (the add, or the final delete) decides.
pub fn apply(cmds: &[Vec<String>], log: &mut dyn FnMut(String)) -> Result<(), String> {
    let mut last = Ok(());
    for (i, c) in cmds.iter().enumerate() {
        let (ok, out) = netsh(c)?;
        log(format!("netsh {} -> {}{}", c.join(" "), if ok { "ok" } else { "failed" }, if ok { String::new() } else { format!(": {}", out.trim()) }));
        if i + 1 == cmds.len() && !ok && c.get(2).map(String::as_str) == Some("add") {
            last = Err(format!("netsh could not add the rule: {}", out.trim()));
        }
    }
    last
}

#[cfg(windows)]
fn netsh(args: &[String]) -> Result<(bool, String), String> {
    use std::os::windows::process::CommandExt;
    if cfg!(test) && args.get(2).map(String::as_str) != Some("show") {
        return Err("tests never change the firewall".into());
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let mut c = std::process::Command::new(sys.join("System32").join("netsh.exe"));
    for a in args {
        c.raw_arg(a);
    }
    c.creation_flags(CREATE_NO_WINDOW);
    let o = c.output().map_err(|e| format!("cannot run netsh: {e}"))?;
    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok((o.status.success(), s))
}

#[cfg(not(windows))]
fn netsh(_args: &[String]) -> Result<(bool, String), String> {
    Err("Windows Firewall exists on Windows only".into())
}

/// Start this launcher elevated (UAC) with `params` and wait for it.
#[cfg(windows)]
fn elevate(params: &str) -> Result<u32, Elevate> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    if cfg!(test) {
        return Err(Elevate::Failed("tests never elevate".into()));
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let me = std::env::current_exe().map_err(|e| Elevate::Failed(format!("cannot find the launcher exe: {e}")))?;
    let (verb, file, par) = (wide("runas"), wide(&me.to_string_lossy()), wide(params));
    // SAFETY: the struct is plain data, zeroed then filled; the strings outlive the call; the
    // process handle is waited on and closed.
    unsafe {
        let mut sei: SHELLEXECUTEINFOW = std::mem::zeroed();
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        sei.lpParameters = par.as_ptr();
        sei.nShow = 0; // SW_HIDE
        if ShellExecuteExW(&mut sei) == 0 {
            let e = GetLastError();
            return Err(if e == ERROR_CANCELLED { Elevate::Declined } else { Elevate::Failed(format!("could not start the elevated helper (error {e})")) });
        }
        if sei.hProcess == 0 {
            return Err(Elevate::Failed("the elevated helper gave no process handle".into()));
        }
        let waited = WaitForSingleObject(sei.hProcess, 120_000);
        let mut code = 1u32;
        let got = GetExitCodeProcess(sei.hProcess, &mut code);
        CloseHandle(sei.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err(Elevate::Failed("the elevated helper did not finish within 2 minutes".into()));
        }
        if got == 0 {
            return Err(Elevate::Failed("cannot read the elevated helper's exit code".into()));
        }
        Ok(code)
    }
}

#[cfg(not(windows))]
fn elevate(_params: &str) -> Result<u32, Elevate> {
    Err(Elevate::Failed("Windows Firewall exists on Windows only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Half Sword\HalfswordUE5\Binaries\Win64\hsmp\hsmp-server.exe")
    }

    #[test]
    fn allow_replaces_every_rule_for_the_exe_then_adds_one() {
        let c = allow_commands(&exe()).unwrap();
        let p = exe().display().to_string();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].join(" "), "advfirewall firewall delete rule name=\"Half Sword MP server\"");
        assert_eq!(c[1].join(" "), format!("advfirewall firewall delete rule name=all dir=in program=\"{p}\""));
        assert_eq!(
            c[2].join(" "),
            format!("advfirewall firewall add rule name=\"Half Sword MP server\" dir=in action=allow program=\"{p}\" protocol=UDP profile=any enable=yes description=\"{RULE_DESC}\"")
        );
        // the path with spaces stays one quoted value
        assert!(c[2].contains(&format!("program=\"{p}\"")));
    }

    #[test]
    fn remove_and_show_commands() {
        let r = remove_commands(None).unwrap();
        assert_eq!(r, vec![vec!["advfirewall", "firewall", "delete", "rule", "name=\"Half Sword MP server\""]]);
        let r = remove_commands(Some(&exe())).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[1].join(" ").ends_with(&format!("dir=in program=\"{}\"", exe().display())));
        assert_eq!(show_command().join(" "), "advfirewall firewall show rule name=\"Half Sword MP server\" verbose");
    }

    #[test]
    fn only_our_server_with_a_clean_path() {
        assert!(allow_commands(Path::new(r"C:\Windows\System32\cmd.exe")).unwrap_err().contains("only for hsmp-server.exe"));
        assert!(allow_commands(Path::new(r"hsmp\hsmp-server.exe")).is_err(), "relative");
        assert!(allow_commands(Path::new("C:\\a\" b\\hsmp-server.exe")).is_err(), "a quote would break the command line");
        assert!(allow_commands(Path::new(r"D:\G\HSMP-SERVER.EXE")).is_ok());
        assert_eq!(elevated_params("firewall-allow", Some(&exe())).unwrap(), format!("firewall-allow \"{}\"", exe().display()));
        assert_eq!(elevated_params("firewall-remove", None).unwrap(), "firewall-remove");
    }

    fn show_out(program: &str, enabled: &str) -> String {
        format!(
            "\r\nRule Name:                            Half Sword MP server\r\n----------------------------------------------------------------------\r\nDescription:                          {RULE_DESC}\r\nEnabled:                              {enabled}\r\nDirection:                            In\r\nProfiles:                             Domain,Private,Public\r\nGrouping:                             \r\nLocalIP:                              Any\r\nRemoteIP:                             Any\r\nProtocol:                             UDP\r\nLocalPort:                            Any\r\nRemotePort:                           Any\r\nEdge traversal:                       No\r\nProgram:                              {program}\r\nInterfaceTypes:                       Any\r\nSecurity:                             NotRequired\r\nRule source:                          Local Setting\r\nAction:                               Allow\r\nOk.\r\n\r\n"
        )
    }

    #[test]
    fn parse_show_states() {
        let p = exe().display().to_string();
        assert_eq!(parse_show(true, &show_out(&p, "Yes"), &exe()), State::Allowed);
        assert_eq!(parse_show(true, &show_out(&p.to_uppercase(), "Yes"), &exe()), State::Allowed, "paths compare case-insensitively");
        assert_eq!(parse_show(true, &show_out(&p, "No"), &exe()), State::Disabled);
        assert_eq!(parse_show(true, &show_out(r"D:\old\hsmp\hsmp-server.exe", "Yes"), &exe()), State::WrongProgram);
        assert_eq!(parse_show(false, "\r\nNo rules match the specified criteria.\r\n\r\n", &exe()), State::Missing);
        let two = show_out(&p, "Yes") + &show_out(&p, "Yes");
        assert_eq!(parse_show(true, &two, &exe()), State::Duplicates(2));
        // German Windows: localized labels, the name and path still match
        let de = show_out(&p, "Ja").replace("Rule Name:", "Regelname:").replace("Enabled:", "Aktiviert:").replace("Program:", "Programm:");
        assert_eq!(parse_show(true, &de, &exe()), State::Allowed);
    }

    #[test]
    fn ensure_skips_uac_when_the_rule_is_right_and_reports_a_decline() {
        let t = TempDir::new("fw_ensure");
        let exe = t.path().join("hsmp").join(SERVER_EXE);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        // not installed yet: nothing is asked
        let mut asked = vec![];
        let r = ensure_with(&exe, &mut |_| State::Missing, &mut |p| {
            asked.push(p.to_string());
            Ok(0)
        });
        assert!(matches!(r, Outcome::Failed(ref e) if e.contains("not installed")), "{r:?}");
        assert!(asked.is_empty());
        std::fs::write(&exe, b"MZ").unwrap();

        let r = ensure_with(&exe, &mut |_| State::Allowed, &mut |_| panic!("no UAC prompt when the rule is already right"));
        assert_eq!(r, Outcome::AlreadyAllowed);

        let mut n = 0;
        let r = ensure_with(
            &exe,
            &mut |_| {
                n += 1;
                if n == 1 {
                    State::WrongProgram
                } else {
                    State::Allowed
                }
            },
            &mut |p| {
                asked.push(p.to_string());
                Ok(0)
            },
        );
        assert_eq!(r, Outcome::Added);
        assert_eq!(asked, vec![format!("firewall-allow \"{}\"", exe.display())]);

        assert_eq!(ensure_with(&exe, &mut |_| State::Missing, &mut |_| Err(Elevate::Declined)), Outcome::Declined);
        assert!(matches!(ensure_with(&exe, &mut |_| State::Missing, &mut |_| Ok(5)), Outcome::Failed(ref e) if e.contains("code 5")));
        assert!(matches!(ensure_with(&exe, &mut |_| State::Missing, &mut |_| Ok(0)), Outcome::Failed(_)), "the rule must be there afterwards");
    }

    #[test]
    fn remove_only_asks_when_there_is_a_rule() {
        assert_eq!(remove_with(&exe(), &mut |_| State::Missing, &mut |_| panic!("no prompt")), Outcome::NothingToRemove);
        let mut asked = String::new();
        let r = remove_with(&exe(), &mut |_| State::WrongProgram, &mut |p| {
            asked = p.to_string();
            Ok(0)
        });
        assert_eq!(r, Outcome::Removed);
        assert_eq!(asked, format!("firewall-remove \"{}\"", exe().display()));
        assert_eq!(remove_with(&exe(), &mut |_| State::Allowed, &mut |_| Err(Elevate::Declined)), Outcome::Declined);
    }

    #[test]
    fn tests_never_change_the_firewall() {
        let e = apply(&allow_commands(&exe()).unwrap(), &mut |_| {}).unwrap_err();
        assert!(e.contains("never") || e.contains("Windows only"), "{e}");
        assert!(matches!(elevate("firewall-allow x"), Err(Elevate::Failed(_))));
    }
}

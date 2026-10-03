//! Read-only process listing: is the game (or an HSMP binary from this
//! install) running? The launcher NEVER terminates processes; it asks the
//! player to close them.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub path: Option<PathBuf>,
}

#[cfg(windows)]
pub fn list() -> Vec<Proc> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};

    fn wide_to_string(w: &[u16]) -> String {
        let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..n])
    }

    let mut out = vec![];
    // SAFETY: plain Win32 snapshot iteration; every handle is closed; buffers are sized.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let name = wide_to_string(&e.szExeFile);
            let mut path = None;
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
            if h != 0 {
                let mut buf = vec![0u16; 1024];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                    path = Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
                }
                CloseHandle(h);
            }
            out.push(Proc { pid: e.th32ProcessID, name, path });
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn list() -> Vec<Proc> {
    vec![]
}

const GAME_EXES: &[&str] = &["HalfSwordUE5-Win64-Shipping.exe", "HalfSwordUE5.exe"];
const HSMP_EXES: &[&str] = &["hsmp-server.exe", "hsmp-sidecar.exe", "hsmp-master.exe", "hsmp-query.exe"];

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn under(p: &Path, dir: &Path) -> bool {
    // full Unicode folding: a game folder named "Spiele\Ärger" must match "spiele\ärger";
    // both sides canonical as well: a game root picked through a junction, a subst
    // drive or an 8.3 name is the same folder the running exe reports
    let raw = crate::util::fold_path(dir) + "\\";
    let canon = crate::util::fold_path(&canonical(dir)) + "\\";
    let (fp, cp) = (crate::util::fold_path(p), crate::util::fold_path(&canonical(p)));
    [&fp, &cp].iter().any(|x| x.starts_with(&raw) || x.starts_with(&canon))
}

/// Processes that hold files the launcher writes: Half Sword running from
/// this game folder (or from an unknown path: assume it is this one), and
/// HSMP binaries started from this game folder. A copy of the game running
/// from another folder does not lock this folder's files.
pub fn blocking(procs: &[Proc], game_root: &Path) -> Vec<String> {
    let mut out = vec![];
    for p in procs {
        let is_game = GAME_EXES.iter().any(|g| g.eq_ignore_ascii_case(&p.name)) && p.path.as_deref().map(|x| under(x, game_root)).unwrap_or(true);
        let is_ours = HSMP_EXES.iter().any(|g| g.eq_ignore_ascii_case(&p.name)) && p.path.as_deref().map(|x| under(x, game_root)).unwrap_or(false);
        if is_game || is_ours {
            out.push(format!("{} (pid {})", p.name, p.pid));
        }
    }
    out
}

pub fn blocking_processes(game_root: &Path) -> Vec<String> {
    blocking(&list(), game_root)
}

pub fn game_running() -> bool {
    list().iter().any(|p| GAME_EXES.iter().any(|g| g.eq_ignore_ascii_case(&p.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_rules() {
        let root = Path::new("D:\\Steam\\steamapps\\common\\Half Sword");
        let procs = vec![
            Proc { pid: 1, name: "explorer.exe".into(), path: None },
            Proc { pid: 2, name: "HalfSwordUE5-Win64-Shipping.exe".into(), path: None },
            Proc { pid: 3, name: "hsmp-server.exe".into(), path: Some(PathBuf::from("C:\\dedicated\\hsmp-server.exe")) },
            Proc { pid: 4, name: "hsmp-sidecar.exe".into(), path: Some(root.join("HalfswordUE5\\Binaries\\Win64\\hsmp\\hsmp-sidecar.exe")) },
            Proc { pid: 5, name: "hsmp-query.exe".into(), path: None },
            Proc { pid: 6, name: "HalfSwordUE5-Win64-Shipping.exe".into(), path: Some(PathBuf::from("D:\\dev\\game\\HalfswordUE5\\Binaries\\Win64\\HalfSwordUE5-Win64-Shipping.exe")) },
            Proc { pid: 7, name: "HalfSwordUE5.exe".into(), path: Some(root.join("HalfSwordUE5.exe")) },
        ];
        // non-ASCII game folder, different case of a non-ASCII letter
        let uroot = Path::new("D:\\Spiele\\ÄRGER 漢字\\Half Sword");
        let up = vec![Proc { pid: 9, name: "HalfSwordUE5.exe".into(), path: Some(PathBuf::from("d:\\spiele\\ärger 漢字\\half sword\\HalfSwordUE5.exe")) }];
        assert_eq!(blocking(&up, uroot).len(), 1, "Unicode case-insensitive path match");
        let b = blocking(&procs, root);
        assert_eq!(
            b,
            vec!["HalfSwordUE5-Win64-Shipping.exe (pid 2)".to_string(), "hsmp-sidecar.exe (pid 4)".to_string(), "HalfSwordUE5.exe (pid 7)".to_string()],
            "unknown path blocks; another folder's copy does not"
        );
    }

    /// The game folder was picked through a junction; the running exe reports the
    /// real path. It must still block the install.
    #[cfg(windows)]
    #[test]
    fn junction_game_root_is_the_same_folder() {
        let t = crate::testutil::TempDir::new("procs_junction");
        let real = t.path().join("real").join("Half Sword");
        std::fs::create_dir_all(real.join("HalfswordUE5")).unwrap();
        let link = t.path().join("link");
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(t.path().join("real")).output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            eprintln!("mklink /J unavailable: skipped");
            return;
        }
        let exe = canonical(&real).join("HalfSwordUE5.exe");
        let procs = vec![Proc { pid: 11, name: "HalfSwordUE5.exe".into(), path: Some(exe) }];
        assert_eq!(blocking(&procs, &link.join("Half Sword")).len(), 1, "junction root vs real exe path");
        // and the other way round: the exe reported through the junction, the root picked for real
        std::fs::write(real.join("HalfswordUE5").join("x.exe"), b"").unwrap();
        let procs = vec![Proc { pid: 12, name: "hsmp-sidecar.exe".into(), path: Some(link.join("Half Sword").join("HalfswordUE5").join("x.exe")) }];
        assert_eq!(blocking(&procs, &real).len(), 1);
        // a different folder still does not block
        let other = t.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let procs = vec![Proc { pid: 13, name: "hsmp-sidecar.exe".into(), path: Some(other.join("hsmp-sidecar.exe")) }];
        assert!(blocking(&procs, &link.join("Half Sword")).is_empty());
    }

    #[test]
    fn listing_works() {
        // the test runner itself must show up
        let me = std::process::id();
        if cfg!(windows) {
            assert!(list().iter().any(|p| p.pid == me));
        }
    }
}

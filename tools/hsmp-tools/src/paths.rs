//! Repo / game directory discovery and small file-system helpers.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Repo-relative folder with the shipped UE4SS Lua mods (`mods/<Mod>/Scripts/*.lua`) and the
/// shared libraries (`mods/shared/*.lua`).
pub const MODS_DIR: &str = "mods";
/// Developer-only mods live one level down: `mods/dev/<Mod>/Scripts/*.lua`.
pub const DEV_MODS_DIR: &str = "dev";

fn is_repo_root(p: &Path) -> bool {
    p.join(MODS_DIR).is_dir() && p.join("server").join("Cargo.toml").is_file()
}

/// `<root>/mods`
pub fn mods_dir(root: &Path) -> PathBuf {
    root.join(MODS_DIR)
}

/// Whether `root` holds the Lua mods (`mods/shared`). A stand-in repo with only
/// `mods/mods.release.txt` (the gate's `_clean_repo` fixture) has none.
pub fn has_mods(root: &Path) -> bool {
    mods_dir(root).join("shared").is_dir()
}

/// The folder of mod `name`: `mods/<name>`, or `mods/dev/<name>` for a developer mod.
/// Returns `mods/<name>` when neither exists.
pub fn mod_dir(root: &Path, name: &str) -> PathBuf {
    let m = mods_dir(root);
    let dev = m.join(DEV_MODS_DIR).join(name);
    if !m.join(name).is_dir() && dev.is_dir() {
        dev
    } else {
        m.join(name)
    }
}

/// The mod a repo-relative, `/`-separated path belongs to: `mods/<Mod>/...` and
/// `mods/dev/<Mod>/...` give `<Mod>`, `mods/shared/...` gives `shared`; anything else "".
pub fn mod_of_rel(rel: &str) -> &str {
    let mut it = rel.split('/');
    if it.next() != Some(MODS_DIR) {
        return "";
    }
    match it.next() {
        Some(DEV_MODS_DIR) => it.next().unwrap_or(""),
        Some(m) => m,
        None => "",
    }
}

/// The HSMP repo root: `HSMP_ROOT`, else the first ancestor of the current
/// dir (then of the executable) that holds `mods` + `server/Cargo.toml`,
/// else the checkout this crate was built from.
pub fn repo_root() -> Result<PathBuf> {
    if let Ok(r) = std::env::var("HSMP_ROOT") {
        let p = PathBuf::from(r);
        if is_repo_root(&p) {
            return Ok(p);
        }
        bail!("HSMP_ROOT={} is not an HSMP checkout", p.display());
    }
    let mut starts = vec![];
    if let Ok(c) = std::env::current_dir() {
        starts.push(c);
    }
    if let Ok(e) = std::env::current_exe() {
        starts.push(e);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for s in starts {
        for a in s.ancestors() {
            if is_repo_root(a) {
                return Ok(a.to_path_buf());
            }
        }
    }
    bail!("cannot find the HSMP repo root (set HSMP_ROOT)")
}

/// The game install: `HSMP_GAME_DIR` when set (and non-empty), else `<repo>/game`, else the
/// `game/` folder of the main checkout when `root` is a linked git worktree (worktrees have
/// no game folder of their own). There is no machine-specific fallback: on a machine
/// without the game this errors, and the checks that need it report a skip.
pub fn game_dir(root: &Path) -> Result<PathBuf> {
    let cands: Vec<PathBuf> = match std::env::var("HSMP_GAME_DIR") {
        Ok(g) if !g.trim().is_empty() => vec![PathBuf::from(g)],
        _ => {
            let mut v = vec![root.join("game")];
            if let Some(main) = main_worktree(root) {
                if main != root {
                    v.push(main.join("game"));
                }
            }
            v
        }
    };
    for c in &cands {
        if c.is_dir() {
            return Ok(c.clone());
        }
    }
    bail!(
        "game dir not found (tried {}); set HSMP_GAME_DIR",
        cands.iter().map(|c| c.display().to_string()).collect::<Vec<_>>().join(", ")
    )
}

/// The main checkout of the git repository `root` belongs to (the parent of the shared
/// `.git` dir), or `None` when git is unavailable or `root` is not in a repository.
fn main_worktree(root: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if common.file_name().map(|n| n == ".git").unwrap_or(false) {
        common.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// `<game>/HalfswordUE5/Binaries/Win64/ue4ss`
pub fn ue4ss_dir(game: &Path) -> PathBuf {
    game.join("HalfswordUE5").join("Binaries").join("Win64").join("ue4ss")
}

/// All files under `dir` (recursive) accepted by `keep`, sorted.
pub fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(_) if keep(&p) => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// Minimal glob: `/`-separated pattern relative to `base`; segments may use
/// `*` and `?`, and a `**` segment matches any number of directories.
pub fn glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let segs: Vec<&str> = pattern.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let mut out = vec![];
    glob_rec(base, &segs, &mut out);
    out.sort();
    out.dedup();
    out
}

fn glob_rec(dir: &Path, segs: &[&str], out: &mut Vec<PathBuf>) {
    let Some((first, rest)) = segs.split_first() else { return };
    if *first == "**" {
        glob_rec(dir, rest, out);
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    glob_rec(&e.path(), segs, out);
                }
            }
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !wildcard(first, &name) {
            continue;
        }
        let p = e.path();
        if rest.is_empty() {
            out.push(p);
        } else if p.is_dir() {
            glob_rec(&p, rest, out);
        }
    }
}

/// `*` / `?` wildcard match of one path segment.
pub fn wildcard(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// `mods/*/Scripts/*.lua` and `mods/dev/*/Scripts/*.lua`, sorted (the mod entry points +
/// siblings).
pub fn mod_script_files(root: &Path) -> Vec<PathBuf> {
    let m = mods_dir(root);
    let mut v = glob(&m, "*/Scripts/*.lua");
    v.extend(glob(&m.join(DEV_MODS_DIR), "*/Scripts/*.lua"));
    v.sort();
    v
}

/// Path relative to `root`, with forward slashes (for messages).
pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Forward-slash string form of a path (what the Lua side expects).
pub fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Read a UTF-8 (lossy) text file with context on error.
pub fn read_text(p: &Path) -> Result<String> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// A fresh, empty temp directory `<tmp>/<prefix><random>`.
pub fn make_temp_dir(prefix: &str) -> Result<PathBuf> {
    use rand::Rng;
    let base = std::env::temp_dir();
    for _ in 0..100 {
        let n: u64 = rand::thread_rng().gen();
        let p = base.join(format!("{prefix}{n:016x}"));
        if std::fs::create_dir(&p).is_ok() {
            return Ok(p);
        }
    }
    bail!("cannot create a temp dir in {}", base.display())
}

#[cfg(test)]
mod tests {
    use super::wildcard;
    #[test]
    fn wild() {
        assert!(wildcard("*.lua", "main.lua"));
        assert!(!wildcard("*.lua", "main.luac"));
        assert!(wildcard("Map_*_?.json", "Map_Arena_A.json"));
        assert!(wildcard("*", ""));
    }

    #[test]
    fn mod_names_from_paths() {
        use super::mod_of_rel;
        assert_eq!(mod_of_rel("mods/HSMPMenu/Scripts/main.lua"), "HSMPMenu");
        assert_eq!(mod_of_rel("mods/dev/HSMPDiag/Scripts/main.lua"), "HSMPDiag");
        assert_eq!(mod_of_rel("mods/shared/hsmp_log.lua"), "shared");
        assert_eq!(mod_of_rel("server/src/main.rs"), "");
    }
}

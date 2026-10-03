//! Embeds the git commit the tools were built from: `hsmp-gate` writes it into
//! report.json and DoD-13 refuses to certify a run judged by a gate built from another commit
//! (or from a dirty tools tree). Without git (a source zip) both are "unknown".
//!
//! Rebuild triggers: the package sources (any change re-evaluates the dirty flag) and the git
//! HEAD / branch ref / packed-refs / index files, so a commit or checkout re-stamps the binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    let mut watch: Vec<String> = vec![];
    for p in ["HEAD", "index", "packed-refs"] {
        if let Some(x) = git(&dir, &["rev-parse", "--git-path", p]) {
            watch.push(x);
        }
    }
    if let Some(r) = git(&dir, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(x) = git(&dir, &["rev-parse", "--git-path", &r]) {
            watch.push(x);
        }
    }
    for w in watch {
        let p = if Path::new(&w).is_absolute() { PathBuf::from(&w) } else { dir.join(&w) };
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    let sha = git(&dir, &["rev-parse", "HEAD"]).filter(|s| s.len() == 40).unwrap_or_else(|| "unknown".into());
    // dirty = uncommitted changes in THIS package (the gate's rules, scenarios and lints)
    let dirty = match git(&dir, &["status", "--porcelain", "--untracked-files=no", "--", "."]) {
        Some(s) if s.is_empty() => "0",
        Some(_) => "1",
        None => "unknown",
    };
    println!("cargo:rustc-env=HSMP_GIT_SHA={sha}");
    println!("cargo:rustc-env=HSMP_GIT_DIRTY={dirty}");
}

//! `hsmp-tools lua-check [paths...]`: compile (not run) every Lua file under
//! Lua 5.4. Default: every `*.lua` under mod/ (recursively, skipping the
//! RE-UE4SS checkout). Exit 1 on any syntax error.

use hsmp_tools::{luatest, paths};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Files or directories to check (default: mods/)
    paths: Vec<PathBuf>,
}

pub fn run(a: Args) -> anyhow::Result<i32> {
    let root = paths::repo_root()?;
    let targets = if a.paths.is_empty() { vec![paths::mods_dir(&root)] } else { a.paths };
    let mut files = vec![];
    for t in targets {
        if t.is_dir() {
            files.extend(paths::walk(&t, &|p| {
                p.extension().map(|e| e == "lua").unwrap_or(false)
                    && !p.components().any(|c| c.as_os_str() == "RE-UE4SS")
            }));
        } else if t.is_file() {
            files.push(t);
        } else {
            anyhow::bail!("no such file or directory: {}", t.display());
        }
    }
    let lua = luatest::new_lua();
    let mut bad = 0;
    for f in &files {
        let src = std::fs::read(f)?;
        let name = format!("@{}", paths::rel(&root, f));
        if let Err(e) = lua.load(&src[..]).set_name(name).into_function() {
            bad += 1;
            println!("FAIL {}: {e}", paths::rel(&root, f));
        }
    }
    println!("{} Lua file(s) checked under Lua 5.4, {} with syntax errors", files.len(), bad);
    Ok(if bad > 0 { 1 } else { 0 })
}

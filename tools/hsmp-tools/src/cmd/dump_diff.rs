//! `hsmp-tools dump-diff FRESH OLD`: diff two HSMP property dumps
//! (`== Section:` headers, `  key = value` lines), ignoring volatile values
//! (addresses, runtime ids, UberGraphFrame, userdata).

use anyhow::Result;
use hsmp_tools::paths;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// The fresh dump
    fresh: PathBuf,
    /// The old dump
    old: PathBuf,
}

fn load(p: &Path) -> Result<BTreeMap<(String, String), String>> {
    let text = paths::read_text(p)?.replace("\r\n", "\n").replace('\r', "\n");
    let sec_re = Regex::new(r"^== (\w+):").unwrap();
    let kv_re = Regex::new(r"^  (.+?) = (.*)$").unwrap();
    let mut d = BTreeMap::new();
    let mut sec: Option<String> = None;
    for line in text.split_terminator('\n') {
        if let Some(c) = sec_re.captures(line) {
            sec = Some(c[1].to_string());
            continue;
        }
        if let (Some(c), Some(s)) = (kv_re.captures(line), &sec) {
            d.insert((s.clone(), c[1].to_string()), c[2].to_string());
        }
    }
    Ok(d)
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn run(a: Args) -> Result<i32> {
    let (da, db) = (load(&a.fresh)?, load(&a.old)?);
    let skip = Regex::new(r"skipped|0x[0-9A-F]{6,}|_\d{8,}|UberGraphFrame|<userdata>").unwrap();
    let mut keys: Vec<&(String, String)> = da.keys().chain(db.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut n = 0;
    for k in keys {
        let x = da.get(k).map(|s| s.as_str()).unwrap_or("<absent>");
        let y = db.get(k).map(|s| s.as_str()).unwrap_or("<absent>");
        if x != y && !(skip.is_match(x) || skip.is_match(y)) {
            println!("{:<12} {:<40} fresh={:<30} old={}", k.0, k.1, cut(x, 60), cut(y, 60));
            n += 1;
        }
    }
    println!("{n} differing properties");
    Ok(0)
}

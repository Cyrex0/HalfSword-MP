//! Lint framework for HSMP Lua mods.
//!
//! A lint loads files ([`LuaFile`]: source + code-only lines from [`crate::lualex`]),
//! pushes [`Finding`]s into a [`Report`], and ends with [`Report::finish`], which
//! prints `file:line  message` lines plus a summary and returns the exit code
//! (1 if anything was found, 0 when clean).
//!
//! ```ignore
//! use hsmp_tools::{lint, paths};
//! let root = paths::repo_root()?;
//! let mut rep = lint::Report::new("my-lint");
//! for f in lint::LuaFile::load_all(&root, &paths::mod_script_files(&root))? {
//!     for (ln, code) in f.code_lines() {
//!         if code.contains("LoopAsync(") { rep.push(&f, ln, "use LoopInGameThreadWithDelay"); }
//!     }
//! }
//! std::process::exit(rep.finish("no LoopAsync"));
//! ```

use crate::{lualex, paths};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// One Lua source file prepared for linting.
pub struct LuaFile {
    pub path: PathBuf,
    /// Path relative to the repo root, `/`-separated.
    pub rel: String,
    /// The raw source.
    pub src: String,
    /// Code-only lines (strings blanked to `""`, comments removed), index 0 = line 1.
    pub code: Vec<String>,
}

impl LuaFile {
    pub fn load(root: &Path, path: &Path) -> Result<LuaFile> {
        let src = paths::read_text(path)?;
        let code = lualex::code_lines(&src);
        Ok(LuaFile { path: path.to_path_buf(), rel: paths::rel(root, path), src, code })
    }

    pub fn load_all(root: &Path, files: &[PathBuf]) -> Result<Vec<LuaFile>> {
        files.iter().map(|p| LuaFile::load(root, p)).collect()
    }

    /// The mod folder name (`mods/<Mod>/Scripts/x.lua` -> `<Mod>`), or the file stem.
    pub fn mod_name(&self) -> String {
        let p = &self.path;
        match (p.parent(), p.parent().and_then(|d| d.parent())) {
            (Some(s), Some(m)) if s.file_name().map(|n| n == "Scripts").unwrap_or(false) => {
                m.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            }
            _ => p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        }
    }

    /// `(line_number, code)` pairs, 1-based.
    pub fn code_lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.code.iter().enumerate().map(|(i, l)| (i + 1, l.as_str()))
    }

    /// `(line_number, raw_line)` pairs, 1-based.
    pub fn raw_lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.src.split('\n').enumerate().map(|(i, l)| (i + 1, l.trim_end_matches('\r')))
    }
}

/// One lint hit.
#[derive(Debug, Clone)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub msg: String,
}

/// Collected findings of one lint run.
pub struct Report {
    pub name: String,
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn new(name: &str) -> Report {
        Report { name: name.to_string(), findings: vec![], notes: vec![] }
    }

    pub fn push(&mut self, f: &LuaFile, line: usize, msg: impl Into<String>) {
        self.findings.push(Finding { file: f.rel.clone(), line, msg: msg.into() });
    }

    /// A finding not tied to a [`LuaFile`] (`file` is any label).
    pub fn push_at(&mut self, file: impl Into<String>, line: usize, msg: impl Into<String>) {
        self.findings.push(Finding { file: file.into(), line, msg: msg.into() });
    }

    /// An informational line printed before the findings.
    pub fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Print notes, findings and a summary; returns the process exit code.
    pub fn finish(&self, ok_msg: &str) -> i32 {
        for n in &self.notes {
            println!("{n}");
        }
        for f in &self.findings {
            println!("{}:{}  {}", f.file, f.line, f.msg);
        }
        if self.findings.is_empty() {
            println!("OK: {ok_msg}");
            0
        } else {
            println!("{}: {} finding(s)", self.name, self.findings.len());
            1
        }
    }
}

/// Byte offsets of every match of `re` in `code`, with its first capture group.
pub fn captures<'a>(re: &regex::Regex, code: &'a str) -> Vec<(usize, &'a str)> {
    re.captures_iter(code)
        .filter_map(|c| c.get(1).or_else(|| c.get(0)).map(|m| (m.start(), m.as_str())))
        .collect()
}

/// `mods/mods.release.txt`: mod name -> enabled in some deploy profile (`1` or `dev`).
/// Mods not listed are deployed disabled (absent from the map).
pub fn release_mod_status(root: &Path) -> std::collections::BTreeMap<String, bool> {
    let mut m = std::collections::BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(root.join("mods").join("mods.release.txt")) {
        for l in t.lines() {
            let l = l.trim();
            if l.starts_with('#') {
                continue;
            }
            if let Some((n, v)) = l.split_once(':') {
                let v = v.trim();
                m.insert(n.trim().to_string(), v == "1" || v == "dev");
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_status() {
        let tmp = paths::make_temp_dir("hsmp_relstat_").unwrap();
        std::fs::create_dir_all(tmp.join("mods")).unwrap();
        std::fs::write(tmp.join("mods/mods.release.txt"), "# c : 1\nA : 1\nB : 0\nC : dev\n\n").unwrap();
        let m = release_mod_status(&tmp);
        assert_eq!(m.get("A"), Some(&true));
        assert_eq!(m.get("B"), Some(&false));
        assert_eq!(m.get("C"), Some(&true));
        assert_eq!(m.get("D"), None);
        assert!(!m.contains_key("# c"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}

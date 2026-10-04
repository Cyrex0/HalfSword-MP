//! check_travel: G0 travel lint (gate rule DoD-4). Only the Director may change
//! the level.
//!
//!     cargo run --release -p hsmp-tools --bin check_travel
//!
//! Fails (exit 1) when any Lua file under `mods/*/Scripts/` or
//! `mods/shared/` outside the allow-list
//! * references `OpenLevel`, `OpenLevelBySoftObjectPtr`, `ServerTravel` or
//!   `ClientTravel`: a call or a bare member reference (`x:OpenLevel(`,
//!   `local open = x.OpenLevel`), a string key `x["OpenLevel"]`, a key built with `..`
//!   (`x["Open".."Level"]`), or the bare name as a string value (`local k = "OpenLevel"`), or
//! * issues a console level change: a string literal starting with `open `,
//!   `travel ` or `servertravel ` on a line that also calls `ConsoleCommand`,
//!   `ExecuteConsoleCommand`, `ExecConsole` or builds an `FString`.
//!
//! Comments are ignored, and so is every string for the call rule, so
//! `RegisterHook(".../GameplayStatics:OpenLevel", ...)` is allowed everywhere.
//!
//! Allow-list (exact files):
//! * `HSMPMatch/Scripts/director.lua`: the Director, the single caller.
//! * `HSMPMenu/Scripts/legacy_travel.lua`: TODO: the temporary
//!   compatibility shim (docs/development/subsystems/director-contract.md). Delete
//!   this entry together with the shim.
//!
//! Retired mods (HSMPLobby, HSMPAdmin, HSMPCharacter, HSMPSettings, HSMPChat;
//! never deployed) are reported as notes, not failures.

use hsmp_tools::{lint, luablock, paths};
use regex::Regex;

const ALLOWED: &[(&str, &str)] = &[
    ("mods/HSMPMatch/Scripts/director.lua", "the Director (sole level-change owner)"),
    (
        "mods/HSMPMenu/Scripts/legacy_travel.lua",
        "TODO(UI): temporary legacy shim, remove with the shim (director-contract §5)",
    ),
];
const RETIRED: &[&str] = &["HSMPLobby", "HSMPAdmin", "HSMPCharacter", "HSMPSettings", "HSMPChat"];

struct Rules {
    call: Regex,
    call_idx: Regex,
    name_lit: Regex,
    idx_concat: Regex,
    lit: Regex,
    console: Regex,
    console_call: Regex,
}

/// The engine functions that change the level.
const TRAVEL_FNS: &[&str] = &["OpenLevel", "OpenLevelBySoftObjectPtr", "ServerTravel", "ClientTravel"];

impl Rules {
    fn new() -> Self {
        Rules {
            // a member reference, called or not (`local open = gs.OpenLevel; open(...)`)
            call: Regex::new(r"[:.]\s*(OpenLevel|OpenLevelBySoftObjectPtr|ServerTravel|ClientTravel)\b").unwrap(),
            call_idx: Regex::new(r#"\[\s*["'](OpenLevel|OpenLevelBySoftObjectPtr|ServerTravel|ClientTravel)["']\s*\]"#)
                .unwrap(),
            // the bare name as a string value (`local k = "OpenLevel"; gs[k](...)`); a hook path
            // ("/Script/Engine.GameplayStatics:OpenLevel") is not the bare name
            name_lit: Regex::new(r#"["'](OpenLevel|OpenLevelBySoftObjectPtr|ServerTravel|ClientTravel)["']"#).unwrap(),
            // a computed index key built with `..` (`gs["Open".."Level"]`)
            idx_concat: Regex::new(r"\[([^\[\]]*\.\.[^\[\]]*)\]").unwrap(),
            lit: Regex::new(r#""([^"\\]*)"|'([^'\\]*)'"#).unwrap(),
            console: Regex::new(r#"["'](?:open|travel|servertravel)\s"#).unwrap(),
            console_call: Regex::new(r"(ExecuteConsoleCommand|ConsoleCommand|ExecConsole|FString)\s*\(").unwrap(),
        }
    }

    /// `gs["Open".."Level"]`, `gs["Open" .. x]`: the literals of a `..`-built key, joined, name a
    /// travel function or are a piece (>= 4 chars) of one.
    fn concat_key(&self, code: &str) -> Option<String> {
        for c in self.idx_concat.captures_iter(code) {
            let lits: Vec<String> = self.lit.captures_iter(&c[1]).filter_map(|l| l.get(1).or_else(|| l.get(2)).map(|m| m.as_str().to_string())).collect();
            if lits.is_empty() {
                continue;
            }
            let joined: String = lits.concat();
            let hit = TRAVEL_FNS.iter().any(|f| {
                joined.eq_ignore_ascii_case(f) || lits.iter().any(|l| l.len() >= 4 && f.to_ascii_lowercase().contains(&l.to_ascii_lowercase()))
            });
            if hit {
                return Some(c[0].trim().to_string());
            }
        }
        None
    }
}

/// `(line, what)` for every level change in `src`.
fn scan(rules: &Rules, src: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (i, code) in luablock::strip_comments(src).iter().enumerate() {
        let blank = luablock::blank_strings(code);
        if let Some(m) = rules.call.captures(&blank) {
            hits.push((i + 1, format!("{}()", &m[1])));
        } else if let Some(m) = rules.call_idx.captures(code) {
            hits.push((i + 1, format!("[\"{}\"]()", &m[1])));
        } else if let Some(k) = rules.concat_key(code) {
            hits.push((i + 1, format!("{k} (a computed travel-function key)")));
        } else if let Some(m) = rules.name_lit.captures(code) {
            hits.push((i + 1, format!("\"{}\" (the travel function's name as a value)", &m[1])));
        } else if rules.console.is_match(code) && rules.console_call.is_match(code) {
            hits.push((i + 1, "console open/travel".into()));
        }
    }
    hits
}

fn main() {
    let root = match paths::repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("check_travel: {e:#}");
            std::process::exit(2);
        }
    };
    let mut files = paths::mod_script_files(&root);
    files.extend(paths::glob(&root.join("mods"), "shared/*.lua"));
    let files = match lint::LuaFile::load_all(&root, &files) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("check_travel: {e:#}");
            std::process::exit(2);
        }
    };
    let rules = Rules::new();
    let mut rep = lint::Report::new("check_travel");
    let mut director_calls = 0;
    for f in &files {
        let hits = scan(&rules, &f.src);
        if hits.is_empty() {
            continue;
        }
        if let Some((_, why)) = ALLOWED.iter().find(|(p, _)| *p == f.rel) {
            if f.rel.ends_with("director.lua") {
                director_calls += hits.len();
            }
            rep.note(format!("allowed  {} ({} call site(s)): {}", f.rel, hits.len(), why));
            continue;
        }
        let m = f.mod_name();
        if RETIRED.contains(&m.as_str()) {
            for (ln, what) in &hits {
                rep.note(format!("retired  {}:{}  {} (not deployed)", f.rel, ln, what));
            }
            continue;
        }
        for (ln, what) in hits {
            rep.push(f, ln, format!("{what} outside the Director (only HSMPMatch/Scripts/director.lua may change the level)"));
        }
    }
    if director_calls == 0 {
        rep.push_at("mods/HSMPMatch/Scripts/director.lua", 1, "the Director has no level-change call (lint out of date?)");
    }
    std::process::exit(rep.finish("no level change outside the Director"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(src: &str) -> usize {
        scan(&Rules::new(), src).len()
    }

    #[test]
    fn flags_calls_index_calls_and_console_open() {
        assert_eq!(n("gs:OpenLevel(world, FName(n), true, FString(''))"), 1);
        assert_eq!(n("gs.OpenLevelBySoftObjectPtr(w, x)"), 1);
        assert_eq!(n("gs[\"OpenLevel\"](gs, w, n)"), 1);
        assert_eq!(n("pc:ConsoleCommand(FString(\"open \" .. path), false)"), 1);
        assert_eq!(n("pc:ConsoleCommand(FString('travel x'), false)"), 1);
    }

    #[test]
    fn ignores_hooks_comments_and_display_strings() {
        assert_eq!(n("RegisterHook(\"/Script/Engine.GameplayStatics:OpenLevel\", f)"), 0);
        assert_eq!(n("-- gs:OpenLevel(w, n)\n--[[ pc:ConsoleCommand(FString(\"open x\")) ]]"), 0);
        assert_eq!(n("local name = \"Open Sallet A1\""), 0);
        assert_eq!(n("Log(\"open the door\")"), 0);
        assert_eq!(n("x = \"gs:OpenLevel(w)\""), 0);
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;

    fn n(src: &str) -> usize {
        scan(&Rules::new(), src).len()
    }

    /// Aliased and computed forms.
    #[test]
    fn flags_aliases_and_computed_keys() {
        assert_eq!(n("local open = gs.OpenLevel; open(gs, w, n)"), 1);
        assert_eq!(n("local f = gs.ServerTravel"), 1);
        assert_eq!(n("pcall(gs.ClientTravel, gs, x)"), 1);
        assert_eq!(n("gs[\"Open\"..\"Level\"](gs, w, n)"), 1);
        assert_eq!(n("gs['Open' .. 'Level'](gs, w, n)"), 1);
        assert_eq!(n("local f = gs[\"Server\" .. suffix]"), 1);
        assert_eq!(n("local k = \"OpenLevel\"\ngs[k](gs, w, n)"), 1);
    }

    #[test]
    fn computed_keys_and_names_that_are_not_travel() {
        assert_eq!(n("local v = t[\"key_\" .. i]"), 0);
        assert_eq!(n("local s = \"Open\" .. \"Level\" -- a string, not a key"), 0);
        assert_eq!(n("for _, fn in ipairs({ \"/Script/Engine.GameplayStatics:OpenLevel\" }) do end"), 0);
        assert_eq!(n("local OpenLevelHook = 1\nlocal x = OpenLevelHook"), 0);
        assert_eq!(n("local open_level_seen = t.OpenLevelSeen"), 0);
    }
}

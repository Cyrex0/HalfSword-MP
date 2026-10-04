//! World-guard lint rules.
//!
//! The crash it prevents: a UObject cached before a level change (the round
//! reset re-opens the arena) is touched afterwards -> access violation that
//! pcall cannot catch. `shared/hsmp_wg.lua` is the one guard; every mod-level
//! UObject cache must be reset in a drop handler (`WG.on_drop` / `WG.cache` /
//! legacy `wg_on_drop`).
//!
//! Rules (line-based heuristics over comment-free Lua, per file):
//!
//! * **W1 guard**: a mod that touches UObjects (FindAllOf, FindFirstOf,
//!   StaticFindObject, NotifyOnNewObject, RegisterHook, GetPlayerController,
//!   StaticConstructObject) must use a guard. Classified as `shared`
//!   (requires `hsmp_wg`), `legacy` (its own `local function wg_check` block),
//!   `loadmap` (only `RegisterLoadMapPreHook`), or `none`.
//! * **W2 cache**: a module-level `local X` (column 0) that is assigned, inside
//!   a function, a value from a UObject source (FindAllOf/FindFirstOf/
//!   StaticFindObject/StaticConstructObject/construct/GetPlayerController/
//!   GetWorld/`:get()`/`.Pawn`/SpawnActor…; also `X.f = …`, `X[k] = …` and
//!   `table.insert(X, …)` next to a FindAllOf/NotifyOnNewObject) must have its
//!   name appear in a drop-handler body (`on_drop(`/`WG.cache(`/
//!   `RegisterLoadMapPreHook(` blocks, plus one level of module-level functions
//!   those blocks call). Persistent objects are exempt automatically when the
//!   source is a `/Script/` or `Default__` StaticFindObject, GetGameInstance,
//!   GetGameplayStatics or GetKismet*, or when the value is converted
//!   (GetFullName, ToString, tostring, tonumber, GetAddress, IsValid,
//!   comparisons). A local in the enclosing function with the same name
//!   shadows the module-level one.
//! * **W3 deferred capture**: an `ExecuteWithDelay` /
//!   `ExecuteInGameThreadWithDelay` / `ExecuteInGameThread` callback that uses
//!   a UObject captured from the enclosing function (a `local x = <UObject
//!   source>`, or a parameter of a RegisterHook/NotifyOnNewObject callback)
//!   without a world check in its body (`same(`, `check(`, `wg_`,
//!   `world_gen`, `WG.`).
//!
//! Suppression: `-- wg: ok <reason>` on the declaration or assignment line.
//!
//! Severity: findings in mods that use the shared guard are **errors** (exit 1).
//! Findings in mods that still carry a legacy block, or no guard, are **todo**
//! (exit 0) unless `--strict` (the DoD-13 target once every
//! mod is converted). Known blind spots: caches assigned through a helper
//! function's return value, aliases (`local t = cache; t.x = obj`), objects
//! kept in closures; review still applies.

use hsmp_tools::luablock::{block_end, blank_strings, has_word, indent, strip_comments};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
/// Retired mods (never deployed): no W1 finding.
pub const RETIRED: &[&str] = &["HSMPLobby", "HSMPAdmin", "HSMPCharacter", "HSMPSettings", "HSMPChat"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    Shared,
    Legacy,
    LoadMap,
    None,
}

impl Guard {
    pub fn label(self) -> &'static str {
        match self {
            Guard::Shared => "shared",
            Guard::Legacy => "legacy",
            Guard::LoadMap => "loadmap",
            Guard::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub rule: &'static str,
    pub msg: String,
    pub error: bool,
}

#[derive(Debug, Clone)]
pub struct ModReport {
    pub name: String,
    pub guard: Guard,
    pub touches: bool,
    pub findings: Vec<Finding>,
    /// Conversion call sites (legacy blocks, wg_* uses, load-map hooks).
    pub sites: Vec<(String, usize, String)>,
}

struct Rx {
    touch: Regex,
    module_local: Regex,
    drop_open: Regex,
    assign: Regex,
    field_assign: Regex,
    insert: Regex,
    uobj: Regex,
    exempt: Regex,
    delayed: Regex,
    hook_line: Regex,
    params: Regex,
    multi_assign: Regex,
    local_decl: Regex,
    local_fn: Regex,
    call_ident: Regex,
    guard_in_body: Regex,
    site: Regex,
}

impl Rx {
    fn new() -> Self {
        Rx {
            touch: Regex::new(r"\b(FindAllOf|FindFirstOf|StaticFindObject|NotifyOnNewObject|RegisterHook|GetPlayerController|StaticConstructObject)\b").unwrap(),
            module_local: Regex::new(r"^local\s+([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s*(?:=|$)").unwrap(),
            drop_open: Regex::new(r"\b(wg_on_drop|on_drop|WG\.cache|RegisterLoadMapPreHook)\s*\(").unwrap(),
            assign: Regex::new(r"^\s+([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s*=\s*([^=].*)$").unwrap(),
            field_assign: Regex::new(r"^\s+([A-Za-z_]\w*)(?:\.[A-Za-z_]\w*|\[[^\]]*\])+\s*=\s*([^=].*)$").unwrap(),
            insert: Regex::new(r"table\.insert\(\s*([A-Za-z_]\w*)\s*,").unwrap(),
            uobj: Regex::new(r"\b(FindAllOf|FindFirstOf|FindObject|StaticFindObject|StaticConstructObject|construct|SpawnActor\w*|BeginDeferredActorSpawnFromClass|GetPlayerController|GetWorld|K2_GetPawn|GetOwner|GetComponentByClass|GetAllActorsOfClass)\s*\(|:get\(\)|\.Pawn\b|\.SK_Skeleton\b|\.Mesh\b|\.RootComponent\b").unwrap(),
            exempt: Regex::new(r#"StaticFindObject\(\s*"/Script/|Default__|GetGameInstance|GetGameplayStatics|GetKismet|GetFullName|ToString|tostring|tonumber|GetAddress|IsValid|==|~=|#"#).unwrap(),
            delayed: Regex::new(r"\b(ExecuteWithDelay|ExecuteInGameThreadWithDelay|ExecuteInGameThread)\s*\(").unwrap(),
            hook_line: Regex::new(r"\b(RegisterHook|NotifyOnNewObject|RegisterBeginPlayPostHook|RegisterBeginPlayPreHook)\b").unwrap(),
            params: Regex::new(r"function\s*[A-Za-z_0-9.:]*\s*\(([^)]*)\)").unwrap(),
            multi_assign: Regex::new(
                r#"^\s+([A-Za-z_][\w.\[\]"]*(?:\s*,\s*[A-Za-z_][\w.\[\]"]*)*)\s*=\s*([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s*$"#,
            )
            .unwrap(),
            local_decl: Regex::new(r"^\s*local\s+([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s*(=\s*(.*))?$").unwrap(),
            local_fn: Regex::new(r"^local\s+function\s+([A-Za-z_]\w*)").unwrap(),
            call_ident: Regex::new(r"\b([A-Za-z_]\w*)\s*\(").unwrap(),
            guard_in_body: Regex::new(r"same\(|check\(|\bwg_|world_gen|\bWG\.").unwrap(),
            site: Regex::new(r"^local WG = \{|^local function wg_\w+|\bwg_(check|on_drop|token|same|travel)\s*\(|RegisterLoadMapPreHook\s*\(").unwrap(),
        }
    }
}

fn split_names(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn is_uobj_source(rx: &Rx, rhs: &str) -> bool {
    let b = blank_strings(rhs);
    // keep string args for the exemption check (/Script/, Default__)
    rx.uobj.is_match(&b) && !rx.exempt.is_match(rhs)
}

/// Lines of the enclosing top-level function of `i` (from the last column-0
/// line at or before `i`).
fn enclosing_start(lines: &[String], i: usize) -> usize {
    let mut j = i;
    loop {
        let l = &lines[j];
        if !l.trim().is_empty() && indent(l) == 0 {
            return j;
        }
        if j == 0 {
            return 0;
        }
        j -= 1;
    }
}

/// Names declared `local x = <UObject source>` in the enclosing top-level
/// function of line `i`, before it.
fn uobj_locals(rx: &Rx, lines: &[String], i: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for l in &lines[enclosing_start(lines, i)..i] {
        if let Some(c) = rx.local_decl.captures(l) {
            if let Some(rhs) = c.get(3) {
                if is_uobj_source(rx, rhs.as_str()) {
                    out.extend(split_names(&c[1]));
                }
            }
        }
    }
    out
}

fn shadowed(rx: &Rx, lines: &[String], i: usize, name: &str) -> bool {
    let start = enclosing_start(lines, i);
    for l in &lines[start..i] {
        if let Some(c) = rx.local_decl.captures(l) {
            if indent(l) > 0 && split_names(&c[1]).iter().any(|n| n == name) {
                return true;
            }
        }
        for c in rx.params.captures_iter(&blank_strings(l)) {
            if split_names(&c[1]).iter().any(|n| n == name) {
                return true;
            }
        }
    }
    false
}

/// Analyse one file. `guard` is the mod-level classification (severity).
pub fn scan_file(rel: &str, src: &str, guard: Guard) -> Vec<Finding> {
    let rx = Rx::new();
    let raw: Vec<&str> = src.split('\n').collect();
    let lines = strip_comments(src);
    let marker = |i: usize| raw.get(i).map(|l| l.contains("wg: ok") || l.contains("wg:ok")).unwrap_or(false);
    let error = guard == Guard::Shared;
    let mut out = Vec::new();

    // Module-level locals and local functions.
    let mut locals: BTreeMap<String, usize> = BTreeMap::new();
    let mut fns: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(c) = rx.local_fn.captures(l) {
            fns.insert(c[1].to_string(), (i, block_end(&lines, i)));
            continue;
        }
        if let Some(c) = rx.module_local.captures(l) {
            for n in split_names(&c[1]) {
                locals.entry(n).or_insert(i);
            }
        }
    }

    // Drop-handler text (+ one level of called module-level functions).
    let mut drop_text = String::new();
    let mut called: BTreeSet<String> = BTreeSet::new();
    for (i, l) in lines.iter().enumerate() {
        if rx.drop_open.is_match(&blank_strings(l)) {
            let e = block_end(&lines, i);
            for b in &lines[i..=e] {
                drop_text.push_str(b);
                drop_text.push('\n');
                for c in rx.call_ident.captures_iter(&blank_strings(b)) {
                    called.insert(c[1].to_string());
                }
            }
        }
    }
    for f in &called {
        if let Some(&(s, e)) = fns.get(f) {
            for b in &lines[s..=e] {
                drop_text.push_str(b);
                drop_text.push('\n');
            }
        }
    }

    // W2: module-level caches assigned from a UObject source inside functions.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for (i, l) in lines.iter().enumerate() {
        if indent(l) == 0 || l.trim_start().starts_with("local ") {
            continue;
        }
        let mut cands: Vec<String> = Vec::new();
        if let Some(c) = rx.field_assign.captures(l) {
            if is_uobj_source(&rx, &c[2]) {
                cands.push(c[1].to_string());
            }
        } else if let Some(c) = rx.assign.captures(l) {
            if is_uobj_source(&rx, &c[2]) {
                cands.extend(split_names(&c[1]));
            }
        }
        // `X.a, X.b = obj_a, obj_b` / `X = obj`: RHS names that are UObject
        // locals of the enclosing function (`local obj = construct(...)`).
        if cands.is_empty() {
            if let Some(c) = rx.multi_assign.captures(l) {
                let objs = uobj_locals(&rx, &lines, i);
                let rhs_obj = split_names(&c[2]).iter().any(|r| objs.contains(r.as_str()));
                if rhs_obj {
                    for part in split_names(&c[1]) {
                        let base: String = part.chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
                        if !base.is_empty() {
                            cands.push(base);
                        }
                    }
                }
            }
        }
        if let Some(c) = rx.insert.captures(l) {
            let lo = i.saturating_sub(8);
            let ctx = lines[lo..=i].join("\n");
            if Regex::new(r"\b(FindAllOf|FindFirstOf|NotifyOnNewObject)\b|:get\(\)").unwrap().is_match(&ctx) {
                cands.push(c[1].to_string());
            }
        }
        for name in cands {
            let Some(&decl) = locals.get(&name) else { continue };
            if reported.contains(&name) || shadowed(&rx, &lines, i, &name) || marker(i) || marker(decl) {
                continue;
            }
            if has_word(&drop_text, &name) {
                continue;
            }
            reported.insert(name.clone());
            out.push(Finding {
                file: rel.into(),
                line: i + 1,
                rule: "W2",
                msg: format!(
                    "module-level UObject cache '{}' (declared line {}) is never reset in a world-guard drop handler",
                    name,
                    decl + 1
                ),
                error,
            });
        }
    }

    // W3: deferred callbacks using a captured UObject without a world check.
    for (i, l) in lines.iter().enumerate() {
        let b = blank_strings(l);
        let Some(m) = rx.delayed.find(&b) else { continue };
        if !b[m.end()..].contains("function") {
            continue;
        }
        let e = block_end(&lines, i);
        let body: String = if e == i {
            b[m.end()..].to_string()
        } else {
            lines[i + 1..e].iter().map(|x| blank_strings(x)).collect::<Vec<_>>().join("\n")
        };
        if rx.guard_in_body.is_match(&body) {
            continue;
        }
        // Captured UObjects from the enclosing function.
        let start = enclosing_start(&lines, i);
        let mut captured: BTreeSet<String> = BTreeSet::new();
        for (j, pl) in lines.iter().enumerate().take(i + 1).skip(start) {
            let pb = blank_strings(pl);
            if rx.hook_line.is_match(&pb) || (j > 0 && rx.hook_line.is_match(&blank_strings(&lines[j - 1])) && pb.contains("function")) {
                for c in rx.params.captures_iter(&pb) {
                    for n in split_names(&c[1]) {
                        if n != "..." && !n.starts_with('_') {
                            captured.insert(n);
                        }
                    }
                }
            }
            if j < i {
                if let Some(c) = rx.local_decl.captures(pl) {
                    if let Some(rhs) = c.get(3) {
                        if is_uobj_source(&rx, rhs.as_str()) {
                            captured.extend(split_names(&c[1]));
                        }
                    }
                }
            }
        }
        let used: Vec<&String> = captured.iter().filter(|n| has_word(&body, n)).collect();
        if used.is_empty() || marker(i) {
            continue;
        }
        out.push(Finding {
            file: rel.into(),
            line: i + 1,
            rule: "W3",
            msg: format!(
                "deferred callback uses captured UObject(s) {} without a world check (WG.token()/WG.same())",
                used.iter().map(|s| format!("'{}'", s)).collect::<Vec<_>>().join(", ")
            ),
            error,
        });
    }
    out
}

/// Classify a mod's guard from all its sources.
pub fn classify(srcs: &[(String, String)]) -> (Guard, bool) {
    let rx = Rx::new();
    let mut touches = false;
    let (mut shared, mut legacy, mut loadmap) = (false, false, false);
    for (_, s) in srcs {
        let code = strip_comments(s).join("\n");
        touches |= rx.touch.is_match(&code);
        shared |= code.contains("\"hsmp_wg\"") || code.contains("'hsmp_wg'");
        legacy |= code.contains("local function wg_check");
        loadmap |= code.contains("RegisterLoadMapPreHook");
    }
    let g = if shared {
        Guard::Shared
    } else if legacy {
        Guard::Legacy
    } else if loadmap {
        Guard::LoadMap
    } else {
        Guard::None
    };
    (g, touches)
}

/// `files`: (mod name, repo-relative path, source) for every mod script.
pub fn scan_mods(files: &[(String, String, String)], strict: bool) -> Vec<ModReport> {
    let rx = Rx::new();
    let mut by_mod: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (m, rel, s) in files {
        if m.is_empty() || m == "shared" {
            continue;
        }
        by_mod.entry(m.clone()).or_default().push((rel.clone(), s.clone()));
    }
    let mut reps = Vec::new();
    for (name, srcs) in by_mod {
        let (guard, touches) = classify(&srcs);
        let mut findings = Vec::new();
        let mut sites = Vec::new();
        for (rel, s) in &srcs {
            let mut f = scan_file(rel, s, guard);
            if strict {
                for x in &mut f {
                    x.error = true;
                }
            }
            findings.extend(f);
            if guard != Guard::Shared {
                for (i, l) in strip_comments(s).iter().enumerate() {
                    if rx.site.is_match(l) {
                        sites.push((rel.clone(), i + 1, l.trim().to_string()));
                    }
                }
            }
        }
        if touches && guard == Guard::None && !RETIRED.contains(&name.as_str()) {
            findings.push(Finding {
                file: srcs[0].0.clone(),
                line: 1,
                rule: "W1",
                msg: "mod touches UObjects but has no world guard (require \"hsmp_wg\")".into(),
                error: strict,
            });
        }
        reps.push(ModReport { name, guard, touches, findings, sites });
    }
    reps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w2_flags_unregistered_cache_and_accepts_registered_one() {
        let src = r#"
local banner = nil
local foes = {}
local function f()
    banner = StaticConstructObject(cls, world)
    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
        table.insert(foes, w)
    end
end
local banner2 = {}
local function build(addr)
    local uw = construct("/Script/UMG.UserWidget", world)
    banner2.uw, banner2.addr = uw, addr
end
WG.on_drop(function() banner = nil end)
"#;
        let f = scan_file("x.lua", src, Guard::Shared);
        assert_eq!(f.len(), 2, "{:?}", f);
        assert!(f[0].msg.contains("'foes'") && f[1].msg.contains("'banner2'"));
        assert!(f[0].error);
    }

    #[test]
    fn w2_exemptions_marker_shadowing_and_called_reset_fn() {
        let src = r#"
local gi = nil
local cls = nil
local pawn = nil
local kept = nil -- wg: ok persistent class
local cache = nil
local function reset() cache = nil end
local function f(pawn)
    gi = UEHelpers.GetGameInstance()
    cls = StaticFindObject("/Script/UMG.UserWidget")
    pawn = pc.Pawn
    kept = FindFirstOf("Thing")
    cache = FindFirstOf("Other")
    local name = w:GetFullName()
end
RegisterLoadMapPreHook(function() reset() end)
"#;
        let f = scan_file("x.lua", src, Guard::Legacy);
        assert!(f.is_empty(), "{:?}", f);
    }

    #[test]
    fn w3_flags_deferred_capture_and_accepts_token_check() {
        let bad = r#"
pcall(function()
    RegisterHook("/Script/MovieScene.MovieSceneSequencePlayer:Play", function(self)
        local p = self and self:get()
        ExecuteWithDelay(1, function() handle_sequence(p, "Play hook") end)
    end)
end)
"#;
        let f = scan_file("x.lua", bad, Guard::Legacy);
        assert_eq!(f.len(), 1, "{:?}", f);
        assert_eq!(f[0].rule, "W3");
        assert!(!f[0].error);
        let good = r#"
local function teleport(pawn)
    local sk = pawn.SK_Skeleton
    local wt = wg_token()
    ExecuteWithDelay(150, function()
        if not wg_same(wt) then return end
        sk:SetSimulatePhysics(true)
    end)
end
"#;
        assert!(scan_file("x.lua", good, Guard::Legacy).is_empty());
    }

    #[test]
    fn classifies_guards() {
        let s = |x: &str| vec![("a.lua".to_string(), x.to_string())];
        assert_eq!(classify(&s("local HW = load_module(\"hsmp_wg\")\nFindAllOf('x')")).0, Guard::Shared);
        assert_eq!(classify(&s("local function wg_check(pc) end")).0, Guard::Legacy);
        assert_eq!(classify(&s("RegisterLoadMapPreHook(function() end)")).0, Guard::LoadMap);
        assert_eq!(classify(&s("FindAllOf('x')")), (Guard::None, true));
    }
}

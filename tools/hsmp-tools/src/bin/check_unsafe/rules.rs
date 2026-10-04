//! The UNSAFE-API rules. Each one is a crash we already had (or a silent
//! no-op that hid a bug); see main.rs for the table and the crash history.

use crate::lex::{self, function_at, paren_close, Kind, Tok};
use crate::sig::Sigs;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub rule: &'static str,
    pub msg: String,
    /// The trimmed source line (baseline key; survives line shifts).
    pub code: String,
}

pub struct SrcFile {
    pub module: String,
    pub rel: String,
    pub src: String,
}

/// The only file allowed to issue console commands (the Director).
pub const CONSOLE_ALLOWED: &[&str] = &["mods/HSMPMatch/Scripts/director.lua"];

/// Hook-style registrations whose callbacks run on engine events.
const HOOK_FNS: &[&str] = &[
    "RegisterHook",
    "NotifyOnNewObject",
    "RegisterBeginPlayPostHook",
    "RegisterBeginPlayPreHook",
    "RegisterProcessConsoleExecPreHook",
    "RegisterProcessConsoleExecPostHook",
    "RegisterCallFunctionByNameWithArgumentsPreHook",
];

struct Ctx<'a> {
    f: &'a SrcFile,
    raw: Vec<&'a str>,
    toks: Vec<Tok>,
    out: Vec<Finding>,
}

impl<'a> Ctx<'a> {
    fn new(f: &'a SrcFile) -> Self {
        Ctx { f, raw: f.src.split('\n').map(|l| l.trim_end_matches('\r')).collect(), toks: lex::lex(&f.src), out: vec![] }
    }
    fn suppressed(&self, line: usize) -> bool {
        self.raw.get(line.wrapping_sub(1)).map(|l| l.contains("unsafe: ok") || l.contains("unsafe:ok")).unwrap_or(false)
    }
    fn push(&mut self, line: usize, rule: &'static str, msg: String) {
        if self.suppressed(line) {
            return;
        }
        let code = self.raw.get(line.wrapping_sub(1)).map(|l| l.trim().to_string()).unwrap_or_default();
        self.out.push(Finding { file: self.f.rel.clone(), line, rule, msg, code });
    }
    /// Token `k` is a member access/call name: preceded by `.` or `:`.
    fn is_member(&self, k: usize) -> bool {
        k > 0 && (self.toks[k - 1].op(".") || self.toks[k - 1].op(":"))
    }
    /// `name` used as `x:name(`, `x.name(`, `x["name"](` or a bare `name(`;
    /// returns the line of each use.
    fn uses(&self, name: &str, member_only: bool) -> Vec<usize> {
        let t = &self.toks;
        let mut v = vec![];
        for k in 0..t.len() {
            if t[k].name(name) && (!member_only || self.is_member(k)) {
                v.push(t[k].line);
            } else if t[k].kind == Kind::Str && t[k].text == name && k > 0 && t[k - 1].op("[") && t.get(k + 1).map(|x| x.op("]")).unwrap_or(false) {
                v.push(t[k].line);
            }
        }
        v
    }
}

/// Named functions of the file: name -> index of their `function` token
/// (`local function f`, `function f`, `local f = function`, `f = function`).
fn named_fns(toks: &[Tok]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for k in 0..toks.len() {
        if toks[k].name("function") {
            if let Some(n) = toks.get(k + 1).filter(|t| t.kind == Kind::Name) {
                if toks.get(k + 2).map(|t| t.op("(")).unwrap_or(false) {
                    m.entry(n.text.clone()).or_insert(k);
                }
            }
        } else if toks[k].op("=") && k > 0 && toks[k - 1].kind == Kind::Name && toks.get(k + 1).map(|t| t.name("function")).unwrap_or(false) {
            let dotted = k > 1 && (toks[k - 2].op(".") || toks[k - 2].op(":"));
            if !dotted {
                m.entry(toks[k - 1].text.clone()).or_insert(k + 1);
            }
        }
    }
    m
}

/// Every hook registration in the file: (hook fn, path literal if any, line,
/// callbacks as (params, body start, body end)). A callback passed by name
/// (`RegisterHook(P, on_pre, on_post)`) resolves to that named function.
type Callback = (Vec<String>, usize, usize);
fn hook_calls(toks: &[Tok]) -> Vec<(String, Option<String>, usize, Vec<Callback>)> {
    let fns = named_fns(toks);
    let mut out = vec![];
    for k in 0..toks.len() {
        if toks[k].kind != Kind::Name || !HOOK_FNS.contains(&toks[k].text.as_str()) {
            continue;
        }
        if k > 0 && (toks[k - 1].op(".") || toks[k - 1].op(":") || toks[k - 1].name("function") || toks[k - 1].name("local")) {
            continue;
        }
        if !toks.get(k + 1).map(|t| t.op("(")).unwrap_or(false) {
            continue;
        }
        let close = paren_close(toks, k + 1);
        let path = match toks.get(k + 2) {
            Some(t) if t.kind == Kind::Str => Some(t.text.clone()),
            // a string constant: `local GET_DAMAGE = "/Game/...:Get Damage"`
            Some(t) if t.kind == Kind::Name && toks.get(k + 3).map(|x| x.op(",")).unwrap_or(false) => (2..toks.len())
                .find(|&q| toks[q].kind == Kind::Str && toks[q - 1].op("=") && toks[q - 2].text == t.text && q >= 3 && toks[q - 3].name("local"))
                .map(|q| toks[q].text.clone()),
            _ => None,
        };
        let mut cbs = vec![];
        let mut j = k + 2;
        let mut depth = 0i32;
        while j < close {
            let t = &toks[j];
            if t.op("(") || t.op("{") || t.op("[") {
                depth += 1;
            } else if t.op(")") || t.op("}") || t.op("]") {
                depth -= 1;
            } else if depth == 0 && t.name("function") {
                if let Some((p, b, e)) = function_at(toks, j) {
                    cbs.push((p, b, e));
                    j = e + 1;
                    continue;
                }
            } else if depth == 0
                && t.kind == Kind::Name
                && j > k + 2
                && toks[j - 1].op(",")
                && toks.get(j + 1).map(|x| x.op(",") || x.op(")")).unwrap_or(false)
            {
                if let Some(cb) = fns.get(&t.text).and_then(|&f| function_at(toks, f)) {
                    cbs.push(cb);
                }
            }
            j += 1;
        }
        out.push((toks[k].text.clone(), path, toks[k].line, cbs));
    }
    out
}

// ---------------------------------------------------------------- U1 / U2

/// U1: a hook callback touches a Soft* parameter (`p:get()`, `p.x`, `p[...]`):
/// UE4SS push_softobjectproperty does a null memcpy -> uncatchable AV.
fn u1_soft_hook_params(c: &mut Ctx, sigs: &Sigs) {
    for (_, path, line, cbs) in hook_calls(&c.toks) {
        let Some(path) = path else { continue };
        let soft = sigs.soft_params(&path);
        if soft.is_empty() {
            continue;
        }
        for (params, b, e) in &cbs {
            // callback param 0 is the context (self); param i+1 = UFunction param i
            for (i, pname) in &soft {
                let Some(lp) = params.get(i + 1) else { continue };
                if lp == "..." || lp.starts_with('_') {
                    continue;
                }
                let mut hit = None;
                for k in *b..*e {
                    let t = &c.toks[k];
                    // any reference: `p:get()` crashes directly, and passing
                    // p on (pv(p), pcall(f, p)) hands it to code that may
                    if t.name(lp) && !c.is_member(k) {
                        hit = Some(t.line);
                        break;
                    }
                }
                if let Some(l) = hit {
                    c.push(
                        l,
                        "U1",
                        format!(
                            "hook {path} (line {line}): param '{lp}' is {pname}, a SoftObject/SoftClass param; touching it (:get()) crashes UE4SS push_softobjectproperty - treat it as opaque"
                        ),
                    );
                }
            }
        }
    }
}

/// U2: reading a member whose name is a Soft{Object,Class}Property on every
/// class that declares it (`obj.Name`, `obj["Name"]`).
fn u2_soft_prop_reads(c: &mut Ctx, soft_only: &BTreeSet<String>, game_amb: &BTreeSet<String>) {
    let n = c.toks.len();
    let mut hits = vec![];
    for k in 1..n {
        let t = &c.toks[k];
        // obj:GetPropertyValue("Name") / obj:SetPropertyValue("Name", v): the same Soft* push
        // by a string key
        if t.kind == Kind::Name && (t.text == "GetPropertyValue" || t.text == "SetPropertyValue") && c.is_member(k) {
            if let (Some(p), Some(s)) = (c.toks.get(k + 1), c.toks.get(k + 2)) {
                if p.op("(") && s.kind == Kind::Str && (soft_only.contains(&s.text) || game_amb.contains(&s.text)) {
                    hits.push((t.line, s.text.clone(), format!("{}(\"{}\")", t.text, s.text), soft_only.contains(&s.text)));
                }
            }
            continue;
        }
        let (name, next) = if t.kind == Kind::Name && c.toks[k - 1].op(".") {
            (&t.text, c.toks.get(k + 1))
        } else if t.kind == Kind::Str && c.toks[k - 1].op("[") && c.toks.get(k + 1).map(|x| x.op("]")).unwrap_or(false) {
            (&t.text, c.toks.get(k + 2))
        } else {
            continue;
        };
        let only = soft_only.contains(name.as_str());
        if !only && !game_amb.contains(name.as_str()) {
            continue;
        }
        // a write (`x.N = v`) or a call (`x.N(...)`) is not a property read
        if next.map(|x| x.op("=") || x.op("(")).unwrap_or(false) {
            continue;
        }
        hits.push((t.line, name.clone(), format!("reads '{name}'"), only));
    }
    for (l, name, what, only) in hits {
        if only {
            c.push(l, "U2", format!("{what}, a SoftObject/SoftClass property: crashes UE4SS push_softobjectproperty (skip Soft* types)"));
        } else if !c.raw.get(l.wrapping_sub(1)).map(|r| r.contains("soft: ok") || r.contains("soft:ok")).unwrap_or(false) {
            c.push(l, "U2", format!("{what}: '{name}' is a SoftObject/SoftClass property on a /Game/ Blueprint class (plain on others): on such an object this crashes UE4SS push_softobjectproperty; if the object can never be one, mark the line `-- soft: ok <class>`"));
        }
    }
}

// ---------------------------------------------------------------- U3

fn u3_banned_calls(c: &mut Ctx) {
    for name in ["SetLeaderPoseComponent", "SetMasterPoseComponent"] {
        for l in c.uses(name, true) {
            c.push(l, "U3", format!("{name}: crashed the game on a stand-in's SK_Skeleton (copy pose another way)"));
        }
    }
    for l in c.uses("K2_DestroyActor", true) {
        c.push(
            l,
            "U3",
            "K2_DestroyActor: a silent no-op on pooled Willie_BP_C (snaps it to the origin); if the target can never be a Willie, mark the line `-- unsafe: ok <why>`"
                .into(),
        );
    }
}

// ---------------------------------------------------------------- U4

/// Per-mod thread-shim facts, from the mod's main.lua.
#[derive(Default, Clone, Debug)]
pub struct Shim {
    /// Line of the ExecuteWithDelay+LoopAsync remap (max of the two), if both exist.
    pub delay_loop: Option<usize>,
    pub keybind: Option<usize>,
    /// Line of the `RegisterKeyBind` remap: sync binds also fire on the input thread.
    pub keybind_sync: Option<usize>,
    /// Lines of the `if LoopInGameThreadWithDelay and ... then ... end` block.
    pub block: Option<(usize, usize)>,
}

/// The game-thread shim of a mod's main.lua: an `if LoopInGameThreadWithDelay
/// and ExecuteInGameThreadWithDelay ... then` block that reassigns
/// `ExecuteWithDelay`/`LoopAsync` (and `RegisterKeyBindAsync`) to functions.
pub fn find_shim(src: &str) -> Shim {
    let toks = lex::lex(src);
    let mut s = Shim::default();
    for k in 0..toks.len() {
        if !toks[k].name("if") {
            continue;
        }
        let line = toks[k].line;
        let cond: Vec<&str> = toks[k..].iter().take_while(|t| !t.name("then")).map(|t| t.text.as_str()).collect();
        if !(cond.contains(&"LoopInGameThreadWithDelay") && cond.contains(&"ExecuteInGameThreadWithDelay")) {
            continue;
        }
        let e = lex::block_close(&toks, k);
        let assigned = |w: &str| {
            (k..e).find(|&j| toks[j].name(w) && toks.get(j + 1).map(|x| x.op("=")).unwrap_or(false) && toks.get(j + 2).map(|x| x.name("function")).unwrap_or(false)).map(|j| toks[j].line)
        };
        if let (Some(a), Some(b)) = (assigned("ExecuteWithDelay"), assigned("LoopAsync")) {
            s.delay_loop = Some(a.max(b));
        }
        s.keybind = assigned("RegisterKeyBindAsync");
        s.keybind_sync = assigned("RegisterKeyBind");
        s.block = Some((line, toks[e].line));
        break;
    }
    s
}

/// Lua modules shipped with UE4SS itself (not ours): loading them before the shim is fine.
const UE4SS_MODULES: &[&str] = &["UEHelpers"];

/// U4: every non-member REFERENCE to a worker-thread API counts, not only a call
/// or a `= X` alias: `pcall(LoopAsync, ...)`, `local f = ExecuteAsync; f(x)` and passing it on
/// are the same raw worker-thread function. In main.lua the shim must also come before the
/// first `require` / `dofile` / `loadfile` of one of our modules: a module loaded earlier runs
/// (and may capture `LoopAsync`) before the remap. Shared libraries (`shared/*.lua`) are
/// scanned too: they are always required after a mod's shim (enforced above), so only
/// `ExecuteAsync` (no game-thread variant at all) is flagged there.
fn u4_thread_shim(c: &mut Ctx, shim: &Shim, is_main: bool, is_shared: bool) {
    let t = &c.toks;
    let mut hits: Vec<(usize, String)> = vec![];
    if is_main {
        if let Some((a, _)) = shim.block {
            for k in 0..t.len() {
                if t[k].line >= a {
                    break;
                }
                if !(t[k].name("require") || t[k].name("dofile") || t[k].name("loadfile")) || c.is_member(k) {
                    continue;
                }
                // the module literal: require("X") / require "X" / pcall(require, "X")
                let lit = t[k + 1..t.len().min(k + 4)].iter().find(|x| x.kind == Kind::Str).map(|x| x.text.clone());
                if lit.as_deref().map(|m| UE4SS_MODULES.contains(&m)).unwrap_or(false) {
                    continue;
                }
                hits.push((
                    t[k].line,
                    format!(
                        "{} {} before the game-thread shim (line {a}): the loaded module runs (and can capture the raw worker-thread LoopAsync / ExecuteWithDelay) before the remap - install the shim first",
                        t[k].text,
                        lit.map(|m| format!("{m:?}")).unwrap_or_else(|| "<dynamic>".into())
                    ),
                ));
            }
        }
    }
    for k in 0..t.len() {
        if t[k].kind != Kind::Name || c.is_member(k) {
            continue;
        }
        let w = t[k].text.as_str();
        let line = t[k].line;
        // inside the shim itself (`local _rkba = RegisterKeyBindAsync`, `return _rkba(...)`)
        let in_shim = shim.block.map(|(a, b)| is_main && line >= a && line <= b).unwrap_or(false);
        let target = match w {
            "ExecuteWithDelay" | "LoopAsync" => shim.delay_loop,
            "RegisterKeyBindAsync" => shim.keybind,
            "RegisterKeyBind" => shim.keybind_sync,
            // The shim's key wrappers run on the UE4SS input thread: any UE4SS
            // call there (ExecuteInGameThread pushes onto the hook state) races
            // the game thread's native hooks. They may only queue.
            "ExecuteInGameThread" if in_shim => {
                hits.push((line, "ExecuteInGameThread inside the game-thread shim: key callbacks run on the input thread and this call pushes onto the mod's hook state mid-hook (push_structproperty crash) - queue the callback and drain it from a LoopInGameThreadWithDelay loop".into()));
                continue;
            }
            "ExecuteAsync" => {
                if !in_shim {
                    hits.push((line, "ExecuteAsync runs Lua on a worker thread (no game-thread variant): use ExecuteInGameThread (a bare reference - pcall(ExecuteAsync, f), local f = ExecuteAsync - is the same call)".into()));
                }
                continue;
            }
            _ => continue,
        };
        if in_shim || is_shared {
            continue;
        }
        let bad = match target {
            None => Some(format!("{w} runs on a UE4SS worker thread and this mod has no game-thread shim (copy the shim from HSMPNoCutscene/main.lua)")),
            Some(s) if is_main && line < s => Some(format!("{w} used before the game-thread shim (line {s}) is installed")),
            _ => None,
        };
        if let Some(m) = bad {
            hits.push((line, m));
        }
    }
    for (l, m) in hits {
        c.push(l, "U4", m);
    }
}

// ---------------------------------------------------------------- U5

/// Module-level locals assigned a UObject somewhere in the file.
fn uobject_caches(c: &Ctx) -> BTreeMap<String, usize> {
    let depths = lex::depths(&c.toks);
    let t = &c.toks;
    let mut module_locals: BTreeSet<String> = BTreeSet::new();
    for k in 0..t.len() {
        if depths[k] == 0 && t[k].name("local") {
            let mut j = k + 1;
            if t.get(j).map(|x| x.name("function")).unwrap_or(false) {
                continue;
            }
            while let Some(x) = t.get(j) {
                if x.kind == Kind::Name {
                    module_locals.insert(x.text.clone());
                    j += 1;
                    if t.get(j).map(|y| y.op(",")).unwrap_or(false) {
                        j += 1;
                        continue;
                    }
                }
                break;
            }
        }
    }
    let uobj = Regex::new(
        r"\b(FindAllOf|FindFirstOf|FindObject|StaticFindObject|StaticConstructObject|construct|SpawnActor\w*|BeginDeferredActorSpawnFromClass|GetPlayerController|GetWorld|K2_GetPawn|GetOwner|GetComponentByClass|GetAllActorsOfClass)\s*\(|:get\(\)|\.Pawn\b|\.SK_Skeleton\b|\.Mesh\b|\.RootComponent\b",
    )
    .unwrap();
    let exempt = Regex::new(r#"StaticFindObject\(\s*"/Script/|Default__|GetGameInstance|GetGameplayStatics|GetKismet|GetFullName|ToString|tostring|tonumber|GetAddress|IsValid|==|~=|#"#).unwrap();
    let assign = Regex::new(r"^\s*([A-Za-z_]\w*)(?:\.[A-Za-z_]\w*|\[[^\]]*\])*\s*=\s*([^=].*)$").unwrap();
    let insert = Regex::new(r"table\.insert\(\s*([A-Za-z_]\w*)\s*,(.*)$").unwrap();
    let code = hsmp_tools::lualex::code_lines(&c.f.src);
    let mut caches = BTreeMap::new();
    for (i, l) in code.iter().enumerate() {
        if l.trim_start().starts_with("local ") {
            continue;
        }
        let raw = c.raw.get(i).copied().unwrap_or("");
        for cap in assign.captures_iter(l).chain(insert.captures_iter(l)) {
            let name = cap[1].to_string();
            if module_locals.contains(&name) && uobj.is_match(&cap[2]) && !exempt.is_match(raw) && !raw.contains("wg: ok") {
                caches.entry(name).or_insert(i + 1);
            }
        }
    }
    caches
}

/// Module-level locals of the file.
fn module_locals(c: &Ctx) -> BTreeSet<String> {
    let depths = lex::depths(&c.toks);
    let t = &c.toks;
    let mut out = BTreeSet::new();
    for k in 0..t.len() {
        if depths[k] == 0 && t[k].name("local") && !t.get(k + 1).map(|x| x.name("function")).unwrap_or(false) {
            let mut j = k + 1;
            while let Some(x) = t.get(j).filter(|x| x.kind == Kind::Name) {
                out.insert(x.text.clone());
                j += 1;
                if !t.get(j).map(|y| y.op(",")).unwrap_or(false) {
                    break;
                }
                j += 1;
            }
        }
    }
    out
}

/// Is the value at token `k` (right of `=`) stored beyond the callback: a
/// field write (`x.f = v`, `x[k] = v`), a module-level variable, or a table
/// constructor that is itself stored that way / pushed with table.insert?
fn escapes(c: &Ctx, k: usize, mods: &BTreeSet<String>) -> bool {
    let t = &c.toks;
    let lhs_escapes = |eq: usize| -> bool {
        // eq = index of `=`; LHS ends at eq-1
        if eq == 0 {
            return false;
        }
        let l = eq - 1;
        if t[l].op("]") {
            return true; // x[k] = v
        }
        if t[l].kind != Kind::Name {
            return false;
        }
        if l > 0 && t[l - 1].op(".") {
            return true; // x.f = v
        }
        if l > 0 && t[l - 1].name("local") {
            return false;
        }
        mods.contains(&t[l].text) && !(l > 0 && (t[l - 1].op("{") || t[l - 1].op(",")))
    };
    if k == 0 || !t[k - 1].op("=") {
        return false;
    }
    let eq = k - 1;
    // `key = v` inside a table constructor?
    let mut d = 0i32;
    let mut j = eq;
    while j > 0 {
        j -= 1;
        let x = &t[j];
        if x.op(")") || x.op("}") || x.op("]") {
            d += 1;
        } else if x.op("(") || x.op("[") {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if x.op("{") {
            if d == 0 {
                // constructor opened at j: where does it go?
                if j == 0 {
                    return false;
                }
                let p = &t[j - 1];
                if p.op("=") {
                    return lhs_escapes(j - 1);
                }
                if p.op(",") {
                    // table.insert(X, { ... })
                    let mut q = j - 1;
                    while q > 0 && !t[q].op("(") {
                        q -= 1;
                    }
                    return q >= 3 && t[q - 1].name("insert") && t[q - 2].op(".") && t[q - 3].name("table");
                }
                return false;
            }
            d -= 1;
        } else if d == 0 && (x.kind == Kind::Name && matches!(x.text.as_str(), "local" | "then" | "do" | "else" | "end" | "return")) {
            break;
        }
    }
    lhs_escapes(eq)
}

/// U5 (escape): a hook param is a RemoteUnrealParam that is valid only during
/// the call; storing the raw param (not `p:get()`) beyond it is a dangling ref.
fn u5_param_escape(c: &mut Ctx) {
    let mods = module_locals(c);
    let mut hits = vec![];
    for (hook, path, line, cbs) in hook_calls(&c.toks) {
        for (params, b, e) in &cbs {
            for k in *b..*e {
                let t = &c.toks[k];
                if t.kind != Kind::Name || !params.contains(&t.text) || c.is_member(k) {
                    continue;
                }
                let nx = c.toks.get(k + 1);
                if nx.map(|x| x.op(":") || x.op(".") || x.op("[") || x.op("(")).unwrap_or(false) {
                    continue;
                }
                if escapes(c, k, &mods) {
                    hits.push((
                        t.line,
                        format!(
                            "{hook}({}) callback (line {line}) stores hook param '{}' beyond the call (a RemoteUnrealParam is only valid during the hook): store p:get() values, never the param",
                            path.clone().unwrap_or_else(|| "<dynamic>".into()),
                            t.text
                        ),
                    ));
                }
            }
        }
    }
    for (l, m) in hits {
        c.push(l, "U5", m);
    }
}

/// World-guard objects of the file: `WG` plus every name bound to `<hsmp_wg module>.new(...)`
/// (`local G = HW.new{...}`, `local G = require("hsmp_wg").new{...}`).
fn guard_objects(t: &[Tok]) -> BTreeSet<String> {
    let mut modules: BTreeSet<String> = BTreeSet::new();
    for k in 0..t.len() {
        // local HW = require("hsmp_wg") / require "hsmp_wg"
        if t[k].kind == Kind::Name && t.get(k + 1).map(|x| x.op("=")).unwrap_or(false) && t.get(k + 2).map(|x| x.name("require")).unwrap_or(false) {
            let lit = t[k + 3..t.len().min(k + 5)].iter().find(|x| x.kind == Kind::Str);
            if lit.map(|x| x.text == "hsmp_wg").unwrap_or(false) && !t.get(k + 5).map(|x| x.op(".")).unwrap_or(false) && !t.get(k + 4).map(|x| x.op(".")).unwrap_or(false) {
                modules.insert(t[k].text.clone());
            }
        }
    }
    let mut objs: BTreeSet<String> = ["WG".to_string()].into_iter().collect();
    for k in 0..t.len() {
        if !(t[k].kind == Kind::Name && t.get(k + 1).map(|x| x.op("=")).unwrap_or(false)) {
            continue;
        }
        // X = M.new(  |  X = require("hsmp_wg").new(
        let rhs: Vec<&Tok> = t[k + 2..t.len().min(k + 9)].iter().collect();
        let direct = rhs.len() >= 3 && rhs[0].kind == Kind::Name && modules.contains(&rhs[0].text) && rhs[1].op(".") && rhs[2].name("new");
        let req = rhs.len() >= 6 && rhs[0].name("require") && rhs.iter().any(|x| x.kind == Kind::Str && x.text == "hsmp_wg")
            && rhs.windows(2).any(|w| w[0].op(".") && w[1].name("new"));
        if direct || req {
            objs.insert(t[k].text.clone());
        }
    }
    objs
}

/// A guard term at `k`: `G.check(`, `G.same(`, `G.pending(` (or `:`) on a guard object;
/// returns (end index after the method name, is_pending).
fn guard_term(t: &[Tok], k: usize, objs: &BTreeSet<String>) -> Option<(usize, bool)> {
    let o = t.get(k)?;
    if o.kind != Kind::Name || !objs.contains(&o.text) || (k > 0 && (t[k - 1].op(".") || t[k - 1].op(":"))) {
        return None;
    }
    let dot = t.get(k + 1)?;
    let m = t.get(k + 2)?;
    if !(dot.op(".") || dot.op(":")) || !t.get(k + 3).map(|x| x.op("(")).unwrap_or(false) {
        return None;
    }
    match m.text.as_str() {
        "check" | "same" => Some((k + 3, false)),
        "pending" => Some((k + 3, true)),
        _ => None,
    }
}

/// Named functions that ARE a guard: their body is `return <guard term>` (e.g.
/// `local function world_ok() return WG.check() end`).
fn guard_fns(t: &[Tok], objs: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (name, f) in named_fns(t) {
        let Some((_, b, e)) = function_at(t, f) else { continue };
        let mut k = b;
        while k < e {
            if t[k].name("return") {
                let neg = t.get(k + 1).map(|x| x.name("not")).unwrap_or(false);
                let at = if neg { k + 2 } else { k + 1 };
                if let Some((_, pend)) = guard_term(t, at, objs) {
                    // a function returning "the world is still good"
                    if neg == pend {
                        out.insert(name.clone());
                    }
                }
            }
            k += 1;
        }
    }
    out
}

/// Token spans inside the function body [rb, re) that a world-guard check dominates:
/// after `if not G.check() then return end` / `if not G.same(tok) then return end` /
/// `if G.pending() then return end` at the body's top level, and inside the then-block of
/// `if G.check() then ... end`. Any other mention of the guard (`HW.log(..)`, a call to some
/// `check()`) dominates nothing.
fn guarded_spans(t: &[Tok], depths: &[i32], rb: usize, re: usize, objs: &BTreeSet<String>, gfns: &BTreeSet<String>) -> Vec<(usize, usize)> {
    let top = depths.get(rb).copied().unwrap_or(0);
    let mut spans = vec![];
    let mut k = rb;
    while k < re {
        if !t[k].name("if") {
            k += 1;
            continue;
        }
        let close = lex::block_close(t, k);
        let Some(th) = (k + 1..close).find(|&j| t[j].name("then")) else {
            k += 1;
            continue;
        };
        // the guard term in the condition: (negated?, pending?)
        let mut term: Option<(bool, bool, usize)> = None;
        let mut has_and = false;
        let mut has_or = false;
        let mut j = k + 1;
        while j < th {
            if t[j].name("and") {
                has_and = true;
            } else if t[j].name("or") {
                has_or = true;
            }
            let neg = j > k + 1 && t[j - 1].name("not");
            if let Some((_, pend)) = guard_term(t, j, objs) {
                term.get_or_insert((neg, pend, j + 3));
            } else if t[j].kind == Kind::Name && gfns.contains(&t[j].text) && t.get(j + 1).map(|x| x.op("(")).unwrap_or(false) && !(j > 0 && (t[j - 1].op(".") || t[j - 1].op(":"))) {
                term.get_or_insert((neg, false, j + 1));
            }
            j += 1;
        }
        let Some((neg, pend, term_end)) = term else {
            k += 1;
            continue;
        };
        // "the world went stale" when the condition holds: not check()/same(), or pending()
        let stale_when_true = neg != pend;
        // else / elseif of this if (top level of its own block)
        let else_at = (th + 1..close).find(|&q| depths[q] == depths[k] + 1 && (t[q].name("else") || t[q].name("elseif")));
        if stale_when_true && !has_and && depths[k] == top && else_at.is_none() {
            // early return: `if not G.check() [or ...] then ... return ... end`
            let returns = (th + 1..close).any(|q| t[q].name("return") && depths[q] == depths[k] + 1);
            if returns {
                spans.push((close + 1, re));
                // the rest of the condition runs only when the guard passed (`not G.check() or not hud`)
                spans.push((term_end, th));
            }
        } else if !stale_when_true && !has_or {
            // `if G.check() [and ...] then <guarded> end`
            spans.push((th + 1, else_at.unwrap_or(close)));
            spans.push((term_end, th));
        }
        k += 1;
    }
    spans
}

fn u5_hook_captures(c: &mut Ctx) {
    u5_param_escape(c);
    let caches = uobject_caches(c);
    if caches.is_empty() {
        return;
    }
    let objs = guard_objects(&c.toks);
    let gfns = guard_fns(&c.toks, &objs);
    let depths = lex::depths(&c.toks);
    let fns = named_fns(&c.toks);
    let mut hits = vec![];
    for (hook, path, line, cbs) in hook_calls(&c.toks) {
        for (params, b, e) in &cbs {
            // the callback body plus one level of named functions it calls
            let mut ranges = vec![(*b, *e)];
            for k in *b..*e {
                let t = &c.toks[k];
                if t.kind == Kind::Name && !c.is_member(k) {
                    if let Some((_, fb, fe)) = fns.get(&t.text).and_then(|&f| function_at(&c.toks, f)) {
                        if !ranges.contains(&(fb, fe)) && !(fb <= *b && fe >= *e) {
                            ranges.push((fb, fe));
                        }
                    }
                }
            }
            // guard dominance per range; a named function's uses are also dominated when every
            // call of it from the callback body sits in a guarded span of the body
            let spans: Vec<Vec<(usize, usize)>> = ranges.iter().map(|&(rb, re)| guarded_spans(&c.toks, &depths, rb, re, &objs, &gfns)).collect();
            let in_spans = |sp: &[(usize, usize)], q: usize| sp.iter().any(|&(a, z)| a <= q && q < z);
            let dominated = |ri: usize, q: usize| -> bool {
                if in_spans(&spans[ri], q) {
                    return true;
                }
                if ri == 0 {
                    return false;
                }
                let (fb, _) = ranges[ri];
                let fname = fns.iter().find(|(_, &f)| function_at(&c.toks, f).map(|x| x.1 == fb).unwrap_or(false)).map(|(n, _)| n.clone());
                let Some(fname) = fname else { return false };
                let (bb, be) = ranges[0];
                let calls: Vec<usize> = (bb..be).filter(|&k| c.toks[k].name(&fname) && !c.is_member(k)).collect();
                !calls.is_empty() && calls.iter().all(|&k| in_spans(&spans[0], k))
            };
            // names shadowed by a local / loop variable inside the ranges
            let mut shadow: BTreeSet<String> = params.iter().cloned().collect();
            for &(rb, re) in &ranges {
                for abs in rb..re {
                    if c.toks[abs].name("local") || c.toks[abs].name("for") {
                        let mut j = abs + 1;
                        while let Some(x) = c.toks.get(j) {
                            if x.kind == Kind::Name && x.text != "function" {
                                shadow.insert(x.text.clone());
                                j += 1;
                                if c.toks.get(j).map(|y| y.op(",")).unwrap_or(false) {
                                    j += 1;
                                    continue;
                                }
                            }
                            break;
                        }
                    }
                }
            }
            let mut used: Vec<(usize, String)> = vec![];
            for (ri, &(rb, re)) in ranges.iter().enumerate() {
                for abs in rb..re {
                    let t = &c.toks[abs];
                    if t.kind == Kind::Name && caches.contains_key(&t.text) && !shadow.contains(&t.text) && !c.is_member(abs) && !used.iter().any(|(_, n)| n == &t.text) && !dominated(ri, abs) {
                        used.push((t.line, t.text.clone()));
                    }
                }
            }
            for (l, n) in used {
                hits.push((
                    l,
                    format!(
                        "{hook}({}) callback (line {line}) uses module-level UObject cache '{n}' (assigned line {}) without a world-guard check (WG.check()/WG.same(token)/WG.pending())",
                        path.clone().unwrap_or_else(|| "<dynamic>".into()),
                        caches[&n]
                    ),
                ));
            }
        }
    }
    for (l, m) in hits {
        c.push(l, "U5", m);
    }
}

// ---------------------------------------------------------------- U6

fn u6_console(c: &mut Ctx) {
    if CONSOLE_ALLOWED.contains(&c.f.rel.as_str()) {
        return;
    }
    for name in ["ProcessConsoleExec", "ConsoleCommand", "ExecuteConsoleCommand"] {
        for l in c.uses(name, true) {
            c.push(l, "U6", format!("{name} outside the Director (HSMPMatch/Scripts/director.lua): console commands can travel/quit/alter the world behind its back"));
        }
    }
}

// ---------------------------------------------------------------- driver

pub fn scan(f: &SrcFile, sigs: Option<&Sigs>, soft_only: &BTreeSet<String>, shim: &Shim) -> Vec<Finding> {
    let mut c = Ctx::new(f);
    if let Some(s) = sigs {
        u1_soft_hook_params(&mut c, s);
    }
    let game_amb = sigs.map(|s| s.game_ambiguous()).unwrap_or_default();
    u2_soft_prop_reads(&mut c, soft_only, &game_amb);
    u3_banned_calls(&mut c);
    let is_main = f.rel.ends_with("/main.lua");
    // shared libraries run inside every mod (each mod installs the shim first, enforced in
    // main.lua): only ExecuteAsync is flagged there
    u4_thread_shim(&mut c, shim, is_main && f.module != "shared", f.module == "shared");
    u5_hook_captures(&mut c);
    u6_console(&mut c);
    let mut v = c.out;
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sig::{Sigs, TEST_DUMP};

    fn run(rel: &str, src: &str) -> Vec<Finding> {
        let sigs = Sigs::parse(TEST_DUMP);
        let f = SrcFile { module: "M".into(), rel: rel.into(), src: src.into() };
        scan(&f, Some(&sigs), &sigs.soft_only(), &find_shim(src))
    }
    fn rules(v: &[Finding]) -> Vec<(&'static str, usize)> {
        v.iter().map(|f| (f.rule, f.line)).collect()
    }

    const SHIM: &str = "if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then\n    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end\n    LoopAsync = function(ms, fn) return LoopInGameThreadWithDelay(ms, fn) end\nend\n";

    #[test]
    fn u1_soft_param_get_is_flagged_and_opaque_use_is_not() {
        let src = format!(
            "{SHIM}RegisterHook(\"/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr\", function(ctx, wc, level, abs)\n    local p = level:get()\nend)\nRegisterHook(\"/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr\", function(ctx, wc, level)\n    note(\"travel\")\nend)\nRegisterHook(\"/Script/Engine.GameplayStatics:OpenLevel\", function(ctx, wc, name)\n    local n = name:get():ToString()\nend)\n"
        );
        assert_eq!(rules(&run("m/main.lua", &src)), vec![("U1", 6)]);
    }

    #[test]
    fn u2_soft_member_reads() {
        let src = "local a = w.TargetLevel\nw.TargetLevel = nil\nlocal b = w[\"SoftList\"]\nlocal c = actor.Level -- soft: ok AActor (ObjectProperty)\n";
        assert_eq!(rules(&run("m/x.lua", src)), vec![("U2", 1), ("U2", 3)]);
    }

    #[test]
    fn u2_game_ambiguous_and_string_keyed_reads() {
        // Level is soft on a /Game/ class (plain on Actor): flagged unless marked
        let src = "local c = entry.Level\nlocal d = entry[\"Level\"]\nlocal w = o:GetPropertyValue(\"CookedWorldAsset\")\no:SetPropertyValue(\"TargetLevel\", x)\nlocal e = o:GetPropertyValue(\"Level\")\n";
        assert_eq!(rules(&run("m/x.lua", src)), vec![("U2", 1), ("U2", 2), ("U2", 3), ("U2", 4), ("U2", 5)]);
        let clean = "local c = actor.Level -- soft: ok AActor\nlocal h = o:GetPropertyValue(\"Health\")\nlocal n = o.NativeOnlySoft\nlocal s = \"Level\"\n";
        assert!(run("m/x.lua", clean).is_empty(), "{:?}", run("m/x.lua", clean));
    }

    #[test]
    fn u3_u6_banned_calls_and_suppression() {
        let src = "m:SetLeaderPoseComponent(x)\nw:K2_DestroyActor()\nweapon:K2_DestroyActor() -- unsafe: ok weapons are not pooled\npc:ConsoleCommand(FString(\"quit\"), false)\n";
        assert_eq!(rules(&run("m/x.lua", src)), vec![("U3", 1), ("U3", 2), ("U6", 4)]);
        assert!(run("mods/HSMPMatch/Scripts/director.lua", "pc:ConsoleCommand(x)\n").is_empty());
    }

    #[test]
    fn u4_needs_shim_before_use() {
        let bare = "LoopAsync(100, function() end)\nRegisterKeyBindAsync(Key.F1, {}, f)\nExecuteAsync(function() end)\n";
        assert_eq!(rules(&run("m/main.lua", bare)), vec![("U4", 1), ("U4", 2), ("U4", 3)]);
        let early = format!("ExecuteWithDelay(1, f)\n{SHIM}LoopAsync(100, f)\n");
        assert_eq!(rules(&run("m/main.lua", &early)), vec![("U4", 1)]);
    }

    #[test]
    fn u4_keybinds_queue_off_the_input_thread() {
        // a sync bind fires on the input thread too: it needs the shim's remap
        let bare = format!("{SHIM}RegisterKeyBind(Key.Q, f)\n");
        assert_eq!(rules(&run("m/main.lua", &bare)), vec![("U4", 5)]);
        // the old wrapper called ExecuteInGameThread from the input thread
        let old = "if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then\n    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end\n    LoopAsync = function(ms, fn) return LoopInGameThreadWithDelay(ms, fn) end\n    local _gt = ExecuteInGameThread\n    local _rkb = RegisterKeyBind\n    RegisterKeyBind = function(key, a) return _rkb(key, function() _gt(a) end) end\nend\nRegisterKeyBind(Key.Q, f)\n";
        assert_eq!(rules(&run("m/main.lua", old)), vec![("U4", 4)]);
        // the queueing wrapper is clean, and so are binds after it
        let queued = "if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then\n    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end\n    LoopAsync = function(ms, fn) return LoopInGameThreadWithDelay(ms, fn) end\n    local q = {}\n    local _rkb = RegisterKeyBind\n    RegisterKeyBind = function(key, a) return _rkb(key, function() q[#q + 1] = a end) end\nend\nRegisterKeyBind(Key.Q, f)\nExecuteInGameThread(f)\n";
        assert!(run("m/main.lua", queued).is_empty(), "{:?}", run("m/main.lua", queued));
    }

    #[test]
    fn u4_references_and_require_order() {
        // bare references count, not only calls
        let refs = "local EA = ExecuteAsync; EA(f)\npcall(LoopAsync, 100, f)\nlocal d = ExecuteWithDelay\n";
        assert_eq!(rules(&run("m/main.lua", refs)), vec![("U4", 1), ("U4", 2), ("U4", 3)]);
        // a module required before the shim (kit.lua captured raw LoopAsync); UEHelpers is UE4SS's own
        let order = format!("local UEHelpers = require(\"UEHelpers\")\nlocal ok, kit = pcall(require, \"kit\")\nlocal k2 = dofile(dir .. \"/kit.lua\")\n{SHIM}local x = require(\"hsmp_log\")\n");
        assert_eq!(rules(&run("m/main.lua", &order)), vec![("U4", 2), ("U4", 3)]);
        // after the shim: references are fine
        let ok = format!("local UEHelpers = require(\"UEHelpers\")\n{SHIM}local x = require(\"kit\")\npcall(LoopAsync, 100, f)\nlocal d = ExecuteWithDelay\n");
        assert!(run("m/main.lua", &ok).is_empty(), "{:?}", run("m/main.lua", &ok));
    }

    #[test]
    fn u4_shared_is_scanned_for_execute_async() {
        let sigs = Sigs::parse(TEST_DUMP);
        let shared = |src: &str| {
            let f = SrcFile { module: "shared".into(), rel: "mods/shared/x.lua".into(), src: src.into() };
            scan(&f, Some(&sigs), &sigs.soft_only(), &Shim::default())
        };
        assert_eq!(rules(&shared("local EA = ExecuteAsync\n")), vec![("U4", 1)]);
        // shared code runs after every mod's shim (enforced in main.lua): LoopAsync is the remapped one
        assert!(shared("LoopAsync(100, f)\nlocal t = obj.ExecuteAsync\n").is_empty());
    }

    #[test]
    fn u5_hook_param_escape() {
        let src = "local hits = {}\nlocal last = nil\nlocal function on_pre(selfp, Raw, Out)\n    local P = { Raw = Raw }\n    pset(P.Raw, 0)\n    table.insert(hits, { raw = num(pv(Raw)), outref = Out })\n    last = selfp\n    local w = selfp\n    hits.x = Raw:get()\nend\nRegisterHook(\"/Game/W.W_C:Get Damage\", on_pre)\n";
        // line 6 twice: the escape (outref = Out) and the use of `hits`, a
        // cache of a :get() value (line 9), without a world check
        let r = run("m/x.lua", src);
        assert_eq!(rules(&r), vec![("U5", 6), ("U5", 6), ("U5", 7)]);
        assert!(r[0].msg.contains("'Out'") || r[1].msg.contains("'Out'"));
    }

    #[test]
    fn u5_hook_uses_cache_without_guard() {
        let src = format!(
            "{SHIM}local WG = make()\nlocal hud = nil\nlocal function f()\n    hud = FindFirstOf(\"X\")\nend\nRegisterHook(\"/Script/A.B:C\", function(self)\n    hud:Refresh()\nend)\nRegisterHook(\"/Script/A.B:D\", function(self)\n    if not WG.check() then return end\n    hud:Refresh()\nend)\n"
        );
        assert_eq!(rules(&run("m/main.lua", &src)), vec![("U5", 11)]);
    }

    /// Only an early-return / enclosing WG.check() / WG.same() / WG.pending() that
    /// dominates the cache use counts as a guard.
    fn u5_case(cb_body: &str) -> Vec<(&'static str, usize)> {
        let src = format!(
            "{SHIM}local HW = require(\"hsmp_wg\")\nlocal G = HW.new{{}}\nlocal WG = G\nlocal hud = nil\nlocal function f()\n    hud = FindFirstOf(\"X\")\nend\nlocal function world_ok() return G.check() end\nlocal function refresh() hud:Refresh() end\nRegisterHook(\"/Script/A.B:C\", function(self)\n{cb_body}\nend)\n"
        );
        // the callback body starts on line 15 (a use inside refresh() reports line 13)
        rules(&run("m/main.lua", &src))
    }

    #[test]
    fn u5_guard_must_dominate() {
        let bad = [
            "    HW.log(\"x\")\n    hud:Refresh()",
            "    hud:Refresh()\n    if not WG.check() then return end",
            "    if x then\n        if not WG.check() then return end\n    end\n    hud:Refresh()",
            "    if not WG.check() then log() end\n    hud:Refresh()",
            "    if x and not WG.check() then return end\n    hud:Refresh()",
            "    if WG.check() or x then hud:Refresh() end",
            "    if check() then hud:Refresh() end",
            "    local tok = WG.token()\n    hud:Refresh()",
            "    refresh()\n    if not WG.check() then return end",
        ];
        for b in bad {
            let r = u5_case(b);
            assert!(r.iter().any(|(rule, _)| *rule == "U5"), "not flagged:\n{b}\n{r:?}");
        }
        let good = [
            "    if not WG.check() then return end\n    hud:Refresh()",
            "    if not G.same(tok) then return end\n    hud:Refresh()",
            "    if not WG.check() or not hud then return end\n    hud:Refresh()",
            "    if WG.pending() then return end\n    hud:Refresh()",
            "    if WG.check() then hud:Refresh() end",
            "    if WG:check() and hud then\n        hud:Refresh()\n    end",
            "    if not world_ok() then return end\n    hud:Refresh()",
            "    if not WG.check() then return end\n    refresh()",
        ];
        for g in good {
            let r = u5_case(g);
            assert!(r.is_empty(), "false positive:\n{g}\n{r:?}");
        }
    }
}

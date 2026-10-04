//! mlua (Lua 5.4, vendored) test harness for HSMP's Lua mods.
//!
//! Suites live in `tools/hsmp-tools/lua-tests/<suite>.lua` and are run with
//! `hsmp-tools lua-test [suite...]`. Each suite is plain Lua executed in a
//! fresh Lua 5.4 state (all standard libraries incl. `debug`, like UE4SS)
//! with a global `T` provided by Rust:
//!
//! | `T.` member | meaning |
//! |---|---|
//! | `check(cond, msg [, detail])` | one counted assertion; `cond` uses *Python* truthiness (nil/false/0/"" fail) |
//! | `root`, `path(rel)`, `tests_dir`, `script` | repo root / repo-relative path / suite dir / this file (all `/`-separated) |
//! | `read(p)` / `write(p, s)` / `append(p, s)` / `remove(p)` / `exists(p)` / `mkdir(p)` | files (read -> nil if missing; mkdir creates parents) |
//! | `tmpdir(prefix)` | fresh temp dir, deleted when the suite ends |
//! | `glob(base, pattern)` | sorted paths (`*`, `?`, `**`) |
//! | `json_decode(s)` | JSON -> Lua (null -> nil) |
//! | `re_match(p, s)` / `re_search(p, s)` | Rust regex; anchored / unanchored; returns `{[0]=whole, g1, ...}` or nil |
//! | `re_findall(p, s)` | Python `re.findall` (whole / group 1 / table of groups) |
//! | `isolated(file, ...)` | run `file` (relative to the suite dir or absolute) in a NEW Lua state with its own `T`; args/returns are copied as JSON-able values; checks count into the same totals |
//! | `counts()` | `pass, fail` so far |
//! | `log(s)` | print a line on stdout (the mod under test may replace `print`) |
//!
//! Plus Lua helpers from the prelude (`T.sorted`, `T.eq`, `T.repr`, `T.count`,
//! `T.contains`, `T.startswith`, `T.lines`, `T.keys`, `T.map`, `T.filter`, `T.any`,
//! `T.all`, `T.set`, `T.truthy`). Reusable mocks live in `lua-tests/lib/` and are
//! `require`-able (e.g. `require("umg_mock")`).

use crate::paths;
use anyhow::{anyhow, Result};
use mlua::{Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Assertion totals.
#[derive(Debug, Default, Clone)]
pub struct Counts {
    pub pass: usize,
    pub fail: usize,
    pub failures: Vec<String>,
}

/// State shared by every Lua state of one suite run.
pub struct Shared {
    pub root: PathBuf,
    pub tests_dir: PathBuf,
    pub verbose: bool,
    pub counts: RefCell<Counts>,
    tmpdirs: RefCell<Vec<PathBuf>>,
    /// errors "attempt to call a nil value (global 'X')" that a pcall/xpcall swallowed
    pub nil_global_calls: RefCell<Vec<String>>,
}

impl Shared {
    pub fn new(root: &Path, verbose: bool) -> Rc<Shared> {
        Rc::new(Shared {
            root: root.to_path_buf(),
            tests_dir: tests_dir(root),
            verbose,
            counts: RefCell::new(Counts::default()),
            tmpdirs: RefCell::new(vec![]),
            nil_global_calls: RefCell::new(vec![]),
        })
    }

    /// Record one assertion (also usable from Rust-side tests).
    pub fn check(&self, ok: bool, msg: &str, detail: &str) {
        let mut c = self.counts.borrow_mut();
        if ok {
            c.pass += 1;
            if self.verbose {
                println!("  PASS  {msg}");
            }
        } else {
            c.fail += 1;
            c.failures.push(msg.to_string());
            if detail.is_empty() {
                println!("  FAIL  {msg}");
            } else {
                println!("  FAIL  {msg}   {detail}");
            }
        }
    }

    fn cleanup(&self) {
        for d in self.tmpdirs.borrow_mut().drain(..) {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Transitional: suites written for the old repo layout name files as
/// `mod/ue4ss_mods/<Mod>/...`; map them to `mods/<Mod>/...` (or `mods/dev/<Mod>/...`).
/// Remove once no suite uses the old prefix.
pub fn legacy_rel(rel: &str) -> String {
    const OLD: &str = "mod/ue4ss_mods";
    let Some(rest) = rel.strip_prefix(OLD) else { return rel.to_string() };
    let rest = rest.trim_start_matches('/');
    let name = rest.split('/').next().unwrap_or("");
    let dev = ["HSMPDiag", "HSMPDump"].contains(&name);
    match (rest.is_empty(), dev) {
        (true, _) => paths::MODS_DIR.to_string(),
        (false, true) => format!("{}/{}/{rest}", paths::MODS_DIR, paths::DEV_MODS_DIR),
        (false, false) => format!("{}/{rest}", paths::MODS_DIR),
    }
}

/// `<root>/tools/hsmp-tools/lua-tests`
pub fn tests_dir(root: &Path) -> PathBuf {
    root.join("tools").join("hsmp-tools").join("lua-tests")
}

/// Suite names (`lua-tests/*.lua`, without extension), sorted.
pub fn suites(root: &Path) -> Vec<String> {
    paths::glob(&tests_dir(root), "*.lua")
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .filter(|s| !s.starts_with('_'))
        .collect()
}

/// A Lua 5.4 state with every standard library (incl. `debug`), as UE4SS has.
pub fn new_lua() -> Lua {
    // SAFETY: we deliberately load `debug` (the mods use debug.getinfo); no C modules are loaded.
    unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) }
}

/// Python truthiness of a Lua value: nil, false, 0, 0.0 and "" are false.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(0) => false,
        Value::Number(x) => *x != 0.0,
        Value::String(s) => !s.as_bytes().is_empty(),
        _ => true,
    }
}

const PRELUDE: &str = include_str!("luatest_prelude.lua");

/// UE4SS Lua API globals: a pcall-swallowed nil call of one of these is a mock gap (reported),
/// not a failure.
pub const UE4SS_GLOBALS: &[&str] = &[
    "RegisterHook", "UnregisterHook", "RegisterLoadMapPreHook", "RegisterLoadMapPostHook", "RegisterBeginPlayPreHook",
    "RegisterBeginPlayPostHook", "RegisterEndPlayPreHook", "RegisterEndPlayPostHook", "RegisterInitGameStatePreHook",
    "RegisterInitGameStatePostHook", "NotifyOnNewObject", "RegisterKeyBind", "RegisterKeyBindAsync", "IsKeyBindRegistered",
    "RegisterConsoleCommandHandler", "RegisterConsoleCommandGlobalHandler", "RegisterProcessConsoleExecPreHook",
    "RegisterProcessConsoleExecPostHook", "RegisterCallFunctionByNameWithArgumentsPreHook",
    "RegisterCallFunctionByNameWithArgumentsPostHook", "RegisterCustomEvent", "UnregisterCustomEvent", "LoadAsset",
    "FindFirstOf", "FindAllOf", "FindObject", "FindObjects", "StaticFindObject", "StaticConstructObject", "CreateInvalidObject",
    "ExecuteInGameThread", "ExecuteWithDelay", "ExecuteAsync", "LoopAsync", "LoopInGameThreadWithDelay",
    "ExecuteInGameThreadWithDelay", "CancelDelayedAction", "RetriggerDelayedAction", "IsValidDelayedActionHandle",
    "FName", "FText", "FString", "ForEachUObject", "IterateGameDirectories", "GetKismetSystemLibrary", "GetKismetMathLibrary",
];

fn ser_opts() -> mlua::SerializeOptions {
    mlua::SerializeOptions::new().serialize_none_to_null(false).serialize_unit_to_null(false)
}

fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    lua.to_value_with(v, ser_opts())
}

fn lua_to_json(lua: &Lua, v: Value) -> serde_json::Value {
    lua.from_value::<serde_json::Value>(v).unwrap_or(serde_json::Value::Null)
}

fn compile(pat: &str) -> mlua::Result<regex::Regex> {
    regex::Regex::new(pat).map_err(|e| mlua::Error::runtime(format!("bad regex {pat:?}: {e}")))
}

fn caps_to_lua(lua: &Lua, c: &regex::Captures) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    for (i, g) in c.iter().enumerate() {
        if let Some(m) = g {
            t.raw_set(i as i64, m.as_str())?;
        }
    }
    Ok(Value::Table(t))
}

fn resolve(sh: &Shared, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        sh.tests_dir.join(pb)
    }
}

/// Install the `T` global into `lua`.
pub fn install(lua: &Lua, sh: &Rc<Shared>, script: &Path) -> mlua::Result<()> {
    let t = lua.create_table()?;
    t.set("root", paths::fwd(&sh.root))?;
    t.set("tests_dir", paths::fwd(&sh.tests_dir))?;
    t.set("script", paths::fwd(script))?;

    let s = sh.clone();
    t.set(
        "check",
        lua.create_function(move |_, (cond, msg, detail): (Value, Option<String>, Value)| {
            let ok = truthy(&cond);
            let d = match &detail {
                Value::Nil => String::new(),
                v => v.to_string().unwrap_or_default(),
            };
            s.check(ok, msg.as_deref().unwrap_or("?"), &d);
            Ok(ok)
        })?,
    )?;
    let s = sh.clone();
    t.set("counts", lua.create_function(move |_, ()| {
        let c = s.counts.borrow();
        Ok((c.pass, c.fail))
    })?)?;
    let s = sh.clone();
    t.set("_nil_global_call", lua.create_function(move |_, msg: String| {
        let mut v = s.nil_global_calls.borrow_mut();
        if !v.contains(&msg) {
            v.push(msg);
        }
        Ok(())
    })?)?;
    let s = sh.clone();
    t.set("path",lua.create_function(move |_, rel: String| Ok(paths::fwd(&s.root.join(legacy_rel(&rel)))))?)?;
    t.set("log", lua.create_function(|_, s: String| {
        println!("{s}");
        Ok(())
    })?)?;
    t.set("read", lua.create_function(|lua, p: String| match std::fs::read(&p) {
        Ok(b) => Ok(Value::String(lua.create_string(&b)?)),
        Err(_) => Ok(Value::Nil),
    })?)?;
    t.set("write", lua.create_function(|_, (p, s): (String, mlua::String)| {
        std::fs::write(&p, s.as_bytes()).map_err(|e| mlua::Error::runtime(format!("write {p}: {e}")))
    })?)?;
    t.set("mkdir", lua.create_function(|_, p: String| {
        std::fs::create_dir_all(&p).map_err(|e| mlua::Error::runtime(format!("mkdir {p}: {e}")))
    })?)?;
    t.set("append", lua.create_function(|_, (p, s): (String, mlua::String)| {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .map_err(|e| mlua::Error::runtime(format!("append {p}: {e}")))?;
        f.write_all(&s.as_bytes()).map_err(|e| mlua::Error::runtime(format!("append {p}: {e}")))
    })?)?;
    t.set("remove", lua.create_function(|_, p: String| Ok(std::fs::remove_file(&p).is_ok()))?)?;
    t.set("exists", lua.create_function(|_, p: String| Ok(Path::new(&p).exists()))?)?;
    let s = sh.clone();
    t.set("tmpdir", lua.create_function(move |_, prefix: Option<String>| {
        let d = paths::make_temp_dir(prefix.as_deref().unwrap_or("hsmp_lua_"))
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        s.tmpdirs.borrow_mut().push(d.clone());
        Ok(paths::fwd(&d))
    })?)?;
    t.set("glob", lua.create_function(|lua, (base, pat): (String, String)| {
        let v: Vec<String> = paths::glob(Path::new(&base), &pat).iter().map(|p| paths::fwd(p)).collect();
        lua.create_sequence_from(v)
    })?)?;
    t.set("json_decode", lua.create_function(|lua, s: String| {
        let v: serde_json::Value =
            serde_json::from_str(&s).map_err(|e| mlua::Error::runtime(format!("json: {e}")))?;
        json_to_lua(lua, &v)
    })?)?;
    t.set("re_match", lua.create_function(|lua, (p, s): (String, String)| {
        let re = compile(&format!(r"\A(?:{p})"))?;
        match re.captures(&s) {
            Some(c) => caps_to_lua(lua, &c),
            None => Ok(Value::Nil),
        }
    })?)?;
    t.set("re_search", lua.create_function(|lua, (p, s): (String, String)| {
        let re = compile(&p)?;
        match re.captures(&s) {
            Some(c) => caps_to_lua(lua, &c),
            None => Ok(Value::Nil),
        }
    })?)?;
    t.set("re_findall", lua.create_function(|lua, (p, s): (String, String)| {
        let re = compile(&p)?;
        let out = lua.create_table()?;
        let ng = re.captures_len() - 1;
        for (i, c) in re.captures_iter(&s).enumerate() {
            let v = match ng {
                0 => Value::String(lua.create_string(c.get(0).map(|m| m.as_str()).unwrap_or(""))?),
                1 => Value::String(lua.create_string(c.get(1).map(|m| m.as_str()).unwrap_or(""))?),
                _ => {
                    let g = lua.create_table()?;
                    for k in 1..=ng {
                        g.raw_set(k as i64, c.get(k).map(|m| m.as_str()).unwrap_or(""))?;
                    }
                    Value::Table(g)
                }
            };
            out.raw_set((i + 1) as i64, v)?;
        }
        Ok(out)
    })?)?;
    let s = sh.clone();
    t.set("isolated", lua.create_function(move |lua, (file, args): (String, MultiValue)| {
        let a: Vec<serde_json::Value> = args.into_iter().map(|v| lua_to_json(lua, v)).collect();
        let rets = run_file(&s, &resolve(&s, &file), &a).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let mut mv = MultiValue::new();
        for r in rets {
            mv.push_back(json_to_lua(lua, &r)?);
        }
        Ok(mv)
    })?)?;

    lua.globals().set("T", t)?;
    let lib = paths::fwd(&sh.tests_dir.join("lib"));
    lua.load(format!("package.path = \"{lib}/?.lua;\" .. package.path")).exec()?;
    lua.load(PRELUDE).set_name("@luatest_prelude.lua").exec()?;
    Ok(())
}

/// Run one Lua file in a fresh state with `T` installed; returns its results as JSON values.
pub fn run_file(sh: &Rc<Shared>, file: &Path, args: &[serde_json::Value]) -> Result<Vec<serde_json::Value>> {
    let lua = new_lua();
    install(&lua, sh, file).map_err(|e| anyhow!("install T: {e}"))?;
    let src = std::fs::read(file).map_err(|e| anyhow!("read {}: {e}", file.display()))?;
    let f = lua
        .load(&src[..])
        .set_name(format!("@{}", paths::fwd(file)))
        .into_function()
        .map_err(|e| anyhow!("{e}"))?;
    let mut mv = MultiValue::new();
    for a in args {
        mv.push_back(json_to_lua(&lua, a).map_err(|e| anyhow!("{e}"))?);
    }
    let rets: MultiValue = f.call(mv).map_err(|e| anyhow!("{e}"))?;
    Ok(rets.into_iter().map(|v| lua_to_json(&lua, v)).collect())
}

/// Run suite `name` (`lua-tests/<name>.lua`) with string args; returns its counts.
/// A Lua error aborts the suite and counts as one failure.
pub fn run_suite(root: &Path, name: &str, args: &[String], verbose: bool) -> Counts {
    let sh = Shared::new(root, verbose);
    let file = sh.tests_dir.join(format!("{name}.lua"));
    let a: Vec<serde_json::Value> = args.iter().map(|s| serde_json::Value::String(s.clone())).collect();
    if let Err(e) = run_file(&sh, &file, &a) {
        sh.check(false, &format!("{name}: suite aborted"), &e.to_string());
    }
    // A call to an undefined global swallowed by pcall/xpcall (a mock gap or a typo
    // in the mod) hides a dead code path behind a green suite.
    // UE4SS's own API missing from a suite's mock (the mods pcall their registrations
    // defensively) is a mock gap: printed, not failed. Anything else (a helper of ours, a typo)
    // fails the suite.
    for m in sh.nil_global_calls.borrow().clone() {
        let g = m.split("(global '").nth(1).and_then(|r| r.split('\'').next()).unwrap_or("");
        if UE4SS_GLOBALS.contains(&g) {
            println!("  note  {name}: a pcall swallowed a call to UE4SS's {g} (not in this suite's mock): {m}");
        } else {
            sh.check(false, &format!("{name}: a pcall/xpcall swallowed a call to an undefined global"), &m);
        }
    }
    // ... and a suite that checked nothing proved nothing
    let ran = { let c = sh.counts.borrow(); c.pass + c.fail };
    if ran == 0 {
        sh.check(false, &format!("{name}: the suite ran no checks"), "");
    }
    sh.cleanup();
    let c = sh.counts.borrow().clone();
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_mod_paths_map_to_mods() {
        assert_eq!(legacy_rel("mod/ue4ss_mods/HSMPCombat/Scripts/main.lua"), "mods/HSMPCombat/Scripts/main.lua");
        assert_eq!(legacy_rel("mod/ue4ss_mods/shared"), "mods/shared");
        assert_eq!(legacy_rel("mod/ue4ss_mods"), "mods");
        assert_eq!(legacy_rel("mod/ue4ss_mods/HSMPDiag/Scripts/main.lua"), "mods/dev/HSMPDiag/Scripts/main.lua");
        assert_eq!(legacy_rel("mods/HSMPMenu/Scripts"), "mods/HSMPMenu/Scripts");
    }

    fn suite(src: &str) -> Counts {
        let root = paths::make_temp_dir("hsmp_luatest_").unwrap();
        std::fs::create_dir_all(tests_dir(&root)).unwrap();
        std::fs::write(tests_dir(&root).join("s.lua"), src).unwrap();
        let c = run_suite(&root, "s", &[], false);
        std::fs::remove_dir_all(&root).ok();
        c
    }

    /// A 0-check suite fails; a pcall-swallowed call to an undefined global of ours fails.
    #[test]
    fn zero_checks_and_swallowed_nil_globals_fail() {
        let c = suite("local x = 1\n");
        assert_eq!((c.pass, c.fail), (0, 1), "{:?}", c.failures);
        let c = suite("T.check(true, 'a')\nlocal ok = pcall(function() my_helper(1) end)\nT.check(not ok, 'b')\n");
        assert_eq!((c.pass, c.fail), (2, 1), "{:?}", c.failures);
        let c = suite("T.check(true, 'a')\nxpcall(function() typo_fn() end, function(e) return e end)\n");
        assert_eq!(c.fail, 1, "{:?}", c.failures);
    }

    #[test]
    fn checks_engine_mock_gaps_and_plain_errors_pass() {
        // UE4SS API missing from a mock: a printed note, not a failure
        let c = suite("pcall(function() RegisterHook('/Script/X:Y', function() end) end)\nT.check(true, 'a')\n");
        assert_eq!((c.pass, c.fail), (1, 0), "{:?}", c.failures);
        // an intentional error, a nil field call (not a global) and the pcall results themselves
        let c = suite("local ok, e = pcall(error, 'boom')\nT.check(not ok and e == 'boom', 'err')\nlocal t = {}\nlocal ok2 = pcall(function() t.f() end)\nT.check(not ok2, 'field')\nlocal ok3, a, b, c2 = pcall(function() return 1, nil, 3 end)\nT.check(ok3 and a == 1 and b == nil and c2 == 3, 'returns')\n");
        assert_eq!((c.pass, c.fail), (3, 0), "{:?}", c.failures);
    }
}

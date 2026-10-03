//! Runs tests/saveguard_spec.lua against the shipped
//! mods/shared/hsmp_saveguard.lua in Lua 5.4 (mlua, vendored).
//! Every scenario gets a fresh Lua state and its own temp state/SaveGames
//! dirs; the real %LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames is never used.

use mlua::{Function, Lua, Table};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SG_SRC: &str = include_str!("../../../mods/shared/hsmp_saveguard.lua");
const SPEC_SRC: &str = include_str!("saveguard_spec.lua");

static SEQ: AtomicU32 = AtomicU32::new(0);

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!(
            "hsmp_sg_test_{}_{}_{}",
            std::process::id(),
            nanos,
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(p.join("state")).unwrap();
        std::fs::create_dir_all(p.join("SaveGames")).unwrap();
        TempRoot(p)
    }
    fn sub(&self, s: &str) -> String {
        self.0.join(s).to_string_lossy().replace('\\', "/")
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn load_spec(lua: &Lua) -> Table {
    lua.load(SPEC_SRC).set_name("saveguard_spec.lua").eval::<Table>().expect("spec loads")
}

#[test]
fn saveguard_lua_syntax() {
    let lua = Lua::new();
    lua.load(SG_SRC).set_name("hsmp_saveguard.lua").into_function().expect("hsmp_saveguard.lua compiles");
    let m: Table = lua.load(SG_SRC).eval().expect("module returns a table");
    for f in ["install", "tick", "refresh", "is_active", "set_active", "mapped", "selftest"] {
        assert!(m.get::<Function>(f).is_ok(), "M.{f} missing");
    }
}

#[test]
fn saveguard_scenarios() {
    let count: i64 = {
        let lua = Lua::new();
        let spec = load_spec(&lua);
        spec.get::<Function>("count").unwrap().call(()).unwrap()
    };
    assert!(count >= 10, "spec has {count} scenarios");

    let mut failures = Vec::new();
    let mut checks = 0;
    for i in 1..=count {
        let lua = Lua::new();
        let spec = load_spec(&lua);
        let tmp = TempRoot::new();
        let run: Function = spec.get("run").unwrap();
        let (name, results): (String, Table) =
            run.call((i, SG_SRC, tmp.sub("state"), tmp.sub("SaveGames"))).expect("scenario runs");
        println!("[{name}]");
        for r in results.sequence_values::<Table>() {
            let r = r.unwrap();
            let check: String = r.get(1).unwrap();
            let ok: bool = r.get(2).unwrap();
            let detail: String = r.get(3).unwrap();
            checks += 1;
            if ok {
                println!("  PASS  {check}");
            } else {
                println!("  FAIL  {check}   {detail}");
                failures.push(format!("{name}: {check} ({detail})"));
            }
        }
    }
    println!("{checks} checks, {} failed", failures.len());
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

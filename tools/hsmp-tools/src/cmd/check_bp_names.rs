//! `hsmp-tools check-bp-names`: flag Lua references that use a squashed (CamelCase) name for
//! a Half Sword Blueprint function or property whose real name has spaces.
//!
//! Half Sword's BP UFunctions and many BP properties have spaces in their real names
//! ("Set Up Armor", "Wake Up", "Component 1"). From UE4SS Lua, `obj:SetUpArmor()` resolves to
//! nothing and fails silently; the call must be `obj["Set Up Armor"](obj, ...)`.
//!
//! Every form of the squashed name is flagged, compared case-insensitively
//! (`AddArmorToList` vs the real "Add Armor to List"):
//! * a call `w:SetUpArmor()` / `w.SetUpArmor(w)`,
//! * a bare member reference `pcall(w.SetUpArmor, w)`, `local f = w.SetUpArmor`,
//! * a string key `w["SetUpArmor"]` / `w['SetUpArmor']`.
//!
//! Scanned: `mods/*/Scripts/*.lua` and `mods/shared/*.lua`.
//!
//! The space-named functions and class properties come from the game's
//! `UE4SS_ObjectDump.txt` (`Function /Game/...:Name With Spaces`, `<X>Property
//! /Game/...:Name With Spaces`; function parameters/locals are not members and are skipped),
//! plus the UE4SS_SDK headers when present. The game dir is `HSMP_GAME_DIR`, else
//! `<repo>/game`, else the main worktree's `game/`; the lint FAILS (exit 2) if no dump is
//! found, so it can never pass vacuously. Comments are ignored; string contents are ignored
//! except as an index key. Exit 1 on misuse.

use hsmp_tools::{lint, lualex, paths};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

#[derive(clap::Args)]
pub struct Args {
    /// Path to UE4SS_ObjectDump.txt (default: <game>/HalfswordUE5/Binaries/Win64/ue4ss/UE4SS_ObjectDump.txt)
    #[arg(long)]
    dump: Option<std::path::PathBuf>,
}

/// One space-named BP member.
#[derive(Clone, Debug)]
pub struct SpaceName {
    /// the real name, e.g. "Set Up Armor"
    pub real: String,
    /// "function" | "property"
    pub kind: &'static str,
    /// some UObject has a real member spelled like the squashed name (any case), e.g. the native
    /// FHitResult.ImpactPoint vs a BP struct's "Impact Point": only the exact squashed
    /// CamelCase spelling of a FUNCTION is then flagged, never a property
    pub native_twin: bool,
}

/// Space-named BP members by squashed lower-case key ("setuparmor").
pub type SpaceNames = BTreeMap<String, SpaceName>;

fn key(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect()
}

/// The dump's members: (space-named /Game/ functions, space-named /Game/ class properties,
/// lower-cased names of every member without spaces, any package).
pub fn parse_dump(text: &str) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    // [addr] Function /Game/Path/X.X_C:Set Up Armor [n: ...] ...
    // [addr] ObjectProperty /Game/Path/X.X_C:Component 1 [o: ...] ...  (a parameter / local of
    // a function has a second ':' segment: X_C:Func:Param - not a member, skipped below)
    let re = Regex::new(r"^\[[0-9A-Fa-f]+\] (Function|[A-Za-z]+Property) (/[A-Za-z]+/)[^:\s]*:(.+?) \[").unwrap();
    let (mut fns, mut props, mut plain) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for line in text.lines() {
        let Some(c) = re.captures(line) else { continue };
        let n = &c[3];
        if n.contains(':') {
            continue;
        }
        // a real internal space: "Name " (trailing blank) is not a space name
        if !n.trim().contains(' ') {
            plain.insert(n.trim().to_lowercase());
            continue;
        }
        if &c[2] != "/Game/" {
            continue;
        }
        if &c[1] == "Function" {
            fns.insert(n.trim().to_string());
        } else {
            props.insert(n.trim().to_string());
        }
    }
    (fns, props, plain)
}

pub fn space_names(fns: &BTreeSet<String>, props: &BTreeSet<String>, plain: &BTreeSet<String>) -> SpaceNames {
    let mut m = SpaceNames::new();
    let mk = |n: &String, kind| SpaceName { real: n.clone(), kind, native_twin: plain.contains(&key(n)) };
    for p in props {
        m.insert(key(p), mk(p, "property"));
    }
    // a function wins over a property of the same squashed name (the call form is the common bug)
    for f in fns {
        m.insert(key(f), mk(f, "function"));
    }
    m
}

/// Should the reference `id` to `n` be flagged? A function: any case (Lua resolves members
/// case-sensitively, but UE4SS's name lookup is case-insensitive, so `addarmortolist` is the
/// same silent no-op), unless a real member has that spelling, then only the exact squashed
/// spelling. A property: only the exact squashed CamelCase spelling, never when a real member is
/// spelled that way, and never when the file itself defines that field on a Lua table
/// (`{ RawDamage = x }`, `t.RawDamage = x`): those are plain Lua tables, not UObjects.
fn flag(id: &str, n: &SpaceName, lua_fields: &BTreeSet<String>) -> bool {
    let exact = id == n.real.replace(' ', "");
    match n.kind {
        "function" => exact || !n.native_twin,
        _ => exact && !n.native_twin && !lua_fields.contains(id),
    }
}

/// Field names the file assigns on Lua tables: `Name =` (constructor or `t.Name =`), not `==`.
fn lua_fields(code: &[String]) -> BTreeSet<String> {
    let re = Regex::new(r"(^|[{,;.\s])([A-Za-z_][A-Za-z0-9_]*)\s*=([^=]|$)").unwrap();
    let mut s = BTreeSet::new();
    for l in code {
        for c in re.captures_iter(l) {
            s.insert(c[2].to_string());
        }
    }
    s
}

/// Member references `.X` / `:X` (not `..` concatenation) in a code line (strings blanked):
/// (byte offset, identifier, is_call).
fn member_refs(code: &str) -> Vec<(usize, &str, bool)> {
    let b = code.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (c == b'.' || c == b':') && !(i > 0 && (b[i - 1] == b'.' || b[i - 1] == b':')) && !(i + 1 < b.len() && (b[i + 1] == b'.' || b[i + 1] == b':')) {
            // a number like 1.5 is not a member access
            let prev_digit = i > 0 && b[i - 1].is_ascii_digit() && {
                let mut k = i;
                while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
                    k -= 1;
                }
                b[k].is_ascii_digit()
            };
            let mut j = i + 1;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            let s = j;
            if j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'_') && !prev_digit {
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let mut k = j;
                while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                let call = k < b.len() && (b[k] == b'(' || b[k] == b'"' || b[k] == b'{');
                out.push((i, &code[s..j], call));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Every squashed-name reference in one Lua source: (1-based line, message).
pub fn scan(src: &str, names: &SpaceNames) -> Vec<(usize, String)> {
    let key_re = Regex::new(r#"\[\s*(?:"([A-Za-z_][A-Za-z0-9_]*)"|'([A-Za-z_][A-Za-z0-9_]*)')\s*\]"#).unwrap();
    let code = lualex::code_lines(src);
    let keep = lualex::comment_free_lines(src);
    let fields = lua_fields(&code);
    let mut hits = vec![];
    for (n, line) in code.iter().enumerate() {
        for (_, id, call) in member_refs(line) {
            let Some(sn) = names.get(&key(id)).filter(|sn| flag(id, sn, &fields)) else { continue };
            let (real, kind) = (&sn.real, sn.kind);
            let form = if kind == "function" {
                if call { format!("{id}()") } else { format!("{id} (a bare reference: pcall(obj.{id}, ...) / local f = obj.{id} is the same no-op)") }
            } else {
                format!(".{id}")
            };
            let fix = if kind == "function" { format!("obj[\"{real}\"](obj, ...)") } else { format!("obj[\"{real}\"]") };
            hits.push((n + 1, format!("{form}  -> BP {kind} \"{real}\": use {fix}")));
        }
        if let Some(raw) = keep.get(n) {
            for c in key_re.captures_iter(raw) {
                let id = c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()).unwrap_or("");
                let Some(sn) = names.get(&key(id)).filter(|sn| flag(id, sn, &fields)) else { continue };
                hits.push((n + 1, format!("[\"{id}\"]  -> BP {} \"{}\": the key needs its spaces: obj[\"{}\"]", sn.kind, sn.real, sn.real)));
            }
        }
    }
    hits
}

/// The files the lint reads: every mod's Scripts/*.lua plus the shared modules.
pub fn lint_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v = paths::mod_script_files(root);
    v.extend(paths::glob(&root.join("mods"), "shared/*.lua"));
    v.sort();
    v.dedup();
    v
}

pub fn run(a: Args) -> anyhow::Result<i32> {
    let root = paths::repo_root()?;
    let dump = match a.dump {
        Some(d) => d,
        None => paths::ue4ss_dir(&paths::game_dir(&root)?).join("UE4SS_ObjectDump.txt"),
    };
    if !dump.is_file() {
        anyhow::bail!(
            "UE4SS_ObjectDump.txt not found at {} (set HSMP_GAME_DIR or --dump; dump it in-game with UE4SS)",
            dump.display()
        );
    }
    let text = paths::read_text(&dump)?;
    let (mut fns, props, plain) = parse_dump(&text);
    let from_dump = fns.len();
    // The UE4SS_SDK headers, if the game has them.
    let sdk = dump.parent().unwrap_or(std::path::Path::new(".")).join("UE4SS_SDK").join("src").join("UE4SS_SDK").join("Game");
    let hpp_re = Regex::new(r#"UE_BEGIN_SCRIPT_FUNCTION_BODY\("/Game/[^"]*:([^"]+)""#).unwrap();
    let mut from_sdk = 0;
    if sdk.is_dir() {
        for p in paths::walk(&sdk, &|p| p.extension().map(|e| e == "hpp").unwrap_or(false)) {
            let s = paths::read_text(&p)?;
            for c in hpp_re.captures_iter(&s) {
                if c[1].contains(' ') {
                    from_sdk += 1;
                    fns.insert(c[1].to_string());
                }
            }
        }
    }
    if fns.is_empty() {
        anyhow::bail!("no space-named BP functions found in {} - wrong file?", dump.display());
    }
    let names = space_names(&fns, &props, &plain);
    let mut rep = lint::Report::new("check-bp-names");
    rep.note(format!(
        "{} space-named BP functions ({} from {}, {} SDK header hits), {} space-named BP properties",
        fns.len(),
        from_dump,
        dump.display(),
        from_sdk,
        props.len()
    ));
    let files = lint::LuaFile::load_all(&root, &lint_files(&root))?;
    for f in &files {
        for (ln, msg) in scan(&f.src, &names) {
            rep.push(f, ln, msg);
        }
    }
    rep.note(format!("{} Lua file(s) scanned (incl. shared/)", files.len()));
    Ok(rep.finish("no CamelCase BP-name misuse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
[0000027ABFEA22A0] Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Set Up Armor [n: 20C7D5] [c: 0]\n\
[0000027ABFEA22A1] Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Add Armor to List [n: 1] [c: 0]\n\
[0000027ABFEA22A2] Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Tick [n: 1] [c: 0]\n\
[0000027AB0E6F180] ObjectProperty /Game/Blueprints/Utility/Constraint_Zero.Constraint_Zero_C:Component 1 [o: 2A0] [n: 1]\n\
[0000027AB0E6F181] BoolProperty /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Set Up Armor:Local Flag [o: 2A0] [n: 1]\n\
[0000027AB0E6F182] Function /Script/Engine.Actor:K2 Destroy Native [n: 1] [c: 0]\n\
[0000027AB0E6F183] StructProperty /Game/Structs/S_Hit.S_Hit:Impact Point [o: 28] [n: 1]\n\
[0000027A57125300] StructProperty /Script/Engine.HitResult:ImpactPoint [o: 28] [n: 1]\n\
[0000027AB0E6F184] FloatProperty /Game/Structs/S_Dmg.S_Dmg:Raw Damage [o: 28] [n: 1]\n\
[0000027AB0E6F185] StrProperty /Game/Structs/S_N.S_N:Name  [o: 28] [n: 1]\n";

    fn names() -> SpaceNames {
        let (f, p, plain) = parse_dump(DUMP);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(p.iter().cloned().collect::<Vec<_>>(), vec!["Component 1".to_string(), "Impact Point".into(), "Raw Damage".into()],
                   "function locals and trailing-blank names are not space names");
        space_names(&f, &p, &plain)
    }

    #[test]
    fn flags_every_form() {
        let n = names();
        let bad = [
            "w:SetUpArmor()",
            "pcall(w.SetUpArmor, w)",
            "local f = w.SetUpArmor; f(w)",
            "w[\"SetUpArmor\"](w)",
            "w['SetUpArmor'](w)",
            "w:AddArmorToList(1)",
            "w:addarmortolist(1)",
            "local c = z.Component1",
            "local c = z[\"Component1\"]",
            "local d = hp.RawDamage",
        ];
        for b in bad {
            assert_eq!(scan(b, &n).len(), 1, "not flagged exactly once: {b} -> {:?}", scan(b, &n));
        }
    }

    #[test]
    fn clean_forms_pass() {
        let n = names();
        let good = [
            "w[\"Set Up Armor\"](w)",
            "pcall(w[\"Set Up Armor\"], w)",
            "w[\"Add Armor to List\"](w, 1)",
            "local c = z[\"Component 1\"]",
            "w:Tick(0.1)",
            "local s = \"SetUpArmor\" .. x -- a log string, not a key",
            "-- w:SetUpArmor() in a comment",
            "--[[ w.SetUpArmor ]]",
            "local s = a ..SetUpArmor",
            "local n = 1.5",
            "w:K2DestroyNative()",
            "local p = hit.ImpactPoint",
            "local p = hit.impactpoint",
            "local n = obj.name",
            "local n = h.component1",
            "local P = { RawDamage = r }\nlocal x = P.RawDamage",
        ];
        for g in good {
            assert!(scan(g, &n).is_empty(), "false positive: {g} -> {:?}", scan(g, &n));
        }
    }

    #[test]
    fn scans_shared() {
        let tmp = paths::make_temp_dir("hsmp_bpnames_").unwrap();
        let m = tmp.join("mods");
        std::fs::create_dir_all(m.join("shared")).unwrap();
        std::fs::create_dir_all(m.join("HSMPX/Scripts")).unwrap();
        std::fs::write(m.join("shared/x.lua"), "").unwrap();
        std::fs::write(m.join("HSMPX/Scripts/main.lua"), "").unwrap();
        let f: Vec<String> = lint_files(&tmp).iter().map(|p| paths::rel(&tmp, p)).collect();
        assert_eq!(f, vec!["mods/HSMPX/Scripts/main.lua".to_string(), "mods/shared/x.lua".to_string()]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}

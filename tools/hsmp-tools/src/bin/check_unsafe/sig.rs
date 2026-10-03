//! Reflection facts from `UE4SS_ObjectDump.txt` that `check_unsafe` needs:
//! UFunction parameter lists (to map RegisterHook callback params to their
//! types) and which member property names are Soft{Object,Class}Property.
//!
//! Dump lines look like
//!   [addr] Function /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr [n: ..] ...
//!   [addr] SoftObjectProperty /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr:Level [o: 8] ...
//!   [addr] SoftObjectProperty /Game/UI/X.X_C:Level [o: 398] ...
//!   [addr] SoftObjectProperty /Script/M.S:Arr:Arr [o: 0] ...        (container inner)
//! A property's owner is everything before its last ':'. Owner = a function
//! -> a parameter (or BP local; they follow the params). Owner = another
//! property -> a container inner, which makes the container soft-holding.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Default)]
pub struct Sigs {
    /// function path -> ordered (param name, soft?) (ReturnValue excluded).
    pub funcs: HashMap<String, Vec<(String, bool)>>,
    /// member property name -> (soft-holding owners, plain owners).
    pub members: BTreeMap<String, (u32, u32)>,
    /// member property names that are soft-holding on at least one `/Game/` class (the game's
    /// own Blueprints: `UI_List_MapSelect_C.Level`, `BP_CombatEvent_Master_C.Level`).
    pub game_soft: BTreeSet<String>,
}

fn is_soft(ty: &str) -> bool {
    ty == "SoftObjectProperty" || ty == "SoftClassProperty"
}

impl Sigs {
    pub fn parse(text: &str) -> Sigs {
        let re = Regex::new(r"^\[[0-9A-Fa-f]+\] (\w+) (.+?) \[").unwrap();
        let mut s = Sigs::default();
        // property path -> (function path, index) for params; member path -> name
        let mut param_at: HashMap<String, (String, usize)> = HashMap::new();
        let mut member_at: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            let Some(c) = re.captures(line) else { continue };
            let (ty, path) = (&c[1], c[2].to_string());
            if ty == "Function" || ty == "DelegateFunction" || ty == "SparseDelegateFunction" {
                s.funcs.entry(path).or_default();
                continue;
            }
            if !ty.ends_with("Property") {
                continue;
            }
            let Some((owner, name)) = path.rsplit_once(':') else { continue };
            if let Some(params) = s.funcs.get_mut(owner) {
                if name != "ReturnValue" {
                    param_at.insert(path.clone(), (owner.to_string(), params.len()));
                    params.push((name.to_string(), is_soft(ty)));
                }
            } else if let Some((f, i)) = param_at.get(owner).cloned() {
                if is_soft(ty) {
                    if let Some(p) = s.funcs.get_mut(&f).and_then(|v| v.get_mut(i)) {
                        p.1 = true;
                    }
                }
            } else if let Some(m) = member_at.get(owner).cloned() {
                if is_soft(ty) {
                    if owner.starts_with("/Game/") {
                        s.game_soft.insert(m.clone());
                    }
                    let e = s.members.entry(m).or_default();
                    e.0 += 1;
                    e.1 = e.1.saturating_sub(1);
                }
            } else if !owner.contains(':') {
                // a class / struct member
                member_at.insert(path.clone(), name.to_string());
                let e = s.members.entry(name.to_string()).or_default();
                if is_soft(ty) {
                    e.0 += 1;
                    if owner.starts_with("/Game/") {
                        s.game_soft.insert(name.to_string());
                    }
                } else {
                    e.1 += 1;
                }
            }
        }
        s
    }

    /// Names that are soft-holding in every owner that declares them: reading
    /// `obj.<name>` is a SoftObject read whatever the object is.
    pub fn soft_only(&self) -> BTreeSet<String> {
        self.members.iter().filter(|(_, (s, p))| *s > 0 && *p == 0).map(|(n, _)| n.clone()).collect()
    }

    /// Names that are soft in some owners and plain in others (not linted: the
    /// object's class decides; reported with --list-soft).
    pub fn soft_ambiguous(&self) -> BTreeSet<String> {
        self.members.iter().filter(|(_, (s, p))| *s > 0 && *p > 0).map(|(n, _)| n.clone()).collect()
    }

    /// Ambiguous names (soft on some owners, plain on others) that are soft on a `/Game/`
    /// class: U2 flags these too - the game's own Blueprint rows (`entry.Level` on a
    /// map-select row) are what the mods read. A read known to be on a plain owner is marked
    /// `-- soft: ok <class>` on the line.
    pub fn game_ambiguous(&self) -> BTreeSet<String> {
        self.soft_ambiguous().into_iter().filter(|n| self.game_soft.contains(n)).collect()
    }

    /// Soft parameters of a function: (index, name).
    pub fn soft_params(&self, func: &str) -> Vec<(usize, String)> {
        self.funcs
            .get(func)
            .map(|v| v.iter().enumerate().filter(|(_, p)| p.1).map(|(i, p)| (i, p.0.clone())).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
pub const TEST_DUMP: &str = "\
[0001] Function /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr [n: 1]
[0002] ObjectProperty /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr:WorldContextObject [o: 0]
[0003] SoftObjectProperty /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr:Level [o: 8]
[0004] BoolProperty /Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr:bAbsolute [o: 30]
[0005] Function /Script/Engine.GameplayStatics:OpenLevel [n: 2]
[0006] ObjectProperty /Script/Engine.GameplayStatics:OpenLevel:WorldContextObject [o: 0]
[0007] NameProperty /Script/Engine.GameplayStatics:OpenLevel:LevelName [o: 8]
[0008] SoftObjectProperty /Game/UI/X.X_C:TargetLevel [o: 398]
[0009] SoftObjectProperty /Game/UI/Y.Y_C:Level [o: 398]
[0010] ObjectProperty /Script/Engine.Actor:Level [o: 1]
[0011] ArrayProperty /Game/UI/Z.Z_C:SoftList [o: 2]
[0012] SoftClassProperty /Game/UI/Z.Z_C:SoftList:SoftList [o: 0]
[0013] Function /Game/Ch/Willie_BP.Willie_BP_C:Set Up Armor [n: 3]
[0014] BoolProperty /Game/Ch/Willie_BP.Willie_BP_C:Set Up Armor:Clear Previous [o: 0]
[0015] SoftObjectProperty /Script/Engine.World:NativeOnlySoft [o: 0]
[0016] ObjectProperty /Game/UI/Y.Y_C:NativeOnlySoft [o: 0]
[0017] SoftObjectProperty /Game/Maps/M.M_C:CookedWorldAsset [o: 0]
";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_params_members_and_containers() {
        let s = Sigs::parse(TEST_DUMP);
        assert_eq!(s.soft_params("/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr"), vec![(1, "Level".to_string())]);
        assert!(s.soft_params("/Script/Engine.GameplayStatics:OpenLevel").is_empty());
        let only = s.soft_only();
        assert!(only.contains("TargetLevel") && only.contains("SoftList"), "{only:?}");
        assert!(!only.contains("Level"));
        assert!(s.soft_ambiguous().contains("Level"));
        // soft on /Game/UI/Y.Y_C, plain on the native Actor: flagged
        assert!(s.game_ambiguous().contains("Level"));
        assert!(!s.game_ambiguous().contains("NativeOnlySoft"), "soft only on a /Script/ class + plain elsewhere: not a /Game/ name");
        assert_eq!(s.funcs["/Game/Ch/Willie_BP.Willie_BP_C:Set Up Armor"].len(), 1);
    }
}

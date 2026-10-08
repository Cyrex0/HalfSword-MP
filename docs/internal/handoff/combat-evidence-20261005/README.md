# Native weapon catalogue audit — 2026-10-05

This is static evidence from the installed game's cooked packages. It is not an exhaustive native gameplay pass. No game actors were spawned by this extraction. The driver invokes the existing MapDump parser in memory and saves no decryption material.

## Authoritative inventory

- `all-pak-asset-packages.txt`: all **12,941** mounted native `.uasset` paths (LogicMods excluded by the existing parser).
- `extraction.json`: **1,309** initial weapon/module/config/geometry packages, zero extraction errors.
- `supplemental-extraction.json`: **50** directly referenced external packages, zero extraction errors. These include meshes for improvised weapons, traps and quivers, native forge constructors, and shared damage classes.
- `blueprints.json`: parsed class defaults, inherited defaults and component templates, original references and local function names. There are **154 built weapon classes** and **401 modular-part classes (399 family/master classes plus the generic Grip and SubModule bases, classified by native inheritance)**. Additional shared/forge/character classes are retained separately.
- `coverage-manifest.json`: one row per built weapon, registration, role, exact class hash, source evidence, possible module choices and explicit missing coverage.
- `module-configurations.json`: all 401 modular classes, their component geometry/assets, physical body defaults, scales and native damage tags.
- `module-choice-edges.json`: **2,462** inheritance/default-reference/literal-class-choice edges, with original function and statement offsets where applicable. The transitive choice lists are conservative static possibilities, not proof every combination is valid or was spawned.
- `geometry-assets.json`: **512 StaticMesh / BodySetup pairs**, four SkeletalMeshes, three weapon PhysicsAssets, twelve skeletal bodies and nine constraints after reference expansion.
- `skeletal-configurations.json`: exact per-bone capsule geometry and mass/physics settings for the three articulated strap variants, plus the crossbow skeletal component.
- `native-function-call-sites.json`: relevant native damage/module/physics/spawn/random call sites. Full bytecode remains in `exports/`.
- `data-assets.json`: **45** equipment module/premade/type data exports.

The 80 remaining external references after expansion largely belong to Willie, UI, armor and effects dependencies. They are explicitly listed in `missing-reference-packages.txt`; this audit does not claim recursively exporting the entire game. There are no unresolved weapon geometry/class references in that list (the two weapon-directory references are scabbard textures).

## Registration and source identity

| Native role | Classes | Current boundary |
|---|---:|---|
| Held melee | 141 | 128 catalogue entries plus 13 additional registered melee classes; common DCD path implemented, exhaustive native coverage pending |
| Fists / feet | 2 | Actual primitive sampling and source identity implemented; full lethal/armor/limb matrix pending |
| Ranged weapon | 2 | Fire/reload/ammo state is not replicated |
| Projectile | 2 | Fired actor lacks held-source identity/authority and lifecycle |
| Quiver | 2 | Native ammo container, not a melee source |
| Trap | 5 | Unheld world source authority is missing |
| Total | **154** | No all-green coverage claim |

All 154 generated short class names fit `Damage.source_class` (`Str48`): the longest is **40 UTF-8 bytes**. Their FNV-1a32 class hashes contain **no collisions**. The full class names and hashes are recorded individually, so this result is reproducible and does not rely on a sample.

## Concrete geometry gaps

The source contains three articulated mace strap modules: `Weapon_Part_Mace_Strap_B_C`, `_C_C`, and `_I_C`. Each resolves to a native PhysicsAsset with **four QueryAndPhysics capsule bodies**, named `Joint1` through `Joint4`, and three constraints. Their native radii, lengths, rotations, centers and masses are in `skeletal-configurations.json`.

Static constructor reachability links those variants to six built mace classes: High Tier Avg/Short, Low Tier Short, and Mid Tier Avg/Long/Short. These are genuine catalogue coverage gaps. The current one-OBB-per-component weapon geometry cannot represent four independently moving capsule bodies faithfully. A new generic body/shape key and current native physics/bone transforms are required; a larger render bounding box is not a correction.

`BP_Weapon_Ranged_Weapon_Crossbow_Light_C` also has an actual skeletal component. Its exported SkeletalMesh has no serialized PhysicsAsset reference. This is retained as an explicit separate configuration, not assigned guessed collision bodies.

Native static flails are different from these skeletal straps: their moving components require the already-added per-frame component transform and module velocity sampling. That code path still needs native dynamic validation. Static defaults alone cannot establish the full pose/timing behavior of a flail.

The 12-row pose change preserves the existing 0.1cm center/extent precision and fits the tested maximum 1,118-byte frame / 1,179-byte UDP packet. It fixes the observed five-parent/five-child overflow. It does not prove all dynamic configurations fit or that skeletal collision geometry is supported. Overflow remains explicit and must not silently discard rows.

## Damage and outcome coverage

See `../native-damage-route-audit-20261005.md` for the independent native function audit. Ordinary initial held-weapon DCD, native cutting child identity, armor inputs and owner replay are implemented. The captured `Constraint_Weapon_Stuck` direct Inside/GetDamage route remains evidence-only and is not forwarded or reconstructed in production. Fired/released sources and traps require durable independent source identity. These are distinct missing paths, not damage multipliers to adjust.

The previous live run proved some sharp-parameter hits changed health and limb fields, but many accepted contacts changed only consciousness or bookkeeping. The receipt analysis is saved under `../../20261005-121745-7edf45-combat_manual/astra-causal-review/`. Neck health did not change in its captured neck-hit receipts. This inventory does not convert acceptance, consciousness loss, or the eventual death into proof of complete sharp/gore parity.

## Reproduction and next proof

`AssetBatch/` uses the existing built MapDump library without copying parser credentials. `analyze_assets.py` produces the inherited class/geometry tables; `build_coverage.py` produces the registration, choice and physics coverage tables. Parse errors are recorded separately in `parse-errors.json` (currently empty).

Next automated native verification should derive cases from this inventory: each of the 141 registered melee classes, every distinct reachable module geometry/tag configuration, all twelve articulated strap bodies, fists/feet, armor layers, and initial versus continued penetration. First verify safe isolated actor construction/retirement. Compare solo-native and authenticated replay outcomes using original native inputs, with health, consciousness, each structural health field, constraints, missing bodies and delayed bleeding recorded separately. Human fighting is supplementary timing evidence, not an adequate exhaustive catalogue test.

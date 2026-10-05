# Damage parity: offline proof and in-game checks

The design is in [combat.md](combat.md) (§5 client, §7 solo parity, §8 health). The offline proof is
`tools/hsmp-tools/lua-tests/damage_parity.lua`; run it with `hsmp-tools lua-test damage_parity`
(`hsmp-tools lua-test` with no argument runs every suite).

## 1. What the offline proof shows

The suite runs the real `HSMPCombat/Scripts/main.lua` against a model of the decompiled native path:
"Deal Complex Damage" (contact gate with its 0.1 s reset; the hit point mapped into the hit bone's
space; the armour layers covering THAT spot, stacked; Raw) then "Get Damage" (Invulnerable gate,
DRS, the per-bone Last Damage Taken gate with its 0.2 s reset, impulse gate, Health with the
per-part floor, bleeding, consciousness with the 'Weapon' factor on light blows, pain and its stumble
direction, part health, blood). As in UE4SS, every hook callback runs after the function body. On the
attacker's screen the stand-in is turned 70° and leaning 15° from where the victim is when the replay
runs, wears other armour and is Invulnerable.

| # | Claim | Check |
|---|---|---|
| 1 | Sword cut, sword thrust, axe, mace, polearm haft, fist and kick × cloth, padded, mail, plate and mixed armour × head, torso, arm, leg × front, side, back (420 blows): the MP change of Health, Consciousness, Bleeding, Pain, part Health, blood marks and stumble direction equals solo, field by field (tables printed). A taller stand-in changes nothing. | `every weapon x armour x part x spot`, `1b.` |
| 2 | One clean blow = one claim, and the Invulnerable stand-in takes nothing. | `2.` |
| 3 | A graze and a harder frame in one tick both land (two claims), as in solo; a weaker second frame is stopped by the game's gate on both sides. | `3.` |
| 4 | Two blows 350 ms apart whose replays arrive in one victim tick are not gated by each other; a weaker blow 150 ms after the first is, as in solo. Also when the replays arrive 300 ms apart (the game's own 0.2 s reset has fired on the victim by then). | `4.` |
| 4b | A stand-in whose Invulnerable the game reset takes the blow and is put back, but keeps its contact gate: a frame that gate stops in solo is no claim. | `4b.` |
| 5 | A light weapon blow keeps Get Damage's 'Weapon' consciousness factor; the same blow by a fist does not. | `5.` |
| 6 | The echo is undone and reported as a touch; a rejected blow leaves the victim as it was; other screens see the blood on the stand-in, whose vitals stay the owner's. | `6.` |

To see what a change fixes, run the suite against an older `main.lua`: a two-line wrapper suite
setting the global `HSMP_PARITY_SRC` to that file and `dofile`-ing `damage_parity.lua` prints the
same tables without the checks. Against main before the bone-space fix (world-space offsets, strongest call per
tick, no gate emulation, no weapon) 42 of the 420 blows matched:

| armour | blows | identical before / after | same damage (direction aside) before / after | worst Health or Consciousness error before |
|---|---|---|---|---|
| cloth | 84 | 0 / 84 | 78 / 84 | 15.2 (kick: no weapon tag) |
| padded | 84 | 6 / 84 | 78 / 84 | 15.2 |
| mail | 84 | 6 / 84 | 78 / 84 | 15.2 |
| plate | 84 | 24 / 84 | 44 / 84 | 167.2 (a mace blow replayed under another plate layer) |
| mixed | 84 | 6 / 84 | 60 / 84 | 167.2 |

Cloth and padded layers cover the whole part, so a misplaced spot changed only the stumble
direction there; plate covers spots, so the same misplacement switched whole layers on and off. That
is the "armour especially" the playtests reported. The combat sim measures the same effect with real
network timing and moving fighters (`hit_location_survives_the_replay_delay`, combat.md §2).

The model cannot prove the game's own behaviour. The checks below can.

## 2. In-game checks

HSMPCombat logs to `UE4SS.log` with the prefix `[HSMPCombat]`; the server logs to its console (or
wherever you redirect it). Run two clients on a dedicated server, one fight per weapon class (blunt,
sword, rondel, fists). For the solo reference, fight the same blows in a solo duel. Back up
`GameProgress.sav` first: the save redirect (`mods/shared/hsmp_saveguard.lua`) is active only
during an MP session, so solo play writes your career save.

1. **Hooks.** The log says `Get Damage hook registered (after-call: echo undo, stand-in backstop)`
   and `Deal Complex Damage hook registered (claims: armour-stage inputs)`. Each blow gives one
   `HIT peer N stand-in bone=... claim #...` line with plausible `vel` / `rig` / `cut` / `stab`.
2. **Stand-in protection.** No `native damage on stand-in ... put back` lines (a few are tolerable:
   the game reset `Invulnerable`), and no `WARNING: stand-in of peer N ran its native death locally`.
3. **Echo undo.** On the victim, `stand-in N blow on my pawn undone` appears for body contacts;
   never `(NO baseline yet)` after the first second of a round.
   `WARNING: a stand-in echo on my pawn dismembered` must not appear.
4. **Replay.** `HIT from peer ... : native Deal Complex Damage(ok) dmg Health ... [...]` with
   non-empty field lists for clean blows. Compare Health, limb and bleed per blow with the solo duel
   (same weapon, same zone): within the spread of solo hits of similar speed.
5. **Vitals on both screens.** The victim's HUD and the attacker's HUD show the same CON / BODY %
   for the victim. The server log has no `Health rose faster than any heal` for honest players and
   no `death (server ledger)` lines; a `god-mode suspect` warning is information only.
6. **Gore on both screens.** Zoomed screenshots after a cut: blood and wound on the victim's own
   pawn and on the attacker's stand-in of the victim (`hit fx: peer A -> peer V ... fx: replayed on
   stand-in`). Severed limbs are hidden on the stand-in when the owner's "Dismembered Array" lists
   them (`peer P severed '<bone>' -> stand-in bone hidden`).
7. **Parry.** `combat_quality` lines: `parried` only for real blocks; binds that slide into the body
   are accepted. The server log (debug level) shows `touch recorded` for the victim's touch reports.
8. **Picked-up weapons.** Pick up a weapon a stand-in dropped: the log says
   `weapon ... (a stand-in's) is in my hands now: damage gate lifted` and it deals damage.

## 3. In-game experiments (HSMPParity, dev deploy)

`mods/dev/HSMPParity` runs experiments inside the game, driven over the DevCtl ring (no OS input):

```
hsmp-tools ipc-ctl --pid <game pid> autotest parity "kit"
hsmp-tools ipc-ctl --pid <game pid> autotest parity "spots 70"
hsmp-tools ipc-ctl --pid <game pid> autotest parity "near 2"
hsmp-tools ipc-ctl --pid <game pid> autotest parity "swing 1400"
```

Results go to `UE4SS.log` (`[HSMPParity]`) and `<state_dir>/.parity_results.txt`. Every experiment
refuses to run outside an MP session: only there is the career save redirected.

- `kit`: my pawn and every stand-in as Deal Complex Damage sees them: equipped slots, worn meshes,
  the armour proxy tags (`defB` / `defC` / `defS` / `dens`), bone mass scales and masses (armour
  weight raises them, and with them the normal impulse a blade gets), Team Int, height and muscle
  rates, Invulnerable. Run it on both instances and compare the owner's line with its stand-in's.
- `spots [deg]`: on my own pawn, per bone, spot and blow (mace, cut, stab, fist), Deal Complex Damage
  three ways: solo (the spot), `world` (the spot a world offset lands on when the stand-in stood
  `deg` turned: HSMPCombat before the bone-space fix) and `local` (the bone-space offset: HSMPCombat now). Every
  field is put back after each blow. `SPOTS done` gives the counts.
- `near <peer>` / `swing <speed>`: stand in front of a stand-in and push my held weapon at its chest:
  a physics blow through the whole MP path (claim, server, victim replay; the server's
  `combat: impact rescale` line shows claimed vs forwarded speed for the `hit_vel_factor` ceiling).
- Every Deal Complex Damage the game makes is logged (`DCD on ...`: |velocity|, |impulse|, relative
  speed, cutting power, stab rate, rigidity, kick) for calibration.

### First run (2026-10-04, combat_probe, 2 instances, dedicated server, Alley)

- `kit`: for both players the stand-in matches the owner's **armour** exactly: the same 8 equipped
  slots, 10 worn meshes and the same 23 proxy collisions with identical `defB` / `defC` / `defS` /
  `dens` tags. The armour stage therefore sees the same layers on the stand-in as on the owner. The
  **body does not match**. Stand-ins keep the pooled foe's passport body (Height Rate 0.61–0.75,
  Muscle Rate 0.83–0.88; the owners have 0.946 / 0.018). With it they keep that foe's `Mass Scale
  (Set in BP)` (1.21 vs 1.005) and bone masses 2–2.6× the owner's (pelvis 15.7 kg vs 6.05 kg). The
  damage replay uses the victim's own height factor, so it is unaffected. The normal impulse the
  attacker's weapon gets from the heavier stand-in is affected, and Hit Velocity takes the larger of
  that impulse and the weapon's speed. Since then the owner's body travels as the `body` record and
  each stand-in gets its owner's bone masses and rates (combat.md §5 "Stand-in body"); check it
  with the steps in §4.
- `spots 70` on my own pawn: 112 blows (7 bones × 4 spots × mace / cut / stab / fist). The
  world-offset replay of a stand-in that stood 70° turned differed from solo in **31**. For example,
  a head cut did −101 consciousness / +114 bleeding in solo and −4.8 consciousness in the replay. The
  bone-local replay matched in **111**. The one exception is a forearm the first (solo) blow broke:
  `Break Arm` cannot be undone by the harness, and it changed the next two blows' pain.
- `swing`: setting the root velocity of a held weapon did not move it into the stand-in (no contact).
  The experiment needs another way to drive the weapon.

## 4. Stand-in body (the `body` record)

Offline: `hsmp-tools lua-test standin_body` (the record from the owner's pawn, the masses on the
stand-in) and `cargo test -p hsmp-server body` (wire, caps, sidecar slot). In game, with two
instances on a dedicated server of this build:

1. Each `UE4SS.log` has `[HSMPCombat] body: own passport body v=... height=... muscle=...
   bones=N` once after the arena loads (and again only when the body changes, e.g. after
   dressing), with N around 22. The sidecar log has `body sent`; the other sidecar `body
   received` with the same version, and the server `body stored ... receivers=1`.
2. Within a second of the stand-in appearing: `body: stand-in Willie_BP_C_... of peer N <- owner
   body v=...: K bone mass(es) set`. A trailing `(the game reset R before)` that keeps growing
   means the game re-applies its own masses: report it with the log.
3. `hsmp-tools ipc-ctl --pid <game pid> autotest parity "kit"` on both instances: the stand-in's
   `HeightRate`, `MuscleRate`, `mass_scale_bp` and every `mass=...kg` value now equal the owner's
   line on the other instance (within 1 %); the `mass` scales differ where the stand-in's bones are
   a different size. Its height (mesh size) still is the pooled Willie's (not applied).
4. A beta.4 client in the same lobby: no `body` lines for it on any side, and its own stand-ins
   keep the pooled bodies (no error, no disconnect).
5. Fight a round: no new `native damage on stand-in ... put back` or stretch / launch of the
   stand-in after the masses change (`spawn_stretch` stays ≤ 10 uu, combat.md / replication.md).

## 5. Asset-derived coverage and articulated weapons (2026-10-04)

The installed PAK was read directly with the existing MapDump/CUE4Parse tool and matching
usmap. `test-results/dev-feature-checks/combat-asset-packages.txt` lists 2,376 parsed weapon,
armour and equipment-data packages with no parse errors. `weapon-pak-coverage.json` verifies
the existence of all 128 configured weapon packages; `armour-pak-coverage.json` verifies all
116 configured armour packages. This establishes asset availability, not runtime collision
or damage parity. There are 154 packages under `Built_Weapons`; the exact 26 outside the
configured catalogue are in `weapon-pak-outside-catalogue.txt`, including Fists, Feet,
uncatalogued melee variants, traps, ranged weapons/projectiles and quivers.

`weapon-flail-asset.json` is the full cooked export of
`/Game/Assets/Weapons/Blueprints/Built_Weapons/Reforged/ModularWeaponBP_Flail_A`.
Its `Head_GEN_VARIABLE` and `Link 1_GEN_VARIABLE` are **StaticMeshComponent** objects with
`BodyInstance.bSimulatePhysics = true`. `PhysicsConstraint1_GEN_VARIABLE` joins Head to
Link 1 with 180-degree swing limits. Therefore a component's actor-relative transform is
not constant merely because its mesh is static. The sender now samples module transforms
each frame. Server history interpolates matching native component IDs, and contact speed
tracks the selected material point through both the module and actor transforms. Tests
cover a stationary actor/Grip with an independently translating and rotating Head, including
an impact between snapshots and reordered module-array storage. No geometry allowance was
increased by this change.

The Fists and Feet asset exports independently confirm a 13 cm fist sphere and foot box
half-extents of (7.5, 15, 7.5) cm before scaling. Both declare zero edge sharpness and retain
the native Weapon tag; Fists declares rigidity 0.4. The streamed body-striker geometry uses
the actual per-frame native properties and transforms rather than those constants.

Remaining coverage limits are explicit: static mesh envelopes are not exact cooked collision
shapes; skeletal modules and active-module capacity still need a complete runtime inventory.
The first attempted inventory crashed on naming an invalid CDO class reference; those
speculative property reads were removed and reflected names are validity-checked. A later
single-class construction/inspection succeeded but cleanup was not confirmed, so neither
run is a catalogue pass. Class defaults should be inspected offline; runtime diagnostics
must confirm destruction before progressing to another class.

Original cutting-box identity, historical orientation, extent and native scale are now
carried and replayed using a private collision-disabled Box. Native DCD/GetDamage and the matching executable's RVP implementation
use that geometry for cutting/paint; the native paint path snapshots it before asynchronous
work. Complete cutting/gore parity still requires validating the victim proxy's actual
bone frame against its owner's physics and native continued penetration. Persistent injury comparisons also require fresh native reference/network pawn
pairs: restoring scalar health fields does not undo broken joints or severed bodies. These
comparisons can be automated; manual fights are not an exhaustive acceptance test.

## 6. Complete native class audit (2026-10-05)

`test-results/dev-feature-checks/weapon-catalogue-audit/coverage-manifest.json` accounts
for every one of the 154 built weapon classes. There are 141 registered melee classes
(128 canonical and 13 additional native classes), two body strikers, two ranged weapons,
two projectiles, two quivers and five traps. The extraction parsed 1,359 packages without
errors, including 401 modular-part classes and 2,462 conservative constructor/reference
edges. Every full short class name fits the source record; none of their class hashes collide.
This proves static coverage, not every construction combination or native combat outcome.

The matching installed executable was checked against the isolated IDA database by SHA-256.
Native exports confirm RVP snapshots/deep copies and actor-enumeration retirement semantics;
the cooked Blueprint exports provide the gameplay damage, module, firing and penetration logic.
Fired/released actors, traps, independently moving skeletal strap bodies and stuck-weapon
Inside continuation remain explicit incomplete production routes.

Native run `20261005-125818-c9ef9c-combat_manual` exposed 154 accepted fist contacts whose
victim replay dropped the source after the temporary fist actor disappeared, and 272 accepted
contacts whose native input vectors were reduced. The fist replay factory preserves the exact
native Sphere ordinal and original life without replacing held equipment. Impulse-selected
weapon inputs now retain the already-validated impulse envelope instead of using a smaller
speed-unit ceiling. Neither change establishes exhaustive injury parity.

Run `20261005-134221-29e054-combat_manual` completed four rounds without a new crash.
All 73 detailed cutting-geometry rejections had valid original scales/extents. Native-owner
pose reconstruction reproduces the server errors within 1cm/1degree; several inferred proxy
orientations differ by 30–60degrees while their bone origins remain close. These results do
not justify increasing geometry tolerances. Exact native proxy rotation, playback and anatomy
must be resolved. The native inventory also remains gated on positive single-class retirement;
construction observations must never be reported as combat passes.

`scripts/analyze_weapon_inventory.py <run>` joins observations to all 154 classes and explicitly
keeps construction/retirement separate from native combat parity. The opt-in combat pose tap
retains each exact transmitted frame; `combat_pose_decode` decodes it using the production codec.

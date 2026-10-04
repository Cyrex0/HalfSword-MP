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
| 4 | Two blows 350 ms apart whose replays arrive in one victim tick are not gated by each other; a weaker blow 150 ms after the first is, as in solo. | `4.` |
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
  that impulse and the weapon's speed. This is open: the `loadout` record carries no body passport
  (see combat.md §5).
- `spots 70` on my own pawn: 112 blows (7 bones × 4 spots × mace / cut / stab / fist). The
  world-offset replay of a stand-in that stood 70° turned differed from solo in **31**. For example,
  a head cut did −101 consciousness / +114 bleeding in solo and −4.8 consciousness in the replay. The
  bone-local replay matched in **111**. The one exception is a forearm the first (solo) blow broke:
  `Break Arm` cannot be undone by the harness, and it changed the next two blows' pain.
- `swing`: setting the root velocity of a held weapon did not move it into the stand-in (no contact).
  The experiment needs another way to drive the weapon.

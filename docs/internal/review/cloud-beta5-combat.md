# beta5 review: combat (branch `cloud/combat`)

Scope: `mods/HSMPCombat`, `server/src/validate/`, `server/src/combat.rs`, `crates/hsmp-combat-sim`,
`damage_parity.lua`. The body record also touches `crates/hsmp-ipc` (schema, segment, generated
files), `crates/hsmp-net` (one cap bit, one stream key), `server/src/loadout.rs`,
`loadout_client.rs`, `sidecar/{net,handlers}.rs`, `interact_client.rs` (one accessor) and
`mods/dev/HSMPParity` (one log field).

## 1. MP-vs-solo divergences fixed (HSMPCombat)

| change | evidence (fails without the fix) |
|---|---|
| **Get Damage gate on spread replays.** Two blows of one attacker < 0.2 s apart on its clock gate each other in solo. When their replays reached the victim > 0.2 s apart (jitter, a parry hold), the game's RetriggerableDelay had already cleared `Last Damage Taken` and the second blow landed in full. The replay now writes the first blow's value and bone back when the gate must hold. | `damage_parity` check "4. ...also when its replay arrives 300 ms after the first": old main.lua: MP Head Health −100, Consciousness −760 more than solo (one extra mace blow); new: identical. |
| **Stand-in backstop kept the contact gate.** When the game reset a stand-in's `Invulnerable`, the backstop restored the whole baseline from inside the nested Get Damage callback, including Deal Complex Damage's `Last Complex Damage Impulse/Bone`, before the DCD callback read it. Every later frame of the same contact then passed the stand-in's gate and became a claim (a lower-impulse, higher-speed frame then passed the victim's Get Damage gate: extra damage). The backstop now leaves that gate as the call left it. | `damage_parity` check "4b. ...also when the stand-in took the first frame natively": old: 2 claims and extra Pain / Consciousness on the victim; new: 1 claim, identical to solo. |

The damage_parity model's 0.2 s reset now models the delay properly (a value written after the
delay fired stays), otherwise neither the bug nor the fix is visible. `lua-test damage_parity`:
26 checks pass (was 22; 2 of the new ones fail against beta.4's main.lua). `combat` 90/90,
`vitals` 51/51.

## 2. hit_vel_factor calibration (tooling, factor unchanged)

- `hsmp-combat-sim --hvf PROFILE [--seeds N]`: every honest armour-stage claim of the suite fights;
  `--hvf-logs FILE...`: HSMPParity `DCD on` lines and server `combat: impact rescale` lines. Per
  class: needed factor p50/p99/p99.9/max, the share the shipped factor cuts, the share it would cut
  against the true relative speed (sim), the share the looser `hvf·max(peak, rel)` would cut, and a
  recommendation (p99.9 × 1.1) from ≥ 200 binding samples. `src/calib.rs`, `tests/calib.rs`
  (parsers, the math, and `the_calibrated_ceiling_is_the_servers`: over a 72-point grid the real
  `clamp_impact_ex` cuts exactly when the tool says so).
- The server's impact rescale line now carries `class`; HSMPParity logs the weapon actor (`wp=`)
  and keeps 3000 DCD lines per session.
- Procedure documented in combat.md §7 ("Calibrating `hit_vel_factor`").

Numbers (`cargo run -p hsmp-combat-sim -- --hvf typical --seeds 3`, then `wifi`):

| profile | class | claims | shipped cuts | vs true rel | `hvf·max(peak, rel)` | net p99 |
|---|---|---|---|---|---|---|
| typical | dagger | 76 | 10.5 % | 2.6 % | 6.6 % | 2.18 |
| typical | sword | 476 | 5.0 % | 1.3 % | 0.8 % | 2.06 |
| typical | axe | 166 | 3.0 % | 1.2 % | 0.6 % | 2.04 |
| typical | blunt | 139 | 8.6 % | 2.2 % | 2.9 % | 2.05 |
| typical | polearm | 299 | 6.0 % | 1.3 % | 2.3 % | 2.41 |
| wifi | sword | 472 | 3.6 % | 1.5 % | 0.9 % | 2.02 |
| wifi | polearm | 283 | 7.1 % | 1.1 % | 1.1 % | 3.33 |

Finding: most honest blows the ceiling cuts are cut by lag comp's relative speed (true/server
up to 2× at p99), not by the physics. I tried the looser ceiling `hvf·max(peak, rel)` (and
`max(peak, hvf·max(rel, k·peak))` with k = 0.5 / 0.7): `cheats_rejected` then let 14 / 3 / 5 of
187 `DamageInflate` claims through (bar: 1). Reverted; ceiling and factor unchanged. The sim's
physics is the shipped factor itself, so the physics half needs the in-game DCD capture (not done
here: needs Windows and the game).

## 3. Owner's body on stand-ins (`body` record, `caps::BODY`)

- hsmp-ipc: record `body` 0x0515 (head 40 B: version, Height / Muscle Rate, Mass Scale (Set in
  BP), Character Scale; ≤ 24 rows of 40 B: bone, mass kg, mass scale), checks on every bound, game
  slot `body`, per-peer slot `peer_body`, stream 0x89. `hsmp-tools gen-ipc` regenerated
  `hsmp_ipc.h` and `hsmp_ipc_schema.lua`; layout hash changed (`e66f34519195e128`), segment
  0x392000 → 0x39B000 (docs updated).
- Interop: hsmp-net `caps::BODY` (1 << 17), offered by server and sidecar. The sidecar sends
  `body` only when the connection negotiated it; the server relays and replays it only to peers
  that negotiated it. A beta.4 server / sidecar never sees kind 0x0515. Protocol stays v6.
- Server: newest version per owner (the loadout store generalised to `store_versioned`), relayed
  once, replayed to capable joiners, forgotten on leave.
- HSMPCombat `standin_body.lua` (new sibling module; main.lua only got the two hooks, no new
  locals): own body every 2 s into `body` (new version only when it changed); once a second per
  stand-in the owner's rates and, per bone, `SetMassScale(FName(bone), scale·owner_mass/mass_now)`
  on the stand-in's `Mesh`. BP names with spaces, pcall everywhere, no UObject kept (keyed by
  FName), state dropped on world drop, game thread only, nothing destroyed. Geometry (height) is
  carried but **not applied**: HSMPAvatars measures the stand-in's bone offsets once per drive
  (`servo_setup`), and rescaling the mesh under it would stretch every joint. Applying height
  needs Avatars to re-measure on a scale change.
- Tests: `hsmp-ipc body_layout_round_trip_and_hostile_bytes`, server
  `body_records_reach_only_capable_peers_newest_first`,
  `body_records_reach_the_peer_slot_and_the_game_slot_is_read`, `lua-test standin_body`
  (18 checks: record from the pawn, change detection, stand-in masses equal the owner's to 1e-6,
  geometry untouched, reset by the game re-applied and counted, tick cadence, world drop).
- In-game verification steps: combat-parity.md §4.

## 4. Test results

- `cargo test --workspace --locked -j 2 --no-fail-fast`: only the known Linux failures
  (hsmp-launcher 7, hsmp-native 4, `claims_and_held_items_are_typed_records`).
- `cargo test -p hsmp-combat-sim --test combat_sim`: 8/8 with the shipped ceiling (the looser one
  failed `cheats_rejected`, above).
- `cargo clippy --workspace --all-targets --locked -j 2`: exit 0, no new warnings from these
  changes.
- `hsmp-gate g0 --quick --no-stamp`: lua_test reports `hsmpworld` (qobj golden vector 2) and
  `menu_ui` (autotest_mover write) failures that fail the same way on `acbc700`; everything else
  PASS / SKIP (no game dump).

## 5. Needs Windows / in-game

- The body record end to end (combat-parity.md §4), especially whether the game re-applies its own
  mass scales (the log counts it) and that the mass change does not upset the servo.
- The hit_vel_factor physics half: DCD capture per weapon class, `--hvf-logs`.
- Both gate fixes rest on the decompiled behaviour already modelled in damage_parity; the
  `Last Damage Taken` write-back must be checked against a real spread replay (HIT log lines).

## 6. Found, not fixed

- Replays that arrive in the reverse order of the attacker's clock (one held for a parry, the next
  not) are both applied: the weaker one first opens the gate for the stronger. Rare; fixing it needs
  a short reorder hold on the victim.
- `damage::replay_loss` / `trusted_loss` use the weakest piece of each armour layer and no part
  floors, so the god-mode bound can exceed the real replay under strong plate or many torso hits.
  Detection only (not enforced by default); combat.md §8 already says so.
- The server bounds Rigidity and Hit Velocity by the kit's weapon class; a picked-up arena weapon
  of a heavier class (legal) gets the kit class's caps. The pose stream's weapon class tag could
  give the server the held class.

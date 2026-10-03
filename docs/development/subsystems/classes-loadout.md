# Classes, gear selection and customisation

Players pick a class or a custom kit in the lobby. The server validates it against the match
rules and the catalogue, every client dresses its own pawn in the validated kit, and stand-ins
wear their owner's real, replicated appearance.

| Part | Where |
|---|---|
| Catalogue, kits, rules constants, a `check()` that mirrors the server | `mods/HSMPLoadout/Scripts/hsmp_catalog.lua` |
| Own-pawn apply and verify, stand-in filtering, weapons, cosmetics | `mods/HSMPLoadout/Scripts/kit.lua` |
| Appearance writer and stand-in applier, gear dump | `mods/HSMPLoadout/Scripts/main.lua` |
| LOADOUT / CLASS screen | `mods/HSMPMenu/Scripts/classes.lua` (layout: [menu-ui.md](menu-ui.md)) |
| Records | `crates/hsmp-ipc/src/schema/loadout.rs` |
| Server: catalogue mirror, validation, kit store, rules, replay, re-broadcast | `server/src/loadout.rs` |
| Sidecar: resend and ack | `server/src/loadout_client.rs` |

## What players get

- **Classes:** five preset kits built from real Half Sword items. Costs are balance points.

  | Class | Kit | Cost |
  |---|---|---|
  | KNIGHT | Armet, bevor, pauldrons, vambraces, gauntlets, arming doublet, cuirass, mail faulds, hosen, cuisses, greaves, shoes. Longsword III | 42 |
  | MAN-AT-ARMS (default) | Open sallet, mail standart, gambeson, mail hauberk, tabard, half gauntlets, hosen, shoes. Poleaxe (mid) | 20 |
  | DUELIST | Hat, arming doublet, half gauntlets, hosen, shoes. Arming Sword III + Buckler III | 11 |
  | BRUTE | Cap, gambeson, half gauntlets, trousers, shoes. Great Axe | 11 |
  | PEASANT | Tunic, hosen. Pitchfork | 1 |

- **Customisation:** each hand, plus 15 armour layers (head, bevor, mail collar, shoulders, arms,
  hands, body, mail shirt, chest plate, tabard, waist, legs, thighs, knees/shins, feet). Cosmetics
  are face (0–7), hair colour (8) and cloth tint (8). Index 0 always means "game default".
- **Rules (admin picks, lobby only):**
  - `FREE` (default): anything in the catalogue. The "GAME GEAR" tile keeps the arena's own
    equipment.
  - `CLASSES ONLY`: the kit must equal a class preset.
  - `CUSTOM (BUDGET)`: any catalogue items with a total cost ≤ budget. The menu offers
    12 / 20 / 30 / 45 / 60 (default 30); the server accepts 1–200.

  Every mode enforces one item per armour layer, shields in the left hand only, two-handed weapons
  in the right hand only, and an empty left hand with a two-handed weapon.

## Catalogue source (real game items)

Found in `pakchunk0` (see [../halfsword/pak-unlocked.md](../halfsword/pak-unlocked.md)) by parsing
the import tables of the game's own equipment data assets:

- **Armour, per layer:** `/Game/Blueprints/DataAssets/Equipment/Armor/Slots/DA_Equipment_Armor_Slot_{Head_2, Neck_1, Neck_2, Shoulder, Arms, Hand, Body_1..4, Waist, Leg_1..3, Foot}`.
  One UI row per data asset. The classes live under `/Game/Assets/Armor/Blueprints/Modular_Armor/`.
- **Weapons:** `/Game/Blueprints/DataAssets/Equipment/Weapons/PreMade/{Tiers,Types}/*`, which list
  premade weapons per tier and hand type (1H / 2H / Secondary / Shields). Added to these are the
  tiered `ModularWeaponBP_{ArmingSword,LongSword,BastardSword,Falchion_*,Dagger}_T1..3`,
  `GreatSword`, and `Tiers/ModularWeaponBP_{Mace,Hafted,Polearm}_*` in
  `/Game/Assets/Weapons/Blueprints/Built_Weapons/`.
- **Kits:** modelled on the game's `DA_Equipment_Loadout_Tier_{Beggar, Peasant, Commoner, Militia,
  Soldier, ManAtArms, Veteran, Knight}` (class `DA_Equipment_Loadout_Master_C`, holding
  `FStr_SubPassport_Equipment`). The Free Mode lists are `DA_FreeMode_Inventory_Tier_{Beggar,Knight}`.
- **Totals:** 244 items (116 armour, 128 weapons). Every path was checked against the pak file
  list.
- **Excluded unique items:** the Baron's gear (`*_Baron`, `ModularWeaponBP_BaronBeak`), the gold
  shields, treasure items, traps and ranged weapons.

`hsmp_catalog.lua` holds the paths, labels, costs and kits. The server keeps ids, groups, costs and
kits in `server/src/loadout.rs` (`catalog`). The test `catalog_matches_lua` fails if the two differ,
so edit both together. Item ids travel as strings (`Str<32>`), not catalogue indices, so the
catalogue can change between builds without a protocol bump and an unknown id is refused visibly.

## Data flow

All of it is typed records (protocol v6, kinds `0x05xx`, capability `LOADOUT_KIT`):

```
classes.lua (UI) ── SAVE ──► game slot `kit` {seq, class, r, l, cos[4], rows = {{id}}}
                                  │  sidecar: sent as it is; resent every 2 s until a
                                  │  `kit_verdict` for us acks its seq, then every 30 s
                                  ▼
                      hsmp-server (loadout.rs): validate against rules + catalogue
                      ├ valid    → verdict 0 (accepted), broadcast `kit_verdict`
                      ├ invalid  → class default (or the best class that fits), verdict 1 + reason
                      ├ no choice under enforced rules → default class, verdict 2
                      ├ join     → replay the rules + every live kit and loadout to the joiner
                      └ every 10 s → re-broadcast rules + kits (loss repair)
                                  ▼
            every sidecar: per-peer slot `peer_kit` (highest rev wins), slot `kit_rules`
                                  ▼
HSMPLoadout kit.lua: own pawn ← kit on every new pawn / round / arena load
                     stand-ins ← owner's real appearance (`peer_loadout`), filtered to the
                                 kit's armour under CLASSES / CUSTOM, or the kit itself until
                                 the appearance arrives
```

- **Appearance.** Every 2 s in an arena, HSMPLoadout's writer reads the local Willie's armour from
  every place the game keeps it, cross-checks it against `"Worn Armor"`, and puts one `loadout`
  record (both hand weapons' passports in the head, one row per armour piece or passport) into the
  game blob `loadout`, with a version that changes only when the gear does. The sidecar sends each
  version once (reliable; the transport fragments it). The server keeps the newest per peer and
  replays it to late joiners; receivers get it in the per-peer blob `peer_loadout`. The applier
  (every 1 s) dresses each stand-in from it.
- **Rules.** The admin's request goes into the game slot `kit_rules_req` (and as a `kit_rules` lobby
  command, which the server applies as a lobby config change; RCON `KIT` does the same). The sidecar sends the slot only while it is
  admin, resends every 2 s until the server's rules match, and gives up after 5 tries. The server
  checks admin and lobby itself: otherwise the sender gets a `kit rules refused: not admin` (or
  `match running`) chat line and the unchanged rules. A rules change re-validates every stored kit,
  so a player's own choice comes back when the rules allow it again. Each client applies the new
  kit at its next spawn or round reset, never mid-round. A dedicated server can start in an
  enforced mode with `HSMP_KIT_MODE=free|classes|custom` and `HSMP_KIT_BUDGET=<n>`.
- **Revisions.** `rev` is wall-clock-ms based and monotonic, so a restarted server's state still
  supersedes the old one.
- **Limits.** Ids ≤ 32 bytes, ≤ 16 armour items, 8 kit messages and 8 loadout versions per peer
  per second (more are dropped), out-of-range cosmetics reset to 0.
- **Persistence.** The selection lives in the `kit` slot for the game session and is resent on
  every connect or reconnect. It is not written to disk.

## How the gear is put on (HSMPLoadout)

- **Armour.** `kit.lua` resolves each item's class and reads its slot from the class default
  object's `"Armor Slot"` property (`BP_Armor_Master_C`; the name has spaces). The apply then
  dresses the pawn the way the game does. Source: the `Set Up Armor` bytecode, decompiled from
  `Willie_BP.uasset` with kismet-analyzer.
  - `"Set Up Armor"(Clear Previous, No Check Block)` spawns one armour actor per entry of the pawn's
    **own** `"Character Passport"`.`Equipment_26…`.`ArmorinSlots_5…`. That map is
    `ArmorSlots_Enum` → `Str_Passport_Armor1` (core class, modules, colours, slot).
  - It writes the pieces it accepts to `"Currently Equipped Armor"`, an output-only map cleared on
    every call. `ArmorSlots`, `ExternalArmorSlots` and `NewArmorSlots` are never read.
  - It only spawns a piece when `(slot == 16 or not "Spawn in Pants") and (slot in 12/15/16 or not
    "Blossfechten Gear")`. The arena spawns players and fresh foes with `"Spawn in Pants" = true`,
    which is why pawns otherwise stay in their hosen.
  - The apply, in four steps:
    1. Build one passport per kit piece. Templates come from the game's
       `DA_Equipment_Loadout_Tier_*` loadouts where one exists; otherwise the passport is core only,
       with modules 0 and steel type 11, as in those data assets. Non-catalogue base clothing the
       pawn was born with is kept.
    2. Write the passports into the pawn's character-passport map. Pieces that unlock arming points
       go first.
    3. Clear the per-pawn `"Spawn in Pants"` and `"Blossfechten Gear"` flags, then call
       `"Set Up Armor"(true, false)`.
    4. If the game's layering checks refused a piece, rebuild once with `No Check Block = true`.
  - Nothing touches the game instance, `SG_*` or the save. The passport and the flags live on the
    pawn, which the next round replaces.
- **Weapons.** The weapon actor is spawned from its class first, so it builds from its own
  Blueprint defaults. The actor and its own `"Weapon Passport"` are then passed to
  `"Set Up Right Hand Weapon"` / `"Set Up Left Hand Weapon"`; if that fails, the class-only form is
  tried. An empty hand in the kit strips the current weapon. A hand that holds a world item another
  peer picked up (the `world_held` bus key from HSMPWorld) gets no loadout weapon (see
  [world-replication.md](world-replication.md)).
- **Verification.** 3 s after an apply, `"Currently Equipped Armor"` (what `Set Up Armor` really
  spawned) must contain every kit class; the `"Worn Armor"` mesh count is the fallback check. The
  apply is retried up to 3 times. Ctrl+F7 dumps gear and forces a re-apply.
- **One dress per pawn.** The own kit is keyed on the pawn and the kit revision only. Keying it on
  the round as well re-dressed every pawn at the instant the round went Live. A new round is a new
  world and a new pawn, so nothing is lost.
- **Weapons must be visible, not just held.** A weapon actor spawned or re-armed while its pawn was
  hidden for dressing can stay hidden: `SetActorHiddenInGame` on the pawn does not reach attached
  actors. The verify step checks each kit hand's actor (`bHidden`, root component `IsVisible`); a
  hidden one is unhidden and counts as missing until the next verify. Every carried actor
  (`Weapon R/L`, `Weapon R_0/L_0`, `Weapon Slot R 1/2`, `L 1/2`, `Back`) of the own pawn and of
  every stand-in is unhidden.
- **The Director's evidence: the `kit_status` bus key**, written after every verify, when dressing
  starts, when a native re-arm undoes the kit, when the stability window holds and when HSMPLoadout
  gives up: `pawn, rev, ok, armour_n, exp_armour_n, r_class, l_class, r_visible, l_visible, tries,
  error, kit, stable, round, t`. `ok` is true only when every kit armour class is worn and both
  hands hold the kit classes, visibly. No kit selected writes `ok = true, kit = "none"`. The
  verdicts (verified, given up) also emit `kit_verified{who="self",...}` (hsmp_log). Re-arm rules
  for dropped kit weapons are in [spawns.md](spawns.md) §1.7.
- **Cosmetics:** hair through the `"Hair Mat"` scalar parameters `Melanin` / `Redness`; face
  through the `"Face Type"` property; tint through the fabric and leather colours of each kit
  passport (index 0 keeps the template colours).
- **Property names.** The Willie properties have spaces: `"Weapon R"`, `"Weapon L"`,
  `"Weapon R_0"`, `"Worn Armor"`, `"Worn Armor Items"`, `"Use External Armor Slots"`,
  `"Currently Equipped Armor"`, `"Armor Passports related to Armor Collision Meshes"`,
  `"Load Equipment"`, `"Setup Armor in Process"`, `"Armor Swap In Process"`, `"Weapon Slot R 1"`…,
  `"Carried Armor R Hand"`, `"Is Held"`, `"Parent Actor"`, `"Weapon Passport"`. CamelCase reads
  silently return nil. Reference: `../halfsword/willie_props.txt`.

## UI

The LOADOUT / CLASS screen sits next to START / WAITING in the lobby. It has class cards, grouped
slots and a paged item-picker grid; layout and widgets are in [menu-ui.md](menu-ui.md).

- **Actions:** SAVE (refused locally if the kit is invalid under the current rules), RESET TO
  CLASS, BACK (unsaved edits are dropped).
- **Locked editing:** in CLASSES ONLY mode, item rows show a message instead of changing.
- **Readout:** class (customised?), cost / budget, local validity and the server verdict for your
  kit.
- **Lobby rows** read `READY  |  KNIGHT`, where `+` means a customised kit and `GAME GEAR` the
  free-mode default (`Classes.peer_suffix(pid)`).

## Tests

- `cargo test -p hsmp-server kit` and `cargo test -p hsmp-server loadout`: classes valid in every
  mode, balance ordering, classes-only edit → preset, armour order ignored, budget enforcement,
  over-budget fallback → best-fitting class, unknown items and classes, slot and hand rules,
  GAME GEAR only in FREE, malformed input and cosmetic clamping, rules sanitising, store default /
  revalidate / rev behaviour, unique ids, **Lua ↔ Rust catalogue equality**.
- `cargo test -p hsmp-ipc loadout`: record layouts and checks.
- `hsmp-tools lua-test loadout` and `lua-test kit_status`: the Lua sides with the typed HSMPNative
  mock.

## In-game checks

1. After deploy, UE4SS.log shows `[HSMPMenu] [classes] catalogue: 244 items, 5 classes`.
2. HOST GAME, then **LOADOUT / CLASS** in the lobby: `[HSMPMenu] enter screen: classes (…
   widgets)`.
3. Pick DUELIST and SAVE: `[classes] saved kit seq=… class=duelist cost=11 -> kit slot`. The
   sidecar logs `kit sent` and `kit received`; the readout shows the server verdict.
4. As host, set RULES to CLASSES ONLY: `[classes] host rules request: mode=CLASSES ONLY
   budget=30`; the server logs `kit rules changed`. The armour rows lock.
5. BACK: your lobby row reads `… |  DUELIST`.
6. START. A few seconds after the arena loads: `[kit] own kit duelist ... applied: ...`, then
   `[kit] own kit duelist verified ...`. Visually: hat, arming doublet, half gauntlets, arming
   sword and buckler.
   - `no 'Armor Slot' default for …` or `UNRESOLVED=` means the class default object read failed.
   - `still missing after 3 tries` means `Set Up Armor` did not take the slot maps. Press Ctrl+F7
     and look at the `[gear]` dump.
7. A second client picks KNIGHT: on the host, that stand-in wears plate and a longsword.
8. Enforcement: CUSTOM with budget 30 and the joiner keeps KNIGHT. The server logs
   `kit replaced by class default ... reason=over budget (42 > 30)`, and at the next round the
   joiner wears the man-at-arms kit.

## Known limits

- **Weapons on stand-ins are not filtered** under enforced rules. Picking up arena weapons is
  legitimate gameplay, and hiding a weapon the owner really swings would desync combat. Armour is
  filtered to the kit.
- If `"Face Type"` only takes effect at character setup, a face change may not show until the game
  re-initialises the pawn.

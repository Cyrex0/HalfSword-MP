# Damage parity: offline proof and in-game checks

The design is in [combat.md](combat.md) (§5 client, §7 solo parity, §8 health). The offline proof is
`tools/hsmp-tools/lua-tests/damage_parity.lua`; run it with `hsmp-tools lua-test damage_parity`
(`hsmp-tools lua-test` with no argument runs every suite).

## 1. What the offline proof shows

The suite runs the real `HSMPCombat/Scripts/main.lua` against a model of the decompiled native path:
"Deal Complex Damage" (contact gate, armour layer) then "Get Damage" (Invulnerable gate, DRS, contact
gate, impulse gate, Health with the per-part floor, bleeding, consciousness, pain, part health,
blood). As in UE4SS, every hook callback runs after the function body.

| # | Claim | Check |
|---|---|---|
| 1 | Blunt, cut, stab and fist blows on unarmoured, padded, mail and plate victims: the MP change of Health, Consciousness, Bleeding, Pain, part Health and blood marks equals solo, field by field (the table is printed). | `every blow x armour` |
| 2 | One blow = one claim, and the Invulnerable stand-in takes nothing. | `one blow = exactly one claim`, `takes nothing` |
| 3 | The stand-in's copy of the blow lands on the victim first (echo): it is undone, its contact gates do not swallow the replay, echo + replay == solo, and it is reported as a touch. | `echo + replay == solo` |
| 4 | A rejected blow leaves the victim exactly as it was; stand-in weapons get the game's own damage gate. | `a rejected blow` |
| 5 | Other screens see the accepted blow's blood on the stand-in, whose vitals stay the owner's. | `blood appears on the stand-in` |

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

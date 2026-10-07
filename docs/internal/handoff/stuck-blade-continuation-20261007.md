# Stuck-blade continuation evidence and contract (2026-10-07)

Source fixes and offline verification only. No paired native/replay live run has
verified these changes yet. Read alongside `claude-beta6-20261007.md` section 4.2.

## Measured inputs

`test-results/beta6/inside-2/server.log` has seven continuation rejections at
lines 127, 129, 283, 285–288. Their deployed reason does **not** contain the
parent/call bone pair; the more detailed reason in source was added afterward.
Do not claim that this run proves the second group's bone pair.

The exact constraint named `Constraint_Weapon_Stuck_BP_C_2147481160` starts on
`lowerarm_l` in `inside-2/UE4SS.log:1694`, continues on that bone at 1695–1707,
then calls Inside damage on `hand_l` at 1709 and 1711. Both calls still identify
that same constraint, weapon, module, victim and original life. This is evidence
for the directional `lowerarm_l -> hand_l` exception only. Neither the reverse
mapping nor the other hand was measured here.

The local cooked export `test-results/dev-feature-checks/stuck-weapon-native.txt`
explains this change at native offsets 36387–36564: read the first entry of
`Overlapping Bones`, assign it to `Bone Name 2`, call `SetConstrainedComponents`,
then `Renew Costrained Bone`. It is a native constraint rebind, not a tolerance.
The existing `pelvis -> spine_02` construction remap remains supported.

Five logged initial Inside calls have no claimed parent (UE4SS.log:1696, 1752,
2045, 2216, 2402). At least the calls at 1696, 2045 and 2402 follow constraint
BeginPlay immediately, before the Lua claim flush can assign a cid. Other contacts
explicitly fail exact cutting-box representation; that gap is still a rejection,
never a fabricated source or box.

## Current continuation contract

- Native BeginPlay binds the newest original DCD record using exact source and
  target component addresses, source hand/module ordinal, victim name, matching
  initial/remapped bone, and callback age at most 50 ms. Records are captured
  before native-gate filtering and per-bone selection. Constraint parents survive
  that selection. Cross-world state is discarded without touching UObjects.
- A suppressed DCD origin uses existing flags `COMPLEX|INSIDE|WEAPON`, with
  `parent_cid=0`. It carries the complete original contact geometry and source
  identity and must have nonzero original match/round/lives. Full geometry,
  source-class and clash checks still apply. The server zeroes its damage booking;
  the owner acknowledges it without DCD/Get Damage, and cosmetic replay skips it.
  This represents a real native constraint origin without inventing another blow.
- The native initial Inside call may precede flush. Its plain numeric/string
  record waits behind that exact origin until the origin has a cid. No cached
  component wrappers are retained for delayed sending.
- Inside continuations (`INSIDE|WEAPON`, no `COMPLEX`, nonzero `parent_cid`) carry
  the original source class, hand and native module ordinal. The schema permits
  these fields only for a bound Inside call; cutting-box, foot and DCD-only bits
  remain prohibited. The server requires the component identity and class to
  match the accepted parent; a weapon continuation cannot strip its identity and
  downgrade to a component-free call. Legacy continuations remain valid only for
  parents without weapon identity. The owner resolves the exact module afresh as
  `Hit By Component`; missing sources fail before any native damage call.
- Native normals, impulses and velocities are captured rather than replaced
  with made-up vectors. Raw/cut/draw/pain limits, victim/life checks, constraint
  rate/lifetime caps and unrelated bone checks remain unchanged.
- A continuation approved during defender grace is checked again before deferred
  delivery or a cached retransmission. A subsequently parried/rejected parent
  invalidates the cached constraint authorization and rejects its children.

The schema/layout size is unchanged. The native module, sidecar and server must
still be rebuilt/deployed together because the validation contract changed.

## Lab log accounting

First decisions emit structured tracing lines with these exact phrases:

| Phrase | Fields | Meaning |
| --- | --- | --- |
| `stuck blade accepted` | `attacker`, `target`, `hit_id`, `parent_cid`, `bone`, `source`, `class`, `stage="decision"` | First continuation approval; it may still wait behind its parent. |
| `stuck blade rejected` | same identity fields except class, plus `reason`, `stage="decision"` | First refused continuation. |
| `stuck blade rejected` | identity fields, `reason`, `stage="delivery"` | A previously approved continuation lost its parent before delivery. |
| `stuck blade delivered` | identity fields, `stage="delivery"` | Deferred continuation released after its accepted parent. |

Count distinct `(attacker, hit_id)` decisions, then apply delivery-stage rejection
as invalidation. A delivered line is not another acceptance. Ignored claim floods
and cached rejected retransmissions do not emit additional decision lines.
Normal server `damage accepted`/`damage rejected` lines still describe final relay
outcomes. Module source bits use the existing source mask/ordinal contract.

## Offline checks and remaining native work

Focused verification at the continuation change: all 44 server combat tests; all nine IPC
combat-schema tests; Lua combat and damage_parity 28 checks (the existing
420 solo/MP oracle blows remain identical). These are offline checks, not a claim
of live damage parity.

The cooked export proves that all three special events are Inside Get Damage:

| Offset | Bone | Raw | Cut | Lower threshold | Other fixed inputs |
| --- | --- | --- | --- | --- | --- |
| 41353 | `head` | 1000 | 1000 | false | zero velocity/impulse, pain 1, draw 0, actual weapon module, no box |
| 41728 | current `Bone Name 2` | `Bone Mass * 50` | 100 | false | same fixed inputs |
| 43210 | current `Bone Name 2` | native force/bone-health expression | 100 | true | same fixed inputs |

The current hook observes these Inside calls, but this run does not prove their
live delivery. A head trace may hit head while the constraint is bound elsewhere;
that case still fails strict constraint/bone association. No blanket head alias
was added. Other overlapping-bone transitions need exact paired evidence or an
explicit authenticated rebind record before admission.

`Dismemberment Check` is a separate native structural route: it changes HP marker
tags using damage, draw cut and gore rate, then may call dismemberment. It is not
equivalent to Inside Get Damage and remains unforwarded. The continuation's own
constraint Box is also not replayed as a weapon cutting Box; exact historical
representation and side effects still need proof. Next live acceptance target is
paired native/replay Health with continuations enabled, plus initial/orphaned,
rebound and parried-parent counts from the same session.
## Exact probe pairing

Under the dev `combat_probe` switch, successful ordinary and Inside sends emit:

```
LAB_PROBE attacker=9 cid=101 parent_cid=0 bone=head dmg Health -2.000000 [Health-2.0 ...]
LAB_PROBE attacker=9 cid=102 parent_cid=101 bone=head dmg Health -3.000000 [Health-3.0 ...]
LAB_REPLAY attacker=9 cid=102 parent_cid=101 bone=head dmg Health -3.000000 [Health-3.0 ...]
```

Pair `(attacker,cid)` with the victim's `HIT from peer <attacker> (#<cid>)` or
the exact owner outcome record. Parent cid identifies continuations; zero identifies
the ordinary origin. Capture occurs in POST Get Damage before the stand-in backstop
restores its saved baseline. The ordinary DCD consumes the original sample by exact
target/bone/source module; Inside captures its own callback and queues only plain
data if its parent cid has not been assigned yet. Successful sending assigns the
actual cid before logging. The diagnostic data never changes the transport schema.

`LAB_REPLAY` is emitted only for a fresh native owner attempt under the dev probe
switch. It uses the received `damage_in.hit_id` as cid, the original parent cid,
and the six-decimal native `outcome.health_delta` when observed-fields bit zero
records Health. The existing result's bracketed changed fields are retained.
Cached attempts emit no second sample. If Health was not observed, the line says
`dmg Health unavailable [native Health not observed]`; do not infer zero from it.

Missing baseline, Health or original callback association emits `dmg Health unavailable`
with an explicit bracketed reason. Exclude those rows from ratios. Two changes below
the backstop's 0.5 threshold are measured against successive actual post-callback
states within the tick, so the second cannot count the first change again. The native
hooks are POST-only: this measures against saved observed state, not an invented
pre-hook snapshot; native changes between observations remain a live evidence caveat.

## Speculative wound suppression and owner outcomes

Native Willie property `Force Disable Vertex Paint` is proven in `willie_props.txt:1319`.
DCD offsets 4356 and 9789 gate armour bruise/cut painting independently of Get Damage's
Invulnerable gate. Get Damage offsets 8188, 15134 and 24621 gate skin/worn-armour paint.
The flag is set when Avatars claims a stand-in, then reasserted by Combat protection;
damage probes can still enable native injury sampling. A previously gated pawn
becoming the confirmed local pawn regains the normal native paint flag.

`dispatch_damage` no longer broadcasts cosmetics on geometric forward or transport
ACK. A fresh authenticated owner `REPLAY_CHANGED` outcome with actual injury/vital
fields authorizes the exact approved ordinary DCD record. Gate bookkeeping fields
15–17 alone do not authorize paint; a meaningful Pain/CON/bleeding change can do so
without main Health loss. Duplicate, refused, stale and suppressed-origin receipts
cannot generate a wound broadcast. Original match/round/both lives are checked again
at publication and cosmetic replay.

`C3.apply_fx` temporarily opens native paint only for this approved cosmetic call,
then restores the guard on success, native failure or argument failure. A world-guard
drop during the call discards old wrappers without restoring through them. Full owner
vertex-color transport and paint-only native changes with no sampled injury remain
unimplemented; this is owner-outcome-authorized approved-input replay, not a claim
that the stand-in's complete gore state now equals the owner's.

The follow-up has 45 server combat tests and three combat-glue tests passing;
Avatars/Combat/parity suites pass together (470 checks on the latest run: 153/289/28). Probe
tests cover exact queued cids, callback counts, unavailable measurement, tiny-change
accounting and unchanged damage behavior; paint tests cover native errors, argument
errors, world drop, probe sampling and local-pawn guard provenance.

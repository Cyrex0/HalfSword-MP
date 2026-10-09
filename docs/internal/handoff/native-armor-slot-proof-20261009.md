# Native armour map keys and passport slots (2026-10-09)

The actual `5339682b` run completed both 49-component harvests at 39.031 s,
then native binding refused `armor slot binding` at 39.035 s. Its source events
are under `test-results/20261009-133011-adbf6a-native-host/host/96ce45e0-0c7b-464f-bdc9-991de1e44e01/worker/hsmp_events.jsonl`.
That record did not include the rejected row or encoded recipe size. It proves
neither a particular live-armour mismatch nor acceptance of the 512KiB budget.

## Primary native and cooked evidence

The matched shipping image SHA256 is
`367DFCCF1AACA3BBF6824C9BB616F2F31BC30E7AC70C5FA8657E212DBB2C2E03`.
Paths below are local primary exports/dumps, not invented source defaults.

- `game/HalfswordUE5/Binaries/Win64/ue4ss/CXXHeaderDump/Str_SubPassport_Equipment.hpp:6`
  declares `TMap<TEnumAsByte<ArmorSlots_Enum::Type>, FStr_Passport_Armor1>`.
  The native map key and its struct value are separate fields.
- `CXXHeaderDump/Str_Passport_Armor1.hpp:22` declares the passport `Slot` as an
  enum byte at `0x78`. The same struct contains all 24 captured native fields,
  including class, modules, colours, protection requirements and blocked slots.
- `test-results/dev-feature-checks/inventory-harvest-20261008/exports/9cbb81fd71c6fc6404b84052-Str_Passport_Armor1.json:315,361`
  records cooked struct defaults `ArmorCore=null` and
  `Slot=ArmorSlots_Enum::NewEnumerator0`. An empty value does not acquire a slot
  from an enclosing map key. These cooked values are evidence, not substitutes
  for a missing live observation.
- `test-results/dev-feature-checks/willie-armour-initialization-functions.txt:443-454`
  records native Blueprint offsets `10311 Map_Values(construction Armour map)`
  and `10650-10688 IsValidClass(ArmorCore)/skip`. Construction uses the values;
  it does not impose equality between each map key and its passport Slot.
- The same file line532, Blueprint offset `13479`, adds live armour as
  `Map_Add(Currently Equipped Armor, spawned actor.Armor Slot, Current Armor Passport)`.
  The live key comes from the actor. This call alone does not establish the
  complete assignment relationship between actor Armor Slot and passport Slot.
- `CXXHeaderDump/Willie_BP.hpp:1157` independently confirms the actual live
  `Currently Equipped Armor` property is the same enum-to-passport native TMap.

## Corrected representation and remaining boundary

Construction retains every native map occurrence, its exact key, and all copied
passport fields, including independent `pslot` and explicit null class. Ordered,
unique, bounded native enum keys and every passport field remain validated.
No row is dropped, populated from a default, or rewritten to match another field.

The live validator retains its existing non-null class and key/pslot consistency
gate pending complete native dataflow proof. A refusal now identifies the copied
construction/live table, row, slot, pslot and class rather than guessing its cause.
Source capture still performs two equal complete harvests and rejects mutation
even when the mutated armour row has a null class.

Diagnostic encoding statistics operate only on copied typed values and the same
bounded codec plan. They can report planned compact/JSON bytes before semantic
validation, but do not authorize publication, decoding, native binding or client
readiness for an invalid recipe. No native getter or recipe contents are emitted.

The production Lua copy/signature node bound now matches the 512KiB token-derived
Rust bound; depth24 and string512 limits remain. A complete offline 64x512-bone
fixture verifies every occurrence and tail mutation without claiming native
runtime acceptance of that synthetic shape.

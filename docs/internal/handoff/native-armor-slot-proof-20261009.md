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
- `CXXHeaderDump/ArmorSlots_Enum_enums.hpp:5,17` and the matching cooked enum
  export establish reordered labels: NewEnumerator0 is numeric2 and
  NewEnumerator2 is numeric16. Label suffixes must not be parsed as slot numbers.
  The null-class default above has native slot2; fixture slot0 is explicitly
  synthetic and never substituted into an actual source passport.
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

## Actual live Doublet Arming proof

Run `20261009-140709-303c3a-native-host`, exact build `0b9301bd`, source UUID
`a9899fb6-d2db-434b-8acd-ce76714274fd`, captured both complete harvests at
40.238 s. Native binding at40.244 s reported live row0 key12/passport slot2 and
exact class `/Game/Assets/Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Body_Doublet_Arming.BP_Armor_Modular_Core_Body_Doublet_Arming_C`.

- Cooked export `5d9aeee9db4e83ace9885e20-BP_Armor_Modular_Core_Body_Doublet_Arming.json`
  under the same inventory export directory is complete8/8. Its CDO line111 has
  actor `Armor Slot=NewEnumerator1`. Its25-statement UserConstructionScript
  changes team colours, calls the exact core parent at offset1000, then returns;
  it does not rewrite actor or passport slots.
- Pinned `ArmorSlots_Enum_enums.hpp:5,15` and cooked enum export
  `d290dad06c5f8d7454b71ee1-ArmorSlots_Enum.json` map **NewEnumerator1=12** and
  **NewEnumerator0=2**. Symbolic suffixes are not native numeric values;
  NewEnumerator2 is numeric16, so the previously audited pants16 path matches.
- Exact base `BP_Armor_Master` cooked CDO contains passport Slot NewEnumerator0
  (=2). Core BeginPlay ubergraph offset1882 preserves the existing passport Slot
  in its rebuilt struct;2184 stores it. All serialized base/core functions contain
  no actor-slot normalization. Native Map_Add13479 therefore preserves the
  independent actor key12 and Current Armor Passport slot2 observed in this run.
- Source `native_source_fields.lua:33` reads the exact native Slot field as byte.
  Pinned UE4SS `LuaUObject.cpp:513-515` calls `push_integer<uint8_t>`;
  `LuaUObject.hpp:975-987` returns the raw byte. It does not parse an enum name
  or convert a symbolic suffix. The DTO/parser independently preserve both u8s.

## Corrected representation and remaining boundary

Construction retains every native map occurrence, its exact key, and all copied
passport fields, including independent `pslot` and explicit null class. Ordered,
unique, bounded native enum keys and every passport field remain validated.
No row is dropped, populated from a default, or rewritten to match another field.

Live armour also preserves its independent map key and passport slot. The
class-valid native spawn path still requires a populated valid class; ordered,
unique, bounded keys and every passport field remain checked. No actor key is
rewritten from pslot, and no pslot is rewritten from the actor key. Source
component/actor mapping and complete native geometry validation remain separate
requirements. Refusals retain copied table/row/slot/pslot/class context. Two equal
complete harvests still reject changes to either observation or any other field.

The earlier pants-only live audit initially retained the equality gate. Willie offset9449 selects the exact
Modular_Armor Panties class, and10064 creates its passport with numeric slot16.
The complete8/8 child export has no functions and actor Armor Slot
NewEnumerator2 (numeric16); its complete core/base function sets preserve the
passport Slot without writing actor Armor Slot. The live Add therefore uses
key16/passport16. Set Up Armor5524 clears the live map, and13479 adds only
class-valid passports. No seeded empty live row or supported mismatch was
established by that earlier, limited audit. Its primary references and historical
conclusion remain in ignored `test-results/native-live-armor-proof-20261009.md`;
the later actual Doublet proof above establishes independent live slot fields
and supersedes that equality conclusion.

Diagnostic encoding statistics operate only on copied typed values and the same
bounded codec plan. They can report planned compact/JSON bytes before semantic
validation, but do not authorize publication, decoding, native binding or client
readiness for an invalid recipe. No native getter or recipe contents are emitted.
The actual Doublet refusal measured35316 planned compact bytes,74310 JSON bytes,
6556 nodes,8771 dictionary bytes,26545 token bytes,49 components,600 bones and38
material slots for the first entity. These are measured copied-data sizes; native
registration, canonical frames, visible client meshes and input acceptance were
still unproved in that run.

The production Lua copy/signature node bound now matches the 512KiB token-derived
Rust bound; depth24 and string512 limits remain. A complete offline 64x512-bone
fixture verifies every occurrence and tail mutation without claiming native
runtime acceptance of that synthetic shape.

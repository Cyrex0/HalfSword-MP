# Native dynamic-material source proof

## Actual blocking observation

Exact deployed ddccc175 run `test-results/20261009-150349-520cf5-native-host`
uses authority15264/source UUIDbf4912c5-2109-41d3-9cd3-7f3c2e6278eb.
NativeBind40.194s rejects body component7 CharacterMesh0, material0/slot0,
field=base. The copied path is
`/Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard:PersistentLevel.Willie_BP_C_2147482004.CharacterMesh0.MID_M_Body_Inst_Frank_21`.
It is a runtime object path rather than an admitted immutable asset path.
Copied parameter counts are0; no accepted recipe or mirror exists.

The old collector traverses parent/override arrays only while the current
material has RF_Transient. Its final path can therefore remain runtime-owned.
The diagnostic `asset_transient=false` means the copied string does not contain
"Transient"; that diagnostic is not a native RF_Transient observation.

## Pinned native and cooked facts

- `game/HalfswordUE5/Binaries/Win64/ue4ss/CXXHeaderDump/Engine.hpp:19230`
  declares UMaterialInstanceDynamic as a UMaterialInstance subclass.
- The same header at21643 declares PrimitiveComponent.CreateDynamicMaterialInstance
  returning UMaterialInstanceDynamic. ObjectDump2214–2218 independently records
  ElementIndex int32, hard SourceMaterial, OptionalName and hard ReturnValue.
- `CXXHeaderDump/Engine_enums.hpp:1831` declares EMIDCreationFlags None=0 and
  Transient=1. Dynamic type and the optional transient flag are separate facts.
- `test-results/dev-feature-checks/willie-native-all-functions.txt:5653`
  instruction38499 creates Mesh material0 from the actual GetMaterial result.
  File line5654/instruction38572 stores the result in Body Material.
- File line5351/instruction27356 sets Body Material's Muscles scalar;
  line28088/instruction20537 sets a body-hide scalar selected by native enum.
  These live material parameters cannot be omitted.
- File line28650/instruction43798 creates an armour material with CreationFlags0;
  lines28651–28652 assign it and call Setup Armor Material. A transient flag
  is not a valid discriminator for all native dynamic body/armour materials.

Instruction offsets above are cooked bytecode offsets, not text line numbers.
Native asset paths, class, RF flags and every parameter value remain observed
data. Names alone are not the dynamic-type test.

## Required correction and limits

Qualify the current native UMaterialInstanceDynamic type independently of
RF_Transient. Copy every supported scalar/vector/texture override layer,
retaining child-first native parameter precedence and exact parameter info.
Resolve the original immutable base through its hard Parent links. Keep
bounded depth, cycle/original component-slot/parent identity checks and source
scope guards. Unsupported parameter types/base overrides remain explicit
refusals. Do not admit a map-owned path or replace the material with a default.

Independent review identifies a separate borrowed-row lifetime requirement.
Pinned `mod/RE-UE4SS/UE4SS/src/LuaType/LuaTArray.cpp:136` caches array Data
and Num for ForEach before invoking Lua callbacks. Same-material reallocation
can invalidate rows without changing material identity. Header helpers79–109
directly read array address/Data/Num/Max. Reacquire the current original material
field and compare its complete original header after callback-capable guards,
before dereferencing any old row/ParameterInfo/colour wrapper. A header change
must unwind immediately. Copied FName identity uses full native eight-byte
equality, including Number, rather than ComparisonIndex alone.

Implemented correction passes focused Lua source277 assertions/syntax2 and
independent Sol review. Reallocation regressions cover original header address,
Data, Num and Max changes and assert zero stale row/ParameterInfo reads. Nested
MID overrides, RF-clear dynamic type, parent cycles, missing immutable base,
full FName Number replacement and slot/parent changes are covered. No schema,
native ABI or source value changes; actual capture/timing/mesh proof remains open.

This proof supports the collector correction. It does not establish actual
class/flag/layer counts in the next corrected run, accepted full recipes,
client body/gear meshes, view ownership, wounds or cut parity. Those remain
required native observations.

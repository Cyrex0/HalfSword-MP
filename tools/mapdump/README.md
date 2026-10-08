# Offline inventory harvest

Run from the repository root with the installed .NET 10 SDK:

    .\tools\mapdump\inventory.ps1

Use -SkipBuild -Dotnet <runtime-path> with an already-built parser, or -SelfTest
for the small offline ledger/closure/provenance regressions. The reader uses the
existing game, Oodle and mapping configuration. Authentication stays in parser
initialization and is omitted from arguments and outputs. Nothing launches or
writes the game.

To regenerate only the derived catalogue from an existing harvest, pass
--inventory-catalogue <harvest-directory> to MapDump.dll. This reads the saved
class and enum evidence and does not initialize the game package reader.

Results default to ignored test-results/dev-feature-checks/inventory-harvest/:

- coverage-manifest.json accounts for every mounted asset/map as success,
  failure or explicit skip, and separates serialized object references from
  weak name-table candidates.
- canonical-inventory.json supplies gear class families, readable passport
  fields, serialized/inherited stat values, source identities, and availability
  counts. Ambiguous distinct fields remain unavailable with every candidate.
  Native AnimInstance classes stay visible as separate animation evidence rows
  and are excluded from gear/armor totals.
- class-cdo-overrides.json, class-ancestry.json, and native-enums.json preserve
  original class overrides, resolved archetype layers, superclass references,
  and actual enum identity. Enumerator suffixes are never numbers.
- exports/ holds per-package export arrays with native expected and serialized
  counts, parse errors, component templates and collision references.

Game-class inheritance and native engine defaults have separate availability.
Missing fields never become zero. Blueprint construction is not executed:
protection, density, mass, materials and module choices can change at runtime.
Raw SCS/BodySetup properties are retained; decoded complex collision triangles
and cooked physics payloads remain explicitly unavailable. Static reachability
does not prove that every module combination is valid or has run in the game.

Exit code 2 means accounting finished with extraction failures. Inspect those
rows rather than treating package presence as healthy/full coverage. The mount
scope is the existing top-level game Paks reader; LogicMods and shadowed archive
entries are separately described in the manifest.

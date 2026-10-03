# SDK, header and object dumps

How the engine version was identified and how the UE4SS dumps that the rest of these notes rely on
were produced. Which UE4SS build HSMP pins, and why, is in [../ue4ss.md](../ue4ss.md).

## Engine identification

The USMAP file that UE4SS dumps is named after the engine build:

```
HalfswordUE5-5.4.4-2705+++Halfsword+Main-e3ba1016.usmap
             └─────┘ └──┘└──────────────┘ └──────┘
             UE ver   CL   engine branch   UE4SS commit
```

- **Engine:** UE 5.4.4
- **Changelist:** 2705
- **Branch:** `+++Halfsword+Main`, the developers' own engine branch
- The game's own crash reports agree (`WER.Process.Version: 5.4.4.0`).
- The `HalfSwordTrainerMod` README says UE 5.1. That is out of date.

## UE4SS on Half Sword

- The plain UE4SS 3.0.1 release does not work: its bundled patterns are from the UE 5.1 era and the
  FText scan fails. An experimental build (git SHA `e3ba1016`) does.
- That build uses the split layout: `HalfswordUE5/Binaries/Win64/dwmapi.dll` is the proxy and
  everything else is under `HalfswordUE5/Binaries/Win64/ue4ss/`.
- Settings that matter in `ue4ss/UE4SS-settings.ini`:
  - `[EngineVersionOverride]` `MajorVersion=5`, `MinorVersion=4`
  - `[Debug]` `GraphicsAPI = dx11` (the game renders with D3D12; the DX11 overlay is the one that works)
  - `[Debug]` `ConsoleEnabled=1`, `GuiConsoleEnabled=1`, `GuiConsoleVisible=1` for development.
    The launcher turns the console off for players.

## The dumper mod

`mods/dev/HSMPDump/Scripts/main.lua` runs UE4SS's built-in dumpers once, about 8 seconds after it
loads, so the object array is populated:

| Call | Output (under `Win64/ue4ss/`) |
|---|---|
| `GenerateUHTCompatibleHeaders()` | `UHTHeaderDump/<Package>/Classes/*.h`: readable UCLASS / UPROPERTY headers |
| `GenerateSDK()` | `CXXHeaderDump/<Package>/*.hpp`: C++ reflection headers in the `RC::Unreal` namespace |
| `DumpAllObjects()` | `UE4SS_ObjectDump.txt`: every live UObject at that moment (about 36 MB) |
| `DumpUSMAP()` | the `.usmap` mappings file, needed by asset tools such as kismet-analyzer and CUE4Parse |

To use it, copy the folder to `Win64/ue4ss/Mods/HSMPDump/` and add `HSMPDump : 1` to `mods.txt`
before the `Keybinds` line. It schedules the dump with `ExecuteWithDelay`, which runs off the game
thread, so run it on its own, without the HSMP mods, and remove it afterwards. The dumps are
large and are never committed.

The object dump is where the real names of Blueprint functions and properties come from (with
their spaces and GUID suffixes; see [README.md](README.md)). `hsmp-tools check-bp-names` checks the
mods against it.

## Useful paths from the dump

| Path | What it is |
|---|---|
| `/Game/Character/Blueprints/Willie_BP` | the character class: the player and every fighter |
| `/Game/Character/Skeleton/Willie_Skeleton` | its skeleton |
| `/Game/Character/Skeleton/Physic_Asset/Parts/Willie_PhysicsAsset_spine_05` | per-bone physics asset (ragdoll setup) |
| `/Game/Blueprints/GameLogic/Inventory/BP_PlayerInventory` | inventory controller |
| `/Game/Blueprints/Generators/BP_Generator_Characters_Random` | random character generator |
| `/Game/Assets/Weapons/Blueprints/Built_Weapons/Tools/BP_Weapon_Tool_*` | tool weapons: axe, hammer, knife, scythe, pitchfork, scissors, sickle, hoe |
| `/Game/Assets/Weapons/Blueprints/Built_Weapons/Treasure/BP_Weapon_Treasure_*` | treasure items |

The game's own native modules, as listed in the header dump, are `HalfSwordUE5` and
`HSComputeShaders`.

The pak is encrypted (`bEncryptedIndex=1`), so reading cooked assets needs the AES key:
[pak-unlocked.md](pak-unlocked.md).

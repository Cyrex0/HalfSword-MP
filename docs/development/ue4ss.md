# UE4SS in HSMP

HSMP's game side runs as Lua mods and one C++ mod (HSMPNative) inside
[RE-UE4SS](https://github.com/UE4SS-RE/RE-UE4SS) (UE4SS), the Unreal Engine scripting system.
This page records exactly which build HSMP uses, how it is pinned, how to get it, why HSMP does
not build it, and how HSMPNative builds against it anyway.

## 1. The build

| | |
|---|---|
| Project | RE-UE4SS, <https://github.com/UE4SS-RE/RE-UE4SS> |
| Build | the upstream **experimental** release build at commit **`e3ba1016`** |
| Self-reported version | `v3.0.1 Beta #0 - Git SHA #e3ba1016` (first lines of `UE4SS.log`) |
| `release.json` label | `RE-UE4SS experimental e3ba1016 (self-reports v3.0.1 Beta), dwmapi proxy, ue4ss/ layout` |
| Layout | the new split layout: `Binaries\Win64\dwmapi.dll` (proxy) + `Binaries\Win64\ue4ss\` (everything else) |
| Licence | MIT, Copyright (c) 2022 Narknon; shipped as `ue4ss/LICENSE` |

**Identify the build by its commit and its SHA-256 pins, never by a version string.** The binary
calls itself 3.0.1, and it is not a tagged upstream release. The pinned hashes match the upstream
experimental zip byte for byte: HSMP redistributes upstream's own binaries, not a private build.
The `release.json` label says exactly that.

The plain UE4SS 3.0.1 release does **not** work with Half Sword (UE 5.4.4): its bundled patterns
are from the UE 5.1 era and the FText scan fails
([halfsword/sdk-dump-notes.md](halfsword/sdk-dump-notes.md)).

## 2. The pins

From `tools/release/release.json` (`ue4ss`):

| File | SHA-256 |
|---|---|
| `ue4ss/UE4SS.dll` (`dll_sha256`) | `680a026890abb4d0df2211251f8defc1681a584275f1521dcc0fe30af480006f` |
| `dwmapi.dll` (`proxy_sha256`) | `cf440b9eb8643bb7c434acfda696aee57fd981d185dca5e57fb8dbb18f8fc1cd` |

`hsmp-release build` hashes both files in `--ue4ss-dir` and refuses to build if either differs
("Use the pinned build, or update the pin deliberately"). The pins also travel in the signed
release manifest (`ue4ss` section), so the launcher knows which build it installed.

Only these two files are pinned. The other files in the include list (§3) are taken from the
same `--ue4ss-dir` as they are and then hashed into the signed manifest like every packaged file.
Make sure the folder is a clean extraction of the same upstream zip.

## 3. What the launcher installs

The release packages exactly the `include` list of `release.json`, paths relative to
`HalfswordUE5\Binaries\Win64` (a trailing `/` takes the folder recursively):

```
dwmapi.dll
ue4ss/UE4SS.dll
ue4ss/UE4SS-settings.ini
ue4ss/LICENSE
ue4ss/Mods/BPModLoaderMod/
ue4ss/Mods/BPML_GenericFunctions/
ue4ss/Mods/shared/Types.lua
ue4ss/Mods/shared/UEHelpers/
```

Nothing else from the UE4SS folder ships: no PDB, no dumps, no SDK, no developer mods.
`mods/mods.release.txt` keeps `BPModLoaderMod` and `BPML_GenericFunctions` on and every UE4SS
developer mod (console enabler and commands, cheat manager, Kismet debugger, event viewer, line
trace, actor dumper, split screen, Lua profiler) off.

`UE4SS-settings.ini` is packaged with the `settings_overrides` of `release.json` applied:

| Section | Key | Value | Why |
|---|---|---|---|
| `General` | `EnableHotReloadSystem` | `0` | no hot reload for players |
| `General` | `EnableAutoReloadingLuaMods` | `0` | same |
| `Debug` | `ConsoleEnabled` | `0` | no UE4SS console for players |
| `Debug` | `GuiConsoleEnabled` | `0` | same |
| `Debug` | `GuiConsoleVisible` | `0` | same |
| `Debug` | `GraphicsAPI` | `dx11` | the API UE4SS's overlay uses on Half Sword |
| `CrashDump` | `EnableDumping` | `1` | crash dumps for triage (`hsmp-tools crash-triage`) |
| `CrashDump` | `FullMemoryDump` | `0` | small dumps only |

The launcher refuses to install over an **old-layout** UE4SS (`UE4SS.dll` directly in `Win64`),
which would load instead of the pinned build. If UE4SS was already installed for other mods, the
launcher backs up every file it replaces and restores it on uninstall
([releasing.md](releasing.md) §6).

A developer install made with `scripts/build-and-deploy.ps1` does not touch
`UE4SS-settings.ini`. For development you typically want the consoles on and, for the object
dump, `[EngineVersionOverride] MajorVersion=5, MinorVersion=4`
([halfsword/sdk-dump-notes.md](halfsword/sdk-dump-notes.md)).

## 4. Getting the build (a maintainer without the original machine)

The build comes from the upstream RE-UE4SS **experimental** GitHub release.

> **TODO:** record the exact upstream release URL, tag and asset file name whose contents match
> the pins. `docs/development/halfsword/sdk-dump-notes.md` names
> `zDEV-UE4SS_v3.0.1-1152-ge3ba1016.zip` from the `experimental-latest` release tag, but that
> tag is moved by upstream, so it is not a stable reference. Once confirmed, also record the
> zip's own SHA-256 here.

1. Download the upstream experimental zip built from commit `e3ba1016`.
2. Extract it into an empty folder so that the folder looks like a `Win64` folder:
   `dwmapi.dll` next to a `ue4ss\` folder that holds `UE4SS.dll`, `UE4SS-settings.ini`,
   `LICENSE` and `Mods\`.
3. Check the two pins:

   ```powershell
   Get-FileHash -Algorithm SHA256 .\dwmapi.dll, .\ue4ss\UE4SS.dll
   ```

   They must equal the values in §2 (PowerShell prints upper case; compare case-insensitively).
   If they differ, it is not the pinned build: do not "fix" the pin to match a download.
4. Check that every `include` entry of §3 exists in that folder.
5. Build the release with `--ue4ss-dir <that folder>` (or set `HSMP_UE4SS_DIR`).

The folder can also be the `Binaries\Win64` of a game install that has the pinned UE4SS, which is
the default (`HSMP_GAME_DIR`, then the main worktree's `game\`). Use a clean extraction for a
real release, so the unpinned files (§2) are upstream's too.

## 5. Why HSMP does not build UE4SS

RE-UE4SS's Unreal headers live in the `deps/first/Unreal` submodule, **UEPseudo**
(`UE4SS-RE/UEPseudo`), a private repository visible only to GitHub accounts linked to an Epic
Games account, because it contains Unreal Engine-derived code under Epic's EULA. Without it
neither the CMake nor the xmake build of UE4SS compiles, and neither does a C++ mod that includes
UE4SS's own headers: the real `Mod/CppUserModBase.hpp` pulls in `GUI/LiveView` and the Unreal
headers. HSMP therefore:

- ships the identical upstream binary, pinned by hash, and credits upstream in `NOTICE`;
- never commits UEPseudo source or headers;
- builds HSMPNative against a hand-written ABI view of the shipped `UE4SS.dll` instead of the
  official headers (§5.1).

The alternative to bundling is to have the launcher download the pinned upstream zip and verify
the same hashes, so HSMP never redistributes UE4SS at all. HSMP bundles it today. `UE4SS.dll`
statically links further open-source components whose notices belong in
`THIRD-PARTY-NOTICES.html` ([releasing.md](releasing.md) §5).

### 5.1 How HSMPNative builds against the binary

The route that works is to pin to the binary, not to the headers. The files are in
`crates/hsmp-native/cpp/abi/`; the pin generator is `experimental/native-spike/tools/ue4ss_pins`
(Rust: it reads the `UE4SS.dll` export table and `UE4SS.pdb` through dbghelp).

- **`UE4SS.def`**: the UE4SS exports the module uses. CMake runs `lib /def` on it to create the
  import library. If a declaration in the replica header mangled differently from the real
  export, the link would fail.
- **`ue4ss_pins.h`**: the DLL identity (git SHA `e3ba1016`, TimeDateStamp `0x6ABB88FA`,
  SizeOfImage `0x013BA000`), the RVAs of `LuaLock`, `LuaUnlock` and the lock global `Gl`, and the
  sizes of the Lua internal structs as compiled into `UE4SS.dll`.
- **`ue4ss_abi.hpp`**: a replica of `CppUserModBase` (virtual order and members, `sizeof` 0xC0,
  `static_assert`ed) plus the few `RC::` declarations the module calls. It was checked against the
  disassembly of the shipped binary, not only against the source.
- **Lua.** `UE4SS.dll` exports no `lua_*` C API: Lua is linked statically. HSMPNative therefore
  carries a private copy of the same Lua 5.4.7 (from `deps/first/LuaRaw`, which is in the public
  RE-UE4SS repository), compiled as C like UE4SS's. Two Lua copies on one state are safe here:
  Lua 5.4 has no address-identity statics on the hot paths, the struct sizes are
  `_Static_assert`ed against the pinned values (`cpp/src/hsmp_luauser.c`), both copies are C so
  errors unwind by `longjmp` through the shared `vcruntime140`, and the native code never raises
  Lua errors (no `luaL_error` / `luaL_check*`).
- **The Lua lock.** UE4SS maps `lua_lock` to `LuaLock`, one process-wide `CRITICAL_SECTION`. On
  the pinned `UE4SS.dll`, `hsmp_luauser.c` forwards `LuaLock` / `LuaUnlock` to UE4SS's own
  functions at the pinned RVAs (`lock_mode = "host"`); on any other build it falls back to a
  private lock.
- **Refusal on any other build.** `main.dll` refuses to register when the loaded `UE4SS.dll`'s
  TimeDateStamp or SizeOfImage does not match the pin (`HSMPNATIVE_ALLOW_UNPINNED=1` overrides
  this for the offline harness only). The engine-reflection table for native sampling and servo
  is built only on the pinned build.
- **Load order.** UE4SS starts C++ mods before Lua mods, and `on_lua_start` fires before a mod's
  `main.lua` runs, so the `HSMPNative` global exists at every HSMP mod's top level.
- **Fallback.** UE4SS's Lua build has `package.loadlib`, so `hsmp_lua.dll` (the same Rust code
  as `main.dll`, with no `UE4SS.dll` dependency) can be loaded from Lua when the C++ mod is not
  available.
- **Toolchain.** `UE4SS.dll` is linked with a newer MSVC toolset than VS 2022's; both use the
  dynamic CRT (`/MD`), which is binary-compatible across v14x, and building with the older
  toolset is the safe direction.

Measured in game on the pinned build: the native module registered into every HSMP Lua state,
the lock ran in `host` mode, no call ever came from a non-game thread, and the per-frame pump
`frame()` cost 4 µs (p50) / 6 µs (p99) with the sidecar attached.

## 6. What the pinned build means for mod code

Behaviour of this exact build that the Lua mods depend on (details and examples in
[lua-mods.md](lua-mods.md) §5):

- `LoopAsync`, `ExecuteWithDelay` and async key-bind callbacks run off the game thread; every HSMP
  mod reroutes them to `LoopInGameThreadWithDelay` / `ExecuteInGameThreadWithDelay`.
- Reading a SoftObject or SoftClass value crashes in `push_softobjectproperty`.
- A Lua error in a `RegisterHook` callback does not cancel the hooked function, and callbacks on
  Blueprint functions have been observed to run after the function body.
- `button.OnClicked:Bind(fn)` does not accept a Lua closure; HSMP polls `IsPressed()`.
- A `LoopInGameThreadWithDelay` callback keeps being called after it returns `true`; loops that
  must stop use `CancelDelayedAction` (the shim in [lua-mods.md](lua-mods.md) §5.1 does) or their
  own flag.
- UE4SS's `FWeakObjectPtr` constructor faults on an object without a serial number, so native
  code never uses it ([ipc-shared-memory.md](ipc-shared-memory.md) §7.3).

Changing the UE4SS build therefore means re-running the live gate, the Lua suites and the lints
with the new build, re-checking these behaviours, regenerating HSMPNative's ABI pins
(`crates/hsmp-native/cpp/abi/`) and its reflection export list
(`crates/hsmp-native/cpp/test/reflect_check.cpp` checks the names), and updating the hash pins and
this page together. Until the pins match, `main.dll` refuses to register; the Lua facade then
loads `hsmp_lua.dll` instead, with a private Lua lock and without native sampling and servo.

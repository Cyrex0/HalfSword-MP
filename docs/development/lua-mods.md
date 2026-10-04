# Writing and changing the HSMP Lua mods

The game side of HSMP is a set of UE4SS Lua mods plus the native module HSMPNative. This page
covers the layout, how a change gets into the game, how to test it without the game, the lints,
and the rules that keep the game from crashing. Most rules below were learned from real crashes;
each one names the failure it prevents.

Related: [architecture.md](architecture.md) (what each mod does),
[ipc-shared-memory.md](ipc-shared-memory.md) (the IPC facade and the native API),
[ue4ss.md](ue4ss.md) (the UE4SS build), [halfsword/README.md](halfsword/README.md) (game
internals), [testing.md](testing.md) (the gate), [tools.md](tools.md) (the tools).

## 1. Layout

```
mods/
  mods.release.txt              the mods.txt template: which mods are on (1), off (0), dev-only (dev)
  shared/*.lua                  libraries copied into EVERY mod's Scripts/ at deploy
  HSMPMenu/Scripts/main.lua     one folder per shipped mod; main.lua is the UE4SS entry point,
  HSMPMenu/Scripts/*.lua          siblings are loaded with require()
  ...
  dev/HSMPDiag/Scripts/main.lua developer-only mods (never shipped)
  dev/HSMPDump/Scripts/main.lua
```

- **Each mod is its own Lua state.** Mods share no Lua memory. They talk to the sidecar and to
  each other through the shared-memory segment: typed records in slots and rings, and the
  game-local bus for mod-to-mod state ([ipc-shared-memory.md](ipc-shared-memory.md) §6). The
  state directory (`HSMP_STATE_DIR`, default `hsmp_state`) holds only logs, config and the
  identity.
- **`mods/shared/`** holds `hsmp_wg` (world guard), `hsmp_log` (structured events),
  `hsmp_cfg` (`hsmp.cfg` and env), `hsmp_ipc` (the shared-memory facade: typed records),
  `hsmp_session` (is an MP session live?), `hsmp_saveguard` (career save diversion),
  `hsmp_rvp` (the RVP travel guard, [crash-rr.md](crash-rr.md)), `hsmp_ui_scale`, and two
  generated files: `hsmp_ipc_schema` (`hsmp-tools gen-ipc`) and `hsmp_arenas`
  (`hsmp-tools gen-map-data`). Do not edit the generated files.
  Never keep a private copy of a shared file in a mod: deploy and the release tool overwrite it
  with the shared one.
- **Only top-level `Scripts/*.lua` files ship.** Subfolders and test files are not deployed or
  packaged.
- **`mods/mods.release.txt`** decides what is on. A mod folder that is not listed is deployed but
  stays disabled; deploy never turns a mod on by itself. The release package contains only the
  `HSMP*` mods marked `1` (plus `shared/`), and the release tool refuses a retired mod name
  (`HSMPLobby`, `HSMPAdmin`, `HSMPCharacter`, `HSMPSettings`, `HSMPChat`). The lints
  `check_unsafe` and `check_wg --release-set` scan the mods the template enables (`1` or `dev`).

### Loading shared and sibling modules

UE4SS puts the mod's `Scripts/` folder on the Lua path, and deploy copies `shared/*.lua` there, so
`require("hsmp_wg")` works in the game. Mods that should also run straight from the source tree
(offline tests) use a loader that falls back to `dofile` next to `main.lua`:

```lua
local function load_shared(name)              -- require first, then next to main.lua
    local ok, m = pcall(require, name)
    if ok and type(m) == "table" then return m end
    local dir = ((debug.getinfo(1, "S").source or ""):gsub("^@", "")):match("^(.*)[/\\]") or "."
    local ok2, m2 = pcall(dofile, dir .. "/" .. name .. ".lua")
    return ok2 and m2 or nil
end
local HL = load_shared("hsmp_log")
```

### Adding a mod

1. Create `mods/HSMPNew/Scripts/main.lua`, starting with the thread-safety shim (§5.1) and the
   world guard (§5.3).
2. Add `HSMPNew : 1` (or `: dev` while it is experimental) to `mods/mods.release.txt`, above the
   `Keybinds` line, which UE4SS wants last.
3. Add a Lua suite in `tools/hsmp-tools/lua-tests/` (§3) and run the lints (§4).

## 2. Getting a change into the game

`scripts/build-and-deploy.ps1` is the developer deploy (players use the launcher). For a Lua-only
change:

```powershell
.\scripts\build-and-deploy.ps1 -SkipBuild -Dev
```

- `-SkipBuild` skips `cargo build`; the binaries already in `Binaries\Win64\hsmp\` stay, and
  HSMPNative is taken from the previous CMake build in the target dir.
- HSMPNative is required: without a native build the deploy fails, unless `-SkipNative` asks for
  a deploy without it (`HSMPNative : 0`, no multiplayer). The CMake build needs an RE-UE4SS
  checkout at the pinned commit (`-UE4SSSrc`, `HSMP_UE4SS_SRC`, or `<repo>\third_party\RE-UE4SS`).
- `-Dev` turns on the template's `dev` entries (HSMPDiag, UE4SS Keybinds).
- The game folder is `-GamePath`, else `HSMP_GAME_DIR`, else `<repo>\game`, else the main git
  worktree's `game\`.
- It copies every `mods/HSMP*/Scripts/*.lua` and `mods/dev/HSMP*/Scripts/*.lua`, then
  `mods/shared/*.lua`, into `ue4ss\Mods\<Mod>\Scripts\`; writes `ue4ss\Mods\mods.txt` from the
  template (lines of other people's mods are kept); writes `hsmp.cfg` if it is missing; and
  stamps `Binaries\Win64\hsmp_deploy.json` with the commit and the hash of every deployed file.
- By default, on a clean tree with no G0 recorded for HEAD, it runs the full G0 first (`-SkipG0`
  skips that in a dev loop). `-DryRun` shows what would change.

Restart the game after a deploy. The release profile turns UE4SS hot reload off
(`tools/release/release.json` `settings_overrides`); do not depend on it.

## 3. Testing without the game

| Tool | What it does |
|---|---|
| `hsmp-tools lua-check [paths]` | Compiles every `*.lua` under `mods/` with Lua 5.4. Catches syntax errors and the 200-locals limit |
| `hsmp-tools lua-test [suite]` | Runs `tools/hsmp-tools/lua-tests/<suite>.lua` under mlua (Lua 5.4.7) with the `T` harness; `--list` names the suites |
| `cargo test -p hsmp-hud-test` (and `hsmp-interact-test`, `hsmp-saveguard-test`, `hsmpworld-tests`) | Rust-driven harnesses under `tests/` that load the real mod files against their own UE4SS mocks |

A new suite is one file, `tools/hsmp-tools/lua-tests/<suite>.lua`, picked up automatically. The
shared mocks are in `lua-tests/lib/`: `umg_mock.lua` (UE4SS objects, UMG widgets and slots, the
game-thread loop and delay scheduler on a fake clock, click and hover driving, and a record of
every touch of a freed object), `dir_world.lua` (the Director's world for the connection-flow
tests) and `ui_res.lua` (resolutions). Example and the `T` API: [tools.md](tools.md).

Two patterns make code testable offline, and the Director (`director.lua`) and
`HSMPSync/Scripts/spawn_place.lua` are the models: **pure logic over an `env` table** (every
UObject call goes through `env`, which the test replaces), and **tunables in one table** at the
top of the module.

## 4. The lints

All are in `tools/hsmp-tools` and run in G0 (`hsmp-gate g0`). Build them with
`cargo build --release -p hsmp-tools`.

### `check_travel`: only the Director changes the level

Fails on any reference to `OpenLevel`, `OpenLevelBySoftObjectPtr`, `ServerTravel` or
`ClientTravel` (a call, a member reference, a string key, a key built with `..`, or the bare name
as a string) and on console `open` / `travel` / `servertravel` strings fed to a console command,
in any mod or shared file except `mods/HSMPMatch/Scripts/director.lua`. Hook paths such as
`RegisterHook("/Script/Engine.GameplayStatics:OpenLevel", ...)` are allowed everywhere. The
opt-in `mods/HSMPMenu/Scripts/legacy_travel.lua` shim is allow-listed until it is deleted.

```lua
-- wrong (any mod): the level changes behind the Director's back
UEHelpers.GetGameplayStatics():OpenLevel(world, FName("Map_Arena_Pit"), true, "")
-- right: ask the Director through its bus key (director-contract.md)
IPC.bus_put("travel_request", { seq = seq, want = "menu", reason = "user", t = os.time(), from = "HSMPNew" })
```

### `check_wg`: the world guard (W1-W3)

- **W1** a mod that touches UObjects (`FindAllOf`, `FindFirstOf`, `StaticFindObject`,
  `NotifyOnNewObject`, `RegisterHook`, `GetPlayerController`, `StaticConstructObject`) must use a
  guard (`require "hsmp_wg"`).
- **W2** a module-level `local X` assigned a UObject inside a function must be reset in a drop
  handler (`WG.on_drop` / `WG.cache`). Persistent objects (`/Script/` or `Default__` lookups,
  the game instance, the Kismet/Gameplay statics) and converted values (`GetFullName`,
  `tostring`, ...) are exempt.
- **W3** a deferred callback (`ExecuteWithDelay`, `ExecuteInGameThreadWithDelay`,
  `ExecuteInGameThread`) that uses a UObject captured from the enclosing function needs a world
  check in its body (`WG.same(token)` or similar).

Suppress a reviewed line with `-- wg: ok <reason>`. G0 runs it twice: over every mod, and
`--strict --release-set`, where every mod the template enables must be clean.

### `check_unsafe`: UE4SS APIs that crashed the game (U1-U6)

| Rule | Flags | Crash it prevents |
|---|---|---|
| U1 | `:get()`, `.x` or `[..]` on a `RegisterHook` callback parameter whose UFunction parameter is SoftObject/SoftClass (types from the object dump) | null memcpy in UE4SS `push_softobjectproperty`, an access violation `pcall` cannot catch |
| U2 | reading or writing a SoftObject/SoftClass property by name (`obj.Name`, `obj["Name"]`, `GetPropertyValue`, `SetPropertyValue`) | the same |
| U3 | `SetLeaderPoseComponent` / `SetMasterPoseComponent`; every `K2_DestroyActor` unless marked | leader pose on a stand-in's `SK_Skeleton` crashed; destroying a pooled Willie is a no-op that snaps it to the origin |
| U4 | `ExecuteWithDelay`, `LoopAsync`, `RegisterKeyBind(Async)`, `ExecuteAsync` not routed through the mod's game-thread shim | off-thread Lua corrupted the Lua VM |
| U5 | a hook callback (`RegisterHook`, `NotifyOnNewObject`, BeginPlay hooks) that uses a module-level UObject cache without a world-guard check | a stale UObject written after `OpenLevel` |
| U6 | `ProcessConsoleExec`, `ConsoleCommand`, `ExecuteConsoleCommand` outside the Director | travel or quit behind the Director's back |

Scope: the mods the template enables plus `shared/`. U1 and U2 need the game's
`UE4SS_ObjectDump.txt`; without it G0 skips them (`--no-dump`) and reports SKIP. Suppress a
reviewed line with `-- unsafe: ok <reason>` (for U2 on an object that can never be the soft
class: `-- soft: ok <class>`). Known findings not fixed yet are in
`tools/hsmp-tools/check_unsafe.baseline`; any finding not in the baseline fails. `--strict`
ignores the baseline.

### `check-bp-names`: Blueprint names with spaces

`hsmp-tools check-bp-names` flags every squashed (CamelCase) use of a Half Sword Blueprint
function or property whose real name has spaces, compared case-insensitively: a call
`w:SetUpArmor()`, a member reference `pcall(w.SetUpArmor, w)`, or a string key
`w["SetUpArmor"]`. The names come from the game's object dump (and the SDK headers when
present). It needs the game: without a dump the subcommand exits 2 and G0 reports SKIP. After a
game patch, regenerate the dump (`mods/dev/HSMPDump`) and run it before trusting the mods.

## 5. Rules that keep the game alive

### 5.1 Game thread only

In the pinned UE4SS build, `LoopAsync`, `ExecuteWithDelay` and the callbacks of all key binds
(sync and async) run on UE4SS worker threads. Running Lua there alongside game-thread Lua corrupted the mod's Lua
VM: three symbolised crash dumps all faulted inside UE4SS (`lua_next`, `__index` on garbage).
Every HSMP mod therefore starts with the same shim, which reroutes those APIs to the game
thread. Copy it verbatim into a new mod, before any other code:

```lua
-- Thread-safety shim (same as every HSMP mod).
if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
    -- Key callbacks run on the UE4SS input thread. Native-function hooks run
    -- on the game thread on this mod's hook state without UE4SS's lock, so an
    -- ExecuteInGameThread call from a key callback pushed onto that state
    -- mid-hook (crash in push_structproperty, 2026-10-03). The input thread
    -- now only appends to a queue (plain Lua, no UE4SS call) and a
    -- game-thread loop runs what it queued.
    local kq, kq_w, kq_r, kq_loop = {}, 0, 0, nil
    local function kq_wrap(fn)
        if not kq_loop then
            kq_loop = LoopInGameThreadWithDelay(16, function()
                while kq_r < kq_w do
                    kq_r = kq_r + 1
                    local f = kq[kq_r]
                    kq[kq_r] = nil
                    if f then pcall(f) end
                end
            end)
        end
        return function() local n = kq_w + 1; kq[n] = fn; kq_w = n end
    end
    local _rkba = RegisterKeyBindAsync
    RegisterKeyBindAsync = function(key, mods, fn) return _rkba(key, mods, kq_wrap(fn)) end
    local _rkb = RegisterKeyBind
    RegisterKeyBind = function(key, a, b)
        if b then return _rkb(key, a, kq_wrap(b)) end
        return _rkb(key, kq_wrap(a))
    end
end
```

`check_unsafe` U4 fails on a delay or loop API that is not routed through it. Key callbacks only
queue: UE4SS runs native-function hooks on the game thread without its lock, so any UE4SS call
from the input thread (even `ExecuteInGameThread`, which pushes onto the hook state) can corrupt a
hook in flight. That crashed a spectating client in `push_structproperty` (Q/E cycling raced the
`KismetSystemLibrary:Delay` hook); U4 also flags `ExecuteInGameThread` inside the shim. Code that runs
from `NotifyOnNewObject` must hop to the game thread (`ExecuteInGameThread`) before it touches
anything, including `hsmp_log`.

### 5.2 Never read a SoftObject or SoftClass value

Reading a SoftObjectProperty, a SoftClassProperty or a `TSoftObjectPtr` hook parameter crashes
the game with a null memcpy in UE4SS (`push_softobjectproperty`), which `pcall` cannot catch. It
took down a property dump and a soft-travel hook.

```lua
-- wrong: crashes the game
RegisterHook("/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr", function(ctx, World, Level)
    local target = Level:get()                    -- access violation
end)
-- right: treat the parameter as opaque and use your own state for the target
RegisterHook("/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr", function(ctx)
    pending_travel = true                         -- the Director knows where it wants to go
end)
```

Property dumps must skip `Soft*` types. `check_unsafe` U1 and U2 catch both forms.

### 5.3 Drop cached UObjects when the world changes

A level change frees every actor, component and world-owned widget of the old world, and every
round reloads the arena with `OpenLevel`. Touching a freed UObject afterwards (a property write, a
call, even `IsValid()`, which reads the freed object) is an access violation `pcall` cannot catch;
it crashed both games at a round reset (`UStruct::FindProperty <- __newindex` from a delayed
callback). `mods/shared/hsmp_wg.lua` is the one guard:

```lua
local WG = require("hsmp_wg").new{ log = Log }    -- installs the OpenLevel / LoadMap pre-hooks
local my_pawn = nil
WG.on_drop(function(why) my_pawn = nil end)       -- reset Lua references only; never touch them

LoopAsync(100, function()                         -- game thread, thanks to the shim
    if not WG.check() then return false end       -- world changed or a travel is pending: skip
    if not WG.settled() then return false end     -- the first 2 s of a world
    my_pawn = my_pawn or find_my_pawn()
    -- ... use my_pawn ...
    return false
end)

local tok = WG.token()                            -- a delayed one-shot remembers its world
ExecuteWithDelay(500, function()
    if not WG.same(tok) then return end           -- the world changed meanwhile: drop the work
    -- re-find the object here; never use one captured before the delay
end)
```

- The key is the world's full name, the `UWorld` address and the PlayerController's FName: a
  reload of the same arena changes at least the last one.
- **Settle rule:** nothing may walk `FindAllOf` over actors (Willies, weapons, props) or read
  their names in the first 2 s of a world (`WG.SETTLE_S`): the old world is still being purged
  and the new pawn is mid-construction. Two crashes 0.5-0.6 s after a round reload came from
  exactly that.
- `WG.pc()` returns the PlayerController with one lookup per game frame; use it instead of
  calling `UEHelpers.GetPlayerController()` several times per tick.
- A hook callback that defers work never captures the hooked object: it does the work
  synchronously or re-finds the object later.

`check_wg` (W1-W3) and `check_unsafe` U5 enforce this.

### 5.4 Blueprint function names have spaces

Half Sword's Blueprint UFunctions are named with spaces: "Set Up Armor", "Wake Up",
"Get Damage", "Spawn Combatants", "Setup Character Event". From Lua, `pawn:SetUpArmor()` looks up a
function that does not exist and **does nothing, silently**. This broke loadout, the Willie
wake-up and a spawn fallback before it was understood.

```lua
-- wrong: silently a no-op
pawn:SetUpArmor(false, false)
-- right: index by the real name and pass self explicitly
local ok, err = pcall(function() pawn["Set Up Armor"](pawn, false, false) end)
-- properties too
local passport = pawn["Character Passport"]
```

The real name is the part after the last `:` in the dump's `Function /Game/...:<Name>` line.
Native `/Script/...` engine functions have no spaces. `check-bp-names` catches the squashed
forms.

### 5.5 Hooks cannot cancel, and do not count on "pre"

- A Lua `error()` in a `RegisterHook` callback does **not** abort the native call.
- On Blueprint UFunctions the callback has been observed to fire after the function body ran
  (the start-menu Quit button kept quitting; the slow-motion triggers in
  [spawns.md](subsystems/spawns.md) are handled "right after, in the same frame"). Native
  `/Script/` functions (`OpenLevel`, `SetGlobalTimeDilation`, `KismetSystemLibrary:Delay`) do get
  a working pre-hook, and HSMP rewrites their parameters there.
- `RegisterHook(path, pre, post)` takes both callbacks; HSMPCombat registers both on
  `Get Damage`. If your design needs code to run before a Blueprint function, prove the ordering
  in game for that function first.

### 5.6 UMG from Lua

Runtime UI (HSMPMenu, HSMPHud) is built from raw UMG classes. What works in this build:

```lua
local uw = StaticConstructObject(StaticFindObject("/Script/UMG.UserWidget"), outer,
                                 FName("HSMPPanel", FNAME_Add))
uw.WidgetTree = StaticConstructObject(StaticFindObject("/Script/UMG.WidgetTree"), uw)
local canvas = StaticConstructObject(StaticFindObject("/Script/UMG.CanvasPanel"), uw.WidgetTree)
uw.WidgetTree.RootWidget = canvas               -- 1. user widget, 2. tree, 3. root
-- 4. populate children
local slot = canvas:AddChildToCanvas(label)     -- returns a CanvasPanelSlot: position, size, anchors
button:SetContent(text)                         -- a Button is a content widget: not AddChild
-- 5. only now
uw:AddToViewport(990)
KEEP[#KEEP + 1] = uw                            -- keep a Lua reference to every widget
```

- **Construction order matters**: user widget, then its `WidgetTree`, then `RootWidget`, then the
  children, then `AddToViewport`. Any other order gives a blank screen.
- `StaticConstructObject(class, outer, FName)` is the factory that works.
  `WidgetTree:ConstructWidget` is a C++ template and not callable; `CreateWidget` takes only
  `UserWidget` subclasses.
- **Keep a reference to every widget** in a Lua table, or the garbage collector reaps it within
  seconds and it stops working.
- **`button.OnClicked:Bind(fn)` does not work**: multicast delegates accept only a UObject plus a
  real UFunction name, not a Lua closure. Poll instead: a game-thread loop reads
  `button:IsPressed()` and acts on the rising edge (HSMPMenu polls every 33 ms). Text boxes are
  polled the same way (`edit:GetText():ToString()`).
- Viewport size: `GameViewportClient:GetViewportSize()` silently fails here; use
  `mods/shared/hsmp_ui_scale.lua` (`WidgetLayoutLibrary` size and scale).
- A bare `TextBlock` renders in UE's default font, not the game's.
- `KismetSystemLibrary:PrintString` is development-only and does nothing in the shipping game.

### 5.7 Defensive calls

- **Wrap every property setter and UFunction call in `pcall`.** Reflection setters on
  uninitialised paths throw (or worse) in the shipping build.
- **Never pass `nil` where an FName is expected.** UE unwraps it into a bad pointer and crashes;
  build one with `FName("name", FNAME_Add)`.
- **Check `IsValid()` on objects you just looked up**, but never call it on an object from an old
  world (§5.3): validity checks read the object.

### 5.8 Willies are pooled

The game pools its `Willie_BP_C` characters. `K2_DestroyActor` on a Willie is a silent no-op that
snaps it to the origin, and `Willie_BP_C_0` (tag `Persistent`, invisible mesh) sits at the origin
of every arena. To take a Willie out of play, hide it and disable its collision, tick and
physics, and remove its AI controller and weapons. `check_unsafe` U3 flags every
`K2_DestroyActor`; mark a reviewed non-Willie call with `-- unsafe: ok <reason>`. Census and
stand-in code ignores Willies that are hidden, have an invisible mesh or carry the `Persistent`
tag.

### 5.9 Only the Director changes the level

`mods/HSMPMatch/Scripts/director.lua` is the single owner of level changes, enforced by
`check_travel`, and of console commands (`check_unsafe` U6). Every other mod asks it through the
bus keys `travel_request` and `ui_request` ([director.md](subsystems/director.md),
[director-contract.md](subsystems/director-contract.md)). The Director also rewrites the game's own
`OpenLevel` calls while a session exists, so a native "back to menu" cannot strand a player, and
it waits for the RVP guard before every travel ([crash-rr.md](crash-rr.md)).

### 5.10 Tests never inject OS input

Test harnesses must never send OS-level input (no `SendInput`, no synthetic mouse or keyboard
events, no AutoHotkey): a test that did moved a user's real mouse while they were using the PC.
Drive the game only through mod-side hooks, the autotest knob (`HSMP_AUTOTEST=1` and
`hsmp-tools ipc-ctl --pid <game> autotest <cmd> [arg]`, [testing.md](testing.md)) or RCON.
Scripted movement and swings come from inside the game (`mods/HSMPMenu/Scripts/autotest_mover.lua`
walks the pawn and swings its arm by impulses, with the player's own input disabled meanwhile).

## 6. Other conventions

- **Events:** use `shared/hsmp_log.lua` for anything the gate judges; the vocabulary is frozen and
  checked by G0's `events` check ([testing.md](testing.md) §1). Never emit per frame.
- **IPC:** go through `shared/hsmp_ipc.lua` only: `IPC.put` / `IPC.get` / `IPC.rec` for slots,
  `IPC.send` for messages to the sidecar, `IPC.events` for S2G events, `IPC.bus_put` /
  `IPC.bus_table` for mod-to-mod state. A new contract is a new record in
  `crates/hsmp-ipc/src/schema/<domain>.rs`, followed by `hsmp-tools gen-ipc`. Never add a state
  file: G0 `state_files` fails on any file name that is not on the allow-list.
- **Config and log files you write** (only those on the allow-list): `IPC.write` writes a temp
  file and renames it over the target; readers keep the last good value on a torn read.
- **Configuration:** paths and URLs come from `shared/hsmp_cfg.lua` (`hsmp.cfg`, env); there are
  no machine-specific paths in the mods.
- **Career saves:** in MP the game's saving is diverted by `shared/hsmp_saveguard.lua` (installed
  by the Director). Never call anything that writes the career save; see
  [halfsword/README.md](halfsword/README.md) for the "Reset Player Character" trap.

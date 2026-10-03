# HSMP Lua mods

These are the UE4SS Lua mods that run inside Half Sword. Each mod keeps the layout UE4SS requires,
`<Mod>/Scripts/main.lua` (plus sibling modules in `Scripts/`).

| Mod | What it does |
|---|---|
| `HSMPMenu` | The HSMP menus (main-menu buttons, host, server browser, lobby, loadout, settings, character) |
| `HSMPSync` | Sends the local player's pose, weapon and root motion; spawn placement |
| `HSMPAvatars` | Drives the stand-ins that show other players |
| `HSMPMatch` | Match flow; `director.lua` (the Director) is the only code that changes levels |
| `HSMPCombat` | Damage claims, vitals replication |
| `HSMPLoadout` | Classes, kits and dressing the player |
| `HSMPWorld` | Replication of weapons and props in the arena |
| `HSMPHud` | The in-match HUD |
| `HSMPInteract` | Grabs and shoves between players |
| `HSMPNoCutscene` | Skips cutscenes that would break a session |
| `shared/` | Libraries copied into every mod's `Scripts/` at deploy (`hsmp_cfg`, `hsmp_log`, `hsmp_wg` world guard, `hsmp_saveguard`, `hsmp_arenas`, ...) |
| `dev/` | Developer-only mods: `HSMPDiag` (property dumps), `HSMPDump` (UE4SS object/header dumps) |

`mods.release.txt` is the template for the game's `ue4ss/Mods/mods.txt`: `1` = enabled, `0` =
disabled, `dev` = enabled only in a developer deploy. A mod that is not listed stays disabled.

Players get these files from the launcher. Developers deploy them with
`scripts/build-and-deploy.ps1` and test them offline with `hsmp-tools lua-test`. Read
[docs/development/lua-mods.md](../docs/development/lua-mods.md) before changing a mod.

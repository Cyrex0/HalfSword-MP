# Troubleshooting

Find your problem in the tables below. If nothing helps, [report a bug](#reporting-a-bug).

On this page:

* [Installing and starting](#installing-and-starting)
* [Launcher messages](#launcher-messages)
* [Finding and joining games](#finding-and-joining-games)
* [During a match](#during-a-match)
* [Where the logs are](#where-the-logs-are)
* [Reporting a bug](#reporting-a-bug)

---

## Installing and starting

| Symptom | Likely cause | What to do |
|---|---|---|
| "Windows protected your PC" when you start the launcher | The launcher is not code-signed yet, so SmartScreen does not know it. | Click **More info**, then **Run anyway**. Only do this for a zip whose SHA-256 matches the release page (see [Install](install.md#install)). |
| Your antivirus deletes or quarantines `dwmapi.dll`, or warns about it | `dwmapi.dll` is the UE4SS mod loader. Antivirus programs often flag it. It is a false positive. | Restore the file from quarantine and allow it, then click **Repair** in the launcher. |
| The red line **"Half Sword was updated. HSMP ... does not support this game build yet"** | Steam updated Half Sword after this HSMP release came out. | Wait for an HSMP update that supports the new build. **Install** and **Update** are blocked until then, unless you tick **Try anyway**. The game still starts from Steam, but menus or matches may break. |
| The HSMP buttons do not appear on the main menu | The game was not started with HSMP installed, or a file is missing. | Open the launcher. If the status line says files are changed or missing, click **Repair**. Then start Half Sword from Steam. |
| The game crashes while an arena loads | An engine crash in hair streaming. HSMP works around it with an `Engine.ini` line, which the game sometimes removes. | Open the launcher once (it puts the line back), or add the Steam launch option from [Install](install.md#optional-the-steam-launch-option). If it still happens, [report it](#reporting-a-bug) with a crash report. |
| The game crashes about a second after a round ends, while the arena reloads | A bug in the game's blood and wound painting: paint work still queued for the old arena runs after that arena is gone. Before every level change HSMP stops new paint work and waits (at most 2.5 seconds) until the queue is empty, which prevents this crash in normal cases. | [Report it](#reporting-a-bug) with a crash report from the launcher. |

## Launcher messages

| Message | What to do |
|---|---|
| "Extract the WHOLE release zip ..." | You started the launcher from inside the zip. Extract everything to a folder first. |
| "... signed with key ... which this launcher does not trust" or "signature is INVALID" | The download is damaged, was modified, or was signed by somebody else. Download it again from the [Releases page](https://github.com/Cyrex0/HalfSword-MP/releases) and check its SHA-256. |
| "package file ... fails its SHA-256 check" | The download or extraction is damaged. Extract it again, or download it again. |
| "Half Sword was not found through Steam" | Click **Browse...**, pick the folder with `HalfSwordUE5.exe`, then click **Use**. |
| "... is running: close it first" | Close Half Sword, and any `hsmp-*` program started by it, then try again. The launcher never closes programs for you. |
| "Steam is still downloading or updating Half Sword" | Wait until Steam shows the game as ready to play. |
| "another HSMP launcher ... is installing, uninstalling or restoring saves right now" | Another launcher window or a command line is busy. Wait for it, or close the other window. |
| "this release (...) is OLDER" | You opened an older zip. Use the newer one, or tick **Yes, downgrade to it**. |
| "an older UE4SS install is in the game folder" | Remove `UE4SS.dll` and `UE4SS-settings.ini` from `HalfswordUE5\Binaries\Win64`, or run Steam's "Verify integrity of game files". |
| "...enabled.txt makes UE4SS load ..." | Delete that `enabled.txt`. UE4SS loads such mods whatever `mods.txt` says. |
| "HSMP's install record ... does not match it" | The game folder changed under the launcher, for example a move that did not finish. Nothing was changed. Finish or undo the move, otherwise install HSMP again. |
| "HSMP was partly removed ..." / "Uninstall did not finish" | See [Uninstall](uninstall.md#if-uninstall-stops-part-way). |
| "the career save check after an earlier multiplayer crash failed ..." | Uninstall is blocked so your career is not changed further. Do not play your career until it is fixed. Restore your career from the backup list. See [Career saves](career-saves.md#if-the-crash-check-fails). |
| "the Half Sword exe changed since it was checked" | Steam probably updated the game a moment ago. Wait for the game-version line to update, then try again. |

## Finding and joining games

| Symptom | Likely cause | What to do |
|---|---|---|
| The server browser is empty, or says **"Server list unavailable (...). Use DIRECT CONNECT or LAN"** | The public server list could not be reached (no internet, a firewall, or the list is down), or nobody is hosting right now. LAN games and games on your own PC still show. | Ask the host for their address and use **DIRECT CONNECT**. See [Joining a game](playing.md#joining-a-game). |
| A friend's game on the same network does not show under **LAN** | The host uses a port outside 7777 to 7786, or a firewall blocks it. | The host sets **HOST PORT** between 7777 and 7786. Or use **DIRECT CONNECT** with the host's local IP and port. Press **REFRESH** (F5). |
| "Cannot reach the server at ..." in the lobby | Nothing answered. Wrong address, the host's port is not forwarded, or a firewall blocks it. | Check the address and port. The host forwards the UDP port on the router and allows `hsmp-server.exe` in Windows Firewall. See [Hosting from the menu](playing.md#hosting-from-the-menu) and [Ports and firewall](../hosting/ports-and-firewall.md). Then **LEAVE** and try again. |
| "Version mismatch: server protocol vN, yours vM" or **CONNECTION REJECTED** | You and the server run different HSMP versions. | Everybody updates to the same HSMP release. |
| "That server needs a password, which HSMP cannot send yet" | Password-protected servers cannot be joined from the menu yet. | Pick another server. |
| "That server is full" | The server has no free slot. | **REFRESH**, or pick another server. |
| **START MATCH** is greyed out | Not every player is **READY**, or you are not the host. | Read the line above the buttons: it names who is not ready. |
| Friends can join your LAN game, but not over the internet | Your router does not forward the port, or your internet provider uses a shared address (CGNAT). | Forward the UDP port. If that does not help, let someone else host, or run a [dedicated server](../hosting/dedicated-server.md). |

## During a match

| Symptom | Likely cause | What to do |
|---|---|---|
| **RECONNECTING...** in the centre of the screen | Your connection to the server dropped. | Wait. HSMP keeps trying for about 35 seconds. Then choose **RECONNECT** or **BACK TO MENU**. |
| **OPPONENT DISCONNECTED** | Another player's connection dropped. | Wait up to 30 seconds. If they come back, the round is replayed. |
| **NET POOR** or **NET BAD** in the corner | High ping or packet loss. | Use a wired connection, close downloads and streams, or pick a closer server. |
| Another player is invisible, or stands still | Their character (the "stand-in" that shows them on your screen) did not appear or lost track. This is a known issue in this version. | Check **SETTINGS > PEER AVATARS** is **ON**. If it keeps happening, [report it](#reporting-a-bug) with your logs. |
| Another player's arms, weapon or feet do not follow their real movement closely, on heavy arenas (for example Lords Hall) or on a slow PC | Their body on your screen is driven by pose data from their game. When either game runs at a low frame rate, the copy lags or drifts more. This is a known issue in this version. | Lower the graphics settings so the game holds a steady frame rate. Report it with both players' logs if it is bad on a light arena. |
| A fighter starts a round lying on the ground, or without the weapon of their kit in hand | A known issue in this version: now and then a fighter's body is not set up cleanly at the round start. | [Report it](#reporting-a-bug) with your logs, and say on which arena and in which round. |
| Grabbing or shoving another player feels odd | Other players are shown as stand-ins driven over the network, so contact is sent through the server and is not as direct as in single player. | This is a known limitation. |
| The game does not pause when you press Esc | That is on purpose: in multiplayer the fight goes on. | Use **RESUME**, **LEAVE MATCH** or **QUIT GAME**. |
| The HUD is missing | The HUD is turned off, or HUD changes have not applied yet. | Turn **SETTINGS > HUD** on. HUD settings apply from your next match. |

## Where the logs are

`Win64` means `<Steam library>\steamapps\common\Half Sword\HalfswordUE5\Binaries\Win64`.

| Log | Where | What is in it |
|---|---|---|
| UE4SS log | `Win64\ue4ss\UE4SS.log` | Everything the HSMP mods print in the game. The most useful file for bugs. |
| HSMP event log | `Win64\hsmp_state\hsmp_events.jsonl` | HSMP's own events (travel, saves diverted, connection changes). |
| Launcher log | `%LOCALAPPDATA%\HSMP\launcher\launcher.log` | Everything the launcher did, with times. |
| Game crash folders | `%LOCALAPPDATA%\HalfSwordUE5\Saved\Crashes\UECC-*` | Unreal Engine crash data. |
| Crash reports | `%LOCALAPPDATA%\HSMP\crash_reports\` | Redacted reports you saved from the launcher. |

To open a `%LOCALAPPDATA%` folder, press Windows+R, paste the path, and press Enter.

Logs can contain player names, IP addresses and your Windows user name in file paths. Crash
reports from the launcher remove those. If you attach a raw log, look through it first.

## Reporting a bug

Open an issue at <https://github.com/Cyrex0/HalfSword-MP/issues>. Please do not report HSMP bugs
to Half Sword Games.

Include:

1. **What happened**, and what you expected instead.
2. **How to make it happen again**, step by step, if you know.
3. **Your HSMP version.** It is at the top of the launcher, for example "HSMP 0.1.0 - protocol 6".
4. **Your Half Sword build.** It is in the launcher's game list ("Steam build ...") and in the
   game-version line.
5. **Whether you hosted or joined**, and how many players were in the game.
6. **Logs.** Attach `UE4SS.log`, and `launcher.log` for install problems.
7. **After a crash:** a crash report saved from the launcher (**Crash reports > Save a redacted
   report**).

Before you attach anything, check it for personal data.

# Troubleshooting

Find your problem in the tables below. If nothing helps, [report a bug](#sending-us-a-bug-report).

On this page:

* [Installing and starting](#installing-and-starting)
* [Launcher messages](#launcher-messages)
* [Finding and joining games](#finding-and-joining-games)
* [During a match](#during-a-match)
* [Where the logs are](#where-the-logs-are)
* [Sending us a bug report](#sending-us-a-bug-report)

---

## Installing and starting

| Symptom | Likely cause | What to do |
|---|---|---|
| "Windows protected your PC" when you start the launcher | The launcher is not code-signed yet, so SmartScreen does not know it. | Click **More info**, then **Run anyway**. Only do this for a zip whose SHA-256 matches the release page (see [Install](install.md#install)). |
| Your antivirus deletes or quarantines `dwmapi.dll`, or warns about it | `dwmapi.dll` is the UE4SS mod loader. Antivirus programs often flag it. It is a false positive. | Restore the file from quarantine and allow it, then click **Repair** in the launcher. |
| The red line **"Half Sword was updated. HSMP ... does not support this game build yet"** | Steam updated Half Sword after this HSMP release came out. | Wait for an HSMP update that supports the new build. **Install** and **Update** are blocked until then, unless you tick **Try anyway**. The game still starts from Steam, but menus or matches may break. |
| The HSMP buttons do not appear on the main menu | The game was not started with HSMP installed, or a file is missing. | Open the launcher. If the status line says files are changed or missing, click **Repair**. Then start Half Sword from Steam. |
| The game crashes while an arena loads | An engine crash in hair streaming. HSMP works around it with an `Engine.ini` line, which the game sometimes removes. | Start the game with **Launch through Steam** in the launcher (it passes the workaround), or add the Steam launch option from [Install](install.md#optional-the-steam-launch-option). If it still happens, [report it](#sending-us-a-bug-report) with a crash report. |
| The game crashes about a second after a round ends, while the arena reloads | A bug in the game's blood and wound painting: paint work still queued for the old arena runs after that arena is gone. Before every level change HSMP stops new paint work and waits (at most 2.5 seconds) until the queue is empty, which prevents this crash in normal cases. | [Report it](#sending-us-a-bug-report) with a crash report from the launcher. |

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
| Friends can join your LAN game, but not over the internet | Your router did not open the port (the host lobby says "Couldn't open your router port automatically"), and NAT traversal did not get through either; or your internet provider uses a shared address (CGNAT). | Turn UPnP on in the router, or forward the UDP port by hand. If that does not help, let someone else host, or run a [dedicated server](../hosting/dedicated-server.md). |
| The lobby says **"CONNECTING THROUGH YOUR ROUTER..."** | The host did not answer directly; your game is asking the server list to open the host's router (NAT traversal). | Wait a few seconds. It usually connects. |
| **"The host's network blocks incoming connections; ask them to forward UDP 7777"** | Neither the direct path nor NAT traversal reached the host: its router drops every packet it did not ask for, and cannot be punched (a "symmetric" NAT, or the host is not on the server list). | The host forwards that UDP port to their PC (or turns UPnP on), or someone else hosts. In the browser such servers show **--** in PING; **NAT** means NAT traversal will be tried. |

## During a match

| Symptom | Likely cause | What to do |
|---|---|---|
| **RECONNECTING...** in the centre of the screen | Your connection to the server dropped. | Wait. HSMP keeps trying for about 35 seconds. Then choose **RECONNECT** or **BACK TO MENU**. |
| **OPPONENT DISCONNECTED** | Another player's connection dropped. | Wait up to 30 seconds. If they come back, the round is replayed. |
| **NET POOR** or **NET BAD** in the corner | High ping or packet loss. | Use a wired connection, close downloads and streams, or pick a closer server. |
| Another player is invisible, or stands still | Their character (the "stand-in" that shows them on your screen) did not appear or lost track. This is a known issue in this version. | Check **SETTINGS > PEER AVATARS** is **ON**. If it keeps happening, [report it](#sending-us-a-bug-report) with your logs. |
| Another player's arms, weapon or feet do not follow their real movement closely, on heavy arenas (for example Lords Hall) or on a slow PC | Their body on your screen is driven by pose data from their game. When either game runs at a low frame rate, the copy lags or drifts more. This is a known issue in this version. | Lower the graphics settings so the game holds a steady frame rate. Report it with both players' logs if it is bad on a light arena. |
| A fighter starts a round lying on the ground, or without the weapon of their kit in hand | A known issue in this version: now and then a fighter's body is not set up cleanly at the round start. | [Report it](#sending-us-a-bug-report) with your logs, and say on which arena and in which round. |
| Grabbing or shoving another player feels odd | Other players are shown as stand-ins driven over the network, so contact is sent through the server and is not as direct as in single player. | This is a known limitation. |
| The game does not pause when you press Esc | That is on purpose: in multiplayer the fight goes on. | Use **RESUME**, **LEAVE MATCH** or **QUIT GAME**. |
| Hair looks flatter up close than in the unmodded game | On purpose: with HSMP installed, hair is drawn with the game's own hair cards instead of hair strands. Strands hit an engine crash when a match starts. | Nothing to do. [Uninstall HSMP](uninstall.md) to get strands back. |
| The HUD is missing | The HUD is turned off, or HUD changes have not applied yet. | Turn **SETTINGS > HUD** on. HUD settings apply from your next match. |

## Where the logs are

Every time you start Half Sword with HSMP, it keeps that game run's logs in its own folder:
`%LOCALAPPDATA%\HSMP\logs\<date>_<time>_p<number>\`. The newest 10 runs are kept (at most
200 MB in all); older ones are deleted. When the game exits, even when it crashes, a small
helper copies the game's logs into the run's folder before the next start can overwrite them.

| File in the run's folder | What is in it |
|---|---|
| `UE4SS.log` | Everything the HSMP mods print in the game. The most useful file for bugs. |
| `game.log` | The engine's own log. |
| `sidecar.log` | The connection to the server: joins, reconnects, and every 5 seconds the ping, packet loss and how smoothly other players' movement arrives. |
| `server.log`, `server-events.jsonl` | Only when you hosted: what your server did, with a health line every 10 seconds. |
| `hsmp_events.jsonl` | HSMP's own events for that run (travel, saves diverted, connection changes). |
| `crashes\` | The game's crash data for that run, if it crashed. |
| `session.json` | When the run started and ended, and how it ended (`clean`, `crashed`, ...). |

Other logs:

| Log | Where | What is in it |
|---|---|---|
| Launcher log | `%LOCALAPPDATA%\HSMP\launcher\launcher.log` | Everything the launcher did, with times. |
| Saved bug reports | `%LOCALAPPDATA%\HSMP\bug_reports\` | Zips you made with **Create bug report**. |
| Live UE4SS log | `Win64\ue4ss\UE4SS.log` (`Win64` = `<Steam library>\steamapps\common\Half Sword\HalfswordUE5\Binaries\Win64`) | The current run only: the next start overwrites it. |

To open a `%LOCALAPPDATA%` folder, press Windows+R, paste the path, and press Enter.

If the game crashed, the main menu says so the next time: "The game crashed last time. Open the
launcher > Create bug report to send us the logs."

## Sending us a bug report

Please do not report HSMP bugs to Half Sword Games.

1. Open the launcher. Under **Bug report**, tick the game run(s) the problem happened in. After a
   crash, the run that crashed is ticked for you.
2. Click **Create bug report**. The launcher collects the logs and removes personal data (see
   below). Nothing is saved or sent yet.
3. Check the file list. Click any file to see exactly what will be sent.
4. Send it one of two ways:
   - **Open GitHub issue**: saves the zip and opens a bug report on
     <https://github.com/Cyrex0/HalfSword-MP/issues> with your HSMP version, game build and Windows
     version filled in. Drag the zip into the page, then describe what happened.
   - **Upload to HSMP...**: sends the zip to us directly. You get a report id; put it in your
     issue or message so we can find the report.
5. In the issue, write what happened, what you expected, how to make it happen again, whether
   you hosted or joined, and how many players were in the game.

The same from a command prompt: `hsmp-launcher report --list`, then `hsmp-launcher report`
(the newest crashed run, else the newest run), `--session <id>`, `--upload`, `--show UE4SS.log`
to read one file, `--dry-run` to only list them. Running a server? See
[Logs and diagnostics](../hosting/logs-and-diagnostics.md#sending-us-a-report).

### What a bug report contains, and what it does not (privacy)

It contains:

- the logs of the runs you ticked (the files in the table above) and the launcher log;
- the game's crash dumps of those runs (a snapshot of the game's memory stack when it crashed;
  no screenshots), unless you untick **Include crash dumps**;
- `system.json`: your Windows version, CPU, memory, graphics card and driver, the HSMP version,
  protocol and content hash, the Half Sword build, the UE4SS version and the firewall rule;
- `triage.json`: how each run ended, and our crash classifier's guess for each crash;
- `hsmp-report.json`: the list of files with their sizes.

Before you see it, every text file has these removed:

- your Windows user name, your home folder in every path, your computer name and e-mail
  addresses;
- your player key, server keys, RCON passwords, tokens and other secrets;
- player names (yours and other players');
- your own public IP address, always. Other players' IP addresses are replaced by numbers
  (`<ip-1>`, `<ip-2>`) while **Hide other players' IP addresses** is ticked (the default). LAN
  addresses such as `192.168.x.x` are kept; they say nothing about who you are.

To find your public address, the launcher asks the HSMP server list once, so that it can remove
it. A report is never sent without your click. An uploaded report is readable only by the HSMP
developers, is never listed or shown publicly, and is deleted after about 60 days. The server
keeps a daily, one-way code of the address that uploaded it (to limit uploads per address), never
the address itself.

It never contains: chat logs, your settings, your HSMP identity, or your career save.

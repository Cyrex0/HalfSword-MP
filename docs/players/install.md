# Installing HSMP

HalfSword-MP (HSMP) is an unofficial multiplayer mod for Half Sword. This page shows you how to install it,
start it, and update it.

> **Unofficial mod. Use at your own risk.**
>
> * HSMP is a fan-made project. It is not affiliated with, endorsed by, sponsored by or approved
>   by Half Sword Games or Game Seer Publishing. Please do not ask them for help with HSMP.
> * Back up your saves first. The launcher does this for you (see [Career saves](career-saves.md)),
>   but a copy of your own is never a bad idea. Your saves are in
>   `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames`.
> * Consider turning off Steam Cloud for Half Sword while you try HSMP (Steam > Half Sword >
>   Properties > General).
> * Some antivirus programs flag the `dwmapi.dll` loader (UE4SS). That is a false positive.
> * The launcher can remove HSMP cleanly again. See [Uninstall](uninstall.md).

On this page:

* [What you need](#what-you-need)
* [Install](#install)
* [First launch](#first-launch)
* [What the launcher changes](#what-the-launcher-changes)
* [Updating](#updating)
* [Repair](#repair)
* [Moving the game](#moving-the-game)
* [Crash reports](#crash-reports)
* [Command line](#command-line)

---

## What you need

* Windows 10 or 11, 64-bit.
* **Half Sword from Steam** (app 2397300), fully updated, and started at least once.
* A Half Sword build that this HSMP release supports. The launcher checks this for you.
  HSMP 0.1.0 supports **Steam build 24185754** (Unreal Engine 5.4.4).
* Steam running when you play.

HSMP does not run on Linux or the Steam Deck. See the [FAQ](faq.md#does-it-work-on-linux-or-the-steam-deck).

## Install

It takes about a minute.

1. **Download** `hsmp-<version>.zip` from the [HalfSword-MP Releases page](https://github.com/Cyrex0/HalfSword-MP/releases). Do not use mirrors or
   re-uploads.
2. **Check the download.** Open PowerShell in your Downloads folder and run:

   ```powershell
   Get-FileHash hsmp-<version>.zip
   ```

   The long number must match the SHA-256 checksum on the release page exactly. If it does not
   match, delete the zip and download it again.

   Why this matters: the launcher checks the signature of every file, but on a first install the
   launcher itself comes out of the same zip. A tampered zip could bring its own launcher. The
   checksum on the release page is what proves your zip is the real one. (Updates are safer. See
   [Updating](#updating).)
3. **Extract the whole zip** to any folder, for example `Downloads\hsmp-0.1.0`. Running the
   launcher from inside the zip does not work, because it needs the files next to it.
4. **Double-click `hsmp-launcher.exe`.**
   * Windows SmartScreen may say "Windows protected your PC", because the launcher is not
     code-signed yet. Click **More info**, then **Run anyway**.
   * Your antivirus may flag `dwmapi.dll`. This file is UE4SS, the mod loader HSMP is built on.
     Every UE4SS mod has the same problem. The launcher only installs it after it has checked
     the file's signature and SHA-256.
5. **Section "1. Half Sword".** The launcher finds Half Sword through Steam. If you have several
   Steam libraries, pick the right one. If it was not found, click **Browse...**, choose the
   folder that contains `HalfSwordUE5.exe`, then click **Use**.
6. **Check the game-version line** under it:
   * Green, **"Supported Half Sword build (...)"**: carry on.
   * Red, **"Half Sword was updated. HSMP ... does not support this game build yet; please wait
     for an HSMP update."**: Half Sword was patched after this HSMP release. Wait for a new HSMP
     release. You can tick **"Try anyway (unsupported: menus or matches may break)"**, but things
     may not work.
7. **Section "2. Multiplayer mod".** Leave **"Back up my career saves first (recommended)"**
   ticked and click **Install**. The Messages panel at the bottom shows what happens. At the end
   it says "HSMP ... is installed".
8. **Section "3. Play".** Click **Play**.

You never need to edit a file by hand.

### How Play starts the game

* **Play**, when Steam is running: the launcher starts the game itself, from the game folder,
  with the HSMP launch option.
* **Play**, when Steam is not running: the launcher starts the game through Steam
  (`steam -applaunch`). Steam may ask once whether to allow the extra launch option. Allow it:
  it is a crash workaround (see the table below).
* **Launch through Steam** always uses the Steam route.

The Steam route always starts *Steam's own* copy of Half Sword. If you picked a different copy
with **Browse...**, start Steam first and use **Play**. The launcher refuses to start the wrong
copy.

Before every Play, the launcher also finishes the career save check of any multiplayer session
that crashed. See [Career saves](career-saves.md#3-the-crash-check-before-play-and-uninstall).

## First launch

1. Wait for the Half Sword main menu.
2. Next to the game's own menu buttons you see a second column with five HSMP buttons:
   **HOST GAME**, **SERVER BROWSER**, **SETTINGS**, **CHARACTER** and **QUIT MP**.
   (The launcher's hint calls this the "Multiplayer ribbon".)
3. Open **SETTINGS** first and set your **NICKNAME** (the default is "Willie"). Click
   **SAVE & BACK**.

Now read [Playing](playing.md) to join or host a game.

## What the launcher changes

Every change is recorded before it is made, and **Uninstall** reverses all of it.
`Win64` below means `<Steam library>\steamapps\common\Half Sword\HalfswordUE5\Binaries\Win64`.

| What | Where | Why |
|---|---|---|
| UE4SS mod loader (one fixed, hash-checked build) | `Win64\dwmapi.dll` and `Win64\ue4ss\` | runs the HSMP Lua mods |
| HSMP mods | `Win64\ue4ss\Mods\HSMP*\` | the multiplayer mod itself (Lua scripts) |
| HSMP native module | `Win64\ue4ss\Mods\HSMPNative\dlls\` (`main.dll`, `hsmp_lua.dll`) | the fast link between the game and `hsmp-sidecar.exe` (shared memory), and the pose and hit code that runs inside the game |
| `mods.txt` | `Win64\ue4ss\Mods\mods.txt` | turns the HSMP mods on and UE4SS's developer mods off. Lines for your other mods are kept |
| HSMP programs | `Win64\hsmp\` (`hsmp-sidecar.exe`, `hsmp-server.exe`, `hsmp-query.exe`, `hsmp-master.exe`) | networking, the server browser, and hosting from the menu |
| `hsmp.cfg` | `Win64\hsmp.cfg` | tells the mods where the programs are and which server list to use |
| `hsmp_install.json` | `Win64\hsmp_install.json` | an install id, so the launcher finds its records again if you move the game |
| One line in `Engine.ini` | `%LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini`, section `[SystemSettings]`: `r.HairStrands.Streaming=0` | works around an engine crash in hair streaming while an arena loads ([details](../development/halfsword/io-dispatcher-crash.md)). The game sometimes rewrites this file, so the launcher puts the line back before every launch |
| Launch option | `-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0` | the same crash workaround, passed on every launch from the launcher |

While you play, HSMP also creates `Win64\hsmp_state\`. It holds your HSMP settings (`.settings.json`),
your character stats, a copy of your player key (`.player_key`) and HSMP's logs. Uninstall moves it out of the game folder instead of deleting it.

Outside the game folder, the launcher keeps:

| Folder | What is in it |
|---|---|
| `%LOCALAPPDATA%\HSMP\launcher\` | its install records, backed-up original files, `settings.json`, and `launcher.log` |
| `%LOCALAPPDATA%\HSMP\bin\hsmp-launcher.exe` | a verified copy of the launcher, for updates |
| `%LOCALAPPDATA%\HSMP\save_backups\` | your career save backups |
| `%LOCALAPPDATA%\HSMP\crash_reports\` | crash reports you chose to save |

The network helper `hsmp-sidecar.exe` also keeps two files in `%LOCALAPPDATA%\HSMP\`: `identity`, your
player identity (it is created the first time you host or join, and is what makes you the admin of
your own games and of servers that list your player key), and `known_servers.json`, the identity
keys of servers you have joined.

`mods.txt`, `hsmp.cfg` and `Engine.ini` are edited in their own text encoding, so lines from
other mods (including names in non-Latin scripts) are kept exactly as they were.

The launcher **never**:

* edits your career save (it only copies it);
* changes game files outside `HalfswordUE5\Binaries\Win64` (apart from the one `Engine.ini` line);
* kills a process (if the game is running, it asks you to close it);
* sends anything over the internet;
* adds a Windows Firewall rule. If you host games, see
  [Hosting from the menu](playing.md#hosting-from-the-menu).

### If you already use UE4SS mods

* If UE4SS is already installed in the new layout (a `ue4ss\` folder), the launcher backs up every
  file it replaces and puts it back on uninstall.
* An **old-layout** UE4SS (`UE4SS.dll` directly in `Win64`) would load instead of HSMP's build.
  The launcher refuses to install over it. Remove that UE4SS first, or run Steam's "Verify
  integrity of game files".
* A mod folder with an `enabled.txt` file is loaded by UE4SS whatever `mods.txt` says. If such a
  mod is one HSMP turns off, the launcher tells you to delete that `enabled.txt`.

## Updating

There is no automatic update yet. To update:

1. Download the new zip, check its SHA-256 (see [Install](#install)), and extract it.
2. Close Half Sword.
3. Start the launcher you already have: `%LOCALAPPDATA%\HSMP\bin\hsmp-launcher.exe`. (The
   launcher shows this path next to **Open another release...**.)
4. Click **Open another release...** and choose the new zip.
5. Wait for the game-version line. The game build is checked against the *new* release.
6. Click **Update <old> -> <new>**.

This way the new release is checked with the keys of a launcher you already trust, so a zip
signed by somebody else is refused. Starting the new zip's own `hsmp-launcher.exe` also works,
but then you rely on the release page's checksum again, as on a first install.

What Update does:

* It replaces only the files that changed, and removes files the new version no longer ships.
* Your original, pre-HSMP files stay backed up for uninstall.
* If you had edited one of HSMP's own files (a Lua script, `UE4SS-settings.ini`), your version is
  kept in `%LOCALAPPDATA%\HSMP\changed_by_you\<date_time>\` before it is replaced.
* If you changed `master_url` in `hsmp.cfg` by hand, your value is kept.
* If an update fails part-way, everything rolls back to the previous version.
* It refuses a release that is **older** than the installed one, unless you tick
  **Yes, downgrade to it**.
* It waits while Steam is still downloading or updating Half Sword.

## Repair

If an HSMP file was changed or deleted (for example by an antivirus or by Steam's "Verify
integrity"), the status line says so and the button reads **Repair**. Click it.

## Moving the game

If you move Half Sword to another drive or library with Steam ("Move install folder"), the
launcher finds its install record again through `Win64\hsmp_install.json`. It checks that the
HSMP files in the new folder match the record before it changes anything. A *copy* of the game
folder does not take over the original's record.

## Crash reports

When Half Sword has crashed since the launcher last looked, the **Crash reports** section says
so and offers **Save a redacted report**. The report is a zip in
`%LOCALAPPDATA%\HSMP\crash_reports\`, which you can attach to a bug report.

Before you choose, the launcher lists what a report contains:

* the crash dump (a snapshot of the game's memory stack, no screenshots);
* the crash context and the game log, redacted;
* the last 2000 lines of `UE4SS.log`, redacted (player names and IP addresses removed);
* the HSMP version, protocol and the game build hash.

It never includes chat logs, your HSMP settings or profile, or your career save.

**This version sends nothing.** The report is only saved to a folder. Choose **never ask** to turn
the prompt off.

## Command line

For support and power users. Run `hsmp-launcher.exe <command>` in a terminal. Without a command,
the window opens.

```text
status        [--game DIR] [--package DIR|ZIP]   game, package, build check, install status
verify        [--package DIR|ZIP]                signature + SHA-256 of every file
install       [--game DIR] [--package DIR|ZIP] [--allow-unsupported] [--no-save-backup] [--allow-downgrade]
uninstall     [--game DIR] [--forget-missing]
launch        [--game DIR] [--direct | --steam]  default: direct if Steam runs, else through Steam
backup-saves | list-backups | restore-saves <id> | find-game
```

## Next steps

* [Playing](playing.md): join and host games.
* [Troubleshooting](troubleshooting.md): when something goes wrong.
* [Career saves](career-saves.md): how your single-player career is protected.
* [Uninstall](uninstall.md): remove HSMP again.

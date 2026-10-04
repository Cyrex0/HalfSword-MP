# Uninstalling HSMP

You can remove HSMP at any time. The launcher puts your game folder back exactly as it was before
the first install.

Your career saves and your save backups are **not** touched by an uninstall.

On this page:

* [With the launcher (recommended)](#with-the-launcher-recommended)
* [If uninstall stops part-way](#if-uninstall-stops-part-way)
* [By hand, without the launcher](#by-hand-without-the-launcher)
* [What stays on your PC](#what-stays-on-your-pc)

---

## With the launcher (recommended)

1. Close Half Sword.
2. Start the launcher. Use the one from your release folder, or the copy in
   `%LOCALAPPDATA%\HSMP\bin\hsmp-launcher.exe`.
3. In section "2. Multiplayer mod", click **Uninstall...**.
4. Read the note and click **Yes, uninstall HSMP**.
5. The Messages panel ends with "HSMP removed: ... The game is back to how it was."

What Uninstall does:

* Before it removes anything, it finishes the career save check of any multiplayer session that
  crashed (see [Career saves](career-saves.md#3-the-crash-check)). If
  that check fails, Uninstall is refused, so your career is not changed further.
* Every file HSMP changed goes back to exactly how it was, byte for byte and under its original
  name. That includes UE4SS files and `mods.txt` if you had them before.
* Every file and folder HSMP added is removed: `dwmapi.dll`, `ue4ss\` (if HSMP created it),
  `hsmp\`, `hsmp.cfg` and `hsmp_install.json`.
* The `r.HairStrands.Streaming=0` line is removed from `Engine.ini` (or your original value is put
  back).
* HSMP's runtime files (the `hsmp_state` folder with your HSMP settings and logs) are **moved**,
  not deleted, to `%LOCALAPPDATA%\HSMP\uninstalled\<date_time>\`.
* If you edited a file HSMP manages (for example `mods.txt`) after installing, your edited
  version is kept there too, under `changed_by_you\`.

## If uninstall stops part-way

The message reads "HSMP was partly removed ..." and the status line says "Uninstall did not
finish". Usually an antivirus has quarantined or locked one of the backed-up original files, most
often an older `dwmapi.dll`. Everything else is already undone, and the launcher remembers
exactly what is left.

1. Restore the file from your antivirus quarantine, or allow it. Then click **Uninstall...**
   again. It carries on where it stopped.
2. If the backup is gone for good, tick **Give up on backups that are gone for good** and
   uninstall again. (Command line: `hsmp-launcher.exe uninstall --forget-missing`.) HSMP's version
   of that file is removed. If it was a game file, Steam's "Verify integrity of game files" puts it
   back.

Finish the uninstall, or install HSMP again, before you start Half Sword.

## By hand, without the launcher

Use this only if the launcher is gone or does not start.

> If you used other UE4SS mods **before** you installed HSMP, deleting the `ue4ss` folder also
> removes them. In that case, prefer the launcher's Uninstall, which puts your old UE4SS back.

1. Close Half Sword.
2. Open the game's binaries folder:
   `<Steam library>\steamapps\common\Half Sword\HalfswordUE5\Binaries\Win64`
   (In Steam: right-click Half Sword > Manage > Browse local files, then open
   `HalfswordUE5\Binaries\Win64`.)
3. Delete these files and folders:

   | Delete | What it is |
   |---|---|
   | `dwmapi.dll` | the UE4SS loader |
   | `ue4ss\` | UE4SS and the HSMP mods |
   | `hsmp\` | the HSMP programs |
   | `hsmp.cfg` | HSMP's configuration |
   | `hsmp_install.json` | HSMP's install id |
   | `hsmp_state\` | your HSMP settings, loadout and logs (optional: keep a copy if you want them) |

4. Optional: open `%LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini` in Notepad and
   delete the line `r.HairStrands.Streaming=0` under `[SystemSettings]`. It is harmless to leave
   it.
5. In Steam: right-click Half Sword > Properties > Installed Files > **Verify integrity of game
   files**. This puts back any game file that differs from Steam's version.
6. Optional: delete `%LOCALAPPDATA%\HSMP\launcher\` and `%LOCALAPPDATA%\HSMP\bin\`. They only
   hold the launcher's records and its copy. **Keep** `%LOCALAPPDATA%\HSMP\save_backups\`.

## What stays on your PC

These folders are not removed by Uninstall. Delete them by hand only if you are sure.

| Folder | What is in it |
|---|---|
| `%LOCALAPPDATA%\HSMP\save_backups\` | **Your career save backups.** Keep them until you are sure your career is fine. |
| `%LOCALAPPDATA%\HSMP\uninstalled\` | HSMP settings and logs moved out of the game folder. |
| `%LOCALAPPDATA%\HSMP\changed_by_you\` | Your versions of HSMP files you had edited. |
| `%LOCALAPPDATA%\HSMP\logs\`, `%LOCALAPPDATA%\HSMP\bug_reports\` | The logs of your last game runs, and bug reports you saved. |
| `%LOCALAPPDATA%\HSMP\launcher\` | The launcher's records and `launcher.log`. |
| `%LOCALAPPDATA%\HSMP\bin\` | The launcher copy used for updates. |
| `%LOCALAPPDATA%\HSMP\identity` | Your player identity. Keep it if you want the same player key after a reinstall (server admins name admins by that key). |
| `%LOCALAPPDATA%\HSMP\known_servers.json` | The identity keys of servers you joined. |

Your career saves themselves stay where the game keeps them:
`%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames`.

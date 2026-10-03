# Career saves

Half Sword keeps your single-player career in one file. HSMP is built so that multiplayer never
damages it. This page explains why that needs care, what HSMP does about it, and how to restore a
backup yourself.

On this page:

* [Where your saves are](#where-your-saves-are)
* [Why multiplayer is a risk to your career](#why-multiplayer-is-a-risk-to-your-career)
* [How HSMP protects your career](#how-hsmp-protects-your-career)
* [Backups](#backups)
* [Restoring a backup with the launcher](#restoring-a-backup-with-the-launcher)
* [Restoring a backup by hand](#restoring-a-backup-by-hand)
* [What not to do](#what-not-to-do)

---

## Where your saves are

| What | Where |
|---|---|
| Your saves | `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames` |
| The career | `GameProgress.sav` in that folder |
| The game's settings | `Settings.sav` in that folder |
| HSMP's backups | `%LOCALAPPDATA%\HSMP\save_backups\` |

To open one of these folders, press Windows+R, paste the path, and press Enter.

## Why multiplayer is a risk to your career

Half Sword saves your career on its own, often without asking. It writes `GameProgress.sav` every
time an arena loads, when your fighter goes down or dies, and when the end-of-fight screens
appear. A multiplayer match loads arenas and has deaths too. Without protection, every match would
overwrite your career with multiplayer data, such as wounds or the match's gear.

## How HSMP protects your career

HSMP uses three layers. Each one works even if another fails.

### 1. Saves are diverted during a session

While you are in a multiplayer session, HSMP catches the game's save calls and redirects them to
separate save slots whose names start with `HSMP_` (for example `HSMP_0_GameProgress.sav`). Your
real `GameProgress.sav` is not written. The first arena of a session may still *read* your
career, but it cannot change it.

If a save call cannot be redirected, HSMP blocks it instead, so nothing is written.

### 2. A backup before, a check after

When you host or join, HSMP's network helper (`hsmp-sidecar.exe`) first copies every career save
file to a new backup in `%LOCALAPPDATA%\HSMP\save_backups\`, with a checksum for each file.

When you leave the session, it checks your saves against that backup:

* any career file that changed or disappeared is put back from the backup;
* any new career save file that appeared is moved aside (to a `quarantine` folder in the backup);
* the copy the session changed is kept in the backup (in `mp_modified`), in case you need it.

This also means that changes to the game's own settings that you make during a multiplayer
session may be undone when you leave.

HSMP keeps the newest **5** of these session backups and deletes older ones. A backup from a
session that never finished its check is never deleted.

### 3. The crash check

If the game or the network helper crashes during a match, the "check after" in layer 2 does not
run. To catch that, the game runs the same check each time it starts (also when you start it from
Steam), before it can save anything. The launcher runs it too, each time it opens, after every
install or update, before **Launch through Steam** and before **Uninstall** (not while Half Sword
is running). Any career file the crashed session changed is put back from that session's
backup, and the Messages panel says so, for example:

> career guard: restored GameProgress.sav from the MP backup ... (an earlier multiplayer session
> ended without its save check)

A career file written *after* the crashed session ended (for example by a Half Sword started without
HSMP) is newer than that session, and the check leaves it alone.

The game protects the save on its own too: the same check runs at the start of every multiplayer
session, when you host or join, before the new session's backup is taken.

#### If the crash check fails

The launcher shows a red message like "the career save check after an earlier multiplayer crash
failed (...)" and refuses **Uninstall**, so your career is not changed further. Do not play your
career until it is fixed.

1. Close Half Sword.
2. In the launcher, look at the **Career saves** list. Pick the newest backup that says
   "career guard: ..." or one of your own backups from before the problem.
3. Click **Restore selected...**, then **Yes, restore**.
4. Close and reopen the launcher. If the message comes back, [report a bug](troubleshooting.md#reporting-a-bug)
   with `launcher.log`.

## Backups

There are two kinds of backup in `%LOCALAPPDATA%\HSMP\save_backups\`. Both show up in the
launcher's **Career saves** list.

| Kind | Made when | Folder name | Kept |
|---|---|---|---|
| Launcher backup | before the first install (if "Back up my career saves first" was ticked), when you click **Back up now**, and before every restore | `YYYYMMDD_HHMMSS` (date and time in UTC) | forever. The launcher never deletes a backup |
| Session backup ("career guard") | every time you host or join | `YYYYMMDD-HHMMSS-mmm` | the newest 5 |

Every copy is checked by SHA-256 when it is made.

Click **Back up now** in the launcher any time you want an extra copy, for example before trying
a new HSMP version.

## Restoring a backup with the launcher

1. Close Half Sword.
2. Start the launcher.
3. In **Career saves**, click the backup you want. Entries with ", career" contain
   `GameProgress.sav`.
4. Click **Restore selected...**, then **Yes, restore**.

Before it replaces anything, the launcher backs up your current saves automatically, so a restore
can always be undone. **Open folder** shows the backup folder in Explorer.

Steam Cloud may sync the restored files the next time you start the game. If Steam reports a
cloud conflict, choose the **local** files.

From the command line: `hsmp-launcher.exe list-backups`, then
`hsmp-launcher.exe restore-saves <backup id>`.

## Restoring a backup by hand

If the launcher is not available:

1. Close Half Sword. Make sure it is not running in Task Manager.
2. Optional but wise: turn off Steam Cloud for Half Sword (Steam > Half Sword > Properties >
   General).
3. Make a safety copy of your current `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames` folder, for
   example to your Desktop.
4. Open `%LOCALAPPDATA%\HSMP\save_backups\` and pick a backup folder.
   * In a **launcher backup** (`YYYYMMDD_HHMMSS`), the save files are directly in the folder.
   * In a **session backup** (`YYYYMMDD-HHMMSS-mmm`), the save files are in its `files\`
     subfolder. These are your saves from *before* that session started.
5. Copy `GameProgress.sav` (and any other `.sav` files you want back) into
   `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames`, and replace the existing files.
   Do not copy `backup.json` or `manifest.json`.
6. Start Half Sword and check your career. If Steam reports a cloud conflict, choose the
   **local** files.

## What not to do

* **Do not restore saves while Half Sword is running.** The game may overwrite them again.
* **Do not delete `%LOCALAPPDATA%\HSMP\save_backups\`**, even after uninstalling HSMP, until you
  are sure your career is fine.
* **Do not edit or delete `backup.json` or `manifest.json`** in a backup. Without them, the
  launcher cannot use the backup, and the session check cannot restore from it.
* **Do not copy `HSMP_*.sav` files over your career.** They hold multiplayer session data, not
  your career.
* **Do not end `hsmp-sidecar.exe` in Task Manager during a match.** It runs the save check when
  you leave. (If it happens anyway, the crash check catches it the next time you start the game, open the launcher, or
  host or join.)
* **After a multiplayer crash, start the game with HSMP installed before you play your career.**
  The game runs the crash check at start, before your career is saved again.

See also: [Installing HSMP](install.md), [Uninstall](uninstall.md), [FAQ](faq.md).

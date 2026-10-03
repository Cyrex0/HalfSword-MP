# Frequently asked questions

* [Is HSMP official?](#is-hsmp-official)
* [What does it cost?](#what-does-it-cost)
* [Can I get banned? Is there anti-cheat?](#can-i-get-banned-is-there-anti-cheat)
* [Does HSMP change my career save?](#does-hsmp-change-my-career-save)
* [Can I still play my single-player career with HSMP installed?](#can-i-still-play-my-single-player-career-with-hsmp-installed)
* [How many players can play together?](#how-many-players-can-play-together)
* [What game modes are there?](#what-game-modes-are-there)
* [Can I play over the internet?](#can-i-play-over-the-internet)
* [Is there a server list?](#is-there-a-server-list)
* [Do I need to run a server?](#do-i-need-to-run-a-server)
* [Does it work with other mods?](#does-it-work-with-other-mods)
* [Does it work on Linux or the Steam Deck?](#does-it-work-on-linux-or-the-steam-deck)
* [Half Sword just updated and HSMP says my build is not supported](#half-sword-just-updated-and-hsmp-says-my-build-is-not-supported)
* [Why does my antivirus complain?](#why-does-my-antivirus-complain)
* [Does HSMP send data anywhere?](#does-hsmp-send-data-anywhere)
* [How do I remove it?](#how-do-i-remove-it)

---

### Is HSMP official?

No. HalfSword-MP (HSMP) is an unofficial, fan-made multiplayer mod. It is not affiliated with, endorsed by,
sponsored by or approved by Half Sword Games or Game Seer Publishing. Please send HSMP questions
and bug reports to the HSMP project, not to the game's developers.

HSMP ships no Half Sword game code or assets. You need your own copy of Half Sword from Steam.

### What does it cost?

Nothing. HSMP is free and open source, licensed under **MIT OR Apache-2.0** (your choice). It
includes UE4SS, which is MIT-licensed. The source code is at
<https://github.com/Cyrex0/HalfSword-MP>.

### Can I get banned? Is there anti-cheat?

Half Sword is a single-player game on Steam with no anti-cheat (no VAC, no EAC). HSMP does not
call any Steam APIs and does not touch achievements, stats or leaderboards. The launcher only reads
Steam's library files to find the game, and checks the game's exe.

We are not aware of any way to get banned for using it. Like any mod, you use it at your own risk.
The real risks are to your saves and to stability, not to your Steam account. That is why HSMP
backs up and guards your career. See [Career saves](career-saves.md).

### Does HSMP change my career save?

It is designed not to. Half Sword saves your career automatically, also during fights, so HSMP:

1. redirects the game's saves to separate `HSMP_*` slots during a multiplayer session;
2. backs up your saves when you host or join, and puts back anything that changed when you leave;
3. repeats that check before the launcher's **Play** and **Uninstall**, in case a session crashed.

The launcher also backs up your saves before the first install, and you can make more backups
with **Back up now**. Changes to the game's own settings made during a multiplayer session may be
undone when you leave. Details: [Career saves](career-saves.md).

### Can I still play my single-player career with HSMP installed?

Yes. Start the game with **Play** in the launcher and use the game's own menu buttons as usual.
HSMP only adds its own buttons next to them. A few HSMP changes are always on, for example cutscenes
or videos that start while you are idle are skipped. To play the game exactly as it ships,
[uninstall HSMP](uninstall.md).

### How many players can play together?

Up to **8 players** per server, the host included.

### What game modes are there?

Today there is one mode, **DUEL**. With two players it is a one-on-one duel. With three or more it
is last man standing. The host picks the arena (Alley, Pit, Yard, Slums, Cellar, Lords Hall or East
Tower), the number of rounds (best of 1, 3, 5 or 7), and the kit rules (free, classes only, or
custom kits with a point budget). See [Playing](playing.md#the-lobby).

### Can I play over the internet?

Yes, with **DIRECT CONNECT**. One player hosts (from the menu, or with a dedicated server), and the
others type the host's public address, for example `203.0.113.5:7777`, in the server browser. The
host has to forward a UDP port on their router and allow it in Windows Firewall. See
[Hosting from the menu](playing.md#hosting-from-the-menu).

HSMP handles short connection drops: you get about 35 seconds to reconnect during a match.

### Is there a server list?

Not a public one yet. The server browser shows games on your local network (LAN) and a game hosted
on your own PC. For internet games, use **DIRECT CONNECT**.

Anyone can run a server list ("master server"). If a community list exists, add its address under
**SETTINGS > SERVER LISTS**. For server operators: [Master server](../hosting/master-server.md).

### Do I need to run a server?

No. **HOST GAME** in the menu runs a server on your PC for you. If you want a server that stays up
without anyone playing on it, see [Dedicated server](../hosting/dedicated-server.md).

### Does it work with other mods?

Partly, and not much tested.

* The launcher keeps the lines for your other UE4SS mods in `mods.txt`, and puts back any UE4SS
  files it replaced when you uninstall.
* It turns off UE4SS's developer mods (for example the console and the split-screen mod) while HSMP
  is installed.
* It refuses to install over an old-layout UE4SS (`UE4SS.dll` directly in the `Win64` folder).
* Other mods that change the same parts of the game (menus, fighters, saves, arenas) may conflict
  with HSMP. All players in a game should use the same mods. If something breaks, try without
  your other mods first.

### Does it work on Linux or the Steam Deck?

Not for players. The launcher and the in-game mod are made for Windows 10 and 11, and HSMP is not
tested under Proton or on the Steam Deck.

Server operators can run the dedicated server on Linux. See
[Linux](../hosting/linux.md).

### Half Sword just updated and HSMP says my build is not supported

Each HSMP release is tested against specific Half Sword builds. HSMP 0.1.0 supports Steam build
24185754. When Steam updates the game, the launcher shows a red line, and Install and Update are
blocked until an HSMP release supports the new build. **Play** still works, but menus or matches
may break. Wait for an HSMP update. Everybody in a game should use the same HSMP version.

### Why does my antivirus complain?

HSMP uses UE4SS to load its mods. UE4SS loads through a file called `dwmapi.dll` in the game
folder, and some antivirus programs flag that as suspicious. It is a false positive that every
UE4SS mod has. The launcher checks the file's signature and SHA-256 before it installs it.
The launcher is also not code-signed yet, which is why Windows SmartScreen warns about it.

### Does HSMP send data anywhere?

The launcher sends nothing over the internet. It does not even save a crash report unless you
click **Save a redacted report**, and it never uploads one.

In the game, HSMP talks to the game server you join, to the server lists you set, and to servers
in your browser list (to measure their ping). The server sees your nickname, your IP address, your
player key, your loadout and your character's movements, as it needs to for a multiplayer game.
The connection to the game server is encrypted.

### How do I remove it?

Open the launcher and click **Uninstall...**. Your game folder goes back exactly as it was. See
[Uninstall](uninstall.md).

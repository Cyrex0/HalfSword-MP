# Playing HSMP

This page explains the multiplayer menu, how to join and host games, the lobby, your loadout,
the settings, and what you see during a match.

If HSMP is not installed yet, start with [Installing HSMP](install.md).

On this page:

* [The HSMP buttons](#the-hsmp-buttons)
* [Before your first game](#before-your-first-game)
* [Joining a game](#joining-a-game)
* [Hosting from the menu](#hosting-from-the-menu)
* [The lobby](#the-lobby)
* [Your loadout](#your-loadout)
* [Settings](#settings)
* [Character](#character)
* [During a match](#during-a-match)
* [Leaving](#leaving)
* [Disconnects and reconnecting](#disconnects-and-reconnecting)
* [Keys and controller](#keys-and-controller)

---

## The HSMP buttons

Start the game with **Play** in the launcher. On the main menu, next to the game's own buttons,
HSMP adds a column of five buttons:

| Button | What it does |
|---|---|
| **HOST GAME** | Starts a server on your PC and opens your lobby. While you are in a session, this button reads **BACK TO LOBBY** (or **RECONNECTING...**) and takes you back to it. |
| **SERVER BROWSER** | Lists games you can join, and lets you type an address to connect directly. |
| **SETTINGS** | Your nickname, network and HUD settings. |
| **CHARACTER** | Four character stats that are sent to the server with your profile. |
| **QUIT MP** | Leaves any multiplayer session cleanly, then **quits Half Sword**. |

## Before your first game

Open **SETTINGS**, type your **NICKNAME**, and click **SAVE & BACK**. A nickname has 2 to 20
characters: letters, digits, spaces and `. _ -`, starting with a letter or digit. The default is
"Willie", so change it, or everybody will be called Willie.

## Joining a game

### There is no public server list yet

This release has no public internet server list. Out of the box, the server browser shows:

* games on your **local network (LAN)**, and
* a game hosted on **this PC**.

To join a game over the internet, ask the host for their address and use **DIRECT CONNECT**
(below). If a community server list exists, you can add its address under
**SETTINGS > SERVER LISTS** (see [Settings](#settings)).

### Using the server browser

1. Click **SERVER BROWSER**.
2. LAN games appear under a **LAN** heading at the top.
3. Click a row to select it, then click **JOIN**. You can also double-click a row, or select it
   and press Enter.

The browser has these tools:

| Control | What it does |
|---|---|
| Search box | Shows only servers whose name, map or mode contains what you type. |
| **HIDE FULL**, **HIDE EMPTY**, **HIDE LOCKED** | Hide full, empty or password-protected servers. |
| **PING** ANY / <50 / <100 / <150 | Show only servers that answer faster than this. |
| **CLEAR** | Resets every filter. |
| Column headers | Click to sort. Click again to reverse. |
| **REFRESH** (F5) | Asks the server lists and the LAN again. |
| **< PREV** / **NEXT >** | Turns pages (12 servers per page). |

You cannot join:

* a **full** server;
* a **password-protected** server. HSMP cannot send passwords yet;
* a server with a different HSMP version. The browser says "Version mismatch ... Update HSMP."

If the browser says **"Server list unavailable (...). Use DIRECT CONNECT or LAN"**, that is
expected in this release. LAN games and direct connect still work.

### Direct connect

1. Click **SERVER BROWSER**.
2. Type the address in the box at the bottom, for example `203.0.113.5:7777`.
   If you leave out the port, HSMP uses `7777`.
3. Click **CONNECT**, or press Enter.

The last five addresses you used appear as **RECENT** buttons. One click connects again.

After you join, the lobby opens. See [The lobby](#the-lobby).

## Hosting from the menu

When you click **HOST GAME**, HSMP starts a server on your own PC (a "listen server") and puts
you in the lobby as the host. Up to **8 players** can join.

* Your server listens on **UDP port 7777** by default. You can change this in
  **SETTINGS > HOST PORT** (1024 to 65535).
* When you, the host, close the lobby or leave, the server closes for everybody.

### Playing with friends on the same network (LAN)

Nothing else is needed. Your game shows up in your friends' server browser under **LAN**.

Keep the host port between **7777 and 7786**. The browser only looks for LAN games on those ports.

### Playing with friends over the internet

Your friends join with **DIRECT CONNECT** and your public address. For that to work:

1. **Forward the port on your router.** Forward **UDP** port 7777 (or your **HOST PORT**) to your
   PC. Every router is different. Look for "port forwarding" or "virtual server" in its settings.
2. **Allow it in Windows Firewall.** The launcher does not add a firewall rule. The first time
   you host, Windows may ask whether `hsmp-server.exe` may use the network. Allow it.
   If you clicked "Cancel" by mistake, allow `hsmp-server.exe` (in the game's
   `Binaries\Win64\hsmp\` folder) in Windows Security > Firewall > "Allow an app through
   firewall".
3. **Give your friends your public IP address and port**, for example `203.0.113.5:7777`. You can
   find your public IP by searching "what is my IP" in a web browser.

Some internet providers put you behind a shared address (CGNAT), and then port forwarding does
not work. In that case, someone else has to host, or you can run a dedicated server on a rented
machine. See [Ports and firewall](../hosting/ports-and-firewall.md) and
[Dedicated server](../hosting/dedicated-server.md).

## The lobby

Everybody who joins a server first lands in the lobby. The host's screen is called
**HOST LOBBY**.

**Left side: PLAYERS.** Up to 8 rows. Each row shows the player's status (**READY** or
**NOT READY**), their name, their class, and their ping. Your row says **(YOU)**. The host's row
says **HOST**.

**Right side: MATCH.** Only the host can change these. Everybody sees what the server has chosen.

| Setting | Choices |
|---|---|
| **ARENA** | Alley, Pit, Yard, Slums, Cellar, Lords Hall, East Tower. The tile marked **SERVER ARENA** is the one everybody will load. |
| **ROUNDS** | **BEST OF 1**, **3**, **5** or **7**. |
| **KIT RULES** | **FREE** (any kit, no points limit), **CLASSES ONLY** (class presets only), or **CUSTOM** (custom kits within a point budget). |
| **BUDGET** | For **CUSTOM** only: 12, 20, 30, 45 or 60 points. |
| **MODE** | Set by the server. Today it is **DUEL**. |

**Buttons at the bottom:**

| Button | Who | What it does |
|---|---|---|
| **MARK READY** | everybody | Tells everybody you are ready. It then reads **READY [x]**. Press it again to cancel. |
| **LOADOUT** | everybody | Opens the loadout screen. See [Your loadout](#your-loadout). |
| **START MATCH** | host | Starts the match on the server's arena. It works once every player is ready. With only you in the lobby, it reads **START (SOLO)**. Other players see **HOST STARTS (n/m)**. |
| **LEAVE** / **CLOSE LOBBY** | everybody / host | Leaves the server. For the host, **CLOSE LOBBY** closes the server for everybody. |

**Servers without an admin.** On a dedicated server that has no admin connected (the server's
owner names admins by player key), nobody presses START MATCH: once at least 2 players are connected
and everybody is READY, the server starts the match after 5 seconds. The lobby shows the settings
read-only and **AUTO START (n/m)**; un-readying cancels the start. When you host from the menu, you
are always the admin of your own server, also after a reconnect.

The line above the buttons always tells you what is going on, for example
"Waiting for Alice to be READY" or "Everyone is READY - START the match."

**Host tools.** As the host, click a player's row to show two buttons:

* **MAKE HOST** gives that player the host tools (arena, rules, START) as well. The server keeps
  you as admin too. The grant lasts until the server stops.
* **KICK** removes the player from the server.

### Your player key

Each HSMP install has a **player key**: 64 hex characters, created on first use and kept in `%LOCALAPPDATA%\HSMP\identity`. Servers recognise you by it. It is how you stay
the host of your own game after a reconnect, and how a dedicated server's owner makes you an admin.
To give your key to a server owner, open a terminal in the game's
`HalfswordUE5\Binaries\Win64\hsmp\` folder and run:

```powershell
.\hsmp-sidecar.exe --print-player-key
```

The key is not a secret: every server you join sees it. After you have hosted or joined once, the
same key is also in `HalfswordUE5\Binaries\Win64\hsmp_state\.player_key`.

Every change goes to the server first. The lobby shows **WAITING FOR SERVER...** until the server
accepts it, or a **REFUSED** message with the reason.

When the host starts the match, everybody loads the arena. If someone's game takes longer, the
match waits for them (**WAITING FOR PLAYERS**).

## Your loadout

Click **LOADOUT** in the lobby.

* **CLASS** (left): pick a ready-made kit.

  | Class | Kit |
  |---|---|
  | **KNIGHT** | Full plate and a longsword. Slow, nearly cut-proof. |
  | **MAN-AT-ARMS** | Mail over padding and a poleaxe. |
  | **DUELIST** | Light and quick: arming sword and buckler. |
  | **BRUTE** | Gambeson and a great axe. |
  | **PEASANT** | Rags and a pitchfork. Nothing to lose. |
  | **CUSTOM** | Build your own kit (where the kit rules allow it). |
  | **GAME GEAR** | Keep the game's own gear, no points. |

* **EQUIPMENT** (centre): your armour by body region (HEAD, TORSO, ARMS, LEGS), your weapons
  (RIGHT HAND, LEFT HAND), and your look (face, hair, cloth tint).
* **Item grid** (right): pick an item for the selected slot. Filter by **TIER** and **TYPE**.
  Items you cannot use under the current kit rules are greyed out.
* A points bar shows how much of the budget your kit uses (in **CUSTOM** mode).

Buttons: **RESET TO CLASS** puts back the class's own kit. **HOST RULES** (host only) sets the kit
rules. **SAVE & BACK** saves your kit and sends it to the server. **BACK** leaves without saving
(it asks first if you changed something).

The server checks every kit. If yours breaks the rules, the server replaces it, and the loadout
screen tells you why. Your kit is shown in the lobby as **YOUR KIT: ...**.

## Settings

Click **SETTINGS** on the main menu.

| Setting | What it does | Default |
|---|---|---|
| **NICKNAME** | Your name in lobbies, the scoreboard and the kill feed. | Willie |
| **REGION** | Shown in the server browser for games you host. AUTO means not set. | AUTO |
| **SERVER LISTS** | The server lists (master servers) to ask, in order, separated by commas, up to 4. Empty means the default from `hsmp.cfg`. **TEST** asks each one and tells you which answer. | empty |
| **HOST PORT** | The UDP port the game you host listens on (1024 to 65535). Joiners need it open. | 7777 |
| **SEND RATE** | How many movement updates you send per second: 30, 60, 90 or 120 Hz. Higher is smoother but uses more upload. | 60 Hz |
| **HUD** | Shows the multiplayer HUD during a match. | ON |
| **KILL FEED** | "Who slew whom", top right. | ON |
| **NET INDICATOR** | Ping and packet loss, bottom right: ALWAYS, WHEN BAD, or OFF. | ALWAYS |
| **PEER AVATARS** | Shows the other players' bodies. Turn it off only to debug. | ON |

**SAVE & BACK** checks and saves everything. **BACK** leaves without saving. **RESET DEFAULTS**
resets everything except your nickname. HUD settings apply from your next match.

Your settings are stored in the game folder, in `HalfswordUE5\Binaries\Win64\hsmp_state\.settings.json`.

There is no UI scale setting. The HSMP menus and the HUD scale themselves to the game window: they
are laid out for 1920x1080 and scaled by the window's height (or its width on screens narrower than
16:9). On an ultrawide screen they keep the 16:9 size and stay centred. Text never gets smaller
than 9 pixels.

## Character

**CHARACTER** shows four stats: **STRENGTH**, **AGILITY**, **INTELLIGENCE** and **STAMINA**, each
0 to 10, at most 40 in total. **SAVE & PUBLISH** saves them and sends them to the server with your
profile.

## During a match

A match is a series of rounds. The mode today is **DUEL**: with two players it is a duel, with
three or more it is last man standing. The last fighter standing wins the round. If everybody
falls, the round is a draw and nobody scores. The first player to win enough rounds (for example 2
in a best of 3) wins the match.

How hits work: your game sees your weapon hit another fighter and reports the hit. The server
checks it against where that fighter was at the moment you saw it (lag compensation), and if it
holds, the hit is replayed in the victim's own game with the game's own damage, blood and wounds.
So both players see the same wound, and a hit that the server refuses has no effect.

### The HUD

| Where | What |
|---|---|
| Top centre | Round banner, timer and the score. |
| Top left | Every opponent (up to 7): name, **CON** and **BODY %**, a body bar and a stamina bar. The row turns red while that fighter bleeds. |
| Bottom left | You: a body bar with **BODY %**, then **CON**, your raw health (`hp`) and **BLEED** while you bleed, and a stamina bar. |
| Top right | Kill feed, and notices such as players joining or leaving. |
| Bottom right | Net indicator: NET GOOD, NET OK, NET POOR or NET BAD, with ping. |
| Centre | Big messages: ROUND n, FIGHT!, ROUND OVER, YOU DIED, SPECTATING, VICTORY, MATCH OVER. |

What the numbers mean. A Half Sword fight is not decided by raw health: health only drops on hard
hits to the head, neck and torso, and it regenerates. So the HUD shows what matters:

| Label | Meaning |
|---|---|
| **CON** | Consciousness, 0 to 100. Low consciousness means the fighter is knocked out or close to it. |
| **BODY %** | The condition of the body parts, as a weighted average. Head and neck count most, then the upper and lower torso; each arm and leg counts a little. An arm or leg at zero costs about 6 %. |
| **BLEED** | The fighter is losing blood. |
| `hp` | Raw health, shown small for your own fighter only. |

You and your opponent see the same numbers: both screens read them from the same data.

* **Hold TAB** to see the scoreboard.
* **After you die**, you watch the rest of the round. With more than one fighter left, **Q** and
  **E** switch whom you watch.
* **If you join a match that is already running**, you watch until the next round, then you
  fight.

### The pause menu

Press **Esc** in a match. The game does **not** pause: the fight goes on around you.

* **RESUME** closes the menu.
* **SETTINGS** opens the game's own settings.
* **LEAVE MATCH** leaves the server (press it twice to confirm). In a duel, your opponent wins
  by forfeit.
* **QUIT GAME** leaves the server and quits Half Sword (press it twice to confirm).

### After the match

The result screen offers **REMATCH** and **BACK TO LOBBY**. If you do nothing, everybody returns
to the lobby after a few seconds.

## Leaving

* In the lobby: **LEAVE** (or **CLOSE LOBBY** if you are the host).
* In a match: **Esc**, then **LEAVE MATCH**.
* To quit the game: **QUIT MP** on the main menu, or **QUIT GAME** in the pause menu. Both leave
  the server cleanly first.

When you leave, HSMP checks your career saves and puts back anything the session changed. See
[Career saves](career-saves.md).

## Disconnects and reconnecting

HSMP is built to survive short internet hiccups.

**If your connection drops during a match:**

1. The centre of the screen shows **RECONNECTING...**, and the round is frozen for you. A
   countdown shows how long HSMP keeps trying (about 35 seconds).
2. If the connection comes back in time, you carry on where you were.
3. If it does not, you see **CONNECTION LOST** with two buttons:
   * **RECONNECT** tries to rejoin the same server.
   * **BACK TO MENU** gives up and returns to the main menu.

**If another player drops:** you see **OPPONENT DISCONNECTED**. The server waits up to 30 seconds
for them. If they come back, the round is replayed. If they do not, the remaining player wins by
forfeit (in a duel), or the round carries on (with three or more players). A player who
reconnects keeps their wins and watches until the next round.

**Other messages you may see:**

| Message | Meaning |
|---|---|
| **YOU WERE KICKED** | The host removed you. |
| **SERVER CLOSED** | The host closed the server, or the server shut down. |
| **CONNECTION REJECTED** | The server refused you. The reason is shown, for example a full server or another HSMP version. |
| **SERVER RESTARTED** | The server restarted. The match was reset and you are back in the lobby. |
| **MATCH ENDED** | The match ended while you were reconnecting. |
| **DISCONNECTED** | You joined this server from another game. |
| **Cannot reach the server at ...** (in the lobby) | Nothing answered for 15 seconds. Check the address and that the host's UDP port is open, then **LEAVE** and try again. |

## Keys and controller

The HSMP menus work with mouse, keyboard and gamepad.

| Key | Gamepad | In the HSMP menus |
|---|---|---|
| Arrow keys | D-pad / left stick | Move between buttons |
| Enter or Space | A (bottom face button) | Press the focused button |
| Esc | B (right face button) | Back |
| Tab / Shift+Tab | | Next / previous group of controls |
| Page Up / Page Down | LT / RT | Previous / next page |
| Q / E | LB / RB | Switch tabs |
| F5 | Y (top face button) | Refresh |

| Key | In a match |
|---|---|
| Hold TAB | Scoreboard |
| Q / E | Switch whom you watch (after you died) |
| Esc | HSMP pause menu |

Fighting uses the game's normal controls.

## See also

* [Troubleshooting](troubleshooting.md)
* [FAQ](faq.md)
* [Ports and firewall](../hosting/ports-and-firewall.md)
* [Dedicated server](../hosting/dedicated-server.md)

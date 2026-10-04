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

Start Half Sword from Steam as usual. On the main menu, next to the game's own buttons,
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

### The public server list

The server browser lists:

* internet games from the public server list (`https://master.halfswordmp.workers.dev`),
* games on your **local network (LAN)**, and
* a game hosted on **this PC**.

When you host a game, it is added to the public list too. Players on the internet can join it
only if your router forwards the game's UDP port to your PC (see
[Ports and firewall](../hosting/ports-and-firewall.md)). You can always join a game by its
address with **DIRECT CONNECT** (below), and add another server list under
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

The **PING** column shows **--** for a server that did not answer, and **NAT** for one that did
not answer directly but sits behind a router the server list can punch through: joining it tries
that on its own ("CONNECTING THROUGH YOUR ROUTER..."). The line under the table says the same
for the selected server: reachable, will try NAT traversal, or unreachable.

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

Your game is listed in the public server browser, and friends can also join with **DIRECT
CONNECT** and your public address. HSMP tries to make your PC reachable on its own:

1. **Your router port.** When you host, HSMP asks your router to open the UDP port (UPnP, PCP or
   NAT-PMP). The host lobby says how that went:
   * **"Router port opened automatically (UPnP)"**: nothing to do.
   * **"Couldn't open your router port automatically ... Forward UDP 7777 to this PC."**: your
     router has UPnP turned off, or does not support it. Either turn UPnP on in the router's
     settings, or forward **UDP** port 7777 (or your **HOST PORT**) to your PC by hand (look for
     "port forwarding" or "virtual server"). Until then, many players can still get in through
     NAT traversal (below), but not all.
   * You can stop HSMP from touching the router: **SETTINGS > HOSTING > ROUTER PORT: OFF**.
2. **Allow it in Windows Firewall.** The launcher adds the rule when it installs HSMP (accept its
   Windows prompt). If you said no, click **Fix firewall** in the launcher's install section.
3. **For direct connect, give your friends your public IP address and port**, for example
   `203.0.113.5:7777`. You can find your public IP by searching "what is my IP" in a web browser.

**NAT traversal.** When a joiner gets no answer from you, their game asks the server list to
"punch" through your router: your server sends a few small packets towards them, which opens
your router for that player, and the join continues. Their lobby shows **"CONNECTING THROUGH
YOUR ROUTER..."** meanwhile. This works with most home routers. It does not work when your
router (or your provider's) gives every connection a different port (a "symmetric" NAT); then
the joiner sees **"The host's network blocks incoming connections; ask them to forward UDP
7777"**, and you have to forward the port.

Some internet providers put you behind a shared address (CGNAT), and then port forwarding does
not work. NAT traversal often still does. If it does not, someone else has to host, or you can
run a dedicated server on a rented machine. See [Ports and firewall](../hosting/ports-and-firewall.md)
and [Dedicated server](../hosting/dedicated-server.md).

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
| **MODE** | The game mode; the **MODE** button opens the GAME MODE screen. See [Game modes](#game-modes). |

**Buttons at the bottom:**

| Button | Who | What it does |
|---|---|---|
| **MARK READY** | everybody | Tells everybody you are ready. It then reads **READY [x]**. Press it again to cancel. |
| **LOADOUT** | everybody | Opens the loadout screen. See [Your loadout](#your-loadout). |
| **MODE** | everybody | Opens the GAME MODE screen: the host picks the mode and its options, everybody picks a team when teams are **PICK**. |
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
| **ROUTER PORT** | While you host, opens the HOST PORT on your router automatically (UPnP, PCP or NAT-PMP) and closes it again when you stop. OFF: forward the port by hand. | ON |

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

## Game modes

The host picks the mode on the lobby's **GAME MODE** screen (the **MODE** button). Every mode is
played in rounds; the first player or team to win enough rounds (2 in a best of 3) wins the match.

| Mode | How to win a round |
|---|---|
| **DUEL** / **FFA** | Be the last fighter standing. If everybody falls together, the round is a draw. |
| **TEAMS** (team elimination) | Be on the last team standing. Teammates cannot hurt each other unless the host turns **FRIENDLY FIRE** on. |
| **HILL** (King of the hill) | Stand on the hill with no enemy on it: you (or your team) score a point per second. The first to the target (60 s by default) wins. A fight on the hill scores nothing for anybody. Dying still puts you out of the round, and when the round clock runs out the most points win. The top banner says how far the hill is and in which direction ("HILL: 12 m AHEAD-LEFT", "ON THE HILL"). |
| **ROULETTE** (weapon roulette) | Last fighter standing, but everybody fights with the same random weapon and armour set, a new one each round. The countdown names it. Your own loadout comes back after the match. |
| **BRAWL** | Last fighter standing, with fists only and no armour. |
| **DEATHMATCH** | Kill as often as you can before the round clock (5 minutes by default) runs out. When you die, you come back a few seconds later at a spawn point away from the others: your game reloads the arena ("RESPAWN IN 3", then "RESPAWNING..."), and you fight again once you are placed. The most kills win the round; a tie goes to sudden death, where the next kill wins. |

**Teams.** HILL, ROULETTE, BRAWL and DEATHMATCH can be played in teams too. With **AUTO** teams
the server balances the teams when the match starts; with **PICK** teams every player picks
**RED**, **BLUE**, **GREEN** or **GOLD** on the GAME MODE screen (**ANY** = the smallest team).
Teams spawn on their own side of the arena and swap sides every round. When a whole team drops,
the match waits for it like a duel waits for a dropped opponent.

## During a match

A match is a series of rounds (see [Game modes](#game-modes) for how each mode is won). In
**DUEL**, with two players it is a duel, with three or more it is last man standing. If everybody
falls, the round is a draw and nobody scores.

How hits work: your game sees your weapon hit another fighter and reports the hit. The server
checks it against where that fighter was at the moment you saw it (lag compensation), and if it
holds, the hit is replayed in the victim's own game with the game's own damage, blood and wounds.
So both players see the same wound, and a hit that the server refuses has no effect.

### The HUD

| Where | What |
|---|---|
| Top centre | Round banner, timer and the score. In team modes one cell per team (round wins, and this round's points or kills); in King of the hill the banner shows who holds the hill and where it is; with a round clock the timer counts down (SUDDEN DEATH after a tied deathmatch). |
| Top left | Every opponent (up to 7): name, **CON** and **BODY %**, a body bar and a stamina bar. The row turns red while that fighter bleeds. |
| Bottom left | You: a body bar with **BODY %**, then **CON**, your raw health (`hp`) and **BLEED** while you bleed, and a stamina bar. |
| Top right | Kill feed, and notices such as players joining or leaving. |
| Bottom right | Net indicator: NET GOOD, NET OK, NET POOR or NET BAD, with ping. |
| Centre | Big messages: ROUND n (with the round kit in weapon roulette and brawl), FIGHT!, ROUND OVER, YOU DIED, RESPAWN IN n, SPECTATING, VICTORY, MATCH OVER. |
| Centre (hold TAB) | Scoreboard: wins, kills / deaths, ping, status; team tags in team modes. |

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

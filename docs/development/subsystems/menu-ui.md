# Menu UI (HSMPMenu)

Every MP screen is a native sub-screen on the Startup menu's own CanvasPanel. There are no overlays
and no viewport widgets. All screens are built with one kit, `ui_kit.lua`, which is required. The menu
never changes the level itself: it asks the Director ([director-contract.md](director-contract.md)).

Offline tests: `hsmp-tools lua-test menu_ui` (every screen and state at several resolutions and DPI
scales, keyboard-only and gamepad navigation, the lobby states, the leave / quit lifecycle and the
idle cost) and `hsmp-tools lua-test ui_scale` (the scaling rule). `hsmp-tools lua-test menu_ui -- dump`
renders every screen as ASCII.

## Files

| File | What |
|---|---|
| `mods/HSMPMenu/Scripts/ui_kit.lua` | The kit: design tokens, primitives (rect, text, input, button, chips, tiles, pager, bar, spinner), the screen frame, the focus manager, the hint bar, the confirm strip, and perf counters |
| `.../main.lua` | The ribbons, the screen framework, the input queue (keyboard and gamepad), LOBBY and CHARACTER, HOST / JOIN / leave / quit, the autotest hooks |
| `.../settings.lua` | `.settings.json` storage (merged, atomic), validators, and the SETTINGS screen |
| `.../jsonlite.lua` | The menu's JSON reader (tolerant), a flat writer and an atomic write |
| `.../browser.lua` | SERVER BROWSER and DIRECT CONNECT |
| `.../classes.lua` | LOADOUT ([classes-loadout.md](classes-loadout.md)) |
| `.../commands.lua` | Commands with pending / accepted / refused results ([director-contract.md](director-contract.md) section 8) |
| `.../travel.lua` | Travel requests to the Director |
| `.../local_master.lua` | Starts a local `hsmp-master` for HOST when the primary server list is on this machine and nothing answers there |
| `mods/shared/hsmp_ui_scale.lua` | The one UI scaling rule, shared with HSMPHud |
| `tools/hsmp-tools/lua-tests/menu_ui.lua`, `ui_scale.lua`, `lib/umg_mock.lua` | Offline tests and the UMG mock |

## Top level

HSMPMenu injects five ribbons as a second column next to the native main-menu buttons:
**HOST GAME**, **SERVER BROWSER**, **SETTINGS**, **CHARACTER** and **QUIT MP**. A sub-screen hides the
native buttons and the ribbons; BACK restores them.

## UI scaling (`shared/hsmp_ui_scale.lua`)

Scaling is automatic; there is no user setting. HSMPMenu and HSMPHud use the same module.

- **Units.** Physical px (the game window), canvas units (physical px divided by the viewport's DPI
  scale), and design units (a 1920x1080 reference canvas). Every layout is written in design units and
  multiplied by one scale `s`.
- **The rule.** `s = short side / 1080`, then reduced so the 1920-wide design never exceeds the canvas
  width (4:3, 5:4, portrait). On 16:9 and wider `s = ch / 1080`, so an ultrawide (3440x1440,
  5120x1440) gets exactly the 2560x1440 layout, centred, never stretched. `s` is clamped to 0.1..4.
- **Fonts** use the same `s` and never drop below 9 physical px (`MIN_FONT_PX`). Below a 1024x576
  window the minimum shrinks with the window, so a layout that fits still fits.
- **Safe margin:** 24 design units, and at least 2 % of the short side. A centred panel is clamped to
  the safe area and to a 16:9 area of the canvas height.
- **Text fit** is estimated with a glyph advance of 0.6 × the font size and a line height of 1.4 ×.
- **Viewport probe.** `U.probe` reads the viewport size and DPI scale through
  `WidgetLayoutLibrary` (BlueprintCallable statics). The `GameViewportClient` size functions are
  C++-only and cannot be called from Lua. The probe runs on the game thread, looks everything up fresh
  and reads no SoftObject property.
- **Resize.** `U.watcher` reports a new size once it has held for a few polls, so a window drag
  rebuilds the screen once, at the end.

## The kit

### Tokens and frame

| Token | Values (design units) |
|---|---|
| Spacing `K.SP` | xs 4, sm 8, md 12, lg 16, xl 24, xxl 32 |
| Type `K.TS` | title 34, h1 24, h2 19, body 17, label 15, small 14 |
| Heights `K.BH` | action 56, field 44, chip 40, row 40, tab 36, hint 34 |
| Widths `K.BW` | action button 240 (every action button on every screen), chip max 260 |
| Panel `K.PANEL` | 1760 x 1000, padding 24 (screens may ask for a smaller panel) |
| Colours `K.C` | panel, section, track, rule, **focus** (amber, 1.00 / 0.82 / 0.36), text, dim, head, on_text, good, ok, warn, bad |

**Frame (`K.frame`).** Every screen has the same skeleton:
- the title at top left and a status at top right (with a spinner while busy);
- a rule under the title;
- the content;
- a message line;
- the action bar: left actions from the left edge, the primary action, and BACK / LEAVE at the far
  right;
- the hint bar: keys on the left, the focused control's help or disabled reason on the right.

### Button states

A button is a Border for the colour, a transparent UButton for the hit test, and the label on top. The
Border is recoloured per state: normal and hover; **on** (selected); **pending** (amber: sent, not
answered); **disabled** (dim; a click shows the reason); **pressed** (a 150 ms flash on Enter or A).

Colour is never the only signal. Every state also shows text: READY / NOT READY, SERVER ARENA /
REQUESTED..., or the reason.

### Focus manager

- Every button, chip, tile and input is focusable (`opts.focus=false` opts out) and belongs to a
  focus group (`K.group`).
- The focused control gets a 3-unit amber ring made of four Borders moved into place.
- **Arrows / D-pad** use spatial nearest-neighbour navigation.
- **TAB / Shift+TAB** cycle the sections. Each section remembers its last focused control.
- Hovering with the mouse moves the focus too, so hover and focus are one visual state.
- On re-entry the focus returns to the control focused last time on that screen, or to the screen's
  default.
- A confirm strip is modal for the keyboard. The focus stays inside it until it closes.

### Input router

`main.lua` registers each key **once**, because UE4SS cannot unregister binds. A press is queued, and
the 33 ms game-thread poll dispatches it to the active screen, or at the top level to the ribbon
column, or nowhere.

| Action | Keyboard | Gamepad |
|---|---|---|
| move | arrows | D-pad, left stick (hold repeats after 0.4 s, every 0.12 s) |
| select / type | Enter, Space | A |
| back | Esc | B |
| next / previous section | Tab / Shift+Tab | — |
| tab (loadout slot tabs) | Q / E | LB / RB |
| page | PgUp / PgDn | LT / RT |
| refresh (browser) | F5 | Y |

- Shift+TAB fires both TAB binds in UE4SS. A TAB waits one poll, so the shifted one wins and the press
  counts once.
- **Text guard:** while an input has keyboard focus (or had it within 0.3 s), only Enter (done, runs
  `on_submit`), Esc (cancel) and Tab (done, next field) reach the menu. Every other key belongs to the
  text box.
- **Top level:** TAB moves the focus into the MP ribbon column. Arrows and Enter stay with the native
  menu and its own panels (its Settings sliders use LEFT / RIGHT). UP / DOWN / ENTER work in the
  column, LEFT / ESC leave it, and moving the mouse onto a native button drops the column focus.
  Leaving a screen with the keyboard puts the focus on the ribbon that opened it.
- **Gamepad:** `PlayerController:IsInputKeyDown` is polled every 50 ms for the `Gamepad_*` keys. Three
  failed calls disable the poll, which is logged. `menu_gamepad = 0` in `hsmp.cfg` turns it off. In the
  menu's UI-only input mode the controller may never see pad keys; then the pad does nothing and the
  keyboard still reaches everything. The first pad press is logged (`input: gamepad seen`).
- The hint bar switches to pad labels after a pad press. While typing it shows
  `[ENTER] DONE [ESC] CANCEL [TAB] NEXT FIELD`.

### Performance

- A screen is built once per entry. Every later change goes through an in-place setter that writes
  only when the value changed.
- `K.perf` counts per-tick time, renders, polls, widget writes and rebuilds. Every 10 s while a screen
  is open it logs a line such as
  `menu perf (lobby, 10 s): 0.85 ms/s Lua | tick avg 0.04 max 1.0 ms (200) | render avg 0.30 max 1.0 ms (20) | 0 widget writes | 0 builds`.
- The tests require an idle lobby to make **0 rebuilds and 0 widget writes** in 10 s, and an idle
  browser to rewrite only its "Updated Ns ago" text.
- `os.clock` has 1 ms resolution on Windows, so the averages are sums of quantised samples.

## Screens

### LOBBY (main.lua)

```
+--------------------------------------------------------------------------------------------------------------+
| HOST LOBBY  -  Willie's game                                                             CONNECTED   42 MS    |
|--------------------------------------------------------------------------------------------------------------|
| PLAYERS  3 / 8                                       1 READY   MATCH                                          |
|  STATUS     NAME                  CLASS      PING   HOST TOOLS  ARENA                          SERVER: PIT    |
| [NOT READY  Willie  (YOU)  HOST   -          42 MS]            [ALLEY     ][PIT       ][YARD     ][SLUMS    ] |
| [READY      Mate                  KNIGHT+    38 MS][MAKE HOST][KICK] 12 SPAWNS  8 SPAWNS ...                   |
| [NOT READY  Peasant               DUELIST+   -    ]             REQUESTED.. SERVER ARENA                       |
| [           (free slot)                           ]            [CELLAR    ][LORDS HALL][EAST TOWER]           |
| [ ... 8 rows ...                                  ]            ROUNDS                       SERVER: BEST OF 3 |
|                                                                [BEST OF 1][BEST OF 3][BEST OF 5][BEST OF 7]   |
|                                                                KIT RULES                    SERVER: CUSTOM 30 |
|                                                                [FREE     ][CLASSES ONLY ][CUSTOM          ]   |
|                                                                BUDGET [12 PTS][20 PTS][30 PTS][45 PTS][60 PTS]|
| YOUR KIT: DUELIST  11 PTS                                      MODE  DUEL   (set by the server)              |
| START: Waiting for Peasant to be READY (and you)                                     <- message line         |
| [MARK READY] [LOADOUT]                                              [START MATCH] [CLOSE LOBBY]              |
|  [ARROWS] MOVE  [ENTER] SELECT  [ESC] BACK  [TAB] NEXT SECTION      Select a player for the host tools       |
+--------------------------------------------------------------------------------------------------------------+
```

The lobby reads the server's state from shared memory (`shared/hsmp_session.lua`): the `link` record
(status, my peer id, admin flag, transport metrics) and the `session` snapshot (roster, config, phase).

**Roster.**
- One row per connected seat: READY / NOT READY, the name with `(YOU)` and `HOST` tags, the class
  (`KNIGHT+` means an edited kit), and the ping.
- **Ping:** your own row shows the link's smoothed RTT (`srtt_ms`, else `rtt_ms`). Other rows show the
  server-measured RTT from the sidecar's peer directory, or `-` until it is known.
- The `HOST` tag is on every connected seat whose roster `admin_role` is ADMIN or OWNER.

**Host tools** (admins only).
- Selecting a row (click, or ENTER on it) shows **MAKE HOST** and **KICK** on that row.
- Both ask in the inline confirm strip, with CANCEL focused.
- They are the commands `promote{peer}` and `kick{peer}`. Accepted is shown when the server answers,
  or inferred when the player leaves the roster (kick) or shows as admin (promote). A refusal shows the
  server's reason.
- After MAKE HOST you lose the host tools: the controls follow the link record's admin flag.

**Match settings.** Admins edit them; everyone else sees them greyed, and a click says "Only the host
changes the match settings".
- **Arena tiles:** name and spawn count (from `shared/hsmp_arenas.lua`). The selected tile is the
  server's arena (SERVER ARENA). An unanswered pick shows as pending (REQUESTED...). The server decides.
- **ROUNDS:** BEST OF 1/3/5/7, sent as `best_of`. The chip follows the server's `best_of`.
- **KIT RULES:** FREE / CLASSES ONLY / CUSTOM, plus BUDGET chips, sent as the `kit_rules` command and
  as the host's `kit_rules_req` slot. They show as accepted when the server's rules (the `kit_rules`
  slot, else the snapshot's config) match. BUDGET is greyed outside CUSTOM, with the reason.
- **MODE:** read-only. The server's `SetConfig` refuses a mode change ([modes.md](modes.md)).
- On connect as host, the saved boot arena, best-of and kit rules are re-sent.

**START.**
- With an admin present, START is enabled only for an admin and only when the server would accept it:
  connected, the server in the lobby, and either you are alone (`START (SOLO)`) or everyone including
  you is READY. Otherwise it is greyed, and the message line, the hint bar and a click give the reason:
  `Mark yourself READY first`, `Waiting for Mate, Bob to be READY (and you)`, or
  `Not connected to the server yet`. A pending START shows `STARTING...`. Non-admins see
  `HOST STARTS (n/m)`.
- **No admin on the server** (a dedicated server with no admin connected, or the listen host dropped):
  READY is the vote. The button reads `AUTO START (n/m)`, and once everyone is READY the server starts
  the match after a short delay (`STARTING IN n`).

**Message line priority:**
1. the travel state (`LOADING PIT...`, `TRAVEL REFUSED: <reason>`, `THE SERVER REPORTED NO ARENA - WAITING`);
2. "Cannot reach the server..." with advice after 15 s without a connection;
3. the newest command (`ARENA PIT: WAITING FOR SERVER...`, `KICK MATE: ACCEPTED`, `ROUNDS REFUSED - <reason>`);
4. START's reason or a hint.

**Status (top right):** `CONNECTING...` and `RECONNECTING...` with a spinner, or `CONNECTED   42 MS`.

**CLOSE LOBBY** (host) asks first when other players are connected. **LEAVE** (joiner) leaves at
once. ESC does the same as the button.

### SETTINGS (settings.lua)

```
+---------------------------------------------------------------------------------------------------+
| SETTINGS                                                                          UNSAVED CHANGES |
|---------------------------------------------------------------------------------------------------|
| PROFILE                                                    HUD (IN A MATCH)                       |
| NICKNAME      [Willie________________]                     HUD             [ ON  ][ OFF ]         |
|               6 / 20 characters                            KILL FEED       [ ON  ][ OFF ]         |
| REGION        [AUTO][EU][NA][SA][ASIA][OCE]                NET INDICATOR   [ALWAYS][WHEN BAD][OFF]|
| NETWORK                                                    PEER AVATARS    [ ON  ][ OFF ]         |
| SERVER LISTS  [https://a.example:7778, https://b..] [TEST] HUD settings apply from your next match.|
|               a.example OK (12 servers)  |  b.example NO ANSWER                                   |
| HOST PORT     [7777____]                                                                          |
|               UDP port your hosted game listens on                                                |
| SEND RATE     [30 HZ][60 HZ][90 HZ][120 HZ]                                                       |
| Unsaved changes - SAVE & BACK keeps them, BACK discards them                                      |
| [RESET DEFAULTS]                                                     [SAVE & BACK] [BACK]         |
+---------------------------------------------------------------------------------------------------+
```

**Editing.**
- Every control is **staged**. SAVE & BACK validates and writes. BACK (or ESC) discards, and asks
  first when something changed (DISCARD / KEEP EDITING).
- **Typed fields validate live** with a frame colour and a line under the field:
  - nick: 2-20 characters, letters, digits, space and `. _ -`, starting with a letter or digit, no
    double spaces;
  - server lists: comma separated, each `http(s)://host[:port][/path]`, at most 4. Empty means the
    `hsmp.cfg` default, which is shown;
  - host port: 1024-65535.
- SAVE is greyed with the first error as its reason.
- **TEST** asks each listed server list for `/v1/servers` (async curl) and reports per URL
  (`a.example OK (12 servers) | b.example NO ANSWER`). The deadline is 8 s, so the spinner never runs
  forever.
- **RESET DEFAULTS** resets everything but the nickname.

**`<state>/.settings.json`.** One line, written atomically, merged: keys other mods own (for example
`pose_stiffness`) survive. Unsafe values fall back to defaults at load.

| key | type (default) | reader |
|---|---|---|
| `nick` | string (`Willie`) | sidecar `--nick`, lobby, HSMPHud |
| `server` | `"127.0.0.1:<port>"` (`127.0.0.1:7777`) | the port a hosted server binds; the host's sidecar connects there |
| `send_hz` | 30/60/90/120 (60) | HSMPSync send rate |
| `region` | `""`/EU/NA/SA/ASIA/OCE (`""` = auto) | `HSMP_REGION` of a hosted server; the browser highlights your region |
| `master_url` | `""` or `"https://a, https://b"` | the browser's server lists (first = primary) and the hosted server's registration; the `HSMP_MASTER_URL` env var still wins |
| `hud` | bool (true) | HSMPHud master switch |
| `hud_killfeed` | bool (true) | HSMPHud kill feed |
| `hud_net` | `always`/`bad`/`off` (`always`) | HSMPHud net indicator |
| `avatars` | bool (true) | HSMPAvatars: show peer avatars |
| `lobby_map`, `lobby_mode` | string (`Map_Arena_Alley`, `Best of 3`) | the next HOST's boot arena and best-of (re-sent on connect) |
| `host_kit_mode`, `host_kit_budget` | number (-1 = none, 0) | the host's last KIT RULES pick, re-sent on connect as host |

### SERVER BROWSER (browser.lua)

```
+--------------------------------------------------------------------------------------------------------------+
| SERVER BROWSER                                                                             [ REFRESH ]       |
| [search............] [HIDE FULL][HIDE EMPTY][HIDE LOCKED]  PING [ANY][<50][<100][<150]  [   CLEAR   ]         |
| [PW][SERVER NAME                ][MAP      ][MODE     ][PLAYERS v][PING ][REGION / VER     ]                 |
| [    Server 03                   Pit        duel         3/8       41    NA  0.1.0         ]  <- selected     |
| [ PW Server 15                   Pit        duel         3/8      125    NA  0.1.0         ]                  |
| [    ... 12 rows; LAN / INTERNET section rows when LAN servers answer ...                  ]                  |
| [ Updated just now | 18 servers, 18 shown | via master.example   SELECTED: Server 03 (10.0.0.3:7777) ]       |
|                         [< PREV]   PAGE 1 / 2   [NEXT >]                                                     |
| RECENT  [10.0.0.4:7777] [203.0.113.5:7777] ...                                                               |
| ENTER or JOIN to join the selected server.                                                                   |
| [direct ip:port..........] [CONNECT]                                              [ JOIN ] [ BACK ]          |
|  [ARROWS] MOVE  [ENTER] SELECT  [ESC] BACK  [TAB] NEXT SECTION  [PGUP/PGDN] PAGE  [F5] REFRESH               |
+--------------------------------------------------------------------------------------------------------------+
```

**Data.** `hsmp-query.exe` fetches the server lists (`GET /v1/servers` on each `hsmp-master`) and
UDP-pings every server, then prints a tab-separated result on stdout. The native module starts it
with a captured pipe and no window, and the browser tick polls for its exit, so nothing blocks the
game thread. Without `hsmp-query.exe` the browser falls back to `curl.exe` and a tolerant JSON scan
(no pings).

- **Server lists:** the list saved in SETTINGS first, then `hsmp.cfg`'s `master_url` (a primary and
  fallbacks), tried in order. The status bar names the one that answered. When none answers, the table
  says "Server list unavailable (...). Use DIRECT CONNECT or LAN"; every refresh has a deadline (12 s,
  plus 4 s per fallback).
- **LAN:** `hsmp-query --lan` broadcasts the browser query on the LAN and to 127.0.0.1 over
  `hsmp.cfg` `lan_ports` (default `7777-7786`). Servers that answer are listed in a LAN section above
  the internet list, so local play works with no master at all.
- **Auto refresh** on entering the screen when the list is older than 20 s.

**Keyboard and mouse.**
- Moving onto a row with the keyboard selects it. ENTER joins it. A double-click joins. Clicking a
  column header sorts. PgUp / PgDn page. F5 refreshes.
- **DIRECT CONNECT:** type `ip:port` and press CONNECT or ENTER. The last 5 direct addresses are kept
  as one-click RECENT chips.
- The lobby title carries the joined server's name.

### LOADOUT (classes.lua)

- class cards;
- EQUIPMENT slots grouped HEAD / TORSO / ARMS / LEGS / WEAPONS / LOOK;
- the picker with slot tabs, TIER and TYPE chips, a tile grid and a pager;
- the points bar and the server verdict (spinner while waiting) above the message line;
- the APPEARANCE row: `GAME DEFAULT` or `FACE n HAIR n TINT n`.

Q / E switch the slot tab, PgUp / PgDn page the grid, and ESC is BACK. BACK with unsaved edits asks
first (DISCARD / KEEP EDITING). Data, persistence and server validation are in
[classes-loadout.md](classes-loadout.md).

### CHARACTER

Four 0-10 chip rows (strength, agility, intelligence, stamina) plus a total. Editing is staged:
SAVE & PUBLISH writes `<state>/.my_character.json`, and BACK asks before it discards changes. No other
component reads this file yet.

## Session lifecycle

- **Processes.** The server, the sidecar and the local master are started through the native module
  (`IPC.spawn`: CreateProcessW, no window, no shell, arguments as an array). The game keeps the PIDs it
  spawned and stops them with `IPC.proc_kill`, which works only on its own process handles. There are
  no pid files and no kills by image name.
- **Shared memory.** Before the sidecar is started, the game creates the shared-memory segment
  (`IPC.open()`) and passes its name: `--ipc shm:<name>`. The sidecar verifies the segment's game PID
  against `--parent-pid`. Without the segment or the game PID the sidecar is not started and the lobby
  shows a reinstall hint ([ipc-shared-memory.md](../ipc-shared-memory.md)).
- **`--parent-pid <game pid>`** is passed to the server, the sidecar and the local master, so they exit
  when the game does.
- **HOST** starts `hsmp-server --bind 0.0.0.0:<port> --max-peers 8 --map <arena>
  --owner-key-file <state>\.player_key`, with `HSMP_LISTEN_HOST=1`, `HSMP_LOBBY_MAP`,
  `HSMP_MASTER_URL`, `HSMP_SERVER_NAME` (`<nick>'s game`), `HSMP_SERVER_MODE` and `HSMP_REGION` in its
  environment. The host's sidecar writes its player key to `<state>/.player_key` before it connects, so
  the host is the server's owner (admin) by key, never by being first to join. With
  `HSMP_LISTEN_HOST=1`, the server closes when the host leaves and tells every joiner "Host closed the
  server". The sidecar is started 0.5 s later with `--server 127.0.0.1:<port> --state-dir <state>
  --nick <nick>`.
- **JOIN** starts only the sidecar, with `--server <addr>`. `HSMP_NETSIM_ADDR=host:port` (test rigs)
  routes the client through the `hsmp-tools netsim` impairment proxy instead.
- **CANCEL / LEAVE:**
  1. The menu returns at once and sends a G2S `leave` record.
  2. It polls the sidecar status every 100 ms, for up to 1.5 s, for `ended` (an absent status counts
     after 3 polls in a row).
  3. Only then does it kill the sidecar / server this game spawned (by handle; a child that already
     exited is logged as gone).
  - A new HOST / JOIN inside that window leaves the new session alone.
- **QUIT MP** runs the same teardown before the console `quit`, so the sidecar, server and master never
  outlive the game.
- **Session over:** the sidecar status `ended` (counted only after this session's sidecar was seen
  live), or a latched `conn_state` `lost` written during this session, closes the lobby screen and
  cleans up. A `lost` that offers RECONNECT keeps the session: the HUD's RECONNECT / BACK TO MENU
  decide. A terminal status (kicked, server closed, replaced, rejected) waits for the Director's modal
  before the menu leaves. The menu draws no second modal; HSMPHud's explains it.
- **Travel** to the server's arena is requested only while the sidecar status is `connected`.
- **Dev keys:** F11 re-injects the menu only with `HSMP_DEV=1`.

## Styling contract for HSMPHud

HSMPHud's `hud_kit` copies `ui_kit`'s palette and uses the same scaling module. Panels that take input
(MP pause, connection modal, result screen) use the same conventions:
- action buttons 240x56 design units, with BACK / LEAVE at the far right and the primary action to its
  left;
- `K.C.focus` (1.00, 0.82, 0.36) for a focus ring;
- Esc = back, Enter = activate;
- a footer hint line in `K.C.dim` at type size 14.

## Tests

`hsmp-tools lua-test menu_ui` covers:

- **A resolution x DPI matrix.** Each case runs lobby, lobby with host tools, the lobby confirm,
  loadout, loadout rules, browser (full and empty), settings (with errors) and character. Each screen
  is checked for:
  - the panel inside the canvas and every widget inside the panel;
  - no overlapping clickables or texts, no text half over a button;
  - no text taller or wider than its slot, **no clipped label**, fonts ≥ 9 physical px;
  - **every control reachable with the arrows** from the entry focus, and **TAB visits every section**.
- **Navigation:** the ribbon column and settings by keyboard only (text guard, live validation,
  confirm, save, merged file); the browser by keyboard (row select, ENTER join, PgUp / PgDn, F5,
  Shift+TAB, ESC back to the ribbon, search guard); the lobby host tools by keyboard; gamepad (D-pad,
  hold-repeat, A, B, pad hints, self-disable); hover focus, the press flash and greyed-click reasons.
- **Settings:** validators, sanitising at load, legacy files; region and port when hosting; the saved
  primary list; the listen-host flag; TEST per-URL results and its timeout.
- **Lobby:** the joiner view (greyed with reasons, no confirm on LEAVE); connecting, the timeout
  advice, reconnecting, connected with the ping; session over (ended, latched for this session, stale
  ignored, leftover `ended` ignored); travel only while connected; kit rules chips as commands.
- **Processes:** `--parent-pid`; the leave request and its wait; QUIT tears down before quitting.
- **Perf:** the cost line is logged; an idle lobby has 0 rebuilds and 0 writes; an idle browser has
  only age-text writes.

## Checking it in game

`Mods/HSMPMenu/Scripts/` must contain `ui_kit.lua`, `settings.lua`, `jsonlite.lua`, `browser.lua`,
`classes.lua`, `commands.lua`, `travel.lua`, `local_master.lua` and the deployed `shared/*.lua`
copies.

1. **Load log:** `input: N key binds (...)`; `game pid N: passed as --parent-pid` (or
   `game pid unknown ...`: the native module is missing); no `ui_kit.lua missing`.
2. **Top level:** TAB puts an amber ring on HOST GAME; UP / DOWN move it; ENTER on SETTINGS opens it.
   In the native Settings, LEFT / RIGHT / Enter still work as before.
3. **SETTINGS:** ENTER on NICKNAME, then type; arrow keys move the caret, not the focus. The frame
   turns red for `x`. TEST shows a spinner, then a result. SAVE & BACK writes one line to
   `.settings.json` and keeps other keys.
4. **Gamepad:** press the D-pad in SETTINGS. `input: gamepad seen` means it works; otherwise the menu's
   UI-only input mode hides pad keys and the keyboard covers everything.
5. **HOST GAME:** a CONNECTING spinner, then `CONNECTED  N MS`. Your row reads NOT READY; START is
   greyed with "Mark yourself READY first" until you are READY (or alone).
6. **Second instance joins:** the host selects Mate; MAKE HOST / KICK appear and KICK asks first;
   the row disappears and the note reads `KICK MATE: ACCEPTED`.
7. **CLOSE LOBBY with a joiner:** the confirm, then `leave (CANCEL): sidecar ended after 0.x s` (or
   `did not end - stopping it`). The joiner sees HSMPHud's "Host closed the server" and its menu leaves
   the lobby.
8. **QUIT MP during a session:** no `hsmp-sidecar` / `hsmp-server` process is left afterwards.
9. **Resolutions:** at 1280x720, 4K and an ultrawide, nothing is clipped or overlapping, and text stays
   at least 9 px.

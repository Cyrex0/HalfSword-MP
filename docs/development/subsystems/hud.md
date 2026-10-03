# In-match HUD (HSMPHud)

HSMPHud is the one in-match HUD: round banner and timer, score strip, vitals for me and every
opponent, kill feed and server notices, the TAB scoreboard, the net indicator, the centre phase
messages, and three small interactive panels (connection modal, MP pause, match result). The HUD
itself only reads state and never decides anything. The game's own vignettes (`Vignette_Pain`,
`HPDmg*`, `HeadDmg`) are untouched.

Tests: `cargo test -p hsmp-hud-test` (Rust + mlua, Lua 5.4, with a mocked UE4SS / UMG in
`tests/hsmp-hud-test/tests/mock_ue.lua`; part of `cargo test --workspace`) and
`hsmp-tools lua-test ui_scale hud_net`.

## Files

| File | What |
|---|---|
| `mods/HSMPHud/Scripts/main.lua` | Thread shim, world guard, host (native `UI_HUD_C` or viewport), TAB and Esc keybinds, 100 ms game-thread tick |
| `.../hud_state.lua` | Read-only views of the shared-memory records and `.settings.json` (pure Lua, no UObject) |
| `.../hud_model.lua` | Snapshot → what to show (pure; the clock is passed in). Phase edges, FIGHT! flash, kill feed, net grade, centre message, scoreboard rows, panels |
| `.../hud_kit.lua` | Display-only widgets in HSMPMenu ui_kit's style, anchor-aware, no buttons |
| `.../hud_view.lua` | Layout (pure `V.layout(cw, ch, n, dpi)`), built once per host, updated in place |
| `.../hud_panel.lua` | The interactive panels (the only widgets that take clicks) |
| `mods/shared/hsmp_ui_scale.lua` | The UI scaling rule shared with HSMPMenu (below) |

The deploy script copies every `HSMP*` mod and `mods/shared/*.lua` into each mod's `Scripts/`;
`mods/mods.release.txt` enables it in `mods.txt`.

## Screen

```
+------------------------------------------------------------------------------------------+
| Mate                 CON 82 BODY 71%   +-----------------------------+   You slew Mate   |
| [##########.........]                  | ROUND 2  /  BEST OF 3  0:47 |   P3 left the fight|
| [====.....]                            |[Willie  1][ Mate  0][ P3  0]|   Mate joined     |
| Peasant         DOWN CON 40 BODY 55%   +-----------------------------+   (5 lines, 6 s,  |
| [###########.........]                                                     then fade)    |
|  ... up to 7 opponents                     FIGHT!                                        |
|                              Willie 1  -  Mate 0  -  P3 0          <- centre (0.5, 0.3)  |
|                                                                                          |
| YOU                                                                                      |
| [#######  BODY 63%  ##########....]                                                      |
| [======.....]                                                                            |
| CON 75  hp 100  BLEED                                    (o) NET OK  92 ms  0.4%  +-18   |
+------------------------------------------------------------------------------------------+
```

**Scoreboard** (hold TAB; the centre message and the result panel are hidden meanwhile):

```
            +-----------------------------------------------------------+
            | SCOREBOARD  -  BEST OF 3  -  ROUND 2  -  LORDS HALL       |
            |  #  PLAYER                      WINS     PING    STATUS   |
            |  1  Mate                          2     88 ms     ALIVE   |
            |  2  Willie  (you)                 1     42 ms      DEAD   |
            |  3  Peasant                       0         -   LOADING   |
            |                                     release TAB to close  |
            +-----------------------------------------------------------+
```

### Vitals: CON / BODY % / BLEED

Raw Health is not what decides a Half Sword fight. It only drops on hard torso, head or neck hits,
never below a per-part floor, and the game regenerates it. Fights end by knock-out (consciousness),
broken parts (Snap Neck, head crush, disabled limbs) and blood loss. The HUD therefore shows:

| Label | Source (vitals index) | Meaning |
|---|---|---|
| `CON` | `Consciousness` (11) | 0..100 |
| `BODY %` | weighted mean of the part healths | head 3, neck 3, upper torso 2, lower torso 1.5, back 0.5, each arm and leg 0.75. The fatal parts dominate; a limb at 0 costs about 6 %. Unknown parts are skipped. The bar is green above 60 %, amber above 30 %, red below. |
| `BLEED` | `Bleeding` (15) | shown when above 0.25. An opponent's row turns red instead (no room for the word at 1280x720). |
| `hp` | `Health` (0) | raw Health, small, my own block only |
| ST bar | `Stamina` (13) | 0..100 |

My values come from my own `vitals` slot, an opponent's from its `peer_vitals` slot
([vitals.md](vitals.md)), through the same reader and the same 1/64 quantisation
(`hud_state.vitals_from_record`): both screens show identical numbers. A record without part or
consciousness fields falls back to plain `HP n  ST n`. `DOWN` (Fallen or Downed flag) and `DEAD` are
always written out.

### Regions and anchors

Each widget is anchored to its own corner, so it stays put even if the canvas-size estimate is off:

| Region | Anchor | Content |
|---|---|---|
| top-left | (0, 0) | opponents (max 7): name, `DOWN` / `DEAD`, `CON n BODY n%`, BODY bar, ST bar |
| top-centre | (0.5, 0) | `ROUND n / BEST OF k` + timer; score strip, one cell per player (max 8). Own cell highlighted, dead players dimmed |
| top-right | (1, 0) | kill feed and notices (5 lines, 6 s, then a 1 s fade) |
| centre | (0.5, 0.3) | phase message: title + one sub line |
| scoreboard | (0.5, 0.5) | TAB scoreboard |
| bottom-left | (0, 1) | YOU: BODY bar with `BODY 63%` / `DEAD`, ST bar, `CON 75  hp 100  BLEED` |
| bottom-right | (1, 1) | net dot + text |

The timer counts up during a live round, or down when the session carries a round deadline. It
reads `LOADING` while players load, `PAUSED m:ss` while paused, `END` at match over.

**Colour is never the only signal.** The net indicator says its grade in words, and players are
marked `DEAD` / `DOWN` / `LOADING` in text.

## UI scaling (`mods/shared/hsmp_ui_scale.lua`)

One rule for every HSMP widget (HSMPMenu's screens and ribbons, the HUD, the panels). It is pure Lua
except for the engine probe, so the offline tests compute every rect with the same code. There is
no user setting; the scale follows the window and the game's DPI scale.

- **Units.** Physical px (the game window), canvas units (physical px divided by the viewport's DPI
  scale, `GetViewportScale`), and design units (a 1920x1080 reference canvas). Every layout is
  written in design units and multiplied by one scale `s`.
- **The rule.** `s = short side / 1080`, then reduced so the 1920-wide design never exceeds the
  canvas width (4:3, 5:4, portrait). On 16:9 and wider `s = ch / 1080`, so an ultrawide (3440x1440,
  5120x1440) gets exactly the 2560x1440 size, centred, never stretched.
- **Fonts** use the same `s` and never drop under 9 physical px (`MIN_FONT_PX`) on windows of at
  least 1024x576 (`FLOOR_W` x `FLOOR_H`); below that the minimum shrinks with the window so a layout
  that fits still fits. The HUD's scale never goes under the one where its smallest design font
  (12) meets that minimum.
- **Safe margin:** 24 design units, and at least 2 % of the short side. A centred panel is at most
  the safe width and never wider than a 16:9 area of the canvas height.
- **Probe** (`U.probe`, game thread only, fresh lookups): `WidgetLayoutLibrary.GetViewportSize` and
  `GetViewportScale` first (BlueprintCallable statics), then the game viewport client, then
  `GameUserSettings:GetScreenResolution()`. The viewport client's size functions are C++-only in
  UE 5.4; reading them alone made every window look like 1920x1080 and drew menus 1.5x too large on
  a 1280x720 window.
- **Resize:** `U.watcher` answers true once a new canvas size has held for two polls, so a window
  drag rebuilds once. HSMPHud checks about once a second; an open panel is rebuilt too.

The offline layout test runs 1280x720 (canvas and DPI-scaled), 1920x1080, 2560x1440, 2560x1080 and
3840x2160 at DPI 1 and 2. At each one every widget is inside the canvas, no two texts overlap, every
text fits its slot, and the regions are disjoint.

## Centre phase messages

| State | Title | Sub line |
|---|---|---|
| countdown, players still loading | WAITING FOR PLAYERS | `loading: Mate, ...` |
| countdown | ROUND n+1 | score, `fight in N` |
| live, first 1.5 s after the countdown | FIGHT! | score |
| live, me dead (server), < 3 s | YOU DIED | `the round plays on` |
| live, me dead, later | SPECTATING `<NAME>` (HSMPMatch's `spectate` bus record, else the first living foe) | `waiting for the round to end - Q / E switch - TAB scores` (Q / E only with more than one target) |
| live, the server has me out but my pawn lives (late joiner) | SPECTATING `<NAME>` | `you join the next round - ...` |
| roundover | ROUND OVER | winner or draw, score, `next round in N` (score only while the result is pending) |
| match_over | VICTORY / `MATE WINS THE MATCH` / MATCH OVER | score, `opponent forfeited` if so, `back to lobby in N`; plus the result buttons |
| lobby after REMATCH (the Director holds us in the arena for 15 s) | REMATCH | `waiting for everyone to be ready - N s` |
| paused | OPPONENT DISCONNECTED | `waiting for them to reconnect - N` |
| `conn_state` `reconnecting` | RECONNECTING... | `the round is frozen for you - N s left` |
| `conn_state` `lost` / `notice` | (nothing: the connection modal shows it) | |
| no `conn_state` record: link lost / rejected | CONNECTION LOST / CONNECTION REJECTED | `reconnecting... Ns` / the reason |

HSMPHud is the only centre-banner owner. `HSMP_HUD_BANNER=0` turns the centre messages off
(debugging only). Every message comes from the current state, so it is shown once. The centre is
hidden under the TAB scoreboard and under a modal panel.

**Toasts.** Server notices (`notice` events) share the feed with the kill feed: `Mate joined`,
`Mate left`, `The host Al left; Mate is the host now` (amber), `Mate failed to load: ...`, admin
changes and sudden death. Each `event_id` is shown once, and the same text never twice within 5 s.

**Kill feed** (`death` events): `You slew Mate`, `Mate slew You`, `Mate died`, `You died`,
`P3 left the fight`. The event cursor starts at the first poll, so old deaths are not news. Deaths
are deduplicated by (match, victim, round); the match id comes from the record, or from a local
generation that moves when the session falls back to the lobby or a round number goes down.

## Panels (`hud_panel.lua`): the only interactive part

A panel is its own `UserWidget` in the viewport at z 2000. It exists only while open. Each button is
a flat Border, a transparent `UButton` and a label (HSMPMenu ui_kit's pattern). Clicks are
edge-polled with `IsPressed` every 33 ms (`OnClicked` cannot be bound from Lua). In an arena, cursor
and UI input are on only while a panel is open, and the game gets input back when it closes; in the
menu world the input mode is left alone. What to show comes from `hud_model.panel` (pure). Clicks go
through `hud_model.click`, which writes the `ui_request` bus key for the Director
(`{seq, want, reason}`; the Director logs its verdict, there is no ack).

| Panel | When | Buttons |
|---|---|---|
| Connection modal (dim backdrop) | `conn_state` `lost` or `notice`, until answered (menu or arena) | the record's `actions`: RECONNECT, BACK TO MENU, OK |
| MP pause (dim backdrop) | Esc in an MP arena. The native `UI_Pause_C` / `UI_Pause_Eng_C` is removed at once and `SetGamePaused(false)` runs: the match keeps running. Esc again closes it | RESUME, SETTINGS (the game's settings screen; if it is not found, the next Esc shows the native menu), LEAVE MATCH and QUIT GAME (each asks to confirm; the confirm reverts after 4 s) |
| Match result (a button row under the banner) | `match_over` | REMATCH (then "REMATCH: READY", or "REMATCH: STARTING WHEN READY" for an admin), BACK TO LOBBY |

## Inputs (HSMPHud writes only `ui_request`)

All reads go through the shared-memory facade `shared/hsmp_ipc.lua` (`HSMP_IPC`); see
[../ipc-shared-memory.md](../ipc-shared-memory.md). Missing records and fields are tolerated.
State is read every second tick (about 5 Hz).

| Source | Writer | Used for |
|---|---|---|
| `link` record (`hsmp_session.lua` `HS.link()`) | sidecar | status, my peer id, admin flag, reject reason, transport metrics (`rtt_ms`, `loss_pct_10s` / `loss_pct`, `jitter_ms`, `rx_age_ms`, `metrics_wall_ms`) |
| peer directory (`IPC.peer_dir()`) | sidecar | roster nicks and server-measured RTT (scoreboard PING) |
| `session` record (`HS.view()`) | sidecar | state, round, countdown, best-of, scoreboard, waiting / ready lists, last winner, result reason, arena, round deadline |
| `vitals` slot | HSMPCombat | my own vitals |
| `peer_vitals` slots | sidecar | each opponent's vitals; read only for ids in the roster |
| `death` events | sidecar | kill feed |
| `notice` events | sidecar | toasts |
| `conn_state` bus key | HSMPMatch Director | the one connection state (`seq`, `state`, `reason`, `title`, `text`, `remaining_s`, `actions`, ...; [director.md](director.md)) |
| `spectate` bus key | HSMPMatch | who the camera follows after my death |
| `.settings.json` | HSMPMenu | `nick`, and the HUD switches below |

**HUD switches** (`.settings.json`, HSMPMenu SETTINGS → "HUD (IN A MATCH)"; taken only while no
match runs, so a change applies from the next match):

| Key | Values | Default |
|---|---|---|
| `hud` | `true` / `false`: off hides vitals, opponents, kill feed and net (banner, centre, notices and panels stay) | `true` |
| `hud_killfeed` | `true` / `false` (notices stay) | `true` |
| `hud_net` | `always`, `bad` (only when the grade is POOR or BAD; alias `when_bad`), `off` | `always` |

**When the HUD is shown:** in a `Map_Arena_*` world, no native menu open, and an MP session exists:
the link is `connected` (or `reconnecting`) and the sidecar's header heartbeat is fresh
(`shared/hsmp_session.lua`, 5 s). A link record left over from a dead sidecar never makes the HUD
appear. It also shows while reconnecting, while the REMATCH hold lasts, and for a link lost
mid-match.

**Net indicator.** The figure is the ping/pong round trip. Four grades:

- RTT: GOOD < 80 ms, OK < 150 ms, POOR < 250 ms, BAD above.
- Loss (the 10 s window, lifetime as a fallback): ≥ 1 % at best OK, ≥ 3 % at best POOR, > 8 % BAD.
- Jitter (`jitter_ms`, the transport's RTT variation): > 50 ms one grade worse, > 120 ms two.
- No datagram for 1.5 s: BAD. Metrics unchanged for 5 s while connected: `NET BAD  NO DATA`.
- Not connected: `NO LINK - RECONNECTING [Ns]` or `OFFLINE`. No metrics yet: `NET --` (grey dot).

## Host, input, safety

**Host:**

- **Preferred:** `FindAllOf("UI_HUD_C")`, the instance that is `IsInViewport()` and owned by the
  current PlayerController. Its `WidgetTree.RootWidget` is a CanvasPanel. HSMPHud adds one
  `CanvasPanel` there (anchors 0..1, offsets 0, z 500, HitTestInvisible). All widgets are outered to
  the HUD's WidgetTree, so they live and die with the native HUD.
- **Fallback:** if no native HUD appears within 3 s of the world becoming valid, or the native root
  is not a CanvasPanel, HSMPHud builds its own `UserWidget`: UserWidget → WidgetTree → CanvasPanel
  root → populate → `AddToViewport(990)` → HitTestInvisible.
- **Native HUD lost:** about once a second a fresh `FindAllOf` checks that our HUD (by address) is
  still in the viewport. If not, the references are dropped untouched and the fallback is used for
  the rest of that world.

**Hidden:** outside `Map_Arena_*` worlds (the menu, the hub); while a native menu is in the viewport
(`UI_Pause_C`, `UI_Pause_Eng_C`, `UI_PhotoMode_C`, `UI_Gallery_C`, `UI_KeyBinds_C`,
`UI_GameSettings_C`, `UI_DisplaySettings2_C`, `UI_AudioSettings_C`, `UI_Controls_C`; one class is
checked per tick); without an MP session; in the `lobby` state. Hiding collapses our root only.

**Input:**

- The HUD has no Button, CheckBox, EditableTextBox or ScrollBox, and no HUD widget is ever
  `Visible`: everything is HitTestInvisible or Collapsed. Only panel buttons and the modal backdrop
  are `Visible`.
- No `SetInputMode*` or cursor calls, except GameAndUI + cursor while a panel is open in an arena
  and GameOnly when it closes.
- Keys: `RegisterKeyBind(Key.TAB)` and `Key.ESCAPE`, routed to the game thread by the shim. A TAB
  release is read with `PlayerController:IsInputKeyDown(Tab)`. If that FKey call fails, TAB
  toggles instead and auto-closes after 8 s.

**Thread and world safety:**

- The standard HSMP thread shim: every loop and delayed callback runs on the game thread.
- The shared world guard (`shared/hsmp_wg.lua`) with the OpenLevel / OpenLevelBySoftObjectPtr
  pre-hooks, plus a LoadMap pre-hook.
- The kit only touches widgets inside a tick that passed `wg_check()`. On a world change,
  `Kit.forget()` and `Panel.forget()` drop every reference without touching it.
- The only UObjects read are fresh `FindAllOf` / UEHelpers lookups. The HUD never reads a pawn.

## Verify in game

1. **Log at load:** `[HSMPHud] loaded; state_dir=... centre_banner=true`.
2. **In the arena after START:** `HUD shown (world=Map_Arena_... state=countdown)`, then
   `HUD host: inside native UI_HUD_C (canvas WxH dpi D scale S)`. If the log says
   `no UI_HUD_C in the viewport after 3.0 s -> viewport fallback` or `UI_HUD_C root is ...`, report
   it; the fallback still works.
3. **Look:** top centre `ROUND 1 / BEST OF 3`, the timer and the score strip; bottom left BODY / CON
   move when you are hit, ST when you swing; top left the opponent's rows; bottom right
   `NET OK  nn ms`. The mouse still controls the camera, and clicks still swing.
4. **Both screens:** the victim's own CON / BODY % equal what the attacker sees for the victim.
5. **Hold TAB:** the scoreboard appears; release closes it. If it toggles instead, `IsInputKeyDown`
   did not marshal; report it.
6. **Kill the opponent:** `You slew <nick>` top right, fading after 6 s.
7. **Esc in an MP arena:** the MP pause opens (RESUME / SETTINGS / LEAVE MATCH / QUIT GAME) and the
   opponent keeps moving. A native menu HSMPHud does not replace (photo mode) hides the HUD.
8. **Round reset, and return to the menu:** no crash; `world guard: ... -> object caches dropped`; the
   HUD is rebuilt in the new arena; nothing is shown in the menu.
9. **Pull the network cable for 8 s mid-round:** RECONNECTING... with the seconds left, then the
   round continues. Never the menu.
10. **Match over:** REMATCH / BACK TO LOBBY under the result. REMATCH on both sides plus the host's
    START restarts in the same arena.
11. **Resize the window** (windowed mode, drag a corner): the HUD rebuilds once at the new size and
    stays inside the window.

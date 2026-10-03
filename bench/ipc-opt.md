# net-bench after the shm/wire unification (IPC optimisation run, 6c49833)

Same harness and matrix as `baseline.md` (main 64259a3): `hsmp-tools net-bench --clients N
--secs 30`, real release `hsmp-server` / `hsmp-sidecar`, fake games `ipc-game --synth`, loopback
through the counting proxy. Raw reports: `ipc-opt-<clients>-<rep>.json`. The machine was quiet:
no game was running and no builds were active. System idle was 94-98 % before each run and
93-96 % during it, versus 86-96 % for the baseline, so treat CPU differences under ~1 point as
noise.

CPU % is of ONE core. "per player" = server CPU / clients. Wire = payload + 28 B IPv4/UDP per
datagram. Per-client values are averaged over the clients.

| clients | build | server CPU % | per player | sidecar CPU % avg / max | up B/s payload (wire) | up pps | down B/s payload (wire) | down pps | thinned /s | budget drops /s |
|---|---|---|---|---|---|---|---|---|---|---|
| 2 | main | 1.09 | 0.55 | 1.46 / 1.72 | 38 504 (44 086) | 199.4 | 30 027 (33 164) | 112.1 | 181 | 0 |
| 2 | optimised | 0.83 | 0.42 | 0.83 / 1.09 | 34 299 (36 521) | 79.4 | 29 345 (32 358) | 107.6 | 68 | 0 |
| 4 (mean of 3) | main | 2.90 | 0.72 | 3.30 / 4.22 | 38 465 (44 039) | 199.1 | 74 324 (82 423) | 289.3 | 1 238 | 0 |
| 4 (mean of 3) | optimised | 2.45 | 0.61 | 1.71 / 2.13 | 34 290 (36 508) | 79.2 | 71 987 (79 658) | 274.0 | 579 | 0 |
| 8 | main | 7.59 | 0.95 | 4.34 / 4.99 | 38 514 (44 120) | 200.2 | 119 250 (133 874) | 522.3 | 6 961 | 1.8 |
| 8 | optimised | 4.89 | 0.61 | 2.17 / 2.71 | 34 295 (36 521) | 79.5 | 114 927 (128 634) | 489.6 | 3 854 | 0 |

## What changed

- **Upstream:**
  - pps: −60 % (199 → 79 per client). Root, weapon and pose now leave as one datagram per frame.
  - Wire bytes: −17 % (44.0 → 36.5 KB/s).
  - Payload: −11 %.
- **Downstream:**
  - At 8 clients: −4 % on the wire, −6 % pps, and thinning nearly halved (6 961 → 3 854 /s).
  - Budget drops: 0, against 1.8 /s on main.
- **Server CPU per player:** −36 % at 8 clients (0.95 → 0.61 %). At 2 and 4 clients the gain is −15 to −24 %, which is near the noise floor.
- **Sidecar CPU:** −43 % at 2 clients, −48 % at 4 and −50 % at 8. The sidecar forwards typed record bytes and adds only framing, so there is no conversion work.
- **Fake game write cost:** the pose slot write rose from 0.08 µs to about 5.5 µs. This is expected: the synth now does what HSMPNative does in-game, building the pose record and encoding codec v2 once at the source. In the real game this replaces Lua's encoding. The in-game pose sender cost fell 0.667 → 0.102 ms with native sampling on (gate ab-diff), so the move is net positive by a wide margin.

## Reproduce

```powershell
cargo build --release -p hsmp-tools -p hsmp-server
target\release\hsmp-tools.exe net-bench --clients 4 --secs 30 --json bench\ipc-opt-4-1.json
```

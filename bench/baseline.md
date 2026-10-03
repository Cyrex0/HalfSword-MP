# net-bench baseline (main 64259a3, before the shm/wire unification)

`hsmp-tools net-bench --clients N --secs 30` (warmup 5 s, 60 Hz pose/root/weapon, 20 Hz vitals,
server `--tick-hz 30`, default `--client-budget-kbps 128`). Real `hsmp-server` and `hsmp-sidecar`
(release), fake games `ipc-game --synth`, all on loopback through the counting proxy. Raw
reports: `baseline-<clients>-<rep>.json`.

Machine: 20 logical CPUs, Windows 11. CPU % is of ONE core (kernel + user, GetProcessTimes).
Other work was running on the machine (builds came and went). System idle was 91-96 % before
each run and 86-94 % during it, so treat CPU differences under ~1 percentage point as noise.

| clients | rep | server CPU % | per player | sidecar CPU % avg / max | fake game CPU % | up B/s payload (wire) | up pps | down B/s payload (wire) | down pps | thinned /s | budget drops /s | idle % before / during |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2 | 1 | 1.09 | 0.55 | 1.46 / 1.72 | 0.60 | 38 504 (44 087) | 199.4 | 30 027 (33 164) | 112.1 | 181 | 0 | 92.7 / 93.3 |
| 4 | 1 | 2.29 | 0.57 | 2.91 / 4.11 | 1.09 | 38 471 (44 046) | 199.1 | 74 600 (82 741) | 290.8 | 1 232 | 0 | 95.6 / 93.7 |
| 4 | 2 | 3.12 | 0.78 | 3.85 / 4.22 | 1.63 | 38 471 (44 046) | 199.1 | 74 292 (82 387) | 289.1 | 1 240 | 0 | 92.5 / 87.0 |
| 4 | 3 | 3.28 | 0.82 | 3.14 / 3.65 | 0.86 | 38 453 (44 024) | 199.0 | 74 081 (82 142) | 287.9 | 1 242 | 0 | 91.0 / 92.1 |
| **4** | **mean** | **2.90** | **0.72** | **3.30 / 4.22** | **1.19** | **38 465 (44 039)** | **199.1** | **74 324 (82 423)** | **289.3** | **1 238** | **0** | |
| 8 | 1 | 7.59 | 0.95 | 4.34 / 4.99 | 1.48 | 38 514 (44 120) | 200.2 | 119 250 (133 874) | 522.3 | 6 961 | 1.8 | 92.5 / 86.3 |

"wire" = payload + 28 B IPv4/UDP header per datagram. Per-client values are averages over
the clients (the per-flow numbers are in `flows` of each JSON).

## Fake-game slot write cost (µs, avg / p99, averaged over the games)

| clients | LocalRoot | LocalWeapon | LocalPose (1600 B) | vitals write | vitals encode | doorbell (SetEvent) | one sample (root + weapon + pose + doorbell) |
|---|---|---|---|---|---|---|---|
| 2 | 0.04 / 0.10 | 0.03 / 0.10 | 0.08 / 0.20 | 0.11 / 0.30 | 1.03 / 1.70 | 1.93 / 3.80 | 2.32 / 4.95 |
| 4 (rep 1-3) | 0.04 / 0.10 | 0.03 / 0.10-0.12 | 0.08 / 0.20 | 0.08-0.11 / 0.25-0.38 | 1.05-1.38 / 2.3-4.5 | 2.00-2.35 / 3.6-4.4 | 2.38-2.79 / 4.2-6.3 |
| 8 | 0.04 / 0.10 | 0.03 / 0.10 | 0.08 / 0.20 | 0.09 / 0.33 | 1.12 / 2.42 | 2.41 / 4.21 | 2.82 / 5.20 |

Every game held 60.0 Hz (achieved 60.00-60.02 Hz, 0 late samples).

## What the numbers say

- **Upstream is 200 datagrams/s per client** at 60 Hz: root, weapon and pose each go as their
  own datagram (60 + 60 + 60) plus vitals (20). Average payload 193 B; the v2 pose frame is
  332 B (the sidecar's own report: 57 frames/s, 332 B/frame, 0 translation overrides, control
  in 49 % of frames). Merging the per-frame streams into one datagram is where the rewrite can
  cut pps, and with it the per-datagram overhead: the 45 B v5 envelope (relay.rs `SEAL_OVERHEAD`) and the 28 B IP/UDP header.
- **Downstream at 8 clients is capped by the per-client budget** (128 KB/s; 119 KB/s payload
  measured, budget drops 1.8/s, 6 961 frames/s thinned). From 8 clients on, smaller frames
  show up as *fewer thinned frames*, not as fewer bytes. Compare `thinned_per_s` /
  `budget_drop_per_s` too.
- Thinning at 2-4 clients is by design (relay.rs): root to 30 Hz, weapon to 5 Hz.
- **The game-side cost is the doorbell, not the copy.** A seqlock write of the 1.6 KB
  LocalPose is ~0.08 µs; the SetEvent doorbell is ~2 µs, ~85 % of the per-sample cost. (The
  synth encodes vitals in Rust, ~1 µs; the real game encodes them in Lua.)
- Server CPU per player rises from 0.55 % (2 clients) to 0.95 % (8 clients): the total fan-out work grows with N², so the cost per player grows with N.
- No frames were dropped by the server's per-sender stream limit (120 Hz x 1.5 bucket), so
  60 Hz (HSMPSync's default `send_hz`) is the rate used throughout.

## Reproduce

```powershell
cargo build --release -p hsmp-tools -p hsmp-server
target\release\hsmp-tools.exe net-bench --clients 4 --secs 30 --json bench\baseline-4-1.json
```

`--keep` keeps the work dir (server.log, per-client game.log with the sidecar log,
synth.jsonl). The bench CPU (the proxy, in the net-bench process) is reported separately as
`bench_cpu_pct` (0.7 % at 2 clients, 6.7 % at 8 clients) and is not part of any figure above.

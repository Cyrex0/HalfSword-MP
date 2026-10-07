# Native combat lab

`hsmp-lab` is a Rust binary in `hsmp-tools`. It keeps one two-game harness alive,
uses RCON and DevCtl to apply recipes, and reads evidence incrementally by byte
offset. It never injects OS input. It uses the harness's process identities,
side-by-side window placement, save guard, deployment checks and normal teardown.

Build with `cargo build --release --locked -p hsmp-tools`. In one terminal:

```powershell
.\target\release\hsmp-lab.exe session --dir test-results/lab/session-01
```

The owner command stays running. In another terminal, after it prints ready:

```powershell
.\target\release\hsmp-lab.exe exp sword-cloth --session test-results/lab/session-01
.\target\release\hsmp-lab.exe exp polearm-mail --session test-results/lab/session-01
.\target\release\hsmp-lab.exe ab impact_dv sword-cloth --a 0 --b 300 --session test-results/lab/session-01
.\target\release\hsmp-lab.exe stop --session test-results/lab/session-01
```

The harness exits normally on stop or owner-process exit. Do not close its parent
terminal while an experiment is active. An experiment lock prevents overlapping
recipes or the native modes script. Never remove a live experiment's lock.

Recipes are under `tools/hsmp-tools/lab/`: five weapon groups against cloth, mail
and plate, plus idle stand-ins with probe disabled. AI fights use Yard. Each recipe
declares two kits, mode, arena, kit rules, AI, probe, tune knobs, duration and round
timeout. An optional no-claim timeout is separate from biological death or surrender:
`lab-actions.jsonl` records its explicit `DEBUG KILL 1` stimulus. Native dev AI yield
in Duel counts as surrender, as requested by the owner; human KO rules are unchanged.
Probe mutates local stand-in damage state and must stay off for visual sync review.

For server mods, use `session --server-args-json '["--mods-dir","<fixture path>"]'`.
Generate a valid 20 MiB split-file fixture and inspect refusals with
`scripts/lab-server-mods-smoke.ps1`. Run repeated native deathmatch placement checks
with `scripts/lab-modes-test.ps1 -Session <session>`; its backend subchecks are
separate from the full visual and control acceptance plan.

## Evidence and comparisons

```powershell
.\target\release\hsmp-lab.exe analyse --run <raw run> --recipe sword-cloth --out <summary.json>
.\target\release\hsmp-lab.exe compare --baseline <old.json> --current <new.json> --out <delta.json>
.\target\release\hsmp-lab.exe review --summary <new.json> --baseline <old.json>
.\target\release\hsmp-lab.exe baseline --summary <new.json> --accept
```

Summaries preserve raw file/line/text references for decisions, proxy error,
pose distributions per arena/instance/peer, calibration factors and diagnostics.
Cached replays are excluded. New `LAB_PROBE`/`LAB_REPLAY` diagnostics pair the exact
attacker and claim id, including Inside continuations, with six-decimal Health deltas.
Unavailable native observations remain explicit. Legacy native/replay pairs require a unique rounded
input signature across both players; ambiguous signatures are reported and excluded.
They are exploratory, not exact causal proof. A zero native sum has an undefined
ratio. Missing metrics remain incomplete. Stand-in fallen/downed agreement and
sender FPS are only available when the source emitter supplies them; absence never
becomes a pass. Archived ordinary logs may rate-limit some rejection messages;
their tallies are observed logged decisions, not guaranteed total claims.

Comparisons use 2,000 deterministic bootstrap resamples for mean acceptance deltas,
median and p90 changes, and paired replay/native ratios. Per-hit observations within
a round are correlated, so those intervals are exploratory. Use repeated independent
experiments before accepting a build; an A/B command changes only one tune knob in
the same session and saves the run list for review. Review appends bounds, new reasons,
count asymmetry and the three largest evidence groups to `test-results/lab/journal.jsonl`.
Count asymmetry is an investigation signal: inspect downed time before blaming code.
Raw acceptance includes validated parries. `honest_accept` excludes parries from
the denominator, following COMBAT-1's existing acceptance contract; both remain
visible. No rejection tolerance is changed. Clock diagnostics include
`x_pose_clock_reset` with the discrepancy and buffer state before correction.

Accepted baselines require every declared metric to have enough samples within its
existing bound. Promotion is explicit and never automatic. Stage summary JSON only
under `test-results/lab/baseline`; never commit raw logs, plans, passwords or identities.
Failed historical references may be retained there with a name ending `-reference`
and an explicit unaccepted status, so a failed build is never mislabeled accepted.

The 15 combat recipes nominally take 75 minutes at five minutes each, plus arena/load
transitions; idle adds two minutes. This is a configured duration, not a measured
full sweep runtime. Incremental analysis time is measured by `analyse`; live sweep
duration and acceptance require real runs. Release still requires G0, typical p0_gate,
the twice-green DoD scenarios and all outstanding native acceptance checks.

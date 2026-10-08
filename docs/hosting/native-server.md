# Native authority server (development)

`hsmp-server.exe native` supervises one licensed Half Sword game process. The
HSMPNative DLL embeds the existing Rust network service. Native spawning,
physics, AI, equipment and damage remain in that engine world; networking
threads exchange bounded immutable data with the game thread.

This is an implementation under verification. A successful process launch or
an offline network test does not establish playable co-op, native combat parity,
complete dismemberment playback or Abyss waves. See the latest
[handoff](../internal/handoff/claude-beta6-20261007.md) for actual evidence.

Build and deploy from a clean commit with full G0 before hosting. The supervisor
checks the deployment stamp, enabled native/Match mods, all deployed file hashes
and its own executable hash. Start the deployed executable:

```powershell
.\game\HalfswordUE5\Binaries\Win64\hsmp\hsmp-server.exe native `
  --game-dir .\game --identity-dir .\server-identity `
  --state-dir .\test-results\native-host --bind 127.0.0.1:7777 `
  --arena Map_Arena_Yard --backend null
```

Keep the identity directory between runs. Its private key stays on disk and is
never supplied as command-line key material. A fresh UUID run directory contains
process diagnostics and a `worker` subdirectory for game logs. The stop request
belongs to the supervisor run directory, outside `HSMP_STATE_DIR`; it carries no
input, frame, hit or world data.

Ctrl+C stops input admission and requests endpoint shutdown and engine exit. The
supervisor allows ten seconds before stopping only its own child process by its
OS handle. `--parent-pid` also stops hosting when the parent disappears.
`--run-seconds 90` bounds a diagnostic run, and `--boot-only` omits networking
while diagnosing engine startup. These options do not certify readiness.

`--backend null` selects Unreal's NullRHI. Native render-target and wound/cut
behavior still require live verification. `--backend offscreen` retains the RHI
and requests offscreen rendering for paths that need it. Both flags are listed
in [Epic's UE 5.5 command-line reference](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-command-line-arguments-reference?application_version=5.5).
A headless game client
is not a separately compiled Unreal Server target; game/source Server targets
are absent from this workspace. The licensed game and assets are not bundled
with the mod.

Native player input uses authenticated server-assigned entity references with
authority epochs and incarnations. Seven held actions and eight native input
axes reach only the current controller's possessed pawn. Mouse deltas are
consumed once, and silence/disconnection releases held input. Clients cannot
publish health, physics, hit results or directory ownership. The initial
directory covers two human pawns and one native enemy; dynamic Abyss enemy
lifecycles and complete client visual topology remain subsequent work.

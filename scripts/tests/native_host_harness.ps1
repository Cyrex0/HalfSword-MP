#Requires -Version 5.1
# Exercise the real harness helpers without launching a game or supervisor.
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$errors = $null
$tree = [Management.Automation.Language.Parser]::ParseFile((Join-Path $Repo "scripts/native_host_test.ps1"), [ref]$null, [ref]$errors)
if ($errors.Count) { throw $errors[0].Message }
$names = @("Process-Path", "Native-Record", "Native-SameProcess", "Native-ClientLaunch", "Native-ClientStatus", "Native-AuthorityEvidence", "Native-AllStopped")
foreach ($name in $names) {
    $function = $tree.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
    if (-not $function) { throw "Harness function missing: $name" }
    Invoke-Expression $function.Extent.Text
}
$checks = 0
function Check($value, [string]$reason) { if (-not $value) { throw $reason }; $script:checks++ }
$Run = Join-Path $Repo ("test-results/native-host-harness-fixture-" + [Guid]::NewGuid().ToString("N"))
$GameExe = Join-Path $Repo "game/HalfswordUE5/Binaries/Win64/HalfswordUE5-Win64-Shipping.exe"
$Win64 = Split-Path -Parent $GameExe
$utf8 = New-Object Text.UTF8Encoding $false
New-Item -ItemType Directory -Path $Run | Out-Null
# These doubles provide only recorded identity. No native process API is called.
$script:fixtureProcess = @{ Path=$GameExe }
function Hsmp-SameProcess($record) { if ($record.alive) { return $script:fixtureProcess }; return $null }
function Hsmp-ProcRecord($process, $role) { return @{role=$role;pid=1;name="fixture";start_ticks=123;alive=$true} }
$record = Native-Record $script:fixtureProcess "client" $GameExe
Check ((Native-SameProcess $record) -eq $script:fixtureProcess) "The exact recorded executable must qualify."
$script:fixtureProcess.Path = "\\?\$GameExe"
Check ($null -ne (Native-SameProcess $record)) "Extended Windows path spelling must retain exact identity."
$script:fixtureProcess.Path = Join-Path $Repo "unrelated.exe"
Check ($null -eq (Native-SameProcess $record)) "A changed executable path must never authorize cleanup."
Check (-not (Native-AllStopped @($record) @())) "A path mismatch cannot falsely prove that the recorded process stopped."
$record.alive = $false
Check (Native-AllStopped @($record) @()) "A stopped recorded process can be reported stopped."
Check (-not (Native-AllStopped @($record) @(2))) "An unobserved source child prevents a clean shutdown claim."
$record.start_ticks = 0
Check (-not (Native-AllStopped @($record) @())) "An absent start-time proof cannot be reported as verified cleanup."
$first = Native-ClientLaunch 1 30001
$second = Native-ClientLaunch 2 30001
foreach ($launch in @($first,$second)) {
    Check ($launch.start.FileName -eq $GameExe -and -not $launch.start.UseShellExecute) "Normal clients use the owned local game executable."
    Check ($launch.start.EnvironmentVariables["HSMP_RUNTIME_ROLE"] -eq "native_client") "Client presentation must use the explicit native client role."
    Check ($launch.start.EnvironmentVariables["HSMP_NATIVE_CLIENT_AI"] -eq "1" -and $launch.start.EnvironmentVariables["HSMP_NATIVE_SERVER"] -eq "127.0.0.1:30001") "Harness AI uses the authenticated native client input endpoint."
    Check ($launch.start.EnvironmentVariables["HSMP_NATIVE_PROBE"] -eq "0" -and $launch.start.EnvironmentVariables["HSMP_NATIVE_CALLER_PROBE"] -eq "0" -and $launch.start.EnvironmentVariables["HSMP_DEV_CALLER_PROBE"] -eq "0") "All three heavy probes stay disabled."
    Check (-not $launch.start.Arguments.Contains("-NullRHI") -and $launch.start.Arguments.Contains("-ResX=880 -ResY=527")) "Both clients render at the bounded secondary-display size."
}
Check ($first.start.Arguments.Contains("-WinX=-1760") -and $second.start.Arguments.Contains("-WinX=-880")) "The two clients occupy the two secondary-display slots."
Check ($first.state -ne $second.state -and $first.stop -ne $second.stop -and
    $first.start.EnvironmentVariables["HSMP_NATIVE_IDENTITY_DIR"] -ne $second.start.EnvironmentVariables["HSMP_NATIVE_IDENTITY_DIR"]) "Client state, identity and owner stop files are isolated."
$events = @(
    @{ev="x_native_client";state="live";frame_seq=10;wall_ms=100;seq=2;own_entity=1},
    @{ev="x_native_client";state="live";frame_seq=20;wall_ms=200;seq=3;own_entity=1}
)
[IO.File]::WriteAllLines((Join-Path $first.state "hsmp_events.jsonl"), @($events | ForEach-Object { $_ | ConvertTo-Json -Compress }), $utf8)
[IO.File]::WriteAllText((Join-Path $first.state "hsmp_events.1.jsonl"), (@{ev="x_native_client";state="boot";frame_seq=0;wall_ms=50;seq=1} | ConvertTo-Json -Compress), $utf8)
$latest = Native-ClientStatus $first
Check ($latest.state -eq "live" -and $latest.frame_seq -eq 20) "Rotated older events cannot replace the client's latest verified frame."
[IO.File]::WriteAllText((Join-Path $Run "UE4SS.log"), "[HSMPNativeWorker] active_pc0=900 active_pc1=900", $utf8)
$forged = @(
    @{ev="x_native_worker";state="native_ready"},
    @{ev="x_native_worker";state="native_evidence";active_pc0=500;active_pc1=500}
)
[IO.File]::WriteAllLines((Join-Path $first.state "hsmp_events.2.jsonl"), @($forged | ForEach-Object { $_ | ConvertTo-Json -Compress }), $utf8)
$evidence = Native-AuthorityEvidence $Run
Check (-not $evidence.ready -and $evidence.active[0] -eq 0 -and $evidence.active[1] -eq 0) "Shared logs and client streams cannot establish authority readiness or input dispatch."
$authority = Join-Path $Run "host/owned/worker"
New-Item -ItemType Directory -Path $authority | Out-Null
$sourceEvents = @(
    @{ev="x_native_worker";state="native_ready"},
    @{ev="x_native_worker";state="native_evidence";active_pc0=2;active_pc1=3}
)
[IO.File]::WriteAllLines((Join-Path $authority "hsmp_events.jsonl"), @($sourceEvents | ForEach-Object { $_ | ConvertTo-Json -Compress }), $utf8)
$evidence = Native-AuthorityEvidence $Run
Check ($evidence.ready -and $evidence.active[0] -eq 2 -and $evidence.active[1] -eq 3) "Only this run's authority event stream supplies genuine dispatch counts."
Write-Host "Native harness helpers: $checks checks passed; no processes launched."

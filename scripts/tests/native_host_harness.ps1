#Requires -Version 5.1
# Exercise the real harness helpers without launching a game or supervisor.
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$errors = $null
$tree = [Management.Automation.Language.Parser]::ParseFile((Join-Path $Repo "scripts/native_host_test.ps1"), [ref]$null, [ref]$errors)
if ($errors.Count) { throw $errors[0].Message }
$names = @("Write-Json", "Process-Path", "Native-Record", "Native-SameProcess", "Native-ClientLaunch", "Start-NativeClientOutput", "Complete-NativeClientOutput", "Native-ClientExit", "Assert-NativeClientRunning", "Native-ClientStatus", "Native-AuthorityEvidence", "Native-AllStopped")
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
$script:identityLookupForbidden = $false
$script:exitDuringIdentityCheck = $null
function Hsmp-SameProcess($record) {
    if ($script:identityLookupForbidden) { throw "An exited original handle must not inspect a reused PID." }
    if ($null -ne $script:exitDuringIdentityCheck) { $script:exitDuringIdentityCheck.HasExited=$true; $script:identityLookupForbidden=$true; return $null }
    if ($record.alive) { return $script:fixtureProcess }; return $null
}
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
    Check ($launch.start.RedirectStandardOutput -and $launch.start.RedirectStandardError -and $launch.start.EnvironmentVariables["RUST_BACKTRACE"] -eq "1") "Both original client pipes and client-only Rust backtraces are enabled."
    Check ($launch.start.EnvironmentVariables["HSMP_NATIVE_CLIENT_AI"] -eq "1" -and $launch.start.EnvironmentVariables["HSMP_NATIVE_SERVER"] -eq "127.0.0.1:30001") "Harness AI uses the authenticated native client input endpoint."
    Check ($launch.start.EnvironmentVariables["HSMP_NATIVE_PROBE"] -eq "0" -and $launch.start.EnvironmentVariables["HSMP_NATIVE_CALLER_PROBE"] -eq "0" -and $launch.start.EnvironmentVariables["HSMP_DEV_CALLER_PROBE"] -eq "0") "All three heavy probes stay disabled."
    Check (-not $launch.start.Arguments.Contains("-NullRHI") -and $launch.start.Arguments.Contains("-ResX=880 -ResY=527")) "Both clients render at the bounded secondary-display size."
}
Check ($first.start.Arguments.Contains("-WinX=-1760") -and $second.start.Arguments.Contains("-WinX=-880")) "The two clients occupy the two secondary-display slots."
Check ($first.state -ne $second.state -and $first.stop -ne $second.stop -and
    $first.start.EnvironmentVariables["HSMP_NATIVE_IDENTITY_DIR"] -ne $second.start.EnvironmentVariables["HSMP_NATIVE_IDENTITY_DIR"]) "Client state, identity and owner stop files are isolated."
$script:fixtureProcess.Path = $GameExe
$held = [pscustomobject]@{ HasExited=$false; ExitCode=[int]-1073741819; refreshes=0 }
$held | Add-Member ScriptMethod Refresh { $this.refreshes++ }
$owned = @{ process=$held; record=@{role="native_client1";pid=123;exe=$GameExe;start_ticks=456;alive=$true};state=$first.state;index=1 }
Check ($null -eq (Native-ClientExit $owned "before live verification") -and -not (Test-Path -LiteralPath (Join-Path $Run "client1.exit.json"))) "A live original handle produces no invented exit record."
Assert-NativeClientRunning $owned "before live verification"
$held.HasExited = $true
$script:identityLookupForbidden = $true
try { Assert-NativeClientRunning $owned "before live verification"; throw "fixture must refuse exited client" } catch { Check ($_.Exception.Message -eq "Native client 1 exited before live verification.") "Pre-live failure must record the original exit before rejecting it." }
$exit = Get-Content -Raw -LiteralPath (Join-Path $Run "client1.exit.json") | ConvertFrom-Json
Check ($exit.exit_code_known -and $exit.exit_code -eq -1073741819 -and $exit.exit_code_hex -eq "0xC0000005") "A signed original exit code preserves its exact unsigned hex bits."
Check ($exit.pid -eq 123 -and $exit.start_ticks -eq 456 -and $exit.exe -eq $GameExe -and $exit.observed_from -eq "original_process_handle") "The exit record retains original ownership rather than a current PID lookup."
Check ($exit.phase -eq "before live verification" -and ([DateTime]::Parse($exit.observed_utc)).ToUniversalTime() -le [DateTime]::UtcNow) "The exit record includes its real observation time and verification phase."
$firstObserved = $exit.observed_utc
Check ((Native-ClientExit $owned "during observation").observed_utc -eq $firstObserved) "Repeated checks retain one original exit observation."
$unreadable = [pscustomobject]@{ HasExited=$true }
$unreadable | Add-Member ScriptMethod Refresh {}
$unreadable | Add-Member ScriptProperty ExitCode { throw "held ExitCode unavailable" }
$secondOwned = @{process=$unreadable;record=@{role="native_client2";pid=124;exe=$GameExe;start_ticks=457};state=$second.state;index=2}
try { Assert-NativeClientRunning $secondOwned "during observation"; throw "fixture must refuse exited client" } catch { Check ($_.Exception.Message -eq "Native client 2 exited during observation.") "During-live failure also records the held exit without a PID lookup." }
$unknown = Get-Content -Raw -LiteralPath (Join-Path $Run "client2.exit.json") | ConvertFrom-Json
Check (-not $unknown.exit_code_known -and $null -eq $unknown.exit_code -and $null -eq $unknown.exit_code_hex -and $unknown.exit_code_error) "An unreadable held exit code stays explicitly unknown."
$zeroHandle = [pscustomobject]@{HasExited=$true;ExitCode=0}
$zeroHandle | Add-Member ScriptMethod Refresh {}
$zeroOwned = @{process=$zeroHandle;record=@{role="native_client1";pid=125;exe=$GameExe;start_ticks=458};index=3}
$zeroExit = Native-ClientExit $zeroOwned "before live verification"
Check ($zeroExit.exit_code_known -and $zeroExit.exit_code -eq 0 -and $zeroExit.exit_code_hex -eq "0x00000000") "An actual zero exit code stays distinct from unavailable data."
$script:identityLookupForbidden = $false
$raceHandle = [pscustomobject]@{HasExited=$false;ExitCode=18}
$raceHandle | Add-Member ScriptMethod Refresh {}
$raceOwned = @{process=$raceHandle;record=@{role="native_client2";pid=126;exe=$GameExe;start_ticks=459};index=4}
$script:exitDuringIdentityCheck = $raceHandle
try { Assert-NativeClientRunning $raceOwned "during observation"; throw "fixture must refuse racing exit" } catch { Check ($_.Exception.Message -eq "Native client 4 exited during observation.") "An exit during identity lookup is rechecked on the held handle and recorded." }
$raceExit = Get-Content -Raw -LiteralPath (Join-Path $Run "client4.exit.json") | ConvertFrom-Json
Check ($raceExit.pid -eq 126 -and $raceExit.exit_code_known -and $raceExit.exit_code -eq 18 -and $raceExit.exit_code_hex -eq "0x00000012") "The racing exit retains the original handle's actual code and ownership."
$script:exitDuringIdentityCheck = $null
$script:identityLookupForbidden = $false
Check (-not (Test-Path -LiteralPath (Join-Path $first.state "process_exit.json")) -and -not (Test-Path -LiteralPath (Join-Path $second.state "process_exit.json"))) "Harness exit evidence stays outside both game state directories."
$unavailable = @{process=$held;record=@{role="native_client1";pid=123;exe=$GameExe;start_ticks=456;alive=$false};state=$first.state;index=1}
$held.HasExited = $false
try { Assert-NativeClientRunning $unavailable "during observation"; throw "fixture must refuse unavailable identity" } catch { Check ($_.Exception.Message -eq "Native client 1 original process identity unavailable during observation.") "A missing current identity is never relabeled as an observed exit." }
Check ($null -eq $unavailable.exit_record) "Identity failure does not fabricate an exit code."
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
# A real bounded non-game child writes more than either pipe buffer, including
# a final failure marker, then exits3. Waiting without both concurrent drains
# would block this producer before its original exit could be observed.
$childStart = New-Object Diagnostics.ProcessStartInfo
$childStart.FileName = (Get-Process -Id $PID).Path
$childStart.UseShellExecute = $false
$childStart.CreateNoWindow = $true
$childStart.RedirectStandardOutput = $true
$childStart.RedirectStandardError = $true
$producer = '$out="O"*4096;$err="E"*4096;for($i=0;$i -lt 128;$i++){[Console]::Out.Write($out);[Console]::Error.Write($err)};[Console]::Error.WriteLine("original failure marker");exit 3'
$childStart.Arguments = '-NoProfile -NonInteractive -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($producer))
$child = [Diagnostics.Process]::Start($childStart)
$pipeOwned = @{process=$child;record=@{role="pipe_fixture";pid=$child.Id;exe=$childStart.FileName;start_ticks=$child.StartTime.ToUniversalTime().Ticks};index=9}
try {
    $pipeOwned.output = Start-NativeClientOutput $child 9
    Check ($pipeOwned.output.stdout_task -is [Threading.Tasks.Task] -and $pipeOwned.output.stderr_task -is [Threading.Tasks.Task]) "Both pipes use immediately started .NET copy tasks."
    Check ((Split-Path -Parent $pipeOwned.output.stdout_path) -eq $Run -and (Split-Path -Parent $pipeOwned.output.stderr_path) -eq $Run) "Raw output lives outside the game's state directories."
    Check ($child.WaitForExit(10000)) "A producer exceeding both pipe capacities exits without pipe deadlock."
    $script:identityLookupForbidden = $true
    $pipeExit = Native-ClientExit $pipeOwned "pipe fixture"
    Check ($pipeExit.exit_code -eq 3 -and $pipeExit.output_complete) "Original exit3 retains fully drained output rather than inferring a panic."
    $captured = $pipeOwned.output.completed_record
    Check ($captured.stdout_bytes -eq 524288 -and $captured.stderr_bytes -gt 524288) "Neither saturated pipe drops bytes."
    $stderr = [IO.File]::ReadAllText($captured.stderr_path)
    Check ($stderr.EndsWith("original failure marker`n") -or $stderr.EndsWith("original failure marker`r`n")) "The final original-process error remains preserved."
    Check ($captured.pid -eq $child.Id -and $captured.start_ticks -eq $pipeOwned.record.start_ticks -and $captured.observed_from -eq "original_process_handle") "Output records retain exact process lifetime identity."
    Check ((Complete-NativeClientOutput $pipeOwned).stderr_bytes -eq $captured.stderr_bytes) "Repeated cleanup retains the original completed capture."
} finally {
    if (-not $child.HasExited) { $child.Kill(); [void]$child.WaitForExit(5000) }
    [void](Complete-NativeClientOutput $pipeOwned -Cleanup)
    $child.Dispose()
    $script:identityLookupForbidden = $false
}
$starter = $tree.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq "Start-NativeClient"},$true).Extent.Text
Check ($starter.IndexOf('Write-Json') -lt $starter.IndexOf('Start-NativeClientOutput')) "Original identity is persisted before output file setup can fail."
# An incomplete drain must stay explicitly incomplete and cancel in bounded
# time. This task double exercises lifetime/cleanup without another process.
$pendingHandle = [pscustomobject]@{HasExited=$false}
$pendingHandle | Add-Member ScriptMethod Refresh {}
$pending = @{process=$pendingHandle;record=@{pid=991;start_ticks=992};index=10}
$cancel = New-Object Threading.CancellationTokenSource
$pending.output = @{
    stdout_path=(Join-Path $Run "client10.stdout.log"); stderr_path=(Join-Path $Run "client10.stderr.log"); cancel=$cancel
    stdout_source=(New-Object IO.MemoryStream); stderr_source=(New-Object IO.MemoryStream)
    stdout_task=[Threading.Tasks.Task]::Delay(60000,$cancel.Token); stderr_task=[Threading.Tasks.Task]::Delay(60000,$cancel.Token)
}
$pending.output.stdout_file = [IO.File]::Open($pending.output.stdout_path,[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::Read)
$pending.output.stderr_file = [IO.File]::Open($pending.output.stderr_path,[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::Read)
Check ($null -eq (Complete-NativeClientOutput $pending 25) -and -not $cancel.IsCancellationRequested) "A live original process keeps both drains and file ownership alive."
$pendingHandle.HasExited=$true
$watch=[Diagnostics.Stopwatch]::StartNew()
$incomplete=Complete-NativeClientOutput $pending 25
$watch.Stop()
Check ($watch.ElapsedMilliseconds -lt 1000 -and $incomplete.cancelled -and -not $incomplete.complete) "An exhausted drain deadline cancels without inventing complete output."
Check (-not $pending.output.stdout_file.CanWrite -and -not $pending.output.stderr_file.CanWrite -and $incomplete.errors.Count -gt 0) "Cancelled copies retain an explicit error and dispose their owned files."
Write-Host "Native harness helpers: $checks checks passed; one bounded non-game pipe fixture."

#Requires -Version 5.1
<#
.SYNOPSIS
    Bounded native authority with two normal game clients. Diagnostic probes are explicit only.
#>
[CmdletBinding()]
param(
    [string]$GamePath = "",
    [ValidateSet("null", "offscreen")][string]$Backend = "null",
    [ValidateRange(45, 180)][int]$Seconds = 100,
    [switch]$BootOnly,
    [switch]$ExerciseInput,
    [switch]$DiagnosticProbeOnly
)
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "lib\hsmp_runs.ps1")
if (-not $GamePath) { $GamePath = if ($env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR } else { Join-Path $Repo "game" } }
$GamePath = (Resolve-Path -LiteralPath $GamePath).Path
$Win64 = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$Server = Join-Path $Win64 "hsmp\hsmp-server.exe"
$GameExe = Join-Path $Win64 "HalfswordUE5-Win64-Shipping.exe"
if (-not (Test-Path -LiteralPath $Server -PathType Leaf)) { throw "Deployed native server unavailable." }
$exclusive = Hsmp-BeginExclusiveRun
if (-not $exclusive.lock) { throw "Native test refused: $($exclusive.why)" }
$id = Hsmp-NewRunId "native-host"
$Run = Join-Path $Repo "test-results\$id"
$utf8 = New-Object Text.UTF8Encoding $false
$tracked = New-Object Collections.ArrayList
$registry = $null
$supervisor = $null
$failure = $null
$baseline = $null
$crashBaseline = @()
$clients = @()
$clientEvidence = @()
$clientStopAttempted = $false
$clientStopResult = $true
$mode = if ($DiagnosticProbeOnly) { "diagnostic" } else { "pvp" }
$sourceCommit = (& git -C $Repo rev-parse HEAD).Trim()
function Write-Json([string]$path, $value) { [IO.File]::WriteAllText($path, ($value | ConvertTo-Json -Depth 8), $utf8) }
function Saves-Snapshot {
    $saveRoot = Join-Path $env:LOCALAPPDATA "HalfswordUE5\Saved\SaveGames"
    $files = [ordered]@{}
    if (Test-Path -LiteralPath $saveRoot) {
        foreach ($file in @(Get-ChildItem -LiteralPath $saveRoot -File -Recurse)) {
            # Multiplayer redirected slots are retained, never deleted. Compare the player's own saves.
            if ($file.Name.StartsWith("HSMP_", [StringComparison]::OrdinalIgnoreCase)) { continue }
            $relative = $file.FullName.Substring($saveRoot.Length).TrimStart('\')
            $files[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        }
    }
    return $files
}
function Quote-Arg([string]$value) {
    if ($value.Contains('"') -or $value.Contains("`r") -or $value.Contains("`n")) { throw "Unsupported diagnostic argument." }
    return '"' + $value.TrimEnd('\') + '"'
}
function Process-Path([string]$path) {
    if ($path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) { $path = '\\' + $path.Substring(8) }
    elseif ($path.StartsWith('\\?\', [StringComparison]::Ordinal)) { $path = $path.Substring(4) }
    return [IO.Path]::GetFullPath($path)
}
function Native-Record($process, [string]$role, [string]$expectedExe) {
    $record = Hsmp-ProcRecord $process $role
    $record.exe = Process-Path $expectedExe
    return $record
}
function Native-SameProcess($record) {
    $process = Hsmp-SameProcess $record
    if (-not $process -or -not $record.exe) { return $null }
    try { if ((Process-Path $process.Path) -ine $record.exe) { return $null } } catch { return $null }
    return $process
}
function Native-ClientLaunch([int]$index, [int]$port) {
    $state = Join-Path $Run "client$index"
    $identity = Join-Path $Run "client-identity$index"
    $stop = Join-Path $Run "client$index.stop.request"
    New-Item -ItemType Directory -Path $state,$identity | Out-Null
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $GameExe
    $start.WorkingDirectory = $Win64
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $x = if ($index -eq 1) { -1760 } else { -880 }
    $start.Arguments = "-nosound -unattended -windowed -ForceRes -ResX=880 -ResY=527 -WinX=$x -WinY=0"
    $values = @{
        HSMP_RUNTIME_ROLE="native_client"; HSMP_STATE_DIR=$state; HSMP_INST="native_client_$index"
        HSMP_NATIVE_SERVER="127.0.0.1:$port"; HSMP_NATIVE_IDENTITY_DIR=$identity
        HSMP_NATIVE_NICK="native-client-$index"; HSMP_NATIVE_CLIENT_AI="1"
        HSMP_NATIVE_PARENT_PID="$PID"; HSMP_NATIVE_STOP_FILE=$stop
        HSMP_NATIVE_PROBE="0"; HSMP_NATIVE_CALLER_PROBE="0"; HSMP_DEV_CALLER_PROBE="0"
    }
    foreach ($key in $values.Keys) { $start.EnvironmentVariables[$key] = $values[$key] }
    return @{ start=$start; state=$state; stop=$stop; index=$index }
}
function Start-NativeClient([int]$index, [int]$port) {
    $launch = Native-ClientLaunch $index $port
    $process = [Diagnostics.Process]::Start($launch.start)
    $record = Native-Record $process "native_client$index" $GameExe
    [void]$tracked.Add($record)
    Write-Json (Join-Path $Run "processes.json") @($tracked)
    return @{ process=$process; record=$record; state=$launch.state; stop=$launch.stop; index=$index }
}
function Native-ClientExit($client, [string]$phase) {
    if ($client.exit_record) { return $client.exit_record }
    # Process.Start's original handle distinguishes this client from PID reuse.
    # Do not look up another process or infer an exit from unavailable identity.
    [void]$client.process.Refresh()
    $hasExited = $client.process.HasExited
    if ($hasExited -isnot [bool]) { throw "Native client $($client.index) original process exit state unavailable." }
    if (-not $hasExited) { return $null }
    $exit = [ordered]@{
        role=$client.record.role; pid=$client.record.pid; exe=$client.record.exe
        start_ticks=$client.record.start_ticks; phase=$phase
        observed_utc=[DateTime]::UtcNow.ToString("o"); observed_from="original_process_handle"
        exited=$true; exit_code_known=$false; exit_code=$null; exit_code_hex=$null
    }
    try {
        $code = $client.process.ExitCode
        if ($code -isnot [int]) { throw "Original process exit code unavailable." }
        $bits = [BitConverter]::ToUInt32([BitConverter]::GetBytes($code), 0)
        $exit.exit_code = $code
        $exit.exit_code_hex = "0x{0:X8}" -f $bits
        $exit.exit_code_known = $true
    } catch { $exit.exit_code_error = $_.Exception.Message }
    $client.exit_record = $exit
    # Harness evidence lives outside HSMP_STATE_DIR's file allow-list.
    Write-Json (Join-Path $Run "client$($client.index).exit.json") $exit
    return $exit
}
function Assert-NativeClientRunning($client, [string]$phase) {
    if (Native-ClientExit $client $phase) { throw "Native client $($client.index) exited $phase." }
    if (-not (Native-SameProcess $client.record)) {
        # It can exit between the first handle read and ownership lookup.
        if (Native-ClientExit $client $phase) { throw "Native client $($client.index) exited $phase." }
        throw "Native client $($client.index) original process identity unavailable $phase."
    }
}
function Native-ClientStatus($client) {
    $latest = $null
    foreach ($file in @(Get-ChildItem -LiteralPath $client.state -Filter "hsmp_events*.jsonl" -File -ErrorAction SilentlyContinue)) {
        foreach ($line in @(Get-Content -LiteralPath $file.FullName)) {
            try { $event = $line | ConvertFrom-Json } catch { continue }
            if ($event.ev -eq "x_native_client" -and (-not $latest -or $event.wall_ms -gt $latest.wall_ms -or
                ($event.wall_ms -eq $latest.wall_ms -and $event.seq -gt $latest.seq))) { $latest = $event }
        }
    }
    return $latest
}
function Native-AuthorityEvidence([string]$runPath) {
    $result = @{ ready=$false; active=@(0, 0) }
    # The shared UE4SS log and client streams cannot prove source dispatch.
    foreach ($events in @(Get-ChildItem -LiteralPath (Join-Path $runPath "host") -Filter "hsmp_events*.jsonl" -Recurse -ErrorAction SilentlyContinue)) {
        foreach ($line in @(Get-Content -LiteralPath $events.FullName)) {
            try { $event = $line | ConvertFrom-Json } catch { continue }
            if ($event.ev -eq "x_native_worker" -and $event.state -eq "native_ready") { $result.ready = $true }
            if ($event.ev -eq "x_native_worker" -and $event.state -eq "native_evidence") {
                $result.active[0] = [Math]::Max($result.active[0], [int]$event.active_pc0)
                $result.active[1] = [Math]::Max($result.active[1], [int]$event.active_pc1)
            }
        }
    }
    return $result
}
function Stop-NativeClients {
    if ($script:clientStopAttempted) { return $script:clientStopResult }
    $script:clientStopAttempted = $true
    foreach ($client in $clients) { [IO.File]::WriteAllText($client.stop, "stop`n", $utf8) }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline -and @($clients | Where-Object { Hsmp-SameProcess $_.record }).Count) { Start-Sleep -Milliseconds 200 }
    $script:clientStopResult = @($clients | Where-Object { Hsmp-SameProcess $_.record }).Count -eq 0
    return $script:clientStopResult
}
function Native-AllStopped($records, $unobserved) {
    # An unreadable or changed executable path forbids a kill; it cannot prove
    # the originally observed PID/start-time process has exited.
    return @($records | Where-Object { -not $_.start_ticks -or (Hsmp-SameProcess $_) }).Count -eq 0 -and $unobserved.Count -eq 0
}
function Crashes-Snapshot {
    $root = Join-Path $env:LOCALAPPDATA "HalfswordUE5\Saved\Crashes"
    if (Test-Path -LiteralPath $root) { return @(Get-ChildItem -LiteralPath $root -Directory | ForEach-Object Name) }
    return @()
}
try {
    New-Item -ItemType Directory -Path $Run | Out-Null
    $baseline = Saves-Snapshot
    $crashBaseline = @(Crashes-Snapshot)
    Write-Json (Join-Path $Run "save_baseline.json") $baseline
    $port = @(Hsmp-FreePorts 1)[0]
    $pidFile = Join-Path $Run "supervisor.pid.json"
    $registry = Hsmp-RegisterRun @{id=$id; pid_file=(Join-Path $Run "processes.json"); state_dirs=@(); ports=@($port); detached=$false}
    $arguments = @("native", "--game-dir", (Quote-Arg $GamePath), "--identity-dir", (Quote-Arg (Join-Path $Run "host-identity")),
        "--state-dir", (Quote-Arg (Join-Path $Run "host")), "--bind", "127.0.0.1:$port", "--backend", $Backend,
        "--arena", "Map_Arena_Yard", "--mode", $mode, "--parent-pid", "$PID", "--pid-file", (Quote-Arg $pidFile), "--run-seconds", "$Seconds")
    if ($BootOnly) { $arguments += "--boot-only" }
    $supervisor = Start-Process -FilePath $Server -ArgumentList ($arguments -join " ") -WorkingDirectory $Win64 -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $Run "supervisor.out.log") -RedirectStandardError (Join-Path $Run "supervisor.err.log")
    [void]$tracked.Add((Native-Record $supervisor "native_supervisor" $Server))
    Write-Json (Join-Path $Run "processes.json") @($tracked)
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    $native = $null
    while ([DateTime]::UtcNow -lt $deadline -and -not $supervisor.HasExited) {
        $native = @(Get-ChildItem -LiteralPath (Join-Path $Run "host") -Filter "native_process.json" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($native.Count) { break }
        Start-Sleep -Milliseconds 200
        $supervisor.Refresh()
    }
    if (-not $native.Count) { throw "Supervisor did not record an owned native game process." }
    $record = Get-Content -Raw -LiteralPath $native[0].FullName | ConvertFrom-Json
    $gameProcess = Get-Process -Id ([int]$record.pid) -ErrorAction Stop
    if (((Process-Path $gameProcess.Path) -ine (Process-Path $record.exe)) -or
        $gameProcess.StartTime -lt $supervisor.StartTime -or
        $gameProcess.StartTime.ToUniversalTime() -gt $native[0].LastWriteTimeUtc) {
        throw "Native process identity changed before observation."
    }
    [void]$tracked.Add((Native-Record $gameProcess "native_authority" $GameExe))
    Write-Json (Join-Path $Run "processes.json") @($tracked)
    if (-not $BootOnly -and $DiagnosticProbeOnly) {
        $probeArgs = @("native-probe", "--server", "127.0.0.1:$port", "--identity-dir", (Join-Path $Run "clients"),
            "--report", (Join-Path $Run "native_probe.json"), "--ready-seconds", "65", "--observe-seconds", "10")
        if ($ExerciseInput) { $probeArgs += "--exercise-input" }
        $probeCommand = @($probeArgs | ForEach-Object { Quote-Arg ([string]$_) }) -join " "
        $probe = Start-Process -FilePath $Server -ArgumentList $probeCommand -WorkingDirectory $Win64 -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $Run "probe.out.log") -RedirectStandardError (Join-Path $Run "probe.err.log")
        [void]$tracked.Add((Native-Record $probe "native_probe" $Server))
        Write-Json (Join-Path $Run "processes.json") @($tracked)
        if (-not $probe.WaitForExit(80000) -or $probe.ExitCode -ne 0) { throw "Native network probe failed; inspect probe.err.log." }
    } elseif (-not $BootOnly) {
        $clients += Start-NativeClient 1 $port
        $clients += Start-NativeClient 2 $port
        $readyDeadline = [DateTime]::UtcNow.AddSeconds(65)
        $ready = $false
        while ([DateTime]::UtcNow -lt $readyDeadline) {
            $clientEvidence = @()
            foreach ($client in $clients) {
                Assert-NativeClientRunning $client "before live verification"
                $status = Native-ClientStatus $client
                if ($status -and $status.state -eq "error") { throw "Native client $($client.index) refused presentation: $($status.reason)" }
                $clientEvidence += $status
            }
            $ready = @($clientEvidence | Where-Object { $_ -and $_.state -eq "live" -and $_.frame_seq -gt 0 -and $_.own_entity -gt 0 }).Count -eq 2
            if ($ready) { break }
            $supervisor.Refresh()
            if ($supervisor.HasExited) { throw "Native authority exited before both real clients became live." }
            Start-Sleep -Milliseconds 200
        }
        if (-not $ready) { throw "Two normal native clients did not verify current source recipes and live source frames." }
        if ($clientEvidence[0].own_entity -eq $clientEvidence[1].own_entity) { throw "Native clients share a source pawn." }
        $firstClientEvidence = $clientEvidence
        $observeDeadline = [DateTime]::UtcNow.AddSeconds(10)
        while ([DateTime]::UtcNow -lt $observeDeadline) {
            foreach ($client in $clients) {
                Assert-NativeClientRunning $client "during observation"
                $status = Native-ClientStatus $client
                if (-not $status -or $status.state -ne "live") { throw "Native client $($client.index) lost verified live presentation." }
            }
            Start-Sleep -Milliseconds 200
        }
        $clientEvidence = @($clients | ForEach-Object { Native-ClientStatus $_ })
        for ($i=0; $i -lt 2; $i++) {
            if ($clientEvidence[$i].epoch -ne $firstClientEvidence[$i].epoch -or $clientEvidence[$i].own_entity -ne $firstClientEvidence[$i].own_entity -or
                $clientEvidence[$i].frame_seq -le $firstClientEvidence[$i].frame_seq) { throw "Native client $($i+1) did not advance the same owned source scene." }
        }
        Write-Json (Join-Path $Run "native_clients.json") @{ topology="two normal game clients plus one headless authority"; first=$firstClientEvidence; last=$clientEvidence }
        if (-not (Stop-NativeClients)) { throw "A normal native client missed its graceful stop deadline." }
    }
    if (-not $supervisor.WaitForExit(($Seconds + 15) * 1000)) { throw "Native supervisor exceeded the bounded run deadline." }
    if ($supervisor.ExitCode -ne 0) { throw "Native supervisor failed; inspect supervisor.err.log." }
} catch { $failure = $_.Exception.Message }
finally {
    # Clients stop through their own adapter/Director path before the authority.
    if (-not (Stop-NativeClients) -and -not $failure) { $failure = "A normal native client missed its graceful stop deadline." }
    # Ask the owned worker to quit before any fallback. Never alter any save file.
    if ($supervisor -and -not $supervisor.HasExited) {
        foreach ($control in @(Get-ChildItem -LiteralPath (Join-Path $Run "host") -Filter "native_process.json" -Recurse -ErrorAction SilentlyContinue)) {
            [IO.File]::WriteAllText((Join-Path $control.DirectoryName "stop.request"), "stop`n", $utf8)
        }
        [void]$supervisor.WaitForExit(12000)
    }
    foreach ($entry in @($tracked)) { if (Native-SameProcess $entry) { [void](Hsmp-StopRecord $entry) } }
    if (Test-Path -LiteralPath $Run) {
        $after = Saves-Snapshot
        $savesMatch = $null -ne $baseline -and (($baseline | ConvertTo-Json -Compress) -ceq ($after | ConvertTo-Json -Compress))
        $newCrashes = @(Crashes-Snapshot | Where-Object { $crashBaseline -notcontains $_ })
        $log = Join-Path $Win64 "ue4ss\UE4SS.log"
        if (Test-Path -LiteralPath $log) { Copy-Item -LiteralPath $log -Destination (Join-Path $Run "UE4SS.log") }
        $authorityEvidence = Native-AuthorityEvidence $Run
        $readyObserved = $authorityEvidence.ready
        $activeDispatch = $authorityEvidence.active
        # Every supervisor child record must have been qualified/tracked; a path
        # comparison failure must never be reported as successful child cleanup.
        $unobservedChildren = @()
        foreach ($control in @(Get-ChildItem -LiteralPath (Join-Path $Run "host") -Filter "native_process.json" -Recurse -ErrorAction SilentlyContinue)) {
            try {
                $childRecord = Get-Content -Raw -LiteralPath $control.FullName | ConvertFrom-Json
                if (-not @($tracked | Where-Object { $_.pid -eq $childRecord.pid }).Count) { $unobservedChildren += $childRecord.pid }
            } catch { $unobservedChildren += "invalid_record" }
        }
        $allStopped = Native-AllStopped @($tracked) $unobservedChildren
        if (-not $allStopped -and -not $failure) { $failure = "Owned native process cleanup was not verified." }
        if (-not $readyObserved -and -not $failure) { $failure = "Native worker never reported native_ready." }
        if (-not $DiagnosticProbeOnly -and -not $BootOnly -and @($activeDispatch | Where-Object { $_ -le 0 }).Count -and -not $failure) { $failure = "Both real clients were not proven to dispatch active native source input." }
        if ($newCrashes.Count -and -not $failure) { $failure = "Native game created a crash report." }
        if (-not $savesMatch -and -not $failure) { $failure = "A player save changed during the native run; retained for investigation." }
        Write-Json (Join-Path $Run "native_test.json") @{source_commit=$sourceCommit;evidence_level=$(if ($DiagnosticProbeOnly -or $BootOnly) { "explicit diagnostic bootstrap/network only" } else { "two actual game clients plus one native authority; no combat parity claim" }); backend=$Backend; mode=$mode; boot_only=[bool]$BootOnly; diagnostic_probe_only=[bool]$DiagnosticProbeOnly; client_evidence=$clientEvidence; client_exit_records=@($clients | Where-Object { $_.exit_record } | ForEach-Object { $_.exit_record }); active_native_dispatch_by_controller=$activeDispatch;
            failure=$failure; own_saves_unchanged=$savesMatch; processes_stopped=$allStopped; unobserved_children=$unobservedChildren; new_crashes=$newCrashes;
            native_ready_observed=$readyObserved; pass=($null -eq $failure -and $allStopped -and $savesMatch -and $readyObserved -and $newCrashes.Count -eq 0)}
    }
    Hsmp-UnregisterRun $registry
    Hsmp-ReleaseGameLock $exclusive.lock
}
Write-Host "Native run evidence: $Run"
if ($failure) { throw $failure }

#Requires -Version 5.1
<#
.SYNOPSIS
    Bounded real native authority startup/network test. Never a combat or release parity gate.
#>
[CmdletBinding()]
param(
    [string]$GamePath = "",
    [ValidateSet("null", "offscreen")][string]$Backend = "null",
    [ValidateRange(45, 180)][int]$Seconds = 100,
    [switch]$BootOnly,
    [switch]$ExerciseInput
)
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "lib\hsmp_runs.ps1")
if (-not $GamePath) { $GamePath = if ($env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR } else { Join-Path $Repo "game" } }
$GamePath = (Resolve-Path -LiteralPath $GamePath).Path
$Win64 = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$Server = Join-Path $Win64 "hsmp\hsmp-server.exe"
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
        "--arena", "Map_Arena_Yard", "--parent-pid", "$PID", "--pid-file", (Quote-Arg $pidFile), "--run-seconds", "$Seconds")
    if ($BootOnly) { $arguments += "--boot-only" }
    $supervisor = Start-Process -FilePath $Server -ArgumentList ($arguments -join " ") -WorkingDirectory $Win64 -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $Run "supervisor.out.log") -RedirectStandardError (Join-Path $Run "supervisor.err.log")
    [void]$tracked.Add((Hsmp-ProcRecord $supervisor "native_supervisor"))
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
    if ($gameProcess.Path -ine $record.exe -or $gameProcess.StartTime -lt $supervisor.StartTime) { throw "Native process identity changed before observation." }
    [void]$tracked.Add((Hsmp-ProcRecord $gameProcess "native_authority"))
    Write-Json (Join-Path $Run "processes.json") @($tracked)
    if (-not $BootOnly) {
        $probeArgs = @("native-probe", "--server", "127.0.0.1:$port", "--identity-dir", (Join-Path $Run "clients"),
            "--report", (Join-Path $Run "native_probe.json"), "--ready-seconds", "65", "--observe-seconds", "10")
        if ($ExerciseInput) { $probeArgs += "--exercise-input" }
        $probeCommand = @($probeArgs | ForEach-Object { Quote-Arg ([string]$_) }) -join " "
        $probe = Start-Process -FilePath $Server -ArgumentList $probeCommand -WorkingDirectory $Win64 -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $Run "probe.out.log") -RedirectStandardError (Join-Path $Run "probe.err.log")
        [void]$tracked.Add((Hsmp-ProcRecord $probe "native_probe"))
        Write-Json (Join-Path $Run "processes.json") @($tracked)
        if (-not $probe.WaitForExit(80000) -or $probe.ExitCode -ne 0) { throw "Native network probe failed; inspect probe.err.log." }
    }
    if (-not $supervisor.WaitForExit(($Seconds + 15) * 1000)) { throw "Native supervisor exceeded the bounded run deadline." }
    if ($supervisor.ExitCode -ne 0) { throw "Native supervisor failed; inspect supervisor.err.log." }
} catch { $failure = $_.Exception.Message }
finally {
    # Ask the owned worker to quit before any fallback. Never alter any save file.
    if ($supervisor -and -not $supervisor.HasExited) {
        foreach ($control in @(Get-ChildItem -LiteralPath (Join-Path $Run "host") -Filter "native_process.json" -Recurse -ErrorAction SilentlyContinue)) {
            [IO.File]::WriteAllText((Join-Path $control.DirectoryName "stop.request"), "stop`n", $utf8)
        }
        [void]$supervisor.WaitForExit(12000)
    }
    foreach ($entry in @($tracked)) { if (Hsmp-SameProcess $entry) { [void](Hsmp-StopRecord $entry) } }
    if (Test-Path -LiteralPath $Run) {
        $after = Saves-Snapshot
        $savesMatch = $null -ne $baseline -and (($baseline | ConvertTo-Json -Compress) -ceq ($after | ConvertTo-Json -Compress))
        $newCrashes = @(Crashes-Snapshot | Where-Object { $crashBaseline -notcontains $_ })
        $log = Join-Path $Win64 "ue4ss\UE4SS.log"
        if (Test-Path -LiteralPath $log) { Copy-Item -LiteralPath $log -Destination (Join-Path $Run "UE4SS.log") }
        $readyObserved = $false
        foreach ($events in @(Get-ChildItem -LiteralPath (Join-Path $Run "host") -Filter "hsmp_events*.jsonl" -Recurse -ErrorAction SilentlyContinue)) {
            foreach ($line in @(Get-Content -LiteralPath $events.FullName)) {
                try { $event = $line | ConvertFrom-Json } catch { continue }
                if ($event.ev -eq "x_native_worker" -and $event.state -eq "native_ready") { $readyObserved = $true }
            }
        }
        $allStopped = @($tracked | Where-Object { Hsmp-SameProcess $_ }).Count -eq 0
        if (-not $readyObserved -and -not $failure) { $failure = "Native worker never reported native_ready." }
        if ($newCrashes.Count -and -not $failure) { $failure = "Native game created a crash report." }
        if (-not $savesMatch -and -not $failure) { $failure = "A player save changed during the native run; retained for investigation." }
        Write-Json (Join-Path $Run "native_test.json") @{evidence_level="actual game bootstrap/network only"; backend=$Backend; boot_only=[bool]$BootOnly;
            failure=$failure; own_saves_unchanged=$savesMatch; processes_stopped=$allStopped; new_crashes=$newCrashes;
            native_ready_observed=$readyObserved; pass=($null -eq $failure -and $allStopped -and $savesMatch -and $readyObserved -and $newCrashes.Count -eq 0)}
    }
    Hsmp-UnregisterRun $registry
    Hsmp-ReleaseGameLock $exclusive.lock
}
Write-Host "Native run evidence: $Run"
if ($failure) { throw $failure }

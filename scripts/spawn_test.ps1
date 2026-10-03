<#
.SYNOPSIS
  Unattended spawn regression test: launches Half Sword with HSMP_AUTOTEST=1
  (auto host + solo start) with a chosen set of HSMP mods enabled, then reports
  whether a pawn was possessed in the arena.

.DESCRIPTION
  Isolation (scripts\lib\hsmp_runs.ps1):
  * takes the machine-wide HSMP run lock: refuses to start while an mp_test / demo run is live
    (it never kills or wipes another run); a DEAD run's leftovers are reaped by recorded PID;
  * its own state dir (HSMP_STATE_DIR=hsmp_state_<runid>_1, removed afterwards) and its own
    free master port (HSMP_MASTER_URL), never the default hsmp_state or the gate's ports;
  * the mod set is the RELEASE set (mods\mods.release.txt ": 1" HSMP mods); retired mods are
    never enabled, -Disable rejects names outside the set, and mods.txt is restored afterwards.

.EXAMPLE
  .\scripts\spawn_test.ps1 -Disable HSMPWorld,HSMPCombat
#>
param(
    [string[]]$Disable = @(),
    [int]$WaitSeconds = 55,
    [int]$HoldSeconds = 0,    # keep the game running after the verdict (e.g. for HSMPDiag dumps)
    [string]$GamePath = ""
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "lib\hsmp_runs.ps1")

if (-not $GamePath) {
    $cands = @()
    if ($env:HSMP_GAME_DIR) { $cands += $env:HSMP_GAME_DIR }
    $cands += (Join-Path $Repo "game")
    $wl = & git -C $Repo worktree list --porcelain 2>$null
    foreach ($l in $wl) { if ($l -like "worktree *") { $cands += (Join-Path ($l.Substring(9).Trim()) "game") } }
    foreach ($c in $cands) { if (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64")) { $GamePath = $c; break } }
    if (-not $GamePath) { Write-Host "game not found; pass -GamePath or set HSMP_GAME_DIR" -ForegroundColor Red; exit 2 }
}
$bin  = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$mods = Join-Path $bin "ue4ss\Mods\mods.txt"
$log  = Join-Path $bin "ue4ss\UE4SS.log"

# the release set: every HSMP mod the template ships; -Disable may only name those
$release = @()
foreach ($line in (Get-Content (Join-Path $Repo "mods\mods.release.txt"))) {
    if ($line -match '^\s*(HSMP[^:\s]*)\s*:\s*1\s*$') { $release += $Matches[1] }
}
$unknown = @($Disable | Where-Object { $release -notcontains $_ })
if ($unknown.Count) { Write-Host "unknown mod(s) for -Disable: $($unknown -join ', ') (release set: $($release -join ', '))" -ForegroundColor Red; exit 2 }

# bin_dir from hsmp.cfg (the binaries the game runs)
$binDir = $null
$cfg = Join-Path $bin "hsmp.cfg"
if (Test-Path $cfg) { foreach ($l in (Get-Content $cfg)) { if ($l -match '^\s*bin_dir\s*=\s*"?([^"#;]+?)"?\s*([#;].*)?$') { $binDir = $Matches[1].Trim() } } }
if ($binDir -and -not [IO.Path]::IsPathRooted($binDir)) { $binDir = Join-Path $bin $binDir }
if (-not $binDir) { Write-Host "hsmp.cfg has no bin_dir: run scripts\build-and-deploy.ps1" -ForegroundColor Red; exit 2 }

$b = Hsmp-BeginExclusiveRun
if (-not $b.lock) { Write-Host "not starting: $($b.why). Nothing was touched." -ForegroundColor Red; exit 2 }
$runId = Hsmp-NewRunId "spawn"
$stateLeaf = "hsmp_state_${runId}_1"
$state = Join-Path $bin $stateLeaf
$runDir = Join-Path ([IO.Path]::GetTempPath()) "hsmp-spawn-$runId"
New-Item -ItemType Directory -Force $runDir, $state | Out-Null
$pidFile = Join-Path $runDir "pids.json"
$script:started = New-Object System.Collections.ArrayList
function Save-Pids { [IO.File]::WriteAllText($pidFile, (ConvertTo-Json -InputObject @($script:started) -Depth 3)) }
Save-Pids
$masterPort = (Hsmp-FreePorts 1)[0]
$entry = Hsmp-RegisterRun ([ordered]@{ id = $runId; kind = "spawn_test"; real_game = $true; detached = $false; pidfile = $pidFile
                                       state_dirs = @($state); ports = @($masterPort); game = $GamePath })

# PID-only cleanup: never kill by image name (other runs and players run hsmp-* and game processes
# on this box). We stop only what this script started, plus the children those processes spawned
# (the menu's server/sidecar), each verified by PID + image name + start time; a child is taken
# only if it was created after its (verified) parent started.
function Stop-Tree($records) {
    $procs = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
    $kill = @{}; $queue = [System.Collections.Generic.Queue[object]]::new()
    foreach ($e in $records) {
        $p = Hsmp-SameProcess $e
        if ($p) { $queue.Enqueue(@([int]$p.Id, $p.StartTime)) }
    }
    while ($queue.Count) {
        $item = $queue.Dequeue(); $id = $item[0]; $started = $item[1]
        if ($kill.ContainsKey($id)) { continue }; $kill[$id] = 1
        foreach ($c in ($procs | Where-Object { $_.ParentProcessId -eq $id })) {
            if ($c.CreationDate -and $started -and $c.CreationDate -ge $started) { $queue.Enqueue(@([int]$c.ProcessId, $c.CreationDate)) }
        }
    }
    foreach ($id in $kill.Keys) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
}

$origMods = [IO.File]::ReadAllText($mods)
try {
    # only release-set HSMP mods are toggled; everything else (retired mods included) keeps
    # its deployed state, and the original mods.txt comes back afterwards
    $t = $origMods
    foreach ($m in $release) {
        $v = if ($Disable -contains $m) { 0 } else { 1 }
        $t = [regex]::Replace($t, "(?m)^$([regex]::Escape($m))\s*:\s*\d", "$m : $v")
    }
    [IO.File]::WriteAllText($mods, $t)
    $logStart = if (Test-Path $log) { (Get-Item $log).Length } else { 0 }

    $master = Start-Process -PassThru -FilePath (Join-Path $binDir "hsmp-master.exe") -ArgumentList "--bind","127.0.0.1:$masterPort" -WorkingDirectory $binDir -WindowStyle Minimized
    [void]$script:started.Add((Hsmp-ProcRecord $master "master")); Save-Pids
    $saved = @{ HSMP_AUTOTEST = $env:HSMP_AUTOTEST; HSMP_STATE_DIR = $env:HSMP_STATE_DIR; HSMP_MASTER_URL = $env:HSMP_MASTER_URL }
    $env:HSMP_AUTOTEST = "1"; $env:HSMP_STATE_DIR = $stateLeaf; $env:HSMP_MASTER_URL = "http://127.0.0.1:$masterPort"
    try { $game = Start-Process -PassThru -FilePath (Join-Path $bin "HalfswordUE5-Win64-Shipping.exe") -WorkingDirectory $bin }
    finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
    [void]$script:started.Add((Hsmp-ProcRecord $game "game1")); Save-Pids

    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $verdict = "TIMEOUT (no spawn verdict)"
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 3
        $hit = Select-String -Path $log -Pattern "spawn\[spawn\]|spawn: round \d+ slot|pawn OK|ERROR no pawn possessed|world=.*Map_Arena.*pawn_cls=Willie" -ErrorAction SilentlyContinue | Select-Object -Last 1
        if ($hit) {
            if ($hit.Line -match "ERROR no pawn") { $verdict = "FAIL: no pawn possessed" } else { $verdict = "PASS: pawn possessed" }
            break
        }
    }
    "disabled: " + ($(if ($Disable.Count) { $Disable -join "," } else { "(none)" }))
    "verdict:  $verdict"
    if ($HoldSeconds -gt 0) { Start-Sleep -Seconds $HoldSeconds }
    Select-String -Path $log -Pattern "AUTOTEST|world=.*Map_Arena|spawn\[|spawn_rescue|LUA_ERR|attempt to" -ErrorAction SilentlyContinue | Select-Object -Last 6 | ForEach-Object { "  " + ($_.Line -replace '^\[2026-\d\d-\d\d ','[' -replace '\[Lua\] ','') }
} finally {
    Stop-Tree @($script:started)
    Start-Sleep -Seconds 1
    [IO.File]::WriteAllText($mods, $origMods)
    if ((Hsmp-OwnStateDir $state $runId) -and (Test-Path -LiteralPath $state)) { Remove-Item -LiteralPath $state -Recurse -Force -ErrorAction SilentlyContinue }
    Hsmp-UnregisterRun $entry
    Hsmp-ReleaseGameLock $b.lock
}

#Requires -Version 5.1
<#
.SYNOPSIS
    Run isolation for the scripts that start game / server processes:
    scripts\mp_test.ps1, launch_net_demo.ps1, scripts\spawn_test.ps1. Dot-source it.

.DESCRIPTION
    * One machine-wide exclusive lock (%LOCALAPPDATA%\HSMP\runs\game.lock, held open with
      FileShare.None for the harness's lifetime; the OS releases it when the process dies, so a
      crashed harness never leaves it stuck). Career saves, Saved\Crashes, UE4SS.log and the
      sidecar's career guard are per user/machine: two runs at once would contaminate each
      other's evidence (and a FakeGame sidecar's career guard could restore a live game's
      career file), so a second run REFUSES to start. It never kills or wipes the other run.
    * A registry entry per run (%LOCALAPPDATA%\HSMP\runs\<id>.json): the harness PID + start
      time, the run's own PID file, state dirs and ports. A run is live while its harness lives
      (or, for a detached launcher such as launch_net_demo, while any process it started lives).
      Only a run that is NOT live is reaped: its recorded processes are stopped by PID (image
      name + start time verified) and its own state dirs removed.
    * Per-run state dir names (hsmp_state_<runid>_<i>) and per-run ports (random free ports,
      TCP and UDP checked on loopback), so even a forced overlap cannot share a dir or a port.
    Never kills by image name. HSMP_RUNS_DIR overrides the registry dir (offline self-test).
#>

$script:HsmpRunsDir = if ($env:HSMP_RUNS_DIR) { $env:HSMP_RUNS_DIR } else { Join-Path $env:LOCALAPPDATA "HSMP\runs" }

function Hsmp-RunsDir {
    New-Item -ItemType Directory -Force $script:HsmpRunsDir | Out-Null
    return $script:HsmpRunsDir
}

function Hsmp-NewRunId([string]$suffix = "") {
    $hex = ([Guid]::NewGuid().ToString("N")).Substring(0, 6)
    $id = (Get-Date -Format "yyyyMMdd-HHmmss") + "-" + $hex
    if ($suffix) { $id += "-" + $suffix }
    return $id
}

function Hsmp-ProcRecord($proc, [string]$role, [string]$launchedBy = "harness") {
    try { $ticks = $proc.StartTime.ToUniversalTime().Ticks } catch { $ticks = 0 }
    return [ordered]@{ role = $role; pid = $proc.Id; name = $proc.ProcessName; start_ticks = $ticks; launched_by = $launchedBy }
}

# The live process a record describes: same PID, same image name, same start time. A record
# without a start time never matches (a reused PID must not be taken for ours).
function Hsmp-SameProcess($e) {
    if (-not $e -or -not $e.pid) { return $null }
    $p = Get-Process -Id ([int]$e.pid) -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    if ($p.ProcessName -ne $e.name) { return $null }
    if (-not $e.start_ticks) { return $null }
    try { if ($p.StartTime.ToUniversalTime().Ticks -ne [int64]$e.start_ticks) { return $null } } catch { return $null }
    return $p
}

function Hsmp-StopRecord($e) {
    $p = Hsmp-SameProcess $e
    if ($p) { try { Stop-Process -Id $p.Id -Force -ErrorAction Stop; return $true } catch { } }
    return $false
}

# --- ports ---------------------------------------------------------------------------
function Hsmp-PortFree([int]$port) {
    if ($port -le 0 -or $port -gt 65535) { return $false }
    $u = $null; $t = $null
    try {
        $u = New-Object Net.Sockets.UdpClient((New-Object Net.IPEndPoint([Net.IPAddress]::Loopback, $port)))
        $t = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $port)
        $t.Start()
        return $true
    } catch { return $false }
    finally { if ($t) { try { $t.Stop() } catch { } }; if ($u) { try { $u.Close() } catch { } } }
}

# $n distinct ports, each free for TCP and UDP on loopback, from 20000-44999 (below the
# Windows ephemeral range, so outbound sockets do not take them later). Never one of $avoid.
function Hsmp-FreePorts([int]$n, [int[]]$avoid = @()) {
    $rng = New-Object Random
    $got = New-Object System.Collections.Generic.List[int]
    $tries = 0
    while ($got.Count -lt $n) {
        if (++$tries -gt 500) { throw "no free local ports found (wanted $n)" }
        $p = $rng.Next(20000, 45000)
        if ($got.Contains($p) -or ($avoid -contains $p)) { continue }
        if (Hsmp-PortFree $p) { $got.Add($p) }
    }
    return @($got)
}

# --- the machine-wide lock ------------------------------------------------------------
function Hsmp-AcquireGameLock {
    $p = Join-Path (Hsmp-RunsDir) "game.lock"
    try { return [IO.File]::Open($p, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
    catch { return $null }
}
function Hsmp-ReleaseGameLock($fs) { if ($fs) { try { $fs.Dispose() } catch { } } }

# --- registry -------------------------------------------------------------------------
function Hsmp-RegisterRun([System.Collections.IDictionary]$info) {
    $me = Get-Process -Id $PID
    $e = [ordered]@{}
    foreach ($k in $info.Keys) { $e[$k] = $info[$k] }
    $e.harness = Hsmp-ProcRecord $me "harness"
    $e.registered = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    $path = Join-Path (Hsmp-RunsDir) "$($info.id).json"
    [IO.File]::WriteAllText($path, (ConvertTo-Json -InputObject $e -Depth 6), (New-Object System.Text.UTF8Encoding $false))
    return $path
}
function Hsmp-UnregisterRun([string]$path) { if ($path -and (Test-Path -LiteralPath $path)) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue } }

function Hsmp-ReadRuns {
    $out = @()
    foreach ($f in @(Get-ChildItem -File (Hsmp-RunsDir) -Filter "*.json" -ErrorAction SilentlyContinue)) {
        try { $e = Get-Content -LiteralPath $f.FullName -Raw | ConvertFrom-Json } catch { continue }
        $e | Add-Member -NotePropertyName _path -NotePropertyValue $f.FullName -Force
        $out += $e
    }
    return $out
}

function Hsmp-RunRecords($entry) {
    if (-not $entry.pidfile -or -not (Test-Path -LiteralPath $entry.pidfile)) { return @() }
    try { return @(Get-Content -LiteralPath $entry.pidfile -Raw | ConvertFrom-Json) } catch { return @() }
}

# Live: the harness process (PID + image + start time) still runs; for a detached launcher,
# also while any process it recorded still runs.
function Hsmp-RunIsLive($entry) {
    if ($entry.harness -and (Hsmp-SameProcess $entry.harness)) { return $true }
    if ($entry.detached) {
        foreach ($r in (Hsmp-RunRecords $entry)) { if (Hsmp-SameProcess $r) { return $true } }
    }
    return $false
}

function Hsmp-LiveRuns([string]$exceptId = "") {
    return @(Hsmp-ReadRuns | Where-Object { $_.id -ne $exceptId -and (Hsmp-RunIsLive $_) })
}

# A state dir this tooling may delete: one listed by the run itself, named hsmp_state_<runid>_*
# (never the shared default hsmp_state or another run's dir).
function Hsmp-OwnStateDir([string]$dir, [string]$runId) {
    if (-not $dir -or -not $runId) { return $false }
    $leaf = Split-Path -Leaf $dir
    return $leaf -like ("hsmp_state_" + $runId + "_*")
}

# Reap runs that are not live: stop their recorded processes (verified by PID, image name and
# start time) and remove their own state dirs and registry entry. Returns the reaped entries.
function Hsmp-ReapStaleRuns([switch]$Quiet) {
    $reaped = @()
    foreach ($e in @(Hsmp-ReadRuns)) {
        if (Hsmp-RunIsLive $e) { continue }
        $n = 0
        foreach ($r in (Hsmp-RunRecords $e)) { if (Hsmp-StopRecord $r) { $n++ } }
        foreach ($d in @($e.state_dirs)) {
            $ds = [string]$d
            if ((Hsmp-OwnStateDir $ds $e.id) -and (Test-Path -LiteralPath $ds)) { Remove-Item -LiteralPath $ds -Recurse -Force -ErrorAction SilentlyContinue }
        }
        Hsmp-UnregisterRun $e._path
        if (-not $Quiet) { Write-Host "[runs] reaped dead run $($e.id) ($($e.kind)): stopped $n leftover process(es)" -ForegroundColor DarkGray }
        $reaped += $e
    }
    return $reaped
}

# Harness scripts running WITHOUT a registry entry (an older mp_test.ps1 / spawn_test /
# launch_net_demo from before this isolation, e.g. another worktree): never touched, but a new
# run must not start next to them. Plan-only invocations (-DryRun / -AssertOnly) are ignored.
function Hsmp-UnregisteredHarnesses {
    $known = @{}
    foreach ($e in @(Hsmp-ReadRuns)) { if ($e.harness) { $known[[int]$e.harness.pid] = $true } }
    $out = @()
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='powershell.exe' OR Name='pwsh.exe'" -ErrorAction SilentlyContinue)
    foreach ($w in $procs) {
        $cl = [string]$w.CommandLine
        if ([int]$w.ProcessId -eq $PID -or $known.ContainsKey([int]$w.ProcessId)) { continue }
        if ($cl -notmatch '(mp_test|spawn_test|launch_net_demo)\.ps1') { continue }
        if ($cl -match '-(DryRun|AssertOnly|Stop|KillPrevious)\b') { continue }
        $out += [ordered]@{ pid = [int]$w.ProcessId; cmd = $cl }
    }
    return $out
}

# Take the lock and check nobody else is running; returns @{ lock; why }. On refusal lock=$null.
function Hsmp-BeginExclusiveRun([string]$selfId = "") {
    [void](Hsmp-ReapStaleRuns -Quiet)
    $fs = Hsmp-AcquireGameLock
    if (-not $fs) {
        $live = @(Hsmp-LiveRuns $selfId | ForEach-Object { "$($_.id) ($($_.kind), harness pid $($_.harness.pid))" })
        return @{ lock = $null; why = "another HSMP run holds the game lock: $(if ($live.Count) { $live -join '; ' } else { 'unknown holder' })" }
    }
    $live = @(Hsmp-LiveRuns $selfId)
    if ($live.Count) {
        Hsmp-ReleaseGameLock $fs
        return @{ lock = $null; why = "another HSMP run is still live: $(($live | ForEach-Object { "$($_.id) ($($_.kind))" }) -join '; ')" }
    }
    $old = @(Hsmp-UnregisteredHarnesses)
    if ($old.Count) {
        Hsmp-ReleaseGameLock $fs
        return @{ lock = $null; why = "an unregistered (older) harness is running: $(($old | ForEach-Object { "pid $($_.pid)" }) -join ', ')" }
    }
    return @{ lock = $fs; why = "" }
}

#Requires -Version 5.1
<#
.SYNOPSIS
    Offline self-test of scripts\lib\hsmp_runs.ps1 (run isolation). Starts only
    dummy `powershell Start-Sleep` processes, uses a private registry dir (HSMP_RUNS_DIR) and a
    scratch Win64 folder; never touches a game, a real run, career saves or fixed ports.
    Exit 0 = every check passed.
#>
$ErrorActionPreference = "Stop"
$scratch = Join-Path ([IO.Path]::GetTempPath()) ("hsmp-runs-selftest-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
$env:HSMP_RUNS_DIR = Join-Path $scratch "runs"
. (Join-Path (Split-Path -Parent $PSScriptRoot) "lib\hsmp_runs.ps1")
$win64 = Join-Path $scratch "Win64"
New-Item -ItemType Directory -Force $win64 | Out-Null
$script:fails = 0
$script:dummies = @()
function Check([bool]$cond, [string]$what) {
    if ($cond) { Write-Host "  ok   $what" -ForegroundColor Green } else { Write-Host "  FAIL $what" -ForegroundColor Red; $script:fails++ }
}
function Dummy([int]$secs = 120, [string]$tag = "") {
    $cmd = "Start-Sleep -Seconds $secs" + $(if ($tag) { " # $tag" } else { "" })
    $p = Start-Process -PassThru -WindowStyle Hidden -FilePath "powershell.exe" -ArgumentList @("-NoProfile", "-Command", "`"$cmd`"")
    $script:dummies += $p
    Start-Sleep -Milliseconds 300
    return $p
}
function Entry([string]$id, $harnessRec, $records, [string[]]$dirs, [bool]$detached = $false) {
    $pf = Join-Path $scratch "$id.pids.json"
    [IO.File]::WriteAllText($pf, (ConvertTo-Json -InputObject @($records) -Depth 4))
    $e = [ordered]@{ id = $id; kind = "mp_test"; real_game = $true; detached = $detached; pidfile = $pf; state_dirs = $dirs; harness = $harnessRec }
    $path = Join-Path (Hsmp-RunsDir) "$id.json"
    [IO.File]::WriteAllText($path, (ConvertTo-Json -InputObject $e -Depth 6))
    return $path
}
function Alive($p) { return [bool](Get-Process -Id $p.Id -ErrorAction SilentlyContinue) }

try {
    Write-Host "[1] the machine-wide lock is exclusive and dies with its holder"
    $lock = Hsmp-AcquireGameLock
    Check ($null -ne $lock) "first acquire succeeds"
    $lib = Join-Path (Split-Path -Parent $PSScriptRoot) "lib\hsmp_runs.ps1"
    $probe = "`$env:HSMP_RUNS_DIR='$($env:HSMP_RUNS_DIR)'; . '$lib'; if (Hsmp-AcquireGameLock) { 'GOT' } else { 'LOCKED' }"
    $out = (& powershell.exe -NoProfile -Command $probe | Out-String).Trim()
    Check ($out -eq "LOCKED") "a second process cannot take it while held ($out)"
    Hsmp-ReleaseGameLock $lock
    $out = (& powershell.exe -NoProfile -Command $probe | Out-String).Trim()
    Check ($out -eq "GOT") "free again after release ($out)"

    Write-Host "[2] reaping: a live run is never touched; a dead run's own processes and dirs go"
    $h1 = Dummy; $g1 = Dummy; $g2 = Dummy
    $dead = Dummy 1; Start-Sleep -Seconds 2
    $deadRec = Hsmp-ProcRecord $dead "harness"
    $deadRec.start_ticks = 1234567   # a harness that is gone (its PID may be reused: ticks differ)
    $d1 = Join-Path $win64 "hsmp_state_R1_1"; $d2 = Join-Path $win64 "hsmp_state_R2_1"
    $shared = Join-Path $win64 "hsmp_state"; $legacy = Join-Path $win64 "hsmp_state_1"
    foreach ($d in @($d1, $d2, $shared, $legacy)) { New-Item -ItemType Directory -Force $d | Out-Null; Set-Content (Join-Path $d "x.txt") "x" }
    $e1 = Entry "R1" (Hsmp-ProcRecord $h1 "harness") @(Hsmp-ProcRecord $g1 "game1") @($d1)
    # R2 is dead; its entry also (wrongly) lists R1's dir, the shared default dir and a legacy dir
    $e2 = Entry "R2" $deadRec @(Hsmp-ProcRecord $g2 "game1") @($d2, $d1, $shared, $legacy)
    $reaped = @(Hsmp-ReapStaleRuns -Quiet)
    Check ($reaped.Count -eq 1 -and $reaped[0].id -eq "R2") "only the dead run R2 is reaped"
    Check (Alive $g1) "R1's game (live harness) still runs"
    Check (-not (Alive $g2)) "R2's leftover game was stopped by PID"
    Check (Test-Path $e1) "R1's registry entry kept"
    Check (-not (Test-Path $e2)) "R2's registry entry removed"
    Check (-not (Test-Path $d2)) "R2's own state dir removed"
    Check (Test-Path (Join-Path $d1 "x.txt")) "R1's state dir untouched (listed by R2, but not R2's own)"
    Check (Test-Path (Join-Path $shared "x.txt")) "the shared default hsmp_state untouched"
    Check (Test-Path (Join-Path $legacy "x.txt")) "a legacy hsmp_state_1 untouched"

    Write-Host "[3] a reused PID is never taken for ours"
    $g3 = Dummy
    $rec = Hsmp-ProcRecord $g3 "game1"; $rec.start_ticks = [int64]$rec.start_ticks + 10000000
    Check (-not (Hsmp-StopRecord $rec)) "StopRecord refuses a PID whose start time differs"
    Check (Alive $g3) "that process still runs"
    $rec0 = Hsmp-ProcRecord $g3 "game1"; $rec0.start_ticks = 0
    Check (-not (Hsmp-SameProcess $rec0)) "a record without a start time never matches"
    $recName = Hsmp-ProcRecord $g3 "game1"; $recName.name = "HalfswordUE5-Win64-Shipping"
    Check (-not (Hsmp-SameProcess $recName)) "a record with another image name never matches"

    Write-Host "[4] a detached launcher's run is live while any process it started lives"
    $g4 = Dummy
    $e4 = Entry "D1" $deadRec @(Hsmp-ProcRecord $g4 "game1") @((Join-Path $win64 "hsmp_state_D1_1")) $true
    [void](Hsmp-ReapStaleRuns -Quiet)
    Check ((Test-Path $e4) -and (Alive $g4)) "detached run with a live game is not reaped"
    Stop-Process -Id $g4.Id -Force; Start-Sleep -Milliseconds 300
    [void](Hsmp-ReapStaleRuns -Quiet)
    Check (-not (Test-Path $e4)) "reaped once its processes are gone"

    Write-Host "[5] a new run refuses (and touches nothing) while another run is live"
    $b = Hsmp-BeginExclusiveRun "SELF"
    Check ($null -eq $b.lock -and $b.why -match "R1") "BeginExclusiveRun refuses: $($b.why)"
    Check ((Alive $g1) -and (Alive $h1)) "the live run's processes were not touched"
    Stop-Process -Id $h1.Id -Force; Start-Sleep -Milliseconds 300
    $b = Hsmp-BeginExclusiveRun "SELF"
    # (a real, unregistered harness elsewhere on this box may still refuse us: that is correct)
    Check (($null -ne $b.lock) -or ($b.why -match "unregistered")) "after R1's harness died: no live registered run blocks us ($(if ($b.lock) { 'lock taken' } else { $b.why }))"
    Check (-not (Test-Path $e1)) "R1 reaped"
    Check (-not (Alive $g1)) "R1's orphaned game stopped by PID when its harness was gone"
    Hsmp-ReleaseGameLock $b.lock

    Write-Host "[6] an unregistered (older) harness blocks a new run; plan-only invocations do not"
    $old = Dummy 120 "mp_test.ps1 -Scenario p0_gate"
    $dry = Dummy 120 "mp_test.ps1 -Scenario p0_gate -DryRun"
    $un = @(Hsmp-UnregisteredHarnesses)
    Check (@($un | Where-Object { $_.pid -eq $old.Id }).Count -eq 1) "a running pre-isolation mp_test is detected"
    Check (@($un | Where-Object { $_.pid -eq $dry.Id }).Count -eq 0) "a -DryRun invocation is ignored"
    $b = Hsmp-BeginExclusiveRun "SELF"
    Check ($null -eq $b.lock -and $b.why -match "unregistered") "BeginExclusiveRun refuses next to it: $($b.why)"
    Check (Alive $old) "the older harness was not touched"

    Write-Host "[7] ports: distinct, free for TCP and UDP, an explicit busy port is detected"
    $ports = @(Hsmp-FreePorts 8)
    Check ($ports.Count -eq 8 -and (@($ports | Select-Object -Unique).Count -eq 8)) "8 distinct ports: $($ports -join ',')"
    Check (@($ports | Where-Object { $_ -lt 20000 -or $_ -ge 45000 }).Count -eq 0) "all in 20000-44999"
    $busy = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $ports[0]); $busy.Start()
    Check (-not (Hsmp-PortFree $ports[0])) "a TCP-bound port is not free"
    $busy.Stop()
    $u = New-Object Net.Sockets.UdpClient((New-Object Net.IPEndPoint([Net.IPAddress]::Loopback, $ports[1])))
    Check (-not (Hsmp-PortFree $ports[1])) "a UDP-bound port is not free"
    $again = @(Hsmp-FreePorts 4 @($ports[1]))
    Check (@($again | Where-Object { $_ -eq $ports[1] }).Count -eq 0) "an avoided port is never returned"
    $u.Close()

    Write-Host "[8] own-dir rule"
    Check (Hsmp-OwnStateDir (Join-Path $win64 "hsmp_state_R9_2") "R9") "hsmp_state_<id>_<i> is the run's own"
    Check (-not (Hsmp-OwnStateDir (Join-Path $win64 "hsmp_state_R9_2") "R1")) "another run's dir is not"
    Check (-not (Hsmp-OwnStateDir (Join-Path $win64 "hsmp_state") "R9")) "the shared default dir is not"
    Check (-not (Hsmp-OwnStateDir (Join-Path $win64 "hsmp_state_2") "R9")) "a legacy numbered dir is not"
} finally {
    foreach ($p in $script:dummies) { if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
    Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}
Write-Host ""
if ($script:fails) { Write-Host "hsmp_runs selftest: $($script:fails) FAILED" -ForegroundColor Red; exit 1 }
Write-Host "hsmp_runs selftest: all checks passed" -ForegroundColor Green
exit 0

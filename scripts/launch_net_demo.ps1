# Launch the network-multiplayer demo on this PC:
#   - 1x Rust dedicated server on a FREE local port
#   - (each game starts its own sidecar over shared memory when it joins)
#   - 2x Half Sword instances, each with its own state dir
#
# Isolation (scripts\lib\hsmp_runs.ps1):
#   * it takes the machine-wide HSMP run lock and REFUSES to start while an mp_test /
#     spawn_test / other demo run is live: it never stops or wipes another run;
#   * its state dirs are its own (Binaries\Win64\hsmp_state_<runid>_<i>), never the gate's
#     hsmp_state_<i> or the default hsmp_state, and its server port is a free one;
#   * -Stop stops ONLY the processes the demo runs recorded (PID + image name + start time)
#     and removes only their own state dirs.
#
# Usage (from any shell):
#   powershell -ExecutionPolicy Bypass -File <repo>\launch_net_demo.ps1
# Stop the demo:
#   powershell -ExecutionPolicy Bypass -File <repo>\launch_net_demo.ps1 -Stop

param(
    [switch]$Stop,
    [string]$ServerHost = "127.0.0.1",
    [int]$ServerPort = 0,            # 0 = a free port
    [string]$GamePath = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
. (Join-Path $Root "scripts\lib\hsmp_runs.ps1")

function Stop-Demo {
    Write-Host "=== Stopping the demo runs' own processes (recorded PIDs only) ==="
    foreach ($e in @(Hsmp-ReadRuns | Where-Object { $_.kind -eq "net_demo" })) {
        foreach ($r in (Hsmp-RunRecords $e)) {
            if (Hsmp-StopRecord $r) { Write-Host "stopped $($r.role) $($r.name) pid $($r.pid)" }
        }
        Start-Sleep -Milliseconds 500
        foreach ($d in @($e.state_dirs)) {
            $ds = [string]$d
            if ((Hsmp-OwnStateDir $ds $e.id) -and (Test-Path -LiteralPath $ds)) { Remove-Item -LiteralPath $ds -Recurse -Force -ErrorAction SilentlyContinue }
        }
        if ($e.pidfile -and (Test-Path -LiteralPath $e.pidfile)) { Remove-Item -LiteralPath $e.pidfile -Force -ErrorAction SilentlyContinue }
        Hsmp-UnregisterRun $e._path
    }
    Write-Host "done."
}

if ($Stop) { Stop-Demo; exit 0 }

if (-not $GamePath) {
    $cands = @()
    if ($env:HSMP_GAME_DIR) { $cands += $env:HSMP_GAME_DIR }
    $cands += (Join-Path $Root "game")
    $wl = & git -C $Root worktree list --porcelain 2>$null
    foreach ($l in $wl) { if ($l -like "worktree *") { $cands += (Join-Path ($l.Substring(9).Trim()) "game") } }
    foreach ($c in $cands) { if (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64")) { $GamePath = $c; break } }
    if (-not $GamePath) { throw "game not found; pass -GamePath or set HSMP_GAME_DIR" }
}
$GameCwd = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$GameExe = Join-Path $GameCwd "HalfswordUE5-Win64-Shipping.exe"
# the binaries the game runs (hsmp.cfg bin_dir, relative to Win64), else this repo's build
$BinDir = $null
$cfg = Join-Path $GameCwd "hsmp.cfg"
if (Test-Path $cfg) { foreach ($l in (Get-Content $cfg)) { if ($l -match '^\s*bin_dir\s*=\s*"?([^"#;]+?)"?\s*([#;].*)?$') { $BinDir = $Matches[1].Trim() } } }
if ($BinDir -and -not [IO.Path]::IsPathRooted($BinDir)) { $BinDir = Join-Path $GameCwd $BinDir }
if (-not $BinDir) { $BinDir = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $Root "target\release" } }
$ServerExe = Join-Path $BinDir "hsmp-server.exe"
$SidecarExe = Join-Path $BinDir "hsmp-sidecar.exe"
if (-not (Test-Path $ServerExe))  { throw "server not found: $ServerExe (run scripts\build-and-deploy.ps1)" }
if (-not (Test-Path $SidecarExe)) { throw "sidecar not found: $SidecarExe (run scripts\build-and-deploy.ps1)" }
if (-not (Test-Path $GameExe))    { throw "game exe missing: $GameExe" }

$b = Hsmp-BeginExclusiveRun
if (-not $b.lock) { Write-Host "not starting: $($b.why). Nothing was touched (stop the other run first, or -Stop a previous demo)." -ForegroundColor Red; exit 2 }
try {
    if ($ServerPort -le 0) { $ServerPort = (Hsmp-FreePorts 1)[0] }
    elseif (-not (Hsmp-PortFree $ServerPort)) { throw "port $ServerPort is in use" }
    $runId = Hsmp-NewRunId "demo"
    $StateDir1 = Join-Path $GameCwd "hsmp_state_${runId}_1"
    $StateDir2 = Join-Path $GameCwd "hsmp_state_${runId}_2"
    New-Item -ItemType Directory -Force -Path $StateDir1, $StateDir2 | Out-Null
    $LogDir = Join-Path ([IO.Path]::GetTempPath()) "hsmp-demo-$runId"
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $PidFile = Join-Path $LogDir "pids.json"
    $script:Tracked = New-Object System.Collections.ArrayList
    function Track($proc, [string]$role) {
        [void]$script:Tracked.Add((Hsmp-ProcRecord $proc $role))
        [IO.File]::WriteAllText($PidFile, (ConvertTo-Json -InputObject @($script:Tracked) -Depth 4))
    }
    [IO.File]::WriteAllText($PidFile, "[]")
    # detached: the run stays "live" while any process it started runs (this launcher exits now)
    [void](Hsmp-RegisterRun ([ordered]@{ id = $runId; kind = "net_demo"; real_game = $true; detached = $true; pidfile = $PidFile
                                         state_dirs = @($StateDir1, $StateDir2); ports = @($ServerPort); game = $GamePath }))

    Write-Host ""
    Write-Host "=== Starting dedicated server on ${ServerHost}:${ServerPort} ==="
    $env:RUST_LOG = "hsmp_server=debug,hsmp_sidecar=debug"
    $server = Start-Process -FilePath $ServerExe `
        -ArgumentList "--bind", "${ServerHost}:${ServerPort}", "--max-peers", "8" `
        -WorkingDirectory $GameCwd -WindowStyle Minimized `
        -RedirectStandardOutput "$LogDir\server.out.log" -RedirectStandardError "$LogDir\server.err.log" -PassThru
    Track $server "server"
    Write-Host "server pid=$($server.Id)  logs=$LogDir\server.*.log"
    Start-Sleep -Seconds 1

    # No standalone sidecars: each game starts its own sidecar when it joins
    # (HSMPMenu: the shared-memory segment, then `hsmp-sidecar --parent-pid <game> --ipc shm:...`).
    Write-Host ""
    Write-Host "Each game starts its own sidecar when you join ${ServerHost}:${ServerPort} from its server browser."

    $k = 0
    foreach ($sd in @($StateDir1, $StateDir2)) {
        $k++
        Write-Host ""
        Write-Host "=== Launching Half Sword instance #$k (HSMP_STATE_DIR=$sd) ==="
        $prevState = $env:HSMP_STATE_DIR
        $env:HSMP_STATE_DIR = $sd
        try { $game = Start-Process -FilePath $GameExe -ArgumentList "-nohmd", "-emulatestereo" -WorkingDirectory $GameCwd -PassThru }
        finally { $env:HSMP_STATE_DIR = $prevState }
        Track $game "game$k"
        Write-Host "game$k pid=$($game.Id)"
        if ($k -eq 1) { Start-Sleep -Seconds 10 }
    }

    Write-Host ""
    Write-Host "=== Launched (run $runId). Both games should connect to the server via sidecars. ==="
    foreach ($e in $script:Tracked) { Write-Host ("    {0,-9} pid {1}" -f "$($e.role):", $e.pid) }
    Write-Host ""
    Write-Host "UE4SS log: $GameCwd\ue4ss\UE4SS.log"
    Write-Host "To stop the demo:  powershell -ExecutionPolicy Bypass -File $PSCommandPath -Stop"
} finally {
    Hsmp-ReleaseGameLock $b.lock
}

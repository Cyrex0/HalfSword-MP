#Requires -Version 5.1
<#
.SYNOPSIS
    HSMP gate runner. N game instances, per-instance
    state + netsim, RCON-driven scenario, assertions -> report.json + junit.xml.

.DESCRIPTION
     0. Isolation (scripts\lib\hsmp_runs.ps1): takes the machine-wide HSMP run
        lock and REFUSES to start (exit 2) while another run is live; it never kills or
        wipes another run. Only runs whose harness is dead are reaped (their recorded PIDs,
        image name + start time verified; never by image name). Every run has its own PID
        record (<run>\mp_test.pids.json), its own state dirs and its own free ports.
     1. Always (re)builds hsmp-gate / hsmp-tools (cargo --locked; a no-op when current) so
        the run is judged by the rules in this tree; the gate's commit goes into report.json.
     2. Fresh state dirs Binaries\Win64\hsmp_state_<runid>_<i>; HSMP_STATE_DIR, HSMP_INST=<i>,
        HSMP_AUTOTEST=host|join, HSMP_DEV=1, per-instance netsim (HSMP_NETSIM_ADDR).
        Re-hashes the deployed files against hsmp_deploy.json (run.json deploy_check, DoD-13).
     3. Hashes SaveGames\*.sav and lists Saved\Crashes (baseline.json).
     4. Starts hsmp-master, hsmp-server (--events/--debug-verbs only if the binary
        has them), one `hsmp-tools netsim` per client and the `hsmp-gate observe` sampler.
     5. Starts N games; waits for lobby_ready from each.
     6. Drives the scenario steps (data: tools/hsmp-tools/src/bin/hsmp-gate/scenarios.json) through RCON / the autotest
        command file / netsim control files. Waits are event predicates, not sleeps;
        the only sleeps are "hold" steps (impairment or observation durations).
     7. Collects hsmp_events*.jsonl, the sidecar taps ipc_tap.jsonl (the sidecar's record of every server
        command answer, DoD-10), server.jsonl/.log, netsim logs, UE4SS.log;
        re-hashes saves, diffs crashes, scans for orphan hsmp-* processes (final.json).
     8. hsmp-gate assert -> report.json + junit.xml. Exit code = verdict
        (0 pass, 1 fail, 2 incomplete/not runnable).

    Run it against the real game from a desktop session (it starts game windows); the
    verdict is in <run>\report.json. See docs\development\testing.md.

.PARAMETER Scenario
    p0_gate | p0_wifi | map_change | start_refused | reconnect | host_leave |
    server_restart | p0_smoke_autotest | soak_60m | soak_10m   (hsmp-gate scenarios)
    Soak scenarios also start `hsmp-gate memwatch` (mem.jsonl: private bytes /
    working set / handles of every tracked process every soak.mem_sample_s) and are judged
    by the SOAK-CRASH/MEM/LUAERR/HITCH rules. Every run ends with `hsmp-tools crash-triage`
    over the run window (crash_triage.json, used by DoD-2).
.PARAMETER Netsim
    One profile for every client ("typical"), or a comma list per instance
    ("typical,wifi"), or "none". Default: the scenario's profile.
.PARAMETER Mode
    auto (rcon if hsmp-server has --debug-verbs, else the STOPGAP autotest mode),
    rcon, or autotest.
.PARAMETER FakeGame
    Use `hsmp-gate fake-game` instead of the game (harness self-test with
    the real server/sidecar/netsim). -GamePath may then be any scratch folder.
.PARAMETER Ipc
    The game<->sidecar IPC: shm, the only backend (the parameter is kept so
    existing command lines still parse). Each sidecar writes its tap to
    <run>\inst<N>\ipc_tap.jsonl (HSMP_IPC_TAP), an `hsmp-tools ipc-dump --json` snapshot is
    saved to <run>\inst<N>\ipc_dump.json before the games quit, and the final state-dir
    listing goes to <run>\inst<N>\state_dir_listing.txt (gate rule STATE-1: only the
    allow-listed log / config names may be there). `hsmp-gate ab-diff --a <run> --b <run>`
    still compares two runs (docs/development/ipc-shared-memory.md).
.PARAMETER HoldScale
    Multiply "hold" durations (e.g. 0.1 for quick harness tests).
.PARAMETER DryRun
    Print the plan (env, commands, steps) and write run.json/plan.txt; start nothing.
.PARAMETER AssertOnly
    Re-evaluate an existing run directory and exit with its verdict.
.PARAMETER KillPrevious
    Only reap DEAD runs (harness gone: stop the processes they recorded, remove their
    state dirs), then exit. A live run is never touched.
.PARAMETER ServerArgs
    Extra hsmp-server flags appended to the dedicated server's command line
    (e.g. -ServerArgs '--mods-dir','C:\hsmp-test-mods').
.PARAMETER ServerPort
    Ports default to 0 = a free port picked per run (ServerPort, RconPort, MasterPort, and
    one netsim port per instance). An explicit port must be free or the run refuses to start.

.EXAMPLE
    PS> .\scripts\build-and-deploy.ps1
    PS> .\scripts\mp_test.ps1 -Instances 2 -Scenario p0_gate -Netsim typical -Arenas all -Rounds 10
#>
[CmdletBinding()]
param(
    [int]$Instances = 2,
    [string]$Scenario = "p0_gate",
    [ValidateSet("", "duel", "brawl")][string]$CombatMode = "",
    [string]$Netsim = "",
    [string]$Arenas = "",
    [int]$Rounds = 0,
    [ValidateSet("auto", "rcon", "autotest")][string]$Mode = "auto",
    [string]$GamePath = "",
    [string]$BinDir = "",
    [string]$RunRoot = "",
    [int]$ServerPort = 0,
    [int]$RconPort = 0,
    [int]$MasterPort = 0,
    [int]$NetsimBasePort = 0,
    [int]$GameStartTimeout = 90,
    [double]$HoldScale = 1.0,
    [switch]$FakeGame,
    [ValidateSet("", "shm")][string]$Ipc = "shm",
    [switch]$DryRun,
    [string]$AssertOnly = "",
    [switch]$KillPrevious,
    # Extra game command-line arguments (e.g. --ue4ss-path,<dll> for a UE4SS settings A/B) and the
    # allow-listed test cvars HSMPMatch applies (docs/development/testing.md); defaults = the gate's.
    [string[]]$GameArgs = @(),
    [string]$TestCvars = "r.VSync=0;t.MaxFPS=60",
    # Extra .settings.json keys for every instance, as a JSON object (A/B switches such as
    # {"native_sample":true,"native_servo":true}). The instance's nick / server always win.
    [string]$ExtraSettings = "",
    [string[]]$ServerArgs = @(),
    # combat_manual only: arena, kit rules (free|classes|custom), each player's kit
    # ("<class> [r=<id>] [l=<id>] [armor=<id,..>]"), and the dev switches after Live.
    [string]$CombatArena = "",
    [string]$CombatKit = "",
    [string]$CombatKitRules = "",
    [switch]$CombatAi,
    [switch]$CombatProbe,
    # Lab session reuses this harness's save guard, process identities and teardown.
    [string]$LabSession = "",
    [int]$LabOwnerPid = 0,
    [string]$LabServerArgsJson = ""
)

$ErrorActionPreference = "Stop"
if ($LabServerArgsJson) {
    $labExtraArgs = @($LabServerArgsJson | ConvertFrom-Json)
    if (-not $LabServerArgsJson.TrimStart().StartsWith('[') -or @($labExtraArgs | Where-Object { $_ -isnot [string] -or $_ -match '[\r\n]' }).Count) { throw 'Lab server args must be a JSON array of strings.' }
    $ServerArgs += @($labExtraArgs)
}
$Repo = Split-Path -Parent $PSScriptRoot
$Utf8NoBom = New-Object System.Text.UTF8Encoding $false
if (-not $RunRoot) { $RunRoot = Join-Path $Repo "test-results" }
New-Item -ItemType Directory -Force $RunRoot | Out-Null
$HarnessPid = $PID
. (Join-Path $PSScriptRoot "lib\hsmp_runs.ps1")

function Say([string]$m, [ConsoleColor]$c = "Gray") { Write-Host "[mp_test] $m" -ForegroundColor $c }
function NowMs { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
function Write-Utf8([string]$p, [string]$t) { [IO.File]::WriteAllText($p, $t, $Utf8NoBom) }
function Append-Utf8([string]$p, [string]$t) { [IO.File]::AppendAllText($p, $t, $Utf8NoBom) }
function ToJson($o) { return ($o | ConvertTo-Json -Compress -Depth 8) }

# --- -KillPrevious: reap DEAD runs only (a live run is never touched) -----------------
if ($KillPrevious) {
    $r = @(Hsmp-ReapStaleRuns)
    $live = @(Hsmp-LiveRuns)
    Say "reaped $($r.Count) dead run(s); $($live.Count) live run(s) left alone$(if ($live.Count) { ': ' + (($live | ForEach-Object { $_.id }) -join ', ') })"
    exit 0
}

# --- isolation: one run at a time on this machine ---------------------------------------
# Career saves, Saved\Crashes, UE4SS.log and the sidecar career guard are per user: a second run
# would contaminate (or, through its career guard, restore) the first one's files. Refuse; never
# kill. A plan (-DryRun) or a re-evaluation (-AssertOnly) starts nothing and needs no lock.
$script:RunLock = $null
$script:RunEntry = $null
if (-not $DryRun -and -not $AssertOnly) {
    $b = Hsmp-BeginExclusiveRun
    if (-not $b.lock) { Say "not starting: $($b.why). Nothing was touched; wait for it to finish (or -KillPrevious once its harness is gone)." Red; exit 2 }
    $script:RunLock = $b.lock
}

# --- tools (Rust; no Python in this repo): hsmp-gate + hsmp-tools ------------------
# always build (a no-op when current), so the run is judged by THIS tree's rules and run
# with this tree's netsim / crash-triage; --locked: exactly the committed dependency set.
$ToolsDir = Join-Path $Repo "tools\hsmp-tools"
$ToolsTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo "target" }
$Gate = Join-Path $ToolsTarget "release\hsmp-gate.exe"
$Tools = Join-Path $ToolsTarget "release\hsmp-tools.exe"
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargo) { $cargo.Source } else { "$env:USERPROFILE\.cargo\bin\cargo.exe" }
if (Test-Path $cargo) {
    Say "building hsmp-gate / hsmp-tools (release, --locked; no-op when current)..."
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    & $cargo build --release --locked --quiet -p hsmp-tools --manifest-path (Join-Path $Repo "Cargo.toml") 2>&1 | ForEach-Object { Write-Host "  $_" }
    $buildCode = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($buildCode -ne 0) { Say "hsmp-gate / hsmp-tools build failed (exit $buildCode): refusing to judge with stale binaries" Red; Hsmp-ReleaseGameLock $script:RunLock; exit 2 }
} elseif (-not (Test-Path $Gate)) {
    Say "cargo not found and hsmp-gate not built ($Gate)" Red; Hsmp-ReleaseGameLock $script:RunLock; exit 2
} else {
    Say "cargo not found: using the existing $Gate (its commit is recorded; DoD-13 checks it)" Yellow
}
if (-not (Test-Path $Gate) -or -not (Test-Path $Tools)) { Say "hsmp-gate / hsmp-tools missing under $ToolsTarget\release" Red; Hsmp-ReleaseGameLock $script:RunLock; exit 2 }
$GateBuild = $null
try { $GateBuild = (& $Gate version 2>$null | Out-String).Trim() | ConvertFrom-Json } catch { $GateBuild = $null }
function Invoke-Gate([string[]]$a) {
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try { $out = & $Gate @a 2>&1 | ForEach-Object { "$_" }; return @{ code = $LASTEXITCODE; out = ($out -join "`n") } }
    finally { $ErrorActionPreference = $prev }
}

if ($AssertOnly) {
    $r = Invoke-Gate @("assert", "--run", $AssertOnly)
    Write-Host $r.out
    exit $r.code
}

# --- PID record: THIS run's processes only (<run>\mp_test.pids.json, set once the run dir exists) ---
$PidFile = $null
$script:Tracked = New-Object System.Collections.ArrayList
function Save-PidFile { if ($PidFile) { Write-Utf8 $PidFile (ConvertTo-Json -InputObject @($script:Tracked) -Depth 4) } }
function Track-Process($proc, [string]$role, [string]$launchedBy = "harness") {
    # launched_by: "harness" (Start-Tracked) or "game" (a sidecar / listen server the game
    # spawned, found by Discover-Children). Only harness-launched processes are exempt from the
    # DoD-12 orphan scan; a game-launched one alive after its game quit is an orphan.
    [void]$script:Tracked.Add((Hsmp-ProcRecord $proc $role $launchedBy))
    Save-PidFile
}
# PID + image name + start time (a record without a start time never matches)
function Same-Process($e) { return Hsmp-SameProcess $e }
function Stop-Tracked($e) {
    if (Hsmp-StopRecord $e) { Say "  stopped $($e.role) pid $($e.pid)" DarkGray }
}

# --- game / binaries -----------------------------------------------------------------
if (-not $GamePath) {
    $cands = @()
    if ($env:HSMP_GAME_DIR) { $cands += $env:HSMP_GAME_DIR }
    $cands += (Join-Path $Repo "game")
    $wl = & git -C $Repo worktree list --porcelain 2>$null
    foreach ($l in $wl) { if ($l -like "worktree *") { $cands += (Join-Path ($l.Substring(9).Trim()) "game") } }
    foreach ($c in $cands) { if (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64")) { $GamePath = $c; break } }
    if (-not $GamePath) { Say "game not found; pass -GamePath or set HSMP_GAME_DIR" Red; exit 2 }
}
$Win64 = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$GameExe = Join-Path $Win64 "HalfswordUE5-Win64-Shipping.exe"
if ($FakeGame -and -not $DryRun) { New-Item -ItemType Directory -Force $Win64 | Out-Null }
if (-not $BinDir) {
    $cfgPath = Join-Path $Win64 "hsmp.cfg"
    if (Test-Path $cfgPath) {
        foreach ($l in (Get-Content $cfgPath)) {
            if ($l -match '^\s*bin_dir\s*=\s*"?([^"#;]+?)"?\s*([#;].*)?$') { $BinDir = $Matches[1].Trim() }
        }
        if ($BinDir -and -not [IO.Path]::IsPathRooted($BinDir)) { $BinDir = Join-Path $Win64 $BinDir }
    }
    if (-not $BinDir) { $BinDir = Join-Path $ToolsTarget "release" }
}
$ServerExe = Join-Path $BinDir "hsmp-server.exe"
$SidecarExe = Join-Path $BinDir "hsmp-sidecar.exe"
$MasterExe = Join-Path $BinDir "hsmp-master.exe"
Say "game = $GamePath$(if ($FakeGame) { '  (FAKE GAME: hsmp-gate fake-game)' })"
Say "bins = $BinDir"

# --- ports: per run. 0 = a free port; an explicit port must be free ----------------------------
$fixedPorts = @(@($ServerPort, $RconPort, $MasterPort) | Where-Object { $_ -gt 0 })
if ($NetsimBasePort -gt 0) { for ($i = 1; $i -le $Instances; $i++) { $fixedPorts += ($NetsimBasePort + $i) } }
if (-not $DryRun) {
    foreach ($p in $fixedPorts) { if (-not (Hsmp-PortFree $p)) { Say "port $p is in use (another server / run?): pick another or pass 0 for a free one" Red; Hsmp-ReleaseGameLock $script:RunLock; exit 2 } }
}
$want = 3 + $Instances
$free = @(Hsmp-FreePorts $want $fixedPorts)
$k = 0
if ($ServerPort -le 0) { $ServerPort = $free[$k++] }
if ($RconPort -le 0) { $RconPort = $free[$k++] }
if ($MasterPort -le 0) { $MasterPort = $free[$k++] }
$NetsimPorts = @{}
for ($i = 1; $i -le $Instances; $i++) { $NetsimPorts["$i"] = if ($NetsimBasePort -gt 0) { $NetsimBasePort + $i } else { $free[$k++] } }
function Netsim-Port([int]$i) { return [int]$NetsimPorts["$i"] }
Say "ports: server $ServerPort, rcon $RconPort, master $MasterPort, netsim $((1..$Instances | ForEach-Object { Netsim-Port $_ }) -join ',')"

# --- capabilities: tolerate missing --events / --debug-verbs ---------------------------
$caps = [ordered]@{ events = $false; debug_verbs = $false; admin_keys = $false; server_found = (Test-Path $ServerExe) }
if ($caps.server_found) {
    $help = (& $ServerExe --help 2>&1 | Out-String)
    $caps.events = $help -match "--events"
    $caps.debug_verbs = $help -match "--debug-verbs"
    $caps.admin_keys = $help -match "--owner-key-file"
}
$eff = $Mode
if ($Mode -eq "auto") { $eff = if ($caps.debug_verbs) { "rcon" } else { "autotest" } }
if ($eff -eq "autotest") {
    Say "STOPGAP: hsmp-server has no --debug-verbs yet -> legacy HSMP_AUTOTEST mode (observe only, never green)" Yellow
}
if (-not $caps.events) { Say "hsmp-server has no --events yet -> assertions parse server.log" Yellow }

# --- scenario (data lives in hsmp-gate scenarios.json) ------------------------------------------------
$scArgs = @("scenario", $Scenario, "--instances", "$Instances", "--mode", $eff)
if ($Arenas) { $scArgs += @("--arenas", $Arenas) }
if ($Rounds -gt 0) { $scArgs += @("--rounds", "$Rounds") }
$r = Invoke-Gate $scArgs
if ($r.code -ne 0) { Say "scenario error: $($r.out)" Red; exit 2 }
$sc = $r.out | ConvertFrom-Json
if ($CombatMode) {
    if ($Scenario -ne "combat_manual") { throw '-CombatMode applies only to combat_manual.' }
    foreach ($step in $sc.steps) {
        if ($step.do -eq 'rcon' -and $step.cmd -eq 'MODE duel') { $step.cmd = "MODE $CombatMode" }
    }
}
if ($CombatArena -or $CombatKit -or $CombatKitRules -or $CombatAi -or $CombatProbe) {
    # The first match is the real one: arena, kit rules and each player's kit are set in the
    # lobby BEFORE the first START (no default-kit round to abort), and the dev switches go on
    # once it is Live (HSMPParity `ai auto` keeps every later round on the AI too).
    if ($Scenario -ne "combat_manual") { throw '-CombatArena/-CombatKit/-CombatKitRules/-CombatAi/-CombatProbe apply only to combat_manual.' }
    $steps = New-Object System.Collections.Generic.List[object]
    foreach ($step in $sc.steps) {
        if ($CombatArena -and $step.do -eq 'rcon' -and "$($step.cmd)" -like 'MAP *') { $step.cmd = "MAP $CombatArena"; $step.pick = $CombatArena }
        if ($CombatArena -and $step.do -eq 'mark' -and $step.name -eq 'start') { $step.arena = $CombatArena }
        if ($step.do -eq 'rcon' -and $step.cmd -eq 'START') {
            if ($CombatKitRules) { $steps.Add([pscustomobject]@{ do = 'rcon'; cmd = "KIT $CombatKitRules" }) }
            if ($CombatKit) {
                for ($k = 1; $k -le $Instances; $k++) { $steps.Add([pscustomobject]@{ do = 'client_cmd'; inst = $k; cmd = 'kit'; arg = $CombatKit }) }
                $steps.Add([pscustomobject]@{ do = 'hold'; s = 3.0 / [Math]::Max($HoldScale, 0.001); why = 'kit selections reach the server before START (3 s real)' })
            }
        }
        $steps.Add($step)
        if ($step.do -eq 'mark' -and $step.name -eq 'manual_combat_ready') {
            for ($k = 1; $k -le $Instances; $k++) {
                if ($CombatAi) { $steps.Add([pscustomobject]@{ do = 'client_cmd'; inst = $k; cmd = 'parity'; arg = 'ai auto' }) }
                if ($CombatProbe) { $steps.Add([pscustomobject]@{ do = 'client_cmd'; inst = $k; cmd = 'combat_probe'; arg = 'on' }) }
            }
        }
    }
    $sc.steps = $steps.ToArray()
}
if ($LabSession) {
    if ($Scenario -ne 'combat_manual' -or $LabOwnerPid -le 0) { throw 'Lab requires combat_manual and its owner PID.' }
    $LabSession = [IO.Path]::GetFullPath($LabSession)
    New-Item -ItemType Directory -Force $LabSession | Out-Null
    foreach ($step in $sc.steps) {
        if ($step.do -eq 'hold' -and $step.s -eq 1800) { $step.do = 'lab_session' }
    }
}
if ($eff -eq "rcon" -and ($sc.requires -contains "debug_verbs") -and -not $caps.debug_verbs -and -not $DryRun) {
    Say "scenario $($sc.name) needs RCON debug verbs (hsmp-server --debug-verbs)" Red; exit 2
}
if (@($sc.unsupported_in_mode).Count -gt 0) {
    Say "scenario $Scenario cannot run in $eff mode (needs: $(@($sc.unsupported_in_mode) -join ', '))" Red
    if (-not $DryRun) { exit 2 }
}
$topology = $sc.topology
$profiles = @{}
$nsSpec = if ($Netsim) { $Netsim } else { $sc.netsim }
$nsList = @($nsSpec -split ",")
for ($i = 1; $i -le $Instances; $i++) {
    $p = if ($nsList.Count -ge $i) { $nsList[$i - 1].Trim() } else { $nsList[-1].Trim() }
    if ($topology -eq "listen" -and $i -eq 1) { $p = "none" }   # the listen host talks to its own server
    $profiles["$i"] = $p
}

# --- run dir --------------------------------------------------------------------------
$runId = Hsmp-NewRunId ($sc.name + $(if ($DryRun) { "-dryrun" } else { "" }))
$Run = Join-Path $RunRoot $runId
New-Item -ItemType Directory -Force $Run | Out-Null
$Harness = Join-Path $Run "harness.jsonl"
$RconPw = [Guid]::NewGuid().ToString("N").Substring(0, 16)
# per-run state dir NAMES (relative to Win64, as HSMP_STATE_DIR); never another run's dir
$stateLeaf = @{}
$stateDirs = [ordered]@{}
for ($i = 1; $i -le $Instances; $i++) { $stateLeaf["$i"] = "hsmp_state_${runId}_$i"; $stateDirs["$i"] = Join-Path $Win64 $stateLeaf["$i"] }
# each sidecar's tap goes straight into the evidence dir (a log, not IPC)
$Ipc = "shm"
$IpcTaps = [ordered]@{}
for ($i = 1; $i -le $Instances; $i++) { $IpcTaps["$i"] = Join-Path (Join-Path $Run "inst$i") "ipc_tap.jsonl" }
if (-not $DryRun) {
    $PidFile = Join-Path $Run "mp_test.pids.json"
    Save-PidFile
    $script:RunEntry = Hsmp-RegisterRun ([ordered]@{
        id = $runId; kind = "mp_test"; real_game = (-not $FakeGame); detached = $false; run_dir = $Run; pidfile = $PidFile
        state_dirs = @($stateDirs.Values); ports = @($ServerPort, $RconPort, $MasterPort) + @(1..$Instances | ForEach-Object { Netsim-Port $_ })
        game = $GamePath; scenario = $sc.name })
}
# the deployed files must still be the ones the deploy stamp hashed, and the binaries
# this run executes (bin_dir) must be the stamped ones (DoD-13 judges deploy_check)
function Deploy-Check($dep) {
    $chk = [ordered]@{ checked = 0; mismatched = @(); missing = @(); bin_dir = $BinDir; bins_used = [ordered]@{} }
    if ($dep -and $dep.hashes) {
        foreach ($p in $dep.hashes.PSObject.Properties) {
            $f = Join-Path $Win64 ($p.Name -replace '/', '\')
            if (-not (Test-Path -LiteralPath $f)) { $chk.missing += $p.Name; continue }
            $chk.checked++
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $f).Hash -ne ([string]$p.Value).ToUpperInvariant()) { $chk.mismatched += $p.Name }
        }
    }
    foreach ($exe in @($ServerExe, $SidecarExe, $MasterExe)) {
        if (Test-Path -LiteralPath $exe) { $chk.bins_used[(Split-Path -Leaf $exe)] = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash }
    }
    return $chk
}
$deployStamp = $(if (Test-Path (Join-Path $Win64 "hsmp_deploy.json")) { Get-Content (Join-Path $Win64 "hsmp_deploy.json") -Raw | ConvertFrom-Json } else { $null })
$runCfg = [ordered]@{
    run_id = $runId; scenario = $sc.name; requested_scenario = $Scenario; mode = $eff
    stopgap = [bool]$sc.stopgap; instances = $Instances; arenas = @($sc.arenas); rounds = $sc.rounds
    topology = $topology; netsim = $profiles; state_dirs = $stateDirs; caps = $caps
    game = $GamePath; bin_dir = $BinDir; fake_game = [bool]$FakeGame
    ipc = $Ipc; ipc_taps = $IpcTaps
    ports =[ordered]@{ server = $ServerPort; rcon = $RconPort; master = $MasterPort; netsim = $NetsimPorts }
    started_wall_ms = (NowMs); harness_pid = $HarnessPid
    crashes_dir = (Join-Path $env:LOCALAPPDATA "HalfSwordUE5\Saved\Crashes")
    deploy = $deployStamp
    deploy_check = $(if ($DryRun) { $null } else { Deploy-Check $deployStamp })
    gate = $GateBuild
}
Write-Utf8 (Join-Path $Run "run.json") (ConvertTo-Json -InputObject $runCfg -Depth 8)
if (-not $runCfg.deploy -and -not $FakeGame) {
    Say "no hsmp_deploy.json next to the game: DoD-13 cannot tie this run to a G0-checked build (run build-and-deploy.ps1)" Yellow
}
if ($runCfg.deploy_check -and (@($runCfg.deploy_check.mismatched).Count -or @($runCfg.deploy_check.missing).Count)) {
    Say "deployed files differ from the deploy stamp ($(@($runCfg.deploy_check.mismatched).Count) changed, $(@($runCfg.deploy_check.missing).Count) missing): DoD-13 will FAIL" Red
}
# A -Netsim override weaker than the scenario's own profile cannot certify the scenario (gate rule NETSIM)
$order = @("none", "lan", "good", "typical", "wifi", "intl", "bad", "far", "awful")
$scList = @("$($sc.netsim)" -split ",")
for ($i = 1; $i -le $Instances; $i++) {
    if ($topology -eq "listen" -and $i -eq 1) { continue }
    $want = if ($scList.Count -ge $i) { $scList[$i - 1].Trim() } else { $scList[-1].Trim() }
    if ($order.IndexOf($profiles["$i"]) -lt $order.IndexOf($want)) {
        Say "netsim[$i] = $($profiles["$i"]) is weaker than $($sc.name)'s $want profile: the verdict will be INCOMPLETE (NETSIM)" Yellow
    }
}
function HEvent([string]$ev, $fields) {
    $o = [ordered]@{ ev = $ev; wall_ms = (NowMs) }
    if ($fields) { foreach ($k in $fields.Keys) { $o[$k] = $fields[$k] } }
    Append-Utf8 $Harness ((ToJson $o) + "`n")
}

# --- per-instance environment ------------------------------------------------------------
function Inst-Env([int]$i) {
    $e = [ordered]@{
        HSMP_INST = "$i"; HSMP_STATE_DIR = $stateLeaf["$i"]; HSMP_DEV = "1"; HSMP_LOG_ECHO = "1"
        HSMP_SIDECAR_EXE = $SidecarExe; HSMP_QUERY_EXE = (Join-Path $BinDir "hsmp-query.exe")
        HSMP_MASTER_URL = "http://127.0.0.1:$MasterPort"; HSMP_AUTOTEST_READY = "1"
        # Two instances share one GPU: with the user's VSync on, a frame that misses
        # 16.7 ms drops to exactly 30 fps on the heavier arenas, which a player on
        # their own PC would not see (HSMPMatch director.lua D.TEST_CVAR_OK).
        HSMP_TEST_CVARS = $TestCvars
        # Test runs never touch the real router (no UPnP / PCP / NAT-PMP mapping, no public STUN).
        HSMP_PORT_MAP = "off"; HSMP_STUN_SERVERS = "off"
    }
    $e.HSMP_IPC = $Ipc
    if ($IpcTaps.Contains("$i")) {
        $e.HSMP_IPC_TAP = $IpcTaps["$i"]
        if (-not $DryRun) { New-Item -ItemType Directory -Force (Split-Path -Parent $IpcTaps["$i"]) | Out-Null }
    }
    $direct = "127.0.0.1:$ServerPort"
    $addr = if ($profiles["$i"] -ne "none") { "127.0.0.1:$(Netsim-Port $i)" } else { $direct }
    if ($profiles["$i"] -ne "none") { $e.HSMP_NETSIM_ADDR = $addr }
    if ($topology -eq "listen" -and $i -eq 1) {
        $e.HSMP_AUTOTEST = "host"
        $e.HSMP_SERVER_EXE = (Join-Path $Run "hsmp-server-wrap.cmd")
    } else {
        $e.HSMP_AUTOTEST = if ($topology -eq "dedicated" -and $i -eq 1) { "host" } else { "join" }
        $e.HSMP_AUTOTEST_ADDR = $addr
        if ($topology -eq "dedicated") { $e.HSMP_AUTOTEST_EXTERNAL = "1" }
    }
    $se = $sc.env.PSObject.Properties | Where-Object { $_.Name -eq "$i" }
    if ($se) { foreach ($p in $se.Value.PSObject.Properties) { $e[$p.Name] = $p.Value } }
    return $e
}

function Server-Args([int]$gen) {
    $a = @("--bind", "127.0.0.1:$ServerPort", "--max-peers", "8", "--bans-file", (Join-Path $Run "bans.txt"),
           "--rcon-bind", "127.0.0.1:$RconPort", "--rcon-password", $RconPw)
    if ($caps.events) { $a += @("--events", (Join-Path $Run "server.jsonl")) }
    if ($caps.debug_verbs) { $a += "--debug-verbs" }
    # security: no first-joiner admin; seat 1 (the harness's "host" on a dedicated server)
    # is admin by its player key, which its sidecar writes to <state>/.player_key
    # A scenario may run the server with NO admin at all (env.server.HSMP_TEST_NO_ADMINS = "1"):
    # the no-admin auto start from READY.
    $noAdmins = $false
    $sv = $sc.env.PSObject.Properties | Where-Object { $_.Name -eq "server" }
    if ($sv -and "$($sv.Value.HSMP_TEST_NO_ADMINS)" -eq "1") { $noAdmins = $true }
    if ($caps.admin_keys -and -not $noAdmins) { $a += @("--admins-file", (Join-Path $stateDirs["1"] ".player_key")) }
    # extra hsmp-server flags for manual sessions (e.g. --mods-dir <dir> --mods-timeout-s 30)
    if ($ServerArgs.Count -gt 0) { $a += $ServerArgs }
    return $a
}

# --- plan / dry run ---------------------------------------------------------------------
$plan = New-Object System.Collections.Generic.List[string]
$plan.Add("scenario $($sc.name) ($($sc.doc)) mode=$eff topology=$topology DoD=$(@($sc.dod) -join ',')")
$plan.Add("arenas $(@($sc.arenas) -join ',') rounds $($sc.rounds); netsim $(($profiles.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ' ')")
if ($topology -eq "dedicated") { $plan.Add("server: $ServerExe $((Server-Args 1) -join ' ')") }
else { $plan.Add("listen host: instance 1 launches hsmp-server through $Run\hsmp-server-wrap.cmd (adds RCON/--events, logs to server.log)") }
for ($i = 1; $i -le $Instances; $i++) {
    if ($profiles["$i"] -ne "none") { $plan.Add("netsim[$i]: 127.0.0.1:$(Netsim-Port $i) -> 127.0.0.1:$ServerPort --profile $($profiles["$i"])") }
    $ie = Inst-Env $i
    $plan.Add("game[$i] env: $(($ie.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' ')")
}
$k = 0
foreach ($s in $sc.steps) { $k++; $plan.Add(("  {0,4} {1}" -f $k, (ToJson $s))) }
Write-Utf8 (Join-Path $Run "plan.txt") (($plan -join "`r`n") + "`r`n")
if ($DryRun) {
    $plan | Select-Object -First 12 | ForEach-Object { Write-Host "  $_" }
    Say "dry run: $($sc.steps.Count) steps; full plan in $Run\plan.txt" Green
    exit 0
}

$script:Completed = $false
$script:quitDone = $false
$script:Collected = $false
$script:StepEnded = @()
# copy the evidence out of this run's state dirs into <run>\inst<i>: the logs and config only
# (no IPC files exist; the sidecar's view is its tap, ipc_tap.jsonl), plus
# the state dir's file listing for gate rule STATE-1 (state_dir_listing.txt, one name per line)
function Collect-State {
    foreach ($i in $stateDirs.Keys) {
        $dst = Join-Path $Run "inst$i"
        New-Item -ItemType Directory -Force $dst | Out-Null
        $files = @(Get-ChildItem -LiteralPath $stateDirs[$i] -Force -File -ErrorAction SilentlyContinue)
        $files | Where-Object { $_.Name -like "hsmp_events*.jsonl" -or $_.Name -in @(".settings.json", ".career_guard.jsonl", ".sidecar_panic.log", ".parity_results.txt", ".world_scan.txt") } |
            ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $dst -Force }
        if (Test-Path -LiteralPath $stateDirs[$i]) {
            $names = @($files | ForEach-Object { $_.Name } | Sort-Object)
            Write-Utf8 (Join-Path $dst "state_dir_listing.txt") ((($names -join "`n") + "`n"))
        }
    }
    $script:Collected = $true
}
# this run's own state dirs go once its games are gone (the evidence is in <run>\inst<i>)
function Remove-OwnStateDirs {
    $liveGames = @($script:Tracked | Where-Object { $_.role -match '^game\d+$' } | Where-Object { Same-Process $_ })
    if ($liveGames.Count) { Say "  state dirs kept: $($liveGames.Count) game(s) of this run still running" Yellow; return }
    foreach ($d in $stateDirs.Values) {
        if ((Hsmp-OwnStateDir $d $runId) -and (Test-Path -LiteralPath $d)) { Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue }
    }
}
try {
# --- 2. state dirs (this run's own, new names: nothing of another run is ever wiped) --------------
foreach ($i in $stateDirs.Keys) {
    $d = $stateDirs[$i]
    if (Test-Path $d) { Say "state dir $d already exists (run id collision): refusing to reuse it" Red; exit 2 }
    New-Item -ItemType Directory -Force $d | Out-Null
    $srv = if ($profiles[$i] -ne "none") { "127.0.0.1:$(Netsim-Port $i)" } else { "127.0.0.1:$ServerPort" }
    $st = [ordered]@{}
    if ($ExtraSettings) { ($ExtraSettings | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $st[$_.Name] = $_.Value } }
    $st["nick"] = "HSMP$i"; $st["server"] = $srv
    Write-Utf8 (Join-Path $d ".settings.json") ((ToJson $st) + "`n")
}

# --- 3. baseline: saves + crash dirs -----------------------------------------------------------
$Saved = Join-Path $env:LOCALAPPDATA "HalfSwordUE5\Saved"
function Hash-Saves {
    $h = [ordered]@{}
    $dir = Join-Path $Saved "SaveGames"
    if (Test-Path $dir) {
        foreach ($f in (Get-ChildItem -File $dir -Filter "*.sav" | Sort-Object Name)) {
            $h[$f.Name] = (Get-FileHash -Algorithm SHA256 -LiteralPath $f.FullName).Hash
        }
    }
    return $h
}
function Save-Files {
    # DoD-8: size + mtime + sha256 per *.sav. A career file written with the same bytes, or
    # restored afterwards by the sidecar's career guard, keeps its hash; its mtime/size do not.
    $h = [ordered]@{}
    $dir = Join-Path $Saved "SaveGames"
    if (Test-Path $dir) {
        foreach ($f in (Get-ChildItem -File $dir -Filter "*.sav" | Sort-Object Name)) {
            $h[$f.Name] = [ordered]@{
                size = [int64]$f.Length
                mtime_ms = ([DateTimeOffset]$f.LastWriteTimeUtc).ToUnixTimeMilliseconds()
                sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $f.FullName).Hash
            }
        }
    }
    return $h
}
function Crash-Dirs {
    $dir = Join-Path $Saved "Crashes"
    if (Test-Path $dir) { return @(Get-ChildItem -Directory $dir | ForEach-Object { $_.Name }) }
    return @()
}
$baseline = [ordered]@{ save_hashes = (Hash-Saves); save_files = (Save-Files); crash_dirs = (Crash-Dirs); wall_ms = (NowMs) }
Write-Utf8 (Join-Path $Run "baseline.json") (ConvertTo-Json -InputObject $baseline -Depth 4)
Say "baseline: $($baseline.save_hashes.Count) career saves hashed, $($baseline.crash_dirs.Count) crash dirs"

# G0 quick (recorded for DoD-13; never blocks the run)
$g0 = Invoke-Gate @("g0", "--quick", "--no-stamp", "--json", (Join-Path $Run "g0.json"))
Say "G0 quick: $(if ($g0.code -eq 0) { 'pass' } else { 'FAIL (see g0.json)' })" $(if ($g0.code -eq 0) { "Green" } else { "Yellow" })

# --- 4/5. processes -----------------------------------------------------------------------------
$script:Roles = @{}
function Start-Tracked([string]$role, [string]$exe, [string[]]$argv, [string]$out, [string]$wd = $Run, [hashtable]$env = $null) {
    $saved = @{}
    if ($env) { foreach ($k in $env.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, [string]$env[$k]) } }
    try {
        $sp = @{ FilePath = $exe; WorkingDirectory = $wd; PassThru = $true }
        if ($argv -and $argv.Count) { $sp.ArgumentList = ($argv | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) }
        if ($out) { $sp.RedirectStandardOutput = $out; $sp.RedirectStandardError = ($out -replace '\.(log|txt)$', '.stderr.txt'); $sp.WindowStyle = "Hidden" }
        $p = Start-Process @sp
    } finally {
        if ($env) { foreach ($k in $env.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
    }
    Track-Process $p $role
    $script:Roles[$role] = $p
    HEvent "proc_start" @{ role = $role; pid = $p.Id; exe = $exe }
    return $p
}
function Is-Our-Sidecar([string]$cl, [string]$i) {
    # this run's own (unique) state dir AND one of our ports in --server
    if ($cl -notmatch ('--state-dir\s+"?' + [regex]::Escape($stateLeaf["$i"]) + '"?(\s|$)')) { return $false }
    foreach ($port in @($ServerPort, (Netsim-Port $i))) {
        if ($cl -match ('--server\s+"?[^\s"]+:' + $port + '\b')) { return $true }
    }
    return $false
}
function Discover-Children {
    # sidecars / listen servers are launched by the game (the native module's spawn): find
    # them by their command line (state dir / our ports), record them, never match by name alone
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='hsmp-sidecar.exe' OR Name='hsmp-server.exe'" -ErrorAction SilentlyContinue)
    foreach ($w in $procs) {
        $cl = [string]$w.CommandLine
        $role = $null
        foreach ($i in $stateDirs.Keys) { if (Is-Our-Sidecar $cl $i) { $role = "sidecar$i" } }
        if (-not $role -and $cl -match [regex]::Escape("--rcon-password $RconPw")) { $role = "server" }
        if (-not $role) { continue }
        if (@($script:Tracked | Where-Object { $_.pid -eq [int]$w.ProcessId }).Count) { continue }
        $p = Get-Process -Id ([int]$w.ProcessId) -ErrorAction SilentlyContinue
        if ($p) { Track-Process $p $role "game"; $script:Roles[$role] = $p; HEvent "proc_found" @{ role = $role; pid = $p.Id; launched_by = "game" } }
    }
}

if (Test-Path $MasterExe) {
    [void](Start-Tracked "master" $MasterExe @("--bind", "127.0.0.1:$MasterPort") (Join-Path $Run "master.log"))
}
$serverEnv = @{ NO_COLOR = "1"; HSMP_MASTER_URL = "http://127.0.0.1:$MasterPort" }
if ($topology -eq "dedicated") {
    if (-not $caps.server_found) { Say "hsmp-server not found in $BinDir" Red; exit 2 }
    [void](Start-Tracked "server" $ServerExe (Server-Args 1) (Join-Path $Run "server.log") $Run $serverEnv)
} else {
    $sa = @("--rcon-bind", "127.0.0.1:$RconPort", "--rcon-password", $RconPw, "--bans-file", "`"$(Join-Path $Run 'bans.txt')`"")
    if ($caps.events) { $sa += @("--events", "`"$(Join-Path $Run 'server.jsonl')`"") }
    if ($caps.debug_verbs) { $sa += "--debug-verbs" }
    # The listen host owns its server by its player key: HSMPMenu already
    # passes --owner-key-file (a second copy makes hsmp-server refuse to start).
    $wrap = @(
        "@echo off",
        "rem mp_test.ps1 listen-host wrapper (HSMP_SERVER_EXE of instance 1): adds RCON/--events, logs to the run dir",
        "set NO_COLOR=1",
        "`"$ServerExe`" %* $($sa -join ' ') >> `"$(Join-Path $Run 'server.log')`" 2>&1"
    ) -join "`r`n"
    Write-Utf8 (Join-Path $Run "hsmp-server-wrap.cmd") ($wrap + "`r`n")
}
for ($i = 1; $i -le $Instances; $i++) {
    $p = $profiles["$i"]
    if ($p -eq "none") { continue }
    $nsArgs = @("netsim", "--listen", "127.0.0.1:$(Netsim-Port $i)",
                "--upstream", "127.0.0.1:$ServerPort", "--profile", $p,
                "--control-file", (Join-Path $Run "netsim$i.ctl"), "--parent-pid", "$HarnessPid",
                "--events", (Join-Path $Run "netsim$i.jsonl"))
    [void](Start-Tracked "netsim$i" $Tools $nsArgs (Join-Path $Run "netsim$i.log"))
}
[void](Start-Tracked "observer" $Gate (@("observe", "--run", $Run, "--parent-pid", "$HarnessPid")) (Join-Path $Run "observer.log"))
if ($sc.soak) {
    # soak: memory sampler over every process this harness tracks (re-reads the pidfile each tick)
    $memEvery = if ($sc.soak.mem_sample_s) { [double]$sc.soak.mem_sample_s } else { 30 }
    [void](Start-Tracked "memwatch" $Gate (@("memwatch", "--run", $Run, "--pids", $PidFile, "--interval-s", "$memEvery", "--parent-pid", "$HarnessPid")) (Join-Path $Run "memwatch.log"))
    Say "soak: memwatch every $memEvery s -> mem.jsonl; warm-up $($sc.soak.mem_warmup_min) min, slope <= $($sc.soak.mem_slope_max_mb_per_h) MB/h, growth <= $($sc.soak.mem_growth_max_mb) MB"
}

function Wait-Step($step, [int64]$since) {
    $tmp = Join-Path $Run "step.json"
    Write-Utf8 $tmp (ToJson $step)
    $r = Invoke-Gate @("wait", "--run", $Run, "--step-file", $tmp, "--since-ms", "$since")
    return $r
}

# Lay the game windows out side by side on the primary screen (each renders fully visible, no
# overlap). Window placement only (SetWindowPos, no activation, no input, no ini change).
if (-not ("HsmpWin2" -as [type])) {
    # The game process owns two top-level windows: the UE4SS console (created first, so it is
    # Process.MainWindowHandle) and the game's own "UnrealWindow". Only the latter is placed.
    Add-Type -TypeDefinition @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class HsmpWin2 {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    public static IntPtr GameWindow(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid || !IsWindowVisible(h)) return true;
            var sb = new StringBuilder(64); GetClassName(h, sb, 64);
            if (sb.ToString() == "UnrealWindow") { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@
}
function Place-GameWindow([int]$procId, [int]$i, [int]$n) {
    if ($n -lt 2) { return }
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
        $wa = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
        $w = [int][Math]::Floor($wa.Width / $n)
        $h = [int][Math]::Min($wa.Height, [Math]::Floor($w * 9 / 16) + 32)
        $deadline = (Get-Date).AddSeconds(30)
        do {
            $pr = Get-Process -Id $procId -ErrorAction SilentlyContinue
            if (-not $pr) { return }
            $hw = [HsmpWin2]::GameWindow([uint32]$procId)
            if ($hw -ne [IntPtr]::Zero) {
                # SWP_NOZORDER 0x4 | SWP_NOACTIVATE 0x10
                [void][HsmpWin2]::SetWindowPos($hw, [IntPtr]::Zero, $wa.X + ($i - 1) * $w, $wa.Y, $w, $h, 0x14)
                Say "game$i window placed at x=$($wa.X + ($i - 1) * $w) ($w x $h)"
                return
            }
            Start-Sleep -Milliseconds 250
        } while ((Get-Date) -lt $deadline)
        Say "game$i has no main window yet; left where it is" Yellow
    } catch { Say "window placement skipped: $_" Yellow }
}
# A listen host reaches its lobby before the others are even launched: the
# first scenario wait counts events from here, not from after the launches.
$launchSince = NowMs
for ($i = 1; $i -le $Instances; $i++) {
    $ie = Inst-Env $i
    # this instance's own events count from its launch: placing its window waits for the
    # engine's window, which appears after the game already wrote `_open`
    $instSince = (NowMs) - 2000
    if ($FakeGame) {
        [void](Start-Tracked "game$i" $Gate @("fake-game") (Join-Path $Run "fakegame$i.log") $Win64 $ie)
    } else {
        if (-not (Test-Path $GameExe)) { Say "game exe not found: $GameExe" Red; exit 2 }
        [void](Start-Tracked "game$i" $GameExe $GameArgs $null $Win64 $ie)
    }
    $gt = @($script:Tracked | Where-Object { $_.role -eq "game$i" }) | Select-Object -Last 1
    if ($gt -and -not $FakeGame) { Place-GameWindow ([int]$gt.pid) $i $Instances }
    # serialise start-up on the instance's own first event instead of a fixed sleep
    $w = Wait-Step ([ordered]@{ do = "wait"; ev = "_open"; who = "$i"; timeout_s = $GameStartTimeout }) $instSince
    if ($w.code -ne 0) { Say "instance $i wrote no hsmp_log _open event in ${GameStartTimeout}s (mods not instrumented?) - continuing" Yellow }
    if ($topology -eq "listen" -and $i -eq 1 -and $Instances -gt 1) {
        # the listen host connects first (it owns its server by its player key)
        $w = Wait-Step ([ordered]@{ do = "wait"; ev = "lobby_ready"; who = "1"; timeout_s = 150 }) ((NowMs) - 2000)
        if ($w.code -ne 0) { Say "listen host never reached its lobby: $($w.out)" Red; HEvent "step_failed" @{ step = "host lobby_ready"; detail = $w.out } }
    }
}

# The engine re-applies its own window size/position while it finishes starting up (the
# early placement above is undone and the windows end up stacked): place them again now
# that every game is open, and once more when they first reach the lobby.
function Place-AllGameWindows {
    if ($FakeGame) { return }
    foreach ($e in @($script:Tracked | Where-Object { $_.role -match '^game\d+$' })) {
        if (Same-Process $e) { Place-GameWindow ([int]$e.pid) ([int]($e.role -replace '^game', '')) $Instances }
    }
}
Place-AllGameWindows
$script:WindowsReplaced = $false

# --- 6. scenario steps -------------------------------------------------------------------------
# From the first launch in every topology: a game can reach its lobby while the later ones
# are still starting (window placement waits for each engine window), and this run's state
# dirs are fresh, so no older event can match.
$since = $launchSince
$failed = $null
$cmdSeq = 0
$quitDone = $false
# a read-only `ipc-dump --json` of every live game's segment, before the games quit
function Snapshot-Ipc {
    foreach ($e in @($script:Tracked | Where-Object { $_.role -match '^game\d+$' })) {
        if (-not (Same-Process $e)) { continue }
        $i = $e.role -replace '^game', ''
        $dst = Join-Path (Join-Path $Run "inst$i") "ipc_dump.json"
        New-Item -ItemType Directory -Force (Split-Path -Parent $dst) | Out-Null
        $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
        try {
            $out = & $Tools ipc-dump --pid "$($e.pid)" --json 2>&1 | ForEach-Object { "$_" }
            $code = $LASTEXITCODE
        } finally { $ErrorActionPreference = $prev }
        Write-Utf8 $dst (($out -join "`n") + "`n")
        HEvent "ipc_dump" @{ inst = "$i"; ok = ($code -eq 0); file = "inst$i\ipc_dump.json" }
    }
}
# The autotest command channel: `hsmp-tools ipc-ctl --pid <game> autotest <cmd>
# [arg]` pushes a dev_cmd record into the instance's DevCtl ring (no state-dir file).
function Send-Autotest([string]$inst, [string]$cmd, $arg) {
    $e = @($script:Tracked | Where-Object { $_.role -eq "game$inst" }) | Select-Object -Last 1
    if (-not $e -or -not (Same-Process $e)) { return @{ ok = $false; out = "game$inst is not running"; id = $null } }
    $script:cmdSeq++
    $argv = @("ipc-ctl", "--pid", "$($e.pid)", "--id", "$($script:cmdSeq)", "autotest", $cmd)
    if ($null -ne $arg -and "$arg" -ne "") { $argv += "$arg" }
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try {
        $out = & $Tools @argv 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $prev }
    return @{ ok = ($code -eq 0); out = (($out -join " ").Trim()); id = $script:cmdSeq }
}
function Quit-All {
    $alive = [ordered]@{}
    foreach ($e in $script:Tracked) {
        if ($e.role -match '^(game\d+|server|master|netsim\d+)$') { $alive[$e.role] = [bool](Same-Process $e) }
    }
    HEvent "alive_at_end" @{ alive = $alive }
    Snapshot-Ipc
    # quit the way a user does first. 1) the menu path: the autotest command channel's
    # `quit` (HSMPMenu quit_desktop: session teardown, then console quit); 2) WM_CLOSE;
    # 3) a kill by recorded PID, recorded as game_force_killed (a DoD-12 failure).
    $games = @($script:Tracked | Where-Object { $_.role -match '^game\d+$' })
    $pending = @()
    foreach ($e in $games) {
        $p = Same-Process $e
        if (-not $p) { $script:QuitPath[$e.role] = "dead_before_quit"; continue }
        $i = $e.role -replace '^game', ''
        $sent = Send-Autotest $i "quit" $null
        HEvent "client_cmd" @{ inst = "$i"; cmd = "quit"; why = "quit_all (menu path)"; ok = $sent.ok; detail = $sent.out }
        $pending += , @($e, $p)
    }
    $menuGrace = if ($FakeGame) { 8000 } else { 30000 }
    $deadline = (Get-Date).AddMilliseconds($menuGrace)
    foreach ($ep in $pending) {
        $left = [int][Math]::Max(0, ($deadline - (Get-Date)).TotalMilliseconds)
        if ($ep[1].WaitForExit($left)) { $script:QuitPath[$ep[0].role] = "menu_quit" }
    }
    foreach ($ep in $pending) {
        $e = $ep[0]; $p = $ep[1]
        if ($script:QuitPath[$e.role]) { continue }
        try { [void]$p.CloseMainWindow() } catch { }
        $grace = if ($FakeGame) { 0 } else { 15000 }      # a console fake has no window to close
        if ($p.WaitForExit($grace)) { $script:QuitPath[$e.role] = "wm_close"; HEvent "game_closed_by_wm_close" @{ role = $e.role; pid = $p.Id }; continue }
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        $script:QuitPath[$e.role] = "force_killed"
        $script:ForceKilled += $e.role
        HEvent "game_force_killed" @{ role = $e.role; pid = $p.Id; why = "no exit within $([int]($menuGrace/1000)) s of the menu quit nor $([int]($grace/1000)) s of WM_CLOSE" }
        Say "  $($e.role) did not quit (menu quit, WM_CLOSE): killed by PID $($p.Id) - DoD-12 FAIL" Red
    }
    HEvent "quit_path" @{ paths = $script:QuitPath }
    $script:quitDone = $true
    return $alive
}
$script:QuitPath = [ordered]@{}
$script:ForceKilled = @()
$aliveAtEnd = $null
$stepNo = 0
foreach ($s in $sc.steps) {
    $stepNo++
    if ($failed -and $s.do -ne "quit_all") { continue }
    $sj = ToJson $s
    Say ("step {0}/{1}: {2}" -f $stepNo, $sc.steps.Count, $sj) DarkGray
    # a process this step ends ON PURPOSE (e.g. host_leave: the listen host's server) is
    # recorded, so the gate can tell it from a crash (CRASH rule, DoD-12 dead_before_quit)
    foreach ($xr in @($s.expect_exit)) {
        if ($xr) { HEvent "expected_exit" @{ role = "$xr"; step = $stepNo; why = "$($s.do) $($s.cmd)".Trim() }; $script:StepEnded += "$xr" }
    }
    switch ($s.do) {
        "mark" {
            $since = NowMs
            $f = @{ name = $s.name }
            foreach ($p in $s.PSObject.Properties) { if ($p.Name -notin @("do", "name")) { $f[$p.Name] = $p.Value } }
            HEvent "mark" $f
        }
        { $_ -in @("wait", "expect_none") } {
            $w = Wait-Step $s $since
            if ($w.code -ne 0) { $failed = $s; HEvent "step_failed" @{ step = $sj; detail = $w.out } ; Say "  FAILED: $($w.out)" Red }
            Discover-Children
            if ($s.ev -eq "lobby_ready" -and -not $script:WindowsReplaced) { $script:WindowsReplaced = $true; Place-AllGameWindows }
        }
        "rcon" {
            $rr = Invoke-Gate @("rcon", "--addr", "127.0.0.1:$RconPort", "--password", $RconPw, $s.cmd)
            $ok = ($rr.code -eq 0)
            if ($s.expect -eq "ERR") { $ok = ($rr.out -match "^ERR") }
            $f = @{ cmd = $s.cmd; ok = $ok; reply = $rr.out }
            if ($s.pick) { $f.pick = $s.pick }
            HEvent "rcon" $f
            if (-not $ok -and -not $s.optional) { $failed = $s; HEvent "step_failed" @{ step = $sj; detail = $rr.out }; Say "  FAILED: $($rr.out)" Red }
        }
        "client_cmd" {
            # Autotest command channel (contract in docs/development/testing.md; consumer: HSMPMenu under
            # HSMP_AUTOTEST): a dev_cmd AUTOTEST record pushed into the game's DevCtl ring
            $sent = Send-Autotest "$($s.inst)" $s.cmd $s.arg
            $f = @{ inst = "$($s.inst)"; cmd = $s.cmd; arg = $s.arg; ok = $sent.ok; id = $sent.id }
            if (-not $sent.ok) { $f.detail = $sent.out; Say "  ipc-ctl failed for instance $($s.inst): $($sent.out)" Yellow }
            if ($s.pick) { $f.pick = $s.pick }
            HEvent "client_cmd" $f
        }
        "netsim" {
            Write-Utf8 (Join-Path $Run "netsim$($s.inst).ctl") $s.mode
            HEvent "netsim" @{ inst = "$($s.inst)"; mode = $s.mode }
        }
        "hold" {
            # an impairment / observation DURATION, not a synchronisation point
            $secs = [double]$s.s * $HoldScale
            HEvent "hold" @{ s = $secs; why = $s.why }
            Start-Sleep -Milliseconds ([int]($secs * 1000))
        }
        "lab_session" {
            $labOwner = Get-Process -Id $LabOwnerPid -ErrorAction Stop
            $labIdentity = Hsmp-ProcRecord $labOwner 'lab'
            Write-Utf8 (Join-Path $LabSession 'ready.json') (ConvertTo-Json -InputObject @{ run = $Run; owner = $labIdentity } -Depth 4)
            HEvent 'mark' @{ name = 'lab_ready'; arena = $CombatArena }
            Say "lab ready: $LabSession (experiments use RCON and DevCtl)" Cyan
            # Poll a condition, never a fixed startup/round sleep. If the owner
            # exits, normal harness teardown still restores its own save guard.
            while (-not (Test-Path (Join-Path $LabSession 'stop')) -and (Same-Process $labIdentity)) {
                Discover-Children
                $dead = @($script:Tracked | Where-Object { $_.role -match '^game\d+$' } | Where-Object { -not (Same-Process $_) })
                if ($dead.Count) { throw 'A lab game exited before session teardown.' }
                Start-Sleep -Milliseconds 250
            }
        }
        { $_ -in @("kill", "restart") } {
            $e = @($script:Tracked | Where-Object { $_.role -eq $s.role }) | Select-Object -Last 1
            if ($e) { Stop-Tracked $e; HEvent "proc_killed" @{ role = $s.role; pid = $e.pid }; $script:StepEnded += "$($s.role)" }
            if ($s.do -eq "restart" -and $s.role -eq "server") {
                $gen = @($script:Tracked | Where-Object { $_.role -eq "server" }).Count + 1
                [void](Start-Tracked "server" $ServerExe (Server-Args $gen) (Join-Path $Run "server.$gen.log") $Run $serverEnv)
            }
        }
        "quit_all" { Discover-Children; $aliveAtEnd = Quit-All; $quitDone = $true }
        default { $failed = $s; HEvent "step_failed" @{ step = $sj; detail = "unknown step kind" } }
    }
}
if (-not $quitDone) { Discover-Children; $aliveAtEnd = Quit-All }

# --- 7. collect + teardown ----------------------------------------------------------------------------
# Every game-launched process (sidecars, a listen host's server) gets 15 s after its game quit to
# notice (--parent-pid, the menu's teardown); one still alive then is an orphan (DoD-12).
$deadline = (Get-Date).AddSeconds(15)
do {
    $left = @($script:Tracked | Where-Object { $_.launched_by -eq "game" } | Where-Object { Same-Process $_ })
    if (-not $left.Count) { break }
    Start-Sleep -Milliseconds 500
} while ((Get-Date) -lt $deadline)
# stop the harness's own samplers (observer, memwatch) BEFORE the scan: stop file, then wait
# for their PIDs (a live memwatch would otherwise be listed as an orphan)
New-Item -ItemType File -Force (Join-Path $Run ".stop_observer") | Out-Null
New-Item -ItemType File -Force (Join-Path $Run ".stop_memwatch") | Out-Null
$deadline = (Get-Date).AddSeconds(5)
do {
    $left = @($script:Tracked | Where-Object { $_.role -in @("observer", "memwatch") } | Where-Object { Same-Process $_ })
    if (-not $left.Count) { break }
    Start-Sleep -Milliseconds 250
} while ((Get-Date) -lt $deadline)
foreach ($e in @($script:Tracked | Where-Object { $_.role -in @("observer", "memwatch") })) { Stop-Tracked $e }
$runStart = [DateTimeOffset]::FromUnixTimeMilliseconds($runCfg.started_wall_ms).LocalDateTime
$orphans = @()
foreach ($w in @(Get-CimInstance Win32_Process -Filter "Name LIKE 'hsmp-%'" -ErrorAction SilentlyContinue)) {
    # only the harness's own long-lived processes are exempt (stopped below); a game-launched
    # server or sidecar (launched_by=game) is judged like any other process of this run
    $own = @($script:Tracked | Where-Object { $_.pid -eq [int]$w.ProcessId -and $_.launched_by -eq "harness" -and $_.role -notmatch '^game\d+$' })
    if ($own.Count) { continue }
    $created = $null
    try { $created = $w.CreationDate } catch { }
    if ($created -and $created -lt $runStart) { continue }      # not from this run
    # only processes of THIS run count: tracked by us, or their command line names one of our
    # state dirs / our RCON password / our ports (other runs and the user run hsmp-* too)
    $cl = [string]$w.CommandLine
    $ours = @($script:Tracked | Where-Object { $_.pid -eq [int]$w.ProcessId }).Count -gt 0
    if (-not $ours) {
        foreach ($i in $stateDirs.Keys) { if (Is-Our-Sidecar $cl $i) { $ours = $true } }
        if ($cl -match [regex]::Escape($RconPw)) { $ours = $true }
    }
    if ($ours) {
        $rec = @($script:Tracked | Where-Object { $_.pid -eq [int]$w.ProcessId }) | Select-Object -First 1
        $orphans += [ordered]@{ pid = [int]$w.ProcessId; name = $w.Name; parent = [int]$w.ParentProcessId; cmd = [string]$w.CommandLine
                                role = $(if ($rec) { $rec.role } else { $null }); launched_by = $(if ($rec) { $rec.launched_by } else { "game" }) }
    }
}
foreach ($e in @($script:Tracked)) { if ($e.role -notlike "game*") { Stop-Tracked $e } }
foreach ($o in $orphans) { $e = $script:Tracked | Where-Object { $_.pid -eq $o.pid } | Select-Object -First 1; if ($e) { Stop-Tracked $e } }

Collect-State
$ue4ssLog = Join-Path $Win64 "ue4ss\UE4SS.log"
if (Test-Path $ue4ssLog) { Copy-Item $ue4ssLog (Join-Path $Run "UE4SS.log") -Force }

# a dump written while the game went down lands a few seconds after the last process exit;
# settle before the crash scan and the triage window end (the window ends now, not earlier)
if (-not $FakeGame) { Start-Sleep -Seconds 5 }
$after = Hash-Saves
$crashNow = Crash-Dirs
# a role with more than one process during the run restarted (a sidecar the game respawned,
# a listen server relaunched); roles a scenario step ended or restarted on purpose are excluded
$restarted = [ordered]@{}
foreach ($g in @($script:Tracked | Group-Object { $_.role })) {
    if ($g.Count -gt 1 -and $script:StepEnded -notcontains $g.Name) { $restarted[$g.Name] = $true }
}
$final = [ordered]@{
    save_hashes = $after
    save_files = (Save-Files)
    new_crashes = @($crashNow | Where-Object { $baseline.crash_dirs -notcontains $_ })
    alive_at_end = $aliveAtEnd
    orphans = $orphans
    quit_path = $script:QuitPath
    game_force_killed = @($script:ForceKilled)
    restarted = $restarted
    expected_exit = @($script:StepEnded | Select-Object -Unique)
    failed_step = $(if ($failed) { ToJson $failed } else { $null })
    wall_ms = (NowMs)
}
Write-Utf8 (Join-Path $Run "final.json") (ConvertTo-Json -InputObject $final -Depth 6)

# crash-triage over the run window (DoD-2, CRASH, SOAK-CRASH): classify every dump written during the run
$prevEap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
$tr = & $Tools crash-triage --dir $runCfg.crashes_dir --since "$($runCfg.started_wall_ms)" --until "$($final.wall_ms)" `
        --no-state-update --quiet --out (Join-Path $Run "crash_triage.json") 2>&1 | ForEach-Object { "$_" }
$trCode = $LASTEXITCODE
$ErrorActionPreference = $prevEap
switch ($trCode) {
    0 { Say "crash-triage: no new crash dumps" }
    1 { Say "crash-triage: NEW crash dump(s) during the run (crash_triage.json)" Red }
    default { Say "crash-triage failed (exit $trCode): $($tr -join ' ')" Yellow }
}
# keep a record of what this run started (forensics); <run>\mp_test.pids.json stays too
Write-Utf8 (Join-Path $Run "pids.json") (ConvertTo-Json -InputObject @($script:Tracked) -Depth 4)
$script:Completed = $true

# --- 8. assert ------------------------------------------------------------------------------------------
$res = Invoke-Gate @("assert", "--run", $Run)
Write-Host $res.out
Say "report: $(Join-Path $Run 'report.json')" Cyan
exit $res.code
} finally {
    # whatever ended the run (a failure, `exit`, Ctrl+C), tear down THIS run's processes
    # only (recorded PIDs), keep its evidence, drop its own state dirs and release the lock.
    if (-not $script:Completed) {
        Say "run aborted: stopping this run's own processes (recorded PIDs only)" Yellow
        try { if (-not $script:quitDone -and (Get-Command Quit-All -ErrorAction SilentlyContinue)) { Discover-Children; [void](Quit-All) } } catch { }
        foreach ($e in @($script:Tracked)) { Stop-Tracked $e }
        try { if (-not $script:Collected) { Collect-State } } catch { }
    }
    try { Remove-OwnStateDirs } catch { }
    Hsmp-UnregisterRun $script:RunEntry
    Hsmp-ReleaseGameLock $script:RunLock
}

#Requires -Version 5.1
<#
.SYNOPSIS
    Repeated native deathmatch placement checks in an existing hsmp-lab session.
.DESCRIPTION
    Starts no processes. Requires a real two-game lab session. DEBUG KILL is an
    explicit test stimulus, not evidence of combat damage. Backend subchecks are
    kept separate from A12-A14 (HUD, controls, kit and stand-in visuals need review).
    Leaves the session in the lobby. Does not certify release gates.
.EXAMPLE
    .\scripts\lab-modes-test.ps1 -Session test-results\lab\session -DeathsPerPlayer 5
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Session,
    [ValidateRange(1,20)][int]$DeathsPerPlayer = 5,
    [ValidateRange(1,30)][int]$RespawnDelay = 3,
    [ValidateRange(5,180)][int]$RespawnTimeout = 45,
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\hsmp_runs.ps1')
$Utf8 = New-Object Text.UTF8Encoding $false
$ready = Get-Content -LiteralPath (Join-Path $Session 'ready.json') -Raw | ConvertFrom-Json
if (-not (Hsmp-SameProcess $ready.owner)) { throw 'Lab owner identity is no longer live.' }
if (Test-Path -LiteralPath (Join-Path $Session 'stop')) { throw 'Lab is stopping.' }
$run = [string]$ready.run
$cfg = Get-Content -LiteralPath (Join-Path $run 'run.json') -Raw | ConvertFrom-Json
if ($cfg.fake_game) { throw 'Native modes checks require the real game, not FakeGame.' }
$records = @(Get-Content -LiteralPath (Join-Path $run 'mp_test.pids.json') -Raw | ConvertFrom-Json)
$games = @($records | Where-Object { $_.role -in @('game1','game2') } | Sort-Object role)
if ($games.Count -ne 2) { throw 'Expected two recorded games.' }
foreach ($g in $games) { if (-not (Hsmp-SameProcess $g)) { throw "Game identity changed: $($g.role)." } }
if (-not $Out) { $Out = Join-Path $run ('modes-' + [Guid]::NewGuid().ToString('N').Substring(0,8)) }
if (Test-Path -LiteralPath $Out) { throw 'Output directory exists; choose a fresh path.' }
New-Item -ItemType Directory -Path $Out | Out-Null
$plan = Get-Content -LiteralPath (Join-Path $run 'plan.txt') -Raw
$pwMatch = [regex]::Match($plan, '--rcon-password\s+(\S+)')
if (-not $pwMatch.Success) { throw 'Lab RCON credential is absent.' }
$password = $pwMatch.Groups[1].Value
$tools = Join-Path ([string]$cfg.bin_dir) 'hsmp-tools.exe'
if (-not (Test-Path -LiteralPath $tools)) { $tools = Join-Path (Split-Path -Parent $PSScriptRoot) 'target\release\hsmp-tools.exe' }
$checks = New-Object Collections.Generic.List[object]
$samples = New-Object Collections.Generic.List[object]
$lockPath = Join-Path $run 'lab-exp.lock'
$lock = [IO.File]::Open($lockPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
function Save-Json([string]$path, $value) { [IO.File]::WriteAllText($path, (ConvertTo-Json -InputObject $value -Depth 12), $Utf8) }
function Rcon([string]$command, [switch]$AllowError) {
    if (-not (Hsmp-SameProcess $ready.owner)) { throw 'Lab owner exited.' }
    $client = New-Object Net.Sockets.TcpClient
    try {
        $task = $client.ConnectAsync('127.0.0.1', [int]$cfg.ports.rcon)
        if (-not $task.Wait(5000)) { throw 'RCON connection timed out.' }
        $stream = $client.GetStream(); $stream.ReadTimeout = 5000; $stream.WriteTimeout = 5000
        $reader = New-Object IO.StreamReader($stream)
        $writer = New-Object IO.StreamWriter($stream); $writer.AutoFlush = $true
        $writer.WriteLine('AUTH ' + $password)
        if ($reader.ReadLine() -notmatch '^OK') { throw 'RCON authentication failed.' }
        $writer.WriteLine($command); $reply = $reader.ReadLine()
        [IO.File]::AppendAllText((Join-Path $Out 'rcon.jsonl'), ((@{ at=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); command=$command; reply=$reply } | ConvertTo-Json -Compress) + "`n"), $Utf8)
        if (-not $AllowError -and $reply -notmatch '^OK') { throw "RCON ${command}: $reply" }
        return $reply
    } finally { $client.Dispose() }
}
function Status { $r = Rcon 'STATUS'; return ($r.Substring(3) | ConvertFrom-Json) }
function Wait-Status([scriptblock]$predicate, [int]$seconds, [string]$label) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        foreach ($g in $games) { if (-not (Hsmp-SameProcess $g)) { throw "Game exited: $($g.role)." } }
        $s = Status
        if (& $predicate $s) { return $s }
        if ($timer.Elapsed.TotalSeconds -ge $seconds) { throw "Timed out waiting for $label; phase=$($s.phase)." }
        Start-Sleep -Milliseconds 200
    } while ($true)
}
function Dev([int]$instance, [string[]]$arguments) {
    & $tools ipc-ctl --pid ([string]$games[$instance].pid) @arguments | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "DevCtl failed for instance $($instance + 1)." }
}
function Check([string]$id, [bool]$ok, [string]$detail) {
    $checks.Add(@{ id=$id; verdict=$(if ($ok) {'PASS'} else {'FAIL'}); detail=$detail })
    if (-not $ok) { throw "${id}: $detail" }
}
$failed = $false
try {
    [void](Rcon 'ABORT'); [void](Wait-Status { param($s) $s.phase -eq 'Lobby' } 30 'lobby')
    # Explicitly clear team configuration left by earlier experiments.
    foreach ($c in @('MODE deathmatch','TEAMS off','ROUNDTIME 600',"OPTION respawn $RespawnDelay",'BESTOF 31')) { [void](Rcon $c) }
    for ($i=0; $i -lt 2; $i++) { Dev $i @('autotest','parity','ai off'); Dev $i @('autotest','ready') }
    [void](Wait-Status { param($s) @($s.roster).Count -eq 2 -and @($s.roster | Where-Object { -not $_.ready }).Count -eq 0 } 30 'two ready players')
    [void](Rcon 'START')
    $initial = Wait-Status { param($s) $s.phase -eq 'Live' } 180 'Live'
    Check 'native-live' ([bool]$initial.debug_verbs) 'Real games reached Live; debug stimulus enabled.'
    Check 'native-initial-placement' (@($initial.roster | Where-Object { $_.loaded_round -ne $initial.round -or -not $_.alive }).Count -eq 0) 'Both real games completed the current round load barrier.'
    $matchId = $initial.match_id; $round = $initial.round
    foreach ($c in @('MODE koth','TEAMS auto 3','ROUNDTIME 180','OPTION koth_target 90','OPTION respawn 5')) {
        $r = Rcon $c -AllowError
        Check ('A16-backend-' + ($c -replace ' ','-')) ($r -match '^ERR .*lobby') 'Mode configuration must be refused during Live.'
    }
    for ($iteration=1; $iteration -le $DeathsPerPlayer; $iteration++) {
        foreach ($seat in @(1,2)) {
            $before = Status; $old = @($before.roster | Where-Object { $_.seat -eq $seat })[0]
            if (-not $old.alive) { throw "Seat $seat was not alive before the stimulus." }
            $startMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            [void](Rcon "DEBUG KILL $seat")
            $dead = Wait-Status { param($s) $p=@($s.roster | Where-Object { $_.seat -eq $seat })[0]; $p.deaths -eq ($old.deaths + 1) } 5 'death receipt'
            $deadPlayer=@($dead.roster | Where-Object { $_.seat -eq $seat })[0]
            Check "A12-backend-death-$seat-$iteration" (-not $deadPlayer.alive -and $deadPlayer.respawning -and $dead.phase -eq 'Live') 'Death accepted and native respawn pending without ending the round.'
            $after = Wait-Status { param($s) $p=@($s.roster | Where-Object { $_.seat -eq $seat })[0]; $p.alive -and -not $p.respawning -and $p.deaths -eq ($old.deaths + 1) } $RespawnTimeout 'verified respawn placement'
            $elapsed = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - $startMs
            Check "A12-backend-$seat-$iteration" ($after.phase -eq 'Live' -and $after.match_id -eq $matchId -and $after.round -eq $round) 'Deathmatch stays in the same live round after verified placement.'
            $samples.Add(@{ seat=$seat; iteration=$iteration; stimulus='DEBUG KILL'; respawn_delay_s=$RespawnDelay; death_to_verified_placement_ms=$elapsed; placement_under_15_s=($elapsed -lt 15000); before=$before; death=$dead; after=$after })
            Save-Json (Join-Path $Out 'respawns.json') $samples.ToArray()
            # The next debug stimulus waits for the declared 2 s protection; it
            # must not mistake that refusal for a failed new death.
            $protectedUntil = [Diagnostics.Stopwatch]::StartNew()
            while ($protectedUntil.Elapsed.TotalMilliseconds -lt 2200) { [void](Status); Start-Sleep -Milliseconds 200 }
        }
    }
    Check 'A14-backend-repeat' ($samples.Count -eq (2 * $DeathsPerPlayer)) 'Every explicit death returned to a server-verified native placement.'
} catch {
    $failed = $true
    $checks.Add(@{ id='runner'; verdict='FAIL'; detail=$_.Exception.Message })
} finally {
    try { [void](Rcon 'ABORT') } catch { $checks.Add(@{ id='return-lobby'; verdict='FAIL'; detail=$_.Exception.Message }); $failed=$true }
    $lock.Dispose(); Remove-Item -LiteralPath $lockPath -Force
    $ids=@(1..18 | ForEach-Object { "A$_" }) + @(1..19 | ForEach-Object { "B$_" }) + @('C1','C2')
    $acceptance = $ids | ForEach-Object { @{ id=$_; verdict='NOT RUN'; detail='Backend subchecks only. Review the full modes/mods live acceptance plan; A12-A14/A16 also need HUD, controls, kits, protection hits, remote stand-ins and mode-screen evidence.' } }
    Save-Json (Join-Path $Out 'report.json') @{ evidence_level='native backend subchecks'; source_run=$run; checks=$checks.ToArray(); acceptance=@($acceptance); samples=$samples.ToArray(); release_gate_pass=$false }
}
Write-Host "Modes backend evidence: $Out\report.json. A12-A14/A16 acceptance still needs visual/control review."
if ($failed) { exit 1 }

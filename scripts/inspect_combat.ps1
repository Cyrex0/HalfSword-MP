#Requires -Version 5.1
<#
.SYNOPSIS
Save a read-only damage/body/world snapshot of the current manual combat run.
.EXAMPLE
.\scripts\inspect_combat.ps1
#>
[CmdletBinding()]
param([string]$RunDir = '')
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'lib\hsmp_runs.ps1')
if (-not $RunDir) {
    $latest = Get-ChildItem (Join-Path $repo 'test-results') -Directory -Filter '*combat_manual' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $latest) { throw 'No manual combat run found. Run scripts\combat_test.ps1 first.' }
    $RunDir = $latest.FullName
}
$cfg = Get-Content (Join-Path $RunDir 'run.json') -Raw | ConvertFrom-Json
$records = @(Get-Content (Join-Path $RunDir 'mp_test.pids.json') -Raw | ConvertFrom-Json)
$tools = Join-Path $repo 'target\release\hsmp-tools.exe'
$stamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
$outDir = Join-Path $RunDir "observations\$stamp"
New-Item -ItemType Directory -Force $outDir | Out-Null
$clients = @()
foreach ($record in ($records | Where-Object role -match '^game\d+$')) {
    if (-not (Hsmp-SameProcess $record)) { continue }
    $inst = $record.role -replace '^game',''
    $rawPath = Join-Path $outDir "inst$inst-ipc.json"
    $stderr = Join-Path $outDir "inst$inst-ipc.stderr.txt"
    & $tools ipc-dump --pid "$($record.pid)" --json 2>$stderr | Set-Content $rawPath
    if ($LASTEXITCODE -ne 0) { throw "IPC snapshot failed for $($record.role): $(Get-Content $stderr -Raw)" }
    $ipc = Get-Content $rawPath -Raw | ConvertFrom-Json
    $stateDir = $cfg.state_dirs.PSObject.Properties[$inst].Value
    $eventFile = Join-Path $stateDir 'hsmp_events.jsonl'
    if (-not (Test-Path $eventFile)) { $eventFile = Join-Path $RunDir "inst$inst\hsmp_events.jsonl" }
    $events = @()
    if (Test-Path $eventFile) {
        $events = @(Get-Content $eventFile -Tail 6000 | Where-Object { $_ -match '"ev":"(combat_quality|world_sync_quality|pawn_state|spawn_stretch|kit_verified|willie_census|respawn)"' } |
            ForEach-Object { try { $_ | ConvertFrom-Json } catch { } })
    }
    foreach ($file in '.parity_results.txt', '.world_scan.txt') {
        $source = Join-Path $stateDir $file
        if (Test-Path $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $outDir "inst$inst-$file") }
    }
    $clients += [ordered]@{
        inst = $inst; pid = $record.pid
        link = $ipc.records.link.v
        root = $ipc.records.local_root.v
        own_vitals = $ipc.records.vitals.v
        peer_vitals = $ipc.records.peer_vitals
        own_body = $ipc.records.body.v
        peer_bodies = $ipc.records.peer_body
        mode = $ipc.records.mode.v
        combat = @($events | Where-Object ev -eq 'combat_quality' | Select-Object -Last 3)
        world = @($events | Where-Object ev -eq 'world_sync_quality' | Select-Object -Last 3)
        stretch = @($events | Where-Object ev -eq 'spawn_stretch' | Select-Object -Last 3)
        census = @($events | Where-Object ev -eq 'willie_census' | Select-Object -Last 1)
    }
}
$plan = Get-Content (Join-Path $RunDir 'plan.txt') -Raw
$passwordMatch = [regex]::Match($plan, '--rcon-password\s+(\S+)')
$status = $null
if ($passwordMatch.Success) {
    $gate = Join-Path $repo 'target\release\hsmp-gate.exe'
    $reply = & $gate rcon --addr "127.0.0.1:$($cfg.ports.rcon)" --password $passwordMatch.Groups[1].Value STATUS
    if ($LASTEXITCODE -eq 0 -and $reply -match '^OK (.+)$') { $status = $Matches[1] | ConvertFrom-Json }
}
$snapshot = [ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); run = $cfg.run_id; status = $status; clients = $clients; evidence = $outDir }
$snapshot | ConvertTo-Json -Depth 20 | Set-Content (Join-Path $outDir 'snapshot.json')
$snapshot | ConvertTo-Json -Depth 20 -Compress | Add-Content (Join-Path $RunDir 'combat-observations.jsonl')
"Saved combat snapshot: $outDir"
if ($status) { "Phase=$($status.phase) round=$($status.round) players=$($status.peers)" }
if ($clients.Count -eq 0) { 'No tracked game instances are running; this snapshot contains server status only.' }
foreach ($client in $clients) {
    $combat = $client.combat | Select-Object -Last 1
    $world = $client.world | Select-Object -Last 1
    $combatText = if ($combat) { "claims=$($combat.claims) accepted=$($combat.accepted) pending=$($combat.pending)" } else { 'combat data not sampled yet' }
    $worldText = if ($world) { "object snaps=$($world.hard_snaps) max correction=$($world.max_off_cm) cm" } else { 'world correction data not sampled yet' }
    "Player $($client.inst): $combatText; $worldText"
}

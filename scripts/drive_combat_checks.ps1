#Requires -Version 5.1
<#
.SYNOPSIS
Drive native arm input in the current two-client combat session and save evidence.
.DESCRIPTION
The development probe holds the game's actual arm input through its native
input callbacks. It does not inject damage records. Confirm actual contacts
in the native DCD log; results are observations, not a solo-parity verdict.
Use -BringTogether to move the attacking player into range of the other player.
#>
[CmdletBinding()]
param([string]$RunDir = '', [switch]$BringTogether)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'lib/hsmp_runs.ps1')
if (-not $RunDir) {
    $RunDir = (Get-ChildItem (Join-Path $repo 'test-results') -Directory -Filter '*combat_manual' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}
if (-not $RunDir) { throw 'No combat session found.' }
$tracked = @(Get-Content (Join-Path $RunDir 'mp_test.pids.json') -Raw | ConvertFrom-Json)
$games = @($tracked | Where-Object { $_.role -match '^game\d+$' -and (Hsmp-SameProcess $_) } | Sort-Object role)
if ($games.Count -ne 2) { throw 'This check requires two tracked, running game instances.' }
$probe = Join-Path $repo 'target/release/hsmp-tools.exe'
function Send-Experiment($game, [string]$experiment) {
    & $probe ipc-ctl --pid "$($game.pid)" autotest parity $experiment
    if ($LASTEXITCODE -ne 0) { throw "Probe delivery failed: $experiment" }
    [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');player=$game.role;experiment=$experiment} |
        ConvertTo-Json -Compress | Add-Content (Join-Path $RunDir 'driven-combat.jsonl')
}
& (Join-Path $PSScriptRoot 'inspect_combat.ps1') -RunDir $RunDir
foreach ($game in $games) { Send-Experiment $game 'kit' }
if ($BringTogether) {
    Send-Experiment $games[0] 'near'
    Start-Sleep -Seconds 2
}
foreach ($experiment in @('arm r 0.25', 'arm l 0.25', 'arm r 1.25', 'arm l 1.25')) {
    $positions = @()
    foreach ($game in $games) {
        $raw = & $probe ipc-dump --pid "$($game.pid)" --json
        if ($LASTEXITCODE -ne 0) { throw 'Cannot verify current player positions.' }
        $positions += ,(($raw | ConvertFrom-Json).records.local_root.v.pos)
    }
    if ($positions.Count -ne 2 -or $positions[0].Count -ne 3 -or $positions[1].Count -ne 3) {
        throw 'Player positions are unavailable; no arm input delivered.'
    }
    $distanceSquared = 0.0
    for ($axis = 0; $axis -lt 3; $axis++) { $distanceSquared += [math]::Pow($positions[0][$axis] - $positions[1][$axis], 2) }
    if ($distanceSquared -gt 350 * 350) {
        "Skipped $experiment`: players are out of range; this case is not a combat result."
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');experiment=$experiment;skipped='out_of_range';distance_cm=[math]::Sqrt($distanceSquared)} |
            ConvertTo-Json -Compress | Add-Content (Join-Path $RunDir 'driven-combat.jsonl')
        continue
    }
    Send-Experiment $games[0] $experiment
    Start-Sleep -Seconds 3
    & (Join-Path $PSScriptRoot 'inspect_combat.ps1') -RunDir $RunDir
}
foreach ($game in $games) { Send-Experiment $game 'kit' }
Start-Sleep -Seconds 1
& (Join-Path $PSScriptRoot 'inspect_combat.ps1') -RunDir $RunDir
'Physics checks delivered. Inspect driven-combat.jsonl and paired observations for actual contacts and outcomes.'

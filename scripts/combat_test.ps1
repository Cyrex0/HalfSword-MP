#Requires -Version 5.1
<#
.SYNOPSIS
Launch two real clients for manual damage, ragdoll and object-physics inspection.
.DESCRIPTION
Uses the existing isolated multiplayer harness: normal spawns, unmodified player
input, per-client events and IPC taps, server logs, save protection and PID cleanup.
Walk the players together and fight normally. No scripted movement, invulnerability,
teleports or admin kills are applied. Start without network impairment to separate
physics differences from transport effects, then repeat with -Netsim typical.
.EXAMPLE
.\scripts\combat_test.ps1
.EXAMPLE
.\scripts\combat_test.ps1 -Minutes 10 -Netsim typical
#>
[CmdletBinding()]
param(
    [ValidateRange(1,120)][int]$Minutes = 30,
    [string]$Netsim = 'none',
    [ValidateSet('duel','brawl')][string]$CombatMode = 'duel',
    [string]$GamePath = '',
    [string[]]$ServerArgs = @()
)
$argsForRun = @{
    Instances = 2
    Scenario = 'combat_manual'
    CombatMode = $CombatMode
    Netsim = $Netsim
    HoldScale = $Minutes / 30.0
}
if ($GamePath) { $argsForRun.GamePath = $GamePath }
if ($ServerArgs.Count -gt 0) { $argsForRun.ServerArgs = $ServerArgs }
$previousPoseTap = $env:HSMP_IPC_TAP_POSES
try {
    # Preserve each exact transmitted weapon/body frame for contact analysis.
    $env:HSMP_IPC_TAP_POSES = '1'
    & (Join-Path $PSScriptRoot 'mp_test.ps1') @argsForRun
} finally {
    $env:HSMP_IPC_TAP_POSES = $previousPoseTap
}
exit $LASTEXITCODE

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

-Arena / -Kit / -KitRules set the arena, kit rules and both players' kit before the
first START, so the first match is the one under test. Dev builds only: -Ai hands both
players to the game's own fighter AI every Live round (HSMPParity `ai auto`), -Probe
logs each blow's native result on the attacker's stand-in beside the victim's replay
(HSMPCombat `combat_probe`).
.EXAMPLE
.\scripts\combat_test.ps1
.EXAMPLE
.\scripts\combat_test.ps1 -Minutes 10 -Netsim typical
.EXAMPLE
.\scripts\combat_test.ps1 -Minutes 15 -Arena Yard -KitRules custom -Kit "duelist r=w_arming3 l= armor=b_tunic,l_hosen3,f_shoes1" -Ai -Probe
#>
[CmdletBinding()]
param(
    [ValidateRange(1,120)][int]$Minutes = 30,
    [string]$Netsim = 'none',
    [ValidateSet('duel','brawl')][string]$CombatMode = 'duel',
    [string]$GamePath = '',
    [string[]]$ServerArgs = @(),
    [string]$Arena = '',
    [string]$Kit = '',
    [ValidateSet('','free','classes','custom')][string]$KitRules = '',
    [switch]$Ai,
    [switch]$Probe
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
if ($Arena) { $argsForRun.CombatArena = $Arena }
if ($Kit) { $argsForRun.CombatKit = $Kit }
if ($KitRules) { $argsForRun.CombatKitRules = $KitRules }
if ($Ai) { $argsForRun.CombatAi = $true }
if ($Probe) { $argsForRun.CombatProbe = $true }
$previousPoseTap = $env:HSMP_IPC_TAP_POSES
try {
    # Preserve each exact transmitted weapon/body frame for contact analysis.
    $env:HSMP_IPC_TAP_POSES = '1'
    & (Join-Path $PSScriptRoot 'mp_test.ps1') @argsForRun
} finally {
    $env:HSMP_IPC_TAP_POSES = $previousPoseTap
}
exit $LASTEXITCODE

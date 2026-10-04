#Requires -Version 5.1
<#
.SYNOPSIS
    Install the HSMP dedicated server as a Windows service via NSSM.

.DESCRIPTION
    Uses NSSM (the Non-Sucking Service Manager, https://nssm.cc) to wrap hsmp-server.exe as a
    Windows service that starts at boot. NSSM must be on PATH; if it is not, and Chocolatey is
    installed, the script installs it with `choco install nssm`.

    The service runs as LocalSystem. Its state (the server identity key, bans.txt, admins.txt)
    goes to -StateDir (HSMP_STATE_DIR), not to the LocalSystem profile, so it survives
    reinstalls. The firewall rule for the game port is NOT added; see
    docs/hosting/ports-and-firewall.md.

    The server registers with the public server list unless -NoMaster (or -MasterUrl off).
    To change settings, run the script again: it replaces the service.

    Install: .\scripts\install-service.ps1 [-BinDir <folder with hsmp-server.exe>] [-AdminKey <key>]
    Remove:  .\scripts\install-service.ps1 -Uninstall

.PARAMETER BinDir
    Folder with hsmp-server.exe (e.g. the hsmp\ folder of a release zip).
    Default: $env:CARGO_TARGET_DIR\release, else <repo>\target\release.
.PARAMETER StateDir
    Server state folder (identity key, bans, admins). Default: $env:ProgramData\HSMP\server.
.PARAMETER LogLevel
    Server log level: error, warn, info (default), debug, trace, or a RUST_LOG-style filter. The
    server writes <StateDir>ogsserverserver-<day>.log (+ server-events-<day>.jsonl), 14 days.
.PARAMETER Mode
    Game mode (duel, ffa, ...). Default duel.
.PARAMETER Map
    Arena advertised to the server list (e.g. Map_Arena_Pit).
.PARAMETER Region
    Region tag shown in the server browser (e.g. EU, NA-East).
.PARAMETER AdminKey
    Admin player key (64 hex, from `hsmp-sidecar --print-player-key`). Repeatable: -AdminKey k1,k2.
.PARAMETER AdminsFile
    Admin keys file, one key per line. Default: <StateDir>\admins.txt.
.PARAMETER MasterUrl
    Server list to register with. Default: https://master.halfswordmp.workers.dev. "off" = not listed.
.PARAMETER NoMaster
    LAN-only: do not register with any server list.
.PARAMETER RconBind
    RCON address. Default 127.0.0.1:2345. RCON is on only with -RconPassword.
.PARAMETER RconPassword
    RCON password, kept in the service environment (HSMP_RCON_PASSWORD), not on the command line.
.PARAMETER DryRun
    Print the server command line and environment, install nothing (needs no Administrator).

.NOTES
    Must be run as Administrator (service install needs it), except with -DryRun.
#>
[CmdletBinding()]
param(
    [string]$ServiceName = "HsmpDedicatedServer",
    [string]$Bind = "0.0.0.0:7777",
    [string]$Name = "HSMP Dedicated",
    [string]$Mode = "duel",
    [string]$Map = "",
    [string]$Region = "",
    [string[]]$AdminKey = @(),
    [string]$AdminsFile = "",
    [string]$MasterUrl = "https://master.halfswordmp.workers.dev",
    [switch]$NoMaster,
    [string]$RconBind = "127.0.0.1:2345",
    [string]$RconPassword = "",
    [string]$BinDir = "",
    [string]$StateDir = "",
    [string]$LogLevel = "",
    [switch]$DryRun,
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $DryRun -and -not $isAdmin) {
    Write-Host "run this from an Administrator PowerShell (or pass -DryRun)." -ForegroundColor Red
    exit 1
}

$Repo = Split-Path -Parent $PSScriptRoot
if (-not $BinDir) { $BinDir = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $Repo "target\release" } }
if (-not $StateDir) { $StateDir = Join-Path $env:ProgramData "HSMP\server" }
$Bin  = Join-Path $BinDir "hsmp-server.exe"
$Logs = Join-Path $StateDir "logs"

if ($Uninstall) {
    Write-Host "stopping + removing $ServiceName..."
    & nssm stop $ServiceName 2>&1 | Out-Null
    & nssm remove $ServiceName confirm 2>&1 | Out-Null
    Write-Host "done (state in $StateDir is kept)." -ForegroundColor Green
    exit 0
}

$adminKeys = @($AdminKey | ForEach-Object { $_ -split "," } | ForEach-Object { $_.Trim() } |
    Where-Object { $_ } | Select-Object -Unique)
foreach ($k in $adminKeys) {
    if ($k -notmatch "^[0-9a-fA-F]{64}$") { Write-Host "admin key is not 64 hex characters: $k" -ForegroundColor Red; exit 1 }
}
$master = if ($NoMaster -or $MasterUrl.Trim() -in @("", "off", "none", "lan")) { "" } else { $MasterUrl.Trim() }
$Bans   = Join-Path $StateDir "bans.txt"
$Admins = if ($AdminsFile) { $AdminsFile } else { Join-Path $StateDir "admins.txt" }

$svArgs = @("--bind", $Bind, "--name", $Name, "--mode", $Mode, "--max-peers", 8,
            "--bans-file", $Bans, "--admins-file", $Admins)
if ($Map)    { $svArgs += @("--map", $Map) }
if ($Region) { $svArgs += @("--region", $Region) }
foreach ($k in $adminKeys) { $svArgs += @("--admin-key", $k) }
if ($RconPassword) { $svArgs += @("--rcon-bind", $RconBind) }
if ($LogLevel)     { $svArgs += @("--log-level", $LogLevel) }

$envExtra = @("HSMP_STATE_DIR=$StateDir")
if ($master) { $envExtra += "HSMP_MASTER_URL=$master" }
if ($RconPassword) { $envExtra += "HSMP_RCON_PASSWORD=$RconPassword" }

if ($DryRun) {
    Write-Host "service $ServiceName would run: $Bin $($svArgs -join ' ')"
    Write-Host "environment: $(($envExtra | ForEach-Object { if ($_ -like 'HSMP_RCON_PASSWORD=*') { 'HSMP_RCON_PASSWORD=***' } else { $_ } }) -join '; ')"
    Write-Host "server list: $(if ($master) { $master } else { 'none (LAN-only)' })"
    exit 0
}

if (-not (Get-Command nssm -ErrorAction SilentlyContinue)) {
    Write-Host "installing NSSM via chocolatey..."
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
        Write-Host "NSSM not found and Chocolatey is not installed: install NSSM (https://nssm.cc) and put it on PATH." -ForegroundColor Red
        exit 1
    }
    choco install nssm -y --no-progress
}

if (-not (Test-Path $Bin)) {
    Write-Host "hsmp-server.exe missing: $Bin" -ForegroundColor Red
    Write-Host "build it (cargo build --release -p hsmp-server) or pass -BinDir." -ForegroundColor Yellow
    exit 1
}

New-Item -ItemType Directory -Force $StateDir, $Logs | Out-Null

# Re-running replaces the service so new settings take effect.
& nssm status $ServiceName 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Host "replacing existing service $ServiceName..."
    & nssm stop $ServiceName 2>&1 | Out-Null
    & nssm remove $ServiceName confirm 2>&1 | Out-Null
}

Write-Host "installing service $ServiceName -> $Bin"
& nssm install $ServiceName $Bin @svArgs
& nssm set    $ServiceName AppEnvironmentExtra @envExtra
& nssm set    $ServiceName AppStdout (Join-Path $Logs "hsmp-server.stdout.log")
& nssm set    $ServiceName AppStderr (Join-Path $Logs "hsmp-server.stderr.log")
& nssm set    $ServiceName AppRotateFiles 1
& nssm set    $ServiceName AppRotateOnline 1
& nssm set    $ServiceName AppRotateBytes 10485760

& nssm start $ServiceName
Write-Host "service $ServiceName installed + started (state: $StateDir)." -ForegroundColor Green
Write-Host "server list: $(if ($master) { $master } else { 'none (LAN-only)' })"
Write-Host "manage with: nssm status/stop/start/restart $ServiceName"

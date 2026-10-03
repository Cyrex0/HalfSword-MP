#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Install the HSMP dedicated server as a Windows service via NSSM.

.DESCRIPTION
    Uses NSSM (the Non-Sucking Service Manager, https://nssm.cc) to wrap hsmp-server.exe as a
    Windows service that starts at boot. NSSM must be on PATH; if it is not, and Chocolatey is
    installed, the script installs it with `choco install nssm`.

    The service runs as LocalSystem. Its state (the server identity key, bans.txt) goes to
    -StateDir (HSMP_STATE_DIR), not to the LocalSystem profile, so it survives reinstalls.
    The firewall rule for the game port is NOT added; see docs/hosting/ports-and-firewall.md.

    Install: .\scripts\install-service.ps1 [-BinDir <folder with hsmp-server.exe>]
    Remove:  .\scripts\install-service.ps1 -Uninstall

.PARAMETER BinDir
    Folder with hsmp-server.exe (e.g. the hsmp\ folder of a release zip).
    Default: $env:CARGO_TARGET_DIR\release, else <repo>\target\release.
.PARAMETER StateDir
    Server state folder (identity key, bans). Default: $env:ProgramData\HSMP\server.

.NOTES
    Must be run as Administrator (service install needs it).
#>
[CmdletBinding()]
param(
    [string]$ServiceName = "HsmpDedicatedServer",
    [string]$Bind = "0.0.0.0:7777",
    [string]$Name = "HSMP Dedicated",
    [string]$BinDir = "",
    [string]$StateDir = "",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

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
$Bans = Join-Path $StateDir "bans.txt"

Write-Host "installing service $ServiceName -> $Bin"
& nssm install $ServiceName $Bin --bind $Bind --name $Name --max-peers 8 --bans-file $Bans
& nssm set    $ServiceName AppEnvironmentExtra "HSMP_STATE_DIR=$StateDir"
& nssm set    $ServiceName AppStdout (Join-Path $Logs "hsmp-server.stdout.log")
& nssm set    $ServiceName AppStderr (Join-Path $Logs "hsmp-server.stderr.log")
& nssm set    $ServiceName AppRotateFiles 1
& nssm set    $ServiceName AppRotateOnline 1
& nssm set    $ServiceName AppRotateBytes 10485760

& nssm start $ServiceName
Write-Host "service $ServiceName installed + started (state: $StateDir)." -ForegroundColor Green
Write-Host "manage with: nssm status/stop/start/restart $ServiceName"

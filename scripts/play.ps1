#Requires -Version 5.1
<#
.SYNOPSIS
    One-click: build + deploy + diag + launch Half Sword.

.DESCRIPTION
    First-run convenience. Builds Rust bins (unless -SkipBuild), mirrors
    every mod to the game folder, runs the diag check, and launches the
    game if diag passes.

.PARAMETER SkipBuild
    Skip cargo build.

.PARAMETER NoLaunch
    Build + deploy + diag, but do not launch the game.

.PARAMETER Dev
    Deploy the developer profile (HSMPDiag probes, UE4SS Keybinds; see mods\mods.release.txt).

.PARAMETER GamePath
    Override game install dir. Default: $env:HSMP_GAME_DIR, <repo>\game, then the main
    git checkout's game\ (linked worktrees have no game\ of their own).
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$NoLaunch,
    [switch]$Dev,
    [string]$GamePath = ""
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
if (-not $GamePath) {
    $cands = @($env:HSMP_GAME_DIR, (Join-Path $Repo "game"))
    $common = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir 2>$null)
    if ($LASTEXITCODE -eq 0 -and $common) { $cands += (Join-Path (Split-Path -Parent $common.Trim()) "game") }
    foreach ($c in $cands) {
        if ($c -and (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64"))) { $GamePath = $c; break }
    }
    if (-not $GamePath) { Write-Host "game not found; pass -GamePath or set HSMP_GAME_DIR" -ForegroundColor Red; exit 1 }
}

Write-Host ""
Write-Host "===============================================" -ForegroundColor Cyan
Write-Host " HSMP - developer build + deploy + launch" -ForegroundColor Cyan
Write-Host "===============================================" -ForegroundColor Cyan

# Step 1: build + deploy
$deployArgs = @("-GamePath", $GamePath)
if ($SkipBuild) { $deployArgs += "-SkipBuild" }
if ($Dev) { $deployArgs += "-Dev" }
& (Join-Path $PSScriptRoot "build-and-deploy.ps1") @deployArgs
if ($LASTEXITCODE -ne 0) {
    Write-Host "build-and-deploy failed; aborting." -ForegroundColor Red
    exit 1
}

# Step 2: diag
Write-Host ""
Write-Host "Running diag..." -ForegroundColor Cyan
& (Join-Path $PSScriptRoot "diag.ps1") -GamePath $GamePath
if ($LASTEXITCODE -ne 0) {
    Write-Host "diag reported failures; aborting launch." -ForegroundColor Red
    exit 1
}

if ($NoLaunch) {
    Write-Host "-NoLaunch set; stopping before game launch." -ForegroundColor Yellow
    exit 0
}

# Step 3: launch
$gameBin = Join-Path $GamePath "HalfswordUE5\Binaries\Win64\HalfswordUE5-Win64-Shipping.exe"
if (-not (Test-Path $gameBin)) {
    Write-Host "Game binary not found: $gameBin" -ForegroundColor Red
    Write-Host "Install Half Sword to $GamePath or override with -GamePath." -ForegroundColor Yellow
    exit 1
}

Write-Host ""
Write-Host "Launching Half Sword..." -ForegroundColor Green
Write-Host "  UE4SS console will open if ConsoleEnabled is true in UE4SS-settings.ini." -ForegroundColor Gray
Write-Host "  Watch it for the '[HSMPMenu]' lines (the menu ribbon hook)." -ForegroundColor Gray
Write-Host "  Main menu then click the Multiplayer ribbon to open the central menu." -ForegroundColor Gray
Write-Host ""

Start-Process -FilePath $gameBin -WorkingDirectory (Split-Path $gameBin)
Write-Host "Game launched. Have fun." -ForegroundColor Green

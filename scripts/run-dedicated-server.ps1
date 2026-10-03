#Requires -Version 5.1
<#
.SYNOPSIS
    Run a public-facing Half Sword Multiplayer dedicated server.

.DESCRIPTION
    Launches hsmp-server.exe with explicit args from a config file (TOML-ish
    key=value). If no config exists one is written on first run. Optionally
    starts a hsmp-master registry too (self-hosted lobby browser backend).

    This script does NOT need the Half Sword game installed — the dedicated
    server is pure Rust UDP; game clients connect to it via their sidecar.

.PARAMETER Config
    Path to a server config file. Default: dedicated-server.conf in the repo root (or next
    to the binaries when -BinDir is given).

.PARAMETER BinDir
    Folder with hsmp-server.exe (and hsmp-master.exe), e.g. the hsmp\ folder of a release
    zip. Default: $env:CARGO_TARGET_DIR\release, else <repo>\target\release.

.PARAMETER WithMaster
    Also launch an hsmp-master registry on master_bind (default 0.0.0.0:7778).

.PARAMETER DebugLog
    Enable RUST_LOG=debug.

.EXAMPLE
    PS> .\scripts\run-dedicated-server.ps1
    reads/creates dedicated-server.conf, starts hsmp-server bound to 0.0.0.0:7777.

.EXAMPLE
    PS> .\scripts\run-dedicated-server.ps1 -WithMaster
    starts both master (:7778) and server (:7777). Server auto-registers.
#>
param(
    [string]$Config = "",
    [string]$BinDir = "",
    [switch]$WithMaster,
    [switch]$DebugLog
)

$ErrorActionPreference = "Stop"

$Repo = Split-Path -Parent $PSScriptRoot
$Bins = if ($BinDir) { $BinDir } elseif ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $Repo "target\release" }
if (-not $Config) { $Config = Join-Path $(if ($BinDir) { $BinDir } else { $Repo }) "dedicated-server.conf" }

function Say([string]$m, [ConsoleColor]$fg = "Gray") {
    Write-Host "[dedi] " -NoNewline -ForegroundColor Cyan
    Write-Host $m -ForegroundColor $fg
}

# --- Load / seed config ---------------------------------------------------

$defaults = @{
    bind         = "0.0.0.0:7777"
    tick_hz      = 30
    max_peers    = 8
    name         = "HSMP Dedicated"
    mode         = "duel"
    master_url   = ""
    master_bind  = "0.0.0.0:7778"
    bans_file    = "bans.txt"
}

if (-not (Test-Path $Config)) {
    Say "writing default config: $Config" Yellow
    $lines = @("# HSMP dedicated-server config. Edit and re-run.")
    foreach ($k in $defaults.Keys) { $lines += "$k = $($defaults[$k])" }
    Set-Content -Path $Config -Value $lines -Encoding ASCII
}

$cfg = @{}
foreach ($line in Get-Content $Config) {
    if ($line -match "^\s*#" -or -not $line.Trim()) { continue }
    if ($line -match "^\s*([a-zA-Z_]+)\s*=\s*(.+?)\s*$") {
        $cfg[$Matches[1]] = $Matches[2]
    }
}
foreach ($k in $defaults.Keys) { if (-not $cfg.ContainsKey($k)) { $cfg[$k] = $defaults[$k] } }

Say "config: $Config"
foreach ($k in @("bind","tick_hz","max_peers","name","mode")) {
    Say "  $k = $($cfg[$k])"
}

# --- Verify binaries exist ------------------------------------------------

$svBin = Join-Path $Bins "hsmp-server.exe"
$mpBin = Join-Path $Bins "hsmp-master.exe"
if (-not (Test-Path $svBin)) {
    Say "hsmp-server.exe not found at $svBin - build it (cargo build --release -p hsmp-server) or pass -BinDir" Red
    exit 1
}
if ($WithMaster -and -not (Test-Path $mpBin)) {
    Say "hsmp-master.exe not found at $mpBin" Red
    exit 1
}

# --- Environment ----------------------------------------------------------

if ($DebugLog) { $env:RUST_LOG = "hsmp_server=debug,hsmp_master=debug,hsmp_sidecar=debug" }
else          { $env:RUST_LOG = "hsmp_server=info,hsmp_master=info" }

if ($cfg["master_url"]) { $env:HSMP_MASTER_URL = $cfg["master_url"] }

# --- Launch master (optional) --------------------------------------------

$mpProc = $null
if ($WithMaster) {
    Say "starting hsmp-master on $($cfg['master_bind'])" Green
    $mpProc = Start-Process -FilePath $mpBin `
        -ArgumentList @("--bind", $cfg["master_bind"]) `
        -PassThru -NoNewWindow
    Start-Sleep -Milliseconds 400
    # Point this server at the local master so it auto-registers.
    $portPart = ($cfg["master_bind"] -split ":")[-1]
    $env:HSMP_MASTER_URL = "http://127.0.0.1:$portPart"
    Say "server will auto-register with http://127.0.0.1:$portPart"
    Say "dashboard: http://127.0.0.1:$portPart/"
}

# --- Launch server -------------------------------------------------------

# bans_file: relative paths are next to the config file
$bansPath = $cfg["bans_file"]
if (-not [IO.Path]::IsPathRooted($bansPath)) { $bansPath = Join-Path (Split-Path -Parent ([IO.Path]::GetFullPath($Config))) $bansPath }

Say "starting hsmp-server on $($cfg['bind'])" Green
Say "ctrl-c to stop; children also exit"
try {
    # Attach stdio so logs stream here.
    & $svBin --bind $cfg["bind"] `
             --tick-hz ([int]$cfg["tick_hz"]) `
             --max-peers ([int]$cfg["max_peers"]) `
             --name $cfg["name"] `
             --mode $cfg["mode"] `
             --bans-file $bansPath
} finally {
    if ($mpProc) {
        Say "stopping master (PID $($mpProc.Id))" Yellow
        Stop-Process -Id $mpProc.Id -Force -ErrorAction SilentlyContinue
    }
}

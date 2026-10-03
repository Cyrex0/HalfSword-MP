#Requires -Version 5.1
<#
.SYNOPSIS
    Run a public-facing Half Sword Multiplayer dedicated server.

.DESCRIPTION
    Launches hsmp-server.exe with explicit args from a config file (key = value). If no
    config exists one is written on first run. Command-line parameters override the config.
    Optionally starts a hsmp-master registry too (self-hosted server list).

    This script does NOT need the Half Sword game installed: the dedicated server is pure
    Rust UDP; game clients connect to it via their sidecar.

    By default the server registers with the public server list
    (https://master.halfswordmp.workers.dev). Use -NoMaster (or master_url = off) for a
    LAN-only server that is listed nowhere.

.PARAMETER Config
    Path to a server config file. Default: dedicated-server.conf in the repo root (or next
    to the binaries when -BinDir is given).

.PARAMETER BinDir
    Folder with hsmp-server.exe (and hsmp-master.exe), e.g. the hsmp\ folder of a release
    zip. Default: $env:CARGO_TARGET_DIR\release, else <repo>\target\release.

.PARAMETER AdminKey
    Player key (64 hex, from `hsmp-sidecar --print-player-key`) that is admin. Repeatable:
    -AdminKey k1,k2. Added to the config's admin_keys.

.PARAMETER AdminsFile
    Admin keys file, one key per line (re-read when it changes; RCON ADMIN ADD appends).
    Relative paths are next to the config file. Default: admins.txt.

.PARAMETER MasterUrl
    Server list to register with. Default: the public list. "off" = not listed.

.PARAMETER NoMaster
    LAN-only: do not register with any server list.

.PARAMETER Region
    Region tag shown in the server browser (e.g. EU, NA-East).

.PARAMETER Map
    Arena advertised to the server list (e.g. Map_Arena_Pit).

.PARAMETER RconBind
    RCON address. Default 127.0.0.1:2345. RCON is on only when a password is set
    (-RconPassword, rcon_password in the config, or $env:HSMP_RCON_PASSWORD).

.PARAMETER RconPassword
    RCON password. Passed to the server through HSMP_RCON_PASSWORD, not the command line.

.PARAMETER WithMaster
    Also launch an hsmp-master registry on master_bind (default 0.0.0.0:7778) and register
    with it instead of the public list.

.PARAMETER DebugLog
    Enable RUST_LOG=debug.

.EXAMPLE
    PS> .\scripts\run-dedicated-server.ps1 -AdminKey <your key> -Region EU
    reads/creates dedicated-server.conf, starts hsmp-server on 0.0.0.0:7777, listed publicly.

.EXAMPLE
    PS> .\scripts\run-dedicated-server.ps1 -NoMaster -RconPassword "a-long-rcon-password"
    LAN-only server with RCON on 127.0.0.1:2345.
#>
param(
    [string]$Config = "",
    [string]$BinDir = "",
    [string[]]$AdminKey = @(),
    [string]$AdminsFile = "",
    [string]$MasterUrl = "",
    [switch]$NoMaster,
    [string]$Region = "",
    [string]$Map = "",
    [string]$RconBind = "",
    [string]$RconPassword = "",
    [switch]$WithMaster,
    [switch]$DebugLog
)

$ErrorActionPreference = "Stop"

$PublicMaster = "https://master.halfswordmp.workers.dev"
$Repo = Split-Path -Parent $PSScriptRoot
$Bins = if ($BinDir) { $BinDir } elseif ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $Repo "target\release" }
if (-not $Config) { $Config = Join-Path $(if ($BinDir) { $BinDir } else { $Repo }) "dedicated-server.conf" }

function Say([string]$m, [ConsoleColor]$fg = "Gray") {
    Write-Host "[dedi] " -NoNewline -ForegroundColor Cyan
    Write-Host $m -ForegroundColor $fg
}

# --- Load / seed config ---------------------------------------------------

$order = @("bind", "tick_hz", "max_peers", "name", "mode", "map", "region", "master_url", "master_bind",
           "bans_file", "admins_file", "admin_keys", "rcon_bind", "rcon_password")
$defaults = @{
    bind          = "0.0.0.0:7777"
    tick_hz       = 30
    max_peers     = 8
    name          = "HSMP Dedicated"
    mode          = "duel"
    map           = ""
    region        = ""
    master_url    = $PublicMaster
    master_bind   = "0.0.0.0:7778"
    bans_file     = "bans.txt"
    admins_file   = "admins.txt"
    admin_keys    = ""
    rcon_bind     = "127.0.0.1:2345"
    rcon_password = ""
}

if (-not (Test-Path $Config)) {
    Say "writing default config: $Config" Yellow
    $lines = @("# HSMP dedicated-server config. Edit and re-run.",
               "# master_url = off: LAN-only (not on any server list). admin_keys: comma-separated.",
               "# RCON is on only when rcon_password is set.")
    foreach ($k in $order) { $lines += "$k = $($defaults[$k])" }
    Set-Content -Path $Config -Value $lines -Encoding ASCII
}

$cfg = @{}
foreach ($line in Get-Content $Config) {
    if ($line -match "^\s*#" -or -not $line.Trim()) { continue }
    if ($line -match "^\s*([a-zA-Z_]+)\s*=\s*(.*?)\s*$") {
        $cfg[$Matches[1]] = $Matches[2]
    }
}
foreach ($k in $defaults.Keys) { if (-not $cfg.ContainsKey($k)) { $cfg[$k] = $defaults[$k] } }

# Command-line overrides.
if ($AdminsFile)   { $cfg["admins_file"] = $AdminsFile }
if ($MasterUrl)    { $cfg["master_url"] = $MasterUrl }
if ($NoMaster)     { $cfg["master_url"] = "off" }
if ($Region)       { $cfg["region"] = $Region }
if ($Map)          { $cfg["map"] = $Map }
if ($RconBind)     { $cfg["rcon_bind"] = $RconBind }
if ($RconPassword) { $cfg["rcon_password"] = $RconPassword }

$adminKeys = @(@($cfg["admin_keys"] -split ",") + @($AdminKey | ForEach-Object { $_ -split "," }) |
    ForEach-Object { $_.Trim() } | Where-Object { $_ } | Select-Object -Unique)
foreach ($k in $adminKeys) {
    if ($k -notmatch "^[0-9a-fA-F]{64}$") { Say "admin key is not 64 hex characters: $k" Red; exit 1 }
}

# An empty master_url in an older config meant "not listed": keep that meaning.
$master = $cfg["master_url"].Trim()
if ($master -in @("", "off", "none", "lan")) { $master = "" }

$cfgDir = Split-Path -Parent ([IO.Path]::GetFullPath($Config))
function Near-Config([string]$p) { if ([IO.Path]::IsPathRooted($p)) { $p } else { Join-Path $cfgDir $p } }
$bansPath   = Near-Config $cfg["bans_file"]
$adminsPath = Near-Config $cfg["admins_file"]

$rconPw = if ($cfg["rcon_password"]) { $cfg["rcon_password"] } else { $env:HSMP_RCON_PASSWORD }

Say "config: $Config"
foreach ($k in @("bind", "tick_hz", "max_peers", "name", "mode", "map", "region")) {
    if ($cfg[$k] -ne "") { Say "  $k = $($cfg[$k])" }
}
Say "  admins = $($adminKeys.Count) key(s) + $adminsPath"
Say "  rcon = $(if ($rconPw) { $cfg['rcon_bind'] } else { 'off (no password)' })"

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

$env:HSMP_MASTER_URL = $master
if ($rconPw) { $env:HSMP_RCON_PASSWORD = $rconPw }

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
    Say "dashboard: http://127.0.0.1:$portPart/"
}
if ($env:HSMP_MASTER_URL) { Say "server list: $($env:HSMP_MASTER_URL)" }
else                      { Say "server list: none (LAN-only; players join by address)" Yellow }

# --- Launch server -------------------------------------------------------

$svArgs = @("--bind", $cfg["bind"],
            "--tick-hz", [int]$cfg["tick_hz"],
            "--max-peers", [int]$cfg["max_peers"],
            "--name", $cfg["name"],
            "--mode", $cfg["mode"],
            "--bans-file", $bansPath,
            "--admins-file", $adminsPath)
if ($cfg["map"])    { $svArgs += @("--map", $cfg["map"]) }
if ($cfg["region"]) { $svArgs += @("--region", $cfg["region"]) }
foreach ($k in $adminKeys) { $svArgs += @("--admin-key", $k) }
if ($rconPw) { $svArgs += @("--rcon-bind", $cfg["rcon_bind"]) }

Say "starting hsmp-server on $($cfg['bind'])" Green
Say "ctrl-c to stop; children also exit"
try {
    # Attach stdio so logs stream here.
    & $svBin @svArgs
} finally {
    if ($mpProc) {
        Say "stopping master (PID $($mpProc.Id))" Yellow
        Stop-Process -Id $mpProc.Id -Force -ErrorAction SilentlyContinue
    }
}

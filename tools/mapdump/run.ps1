# HSMP MapDump - one-command static extraction of spawn points / world objects / spawners
# from Half Sword's cooked maps. Reads the game pak read-only; never launches the game.
#
#   powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1            # all 88 maps (default)
#   powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1 -ArenasOnly
#   powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1 -Maps Map_Arena_Alley,Map_Arena_Pit
param(
    [switch]$ArenasOnly,
    [string[]]$Maps,
    [string]$Out = (Join-Path $PSScriptRoot '..\..\docs\arena_static')
)
$ErrorActionPreference = 'Stop'

# MapDump reads its inputs from the environment (see Program.cs): the repo root, the game
# (HSMP_GAME_DIR, default <repo>\game) and your own Oodle DLL (HSMP_OODLE_DLL).
$env:HSMP_ROOT = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR = Join-Path $env:HSMP_ROOT 'game' }

# CUE4Parse (needed for usmap v4) targets .NET 10; use a user-local SDK so nothing system-wide changes.
$dn = Join-Path $env:LOCALAPPDATA 'dotnet10'
if (-not (Test-Path (Join-Path $dn 'dotnet.exe'))) {
    Write-Host "Installing user-local .NET 10 SDK to $dn ..."
    $inst = Join-Path $env:TEMP 'dotnet-install.ps1'
    Invoke-WebRequest -UseBasicParsing https://dot.net/v1/dotnet-install.ps1 -OutFile $inst
    & powershell -NoProfile -ExecutionPolicy Bypass -File $inst -Channel 10.0 -InstallDir $dn
}
$env:DOTNET_ROOT = $dn
$dotnet = Join-Path $dn 'dotnet.exe'

& $dotnet build -c Release (Join-Path $PSScriptRoot 'MapDump.csproj') | Select-String -Pattern 'error|Build succeeded'
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$dll = Join-Path $PSScriptRoot 'bin\Release\net10.0\MapDump.dll'
$argsList = @('--out', (Resolve-Path -LiteralPath (New-Item -ItemType Directory -Force $Out)).Path)
if ($Maps) { $argsList += $Maps }
elseif (-not $ArenasOnly) { $argsList += '--all' }
& $dotnet $dll @argsList
if ($LASTEXITCODE -ne 0) { throw 'mapdump failed' }

# per-map summary tables (markdown) for the README (Rust: tools/hsmp-tools)
& cargo run --release --quiet -p hsmp-tools --manifest-path (Join-Path $env:HSMP_ROOT 'Cargo.toml') -- mapdump-summary $argsList[1]
if ($LASTEXITCODE -ne 0) { throw 'mapdump-summary failed' }

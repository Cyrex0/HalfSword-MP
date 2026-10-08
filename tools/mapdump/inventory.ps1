# Offline inventory package/dependency harvest; never launches or writes the game.
param(
    [string]$Out = (Join-Path $PSScriptRoot '../../test-results/dev-feature-checks/inventory-harvest'),
    [string]$Dotnet = (Join-Path $env:LOCALAPPDATA 'dotnet10/dotnet.exe'),
    [switch]$SkipBuild,
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
$inventoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$env:HSMP_ROOT = $inventoryRoot
if (-not $env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR = Join-Path $inventoryRoot 'game' }
# Existing authentication remains in the parser's initialization. Do not print
# environment values or copy authentication into arguments/manifests.
if (-not $SkipBuild) {
    $env:DOTNET_CLI_HOME = Join-Path $inventoryRoot 'test-results/tool-runtime/dotnet-home'
    $env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
    $env:DOTNET_GENERATE_ASPNET_CERTIFICATE = 'false'
    & $Dotnet build --no-restore -c Release (Join-Path $PSScriptRoot 'MapDump.csproj')
    if ($LASTEXITCODE -ne 0) { throw 'Inventory harvester build failed' }
}
$inventoryDll = Join-Path $PSScriptRoot 'bin/Release/net10.0/MapDump.dll'
if ($SelfTest) { & $Dotnet $inventoryDll --inventory-selftest }
else { & $Dotnet $inventoryDll --inventory --out ([IO.Path]::GetFullPath($Out)) }
if ($LASTEXITCODE -eq 2) {
    Write-Warning 'Inventory accounting completed with explicit extraction failures; inspect coverage-manifest.json.'
    exit 2
}
elseif ($LASTEXITCODE -ne 0) { throw 'Inventory harvest failed before complete accounting' }

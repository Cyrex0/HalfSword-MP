#Requires -Version 5.1
<#
.SYNOPSIS
    Run a focused development check for the domains being changed.
.DESCRIPTION
    Builds only the Lua runner and executes selected Lua suites and Rust test
    filters. Every Rust selection must run at least one passing test; every
    selected Lua suite must run assertions. This is a development subset, never
    a G0 stamp, release gate, live-game verdict or automatic dependency audit.
    Use the full G0 before pushing; include integration/stress checks when their
    contracts change. Raw output, command selections and timing go to report.json.
.EXAMPLE
    .\scripts\dev-test.ps1 -Domain combat
.EXAMPLE
    .\scripts\dev-test.ps1 -Domain modes,mods -Plan
.EXAMPLE
    .\scripts\dev-test.ps1 -Domain ipc -Stress
#>
[CmdletBinding()]
param(
    [ValidateSet('combat','pose','modes','mods','kit','ui','ipc','launcher','world')]
    [string[]]$Domain=@('combat'),
    [switch]$List,
    [switch]$Plan,
    [switch]$Stress,
    [string]$Out=''
)
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$utf8=New-Object Text.UTF8Encoding $false
# Each Rust row is an exact cargo argument array, not shell command text.
$domains=[ordered]@{
    combat=@{ description='Hit validation, replay, cuts and native defeat'; lua=@('combat','damage_parity','replay_attempts','body_injury','weapon_bounds','collider_inventory'); rust=@(
        @('-p','hsmp-server','--bin','hsmp-server','combat::'),
        @('-p','hsmp-server','--bin','hsmp-server','lagcomp::'),
        @('-p','hsmp-server','--bin','hsmp-server','combat_glue::')) }
    pose=@{ description='Pose sampling, playback and avatar geometry'; lua=@('avatars','pose','pose_context','framecost'); rust=@(
        @('-p','hsmp-pose'),@('-p','hsmp-server','--bin','hsmp-server','lagcomp::')) }
    modes=@{ description='Mode rules, lobby commands and the spawn/respawn Director'; lua=@('modes_ui','director','commands'); rust=@(
        @('-p','hsmp-server','--bin','hsmp-server','server::modes::tests'),
        @('-p','hsmp-server','--bin','hsmp-server','server::session::tests')) }
    mods=@{ description='Server-mod manifests, consent, transfers, caching and hosting'; lua=@('server_mods','modhost'); rust=@(
        @('-p','hsmp-server','--bin','hsmp-server','server_mods::'),
        @('-p','hsmp-server','--bin','hsmp-server','server::mods_glue::tests'),
        @('-p','hsmp-server','--bin','hsmp-sidecar','mods_client::tests'),
        @('-p','hsmp-server','--bin','hsmp-sidecar','mods_cache::tests')) }
    kit=@{ description='Kit validation, equipment verification and exact SAVE acknowledgements'; lua=@('kit_status','loadout'); rust=@(
        @('-p','hsmp-server','--bin','hsmp-server','loadout::kit_tests'),
        @('-p','hsmp-server','--bin','hsmp-sidecar','loadout_client::kit_client_tests'),
        @('-p','hsmp-tools','--lib','kit_receipt_tests')) }
    ui=@{ description='Complete menu/HUD layout, scaling and behavior matrix'; lua=@('menu_ui','ui_scale'); rust=,@('-p','hsmp-hud-test') }
    ipc=@{ description='IPC schema, native bindings and real sidecar round trip'; lua=@('ipc','records'); rust=@(
        @('-p','hsmp-ipc'),@('-p','hsmp-native'),@('-p','hsmp-server','--test','sidecar_shm')) }
    launcher=@{ description='Launcher installation, updates, manifests and recovery'; lua=@(); rust=,@('-p','hsmp-launcher') }
    world=@{ description='World records, ownership, lifetime guards and mocked physics'; lua=@('hsmpworld','world_state','world_guard'); rust=@(
        @('-p','hsmpworld-tests'),@('-p','hsmp-server','--bin','hsmp-server','world::tests')) }
}
if ($List) {
    foreach($name in $domains.Keys) { Write-Output "$name - $($domains[$name].description)" }
    Write-Output 'Development subsets only. Full G0 is still required before pushing; -Domain ipc -Stress adds the cross-process stress run.'
    exit 0
}
if ($Stress -and $Domain -notcontains 'ipc') { throw '-Stress requires -Domain ipc.' }
$lua=New-Object Collections.Generic.List[string]
$rust=New-Object Collections.Generic.List[object]
$seen=@{}
foreach($name in $Domain) {
    foreach($suite in $domains[$name].lua) { if(-not $lua.Contains($suite)) { $lua.Add($suite) } }
    foreach($arguments in $domains[$name].rust) {
        if($arguments -is [string] -or $arguments.Count -lt 2 -or $arguments[0] -ne '-p') { throw "Invalid Rust selection for $name; expected one cargo argument array per test selection." }
        $key=$arguments -join '|'
        if(-not $seen.ContainsKey($key)) { $seen[$key]=$true; $rust.Add([string[]]$arguments) }
    }
}
if($Stress) { $rust.Add([string[]]@('-p','hsmp-tools','--test','ipc_stress')) }
if($Plan) {
    Write-Output 'Development subset; no G0 stamp or live/release verdict.'
    if($lua.Count) { Write-Output ('Lua suites: '+($lua -join ', ')) }
    foreach($arguments in $rust) { Write-Output ('cargo test --locked '+($arguments -join ' ')) }
    exit 0
}
if(-not $Out) { $Out=Join-Path $repo ('test-results\dev-check-'+[Guid]::NewGuid().ToString('N').Substring(0,8)) }
if(Test-Path -LiteralPath $Out) { throw 'Output directory exists; choose a fresh path.' }
New-Item -ItemType Directory -Path $Out | Out-Null
$Out=(Resolve-Path -LiteralPath $Out).Path
$steps=New-Object Collections.Generic.List[object]
$timer=[Diagnostics.Stopwatch]::StartNew()
$totalRust=0; $luaReport=$null; $failed=$false; $failure=''
function Save-Json([string]$path,$value) { [IO.File]::WriteAllText($path,(ConvertTo-Json -InputObject $value -Depth 12),$utf8) }
function Run-Step([string]$label,[string]$exe,[string[]]$arguments) {
    Write-Host "[dev-test] $label"
    $watch=[Diagnostics.Stopwatch]::StartNew()
    # Windows PowerShell treats native stderr as ErrorRecord. Cargo emits normal
    # progress there; classify the actual exit status, not that stream's presence.
    $oldPreference=$ErrorActionPreference; $ErrorActionPreference='Continue'
    try { $output=@(& $exe @arguments 2>&1 | ForEach-Object { $_.ToString() }); $code=$LASTEXITCODE }
    finally { $ErrorActionPreference=$oldPreference }
    $text=$output -join "`n"
    $log=Join-Path $Out ('{0:D2}.log' -f $steps.Count)
    [IO.File]::WriteAllText($log,$text,$utf8)
    $step=@{ label=$label; executable=$exe; arguments=$arguments; exit_code=$code; seconds=$watch.Elapsed.TotalSeconds; output=$log }
    $steps.Add($step)
    if($code -ne 0) { throw "$label failed (exit $code); see $log" }
    return @{ step=$step; text=$text }
}
Push-Location $repo
try {
    Write-Host '[dev-test] Development subset only; no G0 stamp or live/release certification.'
    if($lua.Count) {
        [void](Run-Step 'Build Lua runner' 'cargo' @('build','--locked','-p','hsmp-tools','--bin','hsmp-tools'))
        $target=if($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
        $tools=Join-Path $target 'debug\hsmp-tools.exe'
        $luaPath=Join-Path $Out 'lua.json'
        $arguments=@('lua-test','--json',$luaPath)+$lua.ToArray()
        [void](Run-Step 'Selected Lua suites' $tools $arguments)
        $luaReport=Get-Content -LiteralPath $luaPath -Raw | ConvertFrom-Json
        if(@($luaReport.suites).Count -ne $lua.Count -or $luaReport.suites_failed -ne 0) { throw 'Lua selection did not complete exactly the requested suites.' }
        foreach($suite in $luaReport.suites) {
            if($suite.assertions_passed -le 0 -or $suite.assertions_failed -ne 0 -or -not $lua.Contains([string]$suite.name)) { throw "Lua suite $($suite.name) did not run nonzero successful assertions." }
        }
    }
    foreach($arguments in $rust) {
        $result=Run-Step ('Rust '+($arguments -join ' ')) 'cargo' (@('test','--locked')+$arguments)
        $matches=[regex]::Matches($result.text,'(?m)^test result: ok\. (\d+) passed; (\d+) failed;')
        $passed=0
        foreach($match in $matches) { $passed += [int]$match.Groups[1].Value }
        $result.step.rust_tests_passed=$passed
        if($passed -le 0) { throw 'Rust selection ran zero passing tests; correct the filter before trusting this check.' }
        $totalRust += $passed
    }
} catch { $failed=$true; $failure=$_.Exception.Message }
finally {
    Pop-Location
    Save-Json (Join-Path $Out 'report.json') @{ evidence_level='focused development subset (offline)'; domains=$Domain;
        verdict=$(if($failed){'FAIL'}else{'PASS'}); error=$failure; steps=$steps.ToArray(); seconds=$timer.Elapsed.TotalSeconds;
        rust_tests_passed=$totalRust; lua=$luaReport; full_g0=$false; release_gate_pass=$false; live_game_pass=$false }
}
if($failed) { Write-Host "[dev-test] FAIL: $failure"; exit 1 }
$suiteCount=if($luaReport) { $luaReport.suites_passed } else { 0 }
Write-Host ('[dev-test] PASS: {0} Lua suites, {1} Rust tests in {2:N1}s. Development subset only. {3}\report.json' -f $suiteCount,$totalRust,$timer.Elapsed.TotalSeconds,$Out)

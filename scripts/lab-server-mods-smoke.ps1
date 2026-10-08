#Requires -Version 5.1
<#
.SYNOPSIS
    Create repeatable server-mod fixtures and check a real dedicated server offline.
.DESCRIPTION
    No game, sidecar, OS input or career files. Tests manifest refusals, lobby RCON
    configuration and UDP browser fields. B1/B15 backend evidence is separate from
    game acceptance. Fixtures use two 10 MiB data files: one 20 MiB file exceeds the
    protocol's 16 MiB per-file limit. Output is always fresh and never removed.
.EXAMPLE
    .\scripts\lab-server-mods-smoke.ps1
#>
[CmdletBinding()]
param([string]$BinDir='', [string]$Out='', [switch]$FixturesOnly)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'lib\hsmp_runs.ps1')
$utf8 = New-Object Text.UTF8Encoding $false
if (-not $BinDir) { $BinDir = Join-Path $repo 'target\release' }
if (-not $Out) { $Out = Join-Path $repo ('test-results\modes-smoke-' + [Guid]::NewGuid().ToString('N').Substring(0,8)) }
if (Test-Path -LiteralPath $Out) { throw 'Output directory exists; choose a fresh path.' }
New-Item -ItemType Directory -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
function Write-Text([string]$path,[string]$text) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $path) | Out-Null
    [IO.File]::WriteAllText($path,$text,$utf8)
}
function Save-Json([string]$path,$value) { Write-Text $path (ConvertTo-Json -InputObject $value -Depth 10) }
$valid = Join-Path $Out 'fixtures\valid'
Write-Text (Join-Path $valid 'TestBanner\mod.json') '{ "version": "1.0", "author": "QA", "description": "Prints, ticks and binds F8" }'
Write-Text (Join-Path $valid 'TestBanner\Scripts\main.lua') @'
local helper = require("helper")
print("[TestBanner] loaded " .. helper.tag)
LoopAsync(5000, function() print("[TestBanner] tick") return false end)
RegisterKeyBind(Key.F8, function() print("[TestBanner] F8") end)
function OnUnload() print("[TestBanner] unloaded") end
'@
Write-Text (Join-Path $valid 'TestBanner\Scripts\helper.lua') 'return { tag = "helper ok" }'
Write-Text (Join-Path $valid 'BigData\Scripts\main.lua') 'print("[BigData] loaded")'
$block = $utf8.GetBytes(('QA blob data.' + "`n").PadRight(1024,'x'))
foreach ($name in @('blob-a.txt','blob-b.txt')) {
    $file = [IO.File]::Open((Join-Path $valid "BigData\Scripts\$name"),[IO.FileMode]::CreateNew)
    try { for ($i=0;$i -lt 10240;$i++) { $file.Write($block,0,$block.Length) } } finally { $file.Dispose() }
}
$invalid = @(
    @{ name='reserved-name'; file='HSMPEvil\Scripts\main.lua'; text='print("forbidden")'; reason='mod name "HSMPEvil".*reserved' },
    @{ name='binary-file'; file='BadBinary\Scripts\main.lua'; text='print("forbidden")'; extra='BadBinary\Scripts\bad.dll'; reason='mod BadBinary: path "Scripts/bad.dll".*not allowed' },
    @{ name='missing-main'; file='MissingMain\Scripts\helper.lua'; text='return {}'; reason='mod MissingMain: Scripts/main.lua is missing' }
)
foreach ($case in $invalid) {
    $dir = Join-Path $Out ('fixtures\' + $case.name)
    Write-Text (Join-Path $dir $case.file) $case.text
    if ($case.extra) { Write-Text (Join-Path $dir $case.extra) 'This is inert text with a refused extension.' }
}
Write-Text (Join-Path $Out 'fixtures\load-error\ErrorMod\Scripts\main.lua') 'error("boom")'
Save-Json (Join-Path $Out 'fixtures.json') @{ valid=$valid; load_error=(Join-Path $Out 'fixtures\load-error'); note='Data payload 20 MiB total in two files below the 16 MiB per-file ceiling.' }
if ($FixturesOnly) { Write-Host "Fixtures ready: $Out\fixtures.json"; exit 0 }
$server = Join-Path $BinDir 'hsmp-server.exe'; $query = Join-Path $BinDir 'hsmp-query.exe'
foreach ($bin in @($server,$query)) { if (-not (Test-Path -LiteralPath $bin)) { throw "Missing binary: $bin; build hsmp-server and hsmp-query first." } }
$checks = New-Object Collections.Generic.List[object]
$started = New-Object Collections.Generic.List[object]
$password = [Guid]::NewGuid().ToString('N')
$ports = Hsmp-FreePorts 2
function Start-Server([string]$tag,[string]$mods,[bool]$rcon) {
    $cwd = Join-Path $Out $tag; New-Item -ItemType Directory -Path $cwd | Out-Null
    $arguments = @('--bind',"127.0.0.1:$($ports[0])",'--mods-dir',('"' + $mods + '"'),'--key-file',('"' + (Join-Path $cwd 'identity.key') + '"'),'--bans-file',('"' + (Join-Path $cwd 'bans.txt') + '"'),'--log-dir',('"' + (Join-Path $cwd 'logs') + '"'))
    if ($rcon) { $arguments += @('--rcon-bind',"127.0.0.1:$($ports[1])",'--rcon-password',$password,'--debug-verbs','--events',('"' + (Join-Path $cwd 'events.jsonl') + '"')) }
    $p = Start-Process -FilePath $server -ArgumentList $arguments -WorkingDirectory $cwd -WindowStyle Hidden -RedirectStandardOutput (Join-Path $cwd 'stdout.log') -RedirectStandardError (Join-Path $cwd 'stderr.log') -PassThru
    $started.Add((Hsmp-ProcRecord $p $tag)); return $p
}
function Rcon([string]$command) {
    $client = New-Object Net.Sockets.TcpClient
    try {
        $task=$client.ConnectAsync('127.0.0.1',[int]$ports[1]); if (-not $task.Wait(1500)) { throw 'RCON connection timeout.' }
        $stream=$client.GetStream(); $stream.ReadTimeout=3000; $stream.WriteTimeout=3000
        $reader=New-Object IO.StreamReader($stream); $writer=New-Object IO.StreamWriter($stream); $writer.AutoFlush=$true
        $writer.WriteLine('AUTH ' + $password); if ($reader.ReadLine() -notmatch '^OK') { throw 'RCON authentication failed.' }
        $writer.WriteLine($command); $reply=$reader.ReadLine()
        [IO.File]::AppendAllText((Join-Path $Out 'rcon.jsonl'), ((@{ command=$command; reply=$reply } | ConvertTo-Json -Compress) + "`n"), $utf8)
        return $reply
    } finally { $client.Dispose() }
}
function Check([string]$id,[bool]$ok,[string]$detail) {
    $checks.Add(@{ id=$id; verdict=$(if ($ok) {'PASS'} else {'FAIL'}); detail=$detail })
    if (-not $ok) { throw "${id}: $detail" }
}
$failed=$false
try {
    $build=@(& $server --build-info); if ($LASTEXITCODE -ne 0) { throw 'Server build-info failed.' }
    Write-Text (Join-Path $Out 'build-info.json') ($build -join "`n")
    foreach ($case in $invalid) {
        $p=Start-Server $case.name (Join-Path $Out ('fixtures\'+$case.name)) $false
        if (-not $p.WaitForExit(10000)) { throw "Invalid fixture $($case.name) did not refuse startup." }
        $log=(Get-Content -LiteralPath (Join-Path $Out ($case.name+'\stderr.log')) -Raw) + (Get-Content -LiteralPath (Join-Path $Out ($case.name+'\stdout.log')) -Raw)
        Check ('B15-backend-'+$case.name) ($p.ExitCode -ne 0 -and $log -notmatch 'unexpected argument|Usage:' -and $log -match $case.reason) 'Dedicated server refuses the invalid fixture with a useful reason.'
    }
    $p=Start-Server 'valid-server' $valid $true
    $timer=[Diagnostics.Stopwatch]::StartNew(); $s=$null
    while (-not $s) {
        if ($p.HasExited) { throw 'Valid mod server exited; see valid-server logs.' }
        try { $reply=Rcon 'STATUS'; if ($reply -match '^OK ') { $s=$reply.Substring(3) | ConvertFrom-Json } } catch { }
        if ($timer.Elapsed.TotalSeconds -ge 15) { throw 'Valid mod server did not become ready.' }
        if (-not $s) { Start-Sleep -Milliseconds 100 }
    }
    Check 'mods-valid-startup' ($s.phase -eq 'Lobby') 'Two valid mods including 20 MiB of data start successfully.'
    foreach ($mode in @('duel','ffa','teams','koth','roulette','brawl','deathmatch')) {
        $r=Rcon "MODE $mode"; Check ('A16-backend-mode-'+$mode) ($r -match '^OK') 'Lobby mode command accepted.'
        $s=(Rcon 'STATUS').Substring(3) | ConvertFrom-Json
        Check ('A16-backend-status-'+$mode) ($s.mode -eq $mode) 'STATUS reports the selected mode.'
        $tsv=@(& $query --direct "127.0.0.1:$($ports[0])" --timeout-ms 1000)
        Write-Text (Join-Path $Out ('query-'+$mode+'.tsv')) ($tsv -join "`n")
        $row=@($tsv | Where-Object { $_ -match '^S\t' })
        if ($row.Count -ne 1) { throw "Browser query did not return one server for $mode." }
        $fields=$row[0] -split "`t"
        Check ('B1-backend-query-'+$mode) ($fields[20] -eq '2' -and [int]$fields[21] -ge 20480) 'UDP browser advertises two mods and the complete payload size.'
        $labels=@{ duel='Duel'; ffa='Free for all'; teams='Team elimination'; koth='King of the hill'; roulette='Weapon roulette'; brawl='Brawl'; deathmatch='Deathmatch' }
        Check ('A2-backend-query-'+$mode) ($fields[5] -eq $labels[$mode]) ('Browser mode label: '+$fields[5])
    }
    foreach ($c in @('TEAMS auto 3','ROUNDTIME 180','OPTION koth_target 90','OPTION respawn 5','OPTION ff on')) { Check ('A16-backend-'+($c -replace ' ','-')) ((Rcon $c) -match '^OK') 'Lobby mode option accepted.' }
    $s=(Rcon 'STATUS').Substring(3) | ConvertFrom-Json
    Save-Json (Join-Path $Out 'status.json') $s
    Check 'A16-backend-options' ($s.teams -eq 3 -and $s.round_time_s -eq 180 -and $s.koth_target_s -eq 90 -and $s.respawn_s -eq 5 -and $s.friendly_fire) 'STATUS agrees with all configured options.'
    [void](Rcon 'SHUTDOWN'); if (-not $p.WaitForExit(5000)) { throw 'Server did not shut down cleanly.' }
} catch { $failed=$true; $checks.Add(@{ id='runner'; verdict='FAIL'; detail=$_.Exception.Message }) }
finally {
    foreach ($record in $started) { [void](Hsmp-StopRecord $record) }
    $ids=@(1..18 | ForEach-Object { "A$_" }) + @(1..19 | ForEach-Object { "B$_" }) + @('C1','C2')
    $acceptance=$ids | ForEach-Object { @{ id=$_; verdict='NOT RUN'; detail='Offline backend smoke only; live plan acceptance is not certified by this report.' } }
    Save-Json (Join-Path $Out 'report.json') @{ evidence_level='real dedicated server, TCP RCON and UDP browser (no game)'; checks=$checks.ToArray(); acceptance=$acceptance; binaries=@(Get-FileHash -Algorithm SHA256 -LiteralPath $server,$query | Select-Object Path,Hash); release_gate_pass=$false }
}
Write-Host "Server-mod backend evidence: $Out\report.json. Live acceptance remains NOT RUN."
if ($failed) { exit 1 }

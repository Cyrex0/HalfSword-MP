#Requires -Version 5.1
<#
.SYNOPSIS
    Runs the actual deploy script against a private scratch repository/game and
    tiny fake Rust executables under Windows PowerShell 5.1. No real game,
    config, Cargo build, G0, native module or user save is touched.
#>
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$fixture = Join-Path $workspace ('test-results/dev-feature-checks/deploy-content-' + [Guid]::NewGuid().ToString('N'))
$repo = Join-Path $fixture 'fixture repo'
$caller = Join-Path $fixture 'caller'
$bin = Join-Path $fixture 'fake bin'
$target = Join-Path $repo 'target'
$game = Join-Path $repo 'game'
$win64 = Join-Path $game 'HalfswordUE5/Binaries/Win64'
$ship = Join-Path $win64 'hsmp'
$localAppData = Join-Path $repo 'local-appdata'
$wrapper = Join-Path $repo 'scripts/build-and-deploy.ps1'
$lua = Join-Path $repo 'mods/HSMPFixture/Scripts/main.lua'
$sourceId = Join-Path $repo 'source-content.txt'
$callLog = Join-Path $fixture 'calls.jsonl'
$utf8 = New-Object Text.UTF8Encoding $false
$git = (Get-Command git -CommandType Application | Select-Object -First 1).Source
$shell = Join-Path $env:WINDIR 'System32/WindowsPowerShell/v1.0/powershell.exe'
$csc = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
foreach ($dir in @($caller,$bin,(Split-Path $wrapper),(Split-Path $lua),(Join-Path $win64 'ue4ss/Mods'),$localAppData)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
Copy-Item -LiteralPath (Join-Path $workspace 'scripts/build-and-deploy.ps1') -Destination $wrapper
[IO.File]::WriteAllText((Join-Path $repo '.gitignore'), "game/`ntarget/`nlocal-appdata/`nsource-content.txt`n", $utf8)
[IO.File]::WriteAllText((Join-Path $repo 'mods/mods.release.txt'), "HSMPFixture : 1`n", $utf8)
[IO.File]::WriteAllText($lua, "return 'first fixture'`n", $utf8)
[IO.File]::WriteAllText($sourceId, ('a' * 64), $utf8)

# The fake CLI implements the real --build-info / --mods-identity boundary.
# Its compiled ID stays fixed even when the source identity changes; it does
# not duplicate the production content-hashing algorithm.
$source = @'
using System;
using System.IO;
using System.Diagnostics;
using System.Web.Script.Serialization;
class DeployFixture {
    const char Built = 'VERSION';
    static int Main(string[] args) {
        var json = new JavaScriptSerializer();
        string image = Process.GetCurrentProcess().MainModule.FileName;
        string name = Path.GetFileNameWithoutExtension(image);
        File.AppendAllText(Environment.GetEnvironmentVariable("HSMP_DEPLOY_CALLS"),
            json.Serialize(new { image=image,args=args,cwd=Directory.GetCurrentDirectory() })+"\n");
        if(name=="cargo") {
            int code=int.Parse(Environment.GetEnvironmentVariable("HSMP_DEPLOY_BUILD_EXIT")??"0");
            if(code!=0) return code;
            string output=Path.Combine(Environment.GetEnvironmentVariable("CARGO_TARGET_DIR"),"release");
            Directory.CreateDirectory(output);
            foreach(string file in new[]{"hsmp-server.exe","hsmp-sidecar.exe","hsmp-master.exe","hsmp-query.exe"})
                File.Copy(Environment.GetEnvironmentVariable("HSMP_DEPLOY_TEMPLATE"),Path.Combine(output,file),true);
            return 0;
        }
        if(args.Length==0) return 41;
        string mode=Environment.GetEnvironmentVariable("HSMP_DEPLOY_BAD_ID");
        if(name=="hsmp-sidecar" && args[0]=="--build-info") {
            if(mode=="malformed") { Console.WriteLine("not json"); return 0; }
            if(mode=="missing-hash") { Console.WriteLine("{\"version\":\"fixture\"}"); return 0; }
            if(mode=="unavailable") return 23;
        }
        string content = args[0]=="--build-info" ? new string(Built,64) : File.ReadAllText(Path.Combine(args[1],"source-content.txt")).Trim();
        if(args[0]=="--mods-identity-lua") {
            Console.WriteLine("return {content_hash=\""+content+"\",version=\"fixture\",protocol=12,abi=1,layout_hash=1}");
        } else {
            Console.WriteLine(json.Serialize(new {content_hash=content,version="fixture",protocol=12,abi=1,layout_hash=1}));
        }
        return 0;
    }
}
'@
$first = Join-Path $fixture 'fixture-a.exe'
$second = Join-Path $fixture 'fixture-b.exe'
foreach ($row in @(@('a',$first),@('b',$second))) {
    $cs = Join-Path $fixture ('fixture-' + $row[0] + '.cs')
    [IO.File]::WriteAllText($cs, $source.Replace('VERSION',$row[0]), $utf8)
    & $csc /nologo /r:System.Web.Extensions.dll ('/out:' + $row[1]) $cs
    if ($LASTEXITCODE -ne 0) { throw 'Fake native fixture compilation failed.' }
}
Copy-Item -LiteralPath $first -Destination (Join-Path $bin 'cargo.exe')
function Git([string[]]$Arguments) {
    $previous = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { & $git -C $repo @Arguments 2>$null | Out-Null; $code = $LASTEXITCODE }
    finally { $ErrorActionPreference = $previous }
    if ($code -ne 0) { throw ('Fixture git failed: ' + ($Arguments -join ' ')) }
}
Git @('init','--quiet')
Git @('add','.')
Git @('-c','user.name=HSMP fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','baseline')
function Literal([string]$Value) { "'" + $Value.Replace("'","''") + "'" }
function Run-Deploy([string[]]$Arguments,[string]$Template=$second,[string]$BadIdentity='',[int]$BuildExit=0) {
    $parameters = @($Arguments | ForEach-Object {
        if ($_ -in @('-GamePath','-SkipG0','-SkipNative','-SkipBuild','-BinDir','-DryRun')) { $_ }
        else { Literal $_ }
    }) -join ' '
    $command = '$ProgressPreference="SilentlyContinue"; Set-Location -LiteralPath ' + (Literal $caller) +
        '; & ' + (Literal $wrapper) + ' ' + $parameters + '; exit $LASTEXITCODE'
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $shell
    $start.Arguments = '-NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    $start.WorkingDirectory = $caller
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    # Restrict child lookup so fake cargo cannot be confused with real Cargo.
    $start.EnvironmentVariables['PATH'] = $bin + ';' + (Split-Path $git) + ';' + (Join-Path $env:WINDIR 'System32')
    $start.EnvironmentVariables['LOCALAPPDATA'] = $localAppData
    $start.EnvironmentVariables['CARGO_TARGET_DIR'] = $target
    $start.EnvironmentVariables['HSMP_DEPLOY_TEMPLATE'] = $Template
    $start.EnvironmentVariables['HSMP_DEPLOY_CALLS'] = $callLog
    $start.EnvironmentVariables['HSMP_DEPLOY_BAD_ID'] = $BadIdentity
    $start.EnvironmentVariables['HSMP_DEPLOY_BUILD_EXIT'] = [string]$BuildExit
    $p = New-Object Diagnostics.Process
    $p.StartInfo = $start
    if (-not $p.Start()) { throw 'Fixture deploy failed to start.' }
    $stdout = $p.StandardOutput.ReadToEndAsync()
    $stderr = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(20000)) { throw 'Bounded fake deploy timed out.' }
    $text = $stdout.Result + $stderr.Result
    [IO.File]::WriteAllText((Join-Path $fixture ('child-' + $p.Id + '.log')), $text, $utf8)
    return [pscustomobject]@{code=$p.ExitCode;text=$text}
}
function Snapshot {
    # Include empty/new directories and file timestamps as well as bytes, so an
    # early refusal cannot pass after rewriting identical game/config content.
    return (@(foreach ($root in @($game,$localAppData)) {
        Get-ChildItem -LiteralPath $root -Recurse -Force | Sort-Object FullName | ForEach-Object {
            if ($_.PSIsContainer) { 'dir|' + $_.FullName }
            else { $_.FullName + '|' + $_.LastWriteTimeUtc.Ticks + '|' + (Get-FileHash -LiteralPath $_.FullName).Hash }
        }
    }) -join "`n")
}
$script:checks = 0
function Check([bool]$Pass,[string]$Name) {
    if (-not $Pass) { throw ('FAIL ' + $Name + '; fixture=' + $fixture) }
    $script:checks++
    Write-Output ('PASS ' + $Name)
}
function Stamp { Get-Content -Raw -LiteralPath (Join-Path $win64 'hsmp_deploy.json') | ConvertFrom-Json }
$base = @('-GamePath',$game,'-SkipG0','-SkipNative')
$skip = $base + '-SkipBuild'

$result = Run-Deploy $base $first
Check ($result.code -eq 0 -and (Stamp).bins_match_commit) 'matching normal build completes actual deploy and stamps its binaries'
[IO.File]::WriteAllText($lua, "return 'Lua-only update'`n", $utf8)
[IO.File]::WriteAllText($sourceId, ('b' * 64), $utf8)
Git @('add','mods')
Git @('-c','user.name=HSMP fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','Lua-only change')
$before = Snapshot
$result = Run-Deploy $skip
Check ($result.code -eq 1 -and $result.text.Contains('content does not match') -and $result.text.Contains('rebuild without -SkipBuild')) 'Lua-only stale binaries refuse with rebuild guidance'
Check ((Snapshot) -ceq $before) 'stale refusal precedes all game/config writes, backups, and copies'

$result = Run-Deploy $base
Check ($result.code -eq 0 -and (Get-Content -Raw -LiteralPath (Join-Path $win64 'ue4ss/Mods/HSMPFixture/Scripts/main.lua')).Contains('Lua-only update') -and (Stamp).bins_match_commit) 'fresh normal build resolves Lua-only mismatch and deploys new source'
$result = Run-Deploy $skip
Check ($result.code -eq 0 -and (Stamp).bins_match_commit -and (Stamp).skip_build) 'unchanged matching SkipBuild still completes and retains binary provenance'

foreach ($case in @(@('malformed','malformed'),@('missing-hash','valid content hash'),@('unavailable','unavailable (exit 23)'))) {
    $before = Snapshot
    $result = Run-Deploy $skip -BadIdentity $case[0]
    Check ($result.code -eq 1 -and $result.text.Contains($case[1]) -and (Snapshot) -ceq $before) ($case[0] + ' identity refuses before game/config mutation')
}
Copy-Item -LiteralPath $first -Destination (Join-Path $ship 'hsmp-server.exe') -Force
$before = Snapshot
$result = Run-Deploy $skip
Check ($result.code -eq 1 -and $result.text.Contains('content does not match') -and (Snapshot) -ceq $before) 'compiled server must match even when sidecar and source match'
Copy-Item -LiteralPath $second -Destination (Join-Path $ship 'hsmp-server.exe') -Force
$before = Snapshot
$result = Run-Deploy $base -BuildExit 17
Check ($result.code -eq 1 -and $result.text.Contains('cargo build failed (exit 17)') -and (Snapshot) -ceq $before) 'failed normal build refuses before game/config mutation'

# A commit touching an unshipped development Lua file leaves the release
# content identity unchanged, but must invalidate the binary-input git claim.
$devLua = Join-Path $repo 'mods/dev/HSMPUnshipped/Scripts/main.lua'
New-Item -ItemType Directory -Force -Path (Split-Path $devLua) | Out-Null
[IO.File]::WriteAllText($devLua, "return 'unshipped development fixture'`n", $utf8)
Git @('add','mods')
Git @('-c','user.name=HSMP fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','mod binary input')
$result = Run-Deploy $skip
Check ($result.code -eq 0 -and -not (Stamp).bins_match_commit) 'mods-only git change cannot retain bins_match_commit from an older build'

# Relative bin_dir is read by the game relative to Win64. Caller-cwd decoys
# deliberately have the opposite identity, proving we inspect the real path.
$relative = 'relative bins'
$actual = Join-Path $win64 $relative
$decoy = Join-Path $caller $relative
New-Item -ItemType Directory -Force -Path $actual,$decoy | Out-Null
foreach ($name in @('hsmp-server.exe','hsmp-sidecar.exe','hsmp-master.exe','hsmp-query.exe')) {
    Copy-Item -LiteralPath $second -Destination (Join-Path $actual $name)
    Copy-Item -LiteralPath $first -Destination (Join-Path $decoy $name)
}
$result = Run-Deploy ($skip + @('-BinDir',$relative))
Check ($result.code -eq 0 -and (Stamp).bin_dir -ceq $relative -and (Test-Path -LiteralPath (Join-Path $actual 'build.json')) -and -not (Test-Path -LiteralPath (Join-Path $decoy 'build.json'))) 'relative BinDir verifies and writes identity in Win64 while preserving config spelling'
Copy-Item -LiteralPath $first -Destination (Join-Path $actual 'hsmp-sidecar.exe') -Force
Copy-Item -LiteralPath $second -Destination (Join-Path $decoy 'hsmp-sidecar.exe') -Force
$before = Snapshot
$result = Run-Deploy ($skip + @('-BinDir',$relative))
Check ($result.code -eq 1 -and $result.text.Contains('content does not match') -and (Snapshot) -ceq $before) 'stale relative BinDir refuses despite a matching caller-cwd decoy'
$missing = Join-Path $win64 'missing sidecar'
New-Item -ItemType Directory -Force -Path $missing | Out-Null
Copy-Item -LiteralPath $second -Destination (Join-Path $missing 'hsmp-server.exe')
$before = Snapshot
$result = Run-Deploy ($skip + @('-BinDir',$missing))
Check ($result.code -eq 1 -and $result.text.Contains('compiled sidecar identity unavailable') -and (Snapshot) -ceq $before) 'missing sidecar executable refuses before writes'
$before = Snapshot
$result = Run-Deploy ($skip + '-DryRun') -BadIdentity malformed
Check ($result.code -eq 0 -and (Snapshot) -ceq $before) 'DryRun remains read-only even with unavailable compiled compatibility'
$calls = Get-Content -LiteralPath $callLog | ForEach-Object { ConvertFrom-Json $_ }
$builds = @($calls | Where-Object { [IO.Path]::GetFileName($_.image) -eq 'cargo.exe' })
Check ($builds.Count -eq 3 -and @($builds | Where-Object { ($_.args -join '|') -cne 'build|--release|--locked|-p|hsmp-server' -or $_.cwd -cne $repo }).Count -eq 0) 'normal build uses the exact locked package command; SkipBuild never invokes Cargo'
Write-Output ('Deploy content selftest: ' + $checks + ' checks passed; fixture=' + $fixture)

#Requires -Version 5.1
<#
.SYNOPSIS
    Build the current lab controller and forward its CLI arguments.
.EXAMPLE
    .\scripts\lab.ps1 session --dir test-results/lab/manual --seconds 600
.EXAMPLE
    .\scripts\lab.ps1 --help
#>
param(
    [Parameter(ValueFromRemainingArguments=$true)]
    [AllowEmptyCollection()]
    [AllowEmptyString()]
    [string[]]$LabArguments=@()
)
$ErrorActionPreference='Stop'
$labRepo=Split-Path -Parent $PSScriptRoot

function ConvertTo-LabNativeArgument([string]$Value) {
    # Windows native argv: preserve empty arguments, quotes, whitespace and
    # backslashes, including the backslashes before the closing quote.
    $quoted=[regex]::Replace($Value,'(\\*)"','$1$1\"')
    $quoted=[regex]::Replace($quoted,'(\\+)$','$1$1')
    return '"'+$quoted+'"'
}
function Invoke-LabNative([string]$Exe,[string[]]$Arguments,[string]$WorkingDirectory,[switch]$Capture) {
    $start=New-Object Diagnostics.ProcessStartInfo
    $start.FileName=$Exe
    $start.Arguments=(@($Arguments | ForEach-Object { ConvertTo-LabNativeArgument $_ }) -join ' ')
    $start.WorkingDirectory=$WorkingDirectory
    $start.UseShellExecute=$false
    $start.CreateNoWindow=$true
    if($Capture) { $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true }
    $process=New-Object Diagnostics.Process
    $process.StartInfo=$start
    try {
        if(-not $process.Start()) { throw 'Lab controller process could not start.' }
        if($Capture) {
            # Drain both pipes concurrently while Cargo builds; a full stderr
            # pipe must not block the compiler/artifact stream.
            $stdout=$process.StandardOutput.ReadToEndAsync()
            $stderr=$process.StandardError.ReadToEndAsync()
        }
        $process.WaitForExit()
        if($Capture) {
            return [pscustomobject]@{Code=$process.ExitCode;Output=$stdout.GetAwaiter().GetResult();Error=$stderr.GetAwaiter().GetResult()}
        }
        return $process.ExitCode
    }
    finally { $process.Dispose() }
}

$cargo=(Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
Write-Host '[lab] Building the current controller.'
$build=Invoke-LabNative $cargo @('build','--release','--locked','-p','hsmp-tools','--bin','hsmp-lab','--message-format=json-render-diagnostics') $labRepo -Capture
if($build.Error) { Write-Host $build.Error }
$artifacts=@()
foreach($line in ($build.Output -split '\r?\n')) {
    if(-not $line) { continue }
    try { $message=$line | ConvertFrom-Json -ErrorAction Stop }
    catch { if($build.Code -ne 0) { continue }; throw 'Cargo returned an unreadable build/artifact record.' }
    if($message.reason -eq 'compiler-message' -and $message.message.rendered) { Write-Host $message.message.rendered }
    if($message.reason -eq 'compiler-artifact' -and $message.target.name -ceq 'hsmp-lab' -and
        $message.target.kind -contains 'bin' -and $message.executable) {
        $artifacts+= [string]$message.executable
    }
}
if($build.Code -ne 0) { exit $build.Code }
if($artifacts.Count -ne 1 -or -not [IO.Path]::IsPathRooted($artifacts[0]) -or
    -not (Test-Path -LiteralPath $artifacts[0] -PathType Leaf)) {
    throw 'Lab build finished without one proven controller executable.'
}
$labBinary=$artifacts[0]

if($LabArguments.Count -gt 0 -and $LabArguments[0] -ceq 'session') {
    # A running Windows owner locks its executable image. Each session keeps
    # its own ignored copy so later Cargo builds can replace the build output.
    $snapshotRoot=Join-Path $labRepo 'test-results/lab/tool-snapshots'
    New-Item -ItemType Directory -Path $snapshotRoot -Force | Out-Null
    $labOwner=Join-Path $snapshotRoot ('hsmp-lab-'+[Guid]::NewGuid().ToString('N')+'.exe')
    Copy-Item -LiteralPath $labBinary -Destination $labOwner -ErrorAction Stop
    $labBinary=$labOwner
}
$labCode=Invoke-LabNative $labBinary $LabArguments (Get-Location).Path
exit $labCode

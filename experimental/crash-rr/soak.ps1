# The game's Win64 folder: -Win64, else $env:HSMP_GAME_DIR, else <repo>\game.
param([int]$Target = 40, [string]$Gate = "0", [string]$Tag = "A", [int]$MaxBoots = 12,
      [string]$Win64 = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($(if ($env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR } else { [System.IO.Path]::Combine($PSScriptRoot, "..\..\game") }), "HalfswordUE5\Binaries\Win64")))
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$sf = Join-Path $Win64 "hsmp_state_crashrr_$Tag\.rvp_stress.txt"
$cdb = "C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe"
$crashes = @()
for ($b = 1; $b -le $MaxBoots; $b++) {
    $done = if (Test-Path $sf) { @(Get-Content $sf).Count } else { 0 }
    if ($done -ge $Target) { break }
    $r = & (Join-Path $here "stress.ps1") -N $Target -Gate $Gate -Tag $Tag -TimeoutS 1800 -Win64 $Win64
    $r | ForEach-Object { Write-Output "[boot $b] $_" }
    $line = $r | Where-Object { $_ -like "RESULT*" }
    if ($line -match 'new_crash_dirs=(\S+)') {
        foreach ($d in ($Matches[1] -split ',')) {
            $dump = Join-Path $env:LOCALAPPDATA "HalfswordUE5\Saved\Crashes\$d\UEMinidump.dmp"
            $sig = "?"
            if (Test-Path $dump) {
                $o = & $cdb -z $dump -c ".ecxr; ? @rip - HalfswordUE5_Win64_Shipping; ~.; q" 2>&1
                $rva = ($o | Select-String 'Evaluate expression: \d+ = ([0-9a-f`]+)' | Select-Object -First 1).Matches
                $thr = ($o | Select-String '^\.\s+\d+\s+Id:.*"([^"]+)"' | Select-Object -First 1).Matches
                $sig = "rva=" + $(if ($rva) { $rva[0].Groups[1].Value } else { "?" }) + " thread=" + $(if ($thr) { $thr[0].Groups[1].Value } else { "?" })
            }
            $at = if (Test-Path $sf) { @(Get-Content $sf).Count } else { 0 }
            $crashes += "$d after_reload=$at $sig"
            Write-Output "CRASH $d after_reload=$at $sig"
        }
    }
    if ($line -match 'result=(timeout|BUSY)') { break }
}
$done = if (Test-Path $sf) { @(Get-Content $sf).Count } else { 0 }
Write-Output "SOAK tag=$Tag gate=$Gate reloads=$done crashes=$($crashes.Count)"
$crashes | ForEach-Object { Write-Output "  $_" }

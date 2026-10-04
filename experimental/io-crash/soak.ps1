# Single-player IO-1 soak: boots the game with only HSMPIoProbe enabled until
# -Target arena loads are marked, and classifies every new crash dump.
#
#   PS> .\experimental\io-crash\soak.ps1 -Mode plain -Target 60 -Tag vanilla
#   PS> .\experimental\io-crash\soak.ps1 -Mode destroy -Target 30 -Tag destroy
#
# Back up SaveGames first (the probe runs single-player). mods.txt and the
# probe folder are put back / removed when the script ends.
param(
    [ValidateSet("plain", "reveal", "armor", "destroy")][string]$Mode = "plain",
    [int]$Target = 60,
    [string]$Tag = "A",
    [string]$Arena = "Map_Arena_Pit",
    [int]$DelayMs = 100,
    [int]$RevealMs = 3000,
    [int]$HideDelayMs = 0,
    [switch]$ViaMenu,
    [string]$Cvars = "",
    # Loads per boot (1 = every trial is the process's first arena load, the only one that crashed).
    [int]$PerBoot = 0,
    [int]$MaxBoots = 40,
    [int]$BootTimeoutS = 1500,
    [string]$Tools = $env:HSMP_TOOLS,
    [string]$Win64 = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($(if ($env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR } else { [System.IO.Path]::Combine($PSScriptRoot, "..\..\game") }), "HalfswordUE5\Binaries\Win64"))
)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = [System.IO.Path]::GetFullPath((Join-Path $here "..\.."))
$exe = Join-Path $Win64 "HalfswordUE5-Win64-Shipping.exe"
$mods = Join-Path $Win64 "ue4ss\Mods"
$modsTxt = Join-Path $mods "mods.txt"
$probeDst = Join-Path $mods "HSMPIoProbe"
$leaf = "hsmp_state_iocrash_$Tag"
$state = Join-Path $Win64 $leaf
$sf = Join-Path $state ".io_stress.txt"
$crashDir = Join-Path $env:LOCALAPPDATA "HalfswordUE5\Saved\Crashes"
$busy = Get-Process | Where-Object { $_.ProcessName -match '(?i)^halfsword' }
if ($busy) { Write-Output "BUSY: $($busy.ProcessName -join ',') running (PIDs $($busy.Id -join ','))"; exit 3 }
New-Item -ItemType Directory -Force $state | Out-Null
function Done { if (Test-Path $sf) { return @(Get-Content $sf).Count } else { return 0 } }

$savedMods = [IO.File]::ReadAllText($modsTxt)
$utf8 = New-Object Text.UTF8Encoding $false
$t0 = [DateTime]::UtcNow
$crashes = @()
try {
    New-Item -ItemType Directory -Force (Join-Path $probeDst "Scripts") | Out-Null
    Copy-Item (Join-Path $here "HSMPIoProbe\Scripts\main.lua") (Join-Path $probeDst "Scripts\main.lua") -Force
    Copy-Item (Join-Path $repo "mods\shared\hsmp_wg.lua") (Join-Path $probeDst "Scripts\hsmp_wg.lua") -Force
    # Only the probe (and UE4SS's keybinds): no HSMP mod runs.
    [IO.File]::WriteAllText($modsTxt, "HSMPIoProbe : 1`r`nKeybinds : 1`r`n", $utf8)
    for ($b = 1; $b -le $MaxBoots -and (Done) -lt $Target; $b++) {
        $before = @(Get-ChildItem -Directory $crashDir | ForEach-Object { $_.Name })
        $vars = @{ HSMP_DEV = "1"; HSMP_STATE_DIR = $leaf; HSMP_IO_STRESS = $(if ($PerBoot -gt 0) { "$([Math]::Min($Target, (Done) + $PerBoot))" } else { "$Target" }); HSMP_IO_MODE = $Mode;
                   HSMP_IO_ARENA = $Arena; HSMP_IO_DELAY_MS = "$DelayMs";
                   HSMP_IO_REVEAL_MS = "$RevealMs"; HSMP_IO_HIDE_DELAY_MS = "$HideDelayMs"; HSMP_IO_VIA_MENU = $(if ($ViaMenu) { "1" } else { "0" }); HSMP_IO_CVARS = $Cvars }
        $saved = @{}
        foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
        try { $p = Start-Process -FilePath $exe -WorkingDirectory $Win64 -PassThru }
        finally { foreach ($k in $vars.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
        $bt = Get-Date
        $start = Done
        $result = "timeout"
        while (((Get-Date) - $bt).TotalSeconds -lt $BootTimeoutS) {
            Start-Sleep -Seconds 5
            $p.Refresh()
            if ($p.HasExited) { $result = "exited"; break }
            if ((Done) -ge $Target -or ($PerBoot -gt 0 -and (Done) -ge $start + $PerBoot)) { Start-Sleep -Seconds 3; $result = "completed"; break }
            # A crashed IoDispatcher leaves the game thread running for a while: a
            # new crash dir means this boot is over.
            $new = @(Get-ChildItem -Directory $crashDir | Where-Object { $before -notcontains $_.Name })
            if ($new.Count -gt 0) { Start-Sleep -Seconds 5; $result = "crash"; break }
        }
        $p.Refresh()
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force; Start-Sleep -Seconds 3 }
        Get-CimInstance Win32_Process -Filter "Name='CrashReportClient.exe'" | Where-Object { $_.ParentProcessId -eq $p.Id } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
        $new = @(Get-ChildItem -Directory $crashDir | Where-Object { $before -notcontains $_.Name } | ForEach-Object { $_.Name })
        Copy-Item (Join-Path $Win64 "ue4ss\UE4SS.log") (Join-Path $state "UE4SS.boot$b.log") -Force -ErrorAction SilentlyContinue
        foreach ($d in $new) { $crashes += "$d boot=$b after_load=$(Done)" }
        Write-Output "[boot $b] result=$result loads=$start->$(Done) new_crash_dirs=$($new -join ',')"
        # A crash before the mark still counts as one trial.
        if ($new.Count -gt 0 -and $PerBoot -gt 0 -and (Done) -lt $start + $PerBoot) { Add-Content $sf "$((Done) + 1) $([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()) $Mode CRASH $($new -join ",")" }
        if ($result -eq "timeout") { break }
    }
} finally {
    [IO.File]::WriteAllText($modsTxt, $savedMods, $utf8)
    Remove-Item -Recurse -Force $probeDst -ErrorAction SilentlyContinue
}
Write-Output "SOAK tag=$Tag mode=$Mode arena=$Arena loads=$(Done) crash_dirs=$($crashes.Count)"
$crashes | ForEach-Object { Write-Output "  $_" }
if ($Tools -and (Test-Path $Tools) -and $crashes.Count -gt 0) {
    & $Tools crash-triage --since $t0.ToString("o") --no-state-update --out (Join-Path $state "crash_triage.json") 2>&1 |
        ForEach-Object { Write-Output "  triage: $_" }
}

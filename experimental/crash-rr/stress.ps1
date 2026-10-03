param(
    [int]$N = 30,
    [string]$Gate = "0",
    [string]$Tag = "A",
    [int]$TimeoutS = 1500,
    [string]$Arena = "Map_Arena_Alley",
    # The game's Win64 folder: $env:HSMP_GAME_DIR, else <repo>\game.
    [string]$Win64 = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($(if ($env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR } else { [System.IO.Path]::Combine($PSScriptRoot, "..\..\game") }), "HalfswordUE5\Binaries\Win64"))
)
$ErrorActionPreference = "Stop"
$Exe = Join-Path $Win64 "HalfswordUE5-Win64-Shipping.exe"
$leaf = "hsmp_state_crashrr_$Tag"
$state = Join-Path $Win64 $leaf
New-Item -ItemType Directory -Force $state | Out-Null
$busy = Get-Process | Where-Object { $_.ProcessName -match '(?i)^halfsword' }
if ($busy) { Write-Output "BUSY: game already running (PIDs $($busy.Id -join ','))"; exit 3 }
$crashDir = Join-Path $env:LOCALAPPDATA "HalfswordUE5\Saved\Crashes"
$before = @(Get-ChildItem -Directory $crashDir | ForEach-Object { $_.Name })
$vars = @{ HSMP_DEV = "1"; HSMP_STATE_DIR = $leaf; HSMP_INST = "9"; HSMP_RVP_STRESS = "$N"; HSMP_RVP_GATE = $Gate; HSMP_RVP_ARENA = $Arena;
           HSMP_TEST_CVARS = "r.VSync=0;t.MaxFPS=60" }
$saved = @{}
foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
try { $p = Start-Process -FilePath $Exe -WorkingDirectory $Win64 -PassThru }
finally { foreach ($k in $vars.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
$t0 = Get-Date
Write-Output "started PID $($p.Id) tag=$Tag gate=$Gate N=$N"
$sf = Join-Path $state ".rvp_stress.txt"
function Done { if (Test-Path $sf) { return @(Get-Content $sf).Count } else { return 0 } }
$result = "timeout"
while (((Get-Date) - $t0).TotalSeconds -lt $TimeoutS) {
    Start-Sleep -Seconds 5
    $p.Refresh()
    if ($p.HasExited) { $result = "exited"; break }
    if ((Done) -ge $N) { Start-Sleep -Seconds 12; $p.Refresh(); if ($p.HasExited) { $result = "exited" } else { $result = "completed" }; break }
}
$n = Done
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force; Start-Sleep -Seconds 3 }
# the crash reporter is a child process: only one started after us whose parent was our PID
Get-CimInstance Win32_Process -Filter "Name='CrashReportClient.exe'" | Where-Object { $_.ParentProcessId -eq $p.Id } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
$after = @(Get-ChildItem -Directory $crashDir | ForEach-Object { $_.Name })
$new = @($after | Where-Object { $before -notcontains $_ })
$elapsed = [int]((Get-Date) - $t0).TotalSeconds
Write-Output "RESULT tag=$Tag gate=$Gate result=$result reloads_marked=$n elapsed_s=$elapsed new_crash_dirs=$($new -join ',')"
Copy-Item (Join-Path $Win64 "ue4ss\UE4SS.log") (Join-Path $state "UE4SS.log") -Force -ErrorAction SilentlyContinue

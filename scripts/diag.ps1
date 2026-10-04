#Requires -Version 5.1
<#
.SYNOPSIS
    Health-check the HSMP install: UE4SS, mods, Rust bins, master connectivity.

.DESCRIPTION
    Reports pass/fail for every piece the stack needs to run. Exits with the number of
    failed checks (0 = nothing critically wrong). Does NOT launch the game and writes nothing.

    The expected mod set comes from mods\mods.release.txt (the template build-and-deploy.ps1
    writes mods.txt from), never from a hard-coded list:
      "Name : 1"   must be deployed (HSMP mods: Scripts\main.lua) and enabled in mods.txt;
      "Name : 0"   must be disabled or absent; an enabled retired HSMP mod is a FAILURE
                   (HSMPLobby & co. are unsafe off the game thread and kill stored PIDs);
      "Name : dev" either (the developer profile turns them on).
    The binaries are looked up where the game runs them: hsmp.cfg bin_dir (relative to Win64).

.PARAMETER GamePath
    Override the game install dir. Default: $env:HSMP_GAME_DIR, <repo>\game, then the
    git worktrees' game\ folders.
#>
[CmdletBinding()]
param(
    [string]$GamePath = ""
)

$ErrorActionPreference = "Continue"
$Repo = Split-Path -Parent $PSScriptRoot
$script:pass = 0
$script:fail = 0

function ok   ([string]$m) { Write-Host "  [+] " -NoNewline -ForegroundColor Green; Write-Host $m; $script:pass++ }
function bad  ([string]$m) { Write-Host "  [X] " -NoNewline -ForegroundColor Red;   Write-Host $m; $script:fail++ }
function warn ([string]$m) { Write-Host "  [!] " -NoNewline -ForegroundColor Yellow; Write-Host $m }
function hdr  ([string]$m) { Write-Host ""; Write-Host "== $m ==" -ForegroundColor Cyan }

if (-not $GamePath) {
    $cands = @()
    if ($env:HSMP_GAME_DIR) { $cands += $env:HSMP_GAME_DIR }
    $cands += (Join-Path $Repo "game")
    $wl = & git -C $Repo worktree list --porcelain 2>$null
    foreach ($l in $wl) { if ($l -like "worktree *") { $cands += (Join-Path ($l.Substring(9).Trim()) "game") } }
    foreach ($c in $cands) { if (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64")) { $GamePath = $c; break } }
    if (-not $GamePath) { $GamePath = Join-Path $Repo "game" }
}

hdr "Repo layout"
foreach ($d in @("server\src","mods","scripts","docs")) {
    if (Test-Path (Join-Path $Repo $d)) { ok $d } else { bad "missing: $d" }
}

$g = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
hdr "UE4SS install at $GamePath"
if (-not (Test-Path $g)) {
    bad "game binary dir not found: $g"
} else {
    foreach ($f in @("dwmapi.dll","HalfswordUE5-Win64-Shipping.exe","ue4ss\ue4ss.dll","ue4ss\UE4SS-settings.ini","ue4ss\Mods\mods.txt")) {
        $p = Join-Path $g $f
        if (Test-Path $p) { ok $f } else { bad "missing: $f" }
    }
}

# --- hsmp.cfg: where the game runs the binaries from -----------------------------------------
$cfgPath = Join-Path $g "hsmp.cfg"
$binDir = $null
$masterUrl = "http://127.0.0.1:7778"
if (Test-Path $cfgPath) {
    foreach ($l in (Get-Content $cfgPath)) {
        if ($l -match '^\s*bin_dir\s*=\s*"?([^"#;]+?)"?\s*([#;].*)?$') { $binDir = $Matches[1].Trim() }
        if ($l -match '^\s*master_url\s*=\s*([^#;]+)') { $masterUrl = (($Matches[1] -split ',')[0]).Trim() }
    }
}
hdr "Rust binaries"
if (-not $binDir) {
    bad "hsmp.cfg has no bin_dir ($cfgPath): run scripts\build-and-deploy.ps1"
} else {
    $abs = if ([IO.Path]::IsPathRooted($binDir)) { $binDir } else { Join-Path $g $binDir }
    foreach ($b in @("hsmp-server.exe","hsmp-sidecar.exe","hsmp-master.exe","hsmp-query.exe")) {
        $p = Join-Path $abs $b
        if (Test-Path $p) { ok "$b ($([math]::Round((Get-Item $p).Length / 1MB, 1)) MB) in $binDir" }
        else { bad "$b not found in bin_dir $abs; run scripts\build-and-deploy.ps1" }
    }
}

# --- the release template ----------------------------------------------------------------------
$tplPath = Join-Path $Repo "mods\mods.release.txt"
$tpl = [ordered]@{}
if (Test-Path $tplPath) {
    foreach ($line in (Get-Content $tplPath)) {
        $t = $line.Trim()
        if (-not $t -or $t.StartsWith("#") -or $t.StartsWith(";")) { continue }
        if ($t -match '^([^:\s]+)\s*:\s*(0|1|dev)\s*$') { $tpl[$Matches[1]] = $Matches[2] }
    }
} else { bad "release template missing: $tplPath" }

$ModsDst = Join-Path $g "ue4ss\Mods"
hdr "HSMP mods deployed (from mods.release.txt)"
foreach ($m in $tpl.Keys) {
    if ($m -notlike "HSMP*" -or $tpl[$m] -ne "1") { continue }
    $p = Join-Path $ModsDst "$m\Scripts\main.lua"
    if (Test-Path $p) { ok "$m ($((Get-Item $p).Length) B)" }
    else { bad "$m missing - run scripts\build-and-deploy.ps1 -SkipBuild" }
}

hdr "mods.txt entries"
$ModsTxt = Join-Path $ModsDst "mods.txt"
if (Test-Path $ModsTxt) {
    $state = @{}
    foreach ($line in (Get-Content $ModsTxt)) {
        if ($line -match '^\s*([^:;\s]+)\s*:\s*(\d+)\s*$') { $state[$Matches[1]] = $Matches[2] }
    }
    foreach ($m in $tpl.Keys) {
        $want = $tpl[$m]
        $have = $state[$m]
        switch ($want) {
            "1" {
                if ($have -eq "1") { ok "$m : 1" }
                elseif ($null -ne $have) { bad "$m is disabled (the release set needs it: set : 1, or re-run build-and-deploy.ps1)" }
                else { bad "$m not in mods.txt (re-run build-and-deploy.ps1)" }
            }
            "0" {
                if ($have -eq "1") {
                    if ($m -like "HSMP*") { bad "$m is ENABLED but retired/disabled in the release set: set it to 0 (re-run build-and-deploy.ps1)" }
                    else { warn "$m is enabled (a UE4SS developer mod; off in a release)" }
                } else { ok "$m off" }
            }
            "dev" { if ($have -eq "1") { warn "$m : 1 (developer profile)" } else { ok "$m off (developer-only)" } }
        }
    }
    foreach ($m in $state.Keys) {
        if ($m -like "HSMP*" -and -not $tpl.Contains($m) -and $state[$m] -eq "1") { bad "$m is enabled but not in the release template" }
    }
} else {
    bad "mods.txt missing: $ModsTxt"
}

hdr "Scripts"
foreach ($s in @("build-and-deploy.ps1","run-dedicated-server.ps1","install-service.ps1",
                 "docker-entry.sh","e2e-test.sh","play.ps1")) {
    $p = Join-Path $Repo "scripts\$s"
    if (Test-Path $p) { ok $s } else { warn "optional: $s not present" }
}

hdr "Docker image (optional)"
if (Get-Command docker -ErrorAction SilentlyContinue) {
    $img = docker images hsmp:latest --format "{{.Repository}}:{{.Tag}}" 2>$null
    if ($img) { ok "hsmp:latest built" } else { warn "no hsmp:latest image; docker build . to make one" }
} else {
    warn "docker not on PATH (OK if you do not need containers)"
}

hdr "Quick connectivity probe"
try {
    $r = Invoke-WebRequest -UseBasicParsing -Uri ($masterUrl.TrimEnd('/') + "/v1/health") -TimeoutSec 1 -ErrorAction Stop
    if ($r.Content.Trim() -eq "ok") { ok "master reachable at $masterUrl" } else { warn "master at $masterUrl answered: $($r.Content.Trim())" }
} catch {
    warn "no master at $masterUrl (OK: the host starts a local one when it hosts)"
}

Write-Host ""
Write-Host "============================================================"
if ($script:fail -eq 0) {
    Write-Host "Diagnostics OK - $script:pass checks passed. Ready to play." -ForegroundColor Green
} else {
    Write-Host "Diagnostics FAILED - $script:fail of $($script:pass + $script:fail) checks failed." -ForegroundColor Red
}
Write-Host "============================================================"
exit $script:fail

#Requires -Version 5.1
<#
.SYNOPSIS
    Build + deploy the HSMP stack into the game folder (developer tool; players use the launcher).

.DESCRIPTION
    1. Builds the Rust binaries (cargo build --release -p hsmp-server) unless -SkipBuild.
    2. Deploys every mod under mods\HSMP* and mods\dev\HSMP*: copies every Scripts\*.lua,
       then copies mods\shared\*.lua into each mod's Scripts\
       Retired mods are not deployed.
    3. Writes ue4ss\Mods\mods.txt from the release template (mods\mods.release.txt).
       It NEVER flips a mod on by itself: a mod missing from the template is
       written as ": 0". "dev" entries (HSMPDiag, Keybinds) are on only with -Dev.
       Non-HSMP lines already in mods.txt that the template does not list are kept.
    4. Copies the built binaries into Binaries\Win64\hsmp\ (the layout the launcher
       installs) and writes Binaries\Win64\hsmp.cfg with bin_dir = hsmp (relative), so a
       later `cargo build` elsewhere can never change the binaries under test.
    5. Writes Binaries\Win64\hsmp_deploy.json: git commit, dirty flag, time, profile,
       G0 stamp, the resulting mod table, the SHA-256 of every deployed file (`hashes`)
       and which commit the shipped binaries were built from (`bins_commit`,
       `bins_match_commit`; -SkipBuild keeps the previous deploy's binaries). mp_test.ps1
       re-hashes them at run start and DoD-13 judges the result.
    Backups (*.hsmp_bak) are written once: they keep the user's pre-HSMP file.

    Idempotent. Safe to re-run.

.PARAMETER SkipBuild
    Skip cargo build (Lua-only change).
.PARAMETER Dev
    Developer profile: enables the "dev" template entries (HSMPDiag probes for the
    gate, UE4SS Keybinds). Use it before running scripts\mp_test.ps1.
.PARAMETER GamePath
    Game install root (contains HalfswordUE5\). Default: $env:HSMP_GAME_DIR, then
    <repo>\game, then the main git worktree's game\ (worktrees have none).
.PARAMETER BinDir
    bin_dir written into hsmp.cfg. Default: the copies shipped into Binaries\Win64\hsmp.
.PARAMETER WriteCfg
    Overwrite an existing hsmp.cfg.
.PARAMETER Template
    mods.txt template. Default: mods\mods.release.txt.
.PARAMETER RequireG0
    Refuse to deploy unless G0 (hsmp-gate g0) passed for this exact commit on a clean tree.
.PARAMETER SkipG0
    Do not run the full G0 when none is recorded for this commit. By default a
    clean tree without a recorded G0 pass runs `hsmp-gate g0` (full, with cargo)
    before deploying, so the deploy stamp carries g0_ok for the gate (DoD-13).
.PARAMETER SkipNative
    Do not build HSMPNative (crates/hsmp-native, the game side of the shared-memory IPC, the
    only game<->sidecar IPC). The deploy then writes "HSMPNative : 0"
    and multiplayer does not work; without -SkipNative a missing native build fails the deploy.
.PARAMETER UE4SSSrc
    RE-UE4SS checkout at the pinned commit (its Lua 5.4.7 sources) for the HSMPNative CMake
    build. Default: $env:HSMP_UE4SS_SRC, <repo>\third_party\RE-UE4SS, then <worktree>\mod\RE-UE4SS
    of every git worktree.
.PARAMETER DryRun
    Print what would change; write nothing.

.EXAMPLE
    PS> .\scripts\build-and-deploy.ps1 -SkipBuild -Dev
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$Dev,
    [string]$GamePath = "",
    [string]$BinDir = "",
    [switch]$WriteCfg,
    [string]$Template = "",
    [switch]$RequireG0,
    [switch]$SkipG0,
    [switch]$SkipNative,
    [string]$UE4SSSrc = "",
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
$ModsSrc = Join-Path $Repo "mods"
$DevModsSrc = Join-Path $ModsSrc "dev"
$SharedSrc = Join-Path $ModsSrc "shared"
# one Cargo workspace: build output in $env:CARGO_TARGET_DIR, else <repo>\target
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo "target" }
if (-not $Template) { $Template = Join-Path $Repo "mods\mods.release.txt" }
$Retired = @("HSMPLobby", "HSMPAdmin", "HSMPCharacter", "HSMPSettings", "HSMPChat")
$Utf8NoBom = New-Object System.Text.UTF8Encoding $false

function Say([string]$msg, [ConsoleColor]$fg = "Gray") {
    Write-Host "[hsmp] " -NoNewline -ForegroundColor Cyan
    Write-Host $msg -ForegroundColor $fg
}
function Fail([string]$msg) { Say $msg Red; exit 1 }
function Write-Text([string]$path, [string]$text) {
    if ($DryRun) { Say "  (dry run) would write $path" DarkGray; return }
    [IO.File]::WriteAllText($path, $text, $Utf8NoBom)
}
function Invoke-Git([string[]]$a) {
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try { $o = & git -C $Repo @a 2>$null; if ($LASTEXITCODE -ne 0) { return "" }; return ($o -join "`n").Trim() }
    catch { return "" } finally { $ErrorActionPreference = $prev }
}

# ----- 0. locate the game -------------------------------------------------
if (-not $GamePath) {
    $cands = @()
    if ($env:HSMP_GAME_DIR) { $cands += $env:HSMP_GAME_DIR }
    $cands += (Join-Path $Repo "game")
    foreach ($l in ((Invoke-Git @("worktree", "list", "--porcelain")) -split "`n")) {
        if ($l -like "worktree *") { $cands += (Join-Path ($l.Substring(9).Trim()) "game") }
    }
    foreach ($c in $cands) {
        if (Test-Path (Join-Path $c "HalfswordUE5\Binaries\Win64\ue4ss\Mods")) { $GamePath = $c; break }
    }
    if (-not $GamePath) { Fail "game not found; pass -GamePath or set HSMP_GAME_DIR (tried: $($cands -join ', '))" }
}
$Win64 = Join-Path $GamePath "HalfswordUE5\Binaries\Win64"
$ModsDst = Join-Path $Win64 "ue4ss\Mods"
$ModsTxt = Join-Path $ModsDst "mods.txt"
$profileName = if ($Dev) { "dev" } else { "release" }
Say "repo    = $Repo"
Say "game    = $GamePath"
Say "profile = $profileName$(if ($DryRun) { ' (dry run)' })"
if (-not (Test-Path $ModsDst)) { Fail "mods dir not found: $ModsDst  (is UE4SS installed?)" }
if (-not (Test-Path $Template)) { Fail "mods.txt template not found: $Template" }

# ----- G0 stamp -------------------------------------------------------------
$commit = Invoke-Git @("rev-parse", "HEAD")
$dirty = [bool](Invoke-Git @("status", "--porcelain"))
$g0 = $null
$common = Invoke-Git @("rev-parse", "--git-common-dir")
if ($common) {
    if (-not [IO.Path]::IsPathRooted($common)) { $common = Join-Path $Repo $common }
    # the record for THIS commit (hsmp-g0\<commit>.json: another worktree's G0 cannot replace it);
    # the legacy hsmp-g0.json (the last clean pass anywhere) only when it names this commit
    $g0p = Join-Path $common "hsmp-g0\$commit.json"
    if (-not (Test-Path $g0p)) { $g0p = Join-Path $common "hsmp-g0.json" }
    if (Test-Path $g0p) { try { $g0 = Get-Content $g0p -Raw | ConvertFrom-Json } catch { $g0 = $null } }
}
$g0ok = ($g0 -ne $null) -and $g0.ok -and ($g0.commit -eq $commit) -and (-not $g0.tree_dirty) -and (-not $dirty)
# DoD-13 needs a FULL G0 recorded for the deployed commit on a clean tree. Run
# it here by default when none is recorded (-SkipG0 to skip; a dirty tree can
# never satisfy it, so it is not run then).
if (-not $g0ok -and -not $SkipG0 -and -not $DryRun) {
    if ($dirty) {
        Say "G0 not run: the tree is dirty (commit first; DoD-13 needs a clean tree)" Yellow
    } elseif ($common) {
        $gateExe = Join-Path $TargetDir "release\hsmp-gate.exe"
        $cargoG = Get-Command cargo -ErrorAction SilentlyContinue
        $cargoGP = if ($cargoG) { $cargoG.Source } elseif (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe") { "$env:USERPROFILE\.cargo\bin\cargo.exe" } else { $null }
        if ($cargoGP) {
            Say "G0: building hsmp-tools (release)..."
            $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
            & $cargoGP build --release -p hsmp-tools --manifest-path (Join-Path $Repo "Cargo.toml") 2>&1 | Out-Null
            $ErrorActionPreference = $prev
        }
        if (Test-Path $gateExe) {
            Say "G0: running the full gate for $($commit.Substring(0, 10)) (lints, Lua suites, cargo tests; -SkipG0 skips)..."
            $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
            if (-not $env:HSMP_GAME_DIR) { $env:HSMP_GAME_DIR = $GamePath }
            Push-Location $Repo
            try { & $gateExe g0 2>&1 | ForEach-Object { Write-Host "  $_" } } finally { Pop-Location }
            $ErrorActionPreference = $prev
            $g0p = Join-Path $common "hsmp-g0\$commit.json"
            if (Test-Path $g0p) { try { $g0 = Get-Content $g0p -Raw | ConvertFrom-Json } catch { $g0 = $null } }
            $g0ok = ($g0 -ne $null) -and $g0.ok -and ($g0.commit -eq $commit) -and (-not $g0.tree_dirty) -and (-not $dirty)
            Say "G0: $(if ($g0ok) { 'PASS recorded for this commit' } else { 'did NOT pass (see above)' })" $(if ($g0ok) { 'Green' } else { 'Red' })
        } else { Say "G0 not run: hsmp-gate.exe not built ($gateExe)" Yellow }
    }
}
if ($RequireG0 -and -not $g0ok) {
    Fail "G0 has not passed for $commit on a clean tree (run: $(Join-Path $TargetDir 'release\hsmp-gate.exe') g0)"
}
if (-not $g0ok) { Say "note: no G0 pass recorded for this exact tree (stamp says g0_ok=false)" Yellow }

# ----- 1. Build Rust bins -------------------------------------------------
# cargo honours CARGO_TARGET_DIR (per-worktree target dirs): the build output is there
$CargoOut = Join-Path $TargetDir "release"
$ShipBins = @("hsmp-server.exe", "hsmp-sidecar.exe", "hsmp-master.exe", "hsmp-query.exe")
if (-not $SkipBuild) {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) { $cargo = Get-Item "$env:USERPROFILE\.cargo\bin\cargo.exe" -ErrorAction SilentlyContinue }
    if (-not $cargo) { Fail "cargo not found; install Rust via https://rustup.rs/ first" }
    $cargoPath = if ($cargo.Source) { $cargo.Source } else { $cargo.FullName }
    Say "building rust bins (release)..."
    if (-not $DryRun) {
        Push-Location $Repo
        try {
            $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
            & $cargoPath build --release --locked -p hsmp-server 2>&1 | ForEach-Object { Write-Host "  $_" }
            $code = $LASTEXITCODE
            $ErrorActionPreference = $prev
            if ($code -ne 0) { Fail "cargo build failed (exit $code)" }
        } finally { Pop-Location }
        foreach ($bin in $ShipBins) {
            $path = Join-Path $CargoOut $bin
            if (Test-Path $path) { Say "  built: $bin ($([math]::Round((Get-Item $path).Length / 1MB, 1)) MB)" Green }
            else { Fail "missing build output: $path" }
        }
    }
} else {
    Say "-SkipBuild: not re-running cargo"
}

# ----- 1b. ship the binaries into Win64\hsmp ---------------------------------
# The game never runs the live cargo output (<repo>\target\release): a later `cargo build` from
# any checkout would silently change the binaries under test while the stamp still named the
# deployed commit. Deploy copies them (as the launcher installs them: bin_dir = hsmp, relative to
# Win64) and stamps their SHA-256; -BinDir keeps an explicit dir.
$StampPath = Join-Path $Win64 "hsmp_deploy.json"
$prevStamp = $null
if (Test-Path $StampPath) { try { $prevStamp = Get-Content $StampPath -Raw | ConvertFrom-Json } catch { $prevStamp = $null } }
$ShipDir = Join-Path $Win64 "hsmp"
$binsCommit = "unknown"
if (-not $BinDir) {
    if (-not $SkipBuild) {
        if (-not $DryRun) {
            New-Item -ItemType Directory -Force $ShipDir | Out-Null
            foreach ($bin in $ShipBins) {
                try { Copy-Item -LiteralPath (Join-Path $CargoOut $bin) -Destination (Join-Path $ShipDir $bin) -Force -ErrorAction Stop }
                catch { Fail "could not copy $bin into $ShipDir (is a game / server of a run still using it?): $($_.Exception.Message)" }
            }
            Say "binaries shipped into $ShipDir" Green
        } else { Say "  (dry run) would copy $($ShipBins -join ', ') into $ShipDir" DarkGray }
        $binsCommit = if ($dirty) { "dirty" } else { $commit }
    } elseif ($prevStamp -and $prevStamp.hashes -and $prevStamp.bins_commit) {
        # -SkipBuild: the shipped binaries are still the previous deploy's if their hashes match
        $same = $true
        foreach ($bin in $ShipBins) {
            $f = Join-Path $ShipDir $bin
            $want = [string]$prevStamp.hashes."hsmp/$bin"
            if (-not (Test-Path -LiteralPath $f) -or -not $want -or (Get-FileHash -Algorithm SHA256 -LiteralPath $f).Hash -ne $want.ToUpperInvariant()) { $same = $false }
        }
        $binsCommit = if ($same) { [string]$prevStamp.bins_commit } else { "unknown" }
    }
}
# the binaries are valid for this commit when built from it, or when nothing they are built
# from (server/, crates/, the workspace manifest, lock and toolchain) changed between their
# commit and this one
$binsMatch = $false
if ($binsCommit -eq $commit -and -not $dirty) { $binsMatch = $true }
elseif ($binsCommit -match '^[0-9a-f]{40}$' -and $commit -and -not $dirty) {
    $prevEap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    & git -C $Repo diff --quiet $binsCommit $commit -- server crates Cargo.toml Cargo.lock rust-toolchain.toml 2>$null
    $binsMatch = ($LASTEXITCODE -eq 0)
    $ErrorActionPreference = $prevEap
}

# ----- 1c. HSMPNative (the game side of the shared-memory IPC) ---------------
# crates/hsmp-native/README.md: the CMake build writes out\HSMPNative\dlls\main.dll (loaded
# by UE4SS as a C++ mod) and hsmp_lua.dll (the facade's fallback: loadlib from
# the SAME folder). It is required (shared memory is the only IPC); only
# -SkipNative deploys without it (HSMPNative : 0, no multiplayer), never a stale native module.
$NativeSrc = Join-Path $Repo "crates\hsmp-native"
$NativeBuild = Join-Path $TargetDir "hsmp-native-cmake"
$NativeOut = Join-Path $NativeBuild "out"
$NativeMod = "HSMPNative"
$NativeFiles = @("dlls/main.dll", "dlls/hsmp_lua.dll")      # relative to Mods\HSMPNative
$NativeProbe = "HSMPNativeProbe"
$nativeDeployed = $false
$probeDeployed = $false
function Find-UE4SSSrc {
    $c = @()
    if ($UE4SSSrc) { $c += $UE4SSSrc }
    if ($env:HSMP_UE4SS_SRC) { $c += $env:HSMP_UE4SS_SRC }
    $c += (Join-Path $Repo "third_party\RE-UE4SS")
    foreach ($l in ((Invoke-Git @("worktree", "list", "--porcelain")) -split "`n")) {
        if ($l -like "worktree *") { $w = $l.Substring(9).Trim(); $c += (Join-Path $w "third_party\RE-UE4SS"); $c += (Join-Path $w "mod\RE-UE4SS") }
    }
    foreach ($d in $c) { if ($d -and (Test-Path (Join-Path $d "deps"))) { return $d } }
    foreach ($d in $c) { if ($d -and (Test-Path $d)) { return $d } }
    return $null
}
$nativeBuilt = $false
# Shared memory is the only game<->sidecar IPC, so a deploy without a fresh
# HSMPNative is a deploy without multiplayer. That is a failure unless -SkipNative asks for it.
function Native-Missing([string]$why) {
    if ($SkipNative) { Say "HSMPNative not deployed ($why): -SkipNative, multiplayer will NOT work in this deploy" Yellow; return }
    Fail "HSMPNative: $why. It is required (shared-memory IPC); fix it, or pass -SkipNative to deploy without multiplayer."
}
if ($SkipNative) {
    Native-Missing "-SkipNative"
} elseif (-not (Test-Path (Join-Path $NativeSrc "CMakeLists.txt"))) {
    Native-Missing "crates\hsmp-native is not in this tree"
} elseif ($SkipBuild) {
    # -SkipBuild: reuse this target dir's last native build if it is there
    $nativeBuilt = -not @($NativeFiles | Where-Object { -not (Test-Path (Join-Path (Join-Path $NativeOut $NativeMod) $_)) }).Count
    if ($nativeBuilt) { Say "-SkipBuild: HSMPNative from the previous build in $NativeOut" }
    else { Native-Missing "-SkipBuild but no previous native build in $NativeOut" }
} else {
    $cmake = Get-Command cmake -ErrorAction SilentlyContinue
    $ue = Find-UE4SSSrc
    if (-not $cmake) { Native-Missing "cmake not found" }
    elseif (-not $ue) { Native-Missing "no RE-UE4SS checkout (-UE4SSSrc / HSMP_UE4SS_SRC)" }
    elseif ($DryRun) { Say "  (dry run) would build HSMPNative with CMake (UE4SS_SRC=$ue) into $NativeBuild" DarkGray }
    else {
        Say "building HSMPNative (CMake, UE4SS_SRC=$ue)..."
        $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
        & $cmake.Source -S $NativeSrc -B $NativeBuild -G "Visual Studio 17 2022" -A x64 "-DUE4SS_SRC=$ue" 2>&1 | ForEach-Object { Write-Host "  $_" }
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            & $cmake.Source --build $NativeBuild --config Release 2>&1 | ForEach-Object { Write-Host "  $_" }
            $code = $LASTEXITCODE
        }
        $ErrorActionPreference = $prev
        $nativeBuilt = ($code -eq 0) -and -not @($NativeFiles | Where-Object { -not (Test-Path (Join-Path (Join-Path $NativeOut $NativeMod) $_)) }).Count
        if ($nativeBuilt) { Say "  built: HSMPNative ($($NativeFiles -join ', '))" Green }
        else { Native-Missing "the native build failed (exit $code)" }
    }
}
if ($nativeBuilt) {
    $dstNative = Join-Path $ModsDst $NativeMod
    if (-not $DryRun) {
        foreach ($f in $NativeFiles) {
            $dst = Join-Path $dstNative ($f -replace '/', '\')
            New-Item -ItemType Directory -Force (Split-Path -Parent $dst) | Out-Null
            try { Copy-Item -LiteralPath (Join-Path (Join-Path $NativeOut $NativeMod) ($f -replace '/', '\')) -Destination $dst -Force -ErrorAction Stop }
            catch { Fail "could not copy $f into $dstNative (is the game running?): $($_.Exception.Message)" }
        }
    }
    $nativeDeployed = $true
    Say "  deployed: $NativeMod ($($NativeFiles -join ', '))" Green
    # dev profile: the in-game IPC probe (prints IPC VERDICT ...), with the schema it checks
    $probeSrc = Join-Path $NativeSrc "probe\$NativeProbe\Scripts"
    if ($Dev -and (Test-Path (Join-Path $probeSrc "main.lua"))) {
        $dstProbe = Join-Path $ModsDst "$NativeProbe\Scripts"
        if (-not $DryRun) {
            New-Item -ItemType Directory -Force $dstProbe | Out-Null
            Get-ChildItem -File $probeSrc -Filter "*.lua" | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $dstProbe $_.Name) -Force }
            $schema = Join-Path $SharedSrc "hsmp_ipc_schema.lua"
            if (Test-Path $schema) { Copy-Item -LiteralPath $schema -Destination (Join-Path $dstProbe "hsmp_ipc_schema.lua") -Force }
        }
        $probeDeployed = $true
        Say "  deployed: $NativeProbe (dev)" Green
    }
}

# ----- 2. Deploy Lua mods + shared libraries ---------------------------------
$shared = @()
if (Test-Path $SharedSrc) { $shared = @(Get-ChildItem -File $SharedSrc -Filter "*.lua") }
Say "shared libs: $(($shared | ForEach-Object { $_.Name }) -join ', ')"
$mods = @(Get-ChildItem -Directory $ModsSrc -Filter "HSMP*")
if (Test-Path $DevModsSrc) { $mods += @(Get-ChildItem -Directory $DevModsSrc -Filter "HSMP*") }
if ($mods.Count -eq 0) { Fail "no HSMP* mod directories under $ModsSrc" }
$deployed = @()
$fileCount = 0
foreach ($m in $mods) {
    if ($Retired -contains $m.Name) { Say "  retired, not deployed: $($m.Name)" DarkGray; continue }
    $srcScripts = Join-Path $m.FullName "Scripts"
    if (-not (Test-Path (Join-Path $srcScripts "main.lua"))) { Say "  skip $($m.Name): no Scripts\main.lua" Yellow; continue }
    $dstScripts = Join-Path $ModsDst "$($m.Name)\Scripts"
    if (-not $DryRun) { New-Item -ItemType Directory -Force $dstScripts | Out-Null }
    $names = @{}
    foreach ($f in (Get-ChildItem -File $srcScripts -Filter "*.lua")) {
        $names[$f.Name] = $true
        if (-not $DryRun) { Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $dstScripts $f.Name) -Force }
        $fileCount++
    }
    foreach ($s in $shared) {
        if ($names.ContainsKey($s.Name)) {
            $same = (Get-FileHash (Join-Path $srcScripts $s.Name)).Hash -eq (Get-FileHash $s.FullName).Hash
            if (-not $same) { Say "  $($m.Name): private Scripts\$($s.Name) differs from shared\ - shared copy wins" Yellow }
        }
        if (-not $DryRun) { Copy-Item -LiteralPath $s.FullName -Destination (Join-Path $dstScripts $s.Name) -Force }
        $names[$s.Name] = $true
        $fileCount++
    }
    $names["hsmp_build_id.lua"] = $true   # written below (2b)
    if (Test-Path $dstScripts) {
        $stale =@(Get-ChildItem -File $dstScripts -Filter "*.lua" | Where-Object { -not $names.ContainsKey($_.Name) })
        if ($stale.Count) { Say "  $($m.Name): deployed files not in source (left in place): $(($stale | ForEach-Object { $_.Name }) -join ', ')" Yellow }
    }
    $deployed += $m.Name
    Say "  deployed: $($m.Name) ($($names.Count) files)" Green
}

# ----- 2b. Build identity of these mod files ----------------------------------
# hsmp_build_id.lua in every mod (HSMPMenu compares it with `hsmp-sidecar --build-info` and
# refuses multiplayer on a mismatch) and hsmp\build.json (the installed version, for the
# launcher). The content hash is computed from this checkout, not taken from the binary.
$IdExe = Join-Path $(if ($BinDir) { $BinDir } else { $ShipDir }) "hsmp-server.exe"
if (Test-Path -LiteralPath $IdExe) {
    $prevEap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    $idLua = (& $IdExe --mods-identity-lua $Repo 2>$null) -join "`n"
    $luaCode = $LASTEXITCODE
    $idJson = (& $IdExe --mods-identity $Repo 2>$null) -join ""
    $jsonCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    if ($luaCode -ne 0 -or $jsonCode -ne 0 -or -not $idLua.Contains("content_hash")) {
        Fail "build identity: $IdExe --mods-identity failed (exit $luaCode/$jsonCode); rebuild the binaries"
    }
    foreach ($n in $deployed) {
        $sd = Join-Path $ModsDst "$n\Scripts"
        if ($DryRun -or (Test-Path $sd)) { Write-Text (Join-Path $sd "hsmp_build_id.lua") ($idLua + "`n") }
    }
    $IdDir = if ($BinDir) { $BinDir } else { $ShipDir }
    if ($DryRun -or (Test-Path $IdDir)) { Write-Text (Join-Path $IdDir "build.json") ($idJson + "`n") }
    Say "build identity: $idJson" Green
} else {
    Say "build identity NOT written: $IdExe missing (multiplayer stays off until a deploy with binaries)" Yellow
}

# ----- 3. mods.txt from the release template -------------------------------
$tpl = [ordered]@{}
foreach ($line in (Get-Content $Template)) {
    $t = $line.Trim()
    if (-not $t -or $t.StartsWith("#") -or $t.StartsWith(";")) { continue }
    if ($t -match '^([^:\s]+)\s*:\s*(0|1|dev)\s*$') { $tpl[$Matches[1]] = $Matches[2] }
    else { Fail "bad template line: '$line'" }
}
$existing = [ordered]@{}
if (Test-Path $ModsTxt) {
    foreach ($line in (Get-Content $ModsTxt)) {
        if ($line -match '^\s*([^:;\s]+)\s*:\s*(\d)\s*$') { $existing[$Matches[1]] = $Matches[2] }
    }
}
$final = [ordered]@{}
foreach ($k in $tpl.Keys) {
    if ($k -eq "Keybinds") { continue }
    $v = $tpl[$k]
    $final[$k] = if ($v -eq "dev") { if ($Dev) { "1" } else { "0" } } else { $v }
}
foreach ($k in $existing.Keys) {   # third-party mods the template does not know: keep as-is
    if (-not $final.Contains($k) -and $k -ne "Keybinds" -and $k -notlike "HSMP*") { $final[$k] = $existing[$k] }
}
foreach ($m in $mods) {            # HSMP mods missing from the template: never enabled by deploy
    if (-not $final.Contains($m.Name)) {
        $final[$m.Name] = "0"
        Say "  mods.txt: $($m.Name) is not in $([IO.Path]::GetFileName($Template)) -> written as 0 (add it to the template)" Yellow
    }
}
# HSMPNative / its probe: on only when THIS deploy shipped them (never a stale native module)
if ($final.Contains($NativeMod) -and -not $nativeDeployed) { $final[$NativeMod] = "0" }
if ($final.Contains($NativeProbe) -and -not $probeDeployed) { $final[$NativeProbe] = "0" }
$kb = if ($tpl.Contains("Keybinds")) { $tpl["Keybinds"] } else { "1" }
$final["Keybinds"] = if ($kb -eq "dev") { if ($Dev) { "1" } else { "0" } } else { $kb }

$out = New-Object System.Collections.Generic.List[string]
$out.Add("; generated by scripts/build-and-deploy.ps1 from mods/mods.release.txt ($profileName profile)")
$out.Add("; edit the template, not this file")
foreach ($k in $final.Keys) {
    if ($k -eq "Keybinds") { continue }
    $out.Add("$k : $($final[$k])")
}
$out.Add("")
$out.Add("; Built-in keybinds, do not move up!")
$out.Add("Keybinds : $($final['Keybinds'])")
foreach ($k in $final.Keys) {
    $was = if ($existing.Contains($k)) { $existing[$k] } else { "-" }
    if ($was -ne $final[$k]) { Say "  mods.txt: $k $was -> $($final[$k])" }
}
# keep the FIRST backup (the user's pre-HSMP file); a later deploy never overwrites it
if ((Test-Path $ModsTxt) -and -not $DryRun -and -not (Test-Path "$ModsTxt.hsmp_bak")) { Copy-Item $ModsTxt "$ModsTxt.hsmp_bak" }
Write-Text $ModsTxt (($out -join "`r`n") + "`r`n")

# mods.json (newer UE4SS builds): keep entries it already has consistent with mods.txt
$ModsJson = Join-Path $ModsDst "mods.json"
if (Test-Path $ModsJson) {
    try {
        $mj = Get-Content $ModsJson -Raw | ConvertFrom-Json
        $changed = $false
        foreach ($e in $mj) {
            if ($final.Contains($e.mod_name)) {
                $want = ($final[$e.mod_name] -eq "1")
                if ($e.mod_enabled -ne $want) { $e.mod_enabled = $want; $changed = $true }
            }
        }
        if ($changed) {
            if (-not $DryRun -and -not (Test-Path "$ModsJson.hsmp_bak")) { Copy-Item $ModsJson "$ModsJson.hsmp_bak" }
            Write-Text $ModsJson (ConvertTo-Json -InputObject @($mj) -Depth 4)
            Say "  mods.json synced"
        }
    } catch { Say "  mods.json not updated: $($_.Exception.Message)" Yellow }
}

# ----- 4. hsmp.cfg ----------------------------------------------------------
$cfgPath = Join-Path $Win64 "hsmp.cfg"
# bin_dir: the shipped copies (relative "hsmp", as the launcher writes it), or an explicit -BinDir
$BinDirCfg = if ($BinDir) { $BinDir -replace '\\', '/' } else { "hsmp" }
if (-not $BinDir) { $BinDir = $ShipDir }
if ($WriteCfg -or -not (Test-Path $cfgPath)) {
    $cfg = @(
        "# hsmp.cfg - read by mods through shared/hsmp_cfg.lua (written by build-and-deploy.ps1)",
        "# bin_dir: folder with hsmp-server.exe / hsmp-sidecar.exe / hsmp-query.exe",
        "#          (absolute, or relative to this Binaries\Win64 folder)",
        "bin_dir = $BinDirCfg",
        "# master_url: primary first, then fallbacks (comma-separated)",
        "master_url = http://127.0.0.1:7778",
        "# local_master = auto | off   (HOST starts hsmp-master when master_url is on this machine)",
        "# lan_discovery = 1           (0 = no LAN section in the browser)",
        "# lan_ports = 7777-7786",
        ""
    ) -join "`r`n"
    Write-Text $cfgPath $cfg
    Say "hsmp.cfg written: bin_dir=$BinDir" Green
} else {
    # The binaries this deploy built (or -BinDir) are what the game must run:
    # a cfg left by a deploy from another checkout (e.g. the main tree while
    # deploying from a worktree) would silently run THAT tree's server/sidecar,
    # and mp_test.ps1 reads bin_dir from here too. Re-point bin_dir only; every
    # other key is kept.
    $cfgText = [IO.File]::ReadAllText($cfgPath)
    $want = $BinDirCfg
    $rx = '(?m)^[ \t]*bin_dir[ \t]*=[ \t]*"?([^"#;\r\n]*?)"?[ \t]*(?:[#;][^\r\n]*)?(\r?\n|\z)'
    $all = [regex]::Matches($cfgText, $rx)
    $m = if ($all.Count) { $all[$all.Count - 1] } else { $null }   # the reader keeps the last value
    $have = if ($m) { $m.Groups[1].Value.Trim() } else { "" }
    $haveAbs = if ($have -and -not [IO.Path]::IsPathRooted($have)) { Join-Path $Win64 $have } else { $have }
    $same = $haveAbs -and ([IO.Path]::GetFullPath($haveAbs).TrimEnd('\', '/') -ieq [IO.Path]::GetFullPath($BinDir).TrimEnd('\', '/'))
    if ($same -and $all.Count -eq 1) {
        Say "hsmp.cfg kept (bin_dir = this repo's binaries): $cfgPath"
    } else {
        if ($all.Count) {
            # one bin_dir line, at the first one's place
            $first = $all[0]
            for ($k = $all.Count - 1; $k -ge 1; $k--) { $cfgText = $cfgText.Remove($all[$k].Index, $all[$k].Length) }
            $cfgText = $cfgText.Remove($first.Index, $first.Length).Insert($first.Index, "bin_dir = $want`r`n")
        } else { $cfgText = $cfgText.TrimEnd() + "`r`nbin_dir = $want`r`n" }
        if (-not $DryRun -and -not (Test-Path "$cfgPath.hsmp_bak")) { Copy-Item $cfgPath "$cfgPath.hsmp_bak" }
        Write-Text $cfgPath $cfgText
        Say "hsmp.cfg: bin_dir '$have' -> '$want' (the binaries of this deploy)" Yellow
    }
    # Without a master_url the mods use the public list; a dev deploy must not register there.
    $cfgText = if (Test-Path $cfgPath) { [IO.File]::ReadAllText($cfgPath) } else { "" }
    if ($cfgText -notmatch '(?m)^[ \t]*master_url[ \t]*=') {
        $cfgText = $cfgText.TrimEnd() + "`r`nmaster_url = http://127.0.0.1:7778`r`n"
        if (-not $DryRun -and -not (Test-Path "$cfgPath.hsmp_bak")) { Copy-Item $cfgPath "$cfgPath.hsmp_bak" }
        Write-Text $cfgPath $cfgText
        Say "hsmp.cfg: master_url = http://127.0.0.1:7778 added (the built-in default is the public list)" Yellow
    }
}

# ----- 4b. engine workaround: hair-strand streaming ------------------------------
# PAK_ASYNC_READ_OOB: the shipping pak reader reads past the end of the hair
# .ubulk while streaming hair LODs and crashes on the IoDispatcher thread.
# Loading hair whole avoids the paged reads. Merged into the user's Engine.ini.
$engineIni = Join-Path $env:LOCALAPPDATA "HalfSwordUE5\Saved\Config\Windows\Engine.ini"
try {
    $ini = if (Test-Path $engineIni) { [IO.File]::ReadAllText($engineIni) } else { "" }
    if ($null -eq $ini) { $ini = "" }
    if ($ini -notmatch '(?m)^\s*r\.HairStrands\.Streaming\s*=' -and $DryRun) {
        # -DryRun promises to write nothing (the user's Engine.ini included)
        Say "  (dry run) would add r.HairStrands.Streaming=0 to $engineIni" DarkGray
    } elseif ($ini -notmatch '(?m)^\s*r\.HairStrands\.Streaming\s*=') {
        New-Item -ItemType Directory -Force (Split-Path $engineIni) | Out-Null
        # keep the first backup (the user's own Engine.ini)
        if ((Test-Path $engineIni) -and -not (Test-Path "$engineIni.hsmp_bak")) { Copy-Item $engineIni "$engineIni.hsmp_bak" }
        if ($ini -match '(?m)^\[SystemSettings\]') {
            $ini = [regex]::Replace($ini, '(?m)^\[SystemSettings\]\r?\n?', "[SystemSettings]`r`nr.HairStrands.Streaming=0`r`n", 1)
        } else {
            $ini = $ini.TrimEnd() + $(if ($ini.Trim()) { "`r`n`r`n" } else { "" }) + "[SystemSettings]`r`nr.HairStrands.Streaming=0`r`n"
        }
        [IO.File]::WriteAllText($engineIni, $ini)
        Say "Engine.ini: r.HairStrands.Streaming=0 (IoDispatcher crash workaround)" Green
    } else {
        Say "Engine.ini: hair streaming setting already present"
    }
} catch { Say "Engine.ini: could not apply the hair-strand streaming workaround: $_" Yellow }

# ----- 4c. IPC backend setting ---------------------------------------------------
# Shared memory is the only IPC. An "ipc" key in the game's default
# <Win64>\hsmp_state\.settings.json is set to "shm" so an older facade can never pick files;
# the other keys of the file are kept.
$Ipc = "shm"
if ($true) {
    $stDir = Join-Path $Win64 "hsmp_state"
    $setPath = Join-Path $stDir ".settings.json"
    $set = [ordered]@{}
    if (Test-Path $setPath) {
        try { (Get-Content $setPath -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $set[$_.Name] = $_.Value } }
        catch { Say "  .settings.json unreadable, rewritten with ipc only: $($_.Exception.Message)" Yellow }
    }
    $set["ipc"] = $Ipc
    if (-not $DryRun) { New-Item -ItemType Directory -Force $stDir | Out-Null }
    Write-Text $setPath (ConvertTo-Json -InputObject $set -Depth 8 -Compress)
    Say "ipc backend: $Ipc ($setPath)" Green
}

# ----- 5. deploy stamp --------------------------------------------------------
# SHA-256 of every deployed file (paths relative to Win64, '/'-separated): the shipped
# binaries, every Lua file of the deployed mods, mods.txt and hsmp.cfg. mp_test.ps1 re-hashes
# them when a run starts; DoD-13 fails a run whose files changed after this deploy.
$hashes = [ordered]@{}
function Add-Hash([string]$rel) {
    $f = Join-Path $Win64 ($rel -replace '/', '\')
    if (Test-Path -LiteralPath $f) { $hashes[$rel] = (Get-FileHash -Algorithm SHA256 -LiteralPath $f).Hash }
}
if (-not $DryRun) {
    if ([IO.Path]::GetFullPath($BinDir).TrimEnd('\') -ieq [IO.Path]::GetFullPath($ShipDir).TrimEnd('\')) {
        foreach ($bin in $ShipBins) { Add-Hash "hsmp/$bin" }
    } else {
        Say "bin_dir is an explicit -BinDir outside the game folder: its binaries are not stamped (DoD-13 will not certify the run)" Yellow
    }
    foreach ($m in $deployed) {
        foreach ($f in @(Get-ChildItem -File (Join-Path $ModsDst "$m\Scripts") -Filter "*.lua" -ErrorAction SilentlyContinue | Sort-Object Name)) {
            Add-Hash "ue4ss/Mods/$m/Scripts/$($f.Name)"
        }
    }
    if ($nativeDeployed) { foreach ($f in $NativeFiles) { Add-Hash "ue4ss/Mods/$NativeMod/$f" } }
    if ($probeDeployed) {
        foreach ($f in @(Get-ChildItem -File (Join-Path $ModsDst "$NativeProbe\Scripts") -Filter "*.lua" -ErrorAction SilentlyContinue | Sort-Object Name)) {
            Add-Hash "ue4ss/Mods/$NativeProbe/Scripts/$($f.Name)"
        }
    }
    Add-Hash "ue4ss/Mods/mods.txt"
    Add-Hash "hsmp.cfg"
}
$stamp = [ordered]@{
    commit = $commit
    branch = (Invoke-Git @("rev-parse", "--abbrev-ref", "HEAD"))
    dirty = $dirty
    time = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    profile = $profileName
    repo = $Repo
    g0_ok = $g0ok
    g0 = $g0
    skip_build = [bool]$SkipBuild
    files = $fileCount
    deployed = $deployed
    mods = $final
    bin_dir = $BinDirCfg
    bins_commit = $binsCommit
    bins_match_commit = $binsMatch
    native = [ordered]@{ deployed = $nativeDeployed; probe = $probeDeployed; files = $(if ($nativeDeployed) { $NativeFiles } else { @() }) }
    ipc = $Ipc
    hashes = $hashes
}
Write-Text (Join-Path $Win64 "hsmp_deploy.json") ($stamp | ConvertTo-Json -Depth 6)
Say "deploy stamp: $($commit.Substring(0, [Math]::Min(10, $commit.Length)))$(if ($dirty) { '+dirty' }) $profileName" Green

Write-Host ""
Say "ready." Green
Write-Host @"

  Half Sword Multiplayer
    Main menu: HSMP adds HOST GAME / SERVER BROWSER / SETTINGS / CHARACTER / QUIT MP.
    Binaries:  hsmp.cfg bin_dir ($BinDir)
    Profile:   $profileName$(if (-not $Dev) { '  (HSMPDiag + UE4SS dev mods off; use -Dev for the gate)' })
    Retired:   $($Retired -join ', ') (not deployed)
    Gate:      scripts\mp_test.ps1 -Scenario p0_gate   (see docs\development\testing.md)

"@ -ForegroundColor Gray
exit 0

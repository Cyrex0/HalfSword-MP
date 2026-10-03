# Releasing HSMP

How a maintainer builds, signs, checks and publishes a release, and what the launcher does with
it. Players read [../players/install.md](../players/install.md) instead.

A release is one zip, `hsmp-<version>.zip`, built by `hsmp-release` (`tools/release`) from a
git commit. It holds a signed `manifest.json` (+ `manifest.sig`), the launcher, the Lua mods, the
native module HSMPNative, the Rust binaries, the pinned UE4SS files, `INSTALL.md` and the licence
files. The launcher verifies
the signature and every file hash before it installs anything.

## 1. Version and tag policy

- **One version number everywhere.** The git tag `v<version>` = `version` in
  `tools/release/release.json` = the launcher's version (the workspace `version` in the root
  `Cargo.toml`, which `hsmp-launcher` inherits and shows in its title bar). Bump both together.
  Nothing checks that they agree yet; check it by hand.
- `version` must look like `0.3.1` or `0.3.1-beta.2` (the tool refuses anything else).
  `channel` is `beta` today; `dev` relaxes two safety checks (§4) and must never be published.
- **Never rebuild a published version.** If something must change, bump the version.
- The wire protocol version is not part of the release version: the manifest's
  `protocol_version` is read from `PROTOCOL_VERSION` in `crates/hsmp-net/src/net/mod.rs` at the
  release commit.

## 2. Pinned inputs

Everything that is not in git is pinned by hash, so a release can be rebuilt by anyone:

| Input | Pin | Where |
|---|---|---|
| Rust toolchain | `1.98.1` | `rust-toolchain.toml` `channel` **and** `release.json` `rust_toolchain`. The test `real_config_pins_the_toolchain` (`tools/release/src/lib.rs`, run by `cargo test --workspace`) fails if they differ. The tool refuses another `rustc` and records `rustc -V` in the manifest |
| Dependencies | `Cargo.lock` | every build runs with `--locked` |
| UE4SS | `dll_sha256`, `proxy_sha256` | `release.json` `ue4ss`; see [ue4ss.md](ue4ss.md) |
| Supported game builds | `exe_sha256`, `exe_size`, `steam_buildid` | `release.json` `game.builds` |
| Trusted signing keys | the public keys | `launcher/trusted_keys.txt`, compiled into the launcher |

## 3. One-time: the signing key

1. Create a key **outside any repository**:

   ```powershell
   cargo run --release -p hsmp-release -- keygen --out <file outside the repo>
   ```

   It writes a 32-byte ed25519 seed (hex) and prints the line for `trusted_keys.txt`. It refuses
   to write inside a git repository and never overwrites an existing file.
2. Add the printed line to `launcher/trusted_keys.txt` and commit it. The launcher compiles that
   file in (`include_str!` in `launcher/src/sign.rs`).
3. Back the key file up somewhere safe and private. Anyone who has it can sign releases that
   every launcher trusts.
4. The release tool reads the key only from the environment: `HSMP_RELEASE_SIGNING_KEY` (64 hex
   characters) or `HSMP_RELEASE_SIGNING_KEY_FILE` (the path to the key file). Never commit it,
   never paste it into an issue, a log or a chat. `hsmp-release pubkey` prints the public line for
   the key in the environment.
5. **Rotation:** add the new key's line, ship one release whose launcher trusts both keys, sign
   later releases with the new key, then remove the old line. Players update through the launcher
   copy in `%LOCALAPPDATA%\HSMP\bin`, which the transition release replaces with one that trusts
   both.

What the signature proves: the launcher trusts only the keys compiled into it, plus an optional
`%LOCALAPPDATA%\HSMP\trusted_keys.txt` a player may pin by hand. Nothing inside a package is ever
used as a key. For a **first** download the launcher comes out of the same zip it verifies, so the
signature detects corruption, not a re-packed zip: the trust root is the SHA-256 published on the
release page (and, later, Authenticode code signing). Updates opened with the installed launcher
copy are verified against keys the player already has.

## 4. Every release

1. **Gate.** The commit has passed G0 (`hsmp-gate g0`, with the game present so nothing is
   skipped), the e2e suite and the live gate ([testing.md](testing.md)). CI
   (`.github/workflows/ci.yml`) runs G0 without the game and the e2e suite on pushes to `main`
   and on pull requests; it does not replace the live gate. Releases come from `main`.
2. **Version.** Bump `version` in `tools/release/release.json` and the workspace `version` in the
   root `Cargo.toml` (§1). If the wire protocol changed, check `PROTOCOL_VERSION`.
3. **Server list.** `master_urls` in `release.json` must hold the public master, primary first:
   `https://master.halfswordmp.workers.dev` (the Cloudflare Worker,
   [master-server.md](../hosting/master-server.md)), then `http://127.0.0.1:7778` as the fallback.
   The tool **refuses** a list that only points at this machine (`localhost`, `127.*`, `0.0.0.0`;
   `is_loopback` in `tools/release/src/lib.rs`) unless the channel is `dev` or you pass
   `--allow-loopback-master`, and then it warns that the browser shows LAN servers only. It also
   refuses any master on another machine that is not `https://`.
4. **Supported game builds.** For each Half Sword build you tested, add
   `{label, steam_buildid, exe_sha256, exe_size}` under `game.builds`. `hsmp-launcher status`
   prints the exe hash of an unknown build; the Steam build id is in
   `steamapps\appmanifest_2397300.acf`. Remove builds you no longer support. When the game
   patches, regenerate the UE4SS object dump and run `hsmp-tools check-bp-names` before adding
   the build ([lua-mods.md](lua-mods.md) §4).
5. **UE4SS pin.** To change the UE4SS build, update `dll_sha256`, `proxy_sha256` and `version`
   together, run the live gate with that build, and review `include` (only those files ship: no
   PDB, dumps, SDK or developer mods). See [ue4ss.md](ue4ss.md).
6. **Toolchain.** `rustup toolchain install 1.98.1` once. To move to a new Rust version, change
   `rust-toolchain.toml`, `release.json` `rust_toolchain` and the `RUST_TOOLCHAIN` line in
   `.github/workflows/ci.yml` in one commit.
7. **Commit everything.** The tree must be clean, untracked files included (an untracked `.rs`
   module would be compiled in without being in the commit). A release is built from git
   objects, not from the working tree.
8. **Build:**

   ```powershell
   $env:HSMP_RELEASE_SIGNING_KEY_FILE = "<path to the key file, outside the repo>"
   cargo run --release -p hsmp-release -- build --ue4ss-dir "<Win64 folder of the pinned UE4SS build>" `
       --native-dir "<CMake out folder of crates/hsmp-native>"
   ```

   `--ue4ss-dir` defaults to `$HSMP_UE4SS_DIR`, then `$HSMP_GAME_DIR\HalfswordUE5\Binaries\Win64`,
   then the main worktree's `game\HalfswordUE5\Binaries\Win64`. `--native-dir` is the `out/`
   folder of the HSMPNative CMake build (`crates/hsmp-native/README.md`); it is required because
   `mods.release.txt` enables `HSMPNative`. Its files (`release.json` `native_mods`) are sealed in
   the manifest like every other file.

   The tool runs a reproducible `cargo build --release --locked` of `server/Cargo.toml` and
   `launcher/Cargo.toml` into `target/release-repro/server/` and `target/release-repro/launcher/`,
   then writes to `target/dist/` (or `--out DIR`):

   - `hsmp-<version>/` (the unpacked package),
   - `hsmp-<version>.zip`,
   - `hsmp-<version>.zip.sha256`.

   Reproducible means:
   - every local path prefix (the user profile, `CARGO_HOME`, `RUSTUP_HOME`, the repo and the
     target folder) is remapped with `--remap-path-prefix`, so no user name or folder ends up in
     a binary;
   - on MSVC the linker runs with `/Brepro` (no timestamp) and `/PDBALTPATH:%_PDB%` (no PDB path);
   - `SOURCE_DATE_EPOCH` is the commit time, `CARGO_INCREMENTAL=0`, `RUSTUP_TOOLCHAIN` is the pin,
     and any `RUSTFLAGS` of your shell are replaced, not merged;
   - the zip lists files sorted, with the commit time as the only timestamp.

   It **refuses** to build when:
   - the tree is dirty or has untracked files (`--allow-dirty` makes a test build, with a warning);
   - `--commit` is not HEAD (unless `--no-build`);
   - the signing key is not in the committed `launcher/trusted_keys.txt` (the launcher in that
     release would refuse its own release);
   - `UE4SS.dll` or `dwmapi.dll` does not match the pin, or an `include` folder is empty;
   - the toolchain is not the pinned one;
   - a binary listed in `release.json` `binaries`, an enabled mod's `main.lua`, or an enabled
     native mod's files under `--native-dir`, is missing;
   - `mods.release.txt` enables a retired mod or no HSMP mod at all;
   - `master_urls` is loopback-only (step 3);
   - `--no-build` binaries are used for a non-`dev` channel without `--allow-prebuilt` (nothing
     would tie them to the commit).
9. **Reproducibility check.** Build the same commit twice, ideally in two different folders (or
   by two people), with the same UE4SS input, and compare: `hsmp-server.exe`,
   `hsmp-sidecar.exe`, `hsmp-launcher.exe`, `manifest.json` and the zip must be byte-identical
   (`Get-FileHash`). Only `manifest.sig` needs the private key; with the same key it is identical
   too. What runs automatically is narrower: `repro_build_is_independent_of_the_build_folder`
   (in `cargo test --workspace`) builds a small **probe** crate in two folders and requires
   identical executables with no build path in them, and `builds_reproducibly_and_verifies`
   packs a fixture twice. Those prove the settings, not the shipped binaries, so do the
   two-build comparison by hand for every release until CI does it.
10. **Verify** like a player would:

    ```powershell
    cargo run --release -p hsmp-release -- verify target/dist/hsmp-<version>.zip
    ```

    It checks the signature against `launcher/trusted_keys.txt` and every file's hash.
11. **Smoke test on a clean PC or VM** (Steam with Half Sword, no developer tools). Extract,
    install, and play one match started with **Launch through Steam** and one started from Steam
    directly. Both times `Win64\hsmp_state\` must appear and the HSMP log must show `hsmp.cfg`
    being read; both routes rely on the engine
    switching its working directory to `Win64`, which no automated test checks. Then uninstall and check that `Win64` is back to the Steam
    files (Steam's "Verify integrity" should re-acquire 0 files, or compare the folder before and
    after).
12. **Publish** on the GitHub release page of `Cyrex0/HalfSword-MP`. The launcher's update check
    depends on these names exactly:
    * the tag is `v<version>` (e.g. `v0.2.0`, `v0.2.0-beta.1`), with `<version>` the manifest's;
    * the assets are `hsmp-<version>.zip` and `hsmp-<version>.zip.sha256`, as `hsmp-release`
      writes them;
    * a beta is marked **pre-release** (a `-` in the version also counts as one). Players on
      "Stable releases only" never see it. Drafts are never offered.

    A release without both assets, or with another tag form, is skipped by the update check.

## 5. What is in the package

| Package path | Installed to | Source |
|---|---|---|
| `manifest.json`, `manifest.sig` | (not installed) | built by `hsmp-release` |
| `hsmp-launcher.exe` | run from the extracted folder; a copy goes to `%LOCALAPPDATA%\HSMP\bin\` | `launcher/` |
| `payload/Win64/hsmp/<bin>.exe` | `HalfswordUE5/Binaries/Win64/hsmp/` | `release.json` `binaries`: `hsmp-server`, `hsmp-sidecar`, `hsmp-master`, `hsmp-query` |
| `payload/Win64/ue4ss/Mods/<Mod>/Scripts/*.lua` | `.../Win64/ue4ss/Mods/<Mod>/Scripts/` | every `HSMP*` mod marked `1` in `mods/mods.release.txt`, plus `mods/shared/*.lua` (the shared copy wins over a private one) |
| `payload/Win64/ue4ss/Mods/HSMPNative/dlls/main.dll`, `hsmp_lua.dll` | `.../Win64/ue4ss/Mods/HSMPNative/dlls/` | `release.json` `native_mods`, from `--native-dir` |
| `payload/Win64/dwmapi.dll`, `payload/Win64/ue4ss/...` | `.../Win64/` | the UE4SS `include` list, with `settings_overrides` applied to `UE4SS-settings.ini` |
| `payload/mods.release.txt` | merged into `ue4ss/Mods/mods.txt` | `mods/mods.release.txt` |
| `INSTALL.md` | (not installed) | `docs/players/install.md` at the release commit |
| `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `THIRD-PARTY-NOTICES.html` | (not installed) | the repo root at the release commit |

Developer mods (`mods/dev/`), Lua in subfolders, and test files never ship.

**Licence files.** The zip carries HSMP's `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` and
`THIRD-PARTY-NOTICES.html`, and UE4SS's own MIT licence as `ue4ss/LICENSE` (part of the UE4SS
`include` list). `THIRD-PARTY-NOTICES.html` is committed at the repository root and holds the full
licence texts of the statically linked Rust crates, Lua 5.4.7 (linked into HSMPNative), the fonts
embedded in the launcher, and the list of components bundled inside `UE4SS.dll`. It is generated
by `cargo-about` from `about.toml` (accepted licences, Windows target, no dev-dependencies) and
`about.hbs` (the template, with the hand-kept Lua, font and UE4SS sections). Regenerate and
commit it whenever `Cargo.lock` changes:

```powershell
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo about generate --workspace --locked --fail -o THIRD-PARTY-NOTICES.html about.hbs
```

The scan covers the whole workspace, a superset of the shipped binaries. `--fail` stops on a
crate whose licence is not in `about.toml`'s `accepted` list.

### The manifest

`manifest.json` is signed as raw bytes by `manifest.sig` (ed25519, with a 16-hex key id). It
contains:

- `version`, `channel`, `git_commit`, `source_date_epoch`, `protocol_version`, `toolchain`;
- `game` (Steam app id and the supported builds with their exe SHA-256);
- `ue4ss` (the pin: version label, DLL and proxy hashes);
- `master_urls`, `launch_args`, `ini_settings` and `mods_template`;
- `files[]`: `{path, sha256, size, role, install}` for every file in the zip.

`launch_args` and `ini_settings` carry the workaround for the IoDispatcher pak-read crash
(`r.HairStrands.Streaming=0`: hair strands load whole instead of through paged streaming; see
[halfsword/io-dispatcher-crash.md](halfsword/io-dispatcher-crash.md)). **Launch through Steam**
(`ops::launch`) passes `launch_args` to `steam -applaunch`; for players who start from Steam
directly, the player docs offer it as an optional Steam launch option.

## 6. Launcher internals

Code: `launcher/src/` (the module table is at the top of `lib.rs`). The GUI (`gui.rs`, egui) is
behind the default `gui` feature; `hsmp-release` depends on the launcher library with
`default-features = false`, so the manifest, signature and packing code exist only once.

| Module | Job |
|---|---|
| `steam`, `vdf` | find Steam and the Half Sword install (`libraryfolders.vdf`, `appmanifest`) |
| `game` | game layout, exe hash against the supported builds, conflicting UE4SS layouts |
| `manifest`, `sign`, `package`, `pack` | the signed manifest: schema, ed25519, verification, writing |
| `trust` | which keys count; the launcher copy used for updates |
| `install` | journaled install, update, uninstall and status; the `Engine.ini` merge |
| `modstxt`, `cfgfile`, `ini` | merging `mods.txt`, `hsmp.cfg` and ini files |
| `saves` | career save backup and restore (never deletes a backup) |
| `careerguard` | recovers career-guard sessions a crash left open, at launcher start, after install/update, before Launch through Steam and before Uninstall (`hsmp-sidecar --career-recover`; `ops::startup_check`) |
| `launch`, `procs` | start the game through Steam (`steam -applaunch`, Steam's own install only); read-only process checks |
| `update` | update check (GitHub releases API), resumable download, SHA-256 + signature check, then the normal install |
| `crash` | crash-report consent and redaction (interface only, no network) |
| `ops` | the operations shared by the CLI and the GUI |

**Verification.** The launcher checks the signature before it parses anything (the manifest and
signature are read with fixed size caps), then the size and SHA-256 of every file, never reading
more than the signed size. It installs only from those verified in-memory bytes, and only to
targets under `HalfswordUE5/Binaries/Win64/`. `hsmp.cfg`, `mods.txt`, `hsmp_install.json` and the
game exe are reserved targets.

**Updates** (`update.rs`). At start and on "Check for updates" the launcher asks
`api.github.com/repos/Cyrex0/HalfSword-MP/releases` (or `/releases/latest` with "Stable releases
only"), unauthenticated, with a `User-Agent`, over rustls (`ureq`, HTTPS only). The answer is
cached with its ETag in `%LOCALAPPDATA%\HSMP\launcher\update_check.json`, so a repeat check is a
`304` that does not count against GitHub's 60 requests per hour; a used-up limit is reported with
its reset time. The newest non-draft `v<version>` release wins by semver order (pre-releases
before their release). The zip is downloaded to `%LOCALAPPDATA%\HSMP\downloads\` through a `.part`
file (resumed with `Range`, 4 attempts, capped at 512 MB and at the asset's listed size), checked
against the `.zip.sha256`, then opened with `Package::open` against the compiled-in keys. A
downgrade (unless allowed under Advanced) and a release that does not support the game build are
refused; the game, sidecar or server running from the folder refuses the update; the career guard
recovers first; then `ops::install_with` runs the journaled install. The new launcher is copied to
`%LOCALAPPDATA%\HSMP\bin\` (renaming a running copy away first) and "Restart the launcher" starts
it; at start that copy opens `downloads\hsmp-<its version>.zip` when no release sits next to it.
Tests use a local HTTP server, never the network.

**Journaled install and rollback** (`install.rs`):

- Every change is recorded before it is made. The first time HSMP writes a game file, the
  journal stores the file's original bytes (a content-addressed blob) or "absent"; uninstall puts
  those bytes back under the original name, or deletes the file. Keys are case-folded relative
  paths, because Windows file names are case-insensitive.
- Folders HSMP created are listed and removed on uninstall (runtime leftovers are moved to an
  archive folder, never deleted). The exact `Engine.ini` edits are recorded and reverted.
- A running install or update is a transaction persisted before each write. Any failure, or a
  crash found on the next start, rolls every step back to the previous state.
- Store: `%LOCALAPPDATA%\HSMP\launcher\<install id>\state.json` plus `blobs\<sha256>.hsmpblob`
  (masked, so antivirus does not see a backed-up UE4SS proxy as a PE file). The install id is
  written into the game folder (`Win64\hsmp_install.json`, with the Steam app id and the exe
  hash), so the journal is found again after Steam moves the library; a journal is used only for
  a folder whose files match it.
- `mods.txt`, `hsmp.cfg` and `Engine.ini` are edited in their own encoding (UTF-8, UTF-16 or a
  local code page), so other mods' lines are kept byte for byte. An old-layout UE4SS
  (`UE4SS.dll` directly in `Win64`) would load instead of the pinned build, so the launcher
  refuses to install over it.
- Career saves are never written by an install: the launcher only copies them
  (`%LOCALAPPDATA%\HSMP\save_backups\`).

**Command line** (`hsmp-launcher.exe <command>`; without a command the window opens):

```
status        [--game DIR] [--package DIR|ZIP]   game, package, build check, install status, backups
verify        [--package DIR|ZIP]                signature + SHA-256 of every file
install       [--game DIR] [--package DIR|ZIP] [--allow-unsupported] [--no-save-backup] [--allow-downgrade]
uninstall     [--game DIR] [--forget-missing]
launch        [--game DIR]                       steam -applaunch with the release's launch_args
check-update  [--stable-only]
update        [--game DIR] [--stable-only] [--allow-downgrade]
backup-saves | list-backups | restore-saves <id> | find-game
```

**Tests** (`cargo test -p hsmp-launcher -p hsmp-release`, part of `cargo test --workspace`)
never touch the real game folder, the real `%LOCALAPPDATA%` or the real saves. They cover Steam
discovery fixtures, manifest and signature tampering, folder and zip packages, byte-identical
install and uninstall into a fake game (clean, with existing UE4SS and third-party mods, update
then uninstall), rollback at every stage and crash recovery, non-ASCII paths and encodings,
case-only renames, moved and copied game folders, the operation lock and refusals, `Engine.ini`,
`mods.txt` and `hsmp.cfg` merging, save backup and restore, crash redaction, and the reproducible
release build.

The developer deploy (`scripts/build-and-deploy.ps1`) and the launcher can coexist: if the
launcher installs over a developer deploy, it treats those files as originals and restores them
on uninstall.

## 7. Not done yet

- **Crash upload.** Consent, detection, redaction and local export exist
  (`launcher/src/crash.rs`, the `CrashSink` trait); nothing uploads.
- **Code signing** (Authenticode) of the launcher and the binaries, which would remove the
  SmartScreen prompt and most antivirus false positives.
- **Version agreement check** between the tag, `release.json` and the workspace version (§1).

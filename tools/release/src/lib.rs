//! hsmp-release: build the signed HSMP release package from a git commit.
//!
//! Inputs (everything that is not in git is pinned by hash):
//!
//! * the commit (default HEAD; the tree must be clean): Lua mods
//!   (`mods/<Mod>/Scripts/*.lua` + `shared/*.lua`, the same rule as
//!   scripts/build-and-deploy.ps1), `mods/mods.release.txt`,
//!   `tools/release/release.json`, `docs/players/install.md`, the protocol version
//!   (`crates/hsmp-net/src/net/mod.rs`) and `launcher/trusted_keys.txt`,
//!   all read from git objects (not the working tree), so line-ending
//!   settings or stray edits cannot change the package;
//! * the pinned UE4SS build (`--ue4ss-dir`): only the files listed in
//!   release.json, with `UE4SS.dll` / `dwmapi.dll` checked against the pin
//!   and the release overrides applied to `UE4SS-settings.ini`;
//! * the Rust binaries and the launcher exe, built reproducibly
//!   (`repro_env`: `--remap-path-prefix` for every local prefix, `/Brepro`,
//!   `SOURCE_DATE_EPOCH` = commit time, the toolchain pinned in release.json,
//!   `cargo --locked`), or taken from `--bin-dir` / `--launcher-exe`
//!   (non-dev channels then need `--allow-prebuilt`);
//! * the signing key from `HSMP_RELEASE_SIGNING_KEY` (64 hex) or
//!   `HSMP_RELEASE_SIGNING_KEY_FILE` (never from the repo).
//!
//! Same inputs -> byte-identical manifest.json, manifest.sig and zip
//! (sorted entries, commit time as the only timestamp).

use hsmp_launcher::manifest::{GameCompat, IniSetting, Manifest, Role, Ue4ssInfo, PRODUCT, SCHEMA};
use hsmp_launcher::pack::{self, Item};
use hsmp_launcher::{ini, modstxt, sign, util};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CONFIG_PATH: &str = "tools/release/release.json";
pub const KEY_ENV: &str = "HSMP_RELEASE_SIGNING_KEY";
pub const KEY_FILE_ENV: &str = "HSMP_RELEASE_SIGNING_KEY_FILE";
const WIN64: &str = "HalfswordUE5/Binaries/Win64";
/// Retired mods (scripts/build-and-deploy.ps1 never deploys them).
const RETIRED: &[&str] = &["HSMPLobby", "HSMPAdmin", "HSMPCharacter", "HSMPSettings", "HSMPChat"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingOverride {
    pub section: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ue4ssPin {
    pub version: String,
    pub dll_sha256: String,
    pub proxy_sha256: String,
    /// Paths relative to the Win64 folder; a trailing '/' takes the folder recursively.
    pub include: Vec<String>,
    pub settings_overrides: Vec<SettingOverride>,
}

/// `tools/release/release.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: String,
    pub channel: String,
    pub master_urls: Vec<String>,
    pub game: GameCompat,
    pub ue4ss: Ue4ssPin,
    pub binaries: Vec<String>,
    pub launch_args: Vec<String>,
    pub ini_settings: Vec<IniSetting>,
    /// Pinned Rust toolchain for the release binaries (RUSTUP_TOOLCHAIN), e.g. "1.98.1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_toolchain: Option<String>,
    /// UE4SS C++ mods built from this repository (HSMPNative: crates/hsmp-native, the game
    /// side of the shared-memory IPC). Enabled ones ("Name : 1" in mods.release.txt) ship
    /// their files from `--native-dir` (the CMake `out/` folder), sealed in the manifest like
    /// every other file, so the launcher pins their hashes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_mods: Vec<NativeMod>,
    /// The master's `/v1/reports` endpoint is live: the launcher offers "Upload to HSMP".
    /// Copied into the manifest; false (the default) hides the button.
    #[serde(default)]
    pub report_upload: bool,
}

/// One UE4SS C++ mod: `<native-dir>/<name>/<file>` -> `ue4ss/Mods/<name>/<file>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeMod {
    pub name: String,
    /// Paths relative to the mod folder, e.g. "dlls/main.dll".
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildOpts {
    pub repo: PathBuf,
    pub commit: String,
    pub ue4ss_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub launcher_exe: PathBuf,
    /// The CMake `out/` folder of crates/hsmp-native (`<name>/dlls/*.dll`), for the enabled
    /// `native_mods` of release.json.
    pub native_dir: Option<PathBuf>,
    pub allow_dirty: bool,
    pub allow_loopback_master: bool,
    /// The binaries were NOT built by this run (`--no-build`).
    pub prebuilt: bool,
    /// The maintainer vouches for prebuilt binaries on a non-dev channel.
    pub allow_prebuilt: bool,
    /// `rustc -V` of the toolchain that built the binaries (recorded in the manifest).
    pub toolchain: String,
}

pub struct Built {
    pub manifest: Manifest,
    pub items: BTreeMap<String, Item>,
    pub manifest_bytes: Vec<u8>,
    pub sig_bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

impl Built {
    pub fn top(&self) -> String {
        format!("hsmp-{}", self.manifest.version)
    }
    pub fn zip(&self) -> Result<Vec<u8>, String> {
        pack::zip_bytes(&self.top(), &self.items, &self.manifest_bytes, &self.sig_bytes, self.manifest.source_date_epoch)
    }
}

pub fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = Command::new("git").arg("-C").arg(repo).args(args).output().map_err(|e| format!("git not runnable: {e}"))?;
    if !o.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(o.stdout)
}

fn git_str(repo: &Path, args: &[&str]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&git(repo, args)?).trim().to_string())
}

/// File bytes at `commit:path`.
pub fn git_file(repo: &Path, commit: &str, path: &str) -> Result<Vec<u8>, String> {
    git(repo, &["cat-file", "blob", &format!("{commit}:{path}")]).map_err(|e| format!("{path} at {commit}: {e}"))
}

/// File paths under `dir` at `commit` (recursive, sorted).
pub fn git_ls(repo: &Path, commit: &str, dir: &str) -> Result<Vec<String>, String> {
    let out = git_str(repo, &["ls-tree", "-r", "--name-only", "--full-tree", commit, "--", dir])?;
    let mut v: Vec<String> = out.lines().map(str::to_string).filter(|s| !s.is_empty()).collect();
    v.sort();
    Ok(v)
}

/// `pub const PROTOCOL_VERSION: u16 = 5;` -> 5
pub fn parse_protocol(src: &str) -> Option<u32> {
    for l in src.lines() {
        let t = l.trim();
        if t.starts_with("pub const PROTOCOL_VERSION") {
            let v = t.split('=').nth(1)?.trim().trim_end_matches(';').trim();
            return v.parse().ok();
        }
    }
    None
}

pub fn load_config(bytes: &[u8]) -> Result<Config, String> {
    let c: Config = serde_json::from_slice(bytes).map_err(|e| format!("{CONFIG_PATH}: {e}"))?;
    if c.version.trim().is_empty() || !c.version.chars().all(|ch| ch.is_ascii_alphanumeric() || ".-+".contains(ch)) {
        return Err(format!("{CONFIG_PATH}: version '{}' must be like 0.3.1 or 0.3.1-beta.2", c.version));
    }
    for i in &c.ue4ss.include {
        if !util::is_safe_rel(i.trim_end_matches('/')) {
            return Err(format!("{CONFIG_PATH}: unsafe ue4ss include '{i}'"));
        }
    }
    Ok(c)
}

fn is_loopback(u: &str) -> bool {
    let h = u.split("://").nth(1).unwrap_or("").split([':', '/']).next().unwrap_or("");
    h == "localhost" || h.starts_with("127.") || h == "0.0.0.0"
}

/// Signing key from the environment (never from the repo).
pub fn signing_key_from_env() -> Result<ed25519_dalek::SigningKey, String> {
    if let Ok(v) = std::env::var(KEY_ENV) {
        return sign::parse_signing_key(&v).map_err(|e| format!("{KEY_ENV}: {e}"));
    }
    if let Ok(p) = std::env::var(KEY_FILE_ENV) {
        let t = std::fs::read_to_string(&p).map_err(|e| format!("{KEY_FILE_ENV} ({p}): {e}"))?;
        return sign::parse_signing_key(&t).map_err(|e| format!("{KEY_FILE_ENV}: {e}"));
    }
    Err(format!("no signing key: set {KEY_ENV} (64 hex) or {KEY_FILE_ENV} (path to a file holding it). Create one with `hsmp-release keygen --out <file outside the repo>`."))
}

fn collect_ue4ss(cfg: &Ue4ssPin, dir: &Path, items: &mut BTreeMap<String, Item>) -> Result<(), String> {
    let proxy = dir.join("dwmapi.dll");
    let dll = dir.join("ue4ss").join("UE4SS.dll");
    for (p, want, what) in [(&proxy, &cfg.proxy_sha256, "dwmapi.dll"), (&dll, &cfg.dll_sha256, "ue4ss/UE4SS.dll")] {
        let (h, _) = util::sha256_file(p).map_err(|e| format!("UE4SS input {}: {e} (pass --ue4ss-dir <Win64 folder of the pinned UE4SS build>)", p.display()))?;
        if &h != want {
            return Err(format!("UE4SS input {what} has sha256 {h}, but release.json pins {want}. Use the pinned build, or update the pin deliberately."));
        }
    }
    let mut rels: Vec<String> = vec![];
    for inc in &cfg.include {
        if let Some(d) = inc.strip_suffix('/') {
            let files = util::list_files_rel(&util::join_rel(dir, d)).map_err(|e| format!("{inc}: {e}"))?;
            if files.is_empty() {
                return Err(format!("UE4SS include {inc} is empty or missing in {}", dir.display()));
            }
            rels.extend(files.into_iter().map(|f| format!("{d}/{f}")));
        } else {
            rels.push(inc.clone());
        }
    }
    for rel in rels {
        let p = util::join_rel(dir, &rel);
        let mut bytes = std::fs::read(&p).map_err(|e| format!("UE4SS input {}: {e}", p.display()))?;
        if rel.eq_ignore_ascii_case("ue4ss/UE4SS-settings.ini") {
            let mut t = String::from_utf8(bytes).map_err(|_| "UE4SS-settings.ini is not UTF-8".to_string())?;
            for o in &cfg.settings_overrides {
                t = ini::set_key(&t, &o.section, &o.key, &o.value);
            }
            bytes = t.into_bytes();
        }
        items.insert(format!("payload/Win64/{rel}"), Item { bytes, role: Role::Ue4ss, install: Some(format!("{WIN64}/{rel}")) });
    }
    Ok(())
}

/// Collect every input and seal the manifest. Pure apart from reading git,
/// the UE4SS folder and the binaries.
pub fn build(o: &BuildOpts, key: &ed25519_dalek::SigningKey) -> Result<Built, String> {
    let repo = &o.repo;
    let commit = git_str(repo, &["rev-parse", "--verify", &format!("{}^{{commit}}", o.commit)])?;
    let mut warnings = vec![];
    // untracked (not ignored) files count: an untracked .rs module would be
    // compiled into the binaries without being in the commit
    let status = git_str(repo, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.is_empty() {
        if !o.allow_dirty {
            return Err(format!(
                "the working tree has uncommitted or untracked files; commit or remove them (a release is built from a commit) or pass --allow-dirty for a test build:\n{}",
                status.lines().take(10).collect::<Vec<_>>().join("\n")
            ));
        }
        warnings.push("built with --allow-dirty: binaries may not match the commit".into());
    }
    let epoch: u64 = git_str(repo, &["show", "-s", "--format=%ct", &commit])?.parse().map_err(|_| "cannot read the commit time")?;
    let cfg = load_config(&git_file(repo, &commit, CONFIG_PATH)?)?;
    if o.prebuilt {
        if !o.allow_prebuilt && cfg.channel != "dev" {
            return Err("--no-build takes binaries this run did not build, so nothing ties them to the commit. For a non-dev release, build them here (drop --no-build), or pass --allow-prebuilt if you built them from this exact commit with `hsmp-release` settings.".into());
        }
        warnings.push("prebuilt binaries (--no-build): not built from the commit by this run".into());
    }
    let protocol = parse_protocol(&String::from_utf8_lossy(&git_file(repo, &commit, "crates/hsmp-net/src/net/mod.rs")?)).ok_or("PROTOCOL_VERSION not found in crates/hsmp-net/src/net/mod.rs")?;

    // the launcher in this package must trust this key
    let trusted = sign::parse_trusted_keys(&String::from_utf8_lossy(&git_file(repo, &commit, "launcher/trusted_keys.txt")?))?;
    let kid = sign::key_id(&key.verifying_key());
    if !trusted.iter().any(|k| k.id == kid) {
        return Err(format!(
            "launcher/trusted_keys.txt (at {}) does not list the signing key {kid}: the launcher in this release would refuse its own release. Add the line from `hsmp-release pubkey` and commit it.",
            &commit[..10]
        ));
    }
    // A list on another machine is only trusted over TLS: plain HTTP lets anyone on the path
    // rewrite it and send players to another server.
    if let Some(u) = cfg.master_urls.iter().find(|u| !is_loopback(u) && !u.starts_with("https://")) {
        return Err(format!("release.json master_urls: '{u}' is not https:// (only this-machine masters may use http)"));
    }
    if cfg.master_urls.iter().all(|u| is_loopback(u)) {
        if !o.allow_loopback_master && cfg.channel != "dev" {
            return Err("release.json master_urls only lists this-machine addresses, so players would see no internet servers. Set the public master URL, or pass --allow-loopback-master (LAN-only test build).".into());
        }
        warnings.push("master_urls is loopback only: the server browser shows LAN servers only".into());
    }

    let mut items: BTreeMap<String, Item> = BTreeMap::new();
    // mods.txt template
    let template_bytes = git_file(repo, &commit, "mods/mods.release.txt")?;
    let template = modstxt::parse_template(&String::from_utf8_lossy(&template_bytes))?;
    items.insert("payload/mods.release.txt".into(), Item { bytes: template_bytes, role: Role::Template, install: None });

    // HSMP Lua mods enabled in the release profile + shared libraries
    let shared: Vec<String> = git_ls(repo, &commit, "mods/shared")?.into_iter().filter(|p| p.ends_with(".lua") && p.matches('/').count() == 2).collect();
    let mut mod_count = 0;
    for e in template.iter().filter(|e| e.value == "1" && e.name.starts_with("HSMP")) {
        if RETIRED.contains(&e.name.as_str()) {
            return Err(format!("mods.release.txt enables retired mod {}", e.name));
        }
        if let Some(nm) = cfg.native_mods.iter().find(|n| n.name == e.name) {
            let dir = o.native_dir.as_ref().ok_or_else(|| {
                format!("{} (a UE4SS C++ mod) is enabled in mods.release.txt: pass --native-dir <crates/hsmp-native CMake out folder>", e.name)
            })?;
            for f in &nm.files {
                if !util::is_safe_rel(f) {
                    return Err(format!("{CONFIG_PATH}: unsafe native mod file '{f}'"));
                }
                let p = util::join_rel(&dir.join(&nm.name), f);
                let bytes = std::fs::read(&p).map_err(|e| format!("native mod file {}: {e}", p.display()))?;
                let rel = format!("ue4ss/Mods/{}/{f}", nm.name);
                items.insert(format!("payload/Win64/{rel}"), Item { bytes, role: Role::Mod, install: Some(format!("{WIN64}/{rel}")) });
            }
            continue;
        }
        let base = format!("mods/{}/Scripts", e.name);
        let files: Vec<String> = git_ls(repo, &commit, &base)?.into_iter().filter(|p| p.ends_with(".lua") && p[base.len() + 1..].matches('/').count() == 0).collect();
        if !files.iter().any(|f| f.ends_with("/main.lua")) {
            return Err(format!("{} is enabled in mods.release.txt but {base}/main.lua is not in the commit", e.name));
        }
        for src in files.iter().chain(shared.iter()) {
            let name = src.rsplit('/').next().unwrap();
            let rel = format!("ue4ss/Mods/{}/Scripts/{name}", e.name);
            // shared copy wins over a private copy of the same name (deploy rule)
            items.insert(format!("payload/Win64/{rel}"), Item { bytes: git_file(repo, &commit, src)?, role: Role::Mod, install: Some(format!("{WIN64}/{rel}")) });
        }
        mod_count += 1;
    }
    if mod_count == 0 {
        return Err("mods.release.txt enables no HSMP mod".into());
    }

    // The build identity of this payload: hsmp_build_id.lua in every Lua mod (checked against
    // `hsmp-sidecar --build-info` at startup) and hsmp/build.json (the installed version, for
    // the launcher). The content hash comes from the git objects, like every shipped file.
    let identity = build_identity(repo, &commit, &cfg.version, protocol)?;
    let lua_mods: Vec<String> = items
        .keys()
        .filter_map(|k| k.strip_prefix("payload/Win64/ue4ss/Mods/")?.strip_suffix("/Scripts/main.lua").map(str::to_string))
        .filter(|m| m.starts_with("HSMP"))
        .collect();
    for m in lua_mods {
        let rel = format!("ue4ss/Mods/{m}/Scripts/hsmp_build_id.lua");
        items.insert(format!("payload/Win64/{rel}"), Item { bytes: identity.to_lua().into_bytes(), role: Role::Mod, install: Some(format!("{WIN64}/{rel}")) });
    }
    items.insert(
        "payload/Win64/hsmp/build.json".into(),
        Item { bytes: format!("{}\n", identity.to_json()).into_bytes(), role: Role::Bin, install: Some(format!("{WIN64}/hsmp/build.json")) },
    );

    collect_ue4ss(&cfg.ue4ss, &o.ue4ss_dir, &mut items)?;

    for b in &cfg.binaries {
        let p = o.bin_dir.join(format!("{b}.exe"));
        let bytes = std::fs::read(&p).map_err(|e| format!("binary {}: {e} (build with `cargo build --release --locked -p hsmp-server`, or pass --bin-dir)", p.display()))?;
        items.insert(format!("payload/Win64/hsmp/{b}.exe"), Item { bytes, role: Role::Bin, install: Some(format!("{WIN64}/hsmp/{b}.exe")) });
    }
    let launcher = std::fs::read(&o.launcher_exe).map_err(|e| format!("launcher {}: {e}", o.launcher_exe.display()))?;
    items.insert("hsmp-launcher.exe".into(), Item { bytes: launcher, role: Role::Launcher, install: None });
    items.insert("INSTALL.md".into(), Item { bytes: git_file(repo, &commit, "docs/players/install.md")?, role: Role::Doc, install: None });
    // Licence and notice texts travel with the binaries (MIT / Apache-2.0 / third-party notices).
    for doc in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.html"] {
        items.insert(doc.into(), Item { bytes: git_file(repo, &commit, doc)?, role: Role::Doc, install: None });
    }

    let manifest = Manifest {
        schema: SCHEMA,
        product: PRODUCT.into(),
        version: cfg.version.clone(),
        channel: cfg.channel.clone(),
        git_commit: commit.clone(),
        source_date_epoch: epoch,
        protocol_version: protocol,
        toolchain: o.toolchain.clone(),
        game: cfg.game.clone(),
        ue4ss: Ue4ssInfo { version: cfg.ue4ss.version.clone(), dll_sha256: cfg.ue4ss.dll_sha256.clone(), proxy_sha256: cfg.ue4ss.proxy_sha256.clone() },
        master_urls: cfg.master_urls.clone(),
        launch_args: cfg.launch_args.clone(),
        ini_settings: cfg.ini_settings.clone(),
        mods_template: "payload/mods.release.txt".into(),
        files: vec![],
        report_upload: cfg.report_upload,
    };
    let (manifest_bytes, sig_bytes) = pack::seal(manifest, &items, key)?;
    let manifest = Manifest::parse(&manifest_bytes)?;
    Ok(Built { manifest, items, manifest_bytes, sig_bytes, warnings })
}

/// Content-hash source over the git objects of one commit.
struct GitSource<'a> {
    repo: &'a Path,
    commit: &'a str,
}

impl hsmp_net::build::content::Source for GitSource<'_> {
    fn files(&self, dir: &str) -> Vec<String> {
        git_ls(self.repo, self.commit, dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.len() > dir.len() + 1 && p.starts_with(dir) && !p[dir.len() + 1..].contains('/'))
            .collect()
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        git_file(self.repo, self.commit, path).ok()
    }
}

/// The identity of the mods in `commit`: release version, protocol and IPC ABI of this
/// tree, content hash of the commit's files.
pub fn build_identity(repo: &Path, commit: &str, version: &str, protocol: u32) -> Result<hsmp_net::build::Identity, String> {
    let (content_hash, _) = hsmp_net::build::content::content_hash(&GitSource { repo, commit }).ok_or("mods/mods.release.txt missing from the commit")?;
    Ok(hsmp_net::build::Identity {
        version: version.to_string(),
        protocol: protocol as u16,
        proto_min: hsmp_net::net::VERSION_MIN,
        proto_max: hsmp_net::net::VERSION_MAX,
        ipc_abi_major: hsmp_ipc::ABI_MAJOR,
        ipc_abi_minor: hsmp_ipc::ABI_MINOR,
        ipc_layout: hsmp_ipc::segment::LAYOUT_HASH,
        content_hash,
    })
}

/// A path as rustc and cargo see it: absolute, canonical, without `\\?\`.
pub fn plain_abs(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = c.to_string_lossy().to_string();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(r) => PathBuf::from(format!(r"\\{r}")),
        None => PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)),
    }
}

fn home_of(var: &str, sub: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(sub)))
}

/// rustc flags that make the binaries independent of WHERE and BY WHOM they
/// were built: every local path prefix is remapped (rustc uses the LAST
/// matching prefix, so the most specific comes last), and on MSVC the
/// linker writes no timestamp (/Brepro) and only the PDB's file name.
pub fn repro_rustflags(repo: &Path, target_dir: &Path) -> Vec<String> {
    let mut f = vec![];
    let mut remap = |from: Option<PathBuf>, to: &str| {
        if let Some(p) = from {
            f.push(format!("--remap-path-prefix={}={to}", plain_abs(&p).display()));
        }
    };
    remap(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from), "/home");
    remap(home_of("CARGO_HOME", ".cargo"), "/cargo");
    remap(home_of("RUSTUP_HOME", ".rustup"), "/rustup");
    remap(Some(repo.to_path_buf()), "/hsmp");
    remap(Some(target_dir.to_path_buf()), "/target");
    if cfg!(target_env = "msvc") {
        f.extend(["-C".into(), "link-arg=/Brepro".into(), "-C".into(), "link-arg=/PDBALTPATH:%_PDB%".into()]);
    }
    f
}

/// Environment for a reproducible `cargo build`.
pub fn repro_env(repo: &Path, target_dir: &Path, epoch: u64, toolchain: Option<&str>) -> Vec<(String, String)> {
    let mut v = vec![
        ("CARGO_ENCODED_RUSTFLAGS".to_string(), repro_rustflags(repo, target_dir).join("\x1f")),
        ("SOURCE_DATE_EPOCH".to_string(), epoch.to_string()),
        ("CARGO_TARGET_DIR".to_string(), plain_abs(target_dir).display().to_string()),
        ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
    ];
    if let Some(t) = toolchain {
        v.push(("RUSTUP_TOOLCHAIN".to_string(), t.to_string()));
    }
    v
}

/// `cargo build --release --locked` with `repro_env` (RUSTFLAGS from the
/// caller's environment are replaced, never merged).
pub fn cargo_build_repro(cargo: &Path, manifest: &Path, repo: &Path, target_dir: &Path, epoch: u64, toolchain: Option<&str>, locked: bool) -> Result<(), String> {
    std::fs::create_dir_all(target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;
    let mut c = Command::new(cargo);
    c.args(["build", "--release", "--quiet"]);
    if locked {
        c.arg("--locked");
    }
    c.arg("--manifest-path").arg(plain_abs(manifest)).current_dir(plain_abs(manifest.parent().unwrap_or(repo)));
    c.env_remove("RUSTFLAGS").env_remove("CARGO_BUILD_RUSTFLAGS");
    for (k, v) in repro_env(repo, target_dir, epoch, toolchain) {
        c.env(k, v);
    }
    let o = c.output().map_err(|e| format!("cargo: {e}"))?;
    if !o.status.success() {
        return Err(format!("cargo build failed for {}:\n{}", manifest.display(), String::from_utf8_lossy(&o.stderr)));
    }
    Ok(())
}

/// `rustc -V` under the pinned toolchain; checks it really is that toolchain.
pub fn rustc_version(toolchain: Option<&str>) -> Result<String, String> {
    let mut c = Command::new("rustc");
    c.arg("-V");
    if let Some(t) = toolchain {
        c.env("RUSTUP_TOOLCHAIN", t);
    }
    let o = c.output().map_err(|e| format!("rustc: {e}"))?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || v.is_empty() {
        return Err(format!("rustc -V failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    if let Some(t) = toolchain {
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) && !v.starts_with(&format!("rustc {t}")) {
            return Err(format!("release.json pins Rust {t}, but the toolchain is '{v}' (rustup toolchain install {t})"));
        }
    }
    Ok(v)
}

/// Write `<out>/hsmp-<v>/`, `<out>/hsmp-<v>.zip` and `<out>/hsmp-<v>.zip.sha256`.
pub fn write_outputs(b: &Built, out: &Path) -> Result<(PathBuf, PathBuf, String), String> {
    let dir = out.join(b.top());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    pack::write_dir(&dir, &b.items, &b.manifest_bytes, &b.sig_bytes).map_err(|e| format!("{}: {e}", dir.display()))?;
    let zip = b.zip()?;
    let sha = util::sha256_hex(&zip);
    let zp = out.join(format!("{}.zip", b.top()));
    util::atomic_write(&zp, &zip).map_err(|e| format!("{}: {e}", zp.display()))?;
    util::atomic_write(&out.join(format!("{}.zip.sha256", b.top())), format!("{sha}  {}.zip\n", b.top()).as_bytes()).map_err(|e| e.to_string())?;
    Ok((dir, zp, sha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_launcher::package::Package;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn w(p: &Path, s: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn run(repo: &Path, args: &[&str]) {
        let o = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    /// A tiny repo with the same layout as HSMP, plus a fake UE4SS and binaries.
    fn fixture(key: &ed25519_dalek::SigningKey) -> (Tmp, BuildOpts) {
        let root = std::env::temp_dir().join(format!("hsmp_release_test_{}_{}", std::process::id(), util::now_unix() % 100000 + rand_suffix()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let ue = root.join("ue4ss_build");
        let bins = root.join("bins");
        w(&ue.join("dwmapi.dll"), b"proxy");
        w(&ue.join("ue4ss/UE4SS.dll"), b"dll");
        w(&ue.join("ue4ss/UE4SS-settings.ini"), b"[Debug]\r\nConsoleEnabled = 1\r\nGuiConsoleEnabled = 1\r\n");
        w(&ue.join("ue4ss/Mods/BPModLoaderMod/Scripts/main.lua"), b"-- bpml");
        w(&ue.join("ue4ss/Mods/ConsoleEnablerMod/Scripts/main.lua"), b"-- dev, not shipped");
        w(&ue.join("ue4ss/UE4SS.pdb"), b"pdb, not shipped");
        for b in ["hsmp-server", "hsmp-sidecar"] {
            w(&bins.join(format!("{b}.exe")), format!("MZ {b}").as_bytes());
        }
        w(&bins.join("hsmp-launcher.exe"), b"MZ launcher");
        let cfg = serde_json::json!({
            "version": "0.9.0",
            "channel": "beta",
            "master_urls": ["https://master.example.net"],
            "game": { "steam_appid": 2397300, "builds": [ { "label": "test", "exe_sha256": util::sha256_hex(b"exe"), "exe_size": 3 } ] },
            "ue4ss": {
                "version": "test pin", "dll_sha256": util::sha256_hex(b"dll"), "proxy_sha256": util::sha256_hex(b"proxy"),
                "include": ["dwmapi.dll", "ue4ss/UE4SS.dll", "ue4ss/UE4SS-settings.ini", "ue4ss/Mods/BPModLoaderMod/"],
                "settings_overrides": [ { "section": "Debug", "key": "ConsoleEnabled", "value": "0" }, { "section": "Debug", "key": "GuiConsoleVisible", "value": "0" } ]
            },
            "binaries": ["hsmp-server", "hsmp-sidecar"],
            "launch_args": ["-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0"],
            "ini_settings": [ { "file": "Engine", "section": "SystemSettings", "key": "r.HairStrands.Streaming", "value": "0", "reason": "hair-strand streaming workaround" } ]
        });
        w(&repo.join(CONFIG_PATH), serde_json::to_string_pretty(&cfg).unwrap().as_bytes());
        w(&repo.join("crates/hsmp-net/src/net/mod.rs"), b"pub const PROTOCOL_VERSION: u16 = 5;\n");
        w(&repo.join("launcher/trusted_keys.txt"), format!("{} test\n", hex::encode(key.verifying_key().as_bytes())).as_bytes());
        w(&repo.join("docs/players/install.md"), b"# install\n");
        for doc in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.html"] {
            w(&repo.join(doc), doc.as_bytes());
        }
        w(&repo.join("mods/mods.release.txt"), b"BPModLoaderMod : 1\nHSMPMenu : 1\nHSMPDiag : dev\nHSMPOff : 0\nKeybinds : dev\n");
        w(&repo.join("mods/HSMPMenu/Scripts/main.lua"), b"-- menu\r\n");
        w(&repo.join("mods/HSMPMenu/Scripts/hsmp_cfg.lua"), b"-- stale private copy");
        w(&repo.join("mods/HSMPMenu/Scripts/sub/ignored.lua"), b"-- not top level");
        w(&repo.join("mods/HSMPMenu/tests/x.lua"), b"-- tests are not shipped");
        w(&repo.join("mods/dev/HSMPDiag/Scripts/main.lua"), b"-- diag (dev only)");
        w(&repo.join("mods/shared/hsmp_cfg.lua"), b"-- shared cfg");
        run(&repo, &["init", "-q"]);
        run(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
        run(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture"]);
        let o = BuildOpts {
            repo: repo.clone(),
            commit: "HEAD".into(),
            ue4ss_dir: ue,
            bin_dir: bins.clone(),
            launcher_exe: bins.join("hsmp-launcher.exe"),
            native_dir: None,
            allow_dirty: false,
            allow_loopback_master: false,
            prebuilt: true,
            allow_prebuilt: true,
            toolchain: "rustc 1.0.0 (test)".into(),
        };
        (Tmp(root), o)
    }

    fn rand_suffix() -> u64 {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).unwrap();
        u32::from_le_bytes(b) as u64
    }

    #[test]
    fn ships_enabled_native_mods_from_native_dir() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let (tmp, mut o) = fixture(&key);
        let cfg_path = o.repo.join(CONFIG_PATH);
        let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&cfg_path).unwrap()).unwrap();
        cfg["native_mods"] = serde_json::json!([{ "name": "HSMPNative", "files": ["dlls/main.dll", "dlls/hsmp_lua.dll"] }]);
        w(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap().as_bytes());
        let tpl = o.repo.join("mods/mods.release.txt");
        let t = std::fs::read_to_string(&tpl).unwrap().replace("HSMPMenu : 1\n", "HSMPNative : 1\nHSMPMenu : 1\n");
        w(&tpl, t.as_bytes());
        run(&o.repo, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
        run(&o.repo, &["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "native"]);
        let err = build(&o, &key).err().unwrap();
        assert!(err.contains("--native-dir"), "{err}");
        let nd = tmp.0.join("native_out");
        w(&nd.join("HSMPNative/dlls/main.dll"), b"MZ main");
        w(&nd.join("HSMPNative/dlls/hsmp_lua.dll"), b"MZ lua");
        o.native_dir = Some(nd);
        let b = build(&o, &key).unwrap();
        let f = b.manifest.files.iter().find(|f| f.path == "payload/Win64/ue4ss/Mods/HSMPNative/dlls/main.dll").expect("main.dll sealed");
        assert_eq!(f.sha256, util::sha256_hex(b"MZ main"));
        assert!(b.manifest.files.iter().any(|f| f.path.ends_with("HSMPNative/dlls/hsmp_lua.dll")));
    }

    #[test]
    fn protocol_parse() {
        assert_eq!(parse_protocol("x\npub const PROTOCOL_VERSION: u16 = 5;\n"), Some(5));
        assert_eq!(parse_protocol("nothing"), None);
    }

    #[test]
    fn builds_reproducibly_and_verifies() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let (tmp, o) = fixture(&key);
        let a = build(&o, &key).unwrap();
        let b = build(&o, &key).unwrap();
        assert_eq!(a.manifest_bytes, b.manifest_bytes);
        assert_eq!(a.sig_bytes, b.sig_bytes);
        assert_eq!(a.zip().unwrap(), b.zip().unwrap(), "byte-reproducible zip");
        let m = &a.manifest;
        assert_eq!(m.version, "0.9.0");
        assert_eq!(m.protocol_version, 5);
        assert_eq!(m.git_commit.len(), 40);
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua"));
        assert!(paths.contains(&"payload/Win64/hsmp/hsmp-server.exe"));
        assert!(!paths.iter().any(|p| p.contains("HSMPDiag")), "dev mods are not shipped");
        assert!(!paths.iter().any(|p| p.contains("ConsoleEnablerMod") || p.ends_with(".pdb")), "only included UE4SS files");
        assert!(!paths.iter().any(|p| p.contains("ignored.lua") || p.contains("tests/")));
        // shared copy wins, git bytes are used (CRLF preserved from the blob)
        assert_eq!(a.items["payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/hsmp_cfg.lua"].bytes, b"-- shared cfg");
        let settings = String::from_utf8(a.items["payload/Win64/ue4ss/UE4SS-settings.ini"].bytes.clone()).unwrap();
        assert_eq!(settings, "[Debug]\r\nConsoleEnabled = 0\r\nGuiConsoleEnabled = 1\r\nGuiConsoleVisible = 0\r\n");

        // the payload's build identity: one per Lua mod, plus hsmp/build.json for the launcher
        let lua = String::from_utf8(a.items["payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/hsmp_build_id.lua"].bytes.clone()).unwrap();
        let fs_hash = hsmp_net::build::content::content_hash_of_dir(&o.repo).unwrap();
        let hex_hash: String = fs_hash.iter().map(|b| format!("{b:02x}")).collect();
        assert!(lua.contains("version = \"0.9.0\"") && lua.contains("protocol = 5") && lua.contains(&hex_hash), "{lua}");
        let json = String::from_utf8(a.items["payload/Win64/hsmp/build.json"].bytes.clone()).unwrap();
        assert!(json.contains(&format!("\"content_hash\":\"{hex_hash}\"")), "{json}");
        assert_eq!(a.items["payload/Win64/hsmp/build.json"].install.as_deref(), Some("HalfswordUE5/Binaries/Win64/hsmp/build.json"));
        assert!(!paths.iter().any(|p| p.contains("BPModLoaderMod") && p.ends_with("hsmp_build_id.lua")), "only HSMP mods carry it");

        // the launcher accepts what we wrote (dir and zip)
        let out = tmp.0.join("dist");
        let (dir, zip, sha) = write_outputs(&a, &out).unwrap();
        assert_eq!(sha, util::sha256_hex(&std::fs::read(&zip).unwrap()));
        let keys = sign::parse_trusted_keys(&format!("{} t", hex::encode(key.verifying_key().as_bytes()))).unwrap();
        for p in [&dir, &zip] {
            let pkg = Package::open(p, &keys).unwrap();
            assert_eq!(pkg.load_files(true).unwrap().len(), m.files.len());
        }
    }

    #[test]
    fn refuses_bad_inputs() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let (_tmp, o) = fixture(&key);
        // untrusted signing key
        let other = ed25519_dalek::SigningKey::from_bytes(&[43; 32]);
        assert!(build(&o, &other).err().unwrap().contains("trusted_keys.txt"));
        // UE4SS not matching the pin
        std::fs::write(o.ue4ss_dir.join("ue4ss/UE4SS.dll"), b"other build").unwrap();
        assert!(build(&o, &key).err().unwrap().contains("pins"));
        std::fs::write(o.ue4ss_dir.join("ue4ss/UE4SS.dll"), b"dll").unwrap();
        // missing binary
        std::fs::remove_file(o.bin_dir.join("hsmp-sidecar.exe")).unwrap();
        assert!(build(&o, &key).err().unwrap().contains("hsmp-sidecar"));
        std::fs::write(o.bin_dir.join("hsmp-sidecar.exe"), b"MZ hsmp-sidecar").unwrap();
        // dirty tree
        std::fs::write(o.repo.join("docs/players/install.md"), b"# edited\n").unwrap();
        assert!(build(&o, &key).err().unwrap().contains("uncommitted"));
        let mut d = o.clone();
        d.allow_dirty = true;
        let b = build(&d, &key).unwrap();
        assert_eq!(b.items["INSTALL.md"].bytes, b"# install\n", "content comes from the commit, not the working tree");
        assert!(!b.warnings.is_empty());
    }

    #[test]
    fn untracked_files_and_prebuilt_binaries_need_opt_in() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let (_tmp, o) = fixture(&key);
        assert_eq!(build(&o, &key).unwrap().manifest.toolchain, "rustc 1.0.0 (test)", "toolchain recorded");
        // an untracked source file would be compiled in without being in the commit
        w(&o.repo.join("server/src/new_module.rs"), b"pub fn x() {}");
        let e = build(&o, &key).err().unwrap();
        assert!(e.contains("untracked") && e.contains("new_module.rs"), "{e}");
        std::fs::remove_dir_all(o.repo.join("server")).unwrap();
        // prebuilt binaries on a non-dev channel
        let mut p = o.clone();
        p.allow_prebuilt = false;
        assert!(build(&p, &key).err().unwrap().contains("--allow-prebuilt"));
        p.prebuilt = false;
        assert!(build(&p, &key).unwrap().warnings.is_empty());
    }

    /// The same sources built in two different folders (different
    /// lengths, both under the user's profile) give byte-identical
    /// executables that contain neither folder nor the user name.
    #[test]
    fn repro_build_is_independent_of_the_build_folder() {
        let cargo = std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
        let base = std::env::temp_dir().join(format!("hsmp_repro_{}_{}", std::process::id(), rand_suffix()));
        let _g = Tmp(base.clone());
        let a = base.join("a").join("hsmp-src");
        let b = base.join("bbbbbbbbbbbb").join("deeper path").join("hsmp-src");
        let main_rs = "fn main() {\n    let l = std::panic::Location::caller();\n    println!(\"{} {}:{}\", file!(), l.file(), l.line());\n    if std::env::args().count() > 5 { panic!(\"boom\"); }\n}\n";
        for d in [&a, &b] {
            w(&d.join("Cargo.toml"), b"[package]\nname = \"repro-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[profile.release]\ncodegen-units = 1\n");
            w(&d.join("src/main.rs"), main_rs.as_bytes());
            cargo_build_repro(&cargo, &d.join("Cargo.toml"), d, &d.join("target"), 1_790_899_200, None, false).unwrap();
        }
        let exe = |d: &Path| std::fs::read(d.join("target").join("release").join(if cfg!(windows) { "repro-probe.exe" } else { "repro-probe" })).unwrap();
        let (ea, eb) = (exe(&a), exe(&b));
        assert!(ea == eb, "builds in two folders differ ({} vs {} bytes)", ea.len(), eb.len());
        let hay = String::from_utf8_lossy(&ea).to_lowercase();
        for needle in [base.display().to_string(), base.display().to_string().replace('\\', "/"), "hsmp_repro_".to_string()] {
            assert!(!hay.contains(&needle.to_lowercase()), "build path {needle} leaked into the binary");
        }
        if let Ok(u) = std::env::var("USERNAME") {
            if u.len() >= 3 {
                assert!(!hay.contains(&format!("\\users\\{}", u.to_lowercase())) && !hay.contains(&format!("/users/{}", u.to_lowercase())), "user name leaked");
            }
        }
    }

    #[test]
    fn real_config_version_is_the_workspace_version() {
        let c = load_config(&std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("release.json")).unwrap()).unwrap();
        assert_eq!(c.version, hsmp_net::build::RELEASE_VERSION, "release.json and the workspace version (Cargo.toml) must agree: the binaries report the workspace one");
    }

    #[test]
    fn real_config_pins_the_toolchain() {
        let c = load_config(&std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("release.json")).unwrap()).unwrap();
        let t = c.rust_toolchain.expect("release.json must pin rust_toolchain (reproducible binaries)");
        assert!(t.split('.').count() == 3 && t.split('.').all(|p| p.parse::<u32>().is_ok()), "an exact version, not a channel: {t}");
        // one toolchain for contributors, CI, Docker and releases: rust-toolchain.toml agrees
        let tc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml")).unwrap();
        let channel = tc
            .lines()
            .find_map(|l| l.trim().strip_prefix("channel").map(|r| r.trim_start().trim_start_matches('=').trim().trim_matches('"').to_string()))
            .expect("rust-toolchain.toml has a channel");
        assert_eq!(channel, t, "rust-toolchain.toml and release.json must pin the same toolchain");
    }

    #[test]
    fn repro_flags_remap_every_local_prefix() {
        let f = repro_rustflags(Path::new(r"D:\src\hsmp"), Path::new(r"D:\src\hsmp\tools\release\target\repro\server"));
        let remaps: Vec<&String> = f.iter().filter(|x| x.starts_with("--remap-path-prefix=")).collect();
        assert!(remaps.iter().any(|x| x.ends_with("=/cargo")) && remaps.iter().any(|x| x.ends_with("=/hsmp")));
        assert!(remaps.last().unwrap().ends_with("=/target"), "most specific prefix last (rustc uses the last match)");
        if cfg!(target_env = "msvc") {
            assert!(f.iter().any(|x| x == "link-arg=/Brepro"));
        }
        let env = repro_env(Path::new("."), Path::new("t"), 42, Some("1.98.1"));
        assert!(env.contains(&("SOURCE_DATE_EPOCH".into(), "42".into())));
        assert!(env.contains(&("RUSTUP_TOOLCHAIN".into(), "1.98.1".into())));
    }

    #[test]
    fn loopback_master_needs_opt_in() {
        assert!(is_loopback("http://127.0.0.1:7778"));
        assert!(is_loopback("http://localhost/x"));
        assert!(!is_loopback("https://master.example.net"));
    }

    #[test]
    fn shipped_master_list_is_https_with_a_local_fallback() {
        let cfg = load_config(include_bytes!("../release.json")).unwrap();
        assert!(cfg.master_urls.first().is_some_and(|u| u.starts_with("https://")), "{:?}", cfg.master_urls);
        assert!(cfg.master_urls.iter().all(|u| is_loopback(u) || u.starts_with("https://")));
    }

    #[test]
    fn plain_http_internet_master_is_refused() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let (_tmp, o) = fixture(&key);
        let cfg_path = o.repo.join(CONFIG_PATH);
        let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&cfg_path).unwrap()).unwrap();
        cfg["master_urls"] = serde_json::json!(["http://master.example.net:7778"]);
        w(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap().as_bytes());
        run(&o.repo, &["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-qam", "http master"]);
        let err = build(&o, &key).err().expect("plain http master must be refused");
        assert!(err.contains("not https"), "{err}");
    }
}

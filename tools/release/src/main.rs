//! hsmp-release: build, sign and verify HSMP release packages.
//!
//!     hsmp-release keygen --out <file outside the repo>
//!     hsmp-release pubkey                         (needs HSMP_RELEASE_SIGNING_KEY[_FILE])
//!     hsmp-release build [--commit REV] [--ue4ss-dir DIR] [--out DIR]
//!                        [--no-build --bin-dir DIR --launcher-exe EXE]
//!                        [--allow-dirty] [--allow-loopback-master]
//!     hsmp-release verify <package dir or zip>    (checks against launcher/trusted_keys.txt)
//!
//! See docs/development/releasing.md.

use hsmp_launcher::package::Package;
use hsmp_launcher::{sign, util};
use hsmp_release::*;
use std::path::{Path, PathBuf};

const USAGE: &str = "hsmp-release keygen --out FILE | pubkey | build [options] | verify <dir|zip>
build options:
  --repo DIR               repository (default: the git top level of the current folder)
  --commit REV             commit to package (default HEAD; must be HEAD unless --no-build)
  --ue4ss-dir DIR          Win64 folder of the pinned UE4SS build (default: $HSMP_UE4SS_DIR,
                           $HSMP_GAME_DIR\\HalfswordUE5\\Binaries\\Win64, <main worktree>\\game\\...)
  --out DIR                output folder (default: target/dist)
  --no-build               do not run cargo; use --bin-dir and --launcher-exe
                           (non-dev channels also need --allow-prebuilt)
  --bin-dir DIR            folder with hsmp-*.exe (default: target/release-repro/server/release)
  --launcher-exe EXE       launcher exe (default: target/release-repro/launcher/release/hsmp-launcher.exe)
  --native-dir DIR         CMake out/ folder of crates/hsmp-native (<Mod>/dlls/*.dll) for the
                           enabled native_mods of release.json (HSMPNative)
  --allow-prebuilt         accept --no-build binaries for a non-dev release (you built them from this commit)
  --allow-dirty            test build from a tree with uncommitted changes
  --allow-loopback-master  allow master_urls that only point at this machine (LAN test build)";

fn cargo() -> PathBuf {
    if let Ok(c) = std::env::var("CARGO") {
        return PathBuf::from(c);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let p = Path::new(&home).join(".cargo").join("bin").join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    if p.exists() {
        p
    } else {
        PathBuf::from("cargo")
    }
}

/// Reproducible build into `<repo>/target/release-repro/<name>`:
/// remapped paths, /Brepro, SOURCE_DATE_EPOCH = commit time, pinned toolchain.
fn cargo_build(repo: &Path, manifest: &Path, name: &str, epoch: u64, toolchain: Option<&str>) -> Result<PathBuf, String> {
    let target = repro_target(repo, name);
    println!("[release] cargo build --release --locked --manifest-path {} (reproducible, target {})", manifest.display(), target.display());
    cargo_build_repro(&cargo(), manifest, repo, &target, epoch, toolchain, true)?;
    Ok(target.join("release"))
}

fn repro_target(repo: &Path, name: &str) -> PathBuf {
    repo.join("target").join("release-repro").join(name)
}

fn default_ue4ss_dir(repo: &Path) -> PathBuf {
    if let Ok(d) = std::env::var("HSMP_UE4SS_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(g) = std::env::var("HSMP_GAME_DIR") {
        return util::join_rel(Path::new(&g), "HalfswordUE5/Binaries/Win64");
    }
    // the main worktree holds game/ (worktrees do not)
    let main = git(repo, &["worktree", "list", "--porcelain"])
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o).lines().find_map(|l| l.strip_prefix("worktree ").map(|s| PathBuf::from(s.trim()))))
        .unwrap_or_else(|| repo.to_path_buf());
    util::join_rel(&main, "game/HalfswordUE5/Binaries/Win64")
}

fn arg_value(it: &mut std::slice::Iter<String>, name: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{name} needs a value"))
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let mut repo: Option<PathBuf> = None;
    let mut commit = "HEAD".to_string();
    let mut ue4ss: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut bin_dir: Option<PathBuf> = None;
    let mut launcher: Option<PathBuf> = None;
    let mut native_dir: Option<PathBuf> = None;
    let (mut no_build, mut allow_dirty, mut allow_loop, mut allow_prebuilt) = (false, false, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => repo = Some(arg_value(&mut it, a)?.into()),
            "--commit" => commit = arg_value(&mut it, a)?,
            "--ue4ss-dir" => ue4ss = Some(arg_value(&mut it, a)?.into()),
            "--out" => out = Some(arg_value(&mut it, a)?.into()),
            "--bin-dir" => bin_dir = Some(arg_value(&mut it, a)?.into()),
            "--launcher-exe" => launcher = Some(arg_value(&mut it, a)?.into()),
            "--native-dir" => native_dir = Some(arg_value(&mut it, a)?.into()),
            "--no-build" => no_build = true,
            "--allow-dirty" => allow_dirty = true,
            "--allow-loopback-master" => allow_loop = true,
            "--allow-prebuilt" => allow_prebuilt = true,
            o => return Err(format!("unknown build option {o}\n{USAGE}")),
        }
    }
    let key = signing_key_from_env()?;
    let repo = match repo {
        Some(r) => r,
        None => PathBuf::from(String::from_utf8_lossy(&git(Path::new("."), &["rev-parse", "--show-toplevel"])?).trim()),
    };
    // one spelling of the repo path for cargo, rustc's path remapping and git
    let repo = plain_abs(&repo);
    let want = String::from_utf8_lossy(&git(&repo, &["rev-parse", &format!("{commit}^{{commit}}")])?).trim().to_string();
    let cfg = load_config(&git_file(&repo, &want, CONFIG_PATH)?)?;
    let pin = cfg.rust_toolchain.as_deref();
    let toolchain = rustc_version(pin)?;
    println!("[release] toolchain {toolchain}{}", if pin.is_some() { " (pinned in release.json)" } else { " (NOT pinned: set rust_toolchain in release.json)" });
    let (mut built_bins, mut built_launcher) = (None, None);
    if !no_build {
        let head = String::from_utf8_lossy(&git(&repo, &["rev-parse", "HEAD"])?).trim().to_string();
        if head != want {
            return Err(format!("--commit {commit} is not HEAD: check it out first, or use --no-build with binaries built from it"));
        }
        let epoch: u64 = String::from_utf8_lossy(&git(&repo, &["show", "-s", "--format=%ct", &want])?).trim().parse().map_err(|_| "cannot read the commit time")?;
        built_bins = Some(cargo_build(&repo, &repo.join("server").join("Cargo.toml"), "server", epoch, pin)?);
        built_launcher = Some(cargo_build(&repo, &repo.join("launcher").join("Cargo.toml"), "launcher", epoch, pin)?.join("hsmp-launcher.exe"));
    }
    let o = BuildOpts {
        bin_dir: bin_dir.or(built_bins).unwrap_or_else(|| repro_target(&repo, "server").join("release")),
        launcher_exe: launcher.or(built_launcher).unwrap_or_else(|| repro_target(&repo, "launcher").join("release").join("hsmp-launcher.exe")),
        native_dir,
        ue4ss_dir: ue4ss.unwrap_or_else(|| default_ue4ss_dir(&repo)),
        repo: repo.clone(),
        commit,
        allow_dirty,
        allow_loopback_master: allow_loop,
        prebuilt: no_build,
        allow_prebuilt,
        toolchain,
    };
    println!("[release] repo      {}", o.repo.display());
    println!("[release] ue4ss     {}", o.ue4ss_dir.display());
    println!("[release] binaries  {}", o.bin_dir.display());
    let b = build(&o, &key)?;
    let out = out.unwrap_or_else(|| repo.join("target").join("dist"));
    let (dir, zip, sha) = write_outputs(&b, &out)?;
    let m = &b.manifest;
    println!("[release] HSMP {} ({}) protocol {} commit {}", m.version, m.channel, m.protocol_version, m.git_commit);
    println!("[release] built with {} (SOURCE_DATE_EPOCH {})", m.toolchain, m.source_date_epoch);
    println!("[release] {} files, signed with key {}", m.files.len(), sign::key_id(&key.verifying_key()));
    println!("[release] folder    {}", dir.display());
    println!("[release] zip       {}", zip.display());
    println!("[release] sha256    {sha}");
    for w in &b.warnings {
        println!("[release] WARNING: {w}");
    }
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let p = args.first().ok_or("verify needs a package folder or zip")?;
    let repo = PathBuf::from(String::from_utf8_lossy(&git(Path::new("."), &["rev-parse", "--show-toplevel"])?).trim());
    let keys_text = std::fs::read_to_string(repo.join("launcher/trusted_keys.txt")).map_err(|e| e.to_string())?;
    let keys = sign::parse_trusted_keys(&keys_text)?;
    let pkg = Package::open(Path::new(p), &keys)?;
    let files = pkg.load_files(true)?;
    println!("OK: HSMP {} signed by {}; {} files verified", pkg.manifest.version, pkg.signed_by, files.len());
    Ok(())
}

fn cmd_keygen(args: &[String]) -> Result<(), String> {
    let out = match args {
        [flag, path] if flag == "--out" => PathBuf::from(path),
        _ => return Err("keygen needs --out <file outside the repository>".into()),
    };
    if out.exists() {
        return Err(format!("{} exists; refusing to overwrite a signing key", out.display()));
    }
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if git(parent, &["rev-parse", "--show-toplevel"]).is_ok() {
            return Err(format!("{} is inside a git repository: keep the private key outside any repo", out.display()));
        }
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("no OS randomness: {e}"))?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    std::fs::write(&out, format!("{}\n", hex::encode(seed))).map_err(|e| e.to_string())?;
    println!("private key written to {} (keep it secret and backed up; never commit it)", out.display());
    println!("use it with:   set {KEY_FILE_ENV}={}", out.display());
    println!("add this line to launcher/trusted_keys.txt and commit:");
    println!("{} hsmp-release {}", hex::encode(key.verifying_key().as_bytes()), util::iso_utc(util::now_unix()));
    Ok(())
}

fn cmd_pubkey() -> Result<(), String> {
    let key = signing_key_from_env()?;
    println!("{} hsmp-release key {}", hex::encode(key.verifying_key().as_bytes()), sign::key_id(&key.verifying_key()));
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let r = match argv.first().map(String::as_str) {
        Some("build") => cmd_build(&argv[1..]),
        Some("verify") => cmd_verify(&argv[1..]),
        Some("keygen") => cmd_keygen(&argv[1..]),
        Some("pubkey") => cmd_pubkey(),
        Some("help") | Some("--help") | Some("-h") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}

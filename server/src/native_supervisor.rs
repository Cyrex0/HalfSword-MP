//! Launch/admin supervision only. The game DLL owns the authoritative world and socket service.
use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Backend {
    /// No rendering device. Native gameplay/material/cut support must be verified for the game build.
    Null,
    /// Render without an on-screen viewport, preserving native render-target gameplay paths.
    Offscreen,
}

#[derive(Parser)]
#[command(
    name = "hsmp-server native",
    about = "Run Half Sword as the native multiplayer authority"
)]
struct Args {
    /// Licensed game install root, containing HalfswordUE5/Binaries/Win64.
    #[arg(long, env = "HSMP_GAME_DIR")]
    game_dir: PathBuf,
    #[arg(long, default_value = "0.0.0.0:7777")]
    bind: SocketAddr,
    /// Persistent server identity directory; never printed or passed as key material.
    #[arg(long)]
    identity_dir: PathBuf,
    /// Parent directory for a fresh run's diagnostics and shutdown signal.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long, default_value = "Map_Arena_Yard")]
    arena: String,
    #[arg(long, value_enum, default_value = "null")]
    backend: Backend,
    /// Stop cleanly if the recorded parent disappears.
    #[arg(long)]
    parent_pid: Option<u32>,
    #[arg(long)]
    pid_file: Option<PathBuf>,
    /// Bounded native diagnostic run. Omit for continuous hosting.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=86400))]
    run_seconds: Option<u64>,
    /// Diagnose engine bootstrap before starting the embedded network service.
    #[arg(long)]
    boot_only: bool,
}

fn safe_manifest_path(base: &Path, relative: &str) -> Result<PathBuf> {
    // Deployment paths use forward slashes. Explicitly reject Windows forms even on Unix tests.
    if relative.is_empty() || relative.contains(['\\', ':']) {
        bail!("invalid deployment path");
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("deployment path escapes the game directory");
    }
    Ok(base.join(path))
}

fn validate_deployment(win64: &Path) -> Result<()> {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(win64.join("hsmp_deploy.json"))
            .context("native deployment manifest missing")?,
    )
    .context("invalid native deployment manifest")?;
    if manifest["g0_ok"].as_bool() != Some(true)
        || manifest["dirty"].as_bool() != Some(false)
        || manifest["bins_match_commit"].as_bool() != Some(true)
        || manifest.pointer("/g0/quick").and_then(|v| v.as_bool()) != Some(false)
        || manifest.pointer("/g0/tree_dirty").and_then(|v| v.as_bool()) != Some(false)
        || manifest["commit"].as_str().is_none()
        || manifest["commit"] != manifest["bins_commit"]
        || manifest["commit"] != manifest["g0"]["commit"]
    {
        bail!("native hosting requires a clean deployment with full G0 for its exact build");
    }
    if manifest
        .pointer("/native/deployed")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        bail!("HSMPNative is not deployed; rebuild and deploy the native mod first");
    }
    let hashes = manifest["hashes"]
        .as_object()
        .context("deployment file hashes missing")?;
    let own_hash = hex::encode(Sha256::digest(std::fs::read(std::env::current_exe()?)?));
    if hashes
        .get("hsmp/hsmp-server.exe")
        .and_then(|v| v.as_str())
        .is_none_or(|hash| !hash.eq_ignore_ascii_case(&own_hash))
    {
        bail!(
            "supervisor build differs from the deployed server; run the deployed hsmp-server.exe"
        );
    }
    for required in [
        "ue4ss/Mods/HSMPNative/dlls/main.dll",
        "ue4ss/Mods/HSMPNative/dlls/hsmp_lua.dll",
        "ue4ss/Mods/HSMPMatch/Scripts/headless_worker.lua",
        "ue4ss/Mods/HSMPMatch/Scripts/hsmp_runtime_role.lua",
    ] {
        if !hashes.contains_key(required) {
            bail!("native server deployment incomplete: {required}");
        }
    }
    for (relative, expected) in hashes {
        let path = safe_manifest_path(win64, relative)?;
        let bytes =
            std::fs::read(&path).with_context(|| format!("deployed file missing: {relative}"))?;
        let actual = hex::encode(Sha256::digest(bytes));
        if expected
            .as_str()
            .is_none_or(|v| !actual.eq_ignore_ascii_case(v))
        {
            bail!("deployed file changed: {relative}; redeploy before hosting");
        }
    }
    let mods = std::fs::read_to_string(win64.join("ue4ss/Mods/mods.txt"))?;
    for name in ["HSMPNative", "HSMPMatch"] {
        if !mods.lines().any(|line| {
            line.split_once(':').is_some_and(|(key, value)| {
                key.trim() == name && value.split(';').next().unwrap_or("").trim() == "1"
            })
        }) {
            bail!("required mod disabled: {name}");
        }
    }
    Ok(())
}

fn game_command(exe: &Path, win64: &Path, state: &Path, identity: &Path, args: &Args) -> Command {
    let mut command = Command::new(exe);
    command
        .current_dir(win64)
        .args([
            "-nosound",
            "-unattended",
            "-windowed",
            "-ForceRes",
            "-ResX=880",
            "-ResY=527",
            "-WinX=-1760",
            "-WinY=0",
        ])
        .arg(match args.backend {
            Backend::Null => "-nullrhi",
            Backend::Offscreen => "-RenderOffscreen",
        })
        .env("HSMP_RUNTIME_ROLE", "native_worker")
        .env("HSMP_STATE_DIR", state)
        .env("HSMP_INST", format!("native_{}", std::process::id()))
        .env("HSMP_NATIVE_IDENTITY_DIR", identity)
        .env("HSMP_NATIVE_PARENT_PID", std::process::id().to_string())
        .env("HSMP_NATIVE_BIND", args.bind.to_string())
        .env("HSMP_NATIVE_ARENA", &args.arena)
        // Lifecycle control belongs to the supervisor run directory, outside the game's state dir.
        .env(
            "HSMP_NATIVE_STOP_FILE",
            state.parent().unwrap_or(state).join("stop.request"),
        )
        .env(
            "HSMP_NATIVE_BOOT_ONLY",
            if args.boot_only { "1" } else { "0" },
        )
        .env("HSMP_NATIVE_CALLER_PROBE", "0")
        .env("HSMP_NATIVE_PROBE", "0")
        .env("HSMP_DEV_CALLER_PROBE", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no helper console.
    }
    command
}

/// The OS child handle refers only to the process this supervisor started, never a reused PID.
struct OwnedGame(Child);
impl Drop for OwnedGame {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn run() -> Result<()> {
    let argv = std::iter::once(std::ffi::OsString::from("hsmp-server native"))
        .chain(std::env::args_os().skip(2));
    let args = Args::parse_from(argv);
    if args.arena.is_empty()
        || args.arena.len() > 40
        || !args
            .arena
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        bail!("arena must be a cooked map name");
    }
    if args.parent_pid == Some(0) {
        bail!("parent PID must be nonzero");
    }
    let game = args
        .game_dir
        .canonicalize()
        .context("game directory unavailable")?;
    let win64 = game.join("HalfswordUE5/Binaries/Win64");
    let exe = win64.join("HalfswordUE5-Win64-Shipping.exe");
    if !exe.is_file() {
        bail!("licensed Half Sword executable missing");
    }
    validate_deployment(&win64)?;
    std::fs::create_dir_all(&args.identity_dir)?;
    let identity = args.identity_dir.canonicalize()?;
    let parent = args
        .state_dir
        .clone()
        .unwrap_or_else(|| win64.join("hsmp_native_state"));
    std::fs::create_dir_all(&parent)?;
    let run = parent
        .canonicalize()?
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&run)?;
    let state = run.join("worker");
    std::fs::create_dir(&state)?;
    let _pid = args
        .pid_file
        .as_ref()
        .map(|path| crate::proc_util::write_pid_file(path, "native_supervisor", args.parent_pid))
        .transpose()?;
    let mut child = OwnedGame(
        game_command(&exe, &win64, &state, &identity, &args)
            .spawn()
            .context("native game launch failed")?,
    );
    let record = serde_json::json!({"role":"native_authority", "pid":child.0.id(), "exe":exe, "supervisor_pid":std::process::id(), "backend":format!("{:?}",args.backend)});
    std::fs::write(
        run.join("native_process.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    eprintln!(
        "Native authority started (PID {}). Diagnostics: {}",
        child.0.id(),
        run.display()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut poll = tokio::time::interval(Duration::from_millis(100));
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
        let parent_exit = crate::proc_util::wait_parent_exit(args.parent_pid);
        tokio::pin!(parent_exit);
        let started = Instant::now();
        loop {
            tokio::select! {
                result = &mut interrupt => { result.context("shutdown signal unavailable")?; break; },
                () = &mut parent_exit => break,
                _ = poll.tick() => {
                    if let Some(status) = child.0.try_wait()? {
                        if !status.success() { bail!("native authority exited with {status}"); }
                        return Ok(());
                    }
                    if args.run_seconds.is_some_and(|seconds| started.elapsed() >= Duration::from_secs(seconds)) { break; }
                    if run.join("stop.request").exists() { break; }
                }
            }
        }
        std::fs::write(run.join("stop.request"), b"stop\n")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait()? {
                if !status.success() { bail!("native authority shutdown returned {status}"); }
                eprintln!("Native authority stopped cleanly.");
                return Ok(());
            }
            if Instant::now() >= deadline {
                child.0.kill()?;
                child.0.wait()?;
                bail!("native authority missed the graceful shutdown deadline; its owned process was stopped");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_paths_cannot_escape_or_use_windows_prefixes() {
        let root = Path::new("game");
        for bad in [
            "",
            "../secret",
            "/secret",
            "C:/secret",
            "ue4ss\\..\\secret",
            "//host/share",
            "./file",
        ] {
            assert!(safe_manifest_path(root, bad).is_err(), "accepted {bad}");
        }
        assert_eq!(
            safe_manifest_path(root, "ue4ss/Mods/HSMPNative/dlls/main.dll").unwrap(),
            root.join("ue4ss/Mods/HSMPNative/dlls/main.dll")
        );
    }
    #[test]
    fn launch_selects_native_role_and_never_an_external_broker() {
        let args = Args::try_parse_from([
            "native",
            "--game-dir",
            "game",
            "--identity-dir",
            "identity",
            "--bind",
            "127.0.0.1:7778",
            "--backend",
            "offscreen",
        ])
        .unwrap();
        let command = game_command(
            Path::new("game.exe"),
            Path::new("win64"),
            Path::new("state"),
            Path::new("identity"),
            &args,
        );
        let options: Vec<_> = command
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        assert!(options.iter().any(|v| v == "-RenderOffscreen"));
        assert!(!options.iter().any(|v| v == "-nullrhi"));
        let env: std::collections::BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(env["HSMP_RUNTIME_ROLE"], "native_worker");
        assert_eq!(env["HSMP_NATIVE_IDENTITY_DIR"], "identity");
        assert_eq!(env["HSMP_NATIVE_BIND"], "127.0.0.1:7778");
        assert!(env["HSMP_NATIVE_STOP_FILE"].ends_with("stop.request"));
    }
}

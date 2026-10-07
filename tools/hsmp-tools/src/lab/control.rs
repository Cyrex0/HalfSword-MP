use super::{
    collect::{self, Collector, Summary},
    Recipe,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "../bin/hsmp-gate/rcon.rs"]
mod rcon;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn save(path: &Path, v: &impl serde::Serialize) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, serde_json::to_vec_pretty(v)?)
        .with_context(|| format!("write {}", path.display()))
}
pub fn journal(path: &Path, v: &Value) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(v)?)?;
    Ok(())
}
pub fn quiet(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let _ = command;
}

/// Blocking owner command; other invocations drive `exp`/`ab`/`stop`. The
/// harness uses this process's start identity to guarantee teardown on exit.
pub fn session(repo: &Path, dir: &Path, seconds: u64, fake: bool, server_args: &str) -> Result<()> {
    ensure!(
        !dir.join("ready.json").exists(),
        "session directory already exists; choose a fresh directory"
    );
    fs::create_dir_all(dir)?;
    let dir = dir.canonicalize()?;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(repo.join("scripts/mp_test.ps1"))
        .args([
            "-Instances",
            "2",
            "-Scenario",
            "combat_manual",
            "-CombatArena",
            "Yard",
            "-LabSession",
        ])
        .arg(&dir)
        .arg("-LabOwnerPid")
        .arg(std::process::id().to_string())
        .env("HSMP_IPC_TAP_POSES", "1")
        .stdout(Stdio::from(fs::File::create(dir.join("harness.log"))?))
        .stderr(Stdio::from(fs::File::create(
            dir.join("harness-errors.log"),
        )?));
    if fake {
        cmd.arg("-FakeGame");
    }
    let extra: Vec<String> =
        serde_json::from_str(server_args).context("server args must be JSON string array")?;
    ensure!(
        extra.iter().all(|s| !s.contains(['\r', '\n'])),
        "invalid server argument"
    );
    cmd.arg("-LabServerArgsJson").arg(server_args);
    quiet(&mut cmd);
    let mut child = cmd.spawn().context("start lab harness")?;
    let start = Instant::now();
    let mut announced = false;
    loop {
        if let Some(code) = child.try_wait()? {
            ensure!(
                code.success(),
                "lab harness failed ({code}); see {}",
                dir.join("harness-errors.log").display()
            );
            return Ok(());
        }
        if dir.join("ready.json").exists() && !announced {
            println!("Session ready: {}", dir.display());
            announced = true;
        }
        if start.elapsed().as_secs() >= seconds {
            fs::write(dir.join("stop"), b"session duration reached\n")?;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

pub struct Live {
    pub run: PathBuf,
    config: Value,
    games: Vec<Value>,
    addr: String,
    password: String,
    tools: PathBuf,
}
impl Live {
    pub fn attach(session: &Path) -> Result<Self> {
        let ready: Value = serde_json::from_slice(
            &fs::read(session.join("ready.json")).context("session is not ready")?,
        )?;
        verify_process(&ready["owner"])?;
        ensure!(!session.join("stop").exists(), "session is stopping");
        let run = PathBuf::from(ready["run"].as_str().context("missing run")?);
        let config: Value = serde_json::from_slice(&fs::read(run.join("run.json"))?)?;
        let processes: Vec<Value> =
            serde_json::from_slice(&fs::read(run.join("mp_test.pids.json"))?)?;
        let mut games: Vec<_> = processes
            .into_iter()
            .filter(|v| {
                v["role"]
                    .as_str()
                    .is_some_and(|s| s == "game1" || s == "game2")
            })
            .collect();
        games.sort_by_key(|g| g["role"].as_str().unwrap().to_string());
        ensure!(games.len() == 2, "expected two recorded games");
        for g in &games {
            verify_process(g)?;
        }
        let plan = fs::read_to_string(run.join("plan.txt"))?;
        let password = plan
            .split("--rcon-password ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .context("no RCON credential in plan")?
            .to_string();
        let addr = format!(
            "127.0.0.1:{}",
            config["ports"]["rcon"].as_u64().context("missing port")?
        );
        let tools = std::env::current_exe()?.with_file_name("hsmp-tools.exe");
        Ok(Self {
            run,
            config,
            games,
            addr,
            password,
            tools,
        })
    }
    pub fn rcon(&self, cmd: &str) -> Result<String> {
        ensure!(!cmd.contains(['\r', '\n']), "invalid RCON command");
        let (ok, reply) = rcon::send(&self.addr, &self.password, cmd, Duration::from_secs(5))
            .context("lab RCON transport")?;
        ensure!(ok, "RCON {cmd}: {reply}");
        Ok(reply)
    }
    pub fn status(&self) -> Result<Value> {
        let r = self.rcon("STATUS")?;
        Ok(serde_json::from_str(
            r.strip_prefix("OK ").context("STATUS reply is not JSON")?,
        )?)
    }
    pub fn wait(&self, phase: &str, seconds: u64) -> Result<Value> {
        let start = Instant::now();
        loop {
            let v = self.status()?;
            if v["phase"] == phase {
                return Ok(v);
            }
            ensure!(
                start.elapsed().as_secs() < seconds,
                "timed out waiting for {phase}; last phase {}",
                v["phase"]
            );
            for g in &self.games {
                verify_process(g)?;
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
    pub fn dev(&self, inst: usize, args: &[&str]) -> Result<()> {
        verify_process(&self.games[inst])?;
        let mut cmd = Command::new(&self.tools);
        cmd.arg("ipc-ctl")
            .arg("--pid")
            .arg(self.games[inst]["pid"].to_string())
            .args(args);
        quiet(&mut cmd);
        let r = cmd.output()?;
        ensure!(
            r.status.success(),
            "DevCtl instance {}: {}",
            inst + 1,
            String::from_utf8_lossy(&r.stderr)
        );
        Ok(())
    }
    fn kit(&self, inst: usize, kit: &str) -> Result<()> {
        use hsmp_ipc::record::{view, Record};
        use hsmp_ipc::schema::{
            loadout::{Kit, K_KIT, K_KIT_VERDICT, VERDICT_ACCEPTED},
            pose::PeerDir,
            session::{sidecar_status, Link, K_LINK},
            SlotMeta,
        };
        use std::sync::atomic::Ordering;
        fn read<R: Record>(
            seg: &hsmp_ipc::Segment,
            name: &str,
            slot: usize,
            kind: u16,
        ) -> Option<(SlotMeta, R, Vec<R::Row>)> {
            let (mut scratch, mut bytes) = (Vec::new(), Vec::new());
            let (meta, got, _) = seg.slot_ref(name, slot)?.peek(&mut scratch, &mut bytes)?;
            if meta.valid != 1 || got != kind {
                return None;
            }
            let v = view::<R>(&bytes).ok()?;
            Some((meta, *v.head, v.rows.to_vec()))
        }
        verify_process(&self.games[inst])?;
        let pid = self.games[inst]["pid"].as_u64().context("game PID")? as u32;
        let mapping = crate::ipcgame::open_pid(pid, hsmp_ipc::shm::Access::ReadOnly)?;
        let seg = crate::ipcgame::segment(&mapping)?;
        let session_epoch = seg.header.session_epoch.load(Ordering::Acquire);
        let game_epoch = seg.header.game.epoch.load(Ordering::Acquire);
        let sidecar_epoch = seg.header.sidecar.epoch.load(Ordering::Acquire);
        let previous_seq = read::<Kit>(seg, "kit", 0, K_KIT)
            .filter(|(meta, _, _)| meta.writer_epoch == game_epoch)
            .map_or(0, |(_, k, _)| k.seq);
        // Queue success is not command success: wait for this specific kit's
        // game-thread result, then the server's receipt of that exact slot seq
        // and contents. Repeated identical kits also require a new SAVE ack.
        let id = (hsmp_ipc::shm::random_u64() as u32).max(1).to_string();
        let log = Path::new(self.config["game"].as_str().context("game path")?)
            .join("HalfswordUE5/Binaries/Win64/ue4ss/UE4SS.log");
        let offset = fs::metadata(&log)?.len();
        self.dev(inst, &["--id", &id, "autotest", "kit", kit])?;
        let needle = format!("AUTOTEST cmd #{id}: kit ");
        let start = Instant::now();
        let mut saved = false;
        loop {
            verify_process(&self.games[inst])?;
            ensure!(
                seg.header.session_epoch.load(Ordering::Acquire) == session_epoch
                    && seg.header.game.epoch.load(Ordering::Acquire) == game_epoch
                    && seg.header.sidecar.epoch.load(Ordering::Acquire) == sidecar_epoch,
                "kit session or IPC writer changed before acknowledgement"
            );
            use std::io::{Read, Seek, SeekFrom};
            if !saved {
                let mut f = fs::File::open(&log)?;
                f.seek(SeekFrom::Start(offset))?;
                let mut tail = String::new();
                f.read_to_string(&mut tail)?;
                if let Some(line) = tail.lines().find(|s| s.contains(&needle)) {
                    ensure!(line.ends_with("-> saved"), "kit failed: {line}");
                    saved = true;
                }
            }
            if saved {
                if let (Some((request_meta, request, rows)), Some((link_meta, link, _))) = (
                    read::<Kit>(seg, "kit", 0, K_KIT),
                    read::<Link>(seg, "link", 0, K_LINK),
                ) {
                    if request_meta.writer_epoch == game_epoch
                        && request.seq > previous_seq
                        && link_meta.writer_epoch == sidecar_epoch
                        && link_meta.session_epoch == session_epoch
                        && link.status == sidecar_status::CONNECTED
                        && link.my_peer_id != 0
                    {
                        let mut dir = Box::<PeerDir>::default();
                        let _ = seg.peers.dir.read_into(&mut dir);
                        if let Some(slot) = dir.slot_of(link.my_peer_id) {
                            if let Some((receipt_meta, receipt, receipt_rows)) =
                                read::<Kit>(seg, "peer_kit", slot, K_KIT_VERDICT)
                            {
                                if receipt_meta.writer_epoch == sidecar_epoch
                                    && receipt_meta.session_epoch == session_epoch
                                    && receipt.seq >= request.seq
                                    && receipt.rev > 0
                                {
                                    ensure!(
                                        receipt.verdict == VERDICT_ACCEPTED,
                                        "kit replaced by server: {}",
                                        receipt.reason.lossy()
                                    );
                                    ensure!(kit_contents_match(&request, &rows, &receipt, &receipt_rows), "server kit receipt does not match requested class, hands, cosmetics or armour");
                                    journal(
                                        &self.run.join("lab-actions.jsonl"),
                                        &json!({"at":now_ms(),"action":"kit_ack","instance":inst+1,"peer":link.my_peer_id,"command_id":id,"selection_seq":request.seq,"ack_seq":receipt.seq,"revision":receipt.rev,"kit":kit}),
                                    )?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
            ensure!(
                start.elapsed().as_secs() < 20,
                "kit command lacks a matching server acknowledgement (saved={saved})"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn experiment(&self, recipe: &Recipe, out: &Path) -> Result<Summary> {
        recipe.validate()?;
        // Refuse overlapping experiments, which would mix source boundaries.
        let lock = self.run.join("lab-exp.lock");
        let f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock)
            .context("another experiment is active (or stale lab-exp.lock; inspect owner first)")?;
        struct Guard(PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _guard = Guard(lock);
        drop(f);
        let result = (|| {
            self.rcon("ABORT")?;
            self.wait("Lobby", 30)?;
            self.rcon(&format!("MODE {}", recipe.mode))?;
            self.rcon(&format!("MAP {}", recipe.arena))?;
            self.rcon(&format!("KIT {}", recipe.kit_rules))?;
            self.rcon("BESTOF 31")?;
            for cmd in &recipe.commands {
                self.rcon(cmd)?;
            }
            for i in 0..2 {
                self.dev(i, &["autotest", "parity", "ai off"])?;
                self.kit(i, &recipe.kits[i])?;
                self.dev(
                    i,
                    &[
                        "autotest",
                        "combat_probe",
                        if recipe.probe { "on" } else { "off" },
                    ],
                )?;
                for (key, v) in &recipe.tune {
                    self.dev(i, &["tune", key, &v.to_string()])?;
                }
                if recipe.ai {
                    self.dev(i, &["autotest", "parity", "ai auto"])?;
                }
                self.dev(i, &["autotest", "ready"])?;
            }
            let ready = Instant::now();
            loop {
                let s = self.status()?;
                if s["roster"]
                    .as_array()
                    .is_some_and(|rs| rs.len() == 2 && rs.iter().all(|r| r["ready"] == true))
                {
                    break;
                }
                ensure!(
                    ready.elapsed().as_secs() < 30,
                    "both fighters did not become ready"
                );
                thread::sleep(Duration::from_millis(100));
            }
            let commit = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
            let commit = String::from_utf8_lossy(&commit.stdout).trim().to_string();
            let mut c = Collector::new(&self.run, &recipe.name, &commit);
            let paths = collect::sources(&self.run)?;
            c.prime(&paths)?;
            journal(
                &self.run.join("lab-actions.jsonl"),
                &json!({"at":now_ms(),"action":"recipe_start","recipe":recipe}),
            )?;
            self.rcon("START")?;
            let initial = self.wait("Live", 180)?;
            if let Some(drive) = &recipe.drive {
                for i in 0..2 {
                    self.dev(i, &["autotest", "parity", &format!("drive {drive}")])?;
                }
            }
            let start = Instant::now();
            let mut round_start = start;
            let mut last_claim = start;
            let mut claims = 0;
            let mut round = initial["round"].clone();
            while start.elapsed().as_secs() < recipe.duration_s {
                c.poll(&collect::sources(&self.run)?)?;
                let n = c.summary.values("accept").len();
                if n != claims {
                    claims = n;
                    last_claim = Instant::now();
                }
                let status = self.status()?;
                if status["round"] != round {
                    round = status["round"].clone();
                    round_start = Instant::now();
                    last_claim = round_start;
                }
                if status["phase"] == "Live"
                    && (round_start.elapsed().as_secs() >= recipe.round_timeout_s
                        || (recipe.no_claim_timeout_s > 0
                            && last_claim.elapsed().as_secs() >= recipe.no_claim_timeout_s))
                {
                    journal(
                        &self.run.join("lab-actions.jsonl"),
                        &json!({"at":now_ms(),"action":"harness_round_timeout","round":round,"claims":claims,"policy":"DEBUG KILL 1"}),
                    )?;
                    self.rcon("DEBUG KILL 1")?;
                    round_start = Instant::now();
                    last_claim = round_start;
                }
                if status["phase"] == "Lobby" {
                    self.rcon("START")?;
                }
                for g in &self.games {
                    verify_process(g)?;
                }
                thread::sleep(Duration::from_millis(250));
            }
            self.rcon("ABORT")?;
            self.wait("Lobby", 30)?;
            c.poll(&collect::sources(&self.run)?)?;
            c.finish();
            save(out, &c.summary)?;
            journal(
                &self.run.join("lab-actions.jsonl"),
                &json!({"at":now_ms(),"action":"recipe_end","recipe":recipe.name,"out":out,"duration_s":start.elapsed().as_secs()}),
            )?;
            Ok(c.summary)
        })();
        if let Err(error) = &result {
            journal(
                &self.run.join("lab-actions.jsonl"),
                &json!({"at":now_ms(),"action":"recipe_failed","recipe":recipe.name,"error":format!("{error:#}"),"out":out}),
            )?;
            let _ = self.rcon("ABORT");
        }
        result
    }
}

fn kit_contents_match(
    request: &hsmp_ipc::schema::loadout::Kit,
    rows: &[hsmp_ipc::schema::loadout::KitItem],
    receipt: &hsmp_ipc::schema::loadout::Kit,
    receipt_rows: &[hsmp_ipc::schema::loadout::KitItem],
) -> bool {
    let mut wanted: Vec<_> = rows.iter().map(|r| r.id.lossy()).collect();
    let mut got: Vec<_> = receipt_rows.iter().map(|r| r.id.lossy()).collect();
    wanted.sort_unstable();
    got.sort_unstable();
    request.class == receipt.class
        && request.r == receipt.r
        && request.l == receipt.l
        && request.cos == receipt.cos
        && wanted == got
}

#[cfg(test)]
mod kit_receipt_tests {
    use super::kit_contents_match;
    use hsmp_ipc::{
        layout::Str,
        schema::loadout::{Kit, KitItem},
    };
    #[test]
    fn kit_receipt_matches_contents_even_if_armour_order_differs() {
        let request = Kit {
            class: Str::new("duelist"),
            r: Str::new("w_arming3"),
            cos: [1, 2, 3, 0],
            ..Default::default()
        };
        let receipt = Kit {
            seq: 8,
            rev: 9,
            ..request
        };
        let armour = [
            KitItem {
                id: Str::new("b_tunic"),
            },
            KitItem {
                id: Str::new("f_shoes1"),
            },
        ];
        assert!(kit_contents_match(
            &request,
            &armour,
            &receipt,
            &[armour[1], armour[0]]
        ));
        assert!(!kit_contents_match(
            &request,
            &armour,
            &Kit {
                r: Str::new("w_axe1"),
                ..receipt
            },
            &armour
        ));
        assert!(!kit_contents_match(
            &request,
            &armour,
            &Kit {
                cos: [0; 4],
                ..receipt
            },
            &armour
        ));
        assert!(!kit_contents_match(
            &request,
            &armour,
            &receipt,
            &armour[..1]
        ));
    }
}

fn verify_process(record: &Value) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, FILETIME},
            System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        let pid = record["pid"].as_u64().context("missing recorded PID")? as u32;
        let ticks = record["start_ticks"]
            .as_u64()
            .context("missing recorded start time")?;
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        ensure!(h != 0, "recorded process {pid} is absent or inaccessible");
        let mut times = [FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        }; 4];
        let ok = unsafe {
            GetProcessTimes(
                h,
                &mut times[0],
                &mut times[1],
                &mut times[2],
                &mut times[3],
            )
        };
        unsafe { CloseHandle(h) };
        let created = ((times[0].dwHighDateTime as u64) << 32) | times[0].dwLowDateTime as u64;
        ensure!(
            ok != 0 && created.checked_add(504911232000000000) == Some(ticks),
            "recorded process identity changed for PID {pid}"
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = record;
        anyhow::bail!("live lab requires Windows; analyse/compare/review are portable")
    }
}

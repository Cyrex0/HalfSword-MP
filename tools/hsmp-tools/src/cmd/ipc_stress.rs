//! `hsmp-tools ipc-stress`: cross-process stress of the shared-memory IPC
//! (docs/development/ipc-shared-memory.md).
//!
//! The parent creates fresh named segments (role = game) and spawns children of this same
//! exe (`ipc-stress --role writer|reader ...`). Scenarios:
//!
//! | # | scenario | pass |
//! |---|---|---|
//! | 1 | max-rate writer vs reader: seqlock (local_pose), 128 KiB blob, G2S ring | 0 torn, ring order continuous, payloads intact |
//! | 2 | kill the writer by PID at a random point, then a writer that dies inside a seqlock write, each restarted | death seen <= 1 s, Busy while the slot is abandoned, recovery <= 100 ms after re-attach, never old-epoch data, 0 torn |
//! | 3 | kill and restart the reader | the writer never blocks (max loop gap < 50 ms), restarted reader consistent |
//! | 4 | stalled consumer | producer gets Full, no corruption, ring continuous after the stall |
//! | 5 | two segments in parallel | full isolation (tags, epochs) |
//! | 6 | wrong parent / layout hash / magic | refuse codes and exit codes 72 / 71 / 70 |
//!
//! Exit 0 only if every scenario passes. `--duration <s>` scales the run (default 25 s).
//! `--fast` runs the writer several times faster (the result must not depend on its pace);
//! `--race-ms <ms>` makes each reader sleep inside the writer-attach window on its first loop
//! (regression knob for the epoch-0 race; it skews scenario 2's attach->recover timing).
//! The parent starts a writer only after its reader printed RSTART (ring reset), never
//! after a fixed sleep.
//! Children are killed by their own process handle (TerminateProcess), never by name.

#[derive(clap::Args)]
pub struct Args {
    /// Total run time in seconds (scenarios scale with it).
    #[arg(long, default_value_t = 25.0)]
    pub duration: f64,
    /// Run only these scenarios (1-6), comma separated.
    #[arg(long)]
    pub only: Option<String>,
    /// Internal: child role (writer | reader).
    #[arg(long, hide = true)]
    pub role: Option<String>,
    #[arg(long, hide = true)]
    pub name: Option<String>,
    #[arg(long, hide = true, default_value_t = 0)]
    pub parent_pid: u32,
    #[arg(long, hide = true, default_value_t = 1.0)]
    pub secs: f64,
    /// Writer: exit inside a seqlock write after this many ms.
    #[arg(long, hide = true)]
    pub die_mid_write_ms: Option<u64>,
    /// Reader: sleep this long before consuming.
    #[arg(long, hide = true, default_value_t = 0)]
    pub stall_ms: u64,
    /// Writer pace: write the pose slot only every 8th iteration, so the ring is pushed
    /// several times faster (the test must not depend on the writer's pace).
    #[arg(long)]
    pub fast: bool,
    /// Reader: on its first loop, sleep this long between loading the producer epoch and
    /// consuming the ring (widens the window in which a writer attaches; a regression knob
    /// for the epoch-0 race).
    #[arg(long, default_value_t = 0)]
    pub race_ms: u64,
}

#[cfg(not(windows))]
pub fn run(_a: Args) -> anyhow::Result<i32> {
    anyhow::bail!("ipc-stress needs Windows named shared memory")
}

#[cfg(windows)]
pub fn run(a: Args) -> anyhow::Result<i32> {
    match a.role.as_deref() {
        Some("writer") => win::writer(&a),
        Some("reader") => win::reader(&a),
        Some(r) => anyhow::bail!("unknown role {}", r),
        None => win::orchestrate(&a),
    }
}

#[cfg(windows)]
mod win {
    use super::Args;
    use hsmp_ipc::handshake::{attach_sidecar, debug_corrupt_prefix, init_game, SideParams};
    use hsmp_ipc::layout::{as_words, as_words_mut, Pod};
    use hsmp_ipc::ring::{Pop, PushError, Record, MAX_PAYLOAD};
    use hsmp_ipc::segment::WorldHashBuf;
    /// The pose slot body (ABI 2: a stamped codec v2 frame buffer).
    type LocalPose = hsmp_ipc::schema::Stamped<hsmp_ipc::schema::pose::PoseBuf>;
    use hsmp_ipc::seqlock::ReadError;
    use hsmp_ipc::shm::{self, Access, ProcessHandle};
    use hsmp_ipc::{Mapping, RefuseCode};
    use rand::Rng;
    use std::collections::HashMap;
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    /// The largest game blob (the world consistency report, ~64 KiB).
    type Blob = WorldHashBuf;

    fn tag_of(name: &str) -> u64 {
        name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    }
    fn mix(k: u64, i: usize) -> u64 {
        (k ^ 0x9E37_79B9_7F4A_7C15).wrapping_mul(i as u64 * 2 + 1).rotate_left((i % 61) as u32)
    }
    fn fill(w: &mut [u64], counter: u64, epoch: u64, tag: u64) {
        w[0] = counter;
        w[1] = epoch;
        w[2] = tag;
        let k = counter ^ epoch ^ tag;
        for (i, x) in w.iter_mut().enumerate().skip(3) {
            *x = mix(k, i);
        }
    }
    /// (consistent, counter, epoch, tag)
    fn check(w: &[u64]) -> (bool, u64, u64, u64) {
        let k = w[0] ^ w[1] ^ w[2];
        (w.iter().enumerate().skip(3).all(|(i, x)| *x == mix(k, i)), w[0], w[1], w[2])
    }
    fn ring_payload(buf: &mut [u8], counter: u64, epoch: u64) -> usize {
        let len = 16 + (counter as usize * 37) % (MAX_PAYLOAD - 16);
        buf[..8].copy_from_slice(&counter.to_le_bytes());
        buf[8..16].copy_from_slice(&epoch.to_le_bytes());
        for (i, b) in buf[16..len].iter_mut().enumerate() {
            *b = (counter as usize).wrapping_add(epoch as usize).wrapping_add(i) as u8;
        }
        len
    }
    fn ring_ok(p: &[u8], epoch: u64) -> bool {
        if p.len() < 16 {
            return false;
        }
        let counter = u64::from_le_bytes(p[..8].try_into().unwrap());
        let e = u64::from_le_bytes(p[8..16].try_into().unwrap());
        let len = 16 + (counter as usize * 37) % (MAX_PAYLOAD - 16);
        e == epoch
            && p.len() == len
            && p[16..].iter().enumerate().all(|(i, b)| *b == (counter as usize).wrapping_add(e as usize).wrapping_add(i) as u8)
    }

    fn ms_between(a: u64, b: u64, freq: u64) -> f64 {
        (b as f64 - a as f64) * 1000.0 / freq as f64
    }

    // ---- children ---------------------------------------------------------------------

    pub fn writer(a: &Args) -> anyhow::Result<i32> {
        let name = a.name.clone().unwrap_or_default();
        let m = Mapping::open(&name, Access::ReadWrite)?;
        let s = m.segment().ok_or_else(|| anyhow::anyhow!("mapping too small"))?;
        let pid = shm::current_pid();
        let epoch = shm::random_u64();
        let p = SideParams { pid, create_time: shm::process_create_time(pid).unwrap_or(0), epoch, caps: 0, build_id: "ipc-stress" };
        let att = match attach_sidecar(s, m.len(), a.parent_pid, None, &p) {
            Ok(x) => x,
            Err(r) => {
                println!("REFUSED code={} detail={}", r.code.as_str(), r.detail);
                return Ok(r.code.exit_code());
            }
        };
        let tag = tag_of(&name);
        println!("ATTACH epoch={:x} qpc={} count={}", epoch, shm::qpc(), att.attach_count);
        let _ = std::io::stdout().flush();
        let mut pose = LocalPose::zeroed();
        let mut blob = Blob::new_boxed();
        let mut rbuf = [0u8; MAX_PAYLOAD];
        let (mut counter, mut blobs, mut pushed, mut full) = (0u64, 0u64, 0u64, 0u64);
        let freq = shm::qpc_freq();
        let start = Instant::now();
        let deadline = start + Duration::from_secs_f64(a.secs);
        let die_at = a.die_mid_write_ms.map(|ms| start + Duration::from_millis(ms));
        let mut last = shm::qpc();
        let mut max_gap = 0u64;
        loop {
            counter += 1;
            if !a.fast || counter % 8 == 1 {
                fill(as_words_mut(&mut pose), counter, epoch, tag);
                s.game_out.local_pose.write(&pose);
            }
            if counter % 16 == 0 {
                fill(as_words_mut(&mut *blob), counter, epoch, tag);
                s.game_blobs.world_hash.publish(&blob);
                blobs += 1;
            }
            let n = ring_payload(&mut rbuf, counter, epoch);
            match s.g2s().push(epoch, 0x0280, 0, counter, &rbuf[..n]) {
                Ok(_) => pushed += 1,
                Err(PushError::Full) => full += 1,
                Err(e) => anyhow::bail!("push: {:?}", e),
            }
            let now = shm::qpc();
            s.header.sidecar.beat(now);
            max_gap = max_gap.max(now - last);
            last = now;
            if counter % 64 == 0 {
                let t = Instant::now();
                if let Some(d) = die_at {
                    if t >= d {
                        s.game_out.local_pose.begin_write_and_abandon();
                        println!("DIEMID qpc={}", shm::qpc());
                        let _ = std::io::stdout().flush();
                        std::process::exit(3);
                    }
                }
                if t >= deadline {
                    break;
                }
            }
        }
        println!(
            "WSTATS writes={} blobs={} pushed={} full={} max_gap_ms={:.3}",
            counter,
            blobs,
            pushed,
            full,
            ms_between(0, max_gap, freq)
        );
        Ok(0)
    }

    pub fn reader(a: &Args) -> anyhow::Result<i32> {
        let name = a.name.clone().unwrap_or_default();
        let m = Mapping::open(&name, Access::ReadWrite)?;
        let s = m.segment().ok_or_else(|| anyhow::anyhow!("mapping too small"))?;
        let tag = tag_of(&name);
        s.g2s().reset_consumer();
        println!("RSTART qpc={}", shm::qpc());
        let _ = std::io::stdout().flush();
        if a.stall_ms > 0 {
            std::thread::sleep(Duration::from_millis(a.stall_ms));
        }
        let mut st: HashMap<&'static str, u64> = HashMap::new();
        let mut inc = |k: &'static str, n: u64| *st.entry(k).or_insert(0) += n;
        let mut pose = LocalPose::zeroed();
        let mut blob = Blob::new_boxed();
        let mut rec = Record::zeroed();
        let mut cur_epoch = 0u64;
        let mut awaiting_recover = false;
        let mut last_ring_seq: Option<u64> = None;
        let mut watched: Option<(u32, ProcessHandle)> = None;
        let mut dead_reported = 0u32;
        let mut last_alive_check = Instant::now();
        let mut last_counter = 0u64;
        let deadline = Instant::now() + Duration::from_secs_f64(a.secs);
        let mut iter = 0u64;
        let mut rbad_logged = 0u32;
        loop {
            iter += 1;
            let e = s.header.sidecar.epoch.load(Ordering::Acquire);
            if e != cur_epoch {
                println!("EPOCH new={:x} qpc={}", e, shm::qpc());
                cur_epoch = e;
                awaiting_recover = true;
                last_counter = 0;
            }
            match s.game_out.local_pose.read_into(&mut pose) {
                Ok(_) => {
                    let (ok, counter, ep, tg) = check(as_words(&pose));
                    if !ok {
                        inc("torn", 1);
                    } else if tg != tag {
                        inc("tag_mismatch", 1);
                    } else if ep == cur_epoch {
                        inc("accepted", 1);
                        if counter < last_counter {
                            inc("went_back", 1);
                        }
                        last_counter = counter;
                        if awaiting_recover {
                            awaiting_recover = false;
                            println!("RECOVER epoch={:x} qpc={}", ep, shm::qpc());
                        }
                    } else {
                        inc("stale_epoch_slot", 1);
                    }
                }
                Err(ReadError::Busy) => inc("busy", 1),
                Err(ReadError::Empty) => inc("empty", 1),
            }
            if s.game_blobs.world_hash.take_into(&mut blob).is_some() {
                let (ok, _, _, tg) = check(as_words(&*blob));
                if !ok {
                    inc("blob_torn", 1);
                } else if tg != tag {
                    inc("tag_mismatch", 1);
                } else {
                    inc("blobs", 1);
                }
            }
            // No producer attached yet (epoch 0): leave the ring alone. `pop(0, ..)` accepts any
            // producer epoch, so a writer that attaches between the epoch load above and the pop
            // would hand us its records while we still compare them against epoch 0 (this race
            // showed up as an intermittent ring_bad in scenarios 2 / 4 / 5 under load or with a
            // faster writer).
            if iter == 1 && a.race_ms > 0 {
                std::thread::sleep(Duration::from_millis(a.race_ms));
            }
            let pops = if cur_epoch == 0 { 0 } else { 256 };
            for _ in 0..pops {
                let r = s.g2s().pop(cur_epoch, &mut rec);
                match r {
                    Pop::Empty => break,
                    Pop::Resynced => {
                        inc("ring_resynced", 1);
                        last_ring_seq = None;
                        continue;
                    }
                    _ => {}
                }
                if let Some(l) = last_ring_seq {
                    if rec.hdr.seq != l + 1 {
                        inc("ring_gap", 1);
                    }
                }
                last_ring_seq = Some(rec.hdr.seq);
                match r {
                    Pop::Record => {
                        if ring_ok(rec.payload(), cur_epoch) {
                            inc("ring_ok", 1);
                        } else {
                            inc("ring_bad", 1);
                            rbad_logged += 1;
                            if rbad_logged <= 5 {
                                eprintln!("RBAD record cur_epoch={:x} rec_epoch={:x} hdr_epoch_now={:x} seq={} len={}", cur_epoch, rec.hdr.producer_epoch, s.header.sidecar.epoch.load(Ordering::Acquire), rec.hdr.seq, rec.hdr.len);
                            }
                        }
                    }
                    Pop::StaleEpoch => inc("ring_stale_epoch", 1),
                    Pop::Bad => {
                        inc("ring_bad", 1);
                        eprintln!("RBAD malformed seq={} len={} head={} tail={}", rec.hdr.seq, rec.hdr.len, s.g2s().head(), s.g2s().tail());
                    }
                    _ => {}
                }
            }
            if last_alive_check.elapsed() >= Duration::from_millis(20) {
                last_alive_check = Instant::now();
                let pid = s.header.sidecar.pid.load(Ordering::Acquire);
                if pid != 0 && watched.as_ref().map(|w| w.0) != Some(pid) {
                    watched = ProcessHandle::open(pid).ok().map(|h| (pid, h));
                    dead_reported = 0;
                }
                if let Some((pid, h)) = &watched {
                    if !h.is_alive() && dead_reported != *pid {
                        dead_reported = *pid;
                        println!("DEAD pid={} qpc={}", pid, shm::qpc());
                        let _ = std::io::stdout().flush();
                    }
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            if iter % 4096 == 0 && Instant::now() >= deadline {
                break;
            }
        }
        let mut keys: Vec<_> = st.iter().collect();
        keys.sort();
        let line: Vec<String> = keys.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        println!("RSTATS {}", line.join(" "));
        Ok(0)
    }

    // ---- orchestration ----------------------------------------------------------------

    struct Seg {
        m: Mapping,
        name: String,
    }

    fn new_segment(k: &str) -> anyhow::Result<Seg> {
        let pid = shm::current_pid();
        let ct = shm::process_create_time(pid)?;
        let name = format!("{}.stress{:x}.{}", shm::mapping_name(pid, ct), shm::random_u64(), k);
        let (m, existed) = Mapping::create(&name)?;
        anyhow::ensure!(!existed, "segment name collision");
        let p = SideParams { pid, create_time: ct, epoch: shm::random_u64(), caps: u64::MAX, build_id: "ipc-stress-game" };
        // SAFETY: freshly created mapping of SEGMENT_SIZE bytes, owned by this process.
        unsafe { init_game(m.segment_ptr(), m.len(), &p, shm::qpc_freq()) }.map_err(|r| anyhow::anyhow!("{}", r))?;
        Ok(Seg { m, name })
    }

    static FAST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static RACE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// A child with its stdout pumped into `lines` by a thread (so the parent can wait
    /// for a line while the child runs).
    struct Kid {
        child: Child,
        lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        pump: Option<std::thread::JoinHandle<()>>,
    }

    fn adopt(mut child: Child) -> Kid {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let pump = child.stdout.take().map(|out| {
            let l = lines.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    l.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            })
        });
        Kid { child, lines, pump }
    }

    /// Wait until the child printed a line starting with `prefix` (it is up), at most `secs`.
    /// Fixed sleeps were not enough on a loaded machine: a reader that starts after its writer
    /// resets the ring past everything the writer pushed (scenario 4: `ring ok 0`).
    fn wait_line(k: &Kid, prefix: &str, secs: f64) -> anyhow::Result<()> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs_f64(secs) {
            if k.lines.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|l| l.starts_with(prefix)) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        anyhow::bail!("child did not print {} within {} s", prefix, secs)
    }

    /// Spawn a reader and wait until it reset the ring and is consuming (RSTART).
    fn spawn_reader(name: &str, secs: f64, extra: &[String]) -> anyhow::Result<Kid> {
        let r = spawn("reader", name, secs, extra)?;
        wait_line(&r, "RSTART", 20.0)?;
        Ok(r)
    }

    fn spawn(role: &str, name: &str, secs: f64, extra: &[String]) -> anyhow::Result<Kid> {
        let exe = std::env::current_exe()?;
        let mut c = Command::new(exe);
        c.args(["ipc-stress", "--role", role, "--name", name, "--secs", &format!("{:.3}", secs)]);
        c.args(["--parent-pid", &shm::current_pid().to_string()]);
        c.args(extra);
        if role == "writer" && FAST.load(Ordering::Relaxed) {
            c.arg("--fast");
        }
        let race = RACE_MS.load(Ordering::Relaxed);
        if role == "reader" && race > 0 {
            c.args(["--race-ms".to_string(), race.to_string()]);
        }
        c.stdout(Stdio::piped()).stderr(Stdio::inherit());
        Ok(adopt(c.spawn()?))
    }

    struct Out {
        code: i32,
        lines: Vec<String>,
    }
    impl Out {
        fn kv(&self, prefix: &str) -> HashMap<String, f64> {
            let mut m = HashMap::new();
            for l in self.lines.iter().filter(|l| l.starts_with(prefix)) {
                for part in l.split_whitespace().skip(1) {
                    if let Some((k, v)) = part.split_once('=') {
                        if let Ok(x) = v.parse::<f64>() {
                            m.insert(k.to_string(), x);
                        }
                    }
                }
            }
            m
        }
        fn qpcs(&self, prefix: &str) -> Vec<u64> {
            self.lines
                .iter()
                .filter(|l| l.starts_with(prefix))
                .filter_map(|l| l.split_whitespace().find_map(|p| p.strip_prefix("qpc=")).and_then(|v| v.parse().ok()))
                .collect()
        }
    }
    fn finish(mut k: Kid) -> anyhow::Result<Out> {
        let st = k.child.wait()?;
        if let Some(p) = k.pump.take() {
            let _ = p.join();
        }
        let lines = std::mem::take(&mut *k.lines.lock().unwrap_or_else(|e| e.into_inner()));
        Ok(Out { code: st.code().unwrap_or(-1), lines })
    }
    fn kill(mut k: Kid) -> anyhow::Result<(u64, Out)> {
        let q = shm::qpc();
        k.child.kill()?; // TerminateProcess on this child's own handle
        Ok((q, finish(k)?))
    }    fn g(m: &HashMap<String, f64>, k: &str) -> f64 {
        m.get(k).copied().unwrap_or(0.0)
    }
    fn sleep(s: f64) {
        std::thread::sleep(Duration::from_secs_f64(s.max(0.0)));
    }

    struct Res {
        pass: bool,
        detail: String,
    }
    fn res(checks: &[(bool, String)], info: String) -> Res {
        let failed: Vec<&String> = checks.iter().filter(|c| !c.0).map(|c| &c.1).collect();
        if failed.is_empty() {
            Res { pass: true, detail: info }
        } else {
            Res { pass: false, detail: format!("FAILED: {} | {}", failed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; "), info) }
        }
    }
    fn reader_clean(r: &HashMap<String, f64>) -> Vec<(bool, String)> {
        vec![
            (g(r, "torn") == 0.0, format!("torn={}", g(r, "torn"))),
            (g(r, "blob_torn") == 0.0, format!("blob_torn={}", g(r, "blob_torn"))),
            (g(r, "ring_bad") == 0.0, format!("ring_bad={}", g(r, "ring_bad"))),
            (g(r, "ring_gap") == 0.0, format!("ring_gap={}", g(r, "ring_gap"))),
            (g(r, "tag_mismatch") == 0.0, format!("tag_mismatch={}", g(r, "tag_mismatch"))),
            (g(r, "went_back") == 0.0, format!("went_back={}", g(r, "went_back"))),
        ]
    }

    fn s1_rate(f: f64) -> anyhow::Result<Res> {
        let seg = new_segment("rate")?;
        let d = 5.0 * f;
        let r = spawn_reader(&seg.name, d + 0.5, &[])?;
        let w = spawn("writer", &seg.name, d, &[])?;
        let (wo, ro) = (finish(w)?, finish(r)?);
        let (ws, rs) = (wo.kv("WSTATS"), ro.kv("RSTATS"));
        let mut c = reader_clean(&rs);
        c.push((wo.code == 0 && ro.code == 0, format!("exit w={} r={}", wo.code, ro.code)));
        c.push((g(&rs, "accepted") > 0.0, "reader accepted slot values".into()));
        c.push((g(&rs, "blobs") > 0.0, "reader took blobs".into()));
        c.push((g(&rs, "ring_ok") > 0.0, "reader consumed ring records".into()));
        let info = format!(
            "writes {:.0} ({:.0}/s), blobs {:.0}, ring {:.0} ({:.0}/s, full {:.0}); reads ok {:.0} busy {:.0}, blobs {:.0}, ring ok {:.0}",
            g(&ws, "writes"),
            g(&ws, "writes") / d,
            g(&ws, "blobs"),
            g(&ws, "pushed"),
            g(&ws, "pushed") / d,
            g(&ws, "full"),
            g(&rs, "accepted"),
            g(&rs, "busy"),
            g(&rs, "blobs"),
            g(&rs, "ring_ok")
        );
        drop(seg.m);
        Ok(res(&c, info))
    }

    fn s2_kill_writer(f: f64) -> anyhow::Result<Res> {
        let seg = new_segment("killw")?;
        let freq = shm::qpc_freq();
        let mut rng = rand::thread_rng();
        let total = 7.0 * f.max(0.6);
        let r = spawn_reader(&seg.name, total, &[])?;
        let w1 = spawn("writer", &seg.name, 100.0, &[])?;
        sleep(rng.gen_range(0.5..1.5));
        let (k1, w1o) = kill(w1)?;
        sleep(0.3);
        let die_ms = rng.gen_range(300..800u64);
        let w2 = spawn("writer", &seg.name, 100.0, &["--die-mid-write-ms".into(), die_ms.to_string()])?;
        let w2o = finish(w2)?;
        let k2 = w2o.qpcs("DIEMID").first().copied().unwrap_or(0);
        sleep(0.4);
        let w3 = spawn("writer", &seg.name, (total - 3.5).max(1.0), &[])?;
        let w3o = finish(w3)?;
        let ro = finish(r)?;
        let rs = ro.kv("RSTATS");
        let deads = ro.qpcs("DEAD");
        let recovers = ro.qpcs("RECOVER");
        let attaches: Vec<u64> = [&w1o, &w2o, &w3o].iter().filter_map(|o| o.qpcs("ATTACH").first().copied()).collect();
        let dead_after = |k: u64| deads.iter().filter(|&&d| d >= k).map(|&d| ms_between(k, d, freq)).fold(f64::INFINITY, f64::min);
        let (d1, d2) = (dead_after(k1), dead_after(k2));
        let rec_ms: Vec<f64> = attaches
            .iter()
            .map(|&a| recovers.iter().filter(|&&r| r >= a).map(|&r| ms_between(a, r, freq)).fold(f64::INFINITY, f64::min))
            .collect();
        let mut c = reader_clean(&rs);
        c.push((w2o.code == 3 && k2 != 0, format!("die-mid-write writer exit {}", w2o.code)));
        c.push((w3o.code == 0 && ro.code == 0, format!("exit w3={} r={}", w3o.code, ro.code)));
        c.push((d1 <= 1000.0, format!("death after kill seen in {:.0} ms", d1)));
        c.push((d2 <= 1000.0, format!("death after mid-write exit seen in {:.0} ms", d2)));
        c.push((g(&rs, "busy") > 0.0, "reader saw Busy on the abandoned slot".into()));
        c.push((attaches.len() == 3, format!("{} attaches", attaches.len())));
        c.push((rec_ms.iter().all(|&x| x <= 100.0), format!("recovery after attach {:?} ms", rec_ms.iter().map(|x| x.round()).collect::<Vec<_>>())));
        let info = format!(
            "kill->dead {:.0} ms, midwrite->dead {:.0} ms, attach->recover {:?} ms, busy {:.0}, stale-epoch slot reads rejected {:.0}, stale ring records dropped {:.0}",
            d1,
            d2,
            rec_ms.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>(),
            g(&rs, "busy"),
            g(&rs, "stale_epoch_slot"),
            g(&rs, "ring_stale_epoch")
        );
        drop(seg.m);
        Ok(res(&c, info))
    }

    fn s3_kill_reader(f: f64) -> anyhow::Result<Res> {
        let seg = new_segment("killr")?;
        let d = 5.0 * f.max(0.6);
        let w = spawn("writer", &seg.name, d, &[])?;
        let r1 = spawn_reader(&seg.name, 100.0, &[])?;
        sleep(d * 0.3);
        let (_, r1o) = kill(r1)?;
        sleep(d * 0.1);
        let r2 = spawn_reader(&seg.name, d * 0.5, &[])?;
        let (wo, r2o) = (finish(w)?, finish(r2)?);
        let (ws, rs) = (wo.kv("WSTATS"), r2o.kv("RSTATS"));
        let mut c = reader_clean(&rs);
        c.push((wo.code == 0 && r2o.code == 0, format!("exit w={} r2={}", wo.code, r2o.code)));
        c.push((g(&ws, "max_gap_ms") < 50.0, format!("writer max loop gap {:.2} ms", g(&ws, "max_gap_ms"))));
        c.push((g(&rs, "ring_ok") > 0.0, "restarted reader consumes the ring".into()));
        c.push((!r1o.lines.iter().any(|l| l.starts_with("RSTATS")), "first reader was killed".into()));
        let info = format!("writer max gap {:.2} ms, full {:.0}; restarted reader ring ok {:.0}", g(&ws, "max_gap_ms"), g(&ws, "full"), g(&rs, "ring_ok"));
        drop(seg.m);
        Ok(res(&c, info))
    }

    fn s4_stall(f: f64) -> anyhow::Result<Res> {
        let seg = new_segment("stall")?;
        let d = 4.0 * f.max(0.6);
        let stall = (d * 500.0) as u64;
        let r = spawn_reader(&seg.name, d + 0.5, &["--stall-ms".into(), stall.to_string()])?;
        let w = spawn("writer", &seg.name, d, &[])?;
        let (wo, ro) = (finish(w)?, finish(r)?);
        let (ws, rs) = (wo.kv("WSTATS"), ro.kv("RSTATS"));
        let mut c = reader_clean(&rs);
        c.push((g(&ws, "full") > 0.0, format!("producer saw Full {:.0} times", g(&ws, "full"))));
        c.push((g(&rs, "ring_ok") > 0.0, "consumer recovered after the stall".into()));
        c.push((wo.code == 0 && ro.code == 0, format!("exit w={} r={}", wo.code, ro.code)));
        let info = format!("stall {} ms: full {:.0}, ring ok {:.0}", stall, g(&ws, "full"), g(&rs, "ring_ok"));
        drop(seg.m);
        Ok(res(&c, info))
    }

    fn s5_two_segments(f: f64) -> anyhow::Result<Res> {
        let a = new_segment("a")?;
        let b = new_segment("b")?;
        let d = 3.0 * f.max(0.6);
        let ra = spawn_reader(&a.name, d + 0.5, &[])?;
        let rb = spawn_reader(&b.name, d + 0.5, &[])?;
        let wa = spawn("writer", &a.name, d, &[])?;
        let wb = spawn("writer", &b.name, d, &[])?;
        let outs = [finish(wa)?, finish(wb)?, finish(ra)?, finish(rb)?];
        let mut c = Vec::new();
        for (i, o) in outs.iter().enumerate() {
            c.push((o.code == 0, format!("child {} exit {}", i, o.code)));
        }
        for o in &outs[2..] {
            let rs = o.kv("RSTATS");
            c.extend(reader_clean(&rs));
            c.push((g(&rs, "accepted") > 0.0, "reader accepted values".into()));
        }
        let ea = a.m.segment().unwrap().header.sidecar.epoch.load(Ordering::Acquire);
        let eb = b.m.segment().unwrap().header.sidecar.epoch.load(Ordering::Acquire);
        c.push((ea != eb && ea != 0 && eb != 0, "distinct sidecar epochs".into()));
        Ok(res(&c, "two segments isolated (tags and epochs)".into()))
    }

    fn s6_refuse() -> anyhow::Result<Res> {
        let mut c = Vec::new();
        let seg = new_segment("ref1")?;
        let exe_wrong = {
            let exe = std::env::current_exe()?;
            Command::new(exe)
                .args(["ipc-stress", "--role", "writer", "--name", &seg.name, "--secs", "0.1", "--parent-pid", "1"])
                .stdout(Stdio::piped())
                .spawn()?
        };
        let exe_wrong = adopt(exe_wrong);
        let o = finish(exe_wrong)?;
        c.push((o.code == RefuseCode::WrongParent.exit_code(), format!("wrong parent exit {} (want 72)", o.code)));
        c.push((seg.m.segment().unwrap().header.prefix.refuse_code() == RefuseCode::WrongParent, "wrong parent recorded in header".into()));

        let seg2 = new_segment("ref2")?;
        // SAFETY: our own live mapping.
        unsafe { debug_corrupt_prefix(seg2.m.segment_ptr(), None, None, Some(hsmp_ipc::LAYOUT_HASH ^ 0x55), None) };
        let o = finish(spawn("writer", &seg2.name, 0.1, &[])?)?;
        c.push((o.code == RefuseCode::AbiMismatch.exit_code(), format!("layout hash exit {} (want 71)", o.code)));
        c.push((seg2.m.segment().unwrap().header.prefix.refuse_code() == RefuseCode::AbiMismatch, "abi mismatch recorded".into()));

        let seg3 = new_segment("ref3")?;
        // SAFETY: as above.
        unsafe { debug_corrupt_prefix(seg3.m.segment_ptr(), Some(*b"XXXXXXXX"), None, None, None) };
        let o = finish(spawn("writer", &seg3.name, 0.1, &[])?)?;
        c.push((o.code == RefuseCode::BadMagic.exit_code(), format!("bad magic exit {} (want 70)", o.code)));
        c.push((seg3.m.segment().unwrap().header.prefix.refuse_code() == RefuseCode::None, "bad magic leaves the header alone".into()));
        Ok(res(&c, "refuse codes 72 / 71 / 70".into()))
    }

    pub fn orchestrate(a: &Args) -> anyhow::Result<i32> {
        let f = (a.duration / 25.0).max(0.2);
        FAST.store(a.fast, Ordering::Relaxed);
        RACE_MS.store(a.race_ms, Ordering::Relaxed);
        let only: Option<Vec<u32>> = a.only.as_ref().map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect());
        let want = |n: u32| only.as_ref().map_or(true, |v| v.contains(&n));
        let t0 = Instant::now();
        let mut rows: Vec<(u32, &str, Res)> = Vec::new();
        type Scen = fn(f64) -> anyhow::Result<Res>;
        let scen: [(u32, &str, Scen); 5] = [
            (1, "max-rate writer vs reader", s1_rate),
            (2, "kill writer / die mid-write / restart", s2_kill_writer),
            (3, "kill + restart reader", s3_kill_reader),
            (4, "stalled consumer", s4_stall),
            (5, "two segments in parallel", s5_two_segments),
        ];
        for (n, title, fun) in scen {
            if want(n) {
                println!("ipc-stress: scenario {} {} ...", n, title);
                let r = fun(f).unwrap_or_else(|e| Res { pass: false, detail: format!("error: {:#}", e) });
                rows.push((n, title, r));
            }
        }
        if want(6) {
            println!("ipc-stress: scenario 6 refusals ...");
            rows.push((6, "wrong parent / hash / magic", s6_refuse().unwrap_or_else(|e| Res { pass: false, detail: format!("error: {:#}", e) })));
        }
        println!("\n{:<3} {:<40} {:<5} detail", "#", "scenario", "pass");
        for (n, t, r) in &rows {
            println!("{:<3} {:<40} {:<5} {}", n, t, if r.pass { "PASS" } else { "FAIL" }, r.detail);
        }
        let all = rows.iter().all(|r| r.2.pass);
        println!("\nipc-stress: {} in {:.1} s", if all { "ALL PASS" } else { "FAILED" }, t0.elapsed().as_secs_f64());
        Ok(if all { 0 } else { 1 })
    }
}

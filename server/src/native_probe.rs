//! Bounded real-network observation of a running native host. This does not certify combat parity.
use anyhow::{bail, Context, Result};
use clap::Parser;
use hsmp_server::{
    native_service::ClientHandle,
    native_wire::{self as w, EntityRef, InputFrame},
};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "hsmp-server native-probe",
    about = "Observe a native host with two real network clients"
)]
struct Args {
    #[arg(long)]
    server: SocketAddr,
    #[arg(long)]
    identity_dir: PathBuf,
    #[arg(long)]
    report: PathBuf,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=180))]
    ready_seconds: u64,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(2..=60))]
    observe_seconds: u64,
    /// Exercise each owned pawn's move axis briefly. Requires an explicitly authorized native test.
    #[arg(long)]
    exercise_input: bool,
}

fn own_entity(client: &ClientHandle) -> Option<EntityRef> {
    let peer = client.peer_id();
    client
        .directory()?
        .entities
        .iter()
        .find(|e| e.kind == w::HUMAN && e.owner_peer == peer && peer != 0)
        .map(|e| e.reference)
}
fn ready(client: &ClientHandle) -> bool {
    client.connected()
        && client.directory().is_some_and(|d| {
            matches!(d.state, w::READY | w::LIVE)
                && d.entities.len() == 3
                && d.entities
                    .iter()
                    .filter(|e| e.kind == w::HUMAN && e.owner_peer != 0)
                    .count()
                    == 2
        })
        && own_entity(client).is_some()
}

pub fn run() -> Result<()> {
    let argv = std::iter::once(std::ffi::OsString::from("hsmp-server native-probe"))
        .chain(std::env::args_os().skip(2));
    let args = Args::parse_from(argv);
    std::fs::create_dir_all(&args.identity_dir)?;
    let a = ClientHandle::start(
        args.server,
        &args.identity_dir.join("a"),
        "NativeProbeA",
        None,
    )?;
    let b = ClientHandle::start(
        args.server,
        &args.identity_dir.join("b"),
        "NativeProbeB",
        None,
    )?;
    let started = Instant::now();
    while !ready(&a) || !ready(&b) {
        if started.elapsed() >= Duration::from_secs(args.ready_seconds) {
            bail!("native host did not provide two authenticated seats and a ready native world");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let refs = [
        own_entity(&a).context("client A has no native seat")?,
        own_entity(&b).context("client B has no native seat")?,
    ];
    if refs[0] == refs[1] {
        bail!("native clients share a pawn reference");
    }
    let mut frames = [0u32; 2];
    let mut observations = [0u32; 2];
    let mut shared = 0u32;
    let mut inputs = [0u32; 2];
    let mut seen: BTreeMap<u32, [Option<std::sync::Arc<w::World>>; 2]> = BTreeMap::new();
    let mut first = None;
    let mut last = None;
    let observing = Instant::now();
    let mut seq = 1u32;
    while observing.elapsed() < Duration::from_secs(args.observe_seconds) {
        for (i, client) in [&a, &b].into_iter().enumerate() {
            if own_entity(client) != Some(refs[i]) {
                bail!("native pawn ownership changed during observation");
            }
            if let Some(world) = client.snapshot() {
                if world.frame_seq > frames[i] {
                    if world.entities.len() != 3 {
                        bail!("incomplete native world frame");
                    }
                    frames[i] = world.frame_seq;
                    observations[i] += 1;
                    if first.is_none() {
                        first = Some(world.clone());
                    }
                    last = Some(world.clone());
                    let pair = seen.entry(world.frame_seq).or_insert_with(|| [None, None]);
                    pair[i] = Some(world);
                    if let [Some(x), Some(y)] = pair {
                        if x != y {
                            bail!("same native frame differs between authenticated clients");
                        }
                        shared += 1;
                    }
                    while seen.len() > 128 {
                        seen.pop_first();
                    }
                }
            }
            if args.exercise_input {
                let t = observing.elapsed().as_secs_f64();
                let mut axes = [0.0; 8];
                if (i == 0 && (1.0..2.0).contains(&t)) || (i == 1 && (3.0..4.0).contains(&t)) {
                    axes[0] = 0.3;
                }
                client
                    .input(InputFrame {
                        reference: refs[i],
                        seq,
                        axes,
                        ..Default::default()
                    })
                    .map_err(anyhow::Error::msg)?;
                inputs[i] += 1;
            }
        }
        seq = seq.checked_add(1).context("probe sequence exhausted")?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = serde_json::json!({
        "evidence_level":"actual native host over authenticated UDP; no combat parity claim",
        "observed_frames":observations,"last_frame_seq":frames,"identical_shared_frames":shared,
        "input_frames_submitted":inputs,"owned_refs":refs.map(|r|serde_json::json!({"epoch":r.epoch,"id":r.id,"incarnation":r.incarnation})),
        "first_roots":first.as_ref().map(|world|world.entities.iter().map(|e|serde_json::json!({"id":e.reference.id,"pos":e.root.pos,"health_q64":e.vitals.v[0],"flags":e.vitals.flags})).collect::<Vec<_>>()),
        "last_roots":last.as_ref().map(|world|world.entities.iter().map(|e|serde_json::json!({"id":e.reference.id,"pos":e.root.pos,"health_q64":e.vitals.v[0],"flags":e.vitals.flags})).collect::<Vec<_>>()),
        "pass":observations.iter().all(|n|*n>=10)&&shared>=3,
    });
    if let Some(parent) = args.report.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.report, serde_json::to_vec_pretty(&report)?)?;
    eprintln!(
        "Native frames observed: {observations:?}; identical shared frames: {shared}. Report: {}",
        args.report.display()
    );
    if report["pass"] != true {
        bail!("insufficient coherent native publication evidence");
    }
    Ok(())
}

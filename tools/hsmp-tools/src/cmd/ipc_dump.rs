//! `hsmp-tools ipc-dump`: a live, READ-ONLY view of a running game's HSMP-SHM segment
//! (docs/development/ipc-shared-memory.md). Opens the mapping with FILE_MAP_READ only and
//! never writes.
//!
//!   hsmp-tools ipc-dump --pid <gamePID> [--json] [--full] [--records N]
//!   hsmp-tools ipc-dump --name <mapping> --follow        (tail the G2S/S2G/DevCtl rings)
//!   hsmp-tools ipc-dump --pid <gamePID> --get peer_play --peer 2 --json

use anyhow::{bail, Result};
use hsmp_ipc::ring::Record;
use hsmp_ipc::schema::pose::PeerDir;
use hsmp_ipc::shm::Access;
use hsmp_tools::ipcgame as ig;
use serde_json::{json, Value as J};
use std::time::Duration;

#[derive(clap::Args)]
pub struct Args {
    /// The game process id (the mapping name is derived from it and its creation time).
    #[arg(long)]
    pub pid: Option<u32>,
    /// The mapping name (`Local\HSMP.ipc.<abi>.<pid>.<ctime>`), instead of --pid.
    #[arg(long)]
    pub name: Option<String>,
    /// One JSON object instead of the text summary.
    #[arg(long)]
    pub json: bool,
    /// Include every bone of the local pose and the peer plays.
    #[arg(long)]
    pub full: bool,
    /// Ring records to show per ring.
    #[arg(long, default_value_t = 8)]
    pub records: usize,
    /// Keep running and print every new G2S / S2G / DevCtl record (JSON lines).
    #[arg(long)]
    pub follow: bool,
    /// Print one part only: header, local_root, local_weapon, local_pose, pose_lead, vitals,
    /// kit, peer_dir, peer_play, peer_root, peer_vitals, peer_kit, session, bus, g2s, s2g, devctl,
    /// records (every ABI-2 record slot), or any record slot by its schema name.
    #[arg(long)]
    pub get: Option<String>,
    /// With --get peer_*: the peer id ...
    #[arg(long)]
    pub peer: Option<u32>,
    /// ... or the peer slot.
    #[arg(long)]
    pub slot: Option<usize>,
}

pub fn open(pid: Option<u32>, name: Option<&str>, access: Access) -> Result<hsmp_ipc::Mapping> {
    match (pid, name) {
        (_, Some(n)) => ig::open_name(n, access),
        (Some(p), None) => ig::open_pid(p, access),
        (None, None) => bail!("pass --pid <gamePID> or --name <mapping>"),
    }
}

pub fn run(a: Args) -> Result<i32> {
    let m = open(a.pid, a.name.as_deref(), Access::ReadOnly)?;
    let s = ig::segment(&m)?;
    if a.follow {
        return follow(s);
    }
    let d = ig::dump_json(s, a.records, a.full);
    if let Some(part) = a.get.as_deref() {
        let v = get_part(s, &d, part, a.peer, a.slot)?;
        println!("{}", if a.json { v.to_string() } else { serde_json::to_string_pretty(&v)? });
        return Ok(if v.is_null() { 1 } else { 0 });
    }
    if a.json {
        let mut d = d;
        d["name"] = json!(m.name());
        println!("{d}");
        return Ok(0);
    }
    print_text(m.name(), &d);
    Ok(0)
}

fn get_part(s: &hsmp_ipc::Segment, d: &J, part: &str, peer: Option<u32>, slot: Option<usize>) -> Result<J> {
    if part == "records" {
        return Ok(d["records"].clone());
    }
    // An ABI-2 record slot by its schema name (per-peer ones with --peer / --slot).
    if let Some(info) = hsmp_ipc::schema::slot_by_name(part) {
        let v = &d["records"][part];
        return Ok(match info.form {
            hsmp_ipc::schema::SlotForm::PeerSlot | hsmp_ipc::schema::SlotForm::PeerBlob => match (peer, slot) {
                (Some(id), _) => v[id.to_string()].clone(),
                (None, Some(sl)) => v.as_object().into_iter().flatten().map(|(_, e)| e).find(|e| e["slot"] == json!(sl)).cloned().unwrap_or(J::Null),
                (None, None) => v.clone(),
            },
            _ => v.clone(),
        });
    }
    if let Some(p) = part.strip_prefix("peer_").filter(|p| *p != "dir") {
        let mut dir = Box::<PeerDir>::default();
        let _ = s.peers.dir.read_into(&mut dir);
        let slot = match (slot, peer) {
            (Some(sl), _) => Some(sl),
            (None, Some(id)) => dir.slot_of(id),
            (None, None) => bail!("--get peer_{p} needs --peer <id> or --slot <n>"),
        };
        let Some(slot) = slot else { return Ok(J::Null) };
        let e = d["peers"].as_array().into_iter().flatten().find(|e| e["slot"] == json!(slot));
        return Ok(e.map(|e| e[p]["v"].clone()).unwrap_or(J::Null));
    }
    Ok(match part {
        "header" => d["header"].clone(),
        "peer_dir" => d["peer_dir"].clone(),
        "session" => d["records"]["session"]["v"].clone(),
        "bus" => d["bus"].clone(),
        "g2s" => d["g2s_last"].clone(),
        "s2g" => d["s2g_last"].clone(),
        "devctl" => d["devctl_last"].clone(),
        other => {
            let v = &d["game_out"][other];
            if v.is_null() {
                J::Null
            } else if v.get("v").is_some() {
                v["v"].clone()
            } else {
                v.clone()
            }
        }
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn follow(s: &hsmp_ipc::Segment) -> Result<i32> {
    let mut pos = [s.g2s().tail(), s.s2g().tail(), s.devctl().tail()];
    let mut r = Record::default();
    loop {
        for (i, ring) in ["g2s", "s2g", "devctl"].into_iter().enumerate() {
            let tail = match i {
                0 => s.g2s().tail(),
                1 => s.s2g().tail(),
                _ => s.devctl().tail(),
            };
            while pos[i] < tail {
                let q = pos[i];
                let ok = match i {
                    0 => s.g2s().peek(q, &mut r),
                    1 => s.s2g().peek(q, &mut r),
                    _ => s.devctl().peek(q, &mut r),
                };
                if ok {
                    let mut j = ig::record_json(&r);
                    j["ring"] = json!(ring);
                    j["t"] = json!(now_ms());
                    println!("{j}");
                } else {
                    println!("{}", json!({"ring": ring, "seq": q, "lost": true, "t": now_ms()}));
                }
                pos[i] += 1;
            }
        }
        if s.header.game.state() == Some(hsmp_ipc::SideState::Absent) {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn print_text(name: &str, d: &J) {
    let h = &d["header"];
    println!("segment {name}");
    println!("  abi {}  layout {}  refuse {} {}", h["abi"], h["layout_hash"], h["refuse"], h["refuse_detail"]);
    println!("  caps effective {} {}", h["caps_effective"], h["caps_effective_names"]);
    println!(
        "  epochs: world {}  session {}  lua_gen {}  attach_count {}  resync_req {}",
        h["world_epoch"], h["session_epoch"], h["game_lua_gen"], h["attach_count"], h["resync_req"]
    );
    for side in ["game", "sidecar"] {
        let b = &h[side];
        println!(
            "  {side:8} pid {} state {} epoch {} caps {} hb {} age_s {} build {}  counters {}",
            b["pid"], b["state"], b["epoch"], b["caps"], b["hb_count"], b["hb_age_s"], b["build_id"], b["counters"]
        );
    }
    println!("  rings: g2s {}  s2g {}  devctl {}", h["g2s"], h["s2g"], h["devctl"]);
    for (k, v) in d["game_out"].as_object().into_iter().flatten() {
        println!("  game_out.{k}: seq {} {}", v["seq"], short(&v["v"]));
    }
    println!("  session: {}", short(&d["records"]["session"]));
    println!("  link: {}", short(&d["records"]["link"]));
    println!("  peer_dir: {}", short(&d["peer_dir"]));
    for p in d["peers"].as_array().into_iter().flatten() {
        let pl = &p["play"]["v"];
        println!(
            "  peer {} slot {} {:?}: play seq {} mode {} age {} delay {} slots {} | root {} | vitals {} | kit {}",
            p["peer_id"], p["slot"], p["nick"].as_str().unwrap_or(""), p["play"]["seq"], pl["mode"], pl["age"], pl["delay"],
            pl["slots"], short(&p["root"]["v"]["pos"]), short(&p["vitals"]["v"]), short(&p["kit"]["v"])
        );
    }
    for (k, v) in d["bus"].as_object().into_iter().flatten() {
        println!("  bus.{k}: {}", short(v));
    }
    for (k, v) in d["records"].as_object().into_iter().flatten() {
        println!("  record.{k}: {}", short(v));
    }
    for ring in ["g2s_last", "s2g_last", "devctl_last"] {
        for r in d[ring].as_array().into_iter().flatten() {
            println!("  {ring} #{} {} req {} {}", r["seq"], r["kind"], r["req_id"], short(&r["v"]));
        }
    }
}

fn short(v: &J) -> String {
    let s = v.to_string();
    if s.chars().count() > 200 {
        format!("{}...", s.chars().take(200).collect::<String>())
    } else {
        s
    }
}

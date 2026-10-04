//! `hsmp-tools natlab`: local stand-ins for the NAT traversal tests (scripts/e2e-nat.sh,
//! scripts/e2e-master-cf.sh). Nothing here talks to a real router or a public server.
//!
//!     hsmp-tools natlab stun  --bind 127.0.0.1:34780 [--secs 600]
//!         a STUN server: answers every Binding request with the address it came from
//!     hsmp-tools natlab catch --bind 127.0.0.1:34790 [--secs 10]
//!         wait for punch probes (hsmp_nat::probe); prints `probes <n> from <addr>`,
//!         exit 0 if at least one arrived, 1 if none did

use anyhow::{Context, Result};
use hsmp_nat::{probe, stun};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    what: What,
}

#[derive(clap::Subcommand)]
enum What {
    /// Answer STUN Binding requests with the source address
    Stun {
        #[arg(long)]
        bind: String,
        /// Exit after this long (no orphan when a test dies)
        #[arg(long, default_value_t = 600)]
        secs: u64,
    },
    /// Wait for punch probes
    Catch {
        #[arg(long)]
        bind: String,
        #[arg(long, default_value_t = 10)]
        secs: u64,
    },
}

pub fn run(a: Args) -> Result<i32> {
    match a.what {
        What::Stun { bind, secs } => stun_server(&bind, secs),
        What::Catch { bind, secs } => catch(&bind, secs),
    }
}

fn stun_server(bind: &str, secs: u64) -> Result<i32> {
    let s = UdpSocket::bind(bind).with_context(|| format!("bind {bind}"))?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    println!("stun listening {}", s.local_addr()?);
    let end = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 1500];
    let mut n_ans = 0u64;
    while Instant::now() < end {
        let Ok((n, from)) = s.recv_from(&mut buf) else { continue };
        if let Some(stun::Parsed::Request { tx }) = stun::parse(&buf[..n]) {
            let _ = s.send_to(&stun::binding_success(&tx, from), from);
            n_ans += 1;
            if n_ans <= 20 {
                println!("stun answered {from}");
            }
        }
    }
    Ok(0)
}

fn catch(bind: &str, secs: u64) -> Result<i32> {
    let s = UdpSocket::bind(bind).with_context(|| format!("bind {bind}"))?;
    s.set_read_timeout(Some(Duration::from_millis(200)))?;
    let end = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 1500];
    let (mut n_probe, mut last) = (0u32, None);
    // a probe burst is 4 datagrams within ~1 s: keep counting a moment after the first
    let mut stop_at = end;
    while Instant::now() < stop_at {
        let Ok((n, from)) = s.recv_from(&mut buf) else { continue };
        if probe::is_probe(&buf[..n]) {
            n_probe += 1;
            last = Some(from);
            stop_at = stop_at.min(Instant::now() + Duration::from_millis(1500));
        }
    }
    match last {
        Some(from) => {
            println!("probes {n_probe} from {from}");
            Ok(0)
        }
        None => {
            println!("probes 0");
            Ok(1)
        }
    }
}

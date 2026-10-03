//! `hsmp-tools rcon ADDR "LINE[@ms]"...`: talk to hsmp-server's RCON TCP port.
//!
//! For each LINE: send `LINE\n`, wait `ms` (default 300), read one reply (up
//! to 256 bytes, 2 s timeout). Prints the trimmed LAST reply (used by
//! scripts/e2e-test.sh):
//!
//!     hsmp-tools rcon 127.0.0.1:38301 "AUTH e2epw"
//!     hsmp-tools rcon 127.0.0.1:38301 "AUTH e2epw@200" "BAN 1@500"

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(clap::Args)]
pub struct Args {
    /// RCON address, host:port
    addr: String,
    /// Lines to send, each optionally suffixed with @<ms> to wait before reading (default 300)
    #[arg(required = true)]
    lines: Vec<String>,
    /// Read timeout per reply, ms
    #[arg(long, default_value_t = 2000)]
    timeout_ms: u64,
}

fn split(spec: &str) -> (String, u64) {
    if let Some((l, ms)) = spec.rsplit_once('@') {
        if let Ok(ms) = ms.parse() {
            return (l.to_string(), ms);
        }
    }
    (spec.to_string(), 300)
}

pub fn run(a: Args) -> Result<i32> {
    let addr = a.addr.to_socket_addrs()?.next().context("bad address")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(a.timeout_ms))
        .with_context(|| format!("connect {}", a.addr))?;
    s.set_read_timeout(Some(Duration::from_millis(a.timeout_ms)))?;
    let mut last = String::new();
    for spec in &a.lines {
        let (line, ms) = split(spec);
        s.write_all(format!("{line}\n").as_bytes())?;
        std::thread::sleep(Duration::from_millis(ms));
        let mut buf = [0u8; 256];
        let n = match s.read(&mut buf) {
            Ok(n) => n,
            Err(e) => bail!("no reply to {line:?}: {e}"),
        };
        last = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    }
    println!("{last}");
    Ok(0)
}

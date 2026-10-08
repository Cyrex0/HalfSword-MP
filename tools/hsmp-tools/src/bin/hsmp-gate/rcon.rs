//! RCON client for hsmp-server's line protocol (server/src/rcon.rs):
//! `AUTH <pw>` -> `OK ...`, then one command; the reply ends with a line starting
//! with OK / ERR, or `END` for listings.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Returns (accepted, full reply). Err = connection problem.
pub fn send(
    addr: &str,
    password: &str,
    cmd: &str,
    timeout: Duration,
) -> std::io::Result<(bool, String)> {
    let sa = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad address"))?;
    let stream = TcpStream::connect_timeout(&sa, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut w = stream.try_clone()?;
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    let mut recv = |r: &mut BufReader<TcpStream>| -> std::io::Result<String> {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "rcon closed",
            ));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    w.write_all(format!("AUTH {password}\n").as_bytes())?;
    let a = recv(&mut r)?;
    if !a.starts_with("OK") {
        return Ok((false, a));
    }
    w.write_all(format!("{cmd}\n").as_bytes())?;
    let mut lines = vec![];
    loop {
        let l = recv(&mut r)?;
        let done = l.starts_with("OK") || l.starts_with("ERR") || l == "END";
        lines.push(l);
        if done {
            break;
        }
    }
    let last = lines.last().cloned().unwrap_or_default();
    Ok((last.starts_with("OK") || last == "END", lines.join("\n")))
}

//! The punch relay socket: one WebSocket from this server to the master
//! (`GET /v1/punch/listen/{id}?ts=..`, signed with the listing key), held open while the
//! server is listed and wants punches. The master sends one JSON message per accepted punch
//! request (hsmp_master_core::punch::PunchMsg); the server answers it with probes from the
//! game socket (nat::punch). A text "ping" every 45 s keeps it alive (on the Cloudflare
//! Worker it is answered without waking the Durable Object).

use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use hsmp_master_core::{auth, punch, SIG_HEADER};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tracing::{debug, info, warn};

const MIN_BACKOFF_S: u64 = 5;
const MAX_BACKOFF_S: u64 = 300;
/// A master without the relay (404) or that refused us is asked again this much later.
const REFUSED_RETRY_S: u64 = 900;

/// `https://m` -> `wss://m<path>`, `http://m` -> `ws://m<path>`.
pub fn ws_url(master: &str, path: &str) -> Option<String> {
    let m = master.trim().trim_end_matches('/');
    if let Some(r) = m.strip_prefix("https://") {
        Some(format!("wss://{r}{path}"))
    } else {
        m.strip_prefix("http://").map(|r| format!("ws://{r}{path}"))
    }
}

enum End {
    /// Connected, then lost: reconnect soon.
    Lost,
    /// Could not connect.
    Failed(String),
    /// The master refused (old master, unknown listing, bad signature).
    Refused(u16),
}

/// Hold the relay socket for `server_id` until the task is aborted.
pub async fn run(master: String, server_id: String, key: SigningKey, next_ts: Arc<dyn Fn() -> u64 + Send + Sync>, socket: Arc<UdpSocket>, v4_only: bool) {
    let mut backoff = MIN_BACKOFF_S;
    loop {
        let end = once(&master, &server_id, &key, next_ts(), &socket, v4_only).await;
        super::set_listening(false);
        let wait = match end {
            End::Lost => {
                backoff = MIN_BACKOFF_S;
                MIN_BACKOFF_S
            }
            End::Failed(why) => {
                debug!(error = %why, retry_s = backoff, "punch relay: connect failed");
                let w = backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF_S);
                w
            }
            End::Refused(code) => {
                info!(status = code, retry_s = REFUSED_RETRY_S, "punch relay: the server list refused the listen socket (it may predate NAT traversal)");
                REFUSED_RETRY_S
            }
        };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

async fn once(master: &str, id: &str, key: &SigningKey, ts: u64, socket: &Arc<UdpSocket>, v4_only: bool) -> End {
    let path = punch::listen_path(id, ts);
    let Some(url) = ws_url(master, &path) else { return End::Failed(format!("not an http(s) URL: {master}")) };
    let mut req = match url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return End::Failed(e.to_string()),
    };
    if let Ok(v) = auth::sign(key, "GET", &path, b"").parse() {
        req.headers_mut().insert(SIG_HEADER, v);
    }
    let (host, port) = match host_port(&url) {
        Some(x) => x,
        None => return End::Failed("bad URL".into()),
    };
    // The same family as the game socket (hsmp-master checks the listing's address).
    let addr: Option<SocketAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(it) => it.into_iter().find(|a| !v4_only || a.is_ipv4()),
        Err(e) => return End::Failed(format!("resolve {host}: {e}")),
    };
    let Some(addr) = addr else { return End::Failed(format!("{host} has no usable address")) };
    let tcp = match tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return End::Failed(e.to_string()),
        Err(_) => return End::Failed("connect timeout".into()),
    };
    let (mut ws, _) = match tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::client_async_tls_with_config(req, tcp, None, None)).await {
        Ok(Ok(x)) => x,
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) => return End::Refused(r.status().as_u16()),
        Ok(Err(e)) => return End::Failed(e.to_string()),
        Err(_) => return End::Failed("handshake timeout".into()),
    };
    info!(server_id = %id, "punch relay connected: joiners behind other NATs can reach this server through the list");
    super::set_listening(true);
    let mut ping = tokio::time::interval(Duration::from_secs(punch::PING_EVERY_S));
    ping.tick().await;
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            m = ws.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    last_rx = Instant::now();
                    if t == punch::PONG { continue; }
                    match serde_json::from_str::<punch::PunchMsg>(&t) {
                        Ok(p) if p.t == "punch" => match punch::parse_endpoint(&p.to) {
                            Some(to) => super::punch(socket, to, &p.nonce),
                            None => warn!(to = %p.to, "punch relay: bad endpoint"),
                        },
                        _ => debug!(len = t.len(), "punch relay: unknown message ignored"),
                    }
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => last_rx = Instant::now(),
                Some(Ok(Message::Close(_))) | None => return End::Lost,
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    debug!(error = %e, "punch relay: read error");
                    return End::Lost;
                }
            },
            _ = ping.tick() => {
                if last_rx.elapsed() > Duration::from_secs(punch::PING_EVERY_S * 2 + 15) {
                    debug!("punch relay: no answer to pings; reconnecting");
                    return End::Lost;
                }
                if ws.send(Message::Text(punch::PING.into())).await.is_err() {
                    return End::Lost;
                }
            }
        }
    }
}

/// Host and port of a ws:// / wss:// URL.
fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?']).next()?;
    let default = if scheme == "wss" { 443 } else { 80 };
    if let Some(v6) = auth.strip_prefix('[') {
        let (h, p) = v6.split_once(']')?;
        let port = p.strip_prefix(':').map(|p| p.parse().ok()).unwrap_or(Some(default))?;
        return Some((h.to_string(), port));
    }
    match auth.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((auth.to_string(), default)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(ws_url("https://master.example.dev/", "/v1/punch/listen/ab?ts=1").as_deref(), Some("wss://master.example.dev/v1/punch/listen/ab?ts=1"));
        assert_eq!(ws_url("http://127.0.0.1:7778", "/p").as_deref(), Some("ws://127.0.0.1:7778/p"));
        assert_eq!(ws_url("ftp://x", "/p"), None);
        assert_eq!(host_port("wss://m.example/p?x"), Some(("m.example".into(), 443)));
        assert_eq!(host_port("ws://127.0.0.1:7778/p"), Some(("127.0.0.1".into(), 7778)));
        assert_eq!(host_port("ws://[::1]:9/p"), Some(("::1".into(), 9)));
    }
}

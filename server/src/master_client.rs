//! Master-registry client for `hsmp-server`.
//!
//! When `HSMP_MASTER_URL` is set in the environment, the relay registers
//! itself and heartbeats every 10 s so it shows up in the in-game server
//! browser's `GET /v1/servers` list.
//!
//! Survives outages: a master that is down at startup, a transient HTTP
//! failure, a 429 or the master forgetting us (410 after an outage / master
//! restart) never permanently delists the server — we retry registration
//! with capped exponential backoff, forever. Heartbeats carry the live
//! player count and current arena.

use crate::server::ServerState;
use crate::server_info;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn};

const HEARTBEAT_INTERVAL_SECS: u64 = 10;
const HTTP_TIMEOUT_SECS: u64 = 5;
/// Master's per-IP register throttle is 60 s; back off past it on 429.
const THROTTLED_RETRY_SECS: u64 = 61;
const MAX_BACKOFF_SECS: u64 = 60;

#[derive(Debug, Serialize)]
struct RegisterReq<'a> {
    name: &'a str,
    host: &'a str, // server ignores and uses remote_addr
    port: u16,
    mode: &'a str,
    map: &'a str,
    players: u32,
    max_players: u32,
    proto_ver: u32,
    proto_min: u32,
    proto_max: u32,
    server_key: &'a str,
    pwd_protected: bool,
    region: &'a str,
    version: &'a str,
    nonce: String,
    hmac: String,
}

#[derive(Debug, Deserialize)]
struct RegisterResp {
    server_id: String,
    #[allow(dead_code)]
    ttl_s: u64,
    secret: String,
}

#[derive(Debug, Serialize)]
struct HeartbeatReq {
    players: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    map: Option<String>,
    /// The live mode label (best-of changes in the lobby); a master
    /// that predates the field ignores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    nonce: String,
    hmac: String,
}

fn gen_nonce() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn hmac_sig(secret: &str, host: &str, port: u16, name: &str, nonce: &str) -> String {
    let mut v = Vec::with_capacity(secret.len() + host.len() + name.len() + nonce.len() + 8);
    v.extend_from_slice(secret.as_bytes());
    v.extend_from_slice(host.as_bytes());
    v.extend_from_slice(port.to_string().as_bytes());
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(nonce.as_bytes());
    hex::encode(Sha256::digest(&v))
}

/// Next backoff delay: 5, 10, 20, 40, 60, 60, ...
fn next_backoff(prev: u64) -> u64 {
    if prev == 0 { 5 } else { (prev * 2).min(MAX_BACKOFF_SECS) }
}

enum RegisterOutcome {
    Ok { server_id: String, secret: String },
    Throttled,
    Failed(String),
}

async fn register_once(
    client: &reqwest::Client,
    master_url: &str,
    port: u16,
    state: &ServerState,
) -> RegisterOutcome {
    let a = server_info::advertised();
    let (players, map) = server_info::live(state);
    let mode = server_info::live_mode(state);
    let nonce = gen_nonce();
    let req = RegisterReq {
        name: &a.name,
        host: "",
        port,
        mode: &mode,
        map: &map,
        players,
        max_players: a.max_players,
        proto_ver: crate::proto::PROTOCOL_VERSION,
        proto_min: hsmp_net::net::VERSION_MIN as u32,
        proto_max: hsmp_net::net::VERSION_MAX as u32,
        server_key: &a.server_key,
        pwd_protected: a.password,
        region: &a.region,
        version: env!("CARGO_PKG_VERSION"),
        hmac: hmac_sig("", "", port, &a.name, &nonce),
        nonce,
    };
    let url = format!("{}/v1/register", master_url.trim_end_matches('/'));
    let resp = match client.post(&url).json(&req).send().await {
        Ok(r) => r,
        Err(e) => return RegisterOutcome::Failed(format!("send: {e}")),
    };
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return RegisterOutcome::Throttled;
    }
    if !resp.status().is_success() {
        let st = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return RegisterOutcome::Failed(format!("status {st}: {}", body.chars().take(120).collect::<String>()));
    }
    match resp.json::<RegisterResp>().await {
        Ok(b) => RegisterOutcome::Ok { server_id: b.server_id, secret: b.secret },
        Err(e) => RegisterOutcome::Failed(format!("parse: {e}")),
    }
}

/// Heartbeat until the master forgets us (returns) — transient errors are
/// logged and retried on the next tick.
async fn heartbeat_loop(
    client: &reqwest::Client,
    master_url: &str,
    port: u16,
    state: &ServerState,
    server_id: &str,
    secret: &str,
) {
    let url = format!("{}/v1/heartbeat/{}", master_url.trim_end_matches('/'), server_id);
    let name = server_info::advertised().name;
    let mut ticker = time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    ticker.tick().await; // skip the immediate first tick
    let mut fails = 0u32;
    loop {
        ticker.tick().await;
        let (players, map) = server_info::live(state);
        let nonce = gen_nonce();
        let mode = server_info::live_mode(state);
        let hb = HeartbeatReq {
            players,
            map: if map.is_empty() { None } else { Some(map) },
            mode: if mode.is_empty() { None } else { Some(mode) },
            hmac: hmac_sig(secret, "", port, &name, &nonce),
            nonce,
        };
        match client.post(&url).json(&hb).send().await {
            Ok(r) if r.status().is_success() => {
                if fails > 0 {
                    info!(after = fails, "master heartbeat recovered");
                }
                fails = 0;
            }
            Ok(r) if r.status() == reqwest::StatusCode::GONE
                || r.status() == reqwest::StatusCode::NOT_FOUND =>
            {
                warn!("master forgot us (expired / restarted); re-registering");
                return;
            }
            Ok(r) => {
                fails += 1;
                warn!(status = %r.status(), fails, "heartbeat non-success");
            }
            Err(e) => {
                fails += 1;
                warn!(error = %e, fails, "heartbeat error (master unreachable?)");
            }
        }
    }
}

/// Register + heartbeat forever.
pub async fn run(master_url: String, port: u16, state: Arc<ServerState>) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .context("reqwest client")
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "master client disabled");
            return;
        }
    };
    let mut backoff = 0u64;
    loop {
        match register_once(&client, &master_url, port, &state).await {
            RegisterOutcome::Ok { server_id, secret } => {
                info!(server_id = %server_id, master = %master_url, "registered with master");
                backoff = 0;
                heartbeat_loop(&client, &master_url, port, &state, &server_id, &secret).await;
                continue; // forgotten → re-register right away
            }
            RegisterOutcome::Throttled => {
                warn!(retry_s = THROTTLED_RETRY_SECS, "master register throttled (429)");
                time::sleep(Duration::from_secs(THROTTLED_RETRY_SECS)).await;
            }
            RegisterOutcome::Failed(why) => {
                backoff = next_backoff(backoff);
                warn!(error = %why, retry_s = backoff, url = %master_url, "master register failed");
                time::sleep(Duration::from_secs(backoff)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_capped() {
        let mut b = 0;
        let seq: Vec<u64> = (0..7).map(|_| { b = next_backoff(b); b }).collect();
        assert_eq!(seq, vec![5, 10, 20, 40, 60, 60, 60]);
    }

    #[test]
    fn register_body_carries_browser_fields() {
        let r = RegisterReq {
            name: "n", host: "", port: 7777, mode: "Free Fight", map: "Map_Arena_Pit",
            players: 2, max_players: 8, proto_ver: 5, proto_min: 5, proto_max: 5, server_key: "ab", pwd_protected: false,
            region: "EU", version: "0.1.0", nonce: "x".into(), hmac: "y".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        for k in ["name", "map", "mode", "players", "max_players", "pwd_protected", "region", "version", "proto_ver", "proto_min", "proto_max", "server_key"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }
}

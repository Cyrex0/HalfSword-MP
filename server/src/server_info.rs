//! What this `hsmp-server` advertises about itself — to the master registry
//! (master_client heartbeat) and to server-browser UDP queries (query.rs).

use crate::query::{self, QueryInfo, QueryLimiter};
use crate::server::ServerState;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use tokio::net::UdpSocket;
use tracing::debug;

#[derive(Debug, Clone, Default)]
pub struct Advertised {
    pub name: String,
    pub mode: String,
    /// Map from --map / HSMP_LOBBY_MAP; used until the lobby picks an arena.
    pub default_map: String,
    pub region: String,
    pub max_players: u32,
    pub password: bool,
    /// Hex X25519 public key of the server identity (v5 pinning).
    pub server_key: String,
    /// The content hash the server enforces (64 hex), or "" when it accepts any
    /// (`--allow-mismatched-content`).
    pub content_hash: String,
}

static ADVERTISED: OnceLock<Advertised> = OnceLock::new();
/// Query reply budget (per source, then server-wide): protects the recv loop from floods.
static QUERY_RL: OnceLock<Mutex<QueryLimiter>> = OnceLock::new();

pub fn init(a: Advertised) {
    let _ = ADVERTISED.set(a);
}

pub fn advertised() -> Advertised {
    ADVERTISED.get().cloned().unwrap_or_default()
}

/// Live snapshot: (players, current map). Never blocks — if the match lock
/// is busy we fall back to the advertised default map.
pub fn live(state: &ServerState) -> (u32, String) {
    let a = advertised();
    let players = state.net.addrs().len() as u32;
    let map = state
        .current_arena()
        .filter(|m| !m.is_empty() && m != "default")
        .unwrap_or(a.default_map);
    (players, map)
}

/// The listed mode follows the live best-of. The menu boots a hosted
/// server with its saved "Best of N" as HSMP_SERVER_MODE, but the match
/// starts at best_of 3 and the host may change it in the lobby, so a fixed
/// label would be wrong ("Best of 5" listed for a BO3 match). A "Best of N" label is
/// rewritten to the live value; any other mode name (duel, FFA, a custom
/// label) is kept as advertised.
pub fn mode_label(advertised: &str, best_of: Option<u8>) -> String {
    let is_rounds = advertised.trim().to_ascii_lowercase().starts_with("best of");
    match best_of {
        Some(n) if is_rounds && n > 0 => format!("Best of {n}"),
        _ => advertised.to_string(),
    }
}

/// `mode_label`, unless the lobby plays a game mode other than duel: then its name
/// ("Deathmatch"), whatever the boot label said.
pub fn mode_label_for(advertised: &str, best_of: Option<u8>, mode: Option<u8>) -> String {
    match mode {
        Some(m) if m != crate::proto::v5::Mode::DUEL && crate::server::parse_mode(advertised) != Some(m) =>
            crate::server::mode_label(m).to_string(),
        _ => mode_label(advertised, best_of),
    }
}

/// The mode label to advertise right now (see `mode_label_for`).
pub fn live_mode(state: &ServerState) -> String {
    mode_label_for(&advertised().mode, state.current_best_of(), state.current_mode())
}

pub fn snapshot(state: &ServerState) -> QueryInfo {
    let a = advertised();
    let (players, map) = live(state);
    QueryInfo {
        qver: query::QUERY_VERSION,
        name: a.name,
        map,
        mode: mode_label_for(&a.mode, state.current_best_of(), state.current_mode()),
        players,
        max_players: a.max_players,
        password: a.password,
        proto_ver: crate::proto::PROTOCOL_VERSION,
        build: env!("CARGO_PKG_VERSION").to_string(),
        region: a.region,
        proto_min: hsmp_net::net::VERSION_MIN as u32,
        proto_max: hsmp_net::net::VERSION_MAX as u32,
        server_key: a.server_key,
        content_tag: query::content_tag(&a.content_hash),
    }
}

/// Answer a browser query datagram. Silently drops malformed / unpadded
/// requests and anything over the rate budget.
pub async fn answer(socket: &UdpSocket, state: &ServerState, from: SocketAddr, data: &[u8]) {
    let Some(nonce) = query::parse_request(data) else {
        debug!(%from, len = data.len(), "dropped malformed browser query");
        return;
    };
    let ok = QUERY_RL
        .get_or_init(|| Mutex::new(QueryLimiter::new()))
        .lock()
        .map(|mut rl| rl.allow_at(from.ip(), std::time::Instant::now()))
        .unwrap_or(false);
    if !ok {
        return;
    }
    let reply = query::build_reply(nonce, &snapshot(state));
    let _ = socket.send_to(&reply, from).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn recv_loop_answers_browser_query() {
        init(Advertised {
            name: "Test Srv".into(), mode: "Free Fight".into(),
            default_map: "Map_Arena_Pit".into(), region: "EU".into(),
            max_players: 8, password: false, server_key: "cd".repeat(32), content_hash: "ef".repeat(32),
        });
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = sock.local_addr().unwrap();
        let state = Arc::new(ServerState::new(8));
        tokio::spawn(crate::server::recv_loop(sock, state));

        let c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        // Unpadded request: ignored (anti-amplification).
        c.send_to(&query::build_request(1)[..32], addr).await.unwrap();
        c.send_to(&query::build_request(42), addr).await.unwrap();
        let mut buf = [0u8; 2048];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), c.recv_from(&mut buf))
            .await.expect("no reply").unwrap();
        assert!(n <= query::REQ_LEN);
        let (nonce, info) = query::parse_reply(&buf[..n]).unwrap();
        assert_eq!(nonce, 42);
        assert_eq!(info.name, "Test Srv");
        assert_eq!(info.players, 0);
        assert_eq!(info.max_players, 8);
        // Lobby arena is still "default" → advertised map.
        assert_eq!(info.map, "Map_Arena_Pit");
        assert_eq!(info.proto_ver, crate::proto::PROTOCOL_VERSION);
        assert_eq!(info.content_tag, "efefefefefefefef");
    }

    #[test]
    fn mode_label_follows_the_live_best_of() {
        // A host booted with its saved "Best of 5" plays BO3 until the
        // lobby sets 5: the listing says what is actually played.
        assert_eq!(mode_label("Best of 5", Some(3)), "Best of 3");
        assert_eq!(mode_label("best of 3", Some(7)), "Best of 7");
        assert_eq!(mode_label("Best of 5", None), "Best of 5"); // lock busy: keep the label
        // other mode names are not rounds labels
        assert_eq!(mode_label("duel", Some(5)), "duel");
        assert_eq!(mode_label("Free Fight", Some(3)), "Free Fight");
        assert_eq!(mode_label("", Some(3)), "");
    }

    #[test]
    fn the_listing_names_the_game_mode() {
        use crate::proto::v5::Mode;
        assert_eq!(mode_label_for("Best of 5", Some(3), Some(Mode::DUEL)), "Best of 3");
        assert_eq!(mode_label_for("Best of 5", Some(3), Some(Mode::DEATHMATCH)), "Deathmatch");
        assert_eq!(mode_label_for("koth", Some(3), Some(Mode::KING_OF_HILL)), "koth", "the host's own label for it");
        assert_eq!(mode_label_for("duel", Some(3), Some(Mode::TEAM_ELIM)), "Team elimination");
        assert_eq!(mode_label_for("duel", None, None), "duel");
    }

    #[tokio::test]
    async fn live_mode_reads_the_server_best_of() {
        // a fresh server plays best of 3 whatever the boot label said
        let state = ServerState::new(8);
        assert_eq!(state.current_best_of(), Some(3));
        assert_eq!(mode_label("Best of 5", state.current_best_of()), "Best of 3");
    }
}

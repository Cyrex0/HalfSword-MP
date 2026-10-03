//! Admin: who is admin (`AdminPolicy`), banlist IO (load /
//! save / path), ban-list privacy (masked IPs) and the admin verbs
//! (kick / ban / unban / promote / best-of ...).
//!
//! Admin assignment:
//! * never "the first to join";
//! * listen server: the hosting player's Ed25519 key (`--owner-key`, or
//!   `--owner-key-file` = the host sidecar's `<state>/.player_key`) is the
//!   OWNER whenever it is connected, also after a drop and reconnect; nobody
//!   else ever inherits admin automatically;
//! * dedicated server: no admin unless configured: `--admin-key <hex>`,
//!   `--admins-file` (one key per line, re-read when it changes), RCON
//!   `ADMIN ADD`, or a grant by a connected admin (`Promote`);
//! * no admin connected: ready players start the match themselves
//!   (`session::auto_start_step`).
//!
//! See docs/development/server-modules.md.

use super::*;
use session::PlayerKey;

// =============================================================================
// Admin policy
// =============================================================================

/// Where an admin key came from (RCON `ADMIN LIST`).
fn source_name(p: &AdminPolicy, k: &PlayerKey) -> &'static str {
    if p.owner == Some(*k) { "owner" }
    else if p.cli.contains(k) { "config" }
    else if p.file.contains(k) { "file" }
    else { "runtime" }
}

/// Who may administer this server. Roles are keyed by the v5 handshake's
/// verified player key, never by join order, address or nick.
#[derive(Debug)]
pub(crate) struct AdminPolicy {
    /// The listen host (OWNER). Fixed once that player has joined.
    pub owner: Option<PlayerKey>,
    owner_file: Option<PathBuf>,
    owner_pinned: bool,
    /// `--admin-key` (repeatable).
    cli: HashSet<PlayerKey>,
    /// `--admins-file` contents (re-read when its size / mtime change).
    file: HashSet<PlayerKey>,
    file_path: Option<PathBuf>,
    file_stamp: Option<(std::time::SystemTime, u64)>,
    /// RCON `ADMIN ADD` without an admins file, and in-game grants (`Promote`).
    runtime: HashSet<PlayerKey>,
    /// Per-run salt of the masked ban tokens (`ban_token`).
    salt: [u8; 16],
}

impl Default for AdminPolicy {
    fn default() -> Self {
        let mut salt = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut salt);
        AdminPolicy {
            owner: None, owner_file: None, owner_pinned: false,
            cli: HashSet::new(), file: HashSet::new(), file_path: None, file_stamp: None,
            runtime: HashSet::new(), salt,
        }
    }
}

impl AdminPolicy {
    /// The role of `key`: OWNER (listen host), ADMIN (configured / granted) or NONE.
    pub fn role(&self, key: &PlayerKey) -> u8 {
        if *key == [0u8; 32] { return v5::AdminRole::NONE; }
        if self.owner == Some(*key) { v5::AdminRole::OWNER }
        else if self.cli.contains(key) || self.file.contains(key) || self.runtime.contains(key) { v5::AdminRole::ADMIN }
        else { v5::AdminRole::NONE }
    }
    pub fn is_admin(&self, key: &PlayerKey) -> bool { self.role(key) >= v5::AdminRole::ADMIN }
    /// A listen server's owner is configured (or expected from its key file).
    pub fn has_owner(&self) -> bool { self.owner.is_some() || self.owner_file.is_some() }
    /// Grant admin at run time (in-game Promote, RCON ADD without a file).
    pub fn grant(&mut self, key: PlayerKey) { if self.role(&key) == v5::AdminRole::NONE { self.runtime.insert(key); } }
    /// Every admin key with its source (owner first).
    pub fn all(&self) -> Vec<(PlayerKey, &'static str)> {
        let mut v: Vec<PlayerKey> = self.owner.iter().copied()
            .chain(self.cli.iter().copied()).chain(self.file.iter().copied()).chain(self.runtime.iter().copied())
            .collect();
        v.dedup();
        let mut seen = HashSet::new();
        v.into_iter().filter(|k| seen.insert(*k)).map(|k| (k, source_name(self, &k))).collect()
    }
}

/// Startup admin configuration (main.rs flags).
#[derive(Debug, Clone, Default)]
pub struct AdminOpts {
    pub owner_key: Option<String>,
    pub owner_key_file: Option<PathBuf>,
    pub admin_keys: Vec<String>,
    pub admins_file: Option<PathBuf>,
}

/// A player key: exactly 64 hex characters.
pub(crate) fn parse_key_hex(s: &str) -> Option<PlayerKey> {
    let s = s.trim();
    if s.len() != 64 { return None; }
    let b = hex::decode(s).ok()?;
    <PlayerKey>::try_from(b.as_slice()).ok().filter(|k| *k != [0u8; 32])
}

/// An admins file: one 64-hex player key per line (the first token; `#`
/// comments and blank lines ignored). Returns the keys and the bad lines.
pub(crate) fn parse_admins_text(text: &str) -> (HashSet<PlayerKey>, Vec<String>) {
    let mut keys = HashSet::new();
    let mut bad = Vec::new();
    for line in text.lines() {
        let s = line.split('#').next().unwrap_or("").trim();
        let Some(tok) = s.split_whitespace().next() else { continue };
        match parse_key_hex(tok) { Some(k) => { keys.insert(k); } None => bad.push(tok.chars().take(80).collect()) }
    }
    (keys, bad)
}

fn file_stamp(p: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Apply the startup configuration (bad keys are a startup error: an
/// operator who typed a key wrong must not silently run without an admin).
pub async fn configure_admins(state: &Arc<ServerState>, o: AdminOpts) -> anyhow::Result<()> {
    let owner = match o.owner_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => Some(parse_key_hex(h).ok_or_else(|| anyhow::anyhow!("--owner-key: expected 64 hex chars (a player key)"))?),
        None => None,
    };
    let mut cli = HashSet::new();
    for h in &o.admin_keys {
        for part in h.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            cli.insert(parse_key_hex(part).ok_or_else(|| anyhow::anyhow!("--admin-key {part}: expected 64 hex chars (a player key)"))?);
        }
    }
    {
        let mut inner = state.inner.lock().await;
        let a = &mut inner.admins;
        a.owner = owner;
        a.owner_pinned = false;
        a.owner_file = o.owner_key_file.clone();
        a.cli = cli;
        a.file_path = o.admins_file.clone();
    }
    reload_admin_files(state).await;
    let inner = state.inner.lock().await;
    let a = &inner.admins;
    info!(owner = ?a.owner.map(|k| session::player_id_hex(&k)), owner_file = ?a.owner_file,
          admins = a.cli.len() + a.file.len(), admins_file = ?a.file_path,
          "admin policy: {}", if a.has_owner() { "listen host is the owner" }
              else if a.cli.is_empty() && a.file.is_empty() && a.file_path.is_none() { "no admin (players start matches by READY)" }
              else { "configured admins only" });
    Ok(())
}

/// Re-read the owner key file (until the owner has joined) and the admins
/// file (when it changed). File IO runs outside the game lock. Called at
/// startup and before every admission.
pub(crate) async fn reload_admin_files(state: &Arc<ServerState>) -> bool {
    let (owner_file, admins_file, stamp) = {
        let inner = state.inner.lock().await;
        let a = &inner.admins;
        (a.owner_file.clone().filter(|_| !a.owner_pinned), a.file_path.clone(), a.file_stamp)
    };
    let owner = owner_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| t.split_whitespace().next().and_then(parse_key_hex));
    let admins = admins_file.as_ref().and_then(|p| {
        let st = file_stamp(p);
        if st.is_some() && st == stamp { return None; }
        match std::fs::read_to_string(p) {
            Ok(t) => Some((parse_admins_text(&t), st)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(((HashSet::new(), Vec::new()), None)),
            Err(e) => { warn!(path = ?p, error = %e, "admins file unreadable; keeping the previous list"); None }
        }
    });
    let mut inner = state.inner.lock().await;
    let mut changed = false;
    if let Some(k) = owner {
        if !inner.admins.owner_pinned && inner.admins.owner != Some(k) {
            info!(owner = %session::player_id_hex(&k), "listen host key read from the owner key file");
            inner.admins.owner = Some(k);
            changed = true;
        }
    }
    if let Some(((keys, bad), st)) = admins {
        for b in &bad { warn!(line = %b, "admins file: not a 64-hex player key; skipped"); }
        if keys != inner.admins.file {
            info!(count = keys.len(), "admins file loaded");
            inner.admins.file = keys;
            changed = true;
        }
        inner.admins.file_stamp = st;
    }
    changed && refresh_admins(&mut inner)
}

/// Recompute every peer's admin flag and `admin_peer_id` (the lowest-id
/// connected admin, 0 = none) from the policy. Pins the owner once it is
/// here. True when the set of admins changed. Never promotes anyone the
/// policy does not name: an admin leaving leaves no admin behind.
pub(crate) fn refresh_admins(inner: &mut Inner) -> bool {
    let mut changed = false;
    let mut primary: Option<PeerId> = None;
    let owner = inner.admins.owner;
    let mut owner_here = false;
    let roles: Vec<(SocketAddr, bool, PeerId)> = inner.peers.iter().map(|(a, p)| {
        let k = session::peer_key(p);
        if owner == Some(k) { owner_here = true; }
        (*a, inner.admins.is_admin(&k), p.id)
    }).collect();
    for (a, adm, id) in roles {
        if let Some(p) = inner.peers.get_mut(&a) {
            if p.is_admin != adm { p.is_admin = adm; changed = true; }
        }
        if adm { primary = Some(primary.map_or(id, |x: PeerId| x.min(id))); }
    }
    if owner_here { inner.admins.owner_pinned = true; }
    let primary = primary.unwrap_or(0);
    if inner.admin_peer_id != primary { inner.admin_peer_id = primary; changed = true; }
    if changed {
        inner.match_state_dirty = true;
        let ids: Vec<PeerId> = inner.peers.values().filter(|p| p.is_admin).map(|p| p.id).collect();
        crate::events::emit("admin_changed", serde_json::json!({"admin_peer_id": primary, "admins": ids}));
    }
    changed
}

/// The S2CAdminState one peer gets (wire unchanged, semantics per
/// recipient): an admin sees its OWN id as `admin_peer_id` (so every admin's
/// client shows the admin tools: v5 sidecars set `is_admin = (my id ==
/// admin_peer_id)`) and the ban list with masked IPs; everyone else sees the
/// lowest-id connected admin (0 = none) and no ban list at all.
pub(crate) fn admin_state_for(inner: &Inner, to: SocketAddr) -> Vec<u8> {
    match inner.peers.get(&to).filter(|p| p.is_admin) {
        Some(p) => admin_state_msg(p.id, &masked_bans(inner)),
        None => admin_state_msg(inner.admin_peer_id, &[]),
    }
}

/// Short per-run token of a banned IP (admins un-ban by it, never see the IP).
pub(crate) fn ban_token(salt: &[u8; 16], ip: &IpAddr) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt);
    h.update(ip.to_string().as_bytes());
    hex::encode(&h.finalize()[..4])
}

/// A banned IP as an in-game admin sees it: a partial prefix plus the token,
/// e.g. `203.0.x.x#1a2b3c4d`, `2001:db8:x#1a2b3c4d`. The full IP is only
/// available over RCON (`BANS`).
pub(crate) fn mask_ip(salt: &[u8; 16], ip: &IpAddr) -> String {
    let prefix = match ip {
        IpAddr::V4(v) => { let o = v.octets(); format!("{}.{}.x.x", o[0], o[1]) }
        IpAddr::V6(v) => { let s = v.segments(); format!("{:x}:{:x}:x", s[0], s[1]) }
    };
    format!("{}#{}", prefix, ban_token(salt, ip))
}

fn masked_bans(inner: &Inner) -> Vec<String> {
    let mut v: Vec<String> = inner.banned_ips.iter().take(256).map(|ip| mask_ip(&inner.admins.salt, ip)).collect();
    v.sort();
    v
}

/// The banned IP an `unban:` argument names: the IP itself, or a masked
/// entry / its `#token`.
pub(crate) fn resolve_ban(inner: &Inner, arg: &str) -> Option<IpAddr> {
    let arg = arg.trim();
    if let Ok(ip) = arg.parse::<IpAddr>() { return Some(ip); }
    let tok = arg.rsplit('#').next().unwrap_or("").trim().to_ascii_lowercase();
    if tok.len() != 8 { return None; }
    inner.banned_ips.iter().copied().find(|ip| ban_token(&inner.admins.salt, ip) == tok)
}

fn admin_ids(inner: &Inner) -> Vec<PeerId> {
    let mut v: Vec<PeerId> = inner.peers.values().filter(|p| p.is_admin).map(|p| p.id).collect();
    v.sort_unstable();
    v
}

/// RCON `ADMIN ADD|REMOVE <peer_id|player_id|key>` / `ADMIN LIST`.
pub(crate) async fn rcon_admin(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, args: &str) -> Result<String, String> {
    let mut it = args.split_whitespace();
    let sub = it.next().unwrap_or("").to_ascii_uppercase();
    let target = it.next().unwrap_or("");
    if sub == "LIST" {
        let inner = state.inner.lock().await;
        let online: HashMap<PlayerKey, (PeerId, String)> = inner.peers.values()
            .map(|p| (session::peer_key(p), (p.id, p.nick.clone()))).collect();
        let mut out = String::new();
        for (k, src) in inner.admins.all() {
            let on = online.get(&k).map(|(id, n)| format!("online peer {} {}", id, n)).unwrap_or_else(|| "offline".into());
            out.push_str(&format!("{} {} {}\n", hex::encode(k), src, on));
        }
        out.push_str("END");
        return Ok(out);
    }
    if !(sub == "ADD" || sub == "REMOVE") || target.is_empty() {
        return Err("usage: ADMIN ADD|REMOVE <peer_id|player_id|key> | ADMIN LIST".into());
    }
    // The key: a 64-hex key, a connected peer's 16-hex player id, or its peer id.
    let (key, file) = {
        let inner = state.inner.lock().await;
        let key = parse_key_hex(target).or_else(|| {
            let t = target.to_ascii_lowercase();
            inner.peers.values().find(|p| {
                (t.len() == 16 && session::player_id_hex(&session::peer_key(p)) == t)
                    || target.parse::<PeerId>().map_or(false, |id| id == p.id)
            }).map(session::peer_key)
        });
        (key, inner.admins.file_path.clone())
    };
    let Some(key) = key else { return Err(format!("no such player: {}", target)) };
    let hexk = hex::encode(key);
    let admins_before = admin_ids(&*state.inner.lock().await);
    if sub == "ADD" {
        match &file {
            // Persisted: appended to the admins file, which is then re-read.
            Some(p) => {
                use std::io::Write;
                let have = std::fs::read_to_string(p).map(|t| parse_admins_text(&t).0.contains(&key)).unwrap_or(false);
                if !have {
                    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)
                        .map_err(|e| format!("admins file {}: {}", p.display(), e))?;
                    writeln!(f, "{}  # added by RCON", hexk).map_err(|e| format!("admins file: {}", e))?;
                }
                { state.inner.lock().await.admins.file_stamp = None; }
                reload_admin_files(state).await;
            }
            None => { state.inner.lock().await.admins.runtime.insert(key); }
        }
    } else {
        if state.inner.lock().await.admins.owner == Some(key) { return Err("the listen host is always admin".into()); }
        if let Some(p) = &file {
            if let Ok(t) = std::fs::read_to_string(p) {
                let kept: Vec<&str> = t.lines().filter(|l| {
                    l.split('#').next().unwrap_or("").split_whitespace().next().and_then(parse_key_hex) != Some(key)
                }).collect();
                let _ = std::fs::write(p, format!("{}\n", kept.join("\n")));
            }
        }
        let mut inner = state.inner.lock().await;
        inner.admins.cli.remove(&key);
        inner.admins.file.remove(&key);
        inner.admins.runtime.remove(&key);
        inner.admins.file_stamp = None;
    }
    // (reload_admin_files may already have refreshed: compare with before.)
    let changed = {
        let mut inner = state.inner.lock().await;
        refresh_admins(&mut inner);
        admin_ids(&inner) != admins_before
    };
    if changed {
        let addrs = { state.inner.lock().await.peers.keys().copied().collect::<Vec<_>>() };
        broadcast_admin_state(socket, state, &addrs).await;
        flush_out(socket, state).await;
    }
    info!(sub = %sub, player = %session::player_id_hex(&key), "rcon admin change");
    Ok(format!("{} {}", if sub == "ADD" { "admin" } else { "not admin" }, session::player_id_hex(&key)))
}

/// Load `path` → `banned_ips`. Blank lines and `#` comments ignored.
pub async fn load_banlist(state: &Arc<ServerState>, path: &std::path::Path) -> anyhow::Result<()> {
    let data = match tokio::fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(?path, "banlist file not found; starting empty");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let mut inner = state.inner.lock().await;
    let mut n = 0;
    for line in data.lines() {
        let s = line.split('#').next().unwrap_or("").trim();
        if s.is_empty() { continue; }
        if let Ok(ip) = s.parse::<IpAddr>() {
            inner.banned_ips.insert(ip);
            n += 1;
        } else {
            warn!(line = %s, "skipping invalid IP in banlist");
        }
    }
    info!(count = n, ?path, "loaded banlist");
    Ok(())
}

/// Point the server at the banlist file; subsequent changes to `banned_ips`
/// write through to it.
pub async fn set_banlist_path(state: &Arc<ServerState>, path: PathBuf) {
    let mut inner = state.inner.lock().await;
    inner.banlist_path = Some(path);
}

/// Save the current banned_ips to banlist_path if configured. Takes the inner
/// lock briefly to snapshot the set.
pub(crate) async fn save_banlist(state: &Arc<ServerState>) {
    let (path, snapshot) = {
        let inner = state.inner.lock().await;
        let Some(p) = inner.banlist_path.clone() else { return };
        (p, inner.banned_ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>())
    };
    let header = "# hsmp-server banlist — one IP per line\n";
    let body = snapshot.join("\n");
    let data = format!("{}{}\n", header, body);
    // Atomic write: .tmp + rename.
    let tmp = path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp, data.as_bytes()).await {
        warn!(?path, error = %e, "banlist tmp write failed");
        return;
    }
    let _ = tokio::fs::remove_file(&path).await;
    if let Err(e) = tokio::fs::rename(&tmp, &path).await {
        warn!(?path, error = %e, "banlist rename failed");
    }
}

/// Kick (or ban + kick) one connected peer: ban persisted, S2CKicked to the
/// target, removal (admin hand-over), a public chat line, admin state.
/// The effect of `Command::Kick/Ban` from every path (typed, legacy, RCON).
pub(super) async fn kick_peer(socket: &UdpSocket, state: &Arc<ServerState>, target_addr: SocketAddr,
                              banned_this: bool, reason: &str, by: &str) {
    let target_nick = {
        let inner = state.inner.lock().await;
        match inner.peers.get(&target_addr) { Some(p) => p.nick.clone(), None => return }
    };
    if banned_this {
        {
            let mut inner = state.inner.lock().await;
            inner.banned_ips.insert(target_addr.ip());
        }
        info!(ip = %target_addr.ip(), target = %target_nick, "ip banned by {}", by);
        save_banlist(state).await;
    }
    // v5: Event::Kicked + KICKED close (terminal for the client), then remove.
    kick(socket, state, target_addr, reason, 0).await;
    let _ = peer_leave(socket, state, target_addr).await;
    // Public chat.
    let addrs = { state.inner.lock().await.peers.keys().copied().collect::<Vec<_>>() };
    let text = format!("{} {} by {}", target_nick, if banned_this {"banned"} else {"kicked"}, by);
    broadcast_msg(socket, state, &addrs, server_chat_msg(&text)).await;
    broadcast_admin_state(socket, state, &addrs).await;
}


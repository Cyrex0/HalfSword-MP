//! HSMP public server list on Cloudflare Workers.
//!
//! The Worker terminates HTTPS, applies the CORS / Origin policy, takes the client address from
//! `CF-Connecting-IP` and forwards writes to one SQLite-backed Durable Object (`Registry`),
//! which runs the shared registry state machine (crates/hsmp-master-core) and persists its
//! effects. Same API as hsmp-master: docs/hosting/master-server.md.

use hsmp_master_core::{dashboard, discord, Config, Effect, Entry, Listing, Registry as Core, Reply, MAX_BODY_BYTES, SIG_HEADER};
use hsmp_master_core::registry::{AnnounceState, Announcement};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use worker::*;

/// Client address, set by the Worker on requests to the Durable Object (which is not
/// reachable from outside, so the header cannot be forged).
const IP_HEADER: &str = "x-hsmp-client-ip";
const DO_NAME: &str = "global";
/// The list is served from isolate memory for this long before the Durable Object is asked again.
const LIST_CACHE_MS: u64 = 5_000;
/// `GET` requests per client address and isolate: burst and refill.
const GET_BURST: f64 = 30.0;
const GET_EVERY_MS: f64 = 2_000.0;

fn now_ms() -> u64 {
    Date::now().as_millis()
}

fn loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// The request's source address. Cloudflare sets `CF-Connecting-IP` and overwrites any value
/// a client sends; only `wrangler dev` on loopback may lack it.
fn client_ip(req: &Request, url: &Url) -> Option<IpAddr> {
    match req.headers().get("cf-connecting-ip").ok().flatten() {
        Some(v) => v.trim().parse().ok(),
        None if loopback(url) => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        None => None,
    }
}

fn secure(mut r: Response, cache: &str) -> Result<Response> {
    let h = r.headers_mut();
    h.set("strict-transport-security", "max-age=31536000")?;
    h.set("x-content-type-options", "nosniff")?;
    h.set("cache-control", cache)?;
    Ok(r)
}

fn text(status: u16, msg: &str) -> Result<Response> {
    secure(Response::ok(msg)?.with_status(status), "no-store")
}

thread_local! {
    static LIST_CACHE: RefCell<Option<(u64, String)>> = const { RefCell::new(None) };
    static GET_RATE: RefCell<HashMap<IpAddr, (f64, u64)>> = RefCell::new(HashMap::new());
}

/// Approximate per-address limit for reads (per isolate; writes are limited exactly in the
/// Durable Object).
fn get_allowed(ip: IpAddr, now: u64) -> bool {
    GET_RATE.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 20_000 {
            m.clear();
        }
        let key = hsmp_master_core::ip::host_key(ip);
        let (tokens, at) = m.entry(key).or_insert((GET_BURST, now));
        *tokens = (*tokens + now.saturating_sub(*at) as f64 / GET_EVERY_MS).min(GET_BURST);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    })
}

fn registry_stub(env: &Env) -> Result<Stub> {
    env.durable_object("REGISTRY")?.id_from_name(DO_NAME)?.get_stub()
}

async fn list_json(env: &Env) -> Result<String> {
    let now = now_ms();
    if let Some(body) = LIST_CACHE.with(|c| c.borrow().as_ref().filter(|(t, _)| now.saturating_sub(*t) < LIST_CACHE_MS).map(|(_, b)| b.clone())) {
        return Ok(body);
    }
    let mut r = registry_stub(env)?.fetch_with_str("https://registry/list").await?;
    let body = r.text().await?;
    if r.status_code() == 200 {
        LIST_CACHE.with(|c| *c.borrow_mut() = Some((now, body.clone())));
    }
    Ok(body)
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let method = req.method();
    let path = url.path().to_string();

    // HTTPS only. Reads are redirected; writes over plain HTTP are refused, never followed.
    if url.scheme() != "https" && !loopback(&url) {
        if method == Method::Get || method == Method::Head {
            let mut to = url.clone();
            let _ = to.set_scheme("https");
            return Response::redirect_with_status(to, 301);
        }
        return text(403, "https required");
    }
    // No CORS: the game does not use a browser. Preflights get no Allow headers, and a write
    // that carries an Origin (a web page) is refused outright.
    if method == Method::Options {
        return secure(Response::empty()?.with_status(204), "no-store");
    }
    let write = matches!(method, Method::Post | Method::Delete);
    if write && req.headers().get("origin")?.is_some() {
        return text(403, "browser requests are not accepted");
    }
    let Some(ip) = client_ip(&req, &url) else {
        return text(400, "no client address");
    };

    match (method.clone(), path.as_str()) {
        (Method::Get, "/v1/health") => text(200, "ok"),
        (Method::Get, "/v1/myaddr") => {
            let ip = hsmp_master_core::ip::canonical(ip).to_string();
            secure(Response::from_json(&serde_json::json!({ "ip": ip, "port": 0, "addr": ip }))?, "no-store")
        }
        (Method::Get, "/v1/servers") | (Method::Get, "/") | (Method::Get, "/dashboard") => {
            if !get_allowed(ip, now_ms()) {
                return text(429, "slow down");
            }
            let body = list_json(&env).await?;
            if path == "/v1/servers" {
                let mut r = Response::ok(body)?;
                r.headers_mut().set("content-type", "application/json")?;
                secure(r, "public, max-age=5")
            } else {
                let entries: Vec<Entry> = serde_json::from_str(&body).unwrap_or_default();
                let mut r = Response::from_html(dashboard::render(&entries))?;
                r.headers_mut().set("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'")?;
                secure(r, "public, max-age=5")
            }
        }
        (Method::Post, "/v1/register") | (Method::Post, _) | (Method::Delete, _)
            if path == "/v1/register" || path.starts_with("/v1/heartbeat/") || path.starts_with("/v1/servers/") =>
        {
            if (method == Method::Post) == path.starts_with("/v1/servers/") {
                return text(405, "method not allowed");
            }
            let len: Option<usize> = req.headers().get("content-length")?.and_then(|v| v.trim().parse().ok());
            match len {
                None => return text(411, "content-length required"),
                Some(n) if n > MAX_BODY_BYTES => return text(413, "body too large"),
                _ => {}
            }
            let body = req.bytes().await?;
            if body.len() > MAX_BODY_BYTES {
                return text(413, "body too large");
            }
            let h = Headers::new();
            h.set(IP_HEADER, &ip.to_string())?;
            if let Some(sig) = req.headers().get(SIG_HEADER)? {
                h.set(SIG_HEADER, &sig)?;
            }
            let mut init = RequestInit::new();
            init.with_method(method).with_headers(h).with_body(Some(worker::js_sys::Uint8Array::from(&body[..]).into()));
            let inner = Request::new_with_init(&format!("https://registry{path}"), &init)?;
            // A fetched response's headers are immutable: copy it into a new one.
            let mut r = registry_stub(&env)?.fetch_with_request(inner).await?;
            let status = r.status_code();
            if (200..300).contains(&status) {
                // this isolate shows the change at once (others within LIST_CACHE_MS)
                LIST_CACHE.with(|c| *c.borrow_mut() = None);
            }
            if status == 204 {
                return secure(Response::empty()?.with_status(204), "no-store");
            }
            let ctype = r.headers().get("content-type")?.unwrap_or_else(|| "text/plain; charset=utf-8".into());
            let mut out = Response::ok(r.text().await?)?.with_status(status);
            out.headers_mut().set("content-type", &ctype)?;
            secure(out, "no-store")
        }
        _ => text(404, "not found"),
    }
}

#[derive(Deserialize)]
struct DataRow {
    data: String,
}

/// The one Durable Object that holds the list.
#[durable_object]
pub struct Registry {
    state: State,
    env: Env,
    core: RefCell<Option<Core>>,
    /// Whether a sweep alarm is known to be pending (None = not checked since wake).
    alarm_pending: Cell<Option<bool>>,
}

impl Registry {
    fn var_u64(&self, name: &str, default: u64) -> u64 {
        self.env.var(name).ok().and_then(|v| v.to_string().trim().parse().ok()).unwrap_or(default)
    }

    fn webhook(&self) -> Option<String> {
        let u = self.env.secret("DISCORD_WEBHOOK_URL").ok()?.to_string();
        let u = u.trim().to_string();
        if discord::webhook_url_ok(&u) {
            Some(u)
        } else {
            if !u.is_empty() {
                console_error!("DISCORD_WEBHOOK_URL is not a Discord webhook URL; announcements off");
            }
            None
        }
    }

    /// Create the tables and load the list on the first request after a wake.
    fn load(&self, now: u64) -> Result<()> {
        if self.core.borrow().is_some() {
            return Ok(());
        }
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS servers (server_id TEXT PRIMARY KEY, data TEXT NOT NULL)", None)?;
        sql.exec("CREATE TABLE IF NOT EXISTS announce (key TEXT PRIMARY KEY, data TEXT NOT NULL)", None)?;
        let rows: Vec<Listing> = sql
            .exec("SELECT data FROM servers", None)?
            .to_array::<DataRow>()?
            .into_iter()
            .filter_map(|r| serde_json::from_str(&r.data).ok())
            .collect();
        let ann: Vec<AnnounceState> = sql
            .exec("SELECT data FROM announce", None)?
            .to_array::<DataRow>()?
            .into_iter()
            .filter_map(|r| serde_json::from_str(&r.data).ok())
            .collect();
        let cfg = Config::public(self.var_u64("HEARTBEAT_S", 120), self.var_u64("TTL_S", 360), self.webhook().is_some());
        *self.core.borrow_mut() = Some(Core::new(cfg, rows, ann, now));
        Ok(())
    }

    /// Persist the effects, keep a sweep alarm pending while anything is listed, and post
    /// announcements.
    async fn apply(&self, fx: Vec<Effect>) -> Result<()> {
        let sql = self.state.storage().sql();
        let mut news: Vec<Announcement> = Vec::new();
        for e in fx {
            match e {
                Effect::Put(l) => {
                    sql.exec(
                        "INSERT INTO servers (server_id, data) VALUES (?, ?) ON CONFLICT(server_id) DO UPDATE SET data = excluded.data",
                        vec![l.server_id.clone().into(), serde_json::to_string(&l)?.into()],
                    )?;
                }
                Effect::Remove(id) => {
                    sql.exec("DELETE FROM servers WHERE server_id = ?", vec![id.into()])?;
                }
                Effect::PutAnnounce(a) => {
                    sql.exec(
                        "INSERT INTO announce (key, data) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET data = excluded.data",
                        vec![a.key.clone().into(), serde_json::to_string(&a)?.into()],
                    )?;
                }
                Effect::RemoveAnnounce(k) => {
                    sql.exec("DELETE FROM announce WHERE key = ?", vec![k.into()])?;
                }
                Effect::Announce(a) => news.push(a),
            }
        }
        let next = self.core.borrow().as_ref().and_then(|c| c.next_expiry_ms());
        if let Some(at) = next {
            let pending = match self.alarm_pending.get() {
                Some(p) => p,
                None => self.state.storage().get_alarm().await?.is_some(),
            };
            if !pending {
                self.state.storage().set_alarm(std::time::Duration::from_millis(at.saturating_sub(now_ms()).max(1_000))).await?;
            }
            self.alarm_pending.set(Some(true));
        }
        if !news.is_empty() {
            if let Some(url) = self.webhook() {
                let show = self.env.var("DISCORD_SHOW_ADDRESS").map(|v| v.to_string() != "0").unwrap_or(true);
                for a in news {
                    let (url, body) = (url.clone(), discord::webhook_body(&a, show));
                    self.state.wait_until(async move {
                        if let Err(e) = post_webhook(&url, body).await {
                            console_error!("discord webhook: {e}");
                        }
                    });
                }
            }
        }
        Ok(())
    }

    fn run<F: FnOnce(&mut Core) -> (Reply, Vec<Effect>)>(&self, f: F) -> (Reply, Vec<Effect>) {
        let mut c = self.core.borrow_mut();
        f(c.as_mut().expect("loaded"))
    }
}

async fn post_webhook(url: &str, body: String) -> Result<()> {
    let h = Headers::new();
    h.set("content-type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(h).with_body(Some(body.into()));
    let r = Fetch::Request(Request::new_with_init(url, &init)?).send().await?;
    if r.status_code() >= 300 {
        return Err(Error::RustError(format!("status {}", r.status_code())));
    }
    Ok(())
}

fn reply(r: Reply) -> Result<Response> {
    if r.status == 204 {
        return Ok(Response::empty()?.with_status(204));
    }
    let mut out = Response::ok(r.body)?.with_status(r.status);
    out.headers_mut().set("content-type", if r.json { "application/json" } else { "text/plain; charset=utf-8" })?;
    Ok(out)
}

impl DurableObject for Registry {
    fn new(state: State, env: Env) -> Self {
        Self { state, env, core: RefCell::new(None), alarm_pending: Cell::new(None) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let now = now_ms();
        self.load(now)?;
        let path = req.path();
        if req.method() == Method::Get && path == "/list" {
            let list = self.core.borrow().as_ref().map(|c| c.list(now)).unwrap_or_default();
            return Response::from_json(&list);
        }
        let ip: IpAddr = match req.headers().get(IP_HEADER)?.and_then(|v| v.parse().ok()) {
            Some(ip) => ip,
            None => return Response::error("no client address", 400),
        };
        let sig = req.headers().get(SIG_HEADER)?;
        let body = req.bytes().await?;
        let sig = sig.as_deref();
        let (r, fx) = match (req.method(), path.as_str()) {
            (Method::Post, "/v1/register") => self.run(|c| c.register(now, ip, &body, sig)),
            (Method::Post, p) if p.starts_with("/v1/heartbeat/") => {
                let id = p.trim_start_matches("/v1/heartbeat/").to_string();
                self.run(|c| c.heartbeat(now, ip, &id, &body, sig))
            }
            (Method::Delete, p) if p.starts_with("/v1/servers/") => {
                let id = p.trim_start_matches("/v1/servers/").to_string();
                self.run(|c| c.delete(now, &id, &body, sig))
            }
            _ => return Response::error("not found", 404),
        };
        self.apply(fx).await?;
        reply(r)
    }

    async fn alarm(&self) -> Result<Response> {
        let now = now_ms();
        self.load(now)?;
        self.alarm_pending.set(Some(false));
        let fx = self.core.borrow_mut().as_mut().map(|c| c.sweep(now)).unwrap_or_default();
        self.apply(fx).await?;
        Response::ok("swept")
    }
}

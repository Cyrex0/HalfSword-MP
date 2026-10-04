//! Bug-report uploads from the launcher, stored in R2 (binding `REPORTS`).
//!
//! | route | who | does |
//! |---|---|---|
//! | `POST /v1/reports` | the launcher | checks the zip (hsmp_master_core::reports::check), stores it as `r/<day>/<hex>.zip`, returns `{"id": "<day>-<hex>"}` |
//! | `GET /v1/reports/admin[?day=YYYYMMDD][&cursor=..]` | us (Bearer `REPORTS_ADMIN_TOKEN`) | lists reports |
//! | `GET /v1/reports/admin/<id>` | us | downloads one |
//! | `DELETE /v1/reports/admin/<id>` | us | deletes one |
//! | `POST /v1/reports/admin/sweep` | us | deletes reports older than `REPORTS_RETENTION_DAYS` (the R2 lifecycle rule does this too) |
//!
//! Limits: `MAX_REPORT_BYTES` (25 MB) per report; the `REPORT_RATE` rate-limit binding
//! (short bursts, when bound) and a per-isolate bucket; `REPORTS_PER_IP_DAY` reports per
//! address and day and `REPORTS_DAY_BYTES` in total per day, counted from the day's R2
//! listing. Without `REPORTS_ADMIN_TOKEN` the admin routes do not exist (404). Nothing is ever
//! listed or served without the token.

use hsmp_master_core::reports as fmt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::IpAddr;
use worker::wasm_bindgen::JsCast;
use worker::*;

/// Per-isolate burst limit on uploads (the rate-limit binding is the real one).
const BURST: f64 = 3.0;
const REFILL_MS: f64 = 60_000.0;

thread_local! {
    static RATE: RefCell<HashMap<IpAddr, (f64, u64)>> = RefCell::new(HashMap::new());
}

fn isolate_allows(ip: IpAddr, now: u64) -> bool {
    RATE.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 20_000 {
            m.clear();
        }
        let (tokens, at) = m.entry(hsmp_master_core::ip::host_key(ip)).or_insert((BURST, now));
        *tokens = (*tokens + now.saturating_sub(*at) as f64 / REFILL_MS).min(BURST);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    })
}

fn respond(status: u16, body: &str) -> Result<Response> {
    let mut r = Response::ok(body)?.with_status(status);
    let h = r.headers_mut();
    h.set("cache-control", "no-store")?;
    h.set("x-content-type-options", "nosniff")?;
    Ok(r)
}

fn json(status: u16, v: &serde_json::Value) -> Result<Response> {
    let mut r = respond(status, &v.to_string())?;
    r.headers_mut().set("content-type", "application/json")?;
    Ok(r)
}

fn var_u64(env: &Env, name: &str, default: u64) -> u64 {
    env.var(name).ok().and_then(|v| v.to_string().trim().parse().ok()).unwrap_or(default)
}

fn random_hex16() -> Result<String> {
    let crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).map_err(|_| Error::RustError("no crypto".into()))?;
    let f: js_sys::Function = js_sys::Reflect::get(&crypto, &"getRandomValues".into()).map_err(|_| Error::RustError("no getRandomValues".into()))?.dyn_into().map_err(|_| Error::RustError("getRandomValues".into()))?;
    let a = js_sys::Uint8Array::new_with_length(16);
    f.call1(&crypto, &a).map_err(|_| Error::RustError("getRandomValues failed".into()))?;
    Ok(a.to_vec().iter().map(|b| format!("{b:02x}")).collect())
}

/// (reports from this address today, bytes stored today)
async fn day_usage(bucket: &Bucket, day: &str, tag: &str) -> Result<(u64, u64)> {
    let (mut mine, mut bytes) = (0u64, 0u64);
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut l = bucket.list().prefix(format!("r/{day}/")).include(vec![Include::CustomMetadata]).limit(1000);
        if let Some(c) = cursor.take() {
            l = l.cursor(c);
        }
        let page = l.execute().await?;
        for o in page.objects() {
            bytes += o.size();
            if o.custom_metadata().ok().and_then(|m| m.get("ip").cloned()).as_deref() == Some(tag) {
                mine += 1;
            }
        }
        if !page.truncated() {
            break;
        }
        cursor = page.cursor();
        if cursor.is_none() {
            break;
        }
    }
    Ok((mine, bytes))
}

async fn upload(mut req: Request, env: &Env, ip: IpAddr) -> Result<Response> {
    let len: Option<usize> = req.headers().get("content-length")?.and_then(|v| v.trim().parse().ok());
    match len {
        None => return respond(411, "content-length required"),
        Some(n) if n > fmt::MAX_REPORT_BYTES => return respond(413, "report too large (25 MB at most)"),
        _ => {}
    }
    let now = Date::now().as_millis();
    if let Ok(rl) = env.rate_limiter("REPORT_RATE") {
        if !rl.limit(hsmp_master_core::ip::host_key(ip).to_string()).await.map(|o| o.success).unwrap_or(true) {
            return respond(429, "too many reports, wait a minute");
        }
    }
    if !isolate_allows(ip, now) {
        return respond(429, "too many reports, wait a minute");
    }
    let Ok(bucket) = env.bucket("REPORTS") else { return respond(503, "bug reports are not enabled on this server") };
    let body = req.bytes().await?;
    if body.len() > fmt::MAX_REPORT_BYTES {
        return respond(413, "report too large (25 MB at most)");
    }
    let checked = match fmt::check(&body) {
        Ok(c) => c,
        Err(why) => return respond(400, why),
    };
    let day = fmt::day_of(now);
    let tag = fmt::ip_tag(&day, ip);
    let (mine, bytes) = day_usage(&bucket, &day, &tag).await?;
    if mine >= var_u64(env, "REPORTS_PER_IP_DAY", 3) {
        return respond(429, "too many reports from your address today; try again tomorrow, or attach the zip to a GitHub issue");
    }
    if bytes + body.len() as u64 > var_u64(env, "REPORTS_DAY_BYTES", 150 * 1024 * 1024) {
        return respond(429, "the report store is full for today; attach the zip to a GitHub issue instead");
    }
    let hex = random_hex16()?;
    let id = format!("{day}-{hex}");
    let key = fmt::object_key(&id).ok_or_else(|| Error::RustError("bad id".into()))?;
    let clip = |s: &str| s.chars().filter(|c| c.is_ascii_graphic()).take(40).collect::<String>();
    let meta: HashMap<String, String> = [
        ("ip".to_string(), tag),
        ("launcher".to_string(), clip(&checked.head.launcher)),
        ("hsmp".to_string(), clip(&checked.head.hsmp)),
        ("sessions".to_string(), checked.head.sessions.len().to_string()),
        ("entries".to_string(), checked.entries.to_string()),
        ("kind".to_string(), if checked.head.kind == "server" { "server" } else { "client" }.to_string()),
    ]
    .into_iter()
    .collect();
    let size = body.len();
    bucket
        .put(&key, Data::Bytes(body))
        .http_metadata(HttpMetadata { content_type: Some("application/zip".into()), ..Default::default() })
        .custom_metadata(meta)
        .execute()
        .await?;
    console_log!("report {id} stored ({size} bytes)");
    json(201, &serde_json::json!({"id": id, "retention_days": var_u64(env, "REPORTS_RETENTION_DAYS", 60)}))
}

fn admin_token(env: &Env) -> Option<String> {
    let t = env.secret("REPORTS_ADMIN_TOKEN").map(|s| s.to_string()).or_else(|_| env.var("REPORTS_ADMIN_TOKEN").map(|s| s.to_string())).ok()?;
    let t = t.trim().to_string();
    (t.len() >= 16).then_some(t)
}

async fn admin(req: Request, env: &Env, url: &Url, rest: &str) -> Result<Response> {
    let Some(token) = admin_token(env) else { return respond(404, "not found") };
    let given = req.headers().get("authorization")?.unwrap_or_default();
    let given = given.strip_prefix("Bearer ").unwrap_or("").trim().to_string();
    if !fmt::same_secret(&given, &token) {
        return respond(403, "forbidden");
    }
    let Ok(bucket) = env.bucket("REPORTS") else { return respond(503, "no REPORTS bucket bound") };
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    match (req.method(), rest) {
        (Method::Get, "") => {
            let prefix = match q.get("day") {
                Some(d) if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) => format!("r/{d}/"),
                Some(_) => return respond(400, "day=YYYYMMDD"),
                None => "r/".to_string(),
            };
            let mut l = bucket.list().prefix(prefix).include(vec![Include::CustomMetadata]).limit(500);
            if let Some(c) = q.get("cursor") {
                l = l.cursor(c.clone());
            }
            let page = l.execute().await?;
            let items: Vec<serde_json::Value> = page
                .objects()
                .iter()
                .filter_map(|o| {
                    let id = fmt::id_of_key(&o.key())?;
                    let mut m = o.custom_metadata().unwrap_or_default();
                    m.remove("ip");
                    Some(serde_json::json!({"id": id, "size": o.size(), "uploaded_ms": o.uploaded().as_millis(), "meta": m}))
                })
                .collect();
            json(200, &serde_json::json!({"reports": items, "cursor": if page.truncated() { page.cursor() } else { None }}))
        }
        (Method::Post, "/sweep") => {
            let days = var_u64(env, "REPORTS_RETENTION_DAYS", 60);
            let cutoff = fmt::day_of(Date::now().as_millis().saturating_sub(days * 86_400_000));
            let mut deleted = 0usize;
            let top = bucket.list().prefix("r/").delimiter("/").limit(1000).execute().await?;
            for p in top.delimited_prefixes() {
                let day = p.trim_start_matches("r/").trim_end_matches('/').to_string();
                if day.len() != 8 || day >= cutoff {
                    continue;
                }
                loop {
                    let page = bucket.list().prefix(p.clone()).limit(1000).execute().await?;
                    let keys: Vec<String> = page.objects().iter().map(|o| o.key()).collect();
                    if keys.is_empty() {
                        break;
                    }
                    deleted += keys.len();
                    bucket.delete_multiple(keys).await?;
                    if !page.truncated() {
                        break;
                    }
                }
            }
            json(200, &serde_json::json!({"deleted": deleted, "older_than": cutoff}))
        }
        (m, r) if r.starts_with('/') && (m == Method::Get || m == Method::Delete) => {
            let Some(key) = fmt::object_key(&r[1..]) else { return respond(400, "bad report id") };
            if m == Method::Delete {
                bucket.delete(&key).await?;
                return respond(204, "");
            }
            let Some(obj) = bucket.get(&key).execute().await? else { return respond(404, "no such report") };
            let bytes = obj.body().ok_or_else(|| Error::RustError("no body".into()))?.bytes().await?;
            let mut resp = Response::from_bytes(bytes)?;
            let h = resp.headers_mut();
            h.set("content-type", "application/zip")?;
            h.set("content-disposition", &format!("attachment; filename=\"hsmp-report-{}.zip\"", &r[1..]))?;
            h.set("cache-control", "no-store")?;
            Ok(resp)
        }
        _ => respond(404, "not found"),
    }
}

/// Every `/v1/reports*` request (the caller already enforced HTTPS and the Origin rule).
pub async fn handle(req: Request, env: &Env, ip: IpAddr, url: &Url) -> Result<Response> {
    let path = url.path().to_string();
    if path == "/v1/reports" {
        return match req.method() {
            Method::Post => upload(req, env, ip).await,
            _ => respond(405, "method not allowed"),
        };
    }
    if let Some(rest) = path.strip_prefix("/v1/reports/admin") {
        if rest.is_empty() || rest.starts_with('/') {
            return admin(req, env, url, rest).await;
        }
    }
    respond(404, "not found")
}

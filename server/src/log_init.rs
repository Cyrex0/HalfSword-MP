//! Log setup shared by hsmp-server and hsmp-sidecar: stdout as before, plus the log file in
//! `--log-dir` (or `HSMP_LOG_DIR`), and for the server a JSON-lines copy of every log event
//! next to it (`server-events*.jsonl`: one object per event with its fields, for tools).
//! The files get the same filter as stdout, no colours, and are written by background
//! threads (hsmp_diag::logfile), so logging never blocks the caller.
//!
//! Filter: `--log-level` (a level such as `debug`, or a full `RUST_LOG`-style filter), else
//! `RUST_LOG`, else the binary's default (info for its own crate).

use std::path::Path;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// Targets that go to the JSON-lines events file only (the structured stats event).
pub const EVENT_ONLY: &str = "hsmp_server::stats::event";

/// A session folder's file is rotated above this size; three are kept.
pub const FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const FILE_KEEP: usize = 2;
/// A dedicated server's daily files: 64 MB per file, 14 days, 400 MB of logs and 100 MB of
/// events in all.
pub const DAILY_MAX_FILE: u64 = 64 * 1024 * 1024;
pub const DAILY_KEEP_DAYS: u64 = 14;
pub const DAILY_LOG_TOTAL: u64 = 400 * 1024 * 1024;
pub const DAILY_EVENTS_TOTAL: u64 = 100 * 1024 * 1024;

/// `--log-level` / RUST_LOG / default -> the filter string. A bare level applies to the HSMP
/// crates (`own` and the shared libraries); other crates stay at warn.
pub fn filter_spec(level: Option<&str>, rust_log: Option<&str>, own: &str, default_filter: &str) -> String {
    let pick = level.filter(|s| !s.trim().is_empty()).or(rust_log.filter(|s| !s.trim().is_empty()));
    match pick.map(str::trim) {
        Some(l) if !own.is_empty() && ["error", "warn", "info", "debug", "trace"].contains(&l.to_ascii_lowercase().as_str()) => {
            let l = l.to_ascii_lowercase();
            format!("warn,{own}={l},hsmp_net={l},hsmp_master_core={l},hsmp_diag={l}")
        }
        Some(spec) => spec.to_string(),
        None => default_filter.to_string(),
    }
}

/// Where the files go and how they rotate.
pub struct FileOpts<'a> {
    pub dir: &'a Path,
    /// `server` / `sidecar`.
    pub name: &'a str,
    /// Daily files with pruning (a dedicated server), else one size-rotated file (a session folder).
    pub daily: bool,
    /// Also the JSON-lines events file.
    pub events: bool,
}

/// Keep until `main` returns: flushes the files.
pub struct Guards(#[allow(dead_code)] Vec<hsmp_diag::logfile::FlushGuard>);

type Opened = (hsmp_diag::logfile::LogWriter, hsmp_diag::logfile::FlushGuard);

fn open(o: &FileOpts, name: &str, ext: &str, total: u64, banner: bool) -> std::io::Result<Opened> {
    use hsmp_diag::logfile::{open_with, Rotation};
    let rot = if o.daily {
        Rotation::Daily { max_file: DAILY_MAX_FILE, keep_days: DAILY_KEEP_DAYS, max_total: total }
    } else {
        Rotation::Size { max: FILE_MAX_BYTES, keep: FILE_KEEP }
    };
    open_with(o.dir, name, ext, rot, banner)
}

/// Install the global subscriber. A log dir that cannot be opened is reported on stderr and
/// logging goes on to stdout only.
pub fn init_with(filter: &str, file: Option<FileOpts>) -> Option<Guards> {
    let f = |s: &str| EnvFilter::try_new(s).unwrap_or_else(|_| EnvFilter::new("info"));
    let text_filter = format!("{filter},{EVENT_ONLY}=off");
    let stdout = fmt::layer().with_filter(f(&text_filter));
    let Some(o) = file else {
        tracing_subscriber::registry().with(stdout).init();
        return None;
    };
    let mut guards = vec![];
    let text = match open(&o, o.name, "log", DAILY_LOG_TOTAL, true) {
        Ok((w, g)) => {
            guards.push(g);
            let _ = WRITER.set(w.clone());
            Some(fmt::layer().with_ansi(false).with_writer(move || w.clone()).with_filter(f(&text_filter)))
        }
        Err(e) => {
            eprintln!("log file in {}: {e}; logging to stdout only", o.dir.display());
            None
        }
    };
    let json = if o.events && text.is_some() {
        match open(&o, &format!("{}-events", o.name), "jsonl", DAILY_EVENTS_TOTAL, false) {
            Ok((w, g)) => {
                guards.push(g);
                Some(JsonLayer { w }.with_filter(f(&format!("{filter},{EVENT_ONLY}=info"))))
            }
            Err(e) => {
                eprintln!("events file in {}: {e}", o.dir.display());
                None
            }
        }
    } else {
        None
    };
    tracing_subscriber::registry().with(stdout).with(text).with(json).init();
    Some(Guards(guards))
}

/// The sidecar's setup: stdout, plus `<dir>/<name>.log` (size-rotated) when a dir is given.
#[allow(dead_code)]
pub fn init(default_filter: &str, log_dir: Option<&Path>, name: &str) -> Option<Guards> {
    let spec = filter_spec(None, std::env::var("RUST_LOG").ok().as_deref(), "", default_filter);
    init_with(&spec, log_dir.map(|dir| FileOpts { dir, name, daily: false, events: false }))
}

/// `--log-dir`, else a non-empty `HSMP_LOG_DIR`.
pub fn dir_from(arg: Option<&Path>) -> Option<std::path::PathBuf> {
    arg.map(Path::to_path_buf).or_else(|| std::env::var_os("HSMP_LOG_DIR").filter(|v| !v.is_empty()).map(Into::into))
}

static WRITER: std::sync::OnceLock<hsmp_diag::logfile::LogWriter> = std::sync::OnceLock::new();

/// Put what is queued on disk (at most 500 ms): call before `std::process::exit` or from a
/// panic hook.
pub fn flush() {
    if let Some(w) = WRITER.get() {
        w.flush_wait(std::time::Duration::from_millis(500));
    }
}

/// Every log event as one JSON object: `ts` (UTC), `level`, `target`, `message` and the
/// event's own fields (numbers and booleans typed, the rest as text).
struct JsonLayer {
    w: hsmp_diag::logfile::LogWriter,
}

struct JsonVisitor(serde_json::Map<String, serde_json::Value>);

impl Visit for JsonVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        // a `report = %json` field (the stats event) goes in as JSON, not as a string of it
        let v = if field.name() == "report" { serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s)) } else { serde_json::Value::String(s) };
        self.0.insert(field.name().to_string(), v);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), serde_json::Value::String(value.to_string()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.into());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_string(), serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.into());
    }
}

/// One JSON line for an event's metadata and fields.
pub fn json_line(level: &str, target: &str, fields: serde_json::Map<String, serde_json::Value>) -> String {
    let mut m = serde_json::Map::new();
    m.insert("ts".into(), hsmp_diag::time::iso_ms(hsmp_diag::time::now_ms()).into());
    m.insert("level".into(), level.into());
    m.insert("target".into(), target.into());
    m.insert("pid".into(), std::process::id().into());
    m.extend(fields);
    let mut s = serde_json::Value::Object(m).to_string();
    s.push('\n');
    s
}

impl<S: tracing::Subscriber> Layer<S> for JsonLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        use std::io::Write;
        let mut v = JsonVisitor(serde_json::Map::new());
        event.record(&mut v);
        let md = event.metadata();
        let line = json_line(md.level().as_str(), md.target(), v.0);
        let mut w = self.w.clone();
        let _ = w.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_from_level_env_and_default() {
        assert_eq!(filter_spec(None, None, "hsmp_server", "hsmp_server=info"), "hsmp_server=info");
        assert_eq!(filter_spec(Some("debug"), Some("trace"), "hsmp_server", "x"), "warn,hsmp_server=debug,hsmp_net=debug,hsmp_master_core=debug,hsmp_diag=debug");
        assert_eq!(filter_spec(None, Some("hsmp_server=trace,hyper=info"), "hsmp_server", "x"), "hsmp_server=trace,hyper=info");
        assert_eq!(filter_spec(Some(" "), None, "hsmp_server", "d"), "d");
    }

    #[test]
    fn json_lines_are_objects_with_the_fields() {
        let mut f = serde_json::Map::new();
        f.insert("message".into(), "peer joined".into());
        f.insert("peer_id".into(), 3.into());
        let l = json_line("INFO", "hsmp_server::server::dispatch", f);
        assert!(l.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(l.trim()).unwrap();
        assert_eq!(v["level"], "INFO");
        assert_eq!(v["peer_id"], 3);
        assert_eq!(v["message"], "peer joined");
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
    }
}

//! Sidecar panic policy: a panic anywhere — main task, any
//! spawned tokio task, any blocking thread — is fatal for the process.
//!
//! Without this, tokio swallows a task panic: the task is gone (e.g. the
//! recv loop), the process keeps running and the game sees a sidecar that is
//! alive but deaf. A half-dead sidecar is worse than a dead one: the harness
//! and the Lua side detect a dead process (exit code, a stale heartbeat), but
//! not a missing thread.
//!
//! On the first panic the hook:
//!   1. logs message, location, thread name and a forced backtrace to stderr
//!      and appends them to `<state-dir>/.sidecar_panic.log`;
//!   2. runs the registered fatal callbacks once (best effort, `try_lock`
//!      only, never blocks): the career-save restore;
//!   3. `std::process::exit(EXIT_PANIC)`.
//! A panic raised while the hook itself runs (re-entrancy) exits at once.
//!
//! Self-contained (std only, plus tokio for the debug test hook), so the
//! server can adopt the same policy with
//! `#[path = "sidecar/panic_guard.rs"] mod panic_guard;` + `panic_guard::install(None)`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Process exit code after a panic (Rust's own panic=abort code is 101 too).
pub const EXIT_PANIC: i32 = 101;

type FatalFn = Box<dyn Fn() + Send + Sync>;

static IN_PANIC: AtomicBool = AtomicBool::new(false);
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static ON_FATAL: OnceLock<Mutex<Vec<FatalFn>>> = OnceLock::new();

fn on_fatal() -> &'static Mutex<Vec<FatalFn>> {
    ON_FATAL.get_or_init(|| Mutex::new(Vec::new()))
}

/// Install the hook. Call first thing in `main`; `state_dir` is where the
/// panic log goes (`.sidecar_panic.log`).
pub fn install(state_dir: Option<PathBuf>) {
    if let Some(d) = state_dir {
        let _ = LOG_PATH.set(d.join(".sidecar_panic.log"));
    }
    std::panic::set_hook(Box::new(|info| {
        if IN_PANIC.swap(true, Ordering::SeqCst) {
            // panic inside the hook or a second thread panicking meanwhile
            std::process::exit(EXIT_PANIC);
        }
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".into());
        let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let th = std::thread::current();
        let bt = std::backtrace::Backtrace::force_capture();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "hsmp".into());
        let text = format!(
            "{exe} FATAL panic (pid {}, unix_ms {ts}) in thread '{}': {msg}\n  at {loc}\nbacktrace:\n{bt}\n",
            std::process::id(),
            th.name().unwrap_or("<unnamed>"),
        );
        let _ = std::io::stderr().write_all(text.as_bytes());
        if let Some(p) = LOG_PATH.get() {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = f.write_all(text.as_bytes());
            }
        }
        // Best-effort cleanup; a callback that panics re-enters and exits.
        if let Ok(fns) = on_fatal().try_lock() {
            for f in fns.iter() {
                f();
            }
        }
        let _ = std::io::stderr().write_all(format!("{exe}: exiting after panic (code {EXIT_PANIC})\n").as_bytes());
        std::process::exit(EXIT_PANIC);
    }));
}

/// Register a callback run once from the panic hook before exit. It must not
/// block (use `try_lock`), and must tolerate any state.
pub fn on_fatal_panic(f: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut v) = on_fatal().lock() {
        v.push(Box::new(f));
    }
}

/// Debug/test builds only: `HSMP_SIDECAR_TEST_PANIC=1` panics inside a
/// spawned tokio task shortly after startup (proves task panics are fatal).
pub fn maybe_test_panic() {
    #[cfg(debug_assertions)]
    if std::env::var("HSMP_SIDECAR_TEST_PANIC").map_or(false, |v| v == "1") {
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            panic!("HSMP_SIDECAR_TEST_PANIC: forced test panic in a spawned task");
        });
    }
}

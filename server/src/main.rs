//! Command line entry point; server logic is also embedded in HSMPNative.
mod native_supervisor;
mod native_probe;
#[path = "proc_util.rs"]
mod proc_util;

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("native")) {
        native_supervisor::run()
    } else if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("native-probe")) {
        native_probe::run()
    } else {
        hsmp_server::run()
    }
}

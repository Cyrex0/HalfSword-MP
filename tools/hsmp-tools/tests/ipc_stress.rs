//! A short cross-process run of `hsmp-tools ipc-stress` (Windows; all six scenarios at a
//! reduced duration), then a pace-robustness run: the writer several times faster (`--fast`)
//! and the readers asleep inside the attach window (`--race-ms`), which once produced
//! `ring_bad` (a reader validating records of a just-attached writer against epoch 0).
//! The full run is `hsmp-tools ipc-stress [--duration 60]`.

#[cfg(windows)]
fn run(args: &[&str]) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hsmp-tools"))
        .arg("ipc-stress")
        .args(args)
        .output()
        .expect("run hsmp-tools ipc-stress");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "ipc-stress {:?} failed:\n{}\n{}", args, text, String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("ALL PASS"), "{}", text);
}

#[cfg(windows)]
#[test]
fn ipc_stress_short_run_passes() {
    run(&["--duration", "8"]);
    // Scenario 2 measures recovery from attach, which the reader's race sleep would skew.
    run(&["--duration", "5", "--fast", "--race-ms", "300", "--only", "1,4,5"]);
}
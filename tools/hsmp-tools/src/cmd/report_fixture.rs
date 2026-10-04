//! `hsmp-tools report-fixture`: write a minimal bug-report zip in the launcher's upload format
//! (or a deliberately broken one) for the Worker's `POST /v1/reports` test
//! (scripts/e2e-master-cf.sh). The real launcher zip is checked against the same format in
//! the launcher's unit tests.
//!
//!     hsmp-tools report-fixture --out ok.zip
//!     hsmp-tools report-fixture --out bad.zip --magic nope
//!     hsmp-tools report-fixture --out pad.zip --pad-mb 26

use anyhow::Result;
use hsmp_master_core::reports as fmt;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Where to write the zip
    #[arg(long)]
    out: PathBuf,
    /// The manifest's magic (default: the real one)
    #[arg(long, default_value = fmt::MAGIC)]
    magic: String,
    /// Add a stored entry of this many MB (size-cap tests)
    #[arg(long, default_value_t = 0)]
    pad_mb: usize,
}

pub fn run(a: Args) -> Result<i32> {
    let manifest = serde_json::json!({"magic": a.magic, "format": fmt::FORMAT, "launcher": "e2e", "hsmp": "e2e", "sessions": ["20261004_000000_p1"], "files": []}).to_string();
    let log = b"[Lua] e2e report\n".to_vec();
    let pad = vec![0u8; a.pad_mb << 20];
    let mut entries: Vec<(&str, &[u8])> = vec![(fmt::MANIFEST_NAME, manifest.as_bytes()), ("sessions/20261004_000000_p1/UE4SS.log", &log)];
    if !pad.is_empty() {
        entries.push(("pad.bin", &pad));
    }
    std::fs::write(&a.out, fmt::fixture_zip(&entries))?;
    Ok(0)
}

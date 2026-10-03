//! `hsmp-tools gen-ipc`: write the generated IPC schema files from `crates/hsmp-ipc`
//! (`crates/hsmp-native/cpp/gen/hsmp_ipc.h` and `mods/shared/hsmp_ipc_schema.lua`).
//! `--check` regenerates in memory and exits 1 if a committed copy differs (the G0 gate's
//! `ipc-schema` check).

use hsmp_tools::paths;

#[derive(clap::Args)]
pub struct Args {
    /// Only compare; exit 1 if a committed file is missing or differs.
    #[arg(long)]
    pub check: bool,
}

pub fn run(a: Args) -> anyhow::Result<i32> {
    let root = paths::repo_root()?;
    if a.check {
        let bad = hsmp_ipc::gen::check_all(&root);
        if bad.is_empty() {
            println!("gen-ipc: generated IPC schema files are up to date (layout hash {:016x})", hsmp_ipc::LAYOUT_HASH);
            return Ok(0);
        }
        for b in bad {
            eprintln!("gen-ipc: {} is stale; run `hsmp-tools gen-ipc`", b);
        }
        return Ok(1);
    }
    for p in hsmp_ipc::gen::write_all(&root)? {
        println!("gen-ipc: wrote {}", p.display());
    }
    Ok(0)
}

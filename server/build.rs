// Embeds the content hash (crates/hsmp-net/src/build/content.rs) of this checkout as
// HSMP_CONTENT_HASH, so every server and sidecar knows which mod files and server data it
// was built with. A build without the repo around it can preset HSMP_CONTENT_HASH.

#[path = "../crates/hsmp-net/src/build/content.rs"]
#[allow(dead_code)]
mod content;

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=HSMP_CONTENT_HASH");
    println!("cargo:rerun-if-changed=../crates/hsmp-net/src/build/content.rs");
    if let Ok(h) = std::env::var("HSMP_CONTENT_HASH") {
        let h = h.trim().to_ascii_lowercase();
        assert!(h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()), "HSMP_CONTENT_HASH must be 64 hex chars");
        println!("cargo:rustc-env=HSMP_CONTENT_HASH={h}");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let src = content::FsSource(root.clone());
    let Some((hash, _)) = content::content_hash(&src) else {
        panic!("{} not found: build from the full repository, or set HSMP_CONTENT_HASH", root.join(content::TEMPLATE).display());
    };
    // Cargo scans a directory recursively: any edit, added or removed file reruns this.
    println!("cargo:rerun-if-changed=../mods");
    println!("cargo:rerun-if-changed=../server/data");
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=HSMP_CONTENT_HASH={hex}");
}

//! Lift `pub mod catalog { ... }` (the loadout catalogue: weapon/armour ids,
//! slot groups) out of server/src/loadout.rs, whose other parts need the full
//! server, so `validate::damage` resolves kits exactly as the server does.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../server/src/loadout.rs");
    println!("cargo:rerun-if-changed={}", src.display());
    let text = std::fs::read_to_string(&src).expect("server/src/loadout.rs");
    let start = text.find("pub mod catalog {").expect("loadout.rs: `pub mod catalog {` not found");
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut end = None;
    let mut i = start;
    // Brace matching that skips string / char literals and comments.
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' { i += 1; }
                    i += 1;
                }
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => i += 2,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 { end = Some(i + 1); break; }
            }
            _ => {}
        }
        i += 1;
    }
    let end = end.expect("loadout.rs: unbalanced catalog module");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("catalog.rs");
    std::fs::write(&out, &text[start..end]).unwrap();
}

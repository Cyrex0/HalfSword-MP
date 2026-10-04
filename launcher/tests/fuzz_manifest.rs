//! Seeded mutation tests of the update path's parsers: the manifest JSON, the signature file,
//! the published .sha256, GitHub's release JSON, and a signed release zip as a whole.
//! Deterministic; HSMP_FUZZ_ITERS raises the count for a local soak.
//!
//! Invariants beyond "no panic": whatever manifest validates only installs under the Win64
//! prefix with safe paths, a release asset name can never leave the downloads folder, and a
//! mutated zip never opens with a manifest other than the signed one or loads changed files.

use ed25519_dalek::SigningKey;
use hsmp_launcher::manifest::{self, GameBuild, GameCompat, IniSetting, Manifest, Role, Ue4ssInfo, INSTALL_PREFIX};
use hsmp_launcher::pack::{self, Item};
use hsmp_launcher::package::Package;
use hsmp_launcher::{sign, update, util};
use std::collections::BTreeMap;

fn iters(ci: usize) -> usize {
    std::env::var("HSMP_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(ci)
}

struct Xs(u64);

impl Xs {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn mutate(r: &mut Xs, seed: &[u8]) -> Vec<u8> {
    const TOKENS: &[&[u8]] = &[b"\"", b"\\", b"..", b"/", b"\\\\", b":", b"C:", b"%", b"&", b" ", b"\0", b"{", b"}", b",",
        b"-1", b"null", b"HalfswordUE5/Binaries/Win64/", b"../", b"hsmp.cfg", b"\\u002e\\u002e"];
    let mut d = seed.to_vec();
    for _ in 0..1 + r.below(4) {
        match r.below(4) {
            0 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] ^= 1 << r.below(8);
            }
            1 if !d.is_empty() => {
                let i = r.below(d.len());
                d.remove(i);
            }
            _ => {
                let i = r.below(d.len() + 1);
                let t = TOKENS[r.below(TOKENS.len())];
                d.splice(i..i, t.iter().copied());
            }
        }
    }
    d
}

fn sample() -> Manifest {
    let h = |b: &[u8]| util::sha256_hex(b);
    Manifest {
        schema: manifest::SCHEMA,
        product: manifest::PRODUCT.into(),
        version: "0.1.0".into(),
        channel: "test".into(),
        git_commit: "0".repeat(40),
        source_date_epoch: 1_790_899_200,
        protocol_version: 6,
        toolchain: String::new(),
        game: GameCompat { steam_appid: 2397300, builds: vec![GameBuild { label: "t".into(), steam_buildid: None, exe_sha256: h(b"exe"), exe_size: 3 }] },
        ue4ss: Ue4ssInfo { version: "pin".into(), dll_sha256: h(b"dll"), proxy_sha256: h(b"proxy") },
        master_urls: vec!["https://master.example.net".into()],
        launch_args: vec!["-ini:Engine:[SystemSettings]:r.X=0".into()],
        ini_settings: vec![IniSetting { file: "Engine".into(), section: "SystemSettings".into(), key: "r.X".into(), value: "0".into(), reason: String::new() }],
        mods_template: "payload/mods.release.txt".into(),
        files: vec![],
        report_upload: false,
    }
}

fn items() -> BTreeMap<String, Item> {
    let mut m = BTreeMap::new();
    m.insert("payload/mods.release.txt".into(), Item { bytes: b"HSMPMenu : 1\n".to_vec(), role: Role::Template, install: None });
    m.insert("payload/Win64/dwmapi.dll".into(), Item { bytes: b"proxy".to_vec(), role: Role::Ue4ss, install: Some(format!("{INSTALL_PREFIX}dwmapi.dll")) });
    m.insert("payload/mods/x.lua".into(), Item { bytes: b"-- x".to_vec(), role: Role::Mod, install: Some(format!("{INSTALL_PREFIX}ue4ss/Mods/X/x.lua")) });
    m
}

#[test]
fn manifest_and_sig_parsers_survive_mutation() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let (mb, sb) = pack::seal(sample(), &items(), &key).unwrap();
    let mut r = Xs(0x1A0C_0001);
    for i in 0..iters(5_000) {
        let d = mutate(&mut r, if i % 4 == 0 { &sb } else { &mb });
        if let Ok(m) = Manifest::parse(&d) {
            for (_, t) in m.installed_files() {
                assert!(util::is_safe_rel(t) && t.starts_with(INSTALL_PREFIX), "unsafe install target {t}");
                assert!(t.split('/').all(|c| c != ".." && c != "."), "{t}");
            }
            for f in &m.files {
                assert!(util::is_safe_rel(&f.path), "unsafe package path {}", f.path);
            }
            for u in &m.master_urls {
                assert!(manifest::safe_url(u), "{u}");
            }
        }
        if let Ok(s) = sign::SigFile::parse(&d) {
            let keys = sign::parse_trusted_keys(&format!("{} k\n", hex::encode(key.verifying_key().as_bytes()))).unwrap();
            if sign::verify(&mb, &s, &keys).is_ok() {
                assert_eq!(s.sig.to_ascii_lowercase(), sign::SigFile::parse(&sb).unwrap().sig);
            }
        }
    }
}

#[test]
fn release_json_and_checksum_parsers_survive_mutation() {
    let rel = br#"{"tag_name":"v0.1.0","name":"x","draft":false,"prerelease":false,"html_url":"","assets":[
        {"name":"hsmp-0.1.0.zip","browser_download_url":"https://github.com/a/b/releases/download/v0.1.0/hsmp-0.1.0.zip","size":40000000},
        {"name":"hsmp-0.1.0.zip.sha256","browser_download_url":"https://github.com/a/b/x.sha256","size":90}]}"#;
    let sha = format!("{}  hsmp-0.1.0.zip\n", "ab".repeat(32));
    let mut r = Xs(0x1A0C_0002);
    for i in 0..iters(8_000) {
        let d = mutate(&mut r, if i % 3 == 0 { sha.as_bytes() } else { rel });
        if let Ok(rel) = serde_json::from_slice::<update::Release>(&d) {
            if let Ok((zip, s)) = update::select_assets(&rel) {
                for n in [&zip.name, &s.name] {
                    assert!(!n.contains('/') && !n.contains('\\') && !n.contains("..") && !n.contains(':'), "asset name {n}");
                }
            }
            let _ = update::pick_latest(std::slice::from_ref(&rel), i % 2 == 0);
        }
        if let Ok(h) = update::parse_sha256(&String::from_utf8_lossy(&d)) {
            assert!(util::is_sha256_hex(&h));
        }
    }
}

/// A signed release zip, mutated byte-wise: it either fails to open, or opens with exactly the
/// signed manifest and then loads only the signed bytes (or fails).
#[test]
fn mutated_release_zip_never_installs_changed_bytes() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let it = items();
    let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
    let zip = pack::zip_bytes("hsmp-0.1.0", &it, &mb, &sb, 1_790_899_200).unwrap();
    let keys = sign::parse_trusted_keys(&format!("{} k\n", hex::encode(key.verifying_key().as_bytes()))).unwrap();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hsmp-fuzz-zip");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.zip");
    let mut r = Xs(0x1A0C_0003);
    let mut opened = 0;
    for _ in 0..iters(600) {
        let mut d = zip.clone();
        for _ in 0..1 + r.below(3) {
            let i = r.below(d.len());
            d[i] ^= 1 << r.below(8);
        }
        std::fs::write(&path, &d).unwrap();
        let Ok(p) = Package::open(&path, &keys) else { continue };
        opened += 1;
        assert_eq!(p.manifest_bytes, mb, "a zip opened with an unsigned manifest");
        if let Ok(files) = p.load_files(true) {
            for (k, v) in files {
                assert_eq!(v, it[&k].bytes, "{k} loaded with changed bytes");
            }
        }
    }
    println!("{opened} mutated zips still opened (mutations outside the signed bytes)");
}

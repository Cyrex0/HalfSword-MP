//! End-to-end package tests: seal -> write (dir and zip) -> open/verify ->
//! tamper detection -> install from a verified zip into a fake game dir.

use crate::manifest::{tests::sample, Role};
use crate::pack::{self, Item};
use crate::package::{Package, MANIFEST, SIGNATURE};
use crate::sign::tests::{test_key, trusted};
use crate::testutil::TempDir;
use std::collections::BTreeMap;

fn items() -> BTreeMap<String, Item> {
    let mut m = BTreeMap::new();
    let mut add = |p: &str, b: &[u8], role: Role, install: Option<&str>| {
        m.insert(p.to_string(), Item { bytes: b.to_vec(), role, install: install.map(String::from) });
    };
    add("payload/mods.release.txt", b"HSMPMenu : 1\nKeybinds : dev\n", Role::Template, None);
    add("payload/Win64/dwmapi.dll", b"proxy bytes", Role::Ue4ss, Some("HalfswordUE5/Binaries/Win64/dwmapi.dll"));
    add("payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua", b"-- menu", Role::Mod, Some("HalfswordUE5/Binaries/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua"));
    add("hsmp-launcher.exe", b"MZ launcher", Role::Launcher, None);
    m
}

#[test]
fn seal_open_verify_dir_and_zip() {
    let t = TempDir::new("pkg");
    let key = test_key(3);
    let it = items();
    let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
    // deterministic
    let (mb2, sb2) = pack::seal(sample(), &it, &key).unwrap();
    assert_eq!((mb.clone(), sb.clone()), (mb2, sb2));
    let z1 = pack::zip_bytes("hsmp-0.1.0", &it, &mb, &sb, 1_790_899_200).unwrap();
    let z2 = pack::zip_bytes("hsmp-0.1.0", &it, &mb, &sb, 1_790_899_200).unwrap();
    assert_eq!(z1, z2, "zip is byte-reproducible");

    let dir = t.path().join("hsmp-0.1.0");
    pack::write_dir(&dir, &it, &mb, &sb).unwrap();
    let zip = t.path().join("hsmp-0.1.0.zip");
    std::fs::write(&zip, &z1).unwrap();
    let keys = trusted(&key);
    for src in [&dir, &zip] {
        let p = Package::open(src, &keys).unwrap();
        assert_eq!(p.manifest.version, "0.1.0");
        let files = p.load_files(false).unwrap();
        assert_eq!(files.len(), 3, "launcher not loaded by default");
        assert_eq!(files["payload/Win64/dwmapi.dll"], b"proxy bytes");
        assert_eq!(p.load_files(true).unwrap().len(), 4);
    }
    // untrusted key
    assert!(Package::open(&dir, &trusted(&test_key(4))).err().unwrap().contains("does not trust"));
}

#[test]
fn tampering_is_rejected() {
    let t = TempDir::new("pkg_tamper");
    let key = test_key(3);
    let keys = trusted(&key);
    let it = items();
    let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
    let fresh = |name: &str| {
        let d = t.path().join(name);
        pack::write_dir(&d, &it, &mb, &sb).unwrap();
        d
    };
    // payload byte flipped
    let d = fresh("a");
    std::fs::write(d.join("payload/Win64/dwmapi.dll"), b"proxy byteZ").unwrap();
    let p = Package::open(&d, &keys).unwrap();
    assert!(p.load_files(false).unwrap_err().contains("SHA-256"));
    // payload truncated
    let d = fresh("b");
    std::fs::write(d.join("payload/Win64/dwmapi.dll"), b"proxy").unwrap();
    assert!(Package::open(&d, &keys).unwrap().load_files(false).unwrap_err().contains("size"));
    // payload missing
    let d = fresh("c");
    std::fs::remove_file(d.join("payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua")).unwrap();
    assert!(Package::open(&d, &keys).unwrap().load_files(false).unwrap_err().contains("missing"));
    // manifest edited (e.g. a hash swapped to match a malicious file)
    let d = fresh("d");
    let edited = String::from_utf8(mb.clone()).unwrap().replace("0.1.0", "0.1.1");
    std::fs::write(d.join(MANIFEST), edited).unwrap();
    assert!(Package::open(&d, &keys).err().unwrap().contains("INVALID"));
    // signature removed
    let d = fresh("e");
    std::fs::remove_file(d.join(SIGNATURE)).unwrap();
    assert!(Package::open(&d, &keys).err().unwrap().contains("not signed"));
    // not a package
    assert!(Package::open(&t.path().join("nothing"), &keys).is_err());
    let empty = t.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(Package::open(&empty, &keys).err().unwrap().contains("Extract the WHOLE"));
}

/// Low: nothing is read beyond fixed caps before verification, nor beyond the
/// signed size after it (a zip header's declared size is never trusted).
#[test]
fn reads_are_capped() {
    let t = TempDir::new("pkg_caps");
    let key = test_key(3);
    let keys = trusted(&key);
    let it = items();
    let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
    // a huge "manifest" (folder and zip)
    let d = t.path().join("big");
    pack::write_dir(&d, &it, &mb, &sb).unwrap();
    let mut huge = mb.clone();
    huge.resize((crate::package::MAX_MANIFEST + 10) as usize, b' ');
    std::fs::write(d.join(MANIFEST), &huge).unwrap();
    assert!(Package::open(&d, &keys).err().unwrap().contains("larger than"));
    let mut big_items = BTreeMap::new();
    big_items.insert("x".to_string(), Item { bytes: vec![], role: Role::Doc, install: None });
    let z = pack::zip_bytes("hsmp-0.1.0", &big_items, &huge, &sb, 1_790_899_200).unwrap();
    std::fs::write(t.path().join("big.zip"), z).unwrap();
    assert!(Package::open(&t.path().join("big.zip"), &keys).err().unwrap().contains("larger than"));
    // a payload longer than the signed size
    let d = t.path().join("long");
    pack::write_dir(&d, &it, &mb, &sb).unwrap();
    std::fs::write(d.join("payload/Win64/dwmapi.dll"), vec![b'x'; 4096]).unwrap();
    assert!(Package::open(&d, &keys).unwrap().load_files(false).unwrap_err().contains("wrong size"));
}

#[test]
fn zip_package_installs_and_uninstalls() {
    use crate::install::{self, Env, InstallOpts};
    let t = TempDir::new("pkg_install");
    let key = test_key(5);
    let it = items();
    let (mb, sb) = pack::seal(sample(), &it, &key).unwrap();
    let zip = t.path().join("rel.zip");
    std::fs::write(&zip, pack::zip_bytes("hsmp-0.1.0", &it, &mb, &sb, 1_790_899_200).unwrap()).unwrap();
    let p = Package::open(&zip, &trusted(&key)).unwrap();
    let files = p.load_files(false).unwrap();

    let root = t.path().join("game");
    let w = crate::game::win64(&root);
    std::fs::create_dir_all(&w).unwrap();
    std::fs::write(w.join(crate::game::EXE_NAME), b"exe").unwrap();
    let env = Env::new(&root, t.path().join("la/HSMP"), t.path().join("la/HalfSwordUE5/Saved"));
    let before = crate::testutil::snapshot(t.path().join("game").as_path());
    let opts = InstallOpts { backup_saves: true, ..Default::default() };
    install::install(&env, &p.manifest, &files, &opts, &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(w.join("ue4ss/Mods/HSMPMenu/Scripts/main.lua")).unwrap(), b"-- menu");
    assert!(env.ini_path("Engine").is_file(), "Engine.ini created");
    install::uninstall(&env, false, &mut |_| {}).unwrap();
    assert_eq!(crate::testutil::snapshot(&root), before);
    assert!(!env.ini_path("Engine").exists(), "Engine.ini we created is removed again");
}

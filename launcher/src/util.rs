//! Small shared helpers: hashing, atomic writes, timestamps, path joins.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Lower-case hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Streaming SHA-256 of a file: (hex digest, size in bytes).
pub fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
    }
    Ok((hex::encode(h.finalize()), n_total))
}

/// True when `s` is a 64-char lower-case hex digest.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `.<name>.hsmp_tmp` next to `path` (built from OS strings: exact for any name).
fn tmp_path_for(path: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut n = std::ffi::OsString::from(".");
    n.push(name);
    n.push(".hsmp_tmp");
    path.with_file_name(n)
}

/// The temp file `atomic_write` uses for `path` (left behind only by a crash).
pub fn atomic_tmp_path(path: &Path) -> Option<PathBuf> {
    path.file_name().map(|n| tmp_path_for(path, n))
}

/// Case-insensitive key for a path or name, as Windows (NTFS) compares them.
/// Full Unicode lower-casing, not ASCII-only: "Ä" and "ä" are one file.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Folded absolute path for comparisons ('/' -> '\', no trailing '\', no `\\?\`).
pub fn fold_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix("\\\\?\\UNC\\").map(|r| format!("\\\\{r}")).unwrap_or_else(|| s.strip_prefix("\\\\?\\").unwrap_or(&s).to_string());
    fold(s.trim_end_matches('\\'))
}

/// Write `bytes` to `path` atomically: temp file in the same directory,
/// flushed to disk, then renamed over the target. A failure leaves the
/// target untouched (and no temp file behind).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("no file name: {}", path.display())))?;
    let tmp = tmp_path_for(path, name);
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// How a text file the launcher merges (mods.txt, hsmp.cfg, Engine.ini) was
/// encoded, so it is written back the same way. Unreal writes ini files as
/// UTF-16LE with a BOM as soon as they hold a non-ANSI character, and other
/// tools write mods.txt in a local code page (GBK, Shift-JIS...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEnc {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// Plane-16 private-use code points stand in for bytes that are not valid
/// UTF-8 (U+10FF00 + byte) and for unpaired UTF-16 surrogates
/// (U+100000 + unit), so `encode_text(decode_text(b)) == b` for ANY input.
const ESC_BYTE: u32 = 0x10_FF00;
const ESC_UNIT: u32 = 0x10_0000;

fn esc(c: u32) -> char {
    char::from_u32(c).expect("plane-16 code point")
}

/// Decode a text file without losing a single byte (see `TextEnc`).
pub fn decode_text(b: &[u8]) -> (String, TextEnc) {
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        return (decode_utf16(rest, u16::from_le_bytes), TextEnc::Utf16Le);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        return (decode_utf16(rest, u16::from_be_bytes), TextEnc::Utf16Be);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (decode_utf8_escaped(rest), TextEnc::Utf8Bom);
    }
    (decode_utf8_escaped(b), TextEnc::Utf8)
}

fn decode_utf8_escaped(mut b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let ok = e.valid_up_to();
                out.push_str(std::str::from_utf8(&b[..ok]).expect("valid prefix"));
                let bad = e.error_len().unwrap_or(b.len() - ok);
                for &x in &b[ok..ok + bad] {
                    out.push(esc(ESC_BYTE + x as u32));
                }
                b = &b[ok + bad..];
            }
        }
    }
}

fn decode_utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| f([c[0], c[1]])).collect();
    let mut out: String = char::decode_utf16(units.iter().copied()).map(|r| r.unwrap_or_else(|e| esc(ESC_UNIT + e.unpaired_surrogate() as u32))).collect();
    if b.len() % 2 == 1 {
        out.push(esc(ESC_BYTE + b[b.len() - 1] as u32));
    }
    out
}

/// Inverse of `decode_text`.
pub fn encode_text(s: &str, enc: TextEnc) -> Vec<u8> {
    let mut out = vec![];
    match enc {
        TextEnc::Utf8 | TextEnc::Utf8Bom => {
            if enc == TextEnc::Utf8Bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            let mut buf = [0u8; 4];
            for c in s.chars() {
                let u = c as u32;
                if (ESC_BYTE..=ESC_BYTE + 0xFF).contains(&u) {
                    out.push((u - ESC_BYTE) as u8);
                } else {
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        TextEnc::Utf16Le | TextEnc::Utf16Be => {
            let le = enc == TextEnc::Utf16Le;
            out.extend_from_slice(if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
            let push = |u: u16, out: &mut Vec<u8>| out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
            let mut buf = [0u16; 2];
            for c in s.chars() {
                let u = c as u32;
                if (ESC_BYTE..=ESC_BYTE + 0xFF).contains(&u) {
                    out.push((u - ESC_BYTE) as u8); // a trailing odd byte
                } else if (ESC_UNIT + 0xD800..=ESC_UNIT + 0xDFFF).contains(&u) {
                    push((u - ESC_UNIT) as u16, &mut out);
                } else {
                    for &w in c.encode_utf16(&mut buf).iter() {
                        push(w, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// The on-disk name (exact case) of the file `path` names, compared the way
/// Windows does (case-insensitive). None when there is no such entry.
pub fn actual_file_name(path: &Path) -> Option<std::ffi::OsString> {
    let want = path.file_name()?;
    let parent = path.parent()?;
    let wf = fold(&want.to_string_lossy());
    let mut hit = None;
    for e in fs::read_dir(parent).ok()?.flatten() {
        let n = e.file_name();
        if n == want {
            return Some(n); // exact match wins (case-sensitive folders)
        }
        if hit.is_none() && fold(&n.to_string_lossy()) == wf {
            hit = Some(n);
        }
    }
    hit
}

/// Make the on-disk name of `path` use exactly the case `path` spells (a
/// rename through a temporary name; NTFS keeps the old case when a file is
/// replaced). No-op when it already does or the file does not exist.
pub fn set_name_case(path: &Path) -> io::Result<()> {
    let (Some(want), Some(parent)) = (path.file_name(), path.parent()) else { return Ok(()) };
    match actual_file_name(path) {
        Some(actual) if actual != want => {
            let mut t = std::ffi::OsString::from(".");
            t.push(want);
            t.push(".hsmp_case");
            let tmp = parent.join(t);
            fs::rename(parent.join(&actual), &tmp)?;
            fs::rename(&tmp, path)
        }
        _ => Ok(()),
    }
}

/// Transient Windows failures worth retrying: access denied while an
/// antivirus scans the file, sharing / lock violations.
pub fn is_transient(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33)) || e.kind() == io::ErrorKind::PermissionDenied
}

/// Run `f` up to 5 times (about 1.5 s in total) while it fails transiently.
pub fn retry_io<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut delay = std::time::Duration::from_millis(100);
    let mut attempt = 0;
    loop {
        match f() {
            Err(e) if attempt < 4 && is_transient(&e) => {
                std::thread::sleep(delay);
                delay *= 2;
                attempt += 1;
            }
            r => return r,
        }
    }
}

/// Plain-language reason for an I/O error on a backup or game file.
pub fn explain_io(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(225) => format!("{e} (your antivirus blocked this file)"),
        Some(226) => format!("{e} (your antivirus removed this file)"),
        Some(32) | Some(33) => format!("{e} (another program, often an antivirus scan, has the file open)"),
        _ if e.kind() == io::ErrorKind::NotFound => format!("{e} (deleted, or quarantined by an antivirus)"),
        _ if e.kind() == io::ErrorKind::PermissionDenied => format!("{e} (access denied: an antivirus may have locked it)"),
        _ => e.to_string(),
    }
}

/// Compare release versions like "0.3.1" / "0.3.1-beta.2" (semver order:
/// numeric parts compared as numbers, a pre-release sorts before its release).
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn parts(s: &str) -> Vec<&str> {
        s.split(['.', '-', '+']).collect()
    }
    fn cmp_part(x: &str, y: &str) -> Ordering {
        match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(p), Ok(q)) => p.cmp(&q),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => x.cmp(y),
        }
    }
    let (ac, ap) = a.trim().split_once('-').map(|(c, p)| (c, Some(p))).unwrap_or((a.trim(), None));
    let (bc, bp) = b.trim().split_once('-').map(|(c, p)| (c, Some(p))).unwrap_or((b.trim(), None));
    let (pa, pb) = (parts(ac), parts(bc));
    for i in 0..pa.len().max(pb.len()) {
        let o = cmp_part(pa.get(i).unwrap_or(&"0"), pb.get(i).unwrap_or(&"0"));
        if o != Ordering::Equal {
            return o;
        }
    }
    match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let (px, py) = (parts(x), parts(y));
            for i in 0..px.len().max(py.len()) {
                match (px.get(i), py.get(i)) {
                    (Some(p), Some(q)) => {
                        let o = cmp_part(p, q);
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (None, None) => break,
                }
            }
            Ordering::Equal
        }
    }
}

/// Read a file that may be absent: Ok(None) when it does not exist.
pub fn read_opt(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Join a forward-slash relative path onto `root`.
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

/// A forward-slash relative path is "safe" when it has no empty, `.` or `..`
/// components, no drive letters / colons, no backslashes and no leading slash.
pub fn is_safe_rel(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.contains(':') || rel.contains('\0') {
        return false;
    }
    rel.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.ends_with(' ') && !c.ends_with('.'))
}

pub fn is_safe_rel_all<'a>(mut it: impl Iterator<Item = &'a str>) -> bool {
    it.all(is_safe_rel)
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// (year, month, day, hour, minute, second) in UTC for a Unix timestamp.
pub fn civil_utc(ts: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (ts / 86_400) as i64;
    let secs = ts % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32)
}

/// "2026-10-02T13:04:05Z"
pub fn iso_utc(ts: u64) -> String {
    let (y, mo, d, h, mi, s) = civil_utc(ts);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// "20261002_130405" (sortable, file-name safe)
pub fn stamp_utc(ts: u64) -> String {
    let (y, mo, d, h, mi, s) = civil_utc(ts);
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

/// Recursively list files under `root` as sorted forward-slash relative paths.
pub fn list_files_rel(root: &Path) -> io::Result<Vec<String>> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() {
                walk(base, &p, out)?;
            } else {
                let rel = p.strip_prefix(base).unwrap_or(&p);
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    if root.is_dir() {
        walk(root, root, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// Copy a whole directory tree (files and directories).
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else {
            fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

/// Move a file or directory; falls back to copy+delete across volumes.
pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            if src.is_dir() {
                copy_tree(src, dst)?;
                fs::remove_dir_all(src)
            } else {
                fs::copy(src, dst)?;
                fs::remove_file(src)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_790_899_200), "2026-10-02T00:00:00Z");
        assert_eq!(stamp_utc(1_790_899_200 + 3661), "20261002_010101");
    }

    #[test]
    fn safe_rel_paths() {
        assert!(is_safe_rel("HalfswordUE5/Binaries/Win64/dwmapi.dll"));
        assert!(is_safe_rel("payload/Win64/ue4ss/Mods/HSMPMenu/Scripts/main.lua"));
        for bad in ["", "/abs", "a/../b", "a/./b", "a//b", "C:/x", "a\\b", "..", "a/b.", "a/b "] {
            assert!(!is_safe_rel(bad), "{bad}");
        }
    }

    #[test]
    fn text_codec_is_lossless() {
        let cases: Vec<Vec<u8>> = vec![
            b"plain\r\n".to_vec(),
            "漢字モッド : 1\r\n".as_bytes().to_vec(),
            b"\xba\xba\xd7\xd6Mod : 1\r\n".to_vec(), // GBK, not UTF-8
            b"\xef\xbb\xbfBOM : 1\n".to_vec(),
            b"\xff\xfe[\x00x\x00]\x00\r\x00\n\x00".to_vec(),
            b"\xff\xfe\x00\xd8x\x00".to_vec(), // unpaired surrogate
            b"\xff\xfeA\x00\x7f".to_vec(),     // odd trailing byte
            b"\xfe\xff\x00A\x6f\x22".to_vec(),
            vec![0x80, 0xC0, 0xE2, 0x82],       // truncated sequences
        ];
        for b in cases {
            let (s, e) = decode_text(&b);
            assert_eq!(encode_text(&s, e), b, "{s:?} {e:?}");
        }
        let (s, e) = decode_text(b"\xff\xfeA\x00=\x001\x00");
        assert_eq!((s.as_str(), e), ("A=1", TextEnc::Utf16Le));
        assert_eq!(decode_text("漢 : 1".as_bytes()).0, "漢 : 1");
    }

    #[test]
    fn versions() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_version("0.2.0", "0.10.0"), Less);
        assert_eq!(cmp_version("1.0", "1.0.0"), Equal);
        assert_eq!(cmp_version("0.3.1-beta.2", "0.3.1"), Less);
        assert_eq!(cmp_version("0.3.1-beta.10", "0.3.1-beta.2"), Greater);
        assert_eq!(cmp_version("0.3.1-beta", "0.3.1-beta.1"), Less);
        assert_eq!(cmp_version("0.4.0", "0.3.9"), Greater);
    }

    #[test]
    fn name_case_is_set_exactly() {
        let d = std::env::temp_dir().join(format!("hsmp_util_case_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("Menu_UI.lua"), b"x").unwrap();
        assert_eq!(actual_file_name(&d.join("menu_ui.lua")).unwrap(), "Menu_UI.lua");
        set_name_case(&d.join("menu_ui.lua")).unwrap();
        assert_eq!(actual_file_name(&d.join("MENU_UI.LUA")).unwrap(), "menu_ui.lua");
        assert_eq!(fs::read(d.join("menu_ui.lua")).unwrap(), b"x");
        set_name_case(&d.join("absent.lua")).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn hex_check() {
        assert!(is_sha256_hex(&sha256_hex(b"x")));
        assert!(!is_sha256_hex(&sha256_hex(b"x").to_uppercase()));
        assert!(!is_sha256_hex("abc"));
    }
}

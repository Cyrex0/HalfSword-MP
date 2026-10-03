//! Release `ue4ss/Mods/mods.txt`, rendered from `mods/mods.release.txt`.
//!
//! Same rules as scripts/build-and-deploy.ps1 (release profile):
//! * template `1` / `0` as written, `dev` -> `0`;
//! * third-party lines already in mods.txt that the template does not list
//!   are kept (order and value);
//! * an HSMP mod folder that the template does not list is written as `0`
//!   (nothing is ever switched on by accident);
//! * `Keybinds` goes last (UE4SS wants that).
//!
//! UE4SS parsing (UE4SSProgram.cpp): any line containing ';' is ignored,
//! spaces are removed, the text before ':' is the name and the first char
//! after the last ':' must be '1' to start the mod.

/// One `Name : value` template entry; value is "0", "1" or "dev".
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub value: String,
}

pub fn parse_template(text: &str) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = vec![];
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let (name, value) = t.split_once(':').ok_or_else(|| format!("mods template line {}: '{line}'", n + 1))?;
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || name.contains(char::is_whitespace) || !matches!(value, "0" | "1" | "dev") {
            return Err(format!("mods template line {}: '{line}'", n + 1));
        }
        if out.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
            return Err(format!("mods template line {}: duplicate '{name}'", n + 1));
        }
        out.push(Entry { name: name.into(), value: value.into() });
    }
    Ok(out)
}

/// Entries of an existing mods.txt, with UE4SS's own parsing rules.
pub fn parse_existing(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for line in text.trim_start_matches('\u{feff}').lines() {
        // UE4SS works on wide chars: count characters, never slice bytes
        if line.contains(';') || line.chars().count() <= 4 {
            continue;
        }
        let compact: String = line.chars().filter(|c| *c != ' ').collect();
        let Some((name, _)) = compact.split_once(':') else { continue };
        let value = compact.rsplit(':').next().unwrap_or("");
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let v = if value.starts_with('1') { "1" } else { "0" };
        if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            out.push((name.to_string(), v.to_string()));
        }
    }
    out
}

/// An HSMP mod name (case-insensitive "HSMP" prefix). Safe for any Unicode
/// name: `get(..4)` is None when byte 4 is inside a character (a CJK name).
pub fn is_hsmp(name: &str) -> bool {
    name.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("HSMP"))
}

/// Render mods.txt from the raw bytes of the current file (any encoding;
/// third-party names come back byte-for-byte). Returns the bytes to write.
pub fn render_bytes(template: &[Entry], existing: Option<&[u8]>, hsmp_dirs_on_disk: &[String], version: &str) -> Vec<u8> {
    let (text, enc) = match existing {
        Some(b) => {
            let (t, e) = crate::util::decode_text(b);
            (Some(t), e)
        }
        None => (None, crate::util::TextEnc::Utf8),
    };
    let (out, _) = render(template, text.as_deref(), hsmp_dirs_on_disk, version);
    crate::util::encode_text(&out, enc)
}

/// Render the release mods.txt. Returns (text, final table in file order).
pub fn render(template: &[Entry], existing: Option<&str>, hsmp_dirs_on_disk: &[String], version: &str) -> (String, Vec<(String, String)>) {
    let mut table: Vec<(String, String)> = vec![];
    let has = |t: &Vec<(String, String)>, n: &str| t.iter().any(|(x, _)| x.eq_ignore_ascii_case(n));
    for e in template {
        if e.name.eq_ignore_ascii_case("Keybinds") {
            continue;
        }
        table.push((e.name.clone(), if e.value == "dev" { "0".into() } else { e.value.clone() }));
    }
    for (n, v) in existing.map(parse_existing).unwrap_or_default() {
        if !has(&table, &n) && !n.eq_ignore_ascii_case("Keybinds") && !is_hsmp(&n) {
            table.push((n, v));
        }
    }
    let mut extra: Vec<&String> = hsmp_dirs_on_disk.iter().filter(|d| is_hsmp(d)).collect();
    extra.sort();
    for d in extra {
        if !has(&table, d) {
            table.push((d.clone(), "0".into()));
        }
    }
    let kb = template
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("Keybinds"))
        .map(|e| if e.value == "dev" { "0".to_string() } else { e.value.clone() })
        .unwrap_or_else(|| "1".into());
    let mut out = String::new();
    out.push_str(&format!("; Written by hsmp-launcher {version} from mods.release.txt (release profile).\r\n"));
    out.push_str("; Lines of other mods are kept. Re-run the launcher rather than editing HSMP lines.\r\n");
    for (n, v) in &table {
        out.push_str(&format!("{n} : {v}\r\n"));
    }
    out.push_str("\r\n; Built-in keybinds, do not move up!\r\n");
    out.push_str(&format!("Keybinds : {kb}\r\n"));
    table.push(("Keybinds".into(), kb));
    (out, table)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPL: &str = "# c\nBPModLoaderMod : 1\nConsoleEnablerMod : 0\nHSMPMenu : 1\nHSMPDiag : dev\nHSMPLobby : 0\nKeybinds : dev\n";

    #[test]
    fn template_parsing() {
        let t = parse_template(TPL).unwrap();
        assert_eq!(t.len(), 6);
        assert!(parse_template("A : 2").is_err());
        assert!(parse_template("A 1").is_err());
        assert!(parse_template("A : 1\na : 0").is_err());
    }

    #[test]
    fn real_template_parses() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mods/mods.release.txt");
        let t = parse_template(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert!(t.iter().any(|e| e.name == "HSMPMenu" && e.value == "1"));
    }

    #[test]
    fn render_rules() {
        let t = parse_template(TPL).unwrap();
        let existing = "ThirdParty : 1\nConsoleEnablerMod : 1\n; comment : 1\nHSMPOld : 1\nOther:0\n\nKeybinds : 1\n";
        let (text, table) = render(&t, Some(existing), &["HSMPMenu".into(), "HSMPNew".into(), "NotOurs".into()], "0.1.0");
        let m: std::collections::HashMap<_, _> = table.iter().cloned().collect();
        assert_eq!(m["BPModLoaderMod"], "1");
        assert_eq!(m["ConsoleEnablerMod"], "0", "template wins over the user's dev mod");
        assert_eq!(m["HSMPDiag"], "0", "dev -> 0 in release");
        assert_eq!(m["ThirdParty"], "1", "third-party kept");
        assert_eq!(m["Other"], "0");
        assert_eq!(m["HSMPNew"], "0", "unknown HSMP mod never enabled");
        assert!(!m.contains_key("HSMPOld"), "stale HSMP lines from an old mods.txt are dropped");
        assert_eq!(m["Keybinds"], "0");
        assert!(text.trim_end().ends_with("Keybinds : 0"));
        // UE4SS reads back exactly the table
        let back = parse_existing(&text);
        assert_eq!(back, table);
        // idempotent
        let (text2, _) = render(&t, Some(&text), &["HSMPMenu".into(), "HSMPNew".into()], "0.1.0");
        assert_eq!(text, text2);
    }

    #[test]
    fn non_ascii_names_never_panic() {
        // byte 4 falls inside a character in every one of these
        for n in ["漢字モッド", "HSM漢", "ÄÖÜmod", "日本", "🗡️sword", "H", ""] {
            assert!(!is_hsmp(n), "{n}");
        }
        assert!(is_hsmp("HSMPMenu") && is_hsmp("hsmp漢字"));
        let t = parse_template(TPL).unwrap();
        let existing = "漢字モッド : 1\r\n日本 : 1\r\nHSMP漢 : 1\r\nKeybinds : 1\r\n";
        let dirs = vec!["漢字モッド".to_string(), "HSM漢".to_string(), "hsmp漢字".to_string()];
        let (text, table) = render(&t, Some(existing), &dirs, "0.1.0");
        let m: std::collections::HashMap<_, _> = table.iter().cloned().collect();
        assert_eq!(m["漢字モッド"], "1", "{text}");
        assert_eq!(m["日本"], "1", "a 2-char name is kept (UE4SS counts characters)");
        assert_eq!(m["hsmp漢字"], "0");
        assert!(!m.contains_key("HSM漢"), "not an HSMP folder, and not in mods.txt");
        assert!(!m.contains_key("HSMP漢"), "stale HSMP line dropped");
    }

    #[test]
    fn third_party_bytes_and_encoding_survive() {
        let t = parse_template(TPL).unwrap();
        // GBK-encoded "汉字" mod name (not UTF-8) plus a UTF-8 one
        let mut cur = b"\xba\xba\xd7\xd6Mod : 1\r\n".to_vec();
        cur.extend_from_slice("漢字 : 1\r\n".as_bytes());
        let out = render_bytes(&t, Some(&cur), &[], "0.1.0");
        assert!(out.windows(9).any(|w| w == b"\xba\xba\xd7\xd6Mod :"), "GBK bytes kept verbatim");
        assert!(out.windows("漢字 : 1".len()).any(|w| w == "漢字 : 1".as_bytes()));
        // UTF-16LE with BOM stays UTF-16LE with BOM
        let mut u16 = vec![0xFF, 0xFE];
        for w in "漢字Mod : 1\r\n".encode_utf16() {
            u16.extend_from_slice(&w.to_le_bytes());
        }
        let out = render_bytes(&t, Some(&u16), &[], "0.1.0");
        assert_eq!(&out[..2], &[0xFF, 0xFE]);
        let (s, e) = crate::util::decode_text(&out);
        assert_eq!(e, crate::util::TextEnc::Utf16Le);
        assert!(s.contains("漢字Mod : 1") && s.contains("HSMPMenu : 1"), "{s}");
        // no file yet -> plain UTF-8
        assert!(String::from_utf8(render_bytes(&t, None, &[], "0.1.0")).is_ok());
    }
}

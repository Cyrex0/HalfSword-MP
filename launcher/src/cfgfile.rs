//! `hsmp.cfg` (next to the game exe; read by shared/hsmp_cfg.lua).
//!
//! The launcher owns `bin_dir` (it installs the binaries) and writes
//! `master_url` from the release manifest. A `master_url` the player changed
//! by hand is respected on updates: it is replaced only when it still holds
//! the value the launcher wrote last time. Every other line (comments,
//! other keys) is kept as is.

/// The binaries folder, relative to Binaries\Win64 (hsmp_cfg.lua's default;
/// no spaces, so it survives any game path inside os.execute command lines).
pub const BIN_DIR: &str = "hsmp";

fn key_value(line: &str) -> Option<(String, &str)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
        return None;
    }
    let (k, v) = t.split_once('=')?;
    let k = k.trim();
    if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
        return None;
    }
    Some((k.to_ascii_lowercase(), v.trim()))
}

/// Value of `key` (last occurrence wins, like hsmp_cfg.lua), trailing comment stripped.
pub fn get(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for l in text.lines() {
        if let Some((k, v)) = key_value(l) {
            if k == key {
                let v = match v.find([' ', '\t']) {
                    Some(i) if v[i..].trim_start().starts_with(['#', ';']) => v[..i].trim_end(),
                    _ => v,
                };
                found = Some(v.trim_matches('"').to_string());
            }
        }
    }
    found
}

const HEADER: &str = "# hsmp.cfg - written by hsmp-launcher";

fn fresh(version: &str, master: &str) -> String {
    [
        format!("{HEADER} {version}. Read by the HSMP mods (shared/hsmp_cfg.lua)."),
        "# bin_dir: folder with hsmp-server.exe / hsmp-sidecar.exe / hsmp-query.exe / hsmp-master.exe".into(),
        "#          (relative to this Binaries\\Win64 folder, or absolute)".into(),
        format!("bin_dir = {BIN_DIR}"),
        "# master_url: server-browser master(s), primary first, comma-separated.".into(),
        "# The launcher keeps a value you changed by hand.".into(),
        format!("master_url = {master}"),
        String::new(),
    ]
    .join("\r\n")
}

/// Merge result: new text plus the master_url value now in the file.
pub struct Merged {
    pub text: String,
    pub master_url: String,
    pub kept_user_master: bool,
}

/// `prev_written_master`: what the launcher wrote last time (None on first install).
pub fn merge(existing: Option<&str>, version: &str, master_urls: &[String], prev_written_master: Option<&str>) -> Merged {
    let want_master = if master_urls.is_empty() { "http://127.0.0.1:7778".to_string() } else { master_urls.join(", ") };
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else {
        return Merged { text: fresh(version, &want_master), master_url: want_master, kept_user_master: false };
    };
    let eol = crate::ini::eol_of(text);
    let cur_master = get(text, "master_url");
    // keep the player's value when they changed it after our last write
    let keep_user = match (&cur_master, prev_written_master) {
        (Some(cur), Some(prev)) => cur.trim() != prev.trim() && !cur.trim().is_empty(),
        _ => false,
    };
    let master_value = if keep_user { cur_master.clone().unwrap() } else { want_master.clone() };
    let mut out: Vec<String> = vec![];
    let mut wrote_bin = false;
    let mut wrote_master = false;
    for l in text.split_inclusive('\n') {
        let body = l.trim_end_matches(['\r', '\n']);
        match key_value(body) {
            Some((k, _)) if k == "bin_dir" => {
                if !wrote_bin {
                    out.push(format!("bin_dir = {BIN_DIR}{eol}"));
                    wrote_bin = true;
                }
            }
            Some((k, _)) if k == "master_url" => {
                if !wrote_master {
                    out.push(format!("master_url = {master_value}{eol}"));
                    wrote_master = true;
                }
            }
            _ if body.starts_with(HEADER) => out.push(format!("{HEADER} {version}. Read by the HSMP mods (shared/hsmp_cfg.lua).{eol}")),
            _ => out.push(if l.ends_with('\n') { l.to_string() } else { format!("{l}{eol}") }),
        }
    }
    if !wrote_bin {
        out.push(format!("bin_dir = {BIN_DIR}{eol}"));
    }
    if !wrote_master {
        out.push(format!("master_url = {master_value}{eol}"));
    }
    Merged { text: out.concat(), master_url: master_value, kept_user_master: keep_user }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_file() {
        let m = merge(None, "0.1.0", &["https://a.example".into(), "http://b:7778".into()], None);
        assert_eq!(get(&m.text, "bin_dir").as_deref(), Some("hsmp"));
        assert_eq!(get(&m.text, "master_url").as_deref(), Some("https://a.example, http://b:7778"));
        assert!(!m.kept_user_master);
    }

    #[test]
    fn merge_dev_cfg_first_install() {
        let dev = "# dev\r\nbin_dir = C:/src/hsmp/target/release\r\nmaster_url = http://127.0.0.1:7778\r\nlan_ports = 7777-7786\r\n";
        let m = merge(Some(dev), "0.1.0", &["https://m.example".into()], None);
        assert_eq!(m.text, "# dev\r\nbin_dir = hsmp\r\nmaster_url = https://m.example\r\nlan_ports = 7777-7786\r\n");
    }

    #[test]
    fn user_master_kept_on_update() {
        let ours = merge(None, "0.1.0", &["https://m.example".into()], None);
        // player edits master_url by hand
        let edited = ours.text.replace("master_url = https://m.example", "master_url = http://my.server:7778 # mine");
        let m = merge(Some(&edited), "0.2.0", &["https://m2.example".into()], Some(&ours.master_url));
        assert!(m.kept_user_master);
        assert_eq!(get(&m.text, "master_url").as_deref(), Some("http://my.server:7778"));
        // untouched value follows the new release
        let m = merge(Some(&ours.text), "0.2.0", &["https://m2.example".into()], Some(&ours.master_url));
        assert!(!m.kept_user_master);
        assert_eq!(get(&m.text, "master_url").as_deref(), Some("https://m2.example"));
    }

    #[test]
    fn duplicates_collapse_and_missing_keys_added() {
        let t = "bin_dir = a\nbin_dir = b\nfoo = 1";
        let m = merge(Some(t), "0.1.0", &[], None);
        assert_eq!(m.text, "bin_dir = hsmp\nfoo = 1\nmaster_url = http://127.0.0.1:7778\n");
    }
}

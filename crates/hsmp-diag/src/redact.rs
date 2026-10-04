//! Removing personal data from log text before it leaves the machine (bug reports from the
//! launcher and from `hsmp-server --report`).

/// Which IP addresses a [`Redactor`] removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpMode {
    /// Every IPv4 / IPv6 address (and its port) becomes `<ip>`.
    All,
    /// Public addresses become `<ip-1>`, `<ip-2>`... (the same address keeps its number, the
    /// port is kept); private, loopback and link-local ones stay. Bug-report default.
    Public,
    /// Only the player's own public address (`own_ips`) is removed.
    OwnOnly,
}

/// Removes personal data from log text before it leaves the machine:
///
/// * the Windows user name and home folder (`C:\Users\<name>\...` of any user), the
///   computer name, e-mail addresses and Steam ids,
/// * values of secret-looking keys (`password`, `secret`, `token`, `player_key`,
///   `server_key`, `admin_key`, `webhook`, `--rcon-password <x>`, `Bearer <x>`...) and the
///   exact strings in `secrets` (this install's player key),
/// * player names (`nick=`, `name=`, `"nick":"..."`, `nick: "..."`),
/// * IP addresses as `ip` says; `own_ips` (the player's public address) always.
#[derive(Debug, Default)]
pub struct Redactor {
    pub user: Option<String>,
    pub home: Option<String>,
    pub computer: Option<String>,
    pub secrets: Vec<String>,
    pub own_ips: Vec<String>,
    pub ip: Option<IpMode>,
    /// The `<ip-N>` numbering, shared by every file one redactor handles.
    pub numbers: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

fn re(s: &str) -> regex::Regex {
    regex::Regex::new(s).expect("redaction regex")
}

/// User or computer names that are also ordinary log words: only their paths are redacted.
const COMMON_WORDS: &[&str] = &["admin", "administrator", "user", "owner", "player", "guest", "test", "game", "host", "server", "client", "default", "public", "steam", "windows", "home", "desktop", "local"];

const SECRET_NAMES: &str = r"password|passwd|passphrase|secret|token|api_?key|auth_?key|authorization|private_?key|player_?key|server_?key|owner_?key|admin_?key|webhook(?:_url)?|cookie";

struct Patterns {
    users: regex::Regex,
    kv_debug: regex::Regex,
    kv: regex::Regex,
    flag: regex::Regex,
    bearer: regex::Regex,
    email: regex::Regex,
    steam: regex::Regex,
    nick_debug: regex::Regex,
    v6: regex::Regex,
}

fn patterns() -> &'static Patterns {
    static P: std::sync::OnceLock<Patterns> = std::sync::OnceLock::new();
    P.get_or_init(|| Patterns {
        // C:\Users\<name>\ and /Users/<name>/ (forward slashes from Lua, "\\" in JSON)
        users: re(r"(?i)([\\/]{1,2}Users[\\/]{1,2})([^\\/:*?<>|\r\n]+)"),
        kv_debug: re(&format!(r#"(?i)\b([\w\-]*(?:{SECRET_NAMES})[\w\-]*)(\s*:\s*)Some\("[^"]*"\)"#)),
        kv: re(&format!(r#"(?i)\b([\w\-]*(?:{SECRET_NAMES})[\w\-]*)("?\s*[=:]\s*"?)([^\s",;}}&'\]\)]+)"#)),
        flag: re(&format!(r#"(?i)(--[\w\-]*(?:{SECRET_NAMES}|password|key)[\w\-]*)(\s+|=)("[^"]*"|\S+)"#)),
        bearer: re(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=\-]{8,}"),
        email: re(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}"),
        steam: re(r"\b7656119\d{10}\b"),
        nick_debug: re(r#"(?i)\b(nick|name)(\s*[:=]\s*)"[^"]*""#),
        // an address token, not part of a word or a Rust path (`std::fs`; the closure checks
        // the character after it)
        v6: re(r"(?i)(^|[^0-9a-z_:.\-])([0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,7}(?:%[\w.]+)?)"),
    })
}

fn v4_public(a: std::net::Ipv4Addr) -> bool {
    let o = a.octets();
    !(a.is_private() || a.is_loopback() || a.is_link_local() || a.is_unspecified() || a.is_broadcast() || a.is_multicast() || (o[0] == 100 && (64..128).contains(&o[1])))
}

fn v6_public(a: std::net::Ipv6Addr) -> bool {
    let s = a.segments();
    if let Some(v4) = a.to_ipv4_mapped() {
        return v4_public(v4);
    }
    !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xfe00) == 0xfc00)
}

impl Redactor {
    /// This machine's user name, home folder and computer name; every IP address removed.
    pub fn system() -> Redactor {
        let env = |k: &str| std::env::var(k).ok().filter(|v| v.len() >= 2);
        Redactor { user: env("USERNAME"), home: env("USERPROFILE"), computer: env("COMPUTERNAME"), ip: Some(IpMode::All), ..Default::default() }
    }

    pub fn with_ips(mut self, mode: IpMode, own: Vec<String>) -> Redactor {
        self.ip = Some(mode);
        self.own_ips = own;
        self
    }

    fn ip_token(&self, addr: &str, public: bool) -> Option<String> {
        if self.own_ips.iter().any(|o| o.eq_ignore_ascii_case(addr)) {
            return Some("<my-ip>".into());
        }
        match self.ip.unwrap_or(IpMode::All) {
            IpMode::All => Some("<ip>".into()),
            IpMode::Public if public => {
                let mut m = self.numbers.lock().unwrap_or_else(|e| e.into_inner());
                let n = m.len() + 1;
                let k = *m.entry(addr.to_ascii_lowercase()).or_insert(n);
                Some(format!("<ip-{k}>"))
            }
            _ => None,
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let p = patterns();
        let mut s = text.to_string();
        for sec in self.secrets.iter().filter(|x| x.len() >= 8) {
            s = replace_ci(&s, sec, "<secret>");
        }
        if let Some(h) = &self.home {
            for form in [h.replace('\\', "\\\\"), h.clone(), h.replace('\\', "/")] {
                s = replace_ci(&s, &form, "%USERPROFILE%");
            }
        }
        s = p
            .users
            .replace_all(&s, |c: &regex::Captures| {
                let n = c[2].to_ascii_lowercase();
                if ["public", "default", "all users", "<user>"].contains(&n.as_str()) {
                    c[0].to_string()
                } else {
                    format!("{}<user>", &c[1])
                }
            })
            .into_owned();
        s = p.bearer.replace_all(&s, "$1 <redacted>").into_owned();
        s = p.kv_debug.replace_all(&s, "$1$2<redacted>").into_owned();
        s = p.flag.replace_all(&s, "$1$2<redacted>").into_owned();
        s = p.kv.replace_all(&s, "$1$2<redacted>").into_owned();
        s = p.email.replace_all(&s, "<email>").into_owned();
        s = p.steam.replace_all(&s, "<steam-id>").into_owned();
        s = p.nick_debug.replace_all(&s, "$1$2\"<redacted>\"").into_owned();
        s = self.scan(&s);
        s = p
            .v6
            .replace_all(&s, |c: &regex::Captures| {
                let whole = &c[0];
                let a = &c[2];
                let next = s[c.get(0).map_or(0, |m| m.end())..].chars().next();
                let plausible = a.matches(':').count() >= 2 && a.chars().any(|ch| ch.is_ascii_hexdigit()) && !next.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
                match a.split('%').next().unwrap_or(a).parse::<std::net::Ipv6Addr>() {
                    Ok(ip) if plausible => match self.ip_token(&ip.to_string(), v6_public(ip)) {
                        Some(t) => whole.replace(a, &t),
                        None => whole.to_string(),
                    },
                    _ => whole.to_string(),
                }
            })
            .into_owned();
        for name in [&self.computer, &self.user].into_iter().flatten().filter(|n| n.len() >= 3 && !COMMON_WORDS.contains(&n.to_ascii_lowercase().as_str())) {
            let r = re(&format!(r"(?i)\b{}\b", regex::escape(name)));
            s = r.replace_all(&s, "<user>").into_owned();
        }
        s
    }

    /// `nick=` / `name=` / `"nick":"` values and IPv4 addresses, in one pass.
    fn scan(&self, text: &str) -> String {
        let mut s = String::with_capacity(text.len());
        let b = text.as_bytes();
        let mut i = 0;
        let drop_port = self.ip.unwrap_or(IpMode::All) == IpMode::All;
        while i < b.len() {
            let rest = &text[i..];
            // `get` (not `[..n]`): byte n may fall inside a non-ASCII character
            let lower_starts = |p: &str| rest.get(..p.len()).is_some_and(|x| x.eq_ignore_ascii_case(p));
            let word_start = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            if word_start && (lower_starts("nick=") || lower_starts("\"nick\":\"") || lower_starts("name=")) {
                let key_len = if lower_starts("nick=") || lower_starts("name=") { 5 } else { 8 };
                s.push_str(&rest[..key_len]);
                s.push_str("<redacted>");
                let mut j = i + key_len;
                while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\r' | b'\n' | b',' | b'"' | b'}' | b';') {
                    j += 1;
                }
                i = j;
                continue;
            }
            // IPv4: four dot-separated 1-3 digit groups (optional :port)
            if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.')) {
                let mut j = i;
                let mut groups = 0;
                loop {
                    let start = j;
                    while j < b.len() && b[j].is_ascii_digit() && j - start < 3 {
                        j += 1;
                    }
                    if j == start {
                        break;
                    }
                    groups += 1;
                    if groups == 4 || j >= b.len() || b[j] != b'.' {
                        break;
                    }
                    j += 1;
                }
                if groups == 4 && (j >= b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'.')) {
                    let addr = &text[i..j];
                    if let Some(t) = addr.parse::<std::net::Ipv4Addr>().ok().and_then(|ip| self.ip_token(addr, v4_public(ip))) {
                        if drop_port && j < b.len() && b[j] == b':' {
                            let mut k = j + 1;
                            while k < b.len() && b[k].is_ascii_digit() {
                                k += 1;
                            }
                            if k > j + 1 {
                                j = k;
                            }
                        }
                        s.push_str(&t);
                        i = j;
                        continue;
                    }
                    // not removed: copy the whole address (its groups are not restarted)
                    s.push_str(addr);
                    i = j;
                    continue;
                }
            }
            let ch = rest.chars().next().unwrap();
            s.push(ch);
            i += ch.len_utf8();
        }
        s
    }
}

/// Case-insensitive replace of every occurrence of `pat`.
fn replace_ci(s: &str, pat: &str, with: &str) -> String {
    if pat.is_empty() {
        return s.to_string();
    }
    re(&format!("(?i){}", regex::escape(pat))).replace_all(s, regex::NoExpand(with)).into_owned()
}


//! The read-only HTML page at `/` (no scripts, every value escaped).

use crate::registry::Entry;

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

pub fn render(servers: &[Entry]) -> String {
    let rows = if servers.is_empty() {
        r#"<tr><td colspan="7" class="none">no servers listed</td></tr>"#.to_string()
    } else {
        servers
            .iter()
            .map(|s| {
                let host = if s.host.contains(':') { format!("[{}]", s.host) } else { s.host.clone() };
                format!(
                    "<tr><td><b>{}</b></td><td>{}</td><td>{}</td><td><code>{}:{}</code></td><td>{}/{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&s.name), escape(&s.mode), escape(&s.map), escape(&host), s.port,
                    s.players, s.max_players, escape(&s.region), escape(&s.version)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Half Sword MP servers</title>
<meta http-equiv="refresh" content="30">
<style>
:root {{ color-scheme: dark; }}
body {{ font-family: system-ui, sans-serif; background:#111; color:#ddd; margin:0; padding:16px; }}
h1 {{ font-size:20px; margin:0 0 4px; }}
.sub {{ color:#888; font-size:12px; margin-bottom:16px; }}
.wrap {{ overflow-x:auto; }}
table {{ border-collapse:collapse; width:100%; max-width:1100px; }}
th, td {{ text-align:left; padding:8px 10px; border-bottom:1px solid #222; font-size:14px; white-space:nowrap; }}
th {{ color:#888; font-weight:500; font-size:11px; text-transform:uppercase; }}
code {{ color:#8fd; }}
.none {{ text-align:center; color:#888; padding:24px; }}
a {{ color:#8af; }}
</style></head>
<body>
<h1>Half Sword MP servers</h1>
<div class="sub">{n} listed, refreshes every 30 s. Join from the game's server browser. <a href="/v1/servers">JSON</a></div>
<div class="wrap"><table>
<thead><tr><th>name</th><th>mode</th><th>map</th><th>address</th><th>players</th><th>region</th><th>version</th></tr></thead>
<tbody>
{rows}
</tbody></table></div>
</body></html>
"#,
        n = servers.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_escaped() {
        let e = Entry {
            server_id: "a".into(), name: "<script>alert(1)</script>".into(), host: "2001:db8::1".into(), port: 7777,
            mode: "\"m\"".into(), map: "Map".into(), players: 1, max_players: 8, proto_ver: 5, proto_min: 5, proto_max: 5,
            server_key: String::new(), pwd_protected: false, version: "1".into(), region: "&".into(), content_hash: String::new(), ping_ms: 0,
            reachable: false, last_seen_utc_ms: 0, age_s: 0, nat: String::new(), punch: false,
        };
        let h = render(&[e]);
        assert!(!h.contains("<script>"));
        assert!(h.contains("&lt;script&gt;") && h.contains("&quot;m&quot;") && h.contains("&amp;"));
        assert!(h.contains("[2001:db8::1]:7777"));
        assert!(render(&[]).contains("no servers listed"));
    }
}

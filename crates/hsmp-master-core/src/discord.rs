//! Discord webhook messages for server up / down announcements.

use crate::registry::Announcement;

/// Only real Discord webhook URLs are posted to.
pub fn webhook_url_ok(u: &str) -> bool {
    ["https://discord.com/api/webhooks/", "https://discordapp.com/api/webhooks/", "https://canary.discord.com/api/webhooks/"]
        .iter()
        .any(|p| u.starts_with(p))
        && u.len() < 300
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Text shown inside an inline code span: no backticks (they would end it), no line breaks.
/// Inside a code span Discord renders no markdown, links or mentions.
fn code(s: &str) -> String {
    let t: String = s.chars().map(|c| if c == '`' { '\'' } else if c.is_control() { ' ' } else { c }).collect();
    let t = t.trim();
    if t.is_empty() { "-".into() } else { format!("`{t}`") }
}

/// The JSON body to POST to the webhook. Mentions are disabled whatever the text says.
pub fn webhook_body(a: &Announcement, show_addr: bool) -> String {
    let mut line = if a.up {
        format!("**Server up:** {}  |  {} on {}  |  {}/{} players", code(&a.name), code(&a.mode), code(&a.map), a.players, a.max_players)
    } else {
        format!("**Server down:** {}", code(&a.name))
    };
    if a.up && !a.region.is_empty() {
        line.push_str(&format!("  |  region {}", code(&a.region)));
    }
    if a.up && !a.version.is_empty() {
        line.push_str(&format!("  |  v{}", code(&a.version)));
    }
    if a.up && show_addr {
        line.push_str(&format!("\nJoin: {}", code(&a.addr)));
    }
    serde_json::json!({
        "username": "Half Sword MP",
        "content": line,
        "allowed_mentions": { "parse": [] },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(up: bool, name: &str) -> Announcement {
        Announcement {
            up, name: name.into(), mode: "Best of 3".into(), map: "Map_Arena_Pit".into(), players: 1, max_players: 8,
            region: "EU".into(), version: "0.2.0".into(), addr: "203.0.113.5:7777".into(),
        }
    }

    #[test]
    fn messages_neutralise_markdown_and_mentions() {
        let v: serde_json::Value = serde_json::from_str(&webhook_body(&ann(true, "@everyone `x` **b** https://evil"), true)).unwrap();
        let c = v["content"].as_str().unwrap();
        assert!(c.contains("`@everyone 'x' **b** https://evil`"), "{c}");
        assert!(c.contains("Join: `203.0.113.5:7777`"));
        assert_eq!(v["allowed_mentions"]["parse"], serde_json::json!([]));
        let v: serde_json::Value = serde_json::from_str(&webhook_body(&ann(true, "x"), false)).unwrap();
        assert!(!v["content"].as_str().unwrap().contains("203.0.113.5"));
        let v: serde_json::Value = serde_json::from_str(&webhook_body(&ann(false, "x\ny"), true)).unwrap();
        assert_eq!(v["content"], "**Server down:** `x y`");
    }

    #[test]
    fn only_discord_webhooks() {
        assert!(webhook_url_ok("https://discord.com/api/webhooks/1/abc"));
        assert!(!webhook_url_ok("http://discord.com/api/webhooks/1/abc"));
        assert!(!webhook_url_ok("https://discord.com.evil.net/api/webhooks/1"));
        assert!(!webhook_url_ok("https://example.com/"));
    }
}

//! Small shared helpers: time, JSON event access, map-name normalisation, paths, PIDs.

use serde_json::{Map, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// One normalised event (a JSON object).
pub type Ev = Map<String, Value>;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// "YYYY-MM-DDTHH:MM:SS" + optional ".ffffff" -> epoch ms (UTC).
pub fn iso_to_ms(s: &str) -> Option<i64> {
    let re = regex::Regex::new(r"^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(\.\d+)?").ok()?;
    let c = re.captures(s)?;
    let n = |i: usize| c.get(i).unwrap().as_str().parse::<i64>().unwrap_or(0);
    let days = days_from_civil(n(1), n(2), n(3));
    let mut ms = ((days * 24 + n(4)) * 60 + n(5)) * 60 * 1000 + n(6) * 1000;
    if let Some(f) = c.get(7) {
        let digits = &f.as_str()[1..];
        let mut v: i64 = 0;
        for (k, ch) in digits.chars().take(3).enumerate() {
            v += (ch as i64 - '0' as i64) * 10_i64.pow(2 - k as u32);
        }
        ms += v;
    }
    Some(ms)
}

/// epoch ms -> "YYYY-MM-DDTHH:MM:SS.mmmZ"
pub fn ms_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    // civil_from_days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, m, d, sod / 3600, (sod / 60) % 60, sod % 60, frac)
}

pub fn s<'a>(e: &'a Ev, k: &str) -> Option<&'a str> {
    e.get(k).and_then(|v| v.as_str())
}

/// Field as a display string (numbers and bools included); None for missing/null.
pub fn sv(e: &Ev, k: &str) -> Option<String> {
    match e.get(k) {
        None | Some(Value::Null) => None,
        Some(Value::String(x)) => Some(x.clone()),
        Some(v) => Some(v.to_string()),
    }
}

pub fn f64v(e: &Ev, k: &str) -> Option<f64> {
    match e.get(k) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(x)) => x.parse().ok(),
        _ => None,
    }
}

pub fn i64v(e: &Ev, k: &str) -> Option<i64> {
    match e.get(k) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(x)) => x.parse().ok(),
        _ => None,
    }
}

pub fn bv(e: &Ev, k: &str) -> Option<bool> {
    e.get(k).and_then(|v| v.as_bool())
}

pub fn ev_name(e: &Ev) -> &str {
    s(e, "ev").unwrap_or("")
}
pub fn inst(e: &Ev) -> &str {
    s(e, "inst").unwrap_or("?")
}
pub fn src(e: &Ev) -> &str {
    s(e, "src").unwrap_or("")
}
pub fn wall(e: &Ev) -> i64 {
    i64v(e, "wall_ms").unwrap_or(0)
}

/// "Map_Arena_LordsHall", "/Game/Maps/Map_Arena_Alley.Map_Arena_Alley", "Lords Hall" -> "lordshall"
pub fn norm_map(v: Option<&str>) -> String {
    let Some(s) = v else { return String::new() };
    let last = s.rsplit('/').next().unwrap_or(s);
    let last = last.rsplit('.').next().unwrap_or(last);
    let last = last.rsplit(':').next().unwrap_or(last);
    let lower = last.to_lowercase();
    let stripped = lower.strip_prefix("map_arena_").unwrap_or(&lower);
    stripped.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

pub fn is_menu_world(v: Option<&str>) -> bool {
    let n = norm_map(v);
    n.contains("tavern") || n.starts_with("hub") || n.contains("mapmenu") || n.starts_with("menu")
        || n.contains("startup") || n.contains("splash")
}

pub fn read_text(p: &Path) -> Option<String> {
    let b = std::fs::read(p).ok()?;
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&b);
    Some(String::from_utf8_lossy(b).into_owned())
}

pub fn read_json(p: &Path) -> Option<Value> {
    let t = read_text(p)?;
    serde_json::from_str(&t).ok()
}

pub fn write_json(p: &Path, v: &Value) -> anyhow::Result<()> {
    std::fs::write(p, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

pub fn append_line(p: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")
}

/// Is process `pid` alive? (0 = "no parent given" = alive). Uses tasklist so the
/// gate needs no extra windows-sys features; callers poll it every few seconds.
#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    match std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
        Err(_) => true,
    }
}

#[cfg(not(windows))]
pub fn pid_alive(pid: u32) -> bool {
    pid == 0 || Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iso_roundtrip() {
        let ms = iso_to_ms("2026-10-01T20:52:00.857161Z").unwrap();
        assert_eq!(ms_to_iso(ms), "2026-10-01T20:52:00.857Z");
        assert_eq!(iso_to_ms("1970-01-01T00:00:01").unwrap(), 1000);
    }
    #[test]
    fn maps() {
        assert_eq!(norm_map(Some("Map_Arena_LordsHall")), "lordshall");
        assert_eq!(norm_map(Some("Lords Hall")), "lordshall");
        assert_eq!(norm_map(Some("/Game/Maps/Map_Arena_Alley.Map_Arena_Alley")), "alley");
        assert!(is_menu_world(Some("Hub_Tavern")));
        assert!(is_menu_world(Some("Map_Menu_Startup")));
        assert!(!is_menu_world(Some("Map_Arena_Pit")));
    }
}

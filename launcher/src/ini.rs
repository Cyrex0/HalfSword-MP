//! Byte-preserving ini editing.
//!
//! * `set_key`: release-time overrides in UE4SS-settings.ini (keeps the
//!   file's `Key = value` style, comments and line endings).
//! * `apply_setting` / `revert_setting`: the journaled Engine.ini merge.
//!   `apply_setting` returns an `IniEdit` that says exactly what was done,
//!   so `revert_setting` can undo it: byte-identical when nobody touched the
//!   file in between, and a minimal line-level undo when the game rewrote it.

use serde::{Deserialize, Serialize};

pub fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") || !text.contains('\n') {
        "\r\n"
    } else {
        "\n"
    }
}

struct Line<'a> {
    start: usize,
    /// end including the line terminator
    end: usize,
    content: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = vec![];
    let mut start = 0;
    while start < text.len() {
        let rest = &text[start..];
        let (len, content_len) = match rest.find('\n') {
            Some(i) => (i + 1, if i > 0 && rest.as_bytes()[i - 1] == b'\r' { i - 1 } else { i }),
            None => (rest.len(), rest.len()),
        };
        out.push(Line { start, end: start + len, content: &rest[..content_len] });
        start += len;
    }
    out
}

fn section_of(content: &str) -> Option<&str> {
    let t = content.trim();
    if t.starts_with('[') && t.ends_with(']') && t.len() >= 2 {
        Some(t[1..t.len() - 1].trim())
    } else {
        None
    }
}

fn key_of(content: &str) -> Option<&str> {
    let t = content.trim_start();
    if t.starts_with(';') || t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    let k = k.trim().trim_start_matches(['+', '-', '.', '!']);
    if k.is_empty() {
        None
    } else {
        Some(k)
    }
}

/// Indices of lines inside `[section]` (all occurrences of the section),
/// excluding the header lines. Also returns the header line indices.
fn section_lines(ls: &[Line<'_>], section: &str) -> (Vec<usize>, Vec<usize>) {
    let mut body = vec![];
    let mut headers = vec![];
    let mut inside = false;
    for (i, l) in ls.iter().enumerate() {
        if let Some(s) = section_of(l.content) {
            inside = s.eq_ignore_ascii_case(section);
            if inside {
                headers.push(i);
            }
        } else if inside {
            body.push(i);
        }
    }
    (body, headers)
}

/// Set `key` in `[section]` to `value`, keeping the existing line's
/// `Key = ` prefix. Inserts after the section's last key line when absent;
/// appends the section when missing.
pub fn set_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let eol = eol_of(text);
    let ls = lines(text);
    let (body, headers) = section_lines(&ls, section);
    if let Some(&i) = body.iter().find(|&&i| key_of(ls[i].content).map(|k| k.eq_ignore_ascii_case(key)).unwrap_or(false)) {
        let l = &ls[i];
        let eq = l.content.find('=').unwrap();
        let after = &l.content[eq + 1..];
        let pad: String = after.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let new_line = format!("{}{}{}", &l.content[..=eq], pad, value);
        return format!("{}{}{}", &text[..l.start], new_line, &text[l.start + l.content.len()..]);
    }
    if let Some(&h) = headers.first() {
        let last_key = body.iter().copied().filter(|&i| i > h && key_of(ls[i].content).is_some()).take_while(|&i| !headers.iter().any(|&x| x > h && x < i)).last();
        let anchor = last_key.unwrap_or(h);
        let l = &ls[anchor];
        let mut out = String::from(&text[..l.end]);
        if l.end == l.start + l.content.len() {
            out.push_str(eol); // last line had no terminator
        }
        out.push_str(&format!("{key} = {value}{eol}"));
        out.push_str(&text[l.end..]);
        return out;
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&format!("[{section}]{eol}{key} = {value}{eol}"));
    out
}

/// Value of `key` in `[section]` (last occurrence), if any.
pub fn get_key(text: &str, section: &str, key: &str) -> Option<String> {
    let ls = lines(text);
    let (body, _) = section_lines(&ls, section);
    body.iter()
        .rev()
        .find(|&&i| key_of(ls[i].content).map(|k| k.eq_ignore_ascii_case(key)).unwrap_or(false))
        .map(|&i| ls[i].content.split_once('=').map(|(_, v)| v.trim().to_string()).unwrap_or_default())
}

/// What `apply_setting` did (journaled; drives `revert_setting`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IniEdit {
    /// The setting was already there with the wanted value: nothing to undo.
    AlreadyPresent,
    /// The file did not exist; we created it with exactly `content`.
    CreatedFile { section: String, line: String, content: String },
    /// The section did not exist; we appended exactly `suffix`.
    AppendedSection { section: String, line: String, suffix: String },
    /// We inserted `line` right after the `[section]` header.
    InsertedLine { section: String, line: String },
    /// The section header was the last line without a newline; we appended exactly `suffix`.
    AppendedLine { section: String, line: String, suffix: String },
    /// We replaced `original` with `line`.
    ReplacedLine { section: String, original: String, line: String },
}

/// Result of a revert: what to do with the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Revert {
    Keep,
    Write(String),
    Delete,
}

/// Merge `key=value` into `[section]`. Returns the new text (None = no change).
pub fn apply_setting(text: Option<&str>, section: &str, key: &str, value: &str) -> (Option<String>, IniEdit) {
    let line = format!("{key}={value}");
    let Some(text) = text else {
        let content = format!("[{section}]\r\n{line}\r\n");
        return (Some(content.clone()), IniEdit::CreatedFile { section: section.into(), line, content });
    };
    let eol = eol_of(text);
    let ls = lines(text);
    let (body, headers) = section_lines(&ls, section);
    let hits: Vec<usize> = body.iter().copied().filter(|&i| key_of(ls[i].content).map(|k| k.eq_ignore_ascii_case(key)).unwrap_or(false)).collect();
    if let Some(&last) = hits.last() {
        let l = &ls[last];
        let cur = l.content.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
        if cur == value {
            return (None, IniEdit::AlreadyPresent);
        }
        let new_text = format!("{}{}{}", &text[..l.start], line, &text[l.start + l.content.len()..]);
        return (Some(new_text), IniEdit::ReplacedLine { section: section.into(), original: l.content.to_string(), line });
    }
    if let Some(&h) = headers.first() {
        let l = &ls[h];
        if l.end == l.start + l.content.len() {
            // the header is the last line and has no terminator: a pure suffix
            let suffix = format!("{eol}{line}{eol}");
            return (Some(format!("{text}{suffix}")), IniEdit::AppendedLine { section: section.into(), line, suffix });
        }
        let mut out = String::from(&text[..l.end]);
        out.push_str(&line);
        out.push_str(eol);
        out.push_str(&text[l.end..]);
        return (Some(out), IniEdit::InsertedLine { section: section.into(), line });
    }
    let mut suffix = String::new();
    if !text.is_empty() && !text.ends_with('\n') {
        suffix.push_str(eol);
    }
    if !text.trim().is_empty() {
        suffix.push_str(eol);
    }
    suffix.push_str(&format!("[{section}]{eol}{line}{eol}"));
    (Some(format!("{text}{suffix}")), IniEdit::AppendedSection { section: section.into(), line, suffix })
}

/// Remove the first line equal to `line` (trimmed) inside `[section]`; when
/// `drop_empty_section`, also drop the header if the section has no other
/// non-blank lines left.
fn remove_line(text: &str, section: &str, line: &str, drop_empty_section: bool) -> Option<String> {
    let ls = lines(text);
    let (body, headers) = section_lines(&ls, section);
    let i = *body.iter().find(|&&i| ls[i].content.trim() == line)?;
    let mut out = format!("{}{}", &text[..ls[i].start], &text[ls[i].end..]);
    if drop_empty_section {
        let ls2 = lines(&out);
        let (body2, headers2) = section_lines(&ls2, section);
        if headers2.len() == 1 && body2.iter().all(|&j| ls2[j].content.trim().is_empty()) {
            let h = &ls2[headers2[0]];
            // also drop one blank separator line right before the header
            let mut start = h.start;
            if headers2[0] > 0 && ls2[headers2[0] - 1].content.trim().is_empty() {
                start = ls2[headers2[0] - 1].start;
            }
            let end = body2.iter().map(|&j| ls2[j].end).max().unwrap_or(h.end).max(h.end);
            out = format!("{}{}", &out[..start], &out[end..]);
        }
    }
    let _ = headers;
    Some(out)
}

/// Undo an `IniEdit` against the file's current text.
pub fn revert_setting(text: Option<&str>, edit: &IniEdit) -> Revert {
    let Some(text) = text else { return Revert::Keep };
    match edit {
        IniEdit::AlreadyPresent => Revert::Keep,
        IniEdit::CreatedFile { section, line, content } => {
            if text == content {
                return Revert::Delete;
            }
            match remove_line(text, section, line, true) {
                Some(t) if t.trim().is_empty() => Revert::Delete,
                Some(t) => Revert::Write(t),
                None => Revert::Keep,
            }
        }
        IniEdit::AppendedSection { section, line, suffix } => {
            if let Some(orig) = text.strip_suffix(suffix.as_str()) {
                return Revert::Write(orig.to_string());
            }
            match remove_line(text, section, line, true) {
                Some(t) => Revert::Write(t),
                None => Revert::Keep,
            }
        }
        IniEdit::AppendedLine { section, line, suffix } => match text.strip_suffix(suffix.as_str()) {
            Some(orig) => Revert::Write(orig.to_string()),
            None => match remove_line(text, section, line, false) {
                Some(t) => Revert::Write(t),
                None => Revert::Keep,
            },
        },
        IniEdit::InsertedLine { section, line } => match remove_line(text, section, line, false) {
            Some(t) => Revert::Write(t),
            None => Revert::Keep,
        },
        IniEdit::ReplacedLine { section, original, line } => {
            let ls = lines(text);
            let (body, _) = section_lines(&ls, section);
            match body.iter().find(|&&i| ls[i].content.trim() == line) {
                Some(&i) => {
                    let l = &ls[i];
                    Revert::Write(format!("{}{}{}", &text[..l.start], original, &text[l.start + l.content.len()..]))
                }
                None => Revert::Keep,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "SystemSettings";
    const K: &str = "r.HairStrands.Streaming";

    fn roundtrip(orig: Option<&str>) -> (Option<String>, IniEdit) {
        let (new, edit) = apply_setting(orig, S, K, "0");
        let after = new.clone().or(orig.map(str::to_string));
        assert_eq!(get_key(after.as_deref().unwrap(), S, K).as_deref(), Some("0"), "{after:?}");
        match revert_setting(after.as_deref(), &edit) {
            Revert::Keep => assert_eq!(after.as_deref(), orig),
            Revert::Delete => assert!(orig.is_none()),
            Revert::Write(t) => assert_eq!(Some(t.as_str()), orig, "byte-identical revert for {orig:?}"),
        }
        (new, edit)
    }

    #[test]
    fn apply_revert_byte_identical() {
        assert!(matches!(roundtrip(None).1, IniEdit::CreatedFile { .. }));
        assert!(matches!(roundtrip(Some("")).1, IniEdit::AppendedSection { .. }));
        assert!(matches!(roundtrip(Some("[/Script/Engine.RendererSettings]\r\nr.X=1\r\n")).1, IniEdit::AppendedSection { .. }));
        assert!(matches!(roundtrip(Some("[Core.System]\nPaths=a")).1, IniEdit::AppendedSection { .. }), "LF file without final newline");
        assert!(matches!(roundtrip(Some("[SystemSettings]\r\nr.A=1\r\n\r\n[Other]\r\nx=1\r\n")).1, IniEdit::InsertedLine { .. }));
        assert!(matches!(roundtrip(Some("[SystemSettings]")).1, IniEdit::AppendedLine { .. }), "header without newline");
        assert!(matches!(roundtrip(Some("[systemsettings]\nr.hairstrands.streaming = 1\n")).1, IniEdit::ReplacedLine { .. }));
        assert_eq!(roundtrip(Some("[SystemSettings]\r\nr.HairStrands.Streaming=0\r\n")).1, IniEdit::AlreadyPresent);
        assert_eq!(roundtrip(Some("[SystemSettings]\r\nr.HairStrands.Streaming = 0\r\n")).1, IniEdit::AlreadyPresent);
    }

    #[test]
    fn revert_after_game_rewrote_file() {
        // we appended; the game then added more settings after ours
        let (new, edit) = apply_setting(Some("[A]\r\nx=1\r\n"), S, K, "0");
        let rewritten = format!("{}r.Other=2\r\n", new.unwrap());
        assert_eq!(revert_setting(Some(&rewritten), &edit), Revert::Write("[A]\r\nx=1\r\n\r\n[SystemSettings]\r\nr.Other=2\r\n".into()));
        // created file, game added a section: only our line goes
        let (new, edit) = apply_setting(None, S, K, "0");
        let rewritten = format!("{}\r\n[B]\r\ny=1\r\n", new.unwrap());
        match revert_setting(Some(&rewritten), &edit) {
            Revert::Write(t) => {
                assert!(!t.contains(K));
                assert!(t.contains("[B]\r\ny=1"));
            }
            r => panic!("{r:?}"),
        }
        // created file, game rewrote it identically formatted with only our section -> delete
        let (_, edit) = apply_setting(None, S, K, "0");
        assert_eq!(revert_setting(Some("[SystemSettings]\nr.HairStrands.Streaming=0\n"), &edit), Revert::Delete);
        // the game dropped our line entirely: nothing to undo
        let (_, edit) = apply_setting(Some("[SystemSettings]\r\n"), S, K, "0");
        assert_eq!(revert_setting(Some("[SystemSettings]\r\n"), &edit), Revert::Keep);
        // file deleted meanwhile
        assert_eq!(revert_setting(None, &edit), Revert::Keep);
    }

    #[test]
    fn set_key_preserves_style() {
        let t = "; c\r\n[Debug]\r\nConsoleEnabled = 1\r\nGuiConsoleEnabled = 1\r\n[Other]\r\nA = 1\r\n";
        let o = set_key(t, "Debug", "ConsoleEnabled", "0");
        assert_eq!(o, "; c\r\n[Debug]\r\nConsoleEnabled = 0\r\nGuiConsoleEnabled = 1\r\n[Other]\r\nA = 1\r\n");
        let o = set_key(&o, "Debug", "GuiConsoleVisible", "0");
        assert_eq!(o, "; c\r\n[Debug]\r\nConsoleEnabled = 0\r\nGuiConsoleEnabled = 1\r\nGuiConsoleVisible = 0\r\n[Other]\r\nA = 1\r\n");
        let o = set_key("[A]\nx=1\n", "B", "y", "2");
        assert_eq!(o, "[A]\nx=1\n[B]\ny = 2\n");
        assert_eq!(get_key("[Debug]\nConsoleEnabled=1\n", "debug", "consoleenabled").as_deref(), Some("1"));
    }
}

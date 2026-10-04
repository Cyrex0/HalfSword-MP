//! Minimal Lua lexing for line-based lints: comments removed, strings kept or
//! blanked, line numbers preserved.

/// Source split into lines with every comment removed (`--` line comments and
/// `--[[ ]]` / `--[==[ ]==]` long comments). String contents are kept. Line
/// count and numbering are unchanged.
pub fn strip_comments(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let n = b.len();
    // Long bracket opener at i ("[", "=" * k, "["); returns k.
    let long_open = |i: usize| -> Option<usize> {
        if i < n && b[i] == '[' {
            let mut j = i + 1;
            while j < n && b[j] == '=' {
                j += 1;
            }
            if j < n && b[j] == '[' {
                return Some(j - i - 1);
            }
        }
        None
    };
    // Index just after the matching "]", "=" * k, "]" (or n), newlines copied when keep_nl.
    let skip_long = |mut i: usize, k: usize, out: &mut String, keep: bool| -> usize {
        while i < n {
            if b[i] == ']' {
                let mut j = i + 1;
                let mut eq = 0;
                while j < n && b[j] == '=' {
                    eq += 1;
                    j += 1;
                }
                if eq == k && j < n && b[j] == ']' {
                    if keep {
                        for c in &b[i..=j] {
                            out.push(*c);
                        }
                    }
                    return j + 1;
                }
            }
            if keep || b[i] == '\n' {
                out.push(b[i]);
            }
            i += 1;
        }
        n
    };
    while i < n {
        let c = b[i];
        if c == '-' && i + 1 < n && b[i + 1] == '-' {
            if let Some(k) = long_open(i + 2) {
                i = skip_long(i + 2 + k + 2, k, &mut out, false);
            } else {
                while i < n && b[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        if c == '"' || c == '\'' {
            out.push(c);
            i += 1;
            while i < n && b[i] != c && b[i] != '\n' {
                if b[i] == '\\' && i + 1 < n {
                    out.push(b[i]);
                    out.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            if i < n && b[i] == c {
                out.push(c);
                i += 1;
            }
            continue;
        }
        if let Some(k) = long_open(i) {
            for ch in &b[i..i + k + 2] {
                out.push(*ch);
            }
            i = skip_long(i + k + 2, k, &mut out, true);
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.split('\n').map(|s| s.trim_end_matches('\r').to_string()).collect()
}

/// A code line with every short string literal replaced by `""`.
pub fn blank_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let b: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '"' || c == '\'' {
            out.push(c);
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Leading whitespace width (tab = 4).
pub fn indent(line: &str) -> usize {
    let mut w = 0;
    for c in line.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += 4,
            _ => break,
        }
    }
    w
}

/// For a line `start` that opens a block (a `function` that is not closed on
/// the same line), the index of the line that closes it: the first later line
/// with indentation <= the opener's whose trimmed text starts with `end`.
/// One-line blocks return `start`.
pub fn block_end(lines: &[String], start: usize) -> usize {
    let open = &lines[start];
    let k = indent(open);
    let code = blank_strings(open);
    let opens = code.matches("function").count() + word_count(&code, "do") + word_count(&code, "then");
    let ends = word_count(&code, "end");
    if opens > 0 && ends >= opens {
        return start;
    }
    for (j, l) in lines.iter().enumerate().skip(start + 1) {
        let t = l.trim_start();
        if t.is_empty() {
            continue;
        }
        if indent(l) <= k && (t.starts_with("end") || t.starts_with("})")) {
            return j;
        }
        if indent(l) < k {
            return j;
        }
    }
    lines.len().saturating_sub(1)
}

/// Whole-word occurrences of `w` in `s`.
pub fn word_count(s: &str, w: &str) -> usize {
    let bytes = s.as_bytes();
    let mut n = 0;
    let mut from = 0;
    while let Some(p) = s[from..].find(w) {
        let a = from + p;
        let z = a + w.len();
        let before = a == 0 || !(bytes[a - 1].is_ascii_alphanumeric() || bytes[a - 1] == b'_');
        let after = z >= bytes.len() || !(bytes[z].is_ascii_alphanumeric() || bytes[z] == b'_');
        if before && after {
            n += 1;
        }
        from = z;
    }
    n
}

/// Whole-word containment.
pub fn has_word(s: &str, w: &str) -> bool {
    word_count(s, w) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_line_and_long_comments_keeps_strings_and_lines() {
        let src = "local a = 1 -- c\n--[[ x\n y ]] local b = \"--not\"\nlocal s = [[a\n--b]]\n";
        let l = strip_comments(src);
        assert_eq!(l[0].trim_end(), "local a = 1");
        assert_eq!(l[1], "");
        assert_eq!(l[2].trim(), "local b = \"--not\"");
        assert!(l[4].contains("--b]]"));
        assert_eq!(l.len(), 6);
    }

    #[test]
    fn blanks_strings_and_counts_words() {
        assert_eq!(blank_strings(r#"x("a\"b", 'c') y"#), r#"x("", '') y"#);
        assert_eq!(word_count("end) end_x xend end", "end"), 2);
    }

    #[test]
    fn finds_block_end_by_indentation() {
        let src = "f(function()\n    a()\n    if x then\n    end\nend)\ng()";
        let l = strip_comments(src);
        assert_eq!(block_end(&l, 0), 4);
        let one = strip_comments("x(1, function() y() end)");
        assert_eq!(block_end(&one, 0), 0);
    }
}

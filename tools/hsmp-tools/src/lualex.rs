//! A small Lua 5.4 lexer for lints: blanks string literals and removes
//! comments while keeping every newline, so line numbers stay exact.
//!
//! * `"..."` / `'...'` (with escapes, incl. `\` + newline and `\z`) -> `""`
//! * `[[...]]`, `[==[...]==]` long strings -> `""` followed by the newlines they spanned
//! * `-- ...` line comments and `--[[ ... ]]` / `--[==[ ... ]==]` block comments -> removed
//!   (block comments keep their newlines)
//!
//! Lints then pattern-match on code only, so a CamelCase call in a comment or
//! a log message never fires.

/// Strip comments and blank strings; the result has the same number of lines.
pub fn strip(src: &str) -> String {
    strip_impl(src, false)
}

/// Strip comments only: string literals are kept verbatim (for lints that read string keys,
/// e.g. `obj["SetUpArmor"]`). The result has the same number of lines as `src`.
pub fn strip_comments(src: &str) -> String {
    strip_impl(src, true)
}

/// [`strip_comments`], split into lines (index 0 = line 1).
pub fn comment_free_lines(src: &str) -> Vec<String> {
    strip_comments(src).split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()
}

fn strip_impl(src: &str, keep_strings: bool) -> String {
    let c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    // Returns the level of a long bracket opening at i (`[` `=`* `[`), if any.
    let long_open = |i: usize| -> Option<usize> {
        if i < n && c[i] == '[' {
            let mut j = i + 1;
            while j < n && c[j] == '=' {
                j += 1;
            }
            if j < n && c[j] == '[' {
                return Some(j - i - 1);
            }
        }
        None
    };
    // Skip a long bracket body starting after its opener; returns (index after close, newlines).
    let long_body = |mut i: usize, level: usize| -> (usize, usize) {
        let mut nl = 0;
        while i < n {
            if c[i] == ']' {
                let mut j = i + 1;
                let mut eq = 0;
                while j < n && c[j] == '=' {
                    eq += 1;
                    j += 1;
                }
                if eq == level && j < n && c[j] == ']' {
                    return (j + 1, nl);
                }
                i += 1;
                continue;
            }
            if c[i] == '\n' {
                nl += 1;
            }
            i += 1;
        }
        (n, nl)
    };
    while i < n {
        let ch = c[i];
        if ch == '-' && i + 1 < n && c[i + 1] == '-' {
            if let Some(level) = long_open(i + 2) {
                let (j, nl) = long_body(i + 2 + level + 2, level);
                out.extend(std::iter::repeat_n('\n', nl));
                i = j;
            } else {
                while i < n && c[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(level) = long_open(i) {
            let (j, nl) = long_body(i + level + 2, level);
            if keep_strings {
                out.extend(c[i..j].iter());
                i = j;
                continue;
            }
            out.push_str("\"\"");
            out.extend(std::iter::repeat_n('\n', nl));
            i = j;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let q = ch;
            let start = i;
            let mut nl = 0;
            i += 1;
            while i < n && c[i] != q {
                if c[i] == '\\' && i + 1 < n {
                    if c[i + 1] == '\n' {
                        nl += 1;
                    } else if c[i + 1] == 'z' {
                        i += 2;
                        while i < n && c[i].is_whitespace() {
                            if c[i] == '\n' {
                                nl += 1;
                            }
                            i += 1;
                        }
                        continue;
                    }
                    i += 2;
                    continue;
                }
                if c[i] == '\n' {
                    break; // unfinished string: stop at end of line
                }
                i += 1;
            }
            if i < n && c[i] == q {
                i += 1;
            }
            if keep_strings {
                out.extend(c[start..i].iter());
                continue;
            }
            out.push_str("\"\"");
            out.extend(std::iter::repeat_n('\n', nl));
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// [`strip`], split into lines (index 0 = line 1).
pub fn code_lines(src: &str) -> Vec<String> {
    strip(src).split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips() {
        let s = "a = \"x--y\" -- c\nb = 'q\\'' .. [[\nlong\n]] --[==[ blk\n ]==] c:Foo()\n";
        let l = code_lines(s);
        assert_eq!(l[0], "a = \"\" ");
        assert_eq!(l[1], "b = \"\" .. \"\"");
        assert_eq!(l[2], "");
        assert_eq!(l[3], " ");
        assert_eq!(l[4], " c:Foo()");
        assert_eq!(l.len(), s.split('\n').count());
    }
    #[test]
    fn strip_comments_keeps_strings() {
        let s = "a = w[\"SetUpArmor\"] -- c\nb = 'q--' .. [[\nlong\n]] --[==[ blk\n ]==] c:Foo()\n";
        let l = comment_free_lines(s);
        assert_eq!(l[0], "a = w[\"SetUpArmor\"] ");
        assert_eq!(l[1], "b = 'q--' .. [[");
        assert_eq!(l[3], "]] ");
        assert_eq!(l[4], " c:Foo()");
        assert_eq!(l.len(), s.split('\n').count());
    }
}

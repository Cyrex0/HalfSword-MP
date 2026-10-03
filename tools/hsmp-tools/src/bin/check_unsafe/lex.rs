//! A token-level Lua 5.4 lexer for `check_unsafe`: names, keywords, string
//! literals (with their decoded-enough contents), numbers and operators, each
//! with its 1-based line. Comments are dropped. Block structure (function/if/
//! do/repeat ... end/until) can then be matched exactly, which line-based
//! heuristics cannot do for multi-line hook registrations.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Name,
    Str,
    Num,
    Op,
}

#[derive(Debug, Clone)]
pub struct Tok {
    pub kind: Kind,
    /// Name / operator text, or the string literal's contents (escapes kept verbatim).
    pub text: String,
    pub line: usize,
}

impl Tok {
    pub fn is(&self, k: Kind, t: &str) -> bool {
        self.kind == k && self.text == t
    }
    pub fn op(&self, t: &str) -> bool {
        self.is(Kind::Op, t)
    }
    pub fn name(&self, t: &str) -> bool {
        self.is(Kind::Name, t)
    }
}

const OPS3: &[&str] = &["..."];
const OPS2: &[&str] = &["..", "==", "~=", "<=", ">=", "::", "//", "<<", ">>"];

pub fn lex(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1usize;
    // Level of a long bracket `[` `=`* `[` opening at i.
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
    // Body of a long bracket starting at `i` (after the opener): (end index after
    // the closer, contents, newlines spanned).
    let long_body = |mut i: usize, level: usize| -> (usize, String, usize) {
        let start = i;
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
                    return (j + 1, c[start..i].iter().collect(), nl);
                }
            }
            if c[i] == '\n' {
                nl += 1;
            }
            i += 1;
        }
        (n, c[start..n].iter().collect(), nl)
    };
    while i < n {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '-' && i + 1 < n && c[i + 1] == '-' {
            if let Some(level) = long_open(i + 2) {
                let (j, _, nl) = long_body(i + 2 + level + 2, level);
                line += nl;
                i = j;
            } else {
                while i < n && c[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(level) = long_open(i) {
            let l0 = line;
            let (j, body, nl) = long_body(i + level + 2, level);
            out.push(Tok { kind: Kind::Str, text: body, line: l0 });
            line += nl;
            i = j;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let l0 = line;
            let mut j = i + 1;
            let mut s = String::new();
            while j < n && c[j] != ch {
                if c[j] == '\\' && j + 1 < n {
                    if c[j + 1] == '\n' {
                        line += 1;
                    }
                    s.push(c[j]);
                    s.push(c[j + 1]);
                    j += 2;
                    continue;
                }
                if c[j] == '\n' {
                    break; // unterminated: stop at end of line
                }
                s.push(c[j]);
                j += 1;
            }
            out.push(Tok { kind: Kind::Str, text: s, line: l0 });
            i = (j + 1).min(n);
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && i + 1 < n && c[i + 1].is_ascii_digit()) {
            let mut j = i;
            while j < n
                && (c[j].is_ascii_alphanumeric()
                    || c[j] == '.'
                    || c[j] == '_'
                    || ((c[j] == '+' || c[j] == '-') && j > i && matches!(c[j - 1], 'e' | 'E' | 'p' | 'P')))
            {
                j += 1;
            }
            out.push(Tok { kind: Kind::Num, text: c[i..j].iter().collect(), line });
            i = j;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let mut j = i;
            while j < n && (c[j].is_alphanumeric() || c[j] == '_') {
                j += 1;
            }
            out.push(Tok { kind: Kind::Name, text: c[i..j].iter().collect(), line });
            i = j;
            continue;
        }
        let rest: String = c[i..(i + 3).min(n)].iter().collect();
        let op = OPS3
            .iter()
            .chain(OPS2.iter())
            .find(|o| rest.starts_with(**o))
            .map(|o| o.to_string())
            .unwrap_or_else(|| ch.to_string());
        i += op.chars().count();
        out.push(Tok { kind: Kind::Op, text: op, line });
    }
    out
}

/// Block delta of a token: +1 for `function`/`if`/`do`/`repeat`, -1 for
/// `end`/`until` (`while`/`for` open through their `do`).
pub fn block_delta(t: &Tok) -> i32 {
    if t.kind != Kind::Name {
        return 0;
    }
    match t.text.as_str() {
        "function" | "if" | "do" | "repeat" => 1,
        "end" | "until" => -1,
        _ => 0,
    }
}

/// Index of the `end` closing the block opened at `open` (a `function`/`if`/
/// `do`/`repeat` token), or the last token if unbalanced.
pub fn block_close(toks: &[Tok], open: usize) -> usize {
    let mut d = 0i32;
    for (k, t) in toks.iter().enumerate().skip(open) {
        d += block_delta(t);
        if d == 0 {
            return k;
        }
    }
    toks.len().saturating_sub(1)
}

/// Index of the bracket closing the one at `open` (`(`, `[`, `{`).
pub fn paren_close(toks: &[Tok], open: usize) -> usize {
    let mut d = 0i32;
    for (k, t) in toks.iter().enumerate().skip(open) {
        if t.kind == Kind::Op {
            match t.text.as_str() {
                "(" | "[" | "{" => d += 1,
                ")" | "]" | "}" => {
                    d -= 1;
                    if d == 0 {
                        return k;
                    }
                }
                _ => {}
            }
        }
    }
    toks.len().saturating_sub(1)
}

/// Block depth before each token (0 = module level).
pub fn depths(toks: &[Tok]) -> Vec<i32> {
    let mut d = 0i32;
    let mut out = Vec::with_capacity(toks.len());
    for t in toks {
        let dd = block_delta(t);
        if dd < 0 {
            d += dd;
        }
        out.push(d.max(0));
        if dd > 0 {
            d += dd;
        }
    }
    out
}

/// A `function (params) ... end` starting at token `f`: (param names, body start, end index).
pub fn function_at(toks: &[Tok], f: usize) -> Option<(Vec<String>, usize, usize)> {
    if !toks.get(f)?.name("function") {
        return None;
    }
    let mut k = f + 1;
    // optional name: a.b:c
    while k < toks.len() && !toks[k].op("(") {
        k += 1;
        if k > f + 8 {
            return None;
        }
    }
    let close = paren_close(toks, k);
    let params: Vec<String> = toks[k + 1..close]
        .iter()
        .filter(|t| t.kind == Kind::Name || t.op("..."))
        .map(|t| t.text.clone())
        .collect();
    let end = block_close(toks, f);
    Some((params, close + 1, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_strings_comments_and_lines() {
        let t = lex("local a = \"x\\\"y\" -- c\n--[[ long\n]] b = [==[s\n]==] .. 'q'\n");
        let s: Vec<String> = t.iter().map(|t| format!("{}:{}", t.line, t.text)).collect();
        assert_eq!(s, vec!["1:local", "1:a", "1:=", "1:x\\\"y", "3:b", "3:=", "3:s\n", "4:..", "4:q"]);
    }

    #[test]
    fn matches_blocks() {
        let t = lex("function f() if x then for i=1,2 do end end repeat until y end g()");
        let close = block_close(&t, 0);
        assert!(t[close].name("end"));
        assert!(t[close + 1].name("g"));
        let (p, _, e) = function_at(&t, 0).unwrap();
        assert!(p.is_empty());
        assert_eq!(e, close);
    }
}

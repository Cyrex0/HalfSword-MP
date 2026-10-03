//! Minimal Valve KeyValues (VDF / ACF) text parser.
//!
//! Enough for `libraryfolders.vdf` and `appmanifest_<id>.acf`:
//! quoted or bare tokens, `{ }` blocks, `//` comments, the `\\ \" \n \t`
//! escapes Steam writes, and `[$WIN32]`-style conditionals (ignored).
//! Keys are matched case-insensitively (Steam itself is inconsistent:
//! "LibraryFolders" vs "libraryfolders").

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// First child with this key (case-insensitive), if this is an object.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(kids) => kids.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }
    pub fn str_of(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Vdf::as_str)
    }
    pub fn children(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(k) => k,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let mut out = vec![];
    let mut it = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            '/' => {
                it.next();
                if it.peek() == Some(&'/') {
                    for c in it.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    return Err("stray '/'".into());
                }
            }
            '{' => {
                it.next();
                out.push(Tok::Open);
            }
            '}' => {
                it.next();
                out.push(Tok::Close);
            }
            '[' => {
                // conditional like [$WIN32] / [!$X360]: skip
                for c in it.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            '"' => {
                it.next();
                let mut s = String::new();
                let mut closed = false;
                while let Some(c) = it.next() {
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('\\') => s.push('\\'),
                            Some('"') => s.push('"'),
                            Some(o) => {
                                s.push('\\');
                                s.push(o);
                            }
                            None => return Err("unterminated escape".into()),
                        },
                        c => s.push(c),
                    }
                }
                if !closed {
                    return Err("unterminated string".into());
                }
                out.push(Tok::Str(s));
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = it.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    it.next();
                }
                out.push(Tok::Str(s));
            }
        }
    }
    Ok(out)
}

fn parse_obj(toks: &[Tok], pos: &mut usize, nested: bool) -> Result<Vec<(String, Vdf)>, String> {
    let mut kids = vec![];
    loop {
        match toks.get(*pos) {
            None if nested => return Err("missing '}'".into()),
            None => return Ok(kids),
            Some(Tok::Close) if nested => {
                *pos += 1;
                return Ok(kids);
            }
            Some(Tok::Close) => return Err("unexpected '}'".into()),
            Some(Tok::Open) => return Err("unexpected '{' (missing key)".into()),
            Some(Tok::Str(k)) => {
                *pos += 1;
                match toks.get(*pos) {
                    Some(Tok::Open) => {
                        *pos += 1;
                        let v = parse_obj(toks, pos, true)?;
                        kids.push((k.clone(), Vdf::Obj(v)));
                    }
                    Some(Tok::Str(v)) => {
                        *pos += 1;
                        kids.push((k.clone(), Vdf::Str(v.clone())));
                    }
                    _ => return Err(format!("key '{k}' has no value")),
                }
            }
        }
    }
}

/// Parse a whole VDF document into a root object.
pub fn parse(text: &str) -> Result<Vdf, String> {
    let toks = tokenize(text)?;
    let mut pos = 0;
    Ok(Vdf::Obj(parse_obj(&toks, &mut pos, false)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_escapes() {
        let v = parse("// c\n\"Root\"\n{\n \"path\" \"D:\\\\Steam Library\" // tail\n \"q\" \"a\\\"b\"\n bare 12\n \"kid\" { \"x\" \"1\" }\n}\n").unwrap();
        let r = v.get("root").unwrap();
        assert_eq!(r.str_of("PATH"), Some("D:\\Steam Library"));
        assert_eq!(r.str_of("q"), Some("a\"b"));
        assert_eq!(r.str_of("bare"), Some("12"));
        assert_eq!(r.get("kid").unwrap().str_of("x"), Some("1"));
    }

    #[test]
    fn errors() {
        assert!(parse("\"a\" { \"b\" \"c\"").is_err());
        assert!(parse("\"a\" \"b\" }").is_err());
        assert!(parse("\"a\" \"unterminated").is_err());
        assert!(parse("\"a\"").is_err());
    }

    #[test]
    fn conditionals_and_bom() {
        let v = parse("\u{feff}\"a\" { \"k\" \"v\" [$WIN32] }").unwrap();
        assert_eq!(v.get("a").unwrap().str_of("k"), Some("v"));
    }
}

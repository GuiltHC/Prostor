//! Minimal Valve KeyValues (VDF/ACF) parser.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Obj(Obj),
}

/// Keys are stored lower-case: VDF keys are case-insensitive.
pub type Obj = BTreeMap<String, Value>;

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
}

pub fn get<'a>(o: &'a Obj, key: &str) -> Option<&'a Value> {
    o.get(&key.to_lowercase())
}

/// Follows a path of keys.
pub fn dig<'a>(o: &'a Obj, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let v = get(o, first)?;
    if rest.is_empty() { Some(v) } else { dig(v.as_obj()?, rest) }
}

enum Tok {
    Str(String),
    Open,
    Close,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn next(&mut self) -> Option<Tok> {
        let s = self.s;
        while self.i < s.len() {
            let c = s[self.i];
            if c.is_ascii_whitespace() {
                self.i += 1;
                continue;
            }
            if c == b'/' && s.get(self.i + 1) == Some(&b'/') {
                while self.i < s.len() && s[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            if c == b'{' {
                self.i += 1;
                return Some(Tok::Open);
            }
            if c == b'}' {
                self.i += 1;
                return Some(Tok::Close);
            }
            if c == b'"' {
                self.i += 1;
                let mut out = Vec::new();
                while self.i < s.len() && s[self.i] != b'"' {
                    if s[self.i] == b'\\' && self.i + 1 < s.len() {
                        self.i += 1;
                        out.push(match s[self.i] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            other => other,
                        });
                    } else {
                        out.push(s[self.i]);
                    }
                    self.i += 1;
                }
                self.i += 1;
                return Some(Tok::Str(String::from_utf8_lossy(&out).into_owned()));
            }
            let start = self.i;
            while self.i < s.len() && !s[self.i].is_ascii_whitespace() && !matches!(s[self.i], b'{' | b'}' | b'"') {
                self.i += 1;
            }
            return Some(Tok::Str(String::from_utf8_lossy(&s[start..self.i]).into_owned()));
        }
        None
    }
}

fn parse_obj(lx: &mut Lexer) -> Obj {
    let mut o = Obj::new();
    loop {
        let key = match lx.next() {
            Some(Tok::Str(k)) => k,
            _ => return o,
        };
        match lx.next() {
            Some(Tok::Open) => {
                o.insert(key.to_lowercase(), Value::Obj(parse_obj(lx)));
            }
            Some(Tok::Str(v)) => {
                o.insert(key.to_lowercase(), Value::Str(v));
            }
            _ => return o,
        }
    }
}

pub fn parse(text: &str) -> Obj {
    parse_obj(&mut Lexer { s: text.as_bytes(), i: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest() {
        let t = r#"
"AppState"
{
	"appid"		"570"
	"name"		"Dota 2" // comment
	"InstallDir"		"dota 2 beta"
	"UserConfig" { "language" "russian" }
	"path"  "D:\\SteamLibrary"
}"#;
        let o = parse(t);
        assert_eq!(dig(&o, &["AppState", "appid"]).and_then(Value::as_str), Some("570"));
        assert_eq!(dig(&o, &["appstate", "installdir"]).and_then(Value::as_str), Some("dota 2 beta"));
        assert_eq!(dig(&o, &["AppState", "UserConfig", "language"]).and_then(Value::as_str), Some("russian"));
        assert_eq!(dig(&o, &["AppState", "path"]).and_then(Value::as_str), Some(r"D:\SteamLibrary"));
    }
}

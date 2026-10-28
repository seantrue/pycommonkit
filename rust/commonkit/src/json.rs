//! A small strict JSON reader (RFC 8259) for the two files the store reads: `.policy.json`
//! (contract §10.5) and `.usage.json` (§9.7).
//!
//! Strict where Python's `json` is: no `NaN` or `Infinity`, no trailing commas, no control
//! characters inside strings, JSON whitespace only. Objects keep their pairs in order, so a
//! caller can see a repeated key (which makes a policy file invalid). Nesting is bounded, as
//! Python's recursion limit bounds it, so a hostile file is invalid rather than a stack overflow.
//! Callers bound the input length before parsing.

/// A parsed JSON value. Numbers keep their text: a setting parses it as a double, and
/// `.usage.json` needs to know whether it was written as an integer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number { text: String, integer: bool },
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value of the LAST pair named `key` in an object (Python's `dict` keeps the last).
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Python's recursion limit is about 1000 frames; a document nested deeper is invalid there
/// (`RecursionError`) and here.
const MAX_DEPTH: usize = 1000;

/// Parse a whole document, or `None` if it is not one JSON value with only whitespace around it.
pub(crate) fn parse(text: &str) -> Option<Json> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.i += 1;
        Some(b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn literal(&mut self, word: &[u8], v: Json) -> Option<Json> {
        let end = self.i.checked_add(word.len())?;
        (self.s.get(self.i..end)? == word).then(|| {
            self.i = end;
            v
        })
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek()? {
            b'n' => self.literal(b"null", Json::Null),
            b't' => self.literal(b"true", Json::Bool(true)),
            b'f' => self.literal(b"false", Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.eat(b']') {
            return Some(Json::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.bump()? {
                b',' => {}
                b']' => return Some(Json::Array(items)),
                _ => return None,
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut pairs = Vec::new();
        self.ws();
        if self.eat(b'}') {
            return Some(Json::Object(pairs));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.string()?;
            self.ws();
            if !self.eat(b':') {
                return None;
            }
            self.ws();
            let value = self.value(depth + 1)?;
            pairs.push((key, value));
            self.ws();
            match self.bump()? {
                b',' => {}
                b'}' => return Some(Json::Object(pairs)),
                _ => return None,
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - start
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`
    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        self.eat(b'-');
        match self.peek()? {
            b'0' => self.i += 1,
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        let mut integer = true;
        if self.eat(b'.') {
            integer = false;
            if self.digits() == 0 {
                return None;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            integer = false;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return None;
            }
        }
        let text = std::str::from_utf8(self.s.get(start..self.i)?).ok()?;
        Some(Json::Number {
            text: text.to_owned(),
            integer,
        })
    }

    fn hex4(&mut self) -> Option<u32> {
        let end = self.i.checked_add(4)?;
        let h = std::str::from_utf8(self.s.get(self.i..end)?).ok()?;
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i = end;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            // Copy the run up to the next quote, backslash or control character in one go. The
            // input is a `&str`, so a run that ends on one of those ASCII bytes is valid UTF-8.
            let start = self.i;
            while matches!(self.peek(), Some(b) if b != b'"' && b != b'\\' && b >= 0x20) {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(self.s.get(start..self.i)?).ok()?);
            match self.bump()? {
                b'"' => return Some(out),
                b'\\' => {}
                _ => return None, // a raw control character
            }
            let c = match self.bump()? {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => {
                    let hi = self.hex4()?;
                    let code = if (0xD800..0xDC00).contains(&hi)
                        && self.s.get(self.i..self.i.saturating_add(2)) == Some(b"\\u")
                    {
                        let save = self.i;
                        self.i += 2;
                        match self.hex4()? {
                            lo @ 0xDC00..0xE000 => 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00),
                            _ => {
                                self.i = save;
                                hi
                            }
                        }
                    } else {
                        hi
                    };
                    // Python keeps a lone surrogate in its `str`; Rust can't. U+FFFD matches no
                    // setting name and no number or flag, so the outcome is the same.
                    char::from_u32(code).unwrap_or('\u{FFFD}')
                }
                _ => return None,
            };
            out.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(text: &str, integer: bool) -> Json {
        Json::Number {
            text: text.into(),
            integer,
        }
    }

    #[test]
    fn values() {
        assert_eq!(parse(" null "), Some(Json::Null));
        assert_eq!(parse("-0"), Some(num("-0", true)));
        assert_eq!(parse("1.5e3"), Some(num("1.5e3", false)));
        assert_eq!(parse("1E-2"), Some(num("1E-2", false)));
        assert_eq!(
            parse(r#""a\u00e9\ud83d\ude00\n""#),
            Some(Json::Str("aé😀\n".into()))
        );
        assert_eq!(parse(r#""\ud800""#), Some(Json::Str("\u{FFFD}".into())));
        let doc = parse(r#"{"a": [1, {"b": true}], "a": 2}"#).unwrap();
        assert_eq!(doc.get("a"), Some(&num("2", true)));
        assert_eq!(parse("[]"), Some(Json::Array(vec![])));
    }

    #[test]
    fn strict() {
        for bad in [
            "",
            "NaN",
            "Infinity",
            "-Infinity",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            ".5",
            "+1",
            "1e",
            "\"\t\"",
            "'a'",
            "{a: 1}",
            "[1] x",
            "\u{feff}{}",
            "tru",
            "\"\\x\"",
            "\"abc",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert!(parse(&"[".repeat(100_000)).is_none());
        let deep = format!("{}{}", "[".repeat(900), "]".repeat(900));
        assert!(parse(&deep).is_some());
    }
}

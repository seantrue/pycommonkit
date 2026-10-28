//! The Python literals a `.npy` header uses: dicts, tuples, lists, strings, integers, `True`,
//! `False` and `None`.
//!
//! The header is hostile input. Nesting is bounded, integers are parsed with checked
//! arithmetic, and every collection grows only with the bytes actually present, so a header
//! can't ask for an allocation larger than itself.

use super::NpyError;

/// The deepest nesting accepted. A structured field costs two levels (its list and its tuple)
/// below the header dict, so this allows about fifteen levels of nested structs, far more than
/// any real dtype, while bounding recursion.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Lit {
    Str(String),
    Int(u64),
    Bool(bool),
    None,
    Tuple(Vec<Lit>),
    List(Vec<Lit>),
    Dict(Vec<(String, Lit)>),
}

/// Parse one literal that spans all of `text`, apart from surrounding whitespace.
pub(crate) fn parse(text: &str) -> Result<Lit, NpyError> {
    let mut p = Parser { s: text, i: 0 };
    let v = p.value(0)?;
    p.skip_ws();
    if p.i != p.s.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a str,
    /// A byte index into `s`, always on a char boundary.
    i: usize,
}

fn is_ws(c: char) -> bool {
    // Python's whitespace between tokens: space, tab, form feed, and the line breaks that a
    // literal inside brackets may contain. VT is included because `str.isspace` includes it.
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

impl Parser<'_> {
    fn err(&self, what: &str) -> NpyError {
        NpyError::BadHeader(format!("{what} at byte {}", self.i))
    }

    fn rest(&self) -> &str {
        self.s.get(self.i..).unwrap_or("")
    }

    fn peek_char(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek_char().is_some_and(is_ws) {
            self.bump();
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.peek_char()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Lit, NpyError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.peek() {
            Some('{') => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    if self.eat('}') {
                        return Ok(Lit::Dict(items));
                    }
                    let Lit::Str(k) = self.string()? else {
                        return Err(self.err("dict key is not a string"));
                    };
                    if !self.eat(':') {
                        return Err(self.err("expected ':'"));
                    }
                    items.push((k, self.value(depth + 1)?));
                    if !self.eat(',') && self.peek() != Some('}') {
                        return Err(self.err("expected ',' or '}'"));
                    }
                }
            }
            Some(open @ ('(' | '[')) => {
                self.bump();
                let close = if open == '(' { ')' } else { ']' };
                let mut items = Vec::new();
                let mut comma = false;
                loop {
                    if self.eat(close) {
                        break;
                    }
                    items.push(self.value(depth + 1)?);
                    if self.eat(',') {
                        comma = true;
                    } else if self.peek() != Some(close) {
                        return Err(self.err("expected ',' or a closing bracket"));
                    }
                }
                if open == '[' {
                    return Ok(Lit::List(items));
                }
                // `(x)` is x itself, not a tuple: only a comma (or `()`) makes one.
                if items.len() == 1 && !comma {
                    return items.pop().ok_or_else(|| self.err("empty parentheses"));
                }
                Ok(Lit::Tuple(items))
            }
            Some('\'' | '"') => self.string(),
            Some(c) if c.is_ascii_digit() => self.int(),
            Some(_) => {
                for (word, v) in [
                    ("True", Lit::Bool(true)),
                    ("False", Lit::Bool(false)),
                    ("None", Lit::None),
                ] {
                    if let Some(after) = self.rest().strip_prefix(word) {
                        if after
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_')
                        {
                            break;
                        }
                        self.i += word.len();
                        return Ok(v);
                    }
                }
                Err(self.err("unexpected character"))
            }
            None => Err(self.err("unexpected end of header")),
        }
    }

    fn int(&mut self) -> Result<Lit, NpyError> {
        let mut n: u64 = 0;
        while let Some(c) = self.peek_char() {
            let Some(d) = c.to_digit(10).filter(|_| c.is_ascii_digit()) else {
                break;
            };
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(d)))
                .ok_or_else(|| self.err("integer out of range"))?;
            self.bump();
        }
        // Python 2 headers wrote long integers as `3L`; numpy still reads them.
        if self.peek_char() == Some('L') {
            self.bump();
        }
        if self
            .peek_char()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return Err(self.err("not an integer"));
        }
        Ok(Lit::Int(n))
    }

    fn hex(&mut self, digits: usize) -> Result<char, NpyError> {
        let mut v: u32 = 0;
        for _ in 0..digits {
            let d = self
                .bump()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.err("bad hex escape"))?;
            v = v * 16 + d;
        }
        char::from_u32(v).ok_or_else(|| self.err("escape is not a character"))
    }

    fn string(&mut self) -> Result<Lit, NpyError> {
        let quote = match self.peek() {
            Some(q @ ('\'' | '"')) => q,
            _ => return Err(self.err("expected a string")),
        };
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated string")),
                Some('\n') => return Err(self.err("newline in a string")),
                Some(c) if c == quote => return Ok(Lit::Str(out)),
                Some('\\') => match self.bump() {
                    None => return Err(self.err("unterminated string")),
                    Some('\n') => {}
                    Some('\\') => out.push('\\'),
                    Some('\'') => out.push('\''),
                    Some('"') => out.push('"'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('a') => out.push('\x07'),
                    Some('b') => out.push('\x08'),
                    Some('f') => out.push('\x0c'),
                    Some('v') => out.push('\x0b'),
                    Some('x') => out.push(self.hex(2)?),
                    Some('u') => out.push(self.hex(4)?),
                    Some('U') => out.push(self.hex(8)?),
                    Some(c @ '0'..='7') => {
                        let mut v = c.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match self.peek_char().and_then(|c| c.to_digit(8)) {
                                Some(d) => {
                                    v = v * 8 + d;
                                    self.bump();
                                }
                                None => break,
                            }
                        }
                        out.push(char::from_u32(v).ok_or_else(|| self.err("bad octal escape"))?);
                    }
                    // Python keeps an unknown escape as written.
                    Some(c) => {
                        out.push('\\');
                        out.push(c);
                    }
                },
                Some(c) => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parenthesised_value_is_not_a_tuple() {
        assert_eq!(parse("(3)").unwrap(), Lit::Int(3));
        assert_eq!(parse("(3,)").unwrap(), Lit::Tuple(vec![Lit::Int(3)]));
        assert_eq!(parse("()").unwrap(), Lit::Tuple(vec![]));
        assert!(parse("(,)").is_err());
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(parse(r#""it's""#).unwrap(), Lit::Str("it's".into()));
        assert_eq!(parse(r"'a\'b\\c'").unwrap(), Lit::Str("a'b\\c".into()));
        assert_eq!(parse(r"'\x41π'").unwrap(), Lit::Str("Aπ".into()));
        assert!(parse("'abc").is_err());
        assert!(parse(r"'\xZZ'").is_err());
    }

    #[test]
    fn words_need_a_boundary() {
        assert_eq!(parse("True").unwrap(), Lit::Bool(true));
        assert!(parse("Truex").is_err());
        assert!(parse("12a").is_err());
        assert!(parse("99999999999999999999999").is_err());
    }

    #[test]
    fn deep_nesting_is_refused() {
        let deep = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(parse(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(20), "]".repeat(20));
        assert!(parse(&ok).is_ok());
    }
}

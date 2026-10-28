//! Descriptors: the `descr` value of a `.npy` header (contract §5.4).

use std::fmt;

use super::literal::{self, Lit};
use super::{MAX_DIMS, NpyError};

/// The byte order of a simple descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByteOrder {
    /// `<`
    Little,
    /// `>`
    Big,
    /// `|`: one-byte kinds, `S` and `V`.
    NotApplicable,
}

impl ByteOrder {
    fn char(self) -> char {
        match self {
            ByteOrder::Little => '<',
            ByteOrder::Big => '>',
            ByteOrder::NotApplicable => '|',
        }
    }

    /// Is data in this order laid out as the host lays it out?
    pub fn is_native(self) -> bool {
        match self {
            ByteOrder::NotApplicable => true,
            ByteOrder::Little => cfg!(target_endian = "little"),
            ByteOrder::Big => cfg!(target_endian = "big"),
        }
    }
}

/// The kind of a simple descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `b`
    Bool,
    /// `i`
    Int,
    /// `u`
    UInt,
    /// `f`
    Float,
    /// `c`
    Complex,
    /// `M`
    Datetime,
    /// `m`
    Timedelta,
    /// `U`: UCS-4 text; the descriptor's number counts characters.
    Unicode,
    /// `S`
    Bytes,
    /// `V`
    Void,
}

impl Kind {
    fn char(self) -> char {
        match self {
            Kind::Bool => 'b',
            Kind::Int => 'i',
            Kind::UInt => 'u',
            Kind::Float => 'f',
            Kind::Complex => 'c',
            Kind::Datetime => 'M',
            Kind::Timedelta => 'm',
            Kind::Unicode => 'U',
            Kind::Bytes => 'S',
            Kind::Void => 'V',
        }
    }
}

/// The descriptors a reader offers typed access to (contract §5.4, "Typed classes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Typed {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F16,
    F32,
    F64,
}

/// A simple descriptor such as `<f4`, `|u1`, `>M8[s]` or `<U3`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Simple {
    order: ByteOrder,
    kind: Kind,
    /// The number in the descriptor: bytes, or characters for `U`.
    n: u64,
    /// The datetime unit between brackets, without them (`s`, `10ms`), if any.
    unit: Option<String>,
}

impl Simple {
    pub fn order(&self) -> ByteOrder {
        self.order
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The datetime or timedelta unit, without its brackets.
    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    /// Bytes per element, or `None` if it overflows (`U` with an absurd length).
    pub fn itemsize(&self) -> Option<u64> {
        match self.kind {
            Kind::Unicode => self.n.checked_mul(4),
            _ => Some(self.n),
        }
    }

    /// The typed class, for the kinds a reader offers typed access to.
    pub fn typed(&self) -> Option<Typed> {
        Some(match (self.kind, self.n) {
            (Kind::Bool, 1) => Typed::Bool,
            (Kind::Int, 1) => Typed::I8,
            (Kind::Int, 2) => Typed::I16,
            (Kind::Int, 4) => Typed::I32,
            (Kind::Int, 8) => Typed::I64,
            (Kind::UInt, 1) => Typed::U8,
            (Kind::UInt, 2) => Typed::U16,
            (Kind::UInt, 4) => Typed::U32,
            (Kind::UInt, 8) => Typed::U64,
            (Kind::Float, 2) => Typed::F16,
            (Kind::Float, 4) => Typed::F32,
            (Kind::Float, 8) => Typed::F64,
            _ => return None,
        })
    }

    /// Parse a simple descriptor written bare (`<f4`, not `'<f4'`).
    ///
    /// The byte order of a one-byte kind, `S` or `V` is normalised to `|`, as numpy does when
    /// it reads `<u1`; `|` on a multi-byte kind is refused, since it would mean "native", which
    /// a file can't depend on. `=` is refused for the same reason. Long doubles (`f16`, `c32`)
    /// are platform-dependent and outside contract version 1.
    pub fn parse(text: &str) -> Result<Simple, NpyError> {
        let bad = || NpyError::BadDescr(text.chars().take(64).collect());
        let mut chars = text.chars();
        let order = match chars.next() {
            Some('<') => ByteOrder::Little,
            Some('>') => ByteOrder::Big,
            Some('|') => ByteOrder::NotApplicable,
            _ => return Err(bad()),
        };
        let kind = match chars.next() {
            Some('b') => Kind::Bool,
            Some('i') => Kind::Int,
            Some('u') => Kind::UInt,
            Some('f') => Kind::Float,
            Some('c') => Kind::Complex,
            Some('M') => Kind::Datetime,
            Some('m') => Kind::Timedelta,
            Some('U') => Kind::Unicode,
            Some('S') => Kind::Bytes,
            Some('V') => Kind::Void,
            // A reader MUST NOT unpickle (contract §5.4, §5.7).
            Some('O') => return Err(NpyError::ObjectDtype),
            _ => return Err(bad()),
        };
        let rest = chars.as_str();
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (digits, tail) = rest.split_at(digits_end);
        if digits.is_empty() || digits.len() > 19 {
            return Err(bad());
        }
        let n: u64 = digits.parse().map_err(|_| bad())?;
        let unit = match (kind, tail) {
            (_, "") => None,
            (Kind::Datetime | Kind::Timedelta, t) => {
                let inner = t
                    .strip_prefix('[')
                    .and_then(|t| t.strip_suffix(']'))
                    .ok_or_else(bad)?;
                if !valid_unit(inner) {
                    return Err(bad());
                }
                Some(inner.to_owned())
            }
            _ => return Err(bad()),
        };
        let size_ok = match kind {
            Kind::Bool => n == 1,
            Kind::Int | Kind::UInt => matches!(n, 1 | 2 | 4 | 8),
            Kind::Float => matches!(n, 2 | 4 | 8),
            Kind::Complex => matches!(n, 8 | 16),
            Kind::Datetime | Kind::Timedelta => n == 8,
            Kind::Unicode | Kind::Bytes | Kind::Void => true,
        };
        if !size_ok {
            return Err(bad());
        }
        let single = matches!(kind, Kind::Bytes | Kind::Void)
            || (matches!(kind, Kind::Bool | Kind::Int | Kind::UInt) && n == 1);
        let order = match (single, order) {
            (true, _) => ByteOrder::NotApplicable,
            (false, ByteOrder::NotApplicable) => return Err(bad()),
            (false, o) => o,
        };
        let s = Simple {
            order,
            kind,
            n,
            unit,
        };
        s.itemsize().ok_or(NpyError::Overflow)?;
        Ok(s)
    }
}

/// numpy's datetime units, optionally with a decimal multiplier (`10ms`).
fn valid_unit(inner: &str) -> bool {
    let unit = inner.trim_start_matches(|c: char| c.is_ascii_digit());
    let mult = inner.get(..inner.len() - unit.len()).unwrap_or("");
    if mult.len() > 9 || mult.starts_with('0') {
        return false;
    }
    matches!(
        unit,
        "Y" | "M" | "W" | "D" | "h" | "m" | "s" | "ms" | "us" | "ns" | "ps" | "fs" | "as"
    )
}

impl fmt::Display for Simple {
    /// The canonical descriptor, bare: `<f4`, `|u1`, `<M8[s]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.order.char(), self.kind.char(), self.n)?;
        if let Some(u) = &self.unit {
            write!(f, "[{u}]")?;
        }
        Ok(())
    }
}

/// One field of a structured descriptor: `(name, descr)` or `(name, descr, shape)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    name: String,
    descr: Descr,
    shape: Vec<u64>,
}

impl Field {
    /// A field. An empty `name` is padding (contract §5.8); an empty `shape` is no subarray.
    pub fn new(name: impl Into<String>, descr: Descr, shape: Vec<u64>) -> Field {
        Field {
            name: name.into(),
            descr,
            shape,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn descr(&self) -> &Descr {
        &self.descr
    }

    /// The subarray shape; empty when the field has none.
    pub fn shape(&self) -> &[u64] {
        &self.shape
    }

    /// Is this a padding field (numpy names padding `''`)?
    pub fn is_padding(&self) -> bool {
        self.name.is_empty()
    }

    fn itemsize(&self) -> Option<u64> {
        self.shape
            .iter()
            .try_fold(self.descr.itemsize()?, |acc, d| acc.checked_mul(*d))
    }
}

/// A `.npy` descriptor: simple or structured. Object dtypes can't be represented.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Descr {
    Simple(Simple),
    Struct(Vec<Field>),
}

impl Descr {
    /// Parse a descriptor as an array spec writes it (contract §13.2): a simple descriptor bare
    /// (`<f4`), or a structured one as its Python literal (`[('a', '<i4')]`).
    pub fn parse(text: &str) -> Result<Descr, NpyError> {
        if text.trim_start().starts_with('[') {
            Descr::from_lit(&literal::parse(text)?, 0)
        } else {
            Simple::parse(text).map(Descr::Simple)
        }
    }

    /// A structured descriptor from its fields. Fails on a repeated non-empty name or an item
    /// size that overflows.
    pub fn structured(fields: Vec<Field>) -> Result<Descr, NpyError> {
        let d = Descr::Struct(fields);
        d.validate()?;
        Ok(d)
    }

    pub(crate) fn from_lit(lit: &Lit, depth: usize) -> Result<Descr, NpyError> {
        if depth > 16 {
            return Err(NpyError::BadDescr(
                "structured dtype nested too deeply".into(),
            ));
        }
        let bad = |what: &str| NpyError::BadDescr(what.to_owned());
        let d = match lit {
            Lit::Str(s) => Descr::Simple(Simple::parse(s)?),
            Lit::List(items) => {
                let mut fields = Vec::new();
                for item in items {
                    let Lit::Tuple(parts) = item else {
                        return Err(bad("a field is not a tuple"));
                    };
                    let (name, descr, shape) = match parts.as_slice() {
                        [n, d] => (n, d, None),
                        [n, d, s] => (n, d, Some(s)),
                        _ => return Err(bad("a field is not (name, descr[, shape])")),
                    };
                    let Lit::Str(name) = name else {
                        // `((title, name), descr)`: titles are outside contract version 1.
                        return Err(bad("a field name is not a string"));
                    };
                    let shape = match shape {
                        None => Vec::new(),
                        Some(Lit::Int(n)) => vec![*n],
                        Some(Lit::Tuple(dims)) => {
                            if dims.len() > MAX_DIMS {
                                return Err(bad("subarray has too many dimensions"));
                            }
                            dims.iter()
                                .map(|d| match d {
                                    Lit::Int(n) => Ok(*n),
                                    _ => Err(bad("subarray dimension is not an integer")),
                                })
                                .collect::<Result<_, _>>()?
                        }
                        Some(_) => return Err(bad("subarray shape is not a tuple")),
                    };
                    fields.push(Field {
                        name: name.clone(),
                        descr: Descr::from_lit(descr, depth + 1)?,
                        shape,
                    });
                }
                Descr::Struct(fields)
            }
            _ => return Err(bad("descr is neither a string nor a list")),
        };
        d.validate()?;
        Ok(d)
    }

    fn validate(&self) -> Result<(), NpyError> {
        if let Descr::Struct(fields) = self {
            let mut names: Vec<&str> = fields
                .iter()
                .map(|f| f.name.as_str())
                .filter(|n| !n.is_empty())
                .collect();
            names.sort_unstable();
            if names.windows(2).any(|w| w.first() == w.get(1)) {
                return Err(NpyError::BadDescr("repeated field name".into()));
            }
            if fields.iter().any(|f| f.shape.len() > MAX_DIMS) {
                return Err(NpyError::BadDescr(
                    "subarray has too many dimensions".into(),
                ));
            }
        }
        self.itemsize().ok_or(NpyError::Overflow).map(|_| ())
    }

    /// Bytes per element: for a structured descriptor, the sum over fields (padding included)
    /// of the field's item size times its subarray's element count. `None` on overflow.
    pub fn itemsize(&self) -> Option<u64> {
        match self {
            Descr::Simple(s) => s.itemsize(),
            Descr::Struct(fields) => fields
                .iter()
                .try_fold(0u64, |acc, f| acc.checked_add(f.itemsize()?)),
        }
    }

    /// The simple descriptor, if this is one.
    pub fn as_simple(&self) -> Option<&Simple> {
        match self {
            Descr::Simple(s) => Some(s),
            Descr::Struct(_) => None,
        }
    }

    /// The typed class and byte order, for the kinds a reader offers typed access to.
    pub fn typed(&self) -> Option<(Typed, ByteOrder)> {
        let s = self.as_simple()?;
        Some((s.typed()?, s.order))
    }

    /// Python's `repr` of the descriptor, exactly as numpy writes it in a header: `'<f4'`, or
    /// `[('a', '<i4'), ('b', '<f8')]` (contract §5.4).
    ///
    /// Fails when a field name holds a character whose `repr` this crate can't reproduce with
    /// certainty (a backslash, a control character, or a non-ASCII character that isn't a
    /// letter or digit); the writer then declines the array.
    pub fn repr(&self) -> Result<String, NpyError> {
        let mut out = String::new();
        self.write_repr(&mut out, true)?;
        Ok(out)
    }

    /// The descriptor text a reader exposes. Python's `repr` where it can be reproduced; where
    /// it can't, the same spelling with such characters left as they are.
    pub fn text(&self) -> String {
        let mut out = String::new();
        // Lenient mode never fails.
        let _ = self.write_repr(&mut out, false);
        out
    }

    fn write_repr(&self, out: &mut String, strict: bool) -> Result<(), NpyError> {
        match self {
            Descr::Simple(s) => {
                out.push('\'');
                out.push_str(&s.to_string());
                out.push('\'');
            }
            Descr::Struct(fields) => {
                out.push('[');
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push('(');
                    py_repr_str(&f.name, out, strict)?;
                    out.push_str(", ");
                    f.descr.write_repr(out, strict)?;
                    if !f.shape.is_empty() {
                        out.push_str(", ");
                        out.push_str(&py_repr_tuple(&f.shape));
                    }
                    out.push(')');
                }
                out.push(']');
            }
        }
        Ok(())
    }

    /// Does any part of this descriptor (any field, recursively) use a non-Latin-1 character?
    pub(crate) fn needs_utf8(&self) -> bool {
        match self {
            Descr::Simple(_) => false,
            Descr::Struct(fields) => fields
                .iter()
                .any(|f| f.name.chars().any(|c| u32::from(c) > 0xff) || f.descr.needs_utf8()),
        }
    }
}

impl fmt::Display for Descr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// Python's `repr` of a tuple of integers: `()`, `(3,)`, `(2, 3)`.
pub(crate) fn py_repr_tuple(dims: &[u64]) -> String {
    match dims {
        [] => "()".to_owned(),
        [one] => format!("({one},)"),
        _ => {
            let parts: Vec<String> = dims.iter().map(u64::to_string).collect();
            format!("({})", parts.join(", "))
        }
    }
}

/// Is `c` printable in Python's sense (`str.isprintable`), as far as this crate can tell
/// without Unicode category tables? `Some(true)` printable, `Some(false)` not, `None` unknown.
fn printable(c: char) -> Option<bool> {
    let u = u32::from(c);
    match u {
        0x20..=0x7e => Some(true),
        0..=0x1f | 0x7f..=0xa0 | 0xad => Some(false),
        0xa1..=0xff => Some(true),
        _ if c.is_alphanumeric() => Some(true),
        _ => None,
    }
}

/// Python's `repr` of a `str`. In strict mode, fail rather than guess.
fn py_repr_str(s: &str, out: &mut String, strict: bool) -> Result<(), NpyError> {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        match (c, printable(c)) {
            ('\\', _) if strict => return Err(NpyError::UnreproducibleName),
            ('\\', _) => out.push_str("\\\\"),
            (c, _) if c == quote => {
                out.push('\\');
                out.push(c);
            }
            (c, Some(true)) => out.push(c),
            (_, _) if strict => return Err(NpyError::UnreproducibleName),
            (c, _) => out.push(c),
        }
    }
    out.push(quote);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_descriptors() {
        for (text, canon, size) in [
            ("<f4", "<f4", 4),
            ("|u1", "|u1", 1),
            ("<u1", "|u1", 1),
            (">i8", ">i8", 8),
            ("<c16", "<c16", 16),
            ("<M8[s]", "<M8[s]", 8),
            ("<m8[10ms]", "<m8[10ms]", 8),
            (">M8", ">M8", 8),
            ("<U3", "<U3", 12),
            ("|S4", "|S4", 4),
            ("<V4", "|V4", 4),
        ] {
            let s = Simple::parse(text).unwrap();
            assert_eq!(s.to_string(), canon, "{text}");
            assert_eq!(s.itemsize(), Some(size), "{text}");
        }
        for text in [
            "",
            "<",
            "f4",
            "=f4",
            "|f4",
            "<f3",
            "<f16",
            "<c32",
            "<b2",
            "<M8[x]",
            "<f4[s]",
            "<i4 ",
            "<U99999999999999999999",
            "<M4",
        ] {
            assert!(Simple::parse(text).is_err(), "{text:?}");
        }
        assert_eq!(Simple::parse("|O"), Err(NpyError::ObjectDtype));
        assert_eq!(Simple::parse("|O8"), Err(NpyError::ObjectDtype));
    }

    #[test]
    fn structured_repr_round_trips() {
        for text in [
            "[('a', '<i4'), ('b', '<f8')]",
            "[('rgb', '|u1', (3,)), ('w', '<f4')]",
            "[('a', '|u1'), ('', '|V3'), ('b', '<i4')]",
            "[('p', [('x', '<f4'), ('y', '<f4')]), ('id', '<u2')]",
            "[(\"it's\", '<i2')]",
            "[('m', '<f4', (2, 3))]",
        ] {
            let d = Descr::parse(text).unwrap();
            assert_eq!(d.repr().unwrap(), text);
        }
        let both = Descr::structured(vec![Field::new(
            "it's \"x\"",
            Descr::parse("<i2").unwrap(),
            vec![],
        )])
        .unwrap();
        assert_eq!(both.repr().unwrap(), r#"[('it\'s "x"', '<i2')]"#);
    }

    #[test]
    fn unreproducible_names_are_refused_by_the_writer_only() {
        for name in ["a\\b", "tab\there", "\u{200b}", "\u{85}"] {
            let d = Descr::structured(vec![Field::new(name, Descr::parse("<i2").unwrap(), vec![])])
                .unwrap();
            assert_eq!(d.repr(), Err(NpyError::UnreproducibleName), "{name:?}");
            assert!(!d.text().is_empty());
        }
    }

    #[test]
    fn bad_structures() {
        for text in [
            "[('a', '<i4'), ('a', '<f8')]",
            "[(('t', 'a'), '<i4')]",
            "[('a', '|O')]",
            "[('a',)]",
            "[('a', '<i4', 'x')]",
            "[('a', '|u1', (4294967296, 4294967296, 4294967296))]",
        ] {
            assert!(Descr::parse(text).is_err(), "{text}");
        }
    }
}

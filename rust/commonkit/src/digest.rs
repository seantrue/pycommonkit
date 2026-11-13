//! The hashing facade (contract §3.8, feature `digest`): the one place that computes digests,
//! in the algorithm the runtime resolved once ([`current`]: `HASH`, then `COMMONKIT_HASH`, then
//! `b3`).
//!
//! The maintainer, D19: "REALLY,REALLY,REALLY try not to use system libraries directly if they
//! are implementations of swappable algorithms. Hashing is a clear one." So adopters depend
//! on this, not on `md-5`, `blake3` or `sha2`, and the default functions name no algorithm.
//!
//! - [`raw`], [`Hasher`] (streaming), [`file()`]: a digest of bytes.
//! - [`nd1`]: PlumCraft's protocol-2 array recipe, verbatim (contract §3.8): the normalised
//!   dtype string, then the shape's Python `repr`, then the C-order bytes; a big-endian source
//!   is swapped to little-endian first, in 16 MiB pieces.
//! - [`fixed`]: a named algorithm for what must not follow the switch (D9): goldens, parity,
//!   security and formats. Allowlisted uses only.
//! - [`build`]: both algorithms at once, for a build script, which runs before the runtime's
//!   algorithm is known (D4). Allowed only in `build.rs` files.
//!
//! The store never calls this module: commonkit never hashes a caller's key.

use std::fmt;
use std::io::{self, Read};
use std::path::Path;

pub use crate::policy::{Algo, current};

/// Above this many bytes in one update, BLAKE3 hashes on several threads.
const B3_PARALLEL: usize = 1 << 20;

/// The big-endian swap works on this many bytes at a time (a multiple of every swap unit).
const SWAP_CHUNK: usize = 16 << 20;

/// The digest algorithms the facade computes: the switchable two and `fixed`'s SHA-256.
#[derive(Clone, Copy)]
enum Any {
    Md5,
    B3,
    Sha256,
}

impl From<Algo> for Any {
    fn from(a: Algo) -> Any {
        match a {
            Algo::Md5 => Any::Md5,
            _ => Any::B3,
        }
    }
}

enum Inner {
    Md5(md5::Md5),
    B3(Box<blake3::Hasher>),
    Sha256(sha2::Sha256),
}

impl Inner {
    fn new(a: Any) -> Inner {
        use sha2::Digest;
        match a {
            Any::Md5 => Inner::Md5(md5::Md5::new()),
            Any::B3 => Inner::B3(Box::new(blake3::Hasher::new())),
            Any::Sha256 => Inner::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        use sha2::Digest;
        match self {
            Inner::Md5(h) => h.update(bytes),
            Inner::B3(h) if bytes.len() >= B3_PARALLEL => {
                h.update_rayon(bytes);
            }
            Inner::B3(h) => {
                h.update(bytes);
            }
            Inner::Sha256(h) => h.update(bytes),
        }
    }

    fn finish(self) -> String {
        use sha2::Digest;
        match self {
            Inner::Md5(h) => hex(&h.finalize()),
            Inner::B3(h) => h.finalize().to_hex().to_string(),
            Inner::Sha256(h) => hex(&h.finalize()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    s
}

/// A digest in progress, in the current algorithm.
pub struct Hasher(Inner);

impl Hasher {
    /// A hasher in [`current`]'s algorithm.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Hasher {
        Hasher(Inner::new(current().into()))
    }

    pub fn update(&mut self, bytes: &[u8]) -> &mut Hasher {
        self.0.update(bytes);
        self
    }

    /// The lower-case hex digest.
    pub fn finish(self) -> String {
        self.0.finish()
    }
}

fn raw_in(a: Any, bytes: &[u8]) -> String {
    let mut h = Inner::new(a);
    h.update(bytes);
    h.finish()
}

/// The digest of `bytes` in the current algorithm.
pub fn raw(bytes: &[u8]) -> String {
    raw_in(current().into(), bytes)
}

/// `(digest, size)` of a file, streamed in the current algorithm. An error when it is, or
/// grows while it is read, over `max` bytes.
pub fn file(path: &Path, max: u64) -> io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    if f.metadata()?.len() > max {
        return Err(io::Error::other(format!(
            "{} is over {max} bytes",
            path.display()
        )));
    }
    let mut h = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > max {
            return Err(io::Error::other(
                "the file grew past the limit while it was read",
            ));
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok((h.finish(), total))
}

// ------------------------------------------------------------------------------ nd1

/// Why an array has no `nd1` digest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Nd1Error {
    /// Object, structured or unreadable dtypes have no content identity.
    Dtype(String),
    /// The bytes don't match the dtype and shape.
    Length { got: usize, expected: Option<usize> },
}

impl fmt::Display for Nd1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Nd1Error::Dtype(d) => write!(f, "no content identity for dtype {d:?}"),
            Nd1Error::Length {
                got,
                expected: Some(e),
            } => {
                write!(f, "array bytes ({got}) don't match dtype and shape ({e})")
            }
            Nd1Error::Length { got, .. } => write!(f, "array bytes ({got}): shape overflows"),
        }
    }
}

impl std::error::Error for Nd1Error {}

/// A numpy `dtype.str`, as far as hashing needs it (PlumCraft's `Dtype::parse`).
struct Dtype {
    normalised: String,
    itemsize: usize,
    /// Bytes per byte-swapped unit for a big-endian source (1: no swap).
    swap_unit: usize,
}

impl Dtype {
    fn parse(s: &str) -> Result<Dtype, Nd1Error> {
        let bad = || Nd1Error::Dtype(s.chars().take(32).collect());
        let mut chars = s.chars();
        let order = chars.next().ok_or_else(bad)?;
        let kind = chars.next().ok_or_else(bad)?;
        let rest = chars.as_str();
        let (digits, suffix) = rest.find('[').map_or((rest, ""), |i| rest.split_at(i));
        if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let n: usize = digits.parse().map_err(|_| bad())?;
        if !suffix.is_empty()
            && !(matches!(kind, 'M' | 'm') && suffix.ends_with(']') && suffix.len() <= 8)
        {
            return Err(bad());
        }
        let (itemsize, unit) = match kind {
            'b' | 'i' | 'u' | 'f' | 'M' | 'm' => (n, n),
            'c' => (n, n / 2),
            'U' => (n.checked_mul(4).ok_or_else(bad)?, 4),
            'S' => (n, 1),
            _ => return Err(bad()),
        };
        if itemsize == 0 && kind != 'S' && kind != 'U' {
            return Err(bad());
        }
        let (order, swap_unit) = match order {
            '<' | '|' => (order, 1),
            '=' => ('<', 1),
            '>' if unit > 1 => ('<', unit),
            '>' => ('|', 1),
            _ => return Err(bad()),
        };
        Ok(Dtype {
            normalised: format!("{order}{kind}{digits}{suffix}"),
            itemsize,
            swap_unit,
        })
    }
}

/// Python's `repr` of a shape tuple: `()`, `(5,)`, `(812, 808, 3)`.
fn shape_repr(shape: &[u64]) -> String {
    match shape {
        [] => "()".into(),
        [one] => format!("({one},)"),
        many => format!(
            "({})",
            many.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn nd1_in(a: Any, dtype: &str, shape: &[u64], bytes: &[u8]) -> Result<String, Nd1Error> {
    let d = Dtype::parse(dtype)?;
    let overflow = Nd1Error::Length {
        got: bytes.len(),
        expected: None,
    };
    let count = shape
        .iter()
        .try_fold(1u64, |n, &s| n.checked_mul(s))
        .ok_or(overflow.clone())?;
    let expected = usize::try_from(count)
        .ok()
        .and_then(|c| c.checked_mul(d.itemsize))
        .ok_or(overflow)?;
    if bytes.len() != expected {
        return Err(Nd1Error::Length {
            got: bytes.len(),
            expected: Some(expected),
        });
    }
    let mut h = Inner::new(a);
    h.update(d.normalised.as_bytes());
    h.update(shape_repr(shape).as_bytes());
    if d.swap_unit > 1 {
        let mut piece = Vec::with_capacity(SWAP_CHUNK.min(bytes.len()));
        for chunk in bytes.chunks(SWAP_CHUNK) {
            piece.clear();
            piece.extend_from_slice(chunk);
            for unit in piece.chunks_exact_mut(d.swap_unit) {
                unit.reverse();
            }
            h.update(&piece);
        }
    } else {
        h.update(bytes);
    }
    Ok(h.finish())
}

/// The `nd1` digest, in the current algorithm, of an array as numpy describes it: `dtype` is
/// its `dtype.str` (`<f4`, `|u1`, `>c8`, `<U3`, `<M8[ns]`), `shape` its dimensions, `bytes`
/// its C-order element bytes.
pub fn nd1(dtype: &str, shape: &[u64], bytes: &[u8]) -> Result<String, Nd1Error> {
    nd1_in(current().into(), dtype, shape, bytes)
}

/// A named algorithm, for digests that must NOT follow the switch (D9): goldens, parity
/// records, security and file formats. Every use is allowlisted where it is made.
pub mod fixed {
    use super::{Any, Inner, Nd1Error};

    /// The fixed algorithms.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Fixed {
        Md5,
        Sha256,
    }

    impl From<Fixed> for Any {
        fn from(f: Fixed) -> Any {
            match f {
                Fixed::Md5 => Any::Md5,
                Fixed::Sha256 => Any::Sha256,
            }
        }
    }

    /// The digest of `bytes` in `a`.
    pub fn raw(a: Fixed, bytes: &[u8]) -> String {
        super::raw_in(a.into(), bytes)
    }

    /// The `nd1` digest in `a` (see [`super::nd1`]).
    pub fn nd1(a: Fixed, dtype: &str, shape: &[u64], bytes: &[u8]) -> Result<String, Nd1Error> {
        super::nd1_in(a.into(), dtype, shape, bytes)
    }

    /// A digest in progress in `a`.
    pub struct Hasher(Inner);

    impl Hasher {
        pub fn new(a: Fixed) -> Hasher {
            Hasher(Inner::new(a.into()))
        }

        pub fn update(&mut self, bytes: &[u8]) -> &mut Hasher {
            self.0.update(bytes);
            self
        }

        pub fn finish(self) -> String {
            self.0.finish()
        }
    }
}

/// For a value computed before the runtime's algorithm is known: a build script bakes both
/// and the runtime picks (D4). Allowed only in `build.rs` files.
pub mod build {
    use super::{Algo, Any, current};

    /// One input's digest in both algorithms.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Salts {
        pub md5: String,
        pub b3: String,
    }

    /// `bytes` in md5 and in b3.
    pub fn both(bytes: &[u8]) -> Salts {
        Salts {
            md5: super::raw_in(Any::Md5, bytes),
            b3: super::raw_in(Any::B3, bytes),
        }
    }

    /// The salt for [`current`]'s algorithm.
    pub fn pick(s: &Salts) -> &str {
        match current() {
            Algo::Md5 => &s.md5,
            _ => &s.b3,
        }
    }
}

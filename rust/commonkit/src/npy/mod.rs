//! The `.npy` segment format (contract §5): reading a header from hostile bytes, and writing
//! the exact header bytes numpy writes.
//!
//! ```text
//! \x93NUMPY <major> <minor> <HLEN: u16 LE (v1) | u32 LE (v2, v3)> <header text> <payload>
//! ```
//!
//! The reader accepts versions 1 to 3, both byte orders, Fortran order, trailing bytes after
//! the payload, and headers up to [`MAX_HEADER_LEN`]. Every way a file can be torn (contract
//! §5.7) is an [`NpyError`], never a panic: lengths are bounded before anything is read,
//! arithmetic is checked, and nothing is allocated beyond the bytes actually present.
//!
//! The parser's shape is ported from the viewer app's `shm` crate reader, widened to the whole of
//! contract §5.4 (structured, complex, datetime, text and void descriptors) and corrected
//! where Python's literal grammar differs (`(3)` is not a tuple).

mod descr;
mod literal;

use std::fmt;

pub use descr::{ByteOrder, Descr, Field, Kind, Simple, Typed};
use literal::Lit;

/// The magic string at the start of every `.npy` file.
pub const MAGIC: [u8; 6] = *b"\x93NUMPY";

/// The longest header a reader accepts and a writer produces: `HLEN <= 2^20` (contract §5.5).
pub const MAX_HEADER_LEN: u32 = 1 << 20;

/// The most dimensions accepted. Contract §5.5 requires 32 (numpy 1.x) and recommends 64
/// (numpy 2), which this crate takes.
pub const MAX_DIMS: usize = 64;

/// The data offset is a multiple of this (numpy's `ARRAY_ALIGN`).
pub const ARRAY_ALIGN: usize = 64;

/// numpy's `GROWTH_AXIS_MAX_DIGITS`: room left in the header for the growth axis to grow.
pub const GROWTH_AXIS_MAX_DIGITS: usize = 21;

/// Why bytes are not a whole `.npy` header, or why a header can't be written. On the read path
/// every one of these means torn (contract §5.7); the variant is detail for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NpyError {
    /// The bytes end before the prefix, the header or the payload does.
    Truncated { need: u64, have: u64 },
    /// The first six bytes are not `\x93NUMPY`.
    BadMagic,
    /// A version other than 1.0, 2.0 or 3.0.
    UnsupportedVersion(u8, u8),
    /// `HLEN` exceeds [`MAX_HEADER_LEN`].
    HeaderTooLarge(u64),
    /// The header text is not valid in its version's encoding.
    BadEncoding,
    /// The header text is not the dict numpy writes.
    BadHeader(String),
    /// `descr` is not a descriptor of contract §5.4.
    BadDescr(String),
    /// `descr` is or contains an object dtype, which is never read or written.
    ObjectDtype,
    /// `shape` is not a tuple of at most [`MAX_DIMS`] non-negative integers.
    BadShape,
    /// The item size, the payload size or the file size overflows.
    Overflow,
    /// The writer can't reproduce Python's `repr` of a field name exactly.
    UnreproducibleName,
    /// The payload handed to the writer is not `itemsize × product(shape)` bytes.
    PayloadLength { need: u64, have: u64 },
}

impl fmt::Display for NpyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NpyError::Truncated { need, have } => {
                write!(f, "truncated .npy: need {need} bytes, have {have}")
            }
            NpyError::BadMagic => f.write_str("not a .npy file (bad magic)"),
            NpyError::UnsupportedVersion(a, b) => write!(f, "unsupported .npy version {a}.{b}"),
            NpyError::HeaderTooLarge(n) => write!(f, ".npy header too large ({n} bytes)"),
            NpyError::BadEncoding => f.write_str(".npy header text does not decode"),
            NpyError::BadHeader(why) => write!(f, "malformed .npy header: {why}"),
            NpyError::BadDescr(d) => write!(f, "unsupported .npy descriptor {d:?}"),
            NpyError::ObjectDtype => f.write_str("object dtypes are never read or written"),
            NpyError::BadShape => f.write_str(".npy shape is not a tuple of non-negative ints"),
            NpyError::Overflow => f.write_str(".npy size overflows"),
            NpyError::UnreproducibleName => {
                f.write_str("a field name whose Python repr can't be reproduced exactly")
            }
            NpyError::PayloadLength { need, have } => {
                write!(f, "payload is {have} bytes, the header needs {need}")
            }
        }
    }
}

impl std::error::Error for NpyError {}

fn len64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// The fixed-size start of a `.npy` file (contract §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    /// `(major, minor)`: `(1, 0)`, `(2, 0)` or `(3, 0)`.
    pub version: (u8, u8),
    /// `L`: 10 for version 1, 12 otherwise.
    pub prefix_len: usize,
    /// `HLEN`, at most [`MAX_HEADER_LEN`].
    pub header_len: u32,
}

impl Prefix {
    /// The longest prefix: read this many bytes to be sure of having one.
    pub const MAX_LEN: usize = 12;

    /// `L + HLEN`: where the payload starts.
    pub fn data_offset(&self) -> usize {
        // Both terms are bounded (12 and 2^20), so this can't overflow.
        self.prefix_len.saturating_add(self.header_len as usize)
    }
}

/// Read the prefix: magic, version and `HLEN`, refusing an `HLEN` over the limit before any
/// header text is read (contract §5.5).
pub fn read_prefix(bytes: &[u8]) -> Result<Prefix, NpyError> {
    let have = len64(bytes.len());
    let head = bytes.get(..MAGIC.len().min(bytes.len())).unwrap_or(&[]);
    if MAGIC.get(..head.len()) != Some(head) {
        return Err(NpyError::BadMagic);
    }
    let (Some(&major), Some(&minor)) = (bytes.get(6), bytes.get(7)) else {
        return Err(NpyError::Truncated { need: 8, have });
    };
    let prefix_len = match (major, minor) {
        (1, 0) => 10,
        (2, 0) | (3, 0) => 12,
        _ => return Err(NpyError::UnsupportedVersion(major, minor)),
    };
    let raw = bytes.get(8..prefix_len).ok_or(NpyError::Truncated {
        need: len64(prefix_len),
        have,
    })?;
    let header_len = raw
        .iter()
        .rev()
        .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    if header_len > u64::from(MAX_HEADER_LEN) {
        return Err(NpyError::HeaderTooLarge(header_len));
    }
    Ok(Prefix {
        version: (major, minor),
        prefix_len,
        header_len: u32::try_from(header_len).map_err(|_| NpyError::HeaderTooLarge(header_len))?,
    })
}

/// A parsed, validated `.npy` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    prefix: Prefix,
    descr: Descr,
    descr_text: String,
    fortran_order: bool,
    shape: Vec<u64>,
    itemsize: u64,
    nbytes: u64,
    payload_end: u64,
    text: String,
}

impl Header {
    pub fn version(&self) -> (u8, u8) {
        self.prefix.version
    }

    /// `HLEN`.
    pub fn header_len(&self) -> u32 {
        self.prefix.header_len
    }

    /// Where the payload starts (`L + HLEN`). Not necessarily aligned (contract §5.6).
    pub fn data_offset(&self) -> usize {
        self.prefix.data_offset()
    }

    pub fn descr(&self) -> &Descr {
        &self.descr
    }

    /// The descriptor as Python's `repr` writes it (`'<f4'`, `[('a', '<i4')]`), the text a
    /// reader exposes beside the raw bytes of an untyped kind.
    pub fn descr_text(&self) -> &str {
        &self.descr_text
    }

    pub fn fortran_order(&self) -> bool {
        self.fortran_order
    }

    pub fn shape(&self) -> &[u64] {
        &self.shape
    }

    /// Bytes per element.
    pub fn itemsize(&self) -> u64 {
        self.itemsize
    }

    /// Payload bytes: `itemsize × product(shape)`.
    pub fn nbytes(&self) -> u64 {
        self.nbytes
    }

    /// `data_offset + nbytes`: a whole file is at least this long.
    pub fn payload_end(&self) -> u64 {
        self.payload_end
    }

    /// The header text with its trailing spaces and newline removed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Is a file of `file_len` bytes long enough for this header's payload? A shorter one is a
    /// short payload, torn (contract §5.7). Longer is fine: trailing bytes are ignored.
    pub fn check_len(&self, file_len: u64) -> Result<(), NpyError> {
        if file_len < self.payload_end {
            return Err(NpyError::Truncated {
                need: self.payload_end,
                have: file_len,
            });
        }
        Ok(())
    }
}

fn decode(text: &[u8], version: u8) -> Result<String, NpyError> {
    if version == 3 {
        return std::str::from_utf8(text)
            .map(str::to_owned)
            .map_err(|_| NpyError::BadEncoding);
    }
    // Latin-1: every byte is the code point of the same value.
    Ok(text.iter().map(|&b| char::from(b)).collect())
}

/// Parse the header at the start of `bytes`, which may hold just the prefix and header or the
/// whole file. The payload is not checked: see [`Header::check_len`] and [`parse_npy`].
pub fn parse_header(bytes: &[u8]) -> Result<Header, NpyError> {
    let prefix = read_prefix(bytes)?;
    let data_offset = prefix.data_offset();
    let raw = bytes
        .get(prefix.prefix_len..data_offset)
        .ok_or(NpyError::Truncated {
            need: len64(data_offset),
            have: len64(bytes.len()),
        })?;
    let decoded = decode(raw, prefix.version.0)?;
    let Lit::Dict(items) = literal::parse(&decoded)? else {
        return Err(NpyError::BadHeader("not a dict".into()));
    };
    if items.len() != 3 {
        return Err(NpyError::BadHeader(format!("{} keys, not 3", items.len())));
    }
    let field = |name: &str| {
        let mut found = items.iter().filter(|(k, _)| k == name).map(|(_, v)| v);
        match (found.next(), found.next()) {
            (Some(v), None) => Ok(v),
            _ => Err(NpyError::BadHeader(format!("no single {name:?}"))),
        }
    };
    let descr = Descr::from_lit(field("descr")?, 0)?;
    let Lit::Bool(fortran_order) = *field("fortran_order")? else {
        return Err(NpyError::BadHeader(
            "fortran_order is not True or False".into(),
        ));
    };
    let Lit::Tuple(dims) = field("shape")? else {
        return Err(NpyError::BadShape);
    };
    if dims.len() > MAX_DIMS {
        return Err(NpyError::BadShape);
    }
    let shape = dims
        .iter()
        .map(|d| match d {
            Lit::Int(n) => Ok(*n),
            _ => Err(NpyError::BadShape),
        })
        .collect::<Result<Vec<u64>, _>>()?;
    let itemsize = descr.itemsize().ok_or(NpyError::Overflow)?;
    let nbytes = shape
        .iter()
        .try_fold(itemsize, |acc, d| acc.checked_mul(*d))
        .ok_or(NpyError::Overflow)?;
    let payload_end = len64(data_offset)
        .checked_add(nbytes)
        .ok_or(NpyError::Overflow)?;
    let text = decoded.trim_end_matches([' ', '\n']).to_owned();
    Ok(Header {
        prefix,
        descr_text: descr.text(),
        descr,
        fortran_order,
        shape,
        itemsize,
        nbytes,
        payload_end,
        text,
    })
}

/// Parse a whole file held in memory: the header, and exactly its payload. A short payload is
/// an error; bytes after the payload are ignored (contract §5.7, C6(d)).
pub fn parse_npy(bytes: &[u8]) -> Result<(Header, &[u8]), NpyError> {
    let header = parse_header(bytes)?;
    header.check_len(len64(bytes.len()))?;
    let end = usize::try_from(header.payload_end).map_err(|_| NpyError::Overflow)?;
    let payload = bytes
        .get(header.data_offset()..end)
        .ok_or(NpyError::Overflow)?;
    Ok((header, payload))
}

/// The memory order an array's payload is laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Order {
    C,
    F,
}

/// The `fortran_order` a writer puts in the header for an array of `shape` laid out in
/// `order`: `True` only for an array that is Fortran-contiguous and not also C-contiguous
/// (contract §5.3). An array with no elements, or with at most one dimension longer than 1,
/// is both, so it is written `False` whatever the order.
pub fn fortran_flag(shape: &[u64], order: Order) -> bool {
    order == Order::F && !shape.contains(&0) && shape.iter().filter(|&&d| d > 1).count() >= 2
}

/// The payload size for `descr` and `shape`, or `None` on overflow.
pub fn nbytes(descr: &Descr, shape: &[u64]) -> Option<u64> {
    shape
        .iter()
        .try_fold(descr.itemsize()?, |acc, d| acc.checked_mul(*d))
}

/// The prefix and header numpy writes for an array of `descr` and `shape` laid out in `order`,
/// byte for byte (contract §5.2): the dict literal, growth-axis padding, alignment padding to a
/// multiple of 64 (1 to 64 spaces, never 0), a newline, and the lowest version that holds it.
/// The payload follows at the returned length.
pub fn encode_header(descr: &Descr, shape: &[u64], order: Order) -> Result<Vec<u8>, NpyError> {
    if shape.len() > MAX_DIMS {
        return Err(NpyError::BadShape);
    }
    nbytes(descr, shape).ok_or(NpyError::Overflow)?;
    let fortran = fortran_flag(shape, order);
    let dict = format!(
        "{{'descr': {}, 'fortran_order': {}, 'shape': {}, }}",
        descr.repr()?,
        if fortran { "True" } else { "False" },
        descr::py_repr_tuple(shape),
    );
    let growth_axis = if fortran { shape.last() } else { shape.first() };
    let growth = growth_axis.map_or(0, |g| {
        GROWTH_AXIS_MAX_DIGITS.saturating_sub(g.to_string().len())
    });
    let utf8 = descr.needs_utf8();
    let encoded: Vec<u8> = if utf8 {
        dict.into_bytes()
    } else {
        // Every char is at most U+00FF here, so each is one Latin-1 byte.
        dict.chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
            .collect()
    };
    let unpadded = encoded
        .len()
        .checked_add(growth)
        .and_then(|n| n.checked_add(1))
        .ok_or(NpyError::Overflow)?;
    let layout = |prefix_len: usize| {
        let pad = ARRAY_ALIGN - (prefix_len + unpadded) % ARRAY_ALIGN;
        (pad, unpadded + pad)
    };
    let (major, prefix_len, (pad, hlen)) = match (utf8, layout(10)) {
        (false, (pad, hlen)) if hlen <= usize::from(u16::MAX) => (1u8, 10, (pad, hlen)),
        (false, _) => (2, 12, layout(12)),
        (true, _) => (3, 12, layout(12)),
    };
    let hlen32 = u32::try_from(hlen).map_err(|_| NpyError::HeaderTooLarge(len64(hlen)))?;
    if hlen32 > MAX_HEADER_LEN {
        return Err(NpyError::HeaderTooLarge(len64(hlen)));
    }
    let mut out = Vec::with_capacity(prefix_len + hlen);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&[major, 0]);
    if major == 1 {
        out.extend_from_slice(&u16::try_from(hlen32).unwrap_or(u16::MAX).to_le_bytes());
    } else {
        out.extend_from_slice(&hlen32.to_le_bytes());
    }
    out.extend_from_slice(&encoded);
    out.resize(out.len() + growth + pad, b' ');
    out.push(b'\n');
    Ok(out)
}

/// A whole `.npy` file in memory: [`encode_header`] followed by `payload`, which must be exactly
/// `itemsize × product(shape)` bytes laid out in `order`.
pub fn encode_npy(
    descr: &Descr,
    shape: &[u64],
    order: Order,
    payload: &[u8],
) -> Result<Vec<u8>, NpyError> {
    let need = nbytes(descr, shape).ok_or(NpyError::Overflow)?;
    if need != len64(payload.len()) {
        return Err(NpyError::PayloadLength {
            need,
            have: len64(payload.len()),
        });
    }
    let mut out = encode_header(descr, shape, order)?;
    out.extend_from_slice(payload);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(version: u8, text: &str) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&[version, 0]);
        if version == 1 {
            v.extend_from_slice(&(text.len() as u16).to_le_bytes());
        } else {
            v.extend_from_slice(&(text.len() as u32).to_le_bytes());
        }
        v.extend_from_slice(text.as_bytes());
        v
    }

    #[test]
    fn round_trip_simple() {
        let d = Descr::parse("<f4").unwrap();
        let h = encode_header(&d, &[3, 4], Order::C).unwrap();
        assert_eq!(h.len() % 64, 0);
        let p = parse_header(&h).unwrap();
        assert_eq!(p.version(), (1, 0));
        assert_eq!(p.shape(), [3, 4]);
        assert_eq!(p.nbytes(), 48);
        assert_eq!(p.descr_text(), "'<f4'");
        assert_eq!(p.descr().typed(), Some((Typed::F32, ByteOrder::Little)));
        assert_eq!(p.data_offset(), h.len());
    }

    #[test]
    fn other_spellings_are_read() {
        let p = parse_header(&raw(
            2,
            "{\"shape\": (2, 2), \"fortran_order\": True, \"descr\": \">i8\"}",
        ))
        .unwrap();
        assert_eq!(
            (p.version(), p.nbytes(), p.fortran_order()),
            ((2, 0), 32, true)
        );
        // Aligned to 16 as numpy before 1.9 wrote, with no final newline.
        let p = parse_header(&raw(
            1,
            "{'descr': '<u2', 'fortran_order': False, 'shape': (), }   ",
        ))
        .unwrap();
        assert_eq!((p.shape().len(), p.nbytes()), (0, 2));
    }

    #[test]
    fn torn_headers() {
        for text in [
            "{'descr': '<f4', 'fortran_order': False}",
            "{'descr': '<f4', 'fortran_order': False, 'shape': (1,), 'x': 1}",
            "{'descr': '<f4', 'descr': '<f4', 'shape': (1,)}",
            "{'descr': '<f4', 'fortran_order': 0, 'shape': (1,)}",
            "{'descr': '<f4', 'fortran_order': False, 'shape': (3)}",
            "{'descr': '<f4', 'fortran_order': False, 'shape': [3]}",
            "{'descr': '<f4', 'fortran_order': False, 'shape': (True,)}",
            "{'descr': '|O', 'fortran_order': False, 'shape': (1,)}",
            "{'descr': [('a', '|O')], 'fortran_order': False, 'shape': (1,)}",
            "{'descr': '<f8', 'fortran_order': False, 'shape': (4294967296, 4294967296)}",
            "[]",
        ] {
            assert!(parse_header(&raw(1, text)).is_err(), "{text}");
        }
        let many = format!(
            "{{'descr': '|u1', 'fortran_order': False, 'shape': ({}), }}",
            "1, ".repeat(65)
        );
        assert_eq!(parse_header(&raw(1, &many)), Err(NpyError::BadShape));
        assert_eq!(read_prefix(b"\x93NUMPZ\x01\x00"), Err(NpyError::BadMagic));
        assert_eq!(
            read_prefix(b"\x93NU"),
            Err(NpyError::Truncated { need: 8, have: 3 })
        );
        assert_eq!(
            read_prefix(b"\x93NUMPY\x01\x01\x00\x00"),
            Err(NpyError::UnsupportedVersion(1, 1))
        );
        let mut big = MAGIC.to_vec();
        big.extend_from_slice(&[2, 0]);
        big.extend_from_slice(&(MAX_HEADER_LEN + 1).to_le_bytes());
        assert_eq!(
            read_prefix(&big),
            Err(NpyError::HeaderTooLarge(u64::from(MAX_HEADER_LEN) + 1))
        );
        // Latin-1 is not UTF-8, but version 3 is.
        let mut v3 = raw(3, "{'descr': [('");
        v3.push(0xff);
        v3.extend_from_slice(b"', '<i4')], 'fortran_order': False, 'shape': (1,), }");
        let len = (v3.len() - 12) as u32;
        v3.splice(8..12, len.to_le_bytes());
        assert_eq!(parse_header(&v3), Err(NpyError::BadEncoding));
        v3[6] = 1;
        v3.splice(8..12, (len as u16).to_le_bytes());
        assert_eq!(parse_header(&v3).unwrap().version(), (1, 0));
    }

    #[test]
    fn payload_and_trailing() {
        let d = Descr::parse("|u1").unwrap();
        let file = encode_npy(&d, &[4], Order::C, &[1, 2, 3, 4]).unwrap();
        let (_, payload) = parse_npy(&file).unwrap();
        assert_eq!(payload, [1, 2, 3, 4]);
        let mut long = file.clone();
        long.extend_from_slice(b"extra");
        assert_eq!(parse_npy(&long).unwrap().1, [1, 2, 3, 4]);
        assert!(matches!(
            parse_npy(&file[..file.len() - 1]),
            Err(NpyError::Truncated { .. })
        ));
        assert!(matches!(
            encode_npy(&d, &[4], Order::C, &[1, 2, 3]),
            Err(NpyError::PayloadLength { need: 4, have: 3 })
        ));
    }

    #[test]
    fn writer_declines() {
        let d = Descr::parse("|u1").unwrap();
        assert_eq!(
            encode_header(&d, &[1; 65], Order::C),
            Err(NpyError::BadShape)
        );
        assert_eq!(
            encode_header(&d, &[u64::MAX, 2], Order::C),
            Err(NpyError::Overflow)
        );
        // A header over 1 MiB: 80000 fields of 13+ bytes each.
        let fields = (0..80_000)
            .map(|i| Field::new(format!("f{i:05}"), d.clone(), vec![]))
            .collect();
        let wide = Descr::structured(fields).unwrap();
        assert!(matches!(
            encode_header(&wide, &[0], Order::C),
            Err(NpyError::HeaderTooLarge(_))
        ));
    }
}

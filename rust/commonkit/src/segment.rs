//! An attached segment: a parsed header and its payload, mapped read-only or copied
//! (contract §8.1), with typed views and a single-element probe.
//!
//! The typed views and the probe are ported from the viewer app's `shm` crate (`as_f32`/`as_f64`
//! became `as_slice::<T>`; `element.rs` as it was, with `u64` indices to match shapes here).
//! Zero-copy views exist only for native byte order at an aligned offset; everything else is
//! read through `element`, `to_f32_vec` or `to_f64_vec`, which handle either byte order.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[cfg(feature = "mmap")]
use crate::map::Map;
use crate::npy::{self, ByteOrder, Descr, Header, Prefix, Typed};

/// How a store hands out segments (contract §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AttachMode {
    /// Map the file read-only: every process shares one physical copy. Without the `mmap`
    /// feature this falls back to [`AttachMode::Read`].
    #[default]
    Mmap,
    /// Copy the payload into private memory.
    Read,
}

impl AttachMode {
    /// Whether this build maps for this mode: [`AttachMode::Mmap`] with the `mmap` feature.
    pub fn maps(self) -> bool {
        cfg!(feature = "mmap") && self == AttachMode::Mmap
    }
}

#[derive(Debug)]
enum Data {
    /// The whole file; the payload is `header.data_offset()..header.payload_end()` of it.
    #[cfg(feature = "mmap")]
    Mapped(Map),
    /// The payload alone, in `u64` words so that every typed view is aligned.
    Owned { words: Vec<u64>, len: usize },
}

/// A whole segment, read-only. Dropping it unmaps or frees it.
#[derive(Debug)]
pub struct Segment {
    header: Header,
    data: Data,
}

/// What a path held.
pub(crate) enum Load {
    /// Nothing at the path (`ENOENT`): a plain miss.
    Absent,
    /// Something that is not a whole segment, or that could not be opened, mapped or read
    /// (contract §5.7, §8.1).
    Torn,
    Whole(Segment),
}

impl Segment {
    /// Open the `.npy` file at `path` outside any store: the viewer app's fallback files,
    /// PlumCraft's `path` locators. No key check, no counter, no touch; the caller decides what to
    /// trust.
    /// `None` when the file is absent or not a whole `.npy` file.
    pub fn open(path: impl AsRef<Path>, attach: AttachMode) -> Option<Segment> {
        match load(path.as_ref(), attach) {
            Load::Whole(s) => Some(s),
            Load::Absent | Load::Torn => None,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn descr(&self) -> &Descr {
        self.header.descr()
    }

    /// The descriptor as Python's `repr` writes it (`'<f4'`, `[('a', '<i4')]`): what to show
    /// beside the raw bytes of a kind with no typed access (contract §5.4).
    pub fn descr_text(&self) -> &str {
        self.header.descr_text()
    }

    /// The typed class and byte order, for the kinds that have typed access.
    pub fn dtype(&self) -> Option<(Typed, ByteOrder)> {
        self.header.descr().typed()
    }

    pub fn shape(&self) -> &[u64] {
        self.header.shape()
    }

    /// The header's `fortran_order`: the payload is in Fortran order.
    pub fn fortran_order(&self) -> bool {
        self.header.fortran_order()
    }

    /// Bytes per element.
    pub fn itemsize(&self) -> u64 {
        self.header.itemsize()
    }

    /// The number of elements.
    pub fn len(&self) -> u64 {
        self.header.shape().iter().product()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Is the payload a read-only mapping of the file (rather than a private copy)?
    pub fn is_mapped(&self) -> bool {
        match self.data {
            #[cfg(feature = "mmap")]
            Data::Mapped(_) => true,
            Data::Owned { .. } => false,
        }
    }

    /// The payload bytes, exactly `itemsize × product(shape)` of them; trailing bytes in the
    /// file are not included.
    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            #[cfg(feature = "mmap")]
            Data::Mapped(map) => {
                let start = self.header.data_offset();
                let end = usize::try_from(self.header.payload_end()).unwrap_or(usize::MAX);
                map.bytes().get(start..end).unwrap_or_default()
            }
            Data::Owned { words, len } => bytemuck::cast_slice::<u64, u8>(words)
                .get(..*len)
                .unwrap_or_default(),
        }
    }

    /// A zero-copy view as `T`: `Some` only when the dtype is `T`'s, in native byte order, and
    /// the payload is aligned for `T` (a mapped payload is unless its header was padded to an
    /// odd length; a copied one always is).
    pub fn as_slice<T: Scalar>(&self) -> Option<&[T]> {
        let (typed, order) = self.dtype()?;
        if !T::accepts(typed) || !order.is_native() {
            return None;
        }
        bytemuck::try_cast_slice(self.bytes()).ok()
    }

    /// The element at `index` (one index per dimension, in either memory order), or `None` when
    /// the index is out of range or the dtype has no typed access.
    pub fn element(&self, index: &[u64]) -> Option<Element> {
        let (typed, order) = self.dtype()?;
        let size = usize::try_from(self.itemsize()).ok()?;
        let at = element_offset(self.shape(), self.fortran_order(), size, index)?;
        decode(typed, order, self.bytes().get(at..at.checked_add(size)?)?)
    }

    /// A converted copy as `f32`, in payload order, for any typed dtype and byte order
    /// (integers and bools by value, `f8` rounded, `f2` exactly).
    pub fn to_f32_vec(&self) -> Option<Vec<f32>> {
        self.convert(|e| match e {
            Element::F32(v) => v,
            Element::F64(v) => v as f32,
            Element::Int(v) => v as f32,
            Element::UInt(v) => v as f32,
            Element::Bool(b) => f32::from(u8::from(b)),
        })
    }

    /// A converted copy as `f64`, in payload order (exact but for 64-bit integers past 2^53).
    pub fn to_f64_vec(&self) -> Option<Vec<f64>> {
        self.convert(Element::as_f64)
    }

    fn convert<T>(&self, f: impl Fn(Element) -> T) -> Option<Vec<T>> {
        let (typed, order) = self.dtype()?;
        let size = usize::try_from(self.itemsize()).ok()?;
        if size == 0 {
            return None;
        }
        let bytes = self.bytes();
        let mut out = Vec::new();
        out.try_reserve_exact(bytes.len() / size).ok()?;
        for chunk in bytes.chunks_exact(size) {
            out.push(f(decode(typed, order, chunk)?));
        }
        Some(out)
    }
}

/// Load the file at `path`: absent, torn, or whole (contract §5.7, §8.1). Never panics, and
/// allocates no more than the file holds.
pub(crate) fn load(path: &Path, attach: AttachMode) -> Load {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Load::Absent,
        Err(_) => return Load::Torn,
    };
    let loaded = if attach.maps() {
        map_whole(&file)
    } else {
        copy_whole(file)
    };
    loaded.map_or(Load::Torn, Load::Whole)
}

#[cfg(feature = "mmap")]
fn map_whole(file: &File) -> Option<Segment> {
    let meta = file.metadata().ok()?;
    // An empty file is torn, and mapping zero bytes is an error on some platforms.
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let map = Map::of(file).ok()?;
    // read_prefix refuses an HLEN over the limit before any header text is looked at.
    let header = npy::parse_header(map.bytes()).ok()?;
    header
        .check_len(u64::try_from(map.bytes().len()).ok()?)
        .ok()?;
    Some(Segment {
        header,
        data: Data::Mapped(map),
    })
}

#[cfg(not(feature = "mmap"))]
fn map_whole(file: &File) -> Option<Segment> {
    let _ = file;
    None
}

fn copy_whole(mut file: File) -> Option<Segment> {
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let header = read_header(&mut file)?;
    header.check_len(meta.len()).ok()?;
    let len = usize::try_from(header.nbytes()).ok()?;
    // Bounded: the header was just checked against the file's real length.
    let mut words: Vec<u64> = Vec::new();
    words.try_reserve_exact(len.div_ceil(8)).ok()?;
    words.resize(len.div_ceil(8), 0);
    let buf = bytemuck::cast_slice_mut::<u64, u8>(&mut words).get_mut(..len)?;
    file.seek(SeekFrom::Start(u64::try_from(header.data_offset()).ok()?))
        .ok()?;
    // A file that shrank since the length check is torn, not a short array.
    file.read_exact(buf).ok()?;
    Some(Segment {
        header,
        data: Data::Owned { words, len },
    })
}

/// Read and parse the header at the start of `file`, reading no more than its prefix promises
/// (at most 12 + 2^20 bytes). Leaves the position unspecified.
pub(crate) fn read_header(file: &mut File) -> Option<Header> {
    let mut prefix = [0u8; Prefix::MAX_LEN];
    let mut n = 0;
    while n < prefix.len() {
        match file.read(prefix.get_mut(n..)?) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let have = prefix.get(..n)?;
    let p = npy::read_prefix(have).ok()?;
    let end = p.data_offset();
    let mut buf = Vec::new();
    buf.try_reserve_exact(end).ok()?;
    buf.extend_from_slice(have.get(..n.min(end))?);
    if end > n {
        buf.resize(end, 0);
        file.read_exact(buf.get_mut(n..)?).ok()?;
    }
    npy::parse_header(&buf).ok()
}

// ---------------------------------------------------------------------------- typed access

mod sealed {
    pub trait Sealed {}
}

/// The element types [`Segment::as_slice`] can view a payload as. `u8` also views `b1` (whose
/// bytes are 0 or 1 in a file numpy wrote, though a hostile one may hold others). `f2` has no
/// Rust type: read it with [`Segment::element`] or [`Segment::to_f32_vec`].
pub trait Scalar: bytemuck::Pod + sealed::Sealed {
    #[doc(hidden)]
    fn accepts(typed: Typed) -> bool;
}

macro_rules! scalar {
    ($($t:ty => $($k:ident)|+),* $(,)?) => {$(
        impl sealed::Sealed for $t {}
        impl Scalar for $t {
            fn accepts(typed: Typed) -> bool {
                matches!(typed, $(Typed::$k)|+)
            }
        }
    )*};
}

scalar! {
    u8 => U8 | Bool, i8 => I8, u16 => U16, i16 => I16, u32 => U32, i32 => I32,
    u64 => U64, i64 => I64, f32 => F32, f64 => F64,
}

/// One element, in its own type, so a value prints exactly as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Element {
    /// `f2` (widened exactly) and `f4`.
    F32(f32),
    F64(f64),
    Int(i64),
    UInt(u64),
    Bool(bool),
}

impl Element {
    /// The value as `f64` (exact for every type but 64-bit integers beyond 2^53).
    pub fn as_f64(self) -> f64 {
        match self {
            Element::F32(v) => f64::from(v),
            Element::F64(v) => v,
            Element::Int(v) => v as f64,
            Element::UInt(v) => v as f64,
            Element::Bool(b) => f64::from(u8::from(b)),
        }
    }

    /// An integer or bool (a label), not a float.
    pub fn is_integral(self) -> bool {
        matches!(self, Element::Int(_) | Element::UInt(_) | Element::Bool(_))
    }
}

impl fmt::Display for Element {
    /// The shortest text that reads back as the same value (Rust's float formatting
    /// round-trips): plain decimals, or scientific below 1e-4 and from 1e16 on; NaN as `nan`,
    /// infinities as `inf` / `-inf`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn float(f: &mut fmt::Formatter<'_>, v: f64, plain: String, sci: String) -> fmt::Result {
            if v.is_nan() {
                f.write_str("nan")
            } else if v.is_infinite() {
                f.write_str(if v > 0.0 { "inf" } else { "-inf" })
            } else if v != 0.0 && !(1e-4..1e16).contains(&v.abs()) {
                f.write_str(&sci)
            } else {
                f.write_str(&plain)
            }
        }
        match *self {
            Element::F32(v) => float(f, f64::from(v), v.to_string(), format!("{v:e}")),
            Element::F64(v) => float(f, v, v.to_string(), format!("{v:e}")),
            Element::Int(v) => write!(f, "{v}"),
            Element::UInt(v) => write!(f, "{v}"),
            Element::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// The byte offset (from the start of the payload) of the element at `index` in an array of
/// `shape` with `size`-byte elements, C order (last index fastest) or Fortran order (first index
/// fastest). `None` when the index has the wrong length or is out of range, or on overflow.
pub fn element_offset(
    shape: &[u64],
    fortran_order: bool,
    size: usize,
    index: &[u64],
) -> Option<usize> {
    if index.len() != shape.len() {
        return None;
    }
    let mut offset = 0usize;
    let mut stride = size;
    let mut step = |dim: u64, i: u64| -> Option<()> {
        if i >= dim {
            return None;
        }
        let (dim, i) = (usize::try_from(dim).ok()?, usize::try_from(i).ok()?);
        offset = offset.checked_add(i.checked_mul(stride)?)?;
        stride = stride.checked_mul(dim)?;
        Some(())
    };
    if fortran_order {
        for (dim, i) in shape.iter().zip(index) {
            step(*dim, *i)?;
        }
    } else {
        for (dim, i) in shape.iter().zip(index).rev() {
            step(*dim, *i)?;
        }
    }
    Some(offset)
}

/// Decode one element of a typed class from exactly its size in bytes.
fn decode(typed: Typed, order: ByteOrder, bytes: &[u8]) -> Option<Element> {
    let size = typed_size(typed);
    if bytes.len() != size {
        return None;
    }
    let mut b = [0u8; 8];
    let dst = b.get_mut(..size)?;
    dst.copy_from_slice(bytes);
    if order == ByteOrder::Big {
        dst.reverse();
    }
    let raw = u64::from_le_bytes(b);
    let signed = |bits: u32| {
        let shift = 64 - bits;
        ((raw as i64) << shift) >> shift
    };
    Some(match typed {
        Typed::Bool => Element::Bool(raw != 0),
        Typed::I8 => Element::Int(signed(8)),
        Typed::I16 => Element::Int(signed(16)),
        Typed::I32 => Element::Int(signed(32)),
        Typed::I64 => Element::Int(raw as i64),
        Typed::U8 | Typed::U16 | Typed::U32 | Typed::U64 => Element::UInt(raw),
        Typed::F16 => Element::F32(f16_to_f32(raw as u16)),
        Typed::F32 => Element::F32(f32::from_bits(raw as u32)),
        Typed::F64 => Element::F64(f64::from_bits(raw)),
    })
}

fn typed_size(typed: Typed) -> usize {
    match typed {
        Typed::Bool | Typed::I8 | Typed::U8 => 1,
        Typed::I16 | Typed::U16 | Typed::F16 => 2,
        Typed::I32 | Typed::U32 | Typed::F32 => 4,
        Typed::I64 | Typed::U64 | Typed::F64 => 8,
    }
}

/// IEEE half to single precision (exact: every half is a single).
fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: value = frac × 2^-24.
            let v = frac as f32 * f32::from_bits(0x3380_0000);
            return if sign != 0 { -v } else { v };
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, _) => sign | 0x7fc0_0000 | (frac << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (frac << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_in_c_and_fortran_order() {
        // (2, 3, 4) f4: C strides (48, 16, 4), Fortran strides (4, 8, 24).
        let s = [2, 3, 4];
        assert_eq!(element_offset(&s, false, 4, &[1, 2, 3]), Some(48 + 32 + 12));
        assert_eq!(element_offset(&s, true, 4, &[1, 2, 3]), Some(4 + 16 + 72));
        assert_eq!(element_offset(&s, false, 4, &[0, 0, 0]), Some(0));
        assert_eq!(element_offset(&s, false, 4, &[2, 0, 0]), None);
        assert_eq!(element_offset(&s, false, 4, &[0, 0]), None);
        assert_eq!(element_offset(&[], false, 8, &[]), Some(0));
        assert_eq!(
            element_offset(&[u64::MAX, 2], false, 8, &[u64::MAX - 1, 1]),
            None
        );
    }

    #[test]
    fn halves_and_special_floats() {
        let half = |h: u16| decode(Typed::F16, ByteOrder::Little, &h.to_le_bytes()).unwrap();
        assert_eq!(half(0x3c00), Element::F32(1.0));
        assert_eq!(half(0xc000), Element::F32(-2.0));
        assert_eq!(half(0x0001), Element::F32(2f32.powi(-24)));
        assert_eq!(half(0x7c00), Element::F32(f32::INFINITY));
        assert!(half(0x7e00).as_f64().is_nan());
        assert_eq!(half(0x7e00).to_string(), "nan");
        assert_eq!(Element::F32(0.1).to_string(), "0.1");
        assert_eq!(Element::F32(1e-30).to_string(), "1e-30");
        assert_eq!(Element::F64(-0.0).to_string(), "-0");
        assert_eq!(decode(Typed::F32, ByteOrder::Little, &[0, 0]), None);
        assert_eq!(
            decode(Typed::I16, ByteOrder::Big, &[0xff, 0xfe]),
            Some(Element::Int(-2))
        );
    }
}

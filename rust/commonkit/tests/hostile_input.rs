//! Hostile input: the header parser and the key grammar never panic, never read past what they
//! are given, and never allocate by a size the input declares (contract §5.5, §5.7; golden rule
//! "never crash"). A deterministic xorshift generator stands in for a fuzzer, so failures replay.

mod common;

use common::{fixture, list, load, str_of};
use commonkit::Key;
use commonkit::npy::{self, Descr, NpyError, Order};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Whatever the bytes, a parse either fails or returns a header consistent with them.
fn check(bytes: &[u8]) {
    if let Ok(h) = npy::parse_header(bytes) {
        assert!(h.data_offset() <= bytes.len());
        assert!(h.shape().len() <= npy::MAX_DIMS);
        assert!(h.header_len() <= npy::MAX_HEADER_LEN);
        let product = h
            .shape()
            .iter()
            .try_fold(h.itemsize(), |a, d| a.checked_mul(*d));
        assert_eq!(product, Some(h.nbytes()));
    }
    if let Ok((h, payload)) = npy::parse_npy(bytes) {
        assert_eq!(payload.len() as u64, h.nbytes());
        assert!(h.payload_end() <= bytes.len() as u64);
    }
}

fn header_fixtures() -> Vec<Vec<u8>> {
    let doc = load("headers.json");
    list(&doc["vectors"])
        .iter()
        .map(|v| fixture(str_of(&v["file"])))
        .collect()
}

#[test]
fn every_truncation_of_every_fixture_is_torn() {
    for data in header_fixtures().iter().filter(|d| d.len() < 4096) {
        let (h, _) = npy::parse_npy(data).unwrap();
        for cut in 0..data.len() {
            let got = npy::parse_npy(&data[..cut]);
            if (cut as u64) < h.payload_end() {
                assert!(
                    matches!(got, Err(NpyError::Truncated { .. })),
                    "cut {cut}: {got:?}"
                );
            } else {
                assert!(got.is_ok());
            }
        }
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    let fixtures = header_fixtures();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let pieces: &[&[u8]] = &[
        b"{",
        b"}",
        b"(",
        b")",
        b"[",
        b"]",
        b"'",
        b"\"",
        b",",
        b":",
        b" ",
        b"\n",
        b"\\",
        b"0",
        b"9",
        b"99999999999999999999",
        b"True",
        b"False",
        b"None",
        b"L",
        b"<f4",
        b"|O",
        b"|V",
        b"descr",
        b"shape",
        b"fortran_order",
        b"\xff",
        b"\xce\x80",
        b"-1",
        b"M8[",
        b"((",
    ];
    for round in 0..20_000 {
        let mut bytes = rng.pick(&fixtures).clone();
        for _ in 0..1 + rng.below(4) {
            let header_end = bytes.len().min(256);
            match rng.below(6) {
                0 => {
                    if header_end > 0 {
                        let i = rng.below(header_end);
                        bytes[i] = rng.next() as u8;
                    }
                }
                1 => {
                    let i = 10 + rng.below(header_end.saturating_sub(10));
                    let piece = rng.pick(pieces);
                    bytes.splice(
                        i.min(bytes.len())..i.min(bytes.len()),
                        piece.iter().copied(),
                    );
                }
                2 => {
                    let i = 10 + rng.below(header_end.saturating_sub(10));
                    let n = rng.below(8);
                    let end = (i + n).min(bytes.len());
                    bytes.drain(i.min(end)..end);
                }
                3 => {
                    // A random HLEN, usually larger than the file.
                    let v = rng.next();
                    let w = if bytes.get(6) == Some(&1) { 2 } else { 4 };
                    for k in 0..w {
                        if let Some(b) = bytes.get_mut(8 + k) {
                            *b = (v >> (8 * k)) as u8;
                        }
                    }
                }
                4 => {
                    if let Some(b) = bytes.get_mut(6) {
                        *b = rng.pick(&[0u8, 1, 2, 3, 4, 255]).to_owned();
                    }
                }
                _ => {
                    let cut = rng.below(bytes.len() + 1);
                    bytes.truncate(cut);
                }
            }
        }
        check(&bytes);
        let _ = round;
    }
}

#[test]
fn random_header_text_never_panics() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let tokens: &[&str] = &[
        "{",
        "}",
        "(",
        ")",
        "[",
        "]",
        ",",
        ":",
        " ",
        "'descr'",
        "'fortran_order'",
        "'shape'",
        "'<f4'",
        "'|u1'",
        "'>c16'",
        "'<M8[s]'",
        "'|O'",
        "'<U5'",
        "True",
        "False",
        "None",
        "0",
        "1",
        "18446744073709551615",
        "18446744073709551616",
        "('a', '<i4')",
        "('', '|V3')",
        "('b', '|u1', (3,))",
        "'\\x41'",
        "'\\u03c0'",
        "'\\",
        "\"it's\"",
        "π",
        "-",
        "3L",
    ];
    for _ in 0..20_000 {
        let mut text = String::new();
        if rng.below(2) == 0 {
            text.push_str("{'descr': ");
        }
        for _ in 0..rng.below(24) {
            text.push_str(rng.pick::<&str>(tokens));
        }
        let version = *rng.pick(&[1u8, 2, 3]);
        let mut bytes = npy::MAGIC.to_vec();
        bytes.extend_from_slice(&[version, 0]);
        if version == 1 {
            bytes.extend_from_slice(&(text.len() as u16).to_le_bytes());
        } else {
            bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
        }
        bytes.extend_from_slice(text.as_bytes());
        check(&bytes);
    }
}

#[test]
fn declared_sizes_are_not_trusted() {
    // HLEN at the limit with nothing behind it: torn at once, without reading or allocating.
    let mut b = npy::MAGIC.to_vec();
    b.extend_from_slice(&[2, 0]);
    b.extend_from_slice(&npy::MAX_HEADER_LEN.to_le_bytes());
    assert!(
        matches!(npy::parse_header(&b), Err(NpyError::Truncated { need, .. }) if need == (1 << 20) + 12)
    );
    // A payload of 2^62 bytes promised by a 128-byte file: a short payload, not an allocation.
    let d = Descr::parse("|u1").unwrap();
    let h = npy::encode_header(&d, &[1 << 31, 1 << 31], Order::C).unwrap();
    assert!(matches!(
        npy::parse_npy(&h),
        Err(NpyError::Truncated { .. })
    ));
    // Sizes that overflow u64 are torn.
    let bump = String::from_utf8_lossy(&h[10..]).replace("'|u1'", "'<u8'");
    let mut over = h[..10].to_vec();
    over.extend_from_slice(bump.as_bytes());
    assert_eq!(npy::parse_header(&over), Err(NpyError::Overflow));
    // Deep nesting is refused, not recursed into without bound.
    let deep = format!(
        "{{'descr': {}'<f4'{}, 'fortran_order': False, 'shape': (), }}",
        "[('a', ".repeat(40),
        ")]".repeat(40)
    );
    let mut b = npy::MAGIC.to_vec();
    b.extend_from_slice(&[1, 0]);
    b.extend_from_slice(&(deep.len() as u16).to_le_bytes());
    b.extend_from_slice(deep.as_bytes());
    assert!(npy::parse_header(&b).is_err());
}

#[test]
fn written_headers_read_back() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let descrs = [
        "|b1",
        "|i1",
        "<i2",
        ">i4",
        "<i8",
        "|u1",
        ">u2",
        "<u4",
        ">u8",
        "<f2",
        ">f4",
        "<f8",
        "<c8",
        ">c16",
        "<M8[ms]",
        ">m8",
        "<U7",
        "|S3",
        "|V5",
        "[('a', '<i4'), ('b', '>f8', (2,))]",
        "[('é', '<i2')]",
        "[('π', '|u1')]",
    ];
    for _ in 0..5_000 {
        let descr = Descr::parse(rng.pick::<&str>(&descrs)).unwrap();
        let ndim = rng.below(9);
        let shape: Vec<u64> = (0..ndim)
            .map(|_| match rng.below(5) {
                0 => 0,
                1 => 1,
                2 => rng.below(10) as u64,
                3 => rng.below(100_000) as u64,
                _ => 1 << rng.below(20),
            })
            .collect();
        let order = if rng.below(2) == 0 {
            Order::C
        } else {
            Order::F
        };
        let Ok(header) = npy::encode_header(&descr, &shape, order) else {
            assert!(npy::nbytes(&descr, &shape).is_none());
            continue;
        };
        assert_eq!(header.len() % 64, 0);
        assert_eq!(header.last(), Some(&b'\n'));
        let h = npy::parse_header(&header).unwrap();
        assert_eq!(h.descr(), &descr);
        assert_eq!(h.shape(), shape.as_slice());
        assert_eq!(h.fortran_order(), npy::fortran_flag(&shape, order));
        assert_eq!(h.data_offset(), header.len());
        assert_eq!(
            h.version().0,
            if descr.repr().unwrap().contains('π') {
                3
            } else {
                1
            }
        );
    }
}

#[test]
fn random_keys_never_panic_and_agree_with_the_grammar() {
    let grammar =
        regex::Regex::new(r"\A[a-z0-9]+:[a-z0-9]+:[a-z0-9]+:[0-9a-f]{32}:[a-z0-9]+\z").unwrap();
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let pieces = [
        "a",
        "z",
        "0",
        "9",
        ":",
        "ab",
        "abababababababababababababababab",
        "AB",
        "\n",
        " ",
        "/",
        ".",
        "..",
        "_",
        "-",
        "é",
        "\u{212a}",
        "\0",
        "fedcba9876543210fedcba9876543210",
    ];
    for _ in 0..50_000 {
        let mut key = String::new();
        for _ in 0..rng.below(12) {
            key.push_str(rng.pick::<&str>(&pieces));
        }
        let ok = Key::parse(&key).is_ok();
        assert_eq!(ok, grammar.is_match(&key), "{key:?}");
        if let Ok(k) = Key::parse(&key) {
            let rel = k.relpath();
            assert!(!rel.starts_with('/') && !rel.contains(".."), "{key:?}");
            assert_eq!(rel.split('/').count(), 3);
        }
    }
}

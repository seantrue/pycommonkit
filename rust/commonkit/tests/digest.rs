//! The hashing facade (contract §3.8): `commonkit::digest` under each algorithm, `fixed`, the
//! build salts, and every vector in `conformance/vectors/digests.json`. One process per
//! algorithm (D17): each algorithm's vectors run in a child whose `COMMONKIT_HASH` names it.

mod child;
mod common;

use common::{list, load, shape_of, str_of};
use commonkit::digest::{self, Algo, Hasher, build, fixed};
use serde_json::Value;

fn data(v: &Value) -> Vec<u8> {
    if let Some(h) = v.get("bytes_hex") {
        let h = str_of(h);
        return (0..h.len() / 2)
            .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
            .collect();
    }
    let n = v["pattern"]["len"].as_u64().unwrap() as usize;
    (0..n).map(|i| ((i * 31) % 251) as u8).collect()
}

/// In a child: every vector, in the algorithm this process resolved.
#[test]
fn child_checks_the_vectors() {
    if !child::is_child() {
        return;
    }
    let doc = load("digests.json");
    let algo = digest::current();
    let name = algo.name();
    for v in list(&doc["raw"]) {
        let id = str_of(&v["id"]);
        let b = data(v);
        assert_eq!(digest::raw(&b), str_of(&v[name]), "{id}");
        let mut h = Hasher::new();
        h.update(&b[..b.len().min(7)]);
        h.update(&b[b.len().min(7)..]);
        assert_eq!(h.finish(), str_of(&v[name]), "{id} streamed");
        assert_eq!(fixed::raw(fixed::Fixed::Md5, &b), str_of(&v["md5"]), "{id}");
        assert_eq!(
            fixed::raw(fixed::Fixed::Sha256, &b),
            str_of(&v["sha256"]),
            "{id}"
        );
        let s = build::both(&b);
        assert_eq!(
            (s.md5.as_str(), s.b3.as_str()),
            (str_of(&v["md5"]), str_of(&v["b3"]))
        );
        assert_eq!(build::pick(&s), str_of(&v[name]), "{id} pick");
    }
    for v in list(&doc["nd1"]) {
        let id = str_of(&v["id"]);
        let (dt, shape, b) = (str_of(&v["dtype"]), shape_of(&v["shape"]), data(v));
        assert_eq!(
            digest::nd1(dt, &shape, &b).unwrap(),
            str_of(&v[name]),
            "{id}"
        );
        let md5 = fixed::nd1(fixed::Fixed::Md5, dt, &shape, &b).unwrap();
        assert_eq!(md5, str_of(&v["md5"]), "{id}");
        let sha = fixed::nd1(fixed::Fixed::Sha256, dt, &shape, &b).unwrap();
        assert_eq!(sha, str_of(&v["sha256"]), "{id}");
    }
    println!("checked {name}");
}

#[test]
fn every_vector_in_each_algorithm() {
    for algo in ["md5", "b3"] {
        let d = child::dirs();
        let o = child::run("child_checks_the_vectors", &d, Some(algo), &[]);
        child::ok(&o);
        assert!(
            child::stdout(&o).contains(&format!("checked {algo}")),
            "{algo}"
        );
        let line = format!("commonkit: hash {algo} (environment COMMONKIT_HASH)");
        assert!(child::stderr(&o).contains(&line), "{}", child::stderr(&o));
    }
}

#[test]
fn the_vectors_cover_big_endian_and_large_inputs() {
    let doc = load("digests.json");
    assert!(list(&doc["raw"]).len() >= 5 && list(&doc["nd1"]).len() >= 8);
    assert!(
        list(&doc["nd1"])
            .iter()
            .any(|v| str_of(&v["dtype"]).starts_with('>'))
    );
    let le = list(&doc["nd1"])
        .iter()
        .find(|v| v["id"] == "f4_le")
        .unwrap();
    let be = list(&doc["nd1"])
        .iter()
        .find(|v| v["id"] == "f4_be")
        .unwrap();
    assert_eq!(le["b3"], be["b3"]);
}

#[test]
fn byte_order_is_normalised_and_bad_dtypes_refused() {
    let le: Vec<u8> = [1.5f32, -2.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let be: Vec<u8> = [1.5f32, -2.0]
        .iter()
        .flat_map(|f| f.to_be_bytes())
        .collect();
    for a in [fixed::Fixed::Md5, fixed::Fixed::Sha256] {
        assert_eq!(
            fixed::nd1(a, "<f4", &[2], &le),
            fixed::nd1(a, ">f4", &[2], &be)
        );
        assert_eq!(
            fixed::nd1(a, "=f4", &[2], &le),
            fixed::nd1(a, "<f4", &[2], &le)
        );
    }
    for bad in ["|O8", "|V16", "<f", "?f4", "<M8[nanoseconds]", ""] {
        assert!(digest::nd1(bad, &[1], &[0; 8]).is_err(), "{bad}");
    }
    assert!(digest::nd1("<f4", &[2, 3], &[0; 23]).is_err());
    assert!(digest::nd1("<f4", &[u64::MAX, 3], &[]).is_err());
}

#[test]
fn fixed_knows_md5_and_sha256() {
    assert_eq!(
        fixed::raw(fixed::Fixed::Md5, b""),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    let mut h = fixed::Hasher::new(fixed::Fixed::Sha256);
    h.update(b"a");
    h.update(b"bc");
    assert_eq!(h.finish(), fixed::raw(fixed::Fixed::Sha256, b"abc"));
    assert!(h_starts(
        &fixed::raw(fixed::Fixed::Sha256, b"abc"),
        "ba7816bf"
    ));
}

fn h_starts(s: &str, p: &str) -> bool {
    s.starts_with(p)
}

#[test]
fn file_streams_and_refuses_over_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("blob");
    let bytes: Vec<u8> = (0..256u32 * 9000).map(|i| (i % 256) as u8).collect();
    std::fs::write(&p, &bytes).unwrap();
    let (d, n) = digest::file(&p, u64::MAX).unwrap();
    assert_eq!((d, n), (digest::raw(&bytes), bytes.len() as u64));
    assert!(digest::file(&p, 100).is_err());
}

#[test]
fn the_algorithm_names() {
    assert_eq!((Algo::Md5.name(), Algo::B3.name()), ("md5", "b3"));
    assert_eq!((Algo::Md5.hex_len(), Algo::B3.hex_len()), (32, 64));
    assert_eq!(Algo::from_name(" MD5\t"), Some(Algo::Md5));
    assert_eq!(Algo::from_name("sha1"), None);
    assert_eq!(digest::raw(b"x").len(), digest::current().hex_len());
}

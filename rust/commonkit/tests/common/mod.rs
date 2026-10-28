//! Shared helpers for the tests that read `conformance/vectors/` (included with `mod`).

#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// `conformance/vectors/`, from the repository root two levels above this crate.
pub fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/vectors")
}

pub fn load(name: &str) -> Value {
    let path = vectors_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["contract"], 1, "{name}");
    doc
}

pub fn fixture(rel: &str) -> Vec<u8> {
    let path = vectors_dir().join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

pub fn list(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("not a list: {v}"))
}

// ------------------------------------------------------------------ array specs (§13.2)

use commonkit::npy::{Descr, Field, Kind, Order};
use commonkit::{KeyScheme, Variants};

pub fn shape_of(v: &Value) -> Vec<u64> {
    list(v).iter().map(|d| d.as_u64().unwrap()).collect()
}

/// The descriptor an array spec names (contract §13.2), built without reading a fixture.
pub fn spec_descr(spec: &Value) -> Descr {
    if let Some(f) = spec.get("fields") {
        let count = f["count"].as_u64().unwrap();
        let digits = f["digits"].as_u64().unwrap() as usize;
        let each = Descr::parse(str_of(&f["descr"])).unwrap();
        let fields = (0..count)
            .map(|i| Field::new(format!("f{i:0digits$}"), each.clone(), vec![]))
            .collect();
        return Descr::structured(fields).unwrap();
    }
    Descr::parse(str_of(&spec["descr"])).unwrap()
}

pub fn spec_order(spec: &Value) -> Order {
    match spec.get("order").and_then(Value::as_str).unwrap_or("C") {
        "C" => Order::C,
        "F" => Order::F,
        o => panic!("order {o}"),
    }
}

/// The generator's `_fill`: deterministic payload bytes for `n_items` elements of `descr`,
/// laid out in the spec's order. `ramp` and `ramp2` differ in every byte.
pub fn spec_fill(descr: &Descr, n_items: u64, fill: &str) -> Vec<u8> {
    let nbytes = (descr.itemsize().unwrap() * n_items) as usize;
    let seed: u64 = match fill {
        "ramp" => 11,
        "ramp2" => 101,
        "zeros" => return vec![0; nbytes],
        f => panic!("fill {f}"),
    };
    if nbytes == 0 {
        return Vec::new();
    }
    let generic = || {
        (0..nbytes as u64)
            .map(|i| ((i * 37 + seed) & 0xff) as u8)
            .collect()
    };
    let Descr::Simple(s) = descr else {
        return generic();
    };
    let big = s.order() == commonkit::npy::ByteOrder::Big;
    match s.kind() {
        Kind::Bool => (0..nbytes).map(|i| (i & 1) as u8).collect(),
        Kind::Bytes => (0..nbytes as u64)
            .map(|i| (97 + (i + seed) % 26) as u8)
            .collect(),
        Kind::Unicode => (0..nbytes as u64 / 4)
            .flat_map(|i| {
                let c = (97 + (i + seed) % 26) as u32;
                if big {
                    c.to_be_bytes()
                } else {
                    c.to_le_bytes()
                }
            })
            .collect(),
        Kind::Datetime | Kind::Timedelta => (0..n_items)
            .flat_map(|i| {
                let v = (i * 1000 + seed) as i64;
                if big {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                }
            })
            .collect(),
        _ => generic(),
    }
}

/// An owned array built from a vector's `array` spec, to lend out as an `ArrayRef`.
pub struct Arr {
    pub descr: Descr,
    pub shape: Vec<u64>,
    pub order: Order,
    pub data: Vec<u8>,
}

impl Arr {
    pub fn from_spec(spec: &Value) -> Arr {
        let descr = spec_descr(spec);
        let shape = shape_of(&spec["shape"]);
        let data = spec_fill(&descr, shape.iter().product(), str_of(&spec["fill"]));
        Arr {
            descr,
            shape,
            order: spec_order(spec),
            data,
        }
    }

    pub fn new(descr: &str, shape: &[u64], order: Order, data: Vec<u8>) -> Arr {
        Arr {
            descr: Descr::parse(descr).unwrap(),
            shape: shape.to_vec(),
            order,
            data,
        }
    }

    pub fn r(&self) -> commonkit::ArrayRef<'_> {
        commonkit::ArrayRef::new(&self.descr, &self.shape, self.order, &self.data).unwrap()
    }
}

/// A scheme from a vector file's `scheme` object: closed variants as a list, a pattern as a
/// string, and an optional `dtypes` map.
pub fn scheme_from(v: &Value) -> KeyScheme {
    let forms: Vec<String> = list(&v["forms"])
        .iter()
        .map(|f| str_of(f).to_owned())
        .collect();
    let variants = match &v["variants"] {
        Value::String(p) => Variants::pattern(p.as_str()).unwrap(),
        other => Variants::closed(list(other).iter().map(|x| str_of(x).to_owned())).unwrap(),
    };
    let dtypes: Vec<(String, String)> = v
        .get("dtypes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, d)| (k.clone(), str_of(d).to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let dtypes: Vec<(&str, &str)> = dtypes
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    KeyScheme::new(
        str_of(&v["domain"]),
        str_of(&v["version"]),
        forms,
        variants,
        &dtypes,
    )
    .unwrap()
}

/// Every `*.tmp` file under `dir`, recursively.
pub fn tmp_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "tmp") {
                out.push(p);
            }
        }
    }
    out
}

/// Set a file's mtime (and atime) to `age_s` seconds ago (negative: in the future). Test-only: opening for writing is
/// what the mutation rule forbids of a store, not of a test arranging one.
pub fn age(path: &std::path::Path, age_s: f64) -> std::time::SystemTime {
    let now = std::time::SystemTime::now();
    let then = if age_s >= 0.0 {
        now - std::time::Duration::from_secs_f64(age_s)
    } else {
        now + std::time::Duration::from_secs_f64(-age_s)
    };
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_times(
        std::fs::FileTimes::new()
            .set_accessed(then)
            .set_modified(then),
    )
    .unwrap();
    then
}

pub fn mtime(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

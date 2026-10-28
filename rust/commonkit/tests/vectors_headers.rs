//! Every vector in `conformance/vectors/headers.json` (contract §5): the reader recovers every
//! field of each numpy-written fixture, and the writer reproduces each fixture byte for byte
//! from the vector's array spec. Also the file-level outcome of `reads.json`.

mod common;

use common::{fixture, list, load, sha256, str_of};
use commonkit::npy::{self, Descr, Field, Order};
use serde_json::Value;

fn shape_of(v: &Value) -> Vec<u64> {
    list(v).iter().map(|d| d.as_u64().unwrap()).collect()
}

/// The descriptor an array spec names (contract §13.2), built without reading the fixture.
fn spec_descr(spec: &Value) -> Descr {
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

fn spec_order(spec: &Value) -> Order {
    match str_of(&spec["order"]) {
        "C" => Order::C,
        "F" => Order::F,
        o => panic!("order {o}"),
    }
}

#[test]
fn reader_recovers_every_field() {
    let doc = load("headers.json");
    let vectors = list(&doc["vectors"]);
    assert!(vectors.len() >= 50);
    for v in vectors {
        let id = str_of(&v["id"]);
        let data = fixture(str_of(&v["file"]));
        assert_eq!(sha256(&data), str_of(&v["sha256"]), "{id}: fixture digest");
        assert_eq!(data.len() as u64, v["file_bytes"].as_u64().unwrap(), "{id}");
        let (h, payload) = npy::parse_npy(&data).unwrap_or_else(|e| panic!("{id}: {e}"));
        let version = list(&v["version"])
            .iter()
            .map(|x| x.as_u64().unwrap() as u8)
            .collect::<Vec<_>>();
        assert_eq!(vec![h.version().0, h.version().1], version, "{id}: version");
        assert_eq!(
            u64::from(h.header_len()),
            v["header_len"].as_u64().unwrap(),
            "{id}: header_len"
        );
        assert_eq!(
            h.data_offset() as u64,
            v["data_offset"].as_u64().unwrap(),
            "{id}: data_offset"
        );
        assert_eq!(h.data_offset() % 64, 0, "{id}: aligned");
        match &v["header_text"] {
            Value::String(t) => assert_eq!(h.text(), t, "{id}: header_text"),
            _ => assert_eq!(
                sha256(h.text().as_bytes()),
                str_of(&v["header_text_sha256"]),
                "{id}"
            ),
        }
        if let Value::String(d) = &v["descr"] {
            assert_eq!(h.descr_text(), d, "{id}: descr");
        }
        assert_eq!(
            h.fortran_order(),
            v["fortran_order"].as_bool().unwrap(),
            "{id}: fortran_order"
        );
        assert_eq!(h.shape(), shape_of(&v["shape"]), "{id}: shape");
        assert_eq!(
            h.itemsize(),
            v["itemsize"].as_u64().unwrap(),
            "{id}: itemsize"
        );
        assert_eq!(
            h.nbytes(),
            v["payload_bytes"].as_u64().unwrap(),
            "{id}: payload_bytes"
        );
        assert_eq!(payload.len() as u64, h.nbytes(), "{id}");
        // The padding split: growth-axis spaces (contract §5.2 item 2), then alignment spaces.
        // Text length in encoded bytes: UTF-8 for version 3, one byte per char (Latin-1) else.
        let encoded = if h.version().0 == 3 {
            h.text().len()
        } else {
            h.text().chars().count()
        };
        let spaces = h.header_len() as usize - encoded - 1;
        let growth = v["growth_padding"].as_u64().unwrap() as usize;
        let axis = if h.fortran_order() {
            h.shape().last()
        } else {
            h.shape().first()
        };
        let want_growth = axis.map_or(0, |g| 21usize.saturating_sub(g.to_string().len()));
        assert_eq!(want_growth, growth, "{id}: growth_padding");
        assert_eq!(
            spaces - growth,
            v["align_padding"].as_u64().unwrap() as usize,
            "{id}: align_padding"
        );
    }
}

#[test]
fn writer_reproduces_numpy_bytes() {
    let doc = load("headers.json");
    let mut checked = 0;
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let data = fixture(str_of(&v["file"]));
        let spec = &v["array"];
        let descr = spec_descr(spec);
        let shape = shape_of(&spec["shape"]);
        let header = npy::encode_header(&descr, &shape, spec_order(spec))
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let offset = v["data_offset"].as_u64().unwrap() as usize;
        assert_eq!(header.len(), offset, "{id}: header length");
        assert_eq!(header.as_slice(), &data[..offset], "{id}: header bytes");
        let file = npy::encode_npy(&descr, &shape, spec_order(spec), &data[offset..]).unwrap();
        assert_eq!(sha256(&file), str_of(&v["sha256"]), "{id}: whole file");
        // A writer that skipped the growth padding would land here instead (contract §5.2).
        let prefix = if v["version"][0] == 1 { 10 } else { 12 };
        let pads = v["growth_padding"].as_u64().unwrap() + v["align_padding"].as_u64().unwrap();
        let text_len = header.len() - prefix - 1 - pads as usize;
        let bare = prefix + text_len + 1;
        let unpadded = bare + (64 - bare % 64);
        assert_eq!(
            unpadded as u64,
            v["unpadded_data_offset"].as_u64().unwrap(),
            "{id}: unpadded"
        );
        checked += 1;
    }
    assert_eq!(checked, list(&doc["vectors"]).len());
}

#[test]
fn read_vectors_at_the_file_level() {
    // The store's counters are step 6; here, only whether the file is whole or torn.
    let doc = load("reads.json");
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let Value::String(rel) = &v["file"] else {
            continue;
        };
        let data = fixture(rel);
        let expect = &v["expect"];
        match (str_of(&expect["outcome"]), npy::parse_npy(&data)) {
            ("hit", Ok((h, payload))) => {
                if let Value::String(d) = &expect["descr"] {
                    assert_eq!(h.descr_text(), d, "{id}");
                }
                assert_eq!(h.shape(), shape_of(&expect["shape"]), "{id}");
                assert_eq!(
                    h.fortran_order(),
                    expect["fortran_order"].as_bool().unwrap(),
                    "{id}"
                );
                assert_eq!(sha256(payload), str_of(&expect["payload_sha256"]), "{id}");
            }
            ("torn", Err(_)) => {}
            (want, got) => panic!("{id}: expected {want}, got {:?}", got.map(|(h, _)| h)),
        }
    }
}

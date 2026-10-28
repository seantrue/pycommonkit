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

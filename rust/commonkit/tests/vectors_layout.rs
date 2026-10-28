//! The shared conformance vectors are reachable from the crate, and each JSON file is contract 1.
//!
//! The vectors live at `conformance/vectors/` in the repository root, two levels above this
//! crate; the Rust and Python suites read the same files at test time.

use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/vectors")
}

#[test]
fn every_vector_file_is_contract_version_1() {
    let mut seen = 0;
    for entry in std::fs::read_dir(vectors_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("\"contract\": 1"), "{}", path.display());
            seen += 1;
        }
    }
    assert!(seen >= 8, "found only {seen} vector files");
    assert_eq!(commonkit::CONTRACT_VERSION, 1);
}

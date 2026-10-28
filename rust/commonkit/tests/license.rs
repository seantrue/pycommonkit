//! The crate's LICENSE is a copy of the repository's, so `cargo package` ships it; keep them equal.

#[test]
fn crate_license_is_the_repository_license() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ours = std::fs::read(dir.join("LICENSE")).unwrap();
    let root = std::fs::read(dir.join("../../LICENSE")).unwrap();
    assert_eq!(
        ours, root,
        "rust/commonkit/LICENSE differs from the repository's LICENSE"
    );
}

//! No hashing is reachable from the store (the alexandria constraint, contract §3.8).
//!
//! commonkit never computes a digest of a stored value and never re-checks a caller's: the
//! digest rule compares a length. So the crate built without its `digest` feature, which is
//! everything the store needs, has no hash crate among its normal dependencies, and the hash
//! crates are named only in `src/digest.rs`.

use std::path::PathBuf;
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_store_build_has_no_hash_crate() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(manifest())
        .args([
            "tree",
            "-p",
            "commonkit",
            "-e",
            "normal",
            "--no-default-features",
        ])
        .args(["--features", "mmap", "--prefix", "none", "--offline"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(tree.starts_with("commonkit v"), "{tree}");
    for banned in ["md-5 ", "md5 ", "blake3 ", "sha2 ", "sha1 ", "digest "] {
        assert!(
            !tree.lines().any(|l| l.starts_with(banned)),
            "{banned}in\n{tree}"
        );
    }
}

#[test]
fn hash_crates_are_named_only_in_the_facade() {
    let src = manifest().join("src");
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.file_name().is_some_and(|n| n == "digest.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for banned in [
                "md5::",
                "blake3::",
                "sha2::",
                "use md5",
                "use blake3",
                "use sha2",
            ] {
                assert!(!text.contains(banned), "{} names {banned}", p.display());
            }
        }
    }
}

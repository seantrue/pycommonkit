//! Test helpers for every suite that uses commonkit (feature `testing`; D25 E, contract §11).
//!
//! A suite runs inside a run folder named by `COMMONKIT_TEST_RUN`, with a fresh
//! `COMMONKIT_STORE_ROOT` and `COMMONKIT_MARK_DIR` in it and `COMMONKIT_HASH` set to the
//! algorithm under test, never inherited from a shell whose root may be a live tree.
//! [`assert_isolated`] reads nothing in the root, so every test binary and every test of a
//! run gets the same answer in any order. Python's `commonkit.testing` is the same.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The variable naming the run folder the root must lie in.
pub const RUN_ENV: &str = "COMMONKIT_TEST_RUN";

fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| crate::store::absolute(p).unwrap_or(p.into()))
}

fn inside(child: &Path, parent: &Path) -> bool {
    real(child).starts_with(real(parent))
}

/// `Ok` when `COMMONKIT_STORE_ROOT` is set, isn't (and doesn't contain) the platform default
/// root, and resolves inside `COMMONKIT_TEST_RUN`, which is set; else why not.
pub fn check_isolated() -> Result<(), String> {
    let root = std::env::var_os("COMMONKIT_STORE_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or("COMMONKIT_STORE_ROOT is unset: run the suite on a fresh root (D25 E)")?;
    if let Some(default) = crate::policy::default_root()
        && inside(&default, &root)
    {
        return Err(format!(
            "the store root {} is or holds the default root",
            root.display()
        ));
    }
    let run = std::env::var_os(RUN_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{RUN_ENV} is unset: the harness names the run folder"))?;
    if !inside(&root, &run) {
        return Err(format!(
            "the store root {} is not inside {RUN_ENV}={}",
            root.display(),
            run.display()
        ));
    }
    Ok(())
}

/// Fail the test unless [`check_isolated`] passes.
#[allow(clippy::panic)] // a test assertion: failing the test is its whole job
pub fn assert_isolated() {
    if let Err(why) = check_isolated() {
        panic!("{why}");
    }
}

/// A new, empty store root inside the run folder (or the system temporary folder).
#[allow(clippy::panic)] // a test helper: a test with no root can't go on
pub fn fresh_root() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let base = std::env::var_os(RUN_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(std::env::temp_dir, PathBuf::from);
    loop {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = base.join(format!("root-{}-{n}", std::process::id()));
        match std::fs::create_dir(&p) {
            Ok(()) => return p,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("can't create a fresh root under {}: {e}", base.display()),
        }
    }
}

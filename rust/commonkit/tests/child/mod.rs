//! Run one test of this binary again in a child process (included with `mod`).
//!
//! The hash setting is resolved once per process (contract §3.8), so a check of the other
//! algorithm, of the policy file against `COMMONKIT_HASH`, or of the line a process prints at
//! its first resolution runs in a child. The child is this test binary, filtered to one test
//! that does nothing unless `CK_CHILD` is set. It gets a fresh run folder, store root and mark
//! folder, and none of the parent's `COMMONKIT_*` variables.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Whether this process is a child started by [`run`].
pub fn is_child() -> bool {
    std::env::var_os("CK_CHILD").is_some()
}

/// A fresh run folder with `store/` (created) and `marks/` (not created) inside it.
pub struct Dirs {
    pub _keep: tempfile::TempDir,
    pub run: PathBuf,
    pub root: PathBuf,
    pub marks: PathBuf,
}

pub fn dirs() -> Dirs {
    let keep = tempfile::tempdir().unwrap();
    let run = keep.path().to_path_buf();
    let root = run.join("store");
    std::fs::create_dir_all(&root).unwrap();
    Dirs {
        marks: run.join("marks"),
        root,
        run,
        _keep: keep,
    }
}

/// Run test `name` in a child with `algo` (None: unset) and `env` (a `None` value removes).
pub fn run(name: &str, d: &Dirs, algo: Option<&str>, env: &[(&str, Option<&str>)]) -> Output {
    command(name, d, algo, env).output().unwrap()
}

/// The child [`run`] starts, for a test that needs its pipes or working directory.
pub fn command(name: &str, d: &Dirs, algo: Option<&str>, env: &[(&str, Option<&str>)]) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([name, "--exact", "--nocapture", "--test-threads=1"]);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("COMMONKIT_") {
            c.env_remove(k);
        }
    }
    c.env("CK_CHILD", "1")
        .env("COMMONKIT_STORE_ROOT", &d.root)
        .env("COMMONKIT_MARK_DIR", &d.marks)
        .env("COMMONKIT_TEST_RUN", &d.run);
    if let Some(a) = algo {
        c.env("COMMONKIT_HASH", a);
    }
    for (k, v) in env {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    c
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Assert the child passed, showing its output when it didn't.
pub fn ok(o: &Output) {
    assert!(
        o.status.success(),
        "child failed\n{}\n{}",
        stdout(o),
        stderr(o)
    );
}

/// The marker names in a mark folder: `started`, `shm_bad_key` and so on.
pub fn marks_in(folder: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(folder)
        .map(|rd| {
            rd.filter_map(|e| {
                let name = e.ok()?.file_name().to_string_lossy().into_owned();
                Some(name.split_once('.')?.1.to_owned())
            })
            .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

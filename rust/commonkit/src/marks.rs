//! Marker files: evidence of a declined key, written when it happens (contract §11).
//!
//! A declined key never reaches disk, so the only evidence of a digest leak is in the process
//! that declined it, and engines leave through `process::exit` or SIGKILL, where nothing runs
//! at exit. So when a store has a mark folder (`COMMONKIT_MARK_DIR`, or
//! `Store::with_mark_dir`), the first `shm_foreign_digest` or `shm_bad_key` event for that
//! folder in a process creates `<folder>/<pid>.<counter>` at once, and the first store built
//! for the folder creates `<pid>.started`. Python's `commonkit._marks` writes the same files.
//! A failed write is ignored: it never fails a store call.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The variable naming the mark folder.
pub const MARK_ENV: &str = "COMMONKIT_MARK_DIR";

static SEEN: Mutex<BTreeSet<(PathBuf, &'static str)>> = Mutex::new(BTreeSet::new());

/// The mark folder named by the environment, if set and non-empty.
pub(crate) fn from_env() -> Option<PathBuf> {
    std::env::var_os(MARK_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "rust".into())
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_once(folder: &Path, name: &'static str, fields: &[(&str, String)]) {
    {
        let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        if !seen.insert((folder.to_path_buf(), name)) {
            return;
        }
    }
    let pid = std::process::id();
    let mut body = format!("{{\"pid\": {pid}, \"program\": {}", quote(&program()));
    for (k, v) in fields {
        body.push_str(&format!(", {}: {}", quote(k), quote(v)));
    }
    body.push_str("}\n");
    let _ = create_dir(folder).and_then(|()| {
        let mut f = private(OpenOptions::new().write(true).create_new(true))
            .open(folder.join(format!("{pid}.{name}")))?;
        f.write_all(body.as_bytes())
    });
}

/// 0600, as Python's: a marker names keys and roots, and its folder is 0700.
#[cfg(unix)]
fn private(o: &mut OpenOptions) -> &mut OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    o.mode(0o600)
}

#[cfg(not(unix))]
fn private(o: &mut OpenOptions) -> &mut OpenOptions {
    o
}

#[cfg(unix)]
fn create_dir(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
}

#[cfg(not(unix))]
fn create_dir(folder: &Path) -> std::io::Result<()> {
    fs::create_dir_all(folder)
}

/// `<pid>.started`: the algorithm, its source and the store's root.
pub(crate) fn started(folder: &Path, root: &Path) {
    let algo = crate::policy::current();
    let source = crate::policy::source();
    write_once(
        folder,
        "started",
        &[
            ("algo", algo.name().to_owned()),
            ("source", source.to_string()),
            ("root", root.to_string_lossy().into_owned()),
        ],
    );
}

/// `<pid>.<counter>`: the key that was declined and the scheme it named.
pub(crate) fn event(folder: &Path, counter: &'static str, key: &str, root: &Path) {
    let key: String = key.chars().take(300).collect();
    let scheme = key.split(':').take(2).collect::<Vec<_>>().join(":");
    write_once(
        folder,
        counter,
        &[
            ("counter", counter.to_owned()),
            ("scheme", scheme),
            ("key", key),
            ("root", root.to_string_lossy().into_owned()),
        ],
    );
}

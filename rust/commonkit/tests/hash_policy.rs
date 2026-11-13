//! The hash setting (contract §3.8, §10.6): `HASH` in `.policy.json`, then `COMMONKIT_HASH`,
//! then `b3`; resolved once per process and announced on stderr with its source. Needs no
//! hashing feature: the resolution lives in `policy`, beside the store's.

mod child;

use std::ffi::OsString;
use std::path::Path;

use commonkit::policy::{self, Algo, PolicyFile, Source};

fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |n: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| OsString::from(v))
    }
}

fn file(text: &str) -> (tempfile::TempDir, PolicyFile) {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(".policy.json"), text).unwrap();
    let f = policy::read_policy(d.path());
    (d, f)
}

#[test]
fn the_pure_resolution() {
    let p = Path::new("/t/.policy.json");
    let none = PolicyFile::default();
    assert_eq!(policy::resolve_hash(&none, p, &env(&[])).0, Algo::B3);
    assert_eq!(policy::resolve_hash(&none, p, &env(&[])).1, Source::Default);
    let r = policy::resolve_hash(&none, p, &env(&[("COMMONKIT_HASH", " MD5 ")]));
    assert_eq!((r.0, r.1), (Algo::Md5, Source::Env));
    let (_d, f) = file(r#"{"HASH": "md5"}"#);
    let r = policy::resolve_hash(&f, p, &env(&[("COMMONKIT_HASH", "b3")]));
    assert_eq!((r.0, r.1), (Algo::Md5, Source::File(p.to_path_buf())));
    let (_d, f) = file(r#"{"HASH": "sha1"}"#);
    let r = policy::resolve_hash(&f, p, &env(&[("COMMONKIT_HASH", "md5")]));
    assert_eq!((r.0, r.1, r.2.len()), (Algo::Md5, Source::Env, 1));
    let (_d, f) = file(r#"{"HASH": 5}"#);
    let r = policy::resolve_hash(&f, p, &env(&[("COMMONKIT_HASH", "x")]));
    assert_eq!((r.0, r.1, r.2.len()), (Algo::B3, Source::Default, 2));
    let r = policy::resolve_hash(&none, p, &env(&[("COMMONKIT_HASH", "")]));
    assert!(r.2.is_empty());
    assert_eq!(Source::Env.to_string(), "environment COMMONKIT_HASH");
    assert_eq!(
        Source::File(p.into()).to_string(),
        "policy file /t/.policy.json"
    );
}

/// In a child: print what this process resolved.
#[test]
fn child_prints_the_resolution() {
    if !child::is_child() {
        return;
    }
    println!(
        "algo={} source={}",
        policy::current().name(),
        policy::source()
    );
}

#[test]
fn the_file_beats_the_variable_and_the_line_says_so() {
    let d = child::dirs();
    std::fs::write(
        d.root.join(".policy.json"),
        r#"{"HASH": "md5", "TTL_S": 5}"#,
    )
    .unwrap();
    let o = child::run("child_prints_the_resolution", &d, Some("b3"), &[]);
    child::ok(&o);
    let pol = d.root.join(".policy.json");
    assert!(
        child::stdout(&o).contains("algo=md5 source=policy file"),
        "{}",
        child::stdout(&o)
    );
    let err = child::stderr(&o);
    assert_eq!(err.matches("commonkit: hash").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "commonkit: hash md5 (policy file {})",
            pol.display()
        )),
        "{err}"
    );
}

#[test]
fn a_malformed_value_falls_through_with_one_warning() {
    let d = child::dirs();
    std::fs::write(d.root.join(".policy.json"), r#"{"HASH": "blake2"}"#).unwrap();
    let o = child::run("child_prints_the_resolution", &d, Some("b3"), &[]);
    child::ok(&o);
    let err = child::stderr(&o);
    assert_eq!(err.matches("commonkit: ignoring HASH").count(), 1, "{err}");
    assert!(
        err.contains("commonkit: hash b3 (environment COMMONKIT_HASH)"),
        "{err}"
    );
}

#[test]
fn the_default_is_b3() {
    let d = child::dirs();
    let o = child::run("child_prints_the_resolution", &d, None, &[]);
    child::ok(&o);
    assert!(child::stderr(&o).contains("commonkit: hash b3 (default)"));
}

/// In a child: resolve, then say so on stdout (stderr may be gone).
#[test]
fn child_resolves_with_stderr_gone() {
    if !child::is_child() {
        return;
    }
    let algo = policy::current();
    println!("resolved {}", algo.name());
}

/// The first resolution's line can't fail a store call: a parent that has gone away leaves
/// stderr a broken pipe, where `eprintln!` would panic (finding 2 of the L0 review).
#[cfg(unix)]
#[test]
fn a_broken_stderr_never_panics_the_resolution() {
    use std::process::Stdio;
    let d = child::dirs();
    let mut c = child::command("child_resolves_with_stderr_gone", &d, Some("md5"), &[]);
    let mut ch = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(ch.stderr.take()); // the read end closes before the child starts writing
    let o = ch.wait_with_output().unwrap();
    assert!(o.status.success(), "{:?}\n{}", o.status, child::stdout(&o));
    assert!(
        child::stdout(&o).contains("resolved md5"),
        "{}",
        child::stdout(&o)
    );
}

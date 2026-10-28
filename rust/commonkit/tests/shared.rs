//! Rust ports of Python's `tests/test_shared.py` (each test keeps the Python name): the shared
//! store's tree-wide policy comes from `.policy.json` and the environment, never from a caller.
//!
//! Python's tests set the process environment with `monkeypatch`. Here the environment is a map
//! handed to `Policy::resolve` and `policy::resolve_root`, which is all `shared_store` adds to
//! them: `std::env::set_var` is `unsafe` in edition 2024 and would race other test threads.

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use common::Arr;
use commonkit::npy::Order;
use commonkit::policy::{self, ENV_PREFIX, Policy};
use commonkit::{AttachMode, KeyScheme, Mode, Store, Variants, keys, shared_store_at};

const DIGEST: &str = "0123456789abcdef0123456789abcdef";

fn scheme() -> Arc<KeyScheme> {
    static SCHEME: OnceLock<Arc<KeyScheme>> = OnceLock::new();
    SCHEME
        .get_or_init(|| {
            keys::register(
                KeyScheme::new(
                    "ckshared",
                    "v1",
                    ["ckshwav"],
                    Variants::pattern("sr[0-9]+").unwrap(),
                    &[],
                )
                .unwrap(),
            )
            .unwrap()
        })
        .clone()
}

/// An environment of `COMMONKIT_STORE_{name}` variables.
fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: BTreeMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| (format!("{ENV_PREFIX}{k}"), OsString::from(v)))
        .collect();
    move |name: &str| map.get(name).cloned()
}

/// What `shared_store` does, with `env` in place of the process environment.
fn shared(
    env: &dyn Fn(&str) -> Option<OsString>,
    mode: Mode,
    attach: AttachMode,
    min_array_bytes: u64,
) -> Store {
    let root = policy::resolve_root(env).unwrap();
    let policy = Policy::for_root(&root, env);
    Store::new(&root, policy.options(mode, attach, min_array_bytes))
}

fn shared_at(root: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Store {
    let policy = Policy::for_root(root, env);
    Store::new(
        root,
        policy.options(
            Mode::ReadWrite,
            AttachMode::Mmap,
            policy::DEFAULT_MIN_ARRAY_BYTES,
        ),
    )
}

#[test]
fn test_defaults_per_platform() {
    let none = env(&[]);
    let root = policy::resolve_root(&none).unwrap();
    let p = Policy::resolve(&Default::default(), &none);
    if cfg!(target_os = "macos") {
        assert!(root.ends_with("Library/Caches/commonkit/store"));
        assert_eq!(p.max_bytes, 2 << 30, "local snapshots pin reaped segments");
    } else if cfg!(unix) {
        assert_eq!(root, Path::new("/dev/shm/commonkit"));
        assert_eq!(p.max_bytes, 8 << 30);
    }
    assert_eq!(p.ttl_s, 900.0);
    assert_eq!(p.min_free_bytes, 8 << 30);
    assert_eq!(p.reap_interval_s, 30.0);
    assert_eq!(p.max_array_bytes, 128 << 20);
    assert!(!p.fsync && !p.verify_republish);
    assert_eq!(p, Policy::platform_default());
}

#[test]
fn test_environment_overrides_and_bad_values_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let e = env(&[
        ("ROOT", dir.path().to_str().unwrap()),
        ("TTL_S", "86400"),
        ("MAX_GB", "not a number"),
    ]);
    assert_eq!(policy::resolve_root(&e).unwrap(), dir.path());
    let p = Policy::resolve(&Default::default(), &e);
    assert_eq!(p.ttl_s, 86400.0);
    assert_eq!(p.max_bytes, Policy::platform_default().max_bytes);
}

#[test]
fn test_store_takes_policy_from_environment() {
    let dir = tempfile::tempdir().unwrap();
    let e = env(&[
        ("ROOT", dir.path().to_str().unwrap()),
        ("MAX_GB", "0.5"),
        ("TTL_S", "60"),
        ("MIN_FREE_GB", "0"),
    ]);
    let store = shared(&e, Mode::ReadWrite, AttachMode::Mmap, 1);
    assert_eq!(store.root(), dir.path());
    assert!(!store.is_disabled());
    let o = store.options();
    assert_eq!((o.max_bytes, o.ttl_s, o.min_free_bytes), (1 << 29, 60.0, 0));
    let key = scheme().key(DIGEST, "sr16000", Some("ckshwav")).unwrap();
    let samples = Arr::new(
        "<i2",
        &[1000],
        Order::C,
        (0..1000i16).flat_map(i16::to_le_bytes).collect(),
    );
    assert_eq!(store.publish(key.as_str(), samples.r()), Ok(true));
    assert_eq!(
        store.attach(key.as_str()).unwrap().bytes(),
        samples.data.as_slice()
    );
}

#[test]
fn test_two_callers_share_one_tree() {
    let dir = tempfile::tempdir().unwrap();
    let e = env(&[("ROOT", dir.path().to_str().unwrap()), ("MIN_FREE_GB", "0")]);
    let writer = shared(&e, Mode::ReadWrite, AttachMode::Mmap, 1);
    let reader = shared(
        &e,
        Mode::Read,
        AttachMode::Mmap,
        policy::DEFAULT_MIN_ARRAY_BYTES,
    );
    let key = scheme().key(DIGEST, "sr44100", Some("ckshwav")).unwrap();
    let ones = Arr::new(
        "<f4",
        &[10],
        Order::C,
        (0..10).flat_map(|_| 1f32.to_le_bytes()).collect(),
    );
    assert_eq!(writer.publish(key.as_str(), ones.r()), Ok(true));
    assert!(reader.attach(key.as_str()).is_some());
    let zeros = Arr::new("<f4", &[10], Order::C, vec![0; 40]);
    assert_eq!(
        reader.publish(key.as_str(), zeros.r()),
        Ok(false),
        "read mode never publishes"
    );
}

#[test]
fn test_root_argument_is_for_tests() {
    let dir = tempfile::tempdir().unwrap();
    let store = shared_store_at(dir.path(), Mode::ReadWrite, AttachMode::Mmap, 1);
    assert_eq!(store.root(), dir.path());
    assert!(!store.is_disabled());
}

#[test]
fn test_attach_is_the_callers_choice() {
    let dir = tempfile::tempdir().unwrap();
    let mapped = shared_store_at(dir.path(), Mode::ReadWrite, AttachMode::Mmap, 1);
    assert_eq!(mapped.attach_mode(), AttachMode::Mmap);
    let copied = shared_store_at(dir.path(), Mode::ReadWrite, AttachMode::Read, 1);
    assert_eq!(copied.attach_mode(), AttachMode::Read);
}

// ------------------------------------------------- .policy.json and the grammar (C7)

#[test]
fn test_the_policy_file_wins_over_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".policy.json"),
        r#"{"TTL_S": 604800, "FSYNC": true}"#,
    )
    .unwrap();
    let e = env(&[("TTL_S", "60"), ("MIN_FREE_GB", "1")]);
    let store = shared_at(dir.path(), &e);
    let o = store.options();
    assert_eq!(o.ttl_s, 604800.0);
    assert!(o.fsync);
    assert_eq!(o.min_free_bytes, 1 << 30, "the environment fills the rest");
}

#[test]
fn test_the_policy_file_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".policy.json"), r#"{"TTL_S": 100}"#).unwrap();
    let store = shared_at(dir.path(), &env(&[]));
    std::fs::write(dir.path().join(".policy.json"), r#"{"TTL_S": 200}"#).unwrap();
    assert_eq!(store.options().ttl_s, 100.0);
}

/// The contract names the top level; a nested object is not a setting at all.
#[test]
fn test_only_a_top_level_duplicate_invalidates_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".policy.json"),
        r#"{"X": {"a": 1, "a": 2}, "TTL_S": 100}"#,
    )
    .unwrap();
    assert_eq!(shared_at(dir.path(), &env(&[])).options().ttl_s, 100.0);
}

/// Opening a FIFO for reading blocks until a writer comes; a policy reader must not.
#[cfg(unix)]
#[test]
fn test_a_fifo_named_policy_json_is_absent_and_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join(".policy.json");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    assert!(policy::read_policy(dir.path()).is_empty());
}

#[test]
fn test_a_malformed_number_never_raises() {
    for raw in [
        "inf", "-inf", "nan", "1e999", "1_000", "١٢", "\u{a0}5", "0x10", "", " ", "5 5",
    ] {
        let vars: Vec<(&str, &str)> = [
            "MAX_GB",
            "MIN_FREE_GB",
            "MAX_ARRAY_BYTES",
            "TTL_S",
            "REAP_S",
        ]
        .iter()
        .map(|n| (*n, raw))
        .collect();
        let dir = tempfile::tempdir().unwrap();
        let store = shared_at(dir.path(), &env(&vars));
        let d = Policy::platform_default();
        assert_eq!(
            store.options().max_array_bytes,
            d.max_array_bytes,
            "{raw:?}"
        );
        assert_eq!(store.options().ttl_s, d.ttl_s, "{raw:?}");
        assert_eq!(store.options().max_bytes, d.max_bytes, "{raw:?}");
    }
}

#[test]
fn a_root_with_a_tilde_is_under_home() {
    let home = std::env::home_dir().unwrap();
    for (raw, want) in [("~/ckstore", home.join("ckstore")), ("~", home.clone())] {
        assert_eq!(policy::resolve_root(&env(&[("ROOT", raw)])), Some(want));
    }
    // `~user` is outside the contract and left alone.
    assert_eq!(
        policy::resolve_root(&env(&[("ROOT", "~nobody/x")])),
        Some("~nobody/x".into())
    );
}

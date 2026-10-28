//! The store-level conformance vectors, read from `conformance/vectors/` at test time:
//! `reads.json` (attach, both modes), `admission.json` (the publish gates), `existing_key.json`
//! (C15), `env.json` and `policy.json` (contract §10). `reaper_paths.json` is step 7.
//!
//! The environment is passed to `Policy::resolve` as a map rather than set in the process:
//! `std::env::set_var` is `unsafe` in edition 2024 and races other test threads.

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Arr, age, fixture, list, load, mtime, scheme_from, sha256, str_of, tmp_files};
use commonkit::npy::{self, Descr, NpyError, Order};
use commonkit::policy::{self, Policy, PolicyFile};
use commonkit::{AttachMode, Mode, Options, Store, keys};
use serde_json::Value;

/// The counters that moved, as the vectors write them.
fn counters(store: &Store) -> BTreeMap<String, u64> {
    store
        .drain_stats()
        .iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

fn expected_counters(v: &Value) -> BTreeMap<String, u64> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, n)| (k.clone(), n.as_u64().unwrap()))
        .collect()
}

fn register(doc: &Value) {
    keys::register(scheme_from(&doc["scheme"])).unwrap();
}

// ------------------------------------------------------------------------------ reads

#[test]
fn reads_in_both_attach_modes() {
    let doc = load("reads.json");
    register(&doc);
    let mut seen = 0;
    for v in list(&doc["vectors"]) {
        for attach in [AttachMode::Mmap, AttachMode::Read] {
            let id = format!("{} ({attach:?})", str_of(&v["id"]));
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(
                dir.path(),
                Options {
                    attach,
                    ..Options::default()
                },
            );
            let key = str_of(&v["key"]);
            if let Some(file) = v["file"].as_str() {
                let path = store.path_for(key).unwrap();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, fixture(file)).unwrap();
            }
            let seg = store.attach(key);
            let exp = &v["expect"];
            assert_eq!(
                counters(&store),
                expected_counters(&exp["counters"]),
                "{id}"
            );
            if str_of(&exp["outcome"]) != "hit" {
                assert!(seg.is_none(), "{id}");
                continue;
            }
            let seg = seg.unwrap_or_else(|| panic!("{id}: a miss"));
            assert_eq!(seg.is_mapped(), attach.maps(), "{id}");
            let shape: Vec<u64> = common::shape_of(&exp["shape"]);
            assert_eq!(seg.shape(), shape.as_slice(), "{id}");
            let order = if seg.fortran_order() {
                Order::F
            } else {
                Order::C
            };
            assert_eq!(
                npy::fortran_flag(seg.shape(), order),
                exp["fortran_order"].as_bool().unwrap(),
                "{id}"
            );
            if let Some(d) = exp["descr"].as_str() {
                assert_eq!(seg.descr_text(), d, "{id}");
            }
            assert_eq!(sha256(seg.bytes()), str_of(&exp["payload_sha256"]), "{id}");
            seen += 1;
        }
    }
    assert!(seen >= 14, "only {seen} hits checked");
}

// -------------------------------------------------------------------------- admission

fn mode_of(s: &str) -> Mode {
    match s {
        "off" => Mode::Off,
        "read" => Mode::Read,
        "readwrite" => Mode::ReadWrite,
        m => panic!("mode {m}"),
    }
}

#[test]
fn admission() {
    let doc = load("admission.json");
    register(&doc);
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let exp = &v["expect"];
        let spec = &v["array"];
        if str_of(&spec["fill"]) == "objects" {
            // An object dtype can't be expressed in Rust: `Descr` refuses it, so no ArrayRef of
            // one exists and nothing can reach `publish` (contract §5.4, §6.3). The vector's
            // outcome, nothing written, holds by construction; there is no counter to tick.
            assert_eq!(str_of(&exp["outcome"]), "declined:dtype", "{id}");
            assert_eq!(
                Descr::parse(str_of(&spec["descr"])),
                Err(NpyError::ObjectDtype),
                "{id}"
            );
            continue;
        }
        let s = &v["store"];
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(
            dir.path(),
            Options {
                mode: mode_of(str_of(&s["mode"])),
                min_array_bytes: s["min_array_bytes"].as_u64().unwrap(),
                max_array_bytes: s["max_array_bytes"].as_i64().unwrap(),
                min_free_bytes: s["min_free_bytes"].as_i64().unwrap(),
                max_bytes: s["max_bytes"].as_i64().unwrap(),
                reap_interval_s: 1e12,
                ..Options::default()
            },
        );
        if let Some(usage) = v.get("usage") {
            std::fs::write(dir.path().join(".usage.json"), usage.to_string()).unwrap();
        }
        let arr = Arr::from_spec(spec);
        let ok = store.publish(str_of(&v["key"]), arr.r()).unwrap();
        assert_eq!(ok, str_of(&exp["outcome"]) == "published", "{id}");
        assert_eq!(
            counters(&store),
            expected_counters(&exp["counters"]),
            "{id}"
        );
        assert!(
            tmp_files(dir.path()).is_empty(),
            "{id}: a .tmp file was left"
        );
        if ok {
            let seg = store.attach(str_of(&v["key"])).unwrap();
            assert_eq!(seg.bytes(), arr.data.as_slice(), "{id}");
        }
    }
}

// ----------------------------------------------------------------------- existing key

#[test]
fn existing_key() {
    let doc = load("existing_key.json");
    register(&doc);
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(
            dir.path(),
            Options {
                mode: Mode::ReadWrite,
                min_array_bytes: 0,
                min_free_bytes: 0,
                reap_interval_s: 1e12,
                verify_republish: v["verify_republish"].as_bool().unwrap(),
                ..Options::default()
            },
        );
        let key = str_of(&v["key"]);
        let path = store.path_for(key).unwrap();
        let mut old = None;
        let mut then = SystemTime::now();
        if let Some(existing) = v["existing"].as_object() {
            let bytes = fixture(str_of(&existing["file"]));
            assert_eq!(sha256(&bytes), str_of(&existing["sha256"]), "{id}");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            then = age(&path, existing["mtime_age_s"].as_f64().unwrap());
            old = Some(bytes);
        }
        let start = SystemTime::now();
        let arr = Arr::from_spec(&v["array"]);
        let outcome = match store.publish(key, arr.r()) {
            Ok(true) => "published",
            Ok(false) => "no",
            Err(c) => {
                assert_eq!(c.key, key);
                assert_eq!(c.path, path);
                "KeyCollision"
            }
        };
        let exp = &v["expect"];
        let want = match str_of(&exp["outcome"]) {
            "touched" => "published", // a republish reports success
            o => o,
        };
        assert_eq!(outcome, want, "{id}");
        assert_eq!(
            counters(&store),
            expected_counters(&exp["counters"]),
            "{id}"
        );
        let after = std::fs::read(&path).unwrap();
        if str_of(&exp["file_after"]) == "unchanged" {
            assert_eq!(Some(&after), old.as_ref(), "{id}");
        } else {
            assert_eq!(sha256(&after), str_of(&exp["file_sha256"]), "{id}");
        }
        let m = mtime(&path);
        if str_of(&exp["mtime"]) == "unchanged" {
            let drift = m.duration_since(then).unwrap_or_else(|e| e.duration());
            assert!(drift < Duration::from_secs(1), "{id}: mtime moved");
        } else {
            assert!(
                m + Duration::from_secs(2) >= start,
                "{id}: mtime not refreshed"
            );
        }
        assert!(tmp_files(dir.path()).is_empty(), "{id}");
    }
}

// ------------------------------------------------------------------------ env, policy

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

fn env_of(v: &Value) -> impl Fn(&str) -> Option<OsString> {
    let map: BTreeMap<String, String> = v
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, x)| (k.clone(), str_of(x).to_owned()))
        .collect();
    move |name: &str| map.get(name).map(OsString::from)
}

fn placeholders(text: &str, root: &Path) -> PathBuf {
    let home = std::env::home_dir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    PathBuf::from(
        text.replace("{home}", home.to_str().unwrap())
            .replace("{cwd}", cwd.to_str().unwrap())
            .replace("{root}", root.to_str().unwrap()),
    )
}

/// Check the effective store fields a vector names. `root` is checked by the caller.
fn check_fields(id: &str, o: &Options, expect: &Value, defaults: &Value) {
    for (field, want) in expect.as_object().unwrap() {
        let want = if want == "default" {
            &defaults[field]
        } else {
            want
        };
        match field.as_str() {
            "root" => {}
            "max_bytes" => assert_eq!(o.max_bytes, want.as_i64().unwrap(), "{id} {field}"),
            "min_free_bytes" => {
                assert_eq!(o.min_free_bytes, want.as_i64().unwrap(), "{id} {field}")
            }
            "max_array_bytes" => {
                assert_eq!(o.max_array_bytes, want.as_i64().unwrap(), "{id} {field}")
            }
            "ttl_s" => assert_eq!(o.ttl_s, want.as_f64().unwrap(), "{id} {field}"),
            "reap_interval_s" => {
                assert_eq!(o.reap_interval_s, want.as_f64().unwrap(), "{id} {field}")
            }
            "fsync" => assert_eq!(o.fsync, want.as_bool().unwrap(), "{id} {field}"),
            "verify_republish" => {
                assert_eq!(o.verify_republish, want.as_bool().unwrap(), "{id} {field}")
            }
            f => panic!("{id}: unknown field {f}"),
        }
    }
}

#[test]
fn env() {
    let doc = load("env.json");
    let defaults = doc["defaults"]
        .get(platform())
        .unwrap_or(&doc["defaults"]["linux"])
        .clone();
    let mut roots = 0;
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let env = env_of(&v["env"]);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(
            dir.path(),
            Policy::for_root(dir.path(), &env).options(Mode::Read, AttachMode::Mmap, 0),
        );
        assert!(!store.is_disabled(), "{id}");
        check_fields(id, store.options(), &v["expect"], &defaults);
        if let Some(want) = v["expect"].get("root") {
            let want = match want {
                Value::Object(per) => match per.get(platform()) {
                    Some(Value::String(s)) => s.clone(),
                    _ => continue, // Windows: unspecified until the Windows work merges (§12)
                },
                w => str_of(w).to_owned(),
            };
            let got = policy::resolve_root(&env).unwrap();
            let got = Store::new(
                &got,
                Options {
                    mode: Mode::Off,
                    ..Options::default()
                },
            );
            assert_eq!(got.root(), placeholders(&want, dir.path()), "{id}");
            roots += 1;
        }
    }
    assert_eq!(roots, 5);
}

#[test]
fn policy_file() {
    let doc = load("policy.json");
    let env_doc = load("env.json");
    let defaults = env_doc["defaults"]
        .get(platform())
        .unwrap_or(&env_doc["defaults"]["linux"])
        .clone();
    for v in list(&doc["vectors"]) {
        let id = str_of(&v["id"]);
        let env = env_of(&v["env"]);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".policy.json");
        match &v["policy_file"] {
            Value::Null => {}
            pf if pf.get("text").is_some() => std::fs::write(&target, str_of(&pf["text"])).unwrap(),
            pf if pf.get("file").is_some() => {
                std::fs::write(&target, fixture(str_of(&pf["file"]))).unwrap()
            }
            _ => std::fs::create_dir(&target).unwrap(),
        }
        let store = Store::new(
            dir.path(),
            Policy::for_root(dir.path(), &env).options(Mode::Read, AttachMode::Mmap, 0),
        );
        check_fields(id, store.options(), &v["expect"], &defaults);
        if let Some(want) = v["expect"].get("root") {
            assert_eq!(store.root(), placeholders(str_of(want), dir.path()), "{id}");
        }
    }
}

/// An invalid file reads as absent, so it can't change anything the environment says.
#[test]
fn an_invalid_policy_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".policy.json"),
        "{\"TTL_S\": 1, \"TTL_S\": 2}",
    )
    .unwrap();
    assert_eq!(policy::read_policy(dir.path()), PolicyFile::default());
}

// ---------------------------------------------------------------------- reaper paths

/// `reaper_paths.json` tests the reaper (contract §9.4, §9.5), which is step 7 of the plan.
#[test]
#[ignore = "step 7: the reaper; reaper_paths.json runs once Store::reap exists"]
fn reaper_paths() {
    let doc = load("reaper_paths.json");
    assert!(!list(&doc["vectors"]).is_empty());
}

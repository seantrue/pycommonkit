//! Cross-language conformance from the Rust side (plan step 8): the crate in-process against
//! the Python package through its driver, `uv run python conformance/ck_conform.py`.
//! `tests/test_conformance.py` runs the full matrix through both drivers; these are the same
//! cases with Rust holding the pen.
//!
//! Without `uv` on PATH these tests print why and pass, unless `CK_CONFORM_REQUIRE_PYTHON` is
//! set (CI's conformance job sets it), when they fail instead.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ck_conform::{Array, describe, register_scheme, sha256};
use commonkit::{Mode, Options, ReapOptions, Store};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Python driver, or `None` (with the reason printed) when `uv` isn't available.
fn python() -> Option<Command> {
    let found = Command::new("uv")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !found {
        assert!(
            std::env::var_os("CK_CONFORM_REQUIRE_PYTHON").is_none(),
            "uv is not on PATH and CK_CONFORM_REQUIRE_PYTHON is set"
        );
        eprintln!("skipped: uv is not on PATH, so the Python side can't run");
        return None;
    }
    let mut cmd = Command::new("uv");
    cmd.current_dir(repo())
        .args(["run", "--quiet", "python", "conformance/ck_conform.py"]);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("COMMONKIT_STORE_") {
            cmd.env_remove(k);
        }
    }
    Some(cmd)
}

fn py(cmd: &str, args: &Value) -> Option<Value> {
    let mut c = python()?;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("args.json");
    fs::write(&file, args.to_string()).unwrap();
    let out = c
        .arg(cmd)
        .arg(format!("@{}", file.display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "python {cmd}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).unwrap())
}

const D: fn(usize) -> String = |i| format!("{i:02x}").repeat(16);

fn key(i: usize) -> String {
    format!("ckconf:v1:raw:{}:x0", D(i))
}

fn relpath(i: usize) -> PathBuf {
    let d = D(i);
    PathBuf::from(format!("raw/{}/{d}.x0.npy", &d[..2]))
}

fn options() -> Options {
    Options {
        mode: Mode::ReadWrite,
        max_bytes: 1 << 40,
        min_free_bytes: 0,
        ttl_s: 1e9,
        min_array_bytes: 0,
        max_array_bytes: 1 << 40,
        reap_interval_s: 1e12,
        verify_republish: true,
        ..Options::default()
    }
}

fn store_json(root: &Path) -> Value {
    json!({ "root": root.to_str().unwrap() })
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn age(path: &Path, ago: f64) {
    let t = UNIX_EPOCH + Duration::from_secs_f64(now() - ago);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

fn headers() -> Vec<Value> {
    let text = fs::read_to_string(repo().join("conformance/vectors/headers.json")).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["vectors"].as_array().unwrap().clone()
}

fn arr(shape: u64, fill: &str) -> Value {
    json!({ "descr": "|u1", "shape": [shape], "order": "C", "fill": fill })
}

fn publish(store: &Store, i: usize, spec: &Value) -> Result<bool, commonkit::KeyCollision> {
    register_scheme().unwrap();
    let a = Array::from_spec(spec).unwrap();
    store.publish(&key(i), a.as_ref().unwrap())
}

#[test]
fn rust_and_python_write_identical_files_and_read_each_others() {
    if python().is_none() {
        return;
    }
    let vectors = headers();
    let dir = tempfile::tempdir().unwrap();
    let (rs_root, py_root) = (dir.path().join("rs"), dir.path().join("py"));
    let rs = Store::new(&rs_root, options());
    for (i, v) in vectors.iter().enumerate() {
        assert_eq!(publish(&rs, i, &v["array"]), Ok(true), "{}", v["id"]);
    }
    let ops: Vec<Value> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| json!({"cmd": "publish", "store": store_json(&py_root), "key": key(i), "array": v["array"]}))
        .collect();
    let outs = py("batch", &Value::Array(ops)).unwrap();
    for (v, out) in vectors.iter().zip(outs.as_array().unwrap()) {
        assert_eq!(out["stored"], true, "{}", v["id"]);
    }

    // Python attaches Rust's files; Rust attaches Python's, in-process.
    let ops: Vec<Value> = (0..vectors.len())
        .map(|i| json!({"cmd": "attach", "store": store_json(&rs_root), "key": key(i)}))
        .collect();
    let py_saw = py("batch", &Value::Array(ops)).unwrap();
    let py_store = Store::new(&py_root, options());
    for (i, v) in vectors.iter().enumerate() {
        let id = &v["id"];
        let a = fs::read(rs_root.join(relpath(i))).unwrap();
        let b = fs::read(py_root.join(relpath(i))).unwrap();
        assert!(a == b, "{id}: files differ");
        assert_eq!(sha256(&a), v["sha256"].as_str().unwrap(), "{id}");
        let off = v["data_offset"].as_u64().unwrap() as usize;
        let want = sha256(&a[off..]);
        let seg = py_store.attach(&key(i)).unwrap();
        let rs_saw = describe(&seg);
        let p = &py_saw[i];
        for k in ["descr", "shape", "fortran_order", "payload_sha256"] {
            assert_eq!(rs_saw[k], p[k], "{id}: {k}");
        }
        // Without the `mmap` feature Rust copies every segment.
        if cfg!(feature = "mmap") {
            assert_eq!(rs_saw["mapped"], p["mapped"], "{id}: mapped");
        }
        assert_eq!(rs_saw["payload_sha256"], want, "{id}");
        if !v["descr"].is_null() {
            assert_eq!(rs_saw["descr"], v["descr"], "{id}");
        }
    }
}

#[test]
fn a_python_touch_protects_a_segment_from_the_rust_reaper() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(&root, options());
    for i in 0..4 {
        assert_eq!(publish(&rs, i, &arr(1000, "ramp")), Ok(true));
        age(&root.join(relpath(i)), 1000.0 - i as f64 * 10.0);
    }
    let out = py(
        "attach",
        &json!({"store": store_json(&root), "key": key(0)}),
    )
    .unwrap();
    assert_eq!(out["hit"], true);
    let size = fs::metadata(root.join(relpath(0))).unwrap().len() as f64;
    let reaper = Store::new(
        &root,
        Options {
            max_bytes: (2.5 * size / 0.8) as i64,
            ..options()
        },
    );
    assert_eq!(reaper.reap(Duration::from_secs(10)).reaped, 2);
    assert!(root.join(relpath(0)).exists() && root.join(relpath(3)).exists());
    assert!(!root.join(relpath(1)).exists() && !root.join(relpath(2)).exists());
}

#[test]
fn collisions_are_raised_on_both_sides() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(&root, options());
    // Python first, then Rust with other bytes.
    let out = py(
        "publish",
        &json!({"store": store_json(&root), "key": key(1), "array": arr(1000, "ramp")}),
    )
    .unwrap();
    assert_eq!(out["stored"], true);
    assert!(publish(&rs, 1, &arr(1000, "ramp2")).is_err());
    assert_eq!(rs.drain_stats().get("shm_key_collision"), 1);
    assert_eq!(publish(&rs, 1, &arr(1000, "ramp")), Ok(true)); // the same array: a republish
    // Rust first, then Python with another shape.
    assert_eq!(publish(&rs, 2, &arr(1000, "ramp")), Ok(true));
    let out = py(
        "publish",
        &json!({"store": store_json(&root), "key": key(2), "array": arr(999, "ramp")}),
    )
    .unwrap();
    assert_eq!(
        out,
        json!({"collision": true, "stats": {"shm_key_collision": 1}})
    );
}

#[test]
fn a_rust_mapping_survives_the_python_reaper() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(&root, options());
    assert_eq!(publish(&rs, 3, &arr(1 << 20, "ramp")), Ok(true));
    let seg = rs.attach(&key(3)).unwrap();
    let before = sha256(seg.bytes());
    age(&root.join(relpath(3)), 7200.0);
    let out = py(
        "reap",
        &json!({"store": {"root": root.to_str().unwrap(), "ttl_s": 3600}}),
    )
    .unwrap();
    assert_eq!(out["reaped"], 1);
    assert!(!root.join(relpath(3)).exists());
    assert_eq!(sha256(seg.bytes()), before);
}

#[test]
fn a_python_mapping_survives_the_rust_reaper() {
    let Some(mut cmd) = python() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(&root, options());
    assert_eq!(publish(&rs, 4, &arr(1 << 20, "ramp")), Ok(true));
    let mut child = cmd
        .arg("hold")
        .arg(json!({"store": store_json(&root), "key": key(4)}).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(
        (&first["hit"], &first["mapped"]),
        (&json!(true), &json!(true))
    );
    age(&root.join(relpath(4)), 7200.0);
    let reaper = Store::new(
        &root,
        Options {
            ttl_s: 3600.0,
            ..options()
        },
    );
    assert_eq!(reaper.reap(Duration::from_secs(10)).reaped, 1);
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let after: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        after,
        json!({"payload_sha256": first["payload_sha256"], "path_exists": false})
    );
}

#[test]
fn python_usage_feeds_the_rust_budget_gate_and_back() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    for i in 0..3 {
        let out = py(
            "publish",
            &json!({"store": store_json(&root), "key": key(i), "array": arr(1000, "ramp")}),
        )
        .unwrap();
        assert_eq!(out["stored"], true);
    }
    assert_eq!(
        py("reap", &json!({"store": store_json(&root)})).unwrap()["complete"],
        true
    );
    let used: u64 = (0..3)
        .map(|i| fs::metadata(root.join(relpath(i))).unwrap().len())
        .sum();
    let usage = Store::new(&root, options()).usage().unwrap();
    assert_eq!((usage.bytes, usage.files), (used, Some(3)));
    let tight = Store::new(
        &root,
        Options {
            max_bytes: used as i64 + 999,
            ..options()
        },
    );
    assert_eq!(publish(&tight, 9, &arr(1000, "ramp")), Ok(false));
    assert_eq!(tight.drain_stats().get("shm_declined_budget"), 1);

    // And Rust's .usage.json, read by Python.
    assert!(
        Store::new(&root, options())
            .reap(Duration::from_secs(10))
            .complete
    );
    let out = py("usage", &json!({"store": store_json(&root)})).unwrap();
    assert_eq!(out["usage"], json!({"bytes": used, "files": 3}));
}

#[test]
fn a_lock_left_by_a_dead_python_reaper_is_broken_by_rust() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(
        &root,
        Options {
            ttl_s: 3600.0,
            ..options()
        },
    );
    for i in 0..3 {
        assert_eq!(publish(&rs, i, &arr(1000, "ramp")), Ok(true));
        age(&root.join(relpath(i)), 7200.0);
    }
    let mut cmd = python().unwrap();
    let status = cmd
        .arg("reap")
        .arg(json!({"store": store_json(&root), "die_in_walk": true}).to_string())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(17));
    let lock = root.join(".reap.lock");
    let pid = fs::read_to_string(&lock).unwrap();
    assert!(!pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(rs.reap(Duration::from_secs(10)).reaped, 0);
    assert!(lock.exists());
    age(&lock, 301.0);
    assert_eq!(rs.reap(Duration::from_secs(10)).reaped, 0);
    assert!(!lock.exists());
    assert_eq!(rs.reap(Duration::from_secs(10)).reaped, 3);
}

#[test]
fn dry_runs_plan_the_same_in_both_languages() {
    if python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s");
    let rs = Store::new(&root, options());
    let t = now();
    for i in 0..12 {
        assert_eq!(
            publish(&rs, i, &arr(1000 + 100 * (i as u64 % 4), "zeros")),
            Ok(true)
        );
        let ago = if i < 2 {
            4000.0 - i as f64 * 100.0
        } else {
            1000.0 - i as f64 * 10.0
        };
        age(&root.join(relpath(i)), ago);
    }
    let orphan = root.join("raw/00/x.npy.1.2.tmp");
    fs::write(&orphan, b"x").unwrap();
    age(&orphan, 4000.0);
    let opts = Options {
        max_bytes: 10_000,
        ttl_s: 3600.0,
        ..options()
    };
    let plan = Store::new(&root, opts).reap_with(&ReapOptions {
        budget: Duration::from_secs(10),
        dry_run: true,
        now: Some(t),
    });
    let theirs = py(
        "reap",
        &json!({"store": {"root": root.to_str().unwrap(), "max_bytes": 10_000, "ttl_s": 3600},
                "dry_run": true, "now": t}),
    )
    .unwrap();
    let rel = |ps: &[PathBuf]| -> Vec<String> {
        ps.iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    };
    assert_eq!(json!(rel(&plan.unlink)), theirs["unlink"]);
    assert_eq!(json!(rel(&plan.sweep)), theirs["sweep"]);
    assert_eq!(json!(plan.low_water), theirs["low_water"]);
    assert_eq!(plan.unlink.len(), 6);
    let swept: BTreeSet<_> = rel(&plan.sweep).into_iter().collect();
    assert_eq!(swept, BTreeSet::from(["raw/00/x.npy.1.2.tmp".to_owned()]));
}

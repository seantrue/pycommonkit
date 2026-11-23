#![cfg(unix)]
//! `prune_folder` against the shared vectors (`conformance/vectors/prune.json`), as Python's
//! `tests/test_prune.py`, and its guards. Every tree is a temporary directory.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use commonkit::{PruneOptions, prune_folder};
use serde_json::Value;

fn vectors() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/vectors/prune.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn set_mtime(path: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn the_vectors_hold() {
    let doc = vectors();
    let opts = &doc["prune"];
    let cases = doc["vectors"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for v in cases {
        let id = v["id"].as_str().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("edits");
        fs::create_dir(&root).unwrap();
        let now = SystemTime::now();
        let mut disclaimed = BTreeSet::new();
        for e in v["entries"].as_array().unwrap() {
            let name = e["name"].as_str().unwrap();
            let p = root.join(name);
            match e["kind"].as_str().unwrap() {
                "dir" => fs::create_dir(&p).unwrap(),
                "symlink" => {
                    let target = tmp.path().join(format!("target-{name}"));
                    fs::write(&target, b"x").unwrap();
                    std::os::unix::fs::symlink(&target, &p).unwrap();
                }
                _ => fs::write(&p, b"x").unwrap(),
            }
            if e["kind"] == "file" {
                set_mtime(
                    &p,
                    now - Duration::from_secs_f64(e["age_s"].as_f64().unwrap()),
                );
            }
            if e["owned"] == false {
                disclaimed.insert(name.to_owned());
            }
        }
        let before = listing(&root);
        let mut o = PruneOptions {
            pattern: opts["pattern"].as_str().unwrap().into(),
            keep: v["keep"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| k.as_str().unwrap().to_owned())
                .collect(),
            min_age: Duration::from_secs_f64(opts["min_age_s"].as_f64().unwrap()),
            companions: opts["companions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap().to_owned())
                .collect(),
            dry_run: true,
            now: Some(now),
        };
        let owns = |p: &Path| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| disclaimed.contains(n))
        };
        let dry = prune_folder(&root, &o, Some(&owns));
        assert_eq!(listing(&root), before, "{id}: a dry run removes nothing");
        o.dry_run = false;
        let got = prune_folder(&root, &o, Some(&owns));
        let want = &v["expect"];
        let removed: Vec<String> = want["removed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(got.removed, removed, "{id}");
        assert_eq!(
            (got.kept as u64, got.young as u64, got.foreign as u64),
            (
                want["kept"].as_u64().unwrap(),
                want["young"].as_u64().unwrap(),
                want["foreign"].as_u64().unwrap()
            ),
            "{id}"
        );
        assert_eq!(dry.removed, got.removed, "{id}");
        assert_eq!((got.failed, got.refused.clone()), (0, None), "{id}");
        let left: Vec<String> = before
            .into_iter()
            .filter(|n| !removed.contains(n))
            .collect();
        assert_eq!(listing(&root), left, "{id}");
    }
}

#[test]
fn it_refuses_what_isnt_a_real_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    fs::write(tmp.path().join("file"), b"x").unwrap();
    let o = PruneOptions {
        pattern: ".*".into(),
        ..Default::default()
    };
    for p in [link, tmp.path().join("missing"), tmp.path().join("file")] {
        assert!(
            prune_folder(&p, &o, None).refused.is_some(),
            "{}",
            p.display()
        );
    }
    assert!(
        prune_folder(
            &real,
            &PruneOptions {
                pattern: "(".into(),
                ..Default::default()
            },
            None
        )
        .refused
        .is_some()
    );
}

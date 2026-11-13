//! The per-scheme digest rule (contract §3.7, D3 (a)) and the marker files (§11).
//!
//! A scheme with `DigestRule::Current` holds only `current()`'s digest length: the other is
//! declined on publish and a miss on attach, both counted `shm_foreign_digest`. A scheme
//! with no rule is exactly as before (the alexandria constraint: `pixels` declares none).

mod child;

use std::path::Path;

use commonkit::keys::DigestRule;
use commonkit::{ArrayRef, Descr, KeyScheme, Mode, Options, Order, Store, Variants, keys};

const D32: &str = "abababababababababababababababab";

fn d64() -> String {
    "cd".repeat(32)
}

fn open(root: &Path) -> Store {
    Store::new(
        root,
        Options {
            mode: Mode::ReadWrite,
            min_free_bytes: 0,
            min_array_bytes: 0,
            reap_interval_s: 0.0,
            ..Options::default()
        },
    )
}

fn rule() -> std::sync::Arc<KeyScheme> {
    let s = KeyScheme::new(
        "ckrule",
        "v1",
        ["arr"],
        Variants::closed(["x0"]).unwrap(),
        &[],
    )
    .unwrap()
    .with_digest(DigestRule::Current);
    keys::register(s).unwrap()
}

fn free() -> std::sync::Arc<KeyScheme> {
    let s = KeyScheme::new(
        "ckpix",
        "v3",
        ["pil"],
        Variants::closed(["o0"]).unwrap(),
        &[],
    );
    keys::register(s.unwrap()).unwrap()
}

fn publish(store: &Store, key: &str) -> bool {
    let descr = Descr::parse("|u1").unwrap();
    let data = [7u8; 16];
    store
        .publish(key, ArrayRef::new(&descr, &[16], Order::C, &data).unwrap())
        .unwrap()
}

fn root() -> std::path::PathBuf {
    std::env::var_os("COMMONKIT_STORE_ROOT").unwrap().into()
}

/// In a child: publish and attach one key of each length under the rule; print the stats.
#[test]
fn child_rule_scheme() {
    if !child::is_child() {
        return;
    }
    let s = rule();
    let store = open(&root());
    let k32 = s.key(D32, "x0", None).unwrap();
    let k64 = s.key(&d64(), "x0", None).unwrap();
    let cur = commonkit::policy::current();
    let (mine, other) = if cur.hex_len() == 32 {
        (&k32, &k64)
    } else {
        (&k64, &k32)
    };
    assert!(!publish(&store, other.as_str()));
    assert!(store.attach(other.as_str()).is_none());
    assert!(publish(&store, mine.as_str()));
    assert!(store.attach(mine.as_str()).is_some());
    let st = store.drain_stats();
    println!(
        "FOREIGN={} PUBLISH={}",
        st.get("shm_foreign_digest"),
        st.get("shm_publish")
    );
    if std::env::var_os("CK_KILL").is_some() {
        let pid = std::process::id().to_string();
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid])
            .status();
    }
}

#[test]
fn a_rule_scheme_declines_and_misses_the_other_length() {
    for algo in ["b3", "md5"] {
        let d = child::dirs();
        let o = child::run("child_rule_scheme", &d, Some(algo), &[]);
        child::ok(&o);
        assert!(child::stdout(&o).contains("FOREIGN=2 PUBLISH=1"), "{algo}");
        assert_eq!(child::marks_in(&d.marks), ["shm_foreign_digest", "started"]);
    }
}

#[test]
fn a_child_killed_right_after_the_event_still_leaves_its_marker() {
    let d = child::dirs();
    let o = child::run(
        "child_rule_scheme",
        &d,
        Some("b3"),
        &[("CK_KILL", Some("1"))],
    );
    assert!(!o.status.success());
    assert!(child::marks_in(&d.marks).contains(&"shm_foreign_digest".to_owned()));
}

/// In a child: a no-rule scheme shaped like `pixels`, with marks off for its store.
#[test]
fn child_no_rule_scheme() {
    if !child::is_child() {
        return;
    }
    let s = free();
    let store = open(&root());
    for d in [D32.to_owned(), d64()] {
        let k = s.key(&d, "o0", None).unwrap();
        assert!(publish(&store, k.as_str()) && store.attach(k.as_str()).is_some());
    }
    let st = store.drain_stats();
    println!(
        "FOREIGN={} PUBLISH={}",
        st.get("shm_foreign_digest"),
        st.get("shm_publish")
    );
}

#[test]
fn a_no_rule_scheme_takes_both_lengths_and_never_resolves() {
    for algo in ["b3", "md5"] {
        let d = child::dirs();
        let o = child::run(
            "child_no_rule_scheme",
            &d,
            Some(algo),
            &[("COMMONKIT_MARK_DIR", None)],
        );
        child::ok(&o);
        assert!(child::stdout(&o).contains("FOREIGN=0 PUBLISH=2"), "{algo}");
        // nothing resolved the hash setting, so nothing announced it
        assert!(
            !child::stderr(&o).contains("commonkit: hash"),
            "{}",
            child::stderr(&o)
        );
        assert!(!d.marks.exists());
    }
}

/// In a child: open a store, publish a bad variant, print the pid.
#[test]
fn child_bad_key() {
    if !child::is_child() {
        return;
    }
    rule();
    let store = open(&root());
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x9", d64())));
    println!("PID={}", std::process::id());
}

#[test]
fn started_and_bad_key_markers_name_what_happened() {
    let d = child::dirs();
    let o = child::run("child_bad_key", &d, Some("md5"), &[]);
    child::ok(&o);
    let out = child::stdout(&o);
    let pid = out
        .split("PID=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let started: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(d.marks.join(format!("{pid}.started"))).unwrap(),
    )
    .unwrap();
    assert_eq!(started["algo"], "md5");
    assert_eq!(started["source"], "environment COMMONKIT_HASH");
    assert_eq!(started["root"], d.root.to_str().unwrap());
    let bad: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(d.marks.join(format!("{pid}.shm_bad_key"))).unwrap(),
    )
    .unwrap();
    assert!(bad["key"].as_str().unwrap().ends_with(":x9"));
    assert_eq!(bad["counter"], "shm_bad_key");
}

#[test]
fn a_missing_mark_folder_is_created() {
    let d = child::dirs();
    let deep = d.run.join("a/b/marks");
    let o = child::run(
        "child_bad_key",
        &d,
        Some("b3"),
        &[("COMMONKIT_MARK_DIR", Some(deep.to_str().unwrap()))],
    );
    child::ok(&o);
    assert_eq!(child::marks_in(&deep), ["shm_bad_key", "started"]);
}

#[test]
fn with_mark_dir_sends_a_stores_marks_elsewhere() {
    if child::is_child() {
        return;
    }
    rule();
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("marks");
    let store = open(&dir.path().join("root")).with_mark_dir(&scratch);
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x9", d64())));
    assert!(child::marks_in(&scratch).contains(&"shm_bad_key".to_owned()));
}

#[test]
fn the_rule_is_part_of_a_schemes_identity() {
    let a = KeyScheme::new(
        "ckident",
        "v1",
        ["arr"],
        Variants::closed(["x0"]).unwrap(),
        &[],
    );
    let a = a.unwrap();
    let b = a.clone().with_digest(DigestRule::Current);
    assert_ne!(a, b);
    assert_eq!(b.digest_rule(), Some(DigestRule::Current));
    assert_eq!(a.digest_rule(), None);
    let reg = keys::Registry::new();
    reg.register(b).unwrap();
    assert!(reg.register(a).is_err());
}

/// A store marking into a fresh scratch folder, and the folder (finding 4 of the L0 review:
/// nothing failed when markers stopped being first-event-only).
fn scratch_store() -> (tempfile::TempDir, Store, std::path::PathBuf) {
    rule();
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("marks");
    let store = open(&dir.path().join("root")).with_mark_dir(&scratch);
    (dir, store, scratch)
}

fn bad_key_marker(scratch: &Path) -> std::path::PathBuf {
    scratch.join(format!("{}.shm_bad_key", std::process::id()))
}

#[test]
fn a_marker_names_the_first_event_only() {
    if child::is_child() {
        return;
    }
    let (_dir, store, scratch) = scratch_store();
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x8", d64())));
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x9", d64())));
    let text = std::fs::read_to_string(bad_key_marker(&scratch)).unwrap();
    assert!(text.contains(":x8"), "{text}");
    assert!(!text.contains(":x9"), "{text}");
}

#[test]
fn a_process_writes_each_marker_once_even_after_it_is_removed() {
    if child::is_child() {
        return;
    }
    let (_dir, store, scratch) = scratch_store();
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x8", d64())));
    let marker = bad_key_marker(&scratch);
    std::fs::remove_file(&marker).unwrap();
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x9", d64())));
    assert!(!marker.exists(), "a second event wrote the marker again");
}

#[test]
fn a_marker_never_overwrites_a_file_already_there() {
    if child::is_child() {
        return;
    }
    let (_dir, store, scratch) = scratch_store();
    let marker = bad_key_marker(&scratch);
    std::fs::write(&marker, "planted\n").unwrap();
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x8", d64())));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "planted\n");
}

/// Markers are 0600, as Python's are: they name keys and roots.
#[cfg(unix)]
#[test]
fn markers_are_private_to_their_owner() {
    use std::os::unix::fs::PermissionsExt;
    if child::is_child() {
        return;
    }
    let (_dir, store, scratch) = scratch_store();
    assert!(!publish(&store, &format!("ckrule:v1:arr:{}:x8", d64())));
    for name in ["started", "shm_bad_key"] {
        let f = scratch.join(format!("{}.{name}", std::process::id()));
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", f.display());
    }
    let dir_mode = std::fs::metadata(&scratch).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
}

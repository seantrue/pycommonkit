//! The reaper's dry run (contract §9.10, an implementation aid), as Python's
//! `tests/test_dry_run.py`: it plans exactly what a reap would unlink and sweep, and changes
//! nothing (no lock, no unlink, no sweep, no `.usage.json`, no mtime). Proved here on a
//! temporary tree, by comparing every entry's `lstat` before and after, before step 8 ran it
//! against a live one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use commonkit::{Mode, Options, ReapOptions, Store, reap_plan};

const NOW: f64 = 1_800_000_000.0;
const BUDGET: Duration = Duration::from_secs(10);

fn at(t: f64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs_f64(t)
}

fn set_mtime(path: &Path, t: f64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(at(t)).set_modified(at(t)))
        .unwrap();
}

fn plain(path: PathBuf, mtime: f64, data: &[u8]) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, data).unwrap();
    set_mtime(&path, mtime);
    path
}

/// A `.npy` of `size` zero bytes, as numpy writes it (128-byte header for these shapes).
fn segment(root: &Path, form: &str, digest: &str, size: u64, mtime: f64) -> PathBuf {
    let descr = commonkit::Descr::parse("|u1").unwrap();
    let data = vec![0u8; size as usize];
    let file = commonkit::npy::encode_npy(&descr, &[size], commonkit::Order::C, &data).unwrap();
    let path = root
        .join(form)
        .join(&digest[..2])
        .join(format!("{digest}.x0.npy"));
    plain(path, mtime, &file)
}

struct Tree {
    segs: Vec<PathBuf>,
    tmp_old: PathBuf,
}

/// Python's `_tree`: ten segments a minute apart, the oldest two past a TTL of an hour;
/// orphaned and fresh `.tmp` files; bystanders; and a stale `.usage.json`.
fn tree(root: &Path, now: f64) -> Tree {
    fs::create_dir_all(root).unwrap();
    let segs = (0..10u64)
        .map(|i| {
            let digest = format!("{i:02x}{}", format!("{i:x}").repeat(30));
            let form = if i % 2 == 1 { "raw" } else { "img" };
            segment(
                root,
                form,
                &digest,
                1000 + i,
                now - 3700.0 + i as f64 * 60.0,
            )
        })
        .collect();
    let tmp_old = plain(root.join("raw/01/orphan.tmp"), now - 4000.0, b"x");
    plain(root.join("raw/01/writing.tmp"), now - 10.0, b"x");
    plain(root.join(".pinned/ab/x.npy"), now - 1e6, b"x");
    plain(root.join("notes.npy"), now - 1e6, b"x");
    plain(root.join("root.tmp"), now - 1e6, b"x");
    plain(root.join(".usage.json"), now - 1e6, b"{\"bytes\": 1}");
    Tree { segs, tmp_old }
}

/// `lstat`'s kind, size, mtime and atime (to the nanosecond the platform keeps).
type Snap = BTreeMap<PathBuf, (bool, bool, u64, SystemTime, Option<SystemTime>)>;

/// Every entry under `root` and `root` itself, by `lstat`. A directory's mtime changes when an
/// entry comes or goes in it, so this also catches a lock or temporary file that came and went.
/// A directory's atime is left out: any walk reads directories, this snapshot's own included,
/// and APFS then updates it (relatime).
fn snapshot(root: &Path) -> Snap {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let m = fs::symlink_metadata(&p).unwrap();
        let atime = (!m.is_dir()).then(|| m.accessed().unwrap());
        out.insert(
            p.strip_prefix(root).unwrap().to_path_buf(),
            (
                m.is_dir(),
                m.file_type().is_symlink(),
                m.len(),
                m.modified().unwrap(),
                atime,
            ),
        );
        if m.is_dir() {
            for e in fs::read_dir(&p).unwrap() {
                stack.push(e.unwrap().path());
            }
        }
    }
    out
}

fn options() -> Options {
    Options {
        mode: Mode::Read,
        max_bytes: 6000,
        min_free_bytes: 0,
        ttl_s: 3600.0,
        ..Options::default()
    }
}

fn dry(now: Option<f64>) -> ReapOptions {
    ReapOptions {
        budget: BUDGET,
        dry_run: true,
        now,
    }
}

#[test]
fn a_dry_run_changes_nothing_and_plans_what_a_reap_does() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    let t = tree(&root, NOW);
    let before = snapshot(&root);

    let plan = reap_plan(&root, &options(), &dry(Some(NOW)));
    assert_eq!(snapshot(&root), before);
    let store = Store::new(&root, options()); // its probe comes and goes
    let before_store = snapshot(&root);
    let via_store = store.reap_with(&dry(Some(NOW)));
    assert_eq!(snapshot(&root), before_store);
    assert!(store.drain_stats().is_empty());

    assert!(plan.dry_run && plan.complete);
    assert_eq!(plan.scanned, 10);
    assert_eq!(plan.sweep, vec![t.tmp_old.clone()]);
    // Expired: the first two (3700 and 3640 s old). Over: ten of ~1.1 kB against a low water
    // mark of 4800 bytes leaves four.
    assert_eq!(plan.unlink, t.segs[..6].to_vec());
    assert_eq!(plan.reaped, 6);
    let bytes: u64 = t.segs[..6]
        .iter()
        .map(|p| fs::metadata(p).unwrap().len())
        .sum();
    assert_eq!(plan.bytes, bytes);
    assert_eq!((plan.low_water, plan.need_free), (4800, 0));
    assert_eq!(
        (&via_store.unlink, &via_store.sweep, via_store.reaped),
        (&plan.unlink, &plan.sweep, plan.reaped)
    );
}

#[test]
fn a_dry_run_matches_a_real_reap() {
    // A real reap reads the clock, so this tree is built in today's terms.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    tree(&root, now);
    let plan = reap_plan(&root, &options(), &dry(None));
    let before: BTreeSet<_> = snapshot(&root).into_keys().collect();
    let out = Store::new(&root, options()).reap(BUDGET);
    let after: BTreeSet<_> = snapshot(&root).into_keys().collect();
    let gone: BTreeSet<_> = before.difference(&after).cloned().collect();
    let want: BTreeSet<_> = plan
        .unlink
        .iter()
        .chain(&plan.sweep)
        .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
        .collect();
    assert_eq!(gone, want);
    assert_eq!((out.reaped, out.bytes), (plan.reaped, plan.bytes));
}

#[test]
fn a_dry_run_takes_no_lock_and_ignores_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    tree(&root, NOW);
    let lock = plain(root.join(".reap.lock"), NOW - 1e6, b"1"); // stale, too
    let before = snapshot(&root);
    assert_eq!(reap_plan(&root, &options(), &dry(Some(NOW))).reaped, 6);
    assert_eq!(snapshot(&root), before);
    assert!(lock.exists());
}

#[cfg(unix)]
#[test]
fn a_dry_run_refuses_what_a_reap_refuses() {
    for root in ["", "/", "/tmp"] {
        let plan = reap_plan(root, &options(), &dry(Some(NOW)));
        assert_eq!(plan, commonkit::ReapReport::default(), "{root:?}");
    }
}

#[test]
fn the_clock_is_fixed_only_for_a_dry_run() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    let seg = segment(&root, "raw", &"ab".repeat(16), 100, now);
    let store = Store::new(
        &root,
        Options {
            mode: Mode::ReadWrite,
            max_bytes: 1 << 30,
            min_free_bytes: 0,
            ttl_s: 60.0,
            ..Options::default()
        },
    );
    let later = Some(now + 3600.0);
    assert_eq!(store.reap_with(&dry(later)).unlink, vec![seg.clone()]);
    let real = ReapOptions {
        dry_run: false,
        ..dry(later)
    };
    assert_eq!(store.reap_with(&real).reaped, 0);
    assert!(seg.exists());
}

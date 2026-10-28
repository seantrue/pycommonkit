//! Rust ports of Python's `tests/test_store.py` (each test keeps the Python name), the touch
//! tests of contract §8.2, and what only Rust has: typed views, the element probe, the mapping
//! surviving unlink and rename-over (§7), and the reap-turn hook.
//!
//! The store is addressed through an audio scheme, as in Python, to prove it is generic. The
//! scheme registry is process-global, so this file's domain (`ckaudio`) is its own.
//!
//! The reaper (contract §9, step 7) is tested here too: Python's reaper tests keep their names,
//! and the refusal, bystander, lock, concurrency and mapping tests are Rust's own.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use common::{Arr, age, mtime, tmp_files};
use commonkit::npy::{Descr, Field, NpyError, Order};
use commonkit::store::Hooks;
use commonkit::{
    ArrayRef, AttachMode, Element, KeyScheme, Mode, Options, Segment, Store, Variants, keys,
};

const D1: &str = "abababababababababababababababab";
const D2: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// `wav16k` must be int16 and `f32chw` float32; `raw` takes any dtype.
fn wav() -> Arc<KeyScheme> {
    static SCHEME: OnceLock<Arc<KeyScheme>> = OnceLock::new();
    SCHEME
        .get_or_init(|| {
            keys::register(
                KeyScheme::new(
                    "ckaudio",
                    "v1",
                    ["wav16k", "f32chw", "raw"],
                    Variants::pattern(r"^sr\d+$").unwrap(),
                    &[("wav16k", "<i2"), ("f32chw", "<f4")],
                )
                .unwrap(),
            )
            .unwrap()
        })
        .clone()
}

fn key_in(digest: &str, form: &str) -> String {
    wav()
        .key(digest, "sr16000", Some(form))
        .unwrap()
        .as_str()
        .to_owned()
}

fn key() -> String {
    key_in(D1, "wav16k")
}

fn wave(n: usize) -> Arr {
    let data = (0..n)
        .flat_map(|i| ((i % 3001) as i16).to_le_bytes())
        .collect();
    Arr::new("<i2", &[n as u64], Order::C, data)
}

fn zeros(descr: &str, shape: &[u64], order: Order) -> Arr {
    let d = Descr::parse(descr).unwrap();
    let n = d.itemsize().unwrap() * shape.iter().product::<u64>();
    Arr::new(descr, shape, order, vec![0; n as usize])
}

fn options() -> Options {
    Options {
        mode: Mode::ReadWrite,
        max_bytes: 64 << 20,
        min_free_bytes: 1,
        min_array_bytes: 1024,
        max_array_bytes: 8 << 20,
        ttl_s: 900.0,
        reap_interval_s: 0.0,
        ..Options::default()
    }
}

/// A store under a fresh temporary directory, as Python's `store` fixture.
fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(dir.path().join("seg"), options());
    assert!(!s.is_disabled());
    (dir, s)
}

fn store_with(o: Options, hooks: Hooks) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(dir.path().join("seg"), o).with_hooks(hooks);
    (dir, s)
}

fn stats(s: &Store) -> Vec<(&'static str, u64)> {
    s.drain_stats().iter().collect()
}

#[test]
fn test_publish_then_attach_returns_the_same_samples() {
    let (_d, s) = store();
    let a = wave(200_000);
    assert_eq!(s.publish(&key(), a.r()), Ok(true));
    let b = s.attach(&key()).unwrap();
    assert_eq!(b.bytes(), a.data.as_slice());
    assert_eq!(b.descr(), &a.descr);
    assert_eq!(b.shape(), [200_000]);
    assert_eq!(
        b.as_slice::<i16>().unwrap().get(3001..3003),
        Some(&[0, 1][..])
    );
}

/// numpy enforces read-only on a mapped array; Rust enforces it by type: a `Segment` lends
/// only `&[u8]` and `&[T]`, and the mapping is `memmap2::Mmap`, never `MmapMut`.
#[test]
fn test_attached_arrays_are_read_only_and_stay_that_way() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let b = s.attach(&key()).unwrap();
    assert_eq!(b.is_mapped(), cfg!(feature = "mmap"));
    let view: &[i16] = b.as_slice().unwrap();
    assert_eq!(view.len(), 200_000);
}

#[test]
fn test_a_miss_is_none_not_an_exception() {
    let (_d, s) = store();
    assert!(s.attach(&key_in(D2, "wav16k")).is_none());
    assert_eq!(stats(&s), [("shm_miss", 1)]);
}

#[test]
fn test_the_path_stays_under_the_root() {
    let (_d, s) = store();
    let p = s.path_for(&key()).unwrap();
    assert!(p.starts_with(s.root()));
    assert_eq!(p.strip_prefix(s.root()).unwrap().components().count(), 3);
}

/// Python's reader checks the registry and counts an unregistered domain as a bad key. The
/// contract lets a reader take any grammatical key (C4, decided: grammar on read), and Rust
/// does: an unregistered domain is an ordinary miss. A key that fails the grammar is counted.
#[test]
fn test_an_unregistered_domain_is_a_miss_not_a_crash() {
    let (_d, s) = store();
    assert!(s.attach(&format!("nobody:v1:pil:{D1}:o1")).is_none());
    assert_eq!(stats(&s), [("shm_miss", 1)]);
    for bad in [
        "nobody",
        &format!("nobody:v1:pil:{D1}:o1\n"),
        &format!("../../x:v1:pil:{D1}:o1"),
        &format!("ckaudio:v1:wav16k:{}:sr1", D1.to_uppercase()),
    ] {
        assert!(s.attach(bad).is_none(), "{bad:?}");
        assert!(s.path_for(bad).is_none(), "{bad:?}");
    }
    assert_eq!(stats(&s), [("shm_bad_key", 4)]);
}

// ------------------------------------------------- dtype policy is the scheme's

#[test]
fn test_a_form_with_a_declared_dtype_refuses_anything_else() {
    let (_d, s) = store();
    assert_eq!(
        s.publish(&key(), zeros("|b1", &[200_000], Order::C).r()),
        Ok(false)
    );
    assert_eq!(stats(&s), [("shm_declined_dtype", 1)]);
}

#[test]
fn test_each_form_enforces_its_own_declared_dtype() {
    let (_d, s) = store();
    let tensor = zeros("<f4", &[3, 256, 256], Order::C);
    assert_eq!(
        s.publish(&key_in(D1, "wav16k"), wave(200_000).r()),
        Ok(true)
    );
    assert_eq!(s.publish(&key_in(D1, "f32chw"), tensor.r()), Ok(true));
    let plane = zeros("<f4", &[256, 256], Order::C);
    assert_eq!(s.publish(&key_in(D2, "wav16k"), plane.r()), Ok(false));
    let f8 = zeros("<f8", &[3, 256, 256], Order::C);
    assert_eq!(s.publish(&key_in(D2, "f32chw"), f8.r()), Ok(false));
    assert_eq!(s.drain_stats().get("shm_declined_dtype"), 2);
}

#[test]
fn test_a_form_with_no_declared_dtype_is_unconstrained() {
    for (i, descr) in ["<f4", "<f8", "|u1", "<i2"].iter().enumerate() {
        let (_d, s) = store();
        let a = zeros(descr, &[3, 256, 256], Order::C);
        let k = key_in(&format!("{i:032x}"), "raw");
        assert_eq!(s.publish(&k, a.r()), Ok(true), "{descr}");
        assert_eq!(s.attach(&k).unwrap().descr(), &a.descr, "{descr}");
    }
}

#[test]
fn test_fortran_order_and_dtype_survive() {
    let (_d, s) = store();
    let a = zeros("<f4", &[50, 600], Order::F);
    assert_eq!(s.publish(&key_in(D1, "f32chw"), a.r()), Ok(true));
    let b = s.attach(&key_in(D1, "f32chw")).unwrap();
    assert_eq!(b.descr_text(), "'<f4'");
    assert!(b.fortran_order());
}

// ------------------------------------------------------------------ admission

#[test]
fn test_declines_below_the_minimum_size() {
    let (_d, s) = store();
    assert_eq!(s.publish(&key(), wave(16).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_small", 1)]);
}

#[test]
fn test_declines_above_the_per_item_ceiling() {
    let (_d, s) = store();
    assert_eq!(
        s.publish(&key(), zeros("<i2", &[9 << 20], Order::C).r()),
        Ok(false)
    );
    assert_eq!(stats(&s), [("shm_declined_size", 1)]);
}

#[test]
fn test_declines_when_the_filesystem_is_near_full_and_never_blocks() {
    let hooks = Hooks {
        free_bytes: Some(Arc::new(|_: &Path| Some(4096))),
        ..Hooks::default()
    };
    let (_d, s) = store_with(
        Options {
            min_free_bytes: 8 << 30,
            ..options()
        },
        hooks,
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_space", 1)]);
}

#[test]
fn test_read_mode_attaches_but_never_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let rw = Store::new(dir.path().join("s"), options());
    assert_eq!(rw.publish(&key(), wave(200_000).r()), Ok(true));
    let ro = Store::new(
        dir.path().join("s"),
        Options {
            mode: Mode::Read,
            ..options()
        },
    );
    assert!(ro.attach(&key()).is_some());
    assert_eq!(
        ro.publish(&key_in(D2, "wav16k"), wave(200_000).r()),
        Ok(false)
    );
    assert_eq!(ro.drain_stats().get("shm_declined_mode"), 1);
}

#[test]
fn test_mode_off_is_entirely_inert() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(
        dir.path().join("s"),
        Options {
            mode: Mode::Off,
            ..options()
        },
    );
    assert!(s.is_disabled());
    assert!(s.attach(&key()).is_none());
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert!(
        !dir.path().join("s").exists(),
        "an off store creates nothing"
    );
    assert_eq!(stats(&s), [("shm_declined_mode", 1)]);
}

#[test]
fn test_an_unusable_root_degrades_instead_of_raising() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let mut roots = vec![file.join("seg"), PathBuf::from("/"), PathBuf::from("")];
    if cfg!(unix) {
        roots.push(PathBuf::from("/proc/nonexistent/segments"));
    }
    for root in roots {
        let s = Store::new(&root, options());
        assert!(s.is_disabled(), "{}", root.display());
        assert!(s.attach(&key()).is_none());
        assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
        assert!(s.usage().is_none());
        assert_eq!(stats(&s), [("shm_declined_mode", 1)], "{}", root.display());
    }
}

// ------------------------------------------------------------------ integrity

/// The reaper's whole licence: it unlinks without knowing who is attached, because a live
/// mapping survives the name going away (contract §7).
#[test]
fn test_unlink_while_mapped_keeps_a_live_reader_working() {
    let (_d, s) = store();
    let a = wave(200_000);
    s.publish(&key(), a.r()).unwrap();
    let b = s.attach(&key()).unwrap();
    std::fs::remove_file(s.path_for(&key()).unwrap()).unwrap();
    assert_eq!(b.bytes(), a.data.as_slice());
    assert_eq!(b.element(&[3000]), Some(Element::Int(3000)));
    assert!(s.attach(&key()).is_none());
}

/// A rename over a mapped segment (a publisher replacing it, contract §6.5) leaves the mapping
/// on the old inode: the reader keeps the bytes it attached, and a new attach gets the new file.
#[test]
fn a_mapping_survives_its_file_being_replaced_by_rename() {
    let (_d, s) = store();
    let a = wave(200_000);
    s.publish(&key(), a.r()).unwrap();
    let held = s.attach(&key()).unwrap();
    let path = s.path_for(&key()).unwrap();
    let other = zeros("<i2", &[300_000], Order::C);
    let tmp = path.with_extension("npy.test.tmp");
    std::fs::write(
        &tmp,
        commonkit::npy::encode_npy(&other.descr, &other.shape, Order::C, &other.data).unwrap(),
    )
    .unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    assert_eq!(held.bytes(), a.data.as_slice());
    assert_eq!(held.shape(), [200_000]);
    let now = s.attach(&key()).unwrap();
    assert_eq!(now.shape(), [300_000]);
    // And the held mapping still reads after the replacement is unlinked too.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(held.as_slice::<i16>().unwrap().get(3001), Some(&0));
}

#[test]
fn test_a_truncated_segment_is_a_miss_not_a_crash() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    // Test-only: the file is not mapped (contract §7).
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(s.path_for(&key()).unwrap())
        .unwrap();
    f.set_len(64).unwrap();
    drop(f);
    s.drain_stats();
    assert!(s.attach(&key()).is_none());
    assert_eq!(stats(&s), [("shm_torn", 1)]);
}

/// K workers starting together on one root must all come up enabled, and leave no probe.
#[test]
fn test_concurrent_construction_never_disables_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("race");
    let disabled: Vec<bool> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let root = root.clone();
                scope.spawn(move || {
                    Store::new(
                        &root,
                        Options {
                            mode: Mode::ReadWrite,
                            min_free_bytes: 1,
                            min_array_bytes: 1,
                            ..Options::default()
                        },
                    )
                    .is_disabled()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(disabled, [false; 16]);
    let probes: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".writable"))
        .collect();
    assert!(probes.is_empty());
}

// ------------------------------------------------------------- reap turns (§9.8)

fn turn_counter() -> (Arc<AtomicUsize>, Hooks) {
    let turns = Arc::new(AtomicUsize::new(0));
    let t = Arc::clone(&turns);
    let hooks = Hooks {
        reap_turn: Some(Arc::new(move || {
            t.fetch_add(1, Ordering::SeqCst);
        })),
        ..Hooks::default()
    };
    (turns, hooks)
}

/// The reaper used to run only after a successful publish: a full store declined forever and was
/// never reaped again. A space decline takes a reap turn, and the turn reaps.
#[test]
fn test_a_declined_publish_still_reaps() {
    let (_d, s) = store();
    let ks = keys_n(3);
    // Publish all, then age: with reap_interval_s = 0 each publish reaps, so aging inside the
    // loop (as Python's test does) has the later publishes reap the earlier segments.
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    for k in &ks {
        age(&s.path_for(k).unwrap(), 2.0 * s.options().ttl_s);
    }
    let seg = file_size(&s, &ks[0]);
    s.drain_stats();
    let (turns, mut hooks) = turn_counter();
    hooks.free_bytes = Some(Arc::new(|_: &Path| Some(4096)));
    let full = again(
        &s,
        Options {
            min_free_bytes: 1 << 30,
            ..options()
        },
    )
    .with_hooks(hooks);
    let k = key_in(&"ef".repeat(16), "wav16k");
    assert_eq!(full.publish(&k, wave(200_000).r()), Ok(false));
    assert_eq!(
        stats(&full),
        [
            ("shm_declined_space", 1),
            ("shm_reaped", 3),
            ("shm_reaped_bytes", 3 * seg)
        ]
    );
    assert_eq!(turns.load(Ordering::SeqCst), 1);
}

#[test]
fn a_budget_decline_and_a_publish_take_reap_turns_but_a_republish_does_not() {
    let (turns, hooks) = turn_counter();
    let (_d, s) = store_with(options(), hooks);
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    assert_eq!(turns.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.publish(&key(), wave(200_000).r()),
        Ok(true),
        "a republish"
    );
    assert_eq!(turns.load(Ordering::SeqCst), 1, "a republish takes no turn");
    let marker = s.root().join(".usage.json");
    std::fs::write(
        &marker,
        format!("{{\"bytes\": {}, \"files\": 1}}", 64 << 20),
    )
    .unwrap();
    // With reap_interval_s = 0 a usage file is fresh only while its mtime is not in the past.
    age(&marker, -3600.0);
    assert_eq!(
        s.publish(&key_in(D2, "wav16k"), wave(200_000).r()),
        Ok(false)
    );
    assert_eq!(s.drain_stats().get("shm_declined_budget"), 1);
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    // Not on a small, size, dtype or key decline.
    assert_eq!(s.publish(&key_in(D2, "wav16k"), wave(16).r()), Ok(false));
    assert_eq!(turns.load(Ordering::SeqCst), 2);
}

#[test]
fn reap_turns_are_rate_limited() {
    let (turns, hooks) = turn_counter();
    let (_d, s) = store_with(
        Options {
            reap_interval_s: 3600.0,
            ..options()
        },
        hooks,
    );
    for i in 0..3 {
        let k = key_in(&format!("{i:032x}"), "wav16k");
        assert_eq!(s.publish(&k, wave(200_000).r()), Ok(true));
    }
    assert_eq!(
        turns.load(Ordering::SeqCst),
        1,
        "the first turn, then none for an hour"
    );
}

// ------------------------------------------------- the on-disk contract (step 3)

fn raw_npy(header: &str, payload: &[u8]) -> Vec<u8> {
    let text = header.as_bytes();
    let pad = 64 - ((10 + text.len() + 1) % 64);
    let hlen = (text.len() + pad + 1) as u16;
    let mut v = b"\x93NUMPY\x01\x00".to_vec();
    v.extend_from_slice(&hlen.to_le_bytes());
    v.extend_from_slice(text);
    v.resize(v.len() + pad, b' ');
    v.push(b'\n');
    v.extend_from_slice(payload);
    v
}

#[test]
fn test_a_garbled_header_is_a_miss_not_an_exception() {
    for attach in [AttachMode::Mmap, AttachMode::Read] {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(
            dir.path().join("seg"),
            Options {
                attach,
                ..Options::default()
            },
        );
        let path = s.path_for(&key()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            raw_npy("{'descr': '<i4', 'fortran_order': Fals", &[0; 40]),
        )
        .unwrap();
        assert!(s.attach(&key()).is_none());
        assert_eq!(stats(&s), [("shm_torn", 1)], "{attach:?}");
    }
}

/// Python declines an object array at the dtype gate. In Rust one can't be built: `Descr`
/// refuses object dtypes, so no `ArrayRef` of one reaches `publish` and nothing is written.
#[test]
fn test_an_object_array_is_declined_and_leaves_no_temporary_file() {
    for d in ["|O", "[('a', '<i4'), ('b', '|O')]"] {
        assert_eq!(Descr::parse(d), Err(NpyError::ObjectDtype), "{d}");
    }
}

#[test]
fn test_a_header_over_the_limit_is_declined() {
    let (_d, s) = store();
    let i4 = Descr::parse("<i4").unwrap();
    let wide = Descr::structured(
        (0..70_000)
            .map(|i| Field::new(format!("f{i:06}"), i4.clone(), vec![]))
            .collect(),
    )
    .unwrap();
    let data = vec![0u8; 4 * 70_000 * 4];
    let a = ArrayRef::new(&wide, &[4], Order::C, &data).unwrap();
    assert_eq!(s.publish(&key_in(D1, "raw"), a), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_dtype", 1)]);
    assert!(tmp_files(s.root()).is_empty());
}

/// A republish used to rewrite the segment and count a second publish (C15).
#[test]
fn test_republishing_the_same_array_touches_and_writes_nothing() {
    let (_d, s) = store();
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    let path = s.path_for(&key()).unwrap();
    age(&path, 3600.0);
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).unwrap());
    s.drain_stats();
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).unwrap()),
        inode,
        "renamed over: the segment was rewritten"
    );
    assert!(
        SystemTime::now().duration_since(mtime(&path)).unwrap() < Duration::from_secs(60),
        "a republish is a touch"
    );
    assert_eq!(stats(&s), []);
}

/// Captures log records, to check the collision is logged at error level.
struct Capture;

static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        RECORDS
            .lock()
            .unwrap()
            .push((record.level(), record.args().to_string()));
    }
    fn flush(&self) {}
}

#[test]
fn test_a_different_array_under_an_existing_key_raises() {
    static LOGGER: Capture = Capture;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);

    let (_d, s) = store();
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    let path = s.path_for(&key()).unwrap();
    let then = age(&path, 3600.0);
    let before = std::fs::read(&path).unwrap();
    s.drain_stats();
    let err = s.publish(&key(), wave(150_000).r()).unwrap_err();
    assert_eq!(err.key, key());
    assert_eq!(err.path, path);
    assert_eq!(err.reason, "a different dtype, shape or memory order");
    let _: &dyn std::error::Error = &err;
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(mtime(&path), then);
    assert_eq!(stats(&s), [("shm_key_collision", 1)]);
    let mine: Vec<_> = RECORDS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, m)| m.contains(path.to_str().unwrap()))
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(mine, [log::Level::Error]);
}

#[test]
fn test_a_torn_segment_is_replaced_and_counted() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(s.path_for(&key()).unwrap())
        .unwrap();
    f.set_len(64).unwrap(); // test-only: unmapped (contract §7)
    drop(f);
    s.drain_stats();
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    assert_eq!(s.drain_stats().get("shm_torn_replaced"), 1);
    assert_eq!(
        s.attach(&key()).unwrap().bytes(),
        wave(200_000).data.as_slice()
    );
}

type Calls = Arc<Mutex<Vec<(&'static str, PathBuf, bool)>>>;

/// Hooks that record each sync: which one, its path, and whether the segment exists yet.
fn recording(segment: PathBuf) -> (Calls, Hooks) {
    let calls: Calls = Arc::default();
    let (c1, c2) = (Arc::clone(&calls), Arc::clone(&calls));
    let (s1, s2) = (segment.clone(), segment);
    let hooks = Hooks {
        sync_file: Some(Arc::new(move |p: &Path| {
            c1.lock().unwrap().push(("file", p.to_owned(), s1.exists()));
            Ok(())
        })),
        sync_dir: Some(Arc::new(move |p: &Path| {
            c2.lock().unwrap().push(("dir", p.to_owned(), s2.exists()));
            Ok(())
        })),
        ..Hooks::default()
    };
    (calls, hooks)
}

#[test]
fn test_fsync_is_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Store::new(dir.path().join("seg"), options());
    let (calls, hooks) = recording(probe.path_for(&key()).unwrap());
    let s = Store::new(dir.path().join("seg"), options()).with_hooks(hooks);
    assert!(!s.options().fsync);
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    assert!(calls.lock().unwrap().is_empty());
}

/// Contract §6.5 steps 3 and 5 (C17), in that order: the temporary file before the rename,
/// the segment's directory after it.
#[test]
fn test_fsync_syncs_the_file_before_the_rename_and_the_directory_after() {
    let dir = tempfile::tempdir().unwrap();
    let o = Options {
        fsync: true,
        ..options()
    };
    let path = Store::new(dir.path().join("seg"), o.clone())
        .path_for(&key())
        .unwrap();
    let (calls, hooks) = recording(path.clone());
    let s = Store::new(dir.path().join("seg"), o).with_hooks(hooks);
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    let calls = calls.lock().unwrap();
    let parent = path.parent().unwrap().to_owned();
    let (kind, tmp, exists) = calls[0].clone();
    assert_eq!((kind, exists), ("file", false));
    assert!(tmp.to_string_lossy().ends_with(".tmp") && tmp.parent() == Some(parent.as_path()));
    assert_eq!(calls[1], ("dir", parent, true));
    assert_eq!(calls.len(), 2);
}

/// With no hooks the real syncs run: `F_FULLFSYNC` on macOS, `fsync` elsewhere, and the
/// directory on POSIX.
#[test]
fn a_real_fsync_publish_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(
        dir.path().join("seg"),
        Options {
            fsync: true,
            ..options()
        },
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    assert_eq!(s.drain_stats().get("shm_publish"), 1);
}

#[test]
fn test_a_failed_directory_sync_fails_the_publish_but_keeps_the_segment() {
    let hooks = Hooks {
        sync_dir: Some(Arc::new(|_: &Path| Err(std::io::Error::other("I/O error")))),
        ..Hooks::default()
    };
    let (_d, s) = store_with(
        Options {
            fsync: true,
            ..options()
        },
        hooks,
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_publish_failed", 1)]);
    assert_eq!(
        s.attach(&key()).unwrap().bytes(),
        wave(200_000).data.as_slice()
    );
}

#[test]
fn a_failed_file_sync_fails_the_publish_and_leaves_nothing() {
    let hooks = Hooks {
        sync_file: Some(Arc::new(|_: &Path| Err(std::io::Error::other("I/O error")))),
        ..Hooks::default()
    };
    let (_d, s) = store_with(
        Options {
            fsync: true,
            ..options()
        },
        hooks,
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_publish_failed", 1)]);
    assert!(!s.path_for(&key()).unwrap().exists());
    assert!(tmp_files(s.root()).is_empty());
}

#[test]
fn a_full_volume_while_writing_is_a_space_decline() {
    let hooks = Hooks {
        sync_file: Some(Arc::new(|_: &Path| {
            Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
        })),
        ..Hooks::default()
    };
    let (_d, s) = store_with(
        Options {
            fsync: true,
            ..options()
        },
        hooks,
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_space", 1)]);
    assert!(tmp_files(s.root()).is_empty());
}

/// A platform with no `statvfs` raised out of Python's publish (§9.9, A10); unknown is 0 free.
#[test]
fn test_free_space_is_zero_without_statvfs() {
    let hooks = Hooks {
        free_bytes: Some(Arc::new(|_: &Path| None)),
        ..Hooks::default()
    };
    let (_d, s) = store_with(options(), hooks);
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_space", 1)]);
}

#[test]
fn test_usage_bytes_must_be_a_non_negative_json_integer() {
    // Python's fixture has reap_interval_s = 0, under which every usage file is stale, so its
    // version of this test can't tell invalid from stale. A 30 s interval keeps the file fresh.
    let (_d, s) = store_with(
        Options {
            reap_interval_s: 30.0,
            ..options()
        },
        Hooks::default(),
    );
    for used in ["\"995\"", "995.0", "-1", "true", "null", "1e3"] {
        std::fs::write(
            s.root().join(".usage.json"),
            format!("{{\"bytes\": {used}, \"files\": 3}}"),
        )
        .unwrap();
        assert_eq!(s.usage(), None, "{used}");
    }
    std::fs::write(
        s.root().join(".usage.json"),
        "{\"bytes\": 995, \"files\": 3}",
    )
    .unwrap();
    assert_eq!(
        s.usage(),
        Some(commonkit::Usage {
            bytes: 995,
            files: Some(3)
        })
    );
}

#[test]
fn test_usage_survives_a_hostile_file() {
    let (_d, s) = store();
    std::fs::write(s.root().join(".usage.json"), "[".repeat(100_000)).unwrap();
    assert_eq!(s.usage(), None);
    std::fs::write(s.root().join(".usage.json"), vec![b' '; 70_000]).unwrap();
    assert_eq!(s.usage(), None, "over 64 KiB");
    std::fs::remove_file(s.root().join(".usage.json")).unwrap();
    std::fs::create_dir(s.root().join(".usage.json")).unwrap();
    assert_eq!(s.usage(), None, "a directory");
}

#[test]
fn a_stale_usage_file_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(
        dir.path(),
        Options {
            reap_interval_s: 30.0,
            ..options()
        },
    );
    let marker = s.root().join(".usage.json");
    std::fs::write(&marker, "{\"bytes\": 1}").unwrap();
    assert!(s.usage().is_some());
    age(&marker, 61.0);
    assert_eq!(s.usage(), None, "older than twice the reap interval");
}

// ------------------------------------------------------------------- touch (§8.2)

#[test]
fn a_hit_touches_a_segment_older_than_a_minute() {
    for attach in [AttachMode::Mmap, AttachMode::Read] {
        let (_d, s) = store_with(
            Options {
                attach,
                ..options()
            },
            Hooks::default(),
        );
        s.publish(&key(), wave(200_000).r()).unwrap();
        let path = s.path_for(&key()).unwrap();
        age(&path, 3600.0);
        assert!(s.attach(&key()).is_some());
        let since = SystemTime::now().duration_since(mtime(&path)).unwrap();
        assert!(since < Duration::from_secs(60), "{attach:?}: not touched");
    }
}

#[test]
fn a_hit_leaves_a_recently_touched_segment_alone() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let path = s.path_for(&key()).unwrap();
    let then = age(&path, 30.0);
    assert!(s.attach(&key()).is_some());
    assert_eq!(mtime(&path), then, "at most one touch a minute");
}

#[test]
fn a_republish_touches_only_an_old_segment() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let path = s.path_for(&key()).unwrap();
    let then = age(&path, 30.0);
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(true));
    assert_eq!(mtime(&path), then);
}

// ------------------------------------------------------------- typed access (Rust)

#[test]
fn typed_views_need_the_dtype_and_native_order() {
    let (_d, s) = store();
    let a = wave(200_000);
    s.publish(&key(), a.r()).unwrap();
    let seg = s.attach(&key()).unwrap();
    assert!(seg.as_slice::<i16>().is_some());
    assert!(seg.as_slice::<u16>().is_none(), "another dtype");
    assert!(seg.as_slice::<f32>().is_none());
    assert_eq!(seg.len(), 200_000);
    assert_eq!(seg.itemsize(), 2);
    let f = seg.to_f32_vec().unwrap();
    assert_eq!((f[0], f[3000], f[3001]), (0.0, 3000.0, 0.0));
    assert_eq!(seg.to_f64_vec().unwrap().len(), 200_000);
    assert_eq!(seg.element(&[200_000]), None);
    assert_eq!(seg.element(&[1, 2]), None);

    // Big-endian: no zero-copy view, but the probe and the conversions read it.
    let be: Vec<u8> = (0..1000u32)
        .flat_map(|v| (v as f32).to_be_bytes())
        .collect();
    let big = Arr::new(">f4", &[10, 100], Order::C, be);
    let k = key_in(D2, "raw");
    s.publish(&k, big.r()).unwrap();
    let seg = s.attach(&k).unwrap();
    assert!(seg.as_slice::<f32>().is_none());
    assert_eq!(seg.element(&[3, 7]), Some(Element::F32(307.0)));
    assert_eq!(seg.to_f32_vec().unwrap()[999], 999.0);
    assert_eq!(seg.element(&[3, 7]).unwrap().to_string(), "307");
}

#[test]
fn the_element_probe_follows_fortran_order() {
    let (_d, s) = store();
    // (r, c) = 10 r + c for a (300, 400) array laid out in Fortran order.
    let (rows, cols) = (300u64, 400u64);
    let data: Vec<u8> = (0..cols)
        .flat_map(|c| (0..rows).map(move |r| (10 * r + c) as f64))
        .flat_map(f64::to_le_bytes)
        .collect();
    let a = Arr::new("<f8", &[rows, cols], Order::F, data);
    let k = key_in(D2, "raw");
    s.publish(&k, a.r()).unwrap();
    let seg = s.attach(&k).unwrap();
    assert!(seg.fortran_order());
    assert_eq!(seg.element(&[7, 3]), Some(Element::F64(73.0)));
    assert_eq!(seg.element(&[299, 399]), Some(Element::F64(3389.0)));
    assert_eq!(seg.as_slice::<f64>().unwrap()[1], 10.0, "payload order");
}

#[test]
fn untyped_kinds_expose_bytes_and_descriptor_text() {
    let (_d, s) = store();
    let a = Arr::new(
        "<c8",
        &[40_000],
        Order::C,
        (0..320_000).map(|i| i as u8).collect(),
    );
    let k = key_in(D2, "raw");
    s.publish(&k, a.r()).unwrap();
    let seg = s.attach(&k).unwrap();
    assert_eq!(seg.dtype(), None);
    assert_eq!(seg.descr_text(), "'<c8'");
    assert_eq!(seg.bytes(), a.data.as_slice());
    assert_eq!(seg.element(&[0]), None);
    assert_eq!(seg.to_f32_vec(), None);
}

#[test]
fn segment_open_reads_a_file_outside_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let a = wave(1000);
    let p = dir.path().join("plain.npy");
    std::fs::write(
        &p,
        commonkit::npy::encode_npy(&a.descr, &a.shape, Order::C, &a.data).unwrap(),
    )
    .unwrap();
    for attach in [AttachMode::Mmap, AttachMode::Read] {
        let seg = Segment::open(&p, attach).unwrap();
        assert_eq!(seg.bytes(), a.data.as_slice());
        assert_eq!(seg.is_mapped(), attach.maps());
    }
    assert!(Segment::open(dir.path().join("absent.npy"), AttachMode::Read).is_none());
    assert!(
        Segment::open(dir.path(), AttachMode::Mmap).is_none(),
        "a directory"
    );
}

#[test]
fn verify_republish_compares_payloads() {
    let (_d, s) = store_with(
        Options {
            verify_republish: true,
            ..options()
        },
        Hooks::default(),
    );
    let a = wave(200_000);
    assert_eq!(s.publish(&key(), a.r()), Ok(true));
    assert_eq!(s.publish(&key(), a.r()), Ok(true), "the same bytes");
    let mut data = a.data.clone();
    data[399_999] ^= 1;
    let b = Arr::new("<i2", &[200_000], Order::C, data);
    let err = s.publish(&key(), b.r()).unwrap_err();
    assert_eq!(err.reason, "different named-field bytes");
}

#[test]
fn an_array_ref_must_match_its_header() {
    let d = Descr::parse("<f4").unwrap();
    assert!(matches!(
        ArrayRef::new(&d, &[3], Order::C, &[0; 11]),
        Err(NpyError::PayloadLength { need: 12, have: 11 })
    ));
    assert_eq!(
        ArrayRef::new(&d, &[u64::MAX, 2], Order::C, &[]).unwrap_err(),
        NpyError::Overflow
    );
    assert_eq!(
        ArrayRef::new(&d, &[1; 65], Order::C, &[0; 4]).unwrap_err(),
        NpyError::BadShape
    );
}

#[test]
fn a_store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
    assert_send_sync::<Segment>();
}

/// One store shared by threads publishing and attaching the same keys: every result is a
/// whole segment, and racing publishers of one absent key both succeed (contract §6.4).
#[test]
fn threads_share_one_store() {
    let (_d, s) = store();
    let a = wave(100_000);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for i in 0..4 {
                    let k = key_in(&format!("{i:032x}"), "wav16k");
                    assert_eq!(s.publish(&k, a.r()), Ok(true));
                    assert_eq!(s.attach(&k).unwrap().bytes(), a.data.as_slice());
                }
            });
        }
    });
    assert!(tmp_files(s.root()).is_empty());
}

// ------------------------------------------------------------------- the reaper (§9)
// (2026-10-16: none of the LRU eviction was tested in Python, and a store sat full for 5.7 days
// with 1,104 expired segments because nothing ever reaped it again.)

const BUDGET: Duration = Duration::from_secs(5);

fn keys_n(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| key_in(&format!("{i:032x}"), "wav16k"))
        .collect()
}

fn file_size(s: &Store, k: &str) -> u64 {
    std::fs::metadata(s.path_for(k).unwrap()).unwrap().len()
}

/// Another store on `s`'s tree with other settings: how Python's tests change `store.ttl_s` or
/// `store.max_bytes` after publishing, and how a second process sees the same tree.
fn again(s: &Store, o: Options) -> Store {
    let t = Store::new(s.root(), o);
    assert!(!t.is_disabled());
    t
}

fn present(s: &Store, ks: &[String]) -> Vec<bool> {
    ks.iter().map(|k| s.path_for(k).unwrap().exists()).collect()
}

/// Every path under `root` (files, links and directories), relative and sorted.
fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
#[test]
fn test_reaper_refuses_a_suspicious_root() {
    // `/tmp` has one separator: refused (contract §9.1). Python's test sets the root to `/`
    // after construction; that exact case is a unit test in src/store/reap.rs.
    let s = Store::new(
        "/tmp",
        Options {
            mode: Mode::ReadWrite,
            min_free_bytes: 1,
            ..Options::default()
        },
    );
    if !s.is_disabled() {
        assert_eq!(s.reap(BUDGET).reaped, 0);
        assert_eq!(stats(&s), [("shm_reap_refused", 1)]);
    }
}

#[test]
fn a_disabled_store_does_not_reap() {
    for s in [
        Store::new("", options()),
        Store::new(
            tempfile::tempdir().unwrap().path(),
            Options {
                mode: Mode::Off,
                ..options()
            },
        ),
    ] {
        assert!(s.is_disabled());
        assert_eq!(s.reap(BUDGET), commonkit::ReapReport::default());
        assert!(s.drain_stats().is_empty());
    }
}

#[test]
fn test_reaper_only_touches_its_own_segments() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let bystander = s.root().join("IMPORTANT.txt");
    std::fs::write(&bystander, "not a segment").unwrap();
    let r = again(
        &s,
        Options {
            ttl_s: -1.0,
            ..options()
        },
    );
    r.reap(BUDGET);
    assert!(bystander.exists());
    assert!(s.attach(&key()).is_none());
}

#[test]
fn test_reaper_leaves_fresh_segments_alone() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    assert_eq!(s.reap(BUDGET).reaped, 0);
    assert!(s.attach(&key()).is_some());
}

#[test]
fn test_over_budget_evicts_least_recently_used_first() {
    let (_d, s) = store();
    let ks = keys_n(6);
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap(); // 400 kB each
    }
    for (i, k) in ks.iter().enumerate() {
        age(&s.path_for(k).unwrap(), 600.0 - i as f64 * 60.0); // ks[0] oldest, within TTL
    }
    assert!(s.attach(&ks[0]).is_some(), "a read refreshes it (LRU)");
    let r = again(
        &s,
        Options {
            max_bytes: 3 * 400_000, // low water: 2 files (plus headers)
            ..options()
        },
    );
    let out = r.reap(BUDGET);
    assert!(out.complete);
    assert_eq!((out.scanned, out.reaped), (6, 4));
    assert_eq!(present(&s, &ks), [true, false, false, false, false, true]);
}

#[test]
fn test_expired_segments_are_reaped() {
    let (_d, s) = store();
    let ks = keys_n(3);
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    age(&s.path_for(&ks[1]).unwrap(), 2.0 * s.options().ttl_s);
    s.reap(BUDGET);
    assert_eq!(present(&s, &ks), [true, false, true]);
}

/// A volume smaller than max_bytes + min_free_bytes fills before the budget does; the floor used
/// to decline publishes without evicting anything.
#[test]
fn test_the_free_space_floor_evicts_least_recently_used() {
    let (_d, s) = store();
    let ks = keys_n(4);
    for (i, k) in ks.iter().enumerate() {
        s.publish(k, wave(200_000).r()).unwrap();
        age(&s.path_for(k).unwrap(), 600.0 - i as f64 * 60.0); // ks[0] oldest, within TTL
    }
    let seg = file_size(&s, &ks[0]);
    let r = again(
        &s,
        Options {
            max_bytes: 1 << 40,         // far under budget
            min_free_bytes: seg as i64, // floor + 25%: 1.25 segments
            ..options()
        },
    )
    .with_hooks(Hooks {
        free_bytes: Some(Arc::new(|_: &Path| Some(0))),
        ..Hooks::default()
    });
    let out = r.reap(BUDGET);
    assert_eq!(out.reaped, 2, "the two least recently used");
    assert_eq!(present(&s, &ks), [false, false, true, true]);
}

/// The time budget bounds the walk, not the eviction: a budget that cut eviction short once left
/// a store 1.2 GB over its low water mark. Here the first unlink takes longer than the budget.
#[test]
fn test_eviction_reaches_its_target_whatever_the_time_budget() {
    let (_d, s) = store();
    let ks = keys_n(6);
    for (i, k) in ks.iter().enumerate() {
        s.publish(k, wave(200_000).r()).unwrap();
        age(&s.path_for(k).unwrap(), 600.0 - i as f64 * 60.0);
    }
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let max_bytes = 3 * 400_000;
    let r = again(
        &s,
        Options {
            max_bytes,
            ..options()
        },
    )
    .with_hooks(Hooks {
        reap_unlink: Some(Arc::new(move |p: &Path| {
            if first.swap(false, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(300));
            }
            std::fs::remove_file(p)
        })),
        ..Hooks::default()
    });
    let out = r.reap(Duration::from_millis(100));
    assert!(out.complete);
    assert!(out.reaped >= 2);
    let used: u64 = ks
        .iter()
        .map(|k| s.path_for(k).unwrap())
        .filter(|p| p.exists())
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(used <= (0.8 * max_bytes as f64) as u64);
}

/// A walk cut short under-counts the tree: it must not record that total, nor evict for a
/// budget it can't judge. Here the walk is past its deadline from the first directory on.
#[test]
fn test_a_partial_walk_neither_evicts_for_budget_nor_writes_usage() {
    let (_d, s) = store();
    for k in keys_n(3) {
        s.publish(&k, wave(200_000).r()).unwrap();
    }
    let marker = s.root().join(".usage.json");
    let _ = std::fs::remove_file(&marker);
    let r = again(
        &s,
        Options {
            max_bytes: 1, // "over budget" if it were judged
            ..options()
        },
    )
    .with_hooks(Hooks {
        reap_dir: Some(Arc::new(|_: &Path| {
            std::thread::sleep(Duration::from_millis(100));
        })),
        ..Hooks::default()
    });
    let out = r.reap(Duration::from_millis(20));
    assert!(!out.complete);
    assert!(!marker.exists());
    assert_eq!(out.reaped, 0);
    assert_eq!(stats(&r), [("shm_reap_partial", 1)]);
}

/// A walk cut short after it found segments still serves expiry; it only stops judging the
/// byte budget (contract §9.6).
#[test]
fn a_partial_walk_still_reaps_what_it_found_expired() {
    let (_d, s) = store();
    let ks = [key_in(D1, "wav16k"), key_in(D2, "wav16k")];
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    for k in &ks {
        age(&s.path_for(k).unwrap(), 2.0 * s.options().ttl_s);
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let r = again(&s, options()).with_hooks(Hooks {
        // root, wav16k, ab: fast; the next shard directory is past the deadline.
        reap_dir: Some(Arc::new(move |_: &Path| {
            if c.fetch_add(1, Ordering::SeqCst) == 3 {
                std::thread::sleep(Duration::from_millis(100));
            }
        })),
        ..Hooks::default()
    });
    let out = r.reap(Duration::from_millis(50));
    assert!(!out.complete);
    assert_eq!((out.scanned, out.reaped), (1, 1));
    assert_eq!(present(&s, &ks).iter().filter(|p| **p).count(), 1);
}

#[test]
fn a_complete_walk_writes_usage_the_budget_gate_reads() {
    let (_d, s) = store();
    let ks = keys_n(3);
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    age(&s.path_for(&ks[0]).unwrap(), 2.0 * s.options().ttl_s);
    let seg = file_size(&s, &ks[1]);
    let out = s.reap(BUDGET);
    assert!(out.complete);
    assert_eq!((out.scanned, out.reaped, out.bytes), (3, 1, seg));
    let text = std::fs::read_to_string(s.root().join(".usage.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["bytes"].as_u64(), Some(2 * seg));
    assert_eq!(doc["files"].as_u64(), Some(2));
    assert!(doc["ts"].is_f64());
    // reap_interval_s = 0 makes every .usage.json stale to `s`; a reader with 30 s trusts it.
    let usage = again(
        &s,
        Options {
            reap_interval_s: 30.0,
            ..options()
        },
    )
    .usage()
    .unwrap();
    assert_eq!((usage.bytes, usage.files), (2 * seg, Some(2)));
    assert!(!s.root().join(".usage.json.tmp").exists());
    assert!(
        !s.root().join(".reap.lock").exists(),
        "the lock is released"
    );
}

/// The reaper writes its pid into the lock while it holds it, and releases it on every exit.
#[test]
fn the_lock_holds_the_pid_while_reaping() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let lock = s.root().join(".reap.lock");
    let (seen2, lock2) = (Arc::clone(&seen), lock.clone());
    let r = again(&s, options()).with_hooks(Hooks {
        reap_dir: Some(Arc::new(move |_: &Path| {
            seen2
                .lock()
                .unwrap()
                .push(std::fs::read_to_string(&lock2).ok());
        })),
        ..Hooks::default()
    });
    assert!(r.reap(BUDGET).complete);
    let pid = std::process::id().to_string();
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|t| t.as_deref() == Some(pid.as_str())));
    assert!(!lock.exists());
}

/// A held lock means no reap; a lock over 300 s old is broken, and that turn still doesn't
/// reap (contract §9.2, C8); the next one does.
#[test]
fn a_held_lock_is_respected_and_a_stale_one_broken() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let r = again(
        &s,
        Options {
            ttl_s: -1.0,
            ..options()
        },
    );
    let lock = s.root().join(".reap.lock");
    std::fs::write(&lock, "99999").unwrap();
    age(&lock, 200.0);
    assert_eq!(r.reap(BUDGET), commonkit::ReapReport::default());
    assert!(lock.exists(), "a lock 200 s old is held");
    assert!(s.path_for(&key()).unwrap().exists());

    age(&lock, 301.0);
    assert_eq!(r.reap(BUDGET), commonkit::ReapReport::default());
    assert!(!lock.exists(), "a lock over 300 s old is broken");
    assert!(
        s.path_for(&key()).unwrap().exists(),
        "and that turn doesn't reap"
    );
    assert!(r.drain_stats().is_empty());

    let out = r.reap(BUDGET);
    assert_eq!((out.reaped, out.complete), (1, true));
    assert!(!s.path_for(&key()).unwrap().exists());
}

/// Nothing but a regular file of the segment shape is ever unlinked: not non-segment files, not
/// reserved dot-names, not symbolic links or what they point at, not files outside the shape,
/// not a `.pinned/` subtree, and not a tree behind a symlinked directory.
#[test]
fn bystanders_are_left_alone() {
    let (d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    let seg = s.path_for(&key()).unwrap();
    let root = s.root().to_path_buf();
    let name = seg.file_name().unwrap().to_owned();
    let write = |rel: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, [0u8; 64]).unwrap();
        age(&p, 7200.0);
    };
    let others = [
        "IMPORTANT.txt",
        "notes.npy",
        "orphan.tmp",
        ".policy.json",
        ".hello/x.tmp",
        ".pinned/wav16k/ab/abababababababababababababababab.sr16000.npy",
        "wav16k/ab/readme.txt",
        "wav16k/ab/abababababababababababababababab.npy",
        "wav16k/ab/ABABABABABABABABABABABABABABABAB.sr16000.npy",
        "wav16k/cd/abababababababababababababababab.sr16000.npy",
        "wav16k/ab/x/abababababababababababababababab.sr16000.npy",
        "x/wav16k/ab/abababababababababababababababab.sr16000.npy",
        "Wav16k/ab/abababababababababababababababab.sr16000.npy",
        "wav16k/AB/abababababababababababababababab.sr16000.npy",
    ];
    for rel in others {
        write(rel);
    }
    // A segment-shaped directory where a file should be: not a regular file.
    std::fs::create_dir_all(root.join("wav16k/ab/abababababababababababababababac.sr1.npy"))
        .unwrap();
    // Outside the root: a segment-shaped tree, reached only through symbolic links.
    let outside = d.path().join("outside");
    let target = outside.join("ab").join(&name);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, [0u8; 64]).unwrap();
    age(&target, 7200.0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&outside, root.join("linked")).unwrap(); // a form-shaped link to a directory
        symlink(outside.join("ab"), root.join("wav16k/ef")).unwrap(); // a shard-shaped link
        symlink(
            &target,
            root.join("wav16k/ab/abababababababababababababababad.sr16000.npy"),
        )
        .unwrap();
        symlink(&target, root.join(".usage.json.tmp")).unwrap(); // never written through
    }
    let before = tree(&root);

    let r = again(
        &s,
        Options {
            ttl_s: -1.0,
            ..options()
        },
    );
    let out = r.reap(BUDGET);
    assert!(out.complete);
    assert_eq!((out.scanned, out.reaped), (1, 1));
    assert!(!seg.exists());
    assert_eq!(
        stats(&r),
        [("shm_reaped", 1), ("shm_reaped_bytes", out.bytes)]
    );
    // Gone: the segment, and the link planted at `.usage.json.tmp` (replaced, then renamed to
    // `.usage.json`). Everything else is as it was.
    let seg_rel = seg
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut expected: Vec<String> = before
        .into_iter()
        .filter(|p| *p != seg_rel && p != ".usage.json.tmp" && p != ".usage.json")
        .chain([".usage.json".to_owned()])
        .collect();
    expected.sort();
    assert_eq!(tree(&root), expected);
    assert_eq!(std::fs::read(&target).unwrap(), [0u8; 64], "never followed");
}

/// A reader's mapping outlives the reaper unlinking its file (contract §7): the reaper never
/// needs to know who is attached. (Without the `mmap` feature the reader holds a copy.)
#[test]
fn a_mapping_survives_the_reaper() {
    let (_d, s) = store();
    let a = wave(200_000);
    s.publish(&key(), a.r()).unwrap();
    let held = s.attach(&key()).unwrap();
    let r = again(
        &s,
        Options {
            ttl_s: -1.0,
            ..options()
        },
    );
    assert_eq!(r.reap(BUDGET).reaped, 1);
    assert!(!s.path_for(&key()).unwrap().exists());
    assert_eq!(held.bytes(), a.data.as_slice());
    assert_eq!(held.element(&[3000]), Some(Element::Int(3000)));
    assert!(s.attach(&key()).is_none());
}

/// A file already gone when the reaper unlinks it (another reaper got there, C8) is skipped:
/// not counted reaped, not a failure. Here a second reaper runs to completion inside the first
/// one's first unlink, as after a broken stale lock.
#[test]
fn unlinking_twice_is_harmless() {
    let (_d, s) = store();
    let ks = keys_n(4);
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    for (i, k) in ks.iter().enumerate() {
        if i % 2 == 0 {
            age(&s.path_for(k).unwrap(), 2.0 * s.options().ttl_s);
        }
    }
    let other = Arc::new(again(&s, options()));
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (o, lock) = (Arc::clone(&other), s.root().join(".reap.lock"));
    let r = again(&s, options()).with_hooks(Hooks {
        reap_unlink: Some(Arc::new(move |p: &Path| {
            if first.swap(false, Ordering::SeqCst) {
                std::fs::remove_file(&lock).unwrap(); // as if broken as stale
                assert_eq!(o.reap(BUDGET).reaped, 2);
            }
            std::fs::remove_file(p)
        })),
        ..Hooks::default()
    });
    let out = r.reap(BUDGET);
    assert_eq!((out.scanned, out.reaped, out.complete), (4, 0, true));
    assert!(
        r.drain_stats().is_empty(),
        "ENOENT is neither reaped nor failed"
    );
    assert_eq!(present(&s, &ks), [false, true, false, true]);
}

/// An unlink that fails for any other reason (on Windows, a segment a reader has mapped, §12) is
/// skipped and counted `shm_reap_failed`; eviction goes on to the next candidate.
#[test]
fn a_failed_unlink_is_skipped_and_counted() {
    let (_d, s) = store();
    let ks = keys_n(3);
    for k in &ks {
        s.publish(k, wave(200_000).r()).unwrap();
    }
    for k in &ks {
        age(&s.path_for(k).unwrap(), 2.0 * s.options().ttl_s);
    }
    let pinned = s.path_for(&ks[1]).unwrap();
    let r = again(&s, options()).with_hooks(Hooks {
        reap_unlink: Some(Arc::new(move |p: &Path| {
            if p == pinned {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            std::fs::remove_file(p)
        })),
        ..Hooks::default()
    });
    let seg = file_size(&s, &ks[0]);
    let out = r.reap(BUDGET);
    assert_eq!((out.scanned, out.reaped, out.complete), (3, 2, true));
    assert_eq!(
        stats(&r),
        [
            ("shm_reap_failed", 1),
            ("shm_reaped", 2),
            ("shm_reaped_bytes", 2 * seg)
        ]
    );
    assert_eq!(present(&s, &ks), [false, true, false]);
    let reader = again(
        &s,
        Options {
            reap_interval_s: 30.0,
            ..options()
        },
    );
    assert_eq!(
        reader.usage().unwrap().bytes,
        seg,
        "usage counts what is still there"
    );
}

/// Reapers in several threads, each its own store on one tree, reach the survivor set one
/// reaper reaches alone: the lock serialises them, and whoever comes second finds the tree at
/// its targets already.
#[test]
fn concurrent_reapers_reach_the_same_survivors() {
    fn build() -> (tempfile::TempDir, Store, Vec<String>) {
        let (d, s) = store();
        let ks = keys_n(10);
        for k in &ks {
            s.publish(k, wave(200_000).r()).unwrap();
        }
        for (i, k) in ks.iter().enumerate() {
            let age_s = if i == 7 {
                2000.0
            } else {
                800.0 - i as f64 * 60.0
            };
            age(&s.path_for(k).unwrap(), age_s);
        }
        (d, s, ks)
    }
    let reaping = Options {
        max_bytes: 5 * 400_000, // low water: 3 files
        ..options()
    };
    let (_d1, alone, ks) = build();
    assert!(again(&alone, reaping.clone()).reap(BUDGET).complete);
    let expected = present(&alone, &ks);
    assert_eq!(expected.iter().filter(|p| **p).count(), 3);

    let (_d2, s, _) = build();
    let gate = std::sync::Barrier::new(8);
    let reaped: u64 = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let r = again(&s, reaping.clone());
                let gate = &gate;
                scope.spawn(move || {
                    gate.wait();
                    let mut n = 0;
                    loop {
                        let out = r.reap(BUDGET);
                        n += out.reaped;
                        if out.complete {
                            break n;
                        }
                        std::thread::yield_now();
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    assert_eq!(reaped, 7, "every segment reaped exactly once");
    assert_eq!(present(&s, &ks), expected);
    assert!(!s.root().join(".reap.lock").exists());
}

/// C9: a Rust publisher's rate-limited turn really reaps.
#[test]
fn a_publish_turn_reaps_expired_segments() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    age(&s.path_for(&key()).unwrap(), 2.0 * s.options().ttl_s);
    s.drain_stats();
    s.publish(&key_in(D2, "wav16k"), wave(200_000).r()).unwrap();
    let st = s.drain_stats();
    assert_eq!((st.get("shm_publish"), st.get("shm_reaped")), (1, 1));
    assert!(s.attach(&key()).is_none());
    assert!(s.root().join(".usage.json").exists());
}

/// Orphaned temporary files over an hour old are swept from segment directories and nowhere
/// else; they are not candidates (contract §9.5).
#[test]
fn old_temporary_files_are_swept_young_ones_kept() {
    let (_d, s) = store();
    s.publish(&key(), wave(200_000).r()).unwrap();
    s.drain_stats();
    let dir = s.path_for(&key()).unwrap().parent().unwrap().to_path_buf();
    let old = dir.join("x.npy.1.2.tmp");
    let young = dir.join("y.npy.1.2.tmp");
    for (p, a) in [(&old, 3601.0), (&young, 3500.0)] {
        std::fs::write(p, b"partial").unwrap();
        age(p, a);
    }
    let out = s.reap(BUDGET);
    assert_eq!((out.scanned, out.reaped), (1, 0));
    assert!(!old.exists());
    assert!(young.exists());
    assert!(s.drain_stats().is_empty());
}

/// A segment already gone when this reaper unlinks it has left the tree all the same, so it
/// counts toward the targets (contract §9.6). Not counting it let two racing reapers (after a
/// broken stale lock, C8) take six segments to none against a low water mark of two. Here
/// "another reaper" removes the two oldest just before this one does.
#[test]
fn an_already_gone_segment_counts_toward_the_target() {
    let (_d, s) = store();
    let ks = keys_n(6);
    for (i, k) in ks.iter().enumerate() {
        s.publish(k, wave(200_000).r()).unwrap();
        age(&s.path_for(k).unwrap(), 600.0 - i as f64 * 60.0);
    }
    let raced = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let r = again(
        &s,
        Options {
            max_bytes: 3 * 400_000,
            ..options()
        },
    )
    .with_hooks(Hooks {
        reap_unlink: Some({
            let raced = Arc::clone(&raced);
            Arc::new(move |p: &Path| {
                std::fs::remove_file(p)?;
                if raced.fetch_add(1, Ordering::SeqCst) < 2 {
                    // The other reaper got there first.
                    return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
                }
                Ok(())
            })
        }),
        ..Hooks::default()
    });
    let out = r.reap(BUDGET);
    assert!(out.complete);
    assert_eq!(
        (out.gone, out.reaped),
        (2, 2),
        "two taken by the other reaper, two by this one"
    );
    assert_eq!(present(&s, &ks), [false, false, false, false, true, true]);
    // Read the file itself: these options make it stale to `usage()` at once.
    let usage: serde_json::Value =
        serde_json::from_slice(&std::fs::read(s.root().join(".usage.json")).unwrap()).unwrap();
    assert_eq!(
        usage["files"], 2,
        ".usage.json counts what is left, gone ones excluded"
    );
}

// ------------------------------------------------------------------------------ remove (§6.7)

/// A caller replacing a value under an unchanged key removes it, then publishes again
/// (`test_remove_lets_a_different_array_be_published`).
#[test]
fn test_remove_lets_a_different_array_be_published() {
    let (_d, s) = store();
    let k = key();
    assert_eq!(s.publish(&k, wave(1000).r()), Ok(true));
    assert!(
        s.publish(&k, wave(999).r()).is_err(),
        "a different array collides"
    );
    assert!(s.remove(&k));
    assert!(s.attach(&k).is_none());
    assert_eq!(s.publish(&k, wave(999).r()), Ok(true));
    assert_eq!(s.attach(&k).unwrap().shape(), [999]);
    assert_eq!(s.drain_stats().get("shm_removed"), 1);
}

#[test]
fn test_remove_of_an_absent_key_is_false_and_counts_nothing() {
    let (_d, s) = store();
    assert!(!s.remove(&key()));
    assert!(s.drain_stats().is_empty());
}

#[test]
fn test_remove_needs_readwrite_and_a_valid_key() {
    let (_d, s) = store();
    s.publish(&key(), wave(1000).r()).unwrap();
    let reader = again(
        &s,
        Options {
            mode: Mode::Read,
            ..options()
        },
    );
    assert!(!reader.remove(&key()));
    assert_eq!(reader.drain_stats().get("shm_declined_mode"), 1);
    assert!(!s.remove(&format!("nosuchdomain:v1:x:{D1}:x")));
    assert_eq!(s.drain_stats().get("shm_bad_key"), 1);
    assert!(s.attach(&key()).is_some());
}

/// It only unlinks (§7): a reader holding the segment keeps the old bytes.
#[test]
fn test_remove_leaves_a_mapped_reader_its_bytes() {
    let (_d, s) = store();
    let a = wave(1000);
    s.publish(&key(), a.r()).unwrap();
    let held = s.attach(&key()).unwrap();
    assert!(s.remove(&key()));
    assert_eq!(held.bytes(), a.data.as_slice());
}

#[cfg(unix)]
#[test]
fn test_remove_never_unlinks_a_symlink() {
    let (d, s) = store();
    let victim = d.path().join("victim.npy");
    std::fs::write(&victim, b"x").unwrap();
    let path = s.path_for(&key()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&victim, &path).unwrap();
    assert!(!s.remove(&key()));
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink() && victim.exists());
}

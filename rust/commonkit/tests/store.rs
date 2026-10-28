//! Rust ports of Python's `tests/test_store.py` (each test keeps the Python name), the touch
//! tests of contract §8.2, and what only Rust has: typed views, the element probe, the mapping
//! surviving unlink and rename-over (§7), and the reap-turn hook.
//!
//! The store is addressed through an audio scheme, as in Python, to prove it is generic. The
//! scheme registry is process-global, so this file's domain (`ckaudio`) is its own.
//!
//! Python tests of the reaper itself are step 7 and not ported here: `test_reaper_refuses_a_
//! suspicious_root`, `test_reaper_only_touches_its_own_segments`, `test_reaper_leaves_fresh_
//! segments_alone`, `test_over_budget_evicts_least_recently_used_first`, `test_expired_
//! segments_are_reaped`, `test_the_free_space_floor_evicts_least_recently_used`, `test_
//! eviction_reaches_its_target_whatever_the_time_budget` and `test_a_partial_walk_neither_
//! evicts_for_budget_nor_writes_usage`. `test_a_declined_publish_still_reaps` is ported as far
//! as step 6 goes: the decline takes a reap turn.

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

/// Python's `test_a_declined_publish_still_reaps`, as far as step 6 goes: a space decline takes
/// a reap turn, as a publish does. (The reaper behind the turn is step 7.)
#[test]
fn test_a_declined_publish_still_reaps() {
    let (turns, mut hooks) = turn_counter();
    hooks.free_bytes = Some(Arc::new(|_: &Path| Some(4096)));
    let (_d, s) = store_with(
        Options {
            min_free_bytes: 1 << 30,
            ..options()
        },
        hooks,
    );
    assert_eq!(s.publish(&key(), wave(200_000).r()), Ok(false));
    assert_eq!(stats(&s), [("shm_declined_space", 1)]);
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

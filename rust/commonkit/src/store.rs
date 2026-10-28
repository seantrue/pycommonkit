//! The store: publish and attach arrays under one root (contract §6, §8).
//!
//! **The invariant:** a missing segment is never an error, only a slower path. Nothing here
//! panics or returns an error to its caller except [`Store::publish`]'s [`KeyCollision`], the
//! contract's second loud path (§1.6, §6.4). Construction never fails: an unusable root gives a
//! disabled store, on which attach misses and publish declines. Every miss and decline ticks a
//! counter (§11), drained with [`Store::drain_stats`].
//!
//! The reference is Python's `commonkit.store.SharedArrayStore`; behaviour matches it, and the
//! module docstrings there carry the production measurements behind each rule.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{self, Json};
use crate::keys::{Key, Registry};
use crate::npy::{self, Descr, Header, NpyError, Order};
use crate::segment::{self, AttachMode, Load, Segment};
use crate::sys;

/// Touch a segment's mtime at most this often (contract §8.2). atime is unusable: ZFS serving
/// reads from ARC never updates it, and a metadata write per read is copy-on-write churn.
pub const TOUCH_INTERVAL_S: f64 = 60.0;

/// `statvfs` is a syscall per publish otherwise; a second of staleness can't matter to a
/// watermark measured in gigabytes.
const FREE_SPACE_TTL_S: f64 = 1.0;

/// The most of `.usage.json` that is read. The reaper writes about 60 bytes; Python reads any
/// length, but a hostile file must not make this allocate without bound.
const USAGE_MAX_BYTES: u64 = 65536;

/// `verify_republish` compares payloads in pieces of about this size, so checking a 128 MiB
/// segment costs a 1 MiB buffer, not a second 128 MiB one.
const COMPARE_CHUNK: usize = 1 << 20;

/// A reap turn after a publish walks for at most this long (contract §9.8).
#[allow(dead_code)] // used by the reaper, step 7
pub(crate) const REAP_TURN_BUDGET_S: f64 = 0.05;

// --------------------------------------------------------------------------- settings

/// What a store may do (contract §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Disabled: attach misses, publish declines.
    Off,
    /// Attach only; every publish is declined (`shm_declined_mode`).
    #[default]
    Read,
    /// Attach and publish.
    ReadWrite,
}

/// A store's settings. The tree-wide ones (`max_bytes` to `verify_republish`) come from the
/// policy for a shared store ([`crate::shared_store`]); a caller chooses only `mode`, `attach`
/// and `min_array_bytes`. Tests construct stores with explicit settings.
///
/// The defaults are those of Python's `SharedArrayStore` constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub mode: Mode,
    pub attach: AttachMode,
    /// The byte budget for the whole tree (the reaper's target is 80% of it).
    pub max_bytes: i64,
    /// Decline a publish, and reap, when the volume has less free than this.
    pub min_free_bytes: i64,
    /// Segments untouched for longer are reaped.
    pub ttl_s: f64,
    /// Smaller arrays are declined: not worth a file.
    pub min_array_bytes: u64,
    /// Larger arrays are declined: one would swallow the tree.
    pub max_array_bytes: i64,
    /// At most one reap turn per this many seconds per store.
    pub reap_interval_s: f64,
    /// Sync each segment and its directory to storage (C17; off by default).
    pub fsync: bool,
    /// Compare a republished array's bytes with the segment's (C15; off by default).
    pub verify_republish: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            mode: Mode::Read,
            attach: AttachMode::Mmap,
            max_bytes: 16 << 30,
            min_free_bytes: 8 << 30,
            ttl_s: 900.0,
            min_array_bytes: 256 * 1024,
            max_array_bytes: 128 << 20,
            reap_interval_s: 30.0,
            fsync: false,
            verify_republish: false,
        }
    }
}

// --------------------------------------------------------------------------- counters

/// The counters of contract §11. Their names are shared with Python, so a stats table can sum
/// them across languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Counter {
    Hit,
    Miss,
    Torn,
    BadKey,
    TornReplaced,
    KeyCollision,
    DeclinedMode,
    DeclinedSmall,
    DeclinedSize,
    DeclinedSpace,
    DeclinedBudget,
    DeclinedDtype,
    Publish,
    BytesPublished,
    PublishFailed,
    Reaped,
    ReapedBytes,
    ReapPartial,
    ReapRefused,
    ReapFailed,
}

impl Counter {
    pub const ALL: [Counter; 20] = [
        Counter::Hit,
        Counter::Miss,
        Counter::Torn,
        Counter::BadKey,
        Counter::TornReplaced,
        Counter::KeyCollision,
        Counter::DeclinedMode,
        Counter::DeclinedSmall,
        Counter::DeclinedSize,
        Counter::DeclinedSpace,
        Counter::DeclinedBudget,
        Counter::DeclinedDtype,
        Counter::Publish,
        Counter::BytesPublished,
        Counter::PublishFailed,
        Counter::Reaped,
        Counter::ReapedBytes,
        Counter::ReapPartial,
        Counter::ReapRefused,
        Counter::ReapFailed,
    ];

    /// The contract's name for this counter.
    pub fn name(self) -> &'static str {
        match self {
            Counter::Hit => "shm_hit",
            Counter::Miss => "shm_miss",
            Counter::Torn => "shm_torn",
            Counter::BadKey => "shm_bad_key",
            Counter::TornReplaced => "shm_torn_replaced",
            Counter::KeyCollision => "shm_key_collision",
            Counter::DeclinedMode => "shm_declined_mode",
            Counter::DeclinedSmall => "shm_declined_small",
            Counter::DeclinedSize => "shm_declined_size",
            Counter::DeclinedSpace => "shm_declined_space",
            Counter::DeclinedBudget => "shm_declined_budget",
            Counter::DeclinedDtype => "shm_declined_dtype",
            Counter::Publish => "shm_publish",
            Counter::BytesPublished => "shm_bytes_published",
            Counter::PublishFailed => "shm_publish_failed",
            Counter::Reaped => "shm_reaped",
            Counter::ReapedBytes => "shm_reaped_bytes",
            Counter::ReapPartial => "shm_reap_partial",
            Counter::ReapRefused => "shm_reap_refused",
            Counter::ReapFailed => "shm_reap_failed",
        }
    }

    fn index(self) -> usize {
        Counter::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// Counters taken from a store by [`Store::drain_stats`], by contract name. Only counters that
/// moved are present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats(BTreeMap<&'static str, u64>);

impl Stats {
    /// The count for a contract name (`"shm_hit"`), 0 if it did not move.
    pub fn get(&self, name: &str) -> u64 {
        self.0.get(name).copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, u64)> + '_ {
        self.0.iter().map(|(k, v)| (*k, *v))
    }

    pub fn into_map(self) -> BTreeMap<&'static str, u64> {
        self.0
    }
}

// ----------------------------------------------------------------------------- arrays

/// An array to publish: a descriptor, a shape, the memory order its bytes are laid out in, and
/// the bytes themselves, borrowed. Checked when built, so a publish never sees an array whose
/// bytes disagree with its header.
#[derive(Debug, Clone, Copy)]
pub struct ArrayRef<'a> {
    descr: &'a Descr,
    shape: &'a [u64],
    order: Order,
    data: &'a [u8],
    nbytes: u64,
}

impl<'a> ArrayRef<'a> {
    /// `data` must be exactly `itemsize × product(shape)` bytes in `order` (C: last index
    /// fastest; F: first index fastest). Fails on more than [`npy::MAX_DIMS`] dimensions, an
    /// overflowing size, or a payload of the wrong length. An object dtype can't be expressed:
    /// [`Descr`] refuses to represent one (contract §5.4).
    pub fn new(
        descr: &'a Descr,
        shape: &'a [u64],
        order: Order,
        data: &'a [u8],
    ) -> Result<ArrayRef<'a>, NpyError> {
        if shape.len() > npy::MAX_DIMS {
            return Err(NpyError::BadShape);
        }
        let nbytes = npy::nbytes(descr, shape).ok_or(NpyError::Overflow)?;
        let have = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if have != nbytes {
            return Err(NpyError::PayloadLength { need: nbytes, have });
        }
        Ok(ArrayRef {
            descr,
            shape,
            order,
            data,
            nbytes,
        })
    }

    pub fn descr(&self) -> &'a Descr {
        self.descr
    }

    pub fn shape(&self) -> &'a [u64] {
        self.shape
    }

    pub fn order(&self) -> Order {
        self.order
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// `itemsize × product(shape)`.
    pub fn nbytes(&self) -> u64 {
        self.nbytes
    }

    /// The `fortran_order` a writer puts in the header for this array (contract §5.3).
    pub fn fortran_order(&self) -> bool {
        npy::fortran_flag(self.shape, self.order)
    }
}

/// A different array was published under a key whose segment already exists: the contract's
/// second loud path (§1.6, §6.4). A key names one immutable content (W1), so this means a key
/// scheme or a digest is broken. The existing file is left exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCollision {
    pub key: String,
    pub path: PathBuf,
    /// What differs: "a different dtype, shape or memory order", or "different named-field
    /// bytes" (with `verify_republish`).
    pub reason: &'static str,
}

impl fmt::Display for KeyCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: an existing segment holds {}", self.key, self.reason)
    }
}

impl std::error::Error for KeyCollision {}

/// The tree's usage from the reaper's last complete walk (`.usage.json`, contract §9.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    /// `None` when the file's `files` is missing or not a non-negative integer.
    pub files: Option<u64>,
}

// ------------------------------------------------------------------------------ hooks

type FreeHook = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;
type SyncHook = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;
type TurnHook = Arc<dyn Fn() + Send + Sync>;

/// Test seams, as Python's tests monkeypatch `os.statvfs`, `_sync` and `_sync_dir`. Each hook,
/// when set, replaces the real call. Not part of the API.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct Hooks {
    /// Free bytes on the root's volume; `None` is "can't tell", read as 0.
    pub free_bytes: Option<FreeHook>,
    /// Sync the temporary file (given its path) before the rename.
    pub sync_file: Option<SyncHook>,
    /// Sync the segment directory after the rename.
    pub sync_dir: Option<SyncHook>,
    /// Called on every reap turn that passes the rate limit (contract §9.8).
    pub reap_turn: Option<TurnHook>,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
            .field("free_bytes", &self.free_bytes.is_some())
            .field("sync_file", &self.sync_file.is_some())
            .field("sync_dir", &self.sync_dir.is_some())
            .field("reap_turn", &self.reap_turn.is_some())
            .finish()
    }
}

// ------------------------------------------------------------------------------ store

/// Publish and attach arrays under one root directory. `Send` and `Sync`: share one per
/// process (behind an `Arc`) as Python shares its store.
pub struct Store {
    root: PathBuf,
    options: Options,
    disabled: bool,
    counters: [AtomicU64; Counter::ALL.len()],
    /// `(measured at, free bytes)`; at 0.0 means never measured.
    free: Mutex<(f64, u64)>,
    /// When the last reap turn ran, in Unix seconds; 0.0 at first, as in Python.
    last_reap: Mutex<f64>,
    hooks: Hooks,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .field("disabled", &self.disabled)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// What the existing-key rule found at a segment path (contract §6.4).
enum Existing {
    Absent,
    Torn,
    /// The same array is already there: touched, nothing to write.
    Republish,
    /// The comparison itself failed: counted, nothing written.
    Failed,
}

fn now_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn mtime_s(meta: &fs::Metadata) -> Option<f64> {
    let t = meta.modified().ok()?;
    Some(match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    })
}

/// `path` made absolute against the working directory and normalised lexically (`.` dropped,
/// `..` taken back a component), as Python's `os.path.abspath`. `None` if the working directory
/// can't be read.
pub(crate) fn absolute(path: &Path) -> Option<PathBuf> {
    let abs = std::path::absolute(path).ok()?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
        }
    }
    Some(out)
}

impl Store {
    /// A store at `root`. Never fails (contract §6.1): the store is disabled when the mode is
    /// `Off`, the root is empty or a filesystem root (`/`), or the root can't be created or
    /// written (checked with a probe file `.writable.{pid}.{tid}`, then unlinked).
    pub fn new(root: impl AsRef<Path>, options: Options) -> Store {
        let given = root.as_ref();
        let root = if given.as_os_str().is_empty() {
            None
        } else {
            absolute(given)
        };
        let mut store = Store {
            root: root.clone().unwrap_or_default(),
            options,
            disabled: true,
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
            free: Mutex::new((0.0, 0)),
            last_reap: Mutex::new(0.0),
            hooks: Hooks::default(),
        };
        let Some(root) = root else {
            return store;
        };
        if store.options.mode == Mode::Off || root.parent().is_none() {
            return store;
        }
        // Unique per process and thread: a fixed name raced when K workers started together,
        // the first unlink won, and every loser came up disabled (measured: 1 of 4).
        let probe = root.join(format!(
            ".writable.{}.{:x}",
            std::process::id(),
            sys::thread_tag()
        ));
        let usable = fs::create_dir_all(&root).is_ok() && fs::write(&probe, b"1").is_ok();
        // Failing to unlink is not failing to write: the write already proved the root usable.
        let _ = fs::remove_file(&probe);
        store.disabled = !usable;
        store
    }

    /// Install test hooks. Not part of the API.
    #[doc(hidden)]
    pub fn with_hooks(mut self, hooks: Hooks) -> Store {
        self.hooks = hooks;
        self
    }

    /// The absolute root (empty for a store given an empty root).
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    pub fn mode(&self) -> Mode {
        self.options.mode
    }

    pub fn attach_mode(&self) -> AttachMode {
        self.options.attach
    }

    /// Whether the store is disabled: it attaches nothing and declines every publish.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The absolute path for a key that satisfies the grammar (contract §3.1), registered or
    /// not; always under the root. `None` for a malformed key.
    pub fn path_for(&self, key: &str) -> Option<PathBuf> {
        Key::parse(key)
            .ok()
            .map(|k| self.root.join(k.relpath_buf()))
    }

    fn count(&self, c: Counter, n: u64) {
        if let Some(slot) = self.counters.get(c.index()) {
            slot.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Take and reset the counters (contract §11), each read and zeroed atomically.
    pub fn drain_stats(&self) -> Stats {
        let mut out = BTreeMap::new();
        for (c, slot) in Counter::ALL.iter().zip(&self.counters) {
            let n = slot.swap(0, Ordering::Relaxed);
            if n != 0 {
                out.insert(c.name(), n);
            }
        }
        Stats(out)
    }

    // ------------------------------------------------------------------------ reading

    /// The segment for `key`, or `None` on any kind of miss (contract §8.1). A reader takes any
    /// key that satisfies the grammar, registered or not (C4). A hit touches the segment.
    pub fn attach(&self, key: &str) -> Option<Segment> {
        if self.disabled {
            return None;
        }
        let Some(path) = self.path_for(key) else {
            self.count(Counter::BadKey, 1);
            return None;
        };
        match segment::load(&path, self.options.attach) {
            Load::Absent => {
                self.count(Counter::Miss, 1);
                None
            }
            Load::Torn => {
                self.count(Counter::Torn, 1);
                None
            }
            Load::Whole(seg) => {
                self.count(Counter::Hit, 1);
                self.touch(&path);
                Some(seg)
            }
        }
    }

    /// Refresh mtime (and atime) for the LRU order, at most once per [`TOUCH_INTERVAL_S`]
    /// (contract §8.2). A touch that fails, say on another user's file, is ignored.
    fn touch(&self, path: &Path) {
        let Ok(meta) = fs::metadata(path) else {
            return;
        };
        if mtime_s(&meta).is_some_and(|m| now_s() - m > TOUCH_INTERVAL_S) {
            let _ = sys::touch_now(path);
        }
    }

    // ------------------------------------------------------------------------ writing

    /// Store `array` under `key` (contract §6).
    ///
    /// `Ok(true)`: written, or the same array was already there (a republish, which touches it).
    /// `Ok(false)`: declined or failed, counted; the caller keeps the array it holds.
    /// `Err(KeyCollision)`: a different array already has a whole segment under `key`; the file
    /// and its mtime are untouched, the collision is counted and logged at error level.
    pub fn publish(&self, key: &str, array: ArrayRef<'_>) -> Result<bool, KeyCollision> {
        // Gate 1.
        if self.disabled || self.options.mode != Mode::ReadWrite {
            self.count(Counter::DeclinedMode, 1);
            return Ok(false);
        }
        // Gates 2 to 5.
        if !self.admit(array.nbytes()) {
            return Ok(false);
        }
        // Gate 6.
        let Some(header) = self.admit_dtype(key, &array) else {
            return Ok(false);
        };
        // Gate 7: a publisher validates against the registered scheme (C4).
        let Ok(k) = Registry::global().parse(key) else {
            self.count(Counter::BadKey, 1);
            return Ok(false);
        };
        let path = self.root.join(k.relpath_buf());

        let existing = self.existing(key, &path, &array)?;
        let torn = match existing {
            Existing::Republish => return Ok(true),
            Existing::Failed => return Ok(false),
            Existing::Torn => true,
            Existing::Absent => false,
        };

        let Some(parent) = path.parent() else {
            self.count(Counter::PublishFailed, 1);
            return Ok(false);
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let tmp = parent.join(format!(
            "{name}.{}.{}.tmp",
            std::process::id(),
            sys::thread_tag()
        ));
        if let Err(e) = self.write_segment(parent, &tmp, &path, &header, array.data()) {
            let _ = fs::remove_file(&tmp);
            let c = if sys::is_full(&e) {
                Counter::DeclinedSpace
            } else {
                Counter::PublishFailed
            };
            self.count(c, 1);
            return Ok(false);
        }
        if torn {
            self.count(Counter::TornReplaced, 1);
        }
        if self.options.fsync && self.sync_dir(parent).is_err() {
            // The segment is whole and stays; but durability was asked for and not had.
            self.count(Counter::PublishFailed, 1);
            return Ok(false);
        }
        self.count(Counter::Publish, 1);
        self.count(Counter::BytesPublished, array.nbytes());
        self.maybe_reap();
        Ok(true)
    }

    /// Contract §6.5 steps 1 to 4: directory, temporary file, optional sync, rename.
    fn write_segment(
        &self,
        parent: &Path,
        tmp: &Path,
        path: &Path,
        header: &[u8],
        data: &[u8],
    ) -> io::Result<()> {
        fs::create_dir_all(parent)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(tmp)?;
        f.write_all(header)?;
        f.write_all(data)?;
        if self.options.fsync {
            match &self.hooks.sync_file {
                Some(hook) => hook(tmp)?,
                None => sys::sync_file(&f)?,
            }
        }
        drop(f);
        fs::rename(tmp, path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        match &self.hooks.sync_dir {
            Some(hook) => hook(dir),
            None => sys::sync_dir(dir),
        }
    }

    /// Gates 2 to 5, cheapest first (contract §6.2). A space or budget decline also takes a
    /// reap turn: a store that filled up used to decline forever and never be reaped again.
    fn admit(&self, nbytes: u64) -> bool {
        let n = i128::from(nbytes);
        if nbytes < self.options.min_array_bytes {
            self.count(Counter::DeclinedSmall, 1);
            return false;
        }
        if n > i128::from(self.options.max_array_bytes) {
            self.count(Counter::DeclinedSize, 1);
            return false;
        }
        if i128::from(self.free_bytes(false)) < i128::from(self.options.min_free_bytes) {
            self.count(Counter::DeclinedSpace, 1);
            self.maybe_reap();
            return false;
        }
        if let Some(u) = self.usage()
            && i128::from(u.bytes) + n > i128::from(self.options.max_bytes)
        {
            self.count(Counter::DeclinedBudget, 1);
            self.maybe_reap();
            return false;
        }
        true
    }

    /// Gate 6 (contract §6.3). Returns the header to write, or `None` (counted) when the array
    /// can't be written or its form declares another dtype. An object dtype never gets here:
    /// [`Descr`] can't represent one.
    fn admit_dtype(&self, key: &str, array: &ArrayRef<'_>) -> Option<Vec<u8>> {
        // A header over 1 MiB, or a field name whose repr can't be reproduced, is never written.
        let Ok(header) = npy::encode_header(array.descr(), array.shape(), array.order()) else {
            self.count(Counter::DeclinedDtype, 1);
            return None;
        };
        // A key that fails its scheme passes here; gate 7 counts it.
        let registry = Registry::global();
        if let Ok(scheme) = registry.scheme_for(key)
            && let Ok(k) = scheme.parse(key)
            && let Some(want) = scheme.dtype_for(k.form())
            && want != array.descr()
        {
            self.count(Counter::DeclinedDtype, 1);
            return None;
        }
        Some(header)
    }

    /// The existing-key rule (contract §6.4, C15), before anything is written.
    fn existing(
        &self,
        key: &str,
        path: &Path,
        array: &ArrayRef<'_>,
    ) -> Result<Existing, KeyCollision> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Existing::Absent),
            // Torn (W4): damage from outside the contract, so replacing it is repair.
            Err(_) => return Ok(Existing::Torn),
        };
        let Some(old) = whole_header(&mut file) else {
            return Ok(Existing::Torn);
        };
        // The header the new array WOULD be written with, against the file's. The file's
        // order is taken as a writer would write it, so a degenerate shape compares equal.
        let old_order = if old.fortran_order() {
            Order::F
        } else {
            Order::C
        };
        let mut same = old.descr() == array.descr()
            && old.shape() == array.shape()
            && npy::fortran_flag(old.shape(), old_order) == array.fortran_order();
        let mut reason = "a different dtype, shape or memory order";
        if same && self.options.verify_republish {
            match same_payload(&mut file, &old, array) {
                Some(s) => same = s,
                None => {
                    // Not knowing is not a collision: the segment stays, nothing raises.
                    self.count(Counter::PublishFailed, 1);
                    return Ok(Existing::Failed);
                }
            }
            reason = "different named-field bytes";
        }
        drop(file); // closed before the touch: Windows pins an open file
        if same {
            self.touch(path);
            return Ok(Existing::Republish);
        }
        self.count(Counter::KeyCollision, 1);
        log::error!(
            "commonkit key collision: {key} already holds a whole segment at {}; the new array \
             has {reason}. A key scheme or digest is broken.",
            path.display()
        );
        Err(KeyCollision {
            key: key.to_owned(),
            path: path.to_owned(),
            reason,
        })
    }

    /// Free bytes on the root's volume (contract §9.9), cached for a second; 0 when it can't be
    /// measured. `fresh` skips the cache (the reaper measures fresh).
    fn free_bytes(&self, fresh: bool) -> u64 {
        let now = now_s();
        let mut cache = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        if fresh || now - cache.0 > FREE_SPACE_TTL_S {
            let free = match &self.hooks.free_bytes {
                Some(hook) => hook(&self.root),
                None => sys::free_bytes(&self.root),
            };
            *cache = (now, free.unwrap_or(0));
        }
        cache.1
    }

    /// The tree's usage from `.usage.json` (contract §9.7), or `None` when it is unknown: the
    /// file is missing, not a regular file, stale (older than twice the reap interval), or not
    /// a JSON object whose `bytes` is a non-negative integer. Then the budget gate passes.
    pub fn usage(&self) -> Option<Usage> {
        if self.disabled {
            return None;
        }
        let marker = self.root.join(".usage.json");
        let meta = fs::metadata(&marker).ok()?;
        // A FIFO named .usage.json would block a read forever; only a file is read.
        if !meta.is_file() || meta.len() > USAGE_MAX_BYTES {
            return None;
        }
        if now_s() - mtime_s(&meta)? > 2.0 * self.options.reap_interval_s {
            return None;
        }
        let mut text = Vec::new();
        File::open(&marker)
            .ok()?
            .take(USAGE_MAX_BYTES + 1)
            .read_to_end(&mut text)
            .ok()?;
        let doc = json::parse(std::str::from_utf8(&text).ok()?)?;
        let bytes = json_uint(doc.get("bytes")?)?;
        let files = doc.get("files").and_then(json_uint);
        Some(Usage { bytes, files })
    }

    // ------------------------------------------------------------------------ reaping

    /// A rate-limited reap turn (contract §9.8): after a successful publish, and after a space
    /// or budget decline. At most one per `reap_interval_s` per store.
    fn maybe_reap(&self) {
        let now = now_s();
        {
            let mut last = self
                .last_reap
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if now - *last < self.options.reap_interval_s {
                return;
            }
            *last = now;
        }
        if let Some(hook) = &self.hooks.reap_turn {
            hook();
        }
        self.reap_turn();
    }

    /// STEP 7 STUB. The reaper (contract §9: lock, walk, LRU toward both targets,
    /// `.usage.json`, the `.tmp` sweep) lands in step 7 of the plan. Then this reaps with a
    /// budget of [`REAP_TURN_BUDGET_S`] and counts any failure as `shm_reap_failed`; Rust
    /// publishers reap (C9). Until then a turn is taken at the right points and does nothing.
    fn reap_turn(&self) {}
}

/// A JSON integer that is not negative (`-0` is 0); one past `u64` saturates, which every
/// budget reads as "over". `None` for anything else: a float, `1e3`, a string, a bool.
fn json_uint(v: &Json) -> Option<u64> {
    let Json::Number {
        text,
        integer: true,
    } = v
    else {
        return None;
    };
    match text.strip_prefix('-') {
        Some(digits) => digits.bytes().all(|b| b == b'0').then_some(0),
        None => Some(text.parse::<u64>().unwrap_or(u64::MAX)),
    }
}

/// The header of a whole file, or `None` if the file is torn (contract §5.7).
fn whole_header(file: &mut File) -> Option<Header> {
    let len = file.metadata().ok().filter(fs::Metadata::is_file)?.len();
    let header = segment::read_header(file)?;
    header.check_len(len).ok()?;
    Some(header)
}

/// Whether the file's payload holds `array`'s named-field bytes (contract §6.4, §5.8). Both
/// have the same dtype, shape and order. Trailing bytes and padding bytes are not compared.
/// `None` if reading fails.
fn same_payload(file: &mut File, old: &Header, array: &ArrayRef<'_>) -> Option<bool> {
    let data = array.data();
    if data.is_empty() {
        return Some(true);
    }
    let itemsize = usize::try_from(old.itemsize()).ok()?.max(1);
    let mask = named_ranges(array.descr())?;
    let per_chunk = (COMPARE_CHUNK / itemsize).max(1);
    let chunk_len = per_chunk.checked_mul(itemsize)?.min(data.len());
    let mut buf = Vec::new();
    buf.try_reserve_exact(chunk_len).ok()?;
    buf.resize(chunk_len, 0);
    file.seek(SeekFrom::Start(u64::try_from(old.data_offset()).ok()?))
        .ok()?;
    for new in data.chunks(chunk_len) {
        let old_bytes = buf.get_mut(..new.len())?;
        file.read_exact(old_bytes).ok()?;
        let equal = match &mask {
            None => old_bytes == new,
            Some(ranges) => old_bytes
                .chunks(itemsize)
                .zip(new.chunks(itemsize))
                .all(|(a, b)| ranges.iter().all(|r| a.get(r.clone()) == b.get(r.clone()))),
        };
        if !equal {
            return Some(false);
        }
    }
    Some(true)
}

/// The byte ranges of an item that named fields cover, merged; `Some(None)` when they cover
/// every byte (no padding: compare whole items). `None` on overflow.
fn named_ranges(descr: &Descr) -> Option<Option<Vec<std::ops::Range<usize>>>> {
    fn cover(d: &Descr, offset: usize, out: &mut Vec<std::ops::Range<usize>>) -> Option<()> {
        match d {
            Descr::Simple(_) => {
                let size = usize::try_from(d.itemsize()?).ok()?;
                if size > 0 {
                    out.push(offset..offset.checked_add(size)?);
                }
            }
            Descr::Struct(fields) => {
                let mut at = offset;
                for f in fields {
                    let size = usize::try_from(f.itemsize()?).ok()?;
                    if !f.is_padding() && size > 0 {
                        match f.descr() {
                            Descr::Simple(_) => out.push(at..at.checked_add(size)?),
                            inner @ Descr::Struct(_) => {
                                // A subarray of structs: each element has its own padding.
                                let base = usize::try_from(inner.itemsize()?).ok()?;
                                let count = size.checked_div(base).unwrap_or(0);
                                for i in 0..count {
                                    cover(inner, at.checked_add(i.checked_mul(base)?)?, out)?;
                                }
                            }
                        }
                    }
                    at = at.checked_add(size)?;
                }
            }
        }
        Some(())
    }
    let itemsize = usize::try_from(descr.itemsize()?).ok()?;
    if matches!(descr, Descr::Simple(_)) {
        return Some(None);
    }
    let mut ranges = Vec::new();
    cover(descr, 0, &mut ranges)?;
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    let covers_all = matches!(merged.as_slice(), [only] if only.start == 0 && only.end == itemsize);
    Some((!covers_all).then_some(merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_ranges_skip_padding_recursively() {
        let d = Descr::parse("[('a', '|u1'), ('', '|V3'), ('b', '<i4')]").unwrap();
        assert_eq!(named_ranges(&d), Some(Some(vec![0..1, 4..8])));
        let d = Descr::parse("[('a', '<i4'), ('b', '<f8')]").unwrap();
        assert_eq!(named_ranges(&d), Some(None));
        let d = Descr::parse("<f4").unwrap();
        assert_eq!(named_ranges(&d), Some(None));
        let d = Descr::parse("[('s', [('x', '|u1'), ('', '|V1')], (2,)), ('', '|V2')]").unwrap();
        assert_eq!(named_ranges(&d), Some(Some(vec![0..1, 2..3])));
    }

    #[test]
    fn json_integers() {
        let n = |t: &str| json::parse(t).and_then(|j| json_uint(&j));
        assert_eq!(n("995"), Some(995));
        assert_eq!(n("-0"), Some(0));
        assert_eq!(n("-1"), None);
        assert_eq!(n("995.0"), None);
        assert_eq!(n("1e3"), None);
        assert_eq!(n("\"995\""), None);
        assert_eq!(n("true"), None);
        assert_eq!(n("99999999999999999999999"), Some(u64::MAX));
    }

    #[cfg(unix)]
    #[test]
    fn absolute_is_lexical() {
        assert_eq!(
            absolute(Path::new("/a/./b/../c")),
            Some(PathBuf::from("/a/c"))
        );
        assert_eq!(absolute(Path::new("/..")), Some(PathBuf::from("/")));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(Path::new("x")), Some(cwd.join("x")));
    }

    #[test]
    fn counter_names_are_the_contracts() {
        let names: Vec<_> = Counter::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 20);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 20);
        assert!(names.iter().all(|n| n.starts_with("shm_")));
        assert_eq!(Counter::ALL.iter().map(|c| c.index()).sum::<usize>(), 190);
    }
}

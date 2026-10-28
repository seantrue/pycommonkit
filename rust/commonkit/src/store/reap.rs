//! The reaper (contract §9): the only thing that removes segments.
//!
//! **Blast radius.** It is deliberately narrow, as Python's `SharedArrayStore.reap`:
//!
//! - it refuses to run at all when the root looks wrong (§9.1);
//! - it walks only the root, never follows a symbolic link, and descends only into directories
//!   of the segment shape (`{form}/{xx}/`), so a `.pinned/` tree or any other `.`-name is never
//!   entered (§9.3);
//! - it unlinks only regular files (by `lstat`) whose path has the segment shape
//!   `{form}/{xx}/{xx…}.{variant}.npy` (C6(a), §9.4), plus orphaned `*.tmp` files over an hour
//!   old in segment directories (C6(b), §9.5); and of the root's reserved names it touches only
//!   its own `.reap.lock`, `.usage.json` and `.usage.json.tmp`;
//! - it only ever unlinks or renames (§7). Unlinking a file another process has mapped is safe on
//!   POSIX: the mapping keeps the inode, and a reader that arrives later misses and re-decodes.
//!
//! **Windows** (§12, to be completed when the maintainer's Windows work is merged): a file that is
//! open or mapped can't be unlinked there, so evicting a segment a reader holds fails. That is a
//! skipped candidate, counted `shm_reap_failed`, never a panic; the reaper goes on to the next
//! one, as for any other failed unlink.
//!
//! The measurements behind the rules are in Python's docstrings and are kept here: eviction
//! always reaches its target, because the time budget limits the walk only (a budget that cut
//! eviction short once left a store 1.2 GB over its low water mark); and a partial walk neither
//! evicts for the byte budget nor writes `.usage.json`, because it under-counts.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::time::{Duration, Instant};

use super::{Counter, Options, Store, absolute, mtime_s, now_s};
use std::sync::PoisonError;

/// The reaper's lock in the root (§9.2).
const LOCK_NAME: &str = ".reap.lock";
/// The tree's usage, written after a complete walk (§9.7).
const USAGE_NAME: &str = ".usage.json";
const USAGE_TMP_NAME: &str = ".usage.json.tmp";

/// A lock older than this was left by a dead reaper and is broken (§9.2, C8).
pub const STALE_LOCK_S: f64 = 300.0;

/// An orphaned temporary file is swept once it is this old (§9.5). A writer holds its `.tmp` for
/// one write -- milliseconds -- so an hour old means the writer crashed, never that it is still
/// writing.
pub const TMP_ORPHAN_S: f64 = 3600.0;

/// How to reap: the walk's time budget, and whether to change anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReapOptions {
    /// Bounds the walk, checked before each directory (§9.3); never the eviction.
    pub budget: Duration,
    /// Change nothing (an implementation aid, contract §9.10): no lock, no unlink, no `.tmp`
    /// sweep, no `.usage.json`, no counters but a refusal. The report then lists what a reap
    /// would unlink (`unlink`) and sweep (`sweep`), treating every unlink as successful.
    pub dry_run: bool,
    /// A fixed clock (Unix seconds) for expiry and the `.tmp` age. Honoured by a dry run only;
    /// a real reap always reads the clock.
    pub now: Option<f64>,
}

impl Default for ReapOptions {
    fn default() -> ReapOptions {
        ReapOptions {
            budget: super::DEFAULT_REAP_BUDGET,
            dry_run: false,
            now: None,
        }
    }
}

/// What one reap did (contract §9.7). A reap that did not run (a disabled store, a refused root,
/// a lock another reaper holds) reports zeros and `complete: false`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ReapReport {
    /// Candidates the walk found (segments, not swept temporary files).
    pub scanned: u64,
    /// Candidates unlinked.
    pub reaped: u64,
    /// Their sizes.
    pub bytes: u64,
    /// The walk visited every directory before the deadline.
    pub complete: bool,
    /// Candidates already gone when this reaper came to unlink them (another reaper got
    /// there first, C8). They count toward the targets but are not this reaper's evictions.
    pub gone: u64,
    /// This was a dry run ([`ReapOptions::dry_run`]): nothing was changed, and `reaped` and
    /// `bytes` are what a reap would have unlinked.
    pub dry_run: bool,
    /// Dry run only: the candidates a reap would unlink, in eviction order.
    pub unlink: Vec<PathBuf>,
    /// Dry run only: the orphaned `.tmp` files a reap would sweep, in walk order.
    pub sweep: Vec<PathBuf>,
    /// `floor(0.8 × max_bytes)`, once the walk has run (§9.6).
    pub low_water: i64,
    /// `max(0, floor(1.25 × min_free_bytes) − free)`, once the walk has run (§9.6).
    pub need_free: u64,
}

/// What a reap of `root` with `options`' policy would do, changing nothing (contract §9.10, an
/// implementation aid): [`Store::reap_with`] with `dry_run` forced on, but without constructing
/// a store, so not even the store's probe file is written. This is how a live tree is
/// inspected. A refused root (§9.1) gives an empty report.
pub fn reap_plan(root: impl AsRef<Path>, options: &Options, reap: &ReapOptions) -> ReapReport {
    let root = root.as_ref();
    if root.as_os_str().is_empty() {
        return ReapReport::default();
    }
    let Some(root) = absolute(root) else {
        return ReapReport::default();
    };
    let store = Store::unprobed(root, options.clone());
    store.reap_with(&ReapOptions {
        dry_run: true,
        ..*reap
    })
}

/// What unlinking one candidate came to.
enum Unlink {
    Removed,
    /// Already gone: not an error, and out of the tree all the same.
    Gone,
    /// Any other failure: skipped and counted `shm_reap_failed`.
    Failed,
}

/// A segment found by the walk.
struct Candidate {
    mtime: f64,
    size: u64,
    path: PathBuf,
}

/// Holding `.reap.lock`; dropping it releases the lock by unlinking it (§9.2: every exit after
/// acquiring the lock releases it).
struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// `[a-z0-9]+`, a form token (§3.1).
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// `[0-9a-f]{2}`, a shard directory (§4).
fn is_shard(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(is_lower_hex)
}

/// A segment file name in shard directory `shard`: `{shard}[0-9a-f]{30}\.[a-z0-9]+\.npy` in
/// full (§9.4).
fn is_segment_name(name: &str, shard: &str) -> bool {
    let Some(stem) = name.strip_suffix(".npy") else {
        return false;
    };
    let (Some(digest), Some(rest)) = (stem.get(..32), stem.get(32..)) else {
        return false;
    };
    digest.bytes().all(is_lower_hex)
        && digest.get(..2) == Some(shard)
        && rest.strip_prefix('.').is_some_and(is_token)
}

/// The refusal of §9.1: an empty root, `/`, or fewer than two path separators (`/tmp` is refused,
/// `/tmp/store` is not). The separator count is a POSIX rule (§12); on Windows it counts `\`.
fn suspicious(root: &Path) -> bool {
    let bytes = root.as_os_str().as_encoded_bytes();
    bytes.is_empty()
        || root == Path::new("/")
        || bytes.iter().filter(|&&b| b == MAIN_SEPARATOR as u8).count() < 2
}

/// The subdirectories of `dir` whose names pass `keep`. Real directories only (a `DirEntry`'s
/// type is `lstat`'s), so a symbolic link to a directory is never entered. A directory that
/// can't be read has none, as in Python's `os.walk`.
fn subdirs(dir: &Path, keep: fn(&str) -> bool) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let real_dir = e.file_type().ok()?.is_dir();
            (real_dir && keep(&name)).then(|| (e.path(), name))
        })
        .collect()
}

enum Lock {
    Held(LockGuard),
    /// Another reaper holds it, or it was stale and has just been broken, or it could not be
    /// created: no reaping this turn.
    NotHeld,
    /// Created, but the pid could not be written (Python raises here; the turn counts it).
    Failed,
}

impl Store {
    /// Unlink expired segments, then the least recently used (oldest mtime; a hit refreshes it)
    /// until the tree is under its low water mark (80% of `max_bytes`) and the volume above its
    /// free-space floor plus 25% (contract §9).
    ///
    /// `budget` bounds the **walk** only, checked before each directory; eviction always reaches
    /// its target. A walk cut short by the budget is partial (`complete: false`): it doesn't
    /// evict for the byte budget, doesn't write `.usage.json`, and counts `shm_reap_partial`.
    /// Expired segments and the free-space floor are still served.
    ///
    /// Never fails and never panics. A disabled store doesn't reap; a suspicious root counts
    /// `shm_reap_refused`; a lock held by another reaper means no reap this time (a lock older
    /// than 300 s is broken, and this call still returns without reaping). A failed
    /// unlink other than "already gone" is skipped and counted `shm_reap_failed`.
    pub fn reap(&self, budget: Duration) -> ReapReport {
        self.reap_with(&ReapOptions {
            budget,
            ..ReapOptions::default()
        })
    }

    /// [`Store::reap`] with options; with `dry_run` it changes nothing and reports what it
    /// would unlink and sweep (contract §9.10). A refusal still counts `shm_reap_refused`.
    pub fn reap_with(&self, options: &ReapOptions) -> ReapReport {
        let mut out = ReapReport::default();
        if self.disabled {
            return out;
        }
        if suspicious(&self.root) {
            self.count(Counter::ReapRefused, 1);
            return out;
        }
        let dry = options.dry_run;
        // A dry run takes no lock: it writes nothing, so it can't race anything.
        let _guard = if dry {
            None
        } else {
            match self.lock() {
                Lock::Held(guard) => Some(guard),
                Lock::NotHeld => return out,
                Lock::Failed => {
                    self.count(Counter::ReapFailed, 1);
                    return out;
                }
            }
        };

        // A deadline past `Instant`'s range is no deadline.
        let deadline = Instant::now().checked_add(options.budget);
        let now = match options.now {
            Some(now) if dry => now,
            _ => now_s(),
        };
        let mut entries = Vec::new();
        let mut sweep = dry.then(Vec::new);
        let complete = self.walk(now, deadline, &mut entries, &mut sweep);
        out.scanned = entries.len() as u64;
        out.dry_run = dry;
        out.sweep = sweep.unwrap_or_default();

        let mut total: i128 = entries.iter().map(|c| i128::from(c.size)).sum();
        entries.sort_by(|a, b| {
            a.mtime
                .total_cmp(&b.mtime)
                .then(a.size.cmp(&b.size))
                .then_with(|| a.path.as_os_str().cmp(b.path.as_os_str()))
        });

        // Two targets, least recently used first: the byte budget's low water mark (judged only
        // on a complete walk: a partial one under-counts), and the free-space floor with 25%
        // headroom. The floor used to be a publish gate only, so a volume that filled before the
        // budget did -- a quota smaller than max_bytes + min_free_bytes -- declined everything
        // and evicted nothing. `as` from f64 truncates toward zero and saturates, as Python's
        // int() of the same products.
        let low_water = (0.8 * self.options.max_bytes as f64) as i128;
        let floor = (1.25 * self.options.min_free_bytes as f64) as i128;
        let need_free = (floor - i128::from(self.free_bytes(true))).max(0);
        out.low_water = i64::try_from(low_water).unwrap_or(i64::MAX);
        out.need_free = u64::try_from(need_free).unwrap_or(u64::MAX);
        let mut freed: i128 = 0;
        for c in &entries {
            let expired = now - c.mtime > self.options.ttl_s;
            let over = complete && total > low_water;
            let short = freed < need_free;
            if !(expired || over || short) {
                break;
            }
            // Unlinking is cheap, so eviction is not under the time budget. A dry run counts
            // every unlink as a success.
            let unlinked = if dry {
                out.unlink.push(c.path.clone());
                Unlink::Removed
            } else {
                self.reap_unlink(&c.path)
            };
            match unlinked {
                Unlink::Failed => continue,
                // Already gone: another reaper took it (C8). It has left the tree all the
                // same, so it counts toward both targets (contract §9.6); not counting it
                // let two racing reapers take six segments to none against a low water
                // mark of two. It isn't this reaper's eviction, so no counter or bytes.
                Unlink::Gone => {
                    out.gone += 1;
                }
                Unlink::Removed => {
                    out.reaped += 1;
                    out.bytes = out.bytes.saturating_add(c.size);
                }
            }
            total -= i128::from(c.size);
            freed += i128::from(c.size);
        }
        out.complete = complete;
        if dry {
            // Nothing written and nothing counted: the tree and the store are as they were.
            return out;
        }

        if complete {
            let bytes = u64::try_from(total.max(0)).unwrap_or(u64::MAX);
            // Python ignores a failure to write the usage file; so does this.
            let files = out
                .scanned
                .saturating_sub(out.reaped)
                .saturating_sub(out.gone);
            let _ = self.write_usage(bytes, files, now);
        } else {
            self.count(Counter::ReapPartial, 1);
        }
        // Free space changed: the next gate re-measures.
        self.free.lock().unwrap_or_else(PoisonError::into_inner).0 = 0.0;

        self.count(Counter::Reaped, out.reaped);
        self.count(Counter::ReapedBytes, out.bytes);
        out
    }

    /// Take `.reap.lock` (§9.2, C8): `O_CREAT | O_EXCL`, mode 0644, the pid as decimal text.
    fn lock(&self) -> Lock {
        let path = self.root.join(LOCK_NAME);
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o644);
        let mut file = match opts.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // A lock left by a dead process is broken, and this turn still doesn't reap.
                // `lstat`: the lock's own mtime, never a link target's.
                if let Ok(meta) = fs::symlink_metadata(&path)
                    && mtime_s(&meta).is_some_and(|m| now_s() - m > STALE_LOCK_S)
                {
                    let _ = fs::remove_file(&path);
                }
                return Lock::NotHeld;
            }
            Err(_) => return Lock::NotHeld,
        };
        let guard = LockGuard(path);
        if file
            .write_all(std::process::id().to_string().as_bytes())
            .is_err()
        {
            return Lock::Failed;
        }
        Lock::Held(guard)
    }

    /// Before each directory: the test hook, then the deadline (§9.3). True when it has passed.
    fn past(&self, dir: &Path, deadline: Option<Instant>) -> bool {
        if let Some(hook) = &self.hooks.reap_dir {
            hook(dir);
        }
        deadline.is_some_and(|d| Instant::now() > d)
    }

    /// Walk the root's segment directories, `{form}/{xx}/`, collecting candidates and sweeping
    /// orphaned temporary files. True when every directory was visited before the deadline.
    /// Nothing else is descended into: candidates are by path shape only (C6(a)), so a walk
    /// pruned to that shape loses none.
    /// `sweep` is `Some` for a dry run: orphaned `.tmp` files are listed there, not unlinked.
    fn walk(
        &self,
        now: f64,
        deadline: Option<Instant>,
        out: &mut Vec<Candidate>,
        sweep: &mut Option<Vec<PathBuf>>,
    ) -> bool {
        if self.past(&self.root, deadline) {
            return false;
        }
        for (form_dir, _) in subdirs(&self.root, is_token) {
            if self.past(&form_dir, deadline) {
                return false;
            }
            for (shard_dir, shard) in subdirs(&form_dir, is_shard) {
                if self.past(&shard_dir, deadline) {
                    return false;
                }
                self.scan(&shard_dir, &shard, now, out, sweep);
            }
        }
        true
    }

    /// One segment directory: its candidates (§9.4) and its orphaned `.tmp` files (§9.5).
    fn scan(
        &self,
        dir: &Path,
        shard: &str,
        now: f64,
        out: &mut Vec<Candidate>,
        sweep: &mut Option<Vec<PathBuf>>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let is_tmp = name.ends_with(".tmp");
            if !is_tmp && !is_segment_name(&name, shard) {
                continue;
            }
            let path = dir.join(&name);
            // lstat: never follow a symbolic link; only a regular file qualifies.
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let Some(mtime) = mtime_s(&meta) else {
                continue;
            };
            if is_tmp {
                // An orphan of a crashed writer (C6(b)). Not a segment: not scanned, not usage,
                // not shm_reaped.
                if now - mtime > TMP_ORPHAN_S {
                    match sweep {
                        Some(list) => list.push(path),
                        None => {
                            let _ = self.reap_unlink(&path);
                        }
                    }
                }
                continue;
            }
            out.push(Candidate {
                mtime,
                size: meta.len(),
                path,
            });
        }
    }

    /// Unlink a candidate or an orphan (§7: unlink, never truncate). True on success. A file
    /// already gone (another reaper got there first, C8) is `Gone`, which still counts toward the targets (§9.6); any other failure
    /// is skipped and counted `shm_reap_failed`. On Windows that includes a segment some reader
    /// has open or mapped, which can't be unlinked there (§12).
    fn reap_unlink(&self, path: &Path) -> Unlink {
        let result = match &self.hooks.reap_unlink {
            Some(hook) => hook(path),
            None => fs::remove_file(path),
        };
        match result {
            Ok(()) => Unlink::Removed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Unlink::Gone,
            Err(e) => {
                log::debug!("commonkit reaper: can't unlink {}: {e}", path.display());
                self.count(Counter::ReapFailed, 1);
                Unlink::Failed
            }
        }
    }

    /// Write `.usage.json` as `{"bytes": B, "files": N, "ts": T}` (§9.7): the temporary file,
    /// then a rename over the old one. The temporary file is created exclusively (after
    /// unlinking any left behind), so a symbolic link planted at its name is replaced, never
    /// written through.
    fn write_usage(&self, bytes: u64, files: u64, now: f64) -> io::Result<()> {
        let tmp = self.root.join(USAGE_TMP_NAME);
        let create = || OpenOptions::new().write(true).create_new(true).open(&tmp);
        let mut file = match create() {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&tmp);
                create()?
            }
            other => other?,
        };
        // `{:?}` is the shortest text that reads back as the same f64, as Python's repr.
        let ts = if now.is_finite() { now } else { 0.0 };
        let text = format!("{{\"bytes\": {bytes}, \"files\": {files}, \"ts\": {ts:?}}}");
        let written = file.write_all(text.as_bytes());
        drop(file);
        let result = written.and_then(|()| fs::rename(&tmp, self.root.join(USAGE_NAME)));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Mode, Options};

    #[test]
    fn segment_names_are_the_contracts_shape() {
        let d = "abababababababababababababababab";
        assert!(is_segment_name(&format!("{d}.o0.npy"), "ab"));
        assert!(!is_segment_name(&format!("{d}.o0.npy"), "cd"));
        assert!(!is_segment_name(&format!("{d}.npy"), "ab"));
        assert!(!is_segment_name(&format!("{d}.O0.npy"), "ab"));
        assert!(!is_segment_name(&format!("{d}.o.0.npy"), "ab"));
        assert!(!is_segment_name(&format!("{d}.o0.npy.1.2.tmp"), "ab"));
        assert!(!is_segment_name(
            &format!("{}.o0.npy", d.to_uppercase()),
            "AB"
        ));
        assert!(!is_segment_name(&format!("{d}a.o0.npy"), "ab"));
        assert!(!is_segment_name(&format!("{}.o0.npy", &d[1..]), "ba"));
        assert!(!is_segment_name("é", "ab"));
        assert!(is_shard("0f") && !is_shard("0F") && !is_shard("0") && !is_shard("0fa"));
        assert!(is_token("pil") && !is_token("") && !is_token(".pinned") && !is_token("Pil"));
    }

    #[cfg(unix)]
    #[test]
    fn suspicious_roots() {
        assert!(suspicious(Path::new("")));
        assert!(suspicious(Path::new("/")));
        assert!(suspicious(Path::new("/tmp")));
        assert!(!suspicious(Path::new("/tmp/store")));
    }

    /// Python's `test_reaper_refuses_a_suspicious_root` exactly: an enabled store whose root is
    /// then `/`.
    #[test]
    fn a_store_rooted_at_slash_refuses_to_reap() {
        let dir = tempfile::tempdir().unwrap();
        let options = Options {
            mode: Mode::ReadWrite,
            min_free_bytes: 1,
            ..Options::default()
        };
        let mut s = Store::new(dir.path().join("s"), options);
        assert!(!s.is_disabled());
        s.root = PathBuf::from("/");
        assert_eq!(s.reap(Duration::from_secs(1)).reaped, 0);
        assert_eq!(s.drain_stats().get("shm_reap_refused"), 1);
    }
}

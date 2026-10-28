//! The few calls that differ by platform: free space, durable sync, the LRU touch, and a
//! per-thread tag for unique file names. POSIX calls go through `rustix` (safe wrappers), so no
//! `unsafe` lives here. Windows is supported (contract §12) and must keep building: every
//! POSIX-only call has a `cfg` branch for it.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Free bytes for an unprivileged writer on the filesystem holding `root`: `f_bavail × f_frsize`
/// from `statvfs` (contract §9.9). `None` if it can't be had; the caller reads that as 0.
#[cfg(unix)]
pub(crate) fn free_bytes(root: &Path) -> Option<u64> {
    let st = rustix::fs::statvfs(root).ok()?;
    // Saturating: a filesystem can't really report more than 2^64 bytes, but a hostile or
    // broken one must not make this panic.
    Some(st.f_bavail.saturating_mul(st.f_frsize))
}

/// Windows has no `statvfs`; how it measures is not yet contract (§12). This is what Python's
/// `shutil.disk_usage(root).free` reports there: bytes available to the caller.
#[cfg(windows)]
pub(crate) fn free_bytes(root: &Path) -> Option<u64> {
    fs4::available_space(root).ok()
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn free_bytes(_root: &Path) -> Option<u64> {
    None
}

/// Push `file`'s data to storage (contract §6.5, step 3).
///
/// On macOS `fsync` only hands the data to the drive, which may keep it in its own cache across
/// a power loss; `F_FULLFSYNC` flushes that cache too, and is what Python does. A filesystem that
/// refuses `F_FULLFSYNC` still gets a plain `fsync`, as in Python. (std's `sync_all` uses
/// `F_FULLFSYNC` on Apple targets too, but without that fallback.)
#[cfg(target_vendor = "apple")]
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    if rustix::fs::fcntl_fullfsync(file).is_ok() {
        return Ok(());
    }
    rustix::fs::fsync(file).map_err(io::Error::from)
}

#[cfg(not(target_vendor = "apple"))]
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

/// Make a rename in directory `dir` durable (contract §6.5, step 5): sync a descriptor opened
/// on the directory. POSIX only: Windows can't open a directory as a file to sync it, so there
/// this is a no-op, as in Python, rather than a publish that always fails.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    sync_file(&File::open(dir)?)
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Set `path`'s atime and mtime to now (contract §8.2), as Python's `os.utime(path, None)`:
/// `UTIME_NOW`, which a process that may write the file can set even if it doesn't own it.
#[cfg(unix)]
pub(crate) fn touch_now(path: &Path) -> io::Result<()> {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, UTIME_NOW, utimensat};
    let now = Timespec {
        tv_sec: 0,
        tv_nsec: UTIME_NOW,
    };
    let times = Timestamps {
        last_access: now,
        last_modification: now,
    };
    utimensat(CWD, path, &times, AtFlags::empty()).map_err(io::Error::from)
}

/// Windows: open with `FILE_WRITE_ATTRIBUTES` only. That is not opening the segment for
/// writing its data, which the mutation rule forbids (contract §7).
#[cfg(windows)]
pub(crate) fn touch_now(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .open(path)?;
    let now = std::time::SystemTime::now();
    file.set_times(
        std::fs::FileTimes::new()
            .set_accessed(now)
            .set_modified(now),
    )
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn touch_now(_path: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// A number unique to the calling thread within this process, for the names that must be
/// unique per process and thread (`.writable.{pid}.{tid}`, `{segment}.{pid}.{tid}.tmp`). std's
/// `ThreadId` has no stable integer, so threads are numbered as they first ask.
pub(crate) fn thread_tag() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static TAG: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TAG.with(|t| *t)
}

/// "The volume is full" (ENOSPC, EDQUOT, or Windows' disk-full errors): a decline, not a
/// failure (contract §6.5).
pub(crate) fn is_full(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    )
}

//! The crate's only `unsafe`: mapping a segment read-only (plan 7.2). Compiled only with the
//! default-on `mmap` feature; without it no `unsafe` exists in the crate and attach copies.

#![allow(unsafe_code)]

use std::fs::File;
use std::io;

use memmap2::Mmap;

/// A read-only mapping of a whole file. The bytes are only ever exposed as `&[u8]`.
#[derive(Debug)]
pub(crate) struct Map(Mmap);

impl Map {
    /// Map all of `file` read-only.
    pub(crate) fn of(file: &File) -> io::Result<Map> {
        // SAFETY: `Mmap::map` is unsafe because the bytes behind a mapping change, or the process
        // takes SIGBUS, if the file is written in place or truncated while mapped. The contract's
        // mutation rule (docs/contract.md §7, plan §6.5) forbids both: a conforming process
        // changes a segment path only by renaming a whole new file over it, by unlinking it, or
        // by setting its mtime; none truncates a segment, writes into it, or opens it for
        // writing. On POSIX a rename-over or an unlink leaves an existing mapping on the old
        // inode, valid until it is unmapped (tests/store.rs checks both). The mapping is
        // read-only (`Mmap`, not `MmapMut`) and this module never hands it out mutably. What the
        // contract can't remove (a non-conforming tool truncating a segment, the volume being
        // unmounted) is listed in plan §7.3.
        unsafe { Mmap::map(file) }.map(Map)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

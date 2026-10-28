# pycommonkit

Named shared-memory segments for numpy arrays. One process decodes and
publishes; every other process attaches the same physical pages.

Installed as **`pycommonkit`**, imported as **`commonkit`** (the PyPI name `commonkit` belongs
to an unrelated project). A Rust crate, `commonkit`, implementing the same on-disk contract is
in this repository under `rust/` (`docs/rust-crate-plan.md`): keys, segments, the store and the
reaper are done; cross-language conformance tests come next.

The name is for Fortran's `COMMON` block: a named region several separately
compiled programs agree to share. That is exactly the contract here — the name
is the whole interface, and the segment outlives whoever wrote it.

## The contract

`docs/contract.md` (version 1) is the authority on the on-disk format: keys, layout, the `.npy`
segment, publishing, the write-once rules, touch, the reaper, policy and counters. Python is the
reference implementation and `conformance/vectors/` decides any disagreement
(`conformance/generate.py` regenerates the fixtures byte for byte). Both the Python package and
the Rust crate pass every vector.

## Status

See `docs/STATUS.md`. Nothing is published to PyPI or crates.io yet.

## What it is

* `SharedArrayStore` — publish/attach `.npy` segments under a root directory,
  with admission gates, a TTL reaper and a byte budget.
* `KeyScheme` — the addressing vocabulary for one *domain*. A scheme fixes the
  domain, a version, the legal forms and the legal variants, and turns
  `(digest, variant, form)` into both a key and a relative path.
* `shared_store()` — the ONE store per host that every user shares. Its byte budget, TTL,
  free-space floor, fsync and republish verification come only from the tree's `.policy.json`
  (which wins), then `COMMONKIT_STORE_*` environment variables, then the defaults (macOS:
  `~/Library/Caches/commonkit/store`, 2 GB, 900 s, 8 GB free), because the reaping process applies
  its own policy to the whole tree; the root comes from the environment or the default. A caller
  chooses only `mode` and the smallest array worth storing.
* `TieredPixelCache` — an in-process L1 memo in front of a store, so a loop
  that looks up the same digest repeatedly pays one attach rather than one per
  call.

## A cache, not a store of record

**A missing segment is never an error, only a slower path.** The store promises
that a key which is present names the right bytes. It never promises that a key
is present. So every caller must be able to compute the value again when
`attach` returns `None`, and must treat a `publish` that returns `False` as
normal.

A segment can be absent at any moment, for reasons the caller doesn't control:

* **Its publish was declined.** Under pressure the store refuses rather than
  blocks: the array was smaller than worth storing or larger than allowed, the
  volume was below its free-space floor, or the tree was over its byte budget.
  A decline is how the store survives back-pressure, not a fault.
* **The reaper removed it.** Segments are evicted when they expire, least
  recently used first to get back under the byte budget, and to restore the
  free-space floor. The reaper doesn't know who might still want a segment, and
  doesn't ask: no reference counts, no coordination between processes.
* **It was torn.** A file shorter than its header promises, after a crash with
  `fsync` off (the default) or damage from outside the contract, is a miss.
* **The store is disabled.** The mode is `off`, or the root can't be created or
  written: every attach misses and every publish declines.
* **The tree is gone.** On Linux it lives in `/dev/shm` and does not survive a
  reboot; any root can be cleared; another machine has its own tree.

That one rule is what makes the rest cheap. The reaper can unlink freely,
publish never blocks, writes need no durability, and a process that fails
costs the others only time: the next reader recomputes.

**What does raise.** Exactly two things, both bugs at the call site rather than
conditions of the store: building a key that isn't valid for its scheme
(`ValueError`), and publishing a *different* array under a key that already
holds one (`KeyCollision`, a `ValueError` subclass), which means a key scheme or
a digest is broken. Everything else (an absent root, a full filesystem, a
reaped or torn segment, a hostile `.usage.json` or `.policy.json`) returns
`None` or `False` and ticks a counter. The constructor never raises: an
unusable root leaves the store `disabled` and every method a no-op.

**A value that can't be recomputed doesn't belong here alone.** Keep its
durable copy somewhere you own and use the store to share it fast. (A pinned
subtree the reaper skips is a possible later addition; it isn't built.)

**Callers never need `.usage.json`.** It is the reaper's record of how much the
tree holds, read only by the byte-budget gate. When it is missing or stale,
usage is unknown, that gate passes, and the free-space floor still applies;
the next complete reap writes a fresh one.

## Keys

    {domain}:{version}:{form}:{digest}:{variant}

and the path it names:

    {root}/{form}/{digest[:2]}/{digest}.{variant}.npy

Two-level sharding, so no directory holds a million entries. The `variant` axis
is whatever distinguishes two renderings of the same source: for images it is
orientation (`o0`, `o1`, `oa`), for audio it would be sample rate (`sr16000`).
Schemes are registered by domain so `parse()` and `relpath()` can dispatch on a
key alone, which is what lets one store hold several domains.

## Tests

    uv run --extra dev pytest -q          # includes the conformance vectors

The suite addresses audio and video rather than images, to show the package is
not pixel-specific. Two modules run end to end against real media and skip
themselves when their tool is not on `PATH`:

* `tests/test_sox.py` — `sox` synthesizes a sine wave and dumps it as raw
  int16 (and float32) samples. The samples are published, attached back as a
  read-only `np.memmap`, attached again from a separate process, and stored at
  two sample rates as two variants (`sr16000`, `sr44100`) of one digest.
* `tests/test_ffmpeg.py` — `ffmpeg` renders its `testsrc` pattern to
  `rawvideo` frames, stored as one `(T, H, W, C)` array. The same checks, with
  the frame rate as the variant (`fr10`, `fr25`), plus a clip over
  `max_array_bytes` being declined whole rather than truncated.

Both use their tool's built-in generators, so no media files are checked in.
Both also exercise per-form dtypes: `dtypes` maps a *form* to the dtype it
requires (`{"wav16k": "int16"}`), and a float dump is refused under the
integer form and accepted under the float one. A form's name is only a label —
the store enforces a dtype only for forms listed in `dtypes`.

## Origin

Lifted on 2026-10-27 from `seantrue/commonkit` (`main` at `4dbfc7d`, plus the contract and
vectors from its `rust-crate-plan` branch at `2b6bcf9` and after), with a fresh history. That
package was extracted from the pixel-cache layer of a private image-corpus
application, where it ran in production first. The measurements in the module
docstrings were taken there; the framework-coupled half (the settings wiring
and the decode entry point) deliberately stayed behind.

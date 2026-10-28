---
title: "commonkit: a Rust crate beside the Python package"
subtitle: "Plan only: no code has been written"
date: 2026-10-27
---

# commonkit: a Rust crate beside the Python package

The maintainer asked: "Make a plan to integrate a crate into the commonkit package."

This is a plan. No code was written and no repository was changed to produce it. Everything below was checked against the source on 2026-10-27:

- commonkit `4dbfc7d`;
- ImpCraft `crates/shm` and `python/src/impcraft_engine/publish.py`;
- PlumCraft `23c58e2`: `crates/store`, `crates/engine/src/data.rs`, `python/src/plumcraft_engine/data.py`, `docs/plan.md` and research notes 11 and 13;
- the live shared tree on thresher, `/Volumes/CommonkitStore/store`.

## 1. Summary

**What to build.** `commonkit` gets a pure Rust crate, also named `commonkit`. It implements the same on-disk contract as the Python package, so a Rust process can publish, attach and reap as fully as a Python one. It does not wrap Python, and Python does not wrap it. The Python package stays pure Python with numpy as its only dependency, and stays importable on 3.11.

**Where it lives.** A Cargo workspace under `rust/` in the commonkit repository, next to the uv-built Python package. Neither build sees the other's files. A scratch build of the renamed package confirmed that `uv build` leaves `rust/` out of both the sdist and the wheel.

**What ties the two together.** Two things:

- a written contract, `docs/contract.md`, which covers the keys, layout, `.npy` header, publish, admission, touch, reaper, `.usage.json` and environment policy;
- a cross-language conformance suite: a shared vector set, plus tests in which Python publishes and Rust reads, Rust publishes and Python attaches, and both reapers run on one tree.

**What it replaces.** Two partial Rust copies of the contract have drifted from Python and from each other:

- ImpCraft's `impcraft-shm`, an mmap reader;
- PlumCraft's `plumcraft-store`, a copying reader and a writer with no gates.

Section 3 lists 16 inconsistencies between them. Each copy keeps only what is its own: ImpCraft keeps its viewer helpers, and PlumCraft keeps its handles, locators and same-host proof.

**Already decided by the maintainer** (section 2):

- the Python distribution is renamed `pycommonkit`, and the import stays `import commonkit`;
- the crate is `commonkit` on crates.io;
- both are MIT;
- Python stays at 3.11+.

**Estimate.** 17 to 21 days in ten steps (section 12). The exit criterion for the crate is the conformance suite. PlumCraft's source-change table estimated 2 to 3 days, but that row covered a writer only. It had no contract, no reader parity, no conformance tests and no migration.

## 2. Decided by the maintainer

| Item | Decision | Quote |
|---|---|---|
| Python distribution name | `pycommonkit` on PyPI. The import name stays `commonkit`, so no consumer's code changes. Only its dependency line changes. | "Let's go for a pypi name of pycommonkit and the crate of commonkit." |
| Rust crate name | `commonkit` on crates.io | (same) |
| Licence | MIT for both. The crate declares `license = "MIT"` and shares the repository's existing `LICENSE`. `pycommonkit` moves to the SPDX form, `license = "MIT"` with `license-files = ["LICENSE"]`. | "Set license to MIT, and python version to 3.11+" |
| Python version | `requires-python = ">=3.11"`, as today. Any Python-facing piece, including a future PyO3 module (C1), must support 3.11. | (same) |
| Write-once contract rules | The contract states the write-once rules plainly (section 6, "The write-once rules"): a key names one immutable content; every writer uses temp-then-rename; the reaper unlinks and never truncates; a file shorter than its header is a miss. | "Add the contract rules." |
| fsync | A policy option, `COMMONKIT_STORE_FSYNC`, **off by default** (§6.4, §6.8). Off, a crash can leave a short file under its final name, which readers treat as a torn miss. On, the writer syncs the file before the rename and the directory after it. | "fsync should be a config option, but default to no." |

The names were free on 2026-10-27. PyPI `pycommonkit` and crates.io `commonkit` both returned 404. PyPI `commonkit` belongs to an unrelated project: version 1.0.8, "A library for handling common stuff in Python", by another author. That clash is why the distribution is renamed.

Two facts about the rename and licence were checked with uv 0.12.5 on a scratch copy, without touching the repository:

- **`module-name` is required.** uv_build infers the module from the project name, so renaming the project alone fails: "Expected a Python module at: src/pycommonkit/__init__.py". The fix is to add `[tool.uv.build-backend] module-name = "commonkit"`. With that line, the wheel holds `commonkit/` under `pycommonkit-0.1.0.dist-info`.
- **uv_build supports PEP 639.** With `license = "MIT"` and `license-files = ["LICENSE"]`, it writes `Metadata-Version: 2.4`, `License-Expression: MIT` and `License-File: LICENSE`, so PyPI will show MIT.

## 3. What exists today, and where it disagrees

### 3.1 The three implementations

| | commonkit (Python) | `impcraft-shm` (Rust) | `plumcraft-store` (Rust) |
|---|---|---|---|
| Role | Reference: publish, attach, reap, policy | Reader only (UI viewer) | Copying reader, minimal writer, handles, locators, proof |
| Size | 469 lines in `store.py`; 72 tests | ~1,200 lines; 18 tests | 435 lines; 7 tests |
| Key check | Registered scheme: domain, version, form, variant, digest | Grammar only (tokens, 32 lowercase hex) | Only `plum:v1:{plumarr,plumraw}:<md5>:c` |
| `.npy` read | numpy: every version, every non-object dtype, Fortran order, trailing bytes tolerated | Versions 1 to 3; `b`, `i`, `u` and `f` dtypes; Fortran order; trailing bytes are an **error** | Version 1 only, C order only, substring parse; **no payload-length check** |
| Miss vs error | Every failure is `None` or `False` plus a counter | Absent or torn is `Ok(None)`; a bad key, bad header or extra bytes is `Err` | `None` throughout |
| mmap | `np.load(mmap_mode="r")`, or `read` | `memmap2`, the crate's only `unsafe` | None (PlumCraft forbids `unsafe`) |
| Touch for LRU | Yes, at most once per 60 s | **No** | **No** |
| Writer | tmp + `os.replace`; gates for minimum and maximum size, free space, budget and dtype | None (only `write_header_v1` for tests) | tmp + `rename` + `sync_data`; **no gates**; skips an existing file **without touching it** |
| Reaper | Lock, TTL, LRU toward two targets, `.usage.json` | None | None ("Python publishers reap") |
| Root default | `COMMONKIT_STORE_ROOT`; macOS `~/Library/Caches/commonkit/store`; otherwise `/dev/shm/commonkit` | Same, plus a Windows branch; `/` when `HOME` is unset | Same; `/tmp` when `HOME` is unset |
| Other policy env | All `COMMONKIT_STORE_*` | Root only | Root only |

### 3.2 The inconsistencies the crate should end

1. **Rust readers never refresh LRU.**
   - Python's `attach` touches mtime. `impcraft-shm` and `plumcraft-store` never do.
   - So a segment the UI is looking at ages exactly like one nobody reads, and the reaper evicts it first.
   - ImpCraft's engine hides part of this: `publish.py` calls `attach` before republishing, which touches.
2. **Rust-only publishing is never reaped.**
   - `plumcraft-store` writes but never reaps.
   - A tree written only by Rust engines repeats the 2026-10-16 failure, where the store sat full for 5.7 days because nothing ran the reaper.
   - It also applies no budget and no free-space floor, which PlumCraft's plan records under "D1 as built".
3. **A Rust writer and a Rust reader disagree on size.**
   - `plumcraft-store` publishes any size.
   - Its own reader refuses anything over `MAX_READ` (128 MiB plus 4 KiB).
   - Python would decline to publish such an array at all (`MAX_ARRAY_BYTES`).
4. **A torn file can return short data.**
   - A torn file can't come from a conforming writer in normal operation: segments are write-once and appear by an atomic rename. It can only come from outside the contract: a crash before unsynced data reached disk (§6.4, fsync off), a non-conforming tool, or disk corruption.
   - `plumcraft-store::read_npy` never compares the payload length with the shape.
   - `read_key` can therefore return a short blob for such a file. `resolve`, PlumCraft's normal path, verifies the handle, so it returns a miss.
5. **Trailing bytes.** numpy loads a file that has bytes after its payload, in both normal and mmap mode (checked with numpy 2.5.4). `impcraft-shm` returns `SizeMismatch` instead, so Rust misses a segment Python hits.
6. **Errors versus misses.** `impcraft-shm` returns `Err` for a malformed key or header. The Python invariant is that these are a counted miss.
7. **Dtype coverage.**
   - Python publishes and reads every non-object dtype: `c8`, `M8`, `U` and structured types included.
   - `impcraft-shm` rejects these as errors.
   - `plumcraft-store` passes the descriptor through as a string, but rejects Fortran order.
   - PlumCraft registers `plumraw` as `uint8` in Python, and its Rust writer doesn't enforce it.
8. **Headers are not byte-identical to numpy's.**
   - numpy pads the header with `GROWTH_AXIS_MAX_DIGITS` spaces (21 minus the digits of the growth axis) before aligning to 64 bytes.
   - Both Rust header writers leave out that padding.
   - For most shapes the results agree by luck. For a 20-dimensional shape numpy writes a 192-byte header and the Rust writers write 128. Both files are valid.
   - `impcraft-shm`'s test "our v1 header is byte-identical to numpy's" passes only for the shape it tests.
9. **Temporary files.**
   - Python names them `{digest}.{variant}.npy.{pid}.{tid}.tmp`.
   - PlumCraft names them `.{digest}.{pid}.{n}.tmp`.
   - Nobody reaps an orphan left by a crashed writer. The reaper looks only at `*.npy`. There are none on thresher today.
10. **Republishing an existing key.** Python rewrites it, which also refreshes mtime. PlumCraft's Rust and Python both skip the write and do not touch, so a hot blob ages out.
11. **The reaper's documentation and code disagree.**
    - `CLAUDE.md` and the `reap()` docstring say the reaper unlinks only files "whose name matches the segment shape".
    - The code checks only the `.npy` suffix, so any regular `*.npy` anywhere under the root is evictable.
12. **Python itself breaks the never-raise invariant twice.**
    - `publish` of an object array raises `ValueError` from `np.save(..., allow_pickle=False)`. Only `OSError` is caught.
    - On Windows, `os.statvfs` doesn't exist, so `_free_bytes` raises `AttributeError`. Its default root there would also be `/dev/shm/commonkit`.
13. **Default roots disagree on unusual hosts.** The two Rust readers fall back to `/` and `/tmp` when `HOME` is unset, and only `impcraft-shm` has a Windows branch. Python has no Windows default at all.
14. **PlumCraft keeps private subtrees in the shared root.**
    - `.hello/` holds same-host nonce files. 100 have accumulated on thresher and are never cleaned.
    - `.sources/` holds uploaded sources. They are never reaped and never counted against the byte budget.
    - Neither is part of the contract.
15. **PlumCraft's plan contradicts itself.**
    - Section 2.1 still lists `plumcraft-shm`, an mmap reader that would be "the only crate allowed `unsafe`", and says the UI "never writes commonkit (no Rust commonkit writer is needed)".
    - "D1 as built" has a Rust writer in `plumcraft-store`.
    - Research note 13 says there is no `plumcraft-shm` and that `unsafe` is forbidden.
    - The plan's own to-do list says to correct the 2.1 row.
16. **Policy depends on how a process was launched.**
    - On thresher, the tree-wide policy is set only in `~/.bashrc`: `ROOT=/Volumes/CommonkitStore/store`, `MAX_GB=7`, `MIN_FREE_GB=1`, `TTL_S=604800`.
    - A process launched from Finder, such as the ImpCraft app bundle (ImpCraft's T17), sees none of it.
    - Such a process gets a second tree with its own budget: the macOS default root, 2 GB and 900 s.
    - A process that gets `ROOT` but not the other variables would reap the 9.3 GiB volume with the default `MIN_FREE_GB=8`. That is a floor the volume can never meet, so it would evict nearly everything.

## 4. Decision C1: what the crate is

| Option | What it gives | Against commonkit's rules |
|---|---|---|
| **A. A pure Rust crate implementing the contract** | Rust processes become first-class publishers, readers and reapers with no Python in the process. ImpCraft's UI and PlumCraft's host and engines are Rust binaries without Python. | Python is untouched: numpy only, 3.11, a pure-Python wheel. |
| B. PyO3 bindings: Rust core, Python on top | One implementation | Turns `pycommonkit` into a compiled extension: a wheel per platform and Python version, maturin instead of uv_build, a toolchain to build from source. The 3.11 model venvs would depend on a native wheel. It doesn't help Rust callers that lack Python. |
| C. Both: a Rust core, plus PyO3 as the Python implementation | One implementation everywhere | All of B's packaging costs. The Python package loses the property its `CLAUDE.md` defends: nothing but numpy. |
| D. A Rust crate now, an optional PyO3 accelerator later | A now. Room for a separate `pycommonkit-native` wheel if a measurement ever justifies it | None now |

**Recommendation: A, keeping D's door open.**

- The consumers that need Rust are Rust binaries without Python, and the Python consumers are 3.11 model venvs that must stay pure.
- Two implementations of one contract cost conformance tests. That cost is real, but small next to the drift that exists now, and the tests are what end the drift.
- If PyO3 is ever added, it is a separate optional distribution that supports 3.11. `pycommonkit` never requires it.

## 5. Decision C2: repository layout and builds

```text
commonkit/
  pyproject.toml          pycommonkit (uv_build), unchanged apart from the rename and licence fields
  src/commonkit/          the Python package
  tests/                  pytest, plus test_conformance.py (new)
  conformance/            the shared vector set (new): JSON plus small .npy fixtures
  docs/contract.md        the on-disk contract (new)
  docs/rust-crate-plan.md this plan
  rust/
    Cargo.toml            [workspace]; edition 2024; rust-version 1.95 (as ImpCraft and PlumCraft)
    Cargo.lock            committed
    commonkit/            the crate (license = "MIT", license-file = "../../LICENSE" or a copy)
    conform/              ck-conform: a CLI the Python tests drive (publish = false)
```

**Why `rust/` and not a root `Cargo.toml`.** The root stays a plain uv project, and Cargo's `target/` stays inside `rust/`.

**Git dependencies still work.** A git dependency finds a crate by package name anywhere in the repository, so consumers write this:

```toml
commonkit = { git = "https://github.com/seantrue/commonkit", rev = "<sha>" }
```

**The two builds stay independent.**

- `uv build` packages only `src/commonkit`, `pyproject.toml`, `README.md` and `LICENSE`. A scratch build with a `rust/` directory present left it out of both the sdist and the wheel.
- `cargo` never reads `pyproject.toml`.
- The only shared inputs are `conformance/` and the `LICENSE` file.
- A crate packaged for crates.io needs its licence file inside the package. Either `license-file` points at the root copy, or step 2 adds a copy, whichever `cargo package` accepts.

**CI.** commonkit has no CI today. Add a GitHub Actions workflow with three jobs on a matrix of `macos-latest` and `ubuntu-latest`:

| Job | What it runs |
|---|---|
| `python` | `uv run --extra dev pytest -q` and `ruff check .`, on Python 3.11 and 3.13 |
| `rust` | `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo test --no-default-features`, on stable and 1.95 |
| `conformance` | Builds `ck-conform`, then runs `pytest tests/test_conformance.py` and the Rust tests that call Python |

Linux runs on `/dev/shm` and macOS on an APFS temporary directory, so both statvfs behaviours are covered.

**Skips.** Cross-language tests skip themselves when the other toolchain is missing, as `test_sox.py` and `test_ffmpeg.py` do today. A Python-only developer still has a green suite.

## 6. Decision C3: the contract, made explicit

`docs/contract.md` becomes the authority, version 1. The Python package is the reference implementation, and an implementation conforms when it passes the conformance suite. The contract describes today's Python behaviour, plus the small tightenings in C6 that both implementations adopt together.

### The write-once rules

The contract opens with these rules, which the sections below make precise:

1. **Write once.** The store is write-once by key. A key names one immutable content, and a publisher must never publish different bytes under a key that already exists. Republishing the same array is a touch, never a write (§6.4 item 4). A different array under an existing key is a **key collision**, a major system error that fails loudly. A change of content is a new key, through a new digest or a new scheme `version`.
2. **Temp-then-rename.** Every writer writes the whole segment to a private temporary file in the segment's own directory, then renames it over the final path (§6.4). A segment never exists under its final name in part.
3. **Unlink, never truncate.** The reaper removes a segment only by unlinking it. No conforming process truncates a segment, writes into it in place, or opens it for writing (§6.5).
4. **Short is a miss.** A reader treats a file whose size is less than its header promises, or whose header doesn't parse, as a torn miss and counts `shm_torn` (§6.3). This is a safety net for faults outside the contract, not part of normal operation.
5. **Durability is policy.** fsync is a policy option, off by default (§6.4, §6.8). With it off, rule 4 covers a crash that leaves a short file.

### 6.1 Keys and schemes

- **Grammar:** `{domain}:{version}:{form}:{digest}:{variant}`.
  - Domain, version, form and variant match `[a-z0-9]+`.
  - The digest matches `[0-9a-f]{32}`.
  - A scheme folds an upper-case digest to lower case when it **builds** a key. A key that is **parsed** must already be lower case.
- **Schemes:** a scheme is a domain, a version, a non-empty tuple of forms, variants (a closed tuple or a regular expression) and an optional form-to-dtype map.
  - `version` is the decode contract: bump it to orphan old segments.
  - Form names are global across domains in a shared tree.
- **Registry:** one scheme per domain per process.
  - Registering an identical scheme again is idempotent. A disagreeing one is a loud error.
  - Building or registering a key is the **one loud path** in both languages: Python raises `ValueError`, and Rust returns `Err(KeyError)`.
- **Readers versus publishers (C4):** a publisher must validate a key against a registered scheme. A reader may attach any key that matches the grammar.
  - The grammar alone already guarantees the path stays under the root, and a key's path doesn't depend on its scheme.
  - Python's stricter reader still conforms.
  - Rust readers such as the ImpCraft UI then need not copy the engine's scheme definitions, which would be another place to drift.
- **Digest algorithm:** the contract names only 32 hex digits. Which algorithm produced them is the scheme's business, which leaves room for BLAKE3 (section 10).

### 6.2 Layout

| Path under the root | Meaning | Who touches it |
|---|---|---|
| `{form}/{digest[:2]}/{digest}.{variant}.npy` | A segment | Publish (rename-over), touch, reap (unlink) |
| `{same dir}/*.tmp` | A segment being written | Its writer; the reaper sweeps orphans older than 1 h (C6) |
| `.reap.lock` | The reaper's lock | Whichever reaper holds it |
| `.usage.json`, `.usage.json.tmp` | The last complete walk | The reaper |
| `.writable.{pid}.{tid}` | The constructor's writability probe | Its process, briefly |
| `.policy.json` | Tree-wide policy (C7, proposed) | The maintainer, or a setup command |
| Other `.`-prefixed names | Reserved. Not segments, not counted, never reaped | Their owners (PlumCraft's `.hello/` and `.sources/` today; see section 9) |

A form can never collide with a reserved name, because forms can't start with `.`.

### 6.3 The `.npy` segment

- **Format.** numpy's `.npy` format, version 1.0, as `np.save(fh, arr, allow_pickle=False)` writes it.
  - The magic is followed by a little-endian `u16` header length and a Python dict literal with `descr`, `fortran_order` and `shape`.
  - The header is padded with spaces, including numpy's growth-axis padding, and ends in `\n`. The data then starts on a 64-byte boundary.
  - Version 2.0 or 3.0 is written only when numpy would write it.
- **Rust writes numpy's exact bytes.** The Rust writer must produce numpy's header **byte for byte** for the same dtype, order and shape. The vectors pin this for shapes where the growth padding changes the length.
- **Readers** accept versions 1 to 3, both byte orders, `fortran_order` either way, and trailing bytes after the payload (numpy does).
  - A file shorter than its header promises is **torn**, which is a miss.
  - An unparseable header is also a miss, counted as `shm_torn`.
- **Dtypes.**
  - A publisher never writes object dtypes.
  - A reader exposes every descriptor. Typed views exist for `b1`, `i1` to `i8`, `u1` to `u8`, `f2`, `f4` and `f8`.
  - Other descriptors, such as complex, datetime, `U` and structured types, are available as raw bytes plus the descriptor string.
- **Dtype admission.** When a form declares a dtype, a publish whose array is anything else is declined (`shm_declined_dtype`). Comparison is by numpy dtype equality, so `>f4` is not `float32`.

### 6.4 Publishing

1. **Gates, in this order, each with its counter:**
   - mode is not `readwrite`, or the store is disabled: `shm_declined_mode`;
   - `nbytes < min_array_bytes`: `shm_declined_small`;
   - `nbytes > max_array_bytes`: `shm_declined_size`;
   - free bytes below `min_free_bytes`: `shm_declined_space`, then a rate-limited reap. Free bytes are `f_bavail × f_frsize`, cached for 1 s.
   - `.usage.json` is fresh and `used + nbytes > max_bytes`: `shm_declined_budget`, then a rate-limited reap. The file is fresh when its mtime is within twice `reap_interval_s`.
   - dtype admission;
   - key: `shm_bad_key`.
2. **Write.** Write `{path}.{pid}.{thread}.tmp` in the segment's own directory, then rename it over `path`, which is atomic within one filesystem.
   - ENOSPC or EDQUOT counts `shm_declined_space`. Any other error counts `shm_publish_failed`. Either way the temporary file is removed.
   - **fsync is a policy option, `fsync`, off by default** (`COMMONKIT_STORE_FSYNC`, §6.8). With it on, the writer syncs the temporary file's data before the rename and syncs the directory after it, so a published segment survives a crash. With it off, which is the default, segments are a cache, and a short file after a crash is a torn miss (rule 4).
   - A failed sync counts `shm_publish_failed` and removes the temporary file, like any other write error.
   - Both languages implement both settings, and the conformance suite runs both. `plumcraft-store`'s writer syncs unconditionally today; it adopts the policy when it migrates (section 9.2).
3. **After a successful publish:** count `shm_publish` and `shm_bytes_published`, then take a rate-limited reap turn.
4. **An existing key** (C15, decided) is never rewritten. Publish reads the existing header first:
   - **Torn** (shorter than its header promises, or an unparseable header): it is damage from outside the contract. Publish writes the new segment over it, temp-then-rename, and counts `shm_torn_replaced`.
   - **Whole, with the same dtype, shape and order:** a republish. Publish writes nothing, touches the segment (mtime to now, so the LRU sees it used), and returns success.
   - **Whole, with a different dtype, shape or order:** a **key collision**, a major system error: a key scheme or digest is broken. Publish leaves the existing file untouched, counts `shm_key_collision`, logs at error level, and fails loudly: Python raises `KeyCollision` (a `ValueError` subclass), Rust returns `Err(KeyCollision)`. This is the second loud path, beside building a bad key (§6.1); the never-raise invariant covers misses and declines, not a broken key scheme.
   - **`verify_republish`** (policy option, `COMMONKIT_STORE_VERIFY_REPUBLISH`, **off by default, on in the conformance suite and CI**): a republish whose header matches also compares the payload bytes with the existing file, and a difference is a key collision. Off, a collision with the same dtype and shape goes unseen; on, every republish reads the existing file back.

### 6.5 The mutation rule

This is what makes mmap safe.

A published segment path is changed only by:

- **rename-over**, when it is published;
- **unlink**, by the reaper;
- **mtime update**, by a touch.

No conforming process truncates a segment, writes into it in place, or opens it for writing. Unlinking or replacing a file that another process has mapped is safe on POSIX: the mapping keeps the old inode. Truncating a mapped file is not safe: a reader touching a page past the new end gets SIGBUS, which kills the process and cannot be caught in Rust. So "unlink, never truncate" is a contract rule, not an implementation detail. Python's existing torn-file test truncates a file that is not mapped, and stays a test-only exception.

### 6.6 Touch (LRU)

On every hit, a reader sets mtime to now if it is more than 60 s old. `TOUCH_INTERVAL_S` limits this to one metadata write per minute per segment. atime is never used: ZFS served from ARC never updates it.

A touch that fails, for example because the file is owned by another user, is ignored. **Rust readers must touch**; that is inconsistency 1. A copy-mode read counts as a hit and touches too.

### 6.7 The reaper

- **Refusals.**
  - It refuses to run when the root is empty, `/`, or has fewer than two separators: `shm_reap_refused`.
  - A disabled store doesn't reap.
- **Lock.**
  - It creates `.reap.lock` with `O_CREAT|O_EXCL` and mode 0644, then writes the pid into it.
  - If the lock exists and its mtime is more than 300 s old, it unlinks the lock and returns **without reaping** on this turn.
  - Every exit releases the lock.
- **Walk.**
  - It walks the whole root without following symlinks, and `lstat`s each candidate.
  - Candidates are regular files matching the segment shape `{form}/{xx}/{32hex}.{variant}.npy` (C6 tightens today's bare `*.npy` check to this).
  - The deadline (`budget_s`) is checked **before each directory**. A walk cut short is `complete: false`.
- **Order.** Sort by `(mtime, size, path)`, oldest first.
- **Eviction.** Walk the sorted list and unlink while any of these holds:
  - the segment is expired (`now - mtime > ttl_s`);
  - the walk was complete and the total is over the low water mark, `0.8 × max_bytes`;
  - bytes freed so far are below `1.25 × min_free_bytes - free_now`, with free space measured fresh.

  Stop at the first segment for which none holds. The time budget limits the walk, never the eviction.
- **Output.**
  - Count `shm_reaped` and `shm_reaped_bytes`.
  - Write `.usage.json` with keys `bytes` (an int), `files` (an int) and `ts` (a float, Unix seconds), via `.usage.json.tmp` and a rename, **only after a complete walk**. A partial walk counts `shm_reap_partial` instead.
- **Rate limit.** `_maybe_reap` runs at most once per `reap_interval_s` per store object: after each successful publish, and after each space or budget decline. Rust publishers reap too (C9).

### 6.8 Environment policy

| Variable `COMMONKIT_STORE_*` | macOS default | Linux default | Notes |
|---|---|---|---|
| `ROOT` | `~/Library/Caches/commonkit/store` | `/dev/shm/commonkit` | `~` is expanded |
| `MAX_GB` | 2.0 | 8.0 | Times 2^30, truncated to an integer |
| `MIN_FREE_GB` | 8.0 | 8.0 | |
| `TTL_S` | 900 | 900 | |
| `REAP_S` | 30 | 30 | |
| `MAX_ARRAY_BYTES` | 134217728 | 134217728 | Parsed as a float, truncated to an integer |
| `FSYNC` | off | off | New. `1`, `true`, `yes` or `on`, in any case, turns it on; anything else, including unset, empty or malformed, is off |
| `VERIFY_REPUBLISH` | off | off | New (C15). Parsed as `FSYNC`. On in the conformance suite and CI |

- An unset, empty or malformed value falls back to the default and never raises. The caller chooses only `mode`, `attach` and `min_array_bytes`.
- Platforms are macOS, Linux and Windows (C10). Thresher today uses `ROOT=/Volumes/CommonkitStore/store`, `MAX_GB=7`, `MIN_FREE_GB=1` and `TTL_S=604800`.

### 6.9 Counters

Both implementations use the same counter names, so a stats table can sum them:

- `shm_hit`, `shm_miss`, `shm_torn`, `shm_torn_replaced`, `shm_key_collision` and `shm_bad_key`;
- `shm_declined_{mode,small,size,space,budget,dtype}`;
- `shm_publish`, `shm_bytes_published` and `shm_publish_failed`;
- `shm_reaped`, `shm_reaped_bytes`, `shm_reap_partial`, `shm_reap_refused` and `shm_reap_failed`.

## 7. The never-raise invariant in Rust

### 7.1 API shape (illustrative, not code to copy)

| Item | Returns | Notes |
|---|---|---|
| `KeyScheme::new(domain, version, forms, variants, dtypes)` | `Result<KeyScheme, KeyError>` | The one loud path, as in Python |
| `register(scheme)` / `scheme_for(key)` | `Result<Arc<KeyScheme>, KeyError>` | A process-global registry under a `Mutex`; a poisoned lock is recovered with `into_inner`, never unwrapped |
| `Key::parse(&str)` | `Result<Key, KeyError>` | Grammar only; `relpath()` never makes an absolute path |
| `Store::new(root, Options)` | `Store` | Never fails; `is_disabled()` reports whether the root is usable |
| `shared_store(mode, attach, min_array_bytes)` | `Store` | Policy from the environment (and `.policy.json` if C7 is adopted) |
| `store.attach(key)` | `Option<Segment>` | A miss, torn file, bad key or disabled store is `None`, plus a counter |
| `store.read(key)` | `Option<OwnedArray>` | A copy; always available, with no `unsafe` |
| `Segment::open_path(path)` / `OwnedArray::read_path(path)` | `Option<…>` | For ImpCraft's fallback files and PlumCraft's `path` locators; the caller decides what to trust |
| `store.publish(key, ArrayRef)` | `Result<bool, KeyCollision>` | `ArrayRef` is a descriptor, a shape, Fortran order and borrowed bytes; checked before writing. `Ok(true)` published or touched, `Ok(false)` declined, `Err` a key collision (§6.4 item 4) |
| `store.reap(budget)` | `ReapReport` | `scanned`, `reaped`, `bytes`, `complete` |
| `store.usage()` | `Option<Usage>` | `None` when `.usage.json` is stale |
| `store.drain_stats()` | `Stats` | Atomic counters taken and reset |
| `Segment` | `descr()`, `dtype()`, `shape()`, `fortran_order()`, `bytes()`, `as_slice::<T>()`, `to_f32_vec()`, `element(index)` | Ported from `impcraft-shm`: zero-copy only when the data is native-endian and aligned |

**Lints.** The crate root carries this:

```rust
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

```

Tests are exempt, as in `clippy.toml`. All arithmetic on header fields is checked, and allocations are bounded by `max_array_bytes` and the file length. A fuzz test covers headers (ported from `impcraft-shm`) and keys.

**Errors.** Typed errors exist only for scheme construction. Everything on the read, publish and reap paths returns `Option` or `bool` and ticks a counter. The fine-grained reason a header was rejected is kept as a debug-level detail, not a return value.

### 7.2 Where `unsafe` lives

- **Feature-gated.** `memmap2::Mmap::map` sits behind a default-on Cargo feature, `mmap`, in one module, `src/map.rs`.
  - The crate is `#![deny(unsafe_code)]`, and only that module carries `#[allow(unsafe_code)]`.
  - Clippy's `undocumented_unsafe_blocks` lint is set to `deny`, as `impcraft-shm` does now.
- **The `SAFETY:` comment** cites contract §6.5, the mutation rule. It says:
  - the map is read-only and never exposed mutably;
  - a conforming process only renames over or unlinks a segment;
  - both are safe for an existing mapping.
- **Without the feature** (`default-features = false`), no `unsafe` is compiled, and `attach` falls back to `read`. No current consumer uses it that way: PlumCraft takes the default features (C5, decided). Its workspace's `unsafe_code = "forbid"` covers its own crates only; the mapping lives in this crate's one audited module.

### 7.3 Residual SIGBUS risks the contract can't remove

| Risk | Where it bites | Mitigation |
|---|---|---|
| A non-conforming tool truncates or rewrites a segment in place | Any mapped reader | Contract rule §6.5, documented in the README. The reaper never truncates. |
| The volume is unmounted while mapped | Thresher's root is a separate APFS volume (`/Volumes/CommonkitStore`); ejecting it under a running ImpCraft | Documented. A long-lived viewer can choose `read` for small arrays. |
| A network filesystem | Not supported | The contract requires a local filesystem for the root. |

## 8. Decision C5: feature parity and scope

| Area | In the crate | Notes |
|---|---|---|
| Keys, schemes, registry | Yes | Parity, plus the reader/publisher split from C4 |
| `.npy` header parse and write | Yes | Parse ported from `impcraft-shm`; the writer matches numpy byte for byte |
| Reader: mmap and copy | Yes | mmap behind the `mmap` feature |
| Typed views, element probe | Yes | Ported from `impcraft-shm` (`as_f32`/`as_f64` become `as_slice::<T>`; `element.rs` as is) |
| Touch | Yes | Inconsistency 1 |
| Writer with all gates | Yes | |
| Reaper and `.usage.json` | Yes | |
| Shared-store policy | Yes | |
| Stats | Yes | |
| `cache.py` (`TieredPixelCache`) | **No** | A Python adapter for a `PixelCache` protocol. Rust consumers keep their own in-process memos (ImpCraft textures, PlumCraft's memo). |
| `read.py` (`note`, `attach`) | **No** | Python counter conventions |
| Digest helpers (`nd1`, `raw`) | Optional (C13) | |
| PlumCraft handles, locators, `DataRef` resolution, same-host proof | **No** | They stay in PlumCraft (section 9) |
| Windows | **Yes** (revised 2026-10-27) | C10 |

**Dependencies.**

- `memmap2`, optional;
- `rustix` for a safe `statvfs`;
- nothing else on the main path.

`.usage.json` is three numeric fields, read and written by a small strict parser rather than `serde_json`. Time comes from `std::time` and mtime from `File::set_modified`.

## 9. Migration

### 9.1 ImpCraft: `impcraft-shm`

- `crates/shm` becomes a thin adapter, or disappears.
  - `ui-egui` imports `commonkit::{Segment, Element}`.
  - `commonkit::npy::write_header_v1` replaces the 13 test call sites. It is kept public behind a `test-util` feature, or the tests publish through a `Store`.
- **ImpCraft's golden rule 2** ("No `unsafe` except `crates/shm`") is reworded: `unsafe` is allowed only in pinned dependencies of the maintainer's own repositories that carry it with `SAFETY:` comments, which today means `commonkit`'s `map.rs`. ImpCraft's own crates then keep `unsafe_code = "forbid"` with no exception.
- **Viewers gain an LRU touch.** That is a behaviour change worth a "try this" note in `docs/acceptance/`. Segments the maintainer is looking at stop being evicted first.
- **Trailing bytes and unknown dtypes become misses rather than errors.** The viewer's error text for a "malformed segment" changes to "missing", and the engine reruns, which is the invariant's intent.
- **Pins and checks.**
  - Pin by git rev in `[workspace.dependencies]`.
  - Register the crate in `xtask/src/layers.rs` as L0 external.
  - Run `cargo xtask layers`.
- **The Python side (`publish.py`) is unchanged** apart from the distribution rename (9.3).

### 9.2 PlumCraft: `plumcraft-store`

- **It keeps** everything above the store:
  - `Handle` checks (`md5:raw`, `md5:nd1`), `put_verified` and `matches_handle`;
  - `Locator` and `DataRef` resolution with trusted roots;
  - inline refs;
  - the same-host nonce proof.
- **It delegates** key building, the path, `.npy` read and write, and publishing to `commonkit` with its default features, so it reads by mapping (C5, decided); PlumCraft's own crates stay free of `unsafe`. It registers the `plum` scheme in Rust (`plumarr`, and `plumraw` as `uint8`), matching `data.py`. It publishes through `shared_store` and so **gains the gates and the reaper**.
- **`publish` touches an existing key instead of skipping it** (C15), so a hot blob no longer ages out (inconsistency 10), and a collision is an error, not a silent skip. The Python `Plane._publish` gets the same change.
- **Its reader** gains the length check (inconsistency 4), Fortran order, and the shared size limit (inconsistency 3).
- **`.hello/` and `.sources/` move or get cleanup.** Recommended:
  - nonces are unlinked by their writer when the session ends, and are otherwise removed by age (a PlumCraft change);
  - `.sources/` moves to PlumCraft's durable tier (`~/Library/Caches/plumcraft/cas`), or is published as raw segments so the budget sees it.

  Until then, the contract's "reserved dot-names" rule (§6.2) makes their status explicit.
- **PlumCraft's `docs/plan.md` changes:**
  - section 2.1's `plumcraft-shm` row becomes "use commonkit's crate (`commonkit`, pinned by git rev, default features)";
  - the row "no Rust commonkit writer is needed" is corrected;
  - the source-change row "Optional: a Rust writer crate…" is marked "planned: `commonkit/docs/rust-crate-plan.md`".

### 9.3 The rename to `pycommonkit`, and what it touches

| File | Change |
|---|---|
| `commonkit/pyproject.toml` | `name = "pycommonkit"`. Add `[tool.uv.build-backend] module-name = "commonkit"` (required; see section 2). Replace `license = { file = "LICENSE" }` with `license = "MIT"` and `license-files = ["LICENSE"]`. `requires-python = ">=3.11"` is unchanged. |
| `commonkit/uv.lock` | Regenerate (`uv lock`). It is currently untracked; decide whether to commit it. |
| `commonkit/README.md`, `CLAUDE.md` | Say "installed as `pycommonkit`, imported as `commonkit`"; add the Rust crate and `docs/contract.md`; `CLAUDE.md` gains the Cargo commands and the cross-language rule. |
| `commonkit/AI.md` | A new entry in the step that commits, following the `ai-md` skill. |
| `impcraft/python/pyproject.toml` | `"commonkit>=0.1.0"` becomes `"pycommonkit>=0.1.0"`; `[tool.uv.sources] commonkit = { path = "../../commonkit", editable = true }` becomes `pycommonkit = {…}` |
| `impcraft/python/uv.lock` | `uv lock` (and `uv sync --frozen` on yellowfin afterwards) |
| `plumcraft/python/pyproject.toml` | `"commonkit"` becomes `"pycommonkit"`, and the source key the same way |
| `plumcraft/python/uv.lock` | `uv lock` |
| Other consumers | Search every repository for a `commonkit` dependency line before the switch. The production image-corpus app and impressionist, which the AGENTS file names as using the shared store, may depend on it. |
| Imports | None change: `import commonkit` everywhere |

**Order.** Rename in commonkit first. Update both consumers' manifests and locks in the same session. An old lock that still names `commonkit` and points at a path whose distribution is now `pycommonkit` fails to resolve, which is loud and harmless.

**When to reserve the names.**

- **crates.io `commonkit`:** publish the real `0.1.0` at the end of step 8, once the crate passes the conformance suite. Not a placeholder: crates.io's policy discourages name-squatting.
- **PyPI `pycommonkit`:** publish `0.1.0` when the rename lands (step 1). The code is already public on GitHub, MIT, and has run in production, so a real release is the cleanest way to hold the name.
- **Until a name is published** it can be taken. If that risk matters, publish `pycommonkit` in step 1 and the crate in step 8, as above, rather than waiting for the migrations.
- **Releases follow the maintainer's release rule:** a clean tree, a version bump, a tag, `AI.md`, then build and upload.

## 10. Future items from PlumCraft's source-change table

| Item | Where it would land | Room to leave now |
|---|---|---|
| A `keep` list or `pinned/` subtree the reaper skips (G5) | A reserved dot-directory, e.g. `.pinned/{form}/{xx}/…`, outside the reaper's candidate pattern. Open question: do pins count against `max_bytes`? | The reaper's candidate test is one function (`is_reapable(path)`) in both languages, and the contract already reserves dot-names. Nothing else. |
| A raw segment kind (`.bin` plus a header sidecar) (G6, G9, G10) | A new segment suffix in the contract | **A caution:** a two-file segment can't be published atomically with one rename, and a reaper would have to evict both files together. A 1-d `\|u1` `.npy` is already "raw bytes after a 128-byte header", which PlumCraft uses today. Prefer that, or a single-file raw format, unless a measurement shows the header matters. Keep segment kinds a `#[non_exhaustive]` enum, and keep the extension a function of the kind. |
| A BLAKE3 key helper (D3) | `KeyScheme` gains an optional `digest` attribute (`md5` by default). A 128-bit BLAKE3 truncation fits the existing 32-hex slot, so paths don't change. | The API says `digest`, never `md5`; the grammar names only 32 hex digits. A BLAKE3 domain needs its own domain or version so a key never mixes algorithms. |
| A shared-memory ring for streaming (D29) | **Not commonkit.** Research note 13 decides "no shared ring in Q". If ever built, it is a Rust-only writer with copy-out reads under a seqlock and its own `unsafe` review. Its writer mutates a mapped file in place, which the mutation rule (§6.5) forbids for segments. | None. The contract says why a ring is a different object. |
| Digest helpers (`nd1`, `raw`) | `commonkit.digest` and `commonkit::digest`, if C13 is adopted | Three copies exist: ImpCraft's `publish.digest`, PlumCraft's `data.nd1`, and `Handle::md5_nd1` in PlumCraft's proto crate. ImpCraft's skips byte-order normalisation, so it differs from `nd1` for big-endian arrays. |

## 11. Risks

| Risk | Likelihood | Effect | Mitigation |
|---|---|---|---|
| The two implementations drift again | Medium | Misses, double budgets, or a reaper evicting what the other considers live | The contract, shared vectors and CI's `conformance` job. A behaviour change lands in both languages in one PR. |
| A reaper runs with partial policy (inconsistency 16) | Medium on thresher once Rust processes reap | A tree evicted down to nothing | C7: a `.policy.json` at the root. Meanwhile, Rust hosts pass all `COMMONKIT_STORE_*` variables to children, and the app bundle sets them (`launchctl setenv` or `LSEnvironment`). |
| SIGBUS from a truncated or unmounted file | Low | ImpCraft's UI crashes | §6.5 and §7.3 |
| Two reapers race on a stale lock | Low | Two concurrent reapers. Unlinking twice is harmless (ENOENT is skipped); `.usage.json.tmp` could interleave, and then parses as stale (`None`) | Documented and tested (C8). Optionally `flock` in contract version 2. |
| The rename breaks a consumer not listed | Medium | `uv sync` fails | The search step in 9.3; failure is loud |
| Squatting before release | Low | The name is lost | Publish early (9.3) |
| Rust readers now touch | Certain | More metadata writes; on ZFS, copy-on-write churn | Already limited to once per 60 s per segment, as Python |
| Scope creep into PlumCraft's handle layer | Medium | The crate grows a second purpose | Section 8's "No" rows |

## 12. Decisions for the maintainer

| # | Decision | Options | Recommendation |
|---|---|---|---|
| C1 | What the crate is | — | **Decided (the maintainer, 2026-10-27): a pure Rust crate**, as recommended. |
| C2 | Layout | — | **Decided (the maintainer, 2026-10-27): a `rust/` workspace**, with `conformance/` at the root, as recommended. |
| C3 | Contract authority | — | **Decided (the maintainer, 2026-10-27): a written `docs/contract.md`, version 1**, as recommended. |
| C4 | Key checking on read | — | **Decided (the maintainer, 2026-10-27): grammar on read, registry on publish**, as recommended. |
| C5 | mmap | — | **Decided (the maintainer, 2026-10-27): a default-on `mmap` feature**, one module, `SAFETY:` citing §6.5. **PlumCraft does not turn it off** ("1, but PlumCraft does not turn it off."): `plumcraft-store` takes the default features and reads by mapping too, so its "no `unsafe`, copy on read" property moves into the crate's one audited module. |
| C6 | Contract tightenings (in both languages) | — | **Decided (the maintainer, 2026-10-27): tighten all five**, as recommended. |
| C7 | Tree policy | — | **Decided (the maintainer, 2026-10-27): add `.policy.json`, obeyed if present, and the file wins over the environment** ("policy at the store root obeyed if present"; then "File over environment"). The root still comes from `COMMONKIT_STORE_ROOT` or the default, since that is how a process finds the file; the environment fills only what the file leaves out, then the defaults. This changes `shared.py`'s "only from the environment" rule. |
| C8 | Reap lock | — | **Decided (the maintainer, 2026-10-27): keep `O_EXCL` plus the 300 s break** in version 1, as recommended. |
| C9 | Do Rust publishers reap? | — | **Decided (the maintainer, 2026-10-27): Rust publishers reap, as Python**, rate-limited. |
| C10 | Platforms | — | **Decided (the maintainer, 2026-10-27): keep Windows.** "Just don't delete support from shared infrastructure like commonkit. There is commonkit work already complete on windows, and we will merge it in later." The crate and the contract must not remove or preclude Windows: keep `impcraft-shm`'s Windows branch, and leave room for a Windows section in the contract (Windows can't unlink an open or mapped file, which touches §6.5 and the reaper) to be filled when the existing Windows work is merged. An earlier recommendation here to drop Windows was the AI's mistake. |
| C11 | Versioning | — | **Decided (the maintainer, 2026-10-27): one version and one tag** for both; the contract versions separately. |
| C12 | Header bytes | — | **Decided (the maintainer, 2026-10-27): byte-identical headers**, pinned by vectors. |
| C13 | Digest helpers | — | **Decided (the maintainer, 2026-10-27): digest helpers in both languages**, low priority (step 10). |
| C14 | `cache.py` in Rust | — | **Decided (the maintainer, 2026-10-27): leave it out.** `TieredPixelCache` is Alexandria's access policy over the store (an in-process memo of attached mappings), not part of the contract; Python keeps it unchanged, and a Rust caller that wants the same holds its attached segments itself. |
| C15 | Republishing an existing key | — | **Decided (the maintainer, 2026-10-27): never rewrite.** A torn file is replaced; a whole file with the same dtype, shape and order is touched; a different array is a key collision, "a major system error", which fails loudly (Python raises `KeyCollision`, Rust returns `Err`), counted and logged, with the existing file untouched. `verify_republish` (off by default, on in conformance and CI) also compares bytes. `publish_if_absent` is dropped. The first recommendation, rewrite by default for parity, was wrong: "Why would I rewrite an existing file?" |
| C16 | `pycommonkit` and crate names, MIT, 3.11+ | — | **Decided** (section 2) |
| C17 | Write-once rules and fsync | — | **Decided** (section 2): the rules open the contract; fsync is a policy option, off by default |

## 13. Step plan

Estimates are in working days for one agent with maintainer review. Each step is its own branch and PR in commonkit, except steps 8 and 9, which land in ImpCraft and PlumCraft.

| Step | Content | Exit criteria | Days |
|---|---|---|---|
| 1. Rename and licence | Section 9.3 in commonkit, ImpCraft and PlumCraft. Search the other repositories for `commonkit` dependency lines. Publish `pycommonkit 0.1.0` to PyPI. | `uv build` gives `pycommonkit-0.1.0` with `License-Expression: MIT`; both consumers `uv sync` and their suites pass; PyPI shows the project | 0.5–1 |
| 2. Contract and vectors | `docs/contract.md` (section 6) with C4, C6, C7, C8 and C15 as decided. `conformance/vectors/`: keys valid and invalid, key-to-path, headers (including growth-padding shapes, versions 2 and 3, Fortran, big-endian), dtype admission, torn and trailing files. Small `.npy` fixtures written by numpy, plus a generator script. | The maintainer accepts the contract; the vectors are reproducible from the generator | 2 |
| 3. Python to contract | Python's side of C6 (and C7 if adopted), the `FSYNC` policy option, off by default (C17), and the existing-key rule with `KeyCollision` and `VERIFY_REPUBLISH` (C15). `tests/test_vectors.py` reads the shared vectors. | Every vector passes in Python; the 72 existing tests pass; no new dependency | 1–1.5 |
| 4. Rust skeleton and CI | The `rust/` workspace, lints, `deny(unsafe_code)`, the `mmap` feature, crate metadata (`license = "MIT"`), GitHub Actions with three jobs on macOS and Linux | CI is green on an empty crate; `cargo package --list` includes the licence; `uv build` still excludes `rust/` | 1 |
| 5. Keys and `.npy` | Scheme, registry, grammar, relpath. Header parse (ported from `impcraft-shm`) and a numpy-exact writer. Fuzz tests. | Every key and header vector passes in Rust; the headers it writes equal numpy's bytes for every vector shape | 1.5–2 |
| 6. Reader, writer, policy | `Store`, `shared_store`, environment (and policy file), mmap and copy attach, touch, typed views and element probe, publish with every gate and the existing-key rule (C15), stats. | Rust ports of Python's store, shared and LRU tests pass; Clippy is clean with the panic lints; no `unsafe` outside `map.rs`; `--no-default-features` builds and passes | 3 |
| 7. Reaper | Lock, walk, LRU toward both targets, `.usage.json`, rate limit, tmp sweep | Ports of Python's six reaper tests pass, plus refusal and bystander tests | 2 |
| 8. Cross-language conformance | `ck-conform` (publish, attach, reap, usage, stats as JSON); `tests/test_conformance.py`; Rust tests that call `uv run python`. Cases: Python publishes and Rust attaches; Rust publishes and Python attaches, with bytes and headers equal; LRU touch from either side protects a segment from the other's reaper; both reapers on one tree, concurrently and in turn, reaching the same survivor set; a stale lock from either language; a torn or partial segment and an orphan `.tmp`; `.usage.json` written by one and trusted by the other's budget gate; publishers racing on one key in both languages; a reader holding a mapping while the other reaper unlinks | The `conformance` CI job is green on macOS and Linux; thresher's real tree is reaped by a Rust reaper in a dry run that reports the same set as Python's; then publish `commonkit 0.1.0` to crates.io | 2.5–3 |
| 9. ImpCraft migration | Section 9.1. Rework golden rule 2. Add an acceptance "try this" note (a viewed segment survives reaping). | `cargo test`, `clippy` and `cargo xtask layers` pass; kittest snapshots are unchanged; the maintainer checks the viewer | 1–1.5 |
| 10. PlumCraft migration | Section 9.2, including the `plum` scheme in Rust, the gates and reaper, the existing-key rule (C15), the `.hello` cleanup and the `plan.md` corrections. Optionally C13's digest helpers in commonkit, with both consumers adopting them. | PlumCraft's D1 and D2 conformance pass in all modes; no `unsafe` in its build (`default-features = false`) | 1.5–2.5 |
| **Total** | | | **17–21** |

Steps 1, 2 and 4 can overlap. Steps 9 and 10 are independent of each other.

## 14. PDF and tracking

This plan's PDF, `docs/rust-crate-plan.pdf`, sits beside its Markdown source. commonkit's `.gitignore` ignores only Python build and cache files, not PDFs, so the PDF would be tracked if `docs/` were added. Nothing has been committed.

# Status

*Updated 2026-10-28 (commonkit 0.1.0 published).*

## Where things stand

- **Lifted** from `seantrue/commonkit` (`main` at `4dbfc7d`; the plan, contract and vectors from its `rust-crate-plan` branch) into this repository with a fresh history. The Python package is unchanged apart from its distribution name (`pycommonkit`, imported as `commonkit`), SPDX licence metadata (`license = "MIT"`, `license-files`), and `module-name = "commonkit"` for uv_build.
- **Plan:** `docs/rust-crate-plan.md`. Every decision, C1–C17, is the maintainer's. Section 13 is the step plan.
- **Step 1 (layout) and step 2 (contract and vectors) done:** `docs/contract.md` version 1, `conformance/vectors/` (98 files), `conformance/generate.py` (byte-for-byte reproducible), `tests/test_vectors.py`. The suite is 369 passed and 91 xfailed: the xfails are the behaviour Python adopts in step 3 (contract Appendix A).
- **Published:** `commonkit` 0.1.0 on crates.io (2026-10-28, from `cdffb50`, tag `commonkit-v0.1.0`). PyPI `pycommonkit` is not published yet; the name was free on 2026-10-27.

## Done since the lift (2026-10-28)

- **Keys (A1–A3):** keys, tokens and digests match in full (`fullmatch`; a final newline is no longer accepted), the digest folds ASCII only, a variant pattern must match the whole variant and the variant must be a token, and registry identity compares pattern text. Six vectors moved from xfail to pass: 375 passed, 87 xfailed.
- **Read back by copy:** `verify_republish` compares named-field bytes only; padding bytes, which a copying `np.load` zeroes, never decide a collision (contract §5.8, §6.4; vectors `existing_key.padded_*`).
- **Ruff clean:** the four `try`/`except`/`pass` handlers in `cache.py` carry `noqa: S110` with the reason; `collections.abc.Iterable` and `X | None` in `keys.py` and `read.py`.

## Step 3: Python to the contract (2026-10-28, merged as PR #1, `main` at `b99151e`)

- **Appendix A4–A14 done**; every contract clause that read "Python: changes in step 3" now reads "Python conforms since pycommonkit 0.1.0 (A*n*)". No vector carries `"python": "step3"`; the generator still reproduces every fixture byte for byte (only the JSON's marks and three stale notes changed; no `.npy` fixture changed).
- **Suite:** 548 passed, 0 xfailed (was 375 passed, 87 xfailed); `ruff check .` clean; clean under `-W error`.
- **What changed:** `store.py` (headers to 1 MiB with a prefix check, broad torn handling, object dtypes and oversized headers declined, the existing-key rule with `KeyCollision` and `verify_republish`, `fsync` with `F_FULLFSYNC` on macOS, segment-shaped reaping and the `.tmp` sweep, free space 0 on any failure, integer-only `.usage.json`); `shared.py` (`.policy.json`, the number grammar, the flags); `cache.py` lets `KeyCollision` through.
- **Behaviour a consumer may notice:** a different array under an existing key now raises `KeyCollision` instead of silently replacing the segment; a republish no longer counts `shm_publish`; `.npy` files outside the segment layout are no longer reaped; a `.policy.json` at the root now overrides the environment.

## Step 4: Rust skeleton and CI (2026-10-28, branch `step4-rust-skeleton`, uncommitted)

- **`rust/`** is a Cargo workspace (edition 2024, `rust-version = "1.95"` as the viewer app and PlumCraft) with one crate, `commonkit` 0.1.0, `license = "MIT"`. `rust/commonkit/LICENSE` is a copy of the root `LICENSE`, because `license-file` alongside `license` draws a cargo warning; `tests/license.rs` keeps the two identical. `cargo package --list` includes it.
- **Lints:** the crate root denies `unwrap_used`, `expect_used`, `panic`, `unimplemented`, `todo` and `unreachable`, and `rust/clippy.toml` allows the first three in tests. The workspace sets `unsafe_code = "deny"` (not `forbid`, so step 6's `src/map.rs` can allow it) and `undocumented_unsafe_blocks = "deny"`. The default-on `mmap` feature exists and is empty until step 6. PlumCraft keeps the default features (C5).
- **CI:** `.github/workflows/ci.yml` runs on Linux only (the maintainer: "No ci on mac"; macOS is checked on thresher). It has three jobs:
  - `python`: pytest on 3.11 and 3.13, ruff, and a `uv build` check that leaves out `rust/`.
  - `rust`: fmt, clippy with all features and with none, tests both ways, and the package list.
  - `conformance`: `cargo test --test 'vectors*'` for now.
  - The workflow passes `actionlint`; it can't run until it is pushed. Nothing in it precludes a Windows job later.
- **`uv build`** still leaves `rust/` and `.github/` out of both the sdist and the wheel.

## Step 5: keys and `.npy` in Rust (2026-10-28, same working tree, uncommitted)

- **`keys`:** `Key::parse` (the grammar, in full), `KeyScheme` (ASCII-only digest folding on build, never on parse; variants as a closed token set, or a `regex` pattern compiled bare and then anchored `\A(?:…)\z`; the variant must also be a token; dtypes as canonical descriptors), and `Registry` (process-global `Registry::global()`, poison-tolerant; identity compares pattern text).
- **`npy`:** `read_prefix` refuses `HLEN > 2^20` before reading any text. `parse_header` and `parse_npy` accept versions 1 to 3, Latin-1 and UTF-8, both byte orders, Fortran order, trailing bytes, 16-byte alignment and other literal spellings. They read every §5.4 descriptor, typed (`Typed`) for the listed kinds and as a `Descr` with its `repr` text otherwise, and miss object dtypes, titles and long doubles. `encode_header` and `encode_npy` write numpy's bytes. No `unsafe`.
- **Tests:** 31 Rust tests. Every key vector (18 build, 24 parse, 8 registry) and every header vector (53) passes, read from `conformance/vectors/` at test time. The writer reproduces all 53 fixtures byte for byte from their array specs, and the file-level outcome of the 25 `reads.json` files is right. Hostile input: every truncation of every fixture, 20 000 mutated fixtures, 20 000 random header texts, 5 000 written-then-read headers, and 50 000 random keys checked against the grammar regex.
- **Dependencies:** `regex` (MIT OR Apache-2.0); dev-only `serde_json` and `sha2` (both MIT OR Apache-2.0).

## Step 6: the Rust store (2026-10-28, branch `step6-rust-store`, uncommitted)

- **Modules** in `rust/commonkit/src/`:
  - `store.rs`: `Store`, `Options`, `Mode`, `ArrayRef`, `KeyCollision`, `Counter`, `Stats`, `Usage`. Construction never fails (§6.1); attach takes any grammatical key (C4); publish runs the seven gates in contract order, then the existing-key rule (torn replaced and counted; whole and matching touched; anything else `Err(KeyCollision)`, file and mtime untouched, counted, logged at error level); `verify_republish` compares named-field bytes only, in 1 MiB pieces; `fsync` off by default.
  - `segment.rs`: `Segment` (mapped or copied), `AttachMode`, typed views (`as_slice::<T>`, zero-copy only for native order and alignment), `to_f32_vec`/`to_f64_vec`, and the element probe ported from the viewer's `shm` crate.
  - `policy.rs`: `Policy`, `.policy.json` (file over environment, C7), the number and flag grammars, `resolve_root`, `shared_store` and `shared_store_at`.
  - `map.rs`: the crate's only `unsafe` (`memmap2::Mmap::map`), behind the default-on `mmap` feature, with a `SAFETY:` comment citing contract §7. Without the feature, attach copies.
  - `sys.rs`: free space (`statvfs` via `rustix`; `fs4` on Windows), `F_FULLFSYNC` with an `fsync` fallback on macOS, directory sync on POSIX only, the touch as `utimensat(UTIME_NOW)`.
  - `json.rs`: a small strict JSON reader for `.policy.json` and `.usage.json`.
- **The reaper is a stub.** Reap turns are taken, rate-limited, after a publish and after a space or budget decline (§9.8); `Store::reap_turn` does nothing until step 7.
- **Tests:** 110 pass with default features and 110 with `--no-default-features` (1 ignored each: `reaper_paths.json`, step 7). Ports of `test_store.py` and `test_shared.py` keep the Python names; the Python reaper tests are listed in `tests/store.rs` for step 7. Every `reads`, `admission`, `existing_key`, `env` and `policy` vector passes, read from the JSON.
- **Dependencies added:** `memmap2` (optional), `bytemuck`, `log`, `rustix` (Unix), `fs4` (Windows); dev-only `tempfile`. All MIT-compatible.
- **Rust and Python differ** (all within the contract):
  - An unregistered but grammatical key is a plain miss in Rust (C4: grammar on read) and a bad key in Python.
  - An object dtype can't be built in Rust (`Descr` refuses it), so the two object admission vectors decline by construction and tick no counter.
  - `.usage.json` is read only if it is a regular file of at most 64 KiB.
  - Windows: the default root is `%LOCALAPPDATA%\commonkit\store` (the viewer app's practice; §12 leaves it open). The Windows target is not installed on thresher, so the Windows build is unchecked.
- **Found in Python (not changed):**
  - `SharedArrayStore("")` makes a store at the working directory; contract §6.1 says an empty root is disabled. Rust follows the contract. Only direct construction reaches it; `shared_store` never passes an empty root.
  - `test_usage_bytes_must_be_a_non_negative_json_integer` passes without testing validity: its fixture's `reap_interval_s=0` makes every `.usage.json` stale. The Rust port uses 30 s.

## Step 6 review (2026-10-28)

- **Windows type-checks:** `cargo check` and `cargo clippy -D warnings` pass for `x86_64-pc-windows-msvc`, with and without `mmap` (target installed on thresher; nothing runs on Windows yet).
- **Two Python defects the Rust port found, fixed:** `SharedArrayStore("")` was an enabled store in the working directory (contract §6.1 says disabled); and `test_usage_bytes_must_be_a_non_negative_json_integer` tested nothing, because its fixture made every `.usage.json` stale. It now uses a fresh store with an integer control. Python: 551 passed.

## Step 7: the Rust reaper (2026-10-28, branch `step7-rust-reaper`, uncommitted)

- **`rust/commonkit/src/store/reap.rs`** (about 430 lines with unit tests) replaces the stub. `Store::reap(budget: Duration) -> ReapReport` (`scanned`, `reaped`, `bytes`, `complete`; `#[non_exhaustive]`), with `DEFAULT_REAP_BUDGET` (250 ms, Python's default) and `REAP_TURN_BUDGET` (50 ms). Publish and space or budget declines take rate-limited turns that now reap (C9).
- **Behaviour (contract §9):** refusals (empty root, `/`, fewer than two separators: `shm_reap_refused`; a disabled store doesn't reap); `.reap.lock` by `create_new`, mode 0644, the pid, released by a guard on every exit, broken when over 300 s old (by `lstat`), and that turn returns without reaping; the walk descends only `{form}/{xx}/` real directories (never a symbolic link, never a `.`-name), with the deadline checked before each directory; candidates by path shape and `lstat` only; `.tmp` over an hour old swept from segment directories only; sort by `(mtime, size, path bytes)`; eviction while expired, over the low water mark (complete walk only) or short of the floor; `.usage.json` after a complete walk only, via `.usage.json.tmp`, which is created exclusively (any left behind is unlinked first, so a planted link is never written through); free-space cache invalidated.
- **Failures:** an unlink that finds the file gone (another reaper) is skipped quietly; any other failed unlink is skipped and counted `shm_reap_failed` (one per file) and eviction goes on. That is how Windows will see a mapped or open segment (§12); a note in `reap.rs` points there. A lock created but whose pid can't be written counts `shm_reap_failed` (Python raises there, and only its turn counts it).
- **Tests:** 134 pass with default features and 134 with `--no-default-features` (unit 31, store 69, vectors_store 7, others unchanged, 1 doctest), none ignored. All 16 `reaper_paths.json` vectors pass. The eight Python reaper tests are ported under their names, and `test_a_declined_publish_still_reaps` now asserts the reaping. Rust's own: refusals, disabled store, bystanders (non-segment files, dot-names, `.pinned/`, shape misses, a segment-shaped directory, symbolic links to files and directories, a link at `.usage.json.tmp`), a mapping surviving the reaper, the lock's pid, held and stale locks, unlinking twice, a failed unlink, eight concurrent reapers on one tree, `.usage.json` contents, a partial walk still reaping what it found expired, the `.tmp` sweep. `cargo fmt --check`, clippy `-D warnings` both ways, and `cargo check`/`clippy` for `x86_64-pc-windows-msvc` both ways pass. Still no `unsafe` outside `map.rs`. Python unchanged: 551 passed, ruff clean.
- **Test seams** (`Hooks`, hidden): `reap_unlink` replaces the reaper's unlink; `reap_dir` runs before each directory's deadline check.
- **Found (not changed; probe `claude-debug/step7_python_reaper_probe.py`):**
  - Python's `test_a_declined_publish_still_reaps` ages each segment inside the publish loop with `reap_interval_s=0`, so the next publish reaps it; the decline's own turn reaps 1, and `shm_reaped == 3` holds only by accumulation. The Rust port publishes, then ages.
  - Racing reapers after a broken stale lock (C8) over-evict: an unlink that finds the file gone is not subtracted from `total` or added to `freed` (§9.6, in both languages), so the later reaper keeps evicting. Measured in Python: six segments with a low water mark of two leave none. Harmless to correctness (a miss is a slower path), but "harmless" in C8 understates it; counting ENOENT as freed would fix it in a later contract version.
  - Python's `.usage.json.tmp` is opened with `open(…, "w")`, which follows a symbolic link at that name and truncates its target. Rust never writes through one.

## Step 7 review (2026-10-28)

Three problems the Rust reaper's agent found in Python and the contract, fixed in both languages ("Go for fix"):

- **Racing reapers over-evicted.** A segment already gone (ENOENT: another reaper took it after a broken stale lock) wasn't counted toward the targets, so two racing reapers took six segments to none against a low water mark of two. It now counts as freed, though not as reaped (contract §9.2, §9.6, §9.7). Tests in both languages fail without the fix.
- **`.usage.json.tmp` followed a symlink.** Python's `open(tmp, "w")` truncated whatever a symlink at that name pointed to; for a mapped segment, SIGBUS in its readers. It is now created exclusively after removing any leftover (contract §9.7), as Rust already did. A test fails without the fix.
- **A weak test.** `test_a_declined_publish_still_reaps` passed only because three turns' counts added up; it now ages after publishing, so only the decline's own turn can reap.
- **Per-candidate failures** (other than already gone) count `shm_reap_failed` in both languages, as Rust did; the contract says so (§9.6).

Rust 135 passed (both feature sets), clippy clean on macOS and the Windows target; Python 553 passed, ruff clean.

## Step 8: cross-language conformance (2026-10-28, branch `step8-conformance`, uncommitted)

- **Drivers** speaking the same JSON: `rust/ck-conform/` (a workspace member, `publish = false`; its `mmap` feature passes through to commonkit's, so `--no-default-features` still builds commonkit without it) and `conformance/ck_conform.py`. Commands: `publish`, `attach` (descr, shape, order, payload SHA-256, mapped), `hold` (attach, wait, re-hash), `reap` (with test seams `die_in_walk` and `sleep_before_unlink`), `plan` (a dry run with the tree's own policy, constructing no store), `usage`, `policy`, and `batch`; every result carries the counters it moved.
- **`tests/test_conformance.py`** (61 cases, ~19 s; builds `ck-conform` with `cargo build`, skips only without cargo) and **`rust/ck-conform/tests/python.rs`** (8 cases, Rust in-process against `uv run python`; they skip without `uv` unless `CK_CONFORM_REQUIRE_PYTHON` is set). Cases: every header vector published by both languages gives identical files equal to numpy's fixture, and each attaches the other's (mapped and copied); a touch (hit or republish) from either side protects a segment from either reaper; both reapers reach one survivor set on copies, in turn, six at once, and in their dry runs; racing reapers after a broken stale lock count gone segments in both directions; a lock left by a dead reaper of either language is respected fresh and broken stale by the other; torn, partial and empty segments are torn misses and replaced alike; orphaned `.tmp` files swept alike; `.usage.json` from either is trusted by the other's budget gate; eight publishers racing on one key (four per language) all succeed; real collisions (other bytes, shape, order, dtype) raise in either and leave the file and mtime alone; a mapping survives the other language's reaper; every `policy.json` vector and three more read the same.
- **Dry run** (contract §9.10, an implementation aid): `reap(dry_run=True, now=…)` and `reap_plan(...)` in Python; `Store::reap_with(&ReapOptions)` and `reap_plan(...)` in Rust. `ReapReport` gained `dry_run`, `unlink`, `sweep`, `low_water`, `need_free` and is no longer `Copy`. Tests in both languages (`tests/test_dry_run.py`, `rust/commonkit/tests/dry_run.rs`) compare every entry's `lstat` before and after, and check the plan against a real reap of the same tree.
- **Thresher's real tree** (`/Volumes/CommonkitStore/store`, `MAX_GB=7`, `MIN_FREE_GB=1`, `TTL_S=604800`, no `.policy.json`), both dry runs with `now=1793197530`: 1725 segments scanned, 136 to unlink (281,616,168 bytes, the oldest, to bring 6.29 GB under the 6.01 GB low water mark; none expired), 0 `.tmp` to sweep. The two lists are identical, in order. `find … -exec stat -f '%N %Fm %z'` before and after (2392 entries) is identical: `claude-debug/step8_real_tree_*`.
- **Disagreement found and fixed:** the Rust test helper `spec_fill` (copied into `ck-conform`) wrote big-endian payloads for `>M8`/`>m8`, but the generator's `arange(…, dtype=">i8") * 1000 + seed` yields native order, which is what the fixtures hold. Both copies now write little-endian. Test-only; no library or contract change.
- **Gates:** Python 621 passed, ruff clean, the generator still reproduces the vectors. Rust: `cargo test` both feature sets, clippy `-D warnings` both ways and for `x86_64-pc-windows-msvc` both ways, `cargo fmt --check`; no new `unsafe`. `actionlint` is not installed on thresher and was not run on the new workflow (it parses as YAML).
- **CI:** the `conformance` job now also installs uv and runs the pytest suite (Python 3.11) and `cargo test -p ck-conform` with `CK_CONFORM_REQUIRE_PYTHON=1`; the `python` job leaves `test_conformance.py` to it.
- **Before publishing `commonkit` 0.1.0:** done in the crates.io prep below.

## `remove(key)` (2026-10-28)

The maintainer: "Add a way to forcibly remove an existing cache entry (if not in use). This is to support existing users of ../impressionist", then chose **no in-use check** and **pycommonkit only for now**. Impressionist's checkpoint `reset=True` recomputes a value and relied on publish rewriting the segment; under C15 a republish is only a touch, or a `KeyCollision`, so reset no longer replaced anything. `SharedArrayStore.remove(key)` and `Store::remove(key)` unlink the segment (regular files only, never a symlink; readers keep their bytes), count `shm_removed` / `shm_remove_failed`, and never raise (contract §6.7, W1's one exception). Tests in both languages, plus a cross-language case: removed in one language, replaced in the other. **Still to do in impressionist:** call `remove` before republishing on reset, and switch its dependency from `../commonkit` to pycommonkit.

## crates.io prep (2026-10-28)

- **Package contents.** `exclude = ["tests/"]`: seven of the nine integration tests read `conformance/vectors/` and one reads the root `LICENSE`, so the earlier plan to exclude only some of them would have shipped tests that fail. The unit tests in `src/` ship. `cargo package` packs 18 files (57 KiB compressed) and builds them alone; `cargo publish --dry-run` stops only at the upload.
- **`rust/commonkit/README.md`** is the crates.io page (`readme = "README.md"`): what the crate is, the invariant, a full example, features, platforms and the minimum Rust version. `lib.rs` compiles it as a doctest, so the example can't drift from the API.
- **Minimum Rust version 1.95, checked.** The library built on 1.95, but `tests/hostile_input.rs` did not: 1.95 inferred `T = str` for `pick` from the expected argument type, where later compilers infer it from the slice. Explicit `pick::<&str>` builds on both and satisfies clippy (`*rng.pick(…)` did not). A new CI job, `msrv`, runs the crate's tests on 1.95 with and without `mmap`.
- **Docs.** `cargo doc` with `-D warnings` failed on a public doc link to the private `STALE_LOCK_S`; the text now gives the value (300 s). CI builds the docs that way.
- **actionlint** (1.7.12) found a real defect from step 4: in `uv build leaves rust/ out`, `! tar … | grep -q '/rust/'` was not the last line, and bash `-e` ignores a negated command, so only the wheel check counted. The checks are `if …; then exit 1; fi` now; the sdist and wheel have no `rust/` today.
- **Published** by the maintainer with `cargo publish -p commonkit` from a clean `main` at `cdffb50`, tagged `commonkit-v0.1.0`. `msrv` is now a required check on `main`, with `python`, `rust` and `conformance`.

## Next

1. **The maintainer accepts the contract** (plan step 2's exit). Open choices the contract made where the plan was silent, listed for confirmation:
   - `.policy.json` format: unprefixed upper-case keys; numbers or strings for numeric settings, booleans or strings for flags; a bad value counts as left out; a file over 64 KiB, invalid JSON or duplicate keys is treated as absent; `ROOT` is ignored; read once by `shared_store`.
   - Headers up to 1 MiB are readable; 32 dimensions guaranteed.
   - A republish counts nothing, takes no reap turn, and touches at most once per 60 s; the existing-key check runs after the admission gates.
   - The same values in a different memory order are a `KeyCollision`.
   - Flag values are not trimmed (`" 1"` is off).
   - A failed directory sync after the rename counts as a failed publish and leaves the segment in place.
   - Swept `.tmp` files are not counted.
   - Rust needs the `regex` crate for variant patterns (missing from the plan's dependency list).
2. **Steps 4 and 5:** decided by the maintainer 2026-10-28 ("Yes for all"):
   - A closed variant set compares as a **set** (contract §3.4) in both languages; Python now compares sets (vector `keys.registry.variants_reordered`).
   - Forms must be distinct (contract §3.2) in both languages; Python now refuses duplicates.
   - The Rust writer declines a field name with a backslash, a control character, or a non-ASCII character that is not a letter or digit (stricter than Python, so safe). A name holding both quote characters is written exactly as Python does.
   - The reader misses `=`, `|` on a multi-byte kind, field titles, and duplicate dict keys, and normalises `<u1` to `|u1`, as numpy does.
   - Deferred, now done (crates.io prep): CI builds on 1.95 as well as stable, and the integration tests are left out of the crate package.
3. **Steps 9–10:** the viewer app and PlumCraft migrations, now unblocked: `commonkit` 0.1.0 is on crates.io. Step 8 is done above.
4. **PyPI:** publish `pycommonkit` 0.1.0 (project links and classifiers in `pyproject.toml`, a TestPyPI run, then `uv publish`).
5. **The old repository** `seantrue/commonkit`: still holds the step-2 work uncommitted on its `rust-crate-plan` branch; what to do with it (archive, or a pointer README) is the maintainer's call.

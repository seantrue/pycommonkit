# Status

*Updated 2026-10-28 (step 6).*

## Where things stand

- **Lifted** from `seantrue/commonkit` (`main` at `4dbfc7d`; the plan, contract and vectors from its `rust-crate-plan` branch) into this repository with a fresh history. The Python package is unchanged apart from its distribution name (`pycommonkit`, imported as `commonkit`), SPDX licence metadata (`license = "MIT"`, `license-files`), and `module-name = "commonkit"` for uv_build.
- **Plan:** `docs/rust-crate-plan.md`. Every decision, C1–C17, is the maintainer's. Section 13 is the step plan.
- **Step 1 (layout) and step 2 (contract and vectors) done:** `docs/contract.md` version 1, `conformance/vectors/` (98 files), `conformance/generate.py` (byte-for-byte reproducible), `tests/test_vectors.py`. The suite is 369 passed and 91 xfailed: the xfails are the behaviour Python adopts in step 3 (contract Appendix A).
- **Nothing published:** PyPI `pycommonkit` and crates.io `commonkit` were free on 2026-10-27.

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

- **`rust/`** is a Cargo workspace (edition 2024, `rust-version = "1.95"` as ImpCraft and PlumCraft) with one crate, `commonkit` 0.1.0, `license = "MIT"`. `rust/commonkit/LICENSE` is a copy of the root `LICENSE`, because `license-file` alongside `license` draws a cargo warning; `tests/license.rs` keeps the two identical. `cargo package --list` includes it.
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
  - `segment.rs`: `Segment` (mapped or copied), `AttachMode`, typed views (`as_slice::<T>`, zero-copy only for native order and alignment), `to_f32_vec`/`to_f64_vec`, and the element probe ported from `impcraft-shm`.
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
  - Windows: the default root is `%LOCALAPPDATA%\commonkit\store` (ImpCraft's practice; §12 leaves it open). The Windows target is not installed on thresher, so the Windows build is unchecked.
- **Found in Python (not changed):**
  - `SharedArrayStore("")` makes a store at the working directory; contract §6.1 says an empty root is disabled. Rust follows the contract. Only direct construction reaches it; `shared_store` never passes an empty root.
  - `test_usage_bytes_must_be_a_non_negative_json_integer` passes without testing validity: its fixture's `reap_interval_s=0` makes every `.usage.json` stale. The Rust port uses 30 s.

## Step 6 review (2026-10-28)

- **Windows type-checks:** `cargo check` and `cargo clippy -D warnings` pass for `x86_64-pc-windows-msvc`, with and without `mmap` (target installed on thresher; nothing runs on Windows yet).
- **Two Python defects the Rust port found, fixed:** `SharedArrayStore("")` was an enabled store in the working directory (contract §6.1 says disabled); and `test_usage_bytes_must_be_a_non_negative_json_integer` tested nothing, because its fixture made every `.usage.json` stale. It now uses a fresh store with an integer control. Python: 551 passed.

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
   - Deferred: CI builds only on stable Rust (the stated minimum, 1.95, is unchecked); the integration tests read `../../conformance` and must be excluded from the crate package before publishing.
3. **Steps 7–10:** the reaper in Rust (replacing the step 6 stub, with `reaper_paths.json` and the Python reaper tests listed in `rust/commonkit/tests/store.rs`), cross-language conformance, then the ImpCraft and PlumCraft migrations.
4. **The old repository** `seantrue/commonkit`: still holds the step-2 work uncommitted on its `rust-crate-plan` branch; what to do with it (archive, or a pointer README) is the maintainer's call.

# AI Contributions — pycommonkit

*About AI slop.* Fair enough — most of this package was written by a machine,
and so was most of this file. Call it instant ramen: fast, cheap, genuinely
edible, and nobody should pretend it simmered on a stove all afternoon. The
honest answer is not a claim about quality, it is the record below — what the
maintainer specified, what the model wrote, what the model got wrong, and which
of those mistakes were caught by the test suite, by mutation probes, or by
cross-process benchmarks rather than by luck. Several of the invariants this
package enforces exist *because* of a defect written down here. Where AI-written
code shipped one, it is recorded in the same detail as the wins.
Bon appétit, but mind the sodium content.

The original implementation was pixel centric, but the framework supports any
numpy array.

Tracks what AI helpers contributed to pycommonkit (and to commonkit, the repository it was lifted from on 2026-10-27, whose ledger continues here) versus the human maintainer.

## Human maintainer

The architecture and the load-bearing decisions are the maintainer's, and they
predate this package — commonkit is an extraction of code that ran in production
inside a non-public application first.

- **The shape of the problem.** An ingest stage, then branching flows whose path
  depends on analysis done in flight, with digests as the key to either finding
  pixels already in memory or loading them into it. Offered as a correction to
  the impoverished linear flows the AI had been benchmarking against.
- **The read-only mechanism** (`a.flags.writeable = False`), and the requirement
  that the store survive back-pressure rather than reading a whole stream into
  memory behind a jam. That requirement is why `_admit` declines instead of
  blocking, and why a decline is normal operation rather than a fault.
- **The storage format, indirectly but decisively.** The observation that what
  is actually wanted on disk is the raw `.shape`, `.dtype` and `.data` triple is
  what sent the AI to check whether `np.save` does more than that. It does not
  materially — a 128-byte header, payload 64-byte aligned, 0.93 ms against
  0.89 ms for a bare buffer write — and that settled `.npy`.
- **The name.** *"Extract shm into a package 'commonkit' named for Fortran
  common."* A Fortran `COMMON` block is a named region several separately
  compiled programs agree to share, which is this contract exactly.
- Direction, review and acceptance throughout, including several corrections
  recorded below.

## Contributions

### 2026-10-28 — The PyPI distribution renamed to `pyuncommonkit` after PyPI refused `pycommonkit` (Claude Opus 5.5, Claude Code)

**What happened.** The maintainer's upload of `pycommonkit` 0.1.0 to PyPI returned 400 Bad Request with no reason shown. The AI asked for a `--verbose` run; the maintainer then reported the cause: the name was too close to `commonkit`, an unrelated existing project. TestPyPI had accepted the same files the day's earlier trial, because TestPyPI has no `commonkit`. The AI's walkthrough had presented the TestPyPI run as the trial that catches problems; it cannot catch this one. The plan (2026-10-27) had checked only that `pycommonkit` was unregistered, not whether PyPI would accept it.

**What the maintainer decided.** "let's try pyuncommon". The AI checked and reported that PyPI has an `uncommon`, so `pyuncommon` would likely be refused the same way, and offered `uncommonkit`, `pyuncommonkit` (both free, with free stems) or `pyuncommon` anyway. The maintainer chose `pyuncommonkit`.

**What the AI did.** Renamed the distribution only where it names what is installed or depended on: `pyproject.toml`, `uv.lock` (regenerated), the README's and `CLAUDE.md`'s "installed as", `SECURITY.md`, `STATUS.md`, and the plan (a dated amendment under the original naming decision, and the consumer dependency lines of section 9.3) (~23 LOC, lock excluded). The repository and project name `pycommonkit`, `import commonkit`, the crate, the contract's "since pycommonkit 0.1.0" labels, the vectors and this file's history are unchanged. Checked: the build makes `pyuncommonkit-0.1.0` wheel and sdist, the wheel holds `commonkit/` and says `Name: pyuncommonkit`, `twine check` passes, 629 tests pass, ruff clean, `uv lock --check` consistent.

### 2026-10-28 — PyPI metadata for pycommonkit: links, classifiers, keywords (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Walk me through the pypi publish", then yes to the first step, the `pyproject.toml` polish; then CPM.

**What the AI did.** `pyproject.toml` (~26 LOC): `[project.urls]` (homepage, repository, issues, the contract, the Rust crate), keywords matching the crate's, and 14 classifiers, all checked against `trove-classifiers` (valid, none deprecated). No `License ::` classifier, since PEP 639's `license` expression replaces it. Python 3.11–3.14 are listed because the suite passed on each: CI covers 3.11 and 3.13, and the AI ran 3.12 and 3.14 locally (566 passed each, conformance excluded). Windows is left out with a comment: the contract supports it, but the Python package has no Windows default root and no CI there. "4 - Beta" is the AI's choice, flagged for the maintainer. Checked: the rebuilt wheel's METADATA carries every field; `twine check` passes on wheel and sdist; 629 tests pass; ruff clean; `uv.lock` unchanged. The wheel holds only the six modules and `LICENSE`, the sdist only `src/`, `pyproject.toml`, `README.md` and `LICENSE`.

### 2026-10-28 — commonkit 0.1.0 published to crates.io (the maintainer; Claude Opus 5.5, Claude Code, for the tag and records)

**The release was the maintainer's.** They made the crates.io account and token, and ran `cargo publish -p commonkit` from a clean `main` at `cdffb50`, after PR #10 had passed CI. Their first attempt, before PR #10 was committed, was refused by cargo for uncommitted changes; the AI advised committing through CI rather than `--allow-dirty`.

**What the AI did.** Made `msrv` a required check on `main` (at the maintainer's request); confirmed the published crate through the crates.io API (0.1.0, MIT, `rust_version` 1.95, the repository link); tagged `cdffb50` as `commonkit-v0.1.0` (annotated) and pushed the tag; updated `STATUS.md` (~5 LOC). The docs.rs build had not appeared when this was written (its pages still answered "no such crate"). It appeared minutes later: `status.json` reported `doc_status: true` for 0.1.0, and the crate's docs page loaded (checked by the AI after PR #11 merged).

### 2026-10-28 — crates.io prep: package contents, a crate README, the MSRV checked, two CI defects found (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Prep for crates.io", after the public-repository prep; then CPM. The maintainer's attempt to publish before committing was refused by cargo, and the AI advised against `--allow-dirty` so that 0.1.0 matches a commit that passed CI.

**What the AI did.** `exclude = ["tests/"]` and `readme` in `rust/commonkit/Cargo.toml` (~5 LOC); `rust/commonkit/README.md` (~75 LOC), compiled as a doctest from `lib.rs` (~6 LOC) so its example tracks the API; Rust 1.95 installed and the crate built and tested on it; a CI `msrv` job, a packaged-crate build, a package-contents check and a docs build with `-D warnings` (~31 LOC in `ci.yml`); a `STATUS.md` section. Checked: 154 tests pass on stable (workspace), 146 on 1.95 (`commonkit` alone); clippy both feature sets, fmt, `cargo doc` both ways, actionlint 1.7.12; `cargo package` builds the 18 packaged files alone and `cargo publish --dry-run` reaches the upload.

**Defects found, in earlier AI-written work.**

- **A CI check that never ran (step 4).** actionlint (SC2251) showed that in `uv build leaves rust/ out`, `! tar … | grep -q '/rust/'` was not the last line of the script, and bash `-e` ignores a negated command, so only the wheel check could fail the step. Rewritten as `if …; then exit 1; fi`. Neither artefact contains `rust/` today, so nothing slipped through.
- **The stated MSRV was false for the tests.** `rust-version = "1.95"` had never been checked. The library built on 1.95, but `tests/hostile_input.rs` did not: 1.95 infers `T = str` for `pick` from the expected argument type. The AI's first fix, `*rng.pick(…)`, built on 1.95 but failed clippy on stable (`explicit_auto_deref`); `pick::<&str>(…)` passes both.
- **A doc link to a private item.** `Store::reap`'s docs linked `STALE_LOCK_S`, which is not re-exported, so `cargo doc -D warnings` (as docs.rs is strict about) failed; the text gives the value now.
- **The plan's exclusion list was wrong.** `STATUS.md` said to exclude `tests/vectors_*`, `tests/license.rs` and `common/`; three test files outside that list (`store.rs`, `shared.rs`, `hostile_input.rs`) include `common/`, so it would have shipped tests that cannot compile. Caught by reading the `mod common` users before editing.

### 2026-10-28 — Preparing to make the repository public: a secrets scan, private names generalised, `SECURITY.md` (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Scan the current code and repo for secrets and prep for making the repo public"; chose gitleaks when asked which scanner; then, on the findings: generalise the viewer app, keep PlumCraft, call the original application "the non-public app", reword the production-host line to "the deployment host", keep the email, add `SECURITY.md`, switch the banner back to the raw-GitHub URL, and update the GitHub settings.

**Scan.** gitleaks 8.30.1 over every non-merge commit on every branch (11) and over the whole working tree, tracked, untracked and ignored (113.7 MB): no leaks. A grep of every blob in history for private names, host names, paths and addresses found nothing that is not also at `HEAD`, so no history rewrite is needed. The findings and the maintainer's decisions are in an untracked report in `claude-debug/`, left untracked because it names what this change removes.

**Changes.** The viewer app's name and its crate's name are replaced in the plan, the contract, `STATUS.md`, this file, `rust/Cargo.toml` and three Rust comments (~55 LOC across 9 files, by a script of exact replacements, then each one read). The script's first pass doubled one article and left several sentence starts in lower case; both were fixed before commit, and one Rust comment was rewrapped to stay within 100 columns. `SECURITY.md` (~34 LOC): private reporting, supported versions, and what is in scope for a store that unlinks files in a shared root. The README banner is linked by raw-GitHub URL again, which renders only once the repository is public (#8 had switched it to a repo path because of that).

**GitHub settings.** Done: branch protection on `main` requiring the `python`, `rust` and `conformance` checks, with force-pushes and deletion off; Dependabot alerts. Refused while the repository is private: secret scanning with push protection, and private vulnerability reporting, which `SECURITY.md` names as the preferred channel.

Checked: `cargo fmt --check`. Not run: the test suites (only comments and documents changed; CI runs them).

### 2026-10-28 — A README banner from public-domain NAIP imagery; `icons/` with attribution; `uv.lock` tracked (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** An aerial photo of Midwest farmland "with a free to use no attribution license", from Flickr first; then a choice of logo among the maintainer's stylized renderings ("Go with faded-illustration. Add to README in pleasing way"); then "Fetch me an NIAP tile", "Try for iowa or kansas, zoomed out 2x", new renderings of the Finney County tile, "a README with attribution and making the impressionist available under the MIT license", and to track `uv.lock`. The renderings, the cleanup of `icons/`, and every choice of image and style are the maintainer's.

**What the AI did.** Searched Flickr's CC0 / Public Domain Mark results (USDA aerials of Iowa and Illinois, 2850×1900) and downloaded candidates. Ranked the seven renderings as logos. Cut the banner (`docs/images/commons-banner.jpg`, 1200×330, cropped to exclude a feedlot) and put it full width above the README title, with an absolute raw-GitHub URL because the README is also the PyPI description and PyPI cannot resolve relative image paths. Pulled NAIP tiles from the USGS National Map `exportImage` endpoint (USDA's own server reset the connection) and compared six areas in Iowa and Kansas; Iowa's grid has no pivots, Finney County, Kansas, was chosen. Embedded title, credit and public-domain rights in the tile's XMP/EXIF. Wrote `icons/README.md` (~62 LOC): source, bounding box, the reproducing request, public domain for the photograph, MIT (the root `LICENSE`) for the renderings. README ~6 LOC. `uv.lock` is generated.

**Defects and wrong turns, all caught by the AI before commit unless noted.**

- The maintainer's first renderings were made from a Google Maps screenshot; the AI recommended one as the logo and cut a banner from it, flagging only then that it was a copyrighted derivative (the Google watermark and map control were visible in every rendering). The maintainer went ahead, then had the AI fetch NAIP imagery and re-rendered from it; the Google-derived files are not committed.
- The first banner crop used `sips --cropOffset`, which was ignored and produced the centre band; seen on viewing the output and redone with ImageMagick.
- The first pandoc preview added a title heading GitHub would not show; re-rendered with `pagetitle`.
- The first NAIP tile (York, Nebraska) was a town edge with no pivots: wrong target, replaced.
- Shipped defect, caught by the maintainer after merge (PR #7): the banner was linked by an absolute raw-GitHub URL, but the repository is private, and raw.githubusercontent.com returns 404 to the unauthenticated fetch GitHub uses to render README images, so the banner did not show. The AI had never checked the repo's visibility, and the PyPI reason for the URL did not apply either: pycommonkit is not on PyPI. Fixed by linking `docs/images/commons-banner.jpg` relatively, which GitHub renders for private and public repositories. If the package is published to PyPI while the repo is private, the banner will not render there.
- The AI tried to query the Flickr API with a key scraped from Flickr's page; the session's permission classifier blocked it, and the search used public pages only.
- "The impressionist" was read as all the renderings (there is no file of that name) and all are put under MIT. The maintainer then said the renderings were made with The Virtual Impressionist (www.virtimp.com), which `icons/README.md` now credits.

Not run: the test suite and ruff (no code changed).

### 2026-10-28 — Step 8: cross-language conformance; a dry-run reap checked on the real tree; `remove(key)` (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Go" for step 8; "What does actionlint do?"; then "Add a way to forcibly remove an existing cache entry (if not in use). This is to support existing users of ../impressionist", choosing no in-use check and pycommonkit only for now; then "CP. One PR".

**Step 8 (the delegated agent).** `rust/ck-conform` (a JSON driver for the crate, unpublished) and `conformance/ck_conform.py` (the same for Python), `tests/test_conformance.py` (pytest calling the Rust driver) and Rust tests calling `uv run python`. Every case in the plan passes in both directions: both languages write all 53 header vectors' files identically (equal to numpy's) and attach each other's; a touch from either side protects a segment from the other's reaper; both reapers reach the same survivors in turn, concurrently and in their dry runs; each breaks the other's stale lock; torn, partial and empty segments and orphan `.tmp` files are handled alike; each trusts the other's `.usage.json`; eight publishers race on one key with no collision; real collisions raise in either language; a mapping survives the other's reaper; `.policy.json` reads alike. The CI `conformance` job runs the suite on Linux. The one disagreement was a test helper writing big-endian datetimes where the fixtures hold little-endian (test-only).

**A dry-run reap** in both languages (contract §9.10, an implementation aid): it takes no lock, writes, touches and unlinks nothing, and returns what it would remove; tested by comparing every entry's lstat before and after. Against thresher's real store (`/Volumes/CommonkitStore/store`) the two dry runs, with a fixed `now`, chose the same 136 of 1,725 segments in the same order (282 MB, the least recently used, to bring 6.29 GB under the 6.01 GB low water mark). A `stat` listing of all 2,392 entries before and after is byte-identical; the AI checked both, and that no lock was left. `actionlint` was not run: it isn't installed and the agent's download was blocked.

**`remove(key)` (the AI).** Impressionist's checkpoint `reset=True` relied on publish rewriting a segment; under C15 a republish is a touch or a `KeyCollision`, so reset silently stopped replacing values. `SharedArrayStore.remove` and `Store::remove` unlink a segment (regular files only, never a symlink, never truncating; no in-use check, so readers keep their bytes), count `shm_removed` / `shm_remove_failed`, and never raise. Contract §6.7, W1's one exception, the mutation rule and the counter table; a README paragraph. Six Python tests, five Rust tests and a cross-language case (removed in one language, replaced in the other). Impressionist itself is not changed yet. A Rust unit test still asserted 20 counters and failed until updated.

Checked by the AI: Python 629 passed, ruff clean; Rust 153 passed with and without `mmap`, clippy clean on macOS and for `x86_64-pc-windows-msvc`, fmt; the real-tree listings identical.

### 2026-10-28 — Step 7: the Rust reaper; a reaper race and a symlink truncation fixed in both languages (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Go" for step 7, "What is .usage.json.tmp", then "Go for fix".

**What the agent did.** `rust/commonkit/src/store/reap.rs` (about 430 lines): refusals, the `O_EXCL` lock with the pid and the 300 s break, the walk without following symlinks with the deadline before each directory, candidates by segment shape, the `.tmp` sweep, eviction toward both targets, `.usage.json` after a complete walk only, the counters and the per-store rate limit; `Store::reap(budget) -> ReapReport`. Publishing now really reaps (C9). About 780 lines of tests: the eight Python reaper tests under the same names, bystanders (non-segments, dot-names, `.pinned/`, symlinks), the 16 reaper-path vectors, a mapping surviving the reaper, stale and held locks, and eight concurrent reapers reaching one reaper's result. Rust was already stricter than Python: per-candidate unlink failures counted, `lstat` for the lock's age, the usage temp file created exclusively.

**Defects the agent found in Python and the contract, fixed by the AI.**

- **Racing reapers over-evicted** (contract §9.2 called the race "harmless"). An ENOENT was skipped without counting the segment toward the targets, so after a broken stale lock two reapers took six segments to none against a low water mark of two. An already-gone segment now counts as freed but not reaped, in Python, Rust and the contract (`ReapReport::gone` in Rust; `.usage.json`'s `files` excludes it). A test in each language fails without the fix (checked by reverting it).
- **Python wrote through a symlink.** `open(".usage.json.tmp", "w")` followed a symlink planted there and truncated its target, which for a mapped segment is SIGBUS for every reader: it broke both "never follow symlinks" and "never truncate". It is now created with `O_CREAT | O_EXCL | O_NOFOLLOW` after removing any leftover. A test fails on the old code.
- **A weak test** from earlier AI work: `test_a_declined_publish_still_reaps` held only because three reap turns' counts added up; it now ages the segments after publishing so only the decline's own turn can reap.
- Per-candidate unlink failures count `shm_reap_failed` in Python too, matching Rust; the contract says so.

Checked: Rust 135 passed with and without `mmap`, clippy clean on macOS and for `x86_64-pc-windows-msvc`, fmt; Python 553 passed, ruff clean.

**README** (the maintainer: "Update the README to reflect this clearly. Do not mention consumers by name", after asking why a caller must be able to recompute a value the cache doesn't hold). The AI rewrote the invariant section as "A cache, not a store of record": the store promises a present key names the right bytes, never that a key is present; every reason a segment can be absent; what that buys; the only two things that raise; keeping a durable copy of anything that can't be recomputed; and that callers never need `.usage.json`. Also removed two stale lines (the Rust crate as "planned", Python as catching up with the contract).

**The Fortran comparison** (the maintainer: "Refine the readme comparison to fortran named common"). The AI's earlier wording was wrong: it called a `COMMON` block a region shared by "separately compiled programs" that "outlives whoever wrote it". A named `COMMON` block is shared by the program units of one executable, joined by name at link time, and lives for that run. The README now says so, and lists where commonkit is stricter: across processes, the layout carried in the header (with `KeyCollision`), write-once and read-only, and possibly absent.

### 2026-10-28 — Step 6: the Rust store; two Python defects the port found (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Go" for step 6, then "Go" for the AI's proposed follow-ups.

**What the agent did** (about 2,400 lines of code and 1,700 of tests in `rust/commonkit`). `Store::new` that never fails, `attach` (mapped or copied) returning `Option<Segment>` with typed views and an element probe, `publish` returning `Result<bool, KeyCollision>` with every gate in contract order, the existing-key rule and `verify_republish` with padding masked, touch at most once per 60 s, `fsync` (`F_FULLFSYNC` on macOS), `.usage.json`, the counters of §11, and `shared_store` with the environment and `.policy.json`. The reap turns are taken at the contract's points; the reaper itself is a stub for step 7. `unsafe` is one `Mmap::map` call in `map.rs` behind the `mmap` feature, its `SAFETY:` comment citing contract §7. Dependencies: `memmap2` (optional), `bytemuck`, `log`, `rustix` (Unix), `fs4` (Windows), `tempfile` (dev). 110 tests pass with and without `mmap`, including ports of Python's store and shared tests under the same names, every read, admission, existing-key, policy and environment vector, and a mapping surviving unlink and rename-over.

**Python defects the port found (fixed by the AI after the maintainer's go).**

- `SharedArrayStore("")` made an **enabled** store in the working directory: `os.path.abspath("")` is the cwd, so the empty-root check never fired. Contract §6.1 says disabled. A new test failed before the fix.
- `test_usage_bytes_must_be_a_non_negative_json_integer` (written in step 3 by an earlier agent) **tested nothing**: its fixture's `reap_interval_s=0` makes every `.usage.json` stale, so it passed whatever the file held. It now uses a store whose file can be fresh, with an integer control proving the file is read.

**Also by the AI.** Installed the `x86_64-pc-windows-msvc` target; `cargo check` and `cargo clippy -D warnings` pass for Windows with both feature sets (type-check only).

### 2026-10-28 — Step 5: keys and `.npy` in Rust; Python's registry compares variant sets (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the agent did** (about 2,800 lines of Rust with tests). `keys.rs`: the grammar in full, schemes, the registry (a closed variant set compared as a set, pattern text compared), ASCII-only digest folding on build and none on parse, variant patterns through the `regex` crate (MIT OR Apache-2.0; missing from the plan's dependency list). `npy/`: a header parser for versions 1–3, both byte orders, Fortran order, trailing bytes and every descriptor class, ported in logic from the viewer app's `shm` crate reader, and a writer of numpy's exact header bytes. Every key vector and all 53 header vectors pass, read from the JSON at test time. The writer builds each header from the vector's array spec, not the fixture, and was checked by breaking it three ways (no growth padding, zero alignment spaces allowed, version 1 for every Latin-1 header): each break failed the vectors. A hand-rolled generator ran about 95,000 hostile cases (every truncation, mutated fixtures, random header texts, write-and-read-back, random keys) with no panic. No `unsafe`.

**Where Rust and Python disagreed, decided by the maintainer ("Yes for all").** The contract says a closed variant *set*; Python compared the tuple, so the same variants in another order conflicted. Python now compares sets (vector `keys.registry.variants_reordered`). The contract says forms are distinct; Python didn't check, and now refuses duplicates (a new test). The Rust writer is stricter than Python on unusual field names (backslash, control or non-letter non-ASCII), which is safe.

**Deferred.** CI checks only stable Rust, not the stated minimum 1.95; the integration tests read `../../conformance` and must be excluded from the crate before publishing.

Checked by the AI: Rust 31 tests (with and without default features), clippy with both feature sets, fmt; Python 550 passed; ruff clean; the generator reproduces the fixtures.

### 2026-10-28 — Step 4: the Rust workspace and CI (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Go" (steps 4 and 5), then "No ci on mac", then "Yes for all".

**What the agent did.** `rust/` workspace with the `commonkit` crate (MIT; a copy of LICENSE in the crate, kept identical by `tests/license.rs`, because setting both `license` and `license-file` warns), the panic lints with a `clippy.toml` for tests, `unsafe_code = "deny"`, an empty default-on `mmap` feature, and `.github/workflows/ci.yml` with Python, Rust and conformance jobs on `ubuntu-latest` only. `uv build` still leaves `rust/` and `.github/` out of the sdist and wheel. The plan now says CI is Linux-only (macOS checked locally on thresher), and no longer says PlumCraft turns `mmap` off (C5).

**The AI** split the agent's single working tree into this commit and step 5's, and checked step 4 builds, tests, lints and formats on its own.

### 2026-10-28 — Step 3: the Python package conforms to the contract (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Cp, and then write some code", then "Yes" to fixing a slower attach before committing.

**What the delegated agent did** (contract Appendix A4–A14; about +930/−300 lines across `store.py`, `shared.py`, `cache.py`, tests and docs):

- **Existing keys (C15):** a torn file is replaced (`shm_torn_replaced`), a whole matching one is touched and counts nothing, and a different dtype, shape or order raises `KeyCollision`, leaving the file and its mtime alone, counting `shm_key_collision` and logging at error level. `verify_republish` compares in 1 MiB chunks with padding masked out. `cache.py` now lets `KeyCollision` through.
- **Never-raise bugs, each reproduced against the old source before the fix:** a garbled header raised `tokenize.TokenError` out of `attach`; an object dtype raised `ValueError` and left a `.tmp` file; a missing `os.statvfs` raised `AttributeError`; `inf` in a setting raised `OverflowError`.
- **Also:** headers up to 1 MiB, `fsync` off by default (`F_FULLFSYNC` on macOS, measured at 3.98 ms against 0.33 ms for plain `fsync` on a 1 MiB write), the reaper by path shape plus the `.tmp` sweep, `.usage.json` validity, the strict number grammar, `.policy.json` over the environment, and a `shutil.disk_usage` fallback where `statvfs` is missing (Windows).
- 216 lines of new tests. The suite went from 375 passed and 87 xfailed to 548 passed and none xfailed; the generator still reproduces every fixture.

**A cost caught by the AI's review, then fixed by the AI.** The agent's mmap attach opened each file twice (prefix check, then `np.load(path, mmap_mode=...)`), costing about 16 µs on the hot path that `cache.py` exists to keep cheap (55 to 71 µs median for a 400 kB segment). The AI mapped through the handle already open (`read_magic`, the public v1/v2 header readers, `np.memmap` on the file object), keeping `np.load` for version 3 headers and empty arrays. The median is now 31 µs, faster than before step 3. Checked: read-only, Fortran order and big-endian preserved, and the mapping survives an unlink.

**Choices the contract left open, made by the agent and listed for the maintainer:** a comparison that itself fails is a failed publish, not a collision; only top-level duplicate keys invalidate `.policy.json`; a BOM makes it invalid; a JSON number that overflows is left out.

### 2026-10-28 — The key fixes, padding in `verify_republish`, ruff clean (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Ensure contract is in pycommonkit. Annotate file to pass ruff. Fix the keys. Special case readback by copy", after the contract agent left a false-collision case unresolved.

**What the AI did.**

- **Keys (contract A1–A3), `keys.py`.** Every pattern is used with `fullmatch`: the `^…$` patterns accepted a key, token or digest ending in a newline, because `$` matches before a final `\n`. Digests fold ASCII letters only (`str.lower` folds far more). A variant pattern must match the whole variant (`re.match` let `sr[0-9]+` accept `sr16000x`) and the variant must be a token (`sr.+` let `sr-1` build a key no parser reads back). Registry identity compares the pattern text (`variant_pattern`): two different patterns used to compare equal through their empty variant tuples. Six vectors moved from xfail to pass.
- **Read back by copy.** Contract §5.8 now defines padding bytes (no named field covers them; `''` descriptor fields become padding when read) and §6.4 says `verify_republish` compares named-field bytes only, so an array whose padding a copying `np.load` zeroed republishes cleanly. Two vectors, `existing_key.padded_padding_differs_verified` (touched) and `padded_field_differs_verified` (collision). Found while doing it: `cache.py` swallows every `publish` exception, so it would hide a `KeyCollision`; recorded under A7.
- **Ruff.** The four `try`/`except`/`pass` handlers in `cache.py` carry `noqa: S110` with their reason; `collections.abc.Iterable`, `X | None` and an unused import in `keys.py` and `read.py`.

Checked: 375 passed, 87 xfailed; `ruff check .` clean; the generator reproduces every fixture byte for byte.

### 2026-10-27 — Lifted into pycommonkit, with the contract and conformance vectors (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "When the contract is done, start a new project in ~/pycommonkit and a new matching repo (private). Checkout the existing code there and start lifting the pieces out of the current project/repo. Clean start but lifted from existing code." Asked to confirm, the maintainer chose `~/workspace/pycommonkit` over the literal `~/pycommonkit`, everything in one repository (Python, the future `rust/`, the contract and the vectors), and a fresh history.

**The contract and vectors (step 2, a delegated agent).** `docs/contract.md` version 1 (about 46 KB): the plan's section 6 as a specification, numbered so code can cite it, with a Windows section left for the maintainer's existing Windows work and Appendix A listing 14 changes Python makes in step 3. `conformance/vectors/` (98 files, about 263 KB): keys, key-to-path, 51 header fixtures across `.npy` versions and dtype classes, reads, admission, the C15 existing-key cases, reaper paths, environment parsing and `.policy.json` precedence. `conformance/generate.py` reproduces them byte for byte (checked by the agent under numpy 1.26.4, 2.1.3 and 2.4.6, and by the AI by regenerating and diffing). `tests/test_vectors.py`: 369 passed, 91 xfailed (strict) for step 3. The agent found a real never-raise bug in today's code: a garbled header lets `tokenize.TokenError` escape `attach`. It also found that `np.load` refuses headers over 10000 bytes, so Python reads no version 2 file.

**The lift (the AI).** Copied `src/`, `tests/`, `conformance/`, `LICENSE` and the plan and contract; renamed the distribution to `pycommonkit` with `module-name = "commonkit"` and SPDX licence metadata (the wheel builds as `pycommonkit-0.1.0` with `License-Expression: MIT`); adapted `README.md` and `CLAUDE.md` (the contract, the cross-language rule, the coming `KeyCollision`, Windows); carried this ledger over; added `docs/STATUS.md`. No source line in `src/` changed.

### 2026-10-27 — A plan for a Rust crate beside the Python package (Claude Opus 5.5, Claude Code, with one delegated agent)

**What the maintainer asked for.** "Make a plan to integrate a crate into the commonkit package." Then the names, "Let's go for a pypi name of pycommonkit and the crate of commonkit", MIT and Python 3.11+, and a decision on every recommendation, several of which overturned the AI's.

**What the AI did.** `docs/rust-crate-plan.md` (553 lines; a PDF beside it, untracked): a pure Rust crate `commonkit` in a `rust/` workspace implementing the same on-disk contract as Python, a written `docs/contract.md` as the authority, shared conformance vectors across both languages, mmap behind one audited module, migration of the viewer app's `shm` crate and PlumCraft's `plumcraft-store`, the rename to `pycommonkit` (checked on a scratch copy with uv 0.12.5: `module-name = "commonkit"` is required; PEP 639 licence metadata works), 16 inconsistencies found between the three implementations, and a ten-step plan of 17–21 days. A delegated agent wrote the first draft; the AI reviewed and edited it.

**The maintainer's decisions and corrections (C1–C17).**

- **Write-once rules** ("Add the contract rules"), after asking "How does an array get torn if it can only be written to once?": the plan had called a torn file a risk without saying it can only come from outside the contract. **fsync** became a policy option, "default to no".
- **Windows (C10).** The AI's plan recommended dropping the viewer's `shm` crate's Windows branch, on its own assumption that only macOS and Linux mattered. The maintainer: "Just don't delete support from shared infrastructure like commonkit. There is commonkit work already complete on windows, and we will merge it in later." A defect in the plan, caught by the maintainer.
- **`.policy.json` (C7):** obeyed if present, and the file wins over the environment.
- **mmap (C5):** default-on, and PlumCraft does not turn it off (the plan had it reading by copy).
- **Republishing (C15).** The AI recommended rewriting an existing key, for parity with Python. The maintainer: "Why would I rewrite an existing file?", then: a different array under a whole file's key "is not torn, that's a major system error". Now a torn file is replaced, a matching one touched, and a collision raises `KeyCollision` (Rust `Err`), with `verify_republish` comparing bytes in CI. A wrong recommendation, caught by the maintainer.
- **`cache.py` (C14):** left out of Rust, after the maintainer asked what it does.
- C1–C4, C6, C8, C9, C11–C13 as recommended.

**Not done.** No code, no rename, nothing published. The names were free on PyPI and crates.io on 2026-10-27.

### 2026-10-16 — The reaper: run on declines, evict toward the free-space floor, finish eviction (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Commonkit is supposed to LRU. Check the logic and tests." Then: "Yes to fix, 7GB, manual reap."

**What was found.** On the maintainer's machine the shared store had sat full for 5.7 days:

- the last reap was 137.8 hours earlier;
- 1,104 segments were past their 7-day TTL;
- every publish was declined (`shm_declined_space`).

The eviction order itself was correct LRU. A probe (`claude-debug/lru_probe.py`, untracked) showed it evicting oldest-first and keeping a recently attached segment. The defects were in when the reaper ran and what it aimed for:

1. **It ran only after a successful publish** (`_maybe_reap` at the end of `publish`). A store that filled declined every publish from then on, so nothing reaped it, not even TTL expiry. Reproduced: 4 expired segments survived 10 declined publishes.
2. **The free-space floor was a publish gate, not an eviction target.** The maintainer's 9.3 GiB quota volume, with `MAX_GB=8` plus `MIN_FREE_GB=1`, filled before the byte budget did. Reproduced: `reap()` with free space below the floor and usage under budget evicted nothing.
3. **The 50 ms time budget also cut eviction short.** The last reap left the store at 7.6 GB, above its 6.4 GB low water mark, and the next 361 publishes landed inside the 30-second reap interval.
4. **Latent:** a walk cut short by the budget wrote its under-counted total to `.usage.json`.

None of this was tested. The reaper tests covered only the root guard, files that aren't segments, and fresh segments left alone. These defects are in AI-written code from the 2026-09 extraction.

**What the AI did:**

- **The fixes** (`store.py`):
  - space and budget declines call `_maybe_reap()`;
  - eviction has a second LRU target, the free-space floor plus 25% headroom, measured fresh;
  - `budget_s` bounds only the walk, and the deadline is checked before each directory;
  - a partial walk records `complete: False`, skips budget eviction and doesn't write `.usage.json`.
- **6 tests** in `tests/test_store.py`: LRU order with an attach refresh, TTL expiry, reaping on a declined publish, free-space-floor eviction, eviction past the time budget, and a partial walk. 72 tests pass.
- **The maintainer's environment:** `COMMONKIT_STORE_MAX_GB` changed from 8 to 7 in `~/.bashrc`.

**Defects in this change's own work, caught by its tests:**

- **The first version checked the walk deadline after each directory,** so a walk that finished its last directory late was marked partial with nothing skipped. Caught by the eviction-past-the-budget test; the check now comes before each directory.
- **Three new tests had wrong expectations as first written:** a miscount of segments under the low water mark (`.npy` headers), ages past the TTL, and fake clocks that didn't move the deadline.

### 2026-10-02 — `shared_store(attach=...)`: how a caller reads is its own choice (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** "Do the follow ups". One follow-up was moving the non-public app onto the shared store, and the non-public app picks `mmap` or `read` attach per process (`PIXEL_SHM_ATTACH`).

**What the AI did.** It added an `attach` parameter to `shared_store()` (default `mmap`; ~6 LOC) and a test (~5 LOC). How a process reads affects only that process, so `attach` sits with `mode` and `min_array_bytes` as a per-caller choice, not tree-wide policy. 66 tests pass.

### 2026-10-02 — `shared_store()`: one store per host, with tree-wide policy from the environment (Claude Opus 5.5, Claude Code)

**What the maintainer asked for.** While adopting commonkit as Virtual Impressionist's checkpoint store, the maintainer pointed out that "all instances of the commonkit cache are supposed to share a common backing store directory with age out" and asked whether separate stores were costing memory. Offered two options, the maintainer chose to define the shared defaults in commonkit rather than reuse the non-public app's `PIXEL_SHM_*` settings.

**What the AI did.** The AI added `src/commonkit/shared.py` (~85 LOC), exported `shared_store` from `__init__`, and wrote `tests/test_shared.py` (~64 LOC, 5 tests). It also updated `README.md` and `CLAUDE.md`.

- **Tree-wide policy comes only from the environment.** The root, byte budget, TTL, free-space floor, reap interval and largest array come from `COMMONKIT_STORE_*` variables or `DEFAULTS`, never from a caller. The reason is the existing reaper design: whichever process reaps applies its own values to every segment in the tree, so a per-caller override would silently win over everyone else's.
- **Callers choose only what concerns their own publishing:** `mode` and `min_array_bytes`. A `root` argument exists for tests.
- **Defaults are the non-public app's measured macOS values:** `~/Library/Caches/commonkit/store`, a 2 GB budget (because Time Machine local snapshots pin reaped segments), 900 s TTL, and an 8 GB free-space floor. On Linux the default root is `/dev/shm/commonkit` with 8 GB.
- **Bad environment values fall back to the defaults** rather than raising, consistent with the never-raise invariant.

**Answer to the memory question, as measured and read.** RAM is not lost: segments are clean, file-backed, read-only mappings, held once in the unified buffer cache and not charged to `phys_footprint` (the non-public app's measurement). Disk was the cost: two trees meant two budgets, and on macOS each tree's turnover pins snapshot space. The consumer's separate 8 GB tree was a mistake by the AI, corrected by this change before anything was written to it.

**Testing.** The full suite passes (65 tests, 5 new). ruff's six findings are all in pre-existing files (`cache.py`, `keys.py`, `read.py`).

**Process slip.** The first commit of this change went out without this entry, because an edit to `AI.md` failed and the commit ran anyway. The AI caught it before merging, and the entry was added as a follow-up commit on the same PR.

**Not done.** The non-public app still configures its own tree through `PIXEL_SHM_*`. Moving it to `shared_store()` is a separate change in that repository.

### 2026-09-18 — CLAUDE.md, clearer dtype tests, sox and ffmpeg end-to-end tests (Claude Opus 5, Claude Code)

**What the maintainer asked for, and the correction that shaped it.** After a
CLAUDE.md was generated (`/init`, ~95 LOC), the maintainer asked what dtype
`wav16k` was "vs the form", then pointed at `tests/test_store.py` where
`wav16k` looked like a dtype, then asked *"where is the mapping for f32chw ->
fp32?"* There was none. The pre-existing suite, written by an AI during the
extraction, used `f32chw` -- a name that says float32 -- as its example of an
*unconstrained* form. That is correct code aimed at a misleading target, and
the maintainer caught it by reading the tests, not any test run. Asked to "make
the tests and configuration clear about what they are testing": `f32chw` now
declares float32, a new `raw` form is the explicitly unconstrained one
(parametrised over four dtypes), a new test shows each form is held to its own
entry, and comments state that `dtypes` maps FORM -> dtype and that a form's
name enforces nothing (~28 LOC changed across `test_store.py`, `test_keys.py`).
A mutation probe -- the store's dtype lookup forced to `None` -- failed exactly
the two dtype-policy tests.

**Real media, end to end, at the maintainer's request.** *"Add tests that will
run if sox is installed, dump sample audio to raw using sox, and then map it
in"*, then, after asking whether video had an equivalent, *"yes, add the ffmpeg
tests"*. `tests/test_sox.py` (~116 LOC, 4 tests) and `tests/test_ffmpeg.py`
(~142 LOC, 5 tests) render from each tool's built-in generator (`synth`,
`lavfi testsrc`), so no media is checked in and the digest names the source
while rate and encoding become variants and forms. Each publishes, attaches as
a read-only `np.memmap`, attaches again from a child process by key alone,
stores two rates side by side, and refuses a float dump under the integer form.
The video file also pins that an over-ceiling clip is declined whole. Both skip
when their tool is off `PATH`, checked by running them with a stripped `PATH`.
Per-frame chunking was deliberately left out: the key has no frame-index field,
and where one goes is a design decision, not a test's. README and CLAUDE.md
gained a short section on these tests (~23 LOC in README).

**Defects in this AI-written change.** One: the sox scheme was first written as
`dict(...)`, which ruff's C408 flagged; rewritten as a literal. Suite: 60
passed with both tools installed.

### 2026-09-18 — Extracted into its own package (Claude Opus 5, Claude Code)

**What moved, and what that means for attribution.** Most of the source is not
new. `store.py` (442 LOC) moved essentially verbatim — it was already numpy-only
— and `cache.py` (118) is a class lifted out of a module whose application-coupled
half stayed behind. `keys.py` (207) is the one substantially rewritten file.
`read.py` (49) is the counter vocabulary, reduced to the genuinely shared part.
Tests are new: 448 LOC across three files. Measured, not estimated.

**The generalisation that made the claim honest.** The key was
`pixels:v3:{form}:{digest}:o{token}`, with orientation welded into the regex. It
is now `{domain}:{version}:{form}:{digest}:{variant}`, built by a `KeyScheme` a
domain instantiates — images bind `o0`/`o1`/`oa`, a waveform store would bind
`sr16000`. Schemes register by domain so `parse()` and `relpath()` dispatch on a
key alone, which is what lets one store hold several domains without growing a
parameter for something the key already says. Keys and paths are **byte-identical**
to before, because the consuming application builds the same string
independently on its other code path and a disagreement between the two is the
worst failure this design allows: no exception, no counter, just a cache that
quietly never hits again.

**A real fix the move forced.** `_admit_dtype` contained `if form != "pil":
return True` — a pixels policy sitting inside the supposedly generic store,
which would have shipped as genericity in name only. The dtype rule is now
*declared* by the scheme and merely enforced by the store, so a float32 tensor
or an int16 waveform is unconstrained without the store knowing anything about
images.

**Defects in this AI-written change, and how each was caught.** An edit script
buffered its edits and wrote only at the end, so for a file edited twice the
second replacement clobbered the first; two independent gates caught it before
anything was committed. A later script aborted mid-way on an anchor whose
indentation was wrong — the assert-before-write guard turned that into a clean
abort rather than a half-copied file. Two of the new tests were wrong rather
than the code: domain names containing underscores are illegal under the
scheme's own `[a-z0-9]+` rule, and an upper-case digest is *folded*, not
refused — deliberate, because the consumer's lenient key builder does not fold
case, so that test became a positive pin instead. Verifying a PNG-embeds-ICC
claim by searching the saved bytes was unsound, since PNG stores the profile in
a zlib-compressed `iCCP` chunk; the conclusion held but the method did not, and
it was replaced with a round-trip.

**Method.** The equivalence proof is the consuming application's own shm suite,
run **unmodified** after the move: 141 tests. Not touching them is what makes
them evidence. commonkit's own 47 tests drive the generic layer through a
deliberately **non-pixels** scheme (audio, variant axis sample rate), because a
suite that only ever addressed images would not demonstrate the genericity being
claimed; one test pins the pixels spelling byte-for-byte. Five mutation probes
against the parts rewritten rather than moved — the scheme's declared dtype, the
store's enforcement of it, the registry's refusal of two disagreeing schemes,
the two-level path sharding, the case fold — each failed exactly the pin meant
to catch it.

### Measured after the extraction (same day)

Re-ran the pre-existing cross-process harnesses rather than writing a new one,
so the numbers stay comparable to the runs that predate the move. Dataset: a
pinned 48-image set, identical on both hosts.

* **Memory sharing is the robust, platform-independent result.** On Linux,
  121.50 MB Pss for 4 consumers against 509.0 MB unshared — 4.2×. On Apple
  Silicon, 11.50 MB of footprint for 4 attachers against 570.77 MB for 4 private
  decoders, and 23.15 MB against 1160.32 MB at 8. All mappings resolve to 48 VM
  objects, resident once.
* **Decode-once holds:** 192 (and 384) attaches, 0 decodes.
* **The time win is modest, and one reported figure was wrong.** A first Linux
  run showed 17.97×; that was an artifact. The harness runs its control first
  with no warm-up, so it paid cold page cache for 127 MB of source images.
  Three successive runs settle at control 7.01 s → 1.01 s → 0.94 s and the
  speedup at **~2.6×**. The inflated number was retracted rather than quoted.
* **No wall-time win at all on Apple Silicon**, and one-shot *including* the
  publisher is slower: 1.72 s against 0.92 s. The store pays for re-reads, not
  for a single pass. Said plainly rather than letting the table imply otherwise.

## Inherited record: defects from before the extraction

These were found while this code ran inside the application it came from. They
are kept because several are the reason a line in the shipped source looks the
way it does — deleting them would leave the invariants looking arbitrary.

- **The writability probe used a fixed filename.** Concurrent constructors
  raced: the first unlink won, every loser's `FileNotFoundError` was caught as
  "unusable root", and that process ran permanently without a cache and without
  a log line. Measured at 8 of 16 disabled. It is why the probe name carries a
  pid and thread id, and why `test_concurrent_construction_never_disables_a_store`
  exists.
- **The two-tier cache double-counted.** `TieredPixelCache` ticked `shm_hit` and
  `shm_publish` itself while the store it delegates to counted the same events,
  and `drain_stats()` sums the two — inflating exactly the hit rate the counters
  exist to measure. The tier now leaves those to the store.
- **A hit rate was inferred from counter deltas and was wrong for two days.** A
  caller classified by reading `shm_hit` off the tier, which deliberately does
  not tick it, so every cross-process attach was reported as a decode; a correct
  production measurement was retracted on the strength of it. That is why
  `get_with_source` returns which tier answered instead of leaving it to be
  inferred, and why `read.py` owns the counter names.
- **L1 did not earn the argument first made for it.** The claim was that two
  tiers were settled by a loop that looks the same digest up 36 times; the
  premise was true and the conclusion did not follow, because an attach is only
  ~3 ms. At `max_items=64` L1 absorbed every lookup and was still the slowest
  configuration measured, and the default dropped. L1 removes syscalls; it is
  not the reason the store is worth running.
- **Three harness bugs preceded any trustworthy number**, each giving a
  plausible wrong answer: forked children inheriting the parent's DB sockets
  (every lookup raised, the exception was swallowed, and the run reported zero
  decodes *and* zero hits); Pss reading 0 because only a `[:1]` slice was
  retained, so the pages never faulted in; and an unsynchronised measurement
  reporting 2.0× for a 4-way share because each consumer sampled whenever it
  happened to finish.
- **A maintainer's question exposed what the harness could not see.** Asked
  whether the processes had all seen an empty cache and decoded in parallel, the
  specific diagnosis turned out not to be what happened — but the question
  revealed the harness pre-published before forking and so could not observe a
  thundering herd at all. Adding a cold run showed the herd is nearly total.
- **On Apple Silicon, three instruments were wrong before any number was
  reported**: a Mach task port cached at import so forked children saw no
  regions; footprint in forked children dominated by copy-on-write noise, which
  forced spawned children; and a Spotlight query that "found" files a global
  index never returned.

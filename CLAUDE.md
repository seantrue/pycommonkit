# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

The build backend is `uv_build`; dev tools are in the `dev` extra.

```bash
uv run --extra dev pytest -q                                   # full suite (<1s)
uv run --extra dev pytest tests/test_store.py::test_publish_then_attach_returns_the_same_samples
uv run --extra dev ruff check .                                # line-length 100
uv build                                                       # wheel + sdist (pycommonkit)
uv run python conformance/generate.py && git diff --exit-code conformance/   # vectors reproduce
```

**Cross-language rule:** a behaviour change lands in the contract, the vectors and every
implementation (Python now, `rust/` later) in one PR.

`requires-python = ">=3.11"` is deliberate: the consumers are python-3.11 model
venvs. Don't use 3.12+ features, and don't add dependencies beyond numpy.

## What this is

**pycommonkit** (installed as `pycommonkit`, imported as `commonkit`), lifted from
`seantrue/commonkit` with a fresh history on 2026-10-27. Read `docs/STATUS.md`, then
`docs/contract.md` (the on-disk contract, version 1: the authority) and
`docs/rust-crate-plan.md` (decisions C1–C17, all the maintainer's, and the step plan).

Named shared-memory segments for numpy arrays: one process decodes and
publishes a `.npy` file under a root directory (typically `/dev/shm`), other
processes `np.load(mmap_mode="r")` the same physical pages. Extracted from a
private image-corpus app; the module docstrings carry the production
measurements that justify each design choice — read them before changing
behavior.

## The invariant

**A missing segment is never an error, only a slower path.** Nothing on the
read/publish path may raise into a caller: absent root, full disk, reaped or
torn segment, bad key, wrong dtype all return `None`/`False` and tick a
counter in `self.stats`. The `SharedArrayStore` constructor never raises — an
unusable root sets `disabled` and every method becomes a no-op. The one
exception is `KeyScheme.key()`/`parse()`, which raise `ValueError` loudly at
the call site; the store catches that and counts `shm_bad_key`. The contract
adds a second loud path (decided, C15, lands in step 3): publishing a different
array under an existing key is a key collision, a major system error, and raises
`KeyCollision`.

## Architecture (src/commonkit)

- `keys.py` — `KeyScheme` defines one *domain*'s vocabulary. Key
  `{domain}:{version}:{form}:{digest}:{variant}` ↔ path
  `{form}/{digest[:2]}/{digest}.{variant}.npy`. `version` is the decode
  contract (bump to orphan old segments), `form` the kind of array, `variant`
  what distinguishes renderings of the same source (closed tuple, or a regex
  string for open axes like sample rate). Optional `dtypes` per form are
  enforced at publish. Schemes live in a **module-global registry keyed by
  domain**; the store dispatches on the key alone, so one store holds many
  domains. Re-registering an identical scheme is idempotent; a disagreeing one
  raises. The strict token/md5 regexes are a safety boundary: they are what
  keeps a key from naming a path the reaper would unlink.
- `store.py` — `SharedArrayStore`. Modes `off`/`read`/`readwrite`; attach
  `mmap` (read-only enforced by numpy) or `read`. Publish writes a
  pid/thread-unique tmp file then `os.replace` (atomic). `_admit` gates
  cheapest-first: min size, max size, statvfs free-space watermark (cached 1s),
  byte budget from `.usage.json`.
- **The reaper.** It runs, rate-limited by `reap_interval_s`, after every publish **and every space or budget decline**. Until 2026-10-16 it ran only after a successful publish, so a full store was never reaped again.
  - It holds `.reap.lock` (O_EXCL, broken after 300 s).
  - It walks only the root, and `lstat`s and unlinks only regular `.npy` files.
  - It evicts TTL-expired segments, then least recently used first (oldest mtime) until **both** targets are met: 80% of `max_bytes`, and the free-space floor plus 25% headroom. Before 2026-10-16 the floor only gated publishes, so a volume smaller than `max_bytes + min_free_bytes` filled and stuck.
  - `budget_s` bounds the **walk** only; eviction always reaches its target. The deadline is checked before each directory.
  - A partial walk neither evicts for the budget nor writes `.usage.json`, the cross-process usage figure, because it would under-count.
- LRU uses explicit mtime touches (at most once per 60 s), not atime.
- Unlink-while-mapped is safe, so no refcounting exists.
- `cache.py` — `TieredPixelCache`: L1 in-process memo (anything with
  `get`/`set_array`) in front of the L2 store. L1 memoises the *attached
  mapping*, not a copy. `get_with_source` returns `(arr, "l1"|"shm"|"decode")`
  so callers never infer the tier from counter deltas. The store counts its own
  hits/publishes; the tier must not double-count them (`drain_stats` sums both).
- `shared.py` — `shared_store()`: the one per-host store all users share. Tree-wide policy (root, `MAX_GB`, `TTL_S`, `MIN_FREE_GB`, `REAP_S`, `MAX_ARRAY_BYTES`) comes only from `COMMONKIT_STORE_*` env vars or `DEFAULTS` (macOS: `~/Library/Caches/commonkit/store`, 2 GB because Time Machine local snapshots pin reaped segments; Linux: `/dev/shm/commonkit`, 8 GB). Callers pick only `mode` and `min_array_bytes`; a `root` argument exists for tests. Form names are global across domains in the shared tree.
- `read.py` — `note()` (the single writer of `{prefix}{source}` counters,
  raises on an unknown source) and a never-raising `attach()` helper. Not
  re-exported from `__init__`.

Both store and cache satisfy a consumer's `PixelCache` protocol
(`get`/`set`/`set_array`/`get_with_source`/`drain_stats`).

## Tests

Tests deliberately use non-pixel domains (audio: `wav16k`, `sr\d+` variants)
to prove genericity. Because the scheme registry is process-global, **each
test module must register a unique domain name** (`ckaudio`, `cktier`, ...) or
it will collide with another module's scheme. Store fixtures lower
`min_free_bytes`/`min_array_bytes` and set `reap_interval_s=0` so gates and
the reaper are exercisable in `tmp_path`.

`tests/test_sox.py` renders a `synth` sine with `sox` to raw samples and maps
them through the store, including from a child process. It skips when `sox` is
not on PATH. `tests/test_ffmpeg.py` does the same for video: an ffmpeg
`testsrc` clip dumped as `rawvideo` and stored as one `(T, H, W, C)` array,
with the frame rate as the variant (`fr10`). It skips without `ffmpeg`.

## Repo conventions

- Never remove or preclude Windows support: it is a supported platform, and
  Windows commonkit work done elsewhere will be merged in (contract §12).
- Scratch scripts and probes go in `claude-debug/` (only `.md` notes are
  tracked; never delete what's there). Reports are Markdown with blank lines
  after headers and around tables, converted to PDF with pandoc; PDFs are not
  tracked.

- `AI.md` is the ledger of AI vs. maintainer contributions, including defects
  AI-written code shipped; update it (via the `ai-md` skill) with each commit.
- Comments explain *why*, often with a measurement; keep that style.

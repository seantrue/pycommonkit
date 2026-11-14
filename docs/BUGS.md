# Bugs

Bugs reported in passing (`bug: …`), kept until fixed. Numbers are never reused. Entries with an issue are also tracked on GitHub.

## Open

## Fixed

### B2. `ck-conform`'s Python child reads the live default root's `.policy.json`

- Reported: 2026-11-13 (found by Claude while fixing B1, not reported by the maintainer)
- Context: `.github/each_algo.sh cargo test` on the maintainer's Mac, on unpatched `main` (`cdfe242`). Under md5, `ck-conform/tests/python.rs` `digests_agree_with_python` fails (`left: "b3", right: "md5"`), and the harness then fails with `under md5, a .started marker names another algorithm`. The Python suite's own md5 run hits the same `.started` failure (all 705 tests pass). CI is green because its runners have no such file.
- Cause: `python()` in `rust/ck-conform/tests/python.rs` removes every `COMMONKIT_STORE_*` variable from the child, so the Python driver resolves the platform default root (`~/Library/Caches/commonkit/store`). Its `.policy.json` holds `{"HASH": "b3"}`, which wins over `COMMONKIT_HASH` (D1), so the child hashes with b3 under an md5 run and writes its `.started` marker to the run's mark folder. That is the leak D25 E forbids: a test reading a live tree. The Python suite's source for the stray marker isn't confirmed yet.
- Issue: none
- Closed: 2026-11-13, md5-specific. It shows only when the live tree's `HASH` differs from the run's algorithm, i.e. a legacy or user md5 setting, and the harness and CI now run b3 only. The maintainer: "Close B2 if it's md5 specific". Not fixed: the child still reads the default root's `.policy.json`.

### B1. Rust `testing::check_isolated` rejects a store root that doesn't exist yet under a symlinked temp folder

- Reported: 2026-11-13
- Report: "Add this a bug: for commonkit in the local dev directory"
- Context: impressionist-rs, branch `claude/per-case-parity`, test-affected tier 0 on macOS (commonkit 0.2.0 from crates.io). `vimp checkpoint::tests::hash_is_what_was_asked` panics at `rust/commonkit/src/testing.rs:56`: `the store root /var/folders/…/tmp.X/store is not inside COMMONKIT_TEST_RUN=/var/folders/…/tmp.X`. The harness sets `COMMONKIT_TEST_RUN=$(mktemp -d)` and `COMMONKIT_STORE_ROOT=$run/store` without creating `store`.
- Cause: `real()` (`rust/commonkit/src/testing.rs:15`) uses `fs::canonicalize`, which fails for a path that doesn't exist, and falls back to `store::absolute`, which leaves symlinks alone. The existing run folder resolves to `/private/var/…` and the store root that doesn't exist yet stays as `/var/…`, so `starts_with` fails. Python's `commonkit.testing` uses `os.path.realpath`, which resolves symlinks in the part of the path that exists, so it passes; Rust doesn't match it. Linux temp folders aren't behind a symlink, so CI doesn't show it. Confirmed: creating `$run/store` first makes the test pass.
- Fix: in `real()`, resolve the deepest parent folder that exists, then add the rest of the path back on (as `realpath` does). Add a test with a root that doesn't exist yet under a symlinked folder. Release as 0.2.1 and bump impressionist-rs.
- Issue: #21
- Fixed: 2026-11-13, branch `fix/21-check-isolated-symlink` (PR pending). Release 0.2.1 and the impressionist-rs bump not done.

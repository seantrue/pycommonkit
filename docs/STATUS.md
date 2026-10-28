# Status

*Updated 2026-10-27.*

## Where things stand

- **Lifted** from `seantrue/commonkit` (`main` at `4dbfc7d`; the plan, contract and vectors from its `rust-crate-plan` branch) into this repository with a fresh history. The Python package is unchanged apart from its distribution name (`pycommonkit`, imported as `commonkit`), SPDX licence metadata (`license = "MIT"`, `license-files`), and `module-name = "commonkit"` for uv_build.
- **Plan:** `docs/rust-crate-plan.md`. Every decision, C1–C17, is the maintainer's. Section 13 is the step plan.
- **Step 1 (layout) and step 2 (contract and vectors) done:** `docs/contract.md` version 1, `conformance/vectors/` (98 files), `conformance/generate.py` (byte-for-byte reproducible), `tests/test_vectors.py`. The suite is 369 passed and 91 xfailed: the xfails are the behaviour Python adopts in step 3 (contract Appendix A).
- **Nothing published:** PyPI `pycommonkit` and crates.io `commonkit` were free on 2026-10-27.

## Done since the lift (2026-10-28)

- **Keys (A1–A3):** keys, tokens and digests match in full (`fullmatch`; a final newline is no longer accepted), the digest folds ASCII only, a variant pattern must match the whole variant and the variant must be a token, and registry identity compares pattern text. Six vectors moved from xfail to pass: 375 passed, 87 xfailed.
- **Read back by copy:** `verify_republish` compares named-field bytes only; padding bytes, which a copying `np.load` zeroes, never decide a collision (contract §5.8, §6.4; vectors `existing_key.padded_*`).
- **Ruff clean:** the four `try`/`except`/`pass` handlers in `cache.py` carry `noqa: S110` with the reason; `collections.abc.Iterable` and `X | None` in `keys.py` and `read.py`.

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
2. **Step 3:** Python to the contract (Appendix A; A1–A3, the key fixes, are done, including a real never-raise bug: a garbled header makes `tokenize.TokenError` escape `attach`).
3. **Steps 4–10:** the Rust crate in `rust/`, cross-language conformance, then the ImpCraft and PlumCraft migrations.
4. **The old repository** `seantrue/commonkit`: still holds the step-2 work uncommitted on its `rust-crate-plan` branch; what to do with it (archive, or a pointer README) is the maintainer's call.

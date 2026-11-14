"""The harness rule (D25 E, contract §11): every suite runs on a fresh root inside its run
folder (``COMMONKIT_TEST_RUN``), with the algorithm it asked for. ``hash_is_what_was_asked``
is the guard every suite that registers a rule scheme carries; this is commonkit's own.
"""
from __future__ import annotations

import os

import pytest

from commonkit import policy, testing


def test_hash_is_what_was_asked():
    testing.assert_isolated()
    asked = os.environ.get("COMMONKIT_HASH", "").strip().lower()
    if asked:
        assert policy.hash_algo() == asked, (policy.hash_algo(), policy.hash_source())


GUARD = """
    from commonkit import testing
    try:
        testing.assert_isolated()
    except AssertionError as e:
        print("refused:", e)
    else:
        print("isolated")
"""


def test_assert_isolated_refuses_each_unsafe_case(child, tmp_path):
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    cases = {
        "root unset": {"COMMONKIT_STORE_ROOT": None},
        "run unset": {"COMMONKIT_TEST_RUN": None},
        "root outside the run": {"COMMONKIT_STORE_ROOT": str(outside)},
        "default root": {"COMMONKIT_STORE_ROOT": policy.default_root(),
                         "COMMONKIT_TEST_RUN": os.path.dirname(policy.default_root())},
    }
    for name, env in cases.items():
        p, _r, _m = child(GUARD, env=env)
        assert p.stdout.startswith("refused:"), (name, p.stdout)


def test_assert_isolated_still_refuses_under_python_O(child, tmp_path):
    # -O strips assert statements; the guard every adopter's suite relies on must not go
    # with them (finding 5 of the L0 review)
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    for env in ({"COMMONKIT_STORE_ROOT": None}, {"COMMONKIT_STORE_ROOT": str(outside)},
                {"COMMONKIT_TEST_RUN": None}):
        p, _r, _m = child(GUARD, env=env, flags=("-O",))
        assert p.stdout.startswith("refused:"), (env, p.stdout)
    p, _r, _m = child("import sys; print(sys.flags.optimize)", flags=("-O",))
    assert p.stdout.strip() == "1"


@pytest.mark.skipif(not hasattr(os, "symlink") or os.name == "nt",
                    reason="symlinks need privileges on Windows")
def test_assert_isolated_passes_on_a_root_not_yet_made_under_a_symlinked_run(child, tmp_path):
    # issue #21: macOS temp folders sit behind /var -> /private/var and a harness may name a
    # root it hasn't made; Rust refused that before its fix, realpath here never did
    real = tmp_path / "real"
    real.mkdir()
    link = tmp_path / "link"
    link.symlink_to(real)
    for root in (link / "store", link / "not" / "yet" / "store"):
        env = {"COMMONKIT_TEST_RUN": str(link), "COMMONKIT_STORE_ROOT": str(root)}
        p, _r, _m = child(GUARD, env=env)
        assert p.stdout.strip() == "isolated", (root, p.stdout)


def test_assert_isolated_passes_on_a_fresh_root_and_one_already_written(child):
    p, _r, _m = child(GUARD)
    assert p.stdout.strip() == "isolated"
    run, root, marks = child.dirs()
    (root / "plumarr" / "ab").mkdir(parents=True)        # an earlier test wrote here
    p, _r, _m = child(GUARD, run=run, root=root, marks=marks)
    assert p.stdout.strip() == "isolated"


def test_the_guard_fails_when_the_file_overrides_the_variable(child):
    p, _r, _m = child("""
        import os
        from commonkit import policy, testing
        testing.assert_isolated()
        print("same" if policy.hash_algo() == os.environ["COMMONKIT_HASH"] else "differs")
    """, algo="b3", policy={"HASH": "md5"})
    assert p.stdout.strip() == "differs"


def test_fresh_root_is_empty_and_inside_the_run(child):
    p, _r, _m = child("""
        import os
        from commonkit import testing
        r = testing.fresh_root()
        assert os.listdir(r) == []
        assert os.path.realpath(r).startswith(os.path.realpath(os.environ["COMMONKIT_TEST_RUN"]))
        print("ok")
    """)
    assert "ok" in p.stdout

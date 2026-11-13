"""The per-scheme digest rule (contract §3.7, D3 (a)) and the marker files (§11).

A scheme with ``digest="current"`` holds only ``current()``'s digest length: a key of the
other length is declined on publish and a miss on attach, both counted
``shm_foreign_digest``. A scheme with no rule is exactly as before: both lengths, no
``current()`` call, no hashing (the alexandria constraint: its ``pixels`` declares nothing).
"""
from __future__ import annotations

import json
import os
import signal
import stat
import textwrap

import pytest
from conftest import marks_in

from commonkit import testing
from commonkit.keys import KeyScheme

SETUP = """
    import json, os, sys
    import numpy as np
    from commonkit import policy
    from commonkit.keys import KeyScheme, register
    from commonkit.store import SharedArrayStore
    RULE = register(KeyScheme("ckrule", "v1", ("arr",), ("x0",), digest="current"))
    FREE = register(KeyScheme("ckpix", "v3", ("pil",), ("o0",)))      # shaped like pixels
    a = np.arange(16, dtype=np.uint8)
    D32, D64 = "ab" * 16, "cd" * 32
"""
SETUP_ONLY = SETUP


def S(base: str, snippet: str) -> str:
    return textwrap.dedent(base) + textwrap.dedent(snippet)


SETUP = S(SETUP, """
    store = SharedArrayStore(os.environ["COMMONKIT_STORE_ROOT"], mode="readwrite",
                             min_free_bytes=0, min_array_bytes=0, reap_interval_s=0)
""")


def test_a_rule_scheme_under_b3_declines_and_misses_a_32_digit_key(child):
    p, _root, marks = child(S(SETUP, """
        k32, k64 = RULE.key(D32, "x0"), RULE.key(D64, "x0")
        assert store.publish(k32, a) is False
        assert store.attach(k32) is None
        assert store.publish(k64, a) is True and store.attach(k64) is not None
        print(json.dumps(store.drain_stats()))
    """), algo="b3")
    stats = json.loads(p.stdout)
    assert stats["shm_foreign_digest"] == 2 and stats["shm_publish"] == 1
    assert marks_in(marks) == ["shm_foreign_digest", "started"]


def test_a_rule_scheme_under_md5_declines_a_64_digit_key(child):
    p, _root, _marks = child(S(SETUP, """
        assert store.publish(RULE.key(D64, "x0"), a) is False
        assert store.publish(RULE.key(D32, "x0"), a) is True
        print(json.dumps(store.drain_stats()))
    """), algo="md5")
    assert json.loads(p.stdout)["shm_foreign_digest"] == 1


@pytest.mark.parametrize("algo", ["md5", "b3"])
def test_a_no_rule_scheme_like_pixels_takes_both_lengths_and_never_resolves(child, algo):
    p, _root, marks = child(S(SETUP_ONLY, """
        store2 = SharedArrayStore(os.environ["COMMONKIT_STORE_ROOT"], mode="readwrite",
                                  min_free_bytes=0, min_array_bytes=0, reap_interval_s=0,
                                  mark_dir="")
        for d in (D32, D64):
            k = FREE.key(d, "o0")
            assert store2.publish(k, a) is True and store2.attach(k) is not None
        assert policy._HASH is None, "a no-rule scheme resolved the hash setting"
        assert "commonkit.digest" not in sys.modules and "blake3" not in sys.modules
        print(json.dumps(store2.drain_stats()))
    """), algo=algo)
    stats = json.loads(p.stdout)
    assert "shm_foreign_digest" not in stats and stats["shm_publish"] == 2
    assert marks_in(marks) == []                         # mark_dir="" turns marks off
    assert "commonkit: hash" not in p.stderr


def test_started_names_the_algorithm_and_root(child):
    p, root, marks = child(S(SETUP, "print(os.getpid())"), algo="md5")
    pid = p.stdout.strip()
    rec = json.loads((marks / f"{pid}.started").read_text())
    assert rec["algo"] == "md5" and rec["source"] == "environment COMMONKIT_HASH"
    assert rec["root"] == os.path.abspath(str(root)) and rec["pid"] == int(pid)


def test_a_bad_variant_leaves_a_bad_key_marker_with_the_key(child):
    p, _root, marks = child(S(SETUP, """
        bad = "ckrule:v1:arr:" + "cd" * 32 + ":x9"
        assert store.publish(bad, a) is False
        print(os.getpid())
    """), algo="b3")
    pid = p.stdout.strip()
    rec = json.loads((marks / f"{pid}.shm_bad_key").read_text())
    assert rec["key"].endswith(":x9") and rec["counter"] == "shm_bad_key"


def test_a_child_killed_right_after_the_event_still_leaves_its_marker(child):
    p, _root, marks = child(S(SETUP, """
        store.attach(RULE.key(D32, "x0"))
        os.kill(os.getpid(), 9)
    """), algo="b3", check=False)
    assert p.returncode == -signal.SIGKILL
    assert "shm_foreign_digest" in marks_in(marks)


def test_a_missing_mark_folder_is_created_and_unset_writes_nothing(child, tmp_path):
    deep = tmp_path / "a" / "b" / "marks"
    _p, _root, _marks = child(SETUP, algo="b3", marks=deep)
    assert marks_in(deep) == ["started"]
    p, _root, marks = child(S(SETUP, """
        store.publish(RULE.key(D32, "x0"), a)
        print(json.dumps(store.drain_stats()))
    """), algo="b3", env={"COMMONKIT_MARK_DIR": None})
    assert json.loads(p.stdout)["shm_foreign_digest"] == 1
    assert not os.path.exists(marks)


def test_expect_marks_passes_on_exactly_the_named_markers(tmp_path):
    from commonkit.store import SharedArrayStore
    scheme = KeyScheme("ckexpect", "v1", ("arr",), ("x0",))
    from commonkit.keys import register
    register(scheme)
    store = SharedArrayStore(str(tmp_path / "root"), mode="readwrite", min_free_bytes=0,
                             min_array_bytes=0, reap_interval_s=0)
    import numpy as np
    with testing.expect_marks(store, ["shm_bad_key"]):
        store.publish("ckexpect:v1:arr:" + "ab" * 16 + ":zz", np.zeros(2))
    with pytest.raises(AssertionError), testing.expect_marks(store, ["shm_bad_key"]):
        pass
    with pytest.raises(AssertionError), testing.expect_marks(store, []):
        store.attach("ckexpect:v1:arr:" + "ab" * 16 + ":zz")


def test_the_rule_is_part_of_a_schemes_identity():
    from commonkit.keys import register
    register(KeyScheme("ckident", "v1", ("arr",), ("x0",), digest="current"))
    with pytest.raises(ValueError):
        register(KeyScheme("ckident", "v1", ("arr",), ("x0",)))
    with pytest.raises(ValueError):
        KeyScheme("ckbadrule", "v1", ("arr",), ("x0",), digest="md5")


# Markers are first-event-only (finding 4 of the L0 review): the seen-set stops a second
# write in a process, the exclusive create stops one over a file already there.

def _scratch_store(tmp_path):
    from commonkit.keys import register
    from commonkit.store import SharedArrayStore
    register(KeyScheme("ckfirst", "v1", ("arr",), ("x0",)))
    store = SharedArrayStore(str(tmp_path / "root"), mode="readwrite", min_free_bytes=0,
                             min_array_bytes=0, reap_interval_s=0)
    store._mark_dir = str(tmp_path / "marks")
    return store, tmp_path / "marks" / f"{os.getpid()}.shm_bad_key"


def _bad(variant):
    return "ckfirst:v1:arr:" + "ab" * 16 + ":" + variant


def test_a_marker_names_the_first_event_only(tmp_path):
    import numpy as np
    store, marker = _scratch_store(tmp_path)
    store.publish(_bad("x8"), np.zeros(2))
    store.publish(_bad("x9"), np.zeros(2))
    key = json.loads(marker.read_text())["key"]
    assert key.endswith(":x8"), key


def test_a_process_writes_each_marker_once_even_after_it_is_removed(tmp_path):
    import numpy as np
    store, marker = _scratch_store(tmp_path)
    store.publish(_bad("x8"), np.zeros(2))
    marker.unlink()
    store.publish(_bad("x9"), np.zeros(2))
    assert not marker.exists(), "a second event wrote the marker again"


def test_a_marker_never_overwrites_a_file_already_there(tmp_path):
    import numpy as np
    store, marker = _scratch_store(tmp_path)
    marker.parent.mkdir(mode=0o700)
    marker.write_text("planted\n")
    store.publish(_bad("x8"), np.zeros(2))
    assert marker.read_text() == "planted\n"


def test_markers_are_private_to_their_owner(tmp_path):
    import numpy as np
    if os.name != "posix":
        pytest.skip("modes are POSIX")
    store, marker = _scratch_store(tmp_path)
    store.publish(_bad("x8"), np.zeros(2))
    assert stat.S_IMODE(marker.stat().st_mode) == 0o600
    assert stat.S_IMODE(marker.parent.stat().st_mode) == 0o700

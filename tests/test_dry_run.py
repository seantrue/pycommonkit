"""The reaper's dry run (contract §9.10, an implementation aid): it plans exactly what a
reap would unlink and sweep, and changes nothing -- no lock, no unlink, no sweep, no
``.usage.json``, no mtime. Step 8 ran it against a live shared tree, so "changes nothing"
is proved here first, on a temporary one, by comparing every entry's ``lstat`` before and
after.
"""
import os
import shutil
import stat
import time

import numpy as np
import pytest

import commonkit.store as cks
from commonkit import SharedArrayStore

NOW = 1_800_000_000.0                       # a fixed clock, as the live comparison uses


def _segment(root, form, digest, variant, size, mtime):
    path = os.path.join(root, form, digest[:2], f"{digest}.{variant}.npy")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        np.save(fh, np.zeros(size, dtype=np.uint8))
    os.utime(path, (mtime, mtime))
    return path


def _plain(path, mtime, data=b"x"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(data)
    os.utime(path, (mtime, mtime))
    return path


def _tree(root):
    """Ten segments a minute apart, the oldest past its TTL; orphaned and fresh ``.tmp``
    files; bystanders the reaper must never touch; and a stale ``.usage.json``."""
    os.makedirs(root)
    segs = []
    for i in range(10):
        digest = f"{i:02x}" + f"{i:x}" * 30
        segs.append(_segment(root, "raw" if i % 2 else "img", digest, "x0", 1000 + i,
                             NOW - 3700 + i * 60))
    tmp_old = _plain(os.path.join(root, "raw", "01", "orphan.tmp"), NOW - 4000)
    tmp_new = _plain(os.path.join(root, "raw", "01", "writing.tmp"), NOW - 10)
    keep = [_plain(os.path.join(root, ".pinned", "ab", "x.npy"), NOW - 10**6),
            _plain(os.path.join(root, "notes.npy"), NOW - 10**6),
            _plain(os.path.join(root, "root.tmp"), NOW - 10**6),
            _plain(os.path.join(root, ".usage.json"), NOW - 10**6, b'{"bytes": 1}')]
    return segs, tmp_old, tmp_new, keep


def _snapshot(root):
    """Every entry under ``root`` (and ``root`` itself): ``lstat`` mode, size, mtime_ns and
    atime_ns. A directory's mtime changes when an entry is created or removed in it, so
    this also catches a lock or temporary file that came and went. A directory's atime is
    left out: any walk reads directories, this snapshot's own included, and APFS then
    updates it (relatime)."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        for name in [""] + dirnames + filenames:
            p = os.path.join(dirpath, name) if name else dirpath
            st = os.lstat(p)
            atime = 0 if stat.S_ISDIR(st.st_mode) else st.st_atime_ns
            out[os.path.relpath(p, root)] = (st.st_mode, st.st_size, st.st_mtime_ns, atime)
    return out


SETTINGS = {"max_bytes": 6000, "min_free_bytes": 0, "ttl_s": 3600}


def test_a_dry_run_changes_nothing_and_plans_what_a_reap_does(tmp_path):
    root = str(tmp_path / "store")
    segs, tmp_old, _young, _bystanders = _tree(root)
    before = _snapshot(root)

    plan = cks.reap_plan(root, now=NOW, budget_s=10, **SETTINGS)
    assert _snapshot(root) == before
    store = SharedArrayStore(root, mode="read", **SETTINGS)    # its probe comes and goes
    before_store = _snapshot(root)
    via_store = store.reap(dry_run=True, now=NOW, budget_s=10)
    assert _snapshot(root) == before_store
    assert store.drain_stats() == {}

    assert plan["dry_run"] and plan["complete"] and plan["scanned"] == 10
    assert plan["sweep"] == [tmp_old]
    # Expired: the first two (3700 and 3640 s old against a TTL of 3600).
    # Over: 10 x ~1.1 kB against a low water mark of 4800 bytes leaves four.
    assert plan["unlink"] == segs[:6]
    assert plan["reaped"] == 6 and plan["bytes"] == sum(os.path.getsize(p) for p in segs[:6])
    assert plan["low_water"] == 4800 and plan["need_free"] == 0
    assert {k: via_store[k] for k in ("unlink", "sweep", "reaped", "bytes")} == \
        {k: plan[k] for k in ("unlink", "sweep", "reaped", "bytes")}


def test_a_dry_run_matches_a_real_reap_of_a_copy(tmp_path):
    """The plan is what a reap does: reap a copy (mtimes kept) and compare what went."""
    root = str(tmp_path / "store")
    _tree(root)
    # Real reaps read the clock, so put the copy's mtimes in today's terms.
    shift = time.time() - NOW
    copy = str(tmp_path / "copy")
    shutil.copytree(root, copy, symlinks=True)
    for dirpath, _dirs, files in os.walk(copy):
        for name in files:
            p = os.path.join(dirpath, name)
            st = os.lstat(p)
            os.utime(p, (st.st_atime + shift, st.st_mtime + shift))
    plan = cks.reap_plan(copy, budget_s=10, **SETTINGS)
    before = set(_snapshot(copy))
    out = SharedArrayStore(copy, mode="read", **SETTINGS).reap(budget_s=10)
    gone = before - set(_snapshot(copy))
    want = {os.path.relpath(p, copy) for p in plan["unlink"] + plan["sweep"]}
    assert gone == want                     # .usage.json is replaced, the lock comes and goes
    assert out["reaped"] == plan["reaped"] and out["bytes"] == plan["bytes"]


def test_a_dry_run_takes_no_lock_and_ignores_one(tmp_path):
    """A held lock stops a reap, not a plan; and the plan leaves the lock alone."""
    root = str(tmp_path / "store")
    _tree(root)
    lock = _plain(os.path.join(root, ".reap.lock"), NOW - 10**6, b"1")  # stale, too
    before = _snapshot(root)
    plan = cks.reap_plan(root, now=NOW, budget_s=10, **SETTINGS)
    assert plan["reaped"] == 6
    assert _snapshot(root) == before and os.path.exists(lock)


@pytest.mark.parametrize("root", ["", "/", "/tmp"])
def test_a_dry_run_refuses_what_a_reap_refuses(root):
    plan = cks.reap_plan(root, now=NOW, **SETTINGS)
    assert plan["refused"] and plan["unlink"] == [] and plan["scanned"] == 0


def test_the_clock_is_fixed_only_for_a_dry_run(tmp_path):
    """``now`` is a dry run's: a real reap reads the clock, so a ``now`` far in the future
    can't make it evict live segments."""
    root = str(tmp_path / "store")
    os.makedirs(root)
    seg = _segment(root, "raw", "ab" * 16, "x0", 100, time.time())
    store = SharedArrayStore(root, mode="readwrite", max_bytes=10**9, min_free_bytes=0,
                             ttl_s=60)
    assert store.reap(dry_run=True, now=time.time() + 3600)["unlink"] == [seg]
    assert store.reap(now=time.time() + 3600)["reaped"] == 0
    assert os.path.exists(seg)

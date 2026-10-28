"""The store, addressed through an audio scheme rather than an image one.

Everything here is the property the whole design rests on: **a missing segment
is never an error, only a slower path**. Absent root, full filesystem, reaped
segment, torn file, malformed key -- all return None or False.
"""
import json
import os
import time
from pathlib import Path

import numpy as np
import pytest

import commonkit.store as cks
from commonkit import KeyCollision, KeyScheme, SharedArrayStore, register

D1 = "ab" * 16
D2 = "cd" * 16

# ``dtypes`` maps FORM -> required dtype. A form's name is only a label; the
# store enforces a dtype only for forms listed here. ``raw`` is deliberately
# absent, so it is the form that accepts any dtype.
WAV = register(KeyScheme(domain="ckaudio", version="v1",
                         forms=("wav16k", "f32chw", "raw"), variants=r"^sr\d+$",
                         dtypes={"wav16k": "int16", "f32chw": "float32"}))


def key(digest=D1, form="wav16k"):
    return WAV.key(digest, "sr16000", form)


def _wave(n=200_000):
    return (np.arange(n) % 3001).astype(np.int16)


@pytest.fixture
def store(tmp_path):
    return SharedArrayStore(str(tmp_path / "seg"), mode="readwrite",
                            max_bytes=64 * 2**20, min_free_bytes=1,
                            min_array_bytes=1024, max_array_bytes=8 * 2**20,
                            ttl_s=900, reap_interval_s=0)


def test_publish_then_attach_returns_the_same_samples(store):
    a = _wave()
    assert store.publish(key(), a) is True
    b = store.attach(key())
    assert np.array_equal(np.asarray(b), a)
    assert b.dtype == a.dtype and b.shape == a.shape


def test_attached_arrays_are_read_only_and_stay_that_way(store):
    """mmap_mode='r' makes read-only ENFORCED, not advisory: a caller cannot
    set the flag back, so a shared base cannot be mutated under its readers."""
    store.publish(key(), _wave())
    b = store.attach(key())
    assert b.flags.writeable is False
    with pytest.raises(ValueError):
        b[0] = 9
    with pytest.raises(ValueError):
        b.flags.writeable = True


def test_a_miss_is_none_not_an_exception(store):
    assert store.attach(key(D2)) is None


def test_the_path_stays_under_the_root(store):
    p = store.path_for(key())
    assert os.path.commonpath([p, store.root]) == store.root


def test_an_unregistered_domain_is_a_miss_not_a_crash(store):
    """A key the store cannot address must be counted and shrugged off."""
    assert store.attach(f"nobody:v1:pil:{D1}:o1") is None
    assert store.drain_stats()["shm_bad_key"] == 1


# ------------------------------------------------- dtype policy is the scheme's

def test_a_form_with_a_declared_dtype_refuses_anything_else(store):
    """The rule that used to be a hardcoded ``form != "pil"`` branch inside the
    store. A bad segment is published once and poisons every later reader."""
    assert store.publish(key(form="wav16k"), np.zeros(200_000, dtype=bool)) is False
    assert store.drain_stats()["shm_declined_dtype"] == 1


def test_each_form_enforces_its_own_declared_dtype(store):
    """``wav16k`` -> int16 and ``f32chw`` -> float32: each form is held to its
    OWN entry, so a dtype legal for one form is refused under the other."""
    wave = _wave()
    tensor = np.zeros((3, 256, 256), dtype=np.float32)

    assert store.publish(key(form="wav16k"), wave) is True
    assert store.publish(key(form="f32chw"), tensor) is True
    assert store.publish(key(D2, form="wav16k"), tensor[0]) is False
    assert store.publish(key(D2, form="f32chw"), tensor.astype(np.float64)) is False
    assert store.drain_stats()["shm_declined_dtype"] == 2


@pytest.mark.parametrize("dtype", [np.float32, np.float64, np.uint8, np.int16])
def test_a_form_with_no_declared_dtype_is_unconstrained(store, dtype):
    """``raw`` has no ``dtypes`` entry, so the store takes whatever it is given."""
    a = np.zeros((3, 256, 256), dtype=dtype)
    assert store.publish(key(form="raw"), a) is True
    assert store.attach(key(form="raw")).dtype == dtype


def test_fortran_order_and_dtype_survive(store):
    a = np.asfortranarray(np.zeros((50, 600), dtype=np.float32))
    store.publish(key(form="f32chw"), a)
    b = store.attach(key(form="f32chw"))
    assert b.dtype == np.float32
    assert b.flags.f_contiguous == a.flags.f_contiguous


# ------------------------------------------------------------------ admission

def test_declines_below_the_minimum_size(store):
    assert store.publish(key(), np.zeros(16, dtype=np.int16)) is False
    assert store.drain_stats()["shm_declined_small"] == 1


def test_declines_above_the_per_item_ceiling(store):
    """Not optional: an hour of audio as a waveform would swallow the tree."""
    assert store.publish(key(), np.zeros(9 * 2**20, dtype=np.int16)) is False
    assert store.drain_stats()["shm_declined_size"] == 1


def test_declines_when_the_filesystem_is_near_full_and_never_blocks(
        store, monkeypatch):
    class _FakeVfs:
        f_bavail, f_frsize = 1, 4096
    monkeypatch.setattr(os, "statvfs", lambda p: _FakeVfs())
    store._statvfs_at = 0.0
    store.min_free_bytes = 8 * 2**30
    assert store.publish(key(), _wave()) is False
    assert store.drain_stats()["shm_declined_space"] == 1


def test_read_mode_attaches_but_never_publishes(tmp_path):
    rw = SharedArrayStore(str(tmp_path / "s"), mode="readwrite",
                          min_free_bytes=1, min_array_bytes=1024)
    rw.publish(key(), _wave())
    ro = SharedArrayStore(str(tmp_path / "s"), mode="read", min_free_bytes=1,
                          min_array_bytes=1024)
    assert ro.attach(key()) is not None
    assert ro.publish(key(D2), _wave()) is False


def test_mode_off_is_entirely_inert(tmp_path):
    s = SharedArrayStore(str(tmp_path / "s"), mode="off")
    assert s.disabled
    assert s.attach(key()) is None and s.publish(key(), _wave()) is False


def test_an_unusable_root_degrades_instead_of_raising():
    s = SharedArrayStore("/proc/nonexistent/segments", mode="readwrite")
    assert s.disabled is True
    assert s.attach(key()) is None and s.publish(key(), _wave()) is False


# ------------------------------------------------------------------ integrity

def test_unlink_while_mapped_keeps_a_live_reader_working(store):
    """The reaper's whole licence: it unlinks on policy without knowing who is
    attached, because a live mapping survives the name going away."""
    a = _wave()
    store.publish(key(), a)
    b = store.attach(key())
    os.unlink(store.path_for(key()))
    assert np.array_equal(np.asarray(b), a)
    assert store.attach(key()) is None


def test_a_truncated_segment_is_a_miss_not_a_crash(store):
    store.publish(key(), _wave())
    with open(store.path_for(key()), "r+b") as fh:
        fh.truncate(64)
    assert store.attach(key()) is None
    assert store.drain_stats()["shm_torn"] == 1


def test_concurrent_construction_never_disables_a_store(tmp_path):
    """K workers starting together on one root must ALL come up enabled."""
    import threading

    root, results, errors = tmp_path / "race", [], []

    def build():
        try:
            results.append(SharedArrayStore(
                str(root), mode="readwrite", min_free_bytes=1,
                min_array_bytes=1).disabled)
        except Exception as exc:                              # noqa: BLE001
            errors.append(exc)

    threads = [threading.Thread(target=build) for _ in range(16)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert errors == [] and results.count(True) == 0
    assert [p.name for p in root.iterdir() if p.name.startswith(".writable")] == []


# ------------------------------------------------------------------- reaping

def test_reaper_refuses_a_suspicious_root(tmp_path):
    s = SharedArrayStore(str(tmp_path / "s"), mode="readwrite", min_free_bytes=1)
    s.root = "/"
    assert s.reap()["reaped"] == 0
    assert s.drain_stats()["shm_reap_refused"] == 1


def test_reaper_only_touches_its_own_segments(store):
    store.publish(key(), _wave())
    bystander = os.path.join(store.root, "IMPORTANT.txt")
    with open(bystander, "w") as fh:
        fh.write("not a segment")
    store.ttl_s = -1
    store.reap(budget_s=5)
    assert os.path.exists(bystander)
    assert store.attach(key()) is None


def test_reaper_leaves_fresh_segments_alone(store):
    store.publish(key(), _wave())
    assert store.reap(budget_s=5)["reaped"] == 0
    assert store.attach(key()) is not None


# ------------------------------------------------------------ LRU eviction
# (2026-10-16: none of this was tested, and a store sat full for 5.7 days with
# 1,104 expired segments because nothing ever reaped it again.)

def _keys_n(n):
    return [WAV.key(f"{i:032x}", "sr16000", "wav16k") for i in range(n)]


def _age(store, k, seconds):
    t = time.time() - seconds
    os.utime(store.path_for(k), (t, t))


class _Vfs:
    def __init__(self, free):
        self.f_bavail, self.f_frsize = free // 4096, 4096


def test_over_budget_evicts_least_recently_used_first(store):
    ks = _keys_n(6)
    for k in ks:
        store.publish(k, _wave())                       # 400 kB each
    for i, k in enumerate(ks):
        _age(store, k, 600 - i * 60)                    # ks[0] oldest, all within TTL
    assert store.attach(ks[0]) is not None              # a read refreshes it (LRU)
    store.max_bytes = 3 * _wave().nbytes                # low water: 2 files (plus headers)
    store.reap(budget_s=5)
    present = [store.attach(k) is not None for k in ks]
    assert present == [True, False, False, False, False, True]


def test_expired_segments_are_reaped(store):
    ks = _keys_n(3)
    for k in ks:
        store.publish(k, _wave())
    _age(store, ks[1], 2 * store.ttl_s)
    store.reap(budget_s=5)
    assert [store.attach(k) is not None for k in ks] == [True, False, True]


def test_a_declined_publish_still_reaps(store, monkeypatch):
    """The reaper used to run only after a successful publish: a full store
    declined forever and was never reaped again."""
    ks = _keys_n(3)
    for k in ks:
        store.publish(k, _wave())
        _age(store, k, 2 * store.ttl_s)
    monkeypatch.setattr(os, "statvfs", lambda p: _Vfs(4096))
    store.min_free_bytes = 2**30
    store._statvfs_at = 0.0
    store._last_reap = 0.0
    assert store.publish(WAV.key("ef" * 16, "sr16000", "wav16k"), _wave()) is False
    stats = store.drain_stats()
    assert stats["shm_declined_space"] == 1 and stats["shm_reaped"] == 3


def test_the_free_space_floor_evicts_least_recently_used(store, monkeypatch):
    """A volume smaller than max_bytes + min_free_bytes fills before the budget does;
    the floor used to decline publishes without evicting anything."""
    ks = _keys_n(4)
    for i, k in enumerate(ks):
        store.publish(k, _wave())
        _age(store, k, 600 - i * 60)                    # ks[0] oldest, all within TTL
    seg = os.path.getsize(store.path_for(ks[0]))
    store.max_bytes = 2**40                             # far under budget
    store.min_free_bytes = seg                          # floor + 25%: 1.25 segments
    monkeypatch.setattr(os, "statvfs", lambda p: _Vfs(0))
    out = store.reap(budget_s=5)
    assert out["reaped"] == 2                           # the two least recently used
    assert [store.attach(k) is not None for k in ks] == [False, False, True, True]


def test_eviction_reaches_its_target_whatever_the_time_budget(store, monkeypatch):
    """The time budget bounds the walk, not the eviction: a budget that cut eviction
    short once left a store 1.2 GB over its low water mark."""
    ks = _keys_n(6)
    for i, k in enumerate(ks):
        store.publish(k, _wave())
        _age(store, k, 600 - i * 60)
    store.max_bytes = 3 * _wave().nbytes
    import commonkit.store as cks
    real_unlink = os.unlink
    late = {"on": False}

    def slow_unlink(path):            # the first unlink takes "longer than the budget"
        late["on"] = True
        real_unlink(path)
    real_time = cks.time.time
    monkeypatch.setattr(cks.time, "time", lambda: real_time() + (1000.0 if late["on"] else 0))
    monkeypatch.setattr(os, "unlink", slow_unlink)
    out = store.reap(budget_s=1)
    monkeypatch.setattr(os, "unlink", real_unlink)
    monkeypatch.setattr(cks.time, "time", real_time)
    assert out["complete"] is True and out["reaped"] >= 2
    used = sum(os.path.getsize(store.path_for(k)) for k in ks
               if os.path.exists(store.path_for(k)))
    assert used <= int(0.8 * store.max_bytes)


def test_a_partial_walk_neither_evicts_for_budget_nor_writes_usage(store, monkeypatch):
    """A walk cut short under-counts the tree: it must not record that total, nor
    evict for a budget it can't judge."""
    for k in _keys_n(3):
        store.publish(k, _wave())
    marker = os.path.join(store.root, ".usage.json")
    if os.path.exists(marker):
        os.unlink(marker)
    store.max_bytes = 1                                 # "over budget" if it were judged
    import commonkit.store as cks
    real = cks.time.time
    calls = {"n": 0}

    def clock():                       # past the deadline from the first directory on
        calls["n"] += 1
        return real() + (1000.0 if calls["n"] > 2 else 0.0)
    monkeypatch.setattr(cks.time, "time", clock)
    out = store.reap(budget_s=1)
    monkeypatch.setattr(cks.time, "time", real)
    assert out["complete"] is False
    assert not os.path.exists(marker)
    assert out["reaped"] == 0


# ------------------------------------------------- the on-disk contract (step 3)



def _raw_npy(header: str, payload: bytes) -> bytes:
    """A version 1 file with ``header`` as its text, padded to 64 bytes as numpy pads."""
    text = header.encode("latin1")
    pad = 64 - ((10 + len(text) + 1) % 64)
    hlen = len(text) + pad + 1
    return b"\x93NUMPY\x01\x00" + hlen.to_bytes(2, "little") + text + b" " * pad + b"\n" \
        + payload


@pytest.mark.parametrize("attach", ["mmap", "read"])
def test_a_garbled_header_is_a_miss_not_an_exception(tmp_path, attach):
    """numpy raises tokenize.TokenError (neither ValueError nor OSError) for this header,
    and attach let it escape to the caller until 2026-10-28 (contract §8.1, A5)."""
    s = SharedArrayStore(str(tmp_path / "seg"), mode="read", attach=attach)
    path = s.path_for(key())
    os.makedirs(os.path.dirname(path))
    with open(path, "wb") as fh:
        fh.write(_raw_npy("{'descr': '<i4', 'fortran_order': Fals", bytes(40)))
    assert s.attach(key()) is None
    assert dict(s.drain_stats()) == {"shm_torn": 1}


def test_an_object_array_is_declined_and_leaves_no_temporary_file(store):
    """np.save(allow_pickle=False) raised ValueError out of publish, past a handler that
    caught only OSError, and left the .tmp behind (contract §6.3, A6)."""
    objs = np.empty(4096, dtype=object)
    assert store.publish(key(form="raw"), objs) is False
    assert dict(store.drain_stats()) == {"shm_declined_dtype": 1}
    assert [n for _d, _s, names in os.walk(store.root) for n in names
            if n.endswith(".tmp")] == []


def test_a_header_over_the_limit_is_declined(store):
    """No reader accepts a header over 1 MiB, so none is written (contract §5.5)."""
    wide = np.dtype([(f"f{i:06d}", "<i4") for i in range(70_000)])
    assert cks._header_len(np.zeros(1, wide)) > cks.MAX_HEADER_BYTES
    assert store.publish(key(form="raw"), np.zeros(4, wide)) is False
    assert dict(store.drain_stats()) == {"shm_declined_dtype": 1}


def test_republishing_the_same_array_touches_and_writes_nothing(store):
    """A republish used to rewrite the segment and count a second publish (C15)."""
    assert store.publish(key(), _wave())
    path = store.path_for(key())
    _age(store, key(), 3600)
    inode = os.stat(path).st_ino
    store.drain_stats()
    assert store.publish(key(), _wave()) is True
    st = os.stat(path)
    assert st.st_ino == inode, "renamed over: the segment was rewritten"
    assert time.time() - st.st_mtime < 60, "a republish is a touch"
    assert dict(store.drain_stats()) == {}


def test_a_different_array_under_an_existing_key_raises(store, caplog):
    """The second loud path (contract §6.4): the file and its mtime stay as they were,
    the collision is counted and logged at error level."""
    assert store.publish(key(), _wave())
    path = store.path_for(key())
    _age(store, key(), 3600)
    before = (Path(path).read_bytes(), os.stat(path).st_mtime)
    store.drain_stats()
    with caplog.at_level("ERROR", logger="commonkit.store"), \
            pytest.raises(KeyCollision) as info:
        store.publish(key(), _wave(150_000))
    assert isinstance(info.value, ValueError)
    assert (Path(path).read_bytes(), os.stat(path).st_mtime) == before
    assert dict(store.drain_stats()) == {"shm_key_collision": 1}
    assert [r.levelname for r in caplog.records] == ["ERROR"]


def test_a_torn_segment_is_replaced_and_counted(store):
    store.publish(key(), _wave())
    with open(store.path_for(key()), "r+b") as fh:      # test-only: unmapped (contract §7)
        fh.truncate(64)
    store.drain_stats()
    assert store.publish(key(), _wave()) is True
    assert store.drain_stats()["shm_torn_replaced"] == 1
    assert np.array_equal(store.attach(key()), _wave())


def test_fsync_is_off_by_default(store, monkeypatch):
    calls = []
    monkeypatch.setattr(cks, "_sync", calls.append)
    assert store.fsync is False and store.publish(key(), _wave())
    assert calls == []


def test_fsync_syncs_the_file_before_the_rename_and_the_directory_after(store, monkeypatch):
    """Contract §6.5 steps 3 and 5 (C17), in that order."""
    seen = []
    path = store.path_for(key())

    def sync(fd):
        seen.append((os.path.isdir(f"/dev/fd/{fd}") if os.path.exists("/dev/fd")
                     else None, os.path.exists(path)))
    monkeypatch.setattr(cks, "_sync", sync)
    store.fsync = True
    assert store.publish(key(), _wave())
    if os.name == "posix":
        assert [exists for _isdir, exists in seen] == [False, True]
        assert seen[1][0] in (True, None)               # the second sync is the directory
    else:                                               # pragma: no cover - Windows
        assert [exists for _isdir, exists in seen] == [False]


def test_a_failed_directory_sync_fails_the_publish_but_keeps_the_segment(store, monkeypatch):
    def broken(_path):
        raise OSError(5, "I/O error")
    monkeypatch.setattr(cks, "_sync_dir", broken)
    store.fsync = True
    assert store.publish(key(), _wave()) is False
    assert dict(store.drain_stats()) == {"shm_publish_failed": 1}
    assert np.array_equal(store.attach(key()), _wave()), "the whole segment stays"


def test_free_space_is_zero_without_statvfs(store, monkeypatch):
    """A platform with no os.statvfs raised AttributeError out of publish (§9.9, A10)."""
    monkeypatch.delattr(os, "statvfs")
    store._statvfs_at = 0.0
    assert store.publish(key(), _wave()) is False
    assert {k: v for k, v in store.drain_stats().items() if v} == {"shm_declined_space": 1}


@pytest.mark.parametrize("used", ["995", 995.0, -1, True, None])
def test_usage_bytes_must_be_a_non_negative_json_integer(store, used):
    """``int()`` took a float or a numeric string, which no other reader trusts (A11)."""
    with open(os.path.join(store.root, ".usage.json"), "w") as fh:
        json.dump({"bytes": used, "files": 3}, fh)
    assert store.usage() == (-1, -1)


def test_usage_survives_a_hostile_file(store):
    with open(os.path.join(store.root, ".usage.json"), "w") as fh:
        fh.write("[" * 100_000)                         # RecursionError, not ValueError
    assert store.usage() == (-1, -1)

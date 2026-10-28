"""The L1 memo in front of the store.

The measurement this class exists for: a caller that looks the same digest up
repeatedly (the original consumer's face path does it twice per face, so 36
times on an 18-face page) would pay one ~3 ms attach per call. At 36 lookups that is three
times SLOWER than no cache at all. L1 memoises the attached MAPPING -- it adds
no bytes, it removes syscalls.
"""
import numpy as np
import pytest

from commonkit import KeyScheme, SharedArrayStore, TieredPixelCache, register

D1 = "ab" * 16

SCHEME = register(KeyScheme("cktier", "v1", ("wav16k",), r"^sr\d+$"))
KEY = SCHEME.key(D1, "sr16000", "wav16k")


class _Memo:
    """The least an L1 can be: get/set_array over a dict."""

    def __init__(self):
        self.d = {}

    def get(self, key):
        return self.d.get(key)

    def set_array(self, key, arr):
        self.d[key] = arr


class _CountingStore(SharedArrayStore):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.attaches = 0

    def attach(self, key):
        self.attaches += 1
        return super().attach(key)


def _wave():
    return (np.arange(200_000) % 3001).astype(np.int16)


@pytest.fixture
def tiers(tmp_path):
    store = SharedArrayStore(str(tmp_path / "seg"), mode="readwrite",
                             min_free_bytes=1, min_array_bytes=1024,
                             max_bytes=64 * 2**20)
    return TieredPixelCache(l1=_Memo(), l2=store)


def test_set_array_writes_both_tiers(tiers):
    tiers.set_array(KEY, _wave())
    assert tiers.l1.get(KEY) is not None
    assert tiers.l2.attach(KEY) is not None


def test_a_repeated_lookup_attaches_exactly_once(tmp_path):
    """The whole reason L1 exists, asserted rather than trusted."""
    store = _CountingStore(str(tmp_path / "seg"), mode="readwrite",
                           min_free_bytes=1, min_array_bytes=1024)
    store.publish(KEY, _wave())
    tiers = TieredPixelCache(l1=_Memo(), l2=store)
    store.attaches = 0

    for _ in range(36):
        assert tiers.get(KEY) is not None

    assert store.attaches == 1


def test_the_cache_says_which_tier_answered(tiers):
    """A return channel, not an inference from counter deltas -- which is what
    this used to be, and it reported every cross-process attach as a decode."""
    assert tiers.get_with_source(KEY) == (None, "decode")

    tiers.set_array(KEY, _wave())
    arr, source = tiers.get_with_source(KEY)
    assert source == "l1" and arr is not None

    tiers.l1 = _Memo()                       # as a second process sees it
    arr, source = tiers.get_with_source(KEY)
    assert source == "shm" and arr is not None


def test_either_tier_may_be_absent(tmp_path):
    """With both None this is a no-op cache, leaving its caller exactly as it
    would be with no cache at all."""
    assert TieredPixelCache().get(KEY) is None
    l1_only = TieredPixelCache(l1=_Memo())
    l1_only.set_array(KEY, _wave())
    assert l1_only.get(KEY) is not None


def test_drain_takes_the_stores_counters_too(tiers):
    tiers.set_array(KEY, _wave())
    tiers.get(KEY)
    drained = tiers.drain_stats()
    assert drained["shm_publish"] == 1
    assert tiers.drain_stats()["shm_publish"] == 0        # reset, not copied

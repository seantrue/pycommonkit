"""Two-tier pixel cache: an in-process memo over the shared store.

Why two tiers and not a straight replacement, in one measurement:

The consumer's face-crop path calls its pixel accessor **twice per face**
-- once for the base dimensions, once for the crop. An 18-face page is
therefore 36 lookups. Today that is one 38 ms decode plus 35 free in-process
hits. Against a shared store alone it would be 36 attaches at ~3.3 ms each =
119 ms, **three times slower than doing nothing at all**. Attaching is cheap, but
it is not free, and a loop that attaches per crop is the one access pattern that
turns a cache into a pessimization.

So L1 memoises the ATTACHED MAPPING, not a second copy of the pixels. The first
lookup attaches; every later lookup in that process is a dict hit. The pixels
still live once in shared memory and are still shared across processes -- L1 adds
no bytes, it removes syscalls.

Tier order: L1 in-process memo -> L2 shared store -> decode.
"""
from __future__ import annotations

from collections import Counter

import numpy as np


class TieredPixelCache:
    """Satisfies the consumer's ``PixelCache`` protocol.

    ``l1`` is anything with ``get``/``set_array`` (the original consumer
    passes an LRU array cache); ``l2`` is a
    :class:`~commonkit.store.SharedArrayStore`. Either may be None,
    which degrades to the other -- and with both None this is a no-op cache,
    leaving its caller exactly as it would be with no cache at all.
    """

    def __init__(self, l1=None, l2=None):
        self.l1 = l1
        self.l2 = l2
        self.stats: Counter = Counter()

    # ``_base_array`` checks for this and prefers it over the bytes path.
    def get(self, key: str):
        """The PixelCache protocol: the array, or None."""
        return self.get_with_source(key)[0]

    def get_with_source(self, key: str):
        """``(array, source)`` -- which TIER answered, or ``(None, "decode")``.

        The return channel this class used to lack, and the lack of which was a
        real defect rather than an inelegance. ``entry.decoded`` could not ask
        what happened, so it inferred it from counter DELTAS -- reading
        ``shm_hit`` off this object, which deliberately does not tick it because
        the store already does. The inference silently defaulted to "decode",
        so every cross-process attach was reported as a decode.

        Answering directly deletes the inference instead of correcting it. It is
        also the only thread-safe shape: this cache is a module-global shared
        across a node's CONCURRENCY threads, so recording "what I did last" on
        ``self`` would be a race between them.
        """
        if self.l1 is not None:
            arr = self.l1.get(key)
            if arr is not None:
                self.stats["l1_hit"] += 1
                return arr, "l1"
        if self.l2 is not None:
            arr = self.l2.attach(key)
            if arr is not None:
                # The store counts the hit (and the publish, below) itself.
                # Counting it here too doubled both in drain_stats, which
                # sums the tier's counters with the store's.
                # Promote so a crop loop pays one attach, not one per crop.
                if self.l1 is not None:
                    try:
                        self.l1.set_array(key, arr)
                    except Exception:  # noqa: BLE001, S110 -- an L1 that can't take the array costs a later attach, never an error
                        pass
                return arr, "shm"
        self.stats["miss"] += 1
        return None, "decode"

    def set_array(self, key: str, arr) -> None:
        """Called by ``_base_array`` after a decode. Never raises."""
        if self.l1 is not None:
            try:
                self.l1.set_array(key, arr)
            except Exception:  # noqa: BLE001, S110 -- an L1 that can't take the array costs a later decode, never an error
                pass
        if self.l2 is not None:
            try:
                if not self.l2.publish(key, arr):
                    self.stats["shm_declined"] += 1
            except Exception:                                  # noqa: BLE001
                self.stats["shm_publish_failed"] += 1

    def set(self, key: str, value: bytes) -> None:
        """Bytes path, for protocol compliance. ``_base_array`` prefers
        ``set_array`` and will not normally reach this."""
        try:
            import io
            self.set_array(key, np.load(io.BytesIO(value), allow_pickle=False))
        except Exception:  # noqa: BLE001, S110 -- undecodable bytes are a miss: the cache never raises
            pass

    def drain_stats(self) -> Counter:
        """Take and reset counters, including the store's own."""
        out = Counter(self.stats)
        self.stats.clear()
        if self.l2 is not None:
            try:
                out.update(self.l2.drain_stats())
            except Exception:  # noqa: BLE001, S110 -- a store whose stats fail still lets the tier's own counters out
                pass
        return out

    def __repr__(self) -> str:
        return f"<TieredPixelCache l1={self.l1!r} l2={self.l2!r}>"

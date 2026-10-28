"""Counting what a lookup did, in ONE vocabulary.

A store answers three ways and the names for them are worth owning centrally,
because the alternative is what happened before this module existed: two paths
to the same pixels each decided independently what to call a hit, wrote the
SAME counter names for DIFFERENT events, and nothing downstream could tell
which one it was reading. One of them classified by counter DELTA and was
wrong for two days; a correct production measurement was retracted on the
strength of the ambiguity.

The counter PREFIX is the caller's, not ours: ``pixels_shm`` and ``audio_shm``
are different measurements and must not collide in one stats table.
"""
from __future__ import annotations

from typing import Any

#: Every tier a lookup can be answered by. ``decode`` means "nothing had it" --
#: the caller recomputes, which is always allowed and never an error.
SOURCES = ("l1", "shm", "decode")


def note(stats: Any, source: str, *, prefix: str) -> str:
    """Tick ``{prefix}{source}``. THE one place that counter is written.

    Returns ``source`` so callers can use this inline. A bad source name is a
    programming error and raises: these counters are the instrument the whole
    exercise is judged by, and a typo would silently invent a tier.
    """
    if source not in SOURCES:
        raise ValueError(f"unknown source {source!r}; have {SOURCES}")
    if stats is not None:
        stats[f"{prefix}{source}"] += 1
    return source


def attach(store: Any, key: str) -> Any | None:
    """The array for ``key``, or None. A miss is never an error.

    An absent store, an unusable key, an absent segment and a torn file all
    return None, because every caller's answer to all four is the same:
    recompute it.
    """
    if store is None or not key:
        return None
    try:
        return store.attach(key)
    except Exception:                                          # noqa: BLE001
        return None

"""The ONE store per host that every commonkit user shares, and its tree-wide policy.

Why one store rather than one per application, measured on an M4 Max (macOS, 16 KB pages)
by the first consumer:

* Every budget is enforced per TREE, by whichever process happens to reap. Two
  applications with two roots therefore hold the SUM of their budgets, and neither
  sees the other's usage. One root means one budget and one age-out for everything.
* On macOS, Time Machine's hourly local snapshots keep a reaped segment's blocks
  allocated (a 539 MB segment reaped after a snapshot freed 0 MB, TM exclusion of
  ~/Library/Caches notwithstanding). A tree that turns over within the hour can pin up
  to ~24x its budget of purgeable space until the snapshots thin. That is why the macOS
  budget is 2 GB, and why a second tree with its own budget multiplies the pinned space.

Tree-wide policy -- root, byte budget, TTL, free-space floor, reap interval, largest
array -- comes ONLY from the environment (``COMMONKIT_STORE_*``) or the defaults here,
never from a caller, because the reaper applies whatever the reaping process was given to
every segment in the tree: a caller with a longer TTL or a bigger budget would silently
override everyone else's. A caller chooses only what concerns itself: ``mode``, how it
reads (``attach``: ``mmap`` shares one physical copy, ``read`` gives it a private one), and
the smallest array worth storing. Tests may pass a ``root`` to stay out of the shared tree.

Segments of different domains coexist because forms partition the tree
(``{form}/{digest[:2]}/...``). Form names are therefore global: pick ones no other
domain uses.
"""
from __future__ import annotations

import os
import sys

from .store import SharedArrayStore

ENV_PREFIX = "COMMONKIT_STORE_"
_DARWIN = sys.platform == "darwin"

#: Per-platform defaults. macOS: the user cache directory on the internal SSD -- it has no
#: /dev/shm, the SSD beat a RAM disk on first attach (0.69 ms against 0.76 ms), and a
#: ``ram://`` disk commits its whole size up front as dirty memory. Linux: /dev/shm, which
#: is RAM, so the budget is its only real bound.
DEFAULTS = {
    "ROOT": (os.path.expanduser("~/Library/Caches/commonkit/store") if _DARWIN
             else "/dev/shm/commonkit"),
    "MAX_GB": 2.0 if _DARWIN else 8.0,
    "MIN_FREE_GB": 8.0,
    "TTL_S": 900.0,
    "REAP_S": 30.0,
    "MAX_ARRAY_BYTES": 128 * 2**20,
}


def setting(name: str):
    """The effective tree-wide value of ``name`` (a key of DEFAULTS).

    A malformed environment value falls back to the default rather than raising: like the
    store itself, configuration must never be the reason a caller fails.
    """
    default = DEFAULTS[name]
    raw = os.environ.get(ENV_PREFIX + name)
    if raw is None or raw == "":
        return default
    if isinstance(default, str):
        return os.path.expanduser(raw)
    try:
        return type(default)(float(raw))
    except ValueError:
        return default


def shared_store(*, mode: str = "readwrite", attach: str = "mmap",
                 min_array_bytes: int = 256 * 1024, root=None) -> SharedArrayStore:
    """The host's shared store, with the tree-wide policy from the environment.

    Never raises (the store's own contract): an unusable root comes up disabled.
    """
    return SharedArrayStore(
        root or setting("ROOT"),
        mode=mode,
        attach=attach,
        max_bytes=int(setting("MAX_GB") * 2**30),
        min_free_bytes=int(setting("MIN_FREE_GB") * 2**30),
        ttl_s=setting("TTL_S"),
        min_array_bytes=min_array_bytes,
        max_array_bytes=setting("MAX_ARRAY_BYTES"),
        reap_interval_s=setting("REAP_S"),
    )

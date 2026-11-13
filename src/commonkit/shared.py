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
array, fsync, republish verification -- comes ONLY from the tree's ``.policy.json``, the
environment (``COMMONKIT_STORE_*``) or the defaults here, never from a caller, because the
reaper applies whatever the reaping process was given to every segment in the tree: a
caller with a longer TTL or a bigger budget would silently override everyone else's. A
caller chooses only what concerns itself: ``mode``, how it reads (``attach``: ``mmap``
shares one physical copy, ``read`` gives it a private one), and the smallest array worth
storing. Tests may pass a ``root`` to stay out of the shared tree.

The policy FILE wins over the environment (C7), because the environment is per process
and the tree is not: on thresher the policy lived in ``~/.bashrc``, so an app launched
from the Finder ran with the defaults and reaped a 7 GB tree down to 2 GB. A file at the
root is seen by every process that finds the root. The environment fills only what the
file leaves out (contract §10.6).

Segments of different domains coexist because forms partition the tree
(``{form}/{digest[:2]}/...``). Form names are therefore global: pick ones no other
domain uses.
"""
from __future__ import annotations

import os
import sys

from .policy import (  # noqa: F401  (re-exported)
    _BYTE_SCALE,
    _FLAG_ON,
    _NUMBER_RE,
    _POLICY_MAX_BYTES,
    DEFAULTS,
    ENV_PREFIX,
    POLICY_FILE,
    _from_env,
    _from_policy,
    _in_range,
    parse_flag,
    parse_number,
    read_policy,
    setting,
)
from .store import SharedArrayStore

__all__ = ["DEFAULTS", "ENV_PREFIX", "POLICY_FILE", "parse_flag", "parse_number",
           "read_policy", "setting", "shared_store"]
_ = (os, sys)                       # kept importable as shared.os and shared.sys


def shared_store(*, mode: str = "readwrite", attach: str = "mmap",
                 min_array_bytes: int = 256 * 1024, root=None) -> SharedArrayStore:
    """The host's shared store, with the tree-wide policy from ``.policy.json`` and the
    environment. The file is read once, here.

    Never raises (the store's own contract): an unusable root comes up disabled.
    """
    root = root or setting("ROOT")
    policy = read_policy(root)

    def get(name):
        return setting(name, policy)

    return SharedArrayStore(
        root,
        mode=mode,
        attach=attach,
        max_bytes=int(get("MAX_GB") * 2**30),
        min_free_bytes=int(get("MIN_FREE_GB") * 2**30),
        ttl_s=get("TTL_S"),
        min_array_bytes=min_array_bytes,
        max_array_bytes=get("MAX_ARRAY_BYTES"),
        reap_interval_s=get("REAP_S"),
        fsync=get("FSYNC"),
        verify_republish=get("VERIFY_REPUBLISH"),
    )

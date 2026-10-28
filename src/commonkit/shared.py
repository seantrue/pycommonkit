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

import json
import math
import os
import re
import stat
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
    "FSYNC": False,
    "VERIFY_REPUBLISH": False,
}

#: The tree's policy file, directly under the root (contract §10.5), and the most of it
#: that is read: a real one is a few hundred bytes.
POLICY_FILE = ".policy.json"
_POLICY_MAX_BYTES = 65536

#: The settings that become byte counts, and their scale. The derived integer must fit
#: in 64 signed bits (contract §10.2), or a value is malformed: ``int(float("inf"))``
#: raised OverflowError out of ``shared_store`` until 2026-10-28.
_BYTE_SCALE = {"MAX_GB": 2**30, "MIN_FREE_GB": 2**30, "MAX_ARRAY_BYTES": 1}

#: The number grammar (contract §10.2). ``float()`` is far looser: ``inf``, ``nan``,
#: ``1_000``, Arabic-Indic digits and Unicode whitespace all parse, and a Rust or C
#: reader of the same environment would disagree with every one of them.
_NUMBER_RE = re.compile(r"[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?")
_ASCII_SPACE = " \t\n\v\f\r"
_FLAG_ON = frozenset({"1", "true", "yes", "on"})


def parse_number(raw: str) -> float | None:
    """A finite float from ``raw`` under the contract's grammar, or None if malformed."""
    text = raw.strip(_ASCII_SPACE)
    if not _NUMBER_RE.fullmatch(text):
        return None
    value = float(text)                 # the grammar is a subset of float()'s
    return value if math.isfinite(value) else None


def parse_flag(raw: str) -> bool:
    """Whether ``raw`` turns a flag on (contract §10.3): exact, ASCII case-insensitive.

    ``isascii`` first, because ``str.lower`` folds beyond ASCII. Nothing is trimmed:
    ``" 1"`` is off, so a value means one thing whichever language reads it.
    """
    return raw.isascii() and raw.lower() in _FLAG_ON


class _Object(dict):
    """A JSON object that remembers whether its text repeated a key."""

    duplicate = False


def _object_pairs(pairs):
    obj = _Object(pairs)
    obj.duplicate = len(obj) != len(pairs)
    return obj


def _not_json(name):
    raise ValueError(f"{name} is not JSON")


def read_policy(root) -> dict:
    """The tree's ``.policy.json`` as a dict, or ``{}`` when it is absent or invalid.

    Invalid is ENTIRELY absent (contract §10.5): not a regular file, over 64 KiB,
    not UTF-8, not JSON (``NaN`` and ``Infinity`` included), not an object, or a
    repeated top-level key, which JSON parsers resolve differently. Never raises.
    """
    path = os.path.join(os.path.abspath(str(root)), POLICY_FILE)
    try:
        # stat first: opening a FIFO someone named .policy.json would block forever.
        st = os.stat(path)
        if not stat.S_ISREG(st.st_mode) or st.st_size > _POLICY_MAX_BYTES:
            return {}
        with open(path, "rb") as fh:
            data = fh.read(_POLICY_MAX_BYTES + 1)
        if len(data) > _POLICY_MAX_BYTES:
            return {}
        # Decoded first: json.loads on bytes would also accept UTF-16 and UTF-32.
        doc = json.loads(data.decode("utf-8"), object_pairs_hook=_object_pairs,
                         parse_constant=_not_json)
    except Exception:                                          # noqa: BLE001
        return {}
    if not isinstance(doc, _Object) or doc.duplicate:
        return {}
    return dict(doc)


def _from_policy(name: str, value):
    """``value`` from the policy file as a setting of ``name``'s kind, or None (left out)."""
    if isinstance(DEFAULTS[name], bool):
        if isinstance(value, bool):
            return value
        return parse_flag(value) if isinstance(value, str) else None
    if isinstance(value, bool):         # bool is an int to Python, never a number here
        return None
    if isinstance(value, (int, float)):
        try:
            value = float(value)        # an integer past the double range overflows
        except OverflowError:
            return None
        return value if math.isfinite(value) else None
    return parse_number(value) if isinstance(value, str) else None


def _from_env(name: str):
    raw = os.environ.get(ENV_PREFIX + name)
    if raw is None or raw == "":
        return None
    if isinstance(DEFAULTS[name], bool):
        return parse_flag(raw)
    return parse_number(raw)


def _in_range(name: str, value) -> bool:
    scale = _BYTE_SCALE.get(name)
    if scale is None:
        return True
    try:
        n = int(value * scale)          # trunc; inf (1e300 GiB) raises OverflowError
    except (OverflowError, ValueError):
        return False
    return -2**63 <= n <= 2**63 - 1


def setting(name: str, policy: dict | None = None):
    """The effective tree-wide value of ``name`` (a key of DEFAULTS).

    The first valid one of: ``policy`` (a :func:`read_policy` result), the environment,
    the default (contract §10.6). ``ROOT`` never comes from the policy file, which is
    found BY the root. A malformed value is skipped rather than raised: like the store
    itself, configuration must never be the reason a caller fails.
    """
    default = DEFAULTS[name]
    if name == "ROOT":
        raw = os.environ.get(ENV_PREFIX + name)
        return os.path.expanduser(raw) if raw else default
    candidates = []
    if policy and name in policy:
        candidates.append(_from_policy(name, policy[name]))
    candidates.append(_from_env(name))
    for value in candidates:
        if value is not None and _in_range(name, value):
            return type(default)(value)
    return default


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

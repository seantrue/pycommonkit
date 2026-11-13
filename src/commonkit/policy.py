"""Tree-wide policy (contract §10): the settings, their grammar, the ``.policy.json`` reader,
and the hash setting (§3.8).

Standard library only, and it imports nothing from commonkit, so the store and the hashing
facade can both depend on it without either reaching the other (the alexandria constraint:
nothing the store imports may hash). ``shared`` re-exports the names that used to live
there, so its public API is unchanged.

**The hash setting.** ``HASH`` in the tree's ``.policy.json``, then ``COMMONKIT_HASH``, then
the compiled default ``b3``; the file wins, as for every other setting (C7). The variable
breaks the ``COMMONKIT_STORE_<KEY>`` pattern by the maintainer's choice (D1, round 7), so
don't "fix" its name. It is resolved ONCE per process, by :func:`hash_algo`, and the store's
digest rule and ``commonkit.digest`` share that one answer; the first resolution prints
``commonkit: hash <algo> (<source>)`` on stderr, so every process says what it hashes.
``HASH`` is deliberately not in :data:`DEFAULTS`: it isn't a store field, and alexandria
compares ``DEFAULTS`` by name.
"""
from __future__ import annotations

import json
import math
import os
import re
import stat
import sys
import threading

_DARWIN = sys.platform == "darwin"
_WINDOWS = sys.platform == "win32"

ENV_PREFIX = "COMMONKIT_STORE_"

#: Per-platform defaults. macOS: the user cache directory on the internal SSD -- it has no
#: /dev/shm, the SSD beat a RAM disk on first attach (0.69 ms against 0.76 ms), and a
#: ``ram://`` disk commits its whole size up front as dirty memory. Linux: /dev/shm, which
#: is RAM, so the budget is its only real bound. Windows: no /dev/shm either (it would
#: resolve to ``\dev\shm`` on the current drive), so the per-user local cache, on disk like
#: macOS and with its budget (contract 10.1; from seantrue/commonkit d8198ae).
DEFAULTS = {
    "ROOT": (os.path.expanduser("~/Library/Caches/commonkit/store") if _DARWIN
             else os.path.join(os.environ.get("LOCALAPPDATA")
                               or os.path.expanduser("~/AppData/Local"), "commonkit", "store")
             if _WINDOWS
             else "/dev/shm/commonkit"),
    "MAX_GB": 2.0 if _DARWIN or _WINDOWS else 8.0,
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


def default_root() -> str:
    """The platform's default root (contract §10.1)."""
    return DEFAULTS["ROOT"]


# ------------------------------------------------------------------- the hash setting

#: The algorithms and their digest lengths in hex (contract §3.7, §3.8).
HEX_LEN = {"md5": 32, "b3": 64}
DEFAULT_HASH = "b3"
HASH_ENV = "COMMONKIT_HASH"


def parse_hash(raw) -> str | None:
    """``md5`` or ``b3`` from a value, trimmed and ASCII case-folded, or None."""
    if not isinstance(raw, str) or not raw.isascii():
        return None
    value = raw.strip(_ASCII_SPACE).lower()
    return value if value in HEX_LEN else None


def resolve_hash(policy: dict, env, policy_path: str | None = None):
    """``(algo, source, warnings)`` from a :func:`read_policy` result and an environment.

    Pure: no caching, no output. ``source`` is ``policy file <path>``, ``environment
    COMMONKIT_HASH`` or ``default``. A present but invalid value is skipped with a warning,
    never raised (the next source applies).
    """
    warnings = []
    where = f"policy file {policy_path or POLICY_FILE}"
    if policy and "HASH" in policy:
        algo = parse_hash(policy["HASH"])
        if algo:
            return algo, where, warnings
        warnings.append(f"commonkit: ignoring HASH {policy['HASH']!r} in {where}: "
                        "not md5 or b3")
    raw = env.get(HASH_ENV)
    if raw:
        algo = parse_hash(raw)
        if algo:
            return algo, f"environment {HASH_ENV}", warnings
        warnings.append(f"commonkit: ignoring {HASH_ENV}={raw!r}: not md5 or b3")
    return DEFAULT_HASH, "default", warnings


def _resolve_hash_at(root: str | None, env):
    """:func:`resolve_hash` for the tree at ``root`` (its ``.policy.json``, read now). With
    no root there is no tree, so no file is read: never the working directory's
    ``.policy.json``. Rust's ``resolve_hash_at`` agrees."""
    if root is None:
        return resolve_hash({}, env)
    return resolve_hash(read_policy(root), env, os.path.join(root, POLICY_FILE))


def _tree_root() -> str | None:
    """The absolute root the hash setting reads its file from, or None when there is none:
    a ``~`` that nothing could expand (no home), where Rust's ``resolve_root`` has none."""
    root = setting("ROOT")
    if root == "~" or root.startswith(("~" + os.sep, "~/")):
        return None
    return os.path.abspath(root)


#: ``(algo, source)`` once resolved; None until the first call. One per process.
_HASH = None
_HASH_LOCK = threading.Lock()


def _say(line: str) -> None:
    # The first resolution can happen inside a store call, which never raises (§3.8): a
    # closed stderr (ValueError) or a parent gone away (OSError) loses the line, nothing else.
    try:
        print(line, file=sys.stderr)
    except (OSError, ValueError):
        pass


def _resolve_once():
    global _HASH
    with _HASH_LOCK:
        if _HASH is None:
            algo, source, warnings = _resolve_hash_at(_tree_root(), os.environ)
            for w in warnings:
                _say(w)
            _say(f"commonkit: hash {algo} ({source})")
            _HASH = (algo, source)
    return _HASH


def hash_algo() -> str:
    """This process's algorithm, ``md5`` or ``b3``: resolved at the first call, then cached."""
    return (_HASH or _resolve_once())[0]


def hash_source() -> str:
    """Where :func:`hash_algo` came from: ``policy file <path>``, ``environment
    COMMONKIT_HASH`` or ``default``."""
    return (_HASH or _resolve_once())[1]

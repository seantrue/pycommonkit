"""The hashing facade (contract §3.8): one place that computes digests, in the algorithm the
process resolved once (``HASH``, then ``COMMONKIT_HASH``, then ``b3``; see ``policy``).

The maintainer, D19: "REALLY,REALLY,REALLY try not to use system libraries directly if they
are implementations of swappable algorithms. Hashing is a clear one." So callers write
``from commonkit import digest`` and never import ``hashlib`` or ``blake3``; the default
functions name no algorithm, and which one runs is the tree's decision, not the call site's.

* :func:`raw`, :class:`Hasher` (streaming), :func:`file`: a digest of bytes.
* :func:`nd1`: PlumCraft's protocol-2 array recipe, verbatim: the normalised dtype string,
  then the shape's ``repr``, then the C-order bytes, big-endian data swapped to
  little-endian first (in 16 MiB pieces, so a big array isn't copied whole).
* :data:`fixed`: a named algorithm (``md5``, ``sha256``) for what must not follow the
  switch: goldens, parity records, security and file formats (D9). Allowlisted uses only.
* :data:`build`: both algorithms at once, for a salt baked before the runtime's algorithm
  is known (Rust build scripts; here for the vectors).

Nothing in the store imports this module, so commonkit never hashes a caller's key (the
alexandria constraint), and ``__init__`` doesn't import it either. ``blake3`` is a hard
dependency imported lazily, on the first BLAKE3 digest.
"""
from __future__ import annotations

import hashlib
import os
from types import SimpleNamespace

from . import policy as _policy

#: The big-endian swap streams this many bytes at a time; a multiple of every swap unit.
_SWAP_CHUNK = 16 * 2**20
_READ_CHUNK = 2**20

_blake3 = None


def _b3():
    global _blake3
    if _blake3 is None:
        import blake3
        _blake3 = blake3
    return _blake3.blake3(max_threads=_blake3.blake3.AUTO)


def _new(algo: str):
    if algo == "b3":
        return _b3()
    if algo == "md5":
        return hashlib.md5()
    if algo == "sha256":
        return hashlib.sha256()
    raise ValueError(f"no such algorithm {algo!r}")


def current() -> str:
    """This process's algorithm, ``md5`` or ``b3`` (resolved once, shared with the store)."""
    return _policy.hash_algo()


class Hasher:
    """A digest in progress, in the current algorithm: ``Hasher().update(b).hexdigest()``.

    It takes no algorithm: a named one is ``fixed.Hasher(name)``, whose every use is
    allowlisted where it is made (D9), so ``Hasher("md5")`` can't route around it."""

    def __init__(self):
        self._h = _new(current())

    def update(self, data) -> Hasher:
        self._h.update(data)
        return self

    def hexdigest(self) -> str:
        return self._h.hexdigest()


def _raw(algo: str, data) -> str:
    h = _new(algo)
    h.update(data)
    return h.hexdigest()


def raw(data) -> str:
    """The digest of ``data`` (bytes or any buffer) in the current algorithm."""
    return _raw(current(), data)


# ------------------------------------------------------------------------------ nd1


def _normalise(text: str):
    """``(normalised, itemsize, swap_unit)`` for a numpy ``dtype.str``, as PlumCraft's Rust
    ``Dtype::parse`` reads it: object and structured kinds are refused, datetime units
    are at most 8 characters, ``=`` reads as ``<``, and a big-endian multi-byte kind is
    written ``<`` with the unit its bytes are swapped in."""
    bad = ValueError(f"no content identity for dtype {text!r} (object, structured or odd)")
    if len(text) < 3:
        raise bad
    order, kind, rest = text[0], text[1], text[2:]
    digits, _, unit = rest.partition("[")
    suffix = f"[{unit}" if _ else ""
    if not digits.isascii() or not digits.isdigit() or len(digits) > 6:
        raise bad
    if suffix and not (kind in "Mm" and suffix.endswith("]") and len(suffix) <= 8):
        raise bad
    n = int(digits)
    if kind in "biufMm":
        itemsize, swap = n, n
    elif kind == "c":
        itemsize, swap = n, n // 2
    elif kind == "U":
        itemsize, swap = 4 * n, 4
    elif kind == "S":
        itemsize, swap = n, 1
    else:
        raise bad
    if itemsize == 0 and kind not in "SU":
        raise bad
    if order in "<|":
        return f"{order}{kind}{digits}{suffix}", itemsize, 1
    if order == "=":
        return f"<{kind}{digits}{suffix}", itemsize, 1
    if order == ">":
        if swap > 1:
            return f"<{kind}{digits}{suffix}", itemsize, swap
        return f"|{kind}{digits}{suffix}", itemsize, 1
    raise bad


def _shape_repr(shape) -> str:
    return repr(tuple(int(n) for n in shape))


def _nd1(algo: str, a) -> str:
    import numpy as np
    if a.dtype.hasobject or a.dtype.fields is not None:
        raise ValueError(f"no content identity for dtype {a.dtype} (object or structured)")
    normalised, _itemsize, swap = _normalise(a.dtype.str)
    # Not for a 0-d array: ascontiguousarray returns ndim >= 1, and "(1,)" is not "()"
    a = np.ascontiguousarray(a) if a.ndim else a
    data = a.reshape(-1).view(np.uint8) if a.size and a.dtype.itemsize else b""
    h = _new(algo)
    h.update(normalised.encode("ascii"))
    h.update(_shape_repr(a.shape).encode("ascii"))
    if swap > 1:
        for i in range(0, len(data), _SWAP_CHUNK):
            piece = np.asarray(data[i:i + _SWAP_CHUNK]).reshape(-1, swap)[:, ::-1]
            h.update(np.ascontiguousarray(piece).tobytes())
    else:
        h.update(data)
    return h.hexdigest()


def nd1(a) -> str:
    """The ``nd1`` digest of a numpy array in the current algorithm. Raises ``ValueError``
    for an object or structured dtype, which has no content identity."""
    return _nd1(current(), a)


def file(path, limit: int | None = None) -> tuple[str, int]:
    """``(digest, size)`` of a file, streamed in the current algorithm. ``OSError`` when it
    is (or grows while read) over ``limit`` bytes."""
    h = Hasher()
    total = 0
    with open(os.fspath(path), "rb") as fh:
        if limit is not None and os.fstat(fh.fileno()).st_size > limit:
            raise OSError(f"{path} is over {limit} bytes")
        while chunk := fh.read(_READ_CHUNK):
            total += len(chunk)
            if limit is not None and total > limit:
                raise OSError(f"{path} grew past {limit} bytes while it was read")
            h.update(chunk)
    return h.hexdigest(), total


# ---------------------------------------------------------------------------- fixed

_FIXED = ("md5", "sha256")


def _fixed(name: str) -> str:
    if name not in _FIXED:
        raise ValueError(f"fixed algorithms are {_FIXED}, not {name!r}")
    return name


class _FixedHasher(Hasher):
    def __init__(self, name: str):
        self._h = _new(_fixed(name))


#: A named algorithm, for digests that must NOT follow the switch (D9): goldens, parity
#: records, security and formats. Every use is allowlisted where it is made.
fixed = SimpleNamespace(
    raw=lambda name, data: _raw(_fixed(name), data),
    nd1=lambda name, a: _nd1(_fixed(name), a),
    Hasher=_FixedHasher,
)


# ---------------------------------------------------------------------------- build

class Salts(SimpleNamespace):
    """One input's digest in both algorithms: ``md5`` and ``b3``."""


def _both(data) -> Salts:
    return Salts(md5=_raw("md5", data), b3=_raw("b3", data))


#: For a value computed before the runtime's algorithm is known (a build salt): bake both,
#: pick at run time.
build = SimpleNamespace(both=_both, pick=lambda s: getattr(s, current()))

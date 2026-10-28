"""A digest-keyed store of DECODED arrays, shared across processes.

Extracted from the pixel-cache layer of a private image-corpus
application, where it ran in production first; the measurements below were
taken there. Nothing but numpy, because the python-3.11 model venvs that
consume it cannot import anything heavier.

Why named ``.npy`` files rather than anything cleverer, all measured on a
Linux host (x86-64, a ZFS pool plus tmpfs ``/dev/shm``) on 2026-09-10:

* A named segment **persists with zero attachers**, so it survives a queue hop --
  a record can sit in RabbitMQ for minutes and a later consumer still attaches.
  An anonymous ``memfd`` is freed when the last fd closes, which disqualifies
  fd-passing however attractive its sealing story is.
* **Unlink while mapped is safe**: the name disappears, live mappings keep
  reading, the kernel reclaims on last unmap. So the reaper never has to know who
  is attached, and no cross-process refcounting is needed anywhere.
* ``np.load(mmap_mode="r")`` returns ``writeable=False`` and re-enabling it is
  REFUSED by numpy. Read-only is enforced rather than advisory, for free.
* ``.npy`` is exactly the (shape, dtype, data) triple, with a 128-byte header and
  the payload 64-byte aligned for mmap. ``np.save`` costs 0.93 ms against 0.89 ms
  for a bare buffer write, so the container is free.
* ``multiprocessing.shared_memory`` is unusable here: its resource tracker
  deletes segments when the first unrelated process exits, and ``track=False``
  needs python 3.13 while every model venv is 3.11.

Running this on macOS is a different trade -- no /dev/shm, no Pss to measure
sharing with, and unified memory in which a RAM disk competes directly with
on-device model weights. Measure before enabling it there: on Apple Silicon the
internal SSD beat a RAM disk (first attach 0.69 ms against 0.76 ms), and a
``ram://`` disk commits its whole size up front as dirty memory.

THE INVARIANT THIS WHOLE DESIGN RESTS ON: *a missing segment is never an error,
only a slower path.* Every miss -- reaped, declined, torn, absent root -- returns
None and the caller re-decodes in 38 ms. Nothing here may raise into a caller,
with one deliberate exception: publishing a DIFFERENT array under a key that already
has a segment raises :class:`KeyCollision`, because that is a broken key scheme, not
a slower path (contract §6.4).
"""
from __future__ import annotations

import errno
import logging
import os
import re
import shutil
import stat
import struct
import sys
import threading
import time
from collections import Counter

import numpy as np
from numpy.lib import format as _npf

from . import keys as _keys

try:                                    # POSIX only; Windows has no fcntl (contract §12)
    import fcntl as _fcntl
except ImportError:                     # pragma: no cover - Windows
    _fcntl = None

_log = logging.getLogger(__name__)


class KeyCollision(ValueError):
    """A different array was published under a key whose segment already exists.

    The second of the contract's two loud paths (§1.6, §6.4). A key names one
    immutable content (W1), so this means a key scheme or a digest is broken: two
    contents hash to one name. Swallowing it would leave every reader of the key
    holding whichever content got there first, silently, so it raises instead.
    A ``ValueError`` so a caller that already treats bad keys as ``ValueError``
    sees it without new handling.
    """

#: Touch mtime at most this often per segment. atime is unusable: /dev/shm
#: updates it per read but the ZFS roots do not, even with atime=on relatime=off,
#: because ARC-served reads never refresh it -- and the root is a parameter that
#: spans both. An explicit touch is also BETTER than working atime would be: a
#: metadata write per read is copy-on-write churn on a pool shared with Postgres.
TOUCH_INTERVAL_S = 60.0

#: statvfs is a syscall per publish otherwise. One second of staleness cannot
#: matter to a watermark measured in gigabytes.
_STATVFS_TTL_S = 1.0

#: The largest header a reader accepts and a publisher writes (contract §5.5). numpy's
#: own default is 10000 bytes, which misses every version 2 segment; 1 MiB holds a
#: structured dtype of ~25,000 fields and still bounds what a hostile length field
#: can make a reader allocate.
MAX_HEADER_BYTES = 2**20

_NPY_MAGIC = b"\x93NUMPY"

#: The reaper's candidates (contract §9.4): ``{form}/{xx}/{digest}.{variant}.npy`` with
#: the shard equal to the digest's first two characters. Matched per path component.
_SHARD_RE = re.compile(r"[0-9a-f]{2}")
_SEGMENT_NAME_RE = re.compile(r"([0-9a-f]{2})[0-9a-f]{30}\.[a-z0-9]+\.npy")

#: An orphaned temporary file is swept once it is this old (contract §9.5). A writer
#: holds its ``.tmp`` for one ``np.save`` -- milliseconds -- so an hour old means the
#: writer crashed, never that it is still writing.
TMP_ORPHAN_S = 3600.0

#: Error numbers that mean "the volume is full": a decline, not a failure. EDQUOT is
#: looked up rather than named because not every platform's ``errno`` defines it.
_FULL_ERRNOS = frozenset(e for e in (errno.ENOSPC, getattr(errno, "EDQUOT", None))
                         if e is not None)


def _sync(fd: int) -> None:
    """Push ``fd``'s data to storage.

    On macOS ``fsync`` only hands the data to the drive, which may hold it in its own
    cache across a power loss; ``F_FULLFSYNC`` flushes that cache too. It is what
    "to storage" means there, and it is dear: a 1 MiB write took 0.25 ms, 0.33 ms
    with ``fsync`` and 3.98 ms with ``F_FULLFSYNC`` on thresher's internal SSD
    (2026-10-28) -- one reason fsync is off by default (C17). A filesystem that
    refuses ``F_FULLFSYNC`` still gets a plain ``fsync``.
    """
    full = getattr(_fcntl, "F_FULLFSYNC", None) if sys.platform == "darwin" else None
    if full is not None:
        try:
            _fcntl.fcntl(fd, full)
            return
        except OSError:
            pass
    os.fsync(fd)


def _sync_dir(path: str) -> None:
    """Make a rename in directory ``path`` durable (contract §6.5, step 5).

    POSIX only: a directory is synced through a descriptor opened on it. Windows
    cannot open a directory with ``os.open`` and has no directory fsync to call, so
    there this is a no-op rather than a publish that always fails.
    """
    if os.name != "posix":
        return
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        _sync(fd)
    finally:
        os.close(fd)


def _check_prefix(fh) -> None:
    """Raise ``ValueError`` unless ``fh`` starts with a ``.npy`` prefix of contract §5.1
    whose header length is within :data:`MAX_HEADER_BYTES`.

    Checked before numpy sees the file, so an oversized length field is refused
    without reading (or allocating for) the text it claims (§5.5).
    """
    prefix = fh.read(12)
    if len(prefix) < 10 or prefix[:6] != _NPY_MAGIC or prefix[7] != 0 \
            or prefix[6] not in (1, 2, 3):
        raise ValueError("not a .npy prefix")
    if prefix[6] == 1:
        return                          # a 16-bit length is always within the limit
    if len(prefix) < 12 or struct.unpack("<I", prefix[8:12])[0] > MAX_HEADER_BYTES:
        raise ValueError("header length over the limit")


def _load(path: str, mmap: bool):
    """The array at ``path``, mapped read-only or copied.

    Raises ``FileNotFoundError`` when there is no file. ANY other exception means the
    file is torn (contract §5.7): callers catch ``Exception``, not a list, because
    numpy's failures on a bad header are not a closed set -- a garbled one raised
    ``tokenize.TokenError`` (neither ``ValueError`` nor ``OSError``) straight out of
    ``attach`` until 2026-10-28.
    """
    with open(path, "rb") as fh:
        _check_prefix(fh)
        fh.seek(0)
        if not mmap:
            return np.load(fh, allow_pickle=False, max_header_size=MAX_HEADER_BYTES)
        # Map through the handle already open: ``np.load(mmap_mode=...)`` takes only a
        # path, and opening it a second time cost ~16 us an attach (a 400 kB segment,
        # 55 -> 71 us median on thresher, 2026-10-28), on the path cache.py exists to
        # keep cheap. Version 3 headers (UTF-8 field names) have no public reader, so
        # they, and empty arrays (which can't be mapped), take np.load's path below.
        version = np.lib.format.read_magic(fh)
        read_header = {(1, 0): np.lib.format.read_array_header_1_0,
                       (2, 0): np.lib.format.read_array_header_2_0}.get(version)
        if read_header is not None:
            shape, fortran, dtype = read_header(fh, max_header_size=MAX_HEADER_BYTES)
            if dtype.hasobject:
                raise ValueError("object arrays are never segments (contract §5.4)")
            if dtype.itemsize and all(shape):
                return np.memmap(fh, dtype=dtype, mode="r", offset=fh.tell(),
                                 shape=shape, order="F" if fortran else "C")
    return np.load(path, mmap_mode="r", allow_pickle=False, max_header_size=MAX_HEADER_BYTES)


def _fortran(arr) -> bool:
    """The ``fortran_order`` numpy writes for ``arr`` (contract §5.3)."""
    return bool(arr.flags.f_contiguous and not arr.flags.c_contiguous)


def _header_len(arr) -> int:
    """``HLEN`` of the header ``np.save`` would write for ``arr`` (contract §5.2)."""
    d = _npf.header_data_from_array_1_0(arr)
    shape = d["shape"]
    text = (f"{{'descr': {d['descr']!r}, 'fortran_order': {d['fortran_order']!r}, "
            f"'shape': {shape!r}, }}")
    growth = (max(0, 21 - len(str(shape[-1 if d["fortran_order"] else 0])))
              if shape else 0)
    try:
        n = len(text.encode("latin1"))
        latin1 = True
    except UnicodeEncodeError:
        n = len(text.encode("utf8"))
        latin1 = False
    prefix = 10
    hlen = n + growth + 64 - ((prefix + n + growth + 1) % 64) + 1
    if not latin1 or hlen > 0xFFFF:
        prefix = 12
        hlen = n + growth + 64 - ((prefix + n + growth + 1) % 64) + 1
    return hlen


def _named_byte_mask(dtype: np.dtype):
    """A bool per byte of an item, True where a NAMED field covers it; None when every
    byte is covered (no padding), the common case, so the caller compares whole items.

    Padding bytes carry no meaning and a copying ``np.load`` zeroes them, so they must
    never decide a collision (contract §5.8).
    """
    if dtype.names is None:
        return None
    mask = np.zeros(dtype.itemsize, dtype=bool)

    def cover(dt: np.dtype, offset: int) -> None:
        if dt.subdtype is not None:
            base, shape = dt.subdtype
            count = int(np.prod(shape))
            if base.names is None:
                mask[offset:offset + count * base.itemsize] = True
            else:
                for i in range(count):
                    cover(base, offset + i * base.itemsize)
        elif dt.names is None:
            mask[offset:offset + dt.itemsize] = True
        else:
            for name in dt.names:
                field = dt.fields[name]
                cover(field[0], offset + field[1])

    cover(dtype, 0)
    return None if mask.all() else mask


def _payload_items(arr):
    """``arr``'s payload as written (contract §5.3), as an ``(items, itemsize)`` byte
    matrix. Fortran order is the transpose's C order; anything else is C order, which
    is what ``np.save`` writes for a non-contiguous array."""
    flat = np.ascontiguousarray(arr.T if _fortran(arr) else arr).reshape(-1)
    return flat.view(np.uint8).reshape(flat.shape[0], arr.dtype.itemsize)


def _same_payload(old, new) -> bool:
    """Whether two arrays of one dtype, shape and order hold the same named-field bytes.

    Compared in chunks of about 1 MiB, so a 128 MiB segment costs a 1 MiB temporary
    rather than a second 128 MiB one.
    """
    if new.nbytes == 0:
        return True
    a, b = _payload_items(old), _payload_items(new)
    mask = _named_byte_mask(new.dtype)
    step = max(1, (1 << 20) // max(1, new.dtype.itemsize))
    for i in range(0, a.shape[0], step):
        x, y = a[i:i + step], b[i:i + step]
        if mask is not None:
            x, y = x[:, mask], y[:, mask]
        if not np.array_equal(x, y):
            return False
    return True


class SharedArrayStore:
    """Publish and attach decoded arrays under one root directory.

    Satisfies a consumer's ``PixelCache`` protocol (``get``/``set``), plus the
    ``set_array`` fast path an array-oriented caller prefers.

    The constructor NEVER raises. An absent, unwritable or unusable root sets
    ``self.disabled`` and every method becomes a no-op: a broken cache backend
    degrades to cache misses rather than breaking its caller.
    """

    def __init__(self, root, *, mode="read", attach="mmap",
                 max_bytes=16 * 2**30, min_free_bytes=8 * 2**30,
                 ttl_s=900, min_array_bytes=256 * 1024,
                 max_array_bytes=128 * 2**20, reap_interval_s=30,
                 fsync=False, verify_republish=False):
        # An empty root stays empty, so the check below disables the store (contract §6.1):
        # ``os.path.abspath("")`` is the working directory, which an empty root used to
        # become, publishing segments into wherever the process happened to start.
        root = str(root or "")
        self.root = os.path.abspath(root) if root else ""
        self.mode = str(mode or "off")
        self.attach_mode = str(attach or "mmap")
        self.max_bytes = int(max_bytes)
        self.min_free_bytes = int(min_free_bytes)
        self.ttl_s = float(ttl_s)
        self.min_array_bytes = int(min_array_bytes)
        self.max_array_bytes = int(max_array_bytes)
        self.reap_interval_s = float(reap_interval_s)
        # Durability (C17): off, a crash can leave a short segment, which readers miss
        # and publishers replace (W4, W5). Verification (C15): off, a republish
        # compares headers only, so a same-header collision goes unseen.
        self.fsync = bool(fsync)
        self.verify_republish = bool(verify_republish)

        self.stats: Counter = Counter()
        self._lock = threading.Lock()
        self._statvfs_at = 0.0
        self._statvfs_free = 0
        self._last_reap = 0.0
        self.disabled = True

        if self.mode == "off" or not self.root or self.root == "/":
            return
        # The probe name MUST be unique per process. A fixed ".writable" races:
        # K workers starting together on one root all write the same path, the
        # first unlink wins, and every loser's unlink raises FileNotFoundError --
        # an OSError, caught below, leaving that worker permanently disabled with
        # no log line. Measured: at 4 forked consumers one came up disabled and
        # decoded all 48 pages while the other three attached.
        probe = os.path.join(
            self.root, f".writable.{os.getpid()}.{threading.get_ident():x}")
        try:
            os.makedirs(self.root, exist_ok=True)
            with open(probe, "wb") as fh:
                fh.write(b"1")
        except OSError:
            return                      # unusable root: stay disabled, silently
        finally:
            # Cleanup failure is NOT a writability failure: the write above
            # already proved the root is usable.
            try:
                os.unlink(probe)
            except OSError:
                pass
        self.disabled = False

    # ------------------------------------------------------------- addressing

    def path_for(self, key: str) -> str:
        """Absolute path for ``key``, guaranteed to live under ``self.root``.

        ``keys.relpath`` is strict about the key shape, so a malformed key raises
        here rather than producing a path the reaper might later unlink.
        """
        return os.path.join(self.root, _keys.relpath(key))

    # ---------------------------------------------------------------- reading

    def attach(self, key: str):
        """The decoded array for ``key``, or None on any kind of miss."""
        if self.disabled:
            return None
        try:
            path = self.path_for(key)
        except ValueError:
            self.stats["shm_bad_key"] += 1
            return None
        try:
            arr = _load(path, mmap=self.attach_mode != "read")
        except FileNotFoundError:
            self.stats["shm_miss"] += 1
            return None
        except Exception:                                      # noqa: BLE001
            # A truncated or half-written segment is a MISS, not an exception.
            # os.replace makes this nearly impossible, but "nearly" is not a
            # reason to let a caller see a traceback. Broad on purpose: see _load.
            self.stats["shm_torn"] += 1
            return None
        self.stats["shm_hit"] += 1
        self._touch(path)
        return arr

    def _touch(self, path: str) -> None:
        """Refresh mtime for LRU, at most once per TOUCH_INTERVAL_S per segment."""
        try:
            st = os.stat(path)
            if time.time() - st.st_mtime > TOUCH_INTERVAL_S:
                os.utime(path, None)
        except OSError:
            pass

    # ---------------------------------------------------------------- writing

    def _admit_dtype(self, key: str, arr) -> bool:
        """Refuse a segment whose dtype its FORM declares it cannot have.

        Enforced HERE rather than trusted of every publisher, because a bad
        segment is not one bad call: it is published once and then poisons
        every reader that attaches it, on a path where a miss is meant to be
        the worst possible outcome. A publisher that forgets should be unable
        to do this, not merely discouraged from it.

        WHICH dtype is the DOMAIN's business, not the store's, so the scheme
        declares it per form and this only enforces what it finds. (Pixels
        declares ``pil`` uint8: consumers recover the channel count from
        ``.shape`` and trust the producer for the colour space, which a bool
        array satisfies neither of -- and the store used to accept and
        round-trip one happily.) A form with no declared dtype, like a float32
        tensor or an int16 waveform, is unconstrained.

        Whatever the form, two things are never written (contract §6.3). An object
        dtype (``hasobject`` also covers a struct with an object field, and numpy 2's
        ``StringDType``) needs pickling, which no reader may do; ``np.save`` used to
        raise ``ValueError`` here, uncaught, and leave the temporary file behind. And
        a header over :data:`MAX_HEADER_BYTES`, which no reader would accept.
        """
        dtype = getattr(arr, "dtype", None)
        if dtype is None or dtype.hasobject:
            self.stats["shm_declined_dtype"] += 1
            return False
        # Only a structured dtype can approach the limit: a simple descriptor and up to
        # 64 dimensions stay under 2 KB, so the common case skips building the header.
        if dtype.names is not None:
            try:
                fits = _header_len(arr) <= MAX_HEADER_BYTES
            except Exception:                                  # noqa: BLE001
                fits = False                # a header numpy cannot describe is not written
            if not fits:
                self.stats["shm_declined_dtype"] += 1
                return False
        try:
            scheme = _keys.scheme_for(key)
            form, _digest, _variant = scheme.parse(key)
        except ValueError:
            return True             # path_for will reject it and count it there
        want = scheme.dtype_for(form)
        if want is None:
            return True
        if getattr(arr, "dtype", None) != np.dtype(want):
            self.stats["shm_declined_dtype"] += 1
            return False
        return True

    def publish(self, key: str, arr) -> bool:
        """Store ``arr`` under ``key``. Returns whether it was stored.

        Returns a bool rather than an array on purpose: the publisher has already
        paid for the decode and holds the pixels, so making it re-attach its own
        data would be strictly worse than keeping what it has.

        Republishing the same array touches the existing segment and returns True.
        Publishing a DIFFERENT array under a key that already has a whole segment
        raises :class:`KeyCollision` (contract §6.4) -- the one way this raises.
        """
        if self.disabled or self.mode != "readwrite":
            self.stats["shm_declined_mode"] += 1
            return False
        try:
            nbytes = int(arr.nbytes)
        except AttributeError:
            return False
        if not self._admit(nbytes):
            return False
        if not self._admit_dtype(key, arr):
            return False
        try:
            path = self.path_for(key)
        except ValueError:
            self.stats["shm_bad_key"] += 1
            return False

        existing = self._existing(key, path, arr)
        if existing in ("republish", "failed"):
            return existing == "republish"

        tmp = f"{path}.{os.getpid()}.{threading.get_ident()}.tmp"
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            # np.save appends .npy when the name lacks it; ours already has one
            # inside, so write through a handle and control the name exactly.
            with open(tmp, "wb") as fh:
                np.save(fh, arr, allow_pickle=False)
                if self.fsync:
                    fh.flush()
                    _sync(fh.fileno())
            os.replace(tmp, path)       # atomic within one filesystem
        except Exception as exc:                               # noqa: BLE001
            # Broad, not OSError: np.save can raise for an array the gates admitted
            # (and a UserWarning becomes an exception under ``-W error``), and a
            # publish never raises into its caller except for a KeyCollision.
            try:
                os.unlink(tmp)
            except OSError:
                pass
            if getattr(exc, "errno", None) in _FULL_ERRNOS:
                self.stats["shm_declined_space"] += 1
            else:
                self.stats["shm_publish_failed"] += 1
            return False
        if existing == "torn":
            self.stats["shm_torn_replaced"] += 1
        if self.fsync:
            # After the rename the segment is whole and readable, so a failed sync
            # leaves it in place; but the caller asked for durability and did not get
            # it, so the publish reports failure (contract §6.5).
            try:
                _sync_dir(os.path.dirname(path))
            except Exception:                                  # noqa: BLE001
                self.stats["shm_publish_failed"] += 1
                return False
        self.stats["shm_publish"] += 1
        self.stats["shm_bytes_published"] += nbytes
        self._maybe_reap()
        return True

    def _existing(self, key: str, path: str, arr) -> str:
        """Apply the existing-key rule (contract §6.4, C15) before anything is written.

        Returns ``"absent"`` or ``"torn"`` (write it), ``"republish"`` (the same array
        is already there: touched, nothing written), or ``"failed"`` (the comparison
        itself failed: counted, nothing written). Raises :class:`KeyCollision` for a
        different array, leaving the file and its mtime exactly as they were.

        Republishing used to rewrite the segment unconditionally, which is a write into
        the tree every reader maps, for nothing, and which hid a broken digest: the
        second content silently replaced the first under readers that had the first.
        """
        try:
            old = _load(path, mmap=True)        # mapped: a check must not copy 128 MiB
        except FileNotFoundError:
            return "absent"
        except Exception:                                      # noqa: BLE001
            # Torn (W4): damage from outside the contract, so replacing it is repair,
            # not a rewrite of a whole segment.
            return "torn"
        try:
            # The header the new array WOULD be written with, against the file's.
            # fortran_order is read off the mapping's flags: they differ from the
            # header's only for a degenerate shape, whose C and F bytes are identical.
            same = (old.dtype == arr.dtype and old.shape == arr.shape
                    and _fortran(old) == _fortran(arr))
            reason = "a different dtype, shape or memory order"
            if same and self.verify_republish:
                same = _same_payload(old, arr)
                reason = "different named-field bytes"
        except Exception:                                      # noqa: BLE001
            # Not knowing is not a collision: the whole segment stays, nothing raises.
            self.stats["shm_publish_failed"] += 1
            return "failed"
        finally:
            del old                             # unmap before the touch (Windows pins it)
        if same:
            self._touch(path)
            return "republish"
        self.stats["shm_key_collision"] += 1
        _log.error("commonkit key collision: %s already holds a whole segment at %s; the "
                   "new array has %s. A key scheme or digest is broken.", key, path, reason)
        raise KeyCollision(f"{key}: an existing segment holds {reason}")

    def _admit(self, nbytes: int) -> bool:
        """Four gates, cheapest first. A decline is normal operation, not a fault.

        A space or budget decline also gives the reaper its rate-limited turn. It used to
        run only after a SUCCESSFUL publish, so a store that filled up declined every
        publish from then on and nothing ever reaped it again -- not even TTL expiry
        (found 2026-10-16: a store left full for 5.7 days with 1,104 expired segments).

        This is the answer to "reading a whole stream into shared memory because
        something downstream jammed". Over any gate we decline and the caller
        keeps the private array it already holds -- we never block and never nack,
        so the pipeline degrades to its pre-cache speed instead of stalling.
        """
        if nbytes < self.min_array_bytes:
            self.stats["shm_declined_small"] += 1
            return False
        if nbytes > self.max_array_bytes:
            # Not optional: a long audio item can reach gigabytes as a
            # waveform, which would swallow the whole tree in one publish.
            self.stats["shm_declined_size"] += 1
            return False
        if self._free_bytes() < self.min_free_bytes:
            self.stats["shm_declined_space"] += 1
            self._maybe_reap()
            return False
        used, _ = self.usage()
        if used >= 0 and used + nbytes > self.max_bytes:
            self.stats["shm_declined_budget"] += 1
            self._maybe_reap()
            return False
        return True

    def _free_bytes(self, fresh: bool = False) -> int:
        now = time.time()
        if fresh or now - self._statvfs_at > _STATVFS_TTL_S:
            # Any failure is 0 free, not an exception (contract §9.9): catching only
            # OSError let AttributeError out on a platform without os.statvfs.
            # Windows has none, so it asks shutil, whose POSIX branch is this same
            # f_bavail * f_frsize; how Windows measures is not yet contract (§12).
            try:
                if hasattr(os, "statvfs"):
                    st = os.statvfs(self.root)
                    self._statvfs_free = int(st.f_bavail * st.f_frsize)
                else:                                          # pragma: no cover
                    self._statvfs_free = int(shutil.disk_usage(self.root).free)
            except Exception:                                  # noqa: BLE001
                self._statvfs_free = 0
            self._statvfs_at = now
        return self._statvfs_free

    # ----------------------------------------------------- PixelCache protocol

    def get(self, key: str):
        return self.attach(key)

    def get_with_source(self, key: str):
        """``(array, source)``, matching ``TieredPixelCache``.

        A bare store is the L2 tier and nothing else, so a hit is always
        ``"shm"``. Implemented here so ``entry.decoded`` never has to GUESS
        which tier a cache represents -- guessing is what it used to do.
        """
        arr = self.attach(key)
        return (arr, "shm") if arr is not None else (None, "decode")

    def set_array(self, key: str, arr) -> None:
        self.publish(key, arr)

    def set(self, key: str, value: bytes) -> None:
        """Protocol compliance. The array path is ``set_array``; this exists so
        the type still satisfies ``PixelCache`` when driven by bytes."""
        try:
            import io
            arr = np.load(io.BytesIO(value), allow_pickle=False)
        except Exception:                                      # noqa: BLE001
            return
        self.publish(key, arr)

    # ---------------------------------------------------------------- upkeep

    def usage(self) -> tuple[int, int]:
        """``(bytes, files)`` for the tree, from the reaper's cached walk.

        An in-process counter under-counts badly when six replicas publish into
        one tree, so the authoritative number comes from a walk written to
        ``.usage.json``. Returns ``(-1, -1)`` when no fresh walk exists, which
        ``_admit`` reads as "unknown" and falls back to the statvfs watermark.
        """
        marker = os.path.join(self.root, ".usage.json")
        try:
            st = os.stat(marker)
            if time.time() - st.st_mtime > 2 * self.reap_interval_s:
                return (-1, -1)
            import json
            with open(marker, "rb") as fh:
                d = json.loads(fh.read())
        except Exception:                                      # noqa: BLE001
            return (-1, -1)                 # RecursionError too: a hostile file is unknown
        # Valid only with an integer ``bytes`` (contract §9.7): ``int()`` used to take a
        # float or a numeric string as well, which no other implementation would trust.
        used = d.get("bytes") if isinstance(d, dict) else None
        if type(used) is not int or used < 0:
            return (-1, -1)
        files = d.get("files")
        return used, (files if type(files) is int else -1)

    def _maybe_reap(self) -> None:
        now = time.time()
        if now - self._last_reap < self.reap_interval_s:
            return
        self._last_reap = now
        try:
            self.reap(budget_s=0.05)
        except Exception:                                      # noqa: BLE001
            self.stats["shm_reap_failed"] += 1

    def reap(self, *, budget_s=0.25) -> dict:
        """Unlink expired segments, then the least recently used (oldest mtime; attach
        refreshes it) until the store is under its low water mark and the volume above
        its free-space floor. Lock-guarded; ``budget_s`` bounds the walk.

        Safe because unlink-while-mapped is safe: a consumer mid-read keeps its
        mapping, and a consumer that arrives later simply misses and re-decodes.

        BLAST RADIUS. This is the only thing in the store that deletes anything,
        so it is deliberately narrow: it walks only ``self.root``, unlinks only
        regular files whose PATH matches the segment shape (and orphaned ``.tmp``
        files over an hour old beside them), never follows symlinks, and refuses
        to run at all if the root looks wrong.
        """
        out = {"scanned": 0, "reaped": 0, "bytes": 0}
        if self.disabled:
            return out
        if not self.root or self.root == "/" or self.root.count(os.sep) < 2:
            self.stats["shm_reap_refused"] += 1
            return out

        lock = os.path.join(self.root, ".reap.lock")
        try:
            fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
        except FileExistsError:
            try:                            # break a lock left by a dead process
                if time.time() - os.stat(lock).st_mtime > 300:
                    os.unlink(lock)
            except OSError:
                pass
            return out
        except OSError:
            return out

        try:
            os.write(fd, str(os.getpid()).encode())
            os.close(fd)
            deadline = time.time() + float(budget_s)
            now = time.time()
            entries = []
            complete = True
            for dirpath, dirnames, filenames in os.walk(self.root):
                # checked BEFORE each directory: "partial" means one went unscanned
                if time.time() > deadline:
                    complete = False
                    break
                # Candidates by PATH SHAPE only (C6(a)): {form}/{xx}/{xx...}.{variant}.npy.
                # Any ``.npy`` anywhere under the root used to qualify, so a reserved
                # ``.pinned/`` tree or a stray ``notes.npy`` was reaped like a segment.
                # Pruning the walk to that shape also means it never descends further.
                rel = os.path.relpath(dirpath, self.root)
                parts = [] if rel == os.curdir else rel.split(os.sep)
                if len(parts) == 0:
                    dirnames[:] = [d for d in dirnames if _keys._TOKEN_RE.fullmatch(d)]
                    continue
                if len(parts) == 1:
                    dirnames[:] = [d for d in dirnames if _SHARD_RE.fullmatch(d)]
                    continue
                dirnames[:] = []
                shard = parts[1]
                for name in filenames:
                    is_tmp = name.endswith(".tmp")
                    m = None if is_tmp else _SEGMENT_NAME_RE.fullmatch(name)
                    if not (is_tmp or (m and m.group(1) == shard)):
                        continue
                    p = os.path.join(dirpath, name)
                    try:
                        st = os.lstat(p)            # lstat: never follow a symlink
                    except OSError:
                        continue
                    if not stat.S_ISREG(st.st_mode):
                        continue
                    if is_tmp:
                        # An orphan of a crashed writer (C6(b)). Not a segment: not
                        # scanned, not usage, not shm_reaped.
                        if now - st.st_mtime > TMP_ORPHAN_S:
                            try:
                                os.unlink(p)
                            except OSError:
                                pass
                        continue
                    out["scanned"] += 1
                    entries.append((st.st_mtime, st.st_size, p))

            total = sum(e[1] for e in entries)
            entries.sort()                          # oldest mtime first: LRU

            # Two targets, least recently used first. The byte budget's low water mark
            # (judged only on a complete walk: a partial one under-counts), and the
            # free-space floor with 25% headroom. The floor used to be a publish gate
            # only, so a volume that filled before the budget did -- a quota smaller than
            # max_bytes + min_free_bytes -- declined everything and evicted nothing.
            low_water = int(0.8 * self.max_bytes)
            need_free = max(0, int(1.25 * self.min_free_bytes) - self._free_bytes(fresh=True))
            freed = 0
            for mtime, size, p in entries:
                expired = (now - mtime) > self.ttl_s
                over = complete and total > low_water
                short = freed < need_free
                if not (expired or over or short):
                    break
                # Unlinking is cheap and bounded by the walk, so eviction always reaches
                # its target: the time budget limits the walk, not the eviction. A budget
                # that cut eviction short left a store 1.2 GB over its low water mark.
                try:
                    os.unlink(p)
                except OSError:
                    continue
                total -= size
                freed += size
                out["reaped"] += 1
                out["bytes"] += size
            out["complete"] = complete

            if complete:                            # a partial walk would under-count
                try:
                    import json
                    tmp = os.path.join(self.root, ".usage.json.tmp")
                    with open(tmp, "w") as fh:
                        json.dump({"bytes": total,
                                   "files": out["scanned"] - out["reaped"],
                                   "ts": now}, fh)
                    os.replace(tmp, os.path.join(self.root, ".usage.json"))
                except OSError:
                    pass
            else:
                self.stats["shm_reap_partial"] += 1
            self._statvfs_at = 0.0                  # free space changed: re-measure

            self.stats["shm_reaped"] += out["reaped"]
            self.stats["shm_reaped_bytes"] += out["bytes"]
            return out
        finally:
            try:
                os.unlink(lock)
            except OSError:
                pass

    def drain_stats(self) -> Counter:
        """Take and reset the counters, for folding into a node's context.stats."""
        with self._lock:
            out = Counter(self.stats)
            self.stats.clear()
        return out

    def __repr__(self) -> str:
        state = "disabled" if self.disabled else f"{self.mode}/{self.attach_mode}"
        return f"<SharedArrayStore {self.root!r} {state}>"

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
None and the caller re-decodes in 38 ms. Nothing here may raise into a caller.
"""
from __future__ import annotations

import errno
import os
import threading
import time
from collections import Counter

import numpy as np

from . import keys as _keys

#: Touch mtime at most this often per segment. atime is unusable: /dev/shm
#: updates it per read but the ZFS roots do not, even with atime=on relatime=off,
#: because ARC-served reads never refresh it -- and the root is a parameter that
#: spans both. An explicit touch is also BETTER than working atime would be: a
#: metadata write per read is copy-on-write churn on a pool shared with Postgres.
TOUCH_INTERVAL_S = 60.0

#: statvfs is a syscall per publish otherwise. One second of staleness cannot
#: matter to a watermark measured in gigabytes.
_STATVFS_TTL_S = 1.0


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
                 max_array_bytes=128 * 2**20, reap_interval_s=30):
        self.root = os.path.abspath(str(root or ""))
        self.mode = str(mode or "off")
        self.attach_mode = str(attach or "mmap")
        self.max_bytes = int(max_bytes)
        self.min_free_bytes = int(min_free_bytes)
        self.ttl_s = float(ttl_s)
        self.min_array_bytes = int(min_array_bytes)
        self.max_array_bytes = int(max_array_bytes)
        self.reap_interval_s = float(reap_interval_s)

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
            if self.attach_mode == "read":
                arr = np.load(path, allow_pickle=False)
            else:
                arr = np.load(path, mmap_mode="r", allow_pickle=False)
        except FileNotFoundError:
            self.stats["shm_miss"] += 1
            return None
        except (ValueError, EOFError, OSError):
            # A truncated or half-written segment is a MISS, not an exception.
            # os.replace makes this nearly impossible, but "nearly" is not a
            # reason to let a caller see a traceback.
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
        """
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

        tmp = f"{path}.{os.getpid()}.{threading.get_ident()}.tmp"
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            # np.save appends .npy when the name lacks it; ours already has one
            # inside, so write through a handle and control the name exactly.
            with open(tmp, "wb") as fh:
                np.save(fh, arr, allow_pickle=False)
            os.replace(tmp, path)       # atomic within one filesystem
        except OSError as exc:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            if exc.errno in (errno.ENOSPC, errno.EDQUOT):
                self.stats["shm_declined_space"] += 1
            else:
                self.stats["shm_publish_failed"] += 1
            return False
        self.stats["shm_publish"] += 1
        self.stats["shm_bytes_published"] += nbytes
        self._maybe_reap()
        return True

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
            try:
                st = os.statvfs(self.root)
                self._statvfs_free = st.f_bavail * st.f_frsize
            except OSError:
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
            with open(marker) as fh:
                d = json.load(fh)
            return int(d.get("bytes", -1)), int(d.get("files", -1))
        except (OSError, ValueError):
            return (-1, -1)

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
        regular files whose name matches the segment shape, never follows
        symlinks, and refuses to run at all if the root looks wrong.
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
                for name in filenames:
                    if not name.endswith(".npy"):
                        continue
                    p = os.path.join(dirpath, name)
                    try:
                        st = os.lstat(p)            # lstat: never follow a symlink
                    except OSError:
                        continue
                    if not os.path.stat.S_ISREG(st.st_mode):
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

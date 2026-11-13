"""Marker files: evidence of a declined key, written when it happens (contract §11).

A declined key never reaches disk, so the only evidence of a digest leak is in the process
that declined it, and engines leave through ``os._exit`` or SIGKILL, where no at-exit hook
runs. So when a store has a mark folder (``COMMONKIT_MARK_DIR``, or ``mark_dir=``), the
first ``shm_foreign_digest`` or ``shm_bad_key`` event for that folder in a process creates
``<folder>/<pid>.<counter>`` at once, and the first store built for the folder creates
``<pid>.started``, naming the algorithm and root. With no folder nothing is written.

Standard library only, and nothing from commonkit: the store imports this, and nothing the
store imports may hash. A failed write is ignored; it never fails a store call.
"""
from __future__ import annotations

import json
import os
import sys
import threading

MARK_ENV = "COMMONKIT_MARK_DIR"

#: ``(pid, folder, name)`` already written in this process; the pid keeps a forked child
#: writing its own.
_SEEN: set[tuple[int, str, str]] = set()
_LOCK = threading.Lock()


def from_env() -> str:
    """The mark folder named by the environment, or ``""``."""
    return os.environ.get(MARK_ENV, "")


def program() -> str:
    return os.path.basename(sys.argv[0]) if sys.argv and sys.argv[0] else "python"


def _write_once(folder: str, name: str, record: dict) -> None:
    if not folder:
        return
    pid = os.getpid()
    with _LOCK:
        if (pid, folder, name) in _SEEN:
            return
        _SEEN.add((pid, folder, name))
    try:
        os.makedirs(folder, mode=0o700, exist_ok=True)
        fd = os.open(os.path.join(folder, f"{pid}.{name}"),
                     os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump({"program": program(), "pid": pid, **record}, fh, sort_keys=True)
            fh.write("\n")
    except OSError:
        pass


def started(folder: str, algo: str, source: str, root: str) -> None:
    _write_once(folder, "started", {"algo": algo, "source": source, "root": root})


def event(folder: str, counter: str, key, root: str) -> None:
    key = str(key)[:300]
    scheme = ":".join(key.split(":")[:2])
    _write_once(folder, counter, {"counter": counter, "scheme": scheme, "key": key,
                                  "root": root})

"""Prune a folder of files nothing refers to.

Not the store: the store's reaper evicts cache segments by age and size (contract section 9).
This is for durable files an application keeps in a folder of its own, named for their content
(a hand edit, a saved variant), where an old file stays useful while anything can still reach it:
an undo history, another document, a list the application keeps. The application says which names
are reachable; this removes the rest, carefully:

- it looks only at the folder's own entries: it never descends and never follows a symbolic
  link, and the folder itself must be a real directory;
- a candidate is a regular file (by ``lstat``) whose name matches ``pattern`` (a regular
  expression, matched against the whole name) and, when ``owns`` is given, that ``owns`` claims
  (a sidecar naming the application, a hash that matches); anything else is foreign and is kept;
- a candidate named in ``keep`` is kept, as is one modified less than ``min_age_s`` ago (another
  process may hold it in a history this one can't see);
- a removed file's companions (its name plus each of ``companions``, such as ``".json"``) go with
  it, when they are regular files;
- ``dry_run`` (the default) lists what would go and removes nothing.

Nothing here raises for a file it meets: a failed unlink is counted and the walk goes on. The
Rust crate's ``commonkit::prune`` does the same, checked by ``conformance/vectors/prune.json``.
"""

from __future__ import annotations

import os
import re
import stat
import time
from collections.abc import Callable, Iterable
from pathlib import Path


def _regular(path: Path) -> bool:
    try:
        return stat.S_ISREG(os.lstat(path).st_mode)
    except OSError:
        return False


def prune_folder(folder, *, pattern: str, keep: Iterable[str], min_age_s: float,
                 companions: Iterable[str] = (), owns: Callable[[Path], bool] | None = None,
                 dry_run: bool = True, now: float | None = None) -> dict:
    """Remove (or with ``dry_run`` list) the files in ``folder`` nothing refers to.

    Returns ``{"removed": [names, sorted], "kept": n reachable, "young": n, "foreign": n,
    "failed": n, "refused": reason or None}``; ``removed`` lists companions too.
    """
    report = {"removed": [], "kept": 0, "young": 0, "foreign": 0, "failed": 0, "refused": None}
    folder = Path(folder)
    try:
        st = os.lstat(folder)
    except OSError as e:
        report["refused"] = f"{folder}: {e.strerror or e}"
        return report
    if not stat.S_ISDIR(st.st_mode):
        report["refused"] = f"{folder} is not a directory (a symbolic link is not followed)"
        return report
    try:
        rx = re.compile(pattern)
    except re.error as e:
        report["refused"] = f"pattern: {e}"
        return report
    keep = set(keep)
    companions = [c for c in companions if c]
    now = time.time() if now is None else now
    try:
        names = sorted(os.listdir(folder))
    except OSError as e:
        report["refused"] = f"{folder}: {e.strerror or e}"
        return report
    present = set(names)
    for name in names:
        path = folder / name
        try:
            st = os.lstat(path)
        except OSError:
            continue
        # a companion of some candidate is judged with it, not on its own
        if any(name.endswith(c) and name[: -len(c)] in present and rx.fullmatch(name[: -len(c)])
               for c in companions):
            continue
        if not stat.S_ISREG(st.st_mode) or not rx.fullmatch(name):
            report["foreign"] += 1
            continue
        if owns is not None:
            try:
                ours = bool(owns(path))
            except Exception:  # noqa: BLE001 - an owner check that fails claims nothing
                ours = False
            if not ours:
                report["foreign"] += 1
                continue
        if name in keep:
            report["kept"] += 1
            continue
        if now - st.st_mtime < min_age_s:
            report["young"] += 1
            continue
        going = [name] + [name + c for c in companions if (name + c) in present and _regular(folder / (name + c))]
        for g in going:
            if not dry_run:
                try:
                    os.unlink(folder / g)
                except OSError:
                    report["failed"] += 1
                    continue
            report["removed"].append(g)
    report["removed"].sort()
    return report

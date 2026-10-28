"""The Python side of the cross-language conformance driver (plan step 8).

``rust/ck-conform`` drives the Rust crate; this drives the Python package; both speak the
same JSON, so ``tests/test_conformance.py`` and the Rust tests in ``rust/ck-conform/tests``
can run any case against either language and compare. It is test tooling, not part of the
package (it lives outside ``src/``).

Usage::

    python conformance/ck_conform.py <command> [<json> | @<file> | -]

``<json>`` is the command's arguments (an object). ``batch`` takes a list of
``{"cmd": ..., ...}`` objects and prints a list of results. Every result is one line of
JSON on stdout; ``hold`` prints two. Commands:

* ``publish``: ``store``, ``key``, ``array`` -> ``stored`` (bool) or ``collision: true``.
* ``remove``: ``store``, ``key`` -> ``removed`` (bool).
* ``attach``: ``store``, ``key`` -> ``hit``, and for a hit ``descr`` (Python's repr of the
  header descriptor), ``shape``, ``fortran_order``, ``payload_sha256``, ``mapped``.
* ``hold``: as ``attach``, then waits for a line on stdin, then prints ``payload_sha256``
  of the same mapping again and ``path_exists``.
* ``reap``: ``store``, ``budget_s``, ``dry_run``, ``now``; test seams ``die_in_walk``
  (exit 17 holding the lock, as a crashed reaper) and ``sleep_before_unlink`` (seconds,
  before the first segment unlink) -> the report, with ``unlink``/``sweep`` relative to the
  root for a dry run.
* ``plan``: ``root``, ``now``, ``budget_s`` -> a dry run with the tree's own policy
  (``.policy.json``, then the environment), constructing no store, so nothing is written.
* ``usage``: ``store`` -> ``usage``: ``{"bytes", "files"}`` or null.
* ``policy``: ``root`` -> the effective tree-wide settings.

Every command but ``plan`` and ``policy`` also returns ``stats``: the counters it moved.
Any command takes ``start_at`` (Unix seconds) to start together with other processes.

``store`` fields are the store's settings (``root``, ``mode``, ``attach``, ``max_bytes``,
``min_free_bytes``, ``ttl_s``, ``min_array_bytes``, ``max_array_bytes``,
``reap_interval_s``, ``fsync``, ``verify_republish``); a missing one takes the driver
default below, which is the same in both drivers. ``array`` is a contract §13.2 spec
(``descr`` or ``fields``, ``shape``, ``order``) with ``fill`` or ``payload_hex``.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
import time

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from generate import _fill, spec_dtype

import commonkit.store as cks
from commonkit import KeyCollision, KeyScheme, SharedArrayStore, register
from commonkit import shared as cksh

#: The conformance scheme, registered identically by both drivers.
SCHEME = register(KeyScheme(domain="ckconf", version="v1", forms=("raw", "img", "f4"),
                            variants=("x0", "x1"), dtypes={"img": "|u1", "f4": "<f4"}))

#: Driver defaults: no gate in the way, no reap turn unless asked for.
STORE_DEFAULTS = {"mode": "readwrite", "attach": "mmap", "max_bytes": 2**40,
                  "min_free_bytes": 0, "ttl_s": 1e9, "min_array_bytes": 0,
                  "max_array_bytes": 2**40, "reap_interval_s": 1e12, "fsync": False,
                  "verify_republish": True}


def _store(args) -> SharedArrayStore:
    s = {**STORE_DEFAULTS, **args["store"]}
    root = s.pop("root")
    return SharedArrayStore(root, **s)


def _array(spec):
    dtype = spec_dtype(spec)
    shape = tuple(spec["shape"])
    n = int(np.prod(shape, dtype=np.int64)) if shape else 1
    if "payload_hex" in spec:
        payload = bytes.fromhex(spec["payload_hex"])
    else:
        payload = _fill(dtype, n, spec.get("fill", "ramp"))
    flat = np.frombuffer(payload, dtype=dtype, count=n)
    return flat.reshape(shape, order=spec.get("order", "C"))


def _stats(store) -> dict:
    return {k: v for k, v in sorted(store.drain_stats().items()) if v}


def _payload_sha(arr) -> str:
    # The payload in file order: Fortran when the array is F- and not C-contiguous.
    return hashlib.sha256(np.asarray(arr).tobytes(order="A")).hexdigest()


def _describe(arr) -> dict:
    descr = np.lib.format.dtype_to_descr(arr.dtype)
    return {"hit": True, "descr": repr(descr), "shape": list(arr.shape),
            "fortran_order": bool(arr.flags.f_contiguous and not arr.flags.c_contiguous),
            "payload_sha256": _payload_sha(arr), "mapped": isinstance(arr, np.memmap)}


def cmd_publish(args):
    store = _store(args)
    try:
        out = {"stored": store.publish(args["key"], _array(args["array"]))}
    except KeyCollision:
        out = {"collision": True}
    return {**out, "stats": _stats(store)}


def cmd_remove(args):
    store = _store(args)
    return {"removed": store.remove(args["key"]), "stats": _stats(store)}


def cmd_attach(args):
    store = _store(args)
    arr = store.attach(args["key"])
    out = {"hit": False} if arr is None else _describe(arr)
    return {**out, "stats": _stats(store)}


def cmd_hold(args):
    store = _store(args)
    arr = store.attach(args["key"])
    first = {"hit": False} if arr is None else _describe(arr)
    _emit({**first, "stats": _stats(store)})
    sys.stdin.readline()
    if arr is None:
        return {"hit": False}
    return {"payload_sha256": _payload_sha(arr),
            "path_exists": os.path.exists(store.path_for(args["key"]))}


def _rel(root, paths):
    return [os.path.relpath(p, root) for p in paths]


def cmd_reap(args):
    store = _store(args)
    if args.get("die_in_walk"):
        def dying_walk(*_a, **_k):
            os._exit(17)                    # holding .reap.lock, as a crashed reaper
            yield                           # pragma: no cover
        cks.os.walk = dying_walk
    pause = args.get("sleep_before_unlink")
    if pause:
        real_unlink, paused = os.unlink, []

        def slow_unlink(path, *a, **kw):
            if str(path).endswith(".npy") and not paused:
                paused.append(path)
                time.sleep(pause)
            return real_unlink(path, *a, **kw)
        cks.os.unlink = slow_unlink
    out = store.reap(budget_s=args.get("budget_s", 10.0), dry_run=bool(args.get("dry_run")),
                     now=args.get("now"))
    return _report(store.root, out) | {"stats": _stats(store)}


def _report(root, out):
    keep = ("scanned", "reaped", "bytes", "complete", "dry_run", "low_water", "need_free")
    rep = {k: out[k] for k in keep if k in out}
    rep.setdefault("complete", False)
    rep.setdefault("dry_run", False)
    if out.get("dry_run"):
        rep["unlink"] = _rel(root, out["unlink"])
        rep["sweep"] = _rel(root, out["sweep"])
    return rep


def _policy(root) -> dict:
    policy = cksh.read_policy(root)

    def get(name):
        return cksh.setting(name, policy)

    return {"max_bytes": int(get("MAX_GB") * 2**30),
            "min_free_bytes": int(get("MIN_FREE_GB") * 2**30),
            "ttl_s": float(get("TTL_S")), "reap_interval_s": float(get("REAP_S")),
            "max_array_bytes": int(get("MAX_ARRAY_BYTES")), "fsync": bool(get("FSYNC")),
            "verify_republish": bool(get("VERIFY_REPUBLISH"))}


def cmd_plan(args):
    root = os.path.abspath(args["root"])
    p = _policy(root)
    out = cks.reap_plan(root, max_bytes=p["max_bytes"], min_free_bytes=p["min_free_bytes"],
                        ttl_s=p["ttl_s"], budget_s=args.get("budget_s", 60.0),
                        now=args.get("now"))
    return _report(root, out) | {"policy": p}


def cmd_policy(args):
    return _policy(os.path.abspath(args["root"]))


def cmd_usage(args):
    store = _store(args)
    used, files = store.usage()
    out = None if used < 0 else {"bytes": used, "files": files if files >= 0 else None}
    return {"usage": out, "stats": _stats(store)}


COMMANDS = {"publish": cmd_publish, "remove": cmd_remove, "attach": cmd_attach, "hold": cmd_hold,
            "reap": cmd_reap, "plan": cmd_plan, "usage": cmd_usage, "policy": cmd_policy}


def run(cmd, args):
    start_at = args.get("start_at")
    if start_at:
        time.sleep(max(0.0, start_at - time.time()))
    return COMMANDS[cmd](args)


def _emit(obj) -> None:
    sys.stdout.write(json.dumps(obj, sort_keys=True) + "\n")
    sys.stdout.flush()


def main(argv) -> int:
    if len(argv) < 2 or (argv[1] not in COMMANDS and argv[1] != "batch"):
        sys.stderr.write(__doc__)
        return 2
    raw = argv[2] if len(argv) > 2 else "{}"
    if raw == "-":
        raw = sys.stdin.read()
    elif raw.startswith("@"):
        with open(raw[1:], encoding="utf-8") as fh:
            raw = fh.read()
    args = json.loads(raw)
    if argv[1] == "batch":
        _emit([run(op["cmd"], op) for op in args])
    else:
        _emit(run(argv[1], args))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

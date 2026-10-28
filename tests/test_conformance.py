"""Cross-language conformance (plan step 8): the Python package and the Rust crate on one
tree, through their drivers (``conformance/ck_conform.py`` and ``rust/ck-conform``), which
speak the same JSON.

Every case runs in both directions. ``ck-conform`` is built with ``cargo build`` once per
session; without cargo the module skips (CI's ``conformance`` job always has it). The
drivers run with ``verify_republish`` on and every ``COMMONKIT_STORE_*`` variable removed,
so the developer's own policy (thresher's points at a live tree) never reaches a test.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import pytest

REPO = Path(__file__).resolve().parent.parent
VECTORS = REPO / "conformance" / "vectors"
PY_DRIVER = REPO / "conformance" / "ck_conform.py"
LANGS = ("py", "rs")
PAIRS = [("py", "rs"), ("rs", "py")]
D = [f"{i:02x}" * 16 for i in range(256)]          # digests: "0000...", "0101...", ...


def key(i, form="raw", variant="x0"):
    return f"ckconf:v1:{form}:{D[i]}:{variant}"


def relpath(i, form="raw", variant="x0"):
    return f"{form}/{D[i][:2]}/{D[i]}.{variant}.npy"


# ------------------------------------------------------------------------------ drivers


@pytest.fixture(scope="session")
def ck_bin():
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("cargo is not on PATH: the cross-language conformance suite needs it to "
                    "build rust/ck-conform (CI's conformance job runs it)")
    subprocess.run([cargo, "build", "--quiet", "-p", "ck-conform"], cwd=REPO / "rust",
                   check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "rust" / "target"))
    exe = target / "debug" / ("ck-conform.exe" if os.name == "nt" else "ck-conform")
    assert exe.exists(), exe
    return exe


def clean_env(extra=None):
    env = {k: v for k, v in os.environ.items() if not k.startswith("COMMONKIT_STORE_")}
    env.update(extra or {})
    return env


class Drivers:
    def __init__(self, ck_bin, tmp_path):
        self.bin = ck_bin
        self.tmp = tmp_path
        self.n = 0

    def argv(self, lang, cmd, args):
        text = json.dumps(args)
        if len(text) > 50_000:                  # a v2 header's 3700 fields, say
            self.n += 1
            path = self.tmp / f"args{self.n}.json"
            path.write_text(text)
            text = f"@{path}"
        if lang == "py":
            return [sys.executable, str(PY_DRIVER), cmd, text]
        return [str(self.bin), cmd, text]

    def run(self, lang, cmd, args, env=None, check=True):
        p = subprocess.run(self.argv(lang, cmd, args), capture_output=True, text=True,
                           env=clean_env(env), timeout=120, check=False)
        if check:
            assert p.returncode == 0, f"{lang} {cmd}: {p.returncode}\n{p.stderr}"
            return json.loads(p.stdout)
        return p

    def popen(self, lang, cmd, args, stdin=None):
        return subprocess.Popen(self.argv(lang, cmd, args), stdout=subprocess.PIPE,
                                stdin=stdin, stderr=subprocess.PIPE, text=True,
                                env=clean_env())

    def batch(self, lang, ops):
        return self.run(lang, "batch", ops)


@pytest.fixture
def ck(ck_bin, tmp_path):
    return Drivers(ck_bin, tmp_path)


def store(root, **kw):
    return {"root": str(root), **kw}


def arr(descr="|u1", shape=(1000,), fill="ramp", order="C"):
    return {"descr": descr, "shape": list(shape), "order": order, "fill": fill}


def tree_files(root):
    """Every regular file under ``root``, relative, with its size and mtime."""
    out = {}
    for dirpath, _dirs, files in os.walk(root):
        for name in files:
            p = os.path.join(dirpath, name)
            st = os.lstat(p)
            out[os.path.relpath(p, root)] = (st.st_size, st.st_mtime_ns)
    return out


def segments(root):
    return {p for p in tree_files(root) if p.endswith(".npy")}


def age(path, seconds_ago):
    t = time.time() - seconds_ago
    os.utime(path, (t, t))


def sha_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


# --------------------------------------------------------------- files, both ways (§5)

HEADERS = json.loads((VECTORS / "headers.json").read_text())["vectors"]


def test_both_languages_write_identical_files_and_read_each_others(ck, tmp_path):
    """Every header vector (simple, big-endian, structured, Fortran, growth padding, version
    2 and 3): each language publishes it, the two files are identical byte for byte (header
    and payload) and equal numpy's fixture, and each language attaches the other's file
    with the same dtype, shape, order and payload, mapped and copied."""
    roots = {lang: tmp_path / lang for lang in LANGS}
    for lang in LANGS:
        ops = [{"cmd": "publish", "store": store(roots[lang]), "key": key(i),
                "array": v["array"]} for i, v in enumerate(HEADERS)]
        for v, out in zip(HEADERS, ck.batch(lang, ops), strict=True):
            assert out.get("stored") is True, (lang, v["id"], out)
    for i, v in enumerate(HEADERS):
        py, rs = roots["py"] / relpath(i), roots["rs"] / relpath(i)
        assert py.read_bytes() == rs.read_bytes(), v["id"]
        assert sha_file(py) == v["sha256"], v["id"]

    seen = {}
    for reader, writer in PAIRS:
        for attach in ("mmap", "read"):
            ops = [{"cmd": "attach", "store": store(roots[writer], attach=attach),
                    "key": key(i)} for i in range(len(HEADERS))]
            for i, (v, out) in enumerate(zip(HEADERS, ck.batch(reader, ops), strict=True)):
                data = (VECTORS / v["file"]).read_bytes()
                want = hashlib.sha256(data[v["data_offset"]:]).hexdigest()
                assert out["hit"] and out["stats"] == {"shm_hit": 1}, (reader, v["id"])
                assert out["shape"] == v["shape"], (reader, v["id"])
                assert out["fortran_order"] == v["fortran_order"], (reader, v["id"])
                if v["descr"] is not None:          # None: too long to keep in the vector
                    assert out["descr"] == v["descr"], (reader, v["id"])
                padded = v["id"] == "struct_padded" and attach == "read" and reader == "py"
                if not padded:
                    # A copying np.load zeroes padding bytes (contract §5.8); a mapping or
                    # Rust's copy keeps them.
                    assert out["payload_sha256"] == want, (reader, attach, v["id"])
                seen[(reader, attach, i)] = out
    # Both languages describe every file alike, and map it or copy it alike.
    for attach in ("mmap", "read"):
        for i, v in enumerate(HEADERS):
            py, rs = seen[("py", attach, i)], seen[("rs", attach, i)]
            assert py["descr"] == rs["descr"], v["id"]
            assert py["mapped"] == rs["mapped"], (attach, v["id"], py["mapped"])


# ---------------------------------------------------------------------- touch (§8.2)


@pytest.mark.parametrize("toucher,reaper", PAIRS + [("py", "py"), ("rs", "rs")])
@pytest.mark.parametrize("how", ["attach", "republish"])
def test_a_touch_from_either_side_protects_a_segment_from_the_others_reaper(
        ck, tmp_path, toucher, reaper, how):
    root = tmp_path / "s"
    ck.batch("py", [{"cmd": "publish", "store": store(root), "key": key(i),
                     "array": arr()} for i in range(4)])
    for i in range(4):
        age(root / relpath(i), 1000 - i * 10)         # 0 oldest; all past the 60 s touch
    if how == "attach":
        out = ck.run(toucher, "attach", {"store": store(root), "key": key(0)})
        assert out["hit"]
    else:
        out = ck.run(toucher, "publish", {"store": store(root), "key": key(0), "array": arr()})
        assert out == {"stored": True, "stats": {}}   # a republish counts nothing (§6.4)
    assert time.time() - os.stat(root / relpath(0)).st_mtime < 30
    size = os.path.getsize(root / relpath(0))
    out = ck.run(reaper, "reap", {"store": store(root, max_bytes=int(2.5 * size / 0.8))})
    assert out["reaped"] == 2
    assert segments(root) == {relpath(0), relpath(3)}


# ------------------------------------------------------------------ the reaper (§9)


def build_tree(root):
    """Twelve segments of differing sizes and ages (two past a one-hour TTL), orphaned and
    fresh ``.tmp`` files, and bystanders the reaper must leave."""
    now = time.time()
    for i in range(12):
        p = root / relpath(i, "img" if i % 3 else "raw")
        p.parent.mkdir(parents=True, exist_ok=True)
        with open(p, "wb") as fh:
            np.save(fh, np.zeros(1000 + 100 * (i % 4), dtype=np.uint8))
        t = now - 4000 + i * 100 if i < 2 else now - 1000 + i * 10
        os.utime(p, (t, t))
    extras = {"raw/00/x.npy.1.2.tmp": 4000, "raw/00/young.tmp": 100, "orphan.tmp": 9000,
              ".pinned/ab/old.tmp": 9000, "notes.npy": 9000,
              ".pinned/ab/" + D[171] + ".x0.npy": 9000}
    for rel, ago in extras.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(b"x")
        age(p, ago)


REAP_STORE = {"max_bytes": 10_000, "ttl_s": 3600}


def test_both_reapers_reach_the_same_survivors(ck, tmp_path):
    """Each reaper on its own copy, then each after the other on one tree, then their dry
    runs: one survivor set, one swept set."""
    template = tmp_path / "template"
    build_tree(template)
    results = {}
    for lang in LANGS:
        root = tmp_path / lang
        shutil.copytree(template, root)
        before = set(tree_files(root))
        out = ck.run(lang, "reap", {"store": store(root, **REAP_STORE)})
        assert out["complete"] and out["scanned"] == 12
        results[lang] = (before - set(tree_files(root)), out["reaped"], out["bytes"])
    assert results["py"] == results["rs"]
    gone, reaped, _ = results["py"]
    assert reaped == 6 and "raw/00/x.npy.1.2.tmp" in gone and len(gone) == 7

    for first, second in PAIRS:
        root = tmp_path / f"{first}-{second}"
        shutil.copytree(template, root)
        before = set(tree_files(root))
        assert ck.run(first, "reap", {"store": store(root, **REAP_STORE)})["reaped"] == 6
        assert ck.run(second, "reap", {"store": store(root, **REAP_STORE)})["reaped"] == 0
        assert before - set(tree_files(root)) == gone

    now = time.time()
    plans = {}
    for lang in LANGS:
        root = tmp_path / f"plan-{lang}"
        shutil.copytree(template, root)
        p = ck.run(lang, "reap", {"store": store(root, **REAP_STORE), "dry_run": True,
                                  "now": now})
        plans[lang] = {k: p[k] for k in ("unlink", "sweep", "reaped", "bytes", "low_water",
                                         "need_free", "scanned", "complete")}
        assert set(p["unlink"]) | set(p["sweep"]) == gone
    assert plans["py"] == plans["rs"]


def test_concurrent_reapers_of_both_languages_reach_the_same_survivors(ck, tmp_path):
    template = tmp_path / "template"
    build_tree(template)
    alone = tmp_path / "alone"
    shutil.copytree(template, alone)
    ck.run("py", "reap", {"store": store(alone, **REAP_STORE)})
    want = segments(alone)
    for round_ in range(3):
        root = tmp_path / f"r{round_}"
        shutil.copytree(template, root)
        start = time.time() + 1.0
        procs = [ck.popen(lang, "reap", {"store": store(root, **REAP_STORE),
                                         "start_at": start})
                 for lang in LANGS * 3]
        outs = [json.loads(p.communicate(timeout=60)[0]) for p in procs]
        # The lock serialises them: whoever reaps reaps all six, the rest find the lock held
        # or find nothing left to do.
        assert sum(o["reaped"] for o in outs) == 6, outs
        assert segments(root) == want
        assert not (root / ".reap.lock").exists()


@pytest.mark.parametrize("slow,fast", PAIRS)
def test_racing_reapers_after_a_broken_lock_count_gone_segments(ck, tmp_path, slow, fast):
    """C8: a reaper whose lock was broken as stale races the next one. The slow reaper has
    walked and sorted; the fast one breaks the lock, reaps to the low water mark, and the
    slow one then finds its first candidates gone. Gone counts toward the target in both
    languages (§9.6), so the slow one stops at the target instead of evicting the rest."""
    root = tmp_path / "s"
    ck.batch("py", [{"cmd": "publish", "store": store(root), "key": key(i), "array": arr()}
                    for i in range(6)])
    for i in range(6):
        age(root / relpath(i), 600 - i * 60)
    size = os.path.getsize(root / relpath(0))
    settings = store(root, max_bytes=int(2.5 * size / 0.8))     # room for two
    p = ck.popen(slow, "reap", {"store": settings, "sleep_before_unlink": 3.0})
    lock = root / ".reap.lock"
    deadline = time.time() + 20
    while not lock.exists():
        assert time.time() < deadline and p.poll() is None, p.stderr.read() if p.stderr else ""
        time.sleep(0.01)
    time.sleep(0.5)                                  # walked and sorted; now in the pause
    age(lock, 400)                                   # as if its owner had died
    assert ck.run(fast, "reap", {"store": settings})["reaped"] == 0      # breaks it
    assert not lock.exists()
    assert ck.run(fast, "reap", {"store": settings})["reaped"] == 4
    out = json.loads(p.communicate(timeout=30)[0])
    assert out["reaped"] == 0 and out["complete"]
    assert segments(root) == {relpath(4), relpath(5)}


@pytest.mark.parametrize("dead,other", PAIRS)
def test_a_stale_lock_left_by_either_language_is_broken_by_the_other(
        ck, tmp_path, dead, other):
    root = tmp_path / "s"
    ck.batch("py", [{"cmd": "publish", "store": store(root), "key": key(i), "array": arr()}
                    for i in range(3)])
    for i in range(3):
        age(root / relpath(i), 7200)
    settings = store(root, ttl_s=3600)
    p = ck.run(dead, "reap", {"store": settings, "die_in_walk": True}, check=False)
    assert p.returncode == 17, p.stderr
    lock = root / ".reap.lock"
    assert lock.read_text().isdigit()               # the dead reaper's pid
    out = ck.run(other, "reap", {"store": settings})
    assert out["reaped"] == 0 and lock.exists()     # fresh: respected
    age(lock, 301)
    out = ck.run(other, "reap", {"store": settings})
    assert out["reaped"] == 0 and not lock.exists()  # stale: broken, and no reap this turn
    out = ck.run(other, "reap", {"store": settings})
    assert out["reaped"] == 3 and segments(root) == set()


# ----------------------------------------------------------- torn files and .tmp (§5.7)


def test_torn_and_partial_segments_are_handled_identically(ck, tmp_path):
    """A short payload, a header cut off, a bare prefix and an empty file: a torn miss in
    both languages, in both attach modes, and replaced by a publish in both."""
    whole = tmp_path / "whole"
    ck.run("py", "publish", {"store": store(whole), "key": key(0), "array": arr()})
    data = (whole / relpath(0)).read_bytes()
    damage = {"short": data[:-1], "half": data[: len(data) // 2], "header_cut": data[:60],
              "prefix": data[:10], "empty": b""}
    results = {}
    for lang in LANGS:
        for name, blob in damage.items():
            for attach in ("mmap", "read"):
                root = tmp_path / f"{lang}-{name}-{attach}"
                p = root / relpath(0)
                p.parent.mkdir(parents=True)
                p.write_bytes(blob)
                a = ck.run(lang, "attach", {"store": store(root, attach=attach), "key": key(0)})
                pub = ck.run(lang, "publish", {"store": store(root), "key": key(0),
                                               "array": arr()})
                results[(lang, name, attach)] = (a, pub)
                assert p.read_bytes() == data, (lang, name)
    for name in damage:
        for attach in ("mmap", "read"):
            a, pub = results[("py", name, attach)]
            assert (a, pub) == results[("rs", name, attach)], name
            assert a == {"hit": False, "stats": {"shm_torn": 1}}
            assert pub["stored"] and pub["stats"]["shm_torn_replaced"] == 1


def test_orphaned_tmp_files_are_swept_identically(ck, tmp_path):
    template = tmp_path / "template"
    ck.run("py", "publish", {"store": store(template), "key": key(0), "array": arr()})
    entries = {"raw/00/a.tmp": 3700, "raw/00/b.npy.123.456.tmp": 7200, "raw/00/young.tmp": 60,
               "raw/old.tmp": 7200, "old.tmp": 7200, ".x/00/old.tmp": 7200,
               "raw/00/sub/old.tmp": 7200}
    for rel, ago in entries.items():
        p = template / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(b"x")
        age(p, ago)
    gone = {}
    for lang in LANGS:
        root = tmp_path / lang
        shutil.copytree(template, root)
        before = set(tree_files(root))
        out = ck.run(lang, "reap", {"store": store(root)})
        assert out["reaped"] == 0 and out["stats"] == {}    # sweeping counts nothing
        gone[lang] = before - set(tree_files(root))
    assert gone["py"] == gone["rs"] == {"raw/00/a.tmp", "raw/00/b.npy.123.456.tmp"}


# ------------------------------------------------------------------- .usage.json (§9.7)


@pytest.mark.parametrize("writer,reader", PAIRS)
def test_usage_written_by_one_is_trusted_by_the_others_budget_gate(ck, tmp_path, writer, reader):
    root = tmp_path / "s"
    ck.batch(writer, [{"cmd": "publish", "store": store(root), "key": key(i), "array": arr()}
                      for i in range(3)])
    out = ck.run(writer, "reap", {"store": store(root)})
    assert out["complete"] and out["reaped"] == 0
    used = sum(os.path.getsize(root / relpath(i)) for i in range(3))
    want = {"bytes": used, "files": 3}
    assert ck.run(writer, "usage", {"store": store(root)})["usage"] == want
    assert ck.run(reader, "usage", {"store": store(root)})["usage"] == want
    over = ck.run(reader, "publish", {"store": store(root, max_bytes=used + 999),
                                      "key": key(9), "array": arr()})
    assert over == {"stored": False, "stats": {"shm_declined_budget": 1}}
    fits = ck.run(reader, "publish", {"store": store(root, max_bytes=used + 1000),
                                      "key": key(9), "array": arr()})
    assert fits["stored"] is True
    # Stale (older than twice the reap interval): unknown, so the gate passes.
    age(root / ".usage.json", 100)
    stale = ck.run(reader, "publish", {"store": store(root, max_bytes=used + 999,
                                                      reap_interval_s=30),
                                       "key": key(10), "array": arr()})
    assert stale["stored"] is True


# --------------------------------------------------------------- publishers racing (§6.4)


def test_publishers_racing_on_one_key_in_both_languages_all_succeed(ck, tmp_path):
    """W1: the same array under one key from eight processes, four per language, started
    together. Each writes and renames or finds a whole segment and touches it; none sees a
    collision, and the file is the one both languages write."""
    ref = tmp_path / "ref"
    ck.run("py", "publish", {"store": store(ref), "key": key(1), "array": arr(shape=(200_000,))})
    want = (ref / relpath(1)).read_bytes()
    for round_ in range(3):
        root = tmp_path / f"r{round_}"
        start = time.time() + 1.0
        procs = [ck.popen(lang, "publish", {"store": store(root), "key": key(1),
                                            "array": arr(shape=(200_000,)), "start_at": start})
                 for lang in LANGS * 4]
        outs = [json.loads(p.communicate(timeout=60)[0]) for p in procs]
        assert all(o.get("stored") is True for o in outs), outs
        assert not any(o.get("collision") for o in outs)
        assert (root / relpath(1)).read_bytes() == want
        assert [p for p in tree_files(root) if p.endswith(".tmp")] == []


@pytest.mark.parametrize("first,second", PAIRS)
@pytest.mark.parametrize("new", [arr(fill="ramp2"), arr(shape=(500, 2)), arr(order="F",
                                                                             shape=(10, 100)),
                                 arr(descr="<i2", shape=(500,))])
def test_a_real_collision_is_raised_in_either_language(ck, tmp_path, first, second, new):
    root = tmp_path / "s"
    base = arr(shape=(10, 100)) if new["order"] == "F" else arr()
    assert ck.run(first, "publish", {"store": store(root), "key": key(2),
                                     "array": base})["stored"]
    path = root / relpath(2)
    age(path, 3600)
    before = (path.read_bytes(), os.stat(path).st_mtime_ns)
    out = ck.run(second, "publish", {"store": store(root), "key": key(2), "array": new})
    assert out == {"collision": True, "stats": {"shm_key_collision": 1}}
    assert (path.read_bytes(), os.stat(path).st_mtime_ns) == before


# ----------------------------------------------------------- remove, then replace (§6.7)


@pytest.mark.parametrize("remover,publisher", PAIRS)
def test_one_language_removes_and_the_other_replaces(ck, tmp_path, remover, publisher):
    """A value replaced under an unchanged key: removed in one language, published
    again (a different array) in the other, then read by both."""
    root = tmp_path / "s"
    ck.run(publisher, "publish", {"store": store(root), "key": key(4), "array": arr()})
    new = arr(shape=(10, 100))
    assert ck.run(publisher, "publish", {"store": store(root), "key": key(4),
                                         "array": new})["collision"]
    out = ck.run(remover, "remove", {"store": store(root), "key": key(4)})
    assert out == {"removed": True, "stats": {"shm_removed": 1}}
    assert ck.run(remover, "remove", {"store": store(root), "key": key(4)}) == \
        {"removed": False, "stats": {}}
    assert ck.run(publisher, "publish", {"store": store(root), "key": key(4),
                                         "array": new})["stored"]
    for lang in ("py", "rs"):
        assert ck.run(lang, "attach", {"store": store(root), "key": key(4)})["shape"] == [10, 100]


# ------------------------------------------------------- a mapping outlives its file (§7)


@pytest.mark.parametrize("holder,reaper", PAIRS)
def test_a_mapping_survives_the_other_languages_reaper(ck, tmp_path, holder, reaper):
    root = tmp_path / "s"
    ck.run("py", "publish", {"store": store(root), "key": key(3), "array": arr(shape=(1 << 20,))})
    p = ck.popen(holder, "hold", {"store": store(root), "key": key(3)}, stdin=subprocess.PIPE)
    first = json.loads(p.stdout.readline())
    assert first["hit"] and first["mapped"]
    age(root / relpath(3), 7200)                    # after the hit's touch: now expired
    out = ck.run(reaper, "reap", {"store": store(root, ttl_s=3600)})
    assert out["reaped"] == 1 and not (root / relpath(3)).exists()
    after = json.loads(p.communicate("go\n", timeout=30)[0])
    assert after == {"payload_sha256": first["payload_sha256"], "path_exists": False}


# --------------------------------------------------------------- .policy.json (§10.5)

POLICY = json.loads((VECTORS / "policy.json").read_text())["vectors"]
EXTRA_POLICIES = [
    {"id": "every_setting", "policy_file": {"text": json.dumps(
        {"MAX_GB": 7, "MIN_FREE_GB": "1", "TTL_S": 604800, "REAP_S": "12.5",
         "MAX_ARRAY_BYTES": 1e6, "FSYNC": "Yes", "VERIFY_REPUBLISH": True})}, "env": {}},
    {"id": "file_and_env", "policy_file": {"text": '{"MAX_GB": 0.5, "FSYNC": "nope"}'},
     "env": {"COMMONKIT_STORE_FSYNC": "on", "COMMONKIT_STORE_TTL_S": " 60 ",
             "COMMONKIT_STORE_MAX_GB": "3"}},
    {"id": "thresher", "policy_file": None,
     "env": {"COMMONKIT_STORE_MAX_GB": "7", "COMMONKIT_STORE_MIN_FREE_GB": "1",
             "COMMONKIT_STORE_TTL_S": "604800"}},
]


@pytest.mark.parametrize("v", POLICY + EXTRA_POLICIES, ids=lambda v: v["id"])
def test_policy_is_read_the_same_by_both(ck, tmp_path, v):
    root = tmp_path / "s"
    root.mkdir()
    pf = v["policy_file"]
    target = root / ".policy.json"
    if pf is None:
        pass
    elif "text" in pf:
        target.write_text(pf["text"], encoding="utf-8")
    elif "file" in pf:
        shutil.copyfile(VECTORS / pf["file"], target)
    else:
        target.mkdir()
    py = ck.run("py", "policy", {"root": str(root)}, env=v["env"])
    rs = ck.run("rs", "policy", {"root": str(root)}, env=v["env"])
    assert py == rs
    for k, want in v.get("expect", {}).items():
        if k in py and want != "default":
            assert py[k] == want, k

"""prune_folder against the shared vectors (conformance/vectors/prune.json), and its guards.
Every tree is a pytest tmp_path: nothing here touches a real folder."""

import json
import os
import time
from pathlib import Path

from commonkit.prune import prune_folder

VECTORS = Path(__file__).resolve().parent.parent / "conformance" / "vectors" / "prune.json"


def build(root: Path, entries, now: float) -> set:
    """The vector's folder; the names the owner check disclaims."""
    disclaimed = set()
    for e in entries:
        p = root / e["name"]
        if e["kind"] == "dir":
            p.mkdir()
        elif e["kind"] == "symlink":
            target = root.parent / ("target-" + e["name"])
            target.write_bytes(b"x")
            os.symlink(target, p)
        else:
            p.write_bytes(b"x")
        if e["kind"] != "symlink":
            os.utime(p, (now - e["age_s"], now - e["age_s"]))
        if e.get("owned") is False:
            disclaimed.add(e["name"])
    return disclaimed


def test_the_vectors_hold(tmp_path):
    doc = json.loads(VECTORS.read_text())
    opts = doc["prune"]
    assert len(doc["vectors"]) >= 10
    for v in doc["vectors"]:
        now = time.time()
        root = tmp_path / v["id"] / "edits"
        root.mkdir(parents=True)
        disclaimed = build(root, v["entries"], now)
        before = sorted(os.listdir(root))
        common = {"pattern": opts["pattern"], "keep": v["keep"], "min_age_s": opts["min_age_s"],
                  "companions": opts["companions"],
                  "owns": lambda p, d=frozenset(disclaimed): p.name not in d, "now": now}
        dry = prune_folder(root, dry_run=True, **common)
        assert sorted(os.listdir(root)) == before, v["id"]
        got = prune_folder(root, dry_run=False, **common)
        want = v["expect"]
        assert {k: got[k] for k in want} == want, (v["id"], got)
        assert dry["removed"] == got["removed"], v["id"]
        assert sorted(os.listdir(root)) == sorted(set(before) - set(want["removed"])), v["id"]
        assert got["failed"] == 0 and got["refused"] is None


def test_it_refuses_what_isnt_a_real_folder(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    link = tmp_path / "link"
    os.symlink(real, link)
    common = {"pattern": r".*", "keep": (), "min_age_s": 0, "dry_run": False}
    assert prune_folder(link, **common)["refused"]
    assert prune_folder(tmp_path / "missing", **common)["refused"]
    (tmp_path / "file").write_bytes(b"x")
    assert prune_folder(tmp_path / "file", **common)["refused"]
    assert prune_folder(real, pattern="(", keep=(), min_age_s=0)["refused"]


def test_a_failing_owner_check_claims_nothing(tmp_path):
    (tmp_path / "a-0000000000000000.png").write_bytes(b"x")

    def boom(_p):
        raise RuntimeError("no")

    r = prune_folder(tmp_path, pattern=r"a-[0-9a-f]{16}\.png", keep=(), min_age_s=0, owns=boom,
                     dry_run=False, now=time.time() + 10)
    assert (r["removed"], r["foreign"]) == ([], 1)
    assert (tmp_path / "a-0000000000000000.png").exists()

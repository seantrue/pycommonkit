"""The hashing facade (contract §3.8): ``commonkit.digest`` under each algorithm, ``fixed``,
the build salts, and the vectors in ``conformance/vectors/digests.json``.

One process per algorithm (D17): each algorithm's checks run in a child whose
``COMMONKIT_HASH`` names it, so ``current()`` is what that child resolved once.
"""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

VECTORS = Path(__file__).resolve().parent.parent / "conformance" / "vectors" / "digests.json"

CHECK_VECTORS = """
    import json, sys
    import numpy as np
    from commonkit import digest
    doc = json.load(open(sys.argv[1] if len(sys.argv) > 1 else {path!r}))
    algo = digest.current()
    assert algo == {algo!r}, algo

    def data(v):
        if "bytes_hex" in v:
            return bytes.fromhex(v["bytes_hex"])
        n = v["pattern"]["len"]
        return bytes((i * 31) % 251 for i in range(n))

    for v in doc["raw"]:
        assert digest.raw(data(v)) == v[algo], v["id"]
        h = digest.Hasher()
        b = data(v)
        h.update(b[:7]).update(b[7:])
        assert h.hexdigest() == v[algo], v["id"]
    for v in doc["nd1"]:
        a = np.frombuffer(data(v), dtype=np.dtype(v["dtype"])).reshape(v["shape"])
        assert digest.nd1(a) == v[algo], v["id"]
        assert digest.fixed.nd1("md5", a) == v["md5"], v["id"]
    for v in doc["raw"]:
        assert digest.fixed.raw("md5", data(v)) == v["md5"], v["id"]
        assert digest.fixed.raw("sha256", data(v)) == v["sha256"], v["id"]
    print("checked", algo)
"""


@pytest.mark.parametrize("algo", ["md5", "b3"])
def test_every_vector_in_each_algorithm(child, algo):
    p, _root, _marks = child(CHECK_VECTORS.format(path=str(VECTORS), algo=algo), algo=algo)
    assert f"checked {algo}" in p.stdout
    assert f"commonkit: hash {algo} (environment COMMONKIT_HASH)" in p.stderr


def test_the_vectors_cover_both_algorithms_and_the_big_endian_cases():
    doc = json.loads(VECTORS.read_text())
    assert len(doc["raw"]) >= 5 and len(doc["nd1"]) >= 8
    for v in doc["raw"] + doc["nd1"]:
        assert len(v["md5"]) == 32 and len(v["b3"]) == 64 and len(v["sha256"]) == 64
    assert any(v["dtype"].startswith(">") for v in doc["nd1"])
    assert any(v.get("pattern", {}).get("len", 0) > 1 << 20 for v in doc["raw"])
    # a big-endian array names the same content as its little-endian twin
    by_id = {v["id"]: v for v in doc["nd1"]}
    assert by_id["f4_be"]["b3"] == by_id["f4_le"]["b3"]


def test_fixed_and_the_rejected_dtypes():
    from commonkit import digest
    assert digest.fixed.raw("md5", b"") == "d41d8cd98f00b204e9800998ecf8427e"
    assert digest.fixed.raw("sha256", b"abc").startswith("ba7816bf")
    h = digest.fixed.Hasher("sha256")
    h.update(b"a").update(b"bc")
    assert h.hexdigest() == digest.fixed.raw("sha256", b"abc")
    for bad in ("sha1", "b3", "MD5x"):
        with pytest.raises(ValueError):
            digest.fixed.raw(bad, b"")
        with pytest.raises(ValueError):
            digest.fixed.Hasher(bad)
    # a named algorithm only through fixed, where every use is allowlisted (finding 9)
    for name in ("md5", "sha256", "b3"):
        with pytest.raises(TypeError):
            digest.Hasher(name)
    with pytest.raises(ValueError):
        digest.nd1(np.array([None], dtype=object))
    with pytest.raises(ValueError):
        digest.nd1(np.zeros(2, dtype=[("a", "<i4")]))


def test_file_streams_and_counts(child, tmp_path):
    f = tmp_path / "blob.bin"
    f.write_bytes(bytes(range(256)) * 9000)
    p, _r, _m = child(f"""
        from commonkit import digest
        d, n = digest.file({str(f)!r})
        assert n == 256 * 9000, n
        assert d == digest.raw(open({str(f)!r}, "rb").read())
        try:
            digest.file({str(f)!r}, limit=100)
        except OSError:
            print("refused")
    """, algo="md5")
    assert "refused" in p.stdout


def test_build_salts_name_both_and_pick_the_current_one(child):
    p, _r, _m = child("""
        from commonkit import digest
        s = digest.build.both(b"salt")
        assert s.md5 == digest.fixed.raw("md5", b"salt")
        assert len(s.b3) == 64
        assert digest.build.pick(s) == s.b3 == digest.raw(b"salt")
        print("ok")
    """, algo="b3")
    assert "ok" in p.stdout


def test_blake3_is_imported_lazily(child):
    p, _r, _m = child("""
        import sys
        from commonkit import digest
        assert "blake3" not in sys.modules
        digest.fixed.raw("md5", b"x")
        assert "blake3" not in sys.modules
        digest.raw(b"x")
        assert "blake3" in sys.modules
        print("ok")
    """, algo="b3")
    assert "ok" in p.stdout

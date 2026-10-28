"""The shared conformance vectors (``conformance/vectors/``), judged by Python.

The vectors state the CONTRACT's outcome (``docs/contract.md``). Python is the reference
implementation. Where it does not yet conform, a vector carries ``"python": "step3"`` and is
``xfail(strict=True)`` here: it must fail until Python changes, at which point the marker
comes off in ``conformance/generate.py``. Since step 3 of the Rust-crate plan (pycommonkit
0.1.0, contract Appendix A) no vector carries it; the mechanism stays for the next change.

Python's side of the names the vectors use:

* ``SharedArrayStore(..., verify_republish=True)`` and the attributes ``store.fsync`` and
  ``store.verify_republish``, set by ``shared_store`` from ``.policy.json`` and the
  environment (contract 10);
* ``commonkit.KeyCollision``, a ``ValueError`` subclass raised by ``publish`` (contract 6.4).

Domains here are ``cv*`` so they never collide with another module's registered scheme.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import sys
import time
from pathlib import Path

import numpy as np
import pytest
from numpy.lib import format as npf

from commonkit import keys as ck_keys
from commonkit import shared as ck_shared
from commonkit.keys import KeyScheme
from commonkit.store import SharedArrayStore

ROOT = Path(__file__).resolve().parent.parent
VEC = ROOT / "conformance" / "vectors"
ENV_PREFIX = "COMMONKIT_STORE_"
STEP3 = pytest.mark.xfail(strict=True, reason="step 3")


def _load_generator():
    spec = importlib.util.spec_from_file_location("ck_conformance_generate",
                                                  ROOT / "conformance" / "generate.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


GEN = _load_generator()


def load(name: str) -> dict:
    return json.loads((VEC / name).read_text())


def params(vectors: list[dict]) -> list:
    return [pytest.param(v, id=v["id"], marks=[STEP3] if v.get("python") == "step3" else [])
            for v in vectors]


def scheme_from(d: dict, domain: str | None = None) -> KeyScheme:
    variants = d.get("variant_pattern") or tuple(d["variants"])
    return KeyScheme(domain or d["domain"], d["version"], tuple(d["forms"]), variants,
                     dtypes=d.get("dtypes"))


def registered(d: dict) -> KeyScheme:
    return ck_keys.register(scheme_from(d))


def nonzero(stats) -> dict:
    return {k: v for k, v in stats.items() if v}


def raw_bytes(arr) -> bytes:
    """The array's memory, padding bytes included (``tobytes`` zeroes a struct's gaps)."""
    return arr.reshape(-1, order="A").view(np.uint8).tobytes()


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


# ------------------------------------------------------------------------------ keys

KEYS = load("keys.json")
SCHEMES = {name: scheme_from(d) for name, d in KEYS["schemes"].items()}


@pytest.mark.parametrize("v", params(KEYS["build"]))
def test_key_build(v):
    scheme = SCHEMES[v["scheme"]]
    if "key" in v["expect"]:
        assert scheme.key(v["digest"], v["variant"], v["form"]) == v["expect"]["key"]
    else:
        with pytest.raises(ValueError):
            scheme.key(v["digest"], v["variant"], v["form"])


@pytest.mark.parametrize("v", [pytest.param(v, id=v["id"]) for v in KEYS["parse"]])
def test_key_grammar(v):
    """The reader's grammar (C4) as the contract writes it. A self-check of the vectors."""
    assert bool(re.search(KEYS["grammar"], v["key"])) is v["expect"]["grammar"]
    if v["expect"]["scheme_ok"]:
        assert v["expect"]["grammar"], "a key a scheme accepts is always grammatical"


@pytest.mark.parametrize("v", params(KEYS["parse"]))
def test_key_parse_by_scheme(v):
    scheme = SCHEMES[v["scheme"]]
    if v["expect"]["scheme_ok"]:
        assert list(scheme.parse(v["key"])) == v["expect"]["parts"]
        assert scheme.relpath(v["key"]) == v["expect"]["relpath"]
    else:
        with pytest.raises(ValueError):
            scheme.parse(v["key"])


@pytest.mark.parametrize("v", params(KEYS["registry"]))
def test_registry(v):
    def make(d):
        d = {k: val for k, val in d.items() if val is not None}
        return scheme_from({**d, "domain": v["domain"]})

    first = ck_keys.register(make(v["first"]))
    if v["expect"] == "idempotent":
        assert ck_keys.register(make(v["second"])) is first
    else:
        with pytest.raises(ValueError):
            ck_keys.register(make(v["second"]))


# --------------------------------------------------------------------------- headers

HEADERS = load("headers.json")


@pytest.mark.parametrize("v", params(HEADERS["vectors"]))
def test_header_fixture_is_what_numpy_writes(v):
    data = (VEC / v["file"]).read_bytes()
    assert sha(data) == v["sha256"]
    assert len(data) == v["file_bytes"]
    assert GEN.npy_bytes(GEN.make_array(v["array"])) == data
    info = GEN.header_info(data)
    for field in ("version", "header_len", "data_offset", "growth_padding", "align_padding",
                  "fortran_order", "shape"):
        assert info[field] == v[field], field
    if v["header_text"] is None:
        assert sha(info["header_text"].encode("utf8")) == v["header_text_sha256"]
    else:
        assert info["header_text"] == v["header_text"]
        assert info["descr"] == v["descr"]
    assert v["data_offset"] % 64 == 0
    assert 1 <= v["align_padding"] <= 64


@pytest.mark.parametrize("v", params(HEADERS["vectors"]))
def test_header_fixture_reads_back(v):
    data = (VEC / v["file"]).read_bytes()
    # mmap, because a copying np.load zeroes the gap bytes of a padded struct.
    arr = np.load(VEC / v["file"], mmap_mode="r", allow_pickle=False, max_header_size=2 ** 20)
    assert list(arr.shape) == v["shape"]
    assert bool(arr.flags.f_contiguous and not arr.flags.c_contiguous) is v["fortran_order"] \
        or arr.ndim < 2
    assert arr.dtype.itemsize == v["itemsize"]
    if v["descr"] is not None:
        assert repr(npf.dtype_to_descr(arr.dtype)) == v["descr"]
    assert arr.nbytes == v["payload_bytes"]
    assert raw_bytes(arr) == data[v["data_offset"]:]


# ----------------------------------------------------------------------------- reads

READS = load("reads.json")


@pytest.fixture(scope="module")
def read_scheme():
    return registered(READS["scheme"])


@pytest.mark.parametrize("attach", ["mmap", "read"])
@pytest.mark.parametrize("v", params(READS["vectors"]))
def test_read(v, attach, tmp_path, read_scheme):
    store = SharedArrayStore(tmp_path, mode="read", attach=attach)
    path = Path(store.path_for(v["key"]))
    if v["file"] is not None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes((VEC / v["file"]).read_bytes())
    arr = store.attach(v["key"])
    exp = v["expect"]
    assert nonzero(store.drain_stats()) == exp["counters"]
    if exp["outcome"] != "hit":
        assert arr is None
        return
    assert arr is not None
    assert list(arr.shape) == exp["shape"]
    assert bool(arr.flags.f_contiguous and not arr.flags.c_contiguous) is exp["fortran_order"]
    if exp["descr"] is not None:
        assert repr(npf.dtype_to_descr(arr.dtype)) == exp["descr"]
    assert sha(raw_bytes(arr)) == exp["payload_sha256"]


# ------------------------------------------------------------------------- admission

ADM = load("admission.json")


@pytest.fixture(scope="module")
def adm_scheme():
    return registered(ADM["scheme"])


@pytest.mark.parametrize("v", params(ADM["vectors"]))
def test_admission(v, tmp_path, adm_scheme):
    s = v["store"]
    store = SharedArrayStore(tmp_path, mode=s["mode"], min_array_bytes=s["min_array_bytes"],
                             max_array_bytes=s["max_array_bytes"],
                             min_free_bytes=s["min_free_bytes"], max_bytes=s["max_bytes"],
                             reap_interval_s=1e12)
    if "usage" in v:
        (tmp_path / ".usage.json").write_text(json.dumps(v["usage"]))
    ok = store.publish(v["key"], GEN.make_array(v["array"]))
    exp = v["expect"]
    assert ok is (exp["outcome"] == "published")
    assert nonzero(store.drain_stats()) == exp["counters"]
    assert not list(tmp_path.rglob("*.tmp")), "a declined publish leaves no temporary file"


# ---------------------------------------------------------------------- existing key

EXISTING = load("existing_key.json")


@pytest.fixture(scope="module")
def ex_scheme():
    return registered(EXISTING["scheme"])


@pytest.mark.parametrize("v", params(EXISTING["vectors"]))
def test_existing_key(v, tmp_path, ex_scheme):
    kwargs = {"verify_republish": True} if v["verify_republish"] else {}
    store = SharedArrayStore(tmp_path, mode="readwrite", min_array_bytes=0,
                             min_free_bytes=0, reap_interval_s=1e12, **kwargs)
    path = Path(store.path_for(v["key"]))
    old = None
    if v["existing"] is not None:
        old = (VEC / v["existing"]["file"]).read_bytes()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(old)
        then = time.time() - v["existing"]["mtime_age_s"]
        os.utime(path, (then, then))
    start = time.time()
    try:
        outcome = "published" if store.publish(v["key"], GEN.make_array(v["array"])) else "no"
    except ValueError as exc:
        if type(exc).__name__ != "KeyCollision":
            raise
        outcome = "KeyCollision"
    exp = v["expect"]
    after = path.read_bytes()
    if exp["outcome"] == "touched":
        assert outcome == "published", "a republish reports success"
    else:
        assert outcome == exp["outcome"]
    assert nonzero(store.drain_stats()) == exp["counters"]
    if exp["file_after"] == "unchanged":
        assert after == old
    else:
        assert sha(after) == exp["file_sha256"]
    mtime = path.stat().st_mtime
    if exp["mtime"] == "unchanged":
        assert abs(mtime - then) < 1.0
    else:
        assert mtime >= start - 2.0


# ---------------------------------------------------------------------- reaper paths

REAPER = load("reaper_paths.json")


@pytest.mark.parametrize("v", params(REAPER["vectors"]))
def test_reaper_paths(v, tmp_path):
    root = tmp_path / "store"
    target = root / v["path"]
    target.parent.mkdir(parents=True, exist_ok=True)
    then = time.time() - v["age_s"]
    if v["kind"] == "symlink":
        outside = tmp_path / "outside.npy"
        outside.write_bytes(bytes(64))
        os.utime(outside, (then, then))
        target.symlink_to(outside)
        os.utime(target, (then, then), follow_symlinks=False)
    else:
        target.write_bytes(bytes(64))
        os.utime(target, (then, then))
    r = REAPER["reap"]
    store = SharedArrayStore(root, mode="readwrite", ttl_s=r["ttl_s"], max_bytes=r["max_bytes"],
                             min_free_bytes=r["min_free_bytes"], reap_interval_s=0)
    out = store.reap(budget_s=10.0)
    assert out.get("complete") is True
    stats = nonzero(store.drain_stats())
    if v["expect"] == "reaped":
        assert not target.exists()
        assert stats == {"shm_reaped": 1, "shm_reaped_bytes": 64}
    elif v["expect"] == "swept":
        assert not os.path.lexists(target)
        assert stats == {}, "a swept temporary file is not a reaped segment"
    else:
        assert os.path.lexists(target)
        assert stats == {}
    if v["kind"] == "symlink":
        assert (tmp_path / "outside.npy").exists()


# --------------------------------------------------------------------- env and policy

ENV = load("env.json")
POLICY = load("policy.json")
PLATFORM = {"darwin": "darwin", "linux": "linux", "win32": "windows"}.get(sys.platform)


@pytest.fixture
def clean_env(monkeypatch):
    for name in list(os.environ):
        if name.startswith(ENV_PREFIX):
            monkeypatch.delenv(name)
    return monkeypatch


def _placeholders(text: str, root: str) -> str:
    return (text.replace("{home}", os.path.expanduser("~")).replace("{cwd}", os.getcwd())
            .replace("{root}", root))


def _check_fields(store, expect: dict, defaults: dict, root: str) -> None:
    for field, want in expect.items():
        if field == "root":
            if isinstance(want, dict):
                want = want.get(PLATFORM)
                if want is None:
                    pytest.skip(f"default root unspecified on {PLATFORM}")
            assert os.path.abspath(store.root if store else ck_shared.setting("ROOT")) == \
                _placeholders(want, root)
            continue
        if want == "default":
            want = defaults[field]
        assert getattr(store, field) == want, field


@pytest.mark.parametrize("v", params(ENV["vectors"]))
def test_env(v, tmp_path, clean_env):
    for name, value in v["env"].items():
        clean_env.setenv(name, value)
    defaults = ENV["defaults"].get(PLATFORM, ENV["defaults"]["linux"])
    expect = dict(v["expect"])
    root_want = expect.pop("root", None)
    if expect:
        store = ck_shared.shared_store(root=str(tmp_path), mode="read")
        _check_fields(store, expect, defaults, str(tmp_path))
    if root_want is not None:
        _check_fields(None, {"root": root_want}, defaults, str(tmp_path))


@pytest.mark.parametrize("v", params(POLICY["vectors"]))
def test_policy_file(v, tmp_path, clean_env):
    for name, value in v["env"].items():
        clean_env.setenv(name, value)
    pf = v["policy_file"]
    target = tmp_path / ".policy.json"
    if pf is not None:
        if "text" in pf:
            target.write_text(pf["text"])
        elif "file" in pf:
            target.write_bytes((VEC / pf["file"]).read_bytes())
        else:
            target.mkdir()
    store = ck_shared.shared_store(root=str(tmp_path), mode="read")
    defaults = ENV["defaults"].get(PLATFORM, ENV["defaults"]["linux"])
    _check_fields(store, v["expect"], defaults, os.path.abspath(str(tmp_path)))


def test_vectors_are_reproducible_and_small():
    """The fixtures are what the generator writes: run it and diff (see the module doc).
    Here only the size rule is checked, so the suite stays read-only."""
    total = sum(p.stat().st_size for p in VEC.rglob("*") if p.is_file())
    assert total < 512 * 1024


@pytest.mark.parametrize("v", params(HEADERS["vectors"]))
def test_publish_computes_the_header_length_numpy_writes(v):
    """The publisher's header-limit gate (contract 5.5) predicts HLEN without writing."""
    from commonkit.store import _header_len
    assert _header_len(GEN.make_array(v["array"])) == v["header_len"]

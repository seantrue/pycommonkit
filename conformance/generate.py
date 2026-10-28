"""Regenerate the shared conformance vectors in ``conformance/vectors/``.

    uv run python conformance/generate.py
    git diff --exit-code conformance/

The second command must be clean: every fixture is a pure function of this script and of
numpy's ``.npy`` writer, with no timestamps, no randomness and no dict-order dependence.
The vectors are the arbiter of ``docs/contract.md`` (contract section 13 describes their
format). Python is the reference implementation, but a vector states the CONTRACT's
outcome; where today's Python disagrees the vector carries ``"python": "step3"`` and
``tests/test_vectors.py`` expects it to fail until step 3 of the Rust-crate plan.

Only numpy and the standard library are used, so this runs in the package's own venv.
"""
from __future__ import annotations

import ast
import hashlib
import json
import shutil
import struct
import sys
import warnings
from pathlib import Path

import numpy as np
from numpy.lib import format as npf

HERE = Path(__file__).resolve().parent
OUT = HERE / "vectors"
STEP3 = "step3"

#: Digests used throughout. Lower-case md5-shaped hex; what produced them is irrelevant.
D1 = "ab" * 16
D2 = "0123456789abcdef" * 2
D3 = "fedcba9876543210" * 2

# --------------------------------------------------------------------------- arrays


def _dtype(descr_text: str) -> np.dtype:
    """The dtype an array spec's ``descr`` names, exactly as numpy's reader builds it.

    A simple descriptor is written bare (``<f4``); a structured one as the Python literal
    numpy puts in the header (``[('a', '<i4')]``).
    """
    if descr_text.startswith("["):
        return npf.descr_to_dtype(ast.literal_eval(descr_text))
    return npf.descr_to_dtype(descr_text)


def _fill(dtype: np.dtype, n_items: int, fill: str) -> bytes:
    """Deterministic payload bytes for ``n_items`` elements of ``dtype``.

    ``ramp`` and ``ramp2`` differ in every byte, so they make "same header, different bytes".
    Bool, string and datetime kinds get values that are valid for the kind, so a fixture
    is something numpy would plausibly hold rather than bit noise.
    """
    nbytes = n_items * dtype.itemsize
    seed = {"ramp": 11, "ramp2": 101, "zeros": None}[fill]
    if seed is None or nbytes == 0:
        return bytes(nbytes)
    kind, order = dtype.kind, ("<" if dtype.byteorder in "<|=" else ">")
    if kind == "b":
        return bytes(i & 1 for i in range(nbytes))
    if kind == "S":
        return bytes(97 + (i + seed) % 26 for i in range(nbytes))
    if kind == "U":
        n = nbytes // 4
        return np.array([97 + (i + seed) % 26 for i in range(n)], dtype=order + "u4").tobytes()
    if kind in "Mm":
        return (np.arange(n_items, dtype=order + "i8") * 1000 + seed).tobytes()
    return bytes((i * 37 + seed) & 0xFF for i in range(nbytes))


def spec_dtype(spec: dict) -> np.dtype:
    """An array spec's dtype. ``fields`` abbreviates a structured dtype too long to inline:
    ``{"count": n, "digits": d, "descr": x}`` is fields ``f0..0`` to ``f{n-1}``, each ``x``.
    """
    if "fields" in spec:
        f = spec["fields"]
        return np.dtype([(f"f{i:0{f['digits']}d}", f["descr"]) for i in range(f["count"])])
    return _dtype(spec["descr"])


def make_array(spec: dict) -> np.ndarray:
    """Build the array a vector's ``array`` spec describes.

    ``{"descr": <header descr text>, "shape": [...], "order": "C"|"F", "fill": ...}``.
    The payload in the file equals ``_fill(...)`` byte for byte: for ``order: F`` the
    bytes are laid out in Fortran order, which is what numpy then writes.
    """
    shape = tuple(spec["shape"])
    if spec["fill"] == "objects":
        arr = np.empty(shape, dtype=object)
        arr.fill(None)
        return arr
    dtype = spec_dtype(spec)
    n_items = 1
    for dim in shape:
        n_items *= dim
    payload = _fill(dtype, n_items, spec["fill"])
    flat = np.frombuffer(payload, dtype=dtype, count=n_items)
    return flat.reshape(shape, order=spec.get("order", "C"))


def npy_bytes(arr: np.ndarray) -> bytes:
    """``np.save(fh, arr, allow_pickle=False)`` as bytes; numpy's version warnings muted."""
    import io

    buf = io.BytesIO()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        np.save(buf, arr, allow_pickle=False)
    return buf.getvalue()


def header_info(data: bytes) -> dict:
    """Parse a numpy-written ``.npy`` prefix into the fields the vectors pin."""
    major = data[6]
    if major == 1:
        (hlen,) = struct.unpack("<H", data[8:10])
        start, enc = 10, "latin1"
    else:
        (hlen,) = struct.unpack("<I", data[8:12])
        start, enc = 12, ("latin1" if major == 2 else "utf8")
    text = data[start:start + hlen].decode(enc)
    body = text.rstrip(" \n")
    d = ast.literal_eval(body)
    shape, fortran = tuple(d["shape"]), d["fortran_order"]
    growth = (npf.GROWTH_AXIS_MAX_DIGITS - len(repr(shape[-1 if fortran else 0]))
              if shape else 0)
    spaces = len(text) - len(body) - 1
    return {
        "version": [major, data[7]],
        "header_len": hlen,
        "data_offset": start + hlen,
        "header_text": body,
        "growth_padding": max(growth, 0),
        "align_padding": spaces - max(growth, 0),
        "descr": repr(d["descr"]),
        "fortran_order": d["fortran_order"],
        "shape": list(d["shape"]),
    }


def unpadded_offset(info: dict, prefix_len: int) -> int:
    """Data offset a writer that skips numpy's growth-axis padding would produce."""
    # header_text is already stripped of every trailing space, growth padding included.
    enc = "utf8" if info["version"][0] == 3 else "latin1"
    hlen = len(info["header_text"].encode(enc)) + 1
    pad = npf.ARRAY_ALIGN - ((prefix_len + hlen) % npf.ARRAY_ALIGN)
    return prefix_len + hlen + pad


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def raw_npy(header_text: str, payload: bytes = b"", *, version=(1, 0), align=64,
            magic=b"\x93NUMPY", hlen_override=None) -> bytes:
    """A hand-built ``.npy``, for the malformed and old-style files numpy won't write."""
    fmt = "<H" if version[0] == 1 else "<I"
    prefix_len = len(magic) + 2 + struct.calcsize(fmt)
    enc = "utf8" if version[0] == 3 else "latin1"
    text = header_text.encode(enc)
    pad = (-(prefix_len + len(text) + 1)) % align
    body = text + b" " * pad + b"\n"
    hlen = len(body) if hlen_override is None else hlen_override
    return magic + bytes(version) + struct.pack(fmt, hlen) + body + payload


# --------------------------------------------------------------------------- output


class Writer:
    def __init__(self, out: Path):
        self.out = out
        self.bytes = 0

    def reset(self) -> None:
        """Remove what this script owns, so a dropped vector leaves no stale fixture."""
        for sub in ("npy", "files", "existing", "policy"):
            shutil.rmtree(self.out / sub, ignore_errors=True)
        for p in self.out.glob("*.json"):
            p.unlink()
        self.out.mkdir(parents=True, exist_ok=True)

    def file(self, rel: str, data: bytes) -> str:
        p = self.out / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        self.bytes += len(data)
        return rel

    def json(self, name: str, doc: dict) -> None:
        text = json.dumps(doc, indent=1, ensure_ascii=True) + "\n"
        self.file(name, text.encode("ascii"))


def doc(category: str, about: str, vectors: list, **extra) -> dict:
    ids = [v["id"] for v in vectors]
    if len(ids) != len(set(ids)):
        raise SystemExit(f"{category}: duplicate vector ids")
    return {"contract": 1, "category": category, "about": about, **extra, "vectors": vectors}


# --------------------------------------------------------------------------- keys

#: Contract 3.1: the reader's grammar (C4). Anchored at both ends with no newline allowance.
GRAMMAR = r"\A[a-z0-9]+:[a-z0-9]+:[a-z0-9]+:[0-9a-f]{32}:[a-z0-9]+\Z"

SCHEMES = {
    "cvkeys": {"domain": "cvkeys", "version": "v1", "forms": ["pil", "f32chw"],
               "variants": ["o0", "o1", "oa"], "dtypes": {"pil": "|u1"}},
    "cvre": {"domain": "cvre", "version": "v2", "forms": ["wav16k"],
             "variant_pattern": "sr[0-9]+", "dtypes": {"wav16k": "<i2"}},
    "cvloose": {"domain": "cvloose", "version": "v1", "forms": ["raw"],
                "variant_pattern": "sr.+"},
}


def keys_doc() -> dict:
    build = []

    def b(id_, scheme, digest, variant, form, expect, python=None, note=None):
        v = {"id": id_, "scheme": scheme, "digest": digest, "variant": variant, "form": form,
             "expect": expect}
        if note:
            v["note"] = note
        if python:
            v["python"] = python
        build.append(v)

    b("lower", "cvkeys", D1, "o0", "pil", {"key": f"cvkeys:v1:pil:{D1}:o0"})
    b("default_form", "cvkeys", D1, "o1", None, {"key": f"cvkeys:v1:pil:{D1}:o1"},
      note="form omitted: the scheme's first form")
    b("second_form", "cvkeys", D2, "oa", "f32chw", {"key": f"cvkeys:v1:f32chw:{D2}:oa"})
    b("upper_folded", "cvkeys", D1.upper(), "o0", "pil", {"key": f"cvkeys:v1:pil:{D1}:o0"},
      note="building folds an upper-case digest (contract 3.3)")
    b("mixed_folded", "cvkeys", D2.upper()[:16] + D2[16:], "o0", "pil",
      {"key": f"cvkeys:v1:pil:{D2}:o0"})
    b("digest_31", "cvkeys", D1[:31], "o0", "pil", {"error": "digest"})
    b("digest_33", "cvkeys", D1 + "a", "o0", "pil", {"error": "digest"})
    b("digest_nonhex", "cvkeys", "g" + D1[1:], "o0", "pil", {"error": "digest"})
    b("digest_empty", "cvkeys", "", "o0", "pil", {"error": "digest"})
    b("digest_space", "cvkeys", " " + D1, "o0", "pil", {"error": "digest"})
    b("digest_newline", "cvkeys", D1 + "\n", "o0", "pil", {"error": "digest"},
      note="a trailing newline is not a digest (`$` matches before one; use fullmatch)")
    b("form_unknown", "cvkeys", D1, "o0", "png", {"error": "form"})
    b("variant_unknown", "cvkeys", D1, "o7", "pil", {"error": "variant"})
    b("variant_case", "cvkeys", D1, "O0", "pil", {"error": "variant"},
      note="variants are never folded")
    b("pattern_ok", "cvre", D3, "sr16000", None, {"key": f"cvre:v2:wav16k:{D3}:sr16000"})
    b("pattern_prefix_only", "cvre", D3, "sr16000x", None, {"error": "variant"},
      note="a variant pattern must match the WHOLE variant (contract 3.2)")
    b("pattern_not_at_start", "cvre", D3, "xsr16000", None, {"error": "variant"})
    b("pattern_breaks_grammar", "cvloose", D3, "sr-1", None, {"error": "variant"},
      note="a variant must satisfy the token grammar whatever the pattern says")

    parse = []

    def p(id_, key, grammar, scheme, expect, python=None, note=None):
        v = {"id": id_, "key": key, "scheme": scheme, "expect": {"grammar": grammar, **expect}}
        if note:
            v["note"] = note
        if python:
            v["python"] = python
        parse.append(v)

    def ok(form, digest, variant):
        return {"scheme_ok": True, "parts": [form, digest, variant],
                "relpath": f"{form}/{digest[:2]}/{digest}.{variant}.npy"}

    bad = {"scheme_ok": False}
    p("ok_closed", f"cvkeys:v1:pil:{D1}:o0", True, "cvkeys", ok("pil", D1, "o0"))
    p("ok_second_form", f"cvkeys:v1:f32chw:{D2}:oa", True, "cvkeys", ok("f32chw", D2, "oa"))
    p("ok_pattern", f"cvre:v2:wav16k:{D3}:sr44100", True, "cvre", ok("wav16k", D3, "sr44100"))
    p("upper_digest", f"cvkeys:v1:pil:{D1.upper()}:o0", False, "cvkeys", bad,
      note="parsing never folds (contract 3.3)")
    p("upper_domain", f"CVKEYS:v1:pil:{D1}:o0", False, "cvkeys", bad)
    p("digest_31", f"cvkeys:v1:pil:{D1[:31]}:o0", False, "cvkeys", bad)
    p("digest_33", f"cvkeys:v1:pil:{D1}a:o0", False, "cvkeys", bad)
    p("digest_nonhex", f"cvkeys:v1:pil:{'g' + D1[1:]}:o0", False, "cvkeys", bad)
    p("four_parts", f"cvkeys:v1:{D1}:o0", False, "cvkeys", bad)
    p("six_parts", f"cvkeys:v1:pil:{D1}:o0:x", False, "cvkeys", bad)
    p("empty_form", f"cvkeys:v1::{D1}:o0", False, "cvkeys", bad)
    p("empty_variant", f"cvkeys:v1:pil:{D1}:", False, "cvkeys", bad)
    p("dotdot_form", f"cvkeys:v1:..:{D1}:o0", False, "cvkeys", bad)
    p("slash_form", f"cvkeys:v1:pil/x:{D1}:o0", False, "cvkeys", bad)
    p("dot_variant", f"cvkeys:v1:pil:{D1}:o0.npy", False, "cvkeys", bad)
    p("underscore", f"cvkeys:v1:pil_x:{D1}:o0", False, "cvkeys", bad)
    p("leading_space", f" cvkeys:v1:pil:{D1}:o0", False, "cvkeys", bad)
    p("trailing_newline", f"cvkeys:v1:pil:{D1}:o0\n", False, "cvkeys", bad,
      note="a key MUST match in full: no final newline (`$` accepts one; use fullmatch)")
    p("empty", "", False, "cvkeys", bad)
    p("unknown_form", f"cvkeys:v1:png:{D1}:o0", True, "cvkeys", bad,
      note="grammatical, so a reader may attach it (C4); the scheme refuses it")
    p("unknown_variant", f"cvkeys:v1:pil:{D1}:o7", True, "cvkeys", bad)
    p("wrong_version", f"cvkeys:v2:pil:{D1}:o0", True, "cvkeys", bad)
    p("other_domain", f"nosuch:v1:pil:{D1}:o0", True, "cvkeys", bad)
    p("pattern_prefix_only", f"cvre:v2:wav16k:{D3}:sr16000x", True, "cvre", bad)

    registry = []

    def r(id_, first, second, expect, python=None, note=None):
        v = {"id": id_, "first": first, "second": second, "expect": expect}
        if note:
            v["note"] = note
        if python:
            v["python"] = python
        registry.append(v)

    base = {"version": "v1", "forms": ["pil", "f32chw"], "variants": ["o0", "o1"],
            "dtypes": {"pil": "|u1"}}
    r("identical", base, base, "idempotent")
    r("other_version", base, {**base, "version": "v2"}, "error")
    r("forms_reordered", base, {**base, "forms": ["f32chw", "pil"]}, "error",
      note="the first form is the default, so order is part of the scheme")
    r("other_variants", base, {**base, "variants": ["o0"]}, "error")
    r("other_dtypes", base, {**base, "dtypes": {}}, "error")
    pat = {"version": "v1", "forms": ["wav"], "variant_pattern": "sr[0-9]+"}
    r("same_pattern", pat, pat, "idempotent")
    r("other_pattern", pat, {**pat, "variant_pattern": "hz[0-9]+"}, "error",
      note="two pattern schemes are the same only when the pattern TEXT is the same")
    r("pattern_vs_closed", pat, {**pat, "variants": ["sr1"], "variant_pattern": None}, "error")
    for i, v in enumerate(registry):
        v["domain"] = f"cvreg{i}"

    for name, section in (("build", build), ("parse", parse), ("registry", registry)):
        ids = [v["id"] for v in section]
        if len(ids) != len(set(ids)):
            raise SystemExit(f"keys.{name}: duplicate vector ids")
    return {"contract": 1, "category": "keys",
            "about": "Key building, parsing (grammar and scheme), key to path, registry",
            "grammar": GRAMMAR, "schemes": SCHEMES, "build": build, "parse": parse,
            "registry": registry}


# --------------------------------------------------------------------------- headers

HEADER_CASES: list[tuple[str, dict, str]] = []


def hc(id_, descr, shape, order="C", fill="ramp", note=""):
    kind = "fields" if isinstance(descr, dict) else "descr"
    HEADER_CASES.append((id_, {kind: descr, "shape": list(shape), "order": order,
                               "fill": fill}, note))


def _header_cases() -> None:
    simple = ["|b1", "|i1", "<i2", "<i4", "<i8", "|u1", "<u2", "<u4", "<u8", "<f2", "<f4",
              "<f8", "<c8", "<c16", "<M8[s]", "<m8[ms]", "<U3", "|S4", "|V4"]
    for d in simple:
        name = d.strip("<>|").replace("[", "_").replace("]", "")
        hc(f"dtype_{name}", d, (3,))
    for d in [">i2", ">i4", ">u8", ">f4", ">f8", ">c8", ">U2", ">M8[s]"]:
        name = d.strip("<>|").replace("[", "_").replace("]", "")
        hc(f"big_{name}", d, (3,), note="big-endian")
    hc("struct_basic", "[('a', '<i4'), ('b', '<f8')]", (2,))
    hc("struct_subarray", "[('rgb', '|u1', (3,)), ('w', '<f4')]", (2,))
    hc("struct_padded", "[('a', '|u1'), ('', '|V3'), ('b', '<i4')]", (2,),
       note="an aligned struct: numpy names its padding ''")
    hc("struct_nested", "[('p', [('x', '<f4'), ('y', '<f4')]), ('id', '<u2')]", (2,))
    hc("struct_quote", "[(\"it's\", '<i2')]", (2,),
       note="Python repr switches to double quotes for a name holding a single quote")
    hc("shape_0d", "<f8", ())
    hc("shape_empty", "<f4", (0,))
    hc("shape_2d", "<f4", (3, 4))
    hc("shape_3d", "|u1", (2, 3, 4))
    hc("fortran_2d", "<i2", (2, 3), "F")
    hc("fortran_3d", "<f4", (2, 3, 4), "F")
    hc("fortran_1d", "<i4", (5,), "F", note="1-d is C-contiguous, so numpy writes False")
    hc("fortran_big", ">f8", (3, 2), "F")
    hc("growth_c_wide", "|u1", (0, 10 ** 15),
       note="growth axis is shape[0]: one digit, 20 spaces of padding")
    hc("growth_c_long", "|u1", (10 ** 15, 0),
       note="growth axis is shape[0]: 16 digits, 5 spaces")
    hc("growth_f_empty", "|u1", (2, 0, 10 ** 15), "F",
       note="asked for Fortran order, but an empty array is C-contiguous too: numpy writes "
       "False, so the growth axis is shape[0]")
    hc("growth_f", "|u1", (2, 1000), "F",
       note="Fortran order: the growth axis is shape[-1] (4 digits, 17 spaces)")
    hc("fortran_degenerate", "<f4", (1, 3), "F",
       note="only one dimension over 1: C-contiguous too, so numpy writes False")
    # Shapes of ones: the header crosses 64-byte boundaries as ndim grows, and numpy's
    # growth-axis padding moves the crossing. The generator keeps every ndim whose padded
    # and unpadded lengths differ, and every ndim whose padding is a full 64 bytes.
    # Up to 32 dimensions: numpy 1.x, which the package still supports, allows no more.
    for ndim in range(1, 33):
        hc(f"ones_{ndim}d", "|u1", (1,) * ndim, note="selected by the generator")
    hc("align_full", "<U100", (1,) * 14,
       note="alignment padding is a full 64 bytes: numpy pads with 1 to 64 spaces, never 0")
    hc("align_min", "<c16", (1,) * 14, note="alignment padding is a single space")
    hc("v1_large", {"count": 600, "digits": 3, "descr": "|u1"}, (0,),
       note="a version 1 header over 10000 bytes: np.load refuses it by default")
    hc("v2", {"count": 3700, "digits": 4, "descr": "|u1"}, (0,),
       note="over 65535 header bytes: numpy writes version 2.0")
    hc("v3", "[('π', '<i4'), ('r', '<f4')]", (2,),
       note="a non-latin-1 field name: numpy writes version 3.0 (utf-8)")


def headers_doc(w: Writer) -> dict:
    _header_cases()
    vectors = []
    differs = {}
    for id_, spec, _note in HEADER_CASES:
        if id_.startswith("ones_"):
            info = header_info(npy_bytes(make_array(spec)))
            differs[len(spec["shape"])] = unpadded_offset(info, 10) != info["data_offset"]
    for id_, spec, note in HEADER_CASES:
        arr = make_array(spec)
        data = npy_bytes(arr)
        info = header_info(data)
        prefix = 10 if info["version"][0] == 1 else 12
        bare = unpadded_offset(info, prefix)
        if id_.startswith("ones_"):
            # Keep the ends of each run of ndims where the growth padding changes the
            # length, the plan's 20-d example, and any ndim padded by a full 64 bytes.
            nd = len(spec["shape"])
            run_end = differs[nd] and not (differs.get(nd - 1) and differs.get(nd + 1))
            if not (run_end or nd == 20 or info["align_padding"] == 64):
                continue
            notes = []
            if bare != info["data_offset"]:
                notes.append("growth padding changes the length")
            if info["align_padding"] == 64:
                notes.append("alignment padding is a full 64 bytes, never 0")
            note = "; ".join(notes)
        assert make_array(spec).tobytes(order="A") == data[info["data_offset"]:], id_
        rel = w.file(f"npy/{id_}.npy", data)
        v = {"id": id_, "file": rel, "array": spec, **info,
             "unpadded_data_offset": bare,
             "itemsize": int(arr.dtype.itemsize),
             "payload_bytes": len(data) - info["data_offset"],
             "file_bytes": len(data), "sha256": sha(data)}
        if len(info["header_text"]) > 2000:
            v["header_text_sha256"] = sha(info["header_text"].encode("utf8"))
            v["header_text"] = v["descr"] = None
        if note:
            v["note"] = note
        vectors.append(v)
    return doc("headers", "numpy-written .npy files: a conforming writer reproduces each "
               "file byte for byte from `array`; a reader recovers every field", vectors)


# --------------------------------------------------------------------------- reads

READ_KEY = "cvread:v1:arr:{digest}:x0"
READ_SCHEME = {"domain": "cvread", "version": "v1", "forms": ["arr"], "variants": ["x0"]}


def reads_doc(w: Writer) -> dict:
    vectors = []
    good_arr = make_array({"descr": "<i4", "shape": [10], "order": "C", "fill": "ramp"})
    good = npy_bytes(good_arr)
    good_text = header_info(good)["header_text"]

    def v(id_, data, outcome, note, python=None, hit=None, ref=None):
        digest = hashlib.md5(id_.encode()).hexdigest()
        rel = ref or (None if data is None else w.file(f"files/{id_}.npy", data))
        rec = {"id": id_, "key": READ_KEY.format(digest=digest), "file": rel,
               "expect": {"outcome": outcome, "counters": {f"shm_{outcome}": 1}},
               "note": note}
        if hit is not None:
            rec["expect"].update(hit)
        if python:
            rec["python"] = python
        vectors.append(rec)

    def hit_of(arr):
        descr = repr(npf.dtype_to_descr(arr.dtype))
        if len(descr) > 2000:
            descr = None
        return {"descr": descr, "shape": list(arr.shape),
                "fortran_order": bool(arr.flags.f_contiguous and not arr.flags.c_contiguous),
                "payload_sha256": sha(arr.tobytes(order="A"))}

    v("absent", None, "miss", "no file at the key's path")
    v("whole", good, "hit", "a numpy-written file", hit=hit_of(good_arr))
    v("trailing_5", good + b"extra", "hit", "trailing bytes after the payload are ignored",
      hit=hit_of(good_arr))
    v("trailing_copy", good + good, "hit", "a whole second copy after the payload",
      hit=hit_of(good_arr))
    v("short_1", good[:-1], "torn", "payload one byte short")
    v("header_only", good[:128], "torn", "the header and no payload")
    v("empty", b"", "torn", "a zero-length file")
    v("magic_only", good[:6], "torn", "six bytes of magic")
    v("prefix_only", good[:10], "torn", "magic, version and length, no header text")
    v("header_cut", good[:60], "torn", "the header text cut short")
    v("bad_magic", b"\x93NUMPX" + good[6:], "torn", "the magic string is wrong")
    v("bad_version", good[:6] + b"\x04\x00" + good[8:], "torn", "format version 4.0")
    v("hlen_past_eof", good[:8] + struct.pack("<H", 4000) + good[10:], "torn",
      "the header length points past the end of the file")
    v("missing_shape", raw_npy("{'descr': '<i4', 'fortran_order': False, }",
                               good_arr.tobytes()), "torn", "no 'shape' key")
    v("extra_key", raw_npy(good_text[:-1] + "'extra': 1, }", good_arr.tobytes()), "torn",
      "an unexpected key")
    v("not_a_dict", raw_npy("[1, 2, 3]", good_arr.tobytes()), "torn", "the header is a list")
    v("garbage_text", raw_npy("{'descr': '<i4', 'fortran_order': Fals", good_arr.tobytes()),
      "torn", "the header does not parse; numpy raises tokenize.TokenError, which "
      "Python's attach does not catch today", python=STEP3)
    v("negative_dim", raw_npy("{'descr': '<i4', 'fortran_order': False, 'shape': (-1,), }",
                              good_arr.tobytes()), "torn", "a negative dimension")
    v("unknown_descr", raw_npy("{'descr': '<X4', 'fortran_order': False, 'shape': (10,), }",
                               good_arr.tobytes()), "torn", "a descriptor no reader knows")
    v("object_descr", raw_npy("{'descr': '|O', 'fortran_order': False, 'shape': (2,), }",
                              bytes(16)), "torn",
      "an object array: a reader never unpickles, so it is a miss")
    v("order_not_bool", raw_npy("{'descr': '<i4', 'fortran_order': 0, 'shape': (10,), }",
                                good_arr.tobytes()), "torn", "fortran_order is not a bool")
    old = raw_npy(good_text, good_arr.tobytes(), align=16)
    v("align_16", old, "hit", "a header padded to 16 bytes, as old numpy wrote it",
      hit=hit_of(good_arr))
    empty_arr = make_array({"descr": "<f4", "shape": [0], "order": "C", "fill": "ramp"})
    v("empty_array", npy_bytes(empty_arr), "hit", "shape (0,): the header is the whole file",
      hit=hit_of(empty_arr))
    f_arr = make_array({"descr": ">f8", "shape": [3, 2], "order": "F", "fill": "ramp"})
    v("fortran_big", npy_bytes(f_arr), "hit", "Fortran order, big-endian", hit=hit_of(f_arr))
    for id_ in ("v1_large", "v2"):
        case = next(c for c in HEADER_CASES if c[0] == id_)
        arr = make_array(case[1])
        v(f"header_{id_}", None, "hit",
          "a header over 10000 bytes but within the contract's 1 MiB limit",
          python=STEP3, hit=hit_of(arr), ref=f"npy/{id_}.npy")
    text = b"{'descr': '|u1', 'fortran_order': False, 'shape': (0,), }"
    over = b"\x93NUMPY\x02\x00" + struct.pack("<I", 2 ** 20 + 52) + text + b" " * 100
    v("header_over_limit", over, "torn",
      "a version 2 length field over the 1 MiB header limit; a reader refuses it before "
      "reading the text (the fixture holds only the first bytes)")
    return doc("reads", "Attach outcomes for files placed at a key's path, in both attach "
               "modes (mmap and read)", vectors, scheme=READ_SCHEME)


# --------------------------------------------------------------------------- admission

ADM_SCHEME = {"domain": "cvadm", "version": "v1", "forms": ["img", "tensor", "free"],
              "variants": ["x0"], "dtypes": {"img": "|u1", "tensor": "<f4"}}
OPEN_STORE = {"mode": "readwrite", "min_array_bytes": 0, "max_array_bytes": 2 ** 20,
              "min_free_bytes": 0, "max_bytes": 2 ** 40}


def admission_doc() -> dict:
    vectors = []

    def a(id_, form, descr, shape, outcome, *, store=None, usage=None, fill="ramp",
          key=None, python=None, note=None):
        spec = {"descr": descr, "shape": list(shape), "order": "C", "fill": fill}
        digest = hashlib.md5(id_.encode()).hexdigest()
        rec = {"id": id_, "key": key or f"cvadm:v1:{form}:{digest}:x0", "array": spec,
               "store": {**OPEN_STORE, **(store or {})}}
        if usage is not None:
            rec["usage"] = usage
        if outcome == "published":
            nbytes = 0 if fill == "objects" else _dtype(descr).itemsize * int(np.prod(shape))
            counters = {"shm_publish": 1}
            if nbytes:
                counters["shm_bytes_published"] = nbytes
        else:
            counters = {"shm_" + outcome.replace(":", "_"): 1}
        rec["expect"] = {"outcome": outcome, "counters": counters}
        if note:
            rec["note"] = note
        if python:
            rec["python"] = python
        vectors.append(rec)

    a("img_u1", "img", "|u1", (4, 4, 3), "published")
    a("img_i1", "img", "|i1", (4, 4, 3), "declined:dtype")
    a("img_bool", "img", "|b1", (4, 4), "declined:dtype")
    a("img_u2", "img", "<u2", (4, 4), "declined:dtype")
    a("tensor_f4", "tensor", "<f4", (2, 3), "published")
    a("tensor_f4_big", "tensor", ">f4", (2, 3), "declined:dtype",
      note="dtype equality, not kind: >f4 is not float32")
    a("tensor_f8", "tensor", "<f8", (2, 3), "declined:dtype")
    a("tensor_f2", "tensor", "<f2", (2, 3), "declined:dtype")
    a("free_c8", "free", "<c8", (3,), "published", note="an undeclared form takes any dtype")
    a("free_struct", "free", "[('a', '<i4'), ('b', '<f8')]", (2,), "published")
    a("free_unicode", "free", "<U3", (2,), "published")
    a("free_datetime", "free", "<M8[s]", (2,), "published")
    a("free_object", "free", "|O", (2,), "declined:dtype", fill="objects", python=STEP3,
      note="C6(c): an object array is declined; Python raises ValueError from np.save today")
    a("img_object", "img", "|O", (2,), "declined:dtype", fill="objects",
      note="the form's declared dtype already declines it")
    # Gate order (contract 6.2): mode, small, size, space, budget, dtype, key.
    a("mode_read", "img", "|u1", (8,), "declined:mode", store={"mode": "read"})
    a("small", "img", "|i1", (8,), "declined:small", store={"min_array_bytes": 9},
      note="the size gate runs before the dtype gate")
    a("size", "img", "|i1", (64,), "declined:size", store={"max_array_bytes": 63})
    a("size_equal", "img", "|u1", (64,), "published", store={"max_array_bytes": 64},
      note="the limit itself is admitted")
    a("small_equal", "img", "|u1", (64,), "published", store={"min_array_bytes": 64})
    a("space", "img", "|i1", (8,), "declined:space", store={"min_free_bytes": 2 ** 62})
    a("budget", "img", "|i1", (8,), "declined:budget", store={"max_bytes": 1000},
      usage={"bytes": 995, "files": 3}, note="a fresh .usage.json: 995 + 8 > 1000")
    a("budget_equal", "img", "|u1", (5,), "published", store={"max_bytes": 1000},
      usage={"bytes": 995, "files": 3}, note="995 + 5 == 1000 is admitted")
    a("bad_key", "img", "|u1", (8,), "bad_key",
      key=f"cvadm:v1:png:{D1}:x0", note="an unknown form: counted after the gates")
    vectors[-1]["expect"]["counters"] = {"shm_bad_key": 1}
    return doc("admission", "Publish gates and dtype admission for a fresh key", vectors,
               scheme=ADM_SCHEME)


# --------------------------------------------------------------------------- existing key

EX_SCHEME = {"domain": "cvex", "version": "v1", "forms": ["arr"], "variants": ["x0"]}


def existing_doc(w: Writer) -> dict:
    vectors = []
    base = {"descr": "<i4", "shape": [2, 3], "order": "C", "fill": "ramp"}

    def e(id_, existing, new, outcome, *, verify=False, after="unchanged", mtime="unchanged",
          python=STEP3, note=None, extra_counters=None):
        digest = hashlib.md5(id_.encode()).hexdigest()
        arr = make_array(new)
        new_bytes = npy_bytes(arr)
        rec = {"id": id_, "key": f"cvex:v1:arr:{digest}:x0",
               "existing": None, "array": new, "verify_republish": verify}
        if existing is not None:
            data = npy_bytes(make_array(existing)) if isinstance(existing, dict) else existing
            rec["existing"] = {"file": w.file(f"existing/{id_}.npy", data),
                               "mtime_age_s": 3600, "sha256": sha(data)}
        counters = dict(extra_counters or {})
        if outcome == "published":
            counters.update({"shm_publish": 1, "shm_bytes_published": arr.nbytes})
        if outcome == "KeyCollision":
            counters["shm_key_collision"] = 1
        rec["expect"] = {"outcome": outcome, "counters": counters, "file_after": after,
                         "mtime": mtime}
        if after == "new":
            rec["expect"]["file_sha256"] = sha(new_bytes)
        if note:
            rec["note"] = note
        if python:
            rec["python"] = python
        vectors.append(rec)

    whole = npy_bytes(make_array(base))
    e("absent", None, base, "published", after="new", mtime="new", python=None)
    e("torn_short", whole[:-3], base, "published", after="new", mtime="new",
      extra_counters={"shm_torn_replaced": 1}, note="damage from outside the contract")
    e("torn_header", b"\x93NUMPY\x01\x00garbage", base, "published", after="new",
      mtime="new", extra_counters={"shm_torn_replaced": 1})
    e("torn_empty", b"", base, "published", after="new", mtime="new",
      extra_counters={"shm_torn_replaced": 1})
    e("match", base, base, "touched", mtime="refreshed",
      note="a republish writes nothing and refreshes mtime")
    e("match_verified", base, base, "touched", verify=True, mtime="refreshed")
    e("match_trailing", whole + b"tail", base, "touched", verify=True, mtime="refreshed",
      note="trailing bytes are not part of the payload compared")
    e("same_header_other_bytes", base, {**base, "fill": "ramp2"}, "touched",
      mtime="refreshed", note="verify_republish off: a same-header collision goes unseen")
    e("same_header_other_bytes_verified", base, {**base, "fill": "ramp2"}, "KeyCollision",
      verify=True)
    e("other_dtype", base, {**base, "descr": "<u4"}, "KeyCollision")
    e("other_byteorder", base, {**base, "descr": ">i4"}, "KeyCollision")
    e("other_shape", base, {**base, "shape": [3, 2]}, "KeyCollision")
    e("other_order", base, {**base, "order": "F"}, "KeyCollision",
      note="same values in another memory order is a different file, so a collision")

    # A structured dtype with padding: a copying np.load zeroes the padding bytes, so an
    # array read back by copy differs from its file only there. verify_republish compares
    # named-field bytes only (contract 6.4, 5.8), so that is a republish, not a collision.
    padded = {"descr": "[('a', '|u1'), ('', '|V3'), ('b', '<i4')]", "shape": [2],
              "order": "C", "fill": "ramp"}
    pad_arr = make_array(padded)
    pad_file = bytearray(npy_bytes(pad_arr))
    offset = len(pad_file) - pad_arr.nbytes
    for item in range(pad_arr.shape[0]):
        for k in range(1, 4):  # bytes 1..3 of each 8-byte item are the '' padding field
            i = offset + item * pad_arr.dtype.itemsize + k
            pad_file[i] = 0 if pad_file[i] else 0xAB
    e("padded_padding_differs_verified", bytes(pad_file), padded, "touched", verify=True,
      mtime="refreshed",
      note="only padding bytes differ (as after a copying read): named fields are compared")
    pad_field = bytearray(npy_bytes(pad_arr))
    pad_field[offset + 4] ^= 0x01  # field 'b' of item 0
    e("padded_field_differs_verified", bytes(pad_field), padded, "KeyCollision", verify=True,
      note="a named field differs: a collision whatever the padding holds")
    return doc("existing_key", "Publishing to a key whose path already exists (C15)", vectors,
               scheme=EX_SCHEME)


# --------------------------------------------------------------------------- reaper paths

def reaper_doc() -> dict:
    vectors = []

    def rp(id_, rel, expect, age_s=7200, kind="file", python=None, note=None):
        rec = {"id": id_, "path": rel, "kind": kind, "age_s": age_s, "expect": expect}
        if note:
            rec["note"] = note
        if python:
            rec["python"] = python
        vectors.append(rec)

    seg = f"pil/ab/{D1}.o0.npy"
    rp("segment", seg, "reaped")
    rp("segment_young", seg, "kept", age_s=60, note="not expired and nothing over target")
    rp("shard_mismatch", f"pil/cd/{D1}.o0.npy", "kept", python=STEP3,
       note="C6(a): the shard directory must be the digest's first two characters")
    rp("too_shallow", f"pil/{D1}.o0.npy", "kept", python=STEP3)
    rp("too_deep", f"x/pil/ab/{D1}.o0.npy", "kept", python=STEP3)
    rp("root_npy", "notes.npy", "kept", python=STEP3)
    rp("no_variant", f"pil/ab/{D1}.npy", "kept", python=STEP3)
    rp("upper_digest", f"pil/ab/{D1.upper()}.o0.npy", "kept", python=STEP3)
    rp("dot_dir", f".pinned/pil/ab/{D1}.o0.npy", "kept", python=STEP3,
       note="reserved dot-names are never reaped (contract 4, 9.4)")
    rp("symlink", seg, "kept", kind="symlink", note="lstat: a symlink is not a regular file")
    rp("tmp_old", f"pil/ab/{D1}.o0.npy.4242.77.tmp", "swept", python=STEP3,
       note="C6(b): an orphaned temporary file over an hour old")
    rp("tmp_dot_old", f"pil/ab/.{D1}.4242.3.tmp", "swept", python=STEP3,
       note="PlumCraft's temporary-file spelling is swept too")
    rp("tmp_young", f"pil/ab/{D1}.o0.npy.4242.77.tmp", "kept", age_s=600)
    rp("tmp_root", "orphan.tmp", "kept", note="only segment directories are swept")
    rp("tmp_dot_dir", ".hello/x.tmp", "kept")
    rp("other_file", "pil/ab/readme.txt", "kept")
    return doc("reaper_paths", "Which paths under the root the reaper may unlink. Each vector "
               "is a tree holding one entry, aged `age_s`, reaped with ttl_s = 3600 and "
               "budgets that require no other eviction", vectors,
               reap={"ttl_s": 3600, "max_bytes": 2 ** 40, "min_free_bytes": 0})


# --------------------------------------------------------------------------- policy

DEFAULT_EXPECT = {"ttl_s": 900.0, "reap_interval_s": 30.0, "min_free_bytes": 8 * 2 ** 30,
                  "max_array_bytes": 128 * 2 ** 20}
GIB = 2 ** 30


def env_doc() -> dict:
    vectors = []

    def ev(id_, env, expect, python=None, note=None):
        rec = {"id": id_, "env": env, "expect": expect}
        if note:
            rec["note"] = note
        if python:
            rec["python"] = python
        vectors.append(rec)

    P = "COMMONKIT_STORE_"
    ev("unset", {}, dict(DEFAULT_EXPECT))
    for id_, raw, val, py in [
            ("ttl_int", "1800", 1800.0, None), ("ttl_float", "2.5", 2.5, None),
            ("ttl_exp", "1e3", 1000.0, None), ("ttl_plus", "+5", 5.0, None),
            ("ttl_minus", "-5", -5.0, None), ("ttl_lead_dot", ".5", 0.5, None),
            ("ttl_trail_dot", "5.", 5.0, None), ("ttl_spaces", " 7\t\n", 7.0, None),
            ("ttl_empty", "", 900.0, None), ("ttl_word", "abc", 900.0, None),
            ("ttl_hex", "0x10", 900.0, None), ("ttl_comma", "7,5", 900.0, None),
            ("ttl_two_dots", "1.2.3", 900.0, None), ("ttl_sign_only", "-", 900.0, None),
            ("ttl_underscore", "1_000", 900.0, STEP3), ("ttl_inf", "inf", 900.0, STEP3),
            ("ttl_infinity", "Infinity", 900.0, STEP3), ("ttl_nan", "nan", 900.0, STEP3),
            ("ttl_overflow", "1e999", 900.0, STEP3),
            ("ttl_arabic_digits", "١٢", 900.0, STEP3),
            ("ttl_nbsp", " 5", 900.0, STEP3)]:
        ev(id_, {P + "TTL_S": raw}, {"ttl_s": val}, python=py)
    ev("reap_zero", {P + "REAP_S": "0"}, {"reap_interval_s": 0.0})
    ev("max_gb_int", {P + "MAX_GB": "7"}, {"max_bytes": 7 * GIB})
    ev("max_gb_frac", {P + "MAX_GB": "0.5"}, {"max_bytes": GIB // 2})
    ev("max_gb_trunc", {P + "MAX_GB": "1e-9"}, {"max_bytes": 1},
       note="1e-9 GiB is 1.07 bytes, truncated toward zero")
    ev("max_gb_huge", {P + "MAX_GB": "1e300"}, {"max_bytes": "default"}, python=STEP3,
       note="a derived byte count over 2**63 - 1 is malformed")
    ev("min_free_gb", {P + "MIN_FREE_GB": "1"}, {"min_free_bytes": GIB})
    ev("max_array_float", {P + "MAX_ARRAY_BYTES": "1000.9"}, {"max_array_bytes": 1000},
       note="parsed as a float, truncated toward zero")
    ev("max_array_exp", {P + "MAX_ARRAY_BYTES": "1.5e3"}, {"max_array_bytes": 1500})
    ev("max_array_word", {P + "MAX_ARRAY_BYTES": "lots"}, {"max_array_bytes": 128 * 2 ** 20})
    ev("max_array_nan", {P + "MAX_ARRAY_BYTES": "nan"}, {"max_array_bytes": 128 * 2 ** 20})
    ev("max_array_inf", {P + "MAX_ARRAY_BYTES": "inf"}, {"max_array_bytes": 128 * 2 ** 20},
       python=STEP3, note="Python raises OverflowError today")
    ev("thresher", {P + "MAX_GB": "7", P + "MIN_FREE_GB": "1", P + "TTL_S": "604800"},
       {"max_bytes": 7 * GIB, "min_free_bytes": GIB, "ttl_s": 604800.0},
       note="the policy thresher sets in ~/.bashrc")
    for flag, attr in (("FSYNC", "fsync"), ("VERIFY_REPUBLISH", "verify_republish")):
        ev(f"{attr}_unset", {}, {attr: False}, python=STEP3)
        for raw, val in [("1", True), ("true", True), ("TRUE", True), ("Yes", True),
                         ("on", True), ("oN", True), ("", False), ("0", False),
                         ("false", False), ("no", False), ("off", False), ("2", False),
                         ("enabled", False), (" 1", False), ("true ", False), ("y", False)]:
            safe = raw.strip().lower() or "empty"
            safe += "_padded" if raw != raw.strip() else ""
            if raw != raw.lower() and raw.strip():
                safe += "_case" + str(sum(c.isupper() for c in raw))
            ev(f"{attr}_{safe}", {P + flag: raw}, {attr: val}, python=STEP3)
    ev("root_tilde", {P + "ROOT": "~/ckstore"}, {"root": "{home}/ckstore"})
    ev("root_absolute", {P + "ROOT": "/Volumes/CommonkitStore/store"},
       {"root": "/Volumes/CommonkitStore/store"})
    ev("root_relative", {P + "ROOT": "rel/store"}, {"root": "{cwd}/rel/store"},
       note="a relative root is resolved against the working directory")
    ev("root_default", {}, {"root": {"darwin": "{home}/Library/Caches/commonkit/store",
                                     "linux": "/dev/shm/commonkit", "windows": None}},
       note="Windows: unspecified until the Windows work merges (contract 12)")
    ev("root_empty", {P + "ROOT": ""}, {"root": {
        "darwin": "{home}/Library/Caches/commonkit/store", "linux": "/dev/shm/commonkit",
        "windows": None}})
    return doc("env", "Environment policy (COMMONKIT_STORE_*) with no .policy.json. "
               "`expect` lists effective store fields; \"default\" means the platform "
               "default; {home} and {cwd} are placeholders", vectors,
               defaults={"darwin": {"max_bytes": 2 * GIB, **DEFAULT_EXPECT},
                         "linux": {"max_bytes": 8 * GIB, **DEFAULT_EXPECT}})


def policy_doc(w: Writer) -> dict:
    vectors = []
    P = "COMMONKIT_STORE_"

    def pv(id_, policy, env, expect, python=None, note=None):
        rec = {"id": id_, "policy_file": policy, "env": env, "expect": expect}
        if note:
            rec["note"] = note
        if python:
            rec["python"] = python
        vectors.append(rec)

    def text(t):
        return {"text": t}

    pv("absent_env", None, {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("file_wins", text('{"TTL_S": 200}'), {P + "TTL_S": "100"}, {"ttl_s": 200.0},
       python=STEP3)
    pv("file_only", text('{"TTL_S": 200}'), {}, {"ttl_s": 200.0}, python=STEP3)
    pv("env_fills", text('{"MAX_GB": 7}'), {P + "MIN_FREE_GB": "1"},
       {"max_bytes": 7 * GIB, "min_free_bytes": GIB}, python=STEP3,
       note="the environment fills what the file leaves out")
    pv("default_fills", text('{"MAX_GB": 7}'), {}, {"max_bytes": 7 * GIB, "ttl_s": 900.0},
       python=STEP3)
    pv("thresher", text('{"MAX_GB": 7, "MIN_FREE_GB": 1, "TTL_S": 604800}'), {},
       {"max_bytes": 7 * GIB, "min_free_bytes": GIB, "ttl_s": 604800.0}, python=STEP3,
       note="a Finder-launched process sees the tree's policy without ~/.bashrc")
    pv("string_value", text('{"TTL_S": "300"}'), {P + "TTL_S": "100"}, {"ttl_s": 300.0},
       python=STEP3, note="a string is parsed as an environment value is")
    pv("float_bytes", text('{"MAX_ARRAY_BYTES": 1000.7}'), {}, {"max_array_bytes": 1000},
       python=STEP3)
    pv("bad_value", text('{"TTL_S": "abc"}'), {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="a malformed value is left out, so the environment fills it")
    pv("bool_for_number", text('{"TTL_S": true}'), {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("null_value", text('{"TTL_S": null}'), {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("nan_value", text('{"TTL_S": NaN}'), {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="NaN is not JSON; the file is malformed")
    pv("bad_json", text('{"TTL_S": 200'), {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="an unparseable file is treated as absent")
    pv("not_object", text("[1, 2]"), {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("duplicate_key", text('{"TTL_S": 200, "TTL_S": 300}'), {P + "TTL_S": "100"},
       {"ttl_s": 100.0}, note="duplicate keys make the file malformed")
    pv("empty_file", text(""), {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("empty_object", text("{}"), {P + "TTL_S": "100"}, {"ttl_s": 100.0})
    pv("root_ignored", text('{"ROOT": "/elsewhere", "TTL_S": 200}'), {},
       {"ttl_s": 200.0, "root": "{root}"}, python=STEP3,
       note="the root is how the file is found, so the file cannot move it")
    pv("lower_case_key", text('{"ttl_s": 200}'), {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="keys are exact")
    pv("unknown_key", text('{"COLOUR": "blue", "TTL_S": 200}'), {}, {"ttl_s": 200.0},
       python=STEP3)
    pv("prefixed_key", text('{"COMMONKIT_STORE_TTL_S": 200}'), {P + "TTL_S": "100"},
       {"ttl_s": 100.0}, note="file keys carry no prefix")
    pv("fsync_true", text('{"FSYNC": true}'), {}, {"fsync": True}, python=STEP3)
    pv("fsync_file_off", text('{"FSYNC": false}'), {P + "FSYNC": "1"}, {"fsync": False},
       python=STEP3, note="file over environment holds for flags too")
    pv("fsync_string", text('{"FSYNC": "yes"}'), {}, {"fsync": True}, python=STEP3)
    pv("fsync_number", text('{"FSYNC": 1}'), {P + "FSYNC": "0"}, {"fsync": False},
       python=STEP3, note="a number is not a flag: left out, the environment says off")
    pv("verify_true", text('{"VERIFY_REPUBLISH": true}'), {}, {"verify_republish": True},
       python=STEP3)
    pad = '{"TTL_S": 200, "PAD": "' + "x" * 70000 + '"}'
    big = w.file("policy/oversize.policy.json", pad.encode("ascii"))
    pv("oversize", {"file": big}, {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="a file over 65536 bytes is treated as absent")
    pv("directory", {"directory": True}, {P + "TTL_S": "100"}, {"ttl_s": 100.0},
       note="a directory named .policy.json is treated as absent")
    return doc("policy", ".policy.json at the store root against the environment (C7). "
               "{root} is the store root", vectors)


# --------------------------------------------------------------------------- main


def main() -> int:
    w = Writer(OUT)
    w.reset()
    w.json("keys.json", keys_doc())
    w.json("headers.json", headers_doc(w))
    w.json("reads.json", reads_doc(w))
    w.json("admission.json", admission_doc())
    w.json("existing_key.json", existing_doc(w))
    w.json("reaper_paths.json", reaper_doc())
    w.json("env.json", env_doc())
    w.json("policy.json", policy_doc(w))
    print(f"wrote {w.bytes} bytes under {OUT.relative_to(HERE.parent)}", file=sys.stderr)
    if w.bytes > 512 * 1024:
        print("fixtures exceed 512 KiB; keep them small", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

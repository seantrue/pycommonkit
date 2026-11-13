"""No hashing is reachable from the store (the alexandria constraint, contract §3.8).

commonkit never computes a digest of a stored value and never re-checks a caller's: the
rule compares a length. So nothing the store imports may import ``hashlib``, ``blake3`` or
``commonkit.digest``. Checked twice: statically over the import graph, and in a child that
imports everything ``__init__`` does.
"""
from __future__ import annotations

import ast
import subprocess
import sys
from pathlib import Path

PKG = Path(__file__).resolve().parent.parent / "src" / "commonkit"
STORE_SIDE = ("__init__", "store", "cache", "keys", "read", "shared", "policy", "_marks")
BANNED = {"hashlib", "blake3", "commonkit.digest", "_hashlib", "_md5", "_sha2"}


def _imports(mod: str) -> set[str]:
    tree = ast.parse((PKG / f"{mod}.py").read_text())
    out = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            out |= {a.name for a in node.names}
        elif isinstance(node, ast.ImportFrom):
            if node.level:                              # from . import x / from .x import y
                base = "commonkit" + (f".{node.module}" if node.module else "")
                out.add(base)
                if not node.module:
                    out |= {f"commonkit.{a.name}" for a in node.names}
            else:
                out.add(node.module or "")
    return out


def test_the_import_graph_from_the_store_reaches_no_hashing():
    seen, todo = set(), [f"commonkit.{m}" if m != "__init__" else "commonkit" for m in STORE_SIDE]
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        assert name not in BANNED, f"reachable from the store: {name}"
        if name == "commonkit" or name.startswith("commonkit."):
            mod = "__init__" if name == "commonkit" else name.split(".", 1)[1]
            if (PKG / f"{mod}.py").exists():
                todo.extend(_imports(mod))
    assert "commonkit.policy" in seen and "commonkit._marks" in seen


def test_importing_the_store_side_loads_no_hashing():
    code = ("import sys, commonkit, commonkit.store, commonkit.cache, commonkit.keys, "
            "commonkit.read, commonkit.shared, commonkit.policy, commonkit._marks\n"
            "bad = sorted(m for m in ('commonkit.digest', 'blake3') if m in sys.modules)\n"
            "print(bad)")
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                         check=True).stdout
    assert out.strip() == "[]"

"""Helpers for the tests that need a process of their own.

The hash setting is resolved once per process (contract §3.8), so a test of the other
algorithm, of the policy file against ``COMMONKIT_HASH``, or of what a process prints at
its first resolution runs a child. Each child gets a fresh store root, mark folder and
run folder of its own, and never the shell's ``COMMONKIT_*`` variables.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap

import pytest

SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src")


class Child:
    def __init__(self, tmp_path):
        self.tmp = tmp_path
        self.n = 0

    def dirs(self):
        self.n += 1
        run = self.tmp / f"run{self.n}"
        root, marks = run / "store", run / "marks"
        root.mkdir(parents=True)
        return run, root, marks

    def __call__(self, code: str, *, algo: str | None = "b3", policy: dict | None = None,
                 env: dict | None = None, root=None, marks=None, run=None, check=True,
                 flags: tuple = ()):
        """Run ``code`` in a fresh interpreter (with ``flags``, such as ``-O``). Returns
        (CompletedProcess, root, marks)."""
        r, rt, mk = self.dirs()
        run, root, marks = run or r, root or rt, marks or mk
        if policy is not None:
            (root / ".policy.json").write_text(json.dumps(policy))
        child_env = {k: v for k, v in os.environ.items() if not k.startswith("COMMONKIT_")}
        child_env.update({"COMMONKIT_STORE_ROOT": str(root), "COMMONKIT_MARK_DIR": str(marks),
                          "COMMONKIT_TEST_RUN": str(run), "PYTHONPATH": SRC,
                          "PYTHONDONTWRITEBYTECODE": "1"})
        if algo is not None:
            child_env["COMMONKIT_HASH"] = algo
        for k, v in (env or {}).items():
            if v is None:
                child_env.pop(k, None)
            else:
                child_env[k] = v
        p = subprocess.run([sys.executable, *flags, "-c", textwrap.dedent(code)], env=child_env,
                           capture_output=True, text=True, timeout=300, check=False)
        if check:
            assert p.returncode == 0, f"child failed {p.returncode}\n{p.stdout}\n{p.stderr}"
        return p, root, marks


@pytest.fixture
def child(tmp_path):
    return Child(tmp_path)


def marks_in(folder) -> list[str]:
    """The marker names in a mark folder: ``started``, ``shm_bad_key`` and so on."""
    if not os.path.isdir(folder):
        return []
    return sorted(name.split(".", 1)[1] for name in os.listdir(folder))

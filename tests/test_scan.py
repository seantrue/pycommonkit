"""``commonkit.scan``: find digests of the wrong length in a tree's rule forms, and
``vimp:v1`` leftovers (a whole variant of 40 hex digits), without commonkit installed.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

from commonkit import scan

SCAN = Path(__file__).resolve().parent.parent / "src" / "commonkit" / "scan.py"


def plant(root: Path, form: str, digest: str, variant: str) -> None:
    d = root / form / digest[:2]
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{digest}.{variant}.npy").write_bytes(b"x")


def tree(tmp_path) -> Path:
    root = tmp_path / "store"
    plant(root, "vimparr", "ab" * 32, "e1")                  # fine under b3
    plant(root, "vimparr", "ab" * 16, "e1")                  # 32 digits: foreign under b3
    plant(root, "vimptup1", "cd" * 32, "0f" * 20 + "e3")     # vimp:v1 variant
    plant(root, "vimptup2", "cd" * 32, "0f" * 32 + "e3")     # vimp:v2 variant: fine
    plant(root, "pil", "ef" * 16, "o0")                      # alexandria's: never read
    (root / "vimparr" / "ab" / "junk.tmp").write_bytes(b"")
    return root


def test_foreign_digests_finds_exactly_the_planted_cases(tmp_path):
    root = tree(tmp_path)
    segments, findings = scan.scan(root, ["vimparr", "vimptup1", "vimptup2"], "b3")
    assert segments == 4
    reasons = sorted((Path(p).parts[0], why) for p, why in findings)
    assert reasons == [("vimparr", "digest length 32: b3 is 64"),
                       ("vimptup1", "vimp:v1 variant: 40 hex digits")]
    assert not any(p.startswith("pil") for p, _ in findings)   # not named, so not read


def test_it_runs_from_stdin_with_no_commonkit(tmp_path):
    root = tree(tmp_path)
    p = subprocess.run([sys.executable, "-I", "-", "--root", str(root), "--algo", "b3",
                        "--forms", "vimparr,vimptup1,vimptup2"], stdin=SCAN.open("rb"),
                       capture_output=True, check=False)
    out = p.stdout.decode()
    assert p.returncode == 1, out
    assert "2 findings in 4 segments" in out
    p = subprocess.run([sys.executable, "-I", "-", "--root", str(root), "--algo", "b3",
                        "--forms", "vimptup2"], stdin=SCAN.open("rb"), capture_output=True,
                       check=False)
    assert p.returncode == 0 and b"0 findings in 1 segments" in p.stdout

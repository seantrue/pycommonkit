"""Find digests that don't belong in a tree (plan D13: "any appearance of unintended hashes
is a bug").

Given a root, the forms of the rule schemes and an algorithm, reports every segment in those
forms whose digest length is not the algorithm's, and every segment whose WHOLE variant is
40 hex digits with an optional ``e<digit>`` (a ``vimp:v1`` SHA-1 leftover; a 64-digit
``vimp:v2`` variant doesn't match). Forms not named, such as alexandria's ``pil``, are never
read. Standard library only and no commonkit import, so it runs on a host without it::

    ssh host 'python3 - --root /dev/shm/commonkit --algo md5 --forms vimparr,impcarr' < scan.py

Exit status: 0 no finding, 1 findings, 2 usage.
"""
from __future__ import annotations

import argparse
import os
import re
import sys

HEX_LEN = {"md5": 32, "b3": 64}
_SEGMENT = re.compile(r"([0-9a-f]+)\.([a-z0-9]+)\.npy")
_SHARD = re.compile(r"[0-9a-f]{2}")
_V1_VARIANT = re.compile(r"[0-9a-f]{40}(e[0-9])?")


def scan(root, forms, algo):
    """``(segments, findings)``: how many segments the named forms hold, and each finding
    as ``(path relative to root, reason)``."""
    want = HEX_LEN[algo]
    segments, findings = 0, []
    for form in forms:
        base = os.path.join(os.fspath(root), form)
        try:
            shards = sorted(os.listdir(base))
        except OSError:
            continue
        for shard in shards:
            if not _SHARD.fullmatch(shard) or not os.path.isdir(os.path.join(base, shard)):
                continue
            for name in sorted(os.listdir(os.path.join(base, shard))):
                m = _SEGMENT.fullmatch(name)
                if not m:
                    continue
                segments += 1
                rel = os.path.join(form, shard, name)
                digest, variant = m.groups()
                if len(digest) != want:
                    findings.append((rel, f"digest length {len(digest)}: {algo} is {want}"))
                if _V1_VARIANT.fullmatch(variant):
                    findings.append((rel, "vimp:v1 variant: 40 hex digits"))
    return segments, findings


def foreign_digests(root, forms, algo):
    """The findings of :func:`scan` alone."""
    return scan(root, forms, algo)[1]


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", required=True)
    ap.add_argument("--algo", required=True, choices=sorted(HEX_LEN))
    ap.add_argument("--forms", required=True, help="comma-separated rule forms")
    a = ap.parse_args(argv)
    forms = [f for f in a.forms.split(",") if f]
    segments, findings = scan(a.root, forms, a.algo)
    for rel, why in findings:
        print(f"finding\t{rel}\t{why}")
    print(f"{len(findings)} findings in {segments} segments under {a.root} "
          f"({', '.join(forms)}; {a.algo})")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

"""The hash setting (contract §3.8, §10.1, §10.6): ``HASH`` in ``.policy.json``, then
``COMMONKIT_HASH``, then the compiled default ``b3``; resolved once per process, shared by
the facade and the store, and announced on stderr with its source.
"""
from __future__ import annotations

import os

from commonkit import policy


def test_the_pure_resolution():
    r = policy.resolve_hash
    assert r({}, {})[:2] == ("b3", "default")
    assert r({}, {"COMMONKIT_HASH": " MD5 "})[:2] == ("md5", "environment COMMONKIT_HASH")
    assert r({"HASH": "md5"}, {"COMMONKIT_HASH": "b3"}, "/t/.policy.json")[:2] == \
        ("md5", "policy file /t/.policy.json")
    algo, source, warnings = r({"HASH": "sha1"}, {"COMMONKIT_HASH": "md5"}, "/t/.policy.json")
    assert (algo, source) == ("md5", "environment COMMONKIT_HASH") and len(warnings) == 1
    algo, source, warnings = r({"HASH": 5}, {"COMMONKIT_HASH": "x"}, "/t/.policy.json")
    assert (algo, source) == ("b3", "default") and len(warnings) == 2
    assert r({}, {"COMMONKIT_HASH": ""}) == ("b3", "default", [])
    assert policy.HEX_LEN == {"md5": 32, "b3": 64}


def test_the_file_beats_the_variable_and_the_line_says_so(child):
    p, root, _m = child("""
        from commonkit import digest, policy
        print(policy.hash_algo(), digest.current(), policy.hash_source())
    """, algo="b3", policy={"HASH": "md5", "TTL_S": 5})
    pol = os.path.join(str(root), ".policy.json")
    assert p.stdout.split()[:2] == ["md5", "md5"]
    assert p.stderr.count("commonkit: hash") == 1
    assert f"commonkit: hash md5 (policy file {pol})" in p.stderr


def test_a_malformed_value_falls_through_with_one_warning(child):
    p, _root, _m = child("""
        from commonkit import policy
        print(policy.hash_algo())
    """, algo="b3", policy={"HASH": "blake2"})
    assert p.stdout.strip() == "b3"
    assert p.stderr.count("commonkit: ignoring HASH") == 1
    assert "commonkit: hash b3 (environment COMMONKIT_HASH)" in p.stderr


def test_the_default_is_b3(child):
    p, _root, _m = child("""
        from commonkit import policy
        print(policy.hash_algo())
    """, algo=None)
    assert p.stdout.strip() == "b3"
    assert "commonkit: hash b3 (default)" in p.stderr


def test_the_hash_key_stays_out_of_the_store_defaults():
    # alexandria's test_store_policy_defaults_match_commonkit compares DEFAULTS by name
    from commonkit.shared import DEFAULTS
    assert "HASH" not in DEFAULTS and "HASH" not in policy.DEFAULTS


class _Gone:
    """A stderr whose parent went away (``OSError``) or that was closed (``ValueError``)."""

    def __init__(self, exc):
        self.exc = exc

    def write(self, _s):
        raise self.exc

    def flush(self):
        raise self.exc


def test_a_broken_stderr_never_fails_the_resolution(monkeypatch):
    # The first resolution can happen inside a store call, which never raises (finding 2)
    for exc in (BrokenPipeError(32, "Broken pipe"), ValueError("I/O on closed file")):
        monkeypatch.setattr(policy, "_HASH", None)
        monkeypatch.setattr("sys.stderr", _Gone(exc))
        assert policy.hash_algo() in policy.HEX_LEN
        assert policy.hash_source()


def test_no_root_reads_no_policy_file_from_the_working_directory(monkeypatch, tmp_path):
    # Finding 7 of the L0 review, through hash_algo() itself: a default root whose ~ nothing
    # could expand (no home) is no root, so neither ./.policy.json nor ./~/store/.policy.json
    # is read. A host always finds a home through the password database, so the default
    # root stands in for that here. Rust's resolve_process test agrees.
    (tmp_path / ".policy.json").write_text('{"HASH": "md5"}')
    (tmp_path / "~" / "store").mkdir(parents=True)
    (tmp_path / "~" / "store" / ".policy.json").write_text('{"HASH": "md5"}')
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("COMMONKIT_STORE_ROOT", raising=False)
    monkeypatch.delenv("COMMONKIT_HASH", raising=False)
    monkeypatch.setitem(policy.DEFAULTS, "ROOT", "~/store")
    monkeypatch.setattr(policy, "_HASH", None)
    assert (policy.hash_algo(), policy.hash_source()) == ("b3", "default")
    # and a root that is there is read
    monkeypatch.setenv("COMMONKIT_STORE_ROOT", str(tmp_path))
    monkeypatch.setattr(policy, "_HASH", None)
    assert (policy.hash_algo(), policy.hash_source()) == \
        ("md5", f"policy file {tmp_path / '.policy.json'}")

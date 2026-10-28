"""The shared store's tree-wide policy comes from the environment, never from a caller."""
import numpy as np
import pytest

from commonkit import KeyScheme, register, shared
from commonkit.shared import DEFAULTS, ENV_PREFIX, setting, shared_store

SCHEME = register(KeyScheme(domain="ckshared", version="v1", forms=("ckshwav",),
                            variants=r"sr[0-9]+"))
DIGEST = "0123456789abcdef0123456789abcdef"


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch):
    for name in DEFAULTS:
        monkeypatch.delenv(ENV_PREFIX + name, raising=False)


def test_defaults_per_platform(monkeypatch):
    if shared.sys.platform == "darwin":
        assert setting("ROOT").endswith("Library/Caches/commonkit/store")
        assert setting("MAX_GB") == 2.0  # local snapshots pin reaped segments
    else:
        assert setting("ROOT") == "/dev/shm/commonkit"
        assert setting("MAX_GB") == 8.0
    assert setting("TTL_S") == 900.0 and setting("MIN_FREE_GB") == 8.0


def test_environment_overrides_and_bad_values_fall_back(monkeypatch, tmp_path):
    monkeypatch.setenv(ENV_PREFIX + "ROOT", str(tmp_path))
    monkeypatch.setenv(ENV_PREFIX + "TTL_S", "86400")
    monkeypatch.setenv(ENV_PREFIX + "MAX_GB", "not a number")
    assert setting("ROOT") == str(tmp_path)
    assert setting("TTL_S") == 86400.0
    assert setting("MAX_GB") == DEFAULTS["MAX_GB"]


def test_store_takes_policy_from_environment(monkeypatch, tmp_path):
    monkeypatch.setenv(ENV_PREFIX + "ROOT", str(tmp_path))
    monkeypatch.setenv(ENV_PREFIX + "MAX_GB", "0.5")
    monkeypatch.setenv(ENV_PREFIX + "TTL_S", "60")
    monkeypatch.setenv(ENV_PREFIX + "MIN_FREE_GB", "0")
    store = shared_store(min_array_bytes=1)
    assert store.root == str(tmp_path) and not store.disabled
    assert store.max_bytes == 2**29 and store.ttl_s == 60 and store.min_free_bytes == 0
    key = SCHEME.key(DIGEST, "sr16000", "ckshwav")
    samples = np.arange(1000, dtype=np.int16)
    assert store.publish(key, samples)
    assert np.array_equal(store.attach(key), samples)


def test_two_callers_share_one_tree(monkeypatch, tmp_path):
    monkeypatch.setenv(ENV_PREFIX + "ROOT", str(tmp_path))
    monkeypatch.setenv(ENV_PREFIX + "MIN_FREE_GB", "0")
    writer, reader = shared_store(min_array_bytes=1), shared_store(mode="read")
    key = SCHEME.key(DIGEST, "sr44100", "ckshwav")
    assert writer.publish(key, np.ones(10, np.float32))
    assert reader.attach(key) is not None
    assert not reader.publish(key, np.zeros(10, np.float32))  # read mode never publishes


def test_root_argument_is_for_tests(tmp_path):
    store = shared_store(root=tmp_path, min_array_bytes=1)
    assert store.root == str(tmp_path)


def test_attach_is_the_callers_choice(tmp_path):
    assert shared_store(root=tmp_path).attach_mode == "mmap"
    assert shared_store(root=tmp_path, attach="read").attach_mode == "read"

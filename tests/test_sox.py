"""Real audio, end to end: sox renders a waveform to headerless raw samples,
the samples are published, and a reader maps the segment back in.

Skipped when ``sox`` is not on PATH. The source is a ``synth`` spec rather
than a checked-in file, so the digest names the SOURCE and each sox rendering
of it (sample rate, encoding) is a variant or a form of that one digest --
which is exactly the addressing the scheme is for.
"""
import hashlib
import shutil
import subprocess
import sys
import textwrap

import numpy as np
import pytest

from commonkit import KeyScheme, SharedArrayStore, register

SOX = shutil.which("sox")
pytestmark = pytest.mark.skipif(SOX is None, reason="sox is not installed")

# ``wav16k`` -> int16 and ``f32`` -> float32 are both declared, so a sox dump
# in the wrong encoding is refused rather than stored under a misleading form.
SCHEME = {"domain": "cksox", "version": "v1", "forms": ("wav16k", "f32"),
          "variants": r"^sr\d+$", "dtypes": {"wav16k": "int16", "f32": "float32"}}
SOXSCHEME = register(KeyScheme(**SCHEME))

#: The "source": two seconds of a 440 Hz sine. Its digest is the segment name.
SYNTH = ("synth", "2", "sine", "440")
DIGEST = hashlib.md5(" ".join(SYNTH).encode()).hexdigest()

#: sox encoding flags and the numpy dtype that reads them back, little-endian.
ENCODINGS = {
    "int16": (("-b", "16", "-e", "signed-integer"), "<i2"),
    "float32": (("-b", "32", "-e", "floating-point"), "<f4"),
}


def sox_raw(tmp_path, rate, encoding):
    """Render SYNTH with sox to a raw file and read the samples back."""
    flags, dtype = ENCODINGS[encoding]
    out = tmp_path / f"sine.{rate}.{encoding}.raw"
    subprocess.run([SOX, "-n", "-L", "-r", str(rate), "-c", "1", *flags,
                    "-t", "raw", str(out), *SYNTH],
                   check=True, capture_output=True)
    return np.fromfile(out, dtype=dtype).astype(np.dtype(dtype).newbyteorder("="))


@pytest.fixture
def store(tmp_path):
    return SharedArrayStore(str(tmp_path / "seg"), mode="readwrite",
                            max_bytes=64 * 2**20, min_free_bytes=1,
                            min_array_bytes=1024, max_array_bytes=8 * 2**20,
                            ttl_s=900, reap_interval_s=0)


def test_a_sox_dump_is_published_and_mapped_back_in(store, tmp_path):
    samples = sox_raw(tmp_path, 16000, "int16")
    assert samples.shape == (32000,) and samples.dtype == np.int16

    key = SOXSCHEME.key(DIGEST, "sr16000", "wav16k")
    assert store.publish(key, samples) is True

    mapped = store.attach(key)
    assert isinstance(mapped, np.memmap)          # mapped, not copied
    assert mapped.flags.writeable is False
    assert np.array_equal(mapped, samples)


def test_another_process_maps_the_same_samples(store, tmp_path):
    """The point of the package: the publisher decodes, a separate process
    attaches the segment by key alone and sees the same samples."""
    samples = sox_raw(tmp_path, 16000, "int16")
    key = SOXSCHEME.key(DIGEST, "sr16000", "wav16k")
    assert store.publish(key, samples) is True

    child = textwrap.dedent(f"""
        import numpy as np
        from commonkit import KeyScheme, SharedArrayStore, register
        register(KeyScheme(**{SCHEME!r}))
        a = SharedArrayStore({store.root!r}, mode="read").attach({key!r})
        print(type(a).__name__, a.dtype, a.shape[0], int(a.astype(np.int64).sum()))
    """)
    out = subprocess.run([sys.executable, "-c", child], check=True,
                         capture_output=True, text=True).stdout.split()

    assert out == ["memmap", "int16", str(samples.size),
                   str(int(samples.astype(np.int64).sum()))]


def test_each_sample_rate_is_a_variant_of_one_source(store, tmp_path):
    """Same digest, two renderings: the rate lives in the variant, so both are
    stored side by side and neither shadows the other."""
    lo = sox_raw(tmp_path, 16000, "int16")
    hi = sox_raw(tmp_path, 44100, "int16")
    k_lo = SOXSCHEME.key(DIGEST, "sr16000", "wav16k")
    k_hi = SOXSCHEME.key(DIGEST, "sr44100", "wav16k")

    assert store.publish(k_lo, lo) and store.publish(k_hi, hi)
    assert store.attach(k_lo).shape == (32000,)
    assert store.attach(k_hi).shape == (88200,)


def test_a_float_dump_lands_only_under_the_float_form(store, tmp_path):
    """sox can emit float32 just as easily; the declared dtypes keep it out of
    ``wav16k`` and let it into ``f32``."""
    floats = sox_raw(tmp_path, 16000, "float32")
    assert floats.dtype == np.float32

    assert store.publish(SOXSCHEME.key(DIGEST, "sr16000", "wav16k"), floats) is False
    assert store.drain_stats()["shm_declined_dtype"] == 1

    key = SOXSCHEME.key(DIGEST, "sr16000", "f32")
    assert store.publish(key, floats) is True
    assert np.array_equal(store.attach(key), floats)

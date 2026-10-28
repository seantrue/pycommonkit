"""Real video, end to end: ffmpeg renders a short clip to headerless raw frames,
the frames are published as one ``(T, H, W, C)`` array, and a reader maps the
segment back in.

Skipped when ``ffmpeg`` is not on PATH. The source is ffmpeg's built-in
``testsrc`` pattern rather than a checked-in file, so the digest names the
SOURCE and each rendering of it (frame rate, pixel format) is a variant or a
form of that one digest -- the video counterpart of ``test_sox``.
"""
import hashlib
import shutil
import subprocess
import sys
import textwrap

import numpy as np
import pytest

from commonkit import KeyScheme, SharedArrayStore, register

FFMPEG = shutil.which("ffmpeg")
pytestmark = pytest.mark.skipif(FFMPEG is None, reason="ffmpeg is not installed")

# ``rgb24`` -> uint8 and ``grayf32`` -> float32 are both declared, so a frame
# dump in the wrong pixel format is refused rather than stored under a
# misleading form. The variant carries the frame rate, as ``sr16000`` carries
# the sample rate for audio.
SCHEME = {"domain": "ckvideo", "version": "v1", "forms": ("rgb24", "grayf32"),
          "variants": r"^fr\d+$", "dtypes": {"rgb24": "uint8", "grayf32": "float32"}}
VIDEO = register(KeyScheme(**SCHEME))

W, H, SECONDS = 160, 120, 1

#: The "source": one second of ffmpeg's test pattern. Its digest is the name.
SOURCE = f"testsrc=size={W}x{H}"
DIGEST = hashlib.md5(SOURCE.encode()).hexdigest()

#: ffmpeg pixel format -> (numpy dtype, channels per pixel), little-endian.
PIX_FMTS = {
    "rgb24": ("u1", 3),
    "grayf32le": ("<f4", 1),
}


def ffmpeg_raw(tmp_path, rate, pix_fmt):
    """Render SOURCE with ffmpeg to raw frames and read them back as
    ``(T, H, W, C)``. Raw video has no header, so the shape is ours to supply."""
    dtype, channels = PIX_FMTS[pix_fmt]
    out = tmp_path / f"clip.{rate}.{pix_fmt}.raw"
    subprocess.run([FFMPEG, "-loglevel", "error", "-y",
                    "-f", "lavfi", "-i", f"{SOURCE}:rate={rate}", "-t", str(SECONDS),
                    "-f", "rawvideo", "-pix_fmt", pix_fmt, str(out)],
                   check=True, capture_output=True)
    frames = np.fromfile(out, dtype=dtype).reshape(-1, H, W, channels)
    return frames.astype(np.dtype(dtype).newbyteorder("="))


@pytest.fixture
def store(tmp_path):
    return SharedArrayStore(str(tmp_path / "seg"), mode="readwrite",
                            max_bytes=64 * 2**20, min_free_bytes=1,
                            min_array_bytes=1024, max_array_bytes=8 * 2**20,
                            ttl_s=900, reap_interval_s=0)


def test_an_ffmpeg_dump_is_published_and_mapped_back_in(store, tmp_path):
    frames = ffmpeg_raw(tmp_path, 10, "rgb24")
    assert frames.shape == (10, H, W, 3) and frames.dtype == np.uint8

    key = VIDEO.key(DIGEST, "fr10", "rgb24")
    assert store.publish(key, frames) is True

    mapped = store.attach(key)
    assert isinstance(mapped, np.memmap)          # mapped, not copied
    assert mapped.flags.writeable is False
    assert mapped.shape == frames.shape
    assert np.array_equal(mapped, frames)
    assert np.array_equal(mapped[7], frames[7])   # one frame is a view, no copy


def test_another_process_maps_the_same_frames(store, tmp_path):
    """The publisher decodes; a separate process attaches the clip by key
    alone and sees the same frames."""
    frames = ffmpeg_raw(tmp_path, 10, "rgb24")
    key = VIDEO.key(DIGEST, "fr10", "rgb24")
    assert store.publish(key, frames) is True

    child = textwrap.dedent(f"""
        import numpy as np
        from commonkit import KeyScheme, SharedArrayStore, register
        register(KeyScheme(**{SCHEME!r}))
        a = SharedArrayStore({store.root!r}, mode="read").attach({key!r})
        print(type(a).__name__, a.dtype, "x".join(map(str, a.shape)),
              int(a.astype(np.int64).sum()))
    """)
    out = subprocess.run([sys.executable, "-c", child], check=True,
                         capture_output=True, text=True).stdout.split()

    assert out == ["memmap", "uint8", "x".join(map(str, frames.shape)),
                   str(int(frames.astype(np.int64).sum()))]


def test_each_frame_rate_is_a_variant_of_one_source(store, tmp_path):
    """Same digest, two renderings: the frame rate lives in the variant, so
    both are stored side by side and neither shadows the other."""
    slow = ffmpeg_raw(tmp_path, 10, "rgb24")
    fast = ffmpeg_raw(tmp_path, 25, "rgb24")
    k_slow = VIDEO.key(DIGEST, "fr10", "rgb24")
    k_fast = VIDEO.key(DIGEST, "fr25", "rgb24")

    assert store.publish(k_slow, slow) and store.publish(k_fast, fast)
    assert store.attach(k_slow).shape[0] == 10 * SECONDS
    assert store.attach(k_fast).shape[0] == 25 * SECONDS


def test_a_float_dump_lands_only_under_the_float_form(store, tmp_path):
    """ffmpeg can emit float32 grayscale just as easily; the declared dtypes
    keep it out of ``rgb24`` and let it into ``grayf32``."""
    gray = ffmpeg_raw(tmp_path, 10, "grayf32le")
    assert gray.shape == (10, H, W, 1) and gray.dtype == np.float32

    assert store.publish(VIDEO.key(DIGEST, "fr10", "rgb24"), gray) is False
    assert store.drain_stats()["shm_declined_dtype"] == 1

    key = VIDEO.key(DIGEST, "fr10", "grayf32")
    assert store.publish(key, gray) is True
    assert np.array_equal(store.attach(key), gray)


def test_a_clip_over_the_per_item_ceiling_is_declined_not_truncated(tmp_path):
    """Raw video outgrows ``max_array_bytes`` fast (one 1080p RGB frame is
    ~6 MB). An oversized clip is declined whole -- the caller keeps its private
    frames -- rather than stored partially."""
    frames = ffmpeg_raw(tmp_path, 25, "rgb24")    # 25 * 160*120*3 = 1.44 MB
    small = SharedArrayStore(str(tmp_path / "small"), mode="readwrite",
                             min_free_bytes=1, min_array_bytes=1024,
                             max_array_bytes=frames.nbytes - 1, reap_interval_s=0)
    key = VIDEO.key(DIGEST, "fr25", "rgb24")

    assert small.publish(key, frames) is False
    assert small.drain_stats()["shm_declined_size"] == 1
    assert small.attach(key) is None

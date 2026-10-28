"""The addressing layer, exercised through a NON-pixels domain.

The package claims to be generic. A test suite that only ever addresses images
would not show that, so the working scheme here is audio -- whose variant axis
is sample rate, not orientation -- and the pixels scheme appears once, to pin
that the extraction did not change a single byte of the keys already in use.
"""
import pytest

from commonkit.keys import KeyScheme, parse, register, relpath, scheme_for

D1 = "ab" * 16


def audio_scheme(domain="audio"):
    # ``dtypes`` maps FORM -> required dtype; see test_store for enforcement.
    return KeyScheme(domain=domain, version="v1", forms=("wav16k", "f32chw"),
                     variants=r"^sr\d+$",
                     dtypes={"wav16k": "int16", "f32chw": "float32"})


def test_a_key_round_trips_through_parse():
    s = audio_scheme("rt")
    key = s.key(D1, "sr16000", "wav16k")

    assert key == f"rt:v1:wav16k:{D1}:sr16000"
    assert s.parse(key) == ("wav16k", D1, "sr16000")


def test_the_path_shards_two_levels_like_a_cas():
    s = audio_scheme("shard")
    assert s.relpath(s.key(D1, "sr16000", "wav16k")) == \
        f"wav16k/ab/{D1}.sr16000.npy"


def test_an_open_ended_variant_axis_is_a_regex():
    """Sample rates are not a closed set the way orientations are."""
    s = audio_scheme("open")
    assert s.valid_variant("sr16000") and s.valid_variant("sr44100")
    assert not s.valid_variant("o1")


def test_a_closed_variant_axis_refuses_what_it_does_not_know():
    s = KeyScheme("closed", "v1", ("pil",), ("o0", "o1", "oa"))
    with pytest.raises(ValueError, match="unknown variant"):
        s.key(D1, "o7", "pil")


@pytest.mark.parametrize("digest", ["nope", "", "ab" * 15, "zz" * 16])
def test_anything_that_is_not_an_md5_is_refused(digest):
    s = audio_scheme("dig")
    with pytest.raises(ValueError, match="md5"):
        s.key(digest, "sr16000", "wav16k")


def test_an_upper_case_digest_is_folded_rather_than_refused():
    """Deliberate, and depended upon: the consumer's lenient key builder
    does NOT fold case, so folding on this side is what keeps the two agreeing
    for the lower-case digests real corpora contain. Asserted here so a change
    that "tightens" it fails loudly instead of silently halving the hit
    rate."""
    s = audio_scheme("fold")
    assert s.key(("AB" * 16), "sr16000", "wav16k").endswith(f"{D1}:sr16000")


def test_an_unknown_form_is_refused():
    s = audio_scheme("form")
    with pytest.raises(ValueError, match="unknown form"):
        s.key(D1, "sr16000", "mp3")


@pytest.mark.parametrize("bad", [
    "nope", "", "other:v1:wav16k:" + D1 + ":sr16000",     # wrong domain
    "bad:v9:wav16k:" + D1 + ":sr16000",                   # wrong version
    "bad:v1:mp3:" + D1 + ":sr16000",                      # unknown form
    "bad:v1:wav16k:zz:sr16000",                           # not a digest
    "bad:v1:wav16k:" + D1 + ":o1",                        # variant off-axis
])
def test_a_malformed_key_never_becomes_a_path(bad):
    """The only thing between a bad key and a path the reaper will unlink."""
    s = audio_scheme("bad")
    with pytest.raises(ValueError):
        s.relpath(bad)


# ------------------------------------------------------------------ registry

def test_parse_dispatches_on_the_domain():
    """One store can hold several domains, so a key must be self-describing."""
    a = register(audio_scheme("dispa"))
    register(KeyScheme("dispb", "v3", ("pil",), ("o1",), {"pil": "uint8"}))

    assert scheme_for(a.key(D1, "sr16000", "wav16k")) is a
    assert parse(f"dispb:v3:pil:{D1}:o1") == ("pil", D1, "o1")
    assert relpath(f"dispb:v3:pil:{D1}:o1") == f"pil/ab/{D1}.o1.npy"


def test_an_unregistered_domain_is_an_error_not_a_guess():
    with pytest.raises(ValueError, match="no registered scheme"):
        parse(f"nobody:v1:pil:{D1}:o1")


def test_registering_the_same_scheme_twice_is_fine():
    """A module binding a scheme at import time may be imported twice."""
    first = register(audio_scheme("idem"))
    assert register(audio_scheme("idem")) is first


def test_two_disagreeing_schemes_for_one_domain_are_refused():
    """The worst failure this design allows: the publisher writes one key and
    the reader asks for another, and the hit rate silently goes to zero with
    no exception and no counter anywhere."""
    register(audio_scheme("clash"))
    with pytest.raises(ValueError, match="disagreeing"):
        register(KeyScheme("clash", "v2", ("wav16k",), r"^sr\d+$"))


# ------------------------------------------------------- the extraction pins

def test_the_pixels_scheme_still_spells_its_keys_exactly_as_before():
    """Byte-for-byte parity with the consumer's key scheme before the move.

    Segments are volatile, so a changed key would only cost one re-decode --
    but the consuming application builds this same string independently on its
    other code path, and if the two stop agreeing the cache quietly never hits
    again.
    """
    pixels = KeyScheme(domain="pixels", version="v3",
                       forms=("pil", "bgr3", "f32chw", "wav16k"),
                       variants=("o0", "o1", "oa"), dtypes={"pil": "uint8"})

    assert pixels.key(D1, "o1", "pil") == f"pixels:v3:pil:{D1}:o1"
    assert pixels.key(D1, "oa", "pil").endswith(":oa")
    assert pixels.relpath(pixels.key(D1, "o1", "pil")) == f"pil/ab/{D1}.o1.npy"
    assert pixels.key(D1, "o1", "pil") != pixels.key(D1, "o1", "bgr3")
    assert pixels.key(D1, "o1", "pil") != pixels.key(D1, "o0", "pil")

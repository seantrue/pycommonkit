"""Addressing: the key for a segment, and the path that key names.

Key shape::

    {domain}:{version}:{form}:{digest}:{variant}

Path shape::

    {root}/{form}/{digest[:2]}/{digest}.{variant}.npy

Two-level sharding like a CAS, so no directory ever holds a million entries.

A :class:`KeyScheme` fixes one domain's vocabulary. The pieces mean:

``domain``
    Who the segments belong to -- ``pixels``, ``audio``. Also the dispatch key:
    schemes register themselves, so :func:`parse` and :func:`relpath` can work
    from a key alone and one store can hold several domains.
``version``
    The DECODE CONTRACT, not a release number. Bump it when what a segment
    contains changes meaning, so old segments become unreachable rather than
    being attached by a reader that assumes the new contract. (``pixels`` is at
    ``v3`` because ``v2`` segments could hold LA/RGBA, which ``v3`` readers
    assume away.)
``form``
    What KIND of array -- ``pil`` uint8 images, ``f32chw`` model tensors,
    ``wav16k`` int16 audio. Deliberately NOT colour order: a BGR view is
    converted from the one stored RGB segment in about a millisecond against a
    38 ms decode, and storing both would double the tree to save that
    millisecond while letting the two drift.
``variant``
    What distinguishes two renderings of the SAME source bytes. This is the
    axis that differs per domain and the reason the scheme is a class rather
    than a constant: images vary by orientation (``o0`` raw, ``o1`` oriented,
    ``oa`` Apple), a waveform would vary by sample rate (``sr16000``).

The form segment precedes the digest so that a domain whose variant suffix is
already load-bearing elsewhere keeps it at the end of the key, untouched.
"""
from __future__ import annotations

import re
from collections.abc import Iterable

#: Segments are addressed by md5, lowercase. Every pattern here is used with
#: ``fullmatch``, never ``match`` with ``$``: ``$`` also matches before a final
#: newline, which let ``"...\n"`` through as a digest or token (contract §3.1).
_DIGEST_RE = re.compile(r"[0-9a-f]{32}")

#: Every other component is a bare lowercase token. Restrictive on purpose:
#: these become PATH components, and a key is the only thing standing between
#: a caller and a path the reaper will later be asked to unlink.
_TOKEN_RE = re.compile(r"[a-z0-9]+")

#: Fold ASCII upper case only (contract §3.3): ``str.lower`` folds far more.
_ASCII_LOWER = str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")


class KeyScheme:
    """One domain's key vocabulary, and the only thing that builds its keys.

    ``variants`` is either an explicit tuple of legal variants or a regex
    string for an open-ended axis (sample rates, say). Explicit is preferred
    where the set is closed: an unknown variant is then a loud error at the
    call site rather than a segment nobody will ever look for.
    """

    def __init__(self, domain: str, version: str, forms: Iterable[str],
                 variants, dtypes: dict[str, str] | None = None):
        for name, value in (("domain", domain), ("version", version)):
            if not _TOKEN_RE.fullmatch(str(value)):
                raise ValueError(f"{name} must be [a-z0-9]+, got {value!r}")
        self.domain = str(domain)
        self.version = str(version)
        self.forms = tuple(forms)
        if not self.forms:
            raise ValueError(f"{self.domain}: at least one form is required")
        for form in self.forms:
            if not _TOKEN_RE.fullmatch(form):
                raise ValueError(f"form must be [a-z0-9]+, got {form!r}")
        if len(set(self.forms)) != len(self.forms):
            raise ValueError(f"{self.domain}: forms must be distinct, got {self.forms}")

        if isinstance(variants, str):
            self.variants: tuple[str, ...] = ()
            self._variant_re = re.compile(variants)
            #: The pattern's text, which is what makes two pattern schemes the
            #: same or different (contract §3.4).
            self.variant_pattern: str | None = variants
        else:
            self.variants = tuple(variants)
            if not self.variants:
                raise ValueError(f"{self.domain}: at least one variant")
            for variant in self.variants:
                if not _TOKEN_RE.fullmatch(variant):
                    raise ValueError(f"variant must be [a-z0-9]+, got {variant!r}")
            self._variant_re = None
            self.variant_pattern = None

        # form -> required numpy dtype NAME, for forms whose consumers rely on
        # one. Declared here rather than branched on inside the store, which
        # would put one domain's policy in the generic layer. A segment that
        # violates it is refused at publish: a bad segment is not one bad call,
        # it is published once and then poisons every reader that attaches it,
        # on a path where a miss is meant to be the worst outcome available.
        self.dtypes = dict(dtypes or {})
        for form in self.dtypes:
            if form not in self.forms:
                raise ValueError(f"{self.domain}: dtype for unknown form {form!r}")

        self._key_re = re.compile(
            rf"{re.escape(self.domain)}:{re.escape(self.version)}:"
            r"(?P<form>[a-z0-9]+):(?P<digest>[0-9a-f]{32}):"
            r"(?P<variant>[a-z0-9]+)")

    # ------------------------------------------------------------- building

    @property
    def default_form(self) -> str:
        return self.forms[0]

    def dtype_for(self, form: str) -> str | None:
        """The dtype ``form`` must have, or None when anything goes."""
        return self.dtypes.get(form)

    def valid_variant(self, variant: str) -> bool:
        """A token, and in the closed set or matched WHOLE by the pattern (contract §3.2).

        ``re.match`` anchored only the start, so ``sr[0-9]+`` accepted ``sr16000x``;
        and a pattern like ``sr.+`` could accept ``sr-1``, which no parser reads back.
        """
        variant = str(variant)
        if not _TOKEN_RE.fullmatch(variant):
            return False
        if self._variant_re is not None:
            return self._variant_re.fullmatch(variant) is not None
        return variant in self.variants

    def key(self, digest: str, variant: str, form: str | None = None) -> str:
        """The store key for one array. Raises on anything it cannot address."""
        digest = str(digest).translate(_ASCII_LOWER)
        if not _DIGEST_RE.fullmatch(digest):
            raise ValueError(f"not an md5 digest: {digest!r}")
        form = self.default_form if form is None else str(form)
        if form not in self.forms:
            raise ValueError(
                f"{self.domain}: unknown form {form!r}; have {self.forms}")
        variant = str(variant)
        if not self.valid_variant(variant):
            raise ValueError(f"{self.domain}: unknown variant {variant!r}")
        return f"{self.domain}:{self.version}:{form}:{digest}:{variant}"

    # -------------------------------------------------------------- reading

    def parse(self, key: str) -> tuple[str, str, str]:
        """``key -> (form, digest, variant)``. Raises on anything unparseable.

        Strict on purpose: this is the only thing between a malformed key and
        a path the reaper might later unlink.
        """
        m = self._key_re.fullmatch(str(key))
        if m is None:
            raise ValueError(f"unparseable {self.domain} key: {key!r}")
        form, variant = m.group("form"), m.group("variant")
        if form not in self.forms or not self.valid_variant(variant):
            raise ValueError(f"unparseable {self.domain} key: {key!r}")
        return form, m.group("digest"), variant

    def relpath(self, key: str) -> str:
        """``key -> "{form}/{2-hex}/{digest}.{variant}.npy"``, never absolute."""
        form, digest, variant = self.parse(key)
        return f"{form}/{digest[:2]}/{digest}.{variant}.npy"

    def __repr__(self) -> str:
        return f"<KeyScheme {self.domain}:{self.version} forms={self.forms}>"


# ------------------------------------------------------------------ registry
#
# Dispatch by domain is what lets ONE store hold several domains: the store
# parses a key without being told which scheme it belongs to, so it never has
# to grow a parameter for something the key already says.

_REGISTRY: dict[str, KeyScheme] = {}


def register(scheme: KeyScheme) -> KeyScheme:
    """Register ``scheme``, refusing a second, different one for its domain.

    Idempotent for an identical re-registration, because a module that binds a
    scheme at import time may legitimately be imported twice under different
    names. A DISAGREEING second registration is the bug worth catching: two
    spellings of one domain means the publisher writes one key and the reader
    asks for another, and the hit rate silently goes to zero with no error.
    """
    prior = _REGISTRY.get(scheme.domain)
    if prior is not None:
        same = (prior.version == scheme.version
                and prior.forms == scheme.forms
                # A closed variant SET (contract §3.4): order carries no meaning there,
                # unlike forms, whose first is the default.
                and set(prior.variants) == set(scheme.variants)
                and prior.variant_pattern == scheme.variant_pattern
                and prior.dtypes == scheme.dtypes)
        if not same:
            raise ValueError(
                f"two disagreeing schemes for domain {scheme.domain!r}")
        return prior
    _REGISTRY[scheme.domain] = scheme
    return scheme


def scheme_for(key: str) -> KeyScheme:
    """The registered scheme that owns ``key``."""
    domain = str(key).split(":", 1)[0]
    scheme = _REGISTRY.get(domain)
    if scheme is None:
        raise ValueError(f"no registered scheme for key: {key!r}")
    return scheme


def parse(key: str) -> tuple[str, str, str]:
    """``key -> (form, digest, variant)``, dispatching on the key's domain."""
    return scheme_for(key).parse(key)


def relpath(key: str) -> str:
    """``key -> relative segment path``, dispatching on the key's domain."""
    return scheme_for(key).relpath(key)

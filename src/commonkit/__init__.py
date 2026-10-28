"""Named shared-memory segments for numpy arrays.

The name is for Fortran's ``COMMON`` block: a named region several separately
compiled programs agree to share. The name is the whole interface, and the
segment outlives whoever wrote it.

``store`` is the mechanism and carries the measured reasons behind it, ``keys``
the addressing, ``cache`` the in-process memo that keeps a tight loop from
paying an attach per call.
"""
from .cache import TieredPixelCache
from .keys import KeyScheme, parse, register, relpath, scheme_for
from .shared import shared_store
from .store import SharedArrayStore

__all__ = [
    "KeyScheme", "SharedArrayStore", "TieredPixelCache",
    "parse", "register", "relpath", "scheme_for", "shared_store",
]

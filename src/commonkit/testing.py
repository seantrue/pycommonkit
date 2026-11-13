"""Test helpers for every suite that uses commonkit (D25 E, contract §11).

A suite runs inside a run folder named by ``COMMONKIT_TEST_RUN``, with a fresh
``COMMONKIT_STORE_ROOT`` and ``COMMONKIT_MARK_DIR`` in it and ``COMMONKIT_HASH`` set to
the algorithm under test, never inherited from a shell (whose root may be a live tree).

* :func:`assert_isolated` refuses to let a suite run otherwise. It reads nothing in the
  root, so every test and test binary of a run gets the same answer in any order.
* :func:`fresh_root` makes an empty root inside the run.
* :func:`expect_marks` sends a store's markers to a scratch folder for its scope, for tests
  that publish or attach a bad or foreign key on purpose, and asserts on exactly those.
"""
from __future__ import annotations

import contextlib
import os
import tempfile

from . import policy as _policy

RUN_ENV = "COMMONKIT_TEST_RUN"


def _real(p: str) -> str:
    return os.path.realpath(os.path.abspath(os.path.expanduser(p)))


def _inside(child: str, parent: str) -> bool:
    child, parent = _real(child), _real(parent)
    return child == parent or child.startswith(parent.rstrip(os.sep) + os.sep)


def assert_isolated() -> None:
    """Fail unless ``COMMONKIT_STORE_ROOT`` is set, isn't (and doesn't contain) the platform
    default root, and resolves inside ``COMMONKIT_TEST_RUN``, which is set."""
    root = os.environ.get(_policy.ENV_PREFIX + "ROOT", "")
    run = os.environ.get(RUN_ENV, "")
    # Raised, not asserted: ``python -O`` strips assert statements, and this guard must hold.
    if not root:
        raise AssertionError("COMMONKIT_STORE_ROOT is unset: run the suite on a fresh root "
                             "(D25 E)")
    default = _policy.default_root()
    if _inside(default, root):
        raise AssertionError(f"the store root {root} is or holds the default root")
    if not run:
        raise AssertionError(f"{RUN_ENV} is unset: the harness names the run folder the root "
                             "must be in")
    if not _inside(root, run):
        raise AssertionError(f"the store root {root} is not inside {RUN_ENV}={run}")


def fresh_root() -> str:
    """A new, empty store root inside the run folder (or the system temp folder)."""
    base = os.environ.get(RUN_ENV) or None
    return tempfile.mkdtemp(prefix="root-", dir=base)


@contextlib.contextmanager
def expect_marks(store, counters):
    """Within the block, ``store`` marks into a scratch folder; on exit exactly the named
    counters' markers (``shm_bad_key``, ``shm_foreign_digest``) must be there."""
    saved = store._mark_dir
    with tempfile.TemporaryDirectory(prefix="marks-",
                                     dir=os.environ.get(RUN_ENV) or None) as folder:
        store._mark_dir = folder
        try:
            yield folder
        finally:
            store._mark_dir = saved
        got = sorted(name.split(".", 1)[1] for name in os.listdir(folder))
        if got != sorted(counters):
            raise AssertionError(f"markers {got}, expected {sorted(counters)}")

#!/usr/bin/env python3
"""The explicit edition images the Python tests can run against.

The tests are not run against "whatever ``.so`` files happen to be in
``python/``". Each run names the image it tests, and ``install`` makes the
modules that image does not contain unimportable. A module that needs the
kernel therefore fails or skips *by construction* in the app image instead of
passing on a developer checkout that happens to have ``ferrocad_combined``.

Images:

``app``
    The primary ``ferrocad`` extension alone, which is what CI's ``headless``
    job builds. ``ferrocad_combined``, ``Part``, ``Mesh`` and ``MeshPart`` are
    hidden.

``combined``
    App + Part + Mesh in one extension (``ferrocad_combined``). Nothing is
    hidden.

``tools/api_docs.py`` is the other user: it generates the reference for the
base facade, so it installs ``app`` and gets the same pages whether or not a
developer has built the workbenches.
"""

from __future__ import annotations

import os
import sys

#: Set by :func:`install`; inherited by tests that spawn their own interpreter,
#: which lets them ask what image they are part of instead of probing the disk.
ENV = "FERROCAD_TEST_IMAGE"

#: The modules each image does *not* contain. Hiding a top-level name also
#: hides its submodules (the finder matches ``fullname.partition(".")[0]``).
IMAGES: dict[str, frozenset[str]] = {
    "app": frozenset({"ferrocad_combined", "Part", "Mesh", "MeshPart"}),
    "combined": frozenset(),
}


class _HiddenModules:
    """A meta-path finder that makes the image's excluded modules unimportable."""

    def __init__(self, image: str, hidden: frozenset[str]) -> None:
        self.image = image
        self.hidden = hidden

    def find_spec(self, fullname, path=None, target=None):
        if fullname.partition(".")[0] in self.hidden:
            raise ModuleNotFoundError(
                f"{fullname} is not in the {self.image!r} test image; "
                "run ./build.sh for the combined extension",
                name=fullname,
            )
        return None

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"<test image {self.image!r}: hiding {sorted(self.hidden)}>"


def excluded(image: str) -> frozenset[str]:
    """The top-level modules ``image`` does not contain.

    An unknown image name is an error: silently treating it as ``combined``
    would test the wrong artifact and report success.
    """

    try:
        return IMAGES[image]
    except KeyError:
        known = ", ".join(sorted(IMAGES))
        raise ValueError(
            f"unknown test image {image!r}; known images: {known}"
        ) from None


def active() -> str | None:
    """The image of the current process, or ``None`` when run outside the runner."""

    return os.environ.get(ENV)


def install(image: str) -> None:
    """Make the modules ``image`` does not contain unimportable in this process.

    Call this before importing (or discovering) anything under test. Import
    caches are per process, so a run that tests more than one image needs a
    fresh interpreter for each. The image name is also exported in ``ENV`` so a
    test that starts a child interpreter can honour the same image.
    """

    hidden = excluded(image)
    os.environ[ENV] = image
    # The image is authoritative: a later call replaces an earlier image's
    # finder instead of leaving both installed. (Import caches are still per
    # process, so a real multi-image run needs a fresh interpreter.)
    sys.meta_path[:] = [
        finder for finder in sys.meta_path if not isinstance(finder, _HiddenModules)
    ]
    if hidden:
        sys.meta_path.insert(0, _HiddenModules(image, hidden))

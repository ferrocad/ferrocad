#!/usr/bin/env python3
"""The extension sets a Python test run can be given.

A run is not tested against "whatever ``.so`` files happen to be in
``python/``". It names the **extension set** it is provided: the workbench
modules that must be importable. ``install`` makes every other workbench module
unimportable, so a test that needs the kernel fails or skips *by construction*
in the base set instead of passing on a developer checkout that has built
``ferrocad_combined``.

The polarity is deliberate: callers pass what is **provided** and this module
hides the rest. A typo in a hide-list would hide nothing and run green against
the wrong build; a typo in a provide-list is a :class:`ValueError`.

``Part``, ``Mesh`` and ``MeshPart`` are shims over ``ferrocad_combined``, so
providing one of them also makes that packaging importable: hiding it would make
the very module that was provided unimportable. The packaging is not itself a
workbench module, so it is not a ``--extension`` value — it is importable exactly
when a shim it implements is provided.

An **edition** is the shipped bundle: binary, app script, defaults, branding and
workbench set. An **extension set** is the test-side view of its workbench half:
a set of names, not a file — and not a packaging either, because
``ferrocad_combined`` is the packaging ``ferrocad_parametric`` also uses.

Presets:

``base``
    The ``ferrocad`` facade alone, which is what CI's ``headless`` job builds.
    Every workbench module is hidden.

``workbenches``
    Every workbench module, with the packaging ``ferrocad_combined`` they are
    built on. Nothing hidden.

``tools/api_docs.py`` is the other user: it generates the reference for the base
facade, so it installs ``base`` and gets the same pages whether or not a
developer has built the workbenches.
"""

from __future__ import annotations

import os
import sys

#: The workbench modules an extension can provide, in a stable order.
WORKBENCH_MODULES = ("Part", "Mesh", "MeshPart")

#: The packaging extension those modules are shims over. Not a workbench module, so
#: not a ``--extension`` value; importable exactly when a shim it implements is
#: provided.
_PACKAGING = "ferrocad_combined"

#: Every module :func:`install` can hide: the workbench modules and the packaging.
_HIDEABLE = frozenset(WORKBENCH_MODULES) | {_PACKAGING}

#: The shims that ``ferrocad_combined`` implements.
_SHIM_MODULES = frozenset(WORKBENCH_MODULES)

#: Named presets, keyed by the set of workbench modules they **provide**.
PRESETS: dict[str, frozenset[str]] = {
    "base": frozenset(),
    "workbenches": frozenset(WORKBENCH_MODULES),
}

#: Set by :func:`install`; inherited by tests that spawn their own interpreter,
#: which lets them ask which extension set they are part of instead of probing
#: the disk. Comma-joined module names; ``""`` is the empty (base) set.
ENV = "FERROCAD_TEST_EXTENSIONS"


class _HiddenModules:
    """A meta-path finder that makes an extension set's absent modules unimportable."""

    def __init__(self, provided: frozenset[str], hidden: frozenset[str]) -> None:
        self.provided = provided
        self.hidden = hidden

    def find_spec(self, fullname, path=None, target=None):
        if fullname.partition(".")[0] in self.hidden:
            available = ", ".join(sorted(self.provided)) or "nothing (base set)"
            raise ModuleNotFoundError(
                f"{fullname} is not in this test extension set; provided: "
                f"{available}. Run ./build.sh to build ferrocad_combined, which "
                "provides Part, Mesh and MeshPart",
                name=fullname,
            )
        return None

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return (
            f"<extension set providing {sorted(self.provided)}: "
            f"hiding {sorted(self.hidden)}>"
        )


def provided(extensions) -> frozenset[str]:
    """``extensions`` after validating every name against :data:`WORKBENCH_MODULES`.

    An unknown name is an error: treating a typo as the empty set would hide
    nothing and test the wrong build.
    """

    unknown = sorted(set(extensions) - set(WORKBENCH_MODULES))
    if unknown:
        raise ValueError(
            f"unknown extension(s) {', '.join(unknown)}; "
            f"known extensions: {', '.join(WORKBENCH_MODULES)}"
        )
    return frozenset(extensions)


def _importable(extensions: frozenset[str]) -> frozenset[str]:
    """``extensions`` plus the packaging if a shim it implements is provided."""

    if extensions & _SHIM_MODULES:
        return extensions | {_PACKAGING}
    return extensions


def hidden(extensions) -> frozenset[str]:
    """Every module :func:`install` would make unimportable for ``extensions``.

    This includes the packaging when no shim is provided: ``base`` must fail an
    ``import ferrocad_combined`` as well as an ``import Part``, or a test could
    reach the kernel through the packaging and pass on a developer checkout.
    """

    return _HIDEABLE - _importable(provided(extensions))


def preset(name: str) -> frozenset[str]:
    """The module set the preset ``name`` provides; an unknown name is an error."""

    try:
        return PRESETS[name]
    except KeyError:
        known = ", ".join(sorted(PRESETS))
        raise ValueError(
            f"unknown extension preset {name!r}; known presets: {known}"
        ) from None


def active() -> frozenset[str] | None:
    """The extension set of this process, or ``None`` outside the runner."""

    value = os.environ.get(ENV)
    if value is None:
        return None
    return frozenset(name for name in value.split(",") if name)


def install(extensions) -> None:
    """Make the workbench modules ``extensions`` does not provide unimportable.

    Call this before importing (or discovering) anything under test. Import
    caches are per process, so a run that tests more than one set needs a fresh
    interpreter for each. The set is also exported in ``ENV`` so a test that
    starts a child interpreter can honour the same set.
    """

    extensions = provided(extensions)
    importable = _importable(extensions)
    absent = _HIDEABLE - importable
    os.environ[ENV] = ",".join(sorted(extensions))
    sys.meta_path[:] = [
        finder for finder in sys.meta_path if not isinstance(finder, _HiddenModules)
    ]
    if absent:
        sys.meta_path.insert(0, _HiddenModules(importable, absent))

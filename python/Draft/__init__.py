"""A minimal, pure-Python ``Draft`` compatibility shim.

FerroCAD does **not** ship the Draft workbench. Upstream Part's
``parttests/TestPartMirror.py`` does ``import Draft`` and calls
``Draft.make_clone(sketch)``; ``src/Mod/Part/TestPartApp.py`` imports that test
file at module scope, so without an importable ``Draft`` *neither* file loads
and TestPartApp's tests never run. This package exists to make those imports
resolve against the one Draft operation they actually use.

What this module implements
---------------------------

``make_clone(source, delta=None, forcedraft=False)``
    Add an ``App::Link`` to the active document, point its ``LinkedObject`` at
    ``source``, and start it at the source's ``Placement`` (matching upstream
    ``Draft.make_clone``). ``delta`` offsets the clone's ``Placement.Base``.

    FerroCAD's ``App::Link`` is the existing object type that already means
    "live copy of another object's ``Shape``, transformed by a ``Placement``":
    its ``execute`` reads the linked object's ``Shape`` and bakes its own
    ``Placement`` into the derived ``Shape`` on every recompute. That is
    exactly the behaviour the mirror test needs, so this shim reuses it
    instead of introducing a private object type. ``make_clone`` recomputes
    afterwards so the returned object already has a ``Shape``.

What this module does **not** implement
---------------------------------------

Everything else in Draft: no GUI, no creation/editing tools, no importers, no
2D-clone special-casing, no Arch/BIM clone modes (``forcedraft`` is accepted
only for signature compatibility). A name outside ``__all__`` raises
``AttributeError`` naming this module and the missing workbench surface; it is
never silently replaced by a placeholder. The full workbench remains upstream
Python, shadowed here only for the headless Part tests.

Notes on the ``App::Link`` reuse
--------------------------------

* ``Shape`` is an *output* property of ``App::Link`` (derived by ``execute``,
  not writable), so ``make_clone`` recomputes before returning rather than
  assigning ``Shape``.
* ``App::Link`` applies its ``Placement`` to the linked object's ``Shape``.
  Like upstream ``Draft.make_clone`` the clone therefore starts at the
  source's ``Placement``. Cloning an object whose ``Shape`` is already in
  world coordinates (another ``App::Link``, for example) would compose that
  placement twice; the Part tests do not do this.
"""

from __future__ import annotations

import FreeCAD as App

__all__ = ["make_clone"]

#: The FerroCAD object type used to represent a Draft clone.
_CLONE_TYPE = "App::Link"
#: The default name upstream's ``Draft.make_clone`` gives the new object.
_CLONE_NAME = "Clone"


def make_clone(source, delta=None, forcedraft=False):
    """Clone ``source`` into the active document as an ``App::Link``.

    Parameters
    ----------
    source :
        A document object. A one-element list/tuple is accepted, as upstream
        accepts ``clone(obj)`` and ``clone([obj])``. Cloning several objects
        at once is not implemented here and raises ``NotImplementedError``.
    delta :
        Optional ``Base.Vector`` added to the clone's ``Placement.Base``.
    forcedraft :
        Accepted for signature compatibility with upstream and ignored; there
        are no Arch/BIM clone modes in FerroCAD.

    Returns
    -------
    The new ``App::Link`` document object, which owns a ``Shape`` derived from
    ``source`` and a settable ``Placement``. A later ``doc.recompute()`` keeps
    ``Shape`` consistent with ``Placement``.
    """
    del forcedraft  # no Arch/BIM clone modes to force Draft over

    if isinstance(source, (list, tuple)):
        if len(source) != 1:
            raise NotImplementedError(
                "Draft.make_clone clones exactly one object in FerroCAD; "
                f"got {len(source)} objects"
            )
        source = source[0]

    doc = App.ActiveDocument
    if doc is None:
        raise RuntimeError(
            "Draft.make_clone needs an active document; "
            "call FreeCAD.newDocument() first"
        )

    clone = doc.addObject(_CLONE_TYPE, _CLONE_NAME)
    clone.LinkedObject = source

    # Match upstream: the clone starts at the source's placement.
    placement = getattr(source, "Placement", None)
    if placement is not None:
        clone.Placement = placement
    if delta is not None:
        # ``Base`` is a live view of the placement property; assigning the
        # summed vector writes the moved placement back. This also works when
        # the source has no ``Placement``, where the clone starts at identity.
        clone.Placement.Base = clone.Placement.Base + delta

    # ``App::Link.Shape`` is an output property, so execute it once now: the
    # returned object then really has the copied shape, not an empty output.
    doc.recompute()
    return clone


def __getattr__(name):
    raise AttributeError(
        f"module {__name__!r} is a compatibility shim, not the Draft "
        f"workbench; it implements only {{{', '.join(__all__)}}}"
    )

"""The FerroCAD-only Python API (the extensions upstream FreeCAD does not have).

A mod that imports ``FerroCAD`` declares a FerroCAD dependency; a mod that only
imports ``FreeCAD``/``FreeCADGui`` is written against the upstream surface and
should run there unchanged. The boundary is deliberate and has no aliases: the
names here are **not** available on ``FreeCAD``. See
``docs/developer/architecture/api-namespaces.md``.

This module holds the app-side extension points:

* :data:`Grid` — the semantic grid-settings singleton;
* :data:`Axis` — the semantic axis-cross settings singleton;
* :data:`Gizmo` — the semantic manipulator-gizmo settings singleton;
* :func:`register_display_provider` / :func:`display_mesh` — how a module draws
  its objects;
* :func:`register_interaction_provider` / :func:`interaction` — how a module
  applies a manipulator's commit;
* :func:`register_manipulator_provider` / :func:`manipulator_specs` — how a module
  declares the handles a selection arms;
* :func:`set_parameter_file` / :func:`parameter_file` — which file backs
  ``FreeCAD.ParamGet`` (see ``docs/project/record/tasks/settings-model.md`` S3);
* the **document lifecycle aliases** — ``newDocument``, ``openDocument`` /
  ``open``, ``closeDocument`` and ``setActiveDocument`` delegate to ``FreeCAD``,
  which fires the ``FerroCADGui`` document-observer slot (deactivation happens
  before the document disappears). They are not exported in ``__all__``: their
  names are upstream-shaped, and the slot they reach is the FerroCAD extension.

The document registry, the base value types and the parameters stay on
``FreeCAD``: those are upstream-shaped.
"""

from __future__ import annotations

import sys

import FreeCAD as _FreeCAD

# The process-wide core identity, re-exported for parity with `FreeCAD`.
_kernel_id = _FreeCAD._kernel_id

from ._params import parameter_file, set_parameter_file

# ---------------------------------------------------------------------------
# The semantic capabilities (see FerroCAD._grid, _axis and _gizmo)
# ---------------------------------------------------------------------------


def __getattr__(name):
    # PEP 562: bound lazily so importing FerroCAD stays headless and GUI-free, and
    # so a build without a capability is detected through
    # ``getattr(..., "Grid", None)`` / ``getattr(..., "Axis", None)`` /
    # ``getattr(..., "Gizmo", None)``.
    if name == "Grid":
        from . import _grid

        return _grid.grid()
    if name == "Axis":
        from . import _axis

        return _axis.axis()
    if name == "Gizmo":
        from . import _gizmo

        return _gizmo.gizmo()
    raise AttributeError("module 'FerroCAD' has no attribute '%s'" % name)


# ---------------------------------------------------------------------------
# Module hooks: display and interaction providers
# ---------------------------------------------------------------------------
# A workbench registers how **its** objects are drawn and how a manipulator's
# commit is applied, so the shell never names a module (the shell must serve any
# edition). Upstream puts this on `FreeCADGui` (one `ViewProvider` per object,
# plus commands); until the M5 GUI module exists these two hooks are the minimal
# equivalent. A provider returns `None` for a request it does not own, and the
# next provider is asked.

_DISPLAY_PROVIDERS = []
_INTERACTION_PROVIDERS = []
_MANIPULATOR_PROVIDERS = []


def register_display_provider(provider):
    """Let ``provider(obj)`` supply the display mesh of a document object.

    The provider returns the tessellation dict of `ferrocad.geom.Tessellation`
    (``positions``, ``triangles``, ``edges``, ``parts``, …) or ``None``. Modules
    register once, when they are imported; registration is idempotent.
    """
    if provider not in _DISPLAY_PROVIDERS:
        _DISPLAY_PROVIDERS.append(provider)


def display_mesh(obj):
    """The display mesh for `obj`, from the first module that claims it."""
    for provider in _DISPLAY_PROVIDERS:
        mesh = provider(obj)
        if mesh is not None:
            return mesh
    return None


def register_interaction_provider(provider):
    """Let ``provider(obj, manipulator, action, element, values)`` apply an edit.

    ``manipulator`` is the registry id of the manipulator that committed
    (``"dress_up"``), ``action`` the operation within it (``"fillet"``,
    ``"chamfer"``), ``element`` the sub-element name (or ``""``), and ``values``
    the numbers in the manipulator's documented order. The provider returns a dict
    ``{"ok": bool, ...}`` (or ``None`` when it does not own the manipulator). The
    edit itself — creating the feature, wrapping it in a transaction, recomputing —
    belongs to the module, not to the shell.
    """
    if provider not in _INTERACTION_PROVIDERS:
        _INTERACTION_PROVIDERS.append(provider)


def interaction(obj, manipulator, action, element="", values=None):
    """Apply a manipulator commit through the first module that supports it."""
    values = list(values or [])
    for provider in _INTERACTION_PROVIDERS:
        result = provider(obj, manipulator, action, element, values)
        if result is not None:
            return result
    return None


def register_manipulator_provider(provider):
    """Let ``provider(obj, element)`` declare handles for a selection.

    The callback is called **when a selection is armed**, so it must be cheap:
    return a list of declarative handle dicts (the same shape the viewport's
    transport validates in ``crates/ferrocad_gpui/src/manipulators.rs``), or
    ``None``/``[]`` when the module has no handle for this object. The provider
    must not mutate the document or raise; a raised exception is reported and
    the other providers still run. Registration is idempotent.

    The commit half is :func:`register_interaction_provider`: when the user
    drags the handle, its ``action``, ``element`` and value arrive there as
    ``manipulator``, ``action``, ``element`` and ``values``.
    """
    if provider not in _MANIPULATOR_PROVIDERS:
        _MANIPULATOR_PROVIDERS.append(provider)


def manipulator_specs(obj, element=""):
    """The declarative handles every module declares for ``obj``, in order.

    Each provider is called as ``provider(obj, element)``. A provider that
    returns ``None`` or ``[]`` contributes nothing. A provider that raises is
    reported once to stderr and skipped, so one broken mod cannot break the
    shell's selection path; the remaining providers still run.
    """
    specs = []
    for provider in _MANIPULATOR_PROVIDERS:
        try:
            result = provider(obj, element)
        except Exception as exc:  # noqa: BLE001 - a mod must not break selection
            name = getattr(provider, "__name__", repr(provider))
            print(
                "FerroCAD: manipulator provider %s raised: %s" % (name, exc),
                file=sys.stderr,
            )
            continue
        if result is None:
            continue
        if not isinstance(result, (list, tuple)):
            name = getattr(provider, "__name__", repr(provider))
            print(
                "FerroCAD: manipulator provider %s returned %s, expected a list"
                % (name, type(result).__name__),
                file=sys.stderr,
            )
            continue
        specs.extend(result)
    return list(specs)


# ---------------------------------------------------------------------------
# Document lifecycle
#
# ``FreeCAD.newDocument`` / ``openDocument`` / ``closeDocument`` /
# ``setActiveDocument`` are the upstream-shaped calls and own the lifecycle: the
# App facade fires the ``FerroCADGui`` document-observer slot (without importing
# the GUI layer in a headless session). These names stay as the app-side aliases
# scripts and the shell already use; only ``closeDocument`` differs, returning
# whether the document was open where ``FreeCAD`` raises. They are not in
# ``__all__`` because the names are upstream-shaped and ``FerroCADGui`` owns the
# extension; see ``docs/developer/architecture/api-namespaces.md``.
# ---------------------------------------------------------------------------


def newDocument(name=None, hidden=False, temp=False):
    """Alias for :func:`FreeCAD.newDocument` (the lifecycle lives there)."""
    return _FreeCAD.newDocument(name, hidden, temp)


def openDocument(path, hidden=False, temporary=False):
    """Alias for :func:`FreeCAD.openDocument`."""
    return _FreeCAD.openDocument(path, hidden, temporary)


def open(name, hidden=False, temporary=False):
    """Upstream's ``FreeCAD.open`` alias, with the same lifecycle."""
    return openDocument(name, hidden, temporary)


def closeDocument(name):
    """Close a document; ``False`` when it was not open (``FreeCAD`` raises)."""
    if _FreeCAD.getDocument(name) is None:
        return False
    _FreeCAD.closeDocument(name)
    return True


def setActiveDocument(name):
    """Alias for :func:`FreeCAD.setActiveDocument`."""
    _FreeCAD.setActiveDocument(name)


__all__ = [
    "Grid",
    "Axis",
    "Gizmo",
    "register_display_provider",
    "display_mesh",
    "register_interaction_provider",
    "interaction",
    "register_manipulator_provider",
    "manipulator_specs",
    "set_parameter_file",
    "parameter_file",
]

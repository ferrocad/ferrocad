"""The FerroCAD-only Python API (the extensions upstream FreeCAD does not have).

A mod that imports ``FerroCAD`` declares a FerroCAD dependency; a mod that only
imports ``FreeCAD``/``FreeCADGui`` is written against the upstream surface and
should run there unchanged. The boundary is deliberate and has no aliases: the
names here are **not** available on ``FreeCAD``. See
``docs/architecture/api-namespaces.md``.

This module holds the app-side extension points:

* :data:`Grid` — the semantic grid-settings singleton;
* :func:`register_display_provider` / :func:`display_mesh` — how a module draws
  its objects;
* :func:`register_interaction_provider` / :func:`interaction` — how a module
  applies a manipulator's commit;
* :func:`set_parameter_file` / :func:`parameter_file` — which file backs
  ``FreeCAD.ParamGet`` (see ``docs/tasks/settings-model.md`` S3).

The document registry, the base value types and the parameters stay on
``FreeCAD``: those are upstream-shaped.
"""

from __future__ import annotations

from ._params import parameter_file, set_parameter_file

# ---------------------------------------------------------------------------
# The semantic grid (see FerroCAD._grid)
# ---------------------------------------------------------------------------


def __getattr__(name):
    # PEP 562: bound lazily so importing FerroCAD stays headless and GUI-free, and
    # so a build without the grid is detected through ``getattr(..., "Grid", None)``.
    if name == "Grid":
        from . import _grid

        return _grid.grid()
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


__all__ = [
    "Grid",
    "register_display_provider",
    "display_mesh",
    "register_interaction_provider",
    "interaction",
    "set_parameter_file",
    "parameter_file",
]

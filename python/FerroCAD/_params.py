"""The file backing of ``FreeCAD.ParamGet`` (a FerroCAD-only control).

Upstream FreeCAD keeps parameters in a platform-specific preferences file; this
module names **which** file FerroCAD uses and lets a caller point it elsewhere.
``ParamGet``/``ParameterGrp`` themselves stay on ``FreeCAD`` because their shape
is upstream's. See ``docs/architecture/api-namespaces.md``.

The default resolution is:

1. the path passed to :func:`set_parameter_file`;
2. ``FERROCAD_PARAM_FILE``;
3. ``$XDG_CONFIG_HOME/ferrocad/parameters.json`` (falling back to
   ``~/.config/ferrocad/parameters.json``), enabled only when
   ``FreeCAD.GuiUp`` is truthy.

Headless processes therefore stay in memory unless a path is set explicitly.
The file is versioned JSON, loaded lazily by the first ``ParamGet`` after a path
is configured, and replaced atomically on every mutation.
"""

from __future__ import annotations

import FreeCAD


def set_parameter_file(path):
    """Point ``FreeCAD.ParamGet`` at ``path`` (always enabled).

    ``path`` is normally a JSON file; its parent directory is created on the
    first write. Pass ``None`` to clear the explicit path and fall back to
    ``FERROCAD_PARAM_FILE`` / the GUI-only default.

    The file is read lazily on the next ``ParamGet``. Calling this again with
    the same path re-arms that load, which is how a restart is simulated.
    """
    FreeCAD._configure_parameter_file(path)


def parameter_file():
    """The parameter file in use, or ``None`` while the store is memory-only."""
    return FreeCAD._parameter_file_path()


__all__ = ["set_parameter_file", "parameter_file"]

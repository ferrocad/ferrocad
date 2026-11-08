"""The `Mesh` module, from the combined FerroCAD extension.

See ``Part.py``: ``ferrocad_combined.abi3.so`` builds ``ferrocad``, ``Part``,
``Mesh`` and ``MeshPart`` from one `.so` so they share one document registry. A host
that linked its own core must not import it (see ``_shim.py``).
"""

import sys

import _shim

if _shim.core_is_builtin():
    raise ImportError(
        "Mesh is not linked in this host; the combined extension would replace its "
        "built-in ferrocad core"
    )

try:
    import ferrocad_combined  # noqa: F401  publishes sys.modules["Mesh"]
except ImportError as error:  # pragma: no cover - depends on the build
    raise ImportError(
        "Mesh needs the combined ferrocad_combined extension; run ./build.sh"
    ) from error

sys.modules[__name__] = sys.modules["Mesh"]

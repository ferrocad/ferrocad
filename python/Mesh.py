"""The `Mesh` module, from the combined FerroCAD image.

See ``Part.py``: ``ferrocad_image.abi3.so`` builds ``ferrocad``, ``Part``,
``Mesh`` and ``MeshPart`` from one `.so` so they share one document registry.
"""

import sys

try:
    import ferrocad_image  # noqa: F401  publishes sys.modules["Mesh"]
except ImportError as error:  # pragma: no cover - depends on the build
    raise ImportError(
        "Mesh needs the combined ferrocad_image extension; run ./build.sh"
    ) from error

sys.modules[__name__] = sys.modules["Mesh"]

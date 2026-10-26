"""The `MeshPart` module, from the combined FerroCAD extension.

See ``Part.py``: ``ferrocad_combined.abi3.so`` builds ``ferrocad``, ``Part``,
``Mesh`` and ``MeshPart`` from one `.so` so they share one document registry.
"""

import sys

try:
    import ferrocad_combined  # noqa: F401  publishes sys.modules["MeshPart"]
except ImportError as error:  # pragma: no cover - depends on the build
    raise ImportError(
        "MeshPart needs the combined ferrocad_combined extension; run ./build.sh"
    ) from error

sys.modules[__name__] = sys.modules["MeshPart"]

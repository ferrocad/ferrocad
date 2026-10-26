"""The `Part` module, from the combined FerroCAD image.

`ferrocad_image.abi3.so` links `ferrocad`, `Part`, `Mesh` and `MeshPart` into one
extension, so they share a single document registry and a single geometry core.
Its module init publishes the four real module objects in ``sys.modules``; this
file only makes a plain ``import Part`` reach the published ``Part``.

Separate ``Part.abi3.so`` builds cannot share that registry (each extension links
its own core), so this shim is the supported host-interpreter entry point. The app
binaries register the same four modules as built-ins and never reach this file.
"""

import sys

try:
    import ferrocad_image  # noqa: F401  publishes sys.modules["Part"]
except ImportError as error:  # pragma: no cover - depends on the build
    raise ImportError(
        "Part needs the combined ferrocad_image extension; run ./build.sh"
    ) from error

sys.modules[__name__] = sys.modules["Part"]

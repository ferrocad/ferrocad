"""The packaged modules: ``import Part`` / ``Mesh`` / ``FreeCAD`` from ``python/``.

The Rust conformance harness boots the modules as built-ins, which hides whether
the *installed* modules can be imported at all. This test starts a plain host
CPython with ``PYTHONPATH=python`` — exactly what a user does — and checks that the
shims reach the combined ``ferrocad_combined`` extension and that the four modules
share one document registry.

It belongs to the ``workbenches`` suite in ``contracts/extension-sets.json``:
the packaged modules only exist when the workbench modules are provided, so an
``--extension base`` run does not run this module at all. Build it with
``./build.sh`` (needs OCCT) or, for a quick local check::

    cargo build -p ferrocad_combined_py --features ferrocad_combined_py/extension-module
    cp target/debug/libferrocad_combined_py.so python/ferrocad_combined.abi3.so
"""

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path

PYTHON_DIR = Path(__file__).resolve().parents[1] / "python"
REPO = PYTHON_DIR.parent
TOOLS = REPO / "tools"
# The extension-set model, not the presence of a `.so`, decides whether this
# module runs.
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))
import extensions  # noqa: E402

# The extension links OCCT, which ships no RUNPATH. The app launcher sets this
# from the packaged `lib/`; a development checkout has the prefix in `.occt/`.
OCCT_LIB = REPO / ".occt" / "prefix" / "lib"

SHARED_CORE = r"""
# Import Part first: its shim imports the combined extension, which publishes
# `ferrocad` into sys.modules. Importing `ferrocad` first would pick up a
# standalone python/ferrocad.abi3.so and create a second core.
import Part
import ferrocad as App
import Mesh
import MeshPart

assert App._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id, (
    "the modules do not share one kernel: %r != %r != %r != %r"
    % (App._kernel_id, Part._kernel_id, Mesh._kernel_id, MeshPart._kernel_id)
)

name = "PackagedImage"
if name in App.listDocuments():
    App.closeDocument(name)
doc = App.newDocument(name)

obj = doc.addObject("Part::Feature", "Box")
shape = Part.makeBox(2, 3, 4)
obj.Shape = shape
assert isinstance(obj.Shape, Part.Solid), type(obj.Shape)
assert obj.Shape.isSame(shape), "Part and App disagree on the shape"

mesh_obj = doc.addObject("Mesh::Feature", "Mesh")
mesh_obj.Mesh = Mesh.createBox(1, 1, 1)
assert type(mesh_obj.Mesh) is Mesh.Mesh, type(mesh_obj.Mesh)
assert mesh_obj.Mesh.CountFacets == 12, mesh_obj.Mesh.CountFacets

built = MeshPart.meshFromShape(shape)
assert type(built) is Mesh.Mesh, type(built)
assert built.CountFacets > 0

App.closeDocument(name)
print("ok")
"""

FREECAD_FIRST = r"""
import FreeCAD
import Part
import Mesh
import MeshPart

# One process, one core: the facade and every workbench module report the same
# application identity when the combined extension publishes them.
assert FreeCAD._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id, (
    "the modules do not share one kernel: %r != %r != %r != %r"
    % (FreeCAD._kernel_id, Part._kernel_id, Mesh._kernel_id, MeshPart._kernel_id)
)

name = "PackagedImageFreeCAD"
if name in FreeCAD.listDocuments():
    FreeCAD.closeDocument(name)
doc = FreeCAD.newDocument(name)
obj = doc.addObject("Part::Feature", "Box")
assert obj.Shape.isNull(), "a fresh Part::Feature is null"

# The shape assigned through `Part` must be the one `FreeCAD` reads back: this
# only holds when both modules come from the same extension. The feature
# classifies the shape it reports (`Part.Solid` here), so the assertion is on
# the classification, not on identity with the bare `Part.Shape` wrapper the
# `make*` factories still return.
obj.Shape = Part.makeBox(1, 1, 1)
assert isinstance(obj.Shape, Part.Solid), type(obj.Shape)
assert not obj.Shape.isNull()
assert abs(obj.Shape.Volume - 1.0) < 1e-9, obj.Shape.Volume

FreeCAD.closeDocument(name)
print("ok")
"""


def _run(source: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = str(PYTHON_DIR) + (os.pathsep + existing if existing else "")
    if OCCT_LIB.is_dir():
        variable = "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH"
        previous = env.get(variable)
        env[variable] = str(OCCT_LIB) + (os.pathsep + previous if previous else "")
    return subprocess.run(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
    )


_ACTIVE = extensions.active()


@unittest.skipIf(
    _ACTIVE is not None and "Part" in extensions.hidden(_ACTIVE),
    "the packaged modules belong to the workbenches extension set",
)
class PackagedModulesTests(unittest.TestCase):
    def assert_ok(self, source: str) -> None:
        result = _run(source)
        self.assertEqual(
            result.returncode,
            0,
            "stdout:\n" + result.stdout + "\nstderr:\n" + result.stderr,
        )
        self.assertIn("ok", result.stdout)

    def test_part_mesh_and_app_share_one_core(self):
        self.assert_ok(SHARED_CORE)

    def test_freecad_imported_first_shares_the_part_core(self):
        self.assert_ok(FREECAD_FIRST)


if __name__ == "__main__":
    unittest.main()

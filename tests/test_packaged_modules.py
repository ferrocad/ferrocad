"""The packaged modules: ``import Part`` / ``Mesh`` / ``FreeCAD`` from ``python/``.

The Rust conformance harness boots the modules as built-ins, which hides whether
the *installed* modules can be imported at all. This test starts a plain host
CPython with ``PYTHONPATH=python`` — exactly what a user does — and checks that the
shims reach the combined ``ferrocad_combined`` extension and that the four modules
share one document registry.

It skips when ``python/ferrocad_combined.abi3.so`` has not been built. Build it with
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
IMAGE = PYTHON_DIR / "ferrocad_combined.abi3.so"
# The extension links OCCT, which ships no RUNPATH. The app launcher sets this
# from the packaged `lib/`; a development checkout has the prefix in `.occt/`.
OCCT_LIB = REPO / ".occt" / "prefix" / "lib"

SHARED_CORE = r"""
import ferrocad as App
import Part
import Mesh
import MeshPart

name = "PackagedImage"
if name in App.listDocuments():
    App.closeDocument(name)
doc = App.newDocument(name)

obj = doc.addObject("Part::Feature", "Box")
shape = Part.makeBox(2, 3, 4)
obj.Shape = shape
assert type(obj.Shape) is Part.Shape, type(obj.Shape)
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

name = "PackagedImageFreeCAD"
if name in FreeCAD.listDocuments():
    FreeCAD.closeDocument(name)
doc = FreeCAD.newDocument(name)
obj = doc.addObject("Part::Feature", "Box")
assert obj.Shape.isNull(), "a fresh Part::Feature is null"

# The shape assigned through `Part` must be the one `FreeCAD` reads back: this
# only holds when both modules come from the same extension.
obj.Shape = Part.makeBox(1, 1, 1)
assert type(obj.Shape) is Part.Shape, type(obj.Shape)
assert not obj.Shape.isNull()

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


@unittest.skipUnless(
    IMAGE.is_file(), "ferrocad_combined.abi3.so is not built; run ./build.sh"
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

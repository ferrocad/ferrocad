//! One image: `ferrocad` + `Part` + `Mesh` + `MeshPart` sharing one core.
//!
//! This is the composition the packaged app performs and the combined
//! `ferrocad_image.abi3.so` reproduces for a host interpreter. It fails if any of
//! the modules was linked (or published) as a second image: the document registry
//! and the extension converters are process-wide, not per module.
//!
//! It needs OCCT (see `docs/architecture/occt-bundling.md`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Register the built-in modules. Must run before the interpreter initialises
/// (`auto-initialize` does that on the first `Python::with_gil`).
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(ferrocad_image_py::register_builtins);
}

fn run(py: Python<'_>, source: &str) {
    let code = CString::new(source).expect("test script has no NUL");
    let globals = PyDict::new(py);
    py.run(&code, Some(&globals), None).expect("run test script");
}

#[test]
fn all_four_modules_share_one_document_core() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part
import Mesh
import MeshPart

name = "SharedPythonImage"
if name in App.listDocuments():
    App.closeDocument(name)
doc = App.newDocument(name)

# Part and App agree on the document registry and on the Shape converter.
boxed = doc.addObject("Part::Feature", "Box")
shape = Part.makeBox(2, 3, 4)
boxed.Shape = shape
assert type(boxed.Shape) is Part.Shape, type(boxed.Shape)
assert boxed.Shape.isSame(shape), "Part and App disagree on the shape"

# Mesh is in the same registry and uses the same App types.
mesh_obj = doc.addObject("Mesh::Feature", "Mesh")
mesh = Mesh.createBox(1, 1, 1)
mesh_obj.Mesh = mesh
assert type(mesh_obj.Mesh) is Mesh.Mesh, type(mesh_obj.Mesh)
assert mesh_obj.Mesh.CountFacets == 12, mesh_obj.Mesh.CountFacets

# MeshPart consumes the Part shape directly (it calls Shape.tessellate).
built = MeshPart.meshFromShape(shape, LinearDeflection=0.5, AngularDeflection=0.5)
assert type(built) is Mesh.Mesh, type(built)
assert built.CountFacets > 0

App.closeDocument(name)
"#,
        );
    });
}

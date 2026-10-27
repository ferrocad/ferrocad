//! The typed sub-shape accessors and `Shape.childShapes()`'s direct-child level.
//!
//! SimpleCAD writes `shape.Faces[0].valueAt(u, v)`, `face.Surface`,
//! `wire.OrderedVertexes` and `shape.childShapes()`, so:
//!
//! - `Shape.Faces` yields `Part.Face` and `Shape.Wires` yields `Part.Wire`;
//! - `Edges`/`Vertexes` keep their `Part.Edge`/`Part.Vertex` wrappers;
//! - `childShapes()` is one topological level (a box's shell, a compound's
//!   members, a shell's faces, a wire's edges, an edge's vertices), while
//!   `childShapes("Face")` is the recursive kind filter;
//! - `getElement`/`ancestorsOfType` stay typed.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// Run a Python snippet against the booted modules, reporting the traceback when
/// it fails.
fn run(py: Python<'_>, source: &str) {
    let code = CString::new(source).expect("the snippet has no NUL");
    let globals = PyDict::new(py);
    if let Err(error) = py.run(&code, Some(&globals), None) {
        error.print(py);
        panic!("the typed-accessor snippet failed:\n{source}");
    }
}

#[test]
fn faces_and_wires_yield_their_typed_wrappers() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 3, 4)

face = box.Faces[0]
assert type(face).__name__ == "Face", type(face).__name__
assert isinstance(face, Part.Face)
assert len(box.Faces) == 6
u0, u1, v0, v1 = face.ParameterRange
point = face.valueAt((u0 + u1) / 2, (v0 + v1) / 2)
assert all(isinstance(getattr(point, axis), float) for axis in ("x", "y", "z"))

wire = box.Wires[0]
assert type(wire).__name__ == "Wire", type(wire).__name__
assert isinstance(wire, Part.Wire)
assert len(box.Wires) == 6
ordered = wire.OrderedVertexes
assert len(ordered) == 4, len(ordered)
assert all(type(v).__name__ == "Vertex" for v in ordered)

# The edge and vertex getters keep their typed wrappers.
assert all(type(edge).__name__ == "Edge" for edge in box.Edges)
assert all(type(vertex).__name__ == "Vertex" for vertex in box.Vertexes)
assert len(box.Edges) == 12 and len(box.Vertexes) == 8
"#,
        );
    });
}

#[test]
fn child_shapes_are_one_topological_level() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
import ferrocad as App

box = Part.makeBox(2, 2, 2)

# A box is one solid; its one direct child is the shell (upstream
# `TopoDS_Iterator`), not the solid itself.
children = box.childShapes()
assert len(children) == 1, len(children)
assert children[0].ShapeType == "Shell", children[0].ShapeType
assert len(box.SubShapes) == 1

# A compound reports its direct members, not the descendants of each member.
first = Part.makeBox(1, 1, 1)
second = Part.makeBox(1, 1, 1)
second = second.translated(App.Vector(2, 0, 0))
compound = Part.makeCompound([first, second])
assert compound.ShapeType == "Compound", compound.ShapeType
members = compound.childShapes()
assert len(members) == 2, len(members)
assert all(member.ShapeType == "Solid" for member in members)

# A shell reports faces, a wire edges, an edge vertices.
shell = box.Shells[0]
assert shell.ShapeType == "Shell"
faces = shell.childShapes()
assert len(faces) == 6 and all(face.ShapeType == "Face" for face in faces)

wire = box.Faces[0].OuterWire
assert wire.ShapeType == "Wire"
edges = wire.childShapes()
assert len(edges) == 4 and all(edge.ShapeType == "Edge" for edge in edges)

vertices = box.Edges[0].childShapes()
assert len(vertices) == 2 and all(v.ShapeType == "Vertex" for v in vertices)
assert box.Vertexes[0].childShapes() == []

# `shape_type` is the recursive kind filter (upstream getChildShapes(type)).
typed = box.childShapes("Face")
assert len(typed) == 6 and all(face.ShapeType == "Face" for face in typed)
assert len(compound.childShapes("Face")) == 12

# A mixed compound keeps its members of every kind: a standalone face and a
# standalone edge are both direct children.
mixed = Part.makeCompound([Part.makeBox(1, 1, 1).Faces[0], Part.makeBox(1, 1, 1).Edges[0]])
kinds = sorted(child.ShapeType for child in mixed.childShapes())
assert kinds == ["Edge", "Face"], kinds
"#,
        );
    });
}

#[test]
fn element_queries_stay_typed() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 2, 2)

assert isinstance(box.getElement("Edge1"), Part.Edge)
assert isinstance(box.getElement("Face1"), Part.Face)
assert isinstance(box.getElement("Vertex1"), Part.Vertex)
assert box.getElement("Face2").isSame(box.Faces[1])

edge = box.Edges[0]
faces = box.ancestorsOfType(edge, Part.Face)
assert len(faces) == 2, len(faces)
assert all(isinstance(face, Part.Face) for face in faces)
"#,
        );
    });
}

//! Booted tests for the `Mesh` and `MeshPart` modules.
//!
//! The modules must share one image with `ferrocad` and `Part` (one
//! `ferrocad_core`, one document registry), exactly as
//! `crates/ferrocad_part_py/tests/part_boot.rs` boots `Part`. The bodies are
//! written as Python so the assertions exercise the real classes and conversion
//! seam, not a Rust-only shadow of them.
//!
//! ```sh
//! cargo test -p ferrocad_mesh_py --test mesh_bindings -- --nocapture
//! ```

use std::ffi::CString;
use std::sync::Once;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Register the four built-in modules. Must run before the interpreter starts
/// (`auto-initialize` does that on the first `Python::with_gil`).
fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_mesh_py::{Mesh as mesh_module, MeshPart as meshpart_module};
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
        pyo3::append_to_inittab!(mesh_module);
        pyo3::append_to_inittab!(meshpart_module);
    });
}

/// Run a Python body; a failed assertion fails the Rust test.
///
/// Each body runs in its own globals dict: the tests share one interpreter and
/// may run in parallel, so a shared `__main__` would let one body's locals
/// overwrite another's between bytecodes.
fn run(py: Python<'_>, code: &str) {
    let code = CString::new(code).expect("the test body has no NUL");
    let globals = PyDict::new(py);
    globals.set_item("__name__", "mesh_bindings").unwrap();
    py.run(&code, Some(&globals), None)
        .expect("python assertions");
}

#[test]
fn constructors_cover_the_upstream_forms() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import ferrocad as App

empty = Mesh.Mesh()
assert empty.CountFacets == 0 and empty.CountPoints == 0
assert len(empty) == 0
assert [tuple(p) for p in empty.Points] == []
assert empty.Topology == ([], [])

V = App.Vector
points = [V(0, 0, 0), V(1, 0, 0), V(0, 1, 0)]
m = Mesh.Mesh((points, [(0, 1, 2)]))
assert m.CountPoints == 3 and m.CountFacets == 1
assert m.Topology[1] == [(0, 1, 2)]
assert [tuple(p) for p in m.Points] == [(0, 0, 0), (1, 0, 0), (0, 1, 0)]

# A list of triangles, each a sequence of three points.
m2 = Mesh.Mesh([[V(0, 0, 0), V(1, 0, 0), V(0, 1, 0)],
                [V(0, 0, 0), V(0, 1, 0), V(-1, 0, 0)]])
assert m2.CountFacets == 2
assert m2.CountPoints == 4, "shared corners are welded"

# A list of triangles, each nine floats.
m3 = Mesh.Mesh([[0, 0, 0, 1, 0, 0, 0, 1, 0]])
assert m3.CountFacets == 1 and m3.CountPoints == 3

# A flat list of 3-float points, three per facet.
m4 = Mesh.Mesh([V(0, 0, 0), V(1, 0, 0), V(0, 1, 0)])
assert m4.CountFacets == 1
m5 = Mesh.Mesh([[0, 0, 0], [1, 0, 0], [0, 1, 0]])
assert m5.CountFacets == 1

# A list of Facet values (test_mesh_slicing.py:34 passes Facet.Points).
box = Mesh.createBox(10, 10, 10)
m6 = Mesh.Mesh([facet.Points for facet in box.Facets])
assert m6.CountFacets == 12 and m6.CountPoints == 8

# Mesh.Mesh(mesh) copies points, facets and the placement.
box.Placement = App.Placement(V(1, 2, 3))
copy = Mesh.Mesh(box)
assert copy.CountFacets == 12 and copy.CountPoints == 8
assert tuple(copy.Placement.Base) == (1, 2, 3)

# A non-Vector point must raise, not be silently dropped (upstream's bug).
try:
    Mesh.Mesh([[None, V(1, 0, 0), V(0, 1, 0)]])
except TypeError:
    pass
else:
    raise AssertionError("a non-vector point must raise TypeError")

# The upstream fourth createBox argument is accepted only as zero.
try:
    Mesh.createBox(1, 1, 1, 2)
except NotImplementedError:
    pass
else:
    raise AssertionError("nonzero edgelen must raise NotImplementedError")
"#,
        );
    });
}

#[test]
fn placement_moves_topology_and_bound_box_but_not_points() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import ferrocad as App

V = App.Vector
box = Mesh.createBox(10, 20, 30)
local = [tuple(p) for p in box.Points]
assert len(local) == 8
assert local[0] == (-5.0, -10.0, -15.0), local[0]
assert [tuple(p) for p in box.Topology[0]] == local, "an unplaced mesh reads local"

box.Placement = App.Placement(V(100, 200, 300))
placed = [tuple(p) for p in box.Topology[0]]
assert placed[0] == (local[0][0] + 100, local[0][1] + 200, local[0][2] + 300), placed[0]
assert [tuple(p) for p in box.Points] == local, "Points stay local"

bb = box.BoundBox
assert bb.XMin == 95 and bb.XMax == 105, (bb.XMin, bb.XMax)
assert bb.YMin == 190 and bb.YMax == 210
assert bb.ZMin == 285 and bb.ZMax == 315
assert tuple(bb.Center) == (100.0, 200.0, 300.0)

assert abs(box.Volume - 6000.0) < 1e-9
assert abs(box.Area - 2200.0) < 1e-9

# Facet.Points and Facet.Normal are local, with a unit normal.
facet = box.Facets[0]
facet_indices = box.Topology[1][0]
assert [tuple(p) for p in facet.Points] == [local[i] for i in facet_indices]
assert abs(facet.Normal.Length - 1.0) < 1e-9

# Setting the placement is absolute and never bakes the points.
box.Placement = App.Placement()
assert [tuple(p) for p in box.Topology[0]] == local
"#,
        );
    });
}

#[test]
fn analysis_mutators_and_components_round_trip() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import ferrocad as App

V = App.Vector
box = Mesh.createBox(10, 10, 10)
assert box.isSolid()
assert not box.hasNonManifolds()
assert not box.hasNonUniformOrientedFacets()
assert not box.hasSelfIntersections()
assert box.getSelfIntersections() == []
assert box.countComponents() == 1

segments = box.getPlanarSegments(1e-6)
assert len(segments) == 6 and all(len(group) == 2 for group in segments)
assert box.getPlanarSegments(1e-6, 3) == []

components = box.getSeparateComponents()
assert len(components) == 1 and components[0].CountFacets == 12

# In-place mutators return None and leave a clean box alone.
for call in (box.removeDuplicatedPoints, box.removeDuplicatedFacets,
             box.fixDegenerations, box.harmonizeNormals):
    assert call() is None
assert box.CountFacets == 12 and box.CountPoints == 8
assert box.isSolid()

# A duplicated point and facet are merged by the mutators.
points = [V(0, 0, 0), V(1, 0, 0), V(0, 1, 0), V(1, 0, 0)]
facets = [(0, 1, 2), (0, 3, 2)]
noisy = Mesh.Mesh((points, facets))
assert noisy.CountFacets == 2
noisy.removeDuplicatedPoints()
assert noisy.CountPoints == 3
noisy.removeDuplicatedFacets()
assert noisy.CountFacets == 1
"#,
        );
    });
}

#[test]
fn a_box_writes_and_reads_back_as_a_closed_binary_stl() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import os
import tempfile
import Mesh
import ferrocad as App

folder = tempfile.mkdtemp()
path = os.path.join(folder, "box.stl")

box = Mesh.createBox(2, 3, 4)
box.Placement = App.Placement(App.Vector(7, 8, 9))
box.write(path)

restored = Mesh.Mesh(path)
assert restored.CountFacets == 12, restored.CountFacets
assert restored.CountPoints == 8, restored.CountPoints
assert restored.isSolid() and not restored.hasNonManifolds()
assert abs(restored.Volume - 24.0) < 1e-4, restored.Volume

# The writer emits the local kernel; the read mesh has identity placement.
assert tuple(restored.Placement.Base) == (0, 0, 0)
assert tuple(restored.BoundBox.Center) == (0, 0, 0)

for bad in (os.path.join(folder, "box.obj"), os.path.join(folder, "box")):
    try:
        box.write(bad)
    except ValueError as error:
        assert ".stl" in str(error), str(error)
    else:
        raise AssertionError("an unsupported write suffix must raise ValueError")
    try:
        Mesh.Mesh(bad)
    except ValueError as error:
        assert ".stl" in str(error), str(error)
    else:
        raise AssertionError("an unsupported read suffix must raise ValueError")
"#,
        );
    });
}

#[test]
fn a_box_writes_and_reads_back_through_bms() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import os
import tempfile
import Mesh
import ferrocad as App

folder = tempfile.mkdtemp()
path = os.path.join(folder, "box.bms")

box = Mesh.createBox(2, 3, 4)
box.Placement = App.Placement(App.Vector(7, 8, 9))
local = [tuple(p) for p in box.Points]
facets = box.Topology[1]
box.write(path)

restored = Mesh.Mesh(path)
assert restored.CountFacets == 12, restored.CountFacets
assert restored.CountPoints == 8, restored.CountPoints
assert restored.Topology[1] == facets, restored.Topology[1]
for got, want in zip([tuple(p) for p in restored.Points], local):
    assert all(abs(a - b) < 1e-5 for a, b in zip(got, want)), (got, want)
assert restored.isSolid() and not restored.hasNonManifolds()
assert abs(restored.Volume - 24.0) < 1e-4, restored.Volume

# The placement rides in the BMS info field; the points stay local.
assert tuple(restored.Placement.Base) == (7, 8, 9), restored.Placement.Base
assert tuple(restored.BoundBox.Center) == (7.0, 8.0, 9.0), restored.BoundBox.Center

# BMS keeps duplicate point identities that STL would weld.
points = [App.Vector(0, 0, 0), App.Vector(1, 0, 0),
          App.Vector(0, 1, 0), App.Vector(1, 0, 0)]
noisy = Mesh.Mesh((points, [(0, 1, 2), (0, 3, 2)]))
noisy_path = os.path.join(folder, "duplicates.bms")
noisy.write(noisy_path)
back = Mesh.Mesh(noisy_path)
assert back.CountPoints == 4, back.CountPoints
assert back.CountFacets == 2, back.CountFacets
assert back.Topology[1] == [(0, 1, 2), (0, 3, 2)], back.Topology[1]
"#,
        );
    });
}

#[test]
fn a_mesh_feature_tracks_its_placement_without_a_recompute() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import ferrocad as App

name = "MeshFeatureBindings"
if name in App.listDocuments():
    App.closeDocument(name)
doc = App.newDocument(name)

obj = doc.addObject("Mesh::Feature", "Mesh")
assert obj.TypeId == "Mesh::Feature"
assert obj.Mesh.CountFacets == 0, "a Mesh::Feature starts with an empty mesh"

box = Mesh.createBox(10, 10, 10)
box.Placement = App.Placement(App.Vector(5, 0, 0))
local = [tuple(p) for p in box.Points]

# Assignment adopts the mesh's placement.
obj.Mesh = box
assert tuple(obj.Placement.Base) == (5, 0, 0), tuple(obj.Placement.Base)
placed = [tuple(p) for p in obj.Mesh.Topology[0]]
assert placed[0] == (local[0][0] + 5, local[0][1], local[0][2]), placed[0]
assert [tuple(p) for p in obj.Mesh.Points] == local, "obj.Mesh.Points stay local"

# A Placement assignment is visible immediately, without a recompute.
obj.Placement = App.Placement(App.Vector(0, 20, 0))
moved = [tuple(p) for p in obj.Mesh.Topology[0]]
assert moved[0] == (local[0][0], local[0][1] + 20, local[0][2]), moved[0]
assert tuple(obj.Mesh.BoundBox.Center) == (0.0, 20.0, 0.0), tuple(obj.Mesh.BoundBox.Center)
assert [tuple(p) for p in obj.Mesh.Points] == local

App.closeDocument(name)
"#,
        );
    });
}

#[test]
fn mesh_part_builds_a_mesh_from_a_shape() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import MeshPart
import Part
import ferrocad as App

shape = Part.makeBox(10, 20, 30)

mesh = MeshPart.meshFromShape(Shape=shape, LinearDeflection=0.1)
assert mesh.CountPoints == 8, mesh.CountPoints
assert mesh.CountFacets == 12, mesh.CountFacets
assert mesh.isSolid() and not mesh.hasNonManifolds()
assert abs(mesh.Volume - 6000.0) < 1e-3, mesh.Volume
assert tuple(mesh.Placement.Base) == (0, 0, 0)

# Positional form: (Shape, LinearDeflection, AngularDeflection, Relative, Segments).
same = MeshPart.meshFromShape(shape, 0.1, 0.5, False, False)
assert same.CountPoints == 8 and same.CountFacets == 12
assert same.countSegments() == 0, "plain meshFromShape records no groups"

# Segments=True: one facet group per box face, welded into one closed mesh.
segmented = MeshPart.meshFromShape(shape, 0.1, 0.5, False, True)
assert segmented.CountPoints == 8, segmented.CountPoints
assert segmented.CountFacets == 12, segmented.CountFacets
assert segmented.countSegments() == 6, segmented.countSegments()
assert segmented.isSolid() and not segmented.hasNonManifolds()

covered = []
for index in range(segmented.countSegments()):
    group = segmented.getSegment(index)
    assert len(group) == 2, (index, group)
    assert all(0 <= facet < segmented.CountFacets for facet in group), group
    covered.extend(group)
assert sorted(covered) == list(range(12)), sorted(covered)
copy = Mesh.Mesh(segmented)
assert copy.countSegments() == 6, "a copy keeps the facet groups"
assert copy.getSegment(0) == segmented.getSegment(0)

# Upstream raises IndexError for an out-of-range group.
try:
    segmented.getSegment(6)
except IndexError:
    pass
else:
    raise AssertionError("getSegment(6) must raise IndexError")
"#,
        );
    });
}

#[test]
fn wire_from_segment_matches_the_face_boundary() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Mesh
import MeshPart
import ferrocad as App

box = Mesh.createBox(10, 20, 30)
segments = box.getPlanarSegments(1e-6)
assert len(segments) == 6, segments

lengths = []
for group in segments:
    wires = MeshPart.wireFromSegment(box, group)
    assert len(wires) == 1, len(wires)
    wire = wires[0]
    assert wire.isClosed(), "a planar face boundary is a closed wire"
    lengths.append(round(wire.Length, 6))

# Each pair of opposite box faces has the same perimeter.
assert sorted(lengths) == [60.0, 60.0, 80.0, 80.0, 100.0, 100.0], sorted(lengths)

# An out-of-range group is a ValueError (the box has 12 facets).
try:
    MeshPart.wireFromSegment(box, [0, 12])
except ValueError:
    pass
else:
    raise AssertionError("an out-of-range facet index must raise ValueError")
"#,
        );
    });
}

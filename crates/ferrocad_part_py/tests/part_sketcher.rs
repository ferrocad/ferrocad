//! `Sketcher::SketchObject`, `PartDesign::Body` and `Part::Mirroring`, end to end.
//!
//! These are the three runtime surfaces the upstream Part tests reach through
//! `TestPartApp.py` and `parttests/TestPartMirror.py`. The sketch here covers the
//! geometry/`Shape` half and the constraint model: constraints are stored,
//! diagnosed and reported, but nothing is solved (see
//! `docs/developer/workbenches/sketch-constraints.md`).
//!
//! Same one-image boot as the other Part tests: `ferrocad` + `Part` as built-ins,
//! one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::sync::{Mutex, Once};

use pyo3::exceptions::PyAttributeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

fn new_document<'py>(py: Python<'py>, name: &str) -> Bound<'py, PyAny> {
    // Importing `Part` runs `register_kernel`, which registers the Part object
    // types (`Sketcher::SketchObject`, `PartDesign::Body`, `Part::Mirroring`).
    // Without it, `addObject` finds only the core property table (no `Shape`
    // output), and the result depends on which test happened to import Part first.
    PyModule::import(py, "Part").expect("import Part");
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .call_method1("newDocument", (name,))
        .expect("newDocument")
}

fn close_document(py: Python<'_>, name: &str) {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .call_method1("closeDocument", (name,))
        .expect("closeDocument");
}

fn ferrocad_vector(py: Python<'_>, x: f64, y: f64, z: f64) -> Bound<'_, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a vector")
}

fn ferrocad_placement(py: Python<'_>, base: (f64, f64, f64)) -> Bound<'_, PyAny> {
    let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
    let rotation = ferrocad
        .getattr("Rotation")
        .expect("ferrocad.Rotation")
        .call0()
        .expect("a rotation");
    ferrocad
        .getattr("Placement")
        .expect("ferrocad.Placement")
        .call1((ferrocad_vector(py, base.0, base.1, base.2), rotation))
        .expect("a placement")
}

fn line_segment<'py>(
    py: Python<'py>,
    start: (f64, f64),
    end: (f64, f64),
) -> Bound<'py, PyAny> {
    let part = PyModule::import(py, "Part").expect("import Part");
    part.getattr("LineSegment")
        .expect("Part.LineSegment")
        .call1((
            ferrocad_vector(py, start.0, start.1, 0.0),
            ferrocad_vector(py, end.0, end.1, 0.0),
        ))
        .expect("a line segment")
}

/// Four line segments forming the unit square in the XY plane.
fn add_unit_square(sketch: &Bound<'_, PyAny>) {
    let py = sketch.py();
    let corners = [
        ((0.0, 0.0), (1.0, 0.0)),
        ((1.0, 0.0), (1.0, 1.0)),
        ((1.0, 1.0), (0.0, 1.0)),
        ((0.0, 1.0), (0.0, 0.0)),
    ];
    for (start, end) in corners {
        sketch
            .call_method1("addGeometry", (line_segment(py, start, end), false))
            .expect("addGeometry");
    }
}

fn is_null(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isNull")
        .expect("Shape.isNull")
        .extract::<bool>()
        .expect("isNull as bool")
}

fn count(shape: &Bound<'_, PyAny>, name: &str) -> usize {
    shape
        .getattr(name)
        .unwrap_or_else(|_| panic!("Shape.{name}"))
        .len()
        .unwrap_or_else(|_| panic!("len(Shape.{name})"))
}

/// The `BoundBox` as `[xmin, ymin, zmin, xmax, ymax, zmax]`.
fn bounds(shape: &Bound<'_, PyAny>) -> [f64; 6] {
    let bound_box = shape.getattr("BoundBox").expect("Shape.BoundBox");
    let value = |name: &str| {
        bound_box
            .getattr(name)
            .unwrap_or_else(|_| panic!("BoundBox.{name}"))
            .extract::<f64>()
            .unwrap_or_else(|_| panic!("BoundBox.{name} as f64"))
    };
    [
        value("XMin"),
        value("YMin"),
        value("ZMin"),
        value("XMax"),
        value("YMax"),
        value("ZMax"),
    ]
}

fn assert_bounds(shape: &Bound<'_, PyAny>, expected: [f64; 6]) {
    let actual = bounds(shape);
    for (index, (have, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (have - want).abs() < 1e-6,
            "bounds[{index}] was {have}, expected {want} (all: {actual:?})"
        );
    }
}

#[test]
fn sketch_geometry_builds_a_wire_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchShape");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");

        assert_eq!(
            sketch
                .call_method1("addGeometry", (line_segment(py, (0.0, 0.0), (1.0, 0.0)), false))
                .expect("first addGeometry")
                .extract::<usize>()
                .expect("index"),
            0
        );
        for (start, end) in [
            ((1.0, 0.0), (1.0, 1.0)),
            ((1.0, 1.0), (0.0, 1.0)),
            ((0.0, 1.0), (0.0, 0.0)),
        ] {
            sketch
                .call_method1("addGeometry", (line_segment(py, start, end), false))
                .expect("addGeometry");
        }
        assert_eq!(
            sketch.getattr("Geometry").expect("Geometry").len().expect("len"),
            4
        );

        doc.call_method0("recompute").expect("recompute");

        let shape = sketch.getattr("Shape").expect("Shape");
        assert!(!is_null(&shape), "a closed sketch has a shape");
        assert_eq!(count(&shape, "Edges"), 4, "one edge per geometry item");
        assert_eq!(count(&shape, "Wires"), 1, "the four edges join into one wire");
        assert_bounds(&shape, [0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
        // Edges keep their geometry order, so a later ruled surface can link them.
        shape.getattr("Edge1").expect("Shape.Edge1");
        shape.getattr("Edge4").expect("Shape.Edge4");

        close_document(py, "RustSketchShape");
    });
}

/// A stand-in for `Sketcher.Constraint`: the document surface duck-types the
/// record by attribute, and the pure-Python `Sketcher` module is not on this test
/// image's `sys.path`. The Python suite covers the real class.
fn fake_constraint(kind: &str, first: i64, value: f64) -> PyResult<Py<PyAny>> {
    fake_record(kind, &[("First", first)], value)
}

/// A two-geometry stand-in (`Equal`, `Parallel`, `Coincident`, …).
fn fake_pair(kind: &str, first: i64, second: i64) -> PyResult<Py<PyAny>> {
    fake_record(kind, &[("First", first), ("Second", second)], 0.0)
}

fn fake_record(kind: &str, fields: &[(&str, i64)], value: f64) -> PyResult<Py<PyAny>> {
    Python::with_gil(|py| {
        let kwargs = PyDict::new(py);
        kwargs.set_item("Type", kind)?;
        for (name, number) in fields {
            kwargs.set_item(*name, *number)?;
        }
        kwargs.set_item("Value", value)?;
        let namespace = py.import("types")?.getattr("SimpleNamespace")?;
        Ok(namespace.call((), Some(&kwargs))?.unbind())
    })
}

/// The raw `ConstraintData` entries: the stored form, which names stable ids.
fn stored_constraints(sketch: &Bound<'_, PyAny>) -> Vec<String> {
    sketch
        .getattr("ConstraintData")
        .expect("ConstraintData")
        .extract()
        .expect("a string list")
}

/// The stable ids, in positional order.
fn geometry_ids(sketch: &Bound<'_, PyAny>) -> Vec<String> {
    sketch
        .getattr("GeometryIds")
        .expect("GeometryIds")
        .extract()
        .expect("a string list")
}

#[test]
fn sketch_constraints_are_stored_diagnosed_and_editable() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchConstraints");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (0.0, 0.0), (1.0, 0.0)), false),
            )
            .expect("addGeometry");

        // A datum between the two endpoints and an axis constraint: two rows out
        // of four parameters, so two degrees of freedom (where the line sits).
        let distance = fake_record(
            "Distance",
            &[("First", 0), ("FirstPos", 1), ("Second", 0), ("SecondPos", 2)],
            5.0,
        )
        .expect("distance constraint");
        let horizontal = fake_constraint("Horizontal", 0, 0.0).expect("horizontal constraint");
        assert_eq!(
            sketch
                .call_method1("addConstraint", (&distance,))
                .expect("addConstraint")
                .extract::<i64>()
                .expect("index"),
            0
        );
        assert_eq!(
            sketch
                .call_method1("addConstraint", (vec![horizontal],))
                .expect("addConstraint(list)")
                .extract::<i64>()
                .expect("index"),
            1
        );
        assert_eq!(attribute::<usize>(&sketch, "ConstraintCount"), 2);
        assert_eq!(
            attribute::<i64>(&sketch, "solve"),
            2,
            "solve returns the remaining degrees of freedom"
        );
        assert_eq!(
            sketch
                .call_method1("getDatum", (0,))
                .expect("getDatum")
                .extract::<f64>()
                .expect("value"),
            5.0
        );
        sketch
            .call_method1("setDatum", (0, 12.5))
            .expect("setDatum");
        assert_eq!(
            sketch
                .call_method1("getDatum", (0,))
                .expect("getDatum")
                .extract::<f64>()
                .expect("value"),
            12.5
        );

        // A non-dimensional constraint has no datum to read or write.
        let error = sketch
            .call_method1("getDatum", (1,))
            .expect_err("Horizontal carries no datum");
        assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));

        sketch
            .call_method1("delConstraint", (0,))
            .expect("delConstraint");
        assert_eq!(attribute::<usize>(&sketch, "ConstraintCount"), 1);

        // A record that names an id the sketch no longer has (a hand-edited or
        // restored property) is a model error: `solve` reports it. The decoded
        // `Constraints` getter refuses it too, which the Python suite pins (the
        // `Sketcher.Constraint` class this image needs is not on its `sys.path`).
        sketch
            .setattr("ConstraintData", vec!["Vertical;first=#gone"])
            .expect("ConstraintData");
        assert_eq!(attribute::<i64>(&sketch, "solve"), -1, "dangling id");

        close_document(py, "RustSketchConstraints");
    });
}

#[test]
fn autoconstraints_find_coincidence_and_verticality() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchAutoconstraints");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        // (0,0) -> (2,2) -> (2,4): the second edge is vertical and the two meet.
        for (start, end) in [((0.0, 0.0), (2.0, 2.0)), ((2.0, 2.0), (2.0, 4.0))] {
            sketch
                .call_method1("addGeometry", (line_segment(py, start, end), false))
                .expect("addGeometry");
        }

        assert_eq!(
            sketch
                .call_method1("detectMissingPointOnPointConstraints", (1e-3,))
                .expect("detect coincident")
                .extract::<usize>()
                .expect("count"),
            2,
            "the pair, plus the first edge's start on the root point"
        );
        assert_eq!(
            sketch
                .call_method1("makeMissingPointOnPointCoincident", (false,))
                .expect("make coincident")
                .extract::<usize>()
                .expect("count"),
            2
        );
        assert_eq!(
            sketch
                .call_method1("detectMissingVerticalHorizontalConstraints", (1e-3,))
                .expect("detect axis")
                .extract::<usize>()
                .expect("count"),
            1
        );
        assert_eq!(
            sketch
                .call_method1("makeMissingVerticalHorizontal", (false,))
                .expect("make axis")
                .extract::<usize>()
                .expect("count"),
            1
        );
        assert_eq!(attribute::<usize>(&sketch, "ConstraintCount"), 3);

        // The staged records name stable ids, and the origin coincidence names
        // the root point (`-1`, `PointPos.start`) rather than a geometry index.
        let stored = stored_constraints(&sketch);
        assert!(
            stored
                .iter()
                .any(|entry| entry == "Coincident;first=#g1;firstPos=2;second=#g2;secondPos=1"),
            "the pair is stored by id: {stored:?}"
        );
        assert!(
            stored
                .iter()
                .any(|entry| entry == "Coincident;first=-1;firstPos=1;second=#g1;secondPos=1"),
            "the origin coincidence names the root: {stored:?}"
        );
        assert!(
            stored.iter().any(|entry| entry == "Vertical;first=#g2"),
            "the axis constraint is stored by id: {stored:?}"
        );

        // Running the detection again finds nothing new: the coincident pair, the
        // root coincidence and the axis are already constrained, and the pending
        // lists were cleared.
        assert_eq!(
            sketch
                .call_method1("detectMissingPointOnPointConstraints", (1e-3,))
                .expect("detect again")
                .extract::<usize>()
                .expect("count"),
            0
        );

        close_document(py, "RustSketchAutoconstraints");
    });
}

#[test]
fn geometry_identity_survives_a_deletion() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchIdentity");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        for (start, end) in [
            ((0.0, 0.0), (1.0, 0.0)),
            ((1.0, 0.0), (2.0, 0.0)),
            ((2.0, 0.0), (3.0, 0.0)),
        ] {
            sketch
                .call_method1("addGeometry", (line_segment(py, start, end), false))
                .expect("addGeometry");
        }
        assert_eq!(geometry_ids(&sketch), vec!["g1", "g2", "g3"]);

        // Two constraints, one of which names the element that is about to go.
        sketch
            .call_method1(
                "addConstraint",
                (fake_pair("Equal", 0, 2).expect("equal"),),
            )
            .expect("addConstraint Equal(0, 2)");
        sketch
            .call_method1(
                "addConstraint",
                (fake_pair("Equal", 1, 2).expect("equal"),),
            )
            .expect("addConstraint Equal(1, 2)");

        sketch.call_method1("delGeometry", (1,)).expect("delGeometry");

        assert_eq!(attribute::<usize>(&sketch, "GeometryCount"), 2);
        assert_eq!(geometry_ids(&sketch), vec!["g1", "g3"]);
        assert_eq!(attribute::<usize>(&sketch, "ConstraintCount"), 1);
        // The survivor still names g1 and g3; only its positional index moved.
        assert_eq!(stored_constraints(&sketch), vec!["Equal;first=#g1;second=#g3"]);
        assert_eq!(
            attribute::<i64>(&sketch, "solve"),
            7,
            "two lines (8 parameters) with one length equality"
        );

        close_document(py, "RustSketchIdentity");
    });
}

#[test]
fn construction_geometry_is_addressable_but_not_drawn() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchConstruction");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (0.0, 0.0), (1.0, 0.0)), false),
            )
            .expect("visible geometry");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (0.0, 1.0), (0.0, 2.0)), true),
            )
            .expect("construction geometry");
        doc.call_method0("recompute").expect("recompute");

        assert_eq!(attribute::<usize>(&sketch, "GeometryCount"), 2);
        assert!(!sketch
            .call_method1("getConstruction", (0,))
            .expect("getConstruction(0)")
            .extract::<bool>()
            .expect("flag"));
        assert!(sketch
            .call_method1("getConstruction", (1,))
            .expect("getConstruction(1)")
            .extract::<bool>()
            .expect("flag"));
        // Only the visible edge reaches the output shape.
        assert_eq!(count(&sketch.getattr("Shape").expect("Shape"), "Edges"), 1);

        // A construction element is still a constraint target.
        sketch
            .call_method1(
                "addConstraint",
                (fake_constraint("Vertical", 1, 0.0).expect("vertical"),),
            )
            .expect("constrain construction geometry");
        // Two lines are eight parameters and one axis row is one; the vertical is
        // already vertical, so the solve is a no-op that reports seven.
        assert_eq!(attribute::<i64>(&sketch, "solve"), 7);

        // Toggling the flag is what changes the drawing, not the identity.
        sketch
            .call_method1("toggleConstruction", (1,))
            .expect("toggleConstruction");
        doc.call_method0("recompute").expect("recompute");
        assert!(!sketch
            .call_method1("getConstruction", (1,))
            .expect("getConstruction(1)")
            .extract::<bool>()
            .expect("flag"));
        assert_eq!(count(&sketch.getattr("Shape").expect("Shape"), "Edges"), 2);
        assert_eq!(geometry_ids(&sketch), vec!["g1", "g2"], "toggling keeps ids");
        assert_eq!(attribute::<usize>(&sketch, "ConstraintCount"), 1);

        // Out-of-range indices are refused by name.
        let error = sketch
            .call_method1("getConstruction", (9,))
            .expect_err("out of range");
        assert!(error.is_instance_of::<pyo3::exceptions::PyIndexError>(py));

        close_document(py, "RustSketchConstruction");
    });
}

#[test]
fn root_axis_references_are_valid_and_external_ones_are_refused() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchRootAxis");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (0.0, 0.0), (1.0, 0.0)), false),
            )
            .expect("addGeometry");

        // `Coincident(root, start, geometry 0, start)`: the origin is the root
        // point, i.e. the start of the X axis.
        let mut kwargs = PyDict::new(py);
        kwargs.set_item("Type", "Coincident").expect("Type");
        kwargs.set_item("First", -1).expect("First");
        kwargs.set_item("FirstPos", 1).expect("FirstPos");
        kwargs.set_item("Second", 0).expect("Second");
        kwargs.set_item("SecondPos", 1).expect("SecondPos");
        let constraint = py
            .import("types")
            .expect("types")
            .getattr("SimpleNamespace")
            .expect("SimpleNamespace")
            .call((), Some(&kwargs))
            .expect("a root constraint");
        sketch
            .call_method1("addConstraint", (&constraint,))
            .expect("a root reference is valid");
        assert_eq!(
            attribute::<i64>(&sketch, "solve"),
            2,
            "four parameters, two coincidence rows"
        );
        assert_eq!(
            stored_constraints(&sketch),
            vec!["Coincident;first=-1;firstPos=1;second=#g1;secondPos=1"]
        );

        // External geometry (`-3` and below) is out of surface, and says so.
        let error = sketch
            .call_method1(
                "addConstraint",
                (fake_constraint("Vertical", -3, 0.0).expect("external"),),
            )
            .expect_err("external geometry");
        assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
        assert!(
            format!("{error}").contains("external geometry"),
            "{error}"
        );

        close_document(py, "RustSketchRootAxis");
    });
}

#[test]
fn find_plane_answers_for_a_wire_and_refuses_a_straight_edge() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let first = line_segment(py, (0.0, 0.0), (2.0, 2.0))
            .call_method0("toShape")
            .expect("toShape");
        let second = line_segment(py, (2.0, 2.0), (2.0, 4.0))
            .call_method0("toShape")
            .expect("toShape");
        let wire = part
            .getattr("Wire")
            .expect("Part.Wire")
            .call1((vec![first.clone(), second],))
            .expect("a wire");

        let plane = wire
            .call_method1("findPlane", (1e-3,))
            .expect("findPlane");
        assert!(!plane.is_none(), "a bent wire lies in a plane");
        let axis = plane.getattr("Axis").expect("Axis");
        let z: f64 = axis.getattr("z").expect("z").extract().expect("z");
        assert!((z.abs() - 1.0).abs() < 1e-9, "wire normal was {axis:?}");

        // A single straight edge has no second direction, so no plane.
        let straight = first
            .call_method1("findPlane", (1e-3,))
            .expect("findPlane");
        assert!(straight.is_none(), "a straight edge has no plane normal");
    });
}

/// Read an int-like attribute that may be a method call's result (`solve`,
/// `getDatum`) or a plain property (`ConstraintCount`) through one path.
fn attribute<T: for<'py> FromPyObject<'py>>(object: &Bound<'_, PyAny>, name: &str) -> T {
    let value = object.getattr(name).expect("attribute");
    let value = if value.is_callable() {
        value.call0().expect("call")
    } else {
        value
    };
    value.extract().expect("value")
}

#[test]
fn sketch_placement_moves_its_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchPlacement");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        add_unit_square(&sketch);
        sketch
            .setattr("Placement", ferrocad_placement(py, (5.0, 0.0, 0.0)))
            .expect("Placement");
        sketch.setattr("MapMode", "Deactivated").expect("MapMode");
        doc.call_method0("recompute").expect("recompute");

        assert_bounds(
            &sketch.getattr("Shape").expect("Shape"),
            [5.0, 0.0, 0.0, 6.0, 1.0, 0.0],
        );
        close_document(py, "RustSketchPlacement");
    });
}

#[test]
fn make_internals_builds_faces_from_the_wires() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchInternals");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");

        // Without `MakeInternals` the internal shape stays null, as upstream.
        add_unit_square(&sketch);
        doc.call_method0("recompute").expect("recompute");
        assert!(
            is_null(&sketch.getattr("InternalShape").expect("InternalShape")),
            "InternalShape is empty unless MakeInternals is set"
        );

        sketch.setattr("MakeInternals", true).expect("MakeInternals");
        doc.call_method0("recompute").expect("recompute");
        let internal = sketch.getattr("InternalShape").expect("InternalShape");
        assert!(!is_null(&internal), "MakeInternals builds bounded faces");
        assert_eq!(count(&internal, "Faces"), 1);
        let area: f64 = internal.getattr("Area").expect("Area").extract().expect("area");
        assert!((area - 1.0).abs() < 1e-7, "area was {area}");

        close_document(py, "RustSketchInternals");
    });
}

#[test]
fn a_construction_only_sketch_has_no_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchConstructionOnly");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (0.0, 0.0), (1.0, 0.0)), true),
            )
            .expect("construction geometry");
        doc.call_method0("recompute").expect("recompute");
        assert!(
            is_null(&sketch.getattr("Shape").expect("Shape")),
            "construction geometry does not build the sketch shape"
        );
        assert_eq!(attribute::<usize>(&sketch, "GeometryCount"), 1);
        assert!(sketch
            .call_method1("getConstruction", (0,))
            .expect("getConstruction")
            .extract::<bool>()
            .expect("flag"));
        close_document(py, "RustSketchConstructionOnly");
    });
}

#[test]
fn add_geometry_is_a_sketch_only_method() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchMethodScope");
        let box_object = doc
            .call_method1("addObject", ("Part::Box", "Box"))
            .expect("add box");
        let error = box_object
            .call_method1("addGeometry", (line_segment(py, (0.0, 0.0), (1.0, 0.0)), false))
            .expect_err("a non-sketch has no addGeometry");
        assert!(error.is_instance_of::<PyAttributeError>(py));
        close_document(py, "RustSketchMethodScope");
    });
}

#[test]
fn body_shape_comes_from_its_last_group_member() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustBodyShape");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("add body");
        let box_object = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Box"))
            .expect("add box");
        box_object.setattr("Length", 10.0).expect("Length");
        box_object.setattr("Width", 10.0).expect("Width");
        box_object.setattr("Height", 10.0).expect("Height");
        body.call_method1("addObject", (&box_object,))
            .expect("body.addObject");

        // One recompute is enough: the body builds its tip on demand when the
        // group ordering would visit it first.
        doc.call_method0("recompute").expect("recompute");
        assert_bounds(
            &body.getattr("Shape").expect("Shape"),
            [0.0, 0.0, 0.0, 10.0, 10.0, 10.0],
        );
        let tip = body.getattr("Tip").expect("Tip");
        assert_eq!(
            tip.getattr("Name").expect("Name").extract::<String>().expect("name"),
            "Box"
        );
        close_document(py, "RustBodyShape");
    });
}

#[test]
fn body_shape_follows_an_explicit_tip() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustBodyTip");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("add body");
        let small = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Small"))
            .expect("add small");
        let large = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Large"))
            .expect("add large");
        large.setattr("Length", 20.0).expect("Length");
        body.call_method1("addObject", (&small,)).expect("add small");
        body.call_method1("addObject", (&large,)).expect("add large");
        body.setattr("Tip", &small).expect("Tip");

        doc.call_method0("recompute").expect("recompute");
        assert_bounds(
            &body.getattr("Shape").expect("Shape"),
            [0.0, 0.0, 0.0, 10.0, 10.0, 10.0],
        );
        close_document(py, "RustBodyTip");
    });
}

#[test]
fn mirroring_reflects_the_source_about_the_plane() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustMirroring");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Box"))
            .expect("add box");
        let mirror = doc
            .call_method1("addObject", ("Part::Mirroring", "Mirror"))
            .expect("add mirror");
        mirror.setattr("Source", &source).expect("Source");
        mirror
            .setattr("Base", ferrocad_vector(py, 0.0, 0.0, 0.0))
            .expect("Base");
        mirror
            .setattr("Normal", ferrocad_vector(py, 1.0, 0.0, 0.0))
            .expect("Normal");
        doc.call_method0("recompute").expect("recompute");

        let shape = mirror.getattr("Shape").expect("Shape");
        assert_bounds(&shape, [-10.0, 0.0, 0.0, 0.0, 10.0, 10.0]);

        // The result must not drift when the document recomputes repeatedly.
        for _ in 0..5 {
            doc.call_method0("recompute").expect("recompute");
        }
        assert_bounds(
            &mirror.getattr("Shape").expect("Shape"),
            [-10.0, 0.0, 0.0, 0.0, 10.0, 10.0],
        );
        close_document(py, "RustMirroring");
    });
}

#[test]
fn ruled_surface_links_sketch_edges() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustSketchRuled");
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("add sketch");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let rotation = ferrocad
            .getattr("Rotation")
            .expect("Rotation")
            .call1((ferrocad_vector(py, 1.0, 0.0, 0.0), 90.0))
            .expect("a rotation");
        sketch
            .setattr(
                "Placement",
                ferrocad
                    .getattr("Placement")
                    .expect("Placement")
                    .call1((ferrocad_vector(py, 0.0, 0.0, 0.0), rotation))
                    .expect("a placement"),
            )
            .expect("Placement");
        sketch.setattr("MapMode", "Deactivated").expect("MapMode");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (-43.475811, 34.364464), (-65.860519, -20.078733)), false),
            )
            .expect("first line");
        sketch
            .call_method1(
                "addGeometry",
                (line_segment(py, (14.004498, 27.390331), (33.577049, -27.952749)), false),
            )
            .expect("second line");

        let ruled = doc
            .call_method1("addObject", ("Part::RuledSurface", "Ruled Surface"))
            .expect("add ruled surface");
        // A link-subscript value is `(object, ["Edge1"])`, as in the upstream test.
        let curve = |name: &str| -> Bound<'_, PyAny> {
            let subs = pyo3::types::PyList::new(py, [name]).expect("sub list");
            pyo3::types::PyTuple::new(
                py,
                [sketch.clone().into_any(), subs.into_any()],
            )
            .expect("curve tuple")
            .into_any()
        };
        ruled.setattr("Curve1", curve("Edge1")).expect("Curve1");
        ruled.setattr("Curve2", curve("Edge2")).expect("Curve2");
        doc.call_method0("recompute").expect("recompute");

        // Every sketch line endpoint is a ruled-surface vertex (upstream's
        // `getCoincidentVertexes` check).
        let sketch_vertices = sketch.getattr("Shape").expect("Shape").getattr("Vertexes").expect("Vertexes");
        let ruled_vertices = ruled.getattr("Shape").expect("Shape").getattr("Vertexes").expect("Vertexes");
        let point = |vertex: &Bound<'_, PyAny>| -> (f64, f64, f64) {
            let point = vertex.getattr("Point").expect("Point");
            (
                point.getattr("x").expect("x").extract().expect("x"),
                point.getattr("y").expect("y").extract().expect("y"),
                point.getattr("z").expect("z").extract().expect("z"),
            )
        };
        let mut coincident = 0;
        for first in sketch_vertices.try_iter().expect("sketch vertices") {
            let a = point(&first.expect("vertex"));
            for second in ruled_vertices.try_iter().expect("ruled vertices") {
                let b = point(&second.expect("vertex"));
                let distance = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                if distance < 1e-7 {
                    coincident += 1;
                }
            }
        }
        assert_eq!(coincident, 4, "the ruled surface shares all four line endpoints");

        close_document(py, "RustSketchRuled");
    });
}

#[test]
fn mirroring_respects_an_app_link_placement() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustMirroringLink");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("add body");
        let box_object = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Box"))
            .expect("add box");
        body.call_method1("addObject", (&box_object,))
            .expect("body.addObject");

        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("add link");
        link.setattr("LinkedObject", &body).expect("LinkedObject");
        link.setattr("Placement", ferrocad_placement(py, (20.0, 0.0, 0.0)))
            .expect("Placement");

        let mirror = doc
            .call_method1("addObject", ("Part::Mirroring", "Mirror"))
            .expect("add mirror");
        mirror.setattr("Source", &link).expect("Source");
        mirror
            .setattr("Base", ferrocad_vector(py, 0.0, 0.0, 0.0))
            .expect("Base");
        mirror
            .setattr("Normal", ferrocad_vector(py, 1.0, 0.0, 0.0))
            .expect("Normal");
        doc.call_method0("recompute").expect("recompute");

        assert_bounds(
            &link.getattr("Shape").expect("Link.Shape"),
            [20.0, 0.0, 0.0, 30.0, 10.0, 10.0],
        );
        assert_bounds(
            &mirror.getattr("Shape").expect("Shape"),
            [-30.0, 0.0, 0.0, -20.0, 10.0, 10.0],
        );
        close_document(py, "RustMirroringLink");
    });
}

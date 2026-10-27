//! The extended `Part` geometry surface: `makeTorus`, `makeWedge`,
//! `makeRevolution`, `makeTube`, `makeHelix`/`makeLongHelix`, `makeThread`,
//! `__sortEdges__`/`sortEdges`, and the document objects `Part::Torus`,
//! `Part::Wedge`, `Part::Revolution`, `Part::Helix` and `Part::Sweep`.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::f64::consts::PI;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

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

fn vector<'py>(py: Python<'py>, x: f64, y: f64, z: f64) -> Bound<'py, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a vector")
}

fn number(value: &Bound<'_, PyAny>, what: &str) -> f64 {
    value
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{what} as f64"))
}

fn shape_type(shape: &Bound<'_, PyAny>) -> String {
    shape
        .getattr("ShapeType")
        .expect("Shape.ShapeType")
        .extract::<String>()
        .expect("ShapeType as str")
}

fn is_valid(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isValid")
        .expect("Shape.isValid")
        .extract::<bool>()
        .expect("isValid as bool")
}

fn volume(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Volume").expect("Shape.Volume"), "Volume")
}

fn area(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Area").expect("Shape.Area"), "Area")
}

fn length(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Length").expect("Shape.Length"), "Length")
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what} was {actual}, expected {expected}"
    );
}

fn bounds(shape: &Bound<'_, PyAny>) -> [f64; 6] {
    let bound_box = shape.getattr("BoundBox").expect("Shape.BoundBox");
    let value = |name: &str| {
        number(
            &bound_box
                .getattr(name)
                .unwrap_or_else(|_| panic!("BoundBox.{name}")),
            name,
        )
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

fn assert_bounds(shape: &Bound<'_, PyAny>, expected: [f64; 6], what: &str) {
    let actual = bounds(shape);
    for (index, (have, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (have - want).abs() < 1e-7,
            "{what} bounds[{index}] was {have}, expected {want} (all: {actual:?})"
        );
    }
}

/// The tight bounds (`Shape.optimalBoundingBox`). A swept B-spline surface's
/// default `BoundBox` follows its control net, which can overshoot a curving
/// sweep even though the surface itself does not.
fn optimal_bounds(shape: &Bound<'_, PyAny>) -> [f64; 6] {
    let bound_box = shape
        .call_method0("optimalBoundingBox")
        .expect("Shape.optimalBoundingBox");
    let value = |name: &str| {
        number(
            &bound_box
                .getattr(name)
                .unwrap_or_else(|_| panic!("BoundBox.{name}")),
            name,
        )
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

/// A closed square wire on the XY plane with its lower-left corner at `corner`.
fn square_wire<'py>(
    part: &Bound<'py, PyModule>,
    py: Python<'py>,
    corner: (f64, f64),
) -> Bound<'py, PyAny> {
    let (x, y) = corner;
    let points = PyList::new(
        py,
        [
            vector(py, x, y, 0.0),
            vector(py, x + 1.0, y, 0.0),
            vector(py, x + 1.0, y + 1.0, 0.0),
            vector(py, x, y + 1.0, 0.0),
        ],
    )
    .expect("point list");
    part.call_method1("makePolygon", (&points, true))
        .expect("Part.makePolygon(points, closed=True)")
}

/// A face from a closed wire.
fn face_from_wire<'py>(part: &Bound<'py, PyModule>, wire: &Bound<'py, PyAny>) -> Bound<'py, PyAny> {
    part.getattr("Face")
        .expect("Part.Face")
        .call1((wire,))
        .expect("Part.Face(wire)")
}

/// An annular face in the XZ plane between radii 4 and 5, from z=0 to z=10.
fn annulus<'py>(part: &Bound<'py, PyModule>, py: Python<'py>) -> Bound<'py, PyAny> {
    let points = PyList::new(
        py,
        [
            vector(py, 4.0, 0.0, 0.0),
            vector(py, 5.0, 0.0, 0.0),
            vector(py, 5.0, 0.0, 10.0),
            vector(py, 4.0, 0.0, 10.0),
        ],
    )
    .expect("point list");
    let wire = part
        .call_method1("makePolygon", (&points, true))
        .expect("annulus wire");
    face_from_wire(part, &wire)
}

/// The distinct endpoint points of an edge.
fn edge_points(edge: &Bound<'_, PyAny>) -> Vec<(f64, f64, f64)> {
    let vertexes = edge.getattr("Vertexes").expect("Edge.Vertexes");
    let mut points = Vec::new();
    for index in 0..vertexes.len().expect("vertex count") {
        let vertex = vertexes.get_item(index).expect("a vertex");
        points.push((
            number(&vertex.getattr("X").expect("Vertex.X"), "X"),
            number(&vertex.getattr("Y").expect("Vertex.Y"), "Y"),
            number(&vertex.getattr("Z").expect("Vertex.Z"), "Z"),
        ));
    }
    points
}

fn points_meet(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 && (a.2 - b.2).abs() < 1e-9
}

/// Whether two edges share an endpoint, i.e. are adjacent in a chain.
fn edges_meet(first: &Bound<'_, PyAny>, second: &Bound<'_, PyAny>) -> bool {
    let a = edge_points(first);
    let b = edge_points(second);
    a.iter().any(|point| b.iter().any(|other| points_meet(*point, *other)))
}

/// The document object's `Shape`, after a recompute.
fn recomputed_shape<'py>(doc: &Bound<'py, PyAny>, name: &str) -> Bound<'py, PyAny> {
    doc.call_method0("recompute").expect("recompute");
    doc.call_method1("getObject", (name,))
        .expect("Document.getObject")
        .getattr("Shape")
        .expect("object.Shape")
}

// ---------------------------------------------------------------------------
// Module functions
// ---------------------------------------------------------------------------

/// `Part.makeTorus(10, 2)`: volume `2·π²·R·r²` and area `4·π²·R·r`.
#[test]
fn make_torus_is_a_solid_torus() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let torus = part
            .call_method1("makeTorus", (10.0, 2.0))
            .expect("Part.makeTorus(10, 2)");

        assert_eq!(shape_type(&torus), "Solid");
        assert!(is_valid(&torus), "the torus is valid");
        assert_close(volume(&torus), 2.0 * PI * PI * 10.0 * 4.0, "torus volume");
        assert_close(area(&torus), 4.0 * PI * PI * 10.0 * 2.0, "torus area");
    });
}

/// `makeTorus(..., angle3=90)` is a quarter of the torus and
/// `angle1=0, angle2=180` a half of its section.
#[test]
fn make_torus_partial_angles() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let full = 2.0 * PI * PI * 10.0 * 4.0;

        let quarter = part
            .call_method1("makeTorus", (10.0, 2.0, py.None(), py.None(), 0.0, 360.0, 90.0))
            .expect("quarter torus");
        assert_eq!(shape_type(&quarter), "Solid");
        assert_close(volume(&quarter), full / 4.0, "quarter torus volume");
        let quarter_bounds = bounds(&quarter);
        assert!(quarter_bounds[0] >= -1e-9, "a +X/+Y quarter starts at x >= 0");
        assert!(quarter_bounds[1] >= -1e-9, "a +X/+Y quarter starts at y >= 0");

        let half_section = part
            .call_method1("makeTorus", (10.0, 2.0, py.None(), py.None(), 0.0, 180.0, 360.0))
            .expect("half-section torus");
        assert_eq!(shape_type(&half_section), "Solid");
        assert_close(
            volume(&half_section),
            full / 2.0,
            "half-section torus volume",
        );
    });
}

/// `pnt` and `dir` place the torus's axis, as upstream's `gp_Ax2`.
#[test]
fn make_torus_is_placed() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let torus = part
            .call_method1(
                "makeTorus",
                (
                    10.0,
                    2.0,
                    vector(py, 100.0, 0.0, 0.0),
                    vector(py, 1.0, 0.0, 0.0),
                ),
            )
            .expect("placed torus");
        let placed = bounds(&torus);
        // Axis along X through (100, 0, 0): the tube spans x in [98, 102].
        assert!((placed[0] - 98.0).abs() < 1e-6, "xmin was {}", placed[0]);
        assert!((placed[3] - 102.0).abs() < 1e-6, "xmax was {}", placed[3]);
        assert_close(volume(&torus), 2.0 * PI * PI * 10.0 * 4.0, "placed volume");
    });
}

/// `Part.makeWedge` builds `BRepPrim_Wedge`'s solid, in both accepted forms.
#[test]
fn make_wedge_builds_the_occ_wedge() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let wedge = part
            .call_method1(
                "makeWedge",
                (0.0, 0.0, 0.0, 2.0, 2.0, 10.0, 10.0, 10.0, 8.0, 8.0),
            )
            .expect("Part.makeWedge(ten parameters)");
        assert_eq!(shape_type(&wedge), "Solid");
        assert!(is_valid(&wedge));
        // The union of the two sections integrated over Y:
        // dx=10, dz=10, dx2=6, dz2=6 at dy=10.
        assert_close(volume(&wedge), 653.3333333333334, "wedge volume");
        assert_bounds(&wedge, [0.0, 0.0, 0.0, 10.0, 10.0, 10.0], "wedge");

        let shorthand = part
            .call_method1("makeWedge", (10.0, 10.0, 10.0, 2.0, 2.0, 8.0, 8.0))
            .expect("Part.makeWedge(seven parameters)");
        assert_eq!(shape_type(&shorthand), "Solid");
        assert_close(volume(&shorthand), 653.3333333333334, "shorthand volume");
        assert_bounds(&shorthand, [0.0, 0.0, 0.0, 10.0, 10.0, 10.0], "shorthand");

        // `pnt` places the primitive, as upstream's `gp_Ax2`.
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs
            .set_item("pnt", vector(py, 100.0, 0.0, 0.0))
            .expect("pnt keyword");
        let placed = part
            .getattr("makeWedge")
            .expect("Part.makeWedge")
            .call((10.0, 10.0, 10.0, 2.0, 2.0, 8.0, 8.0), Some(&kwargs))
            .expect("placed wedge");
        assert_bounds(
            &placed,
            [100.0, 0.0, 0.0, 110.0, 10.0, 10.0],
            "placed wedge",
        );
    });
}

/// Upstream's wedge dimension checks are `ValueError`.
#[test]
fn make_wedge_rejects_degenerate_arguments() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let error = part
            .call_method1(
                "makeWedge",
                (10.0, 0.0, 0.0, 2.0, 2.0, 10.0, 10.0, 10.0, 8.0, 8.0),
            )
            .expect_err("a zero delta x is rejected");
        assert!(
            error.is_instance_of::<PyValueError>(py),
            "expected ValueError, got {error:?}"
        );
        let error = part
            .call_method1(
                "makeWedge",
                (0.0, 0.0, 0.0, 2.0, 2.0, 10.0, 10.0, 10.0, 1.0, 1.0),
            )
            .expect_err("a negative delta x2 is rejected");
        assert!(error.is_instance_of::<PyValueError>(py));
    });
}

/// A face revolved a full turn is a solid, exactly as `Shape.revolve`.
#[test]
fn make_revolution_of_a_face_is_a_solid() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let profile = annulus(&part, py);
        let solid = part
            .call_method1("makeRevolution", (&profile,))
            .expect("Part.makeRevolution(face)");
        assert_eq!(shape_type(&solid), "Solid");
        assert!(is_valid(&solid));
        assert_close(volume(&solid), PI * (25.0 - 16.0) * 10.0, "revolution volume");

        // A half turn gives half the volume and only y >= 0.
        let half = part
            .call_method1("makeRevolution", (&profile, py.None(), py.None(), 180.0))
            .expect("Part.makeRevolution(face, angle=180)");
        assert_eq!(shape_type(&half), "Solid");
        assert_close(
            volume(&half),
            PI * (25.0 - 16.0) * 10.0 / 2.0,
            "half revolution volume",
        );
        assert!(bounds(&half)[1] >= -1e-9, "a +90..180 turn stays at y >= 0");
    });
}

/// An edge revolved a full turn is its face, as upstream's `MakeRevolution`.
#[test]
fn make_revolution_of_an_edge_is_a_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let line = part
            .call_method1(
                "makeLine",
                (vector(py, 4.0, 0.0, 0.0), vector(py, 4.0, 0.0, 10.0)),
            )
            .expect("Part.makeLine");
        let surface = part
            .call_method1("makeRevolution", (&line,))
            .expect("Part.makeRevolution(edge)");
        assert_eq!(shape_type(&surface), "Face");
        assert_close(area(&surface), 2.0 * PI * 4.0 * 10.0, "cylinder area");
        assert_bounds(&surface, [-4.0, -4.0, 0.0, 4.0, 4.0, 10.0], "cylinder");

        // The shorthand form: (shape, axis, angle=360, solid=False).
        let about_x = part
            .call_method1(
                "makeRevolution",
                (&line, vector(py, 1.0, 0.0, 0.0), 360.0, false),
            )
            .expect("Part.makeRevolution(edge, axis)");
        assert_eq!(shape_type(&about_x), "Face");
        let about_x_bounds = bounds(&about_x);
        assert!((about_x_bounds[0] - 4.0).abs() < 1e-9);
        assert!((about_x_bounds[3] - 4.0).abs() < 1e-9);
    });
}

/// `vmin`/`vmax` trim a curve geometry before the revolution.
#[test]
fn make_revolution_trims_a_curve_geometry() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        // A circle of radius 2 in the XZ plane at (10, 0, 0), like a torus
        // cross-section.
        let circle = part
            .getattr("Circle")
            .expect("Part.Circle")
            .call1((
                vector(py, 10.0, 0.0, 0.0),
                vector(py, 0.0, 1.0, 0.0),
                2.0,
            ))
            .expect("Part.Circle");
        let trimmed = part
            .call_method1("makeRevolution", (&circle, 0.0, PI, 360.0))
            .expect("Part.makeRevolution(circle, 0, pi)");
        assert_eq!(shape_type(&trimmed), "Face");
        assert!(area(&trimmed) > 0.0, "the half-torus surface has area");

        // An edge has no parameter query, so its range is reported, not ignored.
        let edge = part
            .call_method1(
                "makeLine",
                (vector(py, 4.0, 0.0, 0.0), vector(py, 4.0, 0.0, 10.0)),
            )
            .expect("Part.makeLine");
        let error = part
            .call_method1("makeRevolution", (&edge, 0.0, 1.0))
            .expect_err("trimming an edge is refused");
        assert!(
            error.to_string().contains("vmin"),
            "the error should name vmin: {error}"
        );
    });
}

/// `Part.makeTube` sweeps a radius-1 circle around a line edge.
#[test]
fn make_tube_around_a_line() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let line = part
            .call_method1(
                "makeLine",
                (vector(py, 4.0, 0.0, 0.0), vector(py, 4.0, 0.0, 10.0)),
            )
            .expect("Part.makeLine");
        let tube = part
            .call_method1("makeTube", (&line, 1.0))
            .expect("Part.makeTube(line, 1)");
        assert_eq!(shape_type(&tube), "Shell");
        assert!(is_valid(&tube));
        assert_close(area(&tube), 2.0 * PI * 1.0 * 10.0, "tube area");
        assert_bounds(&tube, [3.0, -1.0, 0.0, 5.0, 1.0, 10.0], "tube");

        // The third positional argument is the tolerance (upstream's is `scont`).
        let precise = part
            .call_method1("makeTube", (&line, 1.0, 0.01))
            .expect("Part.makeTube(line, 1, 0.01)");
        assert_close(area(&precise), 2.0 * PI * 1.0 * 10.0, "tube area with tolerance");

        // `tolerance` is also a keyword.
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs
            .set_item("tolerance", 0.02)
            .expect("tolerance keyword");
        let keyword = part
            .getattr("makeTube")
            .expect("Part.makeTube")
            .call((&line, 1.0), Some(&kwargs))
            .expect("Part.makeTube(line, 1, tolerance=0.02)");
        assert_close(area(&keyword), 2.0 * PI * 1.0 * 10.0, "tube area with keyword");
    });
}

/// A circle of radius 10 swept with a radius-1 circle is a torus surface.
#[test]
fn make_tube_around_a_circle() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let circle = part
            .call_method1("makeCircle", (10.0,))
            .expect("Part.makeCircle(10)");
        let tube = part
            .call_method1("makeTube", (&circle, 1.0))
            .expect("Part.makeTube(circle, 1)");
        assert_eq!(shape_type(&tube), "Shell");
        assert_close(area(&tube), 4.0 * PI * PI * 10.0 * 1.0, "torus surface area");
    });
}

/// A B-spline spine is evaluated through the kernel's curve adaptor, so the
/// tube can be swept even though `curve_of` has no analytic description for it.
#[test]
fn make_tube_around_a_bspline_spine() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let helix = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0))
            .expect("Part.makeHelix");
        let tube = part
            .call_method1("makeTube", (&helix, 0.5))
            .expect("Part.makeTube(a B-spline spine)");
        assert_eq!(shape_type(&tube), "Shell");
        assert!(is_valid(&tube), "the swept tube is a valid shell");
        assert!(area(&tube) > 0.0, "the tube surface has area");
        let bounds = optimal_bounds(&tube);
        assert!(
            bounds[0] > -3.0 && bounds[3] < 3.0 && bounds[1] > -3.0 && bounds[4] < 3.0,
            "the tube stays around the helix: {bounds:?}"
        );
    });
}

/// `Part.makeHelix` interpolates the analytic helix as a single edge.
#[test]
fn make_helix_is_an_edge_on_the_analytic_helix() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let helix = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0))
            .expect("Part.makeHelix(1, 4, 2)");
        assert_eq!(shape_type(&helix), "Edge");
        assert!(is_valid(&helix));
        // Four turns of pitch 1 at radius 2: 4 * sqrt((2*pi*2)^2 + 1).
        let expected = 4.0 * ((4.0 * PI * PI * 4.0) + 1.0).sqrt();
        assert!(
            (length(&helix) - expected).abs() < 1e-3,
            "helix length was {}, expected {expected}",
            length(&helix)
        );

        let helix_bounds = bounds(&helix);
        assert!((helix_bounds[2] - 0.0).abs() < 1e-9, "starts at z=0");
        assert!((helix_bounds[5] - 4.0).abs() < 1e-9, "ends at z=height");
        // The samples sit on radius 2; interpolation may overshoot slightly.
        assert!(
            helix_bounds[3] >= 1.995 && helix_bounds[3] <= 2.01,
            "xmax was {}",
            helix_bounds[3]
        );

        // The helix starts at (radius, 0, 0) and ends one height up.
        let points = edge_points(&helix);
        assert_eq!(points.len(), 2);
        assert!(
            points.iter().any(|point| points_meet(*point, (2.0, 0.0, 0.0))),
            "one end is (2, 0, 0): {points:?}"
        );
        assert!(
            points.iter().any(|point| points_meet(*point, (2.0, 0.0, 4.0))),
            "the other end is (2, 0, 4): {points:?}"
        );

        // A left-handed helix is the mirrored one.
        let left = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0, -1.0, true, false))
            .expect("left-handed helix");
        assert_eq!(shape_type(&left), "Edge");
        assert!(
            (length(&left) - expected).abs() < 1e-3,
            "left helix length was {}, expected {expected}",
            length(&left)
        );
        assert!(
            !helix
                .call_method1("isSame", (&left,))
                .expect("Shape.isSame")
                .extract::<bool>()
                .expect("isSame as bool"),
            "the two helices are different geometry"
        );
    });
}

/// A conical helix widens as it rises (`angle` is the cone's half-angle).
#[test]
fn make_helix_is_conical() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let helix = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0, 10.0))
            .expect("conical helix");
        assert_eq!(shape_type(&helix), "Edge");
        // Upstream's default (`pvertHeight=false`) runs the cone parameter to
        // `v = height`, so the radius grows by `height * sin(angle)`, not by the
        // axial-rise tangent the old interpolating implementation used.
        let widened = 2.0 + 4.0 * 10.0_f64.to_radians().sin();
        let helix_bounds = bounds(&helix);
        assert!(
            (helix_bounds[3] - widened).abs() < 0.01,
            "xmax was {}, expected about {widened}",
            helix_bounds[3]
        );
    });
}

/// Invalid helix parameters are reported rather than silently accepted.
#[test]
fn make_helix_rejects_invalid_parameters() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let error = part
            .call_method1("makeHelix", (0.0, 4.0, 2.0))
            .expect_err("a zero pitch is rejected");
        assert!(error.to_string().contains("Pitch"), "got {error}");
        let error = part
            .call_method1("makeHelix", (1.0, 4.0, 0.0))
            .expect_err("a zero radius is rejected");
        assert!(error.to_string().contains("Radius"), "got {error}");
        let error = part
            .call_method1("makeHelix", (1.0, -4.0, 2.0))
            .expect_err("a pitch/height sign mismatch is rejected");
        assert!(error.to_string().contains("Pitch and height"), "got {error}");
    });
}

/// `Part.makeLongHelix` splits the same helix into one edge per turn.
#[test]
fn make_long_helix_is_a_wire() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let helix = part
            .call_method1("makeLongHelix", (1.0, 4.0, 2.0))
            .expect("Part.makeLongHelix(1, 4, 2)");
        assert_eq!(shape_type(&helix), "Wire");
        let expected = 4.0 * ((4.0 * PI * PI * 4.0) + 1.0).sqrt();
        assert!(
            (length(&helix) - expected).abs() < 1e-3,
            "long helix length was {}, expected {expected}",
            length(&helix)
        );
        assert_eq!(
            helix.getattr("Edges").expect("Wire.Edges").len().expect("count"),
            4,
            "one edge per whole turn"
        );
    });
}

/// `Part.makeThread` sweeps a circular profile along the helix.
#[test]
fn make_thread_is_a_two_profile_solid() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let thread = part
            .call_method1("makeThread", (1.0, 0.5, 4.0, 2.0))
            .expect("Part.makeThread(1, 0.5, 4, 2)");
        assert_eq!(shape_type(&thread), "Solid");
        assert!(is_valid(&thread));
        assert!(area(&thread) > 0.0);
        // Upstream lofts the two thread-profile wires on the cylinders at
        // `radius` and `radius + depth`, so the solid stays near that band
        // radially. The box is loose because the loft's B-spline control net
        // overshoots the analytic surface (the exact vertex radii are asserted
        // in `ferrocad_occt`'s `make_thread` test).
        let thread_bounds = optimal_bounds(&thread);
        assert!(
            [thread_bounds[0], thread_bounds[1], thread_bounds[3], thread_bounds[4]]
                .iter()
                .all(|value| value.abs() <= 2.6),
            "the thread spans radius 2.0 to radius + depth: {thread_bounds:?}"
        );
        assert!(
            thread_bounds[2] > 0.9 && thread_bounds[5] < 3.5,
            "the thread is centred on height / 2: {thread_bounds:?}"
        );
    });
}

/// `__sortEdges__` orders a shuffled square into one connected chain.
#[test]
fn sort_edges_orders_a_connected_chain() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let wire = square_wire(&part, py, (0.0, 0.0));
        let edges = wire.getattr("Edges").expect("Wire.Edges");
        let shuffled = PyList::new(
            py,
            [
                edges.get_item(2).expect("Edge3"),
                edges.get_item(0).expect("Edge1"),
                edges.get_item(3).expect("Edge4"),
                edges.get_item(1).expect("Edge2"),
            ],
        )
        .expect("shuffled edges");

        let sorted = part
            .call_method1("__sortEdges__", (&shuffled,))
            .expect("Part.__sortEdges__");
        assert_eq!(sorted.len().expect("edge count"), 4);
        for index in 0..3 {
            let first = sorted.get_item(index).expect("an edge");
            let second = sorted.get_item(index + 1).expect("the next edge");
            assert!(
                edges_meet(&first, &second),
                "edges {index} and {} are not adjacent",
                index + 1
            );
        }
        let last = sorted.get_item(3).expect("the last edge");
        let first = sorted.get_item(0).expect("the first edge");
        assert!(edges_meet(&last, &first), "the chain closes");
    });
}

/// `sortEdges` groups disconnected edges into separate chains.
#[test]
fn sort_edges_groups_disconnected_chains() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let near = square_wire(&part, py, (0.0, 0.0));
        let far = square_wire(&part, py, (10.0, 0.0));
        let mut items = Vec::new();
        for wire in [&near, &far] {
            let edges = wire.getattr("Edges").expect("Wire.Edges");
            for index in 0..4 {
                items.push(edges.get_item(index).expect("an edge"));
            }
        }
        let combined = PyList::new(py, items).expect("combined edges");
        let clusters = part
            .call_method1("sortEdges", (&combined,))
            .expect("Part.sortEdges");
        assert_eq!(clusters.len().expect("cluster count"), 2);
        for index in 0..2 {
            assert_eq!(
                clusters
                    .get_item(index)
                    .expect("a cluster")
                    .len()
                    .expect("edge count"),
                4
            );
        }

        let error = part
            .call_method1("sortEdges", (&PyList::new(py, [near]).expect("not edges"),))
            .expect_err("a wire is not an edge");
        assert!(error.is_instance_of::<PyTypeError>(py), "got {error:?}");
    });
}

// ---------------------------------------------------------------------------
// Document objects
// ---------------------------------------------------------------------------

/// `Part::Torus` recomputes into a torus and follows its properties.
#[test]
fn torus_feature_recomputes() {
    boot();
    Python::with_gil(|py| {
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        PyModule::import(py, "Part").expect("import Part");
        let doc = freecad
            .call_method1("newDocument", ("TorusFeature",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::Torus", "Torus"))
            .expect("addObject Part::Torus");

        let shape = recomputed_shape(&doc, "Torus");
        assert_eq!(shape_type(&shape), "Solid");
        assert_close(volume(&shape), 2.0 * PI * PI * 10.0 * 4.0, "default torus");

        object.setattr("Radius1", 5.0).expect("set Radius1");
        object.setattr("Angle3", 180.0).expect("set Angle3");
        let shape = recomputed_shape(&doc, "Torus");
        assert_close(
            volume(&shape),
            2.0 * PI * PI * 5.0 * 4.0 / 2.0,
            "half torus at radius 5",
        );

        freecad
            .call_method1("closeDocument", ("TorusFeature",))
            .expect("closeDocument");
    });
}

/// `Part::Wedge` recomputes into the same solid as `Part.makeWedge`.
#[test]
fn wedge_feature_recomputes() {
    boot();
    Python::with_gil(|py| {
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        PyModule::import(py, "Part").expect("import Part");
        let doc = freecad
            .call_method1("newDocument", ("WedgeFeature",))
            .expect("newDocument");
        doc.call_method1("addObject", ("Part::Wedge", "Wedge"))
            .expect("addObject Part::Wedge");

        let shape = recomputed_shape(&doc, "Wedge");
        assert_eq!(shape_type(&shape), "Solid");
        assert_close(volume(&shape), 653.3333333333334, "default wedge");
        assert_bounds(&shape, [0.0, 0.0, 0.0, 10.0, 10.0, 10.0], "wedge feature");

        freecad
            .call_method1("closeDocument", ("WedgeFeature",))
            .expect("closeDocument");
    });
}

/// `Part::Helix` recomputes into the helix edge and fills in `Length`.
#[test]
fn helix_feature_recomputes() {
    boot();
    Python::with_gil(|py| {
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        PyModule::import(py, "Part").expect("import Part");
        let doc = freecad
            .call_method1("newDocument", ("HelixFeature",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::Helix", "Helix"))
            .expect("addObject Part::Helix");

        let shape = recomputed_shape(&doc, "Helix");
        assert_eq!(shape_type(&shape), "Edge");
        let expected = 2.0 * ((4.0 * PI * PI) + 1.0).sqrt();
        assert!(
            (length(&shape) - expected).abs() < 1e-3,
            "default helix length was {}, expected {expected}",
            length(&shape)
        );
        let reported = number(&object.getattr("Length").expect("Helix.Length"), "Length");
        assert_close(reported, length(&shape), "the Length property");

        freecad
            .call_method1("closeDocument", ("HelixFeature",))
            .expect("closeDocument");
    });
}

/// `Part::Revolution` revolves a linked source, with Base/Axis or an AxisLink.
#[test]
fn revolution_feature_recomputes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = freecad
            .call_method1("newDocument", ("RevolutionFeature",))
            .expect("newDocument");

        let profile = doc
            .call_method1("addObject", ("Part::Feature", "Profile"))
            .expect("addObject profile");
        let face = annulus(&part, py);
        profile.setattr("Shape", &face).expect("set profile shape");

        // The axis edge runs along +Z through the origin.
        let axis = doc
            .call_method1("addObject", ("Part::Feature", "Axis"))
            .expect("addObject axis");
        let axis_shape = part
            .call_method1(
                "makeLine",
                (vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 0.0, 10.0)),
            )
            .expect("the axis line");
        axis.setattr("Shape", &axis_shape).expect("set axis shape");

        let revolution = doc
            .call_method1("addObject", ("Part::Revolution", "Revolution"))
            .expect("addObject Part::Revolution");
        revolution.setattr("Source", &profile).expect("set Source");
        revolution
            .setattr("AxisLink", &axis)
            .expect("set AxisLink");
        revolution.setattr("Angle", 180.0).expect("set Angle");
        revolution.setattr("Solid", true).expect("set Solid");

        let shape = recomputed_shape(&doc, "Revolution");
        assert_eq!(shape_type(&shape), "Solid");
        assert_close(
            volume(&shape),
            PI * (25.0 - 16.0) * 10.0 / 2.0,
            "half revolution volume",
        );
        assert!(bounds(&shape)[1] >= -1e-9, "the half turn stays at y >= 0");

        // Symmetric extends the turn half either side of the profile.
        revolution.setattr("Symmetric", true).expect("set Symmetric");
        let symmetric = recomputed_shape(&doc, "Revolution");
        assert_close(
            volume(&symmetric),
            PI * (25.0 - 16.0) * 10.0 / 2.0,
            "symmetric revolution volume",
        );
        let symmetric_bounds = bounds(&symmetric);
        assert!(
            symmetric_bounds[1] < -1.0 && symmetric_bounds[4] > 1.0,
            "a symmetric turn spans both sides of the profile: {symmetric_bounds:?}"
        );

        freecad
            .call_method1("closeDocument", ("RevolutionFeature",))
            .expect("closeDocument");
    });
}

/// `Part::Extrusion` derives its direction from `DirMode`: a linked straight
/// edge, or the plane normal of the base shape.
#[test]
fn extrusion_dir_modes_use_the_edge_and_the_base_normal() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = freecad
            .call_method1("newDocument", ("ExtrusionDirMode",))
            .expect("newDocument");

        // A 2x2 square in the XZ plane, so +Y is its normal.
        let base = doc
            .call_method1("addObject", ("Part::Feature", "Base"))
            .expect("addObject base");
        let xz_square = || {
            let points = PyList::new(
                py,
                [
                    vector(py, -1.0, 0.0, -1.0),
                    vector(py, 1.0, 0.0, -1.0),
                    vector(py, 1.0, 0.0, 1.0),
                    vector(py, -1.0, 0.0, 1.0),
                ],
            )
            .expect("point list");
            part.call_method1("makePolygon", (points, true))
                .expect("the XZ square")
        };
        base.setattr("Shape", xz_square()).expect("set Base shape");

        // A five-unit line along +Y: the extrusion must follow it, not `Dir`.
        let direction = doc
            .call_method1("addObject", ("Part::Feature", "Direction"))
            .expect("addObject direction");
        direction
            .setattr(
                "Shape",
                part.call_method1(
                    "makeLine",
                    (vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 5.0, 0.0)),
                )
                .expect("the direction line"),
            )
            .expect("set Direction shape");

        let extrusion = doc
            .call_method1("addObject", ("Part::Extrusion", "Extrusion"))
            .expect("addObject Part::Extrusion");
        extrusion.setattr("Base", &base).expect("set Base");
        extrusion.setattr("Solid", true).expect("set Solid");
        extrusion.setattr("LengthFwd", 2.0).expect("set LengthFwd");

        extrusion.setattr("DirLink", &direction).expect("set DirLink");
        extrusion.setattr("DirMode", "Edge").expect("set DirMode=Edge");
        let along_edge = recomputed_shape(&doc, "Extrusion");
        assert_eq!(shape_type(&along_edge), "Solid");
        assert_close(volume(&along_edge), 8.0, "edge-directed extrusion volume");
        assert_bounds(
            &along_edge,
            [-1.0, 0.0, -1.0, 1.0, 2.0, 1.0],
            "the extrusion follows the linked edge",
        );

        // A square in the XY plane has a +Z normal, so Normal mode moves it
        // along Z even though `Dir` still holds the edge's +Y direction.
        base.setattr("Shape", square_wire(&part, py, (-1.0, -1.0)))
            .expect("set the XY base");
        extrusion.setattr("DirMode", "Normal").expect("set DirMode=Normal");
        let along_normal = recomputed_shape(&doc, "Extrusion");
        assert_eq!(shape_type(&along_normal), "Solid");
        assert_close(volume(&along_normal), 2.0, "normal-directed extrusion volume");
        let normal_bounds = bounds(&along_normal);
        let moved_along_z = ((normal_bounds[2]).abs() < 1e-7
            && (normal_bounds[5] - 2.0).abs() < 1e-7)
            || ((normal_bounds[2] + 2.0).abs() < 1e-7 && (normal_bounds[5]).abs() < 1e-7);
        assert!(
            moved_along_z,
            "the extrusion follows the plane normal along Z: {normal_bounds:?}"
        );
        assert!(
            (normal_bounds[0] + 1.0).abs() < 1e-7
                && (normal_bounds[3]).abs() < 1e-7
                && (normal_bounds[1] + 1.0).abs() < 1e-7
                && (normal_bounds[4]).abs() < 1e-7,
            "the extrusion keeps the XY square's X/Y extent: {normal_bounds:?}"
        );

        freecad
            .call_method1("closeDocument", ("ExtrusionDirMode",))
            .expect("closeDocument");
    });
}

/// `Part::Sweep` builds the lateral surface, and a solid when asked.
#[test]
fn sweep_feature_recomputes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = freecad
            .call_method1("newDocument", ("SweepFeature",))
            .expect("newDocument");

        let spine = doc
            .call_method1("addObject", ("Part::Feature", "Spine"))
            .expect("addObject spine");
        let spine_shape = part
            .call_method1(
                "makeLine",
                (vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 0.0, 10.0)),
            )
            .expect("the spine line");
        spine.setattr("Shape", &spine_shape).expect("set spine shape");

        let section = doc
            .call_method1("addObject", ("Part::Feature", "Section"))
            .expect("addObject section");
        let section_shape = square_wire(&part, py, (-1.0, -1.0));
        section
            .setattr("Shape", &section_shape)
            .expect("set section shape");

        let sweep = doc
            .call_method1("addObject", ("Part::Sweep", "Sweep"))
            .expect("addObject Part::Sweep");
        sweep
            .setattr("Spine", &spine)
            .expect("set Spine");
        let sections = PyList::new(py, [&section]).expect("section list");
        sweep.setattr("Sections", sections).expect("set Sections");

        sweep.setattr("Solid", false).expect("set Solid=false");
        let surface = recomputed_shape(&doc, "Sweep");
        assert_eq!(shape_type(&surface), "Shell");
        // A 1x1 square section swept 10 along +Z.
        assert_close(area(&surface), 40.0, "swept lateral area");
        assert_bounds(&surface, [-1.0, -1.0, 0.0, 0.0, 0.0, 10.0], "swept shell");

        sweep.setattr("Solid", true).expect("set Solid=true");
        let solid = recomputed_shape(&doc, "Sweep");
        assert_eq!(shape_type(&solid), "Solid");
        assert_close(volume(&solid), 10.0, "swept solid volume");

        freecad
            .call_method1("closeDocument", ("SweepFeature",))
            .expect("closeDocument");
    });
}

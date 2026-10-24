//! The constructor/overload surface upstream's `TestPartApp` exercises:
//! `Part.Edge()`/`Part.Face()` empty shapes, the default `Part.Line()`, a planar
//! `Part.Plane.toShape(...)`, `Part.Ellipse`, the writable `Circle.Radius`, and the
//! `Part.Geom2d` `Circle2d`/`BSplineCurve2d` additions.
//!
//! Runs in the same image as `part_boot`: `ferrocad` + `Part` as built-ins, one
//! core, one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule, PyType};

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

fn occ_error_type<'py>(py: Python<'py>) -> Bound<'py, PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<PyType>()
        .expect("OCCError is a type")
}

fn point(py: Python<'_>, x: f64, y: f64, z: f64) -> Py<PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a ferrocad.Vector")
        .unbind()
}

#[test]
fn empty_edge_and_face_raise_value_error() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        // `Part.Edge()` is upstream's null edge, and every query on it is a
        // ValueError (upstream's `EmptyEdge`).
        let edge = part.getattr("Edge").unwrap().call0().expect("Part.Edge()");
        assert!(edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        for method in [
            "valueAt",
            "tangentAt",
            "normalAt",
            "curvatureAt",
            "centerOfCurvatureAt",
            "derivative1At",
            "derivative2At",
            "derivative3At",
            "getParameterByLength",
        ] {
            let error = edge.call_method1(method, (0.0,)).expect_err(method);
            assert!(
                error.is_instance_of::<PyValueError>(py),
                "edge.{method} on an empty edge: {error}"
            );
        }
        for attribute in [
            "Tolerance",
            "Length",
            "Curve",
            "ParameterRange",
            "FirstParameter",
            "LastParameter",
        ] {
            let error = edge.getattr(attribute).expect_err(attribute);
            assert!(
                error.is_instance_of::<PyValueError>(py),
                "edge.{attribute} on an empty edge: {error}"
            );
        }
        let error = edge
            .setattr("Tolerance", 0.01)
            .expect_err("Tolerance setter");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        // An empty edge has no geometry to chain into a wire.
        let error = part
            .getattr("Wire")
            .unwrap()
            .call1((&edge,))
            .expect_err("Part.Wire(Part.Edge())");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");

        // `Part.Face()` is the null face, with the same ValueError surface
        // (upstream's `EmptyFace`).
        let face = part.getattr("Face").unwrap().call0().expect("Part.Face()");
        assert!(face.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        for method in [
            "valueAt",
            "normalAt",
            "tangentAt",
            "curvatureAt",
            "derivative1At",
            "derivative2At",
        ] {
            let error = face.call_method1(method, (0.0, 0.0)).expect_err(method);
            assert!(
                error.is_instance_of::<PyValueError>(py),
                "face.{method} on an empty face: {error}"
            );
        }
        let error = face
            .call_method0("getUVNodes")
            .expect_err("face.getUVNodes");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let holes = PyList::empty(py);
        let error = face
            .call_method1("cutHoles", (holes,))
            .expect_err("face.cutHoles");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = face
            .call_method1("makeOffset", (1.0,))
            .expect_err("face.makeOffset");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = face
            .getattr("ParameterRange")
            .expect_err("face.ParameterRange");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = face
            .setattr("Tolerance", 0.01)
            .expect_err("face.Tolerance setter");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn line_defaults_and_projection() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        // `Part.Line()` is upstream's default `Geom_Line`: the Z axis.
        let line = part.getattr("Line").unwrap().call0().expect("Part.Line()");
        let location = line.getattr("Location").unwrap();
        assert!(location.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert!(location.getattr("y").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert!(location.getattr("z").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        let direction = line.getattr("Direction").unwrap();
        assert!(
            (direction.getattr("z").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9,
            "the default direction is +Z"
        );

        // `projectPoint` drops the component across the line.
        let projected = line
            .call_method1("projectPoint", (point(py, 1.0, 2.0, 3.0),))
            .expect("Line.projectPoint");
        assert!(projected.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert!(projected.getattr("y").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert!(
            (projected.getattr("z").unwrap().extract::<f64>().unwrap() - 3.0).abs() < 1e-9
        );
    });
}

#[test]
fn plane_to_shape_builds_a_planar_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plane = part.getattr("Plane").unwrap().call0().expect("Part.Plane()");

        // The parameter rectangle becomes a real four-edge planar face.
        let face = plane
            .call_method1("toShape", (-1.0, 1.0, -1.0, 1.0))
            .expect("Part.Plane.toShape(-1, 1, -1, 1)");
        let face_class = part.getattr("Face").expect("Part.Face");
        assert!(
            face.is_instance(&face_class).unwrap(),
            "Plane.toShape returns a Part.Face"
        );
        assert!(!face.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert_eq!(
            face.getattr("Edges").unwrap().len().unwrap(),
            4,
            "the trimmed plane has four boundary edges"
        );

        // An infinite plane has no natural bounds, so the no-argument form is a
        // clear OCCError rather than an arbitrary patch.
        let error = plane.call_method0("toShape").expect_err("no bounds");
        assert!(error.is_instance(py, &occ_error_type(py)), "{error}");
    });
}

#[test]
fn ellipse_surface_and_writable_circle_radius() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        let ellipse = part
            .getattr("Ellipse")
            .unwrap()
            .call1((point(py, 0.0, 0.0, 0.0), 100.0, 60.0))
            .expect("Part.Ellipse(center, 100, 60)");
        assert_eq!(ellipse.getattr("MajorRadius").unwrap().extract::<f64>().unwrap(), 100.0);
        assert_eq!(ellipse.getattr("MinorRadius").unwrap().extract::<f64>().unwrap(), 60.0);

        // `value(0)` is the point on the major axis, as upstream's `GeomEllipse`.
        let start = ellipse.call_method1("value", (0.0,)).expect("Ellipse.value(0)");
        assert!((start.getattr("x").unwrap().extract::<f64>().unwrap() - 100.0).abs() < 1e-9);
        assert!(start.getattr("y").unwrap().extract::<f64>().unwrap().abs() < 1e-9);

        // The radii upstream's tests assign are writable.
        ellipse.setattr("MinorRadius", 30.0).expect("MinorRadius = 30");
        assert_eq!(ellipse.getattr("MinorRadius").unwrap().extract::<f64>().unwrap(), 30.0);
        ellipse.setattr("MajorRadius", 120.0).expect("MajorRadius = 120");
        assert_eq!(ellipse.getattr("MajorRadius").unwrap().extract::<f64>().unwrap(), 120.0);
        // A minor radius larger than the major one is upstream's construction
        // error, not a silent swap.
        let error = ellipse
            .setattr("MinorRadius", 500.0)
            .expect_err("minor > major");
        assert!(error.is_instance(py, &occ_error_type(py)), "{error}");

        // No `gp_Elips` binding exists, so an exact elliptical edge cannot be
        // built; the failure must be a clear OCCError.
        let error = ellipse.call_method0("toShape").expect_err("no gp_Elips");
        assert!(error.is_instance(py, &occ_error_type(py)), "{error}");

        // `Circle.Radius` has upstream's setter (upstream's `testCircleToNurbs`).
        let circle = part.getattr("Circle").unwrap().call0().expect("Part.Circle()");
        circle.setattr("Radius", 5.0).expect("Radius = 5");
        assert_eq!(circle.getattr("Radius").unwrap().extract::<f64>().unwrap(), 5.0);
    });
}

#[test]
fn geom2d_circle2d_and_bspline_points() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        let vector2d = PyModule::import(py, "ferrocad")
            .expect("import ferrocad")
            .getattr("Vector2d")
            .expect("ferrocad.Vector2d");
        let v2 = |x: f64, y: f64| vector2d.call1((x, y)).expect("Vector2d");

        // Upstream's `PartTestCircle2D`: a valid triple has a centre equidistant
        // from all three points, a collinear triple is a ValueError.
        let circle2d = geom2d.getattr("Circle2d").expect("Part.Geom2d.Circle2d");
        let p1 = v2(0.01, 0.01);
        let p2 = v2(0.02, 0.02);
        let p3 = v2(0.01, -0.01);
        let center = circle2d
            .call_method1("getCircleCenter", (&p1, &p2, &p3))
            .expect("getCircleCenter(valid)");
        let cx = center.getattr("x").unwrap().extract::<f64>().unwrap();
        let cy = center.getattr("y").unwrap().extract::<f64>().unwrap();
        let distance = |p: &Bound<'_, PyAny>| {
            let x = p.getattr("x").unwrap().extract::<f64>().unwrap();
            let y = p.getattr("y").unwrap().extract::<f64>().unwrap();
            ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
        };
        let radius = distance(&p1);
        assert!((distance(&p2) - radius).abs() < 1e-9);
        assert!((distance(&p3) - radius).abs() < 1e-9);

        let c1 = v2(0.01, 0.01);
        let c2 = v2(0.02, 0.02);
        let c3 = v2(0.04, 0.0399);
        let error = circle2d
            .call_method1("getCircleCenter", (&c1, &c2, &c3))
            .expect_err("collinear points");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");

        // Upstream's `BSplineCurve2d` tests pass `Points=` as a keyword.
        let bspline = geom2d
            .getattr("BSplineCurve2d")
            .expect("Part.Geom2d.BSplineCurve2d")
            .call0()
            .expect("BSplineCurve2d()");
        let points = PyList::new(py, [v2(0.0, 0.0), v2(1.0, 0.0)]).unwrap();
        let kwargs = PyDict::new(py);
        kwargs.set_item("Points", points).unwrap();
        bspline
            .call_method("interpolate", (), Some(&kwargs))
            .expect("interpolate(Points=…)");
        bspline
            .call_method("approximate", (), Some(&kwargs))
            .expect("approximate(Points=…)");

        // A lone point is not a curve.
        let one = PyDict::new(py);
        one.set_item("Points", PyList::new(py, [v2(0.0, 0.0)]).unwrap()).unwrap();
        let error = bspline
            .call_method("interpolate", (), Some(&one))
            .expect_err("one point");
        assert!(error.is_instance(py, &occ_error_type(py)), "{error}");
    });
}

#[test]
fn bspline_poles_accept_plain_lists() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let spline = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("BSplineCurve()");

        // Upstream's `PartTestBSplineCurve.setUp` passes `[[0, 0, 0], …]`: a
        // Python list must coerce to a vector, not be read for `.x`.
        let poles = PyList::new(
            py,
            [
                PyList::new(py, [0.0, 0.0, 0.0]).unwrap(),
                PyList::new(py, [1.0, 1.0, 0.0]).unwrap(),
                PyList::new(py, [2.0, 0.0, 0.0]).unwrap(),
            ],
        )
        .unwrap();
        spline
            .call_method1("buildFromPoles", (poles,))
            .expect("buildFromPoles([[0, 0, 0], …]) coerce lists to vectors");
        let edge = spline.call_method0("toShape").expect("spline toShape");
        assert!(!edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
    });
}

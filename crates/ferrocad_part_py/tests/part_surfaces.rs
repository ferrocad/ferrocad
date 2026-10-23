//! `Part.Sphere` and `Part.RectangularTrimmedSurface`: the surface slice upstream's
//! `BRep_tests.py` (`testEdgeSplitFace`, `testProject`) and `Geom2d_tests.py` use.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyType};

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

fn component(value: &Bound<'_, PyAny>, name: &str) -> f64 {
    value
        .getattr(name)
        .unwrap_or_else(|_| panic!("no {name} component"))
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{name} is not a float"))
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

/// Assert `error` is a `Part.OCCError`.
fn assert_occ_error(part: &Bound<'_, PyModule>, error: PyErr) {
    let occ = part.getattr("OCCError").expect("Part.OCCError");
    let occ_type = occ
        .downcast::<PyType>()
        .expect("Part.OCCError is a type");
    assert!(
        error.is_instance(occ.py(), occ_type),
        "expected Part.OCCError, got {error}"
    );
}

#[test]
fn sphere_is_a_surface_with_a_radius() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let sphere_class = part.getattr("Sphere").expect("Part.Sphere");

        // Upstream's default is center (0,0,0), axis +Z, radius 1.
        let sphere = sphere_class.call0().expect("Part.Sphere()");
        assert_close(
            sphere
                .getattr("Radius")
                .expect("Sphere.Radius")
                .extract::<f64>()
                .expect("radius as f64"),
            1.0,
        );
        let center = sphere.getattr("Center").expect("Sphere.Center");
        for axis in ["x", "y", "z"] {
            assert_close(component(&center, axis), 0.0);
        }
        let axis = sphere.getattr("Axis").expect("Sphere.Axis");
        assert_close(component(&axis, "z"), 1.0);

        // The radius/center/axis keywords are this slice's extension.
        let sized = sphere_class.call1((2.5,)).expect("Part.Sphere(2.5)");
        assert_close(
            sized
                .getattr("Radius")
                .expect("Sphere.Radius")
                .extract::<f64>()
                .expect("radius as f64"),
            2.5,
        );

        // It is a surface description, not a B-Rep shape.
        assert!(
            sphere.getattr("Faces").is_err(),
            "Part.Sphere is a surface, not a shape"
        );
    });
}

#[test]
fn line2dsegment_to_shape_accepts_a_sphere() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        // Upstream's `testEdgeSplitFace` points, as (u, v) pairs.
        let segment = part
            .getattr("Geom2d")
            .expect("Part.Geom2d")
            .getattr("Line2dSegment")
            .expect("Part.Geom2d.Line2dSegment")
            .call1(((0.5, -0.5), (1.0, -0.5)))
            .expect("Line2dSegment(p1, p2)");
        let sphere = part
            .getattr("Sphere")
            .expect("Part.Sphere")
            .call0()
            .expect("Part.Sphere()");

        let edge = segment
            .call_method1("toShape", (&sphere,))
            .expect("Line2dSegment.toShape(sphere)");
        assert!(
            !edge
                .call_method0("isNull")
                .expect("Edge.isNull")
                .extract::<bool>()
                .expect("isNull as bool"),
            "toShape(sphere) produced an edge"
        );
        assert!(
            !edge
                .call_method1("curveOnSurface", (0,))
                .expect("curveOnSurface(0)")
                .is_none(),
            "toShape(sphere) keeps the pcurve"
        );
    });
}

#[test]
fn trimmed_plane_to_shape_is_a_four_edge_planar_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plane = part
            .getattr("Plane")
            .expect("Part.Plane")
            .call0()
            .expect("Part.Plane()");
        let trimmed = part
            .getattr("RectangularTrimmedSurface")
            .expect("Part.RectangularTrimmedSurface")
            .call1((&plane, -50.0, 50.0, -50.0, 50.0))
            .expect("Part.RectangularTrimmedSurface(Part.Plane(), ...)");

        // The trim bounds and basis surface are readable.
        assert_close(
            trimmed.getattr("U1").unwrap().extract().unwrap(),
            -50.0,
        );
        assert_close(trimmed.getattr("U2").unwrap().extract().unwrap(), 50.0);
        assert_close(
            trimmed.getattr("V1").unwrap().extract().unwrap(),
            -50.0,
        );
        assert_close(trimmed.getattr("V2").unwrap().extract().unwrap(), 50.0);
        let basis = trimmed.getattr("BasisSurface").expect("BasisSurface");
        assert_eq!(basis.get_type().name().unwrap(), "Plane");
        assert_close(component(&basis.getattr("Axis").unwrap(), "z"), 1.0);

        // toShape() is a real planar face bounded by four edges.
        let face = trimmed.call_method0("toShape").expect("toShape()");
        assert!(
            !face
                .call_method0("isNull")
                .expect("Shape.isNull")
                .extract::<bool>()
                .expect("isNull as bool"),
            "toShape() is not null"
        );
        assert_eq!(
            face.getattr("ShapeType").unwrap().extract::<String>().unwrap(),
            "Face"
        );
        assert_eq!(
            face.getattr("Edges").expect("Face.Edges").len().unwrap(),
            4,
            "the rectangle has four edges"
        );
        assert_close(
            face.getattr("Area").unwrap().extract::<f64>().unwrap(),
            100.0 * 100.0,
        );
        assert_eq!(
            face
                .getattr("Surface")
                .expect("Face.Surface")
                .getattr("TypeId")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Part::GeomPlane"
        );

        // The default plane is the XY plane, so the patch spans -50..50 in X/Y.
        let bounds = face.getattr("BoundBox").expect("Shape.BoundBox");
        assert_close(component(&bounds, "XMin"), -50.0);
        assert_close(component(&bounds, "YMin"), -50.0);
        assert_close(component(&bounds, "ZMin"), 0.0);
        assert_close(component(&bounds, "XMax"), 50.0);
        assert_close(component(&bounds, "YMax"), 50.0);
        assert_close(component(&bounds, "ZMax"), 0.0);
    });
}

#[test]
fn trimmed_plane_uses_the_plane_local_frame() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let yz = part
            .getattr("Plane")
            .expect("Part.Plane")
            .call1((
                ferrocad.getattr("Vector").unwrap().call1((0.0, 0.0, 5.0)).unwrap(),
                ferrocad.getattr("Vector").unwrap().call1((1.0, 0.0, 0.0)).unwrap(),
            ))
            .expect("Part.Plane(origin, +X)");
        let face = part
            .getattr("RectangularTrimmedSurface")
            .expect("Part.RectangularTrimmedSurface")
            .call1((&yz, -2.0, 2.0, -3.0, 3.0))
            .expect("trimmed YZ plane")
            .call_method0("toShape")
            .expect("toShape()");

        // `u` is +Y and `v` is +Z on this plane, so the patch is 4 x 6 in YZ at
        // x = 0, z = 5.
        let bounds = face.getattr("BoundBox").expect("Shape.BoundBox");
        assert_close(component(&bounds, "XMin"), 0.0);
        assert_close(component(&bounds, "XMax"), 0.0);
        assert_close(component(&bounds, "YMin"), -2.0);
        assert_close(component(&bounds, "YMax"), 2.0);
        assert_close(component(&bounds, "ZMin"), 2.0);
        assert_close(component(&bounds, "ZMax"), 8.0);
        assert_close(
            face.getattr("Area").unwrap().extract::<f64>().unwrap(),
            24.0,
        );
    });
}

#[test]
fn trim_bounds_are_validated() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plane = part
            .getattr("Plane")
            .expect("Part.Plane")
            .call0()
            .expect("Part.Plane()");
        let class = part
            .getattr("RectangularTrimmedSurface")
            .expect("Part.RectangularTrimmedSurface");

        // A reversed or empty rectangle is an OCC error (upstream also rejects an
        // empty domain), not a silently empty face.
        let error = class
            .call1((&plane, 50.0, -50.0, -50.0, 50.0))
            .expect_err("u1 < u2 is required");
        assert_occ_error(&part, error);

        // A non-surface basis is a TypeError, as upstream's argument parser.
        let error = class
            .call1((42, -50.0, 50.0, -50.0, 50.0))
            .expect_err("42 is not a surface");
        assert!(
            error.is_instance_of::<PyTypeError>(py),
            "expected TypeError, got {error}"
        );
    });
}

#[test]
fn trimmed_cylinder_and_sphere_describe_but_cannot_make_a_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let class = part
            .getattr("RectangularTrimmedSurface")
            .expect("Part.RectangularTrimmedSurface");

        // Both non-planar bases are constructed and queryable …
        for (name, surface) in [
            (
                "Cylinder",
                part.getattr("Cylinder").unwrap().call0().unwrap(),
            ),
            ("Sphere", part.getattr("Sphere").unwrap().call0().unwrap()),
        ] {
            let trimmed = class
                .call1((&surface, 0.0, 1.0, 0.0, 1.0))
                .unwrap_or_else(|e| panic!("trimmed {name}: {e}"));
            assert_close(trimmed.getattr("U2").unwrap().extract().unwrap(), 1.0);
            assert_eq!(
                trimmed
                    .getattr("BasisSurface")
                    .expect("BasisSurface")
                    .get_type()
                    .name()
                    .unwrap(),
                name
            );

            // … but only a plane has a B-Rep builder in this slice.
            let error = trimmed
                .call_method0("toShape")
                .expect_err("a trimmed non-planar surface has no face builder yet");
            assert_occ_error(&part, error);
        }
    });
}

//! `Part.makePlane`, `Part.Cone`/`Part.Toroid`, `Shape.rotate`, `Part.insert`
//! and the `Part::RuledSurface`/`Part::Extrusion` document types.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::exceptions::PyDeprecationWarning;
use pyo3::prelude::*;
use pyo3::types::PyModule;

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
        .expect("a ferrocad.Vector")
}

fn components(value: &Bound<'_, PyAny>) -> (f64, f64, f64) {
    let component = |name: &str| {
        value
            .getattr(name)
            .unwrap_or_else(|_| panic!("no {name} component"))
            .extract::<f64>()
            .unwrap_or_else(|_| panic!("{name} is not a float"))
    };
    (component("x"), component("y"), component("z"))
}

fn assert_components(value: &Bound<'_, PyAny>, expected: (f64, f64, f64)) {
    let actual = components(value);
    assert!(
        (actual.0 - expected.0).abs() < 1e-9
            && (actual.1 - expected.1).abs() < 1e-9
            && (actual.2 - expected.2).abs() < 1e-9,
        "expected {expected:?}, got {actual:?}"
    );
}

fn f64_of(object: &Bound<'_, PyAny>, name: &str) -> f64 {
    object
        .getattr(name)
        .unwrap_or_else(|_| panic!("no {name}"))
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{name} is not a float"))
}

/// `Part.makePlane(1, 1)` is the upstream `PartTestNormals` fixture: a planar
/// face with a +Z normal and a `Part::GeomPlane` surface.
#[test]
fn make_plane_builds_a_planar_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let face = part
            .call_method1("makePlane", (2.0, 3.0))
            .expect("Part.makePlane(2, 3)");
        assert_eq!(
            face.getattr("ShapeType").unwrap().extract::<String>().unwrap(),
            "Face"
        );
        assert!((f64_of(&face, "Area") - 6.0).abs() < 1e-9);
        assert_components(&face.call_method1("normalAt", (0.0, 0.0)).unwrap(), (0.0, 0.0, 1.0));

        let surface = face.getattr("Surface").expect("Face.Surface");
        assert_eq!(
            surface.getattr("TypeId").unwrap().extract::<String>().unwrap(),
            "Part::GeomPlane"
        );
        assert_components(&surface.call_method1("normal", (0.0, 0.0)).unwrap(), (0.0, 0.0, 1.0));

        // Reversing orients the face, not its surface.
        face.call_method0("reverse").expect("Shape.reverse");
        assert_components(
            &face.call_method1("normalAt", (0.0, 0.0)).unwrap(),
            (0.0, 0.0, -1.0),
        );
        let surface = face.getattr("Surface").expect("Face.Surface");
        assert_components(&surface.call_method1("normal", (0.0, 0.0)).unwrap(), (0.0, 0.0, 1.0));
    });
}

#[test]
fn make_plane_follows_the_given_frame() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let face = part
            .call_method1(
                "makePlane",
                (
                    1.0,
                    2.0,
                    vector(py, 1.0, 2.0, 3.0),
                    vector(py, 0.0, 1.0, 0.0),
                ),
            )
            .expect("Part.makePlane(..., pnt, dir)");
        assert_components(
            &face.call_method1("normalAt", (0.0, 0.0)).unwrap(),
            (0.0, 1.0, 0.0),
        );
        let too_small = part.call_method1("makePlane", (0.0, 1.0)).unwrap_err();
        assert!(too_small.is_instance_of::<pyo3::exceptions::PyValueError>(py));
    });
}

/// `Shape.rotate(center, axis, angle)` rotates the geometry in place (degrees).
#[test]
fn shape_rotate_is_in_place() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("Part.makeBox");
        let result = box_shape
            .call_method1(
                "rotate",
                (vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 0.0, 1.0), 90.0),
            )
            .expect("Shape.rotate");
        assert!(
            result.is(&box_shape),
            "rotate mutates in place and returns the shape (upstream returns IncRef)"
        );

        let bounds = box_shape.getattr("BoundBox").expect("BoundBox");
        assert!((f64_of(&bounds, "XMin") + 1.0).abs() < 1e-9);
        assert!(f64_of(&bounds, "XMax").abs() < 1e-9);
        assert!(f64_of(&bounds, "YMin").abs() < 1e-9);
        assert!((f64_of(&bounds, "YMax") - 1.0).abs() < 1e-9);
    });
}

/// The default `Part.Cone` is radius 1, semi-angle 45°, and its analytic
/// evaluation matches its parameterisation.
#[test]
fn cone_surface_is_analytic() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cone = part.call_method0("Cone").expect("Part.Cone()");
        assert!((f64_of(&cone, "Radius") - 1.0).abs() < 1e-12);
        assert!((f64_of(&cone, "SemiAngle") - std::f64::consts::FRAC_PI_4).abs() < 1e-12);

        cone.setattr("SemiAngle", 0.2).unwrap();
        cone.setattr("Radius", 2.0).unwrap();
        let point = cone.call_method1("value", (5.0, 5.0)).unwrap();
        let (u, v) = cone
            .call_method1("parameter", (&point,))
            .unwrap()
            .extract::<(f64, f64)>()
            .unwrap();
        assert!((u - 5.0).abs() < 1e-9);
        assert!((v - 5.0).abs() < 1e-9);

        // The face handle's surface is the same cone.
        let shape = cone
            .call_method1("toShape", (0.0, 2.0 * std::f64::consts::PI, 0.0, 10.0))
            .unwrap();
        let surface = shape
            .call_method0("toNurbs")
            .unwrap()
            .getattr("Face1")
            .unwrap()
            .getattr("Surface")
            .unwrap();
        let (u, v) = surface
            .call_method1("parameter", (&point,))
            .unwrap()
            .extract::<(f64, f64)>()
            .unwrap();
        assert!((u - 5.0).abs() < 1e-9);
        assert!((v - 5.0).abs() < 1e-9);
    });
}

/// `Part.Toroid().toBSpline()` spans the full `0..2π` parameter range in both
/// directions, and `scaleKnotsToBounds` rescales it.
#[test]
fn toroid_to_bspline_bounds() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let toroid = part.call_method0("Toroid").expect("Part.Toroid()");
        let surface = toroid.call_method0("toBSpline").expect("Toroid.toBSpline");
        surface.call_method0("setUPeriodic").unwrap();
        surface.call_method0("setVPeriodic").unwrap();
        assert!(surface.getattr("UKnotSequence").unwrap().len().unwrap() > 0);
        assert!(surface.getattr("VKnotSequence").unwrap().len().unwrap() > 0);
        let bounds = surface.call_method0("bounds").unwrap();
        let bounds: (f64, f64, f64, f64) = bounds.extract().unwrap();
        assert!((bounds.1 - 2.0 * std::f64::consts::PI).abs() < 1e-12);
        assert!((bounds.3 - 2.0 * std::f64::consts::PI).abs() < 1e-12);

        surface
            .call_method1("scaleKnotsToBounds", (0.0, 1.0, 0.0, 1.0))
            .unwrap();
        let bounds: (f64, f64, f64, f64) =
            surface.call_method0("bounds").unwrap().extract().unwrap();
        assert!((bounds.1 - 1.0).abs() < 1e-12);
        assert!((bounds.3 - 1.0).abs() < 1e-12);
    });
}

/// `Part.insert` validates the extension, then warns; when warnings are errors
/// the warning propagates and no document is created (upstream's lifecycle test).
#[test]
fn insert_warns_before_creating_a_document() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let no_extension = part
            .call_method1("insert", ("missing", "InsertNoExt"))
            .expect_err("a bare name has no extension");
        assert!(no_extension.is_instance_of::<pyo3::exceptions::PyRuntimeError>(py));

        let warnings = py.import("warnings").unwrap();
        let context = warnings.call_method0("catch_warnings").unwrap();
        context.call_method0("__enter__").unwrap();
        warnings
            .call_method1(
                "simplefilter",
                ("error", py.get_type::<PyDeprecationWarning>()),
            )
            .unwrap();
        let result = part.call_method1("insert", ("missing.step", "InsertDoc"));
        context
            .call_method1("__exit__", (py.None(), py.None(), py.None()))
            .unwrap();
        let error = result.expect_err("the deprecation warning is an error here");
        assert!(error.is_instance_of::<PyDeprecationWarning>(py));
        assert_eq!(
            error.value(py).str().unwrap().to_string(),
            "Method 'Part.insert' is deprecated since FreeCAD 26.3 and will be removed in \
             FreeCAD 27.2; use Import.insert instead."
        );
        let documents = freecad.call_method0("listDocuments").unwrap();
        let documents = documents.downcast::<pyo3::types::PyDict>().unwrap();
        assert!(documents.get_item("InsertDoc").unwrap().is_none());
    });
}

/// The document types produce a `Shape` on recompute: a ruled face between two
/// lines, and a solid from extruding a closed square wire.
#[test]
fn ruled_surface_and_extrusion_recompute() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = freecad
            .call_method1("newDocument", ("SurfaceConstructors",))
            .expect("newDocument");

        let line1 = part
            .call_method1(
                "makeLine",
                (vector(py, 0.0, 0.0, 0.0), vector(py, 1.0, 0.0, 0.0)),
            )
            .unwrap();
        let line2 = part
            .call_method1(
                "makeLine",
                (vector(py, 0.0, 1.0, 0.0), vector(py, 1.0, 1.0, 1.0)),
            )
            .unwrap();
        let feature1 = doc
            .call_method1("addObject", ("Part::Feature", "Line1"))
            .unwrap();
        feature1.setattr("Shape", &line1).unwrap();
        let feature2 = doc
            .call_method1("addObject", ("Part::Feature", "Line2"))
            .unwrap();
        feature2.setattr("Shape", &line2).unwrap();

        let ruled = doc
            .call_method1("addObject", ("Part::RuledSurface", "Ruled"))
            .unwrap();
        ruled.setattr("Curve1", &feature1).unwrap();
        ruled.setattr("Curve2", &feature2).unwrap();

        let wire = part
            .call_method1(
                "makePolygon",
                (
                    pyo3::types::PyList::new(
                        py,
                        [
                            vector(py, 0.0, 0.0, 0.0),
                            vector(py, 1.0, 0.0, 0.0),
                            vector(py, 1.0, 1.0, 0.0),
                            vector(py, 0.0, 1.0, 0.0),
                        ],
                    )
                    .unwrap(),
                    true,
                ),
            )
            .unwrap();
        let base = doc
            .call_method1("addObject", ("Part::Feature", "Base"))
            .unwrap();
        base.setattr("Shape", &wire).unwrap();
        let extrusion = doc
            .call_method1("addObject", ("Part::Extrusion", "Extrude"))
            .unwrap();
        extrusion.setattr("Base", &base).unwrap();
        extrusion.setattr("LengthFwd", 5.0).unwrap();
        extrusion.setattr("Solid", true).unwrap();

        doc.call_method0("recompute").expect("recompute");

        let ruled_shape = ruled.getattr("Shape").expect("RuledSurface.Shape");
        assert_eq!(
            ruled_shape
                .getattr("ShapeType")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Face"
        );
        // Two lines, each with two endpoints, bound the ruled face.
        assert_eq!(ruled_shape.getattr("Vertexes").unwrap().len().unwrap(), 4);

        let extruded = extrusion.getattr("Shape").expect("Extrusion.Shape");
        assert!((f64_of(&extruded, "Volume") - 5.0).abs() < 1e-6);
        assert!(
            extruded
                .call_method0("isValid")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );

        freecad
            .call_method1("closeDocument", ("SurfaceConstructors",))
            .unwrap();
    });
}

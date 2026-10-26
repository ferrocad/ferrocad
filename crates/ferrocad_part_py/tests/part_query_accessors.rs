//! The P4 accessor round: typed `Solids`/`Shells`, null-shape getters, the
//! `getTolerance` modes, `Shape.translate`/`check`/`Tolerance`, the surface
//! periodicity/value/derivative queries, and the `App` placement/material gaps
//! SimpleCAD exercises.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

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

#[test]
fn typed_solids_shells_and_empty_shape_getters() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let boxed = part
            .getattr("makeBox")
            .unwrap()
            .call1((1.0, 2.0, 3.0))
            .unwrap();

        let solids = boxed.getattr("Solids").unwrap();
        assert_eq!(solids.len().unwrap(), 1);
        assert_eq!(
            solids.get_item(0).unwrap().get_type().name().unwrap(),
            "Solid"
        );

        let shells = boxed.getattr("Shells").unwrap();
        assert_eq!(shells.len().unwrap(), 1);
        assert_eq!(
            shells.get_item(0).unwrap().get_type().name().unwrap(),
            "Shell"
        );

        // A null shape has no sub-shapes: every getter is an empty list.
        let null = part.getattr("Shape").unwrap().call0().unwrap();
        for name in ["Edges", "Wires", "Faces", "Vertexes", "Solids", "Shells", "Compounds"] {
            assert_eq!(
                null.getattr(name).unwrap().len().unwrap(),
                0,
                "Part.Shape().{name} must be empty"
            );
        }
        assert_eq!(null.call_method0("childShapes").unwrap().len().unwrap(), 0);
    });
}

#[test]
fn tolerance_modes_follow_the_upstream_definition() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let boxed = part
            .getattr("makeBox")
            .unwrap()
            .call1((1.0, 2.0, 3.0))
            .unwrap();
        let average = boxed
            .call_method1("getTolerance", (0,))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        let maximum = boxed
            .call_method1("getTolerance", (1,))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        let minimum = boxed
            .call_method1("getTolerance", (-1,))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!(maximum > 0.0);
        // A box's sub-shapes all share one tolerance, so the average equals it
        // up to the accumulation error.
        let slack = maximum * 1e-12;
        assert!(minimum <= average + slack && average <= maximum + slack);

        // A type filter restricts the considered sub-shapes.
        let vertex = part.getattr("Vertex").unwrap();
        let vertex_maximum = boxed
            .call_method1("getTolerance", (1, vertex))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!(vertex_maximum > 0.0);
        assert!(vertex_maximum <= maximum);

        // `Shape.Tolerance` is the whole-shape tolerance.
        let tolerance = f64_of(&boxed, "Tolerance");
        assert!((tolerance - maximum).abs() < 1e-12);
    });
}

#[test]
fn translate_check_and_material_and_placement_gaps() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let app = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let boxed = part
            .getattr("makeBox")
            .unwrap()
            .call1((1.0, 2.0, 3.0))
            .unwrap();

        // `Shape.translate` mutates in place and returns the shape.
        let before = f64_of(&boxed.getattr("BoundBox").unwrap(), "XMin");
        let moved = boxed
            .call_method1("translate", (vector(py, 4.0, 0.0, 0.0),))
            .unwrap();
        let after = f64_of(&moved.getattr("BoundBox").unwrap(), "XMin");
        assert!((after - before - 4.0).abs() < 1e-9);

        // `check()` is quiet for a valid box.
        assert!(boxed.call_method0("check").unwrap().is_none());

        // `Placement.multiply` composes `self * other` ...
        let placement = app.getattr("Placement").unwrap();
        let first = placement.call1((vector(py, 1.0, 0.0, 0.0),)).unwrap();
        let second = placement.call1((vector(py, 0.0, 2.0, 0.0),)).unwrap();
        let product = first.call_method1("multiply", (second,)).unwrap();
        assert_components(&product.getattr("Base").unwrap(), (1.0, 2.0, 0.0));

        // ... and `Placement(base, rotation, center)` rotates about `center`.
        let rotation = app
            .getattr("Rotation")
            .unwrap()
            .call1((vector(py, 0.0, 0.0, 1.0), 90.0))
            .unwrap();
        let about = placement
            .call1((vector(py, 0.0, 0.0, 0.0), rotation, vector(py, 1.0, 0.0, 0.0)))
            .unwrap();
        assert_components(&about.getattr("Base").unwrap(), (1.0, -1.0, 0.0));

        // `Material.Transparency` is the complement of the diffuse alpha.
        let material = app.getattr("Material").unwrap().call0().unwrap();
        material.setattr("Transparency", 0.25).unwrap();
        assert!((f64_of(&material, "Transparency") - 0.25).abs() < 1e-12);
        let alpha = material
            .getattr("DiffuseColor")
            .unwrap()
            .get_item(3)
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!((alpha - 0.75).abs() < 1e-12);
    });
}

#[test]
fn surface_period_value_derivatives_and_identity() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder = part.getattr("Cylinder").unwrap().call1((2.0,)).unwrap();

        assert!(cylinder
            .call_method0("isUPeriodic")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert!(!cylinder
            .call_method0("isVPeriodic")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        let period = cylinder
            .call_method0("UPeriod")
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!((period - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(
            cylinder
                .call_method0("VPeriod")
                .unwrap()
                .extract::<f64>()
                .unwrap(),
            0.0
        );

        // P(u, v) = C + r(cos u·X + sin u·Y) + v·A on the default frame.
        assert_components(
            &cylinder.call_method1("value", (0.0, 3.0)).unwrap(),
            (2.0, 0.0, 3.0),
        );
        assert_components(
            &cylinder
                .call_method1("value", (std::f64::consts::FRAC_PI_2, 0.0))
                .unwrap(),
            (0.0, 2.0, 0.0),
        );

        // Principal curvatures of a radius-2 cylinder.
        let max = cylinder
            .call_method1("curvature", (0.0, 0.0, "Max"))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        let mean = cylinder
            .call_method1("curvature", (0.0, 0.0, "Mean"))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!((max - 0.5).abs() < 1e-12);
        assert!((mean - 0.25).abs() < 1e-12);

        // `Plane.isSame` compares origin and frame, not identity.
        let plane = part.getattr("Plane").unwrap();
        let first = plane.call0().unwrap();
        let same = plane
            .call1((vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 0.0, 2.0)))
            .unwrap();
        let shifted = plane
            .call1((vector(py, 0.0, 0.0, 1.0), vector(py, 0.0, 0.0, 1.0)))
            .unwrap();
        assert!(first
            .call_method1("isSame", (same,))
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert!(!first
            .call_method1("isSame", (shifted,))
            .unwrap()
            .extract::<bool>()
            .unwrap());
    });
}

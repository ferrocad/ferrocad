//! `Part.Plane` and plane/plane intersection, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::exceptions::PyIndexError;
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

/// A `ferrocad.Vector` (`Base.Vector` in upstream terms).
fn vector<'py>(py: Python<'py>, x: f64, y: f64, z: f64) -> Bound<'py, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a ferrocad.Vector")
}

/// The `(x, y, z)` components of a vector-like Python object.
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

/// The upstream `regression_tests.py::test_issue_4456` case: the default XY plane
/// meets the YZ plane in a line through the origin along +Y, and the returned
/// list is exhausted after that line.
#[test]
fn default_plane_intersects_the_yz_plane_in_a_line() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plane = part.getattr("Plane").expect("Part.Plane");

        // Part.Plane() is the XY plane: origin (0,0,0), normal (0,0,1).
        let p1 = plane.call0().expect("Part.Plane()");
        let p2 = plane
            .call1((vector(py, 0.0, 0.0, 0.0), vector(py, 1.0, 0.0, 0.0)))
            .expect("Part.Plane(Vector(0,0,0), Vector(1,0,0))");

        assert_components(&p1.getattr("Location").expect("Location"), (0.0, 0.0, 0.0));
        assert_components(&p1.getattr("Axis").expect("Axis"), (0.0, 0.0, 1.0));

        let result = p1
            .call_method1("intersect", (&p2,))
            .expect("Plane.intersect(plane)");
        assert_eq!(result.len().expect("list length"), 1, "one shared line");

        let line = result.call_method0("pop").expect("pop the line");
        assert_components(&line.getattr("Location").expect("Location"), (0.0, 0.0, 0.0));
        // n1 × n2 = (0,1,0): the +Y direction this test pins down.
        assert_components(&line.getattr("Direction").expect("Direction"), (0.0, 1.0, 0.0));

        let error = result
            .call_method0("pop")
            .expect_err("the intersection list is exhausted");
        assert!(
            error.is_instance_of::<PyIndexError>(py),
            "popping an empty list raises IndexError, got {error}"
        );
    });
}

#[test]
fn parallel_and_coincident_planes_do_not_intersect() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plane = part.getattr("Plane").expect("Part.Plane");
        let p1 = plane.call0().expect("Part.Plane()");

        // z = 1 is parallel to the XY plane at z = 0.
        let parallel = plane
            .call1((vector(py, 0.0, 0.0, 1.0), vector(py, 0.0, 0.0, 1.0)))
            .expect("Part.Plane(z = 1)");
        let result = p1
            .call_method1("intersect", (&parallel,))
            .expect("parallel intersect");
        assert_eq!(result.len().expect("list length"), 0);

        // The same plane twice is coincident too: still no line.
        let result = p1.call_method1("intersect", (&p1,)).expect("self intersect");
        assert_eq!(result.len().expect("list length"), 0);
    });
}

#[test]
fn a_zero_normal_is_rejected() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let zero = vector(py, 0.0, 0.0, 0.0);
        let error = part
            .getattr("Plane")
            .expect("Part.Plane")
            .call1((&zero, &zero))
            .expect_err("a zero normal has no direction");

        let occ = part.getattr("OCCError").expect("Part.OCCError");
        let occ_type = occ.downcast::<pyo3::types::PyType>().expect("Part.OCCError is a type");
        assert!(
            error.is_instance(py, occ_type),
            "a zero normal raises Part.OCCError, got {error}"
        );
    });
}

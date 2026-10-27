//! `Face.Surface` classification: a planar face is a `Part.Plane`, a cylindrical
//! lateral face a `Part.Cylinder` and a spherical face a `Part.Sphere`, with
//! `parameter(point)` agreeing with `Face.valueAt` and `normalAt`.
//!
//! Same boot as `part_surfaces`: `ferrocad` + `Part` as built-ins. It needs OCCT
//! (see `docs/developer/architecture/occt-bundling.md`).

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

/// The `name` sub-shapes of a `Part.Shape`.
fn sub_shapes<'py>(shape: &Bound<'py, PyAny>, name: &str) -> Vec<Bound<'py, PyAny>> {
    shape
        .getattr(name)
        .unwrap_or_else(|error| panic!("Shape.{name}: {error}"))
        .try_iter()
        .unwrap_or_else(|error| panic!("iter(Shape.{name}): {error}"))
        .map(|item| item.expect("a sub-shape"))
        .collect()
}

/// A named sub-shape (`Shape.getElement("Face1")`), the typed `Part.Face` wrapper.
fn element<'py>(shape: &Bound<'py, PyAny>, name: &str) -> Bound<'py, PyAny> {
    shape
        .call_method1("getElement", (name,))
        .unwrap_or_else(|error| panic!("Shape.getElement({name}): {error}"))
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

fn dot(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
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

fn assert_close(actual: &Bound<'_, PyAny>, expected: &Bound<'_, PyAny>, tolerance: f64) {
    let a = components(actual);
    let b = components(expected);
    let distance = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
    assert!(distance < tolerance, "{a:?} is {distance} from {b:?}");
}

/// Whether `value` is an instance of `Part.<class>`.
fn is_instance(value: &Bound<'_, PyAny>, part: &Bound<'_, PyModule>, class: &str) -> bool {
    let class = part
        .getattr(class)
        .unwrap_or_else(|error| panic!("Part.{class}: {error}"));
    value
        .is_instance(&class)
        .unwrap_or_else(|error| panic!("isinstance({value}, Part.{class}): {error}"))
}

/// The parameters `(u, v)` of a point on `face`.
fn parameters(face: &Bound<'_, PyAny>, point: &Bound<'_, PyAny>) -> (f64, f64) {
    face.getattr("Surface")
        .expect("Face.Surface")
        .call_method1("parameter", (point,))
        .expect("Surface.parameter(point)")
        .extract()
        .expect("(u, v)")
}

/// The face's parameter range as two middles, `(u, v)`.
fn middle_parameters(face: &Bound<'_, PyAny>) -> (f64, f64) {
    let (u0, u1, v0, v1): (f64, f64, f64, f64) = face
        .getattr("ParameterRange")
        .expect("Face.ParameterRange")
        .extract()
        .expect("(u0, u1, v0, v1)");
    (0.5 * (u0 + u1), 0.5 * (v0 + v1))
}

/// `valueAt(*parameter(valueAt(u, v)))` comes back to the same point.
fn assert_parameter_round_trip(face: &Bound<'_, PyAny>, u: f64, v: f64, tolerance: f64) {
    let point = face
        .call_method1("valueAt", (u, v))
        .expect("Face.valueAt(u, v)");
    let (u2, v2) = parameters(face, &point);
    let back = face
        .call_method1("valueAt", (u2, v2))
        .expect("Face.valueAt(parameter)");
    assert_close(&back, &point, tolerance);
}

/// `Face.normalAt(u, v)` is parallel to the analytic normal `expected`.
fn assert_normal_parallel(face: &Bound<'_, PyAny>, u: f64, v: f64, expected: (f64, f64, f64)) {
    let normal = components(
        &face
            .call_method1("normalAt", (u, v))
            .expect("Face.normalAt(u, v)"),
    );
    let length = (normal.0.powi(2) + normal.1.powi(2) + normal.2.powi(2)).sqrt();
    assert!(
        (length - 1.0).abs() < 1e-9,
        "normal is not unit: {normal:?}"
    );
    assert!(
        dot(normal, expected).abs() > 1.0 - 1e-9,
        "normal {normal:?} is not parallel to {expected:?}"
    );
}

/// The radial direction from `center` to `point`, normalized.
fn radial(point: (f64, f64, f64), center: (f64, f64, f64)) -> (f64, f64, f64) {
    let d = (point.0 - center.0, point.1 - center.1, point.2 - center.2);
    let length = (d.0.powi(2) + d.1.powi(2) + d.2.powi(2)).sqrt();
    (d.0 / length, d.1 / length, d.2 / length)
}

#[test]
fn a_box_face_surface_is_a_plane_and_parameter_round_trips_exactly() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (10.0, 20.0, 30.0))
            .expect("Part.makeBox(10, 20, 30)");
        let faces = sub_shapes(&box_shape, "Faces");
        let raw_surface = faces[0].getattr("Surface").expect("Face.Surface");
        assert!(
            is_instance(&raw_surface, &part, "Plane"),
            "box.Faces[0].Surface is a Part.Plane"
        );
        let face = element(&box_shape, "Face1");
        let surface = face.getattr("Surface").expect("Face.Surface");

        assert!(
            is_instance(&surface, &part, "Plane"),
            "a box face is a Part.Plane, got {}",
            surface.get_type().name().expect("type name")
        );
        assert_eq!(
            surface
                .getattr("TypeId")
                .expect("Plane.TypeId")
                .extract::<String>()
                .expect("TypeId as str"),
            "Part::GeomPlane"
        );

        // The elementary plane reads back through its public properties.
        let location = surface.getattr("Location").expect("Plane.Location");
        assert_components(&location, components(&location));
        assert_components(
            &surface.getattr("Position").expect("Plane.Position"),
            components(&location),
        );
        let axis = components(&surface.getattr("Axis").expect("Plane.Axis"));
        // `normal` is upstream's `GeomSurface.normal(u, v)` method (constant on a
        // plane), not a property; `Axis` is the property form.
        let normal = components(
            &surface
                .call_method1("normal", (0.0, 0.0))
                .expect("Plane.normal(u, v)"),
        );
        assert!(
            dot(axis, normal).abs() > 1.0 - 1e-9,
            "Axis {axis:?} and normal {normal:?} differ"
        );

        let (u, v) = middle_parameters(&face);
        assert_parameter_round_trip(&face, u, v, 1e-12);
        // A plane is affine in (u, v), so the parameters are exact.
        let point = face
            .call_method1("valueAt", (u, v))
            .expect("Face.valueAt(u, v)");
        let (u2, v2) = parameters(&face, &point);
        assert!((u2 - u).abs() < 1e-12, "u {u} -> {u2}");
        assert!((v2 - v).abs() < 1e-12, "v {v} -> {v2}");

        assert_normal_parallel(&face, u, v, normal);
    });
}

#[test]
fn a_cylinders_lateral_face_surface_is_a_cylinder_and_parameter_round_trips() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder = part
            .call_method1("makeCylinder", (2.0, 5.0))
            .expect("Part.makeCylinder(2, 5)");
        let faces = sub_shapes(&cylinder, "Faces");
        assert!(
            faces.iter().any(|face| is_instance(
                &face.getattr("Surface").expect("Face.Surface"),
                &part,
                "Cylinder",
            )),
            "one of cylinder.Faces is a Part.Cylinder"
        );
        let face = (1..=faces.len())
            .map(|index| element(&cylinder, &format!("Face{index}")))
            .find(|face| {
                is_instance(
                    &face.getattr("Surface").expect("Face.Surface"),
                    &part,
                    "Cylinder",
                )
            })
            .expect("the lateral face is a Part.Cylinder");
        let surface = face.getattr("Surface").expect("Face.Surface");

        assert_eq!(
            surface
                .getattr("TypeId")
                .expect("Cylinder.TypeId")
                .extract::<String>()
                .expect("TypeId as str"),
            "Part::GeomCylinder"
        );
        let radius = surface
            .getattr("Radius")
            .expect("Cylinder.Radius")
            .extract::<f64>()
            .expect("Radius as float");
        assert!((radius - 2.0).abs() < 1e-12, "radius = {radius}");
        let axis = components(&surface.getattr("Axis").expect("Cylinder.Axis"));
        assert!((axis.2.abs() - 1.0).abs() < 1e-9, "axis = {axis:?}");
        let center = components(&surface.getattr("Center").expect("Cylinder.Center"));

        let (u, v) = middle_parameters(&face);
        assert_parameter_round_trip(&face, u, v, 1e-9);
        assert_parameter_round_trip(&face, 0.7, 2.0, 1e-9);

        // The normal is radial: project the surface point onto the axis plane.
        let point = components(
            &face
                .call_method1("valueAt", (u, v))
                .expect("Face.valueAt(u, v)"),
        );
        let offset = (point.0 - center.0, point.1 - center.1, point.2 - center.2);
        let along = dot(offset, axis);
        let foot = (
            center.0 + axis.0 * along,
            center.1 + axis.1 * along,
            center.2 + axis.2 * along,
        );
        assert_normal_parallel(&face, u, v, radial(point, foot));
    });
}

#[test]
fn a_sphere_face_surface_is_a_sphere_and_parameter_round_trips() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let sphere = part
            .call_method1("makeSphere", (2.0,))
            .expect("Part.makeSphere(2)");
        let faces = sub_shapes(&sphere, "Faces");
        let raw_surface = faces[0].getattr("Surface").expect("Face.Surface");
        assert!(
            is_instance(&raw_surface, &part, "Sphere"),
            "sphere.Faces[0].Surface is a Part.Sphere"
        );
        let face = element(&sphere, "Face1");
        let surface = face.getattr("Surface").expect("Face.Surface");

        assert!(
            is_instance(&surface, &part, "Sphere"),
            "a sphere face is a Part.Sphere, got {}",
            surface.get_type().name().expect("type name")
        );
        assert_eq!(
            surface
                .getattr("TypeId")
                .expect("Sphere.TypeId")
                .extract::<String>()
                .expect("TypeId as str"),
            "Part::GeomSphere"
        );
        let radius = surface
            .getattr("Radius")
            .expect("Sphere.Radius")
            .extract::<f64>()
            .expect("Radius as float");
        assert!((radius - 2.0).abs() < 1e-12, "radius = {radius}");
        let axis = components(&surface.getattr("Axis").expect("Sphere.Axis"));
        assert!((axis.2.abs() - 1.0).abs() < 1e-9, "axis = {axis:?}");
        let center = components(&surface.getattr("Center").expect("Sphere.Center"));

        for (u, v) in [(0.8, 0.4), (4.0, -1.2)] {
            assert_parameter_round_trip(&face, u, v, 1e-9);
            let point = components(
                &face
                    .call_method1("valueAt", (u, v))
                    .expect("Face.valueAt(u, v)"),
            );
            assert_normal_parallel(&face, u, v, radial(point, center));
        }
    });
}

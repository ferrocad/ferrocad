//! The live `Shape.Placement` model.
//!
//! These mirror the two upstream cases that motivated it
//! (`PartTestNormals.testPlacement` and `PartTestShapeRotate.testPlacement`), plus
//! the composition rules the rest of the API relies on:
//!
//! - the geometry is baked (`geometry == placement · base`), so queries see it;
//! - `shape.Placement` is a **live** view — `Base`, `Rotation.Angle`, `Rotation.Axis`
//!   and even `Base.x` write through and re-bake the geometry;
//! - `Shape.rotate`/`translated` compose `P_new = loc · P_old`, while
//!   `Placement.rotate(..., comp=False)` composes `P_new = P_old · loc`.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. Needs OCCT (see `docs/architecture/occt-bundling.md`).

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

fn dot(_py: Python<'_>, a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>) -> f64 {
    a.call_method1("dot", (b,))
        .expect("Vector.dot")
        .extract()
        .expect("a float")
}

#[test]
fn placement_mutation_moves_the_geometry_and_keeps_the_surface_frame() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("Vector");

        let face = part.call_method1("makePlane", (1.0, 1.0)).expect("makePlane");
        face.call_method0("reverse").expect("reverse");

        // The upstream test mutates through two *fresh* `Placement` views:
        // `face.Placement.Rotation.Angle = 1`, then `...Axis = (1, 1, 1)`.
        face.getattr("Placement")
            .unwrap()
            .getattr("Rotation")
            .unwrap()
            .setattr("Angle", 1.0)
            .expect("Placement.Rotation.Angle =");
        let axis = vector.call1((1.0, 1.0, 1.0)).expect("Vector(1,1,1)");
        face.getattr("Placement")
            .unwrap()
            .getattr("Rotation")
            .unwrap()
            .setattr("Axis", axis)
            .expect("Placement.Rotation.Axis =");

        // The rotated (reversed) face normal: Rodrigues(1 rad about (1,1,1)) · -Z.
        let expected = vector
            .call1((-0.63905, 0.33259, -0.69353))
            .expect("expected normal");
        let normal = face
            .call_method1("normalAt", (0.0, 0.0))
            .expect("normalAt");
        assert!(
            dot(py, &normal, &expected) > 0.9999,
            "normalAt must follow the live placement"
        );

        // `Surface` is the un-oriented surface of the placed face, so it is the
        // negative of `normalAt` (the face was reversed) but still rotated.
        let surface = face.getattr("Surface").expect("Surface");
        let surface_normal = surface
            .call_method1("normal", (0.0, 0.0))
            .expect("Surface.normal");
        assert!(
            dot(py, &surface_normal, &expected) < -0.9999,
            "Surface.normal must be the placed, un-oriented normal"
        );

        // The view is live and the reported placement is the mutation.
        let placement = face.getattr("Placement").expect("Placement");
        let angle: f64 = placement
            .getattr("Rotation")
            .unwrap()
            .getattr("Angle")
            .unwrap()
            .extract()
            .unwrap();
        assert!((angle - 1.0).abs() < 1e-12, "the placement stored the angle");
    });
}

#[test]
fn shape_rotate_composes_with_the_existing_placement() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let math = PyModule::import(py, "math").expect("import math");
        let placement_class = ferrocad.getattr("Placement").expect("Placement");
        let vector = ferrocad.getattr("Vector").expect("Vector");

        let origin = vector.call1((0.0, 0.0, 0.0)).unwrap();
        let z_axis = vector.call1((0.0, 0.0, 1.0)).unwrap();

        let box_ = part.call_method1("makeBox", (1.0, 1.0, 1.0)).expect("makeBox");
        // `box.Placement.Base = Vector(10, 10, 10)` — a live sub-field write.
        box_.getattr("Placement")
            .unwrap()
            .setattr("Base", vector.call1((10.0, 10.0, 10.0)).unwrap())
            .expect("Placement.Base =");
        box_.call_method1("rotate", (&origin, &z_axis, 90.0))
            .expect("Shape.rotate");

        // p1 = translation(10, 10, 10); p2 = rotation 90 deg about Z.
        let p1 = placement_class.call0().expect("Placement()");
        p1.setattr("Base", vector.call1((10.0, 10.0, 10.0)).unwrap())
            .unwrap();
        let p2 = placement_class.call0().expect("Placement()");
        p2.getattr("Rotation")
            .unwrap()
            .setattr(
                "Angle",
                math.call_method1("radians", (90.0,)).expect("radians"),
            )
            .expect("p2.Rotation.Angle =");

        let p2_p1 = p2.call_method1("__mul__", (&p1,)).expect("p2 * p1");
        let box_placement = box_.getattr("Placement").expect("box.Placement");
        assert!(
            box_placement
                .call_method1("isSame", (&p2_p1, 1e-12))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "Shape.rotate must compose P_new = R * P_old"
        );

        // Placement.rotate default (`comp=False`) is a *right* multiply:
        // p1.rotate(...) == p1 * p2.
        let p3 = p1.call_method0("copy").expect("p1.copy()");
        p3.call_method1("rotate", (&origin, &z_axis, 90.0))
            .expect("p3.rotate");
        let p1_p2 = p1.call_method1("__mul__", (&p2,)).expect("p1 * p2");
        assert!(
            p3.call_method1("isSame", (&p1_p2, 1e-12))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "Placement.rotate(comp=False) is p * r"
        );
        assert!(
            !box_placement
                .call_method1("isSame", (&p3, 1e-12))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the rotation is not a left multiply"
        );

        // `comp=True` is a left multiply, matching Shape.rotate.
        let p4 = p1.call_method0("copy").expect("p1.copy()");
        p4.call_method1("rotate", (&origin, &z_axis, 90.0, true))
            .expect("p4.rotate(..., True)");
        assert!(
            p4.call_method1("isSame", (&p2_p1, 1e-12))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "Placement.rotate(comp=True) is r * p"
        );
        assert!(
            box_placement
                .call_method1("isSame", (&p4, 1e-12))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "Shape.rotate keeps the accumulated placement"
        );
    });
}

#[test]
fn document_shape_keeps_the_placement_beside_the_geometry() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("Vector");

        let box_ = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("makeBox");
        box_.getattr("Placement")
            .unwrap()
            .setattr("Base", vector.call1((3.0, 0.0, 0.0)).unwrap())
            .expect("Placement.Base =");

        // Assigning the shape to a document object stores the placement beside the
        // baked geometry (`ShapeProperty.placement`), not only the geometry.
        let doc = ferrocad
            .call_method1("newDocument", ("PlacementProperty",))
            .expect("newDocument");
        let obj = doc
            .call_method1("addObject", ("Part::Feature", "Feature"))
            .expect("addObject Part::Feature");
        obj.setattr("Shape", &box_).expect("obj.Shape =");

        let stored = obj.getattr("Shape").expect("obj.Shape");
        let base_x: f64 = stored
            .getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((base_x - 3.0).abs() < 1e-12, "ShapeProperty kept the placement");

        // Queries read the baked geometry, so the bound box already includes it.
        let bounds = stored.getattr("BoundBox").expect("BoundBox");
        let x_min: f64 = bounds.getattr("XMin").unwrap().extract().unwrap();
        assert!((x_min - 3.0).abs() < 1e-9);
    });
}

#[test]
fn translated_rotates_and_reassigns_stay_consistent() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let placement_class = ferrocad.getattr("Placement").expect("Placement");
        let vector = ferrocad.getattr("Vector").expect("Vector");

        let box_ = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("makeBox");
        box_.getattr("Placement")
            .unwrap()
            .setattr("Base", vector.call1((1.0, 0.0, 0.0)).unwrap())
            .expect("Placement.Base =");

        // `translated` is a copy: the original stays put, and the copy's
        // placement is `T * old` (upstream moves the location).
        let shifted = box_
            .call_method1("translated", (vector.call1((2.0, 0.0, 0.0)).unwrap(),))
            .expect("translated");
        let original_base: f64 = box_
            .getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((original_base - 1.0).abs() < 1e-12, "translated does not mutate");
        let shifted_base: f64 = shifted
            .getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((shifted_base - 3.0).abs() < 1e-12);
        let shifted_center = shifted.getattr("CenterOfMass").unwrap();
        assert!((shifted_center.getattr("x").unwrap().extract::<f64>().unwrap() - 4.0).abs() < 1e-9);

        // Assigning a whole placement replaces the accumulated one (upstream's
        // `setTransform`), rather than composing onto it.
        let target = placement_class.call0().expect("Placement()");
        target
            .setattr("Base", vector.call1((5.0, 5.0, 5.0)).unwrap())
            .unwrap();
        box_.setattr("Placement", target).expect("Placement =");
        let base: f64 = box_
            .getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((base - 5.0).abs() < 1e-12);
        let center = box_.getattr("CenterOfMass").unwrap();
        assert!((center.getattr("x").unwrap().extract::<f64>().unwrap() - 6.0).abs() < 1e-9);

        // `Base.x = …` goes through the same live path as `Base = …`.
        box_.getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .setattr("x", 7.0)
            .expect("Placement.Base.x =");
        let center = box_.getattr("CenterOfMass").unwrap();
        assert!((center.getattr("x").unwrap().extract::<f64>().unwrap() - 8.0).abs() < 1e-9);
    });
}

#[test]
fn placement_translate_and_matrix_match_the_property_forms() {
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let placement_class = ferrocad.getattr("Placement").expect("Placement");
        let vector_class = ferrocad.getattr("Vector").expect("Vector");

        let placement = placement_class.call0().expect("Placement()");
        let offset = vector_class.call1((1.0, 2.0, 3.0)).expect("Vector");
        placement
            .call_method1("translate", (&offset,))
            .expect("translate(vector)");
        // Three floats are the other accepted form.
        placement
            .call_method1("translate", (4.0, 5.0, 6.0))
            .expect("translate(x, y, z)");

        let base = placement.getattr("Base").unwrap();
        for (name, expected) in [("x", 5.0), ("y", 7.0), ("z", 9.0)] {
            let value: f64 = base.getattr(name).unwrap().extract().unwrap();
            assert!((value - expected).abs() < 1e-12, "Base.{name}");
        }

        // `Placement.Matrix` is the same matrix `toMatrix()` returns.
        let matrix = placement.getattr("Matrix").expect("Placement.Matrix");
        let to_matrix = placement.call_method0("toMatrix").expect("toMatrix");
        for name in ["A14", "A24", "A34"] {
            let from_attribute: f64 = matrix.getattr(name).unwrap().extract().unwrap();
            let from_method: f64 = to_matrix.getattr(name).unwrap().extract().unwrap();
            assert!((from_attribute - from_method).abs() < 1e-12, "{name}");
        }
        let tx: f64 = matrix.getattr("A14").unwrap().extract().unwrap();
        let ty: f64 = matrix.getattr("A24").unwrap().extract().unwrap();
        let tz: f64 = matrix.getattr("A34").unwrap().extract().unwrap();
        assert!((tx - 5.0).abs() < 1e-12 && (ty - 7.0).abs() < 1e-12 && (tz - 9.0).abs() < 1e-12);
    });
}

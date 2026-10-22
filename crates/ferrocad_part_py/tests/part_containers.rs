//! `Part.Compound`, `Part.makeCompound` and the shape-level booleans, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
}

#[test]
fn booleans_and_compounds_round_trip() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let box_a = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("box A");
        let box_b = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("box B");

        // Shape.fuse / common / multiFuse return shapes.
        let fused = box_a
            .call_method1("fuse", (&box_b,))
            .expect("fuse two boxes");
        assert!(!fused.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        let common = box_a
            .call_method1("common", (&box_b,))
            .expect("common of two boxes");
        assert!(!common.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        let multi = box_a
            .call_method1("multiFuse", (PyList::new(py, [&box_b]).unwrap(),))
            .expect("multiFuse");
        assert!(!multi.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        // Sub-shape shortcuts and the (absent) element map.
        let vertex = box_a.getattr("Vertex1").expect("Shape.Vertex1");
        assert!(!vertex.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert!(box_a.getattr("Face1").is_ok());
        assert!(box_a.getattr("Edge1").is_ok());
        assert_eq!(
            box_a
                .getattr("ElementMapVersion")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            ""
        );
        assert_eq!(
            box_a
                .getattr("ElementMapSize")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            0
        );
        assert!((box_a.getattr("Volume").unwrap().extract::<f64>().unwrap() - 8.0).abs() < 1e-9);
        assert!((box_a.getattr("Area").unwrap().extract::<f64>().unwrap() - 24.0).abs() < 1e-9);
        let center = box_a.getattr("CenterOfMass").expect("CenterOfMass");
        assert!((center.getattr("x").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9);
        let copied = box_a.call_method0("copy").expect("Shape.copy");
        assert!(copied.call_method1("isSame", (&box_a,)).unwrap().extract::<bool>().unwrap());
        let cleaned = box_a.call_method0("cleaned").expect("Shape.cleaned");
        assert!((cleaned.getattr("Volume").unwrap().extract::<f64>().unwrap() - 8.0).abs() < 1e-9);
        let shape_class = part.getattr("Shape").unwrap();
        let wrapped = shape_class.call1((&box_a,)).expect("Part.Shape(box)");
        assert!((wrapped.getattr("Volume").unwrap().extract::<f64>().unwrap() - 8.0).abs() < 1e-9);
        assert!(shape_class
            .call0()
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        let bounds = box_a.getattr("BoundBox").expect("Shape.BoundBox");
        assert!((bounds.getattr("XMax").unwrap().extract::<f64>().unwrap() - 2.0).abs() < 1e-9);
        let copied = ferrocad
            .getattr("BoundBox")
            .unwrap()
            .call1((&bounds,))
            .expect("BoundBox(other)");
        copied.call_method1("enlarge", (1.0,)).expect("enlarge");
        assert!(
            copied
                .call_method1("isInside", (&bounds,))
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );

        // Part.Compound(): null until a shape is added, then a real compound.
        let compound = part.getattr("Compound").expect("Part.Compound");
        let empty = compound.call0().expect("Part.Compound()");
        assert!(empty.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        empty
            .call_method1("add", (&vertex,))
            .expect("add a vertex to an empty compound");
        assert!(!empty.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        // Part.Compound([a, b]) and Part.makeCompound([a, b]) both keep two children.
        let children = PyList::new(py, [&box_a, &box_b]).unwrap();
        let built = compound.call1((&children,)).expect("Part.Compound([a, b])");
        assert_eq!(built.len().unwrap(), 2);
        assert!(!built.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        let made = part
            .call_method1("makeCompound", (&children,))
            .expect("Part.makeCompound([a, b])");
        assert_eq!(made.len().unwrap(), 2);
    });
}

#[test]
fn primitive_volumes_match_the_analytic_formulas() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let volume = |value: Bound<'_, pyo3::PyAny>| value.getattr("Volume").unwrap().extract::<f64>().unwrap();

        let cylinder = part
            .call_method1("makeCylinder", (2.0, 5.0))
            .expect("makeCylinder");
        assert!((volume(cylinder) - std::f64::consts::PI * 4.0 * 5.0).abs() < 1e-6);

        let sphere = part.call_method1("makeSphere", (5.0,)).expect("makeSphere");
        assert!(
            (volume(sphere) - 4.0 / 3.0 * std::f64::consts::PI * 125.0).abs() < 1e-6
        );

        let cone = part
            .call_method1("makeCone", (2.0, 1.0, 3.0))
            .expect("makeCone");
        assert!((volume(cone) - std::f64::consts::PI * 3.0 / 3.0 * (4.0 + 2.0 + 1.0)).abs() < 1e-6);
    });
}

#[test]
fn a_compound_of_wires_is_accepted_by_make_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point = |x: f64, y: f64, z: f64| {
            vector
                .call1((x, y, z))
                .expect("a ferrocad.Vector")
                .unbind()
        };
        let wire_of = |points: [(f64, f64, f64); 4]| {
            let points = PyList::new(py, points.map(|(x, y, z)| point(x, y, z))).unwrap();
            part.call_method1("makePolygon", (points, true))
                .expect("makePolygon")
                .call_method1("toShape", ())
                .expect("toShape")
        };

        let w1 = wire_of([
            (0.0, 0.0, 0.0),
            (10.0, 0.0, 0.0),
            (10.0, 10.0, 0.0),
            (0.0, 10.0, 0.0),
        ]);
        let w2 = wire_of([
            (20.0, 0.0, 0.0),
            (30.0, 0.0, 0.0),
            (30.0, 0.0, 10.0),
            (20.0, 0.0, 10.0),
        ]);
        let wires = PyList::new(py, [&w1, &w2]).unwrap();
        let compound = part
            .getattr("Compound")
            .unwrap()
            .call1((&wires,))
            .expect("Part.Compound([w1, w2])");
        let face = part
            .call_method1("makeFace", (&compound, "Part::FaceMakerUnified"))
            .expect("makeFace(Part.Compound([w1, w2]))");
        assert!(!face.call_method0("isNull").unwrap().extract::<bool>().unwrap());
    });
}

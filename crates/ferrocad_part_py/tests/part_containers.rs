//! `Part.Compound`, `Part.makeCompound` and the shape-level booleans, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

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
        // `copy()` is a deep copy (`BRepBuilderAPI_Copy`): a new TShape, so it is
        // not `isSame`, but the geometry is identical.
        let copied = box_a.call_method0("copy").expect("Shape.copy");
        assert!(!copied.call_method1("isSame", (&box_a,)).unwrap().extract::<bool>().unwrap());
        assert!((copied.getattr("Volume").unwrap().extract::<f64>().unwrap() - 8.0).abs() < 1e-9);
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
fn unified_faces_split_overlapping_circles() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let circle = |x: f64, y: f64| {
            let center = vector.call1((x, y, 0.0)).expect("center");
            let edge = part
                .call_method1("makeCircle", (10.0, center))
                .expect("makeCircle");
            part.getattr("Wire")
                .unwrap()
                .call1((&edge,))
                .expect("Part.Wire(circle)")
        };
        let c1 = circle(0.0, 0.0);
        let c2 = circle(8.0, 0.0);
        let compound = part
            .getattr("Compound")
            .unwrap()
            .call1((PyList::new(py, [&c1, &c2]).unwrap(),))
            .expect("Part.Compound([c1, c2])");
        let face = part
            .call_method1("makeFace", (&compound, "Part::FaceMakerUnified"))
            .expect("makeFace(overlapping circles, unified)");
        assert_eq!(face.getattr("Faces").unwrap().len().unwrap(), 3);
    });
}

#[test]
fn faces_expose_their_surface_and_outer_wire() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder = part.call_method1("makeCylinder", (10.0, 20.0)).expect("cylinder");
        let faces = cylinder.getattr("Faces").expect("Faces");
        let count = faces.len().unwrap();
        let mut lateral = None;
        for index in 0..count {
            let face = faces.get_item(index).unwrap();
            let surface = face.getattr("Surface").expect("Face.Surface");
            if surface.getattr("TypeId").unwrap().extract::<String>().unwrap()
                == "Part::GeomCylinder"
            {
                lateral = Some(face);
                break;
            }
        }
        let lateral = lateral.expect("a cylindrical face");
        let outer = lateral.getattr("OuterWire").expect("Face.OuterWire");
        assert!(outer.getattr("Edges").unwrap().len().unwrap() > 0);
        assert!(outer.is_instance(&part.getattr("Wire").unwrap()).unwrap());
    });
}

#[test]
fn comp_solids_and_shape_placement() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("box");
        let class = part.getattr("CompSolid").expect("Part.CompSolid");
        let empty = class.call0().expect("CompSolid()");
        assert!(empty.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        empty.call_method1("add", (&cube,)).expect("CompSolid.add");
        assert!(!empty.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        let built = class
            .call1((PyList::new(py, [&cube]).unwrap(),))
            .expect("CompSolid([box])");
        assert_eq!(built.len().unwrap(), 1);

        let placement = cube.getattr("Placement").expect("Shape.Placement");
        let base = placement.getattr("Base").expect("Placement.Base");
        assert!(base.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-12);
        assert_eq!(cube.call_method0("childShapes").unwrap().len().unwrap(), 1);
    });
}

#[test]
fn matrix_transforms_move_shapes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let location = vector.call1((10.0, 0.0, 0.0)).expect("Vector(10,0,0)");
        let rotation = ferrocad.getattr("Rotation").unwrap().call0().expect("Rotation()");
        let placement = ferrocad
            .getattr("Placement")
            .unwrap()
            .call1((location, rotation))
            .expect("Placement");
        let matrix = placement.call_method0("toMatrix").expect("toMatrix");

        let shape = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("box");
        let moved = shape
            .call_method1("transformed", (&matrix,))
            .expect("Shape.transformed");
        let center = moved.getattr("CenterOfMass").expect("CenterOfMass");
        assert!((center.getattr("x").unwrap().extract::<f64>().unwrap() - 11.0).abs() < 1e-9);

        let returned = shape
            .call_method1("transformShape", (&matrix,))
            .expect("Shape.transformShape");
        assert!(returned.is(&shape), "transformShape returns the same object");
        let center = shape.getattr("CenterOfMass").expect("CenterOfMass");
        assert!((center.getattr("x").unwrap().extract::<f64>().unwrap() - 11.0).abs() < 1e-9);

        // translated / scaled / mirror and the Placement setter.
        let cube = part.call_method1("makeBox", (2.0, 2.0, 2.0)).expect("cube");
        let shifted = cube
            .call_method1("translated", (vector.call1((1.0, 2.0, 3.0)).unwrap(),))
            .expect("translated");
        let center = shifted.getattr("CenterOfMass").unwrap();
        assert!((center.getattr("y").unwrap().extract::<f64>().unwrap() - 3.0).abs() < 1e-9);
        let scaled = cube.call_method1("scaled", (2.0,)).expect("scaled");
        assert!((scaled.getattr("Volume").unwrap().extract::<f64>().unwrap() - 64.0).abs() < 1e-9);
        let mirrored = cube
            .call_method1("mirror", (vector.call1((0.0, 0.0, 0.0)).unwrap(), vector.call1((0.0, 1.0, 0.0)).unwrap()))
            .expect("mirror");
        let center = mirrored.getattr("CenterOfMass").unwrap();
        assert!((center.getattr("y").unwrap().extract::<f64>().unwrap() + 1.0).abs() < 1e-9);

        let placed = part
            .call_method1("makeBox", (1.0, 1.0, 1.0, vector.call1((10.0, 0.0, 0.0)).unwrap()))
            .expect("makeBox(..., pnt)");
        let bounds = placed.getattr("BoundBox").unwrap();
        assert!((bounds.getattr("XMin").unwrap().extract::<f64>().unwrap() - 10.0).abs() < 1e-9);

        let other = part.call_method1("makeBox", (1.0, 1.0, 1.0)).unwrap();
        let target = ferrocad
            .getattr("Placement")
            .unwrap()
            .call1((vector.call1((5.0, 0.0, 0.0)).unwrap(), ferrocad.getattr("Rotation").unwrap().call0().unwrap()))
            .unwrap();
        other.setattr("Placement", target).expect("Shape.Placement =") ;
        let bounds = other.getattr("BoundBox").unwrap();
        assert!((bounds.getattr("XMin").unwrap().extract::<f64>().unwrap() - 5.0).abs() < 1e-9);
    });
}

#[test]
fn unified_faces_fill_non_planar_wires() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point = |x: f64, y: f64, z: f64| {
            vector.call1((x, y, z)).expect("vector").unbind()
        };
        let make_wire = |points: Vec<(f64, f64, f64)>| {
            let list = PyList::new(py, points.into_iter().map(|(x, y, z)| point(x, y, z))).unwrap();
            part.call_method1("makePolygon", (list, true)).expect("makePolygon")
        };

        // One twisted quad: a non-planar boundary needs BRepFill_Filling.
        let twisted = make_wire(vec![
            (0.0, 0.0, 0.0),
            (10.0, 0.0, 0.0),
            (10.0, 10.0, 5.0),
            (0.0, 10.0, 0.0),
        ]);
        let compound = part
            .getattr("Compound")
            .unwrap()
            .call1((PyList::new(py, [&twisted]).unwrap(),))
            .expect("Part.Compound([twisted])");
        let filled = part
            .call_method1("makeFace", (&compound, "Part::FaceMakerUnified"))
            .expect("makeFace(twisted, unified)");
        assert!(filled.getattr("Area").unwrap().extract::<f64>().unwrap() > 100.0);

        // Two independent freeform boundaries: one face each.
        let second = make_wire(vec![
            (20.0, 0.0, 0.0),
            (30.0, 0.0, 3.0),
            (30.0, 10.0, 0.0),
            (20.0, 10.0, 4.0),
        ]);
        let compound = part
            .getattr("Compound")
            .unwrap()
            .call1((PyList::new(py, [&twisted, &second]).unwrap(),))
            .expect("Part.Compound([two freeform wires])");
        let filled = part
            .call_method1("makeFace", (&compound, "Part::FaceMakerUnified"))
            .expect("makeFace(two freeform wires, unified)");
        assert_eq!(filled.getattr("Faces").unwrap().len().unwrap(), 2);
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

/// A compound caches its children, but the cache must follow the B-Rep when the
/// inherited `Shape` API moves it: rebuilding from a rotated compound with stale
/// children silently drops the rotation (Draft's hatch does exactly this).
#[test]
fn a_rotated_compound_rebuilds_from_its_current_children() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("Vector");
        let origin = vector.call1((0.0, 0.0, 0.0)).unwrap();
        let z_axis = vector.call1((0.0, 0.0, 1.0)).unwrap();

        let edge = part
            .call_method1("makeLine", ((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)))
            .expect("makeLine");
        let children = PyList::new(py, [&edge]).unwrap();
        let compound = part
            .call_method1("makeCompound", (&children,))
            .expect("makeCompound");

        compound
            .call_method1("rotate", (&origin, &z_axis, 90.0))
            .expect("compound.rotate");

        let rebuilt = part
            .call_method1("makeCompound", (PyList::new(py, [&compound]).unwrap(),))
            .expect("makeCompound([rotated])");
        let bounds = rebuilt.getattr("BoundBox").expect("BoundBox");
        let x_max: f64 = bounds.getattr("XMax").unwrap().extract().unwrap();
        let y_max: f64 = bounds.getattr("YMax").unwrap().extract().unwrap();
        assert!(x_max.abs() < 1e-9, "the edge must no longer run along +x");
        assert!((y_max - 1.0).abs() < 1e-9, "the edge must run along +y");
    });
}

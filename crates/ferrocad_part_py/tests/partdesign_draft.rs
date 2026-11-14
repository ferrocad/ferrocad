//! `PartDesign::Draft` and `PartDesign::Plane`, the types SimpleCAD's draft tool
//! builds on a private document.
//!
//! FerroCAD does not implement the PartDesign workbench; these two types exist
//! because `simplecad/surfaces/offset.py` opens a private document, puts the shape
//! in a `Part::Feature`, adds a `PartDesign::Body`, a `PartDesign::Plane` hinge and
//! a `PartDesign::Draft`, and reads the feature's `Shape`. This test drives the same
//! sequence and pins the supported straight-prism class and its refusals.
//!
//! Same one-image boot as `part_link`: `ferrocad` + `Part` registered as built-ins,
//! because CPython initialisation is process-wide and concurrent `with_gil`
//! bootstraps abort the process.

use std::sync::{Mutex, Once};

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

fn new_document<'py>(ferrocad: &Bound<'py, PyModule>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    ferrocad.call_method1("newDocument", (name,))
}

fn close_document(ferrocad: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    ferrocad.call_method1("closeDocument", (name,))?;
    Ok(())
}

fn vector<'py>(ferrocad: &Bound<'py, PyModule>, x: f64, y: f64, z: f64) -> PyResult<Bound<'py, PyAny>> {
    ferrocad.getattr("Vector")?.call1((x, y, z))
}

fn placement<'py>(ferrocad: &Bound<'py, PyModule>) -> PyResult<Bound<'py, PyAny>> {
    ferrocad
        .getattr("Placement")?
        .call1((vector(ferrocad, 0.0, 0.0, 0.0)?, ferrocad.getattr("Rotation")?.call0()?))
}

/// The 1-based names of the box faces whose normal lies in the XY plane.
fn side_faces<'py>(shape: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let mut names = Vec::new();
    for (index, face) in shape.getattr("Faces")?.try_iter()?.enumerate() {
        let face = face?;
        let axis = face.getattr("Surface")?.getattr("Axis")?;
        let z: f64 = axis.getattr("z")?.extract()?;
        if z.abs() < 1e-6 {
            names.push(format!("Face{}", index + 1));
        }
    }
    PyList::new(shape.py(), names)
}

fn volume(shape: &Bound<'_, PyAny>) -> f64 {
    shape.getattr("Volume").expect("Volume").extract().expect("volume")
}

fn count(shape: &Bound<'_, PyAny>, name: &str) -> usize {
    shape
        .getattr(name)
        .unwrap_or_else(|_| panic!("Shape.{name}"))
        .len()
        .unwrap_or_else(|_| panic!("len(Shape.{name})"))
}

fn area(face: &Bound<'_, PyAny>) -> f64 {
    face.getattr("Area").expect("Area").extract().expect("area")
}

/// The face whose centre of mass is at the extreme of Z.
fn face_at_z<'py>(shape: &Bound<'py, PyAny>, top: bool) -> PyResult<Bound<'py, PyAny>> {
    let bound_box = shape.getattr("BoundBox")?;
    let extreme: f64 = if top {
        bound_box.getattr("ZMax")?.extract()?
    } else {
        bound_box.getattr("ZMin")?.extract()?
    };
    for face in shape.getattr("Faces")?.try_iter()? {
        let face = face?;
        let z: f64 = face.getattr("CenterOfMass")?.getattr("z")?.extract()?;
        if (z - extreme).abs() < 1e-6 {
            return Ok(face);
        }
    }
    panic!("no face at z = {extreme}");
}

/// The drafted shape for `base` with `angle` and the given hinge/reversal choices.
fn draft_box<'py>(
    ferrocad: &Bound<'py, PyModule>,
    doc: &Bound<'py, PyAny>,
    name: &str,
    faces: &Bound<'py, PyList>,
    angle: f64,
    reversed: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let base = doc.call_method1("addObject", ("Part::Feature", format!("{name}Base")))?;
    base.setattr(
        "Shape",
        PyModule::import(ferrocad.py(), "Part")?.call_method1("makeBox", (20.0, 20.0, 10.0))?,
    )?;
    let body = doc.call_method1("addObject", ("PartDesign::Body", format!("{name}Body")))?;
    body.setattr("BaseFeature", &base)?;
    let plane = doc.call_method1("addObject", ("PartDesign::Plane", format!("{name}Hinge")))?;
    body.call_method1("addObject", (&plane,))?;
    plane.setattr("MapMode", "Deactivated")?;
    plane.setattr("Placement", placement(ferrocad)?)?;
    let feature = doc.call_method1("addObject", ("PartDesign::Draft", name))?;
    body.call_method1("addObject", (&feature,))?;
    feature.setattr("Base", (&base, faces))?;
    feature.setattr("Angle", angle)?;
    feature.setattr(
        "NeutralPlane",
        (&plane, pyo3::types::PyTuple::new(ferrocad.py(), [""])?),
    )?;
    feature.setattr("Reversed", reversed)?;
    doc.call_method0("recompute")?;
    Ok(feature)
}

#[test]
fn a_draft_narrows_a_box_toward_the_far_cap() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustDraftNarrow").expect("newDocument");

        let box_shape = part.call_method1("makeBox", (20.0, 20.0, 10.0)).expect("box");
        let faces = side_faces(&box_shape).expect("the four side faces");
        assert_eq!(faces.len(), 4);

        let feature = draft_box(&ferrocad, &doc, "Draft", &faces, 5.0, false)
            .expect("draft the box");
        let drafted = feature.getattr("Shape").expect("Draft.Shape");
        assert!(!drafted
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert_eq!(count(&drafted, "Solids"), 1);

        // The hinge cap keeps its area; the far cap is 10 * tan(5) smaller per side.
        let bottom = face_at_z(&drafted, false).expect("the bottom face");
        let roof = face_at_z(&drafted, true).expect("the roof");
        assert!((area(&bottom) - 400.0).abs() < 1e-6, "bottom {}", area(&bottom));
        let side = 20.0 - 2.0 * 10.0 * 5.0_f64.to_radians().tan();
        assert!(
            (area(&roof) - side * side).abs() < 1e-6,
            "roof {} expected {}",
            area(&roof),
            side * side
        );
        assert!(
            volume(&drafted) < 4000.0,
            "a narrowing draft loses volume: {}",
            volume(&drafted)
        );

        close_document(&ferrocad, "RustDraftNarrow").expect("closeDocument");
    });
}

#[test]
fn a_reversed_draft_widens_the_far_cap() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustDraftWiden").expect("newDocument");

        let box_shape = part.call_method1("makeBox", (20.0, 20.0, 10.0)).expect("box");
        let faces = side_faces(&box_shape).expect("the four side faces");

        // Reversed narrows toward the hinge's -Z, so the far (+Z) cap widens.
        let feature = draft_box(&ferrocad, &doc, "Draft", &faces, 5.0, true)
            .expect("draft the box");
        let drafted = feature.getattr("Shape").expect("Draft.Shape");
        let bottom = face_at_z(&drafted, false).expect("the bottom face");
        let roof = face_at_z(&drafted, true).expect("the roof");
        assert!((area(&bottom) - 400.0).abs() < 1e-6, "bottom {}", area(&bottom));
        assert!(area(&roof) > 400.0, "the far cap widens: {}", area(&roof));
        assert!(
            volume(&drafted) > 4000.0,
            "a widening draft gains volume: {}",
            volume(&drafted)
        );

        close_document(&ferrocad, "RustDraftWiden").expect("closeDocument");
    });
}

#[test]
fn a_drafted_cylinder_becomes_a_cone() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustDraftCone").expect("newDocument");

        let cylinder = part
            .call_method1("makeCylinder", (5.0, 10.0))
            .expect("makeCylinder");
        let mut wall = Vec::new();
        for (index, face) in cylinder.getattr("Faces").unwrap().try_iter().unwrap().enumerate() {
            let face = face.unwrap();
            let surface = face.getattr("Surface").unwrap();
            if surface.get_type().name().unwrap() == "Cylinder" {
                wall.push(format!("Face{}", index + 1));
            }
        }
        let faces = PyList::new(py, wall).expect("the wall name");

        let base = doc
            .call_method1("addObject", ("Part::Feature", "Base"))
            .expect("Base");
        base.setattr("Shape", &cylinder).expect("Base.Shape");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("Body");
        body.setattr("BaseFeature", &base).expect("BaseFeature");
        let plane = doc
            .call_method1("addObject", ("PartDesign::Plane", "Hinge"))
            .expect("Hinge");
        body.call_method1("addObject", (&plane,)).expect("add hinge");
        plane
            .setattr("Placement", placement(&ferrocad).unwrap())
            .expect("hinge placement");
        let feature = doc
            .call_method1("addObject", ("PartDesign::Draft", "Draft"))
            .expect("Draft");
        body.call_method1("addObject", (&feature,)).expect("add draft");
        feature.setattr("Base", (&base, faces)).expect("Draft.Base");
        feature.setattr("Angle", 5.0).expect("Draft.Angle");
        feature
            .setattr(
                "NeutralPlane",
                (&plane, pyo3::types::PyTuple::new(py, [""]).unwrap()),
            )
            .expect("NeutralPlane");
        doc.call_method0("recompute").expect("recompute");

        let drafted = feature.getattr("Shape").expect("Draft.Shape");
        assert!(!drafted
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        let cone = drafted
            .getattr("Faces")
            .unwrap()
            .try_iter()
            .unwrap()
            .any(|face| {
                face.unwrap()
                    .getattr("Surface")
                    .unwrap()
                    .get_type()
                    .name()
                    .unwrap()
                    == "Cone"
            });
        assert!(cone, "a drafted cylinder wall is a cone");
        let far_radius = 5.0 - 10.0 * (5.0_f64).to_radians().tan();
        let expected = std::f64::consts::PI * 10.0 / 3.0
            * (25.0 + 5.0 * far_radius + far_radius * far_radius);
        assert!(
            (volume(&drafted) - expected).abs() < 1e-6,
            "frustum volume {}, expected {expected}",
            volume(&drafted)
        );

        close_document(&ferrocad, "RustDraftCone").expect("closeDocument");
    });
}

#[test]
fn a_cap_selected_for_drafting_is_refused() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustDraftRefuse").expect("newDocument");

        // `makeBox` numbers the top cap Face6: square to the pull, so it cannot be
        // drafted (the other tests pin the face order by picking the four walls).
        let faces = PyList::new(py, ["Face6"]).expect("the cap name");
        let feature = draft_box(&ferrocad, &doc, "Draft", &faces, 5.0, false)
            .expect("create the feature");
        assert!(feature
            .getattr("Shape")
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert!(
            feature
                .call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap()
                .contains("square to the pull"),
            "the refusal is reported: {:?}",
            feature.call_method0("getStatusString").unwrap()
        );

        close_document(&ferrocad, "RustDraftRefuse").expect("closeDocument");
    });
}

#[test]
fn the_datum_plane_carries_its_placement_and_map_mode() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustDraftDatum").expect("newDocument");

        let plane = doc
            .call_method1("addObject", ("PartDesign::Plane", "Hinge"))
            .expect("PartDesign::Plane");
        assert_eq!(
            plane.getattr("MapMode").unwrap().extract::<String>().unwrap(),
            "Deactivated"
        );
        plane.setattr("MapMode", "ObjectXY").expect("MapMode is settable");
        // The marker is one planar face at the placement.
        doc.call_method0("recompute").expect("recompute");
        let shape = plane.getattr("Shape").expect("Plane.Shape");
        assert_eq!(shape.getattr("ShapeType").unwrap().extract::<String>().unwrap(), "Face");
        let area: f64 = shape.getattr("Area").unwrap().extract().unwrap();
        assert!((area - 100.0).abs() < 1e-6, "10 x 10 marker: {area}");

        close_document(&ferrocad, "RustDraftDatum").expect("closeDocument");
    });
}

#[test]
fn the_partdesign_module_imports_in_a_linked_host() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .parent()
            .expect("repository root");
        let sys = py.import("sys").expect("sys");
        sys.getattr("path")
            .unwrap()
            .call_method1("insert", (0, repo.join("python").to_str().unwrap()))
            .expect("sys.path.insert");
        // In this image `ferrocad` is a built-in, so the module must not pull the
        // combined extension in; it only guarantees the types are registered.
        let module = PyModule::import(py, "PartDesign").expect("import PartDesign");
        assert!(module.getattr("__doc__").is_ok());
    });
}

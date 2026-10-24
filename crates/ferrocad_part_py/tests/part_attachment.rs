//! `Part::Part2DObjectPython` attachment (P2d).
//!
//! A 2D Python object carries `AttachmentSupport`/`MapMode`/`MapPathParameter`/
//! `AttachmentOffset` from its type defaults and derives its `Placement` from the
//! support on every recompute. These tests mirror the modes Draft sets: a plane
//! attached to a flat face, the XY frame of a whole support object, and the
//! deactivated case that leaves `Placement` alone. A support that cannot satisfy the
//! mode leaves the object invalid with a reason instead of silently keeping a stale
//! placement.
//!
//! Needs OCCT and the one image that links `ferrocad` + `Part`.

use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn boot() {
    use std::sync::Once;

    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// Import both modules and open a document.
fn new_document<'py>(py: Python<'py>, name: &str) -> Bound<'py, PyAny> {
    PyModule::import(py, "Part").expect("import Part");
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .call_method1("newDocument", (name,))
        .expect("newDocument")
}

fn close_document(py: Python<'_>, name: &str) {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .call_method1("closeDocument", (name,))
        .expect("closeDocument");
}

fn vector<'py>(py: Python<'py>, x: f64, y: f64, z: f64) -> Bound<'py, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("Vector")
        .call1((x, y, z))
        .expect("Vector(...)")
}

fn placement<'py>(
    py: Python<'py>,
    base: (f64, f64, f64),
    rotation: Option<(&Bound<'py, PyAny>, f64)>,
) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
    let placement = ferrocad.getattr("Placement").expect("Placement");
    let value = placement.call0().expect("Placement()");
    value
        .setattr("Base", vector(py, base.0, base.1, base.2))
        .expect("Placement.Base =");
    if let Some((axis, angle)) = rotation {
        let rotation = value.getattr("Rotation").expect("Rotation");
        rotation.setattr("Axis", axis).expect("Rotation.Axis =");
        rotation
            .setattr("Angle", angle.to_radians())
            .expect("Rotation.Angle =");
    }
    value
}

fn base_of(obj: &Bound<'_, PyAny>) -> (f64, f64, f64) {
    let placement = obj.getattr("Placement").expect("Placement");
    let base = placement.getattr("Base").expect("Base");
    (
        base.getattr("x").unwrap().extract().unwrap(),
        base.getattr("y").unwrap().extract().unwrap(),
        base.getattr("z").unwrap().extract().unwrap(),
    )
}

fn assert_base(obj: &Bound<'_, PyAny>, expected: (f64, f64, f64)) {
    let actual = base_of(obj);
    let close = (actual.0 - expected.0).abs() < 1e-9
        && (actual.1 - expected.1).abs() < 1e-9
        && (actual.2 - expected.2).abs() < 1e-9;
    assert!(
        close,
        "Placement.Base is {actual:?}, expected {expected:?}"
    );
}

fn state(obj: &Bound<'_, PyAny>) -> Vec<String> {
    obj.getattr("State")
        .expect("State")
        .extract::<Vec<String>>()
        .expect("State is a string list")
}

/// Add a `Part::Feature` whose `Shape` is `shape`.
fn feature_with_shape<'py>(
    doc: &Bound<'py, PyAny>,
    name: &str,
    shape: &Bound<'py, PyAny>,
) -> Bound<'py, PyAny> {
    let object = doc
        .call_method1("addObject", ("Part::Feature", name))
        .expect("addObject Part::Feature");
    object.setattr("Shape", shape).expect("obj.Shape = shape");
    object
}

/// A Python proxy that gives the object a plane on every recompute, so the object
/// is valid and the attachment is independent of the shape's own geometry.
const PROXY_SOURCE: &str = r#"
class PlaneProxy:
    def execute(self, obj):
        obj.Shape = Part.makePlane(1.0, 1.0)
"#;

/// Install a proxy module and return a fresh proxy instance.
fn proxy<'py>(py: Python<'py>, module_name: &str) -> Bound<'py, PyAny> {
    let module = PyModule::new(py, module_name).expect("proxy module");
    module
        .setattr("Part", py.import("Part").expect("import Part"))
        .expect("module.Part");
    let code = std::ffi::CString::new(PROXY_SOURCE).unwrap();
    py.run(&code, Some(&module.dict()), None)
        .expect("define PlaneProxy");
    module
        .getattr("PlaneProxy")
        .expect("PlaneProxy")
        .call0()
        .expect("PlaneProxy()")
}

/// Assign `AttachmentSupport` with whichever spelling the property accepts:
/// `App::PropertyLinkSubList` wants `(object, subnames)` pairs, while the current
/// core table flattens it to a link list of whole objects.
fn set_support(obj: &Bound<'_, PyAny>, support: &Bound<'_, PyAny>) {
    use pyo3::exceptions::PyTypeError;
    use pyo3::types::PyList;

    let py = obj.py();
    let empty_subs = PyTuple::new(py, ["".to_string()]).expect("subnames");
    let pair = PyTuple::new(py, [support.clone(), empty_subs.into_any()])
        .expect("(object, subnames)");
    let attempts: Vec<Bound<'_, PyAny>> = vec![
        PyList::new(py, [pair.clone()])
            .expect("list of (object, subnames)")
            .into_any(),
        PyList::new(py, [support.clone()])
            .expect("list of objects")
            .into_any(),
        pair.into_any(),
    ];
    let mut last_error = None;
    for attempt in attempts {
        match obj.setattr("AttachmentSupport", &attempt) {
            Ok(()) => return,
            Err(error) if error.is_instance_of::<PyTypeError>(py) => last_error = Some(error),
            Err(error) => panic!("AttachmentSupport assignment failed: {error}"),
        }
    }
    panic!(
        "AttachmentSupport rejects every upstream spelling: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_default()
    );
}

#[test]
fn add_extension_exposes_the_attacher_methods() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "PartAttachExtension");
        let part = PyModule::import(py, "Part").expect("import Part");
        let obj = doc
            .call_method1(
                "addObject",
                ("Part::FeaturePython", "Scripted", &proxy(py, "attach_ext_proxy"), py.None()),
            )
            .expect("addObject Part::FeaturePython");

        // `changeAttacherType` is a binding method, but with no extension of the
        // object providing it upstream's dispatch raises `AttributeError`.
        let missing = obj
            .call_method1("changeAttacherType", ("Attacher::AttachEnginePlane",))
            .unwrap_err();
        assert!(
            missing.is_instance_of::<pyo3::exceptions::PyAttributeError>(py),
            "without the extension the method is not provided: {missing}"
        );

        obj.call_method1("addExtension", ("Part::AttachExtensionPython",))
            .expect("addExtension Part::AttachExtensionPython");
        for name in ["Part::AttachExtensionPython", "Part::AttachExtension"] {
            assert!(
                obj.call_method1("hasExtension", (name,))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                "addExtension must report {name}"
            );
        }
        let properties: Vec<String> = obj
            .getattr("PropertiesList")
            .unwrap()
            .extract()
            .unwrap();
        for name in [
            "AttachmentSupport",
            "MapMode",
            "MapPathParameter",
            "AttachmentOffset",
            "AttacherType",
        ] {
            assert!(properties.iter().any(|property| property == name), "missing {name}");
        }

        // wpproxy's exact call: a new engine resets the mode to Deactivated.
        let changed = obj
            .call_method1("changeAttacherType", ("Attacher::AttachEnginePlane",))
            .expect("changeAttacherType")
            .extract::<bool>()
            .expect("changeAttacherType returns a bool");
        assert!(changed);
        assert_eq!(
            obj.getattr("AttacherType").unwrap().extract::<String>().unwrap(),
            "Attacher::AttachEnginePlane"
        );
        assert_eq!(
            obj.getattr("MapMode").unwrap().extract::<String>().unwrap(),
            "Deactivated"
        );
        // Changing to the same engine reports false, exactly as upstream.
        assert!(!obj
            .call_method1("changeAttacherType", ("Attacher::AttachEnginePlane",))
            .unwrap()
            .extract::<bool>()
            .unwrap());

        // An unknown engine is a ValueError.
        let unknown = obj
            .call_method1("changeAttacherType", ("Attacher::Nope",))
            .unwrap_err();
        assert!(unknown.is_instance_of::<pyo3::exceptions::PyValueError>(py));

        // `positionBySupport` arrives through the registry, not the binding.
        let face = part
            .call_method1("makePlane", (4.0, 4.0, vector(py, 0.0, 0.0, 5.0)))
            .expect("makePlane");
        let support = feature_with_shape(&doc, "Support", &face);
        set_support(&obj, &support);
        obj.setattr("MapMode", "FlatFace").expect("MapMode");
        let attached = obj
            .call_method0("positionBySupport")
            .expect("positionBySupport")
            .extract::<bool>()
            .expect("positionBySupport returns a bool");
        assert!(attached);
        assert_base(&obj, (0.0, 0.0, 5.0));

        close_document(py, "PartAttachExtension");
    });
}

#[test]
fn a_part2d_object_has_the_attachment_surface() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "Part2DAttachmentProps");
        let obj = doc
            .call_method1("addObject", ("Part::Part2DObjectPython", "Sketchy"))
            .expect("addObject Part::Part2DObjectPython");

        let properties: Vec<String> = obj
            .getattr("PropertiesList")
            .expect("PropertiesList")
            .extract()
            .expect("PropertiesList is a string list");
        for name in [
            "AttachmentSupport",
            "MapMode",
            "MapPathParameter",
            "AttachmentOffset",
            "AttacherType",
        ] {
            assert!(
                properties.iter().any(|property| property == name),
                "a Part2D Python object has {name}; got {properties:?}"
            );
        }

        // The static attacher is the plane engine, as upstream's Part2DObject.
        assert_eq!(
            obj.getattr("AttacherType")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Attacher::AttachEnginePlane"
        );
        assert_eq!(
            obj.getattr("MapMode").unwrap().extract::<String>().unwrap(),
            "Deactivated"
        );
        // The plane engine offers the 3D modes Draft uses.
        for mode in ["Deactivated", "ObjectXY", "ObjectXZ", "ObjectYZ", "FlatFace"] {
            obj.setattr("MapMode", mode)
                .unwrap_or_else(|error| panic!("MapMode = {mode}: {error}"));
        }
        // A mode the engine does not offer is rejected.
        let rejected = obj.setattr("MapMode", "Vertex").unwrap_err();
        assert!(rejected.is_instance_of::<pyo3::exceptions::PyValueError>(py));

        close_document(py, "Part2DAttachmentProps");
    });
}

#[test]
fn flat_face_attaches_to_a_plane_and_applies_the_offset() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "Part2DFlatFace");
        let part = PyModule::import(py, "Part").expect("import Part");

        // A face on z = 5 with a +Z normal, held by a plain Part::Feature.
        let face = part
            .call_method1("makePlane", (4.0, 4.0, vector(py, 0.0, 0.0, 5.0)))
            .expect("makePlane");
        let _support = feature_with_shape(&doc, "Support", &face);

        let obj = doc
            .call_method1("addObject", ("Part::Part2DObjectPython", "Attached", &proxy(py, "attach_flat_proxy"), py.None()))
            .expect("addObject Part::Part2DObjectPython");
        set_support(&obj, &_support);
        obj.setattr("MapMode", "FlatFace").expect("MapMode");
        obj.setattr(
            "AttachmentOffset",
            placement(py, (1.0, 2.0, 3.0), None),
        )
        .expect("AttachmentOffset");

        doc.call_method0("recompute").expect("recompute");

        // The support object's origin projects onto the face's plane at (0, 0, 5);
        // the offset adds (1, 2, 3) in local coordinates, whose axes are the
        // face's frame (+X, +Y, +Z for this plane).
        assert_base(&obj, (1.0, 2.0, 8.0));
        let rotation = obj
            .getattr("Placement")
            .unwrap()
            .getattr("Rotation")
            .unwrap();
        assert!(
            rotation
                .getattr("Angle")
                .unwrap()
                .extract::<f64>()
                .unwrap()
                .abs()
                < 1e-9,
            "the face's frame is the identity for a +Z plane"
        );
        assert!(
            !state(&obj).iter().any(|state| state == "Invalid"),
            "a satisfiable attachment leaves the object valid: {:?}",
            state(&obj)
        );

        // A second recompute keeps the same placement (the attachment is derived,
        // not accumulated).
        doc.call_method0("recompute").expect("recompute");
        assert_base(&obj, (1.0, 2.0, 8.0));

        close_document(py, "Part2DFlatFace");
    });
}

#[test]
fn object_xy_follows_the_support_objects_placement() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "Part2DObjectXY");
        let box_ = doc
            .call_method1("addObject", ("Part::Box", "SupportBox"))
            .expect("addObject Part::Box");
        let z_axis = vector(py, 0.0, 0.0, 1.0);
        box_
            .setattr(
                "Placement",
                placement(py, (10.0, 20.0, 30.0), Some((&z_axis, 90.0))),
            )
            .expect("support.Placement");
        doc.call_method0("recompute").expect("recompute");

        let obj = doc
            .call_method1(
                "addObject",
                ("Part::Part2DObjectPython", "Attached", &proxy(py, "attach_xy_proxy"), py.None()),
            )
            .expect("addObject Part::Part2DObjectPython");
        set_support(&obj, &box_);
        obj.setattr("MapMode", "ObjectXY").expect("MapMode");
        doc.call_method0("recompute").expect("recompute");

        // ObjectXY is the support's own frame: origin at its placement's base.
        assert_base(&obj, (10.0, 20.0, 30.0));
        // The local Z axis is the support's Z axis (the rotation is about Z).
        let z = obj
            .getattr("Placement")
            .unwrap()
            .getattr("Rotation")
            .unwrap()
            .call_method1("multVec", (vector(py, 0.0, 0.0, 1.0),))
            .expect("multVec");
        assert!(
            (z.getattr("x").unwrap().extract::<f64>().unwrap()).abs() < 1e-9
                && (z.getattr("y").unwrap().extract::<f64>().unwrap()).abs() < 1e-9
                && (z.getattr("z").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9,
            "ObjectXY keeps the support's +Z normal"
        );

        // The local X axis follows the support's rotated X axis.
        let x = obj
            .getattr("Placement")
            .unwrap()
            .getattr("Rotation")
            .unwrap()
            .call_method1("multVec", (vector(py, 1.0, 0.0, 0.0),))
            .expect("multVec");
        assert!(
            (x.getattr("x").unwrap().extract::<f64>().unwrap()).abs() < 1e-9
                && (x.getattr("y").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9,
            "ObjectXY uses the support's rotated X axis"
        );

        close_document(py, "Part2DObjectXY");
    });
}

#[test]
fn deactivated_keeps_the_placement_the_script_set() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "Part2DDeactivated");
        let box_ = doc
            .call_method1("addObject", ("Part::Box", "SupportBox"))
            .expect("addObject Part::Box");
        doc.call_method0("recompute").expect("recompute");

        let obj = doc
            .call_method1(
                "addObject",
                ("Part::Part2DObjectPython", "Free", &proxy(py, "attach_free_proxy"), py.None()),
            )
            .expect("addObject Part::Part2DObjectPython");
        set_support(&obj, &box_);
        obj.setattr("Placement", placement(py, (5.0, 6.0, 7.0), None))
            .expect("Placement");
        assert_eq!(obj.getattr("MapMode").unwrap().extract::<String>().unwrap(), "Deactivated");
        doc.call_method0("recompute").expect("recompute");

        assert_base(&obj, (5.0, 6.0, 7.0));
        assert!(!state(&obj).iter().any(|state| state == "Invalid"));

        close_document(py, "Part2DDeactivated");
    });
}

#[test]
fn an_unusable_support_leaves_the_object_invalid() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "Part2DBadSupport");
        // A solid is not a flat face; FlatFace cannot use it.
        let box_ = doc
            .call_method1("addObject", ("Part::Box", "SupportBox"))
            .expect("addObject Part::Box");
        doc.call_method0("recompute").expect("recompute");

        let obj = doc
            .call_method1(
                "addObject",
                ("Part::Part2DObjectPython", "Broken", &proxy(py, "attach_broken_proxy"), py.None()),
            )
            .expect("addObject Part::Part2DObjectPython");
        set_support(&obj, &box_);
        obj.setattr("MapMode", "FlatFace").expect("MapMode");
        doc.call_method0("recompute").expect("recompute");

        assert!(
            state(&obj).iter().any(|state| state == "Invalid"),
            "an unsatisfiable attachment leaves the object invalid: {:?}",
            state(&obj)
        );

        // An active mode with no support at all is the same honest failure.
        obj.setattr("AttachmentSupport", py.None()).expect("clear support");
        doc.call_method0("recompute").expect("recompute");
        assert!(state(&obj).iter().any(|state| state == "Invalid"));

        close_document(py, "Part2DBadSupport");
    });
}

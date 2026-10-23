//! `App::Link` end to end: the query API, the shared property base and the
//! Part-side execute path.
//!
//! Same one-image boot as `part_link_array`: `ferrocad` + `Part` registered as
//! built-ins, because CPython initialisation is process-wide and concurrent
//! `with_gil` bootstraps abort the process.

use std::sync::{Mutex, Once};

use pyo3::prelude::*;
use pyo3::types::PyModule;

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

fn new_document<'py>(
    ferrocad: &Bound<'py, PyModule>,
    name: &str,
) -> PyResult<Bound<'py, PyAny>> {
    ferrocad.call_method1("newDocument", (name,))
}

fn close_document(ferrocad: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    ferrocad.call_method1("closeDocument", (name,))?;
    Ok(())
}

fn vector<'py>(ferrocad: &Bound<'py, PyModule>, x: f64, y: f64, z: f64) -> PyResult<Bound<'py, PyAny>> {
    ferrocad.getattr("Vector")?.call1((x, y, z))
}

fn placement<'py>(
    ferrocad: &Bound<'py, PyModule>,
    x: f64,
    y: f64,
    z: f64,
) -> PyResult<Bound<'py, PyAny>> {
    ferrocad
        .getattr("Placement")?
        .call1((vector(ferrocad, x, y, z)?, ferrocad.getattr("Rotation")?.call0()?))
}

fn bound(shape: &Bound<'_, PyAny>, key: &str) -> PyResult<f64> {
    shape.getattr("BoundBox")?.getattr(key)?.extract()
}

fn name_of(object: &Bound<'_, PyAny>) -> PyResult<String> {
    object.getattr("Name")?.extract()
}

#[test]
fn is_link_recognises_app_link_and_the_array_peers() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkIsLink").expect("newDocument");
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayPoint", "Array"))
            .expect("Part::LinkArrayPoint");
        let feature = doc
            .call_method1("addObject", ("Part::Box", "Box"))
            .expect("Part::Box");

        assert!(link.call_method0("isLink").unwrap().extract::<bool>().unwrap());
        // `Part::LinkArray*` are peers, not `App::Link` subclasses; the query API
        // treats them as links anyway.
        assert!(array.call_method0("isLink").unwrap().extract::<bool>().unwrap());
        // The link base also makes `setLink` available on the array peers.
        array
            .call_method1("setLink", (&feature,))
            .expect("array.setLink");
        assert_eq!(
            name_of(&array.getattr("LinkedObject").unwrap()).unwrap(),
            "Box"
        );
        assert!(!feature.call_method0("isLink").unwrap().extract::<bool>().unwrap());

        close_document(&ferrocad, "RustLinkIsLink").expect("closeDocument");
    });
}

#[test]
fn get_linked_object_follows_a_two_link_chain() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkChain").expect("newDocument");
        let box_object = doc
            .call_method1("addObject", ("Part::Box", "Box"))
            .expect("Part::Box");
        let first = doc
            .call_method1("addObject", ("App::Link", "First"))
            .expect("First link");
        let second = doc
            .call_method1("addObject", ("App::Link", "Second"))
            .expect("Second link");
        let empty = doc
            .call_method1("addObject", ("App::Link", "Empty"))
            .expect("Empty link");

        first.setattr("LinkedObject", &box_object).expect("First.LinkedObject");
        second.setattr("LinkedObject", &first).expect("Second.LinkedObject");
        doc.call_method0("recompute").expect("recompute");

        // resolve=True (default) walks to the final non-link target.
        let target = second
            .call_method0("getLinkedObject")
            .expect("getLinkedObject")
            .getattr("Name")
            .unwrap()
            .extract::<String>()
            .unwrap();
        assert_eq!(target, "Box");

        // resolve=False stops at the immediate target, which is still a link.
        let immediate = second
            .call_method1("getLinkedObject", (false,))
            .expect("getLinkedObject(resolve=False)")
            .getattr("Name")
            .unwrap()
            .extract::<String>()
            .unwrap();
        assert_eq!(immediate, "First");

        // A plain object returns itself; an unresolved link returns None.
        assert_eq!(
            name_of(&box_object.call_method0("getLinkedObject").unwrap()).unwrap(),
            "Box"
        );
        assert!(empty.call_method0("getLinkedObject").unwrap().is_none());
        assert!(empty
            .call_method1("getLinkedObject", (false,))
            .unwrap()
            .is_none());

        close_document(&ferrocad, "RustLinkChain").expect("closeDocument");
    });
}

#[test]
fn element_references_parse_and_resolve_through_element_list() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkElements").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayLinear", "Array"))
            .expect("Array");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("Occurrences", 3).expect("Occurrences");
        doc.call_method0("recompute").expect("recompute");

        // `ShowElement` defaults to true, so the elements exist as objects.
        let elements = array.getattr("ElementList").expect("ElementList");
        assert_eq!(elements.len().unwrap(), 3);
        let element_one = elements.get_item(1).expect("element 1");
        assert_eq!(name_of(&element_one).unwrap(), "Array_Element1");

        // Every accepted spelling reports the same index.
        for spelling in ["Element1", "Array.Element1", "Array_Element1", "1."] {
            assert_eq!(
                array
                    .call_method1("getElementIndex", (spelling,))
                    .unwrap()
                    .extract::<i64>()
                    .unwrap(),
                1,
                "{spelling}"
            );
        }
        assert_eq!(
            array
                .call_method1("getElementIndex", ("Nope",))
                .unwrap()
                .extract::<i64>()
                .unwrap(),
            -1
        );

        // Path resolution: relative, dotted with the owner prefix, and with the
        // trailing-dot spelling upstream accepts.
        for subname in ["Element1", "Element1.", "Array.Element1"] {
            let resolved = array
                .call_method1("getSubObject", (subname, 1))
                .unwrap();
            assert_eq!(name_of(&resolved).unwrap(), "Array_Element1", "{subname}");
        }
        // `retType=0` is the default and also returns the object.
        let default_type = array.call_method1("getSubObject", ("Element1",)).unwrap();
        assert_eq!(name_of(&default_type).unwrap(), "Array_Element1");

        // retType 3 reports the resolved object's own placement.
        let typed = array.call_method1("getSubObject", ("Element1", 3)).unwrap();
        let own = element_one.getattr("Placement").unwrap();
        let typed_base = typed.getattr("Base").unwrap();
        let own_base = own.getattr("Base").unwrap();
        for axis in ["x", "y", "z"] {
            let a: f64 = typed_base.getattr(axis).unwrap().extract().unwrap();
            let b: f64 = own_base.getattr(axis).unwrap().extract().unwrap();
            assert!((a - b).abs() < 1e-9, "{axis}: {a} vs {b}");
        }

        // The empty subname is the object itself (upstream `getSubObject("", 1)`).
        let itself = array.call_method1("getSubObject", ("", 1)).unwrap();
        assert_eq!(name_of(&itself).unwrap(), "Array");

        close_document(&ferrocad, "RustLinkElements").expect("closeDocument");
    });
}

#[test]
fn app_link_transforms_the_linked_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkShape").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        // Part::Box defaults: 10 x 10 x 10 at the origin.
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        link.setattr("LinkedObject", &source).expect("LinkedObject");

        link.setattr("Placement", placement(&ferrocad, 20.0, 0.0, 0.0).unwrap())
            .expect("Placement");
        doc.call_method0("recompute").expect("recompute");
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!((bound(&shape, "XMin").unwrap() - 20.0).abs() < 1e-6);
        assert!((bound(&shape, "XMax").unwrap() - 30.0).abs() < 1e-6);
        assert!((bound(&shape, "YMin").unwrap()).abs() < 1e-6);

        // `LinkPlacement` is the global placement, used when `LinkTransform` is set.
        link.setattr("LinkTransform", true).expect("LinkTransform");
        link.setattr("LinkPlacement", placement(&ferrocad, 5.0, 0.0, 0.0).unwrap())
            .expect("LinkPlacement");
        doc.call_method0("recompute").expect("recompute");
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!((bound(&shape, "XMin").unwrap() - 5.0).abs() < 1e-6);
        assert!((bound(&shape, "XMax").unwrap() - 15.0).abs() < 1e-6);

        // Scale composes with the placement (uniform scale only for now).
        link.setattr("LinkTransform", false).expect("LinkTransform");
        link.setattr("Placement", placement(&ferrocad, 0.0, 0.0, 0.0).unwrap())
            .expect("Placement");
        link.setattr("Scale", 2.0).expect("Scale");
        doc.call_method0("recompute").expect("recompute");
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!((bound(&shape, "XMin").unwrap()).abs() < 1e-6);
        assert!((bound(&shape, "XMax").unwrap() - 20.0).abs() < 1e-6);

        close_document(&ferrocad, "RustLinkShape").expect("closeDocument");
    });
}

#[test]
fn link_to_an_object_without_a_shape_stays_valid_and_empty() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkEmpty").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Feature", "NoShape"))
            .expect("shape-less feature");
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        link.setattr("LinkedObject", &source).expect("LinkedObject");

        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            link.call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!(shape.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        // A dangling target is empty too, not invalid.
        link.setattr("LinkedObject", py.None()).expect("clear LinkedObject");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            link.call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );

        close_document(&ferrocad, "RustLinkEmpty").expect("closeDocument");
    });
}

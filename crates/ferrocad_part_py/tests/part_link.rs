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

#[test]
fn show_element_expands_into_real_children_and_collapses_back() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkElementsL3").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        link.setattr("LinkedObject", &source).expect("LinkedObject");
        // The upstream default grid expands one element per unit on X.
        link.setattr("ElementCount", 4).expect("ElementCount");
        doc.call_method0("recompute").expect("recompute");

        let elements = link.getattr("ElementList").expect("ElementList");
        assert_eq!(elements.len().unwrap(), 4);
        for index in 0..4 {
            let element = elements.get_item(index).unwrap();
            assert_eq!(name_of(&element).unwrap(), format!("Link_Element{index}"));
            assert_eq!(
                element.getattr("TypeId").unwrap().extract::<String>().unwrap(),
                "App::LinkElement"
            );
            assert!(element.call_method0("isLink").unwrap().extract::<bool>().unwrap());
            let shape = element.getattr("Shape").expect("element Shape");
            assert!(!shape.call_method0("isNull").unwrap().extract::<bool>().unwrap());
            assert!((bound(&shape, "XMin").unwrap() - index as f64).abs() < 1e-6);
            let base = element.getattr("Placement").unwrap().getattr("Base").unwrap();
            let x: f64 = base.getattr("x").unwrap().extract().unwrap();
            assert!((x - index as f64).abs() < 1e-9);
            // Each child links back to the source, not to the owning link
            // (which would make the dependency graph cyclic).
            let linked = element.call_method0("getLinkedObject").unwrap();
            assert_eq!(name_of(&linked).unwrap(), "Source");
        }
        // While expanded the children carry the geometry.
        assert!(link
            .getattr("Shape")
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());

        // `getSubObject` reaches the real child, absolutely and relatively.
        for subname in ["Element3", "Link.Element3"] {
            let resolved = link.call_method1("getSubObject", (subname, 1)).unwrap();
            assert_eq!(name_of(&resolved).unwrap(), "Link_Element3", "{subname}");
        }
        let typed = link.call_method1("getSubObject", ("Element3", 3)).unwrap();
        let x: f64 = typed
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((x - 3.0).abs() < 1e-9);
        // An element spelling that is out of range is missing, not the target.
        assert!(link
            .call_method1("getSubObject", ("Element9", 1))
            .unwrap()
            .is_none());
        // A non-element sub-path still forwards to the linked target.
        let face = link.call_method1("getSubObject", ("Face1", 1)).unwrap();
        assert_eq!(name_of(&face).unwrap(), "Source");

        // Collapsing removes the children and writes the transformed compound.
        link.setattr("ShowElement", false).expect("ShowElement = false");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(link.getattr("ElementList").unwrap().len().unwrap(), 0);
        for index in 0..4 {
            assert!(doc
                .call_method1("getObject", (format!("Link_Element{index}"),))
                .unwrap()
                .is_none());
        }
        let stored = link.getattr("PlacementList").expect("PlacementList");
        assert_eq!(stored.len().unwrap(), 4);
        let x: f64 = stored
            .get_item(3)
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((x - 3.0).abs() < 1e-9);
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!(!shape.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert!((bound(&shape, "XMin").unwrap()).abs() < 1e-6);
        assert!((bound(&shape, "XMax").unwrap() - 13.0).abs() < 1e-6);

        // Expanding restores the same children at the placements it stored.
        link.setattr("ShowElement", true).expect("ShowElement = true");
        doc.call_method0("recompute").expect("recompute");
        let elements = link.getattr("ElementList").expect("ElementList");
        assert_eq!(elements.len().unwrap(), 4);
        let third = elements.get_item(2).unwrap();
        assert_eq!(name_of(&third).unwrap(), "Link_Element2");
        let x: f64 = third
            .getattr("Placement")
            .unwrap()
            .getattr("Base")
            .unwrap()
            .getattr("x")
            .unwrap()
            .extract()
            .unwrap();
        assert!((x - 2.0).abs() < 1e-9);
        assert!(!third
            .getattr("Shape")
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());

        // Shrinking the element count removes the surplus children entirely.
        link.setattr("ElementCount", 2).expect("ElementCount = 2");
        // ElementCount is NoRecompute; touch the link through a normal property.
        link.setattr("ShowElement", true).expect("touch");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(link.getattr("ElementList").unwrap().len().unwrap(), 2);
        assert!(doc
            .call_method1("getObject", ("Link_Element3",))
            .unwrap()
            .is_none());

        close_document(&ferrocad, "RustLinkElementsL3").expect("closeDocument");
    });
}

#[test]
fn array_element_paths_reach_generated_children() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustArrayElementsL3").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayLinear", "Array"))
            .expect("Array");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("Occurrences", 4).expect("Occurrences");
        doc.call_method0("recompute").expect("recompute");

        let elements = array.getattr("ElementList").expect("ElementList");
        assert_eq!(elements.len().unwrap(), 4);
        let third = elements.get_item(3).unwrap();
        assert_eq!(name_of(&third).unwrap(), "Array_Element3");
        assert_eq!(
            third.getattr("TypeId").unwrap().extract::<String>().unwrap(),
            "App::LinkElement"
        );
        assert!(!third
            .getattr("Shape")
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert_eq!(
            name_of(&third.call_method0("getLinkedObject").unwrap()).unwrap(),
            "Source"
        );

        for subname in ["Element3", "Array.Element3"] {
            let resolved = array.call_method1("getSubObject", (subname, 1)).unwrap();
            assert_eq!(name_of(&resolved).unwrap(), "Array_Element3", "{subname}");
        }

        // The pre-L3 type id stays registered, with the same element surface.
        let legacy = doc
            .call_method1("addObject", ("Part::LinkArrayElement", "LegacyElement"))
            .expect("legacy element type");
        for property in ["Shape", "Placement", "Suppressed", "LinkedObject"] {
            assert!(legacy.getattr(property).is_ok(), "legacy {property}");
        }

        // Collapsing the array removes the generated objects and keeps the
        // placements it needs to expand them again.
        array.setattr("ShowElement", false).expect("ShowElement = false");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(array.getattr("ElementList").unwrap().len().unwrap(), 0);
        assert!(doc
            .call_method1("getObject", ("Array_Element3",))
            .unwrap()
            .is_none());
        assert_eq!(array.getattr("PlacementList").unwrap().len().unwrap(), 4);
        assert!(!array
            .getattr("Shape")
            .unwrap()
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());

        close_document(&ferrocad, "RustArrayElementsL3").expect("closeDocument");
    });
}

#[test]
fn non_uniform_scale_vector_stretches_the_linked_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkScaleVector").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        link.setattr("LinkedObject", &source).expect("LinkedObject");
        // Non-uniform: the general affine path, not the gp_Trsf similarity path.
        link.setattr(
            "ScaleVector",
            vector(&ferrocad, 2.0, 1.0, 1.0).unwrap(),
        )
        .expect("ScaleVector");
        doc.call_method0("recompute").expect("recompute");

        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!(!shape.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        // Part::Box defaults to 10 x 10 x 10 at the origin.
        assert!((bound(&shape, "XMin").unwrap()).abs() < 1e-6);
        assert!((bound(&shape, "XMax").unwrap() - 20.0).abs() < 1e-6);
        assert!((bound(&shape, "YMin").unwrap()).abs() < 1e-6);
        assert!((bound(&shape, "YMax").unwrap() - 10.0).abs() < 1e-6);
        assert!((bound(&shape, "ZMin").unwrap()).abs() < 1e-6);
        assert!((bound(&shape, "ZMax").unwrap() - 10.0).abs() < 1e-6);

        // `Scale` multiplies every component of the vector.
        link.setattr("Scale", 0.5).expect("Scale");
        doc.call_method0("recompute").expect("recompute");
        let shape = link.getattr("Shape").expect("Link.Shape");
        assert!((bound(&shape, "XMax").unwrap() - 10.0).abs() < 1e-6);
        assert!((bound(&shape, "YMax").unwrap() - 5.0).abs() < 1e-6);

        close_document(&ferrocad, "RustLinkScaleVector").expect("closeDocument");
    });
}

#[test]
fn link_group_expands_its_members() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkGroup").expect("newDocument");
        let first = doc
            .call_method1("addObject", ("Part::Box", "BoxA"))
            .expect("BoxA");
        let second = doc
            .call_method1("addObject", ("Part::Box", "BoxB"))
            .expect("BoxB");
        let group = doc
            .call_method1("addObject", ("App::LinkGroup", "Group"))
            .expect("App::LinkGroup");
        group
            .setattr("Group", (&first, &second))
            .expect("Group");
        doc.call_method0("recompute").expect("recompute");

        // A container's `ElementList` is its membership, not generated children.
        assert_eq!(group.getattr("ElementCount").unwrap().extract::<i64>().unwrap(), 2);
        let elements = group.getattr("ElementList").expect("ElementList");
        assert_eq!(elements.len().unwrap(), 2);
        assert_eq!(name_of(&elements.get_item(0).unwrap()).unwrap(), "BoxA");
        assert_eq!(name_of(&elements.get_item(1).unwrap()).unwrap(), "BoxB");

        // `ElementN` and the member's own name both resolve through the group.
        for (subname, expected) in [
            ("Element1", "BoxA"),
            ("Element2", "BoxB"),
            ("Group.Element2", "BoxB"),
            ("BoxA", "BoxA"),
        ] {
            let resolved = group.call_method1("getSubObject", (subname, 1)).unwrap();
            assert_eq!(name_of(&resolved).unwrap(), expected, "{subname}");
        }
        // Out of range is missing, and the group still writes a compound shape.
        assert!(group
            .call_method1("getSubObject", ("Element3", 1))
            .unwrap()
            .is_none());
        let shape = group.getattr("Shape").expect("Group.Shape");
        assert!(!shape.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        // `App::LinkList` follows the same list: Element1 is its first member.
        let third = doc
            .call_method1("addObject", ("Part::Box", "BoxC"))
            .expect("BoxC");
        let fourth = doc
            .call_method1("addObject", ("Part::Box", "BoxD"))
            .expect("BoxD");
        let list = doc
            .call_method1("addObject", ("App::LinkList", "List"))
            .expect("App::LinkList");
        list.setattr("Group", (&third, &fourth)).expect("List.Group");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            name_of(&list.call_method1("getSubObject", ("Element1", 1)).unwrap()).unwrap(),
            "BoxC"
        );
        assert_eq!(
            name_of(&list.call_method1("getSubObject", ("Element2", 1)).unwrap()).unwrap(),
            "BoxD"
        );

        close_document(&ferrocad, "RustLinkGroup").expect("closeDocument");
    });
}

#[test]
fn is_link_and_get_linked_object_on_the_containers() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkContainers").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Box"))
            .expect("Box");
        let group = doc
            .call_method1("addObject", ("App::LinkGroup", "Group"))
            .expect("App::LinkGroup");
        let list = doc
            .call_method1("addObject", ("App::LinkList", "List"))
            .expect("App::LinkList");

        for container in [&group, &list] {
            assert!(container
                .call_method0("isLink")
                .unwrap()
                .extract::<bool>()
                .unwrap());
            // With no `LinkedObject`, a container is an empty link, exactly like
            // an `App::Link` with no target.
            assert!(container.call_method0("getLinkedObject").unwrap().is_none());
            assert!(container
                .call_method1("getLinkedObject", (false,))
                .unwrap()
                .is_none());

            container
                .setattr("LinkedObject", &source)
                .expect("LinkedObject");
            doc.call_method0("recompute").expect("recompute");
            assert_eq!(
                name_of(&container.call_method0("getLinkedObject").unwrap()).unwrap(),
                "Box"
            );
            assert_eq!(
                name_of(&container.call_method1("getLinkedObject", (false,)).unwrap())
                    .unwrap(),
                "Box"
            );
        }

        close_document(&ferrocad, "RustLinkContainers").expect("closeDocument");
    });
}

#[test]
fn link_mode_and_copy_on_change_are_stored_but_inert() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustLinkModes").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("Source box");
        let link = doc
            .call_method1("addObject", ("App::Link", "Link"))
            .expect("App::Link");
        link.setattr("LinkedObject", &source).expect("LinkedObject");
        doc.call_method0("recompute").expect("recompute");

        // All three LinkMode values are accepted and stored, but none changes
        // the geometry: a FerroCAD link is always a name reference (KeepLink),
        // and AutoDelete/Translate need lifecycle or copy machinery we do not have.
        for mode in ["AutoDelete", "KeepLink", "Translate"] {
            link.setattr("LinkMode", mode).expect("LinkMode");
            doc.call_method0("recompute").expect("recompute");
            assert_eq!(
                link.getattr("LinkMode")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                mode
            );
            let shape = link.getattr("Shape").expect("Link.Shape");
            assert!((bound(&shape, "XMax").unwrap() - 10.0).abs() < 1e-6, "{mode}");
        }

        // Copy-on-change is inert for the same reason: there is no clone/copy
        // facility, so every value is stored and none is honoured.
        for mode in ["Disabled", "Enabled", "Owned", "Tracking"] {
            link.setattr("LinkCopyOnChange", mode)
                .expect("LinkCopyOnChange");
            doc.call_method0("recompute").expect("recompute");
            assert_eq!(
                link.getattr("LinkCopyOnChange")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                mode
            );
            let shape = link.getattr("Shape").expect("Link.Shape");
            assert!((bound(&shape, "XMax").unwrap() - 10.0).abs() < 1e-6, "{mode}");
        }

        close_document(&ferrocad, "RustLinkModes").expect("closeDocument");
    });
}

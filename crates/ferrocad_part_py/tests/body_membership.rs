//! `PartDesign::Body` membership, declared by the type instead of a list.
//!
//! Upstream `Body::isAllowed` branches on the member's C++ base class. A registered
//! behaviour has no class hierarchy, so each type declares what it contributes
//! (`ObjectType::body_membership`) and `Document::body_add_check` applies the
//! rules: a `Base` only while the body is empty of solids (and it becomes
//! `BaseFeature`), a `Support` never becomes the `Tip`, a `Feature` does, and
//! `Denied` is refused. A direct `Group` write is checked the same way.
//!
//! Same one-image boot as `part_link`: `ferrocad` + `Part` as built-ins.

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

/// The `Name` a link property (`Tip`, `BaseFeature`) points at, if any.
fn linked_name(object: &Bound<'_, PyAny>, property: &str) -> Option<String> {
    object
        .getattr(property)
        .ok()
        .and_then(|link| link.getattr("Name").ok())
        .and_then(|name| name.extract::<String>().ok())
}

/// The `Group` member names, in order.
fn group_names(body: &Bound<'_, PyAny>) -> Vec<String> {
    body.getattr("Group")
        .expect("Group")
        .try_iter()
        .expect("iterable Group")
        .map(|member| {
            member
                .expect("a member")
                .getattr("Name")
                .expect("Name")
                .extract::<String>()
                .expect("name")
        })
        .collect()
}

#[test]
fn a_feature_is_a_member_and_becomes_the_tip() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustBodyFeature").expect("newDocument");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("body");
        let first = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "First"))
            .expect("first feature");
        let second = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Second"))
            .expect("second feature");

        body.call_method1("addObject", (&first,)).expect("add first");
        assert_eq!(group_names(&body), vec!["First"]);
        assert_eq!(linked_name(&body, "Tip"), Some("First".to_string()));
        assert_eq!(linked_name(&body, "BaseFeature"), None);

        body.call_method1("addObject", (&second,))
            .expect("add second");
        assert_eq!(group_names(&body), vec!["First", "Second"]);
        assert_eq!(
            linked_name(&body, "Tip"),
            Some("Second".to_string()),
            "the newest feature is the tip"
        );

        // A support member is allowed but changes neither link.
        let sketch = doc
            .call_method1("addObject", ("Sketcher::SketchObject", "Sketch"))
            .expect("sketch");
        body.call_method1("addObject", (&sketch,))
            .expect("add sketch");
        assert_eq!(group_names(&body), vec!["First", "Second", "Sketch"]);
        assert_eq!(linked_name(&body, "Tip"), Some("Second".to_string()));

        close_document(&ferrocad, "RustBodyFeature").expect("closeDocument");
    });
}

#[test]
fn a_part_shape_is_only_a_base_feature() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustBodyBase").expect("newDocument");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("body");
        let base = doc
            .call_method1("addObject", ("Part::Feature", "Base"))
            .expect("base shape");

        body.call_method1("addObject", (&base,)).expect("add base");
        assert_eq!(group_names(&body), vec!["Base"]);
        assert_eq!(linked_name(&body, "BaseFeature"), Some("Base".to_string()));
        assert_eq!(linked_name(&body, "Tip"), None);

        // A second plain shape is refused, and the refusal leaves the body alone.
        let other = doc
            .call_method1("addObject", ("Part::FeaturePython", "Other"))
            .expect("python shape");
        let rejected = body.call_method1("addObject", (&other,));
        let error = rejected.expect_err("a second base is refused");
        assert!(
            error.is_instance_of::<pyo3::exceptions::PyValueError>(py),
            "expected ValueError, got {error:?}"
        );
        assert!(
            error.to_string().contains("base feature"),
            "the refusal names the rule: {error}"
        );
        assert_eq!(group_names(&body), vec!["Base"]);
        assert_eq!(linked_name(&body, "BaseFeature"), Some("Base".to_string()));

        // A feature may join; then another shape is refused too.
        let feature = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Pad"))
            .expect("feature");
        body.call_method1("addObject", (&feature,))
            .expect("add feature");
        assert_eq!(linked_name(&body, "Tip"), Some("Pad".to_string()));
        assert!(body.call_method1("addObject", (&other,)).is_err());

        close_document(&ferrocad, "RustBodyBase").expect("closeDocument");
    });
}

#[test]
fn a_denied_type_is_refused_with_a_reason() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustBodyDenied").expect("newDocument");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("body");

        // A shape-less Python feature declares nothing, so it is denied.
        let proxy_feature = doc
            .call_method1("addObject", ("App::FeaturePython", "ProxyFeature"))
            .expect("App::FeaturePython");
        let error = body
            .call_method1("addObject", (&proxy_feature,))
            .expect_err("a shape-less feature is refused");
        assert!(error.to_string().contains("is not allowed"), "{error}");
        assert!(group_names(&body).is_empty());

        // A body cannot contain another body.
        let nested = doc
            .call_method1("addObject", ("PartDesign::Body", "Nested"))
            .expect("nested body");
        let error = body
            .call_method1("addObject", (&nested,))
            .expect_err("a nested body is refused");
        assert!(error.to_string().contains("is not allowed"), "{error}");

        // `App::Part` is not policed: it accepts the same object.
        let part = doc
            .call_method1("addObject", ("App::Part", "Part"))
            .expect("App::Part");
        part.call_method1("addObject", (&proxy_feature,))
            .expect("App::Part accepts anything");
        assert_eq!(group_names(&part), vec!["ProxyFeature"]);

        close_document(&ferrocad, "RustBodyDenied").expect("closeDocument");
    });
}

#[test]
fn a_direct_group_write_is_checked() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustBodyGroup").expect("newDocument");
        let body = doc
            .call_method1("addObject", ("PartDesign::Body", "Body"))
            .expect("body");
        let first = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "First"))
            .expect("first");
        let second = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Second"))
            .expect("second");
        let box_ = doc
            .call_method1("addObject", ("Part::Feature", "Shape"))
            .expect("shape");
        let proxy_feature = doc
            .call_method1("addObject", ("App::FeaturePython", "ProxyFeature"))
            .expect("shape-less feature");

        // A valid list is accepted wholesale (features and a support member).
        body.setattr("Group", PyList::new(py, [&first, &second]).unwrap())
            .expect("features are members");
        assert_eq!(group_names(&body), vec!["First", "Second"]);

        // A denied member rejects the assignment and leaves the list as it was.
        let rejected = body.setattr("Group", PyList::new(py, [&first, &proxy_feature]).unwrap());
        assert!(
            rejected
                .expect_err("the write is vetoed")
                .is_instance_of::<pyo3::exceptions::PyValueError>(py)
        );
        assert_eq!(group_names(&body), vec!["First", "Second"]);

        // Two base solids in one list are refused.
        let second_shape = doc
            .call_method1("addObject", ("Part::Feature", "Shape2"))
            .expect("second shape");
        let rejected = body.setattr("Group", PyList::new(py, [&box_, &second_shape]).unwrap());
        assert!(
            rejected
                .expect_err("one base feature only")
                .to_string()
                .contains("only one base feature")
        );
        assert_eq!(group_names(&body), vec!["First", "Second"]);

        // The generic group is unaffected by the rule.
        let part = doc
            .call_method1("addObject", ("App::Part", "Part"))
            .expect("App::Part");
        part.setattr("Group", PyList::new(py, [&box_, &second_shape]).unwrap())
            .expect("App::Part takes both");
        assert_eq!(group_names(&part), vec!["Shape", "Shape2"]);

        close_document(&ferrocad, "RustBodyGroup").expect("closeDocument");
    });
}

#[test]
fn a_member_moves_between_bodies() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = new_document(&ferrocad, "RustBodyMove").expect("newDocument");
        let first = doc
            .call_method1("addObject", ("PartDesign::Body", "BodyA"))
            .expect("body A");
        let second = doc
            .call_method1("addObject", ("PartDesign::Body", "BodyB"))
            .expect("body B");
        let feature = doc
            .call_method1("addObject", ("PartDesign::AdditiveBox", "Pad"))
            .expect("feature");

        first.call_method1("addObject", (&feature,)).expect("add A");
        second
            .call_method1("addObject", (&feature,))
            .expect("add B");
        assert!(group_names(&first).is_empty(), "A loses the member");
        assert_eq!(group_names(&second), vec!["Pad"]);
        assert_eq!(linked_name(&second, "Tip"), Some("Pad".to_string()));

        close_document(&ferrocad, "RustBodyMove").expect("closeDocument");
    });
}

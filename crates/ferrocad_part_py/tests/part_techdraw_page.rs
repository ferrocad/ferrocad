//! The TechDraw document types a Draft view needs: page, template, view.
//!
//! FerroCAD does not implement the TechDraw workbench; it registers the three
//! document types upstream's `drafttests/test_modification.py::test_draft_to_techdraw`
//! builds (`Part::Prism` → `TechDraw::DrawViewDraft` → `TechDraw::DrawPage`), the
//! generic `DocumentObject.addView`, and the outbound-link `OutList` those types
//! rely on. Nothing draws or projects here.
//!
//! Same one-image boot as `part_link`: `ferrocad` + `Part` registered as
//! built-ins, because CPython initialisation is process-wide and concurrent
//! `with_gil` bootstraps abort the process.

use std::sync::{Mutex, Once};

use pyo3::exceptions::PyAttributeError;
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

fn new_document<'py>(ferrocad: &Bound<'py, PyModule>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    ferrocad.call_method1("newDocument", (name,))
}

fn close_document(ferrocad: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    ferrocad.call_method1("closeDocument", (name,))?;
    Ok(())
}

/// The `Name`s of an `OutList`/`InList`, in order.
fn link_names(object: &Bound<'_, PyAny>, property: &str) -> PyResult<Vec<String>> {
    let links = object.getattr(property)?;
    let mut names = Vec::new();
    for link in links.try_iter()? {
        names.push(link?.getattr("Name")?.extract::<String>()?);
    }
    Ok(names)
}

#[test]
fn a_draft_view_is_an_outbound_link_of_its_page() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = new_document(&ferrocad, "RustTechDrawPage").expect("newDocument");
        let prism = doc
            .call_method1("addObject", ("Part::Prism", "Prism"))
            .expect("Part::Prism");
        let page = doc
            .call_method1("addObject", ("TechDraw::DrawPage", "Page"))
            .expect("TechDraw::DrawPage");
        let template = doc
            .call_method1("addObject", ("TechDraw::DrawSVGTemplate", "Template"))
            .expect("TechDraw::DrawSVGTemplate");
        let view = doc
            .call_method1("addObject", ("TechDraw::DrawViewDraft", "DraftView"))
            .expect("TechDraw::DrawViewDraft");

        assert_eq!(
            page.getattr("TypeId").unwrap().extract::<String>().unwrap(),
            "TechDraw::DrawPage"
        );

        // The page's template is a link; the view links its source and carries a
        // projection direction (upstream's test assigns `(0, 0, 1)`).
        page.setattr("Template", &template).expect("page.Template");
        template
            .setattr("Template", "/tmp/A3_Landscape_blank.svg")
            .expect("template.Template");
        view.setattr("Source", &prism).expect("view.Source");
        view.setattr(
            "Direction",
            ferrocad
                .getattr("Vector")
                .unwrap()
                .call1((0.0, 0.0, 1.0))
                .unwrap(),
        )
        .expect("view.Direction");
        page.call_method1("addView", (&view,)).expect("page.addView");

        // Upstream asserts `view in page.OutList`; `OutList` is every outbound
        // link, so the template is there too.
        assert_eq!(
            link_names(&page, "OutList").expect("page.OutList"),
            vec!["Template".to_string(), "DraftView".to_string()]
        );

        // The link properties read back, and `OutList` deduplicates a repeated
        // `addView` rather than listing the view twice.
        let owner = page
            .getattr("Template")
            .expect("page.Template")
            .getattr("Name")
            .unwrap()
            .extract::<String>()
            .unwrap();
        assert_eq!(owner, "Template");
        page.call_method1("addView", (&view,)).expect("addView again");
        assert_eq!(
            link_names(&page, "OutList").expect("page.OutList").len(),
            2,
            "a repeated addView must not duplicate the link"
        );

        // The view links back to its source, and the source sees the view.
        assert_eq!(
            link_names(&view, "OutList").expect("view.OutList"),
            vec!["Prism".to_string()]
        );
        assert_eq!(
            link_names(&prism, "InList").expect("prism.InList"),
            vec!["DraftView".to_string()]
        );

        // `addView` belongs to the page; another type keeps `AttributeError`.
        let rejected = prism.call_method1("addView", (&view,));
        let error = rejected.expect_err("Part::Prism has no addView");
        assert!(
            error.is_instance_of::<PyAttributeError>(py),
            "expected AttributeError, got {error:?}"
        );

        close_document(&ferrocad, "RustTechDrawPage").expect("closeDocument");
    });
}

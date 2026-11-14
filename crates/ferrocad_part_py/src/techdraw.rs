//! The TechDraw document objects the Draft suite creates.
//!
//! FerroCAD does not implement the TechDraw workbench: `python/TechDraw`
//! supplies only the `makeGeomHatch` that Draft's hatch object imports. But
//! upstream's `test_modification` builds a **Draft view on a page**
//! (`Part::Prism` → `TechDraw::DrawViewDraft` → `TechDraw::DrawPage`), and the
//! document side of that is cheap: a page owns its template link and its view
//! list, a template names an SVG file, and a view links its source and carries a
//! projection direction. Nothing draws, projects or writes SVG here.
//!
//! The types are registered from the Part module's kernel bootstrap, beside
//! `Part::Sketch` and `PartDesign::Body` — the same "document shim for a
//! workbench API the tests touch" role.

use std::sync::Arc;

use ferrocad_core::geometry::Vector3;
use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{prop_status, Property};

/// `TechDraw::DrawPage`: a drawing page with a template and a view list.
pub const PAGE_TYPE: &str = "TechDraw::DrawPage";
/// `TechDraw::DrawSVGTemplate`: the page's SVG template (a file path).
pub const TEMPLATE_TYPE: &str = "TechDraw::DrawSVGTemplate";
/// `TechDraw::DrawViewDraft`: a Draft view on a page.
pub const DRAFT_VIEW_TYPE: &str = "TechDraw::DrawViewDraft";

/// The page's property names the generic `DocumentObject.addView` and
/// `OutList` read.
const VIEWS: &str = "Views";
const TEMPLATE: &str = "Template";

/// Register the TechDraw object types this module provides.
pub(crate) fn register_objects() {
    object_registry::register(PAGE_TYPE, Arc::new(DrawPage));
    object_registry::register(TEMPLATE_TYPE, Arc::new(SvgTemplate));
    object_registry::register(DRAFT_VIEW_TYPE, Arc::new(DraftView));
}

/// `TechDraw::DrawPage` — `(Template, Views)`.
struct DrawPage;

impl ObjectType for DrawPage {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                TEMPLATE.to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                VIEWS.to_string(),
                Property::LinkList(Vec::new()),
                prop_status::NONE,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Page".to_string(), format!("Page {name}")))
    }
}

/// `TechDraw::DrawSVGTemplate` — `(Template)`.
///
/// Upstream's `Template` is an `App::PropertyFile`; a plain string carries the
/// path with the same read/write behaviour and no file validation, which is all
/// a page needs before anything is drawn.
struct SvgTemplate;

impl ObjectType for SvgTemplate {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![(
            TEMPLATE.to_string(),
            Property::String(String::new()),
            prop_status::NONE,
        )]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Template".to_string(), format!("Template {name}")))
    }
}

/// `TechDraw::DrawViewDraft` — `(Source, Direction)`.
///
/// `Source` is a plain link: the Draft view is built from one object, and the
/// tests assign a single `Part::Prism`. `Direction` is the projection direction
/// (`(0, 0, 1)` here, upstream's default for a Draft view is free).
struct DraftView;

impl ObjectType for DraftView {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Source".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                "Direction".to_string(),
                Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
                prop_status::NONE,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("View".to_string(), format!("View {name}")))
    }
}

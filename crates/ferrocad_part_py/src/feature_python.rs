//! The Python-feature object types: `Part::FeaturePython` and
//! `Part::Part2DObjectPython`.
//!
//! Upstream these are `Part::Feature` subclasses whose rebuild is a Python
//! `execute` method on the object's `Proxy`
//! (`src/Mod/Part/App/PartFeaturePython.cpp`). The object still owns the same
//! `Shape`/`Placement` properties as a plain `Part::Feature`, so a proxy can
//! proxy can assign `obj.Shape = Part.makeBox(...)` exactly as upstream scripts do.
//!
//! Core is interpreter-free, so `execute` cannot call Python itself. It records a
//! request with [`Document::mark_python_pending`]; the `ferrocad` binding drains it
//! after the recompute (see `PyDocument::recompute`) and calls `proxy.execute(obj)`
//! with the document lock released, so the proxy can read and write its own
//! object normally. This is the same split as the rest of the document-object SPI:
//! core owns document state, the workbench owns the value types
//! (`Part::PropertyPartShape`), and the binding owns the interpreter.
//!
//! When no interpreter is available (a pure-Rust session, or before the `ferrocad`
//! module is initialised) the object is left **invalid** rather than silently
//! succeeding: a proxy-driven feature that never ran has no output.
//!
//! Attachment (`AttachmentSupport`/`MapMode`) is a later slice; these types are
//! registered with the same property set as `Part::Feature` for now.

use std::sync::Arc;

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{prop_status, Document, ObjectId, Property, Placement};
use ferrocad_part::{make_shape_property, ShapeProperty};

/// Upstream `Part::FeaturePython`.
pub const FEATURE_PYTHON_TYPE: &str = "Part::FeaturePython";
/// Upstream `Part::Part2DObjectPython`.
pub const PART2D_PYTHON_TYPE: &str = "Part::Part2DObjectPython";

/// `Shape` and `Placement`, matching [`ferrocad_part`]'s `Part::Feature`.
fn feature_properties() -> Vec<(String, Property, u32)> {
    vec![
        (
            "Shape".to_string(),
            make_shape_property(ShapeProperty::empty()),
            prop_status::NONE,
        ),
        (
            "Placement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
    ]
}

/// The shared behaviour of both types: they differ only in the base class a
/// proxy can detect via `isDerivedFrom`.
struct FeaturePython;

impl ObjectType for FeaturePython {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        feature_properties()
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Base".to_string(), format!("Part feature {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        if !object_registry::python_executor_available() {
            doc.mark_invalid_with(
                id,
                "no Python interpreter is available to run the feature's proxy",
            );
            return;
        }
        // The binding runs `proxy.execute(obj)` once the document lock is released.
        doc.mark_python_pending(id);
    }

    fn is_python_feature(&self) -> bool {
        true
    }
}

/// Register both Python-feature object types. Idempotent.
///
/// The caller (`Part` module initialisation) has already composed the kernel, so
/// `Part::PropertyPartShape` exists and a default `Shape` value can be built.
pub fn register() {
    object_registry::register(FEATURE_PYTHON_TYPE, Arc::new(FeaturePython));
    object_registry::register(PART2D_PYTHON_TYPE, Arc::new(FeaturePython));
}

//! # FerroCAD Part workbench (`ferrocad_part`)
//!
//! The first module built on the document-object SPI. It registers two things with
//! `ferrocad_core`:
//!
//! - **`Part::Feature`** — an object type with a `Shape` and a `Placement`, via
//!   [`ferrocad_core::object_registry::ObjectType`];
//! - **`Part::Fillet`** / **`Part::Chamfer`** — parametric dress-up features whose
//!   `execute` resolves their selected edge names through the backend, expands them to
//!   tangent chains, and writes the rebuilt shape to their output `Shape`;
//! - **`Part::PropertyPartShape`** — a property *value* type holding a
//!   [`ferrocad_geom::Shape`], via [`ferrocad_core::extension`].
//!
//! Part depends **only on the seam** ([`ferrocad_geom`]), never on a concrete kernel:
//! the application composes it with one — `ferrocad_part::register(Arc::new(OcctBackend::new()))`.
//! That is what lets the kernel be replaced (or swapped per edition) without touching
//! Part, and keeps `ferrocad_core` geometry-agnostic. See `docs/architecture/occt-integration.md`.
//!
//! # Building
//!
//! The library needs no kernel. Its tests use OCCT through a dev-dependency, so they
//! still need OCCT 7.8+; the crate is **not** a default member.

mod dress_up;
mod shape_property;

use std::sync::{Arc, OnceLock};

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{Document, ObjectId, Property, prop_status};
use ferrocad_geom::{GeometryBackend, Shape};
use ferrocad_types::Placement;

pub use dress_up::{CHAMFER_TYPE, FILLET_TYPE};
pub use shape_property::{make_shape_property, shape_of, ShapeProperty, TYPE_NAME};

/// The analytic half of Part (lines, segments, circles, arcs, ellipses, wires).
///
/// A consumer of `ferrocad_part` gets **both** halves from one crate — these
/// curves and [`ferrocad_geom::Shape`] — exactly as `import Part` does upstream.
/// The leaf itself is [`ferrocad_curves`]; this is a deliberate re-export, not a
/// second dependency edge from Part to the kernel.
pub use ferrocad_curves;

/// The kernel the Part workbench was composed with.
static BACKEND: OnceLock<Arc<dyn GeometryBackend>> = OnceLock::new();

/// The injected geometry backend.
///
/// # Panics
/// If [`register`] has not been called.
pub fn backend() -> Arc<dyn GeometryBackend> {
    Arc::clone(
        BACKEND
            .get()
            .expect("ferrocad_part::register was not called"),
    )
}

/// Compose Part with a geometry backend and register its types. Idempotent; an
/// application calls this once at startup with the kernel it wants.
pub fn register(backend: Arc<dyn GeometryBackend>) {
    let _ = BACKEND.set(backend);
    shape_property::register();
    object_registry::register("Part::Feature", Arc::new(Feature));
    object_registry::register(BOX_TYPE, Arc::new(BoxPrimitive));
    object_registry::register(FILLET_TYPE, Arc::new(dress_up::Fillet));
    object_registry::register(CHAMFER_TYPE, Arc::new(dress_up::Chamfer));
}

/// A convenience box from the injected backend (what a script's `Part.makeBox` calls).
pub fn make_box(length: f64, width: f64, height: f64) -> Option<Shape> {
    backend().make_box(length, width, height).ok()
}

/// `Part::Box`: a parametric box (`Length` × `Width` × `Height`).
///
/// The shape is rebuilt by `execute` (a document recompute), so the object behaves
/// like upstream's `Part::Box`: set the dimensions, recompute, read `Shape`.
pub const BOX_TYPE: &str = "Part::Box";

struct BoxPrimitive;

/// Read a float property, or the default when it is absent/another kind.
fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    doc.object(id)
        .and_then(|object| object.properties.get(name))
        .and_then(|value| match value {
            Property::Float(value) => Some(*value),
            Property::Integer(value) => Some(*value as f64),
            _ => None,
        })
        .unwrap_or(default)
}

impl ObjectType for BoxPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Length".to_string(),
                Property::Float(10.0),
                prop_status::NONE,
            ),
            (
                "Width".to_string(),
                Property::Float(10.0),
                prop_status::NONE,
            ),
            (
                "Height".to_string(),
                Property::Float(10.0),
                prop_status::NONE,
            ),
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            (
                "Shape".to_string(),
                make_shape_property(ShapeProperty::empty()),
                prop_status::NONE,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Box".to_string(), format!("Part box {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let (length, width, height, placement) = {
            let Some(object) = doc.object(id) else {
                return;
            };
            let placement = match object.properties.get("Placement") {
                Some(Property::Placement(placement)) => Some(*placement),
                _ => None,
            };
            (
                float_property(doc, id, "Length", 10.0),
                float_property(doc, id, "Width", 10.0),
                float_property(doc, id, "Height", 10.0),
                placement,
            )
        };

        if !(length > 0.0 && width > 0.0 && height > 0.0) {
            doc.mark_invalid(id);
            return;
        }

        let shape = match backend().make_box(length, width, height) {
            Ok(shape) => shape,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        let shape = match placement {
            Some(placement) => backend().place(&shape, &placement).unwrap_or(shape),
            None => shape,
        };
        doc.set_property_raw(id, "Shape", make_shape_property(ShapeProperty::new(shape)));
    }
}

/// `Part::Feature`: a shape plus a placement.
struct Feature;

impl ObjectType for Feature {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
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

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Base".to_string(), format!("Part feature {name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_core::{property_types, Document};
    use ferrocad_occt::OcctBackend;

    /// Compose Part with OCCT (as an application would).
    fn setup() {
        register(Arc::new(OcctBackend::new()));
    }

    #[test]
    fn registers_the_property_and_object_types() {
        setup();
        assert!(property_types::contains("Part::PropertyPartShape"));
        assert!(object_registry::is_registered("Part::Feature"));
        assert!(object_registry::is_registered(BOX_TYPE));
    }

    #[test]
    fn a_part_box_executes_into_a_box_shape() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Box", BOX_TYPE);

        // The primitive exposes the properties upstream's Part::Box has.
        let object = doc.object(id).unwrap();
        assert_eq!(
            object.properties.get("Length"),
            Some(&Property::Float(10.0))
        );
        assert!(object.properties.get("Width").is_some());
        assert!(object.properties.get("Height").is_some());
        assert!(
            shape_of(object.properties.get("Shape").unwrap())
                .unwrap()
                .shape()
                .is_none(),
            "the shape is empty until the first recompute"
        );

        doc.set_property(id, "Length", Property::Float(3.0)).unwrap();
        doc.recompute().unwrap();
        let object = doc.object(id).unwrap();
        let shape = shape_of(object.properties.get("Shape").unwrap())
            .unwrap()
            .shape()
            .expect("a recompute builds the box");
        assert!(backend().resolve(&shape, "Face6").is_some(), "a box has six faces");
    }

    #[test]
    fn a_non_positive_box_is_marked_invalid() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("BadBox", BOX_TYPE);
        doc.set_property(id, "Height", Property::Float(0.0)).unwrap();
        let _ = doc.recompute();
        assert!(doc.object(id).unwrap().invalid);
    }

    #[test]
    fn a_part_feature_gets_shape_and_placement() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("box", "Part::Feature");
        let object = doc.object(id).unwrap();
        assert!(object.properties.get("Shape").is_some());
        assert_eq!(
            object.properties.get("Placement"),
            Some(&Property::Placement(Placement::identity()))
        );
    }

    #[test]
    fn a_shape_round_trips_through_the_property() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("box", "Part::Feature");

        let shape = make_box(10.0, 10.0, 10.0).unwrap();
        doc.set_property(id, "Shape", make_shape_property(ShapeProperty::new(shape)))
            .unwrap();

        let stored = doc.object(id).unwrap().properties.get("Shape").unwrap();
        let recovered = shape_of(stored).unwrap().shape().unwrap();
        assert!(backend().resolve(&recovered, "Face1").is_some());
    }

    #[test]
    fn a_shape_persists_through_json() {
        setup();
        let shape = make_box(10.0, 10.0, 10.0).unwrap();
        let value = make_shape_property(ShapeProperty::new(shape));

        let json = serde_json::to_string(&value).unwrap();
        let back: Property = serde_json::from_str(&json).unwrap();
        let recovered = shape_of(&back).unwrap().shape().unwrap();
        assert!(backend().resolve(&recovered, "Face6").is_some());
    }

    #[test]
    fn an_empty_shape_property_round_trips() {
        setup();
        let value = make_shape_property(ShapeProperty::empty());
        let json = serde_json::to_string(&value).unwrap();
        let back: Property = serde_json::from_str(&json).unwrap();
        assert!(shape_of(&back).unwrap().shape().is_none());
    }
}

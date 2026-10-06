//! # FerroCAD Part workbench (`ferrocad_part`)
//!
//! The first module built on the document-object SPI. It registers two things with
//! `ferrocad_core`:
//!
//! - **`Part::Feature`** — an object type with a `Shape` and a `Placement`, via
//!   [`ferrocad_core::object_registry::ObjectType`];
//! - **`Part::PropertyPartShape`** — a property *value* type holding a
//!   [`ferrocad_geom::Shape`], via [`ferrocad_core::extension`].
//!
//! Part depends **only on the seam** ([`ferrocad_geom`]), never on a concrete kernel:
//! the application composes it with one — `ferrocad_part::register(Arc::new(OcctBackend::new()))`.
//! That is what lets the kernel be replaced (or swapped per edition) without touching
//! Part, and keeps `ferrocad_core` geometry-agnostic. See `docs/occt-integration.md`.
//!
//! # Building
//!
//! The library needs no kernel. Its tests use OCCT through a dev-dependency, so they
//! still need OCCT 7.8+; the crate is **not** a default member.

mod shape_property;

use std::sync::{Arc, OnceLock};

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{prop_status, Property};
use ferrocad_geom::{GeometryBackend, Shape};
use ferrocad_types::Placement;

pub use shape_property::{make_shape_property, shape_of, ShapeProperty};

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
}

/// A convenience box from the injected backend (what a script's `Part.makeBox` calls).
pub fn make_box(length: f64, width: f64, height: f64) -> Option<Shape> {
    backend().make_box(length, width, height).ok()
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

//! `Part::DatumPlane`: a persistent, selectable plane in the document.
//!
//! Unlike the ephemeral workplane (viewport state), a datum plane is a document
//! object with a tree row, a `Visibility`, an undo step per edit and a `.FCStd`
//! round trip. It is also a valid `Set WP` target: its `Shape` is one planar face
//! in the object's local XY, centred on the placement origin, which the kernel
//! names `Face1`.
//!
//! # Representation
//!
//! [`Placement`] is the **single source of truth** for the frame: its base is the
//! plane's origin and its rotation maps local `+X`/`+Y`/`+Z` onto the workplane's
//! `u`/`v`/`n`. That is what makes the standard translate/rotate gizmo work
//! without a bespoke handle, and what a script sets from a frame. `Shape` is
//! derived on recompute from `Placement` + `Size`, so an edit is one property
//! write and one undo step.
//!
//! The Python surface builds the rotation from the three axes with
//! `FreeCAD.Rotation(u, v, n)`; no vector properties (`Origin`/`U`/`V`) are
//! needed, so there is exactly one authority.

use ferrocad_core::object_registry::ObjectType;
use ferrocad_core::{Document, ObjectId, Placement, Property, Quantity, Unit, prop_status};
use ferrocad_geom::Shape;
use ferrocad_types::Vector3;

use crate::shape_property::{ShapeProperty, make_shape_property};

/// The registered object-type id (`Part::DatumPlane`).
pub const DATUM_PLANE_TYPE: &str = "Part::DatumPlane";

/// The default side length of the square, in millimetres.
pub const DEFAULT_SIZE: f64 = 100.0;

/// `Part::DatumPlane`: a square planar face derived from a placement and a size.
pub(crate) struct DatumPlane;

/// Read the plane's side from `Size`, falling back to the constructor default.
fn size_mm(doc: &Document, id: ObjectId) -> f64 {
    doc.object(id)
        .and_then(|object| object.properties.get("Size"))
        .and_then(|value| match value {
            Property::Quantity(quantity) => Some(quantity.value_mm()),
            Property::Float(value) => Some(*value),
            Property::Integer(value) => Some(*value as f64),
            _ => None,
        })
        .unwrap_or(DEFAULT_SIZE)
}

/// The object's placement, or the identity when it is missing/mistyped.
fn placement_of(doc: &Document, id: ObjectId) -> Placement {
    match doc
        .object(id)
        .and_then(|object| object.properties.get("Placement"))
    {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

/// A square face centred on the local origin, in the local XY plane.
///
/// The corners are in `u` then `v` order; `polygon` closes the wire, and
/// `make_face` turns it into one planar face.
fn square_face(size: f64) -> Option<Shape> {
    let half = size / 2.0;
    let points = [
        Vector3::new(-half, -half, 0.0),
        Vector3::new(half, -half, 0.0),
        Vector3::new(half, half, 0.0),
        Vector3::new(-half, half, 0.0),
        Vector3::new(-half, -half, 0.0),
    ];
    let curves = ferrocad_curves::polygon(&points, true).ok()?;

    let mut edges = Vec::with_capacity(curves.len());
    for curve in &curves {
        edges.push(crate::backend().make_edge(curve).ok()?);
    }
    let wire = crate::backend().make_wire(&edges).ok()?;
    crate::backend().make_face(&[wire]).ok()
}

impl ObjectType for DatumPlane {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            (
                "Size".to_string(),
                Property::Quantity(Quantity::new(DEFAULT_SIZE, Unit::Millimeter)),
                prop_status::NONE,
            ),
            // Derived on every recompute; a script reads it, never writes it.
            (
                "Shape".to_string(),
                make_shape_property(ShapeProperty::empty()),
                prop_status::OUTPUT | prop_status::READONLY,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Datum".to_string(), format!("Datum plane {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let placement = placement_of(doc, id);
        let size = size_mm(doc, id);
        if !(size > 0.0) {
            doc.mark_invalid(id);
            return;
        }
        let Some(local) = square_face(size) else {
            doc.mark_invalid(id);
            return;
        };
        // Bake the placement into the geometry *and* record it on the property,
        // so `Shape.Placement` matches the object and the display hook does not
        // apply the placement a second time.
        let shape = match crate::backend().place(&local, &placement) {
            Ok(shape) => shape,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        doc.set_property_raw(
            id,
            "Shape",
            make_shape_property(ShapeProperty::with_placement(shape, placement)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_core::object_registry;
    use ferrocad_occt::OcctBackend;
    use ferrocad_types::Rotation;
    use std::sync::Arc;

    fn setup() {
        crate::register(Arc::new(OcctBackend::new()));
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ferrocad-part-datum-{}-{name}",
            std::process::id()
        ))
    }

    fn shape_of(doc: &Document, id: ObjectId) -> Shape {
        let value = doc
            .object(id)
            .and_then(|object| object.properties.get("Shape"))
            .expect("the Shape property");
        crate::shape_of(value)
            .expect("a shape property")
            .shape()
            .expect("an executed shape")
            .clone()
    }

    #[test]
    fn the_type_is_registered_with_the_expected_properties() {
        setup();
        assert!(object_registry::is_registered(DATUM_PLANE_TYPE));

        let mut doc = Document::new();
        let id = doc.add_object("Plane", DATUM_PLANE_TYPE);
        let object = doc.object(id).unwrap();
        assert!(matches!(
            object.properties.get("Placement"),
            Some(Property::Placement(_))
        ));
        assert!(matches!(
            object.properties.get("Size"),
            Some(Property::Quantity(quantity)) if (quantity.value_mm() - DEFAULT_SIZE).abs() < 1e-9
        ));
        assert_eq!(object.properties.get("Size").unwrap().type_name(), "App::PropertyLength");
        assert!(matches!(
            object.properties.get("Shape"),
            Some(Property::Extension(_))
        ));
    }

    #[test]
    fn a_datum_plane_executes_into_one_square_face() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Plane", DATUM_PLANE_TYPE);
        doc.set_property(
            id,
            "Placement",
            Property::Placement(Placement::new(
                Vector3::new(10.0, 20.0, 30.0),
                Rotation::identity(),
            )),
        )
        .unwrap();
        doc.set_property(
            id,
            "Size",
            Property::Quantity(Quantity::new(100.0, Unit::Millimeter)),
        )
        .unwrap();
        doc.recompute().unwrap();

        let shape = shape_of(&doc, id);
        let mass = crate::backend().mass_properties(&shape).expect("mass properties");
        assert!((mass.area - 10_000.0).abs() < 1e-6, "area {}", mass.area);

        // Centred on the placement base: ±50 mm around (10, 20), on z = 30.
        let (min, max) = crate::backend().bounds(&shape).expect("bounds");
        assert!((min.x - -40.0).abs() < 1e-6, "min {min:?}");
        assert!((min.y - -30.0).abs() < 1e-6, "min {min:?}");
        assert!((min.z - 30.0).abs() < 1e-6, "min {min:?}");
        assert!((max.x - 60.0).abs() < 1e-6, "max {max:?}");
        assert!((max.y - 70.0).abs() < 1e-6, "max {max:?}");
        assert!((max.z - 30.0).abs() < 1e-6, "max {max:?}");

        // The kernel names the single face `Face1`, which is what `Set WP` binds.
        assert!(crate::backend().resolve(&shape, "Face1").is_some());
        assert!(crate::backend().resolve(&shape, "Face2").is_none());
    }

    #[test]
    fn the_shape_follows_the_placement_and_the_size() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Plane", DATUM_PLANE_TYPE);

        // A quarter turn about X maps the local XY face onto the world XZ plane.
        let rotation = Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), std::f64::consts::FRAC_PI_2);
        doc.set_property(
            id,
            "Placement",
            Property::Placement(Placement::new(Vector3::zero(), rotation)),
        )
        .unwrap();
        doc.recompute().unwrap();
        let shape = shape_of(&doc, id);
        let (min, max) = crate::backend().bounds(&shape).unwrap();
        assert!(min.y.abs() < 1e-6 && max.y.abs() < 1e-6, "the face lies in XZ: {min:?} {max:?}");
        assert!((min.z - -50.0).abs() < 1e-6, "{min:?} {max:?}");
        assert!((max.z - 50.0).abs() < 1e-6, "{min:?} {max:?}");

        // A size edit rebuilds the same face at the new side, in one property write.
        doc.set_property(
            id,
            "Size",
            Property::Quantity(Quantity::new(50.0, Unit::Millimeter)),
        )
        .unwrap();
        assert!(!doc.object(id).unwrap().invalid, "the edit is pending a recompute");
        doc.recompute().unwrap();
        let shape = shape_of(&doc, id);
        let mass = crate::backend().mass_properties(&shape).unwrap();
        assert!((mass.area - 2_500.0).abs() < 1e-6, "area {}", mass.area);
    }

    #[test]
    fn a_placement_and_size_edit_is_one_undo_step_each() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Plane", DATUM_PLANE_TYPE);

        doc.open_transaction_named("Set size");
        doc.set_property(
            id,
            "Size",
            Property::Quantity(Quantity::new(80.0, Unit::Millimeter)),
        )
        .unwrap();
        doc.commit_transaction();
        doc.recompute().unwrap();
        assert_eq!(doc.undo_names(), vec!["Set size".to_string()]);

        doc.open_transaction_named("Set placement");
        doc.set_property(
            id,
            "Placement",
            Property::Placement(Placement::new(
                Vector3::new(5.0, 0.0, 0.0),
                Rotation::identity(),
            )),
        )
        .unwrap();
        doc.commit_transaction();
        doc.recompute().unwrap();
        assert_eq!(doc.undo_names().len(), 2, "one step per edit");

        assert!(doc.undo());
        doc.recompute().unwrap();
        let shape = shape_of(&doc, id);
        let (min, max) = crate::backend().bounds(&shape).unwrap();
        assert!(
            ((min.x + max.x) / 2.0).abs() < 1e-6,
            "undo restores the base: {min:?} {max:?}"
        );
    }

    #[test]
    fn a_datum_plane_survives_an_fcstd_round_trip() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Datum", DATUM_PLANE_TYPE);
        let placement = Placement::new(
            Vector3::new(10.0, 20.0, 30.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        doc.set_property(id, "Placement", Property::Placement(placement))
            .unwrap();
        doc.set_property(
            id,
            "Size",
            Property::Quantity(Quantity::new(50.0, Unit::Millimeter)),
        )
        .unwrap();
        doc.recompute().unwrap();

        let saved = doc.to_saved("DatumRoundTrip");
        let path = temp_path("round-trip.FCStd");
        ferrocad_core::fcstd_write::write_fcstd(&saved, path.to_str().unwrap())
            .expect("write .FCStd");
        let read = ferrocad_core::fcstd::read_fcstd(path.to_str().unwrap())
            .expect("read back .FCStd");
        let _ = std::fs::remove_file(&path);

        let object = read
            .objects
            .iter()
            .find(|object| object.name == "Datum")
            .expect("the datum plane is restored");
        assert_eq!(object.type_id, DATUM_PLANE_TYPE);
        match object.properties.get("Placement") {
            Some(Property::Placement(restored)) => {
                assert!(restored.is_same(&placement, 1e-9), "placement {restored:?}");
            }
            other => panic!("Placement restored as {other:?}"),
        }
        match object.properties.get("Size") {
            Some(Property::Quantity(size)) => {
                assert!((size.value_mm() - 50.0).abs() < 1e-9, "size {}", size.value_mm());
            }
            other => panic!("Size restored as {other:?}"),
        }
        let shape = crate::shape_of(object.properties.get("Shape").unwrap())
            .expect("a shape property")
            .shape()
            .expect("the restored shape");
        let mass = crate::backend().mass_properties(&shape).unwrap();
        assert!((mass.area - 2_500.0).abs() < 1e-6, "area {}", mass.area);
    }

    #[test]
    fn a_non_positive_size_marks_the_object_invalid() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Plane", DATUM_PLANE_TYPE);
        doc.set_property(
            id,
            "Size",
            Property::Quantity(Quantity::new(0.0, Unit::Millimeter)),
        )
        .unwrap();
        let _ = doc.recompute();
        assert!(doc.object(id).unwrap().invalid);
    }
}

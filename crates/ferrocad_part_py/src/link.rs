//! The `App::Link` base: shared link properties and the Part-side execute path.
//!
//! Upstream `App::Link` is a real object type with a property base
//! (`Placement`, `LinkPlacement`, `Scale`, `LinkTransform`, `LinkedObject`,
//! `ShowElement`, the element list, …) whose `execute` copies or transforms the
//! linked object's shape. `Part::LinkArray*` derive from it.
//!
//! FerroCAD keeps the geometry-free defaults in `ferrocad_core::typeregistry`
//! so an `App::Link` can be created without a kernel, and registers the type
//! here, where the geometry backend exists. Core's `typeregistry` module is
//! private to that crate, so [`link_properties`] mirrors its `App::Link` entry
//! and adds the shape-valued `Shape`. That helper is the one property list the
//! link and the `Part::LinkArray*` types share.
//!
//! This is slice L1/L2 of `docs/tasks/links.md`. Elements (`ShowElement`,
//! `ElementList` contents), copy-on-change and lineage-backed sub-element
//! resolution are deliberately absent: the properties exist and round-trip, but
//! `execute` always produces the whole linked shape.

use std::sync::Arc;

use ferrocad_core::geometry::{Matrix4, Placement, Vector3};
use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{prop_status, Document, ObjectId, Property};
use ferrocad_geom::Shape;
use ferrocad_part::{make_shape_property, shape_of, ShapeProperty};

/// The `App::Link` type id. `Part::LinkArray*` are treated as its peers by the
/// query layer's link table.
pub(crate) const LINK_TYPE: &str = "App::Link";

/// Status flags for the generated properties (`ElementCount`, `ElementList`,
/// `PlacementList`, `Shape`): computed output, never restored, never recomputed
/// from the GUI.
pub(crate) const HIDDEN_OUTPUT: u32 =
    prop_status::HIDDEN | prop_status::OUTPUT | prop_status::NORECOMPUTE | prop_status::READONLY;

/// Register the `App::Link` object type. Idempotent.
pub(crate) fn register() {
    object_registry::register(LINK_TYPE, Arc::new(LinkObject));
}

// ---------------------------------------------------------------------------
// Shared property base
// ---------------------------------------------------------------------------

/// The property base shared by `App::Link` and (upstream) `Part::LinkArray*`.
///
/// This mirrors core's private `typeregistry` entry for the `App::Link` type id,
/// which a document without `Part` uses. Keeping the list in one place for both
/// the link and the arrays is what this helper is for. The shape-valued `Shape`
/// is added here because core cannot construct a `Part::PropertyPartShape`.
pub(crate) fn link_properties() -> Vec<(String, Property, u32)> {
    let mut properties: Vec<(String, Property, u32)> = vec![
        (
            "Placement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        (
            "LinkPlacement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        ("Scale".to_string(), Property::Float(1.0), prop_status::NONE),
        (
            "ScaleVector".to_string(),
            Property::Vector(Vector3::new(1.0, 1.0, 1.0)),
            prop_status::NONE,
        ),
        (
            "LinkTransform".to_string(),
            Property::Bool(false),
            prop_status::NONE,
        ),
        (
            "LinkMode".to_string(),
            Property::Enumeration(
                ["AutoDelete", "KeepLink", "Translate"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                0,
            ),
            prop_status::NONE,
        ),
        (
            "LinkCopyOnChange".to_string(),
            Property::Enumeration(
                ["Disabled", "Enabled", "Owned", "Tracking"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                0,
            ),
            prop_status::NONE,
        ),
        (
            "LinkedObject".to_string(),
            Property::Link(String::new()),
            prop_status::NONE,
        ),
        ("ShowElement".to_string(), Property::Bool(true), prop_status::NONE),
        (
            "ElementCount".to_string(),
            Property::Integer(0),
            HIDDEN_OUTPUT,
        ),
        (
            "PlacementList".to_string(),
            Property::PlacementList(Vec::new()),
            HIDDEN_OUTPUT,
        ),
        (
            "ElementList".to_string(),
            Property::LinkList(Vec::new()),
            HIDDEN_OUTPUT,
        ),
    ];
    properties.push((
        "Shape".to_string(),
        make_shape_property(ShapeProperty::empty()),
        HIDDEN_OUTPUT,
    ));
    properties
}

// ---------------------------------------------------------------------------
// Property access
// ---------------------------------------------------------------------------

fn property<'a>(doc: &'a Document, id: ObjectId, name: &str) -> Option<&'a Property> {
    doc.object(id)?.properties.get(name)
}

fn placement_property(doc: &Document, id: ObjectId, name: &str) -> Placement {
    match property(doc, id, name) {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    match property(doc, id, name) {
        Some(Property::Float(value)) => *value,
        Some(Property::Integer(value)) => *value as f64,
        _ => default,
    }
}

fn bool_property(doc: &Document, id: ObjectId, name: &str, default: bool) -> bool {
    match property(doc, id, name) {
        Some(Property::Bool(value)) => *value,
        _ => default,
    }
}

fn vector_property(doc: &Document, id: ObjectId, name: &str, default: Vector3) -> Vector3 {
    match property(doc, id, name) {
        Some(Property::Vector(value)) => *value,
        _ => default,
    }
}

/// The target object name stored in `LinkedObject`, which may be a plain link or
/// a link-with-sub-elements. A `LinkSub`'s sub-names are ignored for now
/// (sub-elements still resolve positionally, and properly only with lineage, L5).
fn link_target(doc: &Document, id: ObjectId) -> Option<String> {
    match property(doc, id, "LinkedObject") {
        Some(Property::Link(name)) if !name.is_empty() => Some(name.clone()),
        Some(Property::LinkSub(name, _)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// The transform an `App::Link` applies to its target's shape.
///
/// - `LinkTransform = false` (default): the link's own `Placement`.
/// - `LinkTransform = true`: `LinkPlacement`, the link's global placement.
///
/// `Scale` multiplies `ScaleVector` when the vector is uniform. A non-uniform
/// `ScaleVector` needs a general affine transform, which the kernel transform
/// path (an OCCT `gp_Trsf`, i.e. a similarity) cannot represent; it is left for
/// L4 and only `Scale` is applied today.
fn link_matrix(doc: &Document, id: ObjectId) -> Matrix4 {
    let placement = if bool_property(doc, id, "LinkTransform", false) {
        placement_property(doc, id, "LinkPlacement")
    } else {
        placement_property(doc, id, "Placement")
    };
    let mut matrix = placement.to_matrix();

    let scale = float_property(doc, id, "Scale", 1.0);
    let vector = vector_property(doc, id, "ScaleVector", Vector3::new(1.0, 1.0, 1.0));
    let uniform = (vector.x - vector.y).abs() <= 1e-12 && (vector.x - vector.z).abs() <= 1e-12;
    let factor = if uniform { scale * vector.x } else { scale };
    if (factor - 1.0).abs() > 1e-12 {
        matrix = matrix.mul(&uniform_scale_matrix(factor));
    }
    matrix
}

fn uniform_scale_matrix(factor: f64) -> Matrix4 {
    let mut values = Matrix4::identity().m;
    values[0] = factor;
    values[5] = factor;
    values[10] = factor;
    Matrix4::from_values(values)
}

fn write_shape(doc: &mut Document, id: ObjectId, shape: Option<Shape>) {
    let property = match shape {
        Some(shape) => make_shape_property(ShapeProperty::new(shape)),
        None => make_shape_property(ShapeProperty::empty()),
    };
    doc.set_property_raw(id, "Shape", property);
}

// ---------------------------------------------------------------------------
// The object type
// ---------------------------------------------------------------------------

struct LinkObject;

impl ObjectType for LinkObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        link_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        // No target, a dangling target name, or a target whose `Shape` is unset
        // all mean "nothing to show", not a failure: the link stays valid with an
        // empty shape (upstream's `ShowElement`/collapsed-link behaviour).
        let Some(target_name) = link_target(doc, id) else {
            write_shape(doc, id, None);
            return;
        };
        let Some(target_id) = doc.get_by_name(&target_name) else {
            write_shape(doc, id, None);
            return;
        };
        let Some(source) = doc
            .object(target_id)
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            write_shape(doc, id, None);
            return;
        };

        let matrix = link_matrix(doc, id);
        let shape = if matrix.is_unity(1e-12) {
            source
        } else {
            match ferrocad_part::backend().transform(&source, &matrix) {
                Ok(shape) => shape,
                Err(_) => {
                    // The kernel refused a transform we asked for: a real error.
                    doc.mark_invalid(id);
                    return;
                }
            }
        };
        write_shape(doc, id, Some(shape));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_properties_cover_the_core_defaults_and_shape() {
        let properties = link_properties();
        let names: Vec<&str> = properties.iter().map(|(name, _, _)| name.as_str()).collect();
        for expected in [
            "LinkedObject",
            "Placement",
            "LinkPlacement",
            "Scale",
            "ScaleVector",
            "LinkTransform",
            "LinkMode",
            "LinkCopyOnChange",
            "ShowElement",
            "ElementCount",
            "PlacementList",
            "ElementList",
            "Shape",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn uniform_scale_is_applied_after_the_placement() {
        let matrix = Placement {
            base: Vector3::new(2.0, 0.0, 0.0),
            rotation: ferrocad_core::Rotation::identity(),
        }
        .to_matrix()
        .mul(&uniform_scale_matrix(2.0));
        let moved = matrix.transform(&Vector3::new(1.0, 1.0, 1.0));
        assert!((moved.x - 4.0).abs() < 1e-12);
        assert!((moved.y - 2.0).abs() < 1e-12);
        assert!((moved.z - 2.0).abs() < 1e-12);
    }
}

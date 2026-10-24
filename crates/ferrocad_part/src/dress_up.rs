//! The dress-up features: `Part::Fillet` and `Part::Chamfer`.
//!
//! Each is a parametric document object over a base object's shape:
//!
//! ```text
//! Box (Part::Feature)  ──Base──▶  Fillet (Part::Fillet)
//!    Shape                          Shape (output)
//!         ▲                              │
//!         └──────── GeometryBackend ─────┘  resolve(edge) → fillet → Shape
//! ```
//!
//! `Base` is a `PropertyLinkSub` — the object **name** plus the selected sub-element
//! names — so it is also the recompute dependency: changing the base (or its shape)
//! re-executes the feature. `execute` resolves each name through the base shape's
//! lineage first and the backend's positional names second (see
//! [`resolve_element_in`]), expands it to its tangent chain when `Propagate` is set,
//! runs the kernel operation, and writes the result to the output `Shape` property.
//!
//! A rebuild that cannot run (no base, no edges, a name that no longer resolves, a
//! kernel error) marks the object invalid and leaves `Shape` empty, the way FreeCAD
//! clears a feature that failed to recompute. A name that the base shape's lineage no
//! longer justifies fails here rather than moving to whatever edge now holds that
//! ordinal. The viewport sees that as a null shape; `DocumentObject::invalid` reports
//! it to a host.
//!
//! # Property surface
//!
//! | Feature | Property | Meaning |
//! | --- | --- | --- |
//! | both | `Base` | `LinkSub(object, [names])`; `names` is the selection |
//! | both | `Edges` | `StringList` selection override (wins when non-empty) |
//! | both | `Propagate` | expand each edge to its tangent chain (default `true`) |
//! | both | `Shape` | the rebuilt result (an output property) |
//! | `Part::Fillet` | `Radius` | the fillet radius (default `1.0`) |
//! | `Part::Chamfer` | `Distance` | the first chamfer distance (default `1.0`) |
//! | `Part::Chamfer` | `Distance2` | the second distance; `0` means symmetric |
//!
//! The `Edges` override differs from upstream FreeCAD, whose `Edges` carries
//! `(index, radius1, radius2)` triples. Here the per-edge radii live in `Radius` /
//! `Distance`, and `Edges` is a plain name list so a property editor or the F1 arc
//! scrubber can rewrite the selection without touching `Base`.
//!
//! # Example
//!
//! ```ignore
//! use ferrocad_core::{Document, Property};
//! use ferrocad_part::{make_shape_property, ShapeProperty};
//!
//! let mut doc = Document::new();
//! let box_id = doc.add_object("Box", "Part::Feature");
//! doc.set_property(box_id, "Shape", make_shape_property(ShapeProperty::new(shape)))?;
//!
//! let fillet = doc.add_object("Fillet", "Part::Fillet");
//! doc.set_property(fillet, "Base", Property::LinkSub("Box".into(), vec!["Edge1".into()]))?;
//! doc.set_property(fillet, "Radius", Property::Float(2.0))?;
//! doc.recompute()?; // the output Shape is the filleted box
//! ```

use ferrocad_core::object_registry::ObjectType;
use ferrocad_core::prop_status;
use ferrocad_core::{Document, ObjectId, Property};
use ferrocad_geom::{ElementRef, Lineage, Shape};

use crate::shape_property::{
    ResolveElementError, ShapeProperty, make_shape_property, positional_name, resolve_element_in,
    shape_of,
};

/// `Part::Fillet`'s registered type id.
pub const FILLET_TYPE: &str = "Part::Fillet";
/// `Part::Chamfer`'s registered type id.
pub const CHAMFER_TYPE: &str = "Part::Chamfer";

/// The linked object plus its selected sub-elements.
const BASE: &str = "Base";
/// A plain-name override for the selection in [`BASE`].
const EDGES: &str = "Edges";
/// Expand each selected edge to its tangent-continuous chain.
const PROPAGATE: &str = "Propagate";
/// The rebuilt output.
const SHAPE: &str = "Shape";

/// `Part::Fillet`: round the selected edges of a base shape.
pub struct Fillet;

/// `Part::Chamfer`: bevel the selected edges of a base shape.
pub struct Chamfer;

impl ObjectType for Fillet {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (BASE.to_string(), empty_link_sub(), prop_status::NONE),
            (
                EDGES.to_string(),
                Property::StringList(Vec::new()),
                prop_status::NONE,
            ),
            (
                "Radius".to_string(),
                Property::Float(1.0),
                prop_status::NONE,
            ),
            (
                PROPAGATE.to_string(),
                Property::Bool(true),
                prop_status::NONE,
            ),
            (SHAPE.to_string(), output_shape(), prop_status::OUTPUT),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Base".to_string(), format!("Part fillet {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = rebuild(doc, id).and_then(|request| {
            let radius = float_property(doc, id, "Radius").unwrap_or(1.0);
            if !(radius > 0.0) {
                return Err("the fillet radius must be positive".to_string());
            }
            crate::backend()
                .fillet(&request.source, &request.edges, radius)
                .map(|result| result.shape)
                .map_err(|error| error.to_string())
        });
        store(doc, id, result);
    }
}

impl ObjectType for Chamfer {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (BASE.to_string(), empty_link_sub(), prop_status::NONE),
            (
                EDGES.to_string(),
                Property::StringList(Vec::new()),
                prop_status::NONE,
            ),
            (
                "Distance".to_string(),
                Property::Float(1.0),
                prop_status::NONE,
            ),
            (
                "Distance2".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                PROPAGATE.to_string(),
                Property::Bool(true),
                prop_status::NONE,
            ),
            (SHAPE.to_string(), output_shape(), prop_status::OUTPUT),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Base".to_string(), format!("Part chamfer {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = rebuild(doc, id).and_then(|request| {
            let distance = float_property(doc, id, "Distance").unwrap_or(1.0);
            if !(distance > 0.0) {
                return Err("the chamfer distance must be positive".to_string());
            }
            // A second distance of zero (the default) means a symmetric chamfer.
            let second = float_property(doc, id, "Distance2").filter(|second| *second > 0.0);
            crate::backend()
                .chamfer(&request.source, &request.edges, distance, second)
                .map(|result| result.shape)
                .map_err(|error| error.to_string())
        });
        store(doc, id, result);
    }
}

/// The base shape and the resolved edges a dress-up feature applies to.
struct Request {
    source: Shape,
    edges: Vec<ElementRef>,
}

/// Read the inputs and resolve the edge names through the base shape's lineage.
fn rebuild(doc: &Document, id: ObjectId) -> Result<Request, String> {
    let (object, linked) = base(doc, id)?;
    if doc.object(id).is_some_and(|this| this.name == object) {
        return Err("a dress-up feature cannot use itself as its base".to_string());
    }
    let names = selection(doc, id, linked);
    if names.is_empty() {
        return Err("the feature selects no edges".to_string());
    }
    let (source, lineage) = source_shape(doc, &object)?;
    let propagate = !matches!(property(doc, id, PROPAGATE), Some(Property::Bool(false)));

    let backend = crate::backend();
    let mut edges: Vec<ElementRef> = Vec::new();
    for name in names {
        // An earlier selection's chain may already cover this edge; resolving and
        // walking it again would only repeat the enumeration.
        if edges.iter().any(|existing| existing.name == name) {
            continue;
        }
        let sub = resolve_element_in(&source, &lineage, &name).map_err(|error| match error {
            ResolveElementError::Retired => format!(
                "{name} is not in the base shape's lineage (a retired or unknown name)"
            ),
            ResolveElementError::NoShape | ResolveElementError::Missing => {
                format!("{name} does not exist on the base shape")
            }
        })?;
        // The kernel and the chain walker speak the shape's own positional names, so a
        // sub-shape found through the lineage is relabelled with the name the source
        // shape gives it. Transient: the stored name is what resolved it, and nothing
        // here is persisted. Without that label a chain cannot be walked by name — and
        // keeping the stored name would be worse, since `Edge1` would then match the
        // *ordinal* edge in the walker, the very retarget this path exists to avoid.
        let local = positional_name(&source, &name, &sub);
        let walkable = propagate && local.is_some();
        let edge = ElementRef {
            shape: sub,
            name: local.unwrap_or(name),
        };
        // A dress-up applies to the whole smooth chain, not to one segment of it.
        let candidates = if walkable {
            ferrocad_geom::tangent_chain(backend.as_ref(), &source, &edge)
        } else {
            vec![edge]
        };
        for candidate in candidates {
            if !edges.iter().any(|existing| existing.name == candidate.name) {
                edges.push(candidate);
            }
        }
    }

    Ok(Request { source, edges })
}

/// The `Base` link: the object name and its sub-element names.
fn base(doc: &Document, id: ObjectId) -> Result<(String, Vec<String>), String> {
    match property(doc, id, BASE) {
        Some(Property::LinkSub(object, elements)) if !object.is_empty() => {
            Ok((object.clone(), elements.clone()))
        }
        Some(Property::LinkSub(_, _)) => Err("the feature has no base object".to_string()),
        _ => Err("the feature has no Base link".to_string()),
    }
}

/// The selected edge names: `Edges` when set, otherwise `Base`'s sub-values.
fn selection(doc: &Document, id: ObjectId, linked: Vec<String>) -> Vec<String> {
    let listed = match property(doc, id, EDGES) {
        Some(Property::StringList(names)) => names.clone(),
        _ => Vec::new(),
    };
    let names = if listed.is_empty() { linked } else { listed };
    let mut unique = Vec::with_capacity(names.len());
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

/// The shape a linked object currently holds, plus the lineage stored beside it.
fn source_shape(doc: &Document, name: &str) -> Result<(Shape, Lineage), String> {
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    let property = doc
        .object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .ok_or_else(|| format!("object {name} has no shape"))?;
    let shape = property
        .shape()
        .ok_or_else(|| format!("object {name} has no shape"))?;
    Ok((shape, property.lineage().clone()))
}

/// Store a rebuild's outcome: the shape, or an empty output plus the invalid flag.
fn store(doc: &mut Document, id: ObjectId, result: Result<Shape, String>) {
    match result {
        Ok(shape) => {
            doc.set_property_raw(id, SHAPE, make_shape_property(ShapeProperty::new(shape)));
        }
        Err(_) => {
            doc.mark_invalid(id);
            doc.set_property_raw(id, SHAPE, make_shape_property(ShapeProperty::empty()));
        }
    }
}

fn property<'a>(doc: &'a Document, id: ObjectId, name: &str) -> Option<&'a Property> {
    doc.object(id)?.properties.get(name)
}

fn float_property(doc: &Document, id: ObjectId, name: &str) -> Option<f64> {
    match property(doc, id, name) {
        Some(Property::Float(value)) => Some(*value),
        _ => None,
    }
}

fn empty_link_sub() -> Property {
    Property::LinkSub(String::new(), Vec::new())
}

fn output_shape() -> Property {
    make_shape_property(ShapeProperty::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_core::property_types;
    use ferrocad_geom::{ElementKind, TessellationOptions};
    use ferrocad_occt::OcctBackend;
    use std::sync::Arc;

    fn setup() {
        crate::register(Arc::new(OcctBackend::new()));
    }

    /// A `Part::Feature` holding a fresh box.
    fn box_feature(doc: &mut Document, size: f64) -> ObjectId {
        let id = doc.add_object("Box", "Part::Feature");
        let shape = crate::make_box(size, size, size).unwrap();
        doc.set_property(id, SHAPE, make_shape_property(ShapeProperty::new(shape)))
            .unwrap();
        id
    }

    /// A lineage that binds `name` to `target`, as an operation that preserved the
    /// name across a rebuild would report it.
    fn lineage_binding(name: &str, target: &ElementRef) -> Lineage {
        let mut history = ferrocad_geom::History::default();
        history.modified.push((
            ElementRef {
                shape: target.shape.clone(),
                name: name.to_string(),
            },
            target.clone(),
        ));
        Lineage::from_history(&history, ferrocad_geom::MapVersion::default())
    }

    fn direct_fillet(shape: &Shape, edge: &ElementRef, radius: f64) -> Shape {
        crate::backend()
            .fillet(shape, std::slice::from_ref(edge), radius)
            .unwrap()
            .shape
    }

    fn stored_shape(doc: &Document, id: ObjectId) -> Option<Shape> {
        doc.object(id)?
            .properties
            .get(SHAPE)
            .and_then(shape_of)
            .and_then(|property| property.shape())
    }

    fn face_count(shape: &Shape) -> usize {
        crate::backend().elements(shape, ElementKind::Face).len()
    }

    /// The largest world X of the shape's tessellation.
    fn max_x(shape: &Shape) -> f32 {
        let mesh = crate::backend()
            .tessellate(shape, &TessellationOptions::absolute(1.0))
            .unwrap();
        mesh.positions
            .iter()
            .map(|position| position[0])
            .fold(f32::MIN, f32::max)
    }

    /// A coarse fingerprint of the tessellated geometry, to compare two rebuilds.
    fn fingerprint(shape: &Shape) -> f32 {
        let mesh = crate::backend()
            .tessellate(shape, &TessellationOptions::absolute(0.2))
            .unwrap();
        mesh.positions
            .iter()
            .map(|position| position[0] + position[1] + position[2])
            .sum()
    }

    fn add_fillet(doc: &mut Document, base: ObjectId, edges: &[&str]) -> ObjectId {
        let id = doc.add_object("Fillet", FILLET_TYPE);
        let base_name = doc.object(base).unwrap().name.clone();
        doc.set_property(
            id,
            BASE,
            Property::LinkSub(
                base_name,
                edges.iter().map(|name| name.to_string()).collect(),
            ),
        )
        .unwrap();
        id
    }

    #[test]
    fn registers_the_dress_up_object_types() {
        setup();
        assert!(property_types::contains(crate::TYPE_NAME));
        assert!(ferrocad_core::object_registry::is_registered(FILLET_TYPE));
        assert!(ferrocad_core::object_registry::is_registered(CHAMFER_TYPE));
    }

    #[test]
    fn a_fillet_feature_fillets_the_linked_edge() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.set_property(fillet, "Radius", Property::Float(2.0))
            .unwrap();

        let executed = doc.recompute().unwrap();
        assert!(executed.contains(&fillet), "the feature recomputes");

        let object = doc.object(fillet).unwrap();
        assert!(!object.invalid, "the rebuild succeeded");
        let shape = stored_shape(&doc, fillet).expect("the output shape is set");
        assert_eq!(face_count(&shape), 7, "the fillet adds one face to a box");
        assert!(
            crate::backend().resolve(&shape, "Face1").is_some(),
            "the result is a real topology"
        );
    }

    #[test]
    fn changing_the_radius_rebuilds_the_shape() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.recompute().unwrap();
        let first = stored_shape(&doc, fillet).unwrap();

        doc.set_property(fillet, "Radius", Property::Float(3.0))
            .unwrap();
        assert_eq!(doc.recompute().unwrap(), vec![fillet]);
        let second = stored_shape(&doc, fillet).unwrap();

        assert!(
            !first.is_same(&second),
            "a new radius produces a new shape handle"
        );
        assert_eq!(face_count(&second), 7);
    }

    #[test]
    fn the_feature_follows_changes_to_its_base_shape() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.recompute().unwrap();
        let small = max_x(&stored_shape(&doc, fillet).unwrap());
        assert!((small - 10.0).abs() < 0.5, "a 10 mm box: got {small}");

        // Replace the base with a 20 mm box; the dependency edge enforces the fillet.
        let shape = crate::make_box(20.0, 20.0, 20.0).unwrap();
        doc.set_property(
            box_id,
            SHAPE,
            make_shape_property(ShapeProperty::new(shape)),
        )
        .unwrap();
        doc.recompute().unwrap();
        let large = max_x(&stored_shape(&doc, fillet).unwrap());
        assert!(
            (large - 20.0).abs() < 0.5,
            "the fillet rebuilt: got {large}"
        );
    }

    #[test]
    fn a_chamfer_feature_bevels_the_linked_edge() {
        setup();
        let mut doc = Document::new();
        box_feature(&mut doc, 10.0);
        let chamfer = doc.add_object("Chamfer", CHAMFER_TYPE);
        doc.set_property(
            chamfer,
            BASE,
            Property::LinkSub("Box".to_string(), vec!["Edge1".to_string()]),
        )
        .unwrap();
        doc.set_property(chamfer, "Distance", Property::Float(2.0))
            .unwrap();

        doc.recompute().unwrap();
        let object = doc.object(chamfer).unwrap();
        assert!(!object.invalid);
        let shape = stored_shape(&doc, chamfer).unwrap();
        assert_eq!(face_count(&shape), 7, "the chamfer adds one face to a box");
    }

    #[test]
    fn a_second_distance_makes_the_chamfer_asymmetric() {
        setup();
        let mut doc = Document::new();
        box_feature(&mut doc, 10.0);
        let symmetric = doc.add_object("Symmetric", CHAMFER_TYPE);
        doc.set_property(
            symmetric,
            BASE,
            Property::LinkSub("Box".to_string(), vec!["Edge1".to_string()]),
        )
        .unwrap();
        doc.recompute().unwrap();
        let symmetric_shape = stored_shape(&doc, symmetric).unwrap();

        let asymmetric = doc.add_object("Asymmetric", CHAMFER_TYPE);
        doc.set_property(
            asymmetric,
            BASE,
            Property::LinkSub("Box".to_string(), vec!["Edge1".to_string()]),
        )
        .unwrap();
        doc.set_property(asymmetric, "Distance2", Property::Float(3.0))
            .unwrap();
        doc.recompute().unwrap();
        let asymmetric_shape = stored_shape(&doc, asymmetric).unwrap();

        assert!(
            !symmetric_shape.is_same(&asymmetric_shape),
            "two distances bevel differently from one"
        );
        assert_eq!(face_count(&asymmetric_shape), 7);
    }

    #[test]
    fn a_missing_edge_marks_the_feature_invalid_and_leaves_a_null_shape() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge99"]);
        doc.recompute().unwrap();

        let object = doc.object(fillet).unwrap();
        assert!(object.invalid, "a missing edge fails the rebuild");
        assert!(
            stored_shape(&doc, fillet).is_none(),
            "the output is cleared, not left stale"
        );

        // Fixing the selection recovers on the next recompute.
        doc.set_property(
            fillet,
            BASE,
            Property::LinkSub("Box".to_string(), vec!["Edge1".to_string()]),
        )
        .unwrap();
        doc.recompute().unwrap();
        assert!(!doc.object(fillet).unwrap().invalid);
        assert_eq!(face_count(&stored_shape(&doc, fillet).unwrap()), 7);
    }

    #[test]
    fn the_edges_property_overrides_the_base_selection() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &[]);
        doc.set_property(
            fillet,
            EDGES,
            Property::StringList(vec!["Edge2".to_string()]),
        )
        .unwrap();
        doc.set_property(fillet, "Radius", Property::Float(1.0))
            .unwrap();

        doc.recompute().unwrap();
        assert!(!doc.object(fillet).unwrap().invalid);
        assert_eq!(face_count(&stored_shape(&doc, fillet).unwrap()), 7);
    }

    #[test]
    fn undoing_a_radius_change_rebuilds_the_previous_shape() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.recompute().unwrap();
        let one = stored_shape(&doc, fillet).unwrap();
        let one_fingerprint = fingerprint(&one);

        doc.open_transaction_named("radius");
        doc.set_property(fillet, "Radius", Property::Float(3.0))
            .unwrap();
        doc.commit_transaction();
        doc.recompute().unwrap();
        let three = stored_shape(&doc, fillet).unwrap();
        assert!(
            (fingerprint(&three) - one_fingerprint).abs() > 1e-3,
            "a bigger radius is different geometry"
        );

        // Undo restores the property **and** schedules the rebuild, so the output is the
        // radius-1 shape again rather than the stale radius-3 one.
        assert!(doc.undo());
        assert_eq!(
            doc.object(fillet).unwrap().properties.get("Radius"),
            Some(&Property::Float(1.0))
        );
        doc.recompute().unwrap();
        let reverted = stored_shape(&doc, fillet).unwrap();
        assert!(!reverted.is_same(&three), "the output was rebuilt");
        assert!(
            (fingerprint(&reverted) - one_fingerprint).abs() < 1e-3,
            "the rebuilt shape matches the radius-1 geometry"
        );

        assert!(doc.redo());
        doc.recompute().unwrap();
        let redone = stored_shape(&doc, fillet).unwrap();
        assert!(
            (fingerprint(&redone) - fingerprint(&three)).abs() < 1e-3,
            "redo rebuilds the radius-3 geometry"
        );
    }

    #[test]
    fn a_feature_without_a_base_is_invalid() {
        setup();
        let mut doc = Document::new();
        let fillet = doc.add_object("Fillet", FILLET_TYPE);
        doc.touch(fillet, false);
        doc.recompute().unwrap();
        assert!(doc.object(fillet).unwrap().invalid);
        assert!(stored_shape(&doc, fillet).is_none());
    }

    #[test]
    fn a_feature_cannot_use_itself_as_its_base() {
        setup();
        let mut doc = Document::new();
        let fillet = doc.add_object("Fillet", FILLET_TYPE);
        doc.set_property(
            fillet,
            BASE,
            Property::LinkSub("Fillet".to_string(), vec!["Edge1".to_string()]),
        )
        .unwrap();
        doc.recompute().unwrap();
        assert!(doc.object(fillet).unwrap().invalid);
        assert!(stored_shape(&doc, fillet).is_none());
    }

    #[test]
    fn a_fillet_resolves_its_edge_through_the_base_lineage() {
        setup();
        let mut doc = Document::new();
        let shape = crate::make_box(10.0, 10.0, 10.0).unwrap();
        let edges = crate::backend().elements(&shape, ElementKind::Edge);
        // `Edge1` was preserved across some earlier operation and now means edge 5.
        let mapped = edges[4].clone();
        let box_id = doc.add_object("Box", "Part::Feature");
        doc.set_property(
            box_id,
            SHAPE,
            make_shape_property(ShapeProperty::with_lineage(
                shape.clone(),
                lineage_binding("Edge1", &mapped),
            )),
        )
        .unwrap();

        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.set_property(fillet, "Radius", Property::Float(2.0))
            .unwrap();
        doc.recompute().unwrap();

        assert!(!doc.object(fillet).unwrap().invalid, "the rebuild succeeded");
        let out = stored_shape(&doc, fillet).expect("the output shape is set");
        assert_eq!(face_count(&out), 7, "the fillet adds one face to a box");

        let expected = direct_fillet(&shape, &mapped, 2.0);
        let positional = direct_fillet(&shape, &edges[0], 2.0);
        assert!(
            (fingerprint(&expected) - fingerprint(&positional)).abs() > 1e-2,
            "the premise: the two edges fillet differently ({} vs {})",
            fingerprint(&expected),
            fingerprint(&positional)
        );
        assert!(
            (fingerprint(&out) - fingerprint(&expected)).abs() < 1e-2,
            "the mapped edge was filleted, not the edge its ordinal names ({} vs {})",
            fingerprint(&out),
            fingerprint(&expected)
        );
    }

    #[test]
    fn a_retired_edge_fails_instead_of_moving_to_another_edge() {
        setup();
        let mut doc = Document::new();
        let shape = crate::make_box(10.0, 10.0, 10.0).unwrap();
        let edges = crate::backend().elements(&shape, ElementKind::Edge);
        // A non-empty map that never mentions `Edge1`: a positional fallback would
        // silently dress whatever edge now has that ordinal.
        let box_id = doc.add_object("Box", "Part::Feature");
        doc.set_property(
            box_id,
            SHAPE,
            make_shape_property(ShapeProperty::with_lineage(
                shape,
                lineage_binding("Edge2", &edges[4]),
            )),
        )
        .unwrap();

        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.recompute().unwrap();

        assert!(
            doc.object(fillet).unwrap().invalid,
            "a name the lineage does not justify fails the rebuild"
        );
        assert!(
            stored_shape(&doc, fillet).is_none(),
            "the output is cleared, not filleted elsewhere"
        );
    }

    #[test]
    fn a_negative_radius_is_rejected() {
        setup();
        let mut doc = Document::new();
        let box_id = box_feature(&mut doc, 10.0);
        let fillet = add_fillet(&mut doc, box_id, &["Edge1"]);
        doc.set_property(fillet, "Radius", Property::Float(-1.0))
            .unwrap();
        doc.recompute().unwrap();
        assert!(doc.object(fillet).unwrap().invalid);
        assert!(stored_shape(&doc, fillet).is_none());
    }
}

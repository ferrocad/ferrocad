//! `Part::Extrusion`: `Shape` is `Base` swept along `Dir` by `LengthFwd` / `LengthRev`.
//!
//! This is the document type behind push/pull, and it lives here rather than with the
//! Python bindings because it is kernel-neutral: it reads link and number properties,
//! asks the [`GeometryBackend`](ferrocad_geom::GeometryBackend), and writes a `Shape`.
//! A host with no interpreter — the browser, a `--script-host=plain` desktop — can
//! therefore create one.
//!
//! A zero taper uses the kernel prism; a non-zero taper builds a ruled loft through the
//! source wire and its offset copy at the far end, then cuts the inner wires' solids from
//! the outer one — the same construction as `ExtrusionHelper::makeElementDraft`. The
//! offset sign is chosen by which result grows the wire, so it does not depend on the
//! wire's orientation.

use std::f64::consts::PI;
use std::sync::Arc;

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{Document, ObjectId, Property, prop_status};
use ferrocad_geom::{AUTO_FUZZY, ElementKind, Lineage, Shape};
use ferrocad_types::{Matrix4, Vector3};

use crate::{ShapeProperty, backend, make_shape_property, shape_of};

/// `Part::Extrusion` (`FeatureExtrusion.cpp`).
pub const EXTRUSION_TYPE: &str = "Part::Extrusion";

/// `Part::Sketch`, whose `Placement` gives `DirMode = Normal` its direction.
const SKETCH_TYPE: &str = "Part::Sketch";

const SHAPE: &str = "Shape";

/// The precision below which a plane dimension or an extrusion length is zero
/// (OCCT's `Precision::Confusion`).
const CONFUSION: f64 = 1e-7;

/// Add the type to the object registry. Called by [`crate::register`].
pub(crate) fn register() {
    object_registry::register(EXTRUSION_TYPE, Arc::new(Extrusion));
}

struct Extrusion;

impl ObjectType for Extrusion {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Base".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "Dir".to_string(),
                Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
                prop_status::NONE,
            ),
            (
                "DirMode".to_string(),
                Property::Enumeration(
                    vec![
                        "Custom".to_string(),
                        "Edge".to_string(),
                        "Normal".to_string(),
                    ],
                    0,
                ),
                prop_status::NONE,
            ),
            (
                "DirLink".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "LengthFwd".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "LengthRev".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "Solid".to_string(),
                Property::Bool(false),
                prop_status::NONE,
            ),
            (
                "Reversed".to_string(),
                Property::Bool(false),
                prop_status::NONE,
            ),
            (
                "Symmetric".to_string(),
                Property::Bool(false),
                prop_status::NONE,
            ),
            (
                "TaperAngle".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "TaperAngleRev".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "FaceMakerClass".to_string(),
                Property::String("Part::FaceMakerExtrusion".to_string()),
                prop_status::READONLY | prop_status::HIDDEN,
            ),
            (
                "FaceMakerMode".to_string(),
                Property::Enumeration(
                    vec![
                        "Simple".to_string(),
                        "Cheese".to_string(),
                        "Extrusion".to_string(),
                        "Bullseye".to_string(),
                        "Unified".to_string(),
                    ],
                    2,
                ),
                prop_status::NONE,
            ),
            (
                "InnerWireTaper".to_string(),
                Property::Enumeration(
                    vec!["Inverted".to_string(), "SameAsOuter".to_string()],
                    0,
                ),
                prop_status::HIDDEN,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Extrude".to_string(), format!("Part extrusion {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = extrude(doc, id).map_err(|error| error.to_string());
        store(doc, id, result);
    }
}

/// The parameters `Extrusion::computeFinalParameters` derives from the properties.
struct ExtrusionParameters {
    direction: Vector3,
    length_fwd: f64,
    length_rev: f64,
    solid: bool,
    taper_fwd: f64,
    taper_rev: f64,
    same_as_outer: bool,
}

fn extrusion_parameters(doc: &mut Document, id: ObjectId) -> Result<ExtrusionParameters, String> {
    // Upstream's `Extrusion::computeFinalParameters`: `Custom` uses `Dir`, `Edge` takes
    // the direction of the linked line edge, and `Normal` uses the plane normal of
    // `Base` (a sketch's local +Z). The derived vector is written back to `Dir`, so a
    // later read of the property agrees with the extrusion.
    let raw = match enum_index_property(doc, id, "DirMode") {
        0 => vector_property(doc, id, "Dir").unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0)),
        1 => {
            let link = link_sub_property(doc, id, "DirLink").ok_or_else(|| {
                "DirMode is set to use edge, but no edge is linked.".to_string()
            })?;
            let edge = linked_shape(doc, &link)?;
            backend()
                .line_direction(&edge)
                .ok_or_else(|| "DirLink edge is not a line.".to_string())?
        }
        2 => {
            let link = link_sub_property(doc, id, "Base")
                .ok_or_else(|| "No object linked".to_string())?;
            let sketch_normal = doc
                .get_by_name(&link.0)
                .and_then(|object| doc.object(object))
                .filter(|object| object.type_id.as_str() == SKETCH_TYPE)
                .and_then(|object| match object.properties.get("Placement") {
                    Some(Property::Placement(placement)) => Some(
                        placement
                            .rotation
                            .to_matrix()
                            .transform(&Vector3::new(0.0, 0.0, 1.0)),
                    ),
                    _ => None,
                });
            match sketch_normal {
                Some(normal) => normal,
                None => {
                    let base = linked_shape(doc, &link)?;
                    backend().planar_normal(&base).ok_or_else(|| {
                        "Can't find normal direction, because the shape is not on a plane."
                            .to_string()
                    })?
                }
            }
        }
        _ => return Err("Unexpected enum value".to_string()),
    };
    if raw.length() < CONFUSION {
        return Err("Direction is zero-length".to_string());
    }
    doc.set_property_raw(id, "Dir", Property::Vector(raw));
    let mut direction = raw.normalize();
    if bool_property(doc, id, "Reversed", false) {
        direction = direction.neg();
    }
    let mut length_fwd = float_property(doc, id, "LengthFwd", 0.0);
    let mut length_rev = float_property(doc, id, "LengthRev", 0.0);
    if length_fwd.abs() < CONFUSION && length_rev.abs() < CONFUSION {
        length_fwd = raw.length();
    }
    if bool_property(doc, id, "Symmetric", false) {
        length_rev = length_fwd * 0.5;
        length_fwd *= 0.5;
    }
    if (length_fwd + length_rev).abs() < CONFUSION {
        return Err("Total length of extrusion is zero.".to_string());
    }
    let taper_fwd = float_property(doc, id, "TaperAngle", 0.0).to_radians();
    let taper_rev = float_property(doc, id, "TaperAngleRev", 0.0).to_radians();
    if taper_fwd.abs() > PI * 0.5 - 1e-12 || taper_rev.abs() > PI * 0.5 - 1e-12 {
        return Err(
            "Magnitude of taper angle matches or exceeds 90 degrees. That is too much."
                .to_string(),
        );
    }
    Ok(ExtrusionParameters {
        direction,
        length_fwd,
        length_rev,
        solid: bool_property(doc, id, "Solid", false),
        taper_fwd,
        taper_rev,
        same_as_outer: enum_index_property(doc, id, "InnerWireTaper") == 1,
    })
}

fn extrude(doc: &mut Document, id: ObjectId) -> Result<Shape, String> {
    let params = extrusion_parameters(doc, id)?;
    let base = link_sub_property(doc, id, "Base").ok_or_else(|| "No object linked".to_string())?;
    let source = linked_shape(doc, &base)?;
    extrude_shape(&source, &params)
}

fn extrude_shape(source: &Shape, params: &ExtrusionParameters) -> Result<Shape, String> {
    let backend = backend();
    if params.taper_fwd.abs() < 1e-12 && params.taper_rev.abs() < 1e-12 {
        // Regular prism: shift the source back by the reverse length, then sweep the
        // whole source by the total length.
        let mut shape = source.clone();
        if params.length_rev.abs() > CONFUSION {
            let matrix = translation_matrix(params.direction.scale(-params.length_rev));
            shape = backend
                .transform(&shape, &matrix)
                .map_err(|error| error.to_string())?;
        }
        // A wire has no interior, so a solid extrusion first builds the face the wire
        // bounds (upstream's `makeElementFace`).
        if params.solid && backend.elements(&shape, ElementKind::Face).is_empty() {
            let wires = wires_of(&shape);
            if wires.is_empty() {
                return Err("Extrusion: Input must not only consist if a vertex".to_string());
            }
            shape = backend
                .make_face(&wires)
                .map_err(|error| error.to_string())?;
        }
        let vector = params.direction.scale(params.length_fwd + params.length_rev);
        let prism = backend
            .extrude(&shape, vector)
            .map_err(|error| error.to_string())?;
        return ensure_solid(prism, params.solid);
    }

    let wires = wires_of(&source);
    if wires.is_empty() {
        return Err("Extrusion: Input must not only consist if a vertex".to_string());
    }
    // A face with holes drafts its outer wire and cuts the drafted inner wires; any
    // other source drafts each wire on its own (upstream's face/wire split).
    if let Some(outer) = backend.outer_wire(source) {
        let mut result = drafted_wire(&outer, params, params.taper_fwd, params.taper_rev)?;
        for inner in wires
            .iter()
            .filter(|wire| !backend.same_shape(wire, &outer))
        {
            let (taper_fwd, taper_rev) = if params.same_as_outer {
                (params.taper_fwd, params.taper_rev)
            } else {
                (-params.taper_fwd, -params.taper_rev)
            };
            let inner_shape = drafted_wire(inner, params, taper_fwd, taper_rev)?;
            result = backend
                .cut(&result, &inner_shape, AUTO_FUZZY)
                .map(|out| out.shape)
                .map_err(|error| error.to_string())?;
        }
        return Ok(result);
    }
    let mut drafts = Vec::with_capacity(wires.len());
    for wire in &wires {
        drafts.push(drafted_wire(wire, params, params.taper_fwd, params.taper_rev)?);
    }
    if drafts.len() == 1 {
        return Ok(drafts.remove(0));
    }
    backend
        .make_compound(&drafts)
        .map_err(|error| error.to_string())
}

/// Make the output a solid when the caller asked for one (a prism of a face is already
/// a solid; a swept wire's shell is closed into one).
fn ensure_solid(shape: Shape, solid: bool) -> Result<Shape, String> {
    if !solid {
        return Ok(shape);
    }
    let backend = backend();
    if !backend.elements(&shape, ElementKind::Solid).is_empty() {
        return Ok(shape);
    }
    match backend.elements(&shape, ElementKind::Shell).as_slice() {
        [shell] => backend
            .make_solid(&shell.shape)
            .map_err(|error| error.to_string()),
        _ => Ok(shape),
    }
}

fn wires_of(shape: &Shape) -> Vec<Shape> {
    backend()
        .elements(shape, ElementKind::Wire)
        .into_iter()
        .map(|element| element.shape)
        .collect()
}

fn wire_extent(shape: &Shape) -> f64 {
    match backend().bounds(shape) {
        Some((min, max)) => (max.x - min.x).abs() + (max.y - min.y).abs() + (max.z - min.z).abs(),
        None => 0.0,
    }
}

/// Offset `wire` by `distance` in the direction that grows (`grow`) or shrinks it. The
/// kernel offset's sign depends on the wire orientation, so both signs are tried and the
/// result's extent decides.
fn offset_wire(wire: &Shape, distance: f64, grow: bool) -> Result<Shape, String> {
    if distance.abs() < CONFUSION {
        return Ok(wire.clone());
    }
    let backend = backend();
    let mut options: Vec<Shape> = [distance, -distance]
        .into_iter()
        .filter_map(|offset| backend.make_offset(wire, offset, 2).ok())
        .collect();
    if options.len() == 2 && wire_extent(&options[1]) > wire_extent(&options[0]) {
        options.swap(0, 1);
    }
    if options.is_empty() {
        return Err("Extrusion: Offset could not be created".to_string());
    }
    let chosen = if grow {
        options.first().cloned()
    } else {
        options.last().cloned()
    };
    chosen.ok_or_else(|| "Extrusion: Offset could not be created".to_string())
}

/// One wire's drafted solid (or shell when `solid` is false): a ruled loft through the
/// offset section at the reverse end (when any), the source wire and the offset section
/// at the forward end, in that order.
fn drafted_wire(
    wire: &Shape,
    params: &ExtrusionParameters,
    taper_fwd: f64,
    taper_rev: f64,
) -> Result<Shape, String> {
    let backend = backend();
    let has_fwd = params.length_fwd.abs() > CONFUSION;
    let has_rev = params.length_rev.abs() > CONFUSION;
    // Keep the source as a middle section unless the two directions have opposite signs
    // and no angle changes (upstream's `bMid`).
    let keep_middle = !has_fwd || !has_rev || params.length_fwd * params.length_rev > 0.0;
    let mut sections = Vec::new();
    if has_rev {
        let distance = taper_rev.tan() * params.length_rev;
        let offset = offset_wire(wire, distance, distance >= 0.0)?;
        let matrix = translation_matrix(params.direction.scale(-params.length_rev));
        sections.push(
            backend
                .transform(&offset, &matrix)
                .map_err(|error| error.to_string())?,
        );
    }
    if keep_middle {
        sections.push(wire.clone());
    }
    if has_fwd {
        let distance = taper_fwd.tan() * params.length_fwd;
        let offset = offset_wire(wire, distance, distance >= 0.0)?;
        let matrix = translation_matrix(params.direction.scale(params.length_fwd));
        sections.push(
            backend
                .transform(&offset, &matrix)
                .map_err(|error| error.to_string())?,
        );
    }
    if sections.len() < 2 {
        return Err("Extrusion: Loft could not be built".to_string());
    }
    backend
        .make_loft(&sections, params.solid)
        .map_err(|error| error.to_string())
}

/// A straight prism drafted from `section`: the section swept `direction * length`,
/// with the far section offset by `tan(angle) * length` in its own plane.
///
/// `section` is the hinge end (a face, or a wire); a positive `angle` widens the far
/// end, the sign `Extrusion`'s `TaperAngle` uses. A face with holes cuts the drafted
/// inner wires, as a tapered extrusion does. `PartDesign::Draft` calls this for the
/// straight-prism class it supports.
pub(crate) fn draft_section(
    section: &Shape,
    direction: Vector3,
    length: f64,
    angle: f64,
) -> Result<Shape, String> {
    extrude_shape(
        section,
        &ExtrusionParameters {
            direction,
            length_fwd: length,
            length_rev: 0.0,
            solid: true,
            taper_fwd: angle,
            taper_rev: 0.0,
            same_as_outer: true,
        },
    )
}

// ---------------------------------------------------------------------------
// The property readers and the output, shared with the other Part features.
// ---------------------------------------------------------------------------

fn output_shape() -> Property {
    make_shape_property(ShapeProperty::empty())
}

fn output_property() -> (String, Property, u32) {
    (SHAPE.to_string(), output_shape(), prop_status::OUTPUT)
}

/// Store a recompute outcome the way the Part containers do: the rebuilt shape, or an
/// empty output with the invalid flag set.
fn store(doc: &mut Document, id: ObjectId, result: Result<Shape, String>) {
    match result {
        Ok(shape) => {
            doc.set_property_raw(
                id,
                SHAPE,
                make_shape_property(ShapeProperty::with_lineage(shape, Lineage::default())),
            );
        }
        Err(_) => {
            doc.mark_invalid(id);
            doc.set_property_raw(id, SHAPE, output_shape());
        }
    }
}

fn link_sub_property(doc: &Document, id: ObjectId, name: &str) -> Option<(String, Vec<String>)> {
    match doc.object(id)?.properties.get(name)? {
        Property::LinkSub(object, subs) if !object.is_empty() => {
            Some((object.clone(), subs.clone()))
        }
        Property::Link(object) if !object.is_empty() => Some((object.clone(), Vec::new())),
        _ => None,
    }
}

fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Float(value)) => *value,
        Some(Property::Integer(value)) => *value as f64,
        _ => default,
    }
}

fn bool_property(doc: &Document, id: ObjectId, name: &str, default: bool) -> bool {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Bool(value)) => *value,
        _ => default,
    }
}

fn vector_property(doc: &Document, id: ObjectId, name: &str) -> Option<Vector3> {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Vector(value)) => Some(*value),
        _ => None,
    }
}

fn enum_index_property(doc: &Document, id: ObjectId, name: &str) -> usize {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Enumeration(_, index)) => *index,
        _ => 0,
    }
}

/// The shape a link property names, resolving a single sub-element when one is selected
/// (upstream's `Feature::getTopoShape(..., NeedSubElement)`).
fn linked_shape(doc: &Document, link: &(String, Vec<String>)) -> Result<Shape, String> {
    let (name, subs) = link;
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    let shape = doc
        .object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .and_then(|property| property.shape())
        .ok_or_else(|| format!("object {name} has no shape"))?;
    match subs.as_slice() {
        [] => Ok(shape),
        [sub] if sub.is_empty() => Ok(shape),
        [sub] => backend()
            .resolve(&shape, sub)
            .ok_or_else(|| format!("object {name} has no sub-shape {sub}")),
        _ => Err("not exactly one shape linked".to_string()),
    }
}

fn translation_matrix(offset: Vector3) -> Matrix4 {
    let mut values = Matrix4::identity().m;
    values[3] = offset.x;
    values[7] = offset.y;
    values[11] = offset.z;
    Matrix4::from_values(values)
}

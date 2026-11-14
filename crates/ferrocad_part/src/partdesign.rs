//! `PartDesign::Draft` and `PartDesign::Plane`: the dress-up and datum the surface
//! tools build on a private document.
//!
//! FerroCAD does not implement the PartDesign workbench. SimpleCAD's draft tool
//! (`simplecad/surfaces/offset.py`) opens a private document, puts the shape in a
//! `Part::Feature`, adds a `PartDesign::Body`, a `PartDesign::Plane` for the hinge and
//! a `PartDesign::Draft`, recomputes, and reads the feature's `Shape`. Those three
//! document types are all it needs. They live here beside `Part::Extrusion` rather
//! than with the Python bindings because they are kernel-neutral.
//!
//! # The supported draft
//!
//! Upstream's `PartDesign::Draft` is a dress-up over an arbitrary body: it tilts every
//! selected face about its intersection with the neutral plane and heals the
//! neighbours (`BRepOffsetAPI_DraftAngle`). The pinned `ferrocad_occt_cxx` does not
//! bind that class, so this feature implements the case the tools ask for and refuses
//! the rest loudly:
//!
//! - the body is a straight prism along the neutral plane's normal,
//! - every side wall is selected (flat walls, or one cylindrical wall),
//! - the remaining faces are exactly the two end caps, and the neutral plane passes
//!   through one of them.
//!
//! The drafted body is then the hinge cap swept to the far cap with a taper, which is
//! exact for that class: it is the same `BRepOffsetAPI_MakeOffset` + loft construction
//! a tapered `Part::Extrusion` uses, and it is why a drafted cylinder is a real
//! `Part.Cone`. Anything else — a partial wall selection, a hinge inside the body, a
//! free-form wall, a stepped prism — raises with the reason instead of approximating.
//!
//! `Reversed` is `SimpleCAD`'s `narrower = 'bottom'`: not reversed narrows toward the
//! neutral plane's `+Z`, reversed narrows toward `-Z`.

use ferrocad_core::object_registry::ObjectType;
use ferrocad_core::{Document, ObjectId, Placement, Property, prop_status};
use ferrocad_geom::{ElementKind, Lineage, Shape, SurfaceData};
use ferrocad_types::Vector3;

use crate::shape_property::{ShapeProperty, make_shape_property, resolve_element_in, shape_of};

/// `PartDesign::Plane`: the hinge datum.
pub const PLANE_TYPE: &str = "PartDesign::Plane";
/// `PartDesign::Draft`: tilt a straight prism's side walls about a hinge plane.
pub const DRAFT_TYPE: &str = "PartDesign::Draft";

/// The side of the datum plane's square, in millimetres. Upstream draws a fixed
/// marker; nothing reads its size.
const PLANE_SIZE: f64 = 10.0;

/// How close a direction must be to parallel/perpendicular, and how close the neutral
/// plane must be to an end cap, in millimetres.
const TOLERANCE: f64 = 1e-6;

/// The largest draft this supports, matching SimpleCAD's own bound.
const MAX_ANGLE_DEGREES: f64 = 89.9;

/// Add the types to the object registry. Called by [`crate::register`].
pub(crate) fn register() {
    ferrocad_core::object_registry::register(PLANE_TYPE, std::sync::Arc::new(DatumPlane));
    ferrocad_core::object_registry::register(DRAFT_TYPE, std::sync::Arc::new(Draft));
}

// ---------------------------------------------------------------------------
// `PartDesign::Plane`
// ---------------------------------------------------------------------------

/// `PartDesign::Plane`: a square marker face derived from `Placement`, like
/// `Part::DatumPlane`. Its `MapMode` is the attachment flag the tools set to
/// `Deactivated`; no map mode moves the plane, which is what the draft reads.
struct DatumPlane;

impl ObjectType for DatumPlane {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            (
                "MapMode".to_string(),
                Property::Enumeration(
                    vec![
                        "Deactivated".to_string(),
                        "ObjectXY".to_string(),
                        "ObjectXZ".to_string(),
                        "ObjectYZ".to_string(),
                    ],
                    0,
                ),
                prop_status::NONE,
            ),
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
        let Some(local) = crate::datum::square_face(PLANE_SIZE) else {
            doc.mark_invalid_with(id, "the datum plane marker could not be built");
            return;
        };
        let placement = placement_of(doc, id);
        match crate::backend().place(&local, &placement) {
            Ok(shape) => {
                doc.set_property_raw(
                    id,
                    "Shape",
                    make_shape_property(ShapeProperty::with_placement(shape, placement)),
                );
            }
            Err(error) => doc.mark_invalid_with(id, error.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// `PartDesign::Draft`
// ---------------------------------------------------------------------------

/// `PartDesign::Draft`: the dress-up SimpleCAD's draft tool builds.
struct Draft;

impl ObjectType for Draft {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Base".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "BaseFeature".to_string(),
                Property::Link(String::new()),
                prop_status::HIDDEN,
            ),
            (
                "Angle".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "NeutralPlane".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "Reversed".to_string(),
                Property::Bool(false),
                prop_status::NONE,
            ),
            (
                "Shape".to_string(),
                make_shape_property(ShapeProperty::empty()),
                prop_status::OUTPUT,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Draft".to_string(), format!("PartDesign draft {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = rebuild(doc, id);
        match result {
            Ok(shape) => {
                doc.set_property_raw(
                    id,
                    "Shape",
                    make_shape_property(ShapeProperty::with_lineage(shape, Lineage::default())),
                );
            }
            // Keep the last good shape, mirrored on the dress-up features: a failed
            // rebuild reports its reason instead of blanking the body.
            Err(reason) => doc.mark_invalid_with(id, reason),
        }
    }
}

/// How a face bounds the prism along the pull direction.
enum FaceKind {
    /// Parallel to the neutral plane: an end cap.
    Cap { origin: Vector3 },
    /// Parallel to the pull: a side wall.
    Wall,
}

/// The shape and taper the hinge analysis resolves.
struct DraftRequest {
    /// The hinge end, swept to the far end.
    section: Shape,
    direction: Vector3,
    length: f64,
    /// Radians; positive widens the far end.
    taper: f64,
}

fn rebuild(doc: &Document, id: ObjectId) -> Result<Shape, String> {
    let (base_name, subs) = base_link(doc, id)?;
    if doc.object(id).is_some_and(|this| this.name == base_name) {
        return Err("a draft cannot use itself as its base".to_string());
    }
    if subs.is_empty() {
        return Err("the draft selects no faces".to_string());
    }
    let angle = float_property(doc, id, "Angle")
        .ok_or_else(|| "the draft has no Angle".to_string())?;
    if !(angle > 0.0 && angle < MAX_ANGLE_DEGREES) {
        return Err(format!(
            "the draft angle must be between 0 and {MAX_ANGLE_DEGREES} degrees"
        ));
    }
    let reversed = matches!(property(doc, id, "Reversed"), Some(Property::Bool(true)));
    let plane = neutral_plane(doc, id)?;

    let (source, lineage) = source_shape(doc, &base_name)?;
    let request = analyse(&source, &lineage, &subs, &plane, angle, reversed)?;
    crate::extrusion::draft_section(
        &request.section,
        request.direction,
        request.length,
        request.taper,
    )
}

/// The base object and its selected face names: `Base` first, then `BaseFeature`.
fn base_link(doc: &Document, id: ObjectId) -> Result<(String, Vec<String>), String> {
    match property(doc, id, "Base") {
        Some(Property::LinkSub(object, subs)) if !object.is_empty() => {
            return Ok((object.clone(), subs.clone()));
        }
        _ => {}
    }
    match property(doc, id, "BaseFeature") {
        Some(Property::Link(object)) if !object.is_empty() => Ok((object.clone(), Vec::new())),
        _ => Err("the draft has no Base link".to_string()),
    }
}

/// The hinge plane's placement: its origin is on the hinge and its local `+Z` is the
/// pull direction.
fn neutral_plane(doc: &Document, id: ObjectId) -> Result<Placement, String> {
    let name = match property(doc, id, "NeutralPlane") {
        Some(Property::LinkSub(object, _)) if !object.is_empty() => object.clone(),
        Some(Property::Link(object)) if !object.is_empty() => object.clone(),
        _ => return Err("the draft has no NeutralPlane".to_string()),
    };
    let plane = doc
        .get_by_name(&name)
        .and_then(|plane| doc.object(plane))
        .ok_or_else(|| format!("no object named {name}"))?;
    match plane.properties.get("Placement") {
        Some(Property::Placement(placement)) => Ok(*placement),
        _ => Err(format!("the neutral plane {name} has no Placement")),
    }
}

/// The shape a linked object holds, plus the lineage stored beside it.
fn source_shape(doc: &Document, name: &str) -> Result<(Shape, Lineage), String> {
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    let property = doc
        .object(id)
        .and_then(|object| object.properties.get("Shape"))
        .and_then(shape_of)
        .ok_or_else(|| format!("object {name} has no shape"))?;
    let shape = property
        .shape()
        .ok_or_else(|| format!("object {name} has no shape"))?;
    Ok((shape, property.lineage().clone()))
}

/// Classify the body's faces, find the hinge cap, and turn the requested angle into
/// the sweeping taper.
fn analyse(
    source: &Shape,
    lineage: &Lineage,
    subs: &[String],
    plane: &Placement,
    angle_degrees: f64,
    reversed: bool,
) -> Result<DraftRequest, String> {
    let backend = crate::backend();
    if backend.elements(source, ElementKind::Solid).len() != 1 {
        return Err("the draft needs exactly one solid".to_string());
    }
    let direction = plane
        .rotation
        .to_matrix()
        .transform(&Vector3::new(0.0, 0.0, 1.0));
    let direction = direction.normalize();

    let selected: Vec<Shape> = subs
        .iter()
        .map(|name| {
            resolve_element_in(source, lineage, name)
                .map_err(|_| format!("the draft cannot resolve {name} on its base"))
        })
        .collect::<Result<_, _>>()?;

    let mut caps = Vec::new();
    let mut walls = 0usize;
    let mut selected_cap = false;
    let mut unselected_wall = false;
    for element in backend.elements(source, ElementKind::Face) {
        let is_selected = selected
            .iter()
            .any(|face| backend.same_shape(face, &element.shape));
        match classify(&element.shape, &direction)? {
            FaceKind::Cap { origin } => {
                if is_selected {
                    selected_cap = true;
                }
                caps.push((element.shape, origin));
            }
            FaceKind::Wall => {
                if !is_selected {
                    unselected_wall = true;
                }
                walls += 1;
            }
        }
    }
    if selected_cap {
        return Err(
            "a face square to the pull direction cannot be drafted; select the side walls"
                .to_string(),
        );
    }
    if walls == 0 {
        return Err("the draft selects no side walls".to_string());
    }
    if unselected_wall {
        return Err(
            "a straight prism drafts every side wall at once; select all of its walls"
                .to_string(),
        );
    }
    if caps.len() != 2 {
        return Err(format!(
            "the draft supports a straight prism with two end caps, not {}",
            caps.len()
        ));
    }

    // The hinge is the cap the neutral plane lies in; the other cap sets the length.
    let mut hinge = None;
    let mut far = None;
    for (shape, origin) in caps {
        let distance = origin.sub(&plane.base).dot(&direction);
        if distance.abs() <= TOLERANCE {
            hinge = Some((shape, distance));
        } else {
            far = Some(distance);
        }
    }
    let (Some((section, hinge_distance)), Some(far_distance)) = (hinge, far) else {
        return Err(
            "the neutral plane must pass through one end cap of a straight prism".to_string(),
        );
    };
    let height = far_distance - hinge_distance;
    if height.abs() <= TOLERANCE {
        return Err("the draft's two end caps coincide".to_string());
    }

    // Not reversed narrows toward the neutral plane's +Z, reversed toward -Z.
    let narrowing_positive = !reversed;
    let far_is_narrow = (height > 0.0) == narrowing_positive;
    let radians = angle_degrees.to_radians();
    let taper = if far_is_narrow { -radians } else { radians };
    Ok(DraftRequest {
        section,
        direction: direction.scale(height.signum()),
        length: height.abs(),
        taper,
    })
}

/// A face's role: an end cap (with a point on its plane) or a side wall.
fn classify(face: &Shape, direction: &Vector3) -> Result<FaceKind, String> {
    match crate::backend().surface_data(face) {
        Some(SurfaceData::Plane { origin, normal, .. }) => {
            let parallel = normal.normalize().dot(direction).abs();
            if parallel >= 1.0 - TOLERANCE {
                Ok(FaceKind::Cap { origin })
            } else if parallel <= TOLERANCE {
                Ok(FaceKind::Wall)
            } else {
                Err("the draft supports walls parallel and caps square to the pull".to_string())
            }
        }
        Some(SurfaceData::Cylinder { axis, .. }) => {
            if axis.normalize().dot(direction).abs() >= 1.0 - TOLERANCE {
                Ok(FaceKind::Wall)
            } else {
                Err("the draft's cylindrical wall must be parallel to the pull".to_string())
            }
        }
        _ => Err("the draft supports flat and cylindrical walls only".to_string()),
    }
}

fn property<'a>(doc: &'a Document, id: ObjectId, name: &str) -> Option<&'a Property> {
    doc.object(id)?.properties.get(name)
}

fn float_property(doc: &Document, id: ObjectId, name: &str) -> Option<f64> {
    match property(doc, id, name) {
        Some(Property::Float(value)) => Some(*value),
        Some(Property::Integer(value)) => Some(*value as f64),
        _ => None,
    }
}

fn placement_of(doc: &Document, id: ObjectId) -> Placement {
    match property(doc, id, "Placement") {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

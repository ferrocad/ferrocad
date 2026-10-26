//! The `Part::AttachExtension` property set and attach-engine subset.
//!
//! Upstream this is `Part/App/AttachExtension.cpp` plus `Attacher.cpp`'s
//! `AttachEngine3D`/`Plane`/`Line`/`Point`. The extension adds
//! `AttachmentSupport`, `MapMode`, `MapPathParameter` and `AttachmentOffset` to an
//! object and derives the object's **`Placement`** from the support geometry. It is
//! added dynamically to `App::FeaturePython` (`addExtension`) and statically to
//! `Part::Part2DObjectPython`, which is why the property list lives here rather
//! than in one object type's defaults.
//!
//! # What is implemented
//!
//! The modes Draft actually sets, plus the variants that share their math:
//!
//! - plane modes: `Deactivated`, `ObjectXY`, `ObjectXZ`, `ObjectYZ`, `FlatFace`,
//!   `NormalToEdge`, `ThreePointsPlane`, `ThreePointsNormal`, `InertialCS`;
//! - line modes: `ObjectX`, `ObjectY`, `ObjectZ`, `ObjectNormal`, `TwoPointLine`,
//!   `AxisOfInertia1/2/3`;
//! - point modes: `Vertex`, `CenterOfMass`, `Focus1/2`, `ObjectOrigin`;
//! - `Translate` (a vertex plus the offset position).
//!
//! A mode without an implementation fails with a reason naming the mode and leaves
//! the object invalid: silently keeping the last placement would claim an attachment
//! that does not exist.
//!
//! # Coordinates
//!
//! Upstream computes the support sub-shape in *global* coordinates
//! (`ShapeOption::Transform` + `getGlobalPlacement`) and projects the support
//! object's origin onto it. FerroCAD stores Part primitives' `Shape` already moved
//! by their `Placement` (`Part::Box` bakes it in `execute`), while a Python proxy's
//! shape is local and its `Placement` carries the transform. The split is therefore
//! made on the object type: a Python feature's shape is moved by its `Placement`
//! here, a native Part object's shape is used as stored.

use ferrocad_core::object_registry;
use ferrocad_core::{prop_status, Document, ObjectId, Placement, Property, Rotation, Vector3};
use ferrocad_geom::{ElementKind, Shape};
use ferrocad_part::{backend, shape_of};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyTuple};

/// The extension id a script adds (`obj.addExtension("Part::AttachExtensionPython")`).
pub const EXTENSION: &str = "Part::AttachExtensionPython";
/// The base extension. `Part::AttachExtensionPython` derives from it upstream, and
/// `hasExtension("Part::AttachExtension")` is the check Draft's tests make.
pub const BASE_EXTENSION: &str = "Part::AttachExtension";

/// Upstream `Attacher::AttachEngine3D` (the default attacher).
pub const ATTACHER_3D: &str = "Attacher::AttachEngine3D";
/// Upstream `Attacher::AttachEnginePlane` (`Part::Part2DObject`'s default).
pub const ATTACHER_PLANE: &str = "Attacher::AttachEnginePlane";
/// Upstream `Attacher::AttachEngineLine`.
pub const ATTACHER_LINE: &str = "Attacher::AttachEngineLine";
/// Upstream `Attacher::AttachEnginePoint`.
pub const ATTACHER_POINT: &str = "Attacher::AttachEnginePoint";

/// The default mode.
pub const MODE_DEACTIVATED: &str = "Deactivated";

const ATTACH_GROUP: &str = "Attachment";
const SUPPORT: &str = "AttachmentSupport";
const MAP_MODE: &str = "MapMode";
const MAP_PATH_PARAMETER: &str = "MapPathParameter";
const OFFSET: &str = "AttachmentOffset";
const ATTACHER_TYPE: &str = "AttacherType";

/// Upstream `AttachEngine::eMapModeStrings`, in the engine order.
const ALL_MODES: [&str; 50] = [
    "Deactivated",
    "Translate",
    "ObjectXY",
    "ObjectXZ",
    "ObjectYZ",
    "FlatFace",
    "TangentPlane",
    "NormalToEdge",
    "FrenetNB",
    "FrenetTN",
    "FrenetTB",
    "Concentric",
    "SectionOfRevolution",
    "ThreePointsPlane",
    "ThreePointsNormal",
    "Folding",
    "ObjectX",
    "ObjectY",
    "ObjectZ",
    "AxisOfCurvature",
    "Directrix1",
    "Directrix2",
    "Asymptote1",
    "Asymptote2",
    "Tangent",
    "Normal",
    "Binormal",
    "TangentU",
    "TangentV",
    "TwoPointLine",
    "IntersectionLine",
    "ProximityLine",
    "ObjectOrigin",
    "Focus1",
    "Focus2",
    "OnEdge",
    "CenterOfCurvature",
    "CenterOfMass",
    "IntersectionPoint",
    "Vertex",
    "ProximityPoint1",
    "ProximityPoint2",
    "AxisOfInertia1",
    "AxisOfInertia2",
    "AxisOfInertia3",
    "InertialCS",
    "FaceNormal",
    "OZX",
    "OZY",
    "OXY",
];

/// Modes `AttachEngine3D` advertises (`modeRefTypes` entries with a reference
/// combination). `AttachEnginePlane` reuses the 3D set.
fn modes_3d() -> Vec<String> {
    [
        MODE_DEACTIVATED,
        "Translate",
        "ObjectXY",
        "ObjectXZ",
        "ObjectYZ",
        "FlatFace",
        "TangentPlane",
        "NormalToEdge",
        "FrenetNB",
        "FrenetTN",
        "FrenetTB",
        "Concentric",
        "SectionOfRevolution",
        "ThreePointsPlane",
        "ThreePointsNormal",
        "Folding",
        "InertialCS",
        "ParallelPlane",
        "MidPlane",
    ]
    .iter()
    .map(|mode| mode.to_string())
    .collect()
}

/// Modes `AttachEngineLine` advertises.
fn modes_line() -> Vec<String> {
    [
        MODE_DEACTIVATED,
        "ObjectX",
        "ObjectY",
        "ObjectZ",
        "AxisOfCurvature",
        "Binormal",
        "Normal",
        "Tangent",
        "TangentU",
        "TangentV",
        "TwoPointLine",
        "Asymptote1",
        "Asymptote2",
        "Directrix1",
        "Directrix2",
        "ProximityLine",
        "AxisOfInertia1",
        "AxisOfInertia2",
        "AxisOfInertia3",
        "FaceNormal",
        "IntersectionLine",
    ]
    .iter()
    .map(|mode| mode.to_string())
    .collect()
}

/// Modes `AttachEnginePoint` advertises.
fn modes_point() -> Vec<String> {
    [
        MODE_DEACTIVATED,
        "ObjectOrigin",
        "CenterOfCurvature",
        "OnEdge",
        "Vertex",
        "Focus1",
        "Focus2",
        "ProximityPoint1",
        "ProximityPoint2",
        "CenterOfMass",
    ]
    .iter()
    .map(|mode| mode.to_string())
    .collect()
}

/// The modes `attacher` supports, in the order they appear in `MapMode`.
fn modes_for(attacher: &str) -> Vec<String> {
    match attacher {
        ATTACHER_PLANE => modes_3d(),
        ATTACHER_LINE => modes_line(),
        ATTACHER_POINT => modes_point(),
        _ => modes_3d(),
    }
}

/// Whether `name` is one of the four attach engines upstream registers.
fn is_attach_engine(name: &str) -> bool {
    matches!(name, ATTACHER_3D | ATTACHER_PLANE | ATTACHER_LINE | ATTACHER_POINT)
}

/// The extension's property list.
pub fn attachment_properties(attacher: &str) -> Vec<(String, Property, u32)> {
    vec![
        (
            SUPPORT.to_string(),
            // Upstream's `App::PropertyLinkSubList`: one `(object, subnames)`
            // entry per support. The empty subname `""` means the whole object.
            Property::LinkSubList(Vec::new()),
            prop_status::NONE,
        ),
        (
            MAP_MODE.to_string(),
            Property::Enumeration(modes_for(attacher), 0),
            prop_status::NONE,
        ),
        (
            MAP_PATH_PARAMETER.to_string(),
            Property::Float(0.0),
            prop_status::HIDDEN,
        ),
        (
            OFFSET.to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        (
            ATTACHER_TYPE.to_string(),
            Property::String(attacher.to_string()),
            prop_status::HIDDEN,
        ),
    ]
}

/// Add the attachment property set to `id` (`addExtension`'s provider).
///
/// Idempotent: an object that already carries `AttacherType` keeps its values. Both
/// extension ids are recorded, so `hasExtension` answers for the Python template and
/// its base the way the C++ `ExtensionPythonT` does.
pub fn provide(doc: &mut Document, id: ObjectId) -> bool {
    doc.add_extension(id, EXTENSION);
    doc.add_extension(id, BASE_EXTENSION);
    let Some(object) = doc.object(id) else {
        return false;
    };
    if object.properties.get(ATTACHER_TYPE).is_some() {
        return true;
    }
    for (name, value, status) in attachment_properties(ATTACHER_3D) {
        if doc
            .add_property(id, &name, value, status, ATTACH_GROUP, "")
            .is_err()
        {
            return false;
        }
    }
    true
}

/// Whether `id` carries the attachment property set (statically or dynamically).
pub fn has_attachment(doc: &Document, id: ObjectId) -> bool {
    doc.object(id)
        .is_some_and(|object| object.properties.get(ATTACHER_TYPE).is_some())
}

/// `changeAttacherType(name)`: switch the engine driving the attachment.
///
/// The mode goes back to `Deactivated` and `MapMode` advertises the new engine's
/// modes, as upstream's `setAttacher` does before the user picks a mode.
pub fn change_attacher_type(doc: &mut Document, id: ObjectId, name: &str) -> Result<bool, String> {
    if name.is_empty() {
        doc.set_property_raw(id, ATTACHER_TYPE, Property::String(String::new()));
        return Ok(true);
    }
    if !is_attach_engine(name) {
        return Err(format!(
            "Object if this type is not derived from AttachEngine: {name}"
        ));
    }
    let current = property_string(doc, id, ATTACHER_TYPE).unwrap_or_default();
    if current == name {
        return Ok(false);
    }
    doc.set_property_raw(id, ATTACHER_TYPE, Property::String(name.to_string()));
    doc.set_property_raw(
        id,
        MAP_MODE,
        Property::Enumeration(modes_for(name), 0),
    );
    Ok(true)
}

// ---------------------------------------------------------------------------
// Reading the attachment properties
// ---------------------------------------------------------------------------

fn property_string(doc: &Document, id: ObjectId, name: &str) -> Option<String> {
    match doc.object(id)?.properties.get(name) {
        Some(Property::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn mode_name(doc: &Document, id: ObjectId) -> Option<String> {
    match doc.object(id)?.properties.get(MAP_MODE) {
        Some(Property::Enumeration(choices, index)) => choices.get(*index).cloned(),
        Some(Property::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn map_path_parameter(doc: &Document, id: ObjectId) -> f64 {
    match doc.object(id).and_then(|o| o.properties.get(MAP_PATH_PARAMETER)) {
        Some(Property::Float(value)) => *value,
        _ => 0.0,
    }
}

fn attachment_offset(doc: &Document, id: ObjectId) -> Placement {
    match doc.object(id).and_then(|o| o.properties.get(OFFSET)) {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

fn placement_property(doc: &Document, id: ObjectId) -> Option<Placement> {
    match doc.object(id)?.properties.get("Placement") {
        Some(Property::Placement(placement)) => Some(*placement),
        _ => None,
    }
}

/// One support reference: an object name and an optional sub-element name.
#[derive(Debug, Clone)]
struct SupportRef {
    object: String,
    sub: String,
}

/// The supports stored in `AttachmentSupport`.
///
/// `LinkSubList` is the real `App::PropertyLinkSubList`: each `(object, subnames)`
/// entry becomes one reference per sub-element, and an empty subname (or an empty
/// subname list) means the whole object. `LinkSub` and `Link` are accepted too, so
/// a value written by an older document still resolves.
fn supports(doc: &Document, id: ObjectId) -> Vec<SupportRef> {
    let Some(property) = doc.object(id).and_then(|o| o.properties.get(SUPPORT)) else {
        return Vec::new();
    };
    match property {
        Property::LinkSubList(entries) => {
            let mut references = Vec::new();
            for (object, subs) in entries {
                if object.is_empty() {
                    continue;
                }
                let named: Vec<&String> = subs.iter().filter(|sub| !sub.is_empty()).collect();
                if named.is_empty() {
                    references.push(SupportRef {
                        object: object.clone(),
                        sub: String::new(),
                    });
                } else {
                    references.extend(named.into_iter().map(|sub| SupportRef {
                        object: object.clone(),
                        sub: sub.clone(),
                    }));
                }
            }
            references
        }
        Property::LinkSub(object, subs) => {
            if object.is_empty() {
                Vec::new()
            } else if subs.is_empty() {
                vec![SupportRef {
                    object: object.clone(),
                    sub: String::new(),
                }]
            } else {
                subs.iter()
                    .map(|sub| SupportRef {
                        object: object.clone(),
                        sub: sub.clone(),
                    })
                    .collect()
            }
        }
        Property::LinkList(objects) => objects
            .iter()
            .filter(|name| !name.is_empty())
            .map(|object| SupportRef {
                object: object.clone(),
                sub: String::new(),
            })
            .collect(),
        // A single name is accepted as a one-element support list.
        Property::Link(object) if !object.is_empty() => vec![SupportRef {
            object: object.clone(),
            sub: String::new(),
        }],
        _ => Vec::new(),
    }
}

/// A resolved support: its geometry in the coordinates the modes expect, the
/// support object's own placement, and the support object's origin.
struct ResolvedSupport {
    shape: Option<Shape>,
    placement: Placement,
    origin: Vector3,
    whole_object: bool,
}

fn resolve_support(doc: &Document, reference: &SupportRef) -> Result<ResolvedSupport, String> {
    let support_id = doc
        .get_by_name(&reference.object)
        .ok_or_else(|| format!("attachment support '{}' does not exist", reference.object))?;
    let object = doc
        .object(support_id)
        .ok_or_else(|| format!("attachment support '{}' does not exist", reference.object))?;
    let placement = placement_property(doc, support_id).unwrap_or_else(Placement::identity);
    let property = object
        .properties
        .get("Shape")
        .and_then(shape_of)
        .ok_or_else(|| {
            format!(
                "attachment support '{}' has no shape",
                reference.object
            )
        })?;
    let base = property.shape();
    let shape = match base {
        Some(base) if reference.sub.is_empty() => Some(base),
        Some(_base) => Some(
            property
                .resolve_element(&reference.sub)
                .map_err(|error| {
                    format!(
                        "attachment support '{}.{}' does not resolve: {error}",
                        reference.object, reference.sub
                    )
                })?,
        ),
        None => None,
    };
    // A Part primitive bakes its `Placement` into `Shape`; a Python feature's
    // shape is local (see the module docs), so only the latter is moved here.
    let shape = match shape {
        Some(shape) if object_registry::is_python_feature(&object.type_id) => {
            Some(backend().place(&shape, &placement).unwrap_or(shape))
        }
        other => other,
    };
    Ok(ResolvedSupport {
        shape,
        placement,
        origin: placement.base,
        whole_object: reference.sub.is_empty(),
    })
}

fn resolve_all(doc: &Document, id: ObjectId) -> Result<Vec<ResolvedSupport>, String> {
    let references = supports(doc, id);
    if references.is_empty() {
        return Err(
            "AttachmentSupport is empty: an active MapMode needs at least one support".to_string(),
        );
    }
    references
        .iter()
        .map(|reference| resolve_support(doc, reference))
        .collect()
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

fn unit_or_none(value: Vector3) -> Option<Vector3> {
    if value.length() > 1e-12 {
        Some(value.normalize())
    } else {
        None
    }
}

/// The unit normal of a planar face's *surface* (orientation ignored).
fn planar_surface_normal(shape: &Shape) -> Option<Vector3> {
    let backend = backend();
    if backend.surface_type(shape).as_deref() != Some("Part::GeomPlane") {
        return None;
    }
    let wire = backend.outer_wire(shape)?;
    let directions: Vec<Vector3> = backend
        .elements(&wire, ElementKind::Edge)
        .into_iter()
        .filter_map(|edge| backend.curve_of(&edge.shape).ok())
        .map(|curve| curve.tangent_at(0.0))
        .collect();
    for (index, first) in directions.iter().enumerate() {
        for second in directions.iter().skip(index + 1) {
            if let Some(normal) = unit_or_none(first.cross(second)) {
                return Some(normal);
            }
        }
    }
    None
}

/// The face normal at a point: the surface normal flipped when the face is reversed
/// (upstream's `Face.normalAt`).
fn face_normal(shape: &Shape) -> Option<Vector3> {
    let mut normal = planar_surface_normal(shape)?;
    if backend().orientation(shape) == Some("Reversed") {
        normal = normal.neg();
    }
    Some(normal)
}

/// A point on the face's supporting plane.
///
/// The kernel's `MassProperties` centre comes from the volume integral, which is
/// the origin for a face, so a planar face uses the average of its vertices (they
/// all lie on the plane) with the bounds centre as the fallback.
fn face_point(shape: &Shape) -> Option<Vector3> {
    let backend = backend();
    let vertices: Vec<Vector3> = backend
        .elements(shape, ElementKind::Vertex)
        .into_iter()
        .filter_map(|vertex| backend.vertex_point(&vertex.shape))
        .collect();
    if !vertices.is_empty() {
        let sum = vertices
            .iter()
            .fold(Vector3::zero(), |sum, point| sum.add(point));
        return Some(sum.scale(1.0 / vertices.len() as f64));
    }
    let (min, max) = backend.bounds(shape)?;
    Some(min.add(&max).scale(0.5))
}

fn project_onto_plane(point: Vector3, plane_point: Vector3, normal: Vector3) -> Vector3 {
    point.sub(&normal.scale(normal.dot(&point.sub(&plane_point))))
}

/// The three rotation columns of a placement.
fn placement_axes(placement: &Placement) -> (Vector3, Vector3, Vector3) {
    let matrix = placement.rotation.to_matrix();
    (
        matrix.transform(&Vector3::new(1.0, 0.0, 0.0)),
        matrix.transform(&Vector3::new(0.0, 1.0, 0.0)),
        matrix.transform(&Vector3::new(0.0, 0.0, 1.0)),
    )
}

/// Compose `a · b`, keeping the rotation (core's `Placement::mul` elides it).
fn compose(a: &Placement, b: &Placement) -> Placement {
    let rotated = a.rotation.to_matrix().transform(&b.base);
    Placement {
        base: a.base.add(&rotated),
        rotation: a.rotation.multiply(&b.rotation),
    }
}

/// The placement with `z` as its third axis, `x` (when given) as its first, and
/// `origin` as its position.
///
/// This is upstream's `AttachEngine::placementFactory` for the non-reversed case.
/// `x = None` means "make Y point along global Z where possible", which is how every
/// mode that has no natural X axis behaves. `legacy_orientation` is `FlatFace`'s
/// pre-0.17 rule: pick X from the support object's axes instead.
fn placement_from_axes(
    origin: Vector3,
    z: Vector3,
    x: Option<Vector3>,
    legacy_orientation: bool,
    support: Option<&Placement>,
) -> Result<Placement, String> {
    let z = unit_or_none(z).ok_or_else(|| "the attachment normal is degenerate".to_string())?;
    let x = match x.and_then(unit_or_none) {
        Some(x) => x,
        None if legacy_orientation => {
            let place = support.ok_or_else(|| {
                "FlatFace needs the support's placement for its orientation".to_string()
            })?;
            let (dx, dy, dz) = placement_axes(place);
            let cos_x = z.dot(&dx);
            let cos_y = z.dot(&dy);
            let cos_z = z.dot(&dz);
            let ax = cos_x.abs();
            let ay = cos_y.abs();
            let az = cos_z.abs();
            if ax >= ay && ax >= az {
                if cos_x > 0.0 {
                    dy
                } else {
                    dy.neg()
                }
            } else if ay >= az {
                if cos_y > 0.0 {
                    dx.neg()
                } else {
                    dx
                }
            } else {
                dx
            }
        }
        None => {
            // Align Y along global Z: X = Z_global × Z_axis.
            let candidate = Vector3::new(0.0, 0.0, 1.0).cross(&z);
            match unit_or_none(candidate) {
                Some(x) => x,
                // The normal is the global Z axis: pick ±X by its sign, as upstream.
                None => Vector3::new(1.0, 0.0, 0.0).scale(z.z),
            }
        }
    };
    let y = z.cross(&x);
    let matrix = ferrocad_core::Matrix4::from_values([
        x.x, y.x, z.x, 0.0, x.y, y.y, z.y, 0.0, x.z, y.z, z.z, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);
    Ok(Placement {
        base: origin,
        rotation: Rotation::from_matrix(&matrix),
    })
}

/// The point and tangent of `curve` at the attachment parameter `parameter`.
fn point_and_tangent(curve: &ferrocad_curves::EdgeCurve, parameter: f64) -> (Vector3, Vector3) {
    use ferrocad_curves::EdgeCurve;
    let t = parameter.clamp(0.0, 1.0);
    match curve {
        EdgeCurve::Line(_) => (curve.point_at(t), curve.tangent_at(t)),
        EdgeCurve::LineSegment(_) => (curve.point_at(t), curve.tangent_at(t)),
        EdgeCurve::Circle(_) => (curve.point_at(t * std::f64::consts::TAU), curve.tangent_at(t * std::f64::consts::TAU)),
        EdgeCurve::Arc(arc) => {
            let angle = arc.first() + t * (arc.last() - arc.first());
            (arc.point_at(angle), arc.tangent_at(angle))
        }
        EdgeCurve::Ellipse(_) => (
            curve.point_at(t * std::f64::consts::TAU),
            curve.tangent_at(t * std::f64::consts::TAU),
        ),
        EdgeCurve::EllipticalArc(arc) => {
            let angle = arc.first() + t * (arc.last() - arc.first());
            (arc.point_at(angle), arc.tangent_at(angle))
        }
        // The remaining analytic kinds use the edge parameter directly.
        _ => (curve.point_at(t), curve.tangent_at(t)),
    }
}

fn edge_curve(shape: &Shape, what: &str) -> Result<ferrocad_curves::EdgeCurve, String> {
    backend()
        .curve_of(shape)
        .map_err(|_| format!("{what} is not an analytic edge the attacher can read"))
}

/// `ObjectXY`-like axes from a whole support object's placement, or from a conic
/// edge's own frame.
fn object_axes(support: &ResolvedSupport) -> Result<(Vector3, Vector3, Vector3, Vector3), String> {
    let shape = support
        .shape
        .as_ref()
        .ok_or_else(|| "the attachment support has an empty shape".to_string())?;
    if support.whole_object {
        let (x, y, z) = placement_axes(&support.placement);
        return Ok((x, y, z, support.origin));
    }
    let curve = edge_curve(shape, "the attachment support")?;
    match curve {
        ferrocad_curves::EdgeCurve::Circle(circle) => Ok((
            circle.x_axis(),
            circle.y_axis(),
            circle.normal(),
            circle.center(),
        )),
        ferrocad_curves::EdgeCurve::Arc(arc) => {
            let circle = arc.circle();
            Ok((
                circle.x_axis(),
                circle.y_axis(),
                circle.normal(),
                circle.center(),
            ))
        }
        ferrocad_curves::EdgeCurve::Ellipse(ellipse) => Ok((
            ellipse.x_axis(),
            ellipse.y_axis(),
            ellipse.normal(),
            ellipse.center(),
        )),
        ferrocad_curves::EdgeCurve::EllipticalArc(arc) => {
            let ellipse = arc.ellipse();
            Ok((
                ellipse.x_axis(),
                ellipse.y_axis(),
                ellipse.normal(),
                ellipse.center(),
            ))
        }
        _ => Err(
            "ObjectXY/ObjectXZ/ObjectYZ need a conic section edge or a whole object"
                .to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// The mode switch
// ---------------------------------------------------------------------------

fn calculate_placement(
    doc: &Document,
    id: ObjectId,
    mode: &str,
) -> Result<Placement, String> {
    let supports = resolve_all(doc, id)?;
    let parameter = map_path_parameter(doc, id);
    let offset = attachment_offset(doc, id);
    let first = supports.first().expect("resolve_all rejects an empty list");

    let placement = match mode {
        "Translate" => {
            let point = vertex_point(first, "Translate")?;
            let orig = placement_property(doc, id).unwrap_or_else(Placement::identity);
            // Upstream applies only the offset's position here and keeps the
            // original rotation; the common `* AttachmentOffset` is skipped.
            return Ok(Placement {
                base: point.add(&offset.base),
                rotation: orig.rotation,
            });
        }
        "ObjectXY" | "ObjectXZ" | "ObjectYZ" => {
            let (x, y, z, base) = object_axes(first)?;
            let (normal, x_axis) = match mode {
                "ObjectXY" => (z, x),
                "ObjectXZ" => (y.neg(), x),
                _ => (x, y),
            };
            placement_from_axes(base, normal, Some(x_axis), false, Some(&first.placement))?
        }
        "FlatFace" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| "FlatFace needs a support shape".to_string())?;
            let normal = face_normal(shape).ok_or_else(|| {
                "FlatFace needs a planar face support; the linked shape (or its \
                 sub-element) is not planar"
                    .to_string()
            })?;
            let plane_point = face_point(shape)
                .ok_or_else(|| "FlatFace cannot locate the support face".to_string())?;
            let origin = project_onto_plane(first.origin, plane_point, normal);
            placement_from_axes(origin, normal, None, true, Some(&first.placement))?
        }
        "NormalToEdge" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| "NormalToEdge needs an edge support".to_string())?;
            let curve = edge_curve(shape, "the NormalToEdge support")?;
            let (point, tangent) = point_and_tangent(&curve, parameter);
            placement_from_axes(point, tangent.neg(), None, false, None)?
        }
        "ThreePointsPlane" | "ThreePointsNormal" => {
            let points = three_points(&supports)?;
            let v01 = points[1].sub(&points[0]);
            let v02 = points[2].sub(&points[0]);
            let normal = if mode == "ThreePointsPlane" {
                unit_or_none(v01.cross(&v02)).ok_or_else(|| {
                    "ThreePointsPlane needs three non-collinear points".to_string()
                })?
            } else {
                unit_or_none(v02.sub(&v01.scale(v02.dot(&v01))))
                    .map(|normal| normal.neg())
                    .ok_or_else(|| {
                        "ThreePointsNormal needs three non-collinear points".to_string()
                    })?
            };
            let origin = if mode == "ThreePointsPlane" {
                points[0].add(&points[1]).add(&points[2]).scale(1.0 / 3.0)
            } else {
                project_onto_plane(points[2], points[0], normal)
            };
            placement_from_axes(origin, normal, None, false, None)?
        }
        "InertialCS" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| "InertialCS needs a support shape".to_string())?;
            let properties = backend()
                .principal_properties(shape)
                .ok_or_else(|| "InertialCS cannot measure the support".to_string())?;
            let center = backend()
                .mass_properties(shape)
                .map(|mass| mass.center_of_mass)
                .ok_or_else(|| "InertialCS cannot measure the support".to_string())?;
            if properties.symmetry_point {
                return Err(
                    "InertialCS: the inertia tensor is trivial, so the principal axes are \
                     undefined"
                        .to_string(),
                );
            }
            let (first_axis, second_axis, third_axis) = properties.axes;
            let (i1, i2, i3) = properties.moments;
            let (d12, d23, d31) = ((i1 - i2).abs(), (i2 - i3).abs(), (i3 - i1).abs());
            let (z, x) = if properties.symmetry_axis {
                // One moment differs: its axis is the defined one, and X is a guess.
                let z = if d12 < d23 && d12 < d31 {
                    third_axis
                } else if d23 < d31 && d23 < d12 {
                    first_axis
                } else {
                    second_axis
                };
                (z, None)
            } else {
                (first_axis, Some(second_axis))
            };
            placement_from_axes(center, z, x, false, None)?
        }
        "ObjectX" | "ObjectY" | "ObjectZ" => {
            let (x, y, z, base) = object_axes(first)?;
            // The line engine reuses the matching 3D axis mode.
            let (line, x_axis) = match mode {
                "ObjectX" => (x, y),
                "ObjectY" => (y.neg(), x),
                _ => (z, x),
            };
            placement_from_axes(base, line, Some(x_axis), false, Some(&first.placement))?
        }
        "ObjectNormal" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| "ObjectNormal needs an edge support".to_string())?;
            let curve = edge_curve(shape, "the ObjectNormal support")?;
            let (point, tangent) = point_and_tangent(&curve, parameter);
            line_placement(first.origin, point, tangent)?
        }
        "TwoPointLine" => {
            let (start, end) = two_points(&supports)?;
            let direction = end.sub(&start);
            line_placement(first.origin, start, direction)?
        }
        "AxisOfInertia1" | "AxisOfInertia2" | "AxisOfInertia3" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| format!("{mode} needs a support shape"))?;
            let properties = backend()
                .principal_properties(shape)
                .ok_or_else(|| format!("{mode} cannot measure the support"))?;
            let center = backend()
                .mass_properties(shape)
                .map(|mass| mass.center_of_mass)
                .ok_or_else(|| format!("{mode} cannot measure the support"))?;
            let direction = match mode {
                "AxisOfInertia1" => properties.axes.0,
                "AxisOfInertia2" => properties.axes.1,
                _ => properties.axes.2,
            };
            line_placement(first.origin, center, direction)?
        }
        "Vertex" => {
            let point = vertex_point(first, "Vertex")?;
            point_placement(point)?
        }
        "CenterOfMass" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| "CenterOfMass needs a support shape".to_string())?;
            let point = backend()
                .mass_properties(shape)
                .map(|mass| mass.center_of_mass)
                .ok_or_else(|| "CenterOfMass cannot measure the support".to_string())?;
            point_placement(point)?
        }
        "Focus1" | "Focus2" => {
            let shape = first
                .shape
                .as_ref()
                .ok_or_else(|| format!("{mode} needs an ellipse support"))?;
            let curve = edge_curve(shape, "the focus support")?;
            let ellipse = match curve {
                ferrocad_curves::EdgeCurve::Ellipse(ellipse) => ellipse,
                ferrocad_curves::EdgeCurve::EllipticalArc(arc) => arc.ellipse(),
                _ => return Err(format!("{mode} needs an ellipse support")),
            };
            point_placement(focus_point(&ellipse, mode == "Focus1")?)?
        }
        "ObjectOrigin" | "OnEdge" => {
            // Upstream reuses the 3D ObjectXY / NormalToEdge math for these.
            if mode == "ObjectOrigin" {
                let (x, _y, z, base) = object_axes(first)?;
                placement_from_axes(base, z, Some(x), false, Some(&first.placement))?
            } else {
                let shape = first
                    .shape
                    .as_ref()
                    .ok_or_else(|| "OnEdge needs an edge support".to_string())?;
                let curve = edge_curve(shape, "the OnEdge support")?;
                let (point, tangent) = point_and_tangent(&curve, parameter);
                placement_from_axes(point, tangent.neg(), None, false, None)?
            }
        }
        other => {
            return Err(format!(
                "attachment mode '{other}' is not implemented (supported: {} )",
                ALL_MODES
                    .iter()
                    .filter(|mode| implemented(mode))
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };
    Ok(compose(&placement, &offset))
}

/// Whether [`calculate_placement`] handles `mode`.
fn implemented(mode: &str) -> bool {
    matches!(
        mode,
        "Deactivated"
            | "Translate"
            | "ObjectXY"
            | "ObjectXZ"
            | "ObjectYZ"
            | "FlatFace"
            | "NormalToEdge"
            | "ThreePointsPlane"
            | "ThreePointsNormal"
            | "InertialCS"
            | "ObjectX"
            | "ObjectY"
            | "ObjectZ"
            | "ObjectNormal"
            | "TwoPointLine"
            | "AxisOfInertia1"
            | "AxisOfInertia2"
            | "AxisOfInertia3"
            | "Vertex"
            | "CenterOfMass"
            | "Focus1"
            | "Focus2"
            | "ObjectOrigin"
            | "OnEdge"
    )
}

fn point_placement(point: Vector3) -> Result<Placement, String> {
    placement_from_axes(point, Vector3::new(0.0, 0.0, 1.0), Some(Vector3::new(1.0, 0.0, 0.0)), false, None)
}

fn line_placement(origin: Vector3, base: Vector3, direction: Vector3) -> Result<Placement, String> {
    let direction = unit_or_none(direction)
        .ok_or_else(|| "the attachment line direction is degenerate".to_string())?;
    // `useRefOrg_Line`: move the origin to the projection of the reference object's
    // origin onto the line.
    let centred = base.add(&direction.scale(direction.dot(&origin.sub(&base))));
    placement_from_axes(centred, direction, None, false, None)
}

fn vertex_point(support: &ResolvedSupport, mode: &str) -> Result<Vector3, String> {
    let shape = support
        .shape
        .as_ref()
        .ok_or_else(|| format!("{mode} needs a vertex support"))?;
    let backend = backend();
    if let Some(point) = backend.vertex_point(shape) {
        return Ok(point);
    }
    // Upstream accepts a bounded edge for `Vertex` and uses its start point.
    if let Ok(curve) = backend.curve_of(shape) {
        if let Some((start, _)) = curve.endpoints() {
            return Ok(start);
        }
    }
    Err(format!("{mode} needs a vertex support"))
}

/// Two points from the supports: two vertices, or a line's endpoints.
fn two_points(supports: &[ResolvedSupport]) -> Result<(Vector3, Vector3), String> {
    let shape_of = |support: &ResolvedSupport| -> Option<Shape> { support.shape.clone() };
    if supports.len() >= 2 {
        if let (Some(first), Some(second)) = (shape_of(&supports[0]), shape_of(&supports[1])) {
            if backend().vertex_point(&first).is_some() && backend().vertex_point(&second).is_some()
            {
                return Ok((
                    backend().vertex_point(&first).unwrap(),
                    backend().vertex_point(&second).unwrap(),
                ));
            }
        }
    }
    let shape = shape_of(&supports[0])
        .ok_or_else(|| "TwoPointLine needs two vertices or one line".to_string())?;
    let curve = edge_curve(&shape, "the TwoPointLine support")?;
    curve
        .endpoints()
        .ok_or_else(|| "TwoPointLine needs bounded geometry".to_string())
}

/// Three points from the supports: vertices, or a line's endpoints expanded.
fn three_points(supports: &[ResolvedSupport]) -> Result<[Vector3; 3], String> {
    let mut points = Vec::new();
    for support in supports {
        let Some(shape) = support.shape.as_ref() else {
            continue;
        };
        if let Some(point) = backend().vertex_point(shape) {
            points.push(point);
            continue;
        }
        if let Ok(curve) = backend().curve_of(shape) {
            if let Some((start, end)) = curve.endpoints() {
                points.push(start);
                points.push(end);
            }
        }
        if points.len() >= 3 {
            break;
        }
    }
    if points.len() < 3 {
        return Err("this mode needs three points (three vertices, or a line and a point)"
            .to_string());
    }
    Ok([points[0], points[1], points[2]])
}

fn focus_point(ellipse: &ferrocad_curves::Ellipse, first: bool) -> Result<Vector3, String> {
    let a = ellipse.major_radius();
    let b = ellipse.minor_radius();
    if a <= b {
        return Err("a circle has a single focus; the mode needs an ellipse".to_string());
    }
    let distance = (a * a - b * b).sqrt();
    let direction = ellipse.x_axis().scale(distance);
    Ok(if first {
        ellipse.center().sub(&direction)
    } else {
        ellipse.center().add(&direction)
    })
}

// ---------------------------------------------------------------------------
// The public entry points
// ---------------------------------------------------------------------------

/// `positionBySupport()`: recompute `Placement` from the support and the mode.
///
/// Returns `Ok(false)` when the attachment is deactivated (the placement is left
/// alone), `Ok(true)` after writing a new placement, and `Err` with a reason when an
/// active mode cannot be satisfied. A failed attachment leaves the previous
/// placement untouched, as upstream does before it rethrows.
pub fn position_by_support(doc: &mut Document, id: ObjectId) -> Result<bool, String> {
    let Some(mode) = mode_name(doc, id) else {
        return Ok(false);
    };
    if mode == MODE_DEACTIVATED {
        return Ok(false);
    }
    let placement = calculate_placement(doc, id, &mode)?;
    doc.set_property_raw(id, "Placement", Property::Placement(placement));
    Ok(true)
}

/// The placement `position_by_support` would write, without touching the document.
pub fn attached_placement(doc: &Document, id: ObjectId) -> Result<Option<Placement>, String> {
    let Some(mode) = mode_name(doc, id) else {
        return Ok(None);
    };
    if mode == MODE_DEACTIVATED {
        return Ok(None);
    }
    Ok(Some(calculate_placement(doc, id, &mode)?))
}

/// Register the extension with the Python binding.
///
/// The properties are installed by `addExtension(name)` via [`provide`]; the two
/// methods are dispatched by `ferrocad_py` from the object's extension set. Both
/// names are registered: a script adds the Python template
/// (`Part::AttachExtensionPython`), while `Part::Part2DObjectPython` carries the
/// static base extension (`Part::AttachExtension`).
pub fn register() {
    for extension in [EXTENSION, BASE_EXTENSION] {
        ferrocad_py::register_extension_provider(extension, provide);
        ferrocad_py::register_extension_method(
            extension,
            "changeAttacherType",
            change_attacher_type_method,
        );
        ferrocad_py::register_extension_method(
            extension,
            "positionBySupport",
            position_by_support_method,
        );
    }
}

/// The first positional argument of an extension-method call.
fn first_string_argument<'py>(args: &Bound<'py, PyAny>, method: &str) -> PyResult<String> {
    let tuple = args
        .downcast::<PyTuple>()
        .map_err(|_| PyTypeError::new_err(format!("{method} expects positional arguments")))?;
    let item = tuple.get_item(0).map_err(|_| {
        PyTypeError::new_err(format!("{method} expects one string argument"))
    })?;
    item.extract::<String>().map_err(|_| {
        PyTypeError::new_err(format!("{method} expects one string argument"))
    })
}

/// `changeAttacherType(name) -> bool`.
fn change_attacher_type_method(
    doc: &mut Document,
    id: ObjectId,
    method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let py = args.py();
    let name = first_string_argument(args, method)?;
    let changed = change_attacher_type(doc, id, &name).map_err(PyValueError::new_err)?;
    Ok(PyBool::new(py, changed).to_owned().into_any().unbind())
}

/// `positionBySupport() -> bool`.
fn position_by_support_method(
    doc: &mut Document,
    id: ObjectId,
    _method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let py = args.py();
    let attached = position_by_support(doc, id).map_err(PyValueError::new_err)?;
    Ok(PyBool::new(py, attached).to_owned().into_any().unbind())
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENGINE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Register the OCCT backend and the Python-feature object types once.
    fn setup() {
        use std::sync::Once;
        static SETUP: Once = Once::new();
        SETUP.call_once(|| {
            ferrocad_part::register(std::sync::Arc::new(ferrocad_occt::OcctBackend::new()));
            crate::feature_python::register();
        });
    }

    fn add_support(
        doc: &mut Document,
        name: &str,
        shape: Shape,
        placement: Placement,
    ) -> ObjectId {
        let id = doc.add_object(name, "Part::Feature");
        doc.set_property_raw(
            id,
            "Shape",
            ferrocad_part::make_shape_property(ferrocad_part::ShapeProperty::new(shape)),
        );
        doc.set_property_raw(id, "Placement", Property::Placement(placement));
        id
    }

    fn add_target(doc: &mut Document) -> ObjectId {
        doc.add_object("Target", "Part::Part2DObjectPython")
    }

    fn attach_to(doc: &mut Document, target: ObjectId, supports: &[&str], mode: &str) {
        doc.set_property_raw(
            target,
            SUPPORT,
            Property::LinkSubList(
                supports
                    .iter()
                    .map(|name| (name.to_string(), vec![String::new()]))
                    .collect(),
            ),
        );
        doc.set_property_raw(
            target,
            MAP_MODE,
            Property::Enumeration(modes_3d(), 0),
        );
        doc.set_property_raw(
            target,
            MAP_MODE,
            Property::Enumeration(vec![mode.to_string()], 0),
        );
    }

    fn placement_of(doc: &Document, id: ObjectId) -> Placement {
        placement_property(doc, id).expect("Placement")
    }

    fn plane_at(z: f64) -> Shape {
        let backend = backend();
        let corners = [
            Vector3::new(-1.0, -1.0, z),
            Vector3::new(1.0, -1.0, z),
            Vector3::new(1.0, 1.0, z),
            Vector3::new(-1.0, 1.0, z),
        ];
        let mut edges = Vec::new();
        for index in 0..4 {
            let segment = ferrocad_curves::LineSegment::new(
                corners[index],
                corners[(index + 1) % 4],
            )
            .expect("segment");
            edges.push(
                backend
                    .make_edge(&ferrocad_curves::EdgeCurve::LineSegment(segment))
                    .expect("edge"),
            );
        }
        let wire = backend.make_wire(&edges).expect("wire");
        backend.make_face(&[wire]).expect("face")
    }

    fn identity_axes(placement: &Placement) -> (Vector3, Vector3, Vector3) {
        placement_axes(placement)
    }

    #[test]
    fn object_xy_places_the_local_xy_plane_on_the_support_plane() {
        let placement =
            placement_from_axes(Vector3::new(1.0, 2.0, 3.0), Vector3::new(0.0, 0.0, 1.0), Some(Vector3::new(1.0, 0.0, 0.0)), false, None)
                .unwrap();
        assert_eq!(placement.base, Vector3::new(1.0, 2.0, 3.0));
        let (x, y, z) = identity_axes(&placement);
        assert!(x.is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-12), "{x:?}");
        assert!(y.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-12), "{y:?}");
        assert!(z.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-12), "{z:?}");
    }

    #[test]
    fn a_missing_x_axis_aligns_y_with_global_z() {
        // Normal along +X: X = Z_global × X = +Y, Y = X × Y = +Z.
        let placement =
            placement_from_axes(Vector3::zero(), Vector3::new(1.0, 0.0, 0.0), None, false, None)
                .unwrap();
        let (x, y, z) = identity_axes(&placement);
        assert!(x.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-12), "{x:?}");
        assert!(y.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-12), "{y:?}");
        assert!(z.is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-12), "{z:?}");
    }

    #[test]
    fn compose_keeps_the_rotation() {
        let a = Placement::new(
            Vector3::new(1.0, 0.0, 0.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        let b = Placement::new(
            Vector3::new(2.0, 0.0, 0.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        let composed = compose(&a, &b);
        assert!(composed
            .base
            .is_equal(&Vector3::new(1.0, 2.0, 0.0), 1e-12));
        assert!((composed.rotation.angle() - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn the_default_mode_is_deactivated() {
        let properties = attachment_properties(ATTACHER_3D);
        let mode = properties
            .iter()
            .find(|(name, _, _)| name == MAP_MODE)
            .map(|(_, value, _)| value)
            .unwrap();
        match mode {
            Property::Enumeration(choices, index) => {
                assert_eq!(*index, 0);
                assert_eq!(choices.first().map(String::as_str), Some(MODE_DEACTIVATED));
            }
            other => panic!("MapMode is not an enumeration: {other:?}"),
        }
    }

    #[test]
    fn every_engine_has_deactivated_first() {
        for attacher in [ATTACHER_3D, ATTACHER_PLANE, ATTACHER_LINE, ATTACHER_POINT] {
            let modes = modes_for(attacher);
            assert_eq!(modes.first().map(String::as_str), Some(MODE_DEACTIVATED));
            assert!(modes.contains(&"Deactivated".to_string()));
        }
    }

    #[test]
    fn flat_face_projects_the_support_origin_onto_the_face() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        add_support(&mut doc, "Support", plane_at(5.0), Placement::identity());
        let target = add_target(&mut doc);
        attach_to(&mut doc, target, &["Support"], "FlatFace");

        assert!(position_by_support(&mut doc, target).unwrap());
        let placement = placement_of(&doc, target);
        assert!(placement.base.is_equal(&Vector3::new(0.0, 0.0, 5.0), 1e-9), "{:?}", placement.base);
        let (x, y, z) = identity_axes(&placement);
        assert!(z.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9), "{z:?}");
        assert!(x.is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-9), "{x:?}");
        assert!(y.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9), "{y:?}");

        // The offset is applied in the attached frame, on top of the projection.
        doc.set_property_raw(
            target,
            OFFSET,
            Property::Placement(Placement::new(
                Vector3::new(1.0, 2.0, 3.0),
                Rotation::identity(),
            )),
        );
        assert!(position_by_support(&mut doc, target).unwrap());
        let placement = placement_of(&doc, target);
        assert!(placement.base.is_equal(&Vector3::new(1.0, 2.0, 8.0), 1e-9), "{:?}", placement.base);
    }

    #[test]
    fn object_xy_uses_the_support_objects_frame() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let support_placement = Placement::new(
            Vector3::new(10.0, 20.0, 30.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        add_support(
            &mut doc,
            "Support",
            backend().make_box(1.0, 1.0, 1.0).expect("box"),
            support_placement,
        );
        let target = add_target(&mut doc);
        attach_to(&mut doc, target, &["Support"], "ObjectXY");

        assert!(position_by_support(&mut doc, target).unwrap());
        let placement = placement_of(&doc, target);
        assert!(placement.base.is_equal(&Vector3::new(10.0, 20.0, 30.0), 1e-9));
        let (x, y, z) = identity_axes(&placement);
        assert!(x.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9), "{x:?}");
        assert!(y.is_equal(&Vector3::new(-1.0, 0.0, 0.0), 1e-9), "{y:?}");
        assert!(z.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9), "{z:?}");
    }

    #[test]
    fn vertex_and_two_point_line() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let backend = backend();
        add_support(
            &mut doc,
            "V1",
            backend.make_vertex(Vector3::new(0.0, 1.0, 0.0)).expect("vertex"),
            Placement::identity(),
        );
        add_support(
            &mut doc,
            "V2",
            backend.make_vertex(Vector3::new(2.0, 1.0, 0.0)).expect("vertex"),
            Placement::identity(),
        );

        let vertex_target = add_target(&mut doc);
        attach_to(&mut doc, vertex_target, &["V1"], "Vertex");
        assert!(position_by_support(&mut doc, vertex_target).unwrap());
        assert!(placement_of(&doc, vertex_target)
            .base
            .is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));

        let line_target = add_target(&mut doc);
        attach_to(&mut doc, line_target, &["V1", "V2"], "TwoPointLine");
        assert!(position_by_support(&mut doc, line_target).unwrap());
        let placement = placement_of(&doc, line_target);
        let (_, _, z) = identity_axes(&placement);
        assert!(z.is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-9), "{z:?}");
    }

    #[test]
    fn object_normal_follows_the_edge_tangent() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let backend = backend();
        let segment = ferrocad_curves::LineSegment::new(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 1.0, 0.0),
        )
        .expect("segment");
        add_support(
            &mut doc,
            "Edge",
            backend
                .make_edge(&ferrocad_curves::EdgeCurve::LineSegment(segment))
                .expect("edge"),
            Placement::identity(),
        );
        let target = add_target(&mut doc);
        attach_to(&mut doc, target, &["Edge"], "ObjectNormal");

        assert!(position_by_support(&mut doc, target).unwrap());
        let (_, _, z) = identity_axes(&placement_of(&doc, target));
        assert!(
            z.is_equal(&Vector3::new(1.0, 1.0, 0.0).normalize(), 1e-9),
            "{z:?}"
        );
    }

    #[test]
    fn an_unsatisfiable_mode_is_an_error_and_keeps_the_placement() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        add_support(
            &mut doc,
            "Support",
            backend().make_box(1.0, 1.0, 1.0).expect("box"),
            Placement::identity(),
        );
        let target = add_target(&mut doc);
        attach_to(&mut doc, target, &["Support"], "FlatFace");
        let before = Placement::new(Vector3::new(7.0, 8.0, 9.0), Rotation::identity());
        doc.set_property_raw(target, "Placement", Property::Placement(before));

        let error = position_by_support(&mut doc, target).unwrap_err();
        assert!(error.contains("planar"), "{error}");
        assert!(placement_of(&doc, target).base.is_equal(&Vector3::new(7.0, 8.0, 9.0), 1e-12));

        // An active mode with no support is the same failure.
        doc.set_property_raw(target, SUPPORT, Property::LinkSubList(vec![]));
        let error = position_by_support(&mut doc, target).unwrap_err();
        assert!(error.contains("empty"), "{error}");
    }

    #[test]
    fn deactivated_leaves_the_placement_alone() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let target = add_target(&mut doc);
        let before = Placement::new(Vector3::new(1.0, 2.0, 3.0), Rotation::identity());
        doc.set_property_raw(target, "Placement", Property::Placement(before));
        assert!(!position_by_support(&mut doc, target).unwrap());
        assert!(placement_of(&doc, target).base.is_equal(&Vector3::new(1.0, 2.0, 3.0), 1e-12));
    }

    #[test]
    fn the_support_list_keeps_per_entry_subnames() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        add_support(
            &mut doc,
            "Support",
            backend().make_box(1.0, 1.0, 1.0).expect("box"),
            Placement::identity(),
        );
        let target = add_target(&mut doc);
        doc.set_property_raw(
            target,
            SUPPORT,
            Property::LinkSubList(vec![
                ("Support".to_string(), vec!["Face1".to_string(), "Face2".to_string()]),
                ("Other".to_string(), vec![String::new()]),
            ]),
        );
        let references = supports(&doc, target);
        assert_eq!(references.len(), 3, "one per sub-element plus the whole object");
        assert_eq!(references[0].object, "Support");
        assert_eq!(references[0].sub, "Face1");
        assert_eq!(references[1].sub, "Face2");
        assert_eq!(references[2].object, "Other");
        assert!(references[2].sub.is_empty(), "an empty subname is the whole object");

        // An entry with no subnames is also a whole-object reference.
        doc.set_property_raw(
            target,
            SUPPORT,
            Property::LinkSubList(vec![("Support".to_string(), vec![])]),
        );
        let references = supports(&doc, target);
        assert_eq!(references.len(), 1);
        assert!(references[0].sub.is_empty());
    }

    #[test]
    fn the_provider_adds_the_property_set_once() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Scripted", "Part::FeaturePython");
        assert!(!has_attachment(&doc, id));

        assert!(provide(&mut doc, id));
        assert!(has_attachment(&doc, id));
        assert!(doc.has_extension(id, EXTENSION));
        assert!(
            doc.has_extension(id, BASE_EXTENSION),
            "the Python template reports its base extension"
        );
        for name in [SUPPORT, MAP_MODE, MAP_PATH_PARAMETER, OFFSET, ATTACHER_TYPE] {
            assert!(
                doc.object(id).unwrap().properties.get(name).is_some(),
                "{name} was added"
            );
        }
        assert_eq!(
            property_string(&doc, id, ATTACHER_TYPE).as_deref(),
            Some(ATTACHER_3D)
        );

        // The script's value survives a second `addExtension`.
        doc.set_property_raw(id, MAP_MODE, Property::Enumeration(modes_3d(), 3));
        assert!(provide(&mut doc, id));
        match doc.object(id).unwrap().properties.get(MAP_MODE) {
            Some(Property::Enumeration(_, index)) => assert_eq!(*index, 3),
            other => panic!("MapMode changed: {other:?}"),
        }
    }

    #[test]
    fn change_attacher_type_resets_the_mode_and_offers_its_modes() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Scripted", "Part::FeaturePython");
        assert!(provide(&mut doc, id));
        doc.set_property_raw(id, MAP_MODE, Property::Enumeration(modes_3d(), 5));

        assert!(change_attacher_type(&mut doc, id, ATTACHER_PLANE).unwrap());
        assert_eq!(
            property_string(&doc, id, ATTACHER_TYPE).as_deref(),
            Some(ATTACHER_PLANE)
        );
        match doc.object(id).unwrap().properties.get(MAP_MODE) {
            Some(Property::Enumeration(modes, index)) => {
                assert_eq!(*index, 0, "the mode is reset to Deactivated");
                assert_eq!(modes.first().map(String::as_str), Some(MODE_DEACTIVATED));
                assert!(modes.iter().any(|mode| mode == "FlatFace"));
            }
            other => panic!("MapMode is not an enumeration: {other:?}"),
        }

        // Changing to the same engine is a no-op, as upstream's `setAttacher`.
        assert!(!change_attacher_type(&mut doc, id, ATTACHER_PLANE).unwrap());
        // An unknown engine is refused.
        let error = change_attacher_type(&mut doc, id, "Attacher::Nope").unwrap_err();
        assert!(error.contains("Attacher::Nope"), "{error}");
    }

    #[test]
    fn a_part2d_object_carries_the_property_set_by_default() {
        let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        setup();
        let mut doc = Document::new();
        let id = add_target(&mut doc);
        assert!(has_attachment(&doc, id));
        assert_eq!(
            property_string(&doc, id, ATTACHER_TYPE).as_deref(),
            Some(ATTACHER_PLANE)
        );
        match doc.object(id).unwrap().properties.get(MAP_MODE) {
            Some(Property::Enumeration(modes, index)) => {
                assert_eq!(*index, 0);
                assert!(modes.iter().any(|mode| mode == "FlatFace"));
            }
            other => panic!("MapMode is not an enumeration: {other:?}"),
        }
    }
}

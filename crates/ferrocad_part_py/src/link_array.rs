//! `Part::LinkArray{Point,Linear,Path,Circular}` document objects.
//!
//! Upstream these are `App::Link` subclasses whose `execute` fills
//! `PlacementList` (and `ElementList` when `ShowElement` is set) from a pattern
//! extension. FerroCAD has no `App::Link` base object yet, so each array is a
//! self-contained registered [`ObjectType`] carrying the same properties and
//! computing the same placements from the upstream sources in
//! `src/Mod/Part/App/*PatternExtension.cpp`.
//!
//! The supporting types `Part::LinkArrayPolar` (used by the circular tests) and
//! `Part::Part2DObject` (the base sketch object the axis tests use) are registered
//! here too; without them those tests cannot even construct their fixtures.

use std::sync::Arc;

use ferrocad_core::geometry::{Matrix4, Placement, Rotation, Vector3};
use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::quantity::Quantity;
use ferrocad_core::unit::Unit;
use ferrocad_core::{prop_status, Document, ObjectId, Property};
use ferrocad_curves::EdgeCurve;
use ferrocad_geom::{ElementKind, GeometryBackend, Shape};
use ferrocad_part::{make_shape_property, shape_of, ShapeProperty};

/// `Part::LinkArrayPoint` (`PointPatternExtension`).
pub const POINT_TYPE: &str = "Part::LinkArrayPoint";
/// `Part::LinkArrayLinear` (`LinearPatternExtension`).
pub const LINEAR_TYPE: &str = "Part::LinkArrayLinear";
/// `Part::LinkArrayPath` (`PathPatternExtension`).
pub const PATH_TYPE: &str = "Part::LinkArrayPath";
/// `Part::LinkArrayCircular` (`CircularPatternExtension`).
pub const CIRCULAR_TYPE: &str = "Part::LinkArrayCircular";
/// `Part::LinkArrayPolar` (`PolarPatternExtension`); a sibling needed by the
/// upstream circular test's save/restore case.
pub const POLAR_TYPE: &str = "Part::LinkArrayPolar";
/// The generated child of an array when `ShowElement` is true (upstream's
/// `App::Link` element).
pub const ELEMENT_TYPE: &str = "Part::LinkArrayElement";
/// A minimal `Part::Part2DObject` (sketch base) for the axis-reference tests.
pub const PART2D_TYPE: &str = "Part::Part2DObject";

const MAX_OCCURRENCES: usize = 10_000;
const CONFUSION: f64 = 1e-7;

/// Register every link-array type. Idempotent.
pub(crate) fn register() {
    object_registry::register(POINT_TYPE, Arc::new(PointArray));
    object_registry::register(LINEAR_TYPE, Arc::new(LinearArray));
    object_registry::register(PATH_TYPE, Arc::new(PathArray));
    object_registry::register(CIRCULAR_TYPE, Arc::new(CircularArray));
    object_registry::register(POLAR_TYPE, Arc::new(PolarArray));
    object_registry::register(ELEMENT_TYPE, Arc::new(Element));
    object_registry::register(PART2D_TYPE, Arc::new(Part2DObject));
}

// ---------------------------------------------------------------------------
// Document property access
// ---------------------------------------------------------------------------

fn property<'a>(doc: &'a Document, id: ObjectId, name: &str) -> Option<&'a Property> {
    doc.object(id)?.properties.get(name)
}

fn int_property(doc: &Document, id: ObjectId, name: &str, default: i64) -> i64 {
    match property(doc, id, name) {
        Some(Property::Integer(v)) => *v,
        Some(Property::IntegerConstraint { value, .. }) => *value,
        Some(Property::Float(v)) => *v as i64,
        _ => default,
    }
}

fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    match property(doc, id, name) {
        Some(Property::Float(v)) => *v,
        Some(Property::Integer(v)) => *v as f64,
        Some(Property::Quantity(q)) => q.canonical_value(),
        _ => default,
    }
}

fn bool_property(doc: &Document, id: ObjectId, name: &str, default: bool) -> bool {
    match property(doc, id, name) {
        Some(Property::Bool(v)) => *v,
        _ => default,
    }
}

fn enum_index(doc: &Document, id: ObjectId, name: &str) -> usize {
    match property(doc, id, name) {
        Some(Property::Enumeration(_, index)) => *index,
        _ => 0,
    }
}

fn float_list(doc: &Document, id: ObjectId, name: &str) -> Vec<f64> {
    match property(doc, id, name) {
        Some(Property::FloatList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn int_pairs(doc: &Document, id: ObjectId, name: &str) -> Vec<(i64, i64)> {
    match property(doc, id, name) {
        Some(Property::IntPairList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn link_name(doc: &Document, id: ObjectId, name: &str) -> Option<String> {
    match property(doc, id, name) {
        Some(Property::Link(value)) if !value.is_empty() => Some(value.clone()),
        Some(Property::LinkSub(value, _)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn link_sub(doc: &Document, id: ObjectId, name: &str) -> (Option<String>, Vec<String>) {
    match property(doc, id, name) {
        Some(Property::LinkSub(object, subs)) if !object.is_empty() || !subs.is_empty() => {
            (Some(object.clone()).filter(|s| !s.is_empty()), subs.clone())
        }
        Some(Property::Link(object)) if !object.is_empty() => (Some(object.clone()), Vec::new()),
        _ => (None, Vec::new()),
    }
}

fn placement_property(doc: &Document, id: ObjectId, name: &str) -> Placement {
    match property(doc, id, name) {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

fn placements(doc: &Document, id: ObjectId, name: &str) -> Vec<Placement> {
    match property(doc, id, name) {
        Some(Property::PlacementList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn linked_shape(doc: &Document, name: &str) -> Option<Shape> {
    let id = doc.get_by_name(name)?;
    property(doc, id, "Shape").and_then(shape_of).and_then(|s| s.shape())
}

fn object_shape(doc: &Document, id: ObjectId) -> Option<Shape> {
    property(doc, id, "Shape").and_then(shape_of).and_then(|s| s.shape())
}

// ---------------------------------------------------------------------------
// Placements
// ---------------------------------------------------------------------------


/// Rotate `point` about the line through `center` along `axis` by `angle`.
fn rotate_about(center: Vector3, axis: &Vector3, angle: f64, point: Vector3) -> Vector3 {
    let rotation = Rotation::from_axis_angle(axis, angle);
    center.add(&rotation.to_matrix().transform(&point.sub(&center)))
}

fn translation(base: Vector3) -> Placement {
    Placement {
        base,
        rotation: Rotation::identity(),
    }
}

fn rotation_placement(axis: &Vector3, angle: f64) -> Placement {
    Placement {
        base: Vector3::zero(),
        rotation: Rotation::from_axis_angle(axis, angle),
    }
}

// ---------------------------------------------------------------------------
// Common LinkArray behaviour
// ---------------------------------------------------------------------------

/// Properties every array carries: the shared `App::Link` base
/// ([`crate::link::link_properties`]) plus the array-specific ones each type
/// pushes. Upstream the arrays derive from `App::Link`, so the base set is the
/// same list rather than a copy; the arrays are still registered as peers, not
/// subclasses (the base objects do not share an `ObjectType`).
fn array_properties() -> Vec<(String, Property, u32)> {
    crate::link::link_properties()
}

fn push_prop(
    target: &mut Vec<(String, Property, u32)>,
    name: &str,
    value: Property,
    status: u32,
) {
    target.push((name.to_string(), value, status));
}

fn length(value: f64) -> Property {
    Property::Quantity(Quantity::new(value, Unit::Millimeter))
}

fn integer_constraint(value: i64) -> Property {
    Property::IntegerConstraint {
        value,
        min: 1,
        max: 1000,
        step: 1,
    }
}

/// The generated-element object names, resolved.
fn element_ids(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    match property(doc, id, "ElementList") {
        Some(Property::LinkList(names)) => names
            .iter()
            .filter_map(|name| doc.get_by_name(name))
            .collect(),
        _ => Vec::new(),
    }
}

fn element_suppressed(doc: &Document, id: ObjectId) -> bool {
    matches!(
        property(doc, id, "Suppressed"),
        Some(Property::Bool(value)) if *value
    )
}

/// Grow/shrink the generated-element set to `count`, reusing the existing objects
/// so their `Suppressed` flags survive a recompute.
fn sync_element_objects(doc: &mut Document, id: ObjectId, count: usize) -> Vec<ObjectId> {
    let mut ids = element_ids(doc, id);
    if ids.len() > count {
        ids.truncate(count);
    }
    if ids.len() < count {
        let array_name = doc
            .object(id)
            .map(|object| object.name.clone())
            .unwrap_or_else(|| "Array".to_string());
        for index in ids.len()..count {
            let name = format!("{array_name}_Element{index}");
            // No `LinkedObject`: the array already lists the element through
            // `ElementList`, and a link back would be a dependency cycle.
            ids.push(doc.add_object_with(&name, ELEMENT_TYPE, true));
        }
    }
    let names: Vec<String> = ids
        .iter()
        .filter_map(|element| doc.object(*element).map(|object| object.name.clone()))
        .collect();
    doc.set_property_raw(id, "ElementList", Property::LinkList(names));
    ids
}

/// Write the computed result of an array.
///
/// `element_flags` is `Some` for linear arrays, whose per-element suppression is
/// derived from `SuppressedPositions`; `None` leaves the flags the user set.
fn finish_array(
    doc: &mut Document,
    id: ObjectId,
    result: &[Placement],
    element_flags: Option<&[bool]>,
) {
    doc.set_property_raw(id, "ElementCount", Property::Integer(result.len() as i64));

    if bool_property(doc, id, "ShowElement", true) {
        let ids = sync_element_objects(doc, id, result.len());
        for (index, element) in ids.iter().enumerate() {
            if let Some(placement) = result.get(index) {
                doc.set_property_raw(*element, "Placement", Property::Placement(*placement));
            }
            if let Some(flags) = element_flags {
                let suppressed = flags.get(index).copied().unwrap_or(false);
                doc.set_property_raw(*element, "Suppressed", Property::Bool(suppressed));
            }
        }
        return;
    }

    // Collapsed array: the placements are the output, and the elements go away.
    let current = placements(doc, id, "PlacementList");
    if current != result {
        doc.set_property_raw(id, "PlacementList", Property::PlacementList(result.to_vec()));
    }
    doc.set_property_raw(id, "ElementList", Property::LinkList(Vec::new()));

    let shape = if result.is_empty() {
        ShapeProperty::empty()
    } else {
        match link_name(doc, id, "LinkedObject").and_then(|name| linked_shape(doc, &name)) {
            Some(source) => {
                let backend = ferrocad_part::backend();
                let mut shapes = Vec::with_capacity(result.len());
                let mut ok = true;
                for placement in result {
                    match backend.transform(&source, &placement.to_matrix()) {
                        Ok(shape) => shapes.push(shape),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    match backend.make_compound(&shapes) {
                        Ok(compound) => ShapeProperty::new(compound),
                        Err(_) => ShapeProperty::empty(),
                    }
                } else {
                    ShapeProperty::empty()
                }
            }
            None => ShapeProperty::empty(),
        }
    };
    doc.set_property_raw(id, "Shape", make_shape_property(shape));
}

// ---------------------------------------------------------------------------
// Point pattern
// ---------------------------------------------------------------------------

struct PointArray;

impl ObjectType for PointArray {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        let mut properties = array_properties();
        push_prop(
            &mut properties,
            "PointObject",
            Property::Link(String::new()),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "OriginPointIndex",
            Property::Integer(0),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "RelativeToFirst",
            Property::Bool(false),
            prop_status::NONE,
        );
        properties
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let Some(point_object) = link_name(doc, id, "PointObject") else {
            finish_array(doc, id, &[], None);
            return;
        };
        let Some(point_id) = doc.get_by_name(&point_object) else {
            finish_array(doc, id, &[], None);
            return;
        };
        let Some(shape) = object_shape(doc, point_id) else {
            doc.mark_invalid(id);
            return;
        };

        let backend = ferrocad_part::backend();
        let mut points: Vec<Vector3> = Vec::new();
        for element in backend.elements(&shape, ElementKind::Vertex) {
            let Some(point) = backend.vertex_point(&element.shape) else {
                continue;
            };
            if points
                .iter()
                .any(|existing| existing.distance(&point) <= CONFUSION)
            {
                continue;
            }
            points.push(point);
        }
        if points.is_empty() {
            doc.mark_invalid(id);
            return;
        }

        let relative = bool_property(doc, id, "RelativeToFirst", false);
        let origin_index = if relative {
            0
        } else {
            int_property(doc, id, "OriginPointIndex", 0).max(0) as usize
        };
        if origin_index >= points.len() {
            doc.mark_invalid(id);
            return;
        }

        let origin = points[origin_index];
        let mut result = Vec::with_capacity(points.len());
        for offset in 0..points.len() {
            let point_index = (origin_index + offset) % points.len();
            let mut base = points[point_index].sub(&origin);
            if relative {
                // Upstream adds the origin back for the relative form.
                base = points[point_index];
            }
            result.push(translation(base));
        }
        finish_array(doc, id, &result, None);
    }
}

// ---------------------------------------------------------------------------
// Linear pattern
// ---------------------------------------------------------------------------

struct LinearArray;

fn linear_properties() -> Vec<(String, Property, u32)> {
    let mut properties = array_properties();
    let modes = vec!["Extent".to_string(), "Spacing".to_string()];
    for (suffix, occurrences, length_default) in
        [("", 2, 100.0), ("2", 1, 100.0)]
    {
        push_prop(
            &mut properties,
            &format!("Direction{suffix}"),
            Property::LinkSub(String::new(), Vec::new()),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Reversed{suffix}"),
            Property::Bool(false),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Mode{suffix}"),
            Property::Enumeration(modes.clone(), 0),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Length{suffix}"),
            length(length_default),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Offset{suffix}"),
            length(10.0),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Occurrences{suffix}"),
            integer_constraint(occurrences),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("Spacings{suffix}"),
            Property::FloatList(vec![-1.0]),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            &format!("SpacingPattern{suffix}"),
            Property::FloatList(Vec::new()),
            prop_status::NONE,
        );
    }
    push_prop(
        &mut properties,
        "SuppressedPositions",
        Property::IntPairList(Vec::new()),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "GeneratedOccurrences2",
        Property::Integer(0),
        prop_status::HIDDEN,
    );
    properties
}

/// The direction for one linear axis, honouring object-axis subnames when no
/// object is referenced (the `(None, ["Z_Axis"])` form the tests use).
fn linear_axis(doc: &Document, id: ObjectId, direction_prop: &str) -> Vector3 {
    let (reference, subs) = link_sub(doc, id, direction_prop);
    if let Some(name) = reference {
        if let Some(reference_id) = doc.get_by_name(&name) {
            if let Some(axis) = axis_from_reference(doc, reference_id, subs.first()) {
                return axis.1.normalize();
            }
        }
        return Vector3::new(1.0, 0.0, 0.0);
    }
    if let Some(sub) = subs.first() {
        match sub.rsplit('.').next().unwrap_or(sub) {
            "X_Axis" => return Vector3::new(1.0, 0.0, 0.0),
            "Y_Axis" => return Vector3::new(0.0, 1.0, 0.0),
            "Z_Axis" => return Vector3::new(0.0, 0.0, 1.0),
            _ => {}
        }
    }
    Vector3::zero()
}

/// `calculateOffsetVectorWithDefault`: unit direction scaled by one step.
fn linear_step(doc: &Document, id: ObjectId, second: bool) -> Result<Vector3, String> {
    let suffix = if second { "2" } else { "" };
    let occurrences = int_property(doc, id, &format!("Occurrences{suffix}"), if second { 1 } else { 2 });
    if occurrences <= 1 {
        return Ok(Vector3::zero());
    }
    let mut direction = linear_axis(doc, id, &format!("Direction{suffix}"));
    if direction.length() <= CONFUSION {
        direction = if second {
            Vector3::new(0.0, 1.0, 0.0)
        } else {
            Vector3::new(1.0, 0.0, 0.0)
        };
    }
    if bool_property(doc, id, &format!("Reversed{suffix}"), false) {
        direction = direction.neg();
    }
    if enum_index(doc, id, &format!("Mode{suffix}")) == 0 {
        // Extent: the distance is split over the intervals.
        let length = float_property(doc, id, &format!("Length{suffix}"), 100.0);
        if length < CONFUSION {
            return Err("Pattern length too small".to_string());
        }
        Ok(direction.scale(length / (occurrences - 1) as f64))
    } else {
        let offset = float_property(doc, id, &format!("Offset{suffix}"), 10.0);
        Ok(direction.scale(offset))
    }
}

/// `calculateSteps`: the cumulative offsets for one direction.
fn linear_steps(
    doc: &Document,
    id: ObjectId,
    second: bool,
    step: Vector3,
) -> Result<Vec<Vector3>, String> {
    let suffix = if second { "2" } else { "" };
    let occurrences =
        int_property(doc, id, &format!("Occurrences{suffix}"), if second { 1 } else { 2 });
    let mut steps = vec![Vector3::zero()];
    if occurrences <= 1 {
        return Ok(steps);
    }
    if enum_index(doc, id, &format!("Mode{suffix}")) == 1 {
        let mut spacings = float_list(doc, id, &format!("Spacings{suffix}"));
        let pattern = float_list(doc, id, &format!("SpacingPattern{suffix}"));
        let offset = float_property(doc, id, &format!("Offset{suffix}"), 10.0);
        let target = occurrences.saturating_sub(1) as usize;
        if spacings.len() != target {
            spacings.resize(target, -1.0);
        }
        let use_pattern = pattern.len() > 1;
        // `step` already carries the offset length, so its unit direction gives
        // the per-cumulative-distance vector.
        let direction = step.normalize();
        let mut cumulative = 0.0;
        for index in 1..occurrences as usize {
            let spacing = *spacings.get(index - 1).unwrap_or(&-1.0);
            let spacing = if spacing != -1.0 {
                spacing
            } else if use_pattern {
                pattern[(index - 1) % pattern.len()]
            } else {
                offset
            };
            cumulative += spacing;
            steps.push(direction.scale(cumulative));
        }
    } else {
        for index in 1..occurrences as usize {
            steps.push(step.scale(index as f64));
        }
    }
    Ok(steps)
}

/// Which `(row, column)` positions the generated elements currently map to.
fn positions_for(positions: &[(i64, i64)], stride: usize, index: usize) -> bool {
    let position = ((index / stride) as i64, (index % stride) as i64);
    positions.contains(&position)
}

fn set_positions(doc: &mut Document, id: ObjectId, positions: Vec<(i64, i64)>) {
    doc.set_property_raw(id, "SuppressedPositions", Property::IntPairList(positions));
}

/// Fold user edits on `ElementList[i].Suppressed` back into `SuppressedPositions`
/// using the stride the elements were generated with (upstream's change signal).
fn absorb_element_suppression(doc: &mut Document, id: ObjectId, stride: usize) {
    if stride == 0 {
        return;
    }
    let ids = element_ids(doc, id);
    if ids.is_empty() {
        return;
    }
    let mut positions = int_pairs(doc, id, "SuppressedPositions");
    let original = positions.clone();
    for (index, element) in ids.iter().enumerate() {
        let suppressed = element_suppressed(doc, *element);
        let listed = positions_for(&positions, stride, index);
        if suppressed == listed {
            continue;
        }
        let position = ((index / stride) as i64, (index % stride) as i64);
        if suppressed {
            positions.push(position);
        } else {
            positions.retain(|entry| *entry != position);
        }
    }
    if positions != original {
        set_positions(doc, id, positions);
    }
}

impl ObjectType for LinearArray {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        linear_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let occurrences = int_property(doc, id, "Occurrences", 2);
        let occurrences2 = int_property(doc, id, "Occurrences2", 1);
        if occurrences < 1 || occurrences2 < 1 {
            doc.mark_invalid(id);
            return;
        }

        let stride = int_property(doc, id, "GeneratedOccurrences2", 0).max(1) as usize;
        if bool_property(doc, id, "ShowElement", true) {
            absorb_element_suppression(doc, id, stride);
        }

        let step1 = match linear_step(doc, id, false) {
            Ok(step) => step,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        let step2 = match linear_step(doc, id, true) {
            Ok(step) => step,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        let steps1 = match linear_steps(doc, id, false, step1) {
            Ok(steps) => steps,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        let steps2 = match linear_steps(doc, id, true, step2) {
            Ok(steps) => steps,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };

        let mut result = Vec::with_capacity(steps1.len() * steps2.len());
        for first in &steps1 {
            for second in &steps2 {
                result.push(translation(first.add(second)));
            }
        }

        let new_stride = occurrences2 as usize;
        let positions = int_pairs(doc, id, "SuppressedPositions");
        let flags: Vec<bool> = (0..result.len())
            .map(|index| positions_for(&positions, new_stride, index))
            .collect();

        doc.set_property_raw(id, "GeneratedOccurrences2", Property::Integer(occurrences2));
        finish_array(doc, id, &result, Some(&flags));
    }
}

// ---------------------------------------------------------------------------
// Path pattern
// ---------------------------------------------------------------------------

struct PathArray;

/// One path edge, sampled in its own (forward) parameterisation.
struct PathEdge {
    curve: EdgeCurve,
    length: f64,
    /// Line origin/parameters (`EdgeCurve::Line` only).
    line_origin: Vector3,
    line_direction: Vector3,
    line_start: f64,
    line_sign: f64,
    /// Arc parameter range (radians).
    arc_first: f64,
    start: Vector3,
    end: Vector3,
}

impl PathEdge {
    fn from_shape(backend: &dyn GeometryBackend, shape: &Shape) -> Result<Self, String> {
        let curve = backend
            .curve_of(shape)
            .map_err(|_| "Path edge has no supported curve".to_string())?;
        match &curve {
            EdgeCurve::Line(line) => {
                let origin = line.origin();
                let direction = line.direction().normalize();
                let mut parameters: Vec<f64> = backend
                    .elements(shape, ElementKind::Vertex)
                    .iter()
                    .filter_map(|element| backend.vertex_point(&element.shape))
                    .map(|point| point.sub(&origin).dot(&direction))
                    .collect();
                if parameters.len() < 2 {
                    return Err("Path line edge has no endpoints".to_string());
                }
                parameters.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let first = parameters[0];
                let last = *parameters.last().unwrap();
                let sign = if last >= first { 1.0 } else { -1.0 };
                let start = origin.add(&direction.scale(first));
                let end = origin.add(&direction.scale(last));
                Ok(PathEdge {
                    curve,
                    length: (last - first).abs(),
                    line_origin: origin,
                    line_direction: direction,
                    line_start: first,
                    line_sign: sign,
                    arc_first: 0.0,
                    start,
                    end,
                })
            }
            EdgeCurve::LineSegment(segment) => {
                let start = segment.start();
                let end = segment.end();
                Ok(PathEdge {
                    curve,
                    length: start.distance(&end),
                    line_origin: start,
                    line_direction: end.sub(&start).normalize(),
                    line_start: 0.0,
                    line_sign: 1.0,
                    arc_first: 0.0,
                    start,
                    end,
                })
            }
            EdgeCurve::Circle(circle) => Ok(PathEdge {
                curve,
                length: circle.length(),
                line_origin: Vector3::zero(),
                line_direction: Vector3::zero(),
                line_start: 0.0,
                line_sign: 1.0,
                arc_first: 0.0,
                start: circle.point_at(0.0),
                end: circle.point_at(0.0),
            }),
            EdgeCurve::Arc(arc) => {
                let radius = arc.circle().radius();
                let sweep = arc.sweep();
                Ok(PathEdge {
                    curve,
                    length: radius * sweep.abs(),
                    line_origin: Vector3::zero(),
                    line_direction: Vector3::zero(),
                    line_start: 0.0,
                    line_sign: sweep.signum(),
                    arc_first: arc.first(),
                    start: arc.start(),
                    end: arc.end(),
                })
            }
            EdgeCurve::Ellipse(ellipse) => {
                let point = ellipse.point_at(0.0);
                Ok(PathEdge {
                    curve,
                    length: ellipse.length(),
                    line_origin: Vector3::zero(),
                    line_direction: Vector3::zero(),
                    line_start: 0.0,
                    line_sign: 1.0,
                    arc_first: 0.0,
                    start: point,
                    end: point,
                })
            }
        }
    }

    fn point_at(&self, distance: f64) -> Vector3 {
        match &self.curve {
            EdgeCurve::Line(_) => self
                .line_origin
                .add(&self.line_direction.scale(self.line_start + self.line_sign * distance)),
            EdgeCurve::LineSegment(_) => {
                if self.length <= CONFUSION {
                    self.start
                } else {
                    self.start
                        .add(&self.end.sub(&self.start).scale(distance / self.length))
                }
            }
            EdgeCurve::Circle(circle) => circle.point_at(distance / circle.radius()),
            EdgeCurve::Arc(arc) => {
                let radius = arc.circle().radius();
                arc.point_at(self.arc_first + self.line_sign * distance / radius)
            }
            EdgeCurve::Ellipse(ellipse) => {
                let parameter = if self.length <= CONFUSION {
                    0.0
                } else {
                    distance / self.length * std::f64::consts::TAU
                };
                ellipse.point_at(parameter)
            }
        }
    }

    fn tangent_at(&self, distance: f64) -> Vector3 {
        match &self.curve {
            EdgeCurve::Line(_) => self.line_direction.scale(self.line_sign),
            EdgeCurve::LineSegment(_) => self.end.sub(&self.start).normalize(),
            EdgeCurve::Circle(circle) => circle.tangent_at(distance / circle.radius()),
            EdgeCurve::Arc(arc) => {
                let radius = arc.circle().radius();
                arc.tangent_at(self.arc_first + self.line_sign * distance / radius)
                    .scale(self.line_sign)
            }
            EdgeCurve::Ellipse(ellipse) => {
                let parameter = if self.length <= CONFUSION {
                    0.0
                } else {
                    distance / self.length * std::f64::consts::TAU
                };
                ellipse.tangent_at(parameter)
            }
        }
    }
}

/// The upstream `frameAt`: translate, and optionally align X to the tangent with
/// the requested vertical as local Z.
fn path_frame(edge: &PathEdge, local: f64, align: bool, reversed: bool, vertical: Vector3) -> Placement {
    let position = edge.point_at(local);
    if !align {
        return translation(position);
    }
    let mut x = edge.tangent_at(local);
    if reversed {
        x = x.neg();
    }
    if x.length() <= CONFUSION {
        return translation(position);
    }
    x = x.normalize();
    let mut z = vertical.sub(&x.scale(vertical.dot(&x)));
    if z.length() <= CONFUSION {
        z = if x.z.abs() < 0.9 {
            Vector3::new(0.0, 0.0, 1.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        z = z.sub(&x.scale(z.dot(&x)));
    }
    if z.length() <= CONFUSION {
        return translation(position);
    }
    let z = z.normalize();
    let y = z.cross(&x);
    let matrix = Matrix4::from_values([
        x.x, y.x, z.x, 0.0, x.y, y.y, z.y, 0.0, x.z, y.z, z.z, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);
    Placement {
        base: position,
        rotation: Rotation::from_matrix(&matrix),
    }
}

fn path_edges(
    backend: &dyn GeometryBackend,
    doc: &Document,
    id: ObjectId,
) -> Result<Vec<PathEdge>, String> {
    let (reference, subs) = link_sub(doc, id, "Path");
    let Some(name) = reference else {
        return Ok(Vec::new());
    };
    let reference_id = doc
        .get_by_name(&name)
        .ok_or_else(|| "Path object is missing".to_string())?;
    let shape = object_shape(doc, reference_id)
        .ok_or_else(|| "Path shape is empty".to_string())?;
    let mut edges = Vec::new();
    let subnames: Vec<String> = subs.into_iter().filter(|sub| !sub.is_empty()).collect();
    if !subnames.is_empty() {
        for subname in subnames {
            let sub = backend
                .resolve(&shape, &subname)
                .ok_or_else(|| format!("Path subelement {subname} is missing"))?;
            edges.push(PathEdge::from_shape(backend, &sub)?);
        }
    } else {
        for element in backend.elements(&shape, ElementKind::Edge) {
            edges.push(PathEdge::from_shape(backend, &element.shape)?);
        }
    }
    if edges.is_empty() {
        return Err("Path does not contain edges".to_string());
    }
    Ok(edges)
}

/// `calculateDistances`: arc-length positions for the three spacing modes.
fn path_distances(
    path_length: f64,
    closed: bool,
    count: i64,
    mode: usize,
    spacing: f64,
    start_offset: f64,
    end_offset: f64,
) -> Result<Vec<f64>, String> {
    if !path_length.is_finite() || path_length < 0.0 {
        return Err("Path length and offsets must be finite".to_string());
    }
    if !start_offset.is_finite() || !end_offset.is_finite() {
        return Err("Path length and offsets must be finite".to_string());
    }
    if start_offset < 0.0 || end_offset < 0.0 {
        return Err("Path offsets cannot be negative".to_string());
    }
    if start_offset + end_offset > path_length + CONFUSION {
        return Err("Path offsets exceed the path length".to_string());
    }
    if count < 1 {
        return Err("Occurrence count must be at least one".to_string());
    }
    let count = count as usize;
    let available = (path_length - start_offset - end_offset).max(0.0);
    let mut distances = Vec::new();

    if mode == 0 {
        let full_closed = closed && start_offset <= CONFUSION && end_offset <= CONFUSION;
        let segments = if full_closed {
            count
        } else {
            count.saturating_sub(1).max(1)
        };
        let step = available / segments as f64;
        for index in 0..count {
            distances.push(start_offset + index as f64 * step);
        }
        return Ok(distances);
    }

    if !spacing.is_finite() || spacing <= CONFUSION {
        return Err("Path spacing must be greater than zero".to_string());
    }
    let maximum = if mode == 2 {
        count
    } else {
        MAX_OCCURRENCES + 1
    };
    let end_distance = path_length - end_offset;
    for index in 0..maximum {
        let distance = start_offset + index as f64 * spacing;
        if distance > end_distance + CONFUSION {
            break;
        }
        distances.push(distance);
        if distances.len() > MAX_OCCURRENCES {
            return Err(format!(
                "Path pattern would create more than {MAX_OCCURRENCES} occurrences"
            ));
        }
    }
    Ok(distances)
}

impl ObjectType for PathArray {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        let mut properties = array_properties();
        push_prop(
            &mut properties,
            "Path",
            Property::LinkSub(String::new(), Vec::new()),
            prop_status::NONE,
        );
        push_prop(&mut properties, "Count", integer_constraint(4), prop_status::NONE);
        push_prop(
            &mut properties,
            "SpacingMode",
            Property::Enumeration(
                vec![
                    "Fixed count".to_string(),
                    "Fixed spacing".to_string(),
                    "Fixed count and spacing".to_string(),
                ],
                0,
            ),
            prop_status::NONE,
        );
        push_prop(&mut properties, "Spacing", length(20.0), prop_status::NONE);
        push_prop(&mut properties, "StartOffset", length(0.0), prop_status::NONE);
        push_prop(&mut properties, "EndOffset", length(0.0), prop_status::NONE);
        push_prop(
            &mut properties,
            "ReversePath",
            Property::Bool(false),
            prop_status::NONE,
        );
        push_prop(&mut properties, "Align", Property::Bool(false), prop_status::NONE);
        push_prop(
            &mut properties,
            "VerticalVector",
            Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
            prop_status::NONE,
        );
        properties
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        // A null `Path` is silent (upstream's `getElementPlacements` returns {});
        // only a broken path is invalid.
        if link_sub(doc, id, "Path").0.is_none() {
            finish_array(doc, id, &[], None);
            return;
        }
        let backend = ferrocad_part::backend();
        let mut edges = match path_edges(backend.as_ref(), doc, id) {
            Ok(edges) if !edges.is_empty() => edges,
            _ => {
                doc.mark_invalid(id);
                return;
            }
        };

        let reversed = bool_property(doc, id, "ReversePath", false);
        if reversed {
            // The upstream kernel adaptor ignores edge orientation, so reversing
            // swaps the edge order but keeps each edge's own sampling direction.
            edges.reverse();
        }

        let mut ends = Vec::with_capacity(edges.len());
        let mut total = 0.0;
        for edge in &edges {
            total += edge.length;
            ends.push(total);
        }
        if total <= CONFUSION {
            doc.mark_invalid(id);
            return;
        }

        let count = int_property(doc, id, "Count", 4);
        let mode = enum_index(doc, id, "SpacingMode");
        let spacing = float_property(doc, id, "Spacing", 20.0);
        let start = float_property(doc, id, "StartOffset", 0.0);
        let end = float_property(doc, id, "EndOffset", 0.0);
        // Orientation-aware endpoint comparison: a reversed path is traversed from
        // the last edge's far end to the first edge's near end, even though the
        // kernel adaptor still samples each edge forward.
        let closed = edges
            .first()
            .zip(edges.last())
            .map(|(first, last)| {
                if reversed {
                    first.end.distance(&last.start) <= CONFUSION
                } else {
                    first.start.distance(&last.end) <= CONFUSION
                }
            })
            .unwrap_or(false);

        let distances = match path_distances(
            total,
            closed,
            count,
            mode,
            spacing,
            if reversed { end } else { start },
            if reversed { start } else { end },
        ) {
            Ok(distances) => distances,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };
        if distances.is_empty() {
            doc.mark_invalid(id);
            return;
        }

        let align = bool_property(doc, id, "Align", false);
        let vertical = match property(doc, id, "VerticalVector") {
            Some(Property::Vector(vector)) => *vector,
            _ => Vector3::new(0.0, 0.0, 1.0),
        };
        let mut result = Vec::with_capacity(distances.len());
        for distance in distances {
            let mut index = edges.len() - 1;
            for (candidate, end) in ends.iter().enumerate() {
                if *end >= distance - CONFUSION {
                    index = candidate;
                    break;
                }
            }
            let edge_start = if index == 0 { 0.0 } else { ends[index - 1] };
            let local = (distance - edge_start).clamp(0.0, edges[index].length);
            result.push(path_frame(&edges[index], local, align, reversed, vertical));
        }
        finish_array(doc, id, &result, None);
    }
}

// ---------------------------------------------------------------------------
// Axis references (circular / polar / linear)
// ---------------------------------------------------------------------------

/// `(base point, unit direction)` of an axis reference.
fn axis_from_reference(
    doc: &Document,
    reference_id: ObjectId,
    subname: Option<&String>,
) -> Option<(Vector3, Vector3)> {
    let object = doc.object(reference_id)?;
    let subname = subname.map(String::as_str).unwrap_or("");
    let placement = placement_property(doc, reference_id, "Placement");

    if object.type_id == PART2D_TYPE {
        let local = match subname {
            "H_Axis" => Vector3::new(1.0, 0.0, 0.0),
            "V_Axis" => Vector3::new(0.0, 1.0, 0.0),
            "N_Axis" => Vector3::new(0.0, 0.0, 1.0),
            other if other.starts_with("Edge") => {
                return edge_axis(doc, reference_id, other);
            }
            _ => Vector3::new(0.0, 0.0, 1.0),
        };
        return Some((
            placement.base,
            placement.rotation.to_matrix().transform(&local).normalize(),
        ));
    }

    if subname.starts_with("Edge") {
        if let Some(axis) = edge_axis(doc, reference_id, subname) {
            return Some(axis);
        }
    }
    if let Some(shape) = object_shape(doc, reference_id) {
        let backend = ferrocad_part::backend();
        if subname.is_empty() {
            if let Some(element) = backend.elements(&shape, ElementKind::Edge).first() {
                return edge_axis_of(backend.as_ref(), &element.shape);
            }
        }
    }
    None
}

fn edge_axis(doc: &Document, reference_id: ObjectId, subname: &str) -> Option<(Vector3, Vector3)> {
    let shape = object_shape(doc, reference_id)?;
    let backend = ferrocad_part::backend();
    let edge = backend.resolve(&shape, subname)?;
    edge_axis_of(backend.as_ref(), &edge)
}

fn edge_axis_of(backend: &dyn GeometryBackend, edge: &Shape) -> Option<(Vector3, Vector3)> {
    match backend.curve_of(edge).ok()? {
        EdgeCurve::Line(line) => Some((line.origin(), line.direction().normalize())),
        EdgeCurve::LineSegment(segment) => Some((
            segment.start(),
            segment.end().sub(&segment.start()).normalize(),
        )),
        EdgeCurve::Circle(circle) => Some((circle.center(), circle.normal())),
        EdgeCurve::Arc(arc) => Some((arc.circle().center(), arc.circle().normal())),
        EdgeCurve::Ellipse(ellipse) => Some((ellipse.center(), ellipse.normal())),
    }
}

/// `CircularPatternExtension::getRotation`.
fn circular_rotation(
    doc: &Document,
    id: ObjectId,
    property_name: &str,
) -> (Vector3, Vector3) {
    let (reference, subs) = link_sub(doc, id, property_name);
    let subname = subs.first().cloned().unwrap_or_default();
    let Some(name) = reference else {
        return match subname.as_str() {
            "X_Axis" => (Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)),
            "Y_Axis" => (Vector3::zero(), Vector3::new(0.0, 1.0, 0.0)),
            _ => (Vector3::zero(), Vector3::new(0.0, 0.0, 1.0)),
        };
    };
    let Some(reference_id) = doc.get_by_name(&name) else {
        return (Vector3::zero(), Vector3::new(0.0, 0.0, 1.0));
    };
    if let Some((base, direction)) = axis_from_reference(doc, reference_id, subs.first()) {
        // The reference is given in world coordinates; the pattern works in the
        // array's local frame (upstream transforms by the inverse placement).
        let inverse = placement_property(doc, id, "Placement").inverse();
        let rotation = inverse.rotation.to_matrix();
        return (
            inverse.base.add(&rotation.transform(&base)),
            rotation.transform(&direction).normalize(),
        );
    }
    (Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
}

struct CircularArray;

fn circular_properties() -> Vec<(String, Property, u32)> {
    let mut properties = array_properties();
    push_prop(
        &mut properties,
        "Axis",
        Property::LinkSub(String::new(), Vec::new()),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "RadialDistance",
        length(50.0),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "TangentialDistance",
        length(25.0),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "NumberCircles",
        integer_constraint(3),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "Symmetry",
        integer_constraint(1),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "StartAtShape",
        Property::Bool(false),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "BinormalAxis",
        Property::LinkSub(String::new(), Vec::new()),
        prop_status::NONE,
    );
    push_prop(
        &mut properties,
        "FlipBinormalAxis",
        Property::Bool(false),
        prop_status::NONE,
    );
    properties
}

/// `CircularPatternExtension::calculateTransformations`.
fn circular_placements(doc: &Document, id: ObjectId) -> Result<Vec<Placement>, String> {
    let radial_distance = float_property(doc, id, "RadialDistance", 50.0);
    let tangential_distance = float_property(doc, id, "TangentialDistance", 25.0);
    let circle_count = int_property(doc, id, "NumberCircles", 3);
    let symmetry = int_property(doc, id, "Symmetry", 1).max(1);

    if !radial_distance.is_finite() || radial_distance <= CONFUSION {
        return Err("Radial distance must be greater than zero".to_string());
    }
    if !tangential_distance.is_finite() || tangential_distance <= CONFUSION {
        return Err("Tangential distance must be greater than zero".to_string());
    }
    if circle_count < 1 {
        return Err("At least one concentric circles is required".to_string());
    }
    if circle_count as usize > MAX_OCCURRENCES {
        return Err(format!(
            "Circular pattern would create more than {MAX_OCCURRENCES} circles"
        ));
    }

    let (center, axis) = circular_rotation(doc, id, "Axis");
    let (_, binormal) = circular_rotation(doc, id, "BinormalAxis");
    let axis = axis.normalize();
    let binormal = binormal.normalize();

    let mut radial = binormal.sub(&axis.scale(binormal.dot(&axis)));
    if radial.sqr() <= CONFUSION * CONFUSION {
        return Err("Tangent axis must not be parallel to the pattern axis".to_string());
    }
    if bool_property(doc, id, "FlipBinormalAxis", false) {
        radial = radial.neg();
    }
    radial = radial.normalize();

    let start_at_shape = bool_property(doc, id, "StartAtShape", false);
    let start_circle = if start_at_shape { 0 } else { 1 };
    let end_circle = if start_at_shape {
        circle_count - 1
    } else {
        circle_count
    };

    let mut transformations = vec![Placement::identity()];
    let full_circle = std::f64::consts::TAU;

    for circle in start_circle..=end_circle {
        let radius = circle as f64 * radial_distance;
        let mut element_count = if circle == 0 {
            let requested = (full_circle * radial_distance / tangential_distance).floor();
            let rounded = (requested as i64 / symmetry) * symmetry;
            if rounded == 0 {
                1
            } else {
                rounded
            }
        } else {
            let requested = (full_circle * radius / tangential_distance).floor();
            if !requested.is_finite() {
                return Err("Invalid number of occurrences".to_string());
            }
            (requested as i64 / symmetry) * symmetry
        };
        if element_count == 0 {
            continue;
        }
        if transformations.len() + element_count as usize > MAX_OCCURRENCES {
            return Err(format!(
                "Circular pattern created more than {MAX_OCCURRENCES} circles"
            ));
        }
        element_count = element_count.max(0);
        let angular_step = full_circle / element_count as f64;

        for element in 0..element_count {
            if circle == 0 && element == 0 {
                transformations.push(Placement::identity());
                continue;
            }
            let angle = element as f64 * angular_step;
            let translation = radial.scale(radius);
            let base = rotate_about(center, &axis, angle, translation);
            transformations.push(Placement {
                base,
                rotation: Rotation::from_axis_angle(&axis, angle),
            });
        }
    }

    Ok(transformations)
}

impl ObjectType for CircularArray {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        circular_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        match circular_placements(doc, id) {
            Ok(result) => finish_array(doc, id, &result, None),
            Err(_) => doc.mark_invalid(id),
        }
    }
}

// ---------------------------------------------------------------------------
// Polar pattern (supporting type for the circular tests)
// ---------------------------------------------------------------------------

struct PolarArray;

impl ObjectType for PolarArray {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        let mut properties = array_properties();
        push_prop(
            &mut properties,
            "Axis",
            Property::LinkSub(String::new(), Vec::new()),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "Reversed",
            Property::Bool(false),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "Mode",
            Property::Enumeration(vec!["Extent".to_string(), "Spacing".to_string()], 0),
            prop_status::NONE,
        );
        push_prop(&mut properties, "Angle", length(360.0), prop_status::NONE);
        push_prop(&mut properties, "Offset", length(120.0), prop_status::NONE);
        push_prop(
            &mut properties,
            "Occurrences",
            integer_constraint(3),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "Spacings",
            Property::FloatList(vec![-1.0, -1.0, -1.0]),
            prop_status::NONE,
        );
        push_prop(
            &mut properties,
            "SpacingPattern",
            Property::FloatList(Vec::new()),
            prop_status::NONE,
        );
        properties
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        match polar_placements(doc, id) {
            Ok(result) => finish_array(doc, id, &result, None),
            Err(_) => doc.mark_invalid(id),
        }
    }
}

fn polar_placements(doc: &Document, id: ObjectId) -> Result<Vec<Placement>, String> {
    let occurrences = int_property(doc, id, "Occurrences", 3);
    if occurrences < 1 {
        return Err("At least one occurrence required".to_string());
    }
    if occurrences == 1 {
        return Ok(vec![Placement::identity()]);
    }
    // The upstream polar getRotation ignores the subname without a reference
    // object, so this is the default +Z axis in that case.
    let (reference, subs) = link_sub(doc, id, "Axis");
    let (_, axis) = match reference {
        Some(name) => doc
            .get_by_name(&name)
            .and_then(|reference_id| axis_from_reference(doc, reference_id, subs.first()))
            .unwrap_or((Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))),
        None => (Vector3::zero(), Vector3::new(0.0, 0.0, 1.0)),
    };
    let mode = enum_index(doc, id, "Mode");
    let mut angle = 0.0;
    if mode == 0 {
        angle = float_property(doc, id, "Angle", 360.0);
        if (angle - 360.0).abs() < CONFUSION {
            angle /= occurrences as f64;
        } else {
            angle /= (occurrences - 1) as f64;
        }
        angle = angle.to_radians();
        if angle.abs() < 1e-12 {
            return Err("Pattern angle can't be null".to_string());
        }
    }
    let offset = float_property(doc, id, "Offset", 120.0);
    let mut spacings = float_list(doc, id, "Spacings");
    let target = (occurrences - 1) as usize;
    if spacings.len() != target {
        spacings.resize(target, -1.0);
    }
    let pattern = float_list(doc, id, "SpacingPattern");
    let use_pattern = pattern.len() > 1;
    let reversed = bool_property(doc, id, "Reversed", false);
    let axis = if reversed { axis.neg() } else { axis };

    let mut result = vec![Placement::identity()];
    let mut cumulative = 0.0;
    for index in 1..occurrences {
        let rotation = if mode == 1 {
            let spacing = spacings.get(index as usize - 1).copied().unwrap_or(-1.0);
            let spacing = if spacing != -1.0 {
                spacing
            } else if use_pattern {
                pattern[(index as usize - 1) % pattern.len()]
            } else {
                offset
            };
            cumulative += spacing.to_radians();
            cumulative
        } else {
            angle * index as f64
        };
        result.push(rotation_placement(&axis, rotation));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Generated element and sketch base
// ---------------------------------------------------------------------------

struct Element;

impl ObjectType for Element {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            ("Suppressed".to_string(), Property::Bool(false), prop_status::NONE),
            (
                "LinkedObject".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
        ]
    }
}

/// A minimal sketch base: `Shape` plus `Placement`, which is all the axis tests
/// need (`Part::Part2DObject` is abstract upstream).
struct Part2DObject;

impl ObjectType for Part2DObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
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
}

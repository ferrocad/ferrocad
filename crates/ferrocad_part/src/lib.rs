//! # FerroCAD Part workbench (`ferrocad_part`)
//!
//! The first module built on the document-object SPI. It registers two things with
//! `ferrocad_core`:
//!
//! - **`Part::Feature`** — an object type with a `Shape` and a `Placement`, via
//!   [`ferrocad_core::object_registry::ObjectType`];
//! - **`Part::Fillet`** / **`Part::Chamfer`** — parametric dress-up features whose
//!   `execute` resolves their selected edge names through the base shape's lineage
//!   first and the backend's positional names second, expands them to tangent chains,
//!   and writes the rebuilt shape to their output `Shape`;
//! - **`Part::PropertyPartShape`** — a property *value* type holding a
//!   [`ferrocad_geom::Shape`], via [`ferrocad_core::extension`];
//! - **`Part::PropertyTopoShapeList`** — the list counterpart, holding an ordered
//!   `Vec<Shape>` (upstream `PropertyTopoShapeList`).
//!
//! Part depends **only on the seam** ([`ferrocad_geom`]), never on a concrete kernel:
//! the application composes it with one — `ferrocad_part::register(Arc::new(OcctBackend::new()))`.
//! That is what lets the kernel be replaced (or swapped per edition) without touching
//! Part, and keeps `ferrocad_core` geometry-agnostic. See `docs/developer/architecture/occt-integration.md`.
//!
//! # Building
//!
//! The library needs no kernel. Its tests use OCCT through a dev-dependency, so they
//! still need OCCT 7.8+; the crate is **not** a default member.

mod containers;
mod dress_up;
mod shape_property;

use std::any::Any;
use std::fmt;
use std::sync::{Arc, OnceLock};

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{Document, ExtensionData, ExtensionValue, ObjectId, Property, prop_status};
use ferrocad_core::property_types;
use ferrocad_geom::{GeometryBackend, Shape};
use ferrocad_types::{Placement, Vector3};

pub use dress_up::{CHAMFER_TYPE, FILLET_TYPE};
pub use containers::{
    COMMON_TYPE, COMPOUND_TYPE, CUT_TYPE, FUSE_TYPE, MULTICOMMON_TYPE, MULTIFUSE_TYPE,
};
pub use shape_property::{
    make_shape_property, resolve_element_in, shape_of, ResolveElementError, ShapeProperty, TYPE_NAME,
};

/// The registered type name of the shape-list property.
pub const SHAPE_LIST_TYPE_NAME: &str = "Part::PropertyTopoShapeList";

/// A list of kernel shapes, held as a [`Property::Extension`] (upstream
/// `Part::PropertyTopoShapeList`).
///
/// The entries use the same per-shape encoding as [`ShapeProperty`] — the injected
/// backend's in-memory bytes (BREP with OCCT) — framed with a little-endian `u64`
/// count and a `u64` byte length per entry, so the list boundaries survive the trip
/// through the document JSON.
///
/// Limitation: the seam's [`Shape`] cannot represent a null shape, so a null
/// `Part.Shape()` cannot be stored as a list entry (upstream keeps it as a null
/// `TopoShape`); the Python converter rejects such an assignment rather than
/// silently dropping or shifting entries.
pub struct ShapeListProperty {
    shapes: Vec<Shape>,
}

impl ShapeListProperty {
    /// An empty list — the default value.
    pub fn empty() -> Self {
        ShapeListProperty { shapes: Vec::new() }
    }

    /// A property holding `shapes`, in order.
    pub fn new(shapes: Vec<Shape>) -> Self {
        ShapeListProperty { shapes }
    }

    /// A copy of the held shapes (shape handles are cheap to clone).
    pub fn shapes(&self) -> Vec<Shape> {
        self.shapes.clone()
    }

    /// Replace the held shapes.
    pub fn set_shapes(&mut self, shapes: Vec<Shape>) {
        self.shapes = shapes;
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// Whether the list holds no shapes.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

impl ExtensionData for ShapeListProperty {
    fn type_name(&self) -> &str {
        SHAPE_LIST_TYPE_NAME
    }

    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(ShapeListProperty {
            shapes: self.shapes.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq(&self, other: &dyn ExtensionData) -> bool {
        let Some(other) = other.as_any().downcast_ref::<ShapeListProperty>() else {
            return false;
        };
        self.shapes.len() == other.shapes.len()
            && self
                .shapes
                .iter()
                .zip(&other.shapes)
                .all(|(a, b)| a.is_same(b))
    }

    fn save(&self) -> Vec<u8> {
        let backend = crate::backend();
        let mut out = Vec::new();
        out.extend_from_slice(&(self.shapes.len() as u64).to_le_bytes());
        for shape in &self.shapes {
            let blob = backend.save_shape(shape);
            out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
            out.extend_from_slice(&blob);
        }
        out
    }

    /// Upstream `Part::PropertyTopoShapeList` writes one member per entry
    /// (`PartShapeN.0.brp`, `PartShapeN.1.brp`, …), so core does not have to
    /// understand the length-framed `save()` blob.
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        let backend = crate::backend();
        Some(
            self.shapes
                .iter()
                .enumerate()
                .map(|(index, shape)| (index.to_string(), backend.save_shape(shape)))
                .collect(),
        )
    }

    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShapeListProperty({} shapes)", self.shapes.len())
    }
}

/// Wrap a [`ShapeListProperty`] as a `Property::Extension` value.
pub fn make_shape_list_property(property: ShapeListProperty) -> Property {
    Property::Extension(ExtensionValue::new(Box::new(property)))
}

/// Borrow the [`ShapeListProperty`] inside `property`, if it is one.
pub fn shape_list_of(property: &Property) -> Option<&ShapeListProperty> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<ShapeListProperty>(),
        _ => None,
    }
}

/// Register `Part::PropertyTopoShapeList`. Idempotent.
fn register_shape_list() {
    property_types::register_extension(
        SHAPE_LIST_TYPE_NAME,
        || make_shape_list_property(ShapeListProperty::empty()),
        restore_shape_list,
    );
}

/// One little-endian `u64` off the front of `rest`.
fn take_u64(rest: &mut &[u8]) -> Option<u64> {
    if rest.len() < 8 {
        return None;
    }
    let (head, tail) = rest.split_at(8);
    *rest = tail;
    let mut word = [0u8; 8];
    word.copy_from_slice(head);
    Some(u64::from_le_bytes(word))
}

fn restore_shape_list(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    if bytes.is_empty() {
        return Some(Box::new(ShapeListProperty::empty()));
    }
    let backend = crate::backend();
    let mut rest = bytes;
    let count = usize::try_from(take_u64(&mut rest)?).ok()?;
    // Each entry needs at least its length prefix; reject a corrupt count before
    // reserving for it.
    if count > rest.len() / 8 {
        return None;
    }
    let mut shapes = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::try_from(take_u64(&mut rest)?).ok()?;
        if len > rest.len() {
            return None;
        }
        let (blob, tail) = rest.split_at(len);
        rest = tail;
        shapes.push(backend.load_shape(blob)?);
    }
    if !rest.is_empty() {
        return None;
    }
    Some(Box::new(ShapeListProperty::new(shapes)))
}

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
    register_shape_list();
    object_registry::register("Part::Feature", Arc::new(Feature));
    object_registry::register(BOX_TYPE, Arc::new(BoxPrimitive));
    object_registry::register(CYLINDER_TYPE, Arc::new(CylinderPrimitive));
    object_registry::register(SPHERE_TYPE, Arc::new(SpherePrimitive));
    object_registry::register(CONE_TYPE, Arc::new(ConePrimitive));
    object_registry::register(REGULAR_POLYGON_TYPE, Arc::new(RegularPolygonPrimitive));
    object_registry::register(
        FUSE_TYPE,
        Arc::new(containers::Boolean {
            kind: containers::BooleanKind::Fuse,
        }),
    );
    object_registry::register(
        CUT_TYPE,
        Arc::new(containers::Boolean {
            kind: containers::BooleanKind::Cut,
        }),
    );
    object_registry::register(
        COMMON_TYPE,
        Arc::new(containers::Boolean {
            kind: containers::BooleanKind::Common,
        }),
    );
    object_registry::register(
        MULTIFUSE_TYPE,
        Arc::new(containers::Multi {
            kind: containers::BooleanKind::Fuse,
        }),
    );
    object_registry::register(
        MULTICOMMON_TYPE,
        Arc::new(containers::Multi {
            kind: containers::BooleanKind::Common,
        }),
    );
    object_registry::register(COMPOUND_TYPE, Arc::new(containers::Compound));
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

/// `Part::Cylinder`: `Radius` × `Height`, growing along the placement's Z axis.
pub const CYLINDER_TYPE: &str = "Part::Cylinder";

struct CylinderPrimitive;

impl ObjectType for CylinderPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Radius".to_string(), Property::Float(2.0), prop_status::NONE),
            ("Height".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Angle".to_string(), Property::Float(360.0), prop_status::NONE),
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
        Some(("Cylinder".to_string(), format!("Part cylinder {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let (radius, height, angle, placement) = {
            let Some(object) = doc.object(id) else {
                return;
            };
            let placement = match object.properties.get("Placement") {
                Some(Property::Placement(placement)) => Some(*placement),
                _ => None,
            };
            (
                float_property(doc, id, "Radius", 2.0),
                float_property(doc, id, "Height", 10.0),
                float_property(doc, id, "Angle", 360.0),
                placement,
            )
        };

        // The bound OCCT constructor only makes a full cylinder.
        if !(radius > 0.0 && height > 0.0) || (angle - 360.0).abs() > 1e-9 {
            doc.mark_invalid(id);
            return;
        }

        let shape = match backend().make_cylinder(
            radius,
            height,
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
        ) {
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

/// `Part::Sphere`: a sphere of `Radius` centred on the placement's base.
pub const SPHERE_TYPE: &str = "Part::Sphere";

struct SpherePrimitive;

impl ObjectType for SpherePrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Radius".to_string(), Property::Float(5.0), prop_status::NONE),
            (
                "Angle1".to_string(),
                Property::Float(-90.0),
                prop_status::NONE,
            ),
            (
                "Angle2".to_string(),
                Property::Float(90.0),
                prop_status::NONE,
            ),
            (
                "Angle3".to_string(),
                Property::Float(360.0),
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
        Some(("Sphere".to_string(), format!("Part sphere {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let (radius, angle1, angle2, angle3, placement) = {
            let Some(object) = doc.object(id) else {
                return;
            };
            let placement = match object.properties.get("Placement") {
                Some(Property::Placement(placement)) => Some(*placement),
                _ => None,
            };
            (
                float_property(doc, id, "Radius", 5.0),
                float_property(doc, id, "Angle1", -90.0),
                float_property(doc, id, "Angle2", 90.0),
                float_property(doc, id, "Angle3", 360.0),
                placement,
            )
        };

        // The bound OCCT constructor only makes a full sphere.
        if !(radius > 0.0)
            || (angle1 + 90.0).abs() > 1e-9
            || (angle2 - 90.0).abs() > 1e-9
            || (angle3 - 360.0).abs() > 1e-9
        {
            doc.mark_invalid(id);
            return;
        }

        let shape = match backend().make_sphere(radius, Vector3::zero()) {
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

/// `Part::Cone`: `Radius1` at the base, `Radius2` at `Height`, along the placement's Z axis.
pub const CONE_TYPE: &str = "Part::Cone";

struct ConePrimitive;

impl ObjectType for ConePrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Radius1".to_string(),
                Property::Float(2.0),
                prop_status::NONE,
            ),
            (
                "Radius2".to_string(),
                Property::Float(4.0),
                prop_status::NONE,
            ),
            (
                "Height".to_string(),
                Property::Float(10.0),
                prop_status::NONE,
            ),
            (
                "Angle".to_string(),
                Property::Float(360.0),
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
        Some(("Cone".to_string(), format!("Part cone {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let (radius1, radius2, height, angle, placement) = {
            let Some(object) = doc.object(id) else {
                return;
            };
            let placement = match object.properties.get("Placement") {
                Some(Property::Placement(placement)) => Some(*placement),
                _ => None,
            };
            (
                float_property(doc, id, "Radius1", 2.0),
                float_property(doc, id, "Radius2", 4.0),
                float_property(doc, id, "Height", 10.0),
                float_property(doc, id, "Angle", 360.0),
                placement,
            )
        };

        // The bound OCCT constructor only makes a full cone.
        if !(height > 0.0) || (angle - 360.0).abs() > 1e-9 {
            doc.mark_invalid(id);
            return;
        }

        let shape = match backend().make_cone(
            radius1,
            radius2,
            height,
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
        ) {
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

/// `Part::RegularPolygon`: a closed wire of `Polygon` edges at `Circumradius`.
///
/// As upstream, the first vertex lies on the placement's X axis and the wire
/// stays on the placement's XY plane.
pub const REGULAR_POLYGON_TYPE: &str = "Part::RegularPolygon";

struct RegularPolygonPrimitive;

/// Read an integer property, or the default when it is absent/another kind.
fn integer_property(doc: &Document, id: ObjectId, name: &str, default: i64) -> i64 {
    doc.object(id)
        .and_then(|object| object.properties.get(name))
        .and_then(|value| match value {
            Property::Integer(value) => Some(*value),
            Property::Float(value) => Some(*value as i64),
            _ => None,
        })
        .unwrap_or(default)
}

impl ObjectType for RegularPolygonPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Polygon".to_string(),
                Property::Integer(6),
                prop_status::NONE,
            ),
            (
                "Circumradius".to_string(),
                Property::Float(2.0),
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
        Some((
            "RegularPolygon".to_string(),
            format!("Part regular polygon {name}"),
        ))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let (sides, circumradius, placement) = {
            let Some(object) = doc.object(id) else {
                return;
            };
            let placement = match object.properties.get("Placement") {
                Some(Property::Placement(placement)) => Some(*placement),
                _ => None,
            };
            (
                integer_property(doc, id, "Polygon", 6),
                float_property(doc, id, "Circumradius", 2.0),
                placement,
            )
        };

        if sides < 3 || !(circumradius > 0.0) {
            doc.mark_invalid(id);
            return;
        }

        let step = std::f64::consts::TAU / sides as f64;
        let points = (0..sides)
            .map(|index| {
                let angle = step * index as f64;
                Vector3::new(circumradius * angle.cos(), circumradius * angle.sin(), 0.0)
            })
            .collect::<Vec<_>>();
        let curves = match ferrocad_curves::polygon(&points, true) {
            Ok(curves) => curves,
            Err(_) => {
                doc.mark_invalid(id);
                return;
            }
        };

        let mut edges = Vec::with_capacity(curves.len());
        for curve in &curves {
            match backend().make_edge(curve) {
                Ok(edge) => edges.push(edge),
                Err(_) => {
                    doc.mark_invalid(id);
                    return;
                }
            }
        }
        let shape = match backend().make_wire(&edges) {
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

    /// Two overlapping 10 mm boxes, recomputed and ready to link.
    fn two_boxes(doc: &mut Document) {
        two_boxes_at(doc, 5.0, 0.0);
    }

    /// Two 10 mm boxes, the second offset by `(x, y)`, recomputed and ready to link.
    fn two_boxes_at(doc: &mut Document, x: f64, y: f64) {
        for name in ["BoxA", "BoxB"] {
            let id = doc.add_object(name, BOX_TYPE);
            doc.set_property(id, "Length", Property::Float(10.0)).unwrap();
            doc.set_property(id, "Width", Property::Float(10.0)).unwrap();
            doc.set_property(id, "Height", Property::Float(10.0)).unwrap();
            let placement = if name == "BoxB" {
                Placement {
                    base: ferrocad_types::Vector3::new(x, y, 0.0),
                    rotation: ferrocad_types::Rotation::identity(),
                }
            } else {
                Placement::identity()
            };
            doc.set_property(id, "Placement", Property::Placement(placement))
                .unwrap();
        }
        doc.recompute().unwrap();
    }

    fn stored_shape(doc: &Document, id: ObjectId) -> Shape {
        shape_of(doc.object(id).unwrap().properties.get("Shape").unwrap())
            .unwrap()
            .shape()
            .expect("the feature produced a shape")
    }

    #[test]
    fn a_part_cylinder_executes_into_a_cylinder() {
        setup();
        let direct = backend()
            .make_cylinder(2.0, 10.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("the seam builds a cylinder");
        assert!(backend().mass_properties(&direct).unwrap().volume > 0.0);
        let mut doc = Document::new();
        let id = doc.add_object("Cylinder", CYLINDER_TYPE);
        doc.set_property(id, "Radius", Property::Float(2.0)).unwrap();
        doc.recompute().unwrap();
        let shape = stored_shape(&doc, id);
        let mass = backend()
            .mass_properties(&shape)
            .expect("a cylinder has mass properties");
        assert!((mass.volume - std::f64::consts::PI * 4.0 * 10.0).abs() < 1e-6);
    }

    #[test]
    fn a_part_sphere_executes_into_a_sphere() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Sphere", SPHERE_TYPE);
        doc.set_property(id, "Radius", Property::Float(5.0)).unwrap();
        doc.recompute().unwrap();
        let object = doc.object(id).unwrap();
        assert!(!object.invalid);
        let shape = stored_shape(&doc, id);
        let mass = backend()
            .mass_properties(&shape)
            .expect("a sphere has mass properties");
        let expected = 4.0 / 3.0 * std::f64::consts::PI * 125.0;
        assert!((mass.volume - expected).abs() < 1e-6);
    }

    #[test]
    fn a_part_cone_executes_into_a_cone() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Cone", CONE_TYPE);
        doc.set_property(id, "Radius1", Property::Float(2.0)).unwrap();
        doc.set_property(id, "Radius2", Property::Float(1.0)).unwrap();
        doc.set_property(id, "Height", Property::Float(3.0)).unwrap();
        doc.recompute().unwrap();
        let object = doc.object(id).unwrap();
        assert!(!object.invalid);
        let shape = stored_shape(&doc, id);
        let mass = backend()
            .mass_properties(&shape)
            .expect("a cone has mass properties");
        let expected = std::f64::consts::PI * 3.0 / 3.0 * (4.0 + 2.0 + 1.0);
        assert!((mass.volume - expected).abs() < 1e-6);
    }

    #[test]
    fn a_regular_polygon_executes_into_a_wire() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Triangle", REGULAR_POLYGON_TYPE);
        doc.set_property(id, "Polygon", Property::Integer(3))
            .unwrap();
        doc.set_property(id, "Circumradius", Property::Float(1.0))
            .unwrap();
        doc.recompute().unwrap();
        assert!(!doc.object(id).unwrap().invalid);
        let shape = stored_shape(&doc, id);
        assert_eq!(
            backend()
                .elements(&shape, ferrocad_geom::ElementKind::Edge)
                .len(),
            3
        );
    }

    #[test]
    fn a_regular_polygon_rebuilds_when_the_side_count_changes() {
        setup();
        let mut doc = Document::new();
        let id = doc.add_object("Polygon", REGULAR_POLYGON_TYPE);
        doc.set_property(id, "Circumradius", Property::Float(1.0))
            .unwrap();
        doc.recompute().unwrap();
        doc.set_property(id, "Polygon", Property::Integer(8))
            .unwrap();
        doc.recompute().unwrap();
        let shape = stored_shape(&doc, id);
        assert_eq!(
            backend()
                .elements(&shape, ferrocad_geom::ElementKind::Edge)
                .len(),
            8
        );
    }

    #[test]
    fn booleans_execute_from_linked_boxes() {
        setup();
        let mut doc = Document::new();
        two_boxes(&mut doc);

        for (type_id, name) in [(FUSE_TYPE, "Fuse"), (CUT_TYPE, "Cut"), (COMMON_TYPE, "Common")] {
            let id = doc.add_object(name, type_id);
            doc.set_property(id, "Base", Property::Link("BoxA".into()))
                .unwrap();
            doc.set_property(id, "Tool", Property::Link("BoxB".into()))
                .unwrap();
            doc.recompute().unwrap();
            let shape = stored_shape(&doc, id);
            assert!(
                backend().resolve(&shape, "Face1").is_some(),
                "{name} produced a shape"
            );
            assert!(!doc.object(id).unwrap().invalid, "{name} is valid");
        }
    }

    #[test]
    fn multi_booleans_fold_their_linked_boxes() {
        setup();
        let mut doc = Document::new();
        two_boxes(&mut doc);

        for (type_id, name) in [(MULTIFUSE_TYPE, "MultiFuse"), (MULTICOMMON_TYPE, "MultiCommon")] {
            let id = doc.add_object(name, type_id);
            doc.set_property(
                id,
                "Shapes",
                Property::LinkList(vec!["BoxA".into(), "BoxB".into()]),
            )
            .unwrap();
            doc.recompute().unwrap();
            let shape = stored_shape(&doc, id);
            assert!(
                backend().resolve(&shape, "Face1").is_some(),
                "{name} produced a shape"
            );
            assert!(!doc.object(id).unwrap().invalid, "{name} is valid");
            if type_id == MULTIFUSE_TYPE {
                assert!(
                    !stored_property(&doc, id).lineage().map().is_empty(),
                    "{name} folds its operation histories into one lineage"
                );
            }
        }
    }

    #[test]
    fn a_compound_contains_its_linked_shapes() {
        setup();
        let mut doc = Document::new();
        two_boxes(&mut doc);

        let id = doc.add_object("Compound", COMPOUND_TYPE);
        doc.set_property(
            id,
            "Links",
            Property::LinkList(vec!["BoxA".into(), "BoxB".into()]),
        )
        .unwrap();
        doc.recompute().unwrap();
        let shape = stored_shape(&doc, id);
        assert_eq!(
            backend()
                .elements(&shape, ferrocad_geom::ElementKind::Solid)
                .len(),
            2
        );
    }

    #[test]
    fn a_boolean_without_tools_is_invalid() {
        setup();
        let mut doc = Document::new();
        two_boxes(&mut doc);
        let id = doc.add_object("Lonely", FUSE_TYPE);
        doc.set_property(id, "Base", Property::Link("BoxA".into()))
            .unwrap();
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

    /// The `ShapeProperty` stored under `id`'s `Shape`.
    fn stored_property(doc: &Document, id: ObjectId) -> &ShapeProperty {
        shape_of(doc.object(id).unwrap().properties.get("Shape").unwrap())
            .expect("the Shape property is a ShapeProperty")
    }

    /// Whether `name` is a backend positional name (`Face6`, `Edge12`, `Vertex3`, …).
    fn is_positional_name(name: &str) -> bool {
        ["Solid", "Shell", "Face", "Wire", "Edge", "Vertex"]
            .iter()
            .any(|prefix| {
                name.strip_prefix(prefix)
                    .and_then(|ordinal| ordinal.parse::<usize>().ok())
                    .is_some_and(|ordinal| ordinal >= 1)
            })
    }

    fn boolean(doc: &mut Document, name: &str, type_id: &str) -> ObjectId {
        let id = doc.add_object(name, type_id);
        doc.set_property(id, "Base", Property::Link("BoxA".into()))
            .unwrap();
        doc.set_property(id, "Tool", Property::Link("BoxB".into()))
            .unwrap();
        doc.recompute().unwrap();
        id
    }

    #[test]
    fn booleans_store_a_positional_lineage() {
        setup();
        let mut doc = Document::new();
        // Corner overlap: the cut keeps more output names than the tool deletes, so
        // both maps survive the fold's retire-by-name pass.
        two_boxes_at(&mut doc, 5.0, 5.0);

        for (type_id, name) in [(FUSE_TYPE, "Fuse"), (CUT_TYPE, "Cut")] {
            let id = boolean(&mut doc, name, type_id);
            let property = stored_property(&doc, id);
            let lineage = property.lineage();

            assert!(
                !lineage.map().is_empty(),
                "{name} kept a non-empty forward map"
            );
            assert!(
                !lineage.reverse().is_empty(),
                "{name} kept a non-empty reverse map"
            );
            assert_eq!(lineage.version(), &ferrocad_geom::MapVersion::default());

            let mut faces = 0;
            for (element, _) in lineage.map().iter() {
                assert!(
                    is_positional_name(element),
                    "{name} forward key {element:?} is a positional name"
                );
                if element.starts_with("Face") {
                    faces += 1;
                }
            }
            assert!(faces > 0, "{name} names faces");

            for (output, inputs) in lineage.reverse().iter() {
                assert!(
                    is_positional_name(output),
                    "{name} reverse key {output:?} is a positional name"
                );
                assert!(!inputs.is_empty(), "{name} {output} has a producer");
                for input in inputs {
                    assert!(
                        is_positional_name(input),
                        "{name} producer {input:?} is a positional name"
                    );
                }
            }
        }
    }

    #[test]
    fn a_boolean_lineage_survives_a_json_round_trip() {
        setup();
        let mut doc = Document::new();
        two_boxes_at(&mut doc, 5.0, 5.0);
        let id = boolean(&mut doc, "Fuse", FUSE_TYPE);

        let original = stored_property(&doc, id).lineage().clone();
        assert!(!original.map().is_empty());
        assert!(!original.reverse().is_empty());

        let value = doc
            .object(id)
            .unwrap()
            .properties
            .get("Shape")
            .unwrap()
            .clone();
        let json = serde_json::to_string(&value).unwrap();
        let back: Property = serde_json::from_str(&json).unwrap();
        let restored = shape_of(&back).unwrap();
        assert!(restored.shape().is_some(), "the shape survives");

        let lineage = restored.lineage();
        assert_eq!(lineage.version(), original.version());
        assert_eq!(lineage.map().len(), original.map().len());
        assert_eq!(
            lineage
                .map()
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            original
                .map()
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            "the forward names round-trip"
        );
        assert_eq!(
            lineage
                .reverse()
                .iter()
                .map(|(output, inputs)| (output, inputs.to_vec()))
                .collect::<Vec<_>>(),
            original
                .reverse()
                .iter()
                .map(|(output, inputs)| (output, inputs.to_vec()))
                .collect::<Vec<_>>(),
            "the reverse relations round-trip"
        );
        assert!(
            lineage
                .map()
                .iter()
                .any(|(name, _)| name.starts_with("Face")),
            "the restored map still names faces"
        );
    }

    #[test]
    fn legacy_raw_brep_still_restores() {
        setup();
        let shape = make_box(10.0, 10.0, 10.0).unwrap();
        let raw = backend().save_shape(&shape);
        assert!(!raw.is_empty());
        assert!(!raw.starts_with(b"FCSHPROP"), "raw BREP is not enveloped");

        let restored =
            property_types::restore_extension(TYPE_NAME, &raw).expect("legacy raw BREP loads");
        let property = restored
            .as_any()
            .downcast_ref::<ShapeProperty>()
            .expect("restores a ShapeProperty");
        assert!(
            property.lineage().map().is_empty(),
            "legacy bytes carry no lineage"
        );
        assert!(property.lineage().reverse().is_empty());
        let shape = property.shape().expect("the legacy shape");
        assert!(backend().resolve(&shape, "Face6").is_some());
    }

    #[test]
    fn an_empty_history_leaves_the_lineage_empty() {
        setup();
        let mut doc = Document::new();
        two_boxes(&mut doc);

        let id = doc.add_object("Compound", COMPOUND_TYPE);
        doc.set_property(
            id,
            "Links",
            Property::LinkList(vec!["BoxA".into(), "BoxB".into()]),
        )
        .unwrap();
        doc.recompute().unwrap();

        let property = stored_property(&doc, id);
        assert!(property.lineage().map().is_empty());
        assert!(property.lineage().reverse().is_empty());
    }
}

//! `Mesh::PropertyMeshKernel` and `Mesh::Feature`.
//!
//! The property value is a [`Mesh`] behind [`Property::Extension`], with a small
//! hand-written binary envelope for persistence. The object type adds the two
//! properties FreeCAD's `Mesh::Feature` has: `Mesh` (this property) and `Placement`.
//!
//! # Persisted form
//!
//! ```text
//! "FCMESHPROP" | version:u16
//!              | point-count:u64  (x:f64, y:f64, z:f64)*
//!              | facet-count:u64  (a:u32, b:u32, c:u32)*
//!              | base(x,y,z:f64) | quaternion(w,x,y,z:f64)
//! ```
//!
//! Lengths are little-endian. The version is [`ENVELOPE_VERSION`] (1); an
//! unrecognised version is rejected rather than guessed at. The placement's
//! quaternion is stored without the cached `raw_axis`, matching
//! `Rotation`'s equality rule.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::property_types;
use ferrocad_core::{
    Document, ExtensionData, ExtensionValue, ObjectId, Placement, Property, Rotation, Vector3,
    prop_status,
};

use crate::mesh::Mesh;

/// The registered property type name (`Mesh::PropertyMeshKernel`, upstream's
/// `Mesh::PropertyMeshKernel`).
pub const TYPE_NAME: &str = "Mesh::PropertyMeshKernel";

/// The registered object type name (`Mesh::Feature`).
pub const FEATURE_TYPE: &str = "Mesh::Feature";

/// The envelope magic. A raw mesh file has no such prefix, so the forms cannot be
/// confused.
const MAGIC: &[u8; 10] = b"FCMESHPROP";

/// The envelope layout version.
const ENVELOPE_VERSION: u16 = 1;

/// A mesh-valued property (`Mesh::PropertyMeshKernel`).
#[derive(Debug, Clone, PartialEq)]
pub struct MeshProperty {
    mesh: Mesh,
}

impl MeshProperty {
    /// An empty mesh — the default value.
    pub fn empty() -> Self {
        MeshProperty {
            mesh: Mesh::empty(),
        }
    }

    /// A property holding `mesh`.
    pub fn new(mesh: Mesh) -> Self {
        MeshProperty { mesh }
    }

    /// The held mesh.
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// The held mesh, mutably.
    pub fn mesh_mut(&mut self) -> &mut Mesh {
        &mut self.mesh
    }

    /// Replace the held mesh.
    pub fn set_mesh(&mut self, mesh: Mesh) {
        self.mesh = mesh;
    }

    /// Consume the property and return its mesh.
    pub fn into_mesh(self) -> Mesh {
        self.mesh
    }
}

impl ExtensionData for MeshProperty {
    fn type_name(&self) -> &str {
        TYPE_NAME
    }

    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<MeshProperty>()
            .is_some_and(|other| other.mesh == self.mesh)
    }

    fn save(&self) -> Vec<u8> {
        let capacity = MAGIC.len()
            + 2
            + 8
            + self.mesh.count_points() * 24
            + 8
            + self.mesh.count_facets() * 12
            + 3 * 8
            + 4 * 8;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(MAGIC);
        put_u16(&mut out, ENVELOPE_VERSION);

        put_u64(&mut out, self.mesh.count_points() as u64);
        for point in self.mesh.points() {
            put_f64(&mut out, point.x);
            put_f64(&mut out, point.y);
            put_f64(&mut out, point.z);
        }

        put_u64(&mut out, self.mesh.count_facets() as u64);
        for facet in self.mesh.facets() {
            for &index in facet {
                put_u32(&mut out, index);
            }
        }

        let placement = self.mesh.placement();
        put_f64(&mut out, placement.base.x);
        put_f64(&mut out, placement.base.y);
        put_f64(&mut out, placement.base.z);
        for component in placement.rotation.q {
            put_f64(&mut out, component);
        }
        out
    }

    /// `.FCStd` mesh members are a later slice: they need a core change first.
    ///
    /// The intended shape is one **single-payload** entry — an empty suffix and
    /// the bytes [`Mesh::write_bms`](Mesh::write_bms) produces — so upstream's
    /// `MeshKernelN.bms` / `<Mesh file="…"/>` pair can be emitted. Core's
    /// `.FCStd` writer currently hardcodes the `PartShapeN` stem and the `<Part>`
    /// element, so returning the bytes today would mislabel the member (and
    /// therefore the property type on restore). `MeshProperty` should also learn
    /// the matching restore path: read the member bytes with
    /// [`Mesh::read_bms`](Mesh::read_bms) and keep the object's `Placement`
    /// authoritative. A native (`.ferrocad`) document persists this property
    /// through [`save`](ExtensionData::save).
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        None
    }

    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MeshProperty({} points, {} facets)",
            self.mesh.count_points(),
            self.mesh.count_facets()
        )
    }
}

/// Wrap a [`MeshProperty`] as a `Property::Extension` value.
pub fn make_mesh_property(property: MeshProperty) -> Property {
    Property::Extension(ExtensionValue::new(Box::new(property)))
}

/// Borrow the [`MeshProperty`] inside `property`, if it is one.
pub fn mesh_of(property: &Property) -> Option<&MeshProperty> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<MeshProperty>(),
        _ => None,
    }
}

/// Register `Mesh::PropertyMeshKernel` and `Mesh::Feature`. Idempotent through
/// [`crate::register`].
pub(crate) fn register() {
    property_types::register_extension(
        TYPE_NAME,
        || make_mesh_property(MeshProperty::empty()),
        restore,
    );
    object_registry::register(FEATURE_TYPE, Arc::new(Feature));
}

/// `Mesh::Feature`: a mesh plus a placement.
struct Feature;

impl ObjectType for Feature {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Mesh".to_string(),
                make_mesh_property(MeshProperty::empty()),
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
        Some(("Base".to_string(), format!("Mesh feature {name}")))
    }

    /// Keep `Placement` and the mesh transform in sync, upstream
    /// `MeshFeature::onChanged`.
    ///
    /// - `Placement` assignment mirrors the object transform into the mesh, so
    ///   `obj.Mesh.Topology` and `obj.Mesh.BoundBox` follow it **without** a recompute.
    /// - `Mesh` assignment adopts the new mesh's transform into `Placement`.
    ///
    /// Both writes use [`Document::set_property_raw`], which neither records an undo
    /// step nor calls this hook, so the pair cannot recurse. Undo/redo re-runs this
    /// hook from [`Document`]'s restore path with the same result.
    fn on_property_changed(&self, doc: &mut Document, id: ObjectId, name: &str) {
        match name {
            "Placement" => {
                let Some(Property::Placement(placement)) = doc
                    .object(id)
                    .and_then(|object| object.properties.get("Placement"))
                    .cloned()
                else {
                    return;
                };
                let Some(mut property) = doc
                    .object(id)
                    .and_then(|object| object.properties.get("Mesh"))
                    .and_then(mesh_of)
                    .cloned()
                else {
                    return;
                };
                property.mesh_mut().set_placement(placement);
                doc.set_property_raw(id, "Mesh", make_mesh_property(property));
            }
            "Mesh" => {
                let Some(placement) = doc
                    .object(id)
                    .and_then(|object| object.properties.get("Mesh"))
                    .and_then(mesh_of)
                    .map(|property| property.mesh().placement())
                else {
                    return;
                };
                doc.set_property_raw(id, "Placement", Property::Placement(placement));
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Envelope encoding
// ---------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Bounds-checked forward reader over the envelope body.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(rest: &'a [u8]) -> Self {
        Reader { rest }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.rest.len() < count {
            return None;
        }
        let (head, tail) = self.rest.split_at(count);
        self.rest = tail;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn remaining(&self) -> usize {
        self.rest.len()
    }

    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
}

fn restore(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    // An old or default value may have no bytes at all.
    if bytes.is_empty() {
        return Some(Box::new(MeshProperty::empty()));
    }

    let mut reader = Reader::new(bytes);
    if reader.take(MAGIC.len())? != MAGIC.as_slice() {
        return None;
    }
    if reader.u16()? != ENVELOPE_VERSION {
        return None;
    }

    let point_count = usize::try_from(reader.u64()?).ok()?;
    // Every point is three f64. Refuse an impossible count before allocating.
    if point_count.checked_mul(24)? > reader.remaining() {
        return None;
    }
    let mut points = Vec::with_capacity(point_count);
    for _ in 0..point_count {
        points.push(Vector3::new(reader.f64()?, reader.f64()?, reader.f64()?));
    }

    let facet_count = usize::try_from(reader.u64()?).ok()?;
    // Every facet is three u32.
    if facet_count.checked_mul(12)? > reader.remaining() {
        return None;
    }
    let mut facets = Vec::with_capacity(facet_count);
    for _ in 0..facet_count {
        facets.push([reader.u32()?, reader.u32()?, reader.u32()?]);
    }

    let base = Vector3::new(reader.f64()?, reader.f64()?, reader.f64()?);
    let q = [reader.f64()?, reader.f64()?, reader.f64()?, reader.f64()?];
    if !reader.is_empty() {
        return None;
    }

    // `new` re-runs the structural validation; a corrupt envelope is rejected
    // rather than returned as an invalid mesh.
    let mut mesh = Mesh::new(points, facets).ok()?;
    mesh.set_placement(Placement {
        base,
        rotation: Rotation { q, raw_axis: None },
    });
    Some(Box::new(MeshProperty::new(mesh)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_core::{Document, ObjectId, SavedDocument};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ferrocad-mesh-doc-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    fn stored_mesh(doc: &Document, id: ObjectId) -> &Mesh {
        mesh_of(doc.object(id).unwrap().properties.get("Mesh").unwrap())
            .expect("the object owns a Mesh::PropertyMeshKernel")
            .mesh()
    }

    #[test]
    fn placement_and_the_mesh_transform_stay_in_sync() {
        crate::register();
        let mut doc = Document::new();
        let id = doc.add_object("MeshObj", FEATURE_TYPE);
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        doc.set_property(id, "Mesh", make_mesh_property(MeshProperty::new(mesh)))
            .unwrap();
        assert_eq!(
            doc.object(id).unwrap().properties.get("Placement"),
            Some(&Property::Placement(Placement::identity())),
            "an identity mesh keeps the object placement"
        );

        // `Placement` assignment mirrors into the stored mesh transform.
        let moved = Placement::new(Vector3::new(1.0, 2.0, 3.0), Rotation::identity());
        doc.set_property(id, "Placement", Property::Placement(moved))
            .unwrap();
        assert_eq!(stored_mesh(&doc, id).placement(), moved);

        // `Mesh` assignment adopts the mesh's transform into `Placement`.
        let mut shifted = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let adopted = Placement::new(Vector3::new(-1.0, 0.0, 0.0), Rotation::identity());
        shifted.set_placement(adopted);
        doc.set_property(id, "Mesh", make_mesh_property(MeshProperty::new(shifted)))
            .unwrap();
        assert_eq!(
            doc.object(id).unwrap().properties.get("Placement"),
            Some(&Property::Placement(adopted))
        );

        // Undo restores both sides of the pair.
        doc.open_transaction_named("move");
        let later = Placement::new(Vector3::new(5.0, 5.0, 5.0), Rotation::identity());
        doc.set_property(id, "Placement", Property::Placement(later))
            .unwrap();
        doc.commit_transaction();
        assert_eq!(stored_mesh(&doc, id).placement(), later);
        assert!(doc.undo());
        assert_eq!(stored_mesh(&doc, id).placement(), adopted);
    }

    #[test]
    fn registers_the_property_and_object_types() {
        crate::register();
        assert!(property_types::contains(TYPE_NAME));
        assert!(object_registry::is_registered(FEATURE_TYPE));
    }

    #[test]
    fn the_envelope_round_trips_a_mesh_and_its_placement() {
        let mut mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        let placement = Placement::new(
            Vector3::new(1.0, 2.0, 3.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), 0.5),
        );
        mesh.set_placement(placement);
        let property = MeshProperty::new(mesh.clone());

        let bytes = property.save();
        let restored = restore(&bytes).expect("the envelope restores");
        let restored = restored
            .as_any()
            .downcast_ref::<MeshProperty>()
            .expect("a MeshProperty");
        assert_eq!(restored.mesh(), &mesh);
        assert!(restored.mesh().is_solid());
    }

    #[test]
    fn an_empty_property_round_trips() {
        let property = MeshProperty::empty();
        let bytes = property.save();
        let restored = restore(&bytes).expect("the envelope restores");
        let restored = restored
            .as_any()
            .downcast_ref::<MeshProperty>()
            .expect("a MeshProperty");
        assert_eq!(restored, &property);

        // No bytes at all is the default value, not an error.
        let restored = restore(&[]).expect("empty bytes are the default");
        assert_eq!(
            restored
                .as_any()
                .downcast_ref::<MeshProperty>()
                .expect("a MeshProperty"),
            &MeshProperty::empty()
        );
    }

    #[test]
    fn a_corrupt_envelope_is_rejected() {
        assert!(restore(b"not a mesh envelope").is_none());

        let mut bytes = MeshProperty::empty().save();
        bytes[..MAGIC.len()].copy_from_slice(b"XXXXXXXXXX");
        assert!(restore(&bytes).is_none());
    }

    #[test]
    fn a_mesh_feature_gets_mesh_and_placement() {
        crate::register();
        let mut doc = Document::new();
        let id = doc.add_object("MeshObj", FEATURE_TYPE);
        let object = doc.object(id).unwrap();
        assert_eq!(
            mesh_of(object.properties.get("Mesh").unwrap()).unwrap(),
            &MeshProperty::empty()
        );
        assert_eq!(
            object.properties.get("Placement"),
            Some(&Property::Placement(Placement::identity()))
        );
    }

    #[test]
    fn a_mesh_round_trips_through_the_document_in_process() {
        crate::register();
        let mut doc = Document::new();
        let id = doc.add_object("MeshObj", FEATURE_TYPE);
        let mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        doc.set_property(
            id,
            "Mesh",
            make_mesh_property(MeshProperty::new(mesh.clone())),
        )
        .unwrap();

        assert_eq!(stored_mesh(&doc, id), &mesh);

        // Native JSON, in memory.
        let saved = doc.to_saved("MeshRoundTrip");
        let json = serde_json::to_string(&saved).unwrap();
        let saved: SavedDocument = serde_json::from_str(&json).unwrap();
        let restored = Document::from_saved(&saved);
        let restored_mesh = stored_mesh(&restored, id);
        assert_eq!(restored_mesh, &mesh);
        assert!(restored_mesh.is_solid());
    }

    #[test]
    fn a_mesh_round_trips_through_a_native_file() {
        crate::register();
        let mut doc = Document::new();
        let id = doc.add_object("MeshObj", FEATURE_TYPE);
        let mesh = Mesh::box_(1.0, 2.0, 3.0).unwrap();
        doc.set_property(
            id,
            "Mesh",
            make_mesh_property(MeshProperty::new(mesh.clone())),
        )
        .unwrap();

        let path = temp_path("mesh.ferrocad");
        doc.save_to_file("MeshFileRoundTrip", path.to_str().unwrap())
            .unwrap();
        let saved = Document::load_from_file(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);

        let restored = Document::from_saved(&saved);
        let restored_mesh = stored_mesh(&restored, id);
        assert_eq!(restored_mesh, &mesh);
        assert_eq!(restored_mesh.volume(), 6.0);
        assert!(restored_mesh.is_solid());
    }
}

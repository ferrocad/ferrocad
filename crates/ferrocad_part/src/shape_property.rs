//! `Part::PropertyPartShape`: a kernel shape held as a [`Property::Extension`].
//!
//! The value wraps a [`ferrocad_geom::Shape`] directly — no lock. A document is shared
//! behind an `Arc<Mutex<…>>`, so its contents need only be `Send`; every access to a
//! property already runs under that document lock. Its `save`/`restore` go through the
//! **injected** backend's in-memory encoding, so a shape persists through the ordinary
//! document JSON without Part naming a kernel or core knowing anything about it.
//!
//! # Persisted form
//!
//! A shape alone is not enough: a boolean's result also carries its sub-shape
//! [`Lineage`] (which input element means which output element). `save` therefore
//! writes a small **versioned envelope**:
//!
//! ```text
//! "FCSHPROP" | version:u16 | shape-breplen:u64 | shape-brep
//!             | encoding:u16 | policy:str | kernel:opt-str
//!             | forward-count:u64  (name:str, breplen:u64, brep)*
//!             | reverse-count:u64  (output:str, input-count:u64, input:str*)*
//! ```
//!
//! Lengths are little-endian; a `str` is a `u32` byte length plus UTF-8 bytes. The
//! main shape and every lineage sub-shape keep their raw BREP bytes inside the
//! envelope.
//!
//! **Old documents stay readable.** [`restore`] treats anything without the
//! `FCSHPROP` prefix as legacy raw BREP and hands it straight to the backend, so a
//! document written before the envelope still loads (with an empty lineage).
//!
//! The envelope is a *native-document* encoding. [`fcstd_members`] keeps writing
//! raw BREP for `PartShapeN.brp`, so an upstream `.FCStd` reader still sees a BREP
//! file. [`fcstd_sidecar`] additionally writes the envelope to
//! `PartShapeN.lineage`, referenced from the `<Part>` element by a
//! `FerroCADSidecar` attribute upstream ignores; our reader prefers that member and
//! so restores the lineage, while a file without it (upstream, or an older
//! FerroCAD save) still loads the geometry with an empty lineage.

use std::any::Any;
use std::collections::BTreeSet;
use std::fmt;

use ferrocad_core::property_types;
use ferrocad_core::{ExtensionData, ExtensionValue, Property};
use ferrocad_geom::{ElementKind, ElementRef, History, Lineage, MapVersion, Shape};

/// The registered property type name.
pub const TYPE_NAME: &str = "Part::PropertyPartShape";

/// The envelope's magic prefix. A raw BREP file starts with `DBRep_…`/`CASCADE …`,
/// so the two forms cannot be confused.
const MAGIC: &[u8; 8] = b"FCSHPROP";

/// The envelope layout version. An unrecognised version is rejected rather than
/// guessed at; the *legacy* raw-BREP form has no version at all and is always tried.
const ENVELOPE_VERSION: u16 = 1;

/// A shape-valued property.
///
/// The lineage is carried **beside** the shape, never used for equality: two
/// properties are equal when their shapes are the same handle, whatever maps they
/// hold. The map is how a stored name is resolved — see
/// [`resolve_element`](ShapeProperty::resolve_element) — not part of what the shape
/// *is*.
pub struct ShapeProperty {
    shape: Option<Shape>,
    lineage: Lineage,
}

impl ShapeProperty {
    /// An empty (unset) shape — the default value.
    pub fn empty() -> Self {
        ShapeProperty {
            shape: None,
            lineage: Lineage::default(),
        }
    }

    /// A property holding `shape`, with an empty lineage.
    ///
    /// Used for shapes with no operation history (a primitive, a shape assigned by
    /// Python, a legacy restore); an operation result uses
    /// [`with_lineage`](ShapeProperty::with_lineage).
    pub fn new(shape: Shape) -> Self {
        ShapeProperty {
            shape: Some(shape),
            lineage: Lineage::default(),
        }
    }

    /// A property holding `shape` and the `lineage` produced with it.
    pub fn with_lineage(shape: Shape, lineage: Lineage) -> Self {
        ShapeProperty {
            shape: Some(shape),
            lineage,
        }
    }

    /// A copy of the held shape (shape handles are cheap to clone), if any.
    pub fn shape(&self) -> Option<Shape> {
        self.shape.clone()
    }

    /// Replace the held shape.
    ///
    /// The lineage is cleared: names recorded against the old shape have no meaning
    /// for a new one, and a stale map is worse than no map.
    pub fn set_shape(&mut self, shape: Shape) {
        self.shape = Some(shape);
        self.lineage = Lineage::default();
    }

    /// The lineage folded alongside the shape (empty when there is none).
    pub fn lineage(&self) -> &Lineage {
        &self.lineage
    }

    /// Replace the lineage, keeping the shape.
    pub fn set_lineage(&mut self, lineage: Lineage) {
        self.lineage = lineage;
    }

    /// Clear the held shape (and its lineage).
    pub fn clear(&mut self) {
        self.shape = None;
        self.lineage = Lineage::default();
    }

    /// Resolve a stored sub-element name (`Edge1`, `Face3`) to the sub-shape it
    /// denotes, consulting the lineage first. See [`resolve_element_in`] for the
    /// exact rule.
    pub fn resolve_element(&self, name: &str) -> Result<Shape, ResolveElementError> {
        let shape = self.shape.as_ref().ok_or(ResolveElementError::NoShape)?;
        resolve_element_in(shape, &self.lineage, name)
    }

    /// The raw backend encoding of the held shape, without the envelope.
    ///
    /// This is what upstream's `.FCStd` `PartShapeN.brp` member expects.
    fn brep(&self) -> Vec<u8> {
        match self.shape.as_ref() {
            Some(shape) => crate::backend().save_shape(shape),
            None => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolving a stored sub-element reference
// ---------------------------------------------------------------------------

/// Why a stored sub-element name could not be turned into a kernel sub-shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveElementError {
    /// The property holds no shape, so no name can resolve.
    NoShape,
    /// The shape carries a lineage and its forward map does not contain the name:
    /// the element was retired, or the name never belonged to this lineage. The
    /// reference fails instead of falling back positionally, since a positional
    /// guess could silently select a different element.
    Retired,
    /// The shape carries no lineage and the backend has no positional element with
    /// this name.
    Missing,
}

impl fmt::Display for ResolveElementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoShape => f.write_str("the property holds no shape"),
            Self::Retired => f.write_str("the name is not in the shape's lineage"),
            Self::Missing => f.write_str("the name does not exist on the shape"),
        }
    }
}

/// Resolve `name` against `shape`, consulting `lineage` first.
///
/// The rule is *preserved or invalid, never moved*:
///
/// 1. **A name present in the forward map resolves to the mapped sub-shape.** The
///    map records where a name went across earlier operations (a boolean's face 6
///    is now output face 3), so it outranks the name's ordinal — which may denote a
///    different element after an inserted or deleted one.
/// 2. **A name absent from the map falls back to positional resolution only when the
///    shape carries no lineage** — that is, when the forward map is empty. With no
///    relations there is nothing for a positional meaning to contradict, and
///    `Edge1` meaning the first edge is all that can be known (a primitive, a shape
///    assigned from Python, a legacy restore).
/// 3. **A name absent from a non-empty map is [`Retired`](ResolveElementError::Retired).**
///    The lineage exists and does not justify the name: it was retired by an
///    operation, or it names something that never lived here. Falling back
///    positionally would be a guess that could retarget the reference to another
///    edge, so it fails loudly instead — a stale reference makes its feature invalid
///    rather than silently dressing the wrong element.
///
/// The map is read, never written: resolution does not bind, retire or rename.
pub fn resolve_element_in(
    shape: &Shape,
    lineage: &Lineage,
    name: &str,
) -> Result<Shape, ResolveElementError> {
    if let Some(mapped) = lineage.map().get(name) {
        return Ok(mapped.clone());
    }
    if !lineage.map().is_empty() {
        return Err(ResolveElementError::Retired);
    }
    crate::backend()
        .resolve(shape, name)
        .ok_or(ResolveElementError::Missing)
}

/// The element kind a stored name starts with (`Edge1` → `Edge`), if it names one.
pub(crate) fn kind_of_name(name: &str) -> Option<ElementKind> {
    ElementKind::ALL.into_iter().find(|kind| {
        name.strip_prefix(kind.prefix())
            .is_some_and(|ordinal| ordinal.parse::<usize>().is_ok_and(|n| n >= 1))
    })
}

/// The name `shape`'s own element enumeration gives `sub`, when `sub` is one of its
/// elements.
///
/// A sub-shape resolved through the lineage is relabelled with this before a tangent
/// chain is walked, because [`ferrocad_geom::tangent_chain`] matches seeds against
/// the shape's positional names (`Edge5`), not against stored ones (`Edge1`). The
/// label is transient: the stored name is never rewritten. Comparison is topological
/// ([`GeometryBackend::same_shape`](ferrocad_geom::GeometryBackend::same_shape)), since
/// a fresh enumeration hands out fresh handles.
pub(crate) fn positional_name(shape: &Shape, name: &str, sub: &Shape) -> Option<String> {
    let kind = kind_of_name(name)?;
    let backend = crate::backend();
    backend
        .elements(shape, kind)
        .into_iter()
        .find(|element| backend.same_shape(&element.shape, sub))
        .map(|element| element.name)
}

impl ExtensionData for ShapeProperty {
    fn type_name(&self) -> &str {
        TYPE_NAME
    }

    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(ShapeProperty {
            shape: self.shape.clone(),
            lineage: self.lineage.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq(&self, other: &dyn ExtensionData) -> bool {
        let Some(other) = other.as_any().downcast_ref::<ShapeProperty>() else {
            return false;
        };
        // Shape identity only: the lineage is a resolution aid, not part of what the
        // shape is, so a rebuild that produces the same handle is still "equal".
        match (&self.shape, &other.shape) {
            (None, None) => true,
            (Some(x), Some(y)) => x.is_same(y),
            _ => false,
        }
    }

    fn save(&self) -> Vec<u8> {
        let Some(shape) = self.shape.as_ref() else {
            return Vec::new();
        };
        let backend = crate::backend();
        // A backend that cannot encode a shape (the null backend) keeps the old
        // behaviour: no bytes, no value. The envelope is only for real payloads.
        let brep = backend.save_shape(shape);
        if brep.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(MAGIC);
        put_u16(&mut out, ENVELOPE_VERSION);
        put_bytes(&mut out, &brep);

        let version = self.lineage.version();
        put_u16(&mut out, version.encoding());
        put_str(&mut out, version.policy());
        match version.kernel() {
            Some(kernel) => {
                out.push(1);
                put_str(&mut out, kernel);
            }
            None => out.push(0),
        }

        put_u64(&mut out, self.lineage.map().len() as u64);
        for (name, sub_shape) in self.lineage.map().iter() {
            put_str(&mut out, name);
            put_bytes(&mut out, &backend.save_shape(sub_shape));
        }

        put_u64(&mut out, self.lineage.reverse().len() as u64);
        for (output, inputs) in self.lineage.reverse().iter() {
            put_str(&mut out, output);
            put_u64(&mut out, inputs.len() as u64);
            for input in inputs {
                put_str(&mut out, input);
            }
        }
        out
    }

    /// Upstream `Part::PropertyPartShape` writes one `PartShapeN.brp` member and
    /// references it from a `<Part file="…"/>` element. That member stays raw BREP
    /// (the envelope is a native-document form).
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        Some(vec![(String::new(), self.brep())])
    }

    /// Persist the lineage in `.FCStd` beside the raw BREP member.
    ///
    /// The sidecar is the same versioned envelope [`save`](ExtensionData::save)
    /// writes, so restoring it needs no second format: the reader prefers the
    /// sidecar bytes and falls back to the raw `<Part>` member when it is absent.
    /// Nothing is written for a shape without a lineage, so primitive shapes keep
    /// the exact upstream archive layout.
    fn fcstd_sidecar(&self) -> Option<(String, Vec<u8>)> {
        if self.lineage.map().is_empty() && self.lineage.reverse().is_empty() {
            return None;
        }
        let bytes = self.save();
        if bytes.is_empty() {
            None
        } else {
            Some(("lineage".to_string(), bytes))
        }
    }

    /// Resolve a stored name for the geometry-free Python layer.
    ///
    /// Only a name that looks like a sub-element (`Face1`, `Edge3`, `Vertex2`) is
    /// answered; any other spelling is "no opinion" (`Ok(None)`) so the caller
    /// keeps its existing behaviour for datum names and object paths.
    fn resolve_element_name(&self, name: &str) -> Result<Option<String>, &'static str> {
        if kind_of_name(name).is_none() {
            return Ok(None);
        }
        match self.resolve_element(name) {
            Ok(_) => Ok(Some(name.to_string())),
            Err(ResolveElementError::Retired) => Err("retired"),
            Err(ResolveElementError::Missing | ResolveElementError::NoShape) => Err("missing"),
        }
    }

    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.shape.as_ref() {
            Some(shape) => write!(
                f,
                "ShapeProperty({shape:?}, {} lineage entries)",
                self.lineage.map().len()
            ),
            None => f.write_str("ShapeProperty(empty)"),
        }
    }
}

/// Wrap a [`ShapeProperty`] as a `Property::Extension` value.
pub fn make_shape_property(property: ShapeProperty) -> Property {
    Property::Extension(ExtensionValue::new(Box::new(property)))
}

/// Borrow the [`ShapeProperty`] inside `property`, if it is one.
pub fn shape_of(property: &Property) -> Option<&ShapeProperty> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<ShapeProperty>(),
        _ => None,
    }
}

/// Register `Part::PropertyPartShape`. Idempotent.
pub fn register() {
    property_types::register_extension(
        TYPE_NAME,
        || make_shape_property(ShapeProperty::empty()),
        restore,
    );
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

/// A length-prefixed (`u64`) byte string.
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// A length-prefixed (`u32`) UTF-8 string.
fn put_str(out: &mut Vec<u8>, value: &str) {
    put_u32(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
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

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let bytes: [u8; 2] = self.take(2)?.try_into().ok()?;
        Some(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Option<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Option<u64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(u64::from_le_bytes(bytes))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.u64()?).ok()?;
        self.take(len)
    }

    fn string(&mut self) -> Option<String> {
        let len = usize::try_from(self.u32()?).ok()?;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }

    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
}

fn restore(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    if bytes.is_empty() {
        return Some(Box::new(ShapeProperty::empty()));
    }
    if bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC {
        return restore_envelope(bytes).map(|property| Box::new(property) as Box<dyn ExtensionData>);
    }
    // Legacy: a raw BREP blob written before the envelope existed, or an `.FCStd`
    // member. Never reject it.
    let shape = crate::backend().load_shape(bytes)?;
    Some(Box::new(ShapeProperty::new(shape)))
}

fn restore_envelope(bytes: &[u8]) -> Option<ShapeProperty> {
    let mut reader = Reader::new(&bytes[MAGIC.len()..]);
    let version = reader.u16()?;
    if version != ENVELOPE_VERSION {
        return None;
    }
    let backend = crate::backend();
    let shape = backend.load_shape(reader.bytes()?)?;

    let encoding = reader.u16()?;
    let policy = reader.string()?;
    let kernel = match reader.u8()? {
        0 => None,
        1 => Some(reader.string()?),
        _ => return None,
    };

    let forward_count = reader.u64()?;
    let mut entries = Vec::new();
    for _ in 0..forward_count {
        let name = reader.string()?;
        let sub_shape = backend.load_shape(reader.bytes()?)?;
        entries.push((name, sub_shape));
    }

    let reverse_count = reader.u64()?;
    let mut reverse = Vec::new();
    for _ in 0..reverse_count {
        let output = reader.string()?;
        let input_count = reader.u64()?;
        let mut inputs = Vec::new();
        for _ in 0..input_count {
            inputs.push(reader.string()?);
        }
        reverse.push((output, inputs));
    }

    if !reader.is_empty() {
        return None;
    }

    Some(ShapeProperty {
        shape: Some(shape),
        lineage: rebuild_lineage(
            MapVersion::new(encoding, policy, kernel),
            &entries,
            &reverse,
        ),
    })
}

/// Rebuild a [`Lineage`] from its two persisted maps.
///
/// [`Lineage`] only exposes construction from a [`History`], and a forward map entry
/// alone does not remember the *output* name it came from. Binding each entry under a
/// private placeholder output name, retiring the placeholders, and then replaying the
/// stored reverse relations reconstructs both maps exactly: the placeholders never
/// escape, the forward map keeps its stored shapes, and replaying cannot rebind a
/// name that is already present ([`ElementMap::apply`](ferrocad_geom::ElementMap) is
/// insert-if-absent).
fn rebuild_lineage(
    version: MapVersion,
    entries: &[(String, Shape)],
    reverse: &[(String, Vec<String>)],
) -> Lineage {
    let mut lineage = Lineage::new(version);
    if entries.is_empty() {
        return lineage;
    }

    let mut bind = History::default();
    let mut retire = History::default();
    for (index, (name, sub_shape)) in entries.iter().enumerate() {
        // A name no policy can produce, so it cannot collide with a real element.
        let placeholder = format!("\u{1}ferrocad-lineage-{index}");
        bind.modified.push((
            ElementRef {
                shape: sub_shape.clone(),
                name: name.clone(),
            },
            ElementRef {
                shape: sub_shape.clone(),
                name: placeholder.clone(),
            },
        ));
        retire.deleted.push(ElementRef {
            shape: sub_shape.clone(),
            name: placeholder,
        });
    }
    lineage.apply(&bind);
    lineage.apply(&retire);

    // Everything a reverse relation names is already a forward key (a name is a key
    // exactly when some relation produced it), so replaying the reverse map cannot
    // introduce a new forward binding. The shape is an anchor, never stored as a map
    // value.
    let bound: BTreeSet<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    let anchor = entries[0].1.clone();
    let mut replay = History::default();
    for (output, inputs) in reverse {
        for input in inputs {
            if !bound.contains(input.as_str()) {
                continue;
            }
            replay.modified.push((
                ElementRef {
                    shape: anchor.clone(),
                    name: input.clone(),
                },
                ElementRef {
                    shape: anchor.clone(),
                    name: output.clone(),
                },
            ));
        }
    }
    lineage.apply(&replay);
    lineage
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_core::{Document, ObjectId, Property};
    use ferrocad_occt::OcctBackend;
    use std::sync::Arc;

    fn setup() {
        crate::register(Arc::new(OcctBackend::new()));
    }

    /// Two overlapping boxes and the cut that consumes them, recomputed so the
    /// cut's `Shape` carries a real lineage.
    fn cut_document() -> (Document, ObjectId) {
        let mut doc = Document::new();
        for name in ["BoxA", "BoxB"] {
            let id = doc.add_object(name, crate::BOX_TYPE);
            // The primitive path's golden case: a 1 mm box cut out of a 2 mm box,
            // both at the origin. It produces both forward and reverse relations.
            let size = if name == "BoxA" { 2.0 } else { 1.0 };
            doc.set_property(id, "Length", Property::Float(size)).unwrap();
            doc.set_property(id, "Width", Property::Float(size)).unwrap();
            doc.set_property(id, "Height", Property::Float(size)).unwrap();
        }
        doc.recompute().unwrap();
        let cut = doc.add_object("Cut", crate::CUT_TYPE);
        doc.set_property(cut, "Base", Property::Link("BoxA".into()))
            .unwrap();
        doc.set_property(cut, "Tool", Property::Link("BoxB".into()))
            .unwrap();
        doc.recompute().unwrap();
        (doc, cut)
    }

    fn stored_shape_property(doc: &Document, id: ObjectId) -> &ShapeProperty {
        shape_of(doc.object(id).unwrap().properties.get("Shape").unwrap())
            .expect("the object owns a Part::PropertyPartShape")
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ferrocad-part-shape-property-{}-{name}",
            std::process::id()
        ))
    }

    fn box_and_edges(size: f64) -> (Shape, Vec<ElementRef>) {
        let shape = crate::make_box(size, size, size).unwrap();
        let edges = crate::backend().elements(&shape, ElementKind::Edge);
        (shape, edges)
    }

    /// A lineage that binds `name` to `target`, as an operation that preserved the
    /// name across a rebuild would report it.
    fn lineage_binding(name: &str, target: &ElementRef) -> Lineage {
        let mut history = History::default();
        history.modified.push((
            ElementRef {
                shape: target.shape.clone(),
                name: name.to_string(),
            },
            target.clone(),
        ));
        Lineage::from_history(&history, MapVersion::default())
    }

    #[test]
    fn a_mapped_name_resolves_to_the_mapped_edge() {
        setup();
        let (shape, edges) = box_and_edges(10.0);
        // `Edge1` was preserved across some operation and now means edge 5.
        let mapped = edges[4].clone();
        let property = ShapeProperty::with_lineage(shape, lineage_binding("Edge1", &mapped));

        let resolved = property.resolve_element("Edge1").expect("Edge1 resolves");
        assert!(
            crate::backend().same_shape(&resolved, &mapped.shape),
            "the mapped edge is chosen"
        );
        assert!(
            !crate::backend().same_shape(&resolved, &edges[0].shape),
            "not the edge its ordinal names"
        );
    }

    #[test]
    fn without_a_lineage_a_name_resolves_positionally() {
        setup();
        let (shape, edges) = box_and_edges(10.0);
        let property = ShapeProperty::new(shape);
        assert!(property.lineage().map().is_empty());

        let resolved = property.resolve_element("Edge1").expect("Edge1 resolves");
        assert!(
            crate::backend().same_shape(&resolved, &edges[0].shape),
            "the first edge is chosen"
        );
        assert_eq!(
            property.resolve_element("Edge99").err(),
            Some(ResolveElementError::Missing),
            "a name no edge has does not resolve"
        );
    }

    #[test]
    fn a_lineage_without_the_name_does_not_retarget() {
        setup();
        let (shape, edges) = box_and_edges(10.0);
        // A non-empty map that never mentions `Edge1`: the lineage exists, so a
        // positional fallback would silently pick up whatever edge now has that
        // ordinal.
        let property = ShapeProperty::with_lineage(shape, lineage_binding("Edge2", &edges[4]));
        assert!(!property.lineage().map().is_empty());

        assert_eq!(
            property.resolve_element("Edge1").err(),
            Some(ResolveElementError::Retired),
            "a name the map does not justify fails instead of moving"
        );
        assert_eq!(
            property.resolve_element("Edge99").err(),
            Some(ResolveElementError::Retired),
            "an unknown name fails the same way"
        );
        let mapped = property.resolve_element("Edge2").expect("Edge2 resolves");
        assert!(
            crate::backend().same_shape(&mapped, &edges[4].shape),
            "the mapped name still works"
        );
    }

    #[test]
    fn an_empty_property_resolves_nothing() {
        setup();
        assert_eq!(
            ShapeProperty::empty().resolve_element("Edge1").err(),
            Some(ResolveElementError::NoShape)
        );
    }

    #[test]
    fn the_extension_hook_answers_only_element_names() {
        setup();
        let (shape, edges) = box_and_edges(10.0);
        // No lineage: a real element resolves positionally, an out-of-range one
        // is missing, and a name that is not an element spelling has no opinion.
        let primitive = ShapeProperty::new(shape);
        assert_eq!(
            primitive.resolve_element_name("Edge1"),
            Ok(Some("Edge1".to_string()))
        );
        assert!(primitive.resolve_element_name("Edge99").is_err());
        assert_eq!(primitive.resolve_element_name("N_Axis"), Ok(None));

        // With a lineage, only a mapped name resolves; everything else is retired.
        let property = ShapeProperty::with_lineage(
            primitive.shape().unwrap(),
            lineage_binding("Edge2", &edges[4]),
        );
        assert!(property.resolve_element_name("Edge2").is_ok());
        assert!(property.resolve_element_name("Edge1").is_err());
    }

    /// S6b: a `.FCStd` save writes both the upstream-compatible raw BREP member and
    /// a lineage sidecar, and loading restores exactly the stored reverse map.
    #[test]
    fn fcstd_round_trip_preserves_the_lineage() {
        setup();
        let (doc, cut) = cut_document();
        let original = stored_shape_property(&doc, cut);
        assert!(
            !original.lineage().map().is_empty(),
            "a cut carries forward relations"
        );
        assert!(
            !original.lineage().reverse().is_empty(),
            "a cut carries reverse relations"
        );
        let original_reverse = original.lineage().reverse().clone();
        let original_version = original.lineage().version().clone();
        let original_names: Vec<String> = original
            .lineage()
            .map()
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();

        let saved = doc.to_saved("LineageRoundTrip");
        let path = temp_path("lineage.FCStd");
        ferrocad_core::fcstd_write::write_fcstd(&saved, path.to_str().unwrap())
            .expect("write .FCStd");
        let read = ferrocad_core::fcstd::read_fcstd(path.to_str().unwrap())
            .expect("read back .FCStd");
        let _ = std::fs::remove_file(&path);

        let restored = read
            .objects
            .iter()
            .find(|object| object.name == "Cut")
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .expect("Cut.Shape is restored");

        // The observable contract is the reverse map (ElementReverseMap in
        // Part.Shape); the forward keys and version must match too.
        assert_eq!(restored.lineage().reverse(), &original_reverse);
        assert_eq!(restored.lineage().version(), &original_version);
        let restored_names: Vec<String> = restored
            .lineage()
            .map()
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(restored_names, original_names);

        // Each restored mapping still denotes the same geometry. `same_shape` is
        // kernel identity, which a BREP round trip necessarily replaces, so
        // compare the measured properties instead.
        let properties = |shape: &Shape| {
            crate::backend()
                .mass_properties(shape)
                .expect("a lineage sub-shape has mass properties")
        };
        for (name, sub_shape) in original.lineage().map().iter() {
            let restored_sub = restored
                .lineage()
                .map()
                .get(name)
                .unwrap_or_else(|| panic!("'{name}' survives the round trip"));
            let before = properties(sub_shape);
            let after = properties(restored_sub);
            let same = |a: f64, b: f64| (a - b).abs() < 1e-9;
            assert!(
                same(before.length, after.length)
                    && same(before.area, after.area)
                    && same(before.volume, after.volume)
                    && same(before.center_of_mass.x, after.center_of_mass.x)
                    && same(before.center_of_mass.y, after.center_of_mass.y)
                    && same(before.center_of_mass.z, after.center_of_mass.z),
                "'{name}' maps to the same element"
            );
        }
    }

    /// A file with no sidecar (upstream, or a FerroCAD save from before S6b) still
    /// loads its raw BREP; only the lineage is absent.
    #[test]
    fn an_fcstd_without_a_sidecar_still_loads_the_geometry() {
        setup();
        let (mut doc, cut) = cut_document();
        let shape = stored_shape_property(&doc, cut)
            .shape()
            .expect("the cut produced a shape");
        let volume = crate::backend()
            .mass_properties(&shape)
            .expect("the cut has mass properties")
            .volume;

        // Re-wrap the same geometry without its lineage: this is exactly what the
        // writer emits for a pre-sidecar file.
        doc.set_property_raw(
            cut,
            "Shape",
            make_shape_property(ShapeProperty::new(shape)),
        );
        let saved = doc.to_saved("LegacyRoundTrip");
        let path = temp_path("legacy.FCStd");
        ferrocad_core::fcstd_write::write_fcstd(&saved, path.to_str().unwrap())
            .expect("write .FCStd");
        let read = ferrocad_core::fcstd::read_fcstd(path.to_str().unwrap())
            .expect("read back .FCStd");
        let _ = std::fs::remove_file(&path);

        let restored = read
            .objects
            .iter()
            .find(|object| object.name == "Cut")
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .expect("Cut.Shape is restored");
        assert!(restored.lineage().map().is_empty());
        assert!(restored.lineage().reverse().is_empty());

        let restored_shape = restored.shape().expect("the BREP payload survives");
        let restored_volume = crate::backend()
            .mass_properties(&restored_shape)
            .expect("the restored cut has mass properties")
            .volume;
        assert!((restored_volume - volume).abs() < 1e-6, "geometry is intact");
    }
}

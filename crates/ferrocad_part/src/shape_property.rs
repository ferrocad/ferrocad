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
//! The envelope is a *native-document* encoding. [`fcstd_members`] keeps writing raw
//! BREP for `PartShapeN.brp`, so an upstream `.FCStd` reader still sees a BREP file;
//! persisting the map in that format is a separate step.

use std::any::Any;
use std::collections::BTreeSet;
use std::fmt;

use ferrocad_core::property_types;
use ferrocad_core::{ExtensionData, ExtensionValue, Property};
use ferrocad_geom::{ElementRef, History, Lineage, MapVersion, Shape};

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
/// hold. The map is how a stored name is resolved, not part of what the shape *is*.
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
    /// (the envelope is a native-document form), so the lineage is not written into
    /// `.FCStd` yet.
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        Some(vec![(String::new(), self.brep())])
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

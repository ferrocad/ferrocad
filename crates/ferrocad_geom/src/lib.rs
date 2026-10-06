//! # FerroCAD geometry seam (`ferrocad_geom`)
//!
//! The kernel-independent vocabulary for geometry, so the document, persistence,
//! expression and recompute layers can be written and tested **now**, without a
//! geometry kernel, and gain one later without touching them.
//!
//! It provides three things:
//!
//! - [`Shape`] — an opaque, cheap-to-clone handle to a kernel shape. The kernel owns
//!   the data; Rust owns the handle.
//! - [`History`], [`ElementRef`] and [`ElementMap`] — what an operation did, in the
//!   kernel's own terms, and the stable-name registry derived from it.
//! - [`GeometryBackend`] — the trait a kernel implements (OCCT, or [`NullBackend`]).
//!
//! This crate depends on [`ferrocad_types`] for values such as [`Placement`], and on
//! nothing else. It is the boundary OCCT must sit *behind*: no crate below the
//! application depends on the kernel, and `ferrocad_core` stays a leaf (see
//! `docs/occt-integration.md`).
//!
//! # Example
//!
//! ```
//! use ferrocad_geom::{GeometryBackend, NullBackend};
//!
//! let backend = NullBackend;
//! let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let result = backend.fuse(&a, &b).unwrap();
//! assert!(result.history.is_empty()); // the null backend invents no lineage
//! ```

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use ferrocad_types::Placement;

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// An opaque handle to a kernel shape.
///
/// Cheap to clone: the kernel keeps the geometry, and this only shares a reference to
/// it. Equality is *handle identity* (two handles are equal only if they refer to the
/// very same kernel object), never geometric equality; compare geometry by resolving
/// to `ElementRef`s instead.
///
/// `Shape` is `Send + Sync` — documents reach Python, whose classes must be `Sync`.
/// A backend value that is only `Send` (OCCT's `TopoDS_Shape`) therefore wraps any
/// non-thread-safe inner state in a `Mutex`.
#[derive(Clone)]
pub struct Shape {
    inner: Arc<dyn Any + Send + Sync>,
    kind: &'static str,
}

impl Shape {
    /// Wrap a backend-specific, thread-safe value as a shape handle.
    pub fn new<T: Any + Send + Sync>(data: T) -> Self {
        Shape {
            inner: Arc::new(data),
            kind: std::any::type_name::<T>(),
        }
    }

    /// Borrow the backend-specific value, if this handle wraps a `T`.
    ///
    /// A kernel backend uses this to recover its own shape type (for OCCT, the
    /// `TopoDS_Shape` wrapper) from the erased handle.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }

    /// The Rust type name of the wrapped value, for diagnostics.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Whether two handles refer to the same kernel object.
    pub fn is_same(&self, other: &Shape) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl fmt::Debug for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shape({})", self.kind)
    }
}

impl PartialEq for Shape {
    fn eq(&self, other: &Self) -> bool {
        self.is_same(other)
    }
}

impl Eq for Shape {}

// ---------------------------------------------------------------------------
// History and element references
// ---------------------------------------------------------------------------

/// A reference to one sub-element of a shape (a face, an edge, …), by a stable
/// name rather than a positional index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementRef {
    /// The sub-shape itself.
    pub shape: Shape,
    /// Its stable name (e.g. `Face6`, or an encoded lineage token).
    pub name: String,
}

/// What one operation did to its inputs, in the kernel's own terms.
///
/// This is the raw material for the element map: a mapper folds it into stable names.
/// The kernel reports exactly what it knows; the mapper can be no better than this.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// `(input, output)` pairs the operation created.
    pub generated: Vec<(ElementRef, ElementRef)>,
    /// `(input, output)` pairs the operation changed.
    pub modified: Vec<(ElementRef, ElementRef)>,
    /// Inputs that do not survive into the output.
    pub deleted: Vec<ElementRef>,
}

impl History {
    /// An operation that reports no lineage (e.g. the null backend).
    pub fn is_empty(&self) -> bool {
        self.generated.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// The stable-name registry carried with a shape: `name -> current sub-shape`.
///
/// It is a cache and a persistence format, not the identity itself; names should be
/// re-derivable from [`History`] where possible, which is what lets old documents be
/// migrated after a rebuild.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementMap {
    entries: BTreeMap<String, Shape>,
}

impl ElementMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the current sub-shape for `name`.
    pub fn insert(&mut self, name: impl Into<String>, shape: Shape) {
        self.entries.insert(name.into(), shape);
    }

    /// The current sub-shape for `name`, if the name still resolves.
    pub fn get(&self, name: &str) -> Option<&Shape> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(name, shape)` in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Shape)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

// ---------------------------------------------------------------------------
// The backend trait
// ---------------------------------------------------------------------------

/// The output of a shape operation: the resulting shape and what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    pub shape: Shape,
    pub history: History,
}

impl OpResult {
    pub fn new(shape: Shape, history: History) -> Self {
        OpResult { shape, history }
    }
}

/// A boxed kernel error.
///
/// A backend is a shared service injected at runtime (`Arc<dyn GeometryBackend>`),
/// and a trait object cannot carry an associated `Error` type. Each backend therefore
/// keeps its own concrete error enum (for its own tests and diagnostics) and boxes it
/// here — via `?` or `.into()` at the seam. Callers still read a real `Display` message.
pub type GeomError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A geometry kernel.
///
/// One method per operation the workbenches actually call; the set grows with them.
/// Implementations are stateless with respect to the call (`&self`); each operation
/// builds whatever it needs and returns an owned [`OpResult`].
///
/// The trait is `Send + Sync` and object-safe, because a backend is a shared service:
/// an application installs one and hands it out as an `Arc<dyn GeometryBackend>`, so a
/// workbench (Part) can be composed with a kernel without depending on it.
pub trait GeometryBackend: Send + Sync {
    /// A rectangular box with its corner at the origin.
    fn make_box(&self, length: f64, width: f64, height: f64) -> Result<Shape, GeomError>;

    /// Boolean union.
    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError>;

    /// Boolean difference, `a` minus `b`.
    fn cut(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError>;

    /// Round the given edges of `shape`.
    fn fillet(&self, shape: &Shape, edges: &[ElementRef], radius: f64)
    -> Result<OpResult, GeomError>;

    /// Apply a placement (transform) to `shape`.
    fn place(&self, shape: &Shape, placement: &Placement) -> Result<Shape, GeomError>;

    /// Resolve a stable sub-element name against `shape`, if it still exists.
    fn resolve(&self, shape: &Shape, name: &str) -> Option<Shape>;

    /// Serialise a shape for persistence. Core stores the bytes verbatim and never
    /// interprets them; the kernel owns the encoding (OCCT writes BREP).
    fn save_shape(&self, shape: &Shape) -> Vec<u8>;

    /// Reconstruct a shape from [`save_shape`](GeometryBackend::save_shape) bytes,
    /// or `None` if they are not valid.
    fn load_shape(&self, bytes: &[u8]) -> Option<Shape>;
}

// ---------------------------------------------------------------------------
// The null backend
// ---------------------------------------------------------------------------

/// A placeholder shape produced by [`NullBackend`]: enough to type-check and to
/// assert on in tests, with no kernel behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullShape {
    pub kind: String,
}

/// A geometry kernel that produces placeholder shapes and **empty** [`History`].
///
/// It lets the document model, persistence and their tests run without OCCT: objects
/// still get a [`Shape`] to hold, but nothing is computed and no lineage is invented.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullBackend;

impl GeometryBackend for NullBackend {
    fn make_box(&self, _length: f64, _width: f64, _height: f64) -> Result<Shape, GeomError> {
        Ok(Shape::new(NullShape {
            kind: "box".to_string(),
        }))
    }

    fn fuse(&self, _a: &Shape, _b: &Shape) -> Result<OpResult, GeomError> {
        Ok(OpResult::new(
            Shape::new(NullShape {
                kind: "fuse".to_string(),
            }),
            History::default(),
        ))
    }

    fn cut(&self, _a: &Shape, _b: &Shape) -> Result<OpResult, GeomError> {
        Ok(OpResult::new(
            Shape::new(NullShape {
                kind: "cut".to_string(),
            }),
            History::default(),
        ))
    }

    fn fillet(
        &self,
        _shape: &Shape,
        _edges: &[ElementRef],
        _radius: f64,
    ) -> Result<OpResult, GeomError> {
        Ok(OpResult::new(
            Shape::new(NullShape {
                kind: "fillet".to_string(),
            }),
            History::default(),
        ))
    }

    fn place(&self, shape: &Shape, _placement: &Placement) -> Result<Shape, GeomError> {
        // The null backend has no geometry to move, so the handle is unchanged.
        Ok(shape.clone())
    }

    fn resolve(&self, _shape: &Shape, _name: &str) -> Option<Shape> {
        None
    }

    fn save_shape(&self, _shape: &Shape) -> Vec<u8> {
        Vec::new()
    }

    fn load_shape(&self, _bytes: &[u8]) -> Option<Shape> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_backend_makes_placeholder_shapes() {
        let backend = NullBackend;
        let s = backend.make_box(1.0, 2.0, 3.0).unwrap();
        assert_eq!(
            s.downcast_ref::<NullShape>(),
            Some(&NullShape {
                kind: "box".to_string()
            })
        );
        assert!(s.kind().ends_with("NullShape"));
    }

    #[test]
    fn null_backend_reports_no_history() {
        let backend = NullBackend;
        let a = backend.make_box(1.0, 1.0, 1.0).unwrap();
        let b = backend.make_box(1.0, 1.0, 1.0).unwrap();
        let out = backend.fuse(&a, &b).unwrap();
        assert!(out.history.is_empty());
        assert!(backend.resolve(&out.shape, "Face1").is_none());
    }

    #[test]
    fn shape_equality_is_handle_identity() {
        let a = Shape::new(1u32);
        let b = a.clone();
        let c = Shape::new(1u32);
        assert!(a.is_same(&b));
        assert_eq!(a, b);
        assert!(!a.is_same(&c));
        assert_ne!(a, c);
    }

    #[test]
    fn element_map_roundtrips() {
        let mut map = ElementMap::new();
        assert!(map.is_empty());
        map.insert("Face1", Shape::new("face".to_string()));
        assert_eq!(map.len(), 1);
        assert!(map.get("Face1").is_some());
        assert!(map.get("Face2").is_none());
    }
}

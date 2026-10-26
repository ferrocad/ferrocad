//! # FerroCAD geometry seam (`ferrocad_geom`)
//!
//! The kernel-independent vocabulary for geometry, so the document, persistence,
//! expression and recompute layers can be written and tested **now**, without a
//! geometry kernel, and gain one later without touching them.
//!
//! It provides four things:
//!
//! - [`Shape`] — an opaque, cheap-to-clone handle to a kernel shape. The kernel owns
//!   the data; Rust owns the handle.
//! - [`History`], [`ElementRef`] and [`ElementMap`] — what an operation did, in the
//!   kernel's own terms, and the stable-name registry derived from it. These are
//!   re-exports of the payload-generic vocabulary in [`ferrocad_lineage`],
//!   specialised to [`Shape`].
//! - [`Lineage`], [`ReverseMap`], [`MapVersion`] and [`NamePolicy`] — the naming
//!   layer: provenance accumulated over successive histories, its inverse view, and
//!   the versioned rules that turn elements into names. [`Lineage`] is specialised
//!   to [`Shape`]; the others carry no payload and are re-exported as themselves.
//! - [`ElementId`], [`OperandId`] and [`Provenance`] — the identity an operation
//!   reports for its inputs (operand, kind and 1-based ordinal), independent of the
//!   positional name it also reports.
//! - [`Tessellation`], [`ElementKind`] and [`SelectionRef`] — the display mesh with
//!   per-primitive sub-element ids, and the neutral vocabulary a viewport or a
//!   workbench needs to turn a pick into a topology reference.
//! - [`tangent_chain`] — the smooth edge chain a fillet or chamfer applies to.
//! - [`GeometryBackend`] — the trait a kernel implements (OCCT, or [`NullBackend`]).
//!
//! This crate depends on [`ferrocad_types`] for values such as [`Placement`], on
//! [`ferrocad_curves`] for edge curves, and on [`ferrocad_lineage`] for the
//! provenance vocabulary, and on nothing else. It is the boundary OCCT must sit
//! *behind*: no crate below the
//! application depends on the kernel, and `ferrocad_core` stays a leaf (see
//! `docs/architecture/occt-integration.md`).
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

use ferrocad_types::{Matrix4, Placement, Vector3};

use ferrocad_curves::EdgeCurve;

// ---------------------------------------------------------------------------
// SurfaceData
// ---------------------------------------------------------------------------

/// The elementary description of a face's backing surface (`Face.Surface`).
///
/// `Plane`, `Cylinder` and `Sphere` carry the frame the backend's surface
/// evaluator uses, so a point's parameters computed from the frame round-trip
/// through `face_value_at`. `Other` keeps only the FreeCAD type id for surfaces
/// this slice does not classify (cone, torus, B-spline, …).
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceData {
    /// A plane: `point = origin + u * x_dir + v * y_dir`.
    Plane {
        origin: Vector3,
        x_dir: Vector3,
        y_dir: Vector3,
        normal: Vector3,
    },
    /// A cylinder: `point = origin + radius * (cos(u) * x_dir + sin(u) * y_dir)
    /// + v * axis`, with `y_dir = axis × x_dir`.
    Cylinder {
        origin: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        radius: f64,
    },
    /// A sphere: `point = center + radius * (cos(v) * (cos(u) * x_dir
    /// + sin(u) * y_dir) + sin(v) * axis)`, with `y_dir = axis × x_dir`.
    Sphere {
        center: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        radius: f64,
    },
    /// Any other surface, identified only by its FreeCAD type id.
    Other { type_id: String },
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// A kernel-owned shape payload that can be cloned without a lock.
///
/// Kept private: [`Shape`] is the public handle. It exists because `Shape` must be
/// `Send` but **not** `Sync` (a kernel handle such as OCCT's `TopoDS_Shape` is `Send`
/// but not `Sync`), and `Arc<T>` is only `Send` when `T: Sync`. So `Shape` shares its
/// payload by asking the kernel to copy its own handle, never by reference counting it.
trait ShapeData: Any + Send {
    /// A new boxed copy of the payload (a kernel-handle copy is cheap).
    fn clone_box(&self) -> Box<dyn ShapeData>;
    /// Borrow for downcasting.
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Send + Clone> ShapeData for T {
    fn clone_box(&self) -> Box<dyn ShapeData> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// An opaque handle to a kernel shape.
///
/// Cheap to clone: cloning copies the kernel's own (reference-counted) handle through
/// [`ShapeData::clone_box`]. Equality is *handle identity* (two handles are equal only
/// if one was cloned from the other, or both from the same source), never geometric
/// equality; compare geometry by resolving to `ElementRef`s instead.
///
/// `Shape` is `Send` but not `Sync`. Documents are shared behind `Arc<Mutex<…>>`, which
/// only needs their contents to be `Send`; exclusive access comes from the document's
/// lock, so the seam neither needs a per-handle lock nor forces the kernel's handle to
/// be `Sync`.
pub struct Shape {
    inner: Box<dyn ShapeData>,
    /// Shared only to give clones a common identity for [`is_same`](Shape::is_same).
    /// `()` is `Sync`, so this token does not make `Shape` `Sync`.
    identity: Arc<()>,
    kind: &'static str,
}

impl Shape {
    /// Wrap a backend-specific, cloneable value as a shape handle.
    pub fn new<T: Any + Send + Clone>(data: T) -> Self {
        Shape {
            inner: Box::new(data),
            identity: Arc::new(()),
            kind: std::any::type_name::<T>(),
        }
    }

    /// Borrow the backend-specific value, if this handle wraps a `T`.
    ///
    /// A kernel backend uses this to recover its own shape type (for OCCT, the
    /// `TopoDS_Shape` wrapper) from the erased handle.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.inner.as_any().downcast_ref::<T>()
    }

    /// The Rust type name of the wrapped value, for diagnostics.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Whether two handles share one identity (one derives from the other, or both
    /// derive from a common source).
    pub fn is_same(&self, other: &Shape) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Clone for Shape {
    fn clone(&self) -> Self {
        Shape {
            inner: self.inner.clone_box(),
            identity: Arc::clone(&self.identity),
            kind: self.kind,
        }
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
///
/// This is [`ferrocad_lineage::ElementRef`] with [`Shape`] as its payload; the
/// vocabulary itself lives in `ferrocad_lineage`, which never names a geometry
/// type. All fields and methods are those of the generic type.
pub type ElementRef = ferrocad_lineage::ElementRef<Shape>;

/// What one operation did to its inputs, in the kernel's own terms.
///
/// This is the raw material for the element map: a mapper folds it into stable names.
/// The kernel reports exactly what it knows; the mapper can be no better than this.
///
/// This is [`ferrocad_lineage::History`] with [`Shape`] as its payload. Its
/// `provenance` field carries the [`ElementId`] of each input relation; the
/// relation lists themselves keep positional names.
pub type History = ferrocad_lineage::History<Shape>;

/// The stable-name registry carried with a shape: `name -> current sub-shape`.
///
/// It is a cache and a persistence format, not the identity itself; names should be
/// re-derivable from [`History`] where possible, which is what lets old documents be
/// migrated after a rebuild.
///
/// This is [`ferrocad_lineage::ElementMap`] with [`Shape`] as its payload.
pub type ElementMap = ferrocad_lineage::ElementMap<Shape>;

/// A shape's accumulated provenance: the forward [`ElementMap`], its reverse view,
/// and the version of the naming rules that produced them.
///
/// This is [`ferrocad_lineage::Lineage`] with [`Shape`] as its payload.
pub type Lineage = ferrocad_lineage::Lineage<Shape>;

// These lineage types are not generic over the payload, so they are re-exported
// as themselves rather than aliased: [`ReverseMap`] is a `String`-keyed relation,
// [`MapVersion`]/[`NamePolicy`] describe rules, not shapes, and
// [`ElementId`]/[`OperandId`]/[`Provenance`]/[`Relation`] are identity metadata.
pub use ferrocad_lineage::{
    ElementId, MapVersion, NamePolicy, OperandId, Provenance, Relation, ReverseMap,
};

// ---------------------------------------------------------------------------
// Topology and selection vocabulary
// ---------------------------------------------------------------------------

/// The kind of a topological element, in FreeCAD's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ElementKind {
    Solid,
    Shell,
    Face,
    Wire,
    Edge,
    Vertex,
}

impl ElementKind {
    /// The FreeCAD name prefix (`Face` in `Face6`).
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Shell => "Shell",
            Self::Face => "Face",
            Self::Wire => "Wire",
            Self::Edge => "Edge",
            Self::Vertex => "Vertex",
        }
    }

    /// The element kinds a pick can resolve to, coarsest first.
    pub const ALL: [Self; 6] = [
        Self::Solid,
        Self::Shell,
        Self::Face,
        Self::Wire,
        Self::Edge,
        Self::Vertex,
    ];
}

/// How smoothly two elements meet, as far as tangent propagation is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continuity {
    /// Positional only (a sharp corner).
    C0,
    /// Tangent direction continuous; magnitude may jump (the usual fillet target).
    G1,
    /// Tangent direction and magnitude continuous.
    C1,
    /// Curvature continuous.
    G2,
    /// Curvature and its derivative continuous.
    C2,
}

/// How finely a kernel should tessellate a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TessellationOptions {
    /// Linear deflection in model units. `None` lets the backend derive it from the
    /// shape's bounding box, because a fixed number is wrong at some model scale.
    pub linear_deflection: Option<f64>,
    /// Angular deflection in radians (OCCT's default is `0.5`).
    pub angular_deflection: f64,
    /// Whether the linear deflection is relative to the edge length (OCCT's
    /// `isRelative`).
    pub relative: bool,
}

impl Default for TessellationOptions {
    fn default() -> Self {
        Self {
            linear_deflection: None,
            angular_deflection: 0.5,
            relative: false,
        }
    }
}

impl TessellationOptions {
    /// Options with an explicit absolute linear deflection.
    pub fn absolute(linear_deflection: f64) -> Self {
        Self {
            linear_deflection: Some(linear_deflection),
            ..Self::default()
        }
    }
}

/// A display mesh with per-primitive sub-element ids.
///
/// This is the neutral bridge between a kernel and a renderer. It carries no kernel
/// types: `parts` holds the **names** a selection is expressed in (`Face6`, `Edge3`),
/// and a backend resolves a name to a sub-shape on demand.
///
/// The index bases are deliberately different from the names: `parts` ids are
/// **0-based**, while the names they denote are the kernel's own, usually 1-based
/// (`Face6` is `parts[5]`). Convert once, where the mesh is built.
///
/// `positions` are duplicated per face. A geometric vertex shared by two faces cannot
/// carry two part ids, and per-face triangulation keeps picking exact.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tessellation {
    /// Model-space positions.
    pub positions: Vec<[f32; 3]>,
    /// Model-space per-vertex normals; empty ⇒ flat-shaded per triangle.
    pub normals: Vec<[f32; 3]>,
    /// Flat triangle indices, three per triangle.
    pub triangles: Vec<u32>,
    /// The part id of each triangle (`triangles.len() / 3` entries).
    pub triangle_parts: Vec<u32>,
    /// Flat line indices, two per segment; one run per topological edge.
    pub edges: Vec<u32>,
    /// The part id of each segment (`edges.len() / 2` entries).
    pub segment_parts: Vec<u32>,
    /// One position index per topological vertex.
    pub vertices: Vec<u32>,
    /// The part id of each vertex (`vertices.len()` entries).
    pub vertex_parts: Vec<u32>,
    /// Part id → element name (`Face6`, `Edge3`, …).
    pub parts: Vec<String>,
}

impl Tessellation {
    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// The number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.triangles.len() / 3
    }

    /// The number of line segments.
    pub fn segment_count(&self) -> usize {
        self.edges.len() / 2
    }

    /// The part id of triangle `index`, if the mesh is well-formed.
    pub fn part_id_of_triangle(&self, index: usize) -> Option<u32> {
        self.triangle_parts.get(index).copied()
    }

    /// The part id of segment `index`, if the mesh is well-formed.
    pub fn part_id_of_segment(&self, index: usize) -> Option<u32> {
        self.segment_parts.get(index).copied()
    }

    /// The part id of vertex `index`, if the mesh is well-formed.
    pub fn part_id_of_vertex(&self, index: usize) -> Option<u32> {
        self.vertex_parts.get(index).copied()
    }

    /// The element name for part `id`.
    pub fn part_name(&self, id: u32) -> Option<&str> {
        self.parts.get(id as usize).map(String::as_str)
    }

    /// The part id carrying `name`, if present.
    pub fn part_id(&self, name: &str) -> Option<u32> {
        self.parts
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| index as u32)
    }
}

/// The operations a backend supports.
///
/// Presentation code consults this before offering a manipulator: a mesh-only backend
/// can return [`Capabilities::empty`], and the fillet or draft tools simply never
/// materialise, rather than every backend stubbing `fillet` with an error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(u32);

impl Capabilities {
    /// Boolean union/difference.
    pub const BOOLEANS: Self = Self(1 << 0);
    /// Edge filleting.
    pub const FILLET: Self = Self(1 << 1);
    /// Edge chamfering, including the asymmetric form.
    pub const CHAMFER: Self = Self(1 << 2);
    /// Display tessellation.
    pub const TESSELLATION: Self = Self(1 << 3);
    /// Sub-element enumeration and adjacency.
    pub const TOPOLOGY: Self = Self(1 << 4);

    /// No operations (a null or display-only backend).
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Every operation defined above.
    pub const fn all() -> Self {
        Self::BOOLEANS
            .union(Self::FILLET)
            .union(Self::CHAMFER)
            .union(Self::TESSELLATION)
            .union(Self::TOPOLOGY)
    }

    /// Combine two capability sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether **every** capability in `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Capabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

/// One object plus the sub-elements selected on it.
///
/// `object` is the picking identity the viewport reports (a stage item's entity id);
/// the host maps it to a document object. `elements` are kernel names (`Face6`); an
/// empty list means the whole shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionRef {
    pub object: u32,
    pub elements: Vec<String>,
}

impl SelectionRef {
    /// The whole object, with no sub-element.
    pub fn whole(object: u32) -> Self {
        Self {
            object,
            elements: Vec::new(),
        }
    }

    /// One named sub-element of `object`.
    pub fn element(object: u32, name: impl Into<String>) -> Self {
        Self {
            object,
            elements: vec![name.into()],
        }
    }

    /// Whether the whole object is selected.
    pub fn is_whole(&self) -> bool {
        self.elements.is_empty()
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

/// An operation this backend does not implement.
#[derive(Debug)]
pub struct UnsupportedOperation(pub &'static str);

impl fmt::Display for UnsupportedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the geometry backend does not support {}.", self.0)
    }
}

impl std::error::Error for UnsupportedOperation {}

/// The error a backend returns for an operation it does not implement.
pub fn unsupported(operation: &'static str) -> GeomError {
    Box::new(UnsupportedOperation(operation))
}

/// A geometry kernel.
///
/// One method per operation the workbenches actually call; the set grows with them.
/// Implementations are stateless with respect to the call (`&self`); each operation
/// builds whatever it needs and returns an owned [`OpResult`].
///
/// The trait is `Send + Sync` and object-safe, because a backend is a shared service:
/// an application installs one and hands it out as an `Arc<dyn GeometryBackend>`, so a
/// workbench (Part) can be composed with a kernel without depending on it.
/// The integral properties of a shape, in the kernel's length units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProperties {
    pub length: f64,
    pub area: f64,
    pub volume: f64,
    pub center_of_mass: Vector3,
}

/// The principal axes of a shape (`Shape.PrincipalProperties`).
///
/// The kernel diagonalises the inertia matrix; `moments` are the eigenvalues in
/// the kernel's order, `axes` the matching unit eigenvectors, and
/// `radius_of_gyration` the moments divided by the total mass/area/length. The
/// two flags say whether the shape has a symmetry axis (two equal moments) or a
/// symmetry point (all three equal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrincipalProperties {
    pub symmetry_axis: bool,
    pub symmetry_point: bool,
    pub moments: (f64, f64, f64),
    pub axes: (Vector3, Vector3, Vector3),
    pub radius_of_gyration: (f64, f64, f64),
}

/// The result of a closest-point query between two shapes
/// (`BRepExtrema_DistShapeShape`, upstream `Shape.distToShape`).
///
/// `first` is the closest point on the first shape and `second` the closest
/// point on the second, so the pair spans exactly `distance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeDistance {
    pub distance: f64,
    pub first: Vector3,
    pub second: Vector3,
}

pub trait GeometryBackend: Send + Sync {
    /// A rectangular box with its corner at the origin.
    fn make_box(&self, length: f64, width: f64, height: f64) -> Result<Shape, GeomError>;

    /// A cylinder of `radius` and `height`, growing from `base` along `direction`.
    fn make_cylinder(
        &self,
        _radius: f64,
        _height: f64,
        _base: Vector3,
        _direction: Vector3,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("make_cylinder"))
    }

    /// A sphere of `radius` centred at `center`.
    fn make_sphere(&self, _radius: f64, _center: Vector3) -> Result<Shape, GeomError> {
        Err(unsupported("make_sphere"))
    }

    /// A (truncated) cone: `radius1` at `base`, `radius2` at `base + height * direction`.
    fn make_cone(
        &self,
        _radius1: f64,
        _radius2: f64,
        _height: f64,
        _base: Vector3,
        _direction: Vector3,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("make_cone"))
    }

    /// Build an edge from an analytic curve (the analytic half's kernel entry).
    ///
    /// This is the one method that crosses from [`ferrocad_curves`] into the
    /// kernel; the curve types themselves never name a [`Shape`]. An infinite
    /// [`Line`](ferrocad_curves::Line) has no bounded edge, and a backend may not
    /// implement every curve kind (OCCT 7.8's bindings here build lines, segments,
    /// circles and arcs, not ellipses).
    fn make_edge(&self, _curve: &EdgeCurve) -> Result<Shape, GeomError> {
        Err(unsupported("make_edge"))
    }

    /// Join edges into one wire (`Part.Wire`).
    ///
    /// The edges must be bounded and connect in order; a backend that cannot join
    /// them reports the failure. The default reports the operation as unsupported.
    fn make_wire(&self, _edges: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_wire"))
    }

    /// A vertex at `point` (`Part.Vertex`).
    fn make_vertex(&self, _point: Vector3) -> Result<Shape, GeomError> {
        Err(unsupported("make_vertex"))
    }

    /// The point of a vertex shape (`Vertex.Point`), when it is one.
    fn vertex_point(&self, _shape: &Shape) -> Option<Vector3> {
        None
    }

    /// An interpolating B-spline edge through `points` (`Part.BSplineCurve`).
    fn make_bspline_edge(&self, _points: &[Vector3], _periodic: bool) -> Result<Shape, GeomError> {
        Err(unsupported("make_bspline_edge"))
    }

    /// A B-spline edge whose control points are `poles` (`Part.BSplineCurve(poles, …)`).
    fn make_bspline_poles(
        &self,
        _poles: &[Vector3],
        _degree: i32,
        _periodic: bool,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("make_bspline_poles"))
    }

    /// A face from one or more wires (`Part.makeFace`): the first is the outer
    /// boundary, the rest are holes. A backend that cannot build planar faces
    /// reports the operation as unsupported.
    fn make_face(&self, _wires: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_face"))
    }

    /// One face per boundary wire (`Part::FaceMakerUnified` with independent
    /// wires). Wires that bound the same face belong to [`make_face`]; this is the
    /// multi-region form, which returns a compound when there is more than one.
    fn make_faces(&self, _wires: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_faces"))
    }

    /// The backing surface's FreeCAD type id (`Face.Surface.TypeId`).
    fn surface_type(&self, _shape: &Shape) -> Option<String> {
        None
    }

    /// The backing surface's elementary frame (`Face.Surface` for a plane,
    /// cylinder or sphere), in the **actual** parameterisation the backend's
    /// `face_value_at` uses (`BRepAdaptor_Surface` for OCCT).
    ///
    /// The default reports no data; a backend that can classify a face's
    /// surface overrides this. Other surface kinds (a cone, torus or B-spline)
    /// are reported as [`SurfaceData::Other`] so the type id is still available.
    fn surface_data(&self, _shape: &Shape) -> Option<SurfaceData> {
        None
    }

    /// The outer boundary wire of a face (`Face.OuterWire`).
    fn outer_wire(&self, _shape: &Shape) -> Option<Shape> {
        None
    }

    /// The analytic curve behind an edge (`edge.Curve` upstream).
    ///
    /// A bounded edge reports its **basis** curve: an edge built from a segment
    /// comes back as a [`Line`](ferrocad_curves::Line), as upstream
    /// `Part.LineSegment(...).toShape().Curve` does. The default reports the
    /// operation as unsupported.
    fn curve_of(&self, _shape: &Shape) -> Result<EdgeCurve, GeomError> {
        Err(unsupported("curve_of"))
    }

    /// Boolean union.
    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError>;

    /// Boolean difference, `a` minus `b`.
    fn cut(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError>;

    /// Boolean intersection.
    fn common(&self, _a: &Shape, _b: &Shape) -> Result<OpResult, GeomError> {
        Err(unsupported("common"))
    }

    /// A compound of `shapes` (`Part::Compound`).
    fn make_compound(&self, _shapes: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_compound"))
    }

    /// A comp-solid of `solids` (`Part.CompSolid`).
    fn make_comp_solid(&self, _solids: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_comp_solid"))
    }

    /// A solid from one closed shell (`Part.makeSolid`).
    fn make_solid(&self, _shell: &Shape) -> Result<Shape, GeomError> {
        Err(unsupported("make_solid"))
    }

    /// A shell from a list of faces (`Part.makeShell`).
    ///
    /// The faces are grouped into a shell without sewing, so a backend that does
    /// not support shells reports the operation as unsupported.
    fn make_shell(&self, _faces: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_shell"))
    }

    /// A shell from boundary wires, one planar face per wire
    /// (`Part.makeShellFromWires`).
    fn make_shell_from_wires(&self, _wires: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_shell_from_wires"))
    }

    /// A loft through the section wires (`Part.makeLoft`).
    ///
    /// `solid` closes the first and last section into a solid; the default false
    /// produces the open shell upstream's `Part.makeLoft` returns.
    fn make_loft(&self, _wires: &[Shape], _solid: bool) -> Result<Shape, GeomError> {
        Err(unsupported("make_loft"))
    }

    /// A ruled surface spanning two edges (`Part.makeRuledSurface`).
    fn make_ruled_surface(&self, _first: &Shape, _second: &Shape) -> Result<Shape, GeomError> {
        Err(unsupported("make_ruled_surface"))
    }

    /// Sweep `profile` along `spine` (`Part.makeSweepSurface`).
    ///
    /// The profile is carried along the spine with the spine's own frame; the
    /// result is the lateral surface, not a capped solid. `tolerance` is
    /// upstream's approximation tolerance. A backend without pipe sweeping
    /// reports the operation as unsupported.
    fn make_pipe(
        &self,
        _spine: &Shape,
        _profile: &Shape,
        _tolerance: f64,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("make_pipe"))
    }

    /// A face filled through the boundary edges (`Part.makeFilledFace`).
    fn make_filled_face(&self, _edges: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("make_filled_face"))
    }

    /// The element map a shape carries on its own, with no operation history.
    ///
    /// A result a factory builds directly (a shell, a solid, a loft, a filled
    /// face) is named by that factory, the way upstream's `makeElementShell`,
    /// `makeElementSolid`, `makeElementLoft` and `makeElementFilledFace` name
    /// every result element positionally. The default has no names; a backend
    /// that keeps positional identities overrides this.
    ///
    /// This is deliberately *not* attached to every shape: a primitive and a
    /// single-body operation result carry the names of the operation that made
    /// them, and naming all of their elements here would activate upstream's
    /// guarded sizes for shapes whose naming policy differs.
    fn intrinsic_lineage(&self, _shape: &Shape) -> Result<Lineage, GeomError> {
        Ok(Lineage::default())
    }

    /// Sweep `shape` along `vector` (`Shape.extrude`).
    fn extrude(&self, _shape: &Shape, _vector: Vector3) -> Result<Shape, GeomError> {
        Err(unsupported("extrude"))
    }

    /// Revolve `shape` about the axis through `center` along `axis` by `angle`
    /// **radians** (`Shape.revolve`).
    fn revolve(
        &self,
        _shape: &Shape,
        _center: Vector3,
        _axis: Vector3,
        _angle: f64,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("revolve"))
    }

    /// The axis-aligned bounds `(min, max)` of `shape` (`Shape.BoundBox`).
    ///
    /// A backend that cannot bound the shape (or a null shape) reports `None`.
    fn bounds(&self, _shape: &Shape) -> Option<(Vector3, Vector3)> {
        None
    }

    /// Length, area, volume and centre of mass (`Shape.Length`/`Area`/`Volume`/
    /// `CenterOfMass`). A backend that cannot measure the shape reports `None`.
    fn mass_properties(&self, _shape: &Shape) -> Option<MassProperties> {
        None
    }

    /// The first moments of volume `(lx, ly, lz)` (`Shape.StaticMoments`),
    /// straight from the kernel's integral so the values match its precision.
    fn static_moments(&self, _shape: &Shape) -> Option<[f64; 3]> {
        None
    }

    /// Whether every sub-shape of `shape` passes the kernel's validity checks
    /// (`Shape.isValid`).
    ///
    /// A backend that cannot check validity reports `false`; so does a null
    /// shape.
    fn is_valid(&self, _shape: &Shape) -> bool {
        false
    }

    /// The 3x3 rotational inertia matrix of `shape`, embedded in the top-left of
    /// a 4x4 whose last row and column are the identity's (`Shape.MatrixOfInertia`).
    ///
    /// The kernel integrates over the shape's highest dimension, as upstream does:
    /// volume for a solid, area for a face, length for an edge. A backend that
    /// cannot measure the shape reports `None`.
    fn inertia_matrix(&self, _shape: &Shape) -> Option<Matrix4> {
        None
    }

    /// The principal moments, axes and radii of gyration
    /// (`Shape.PrincipalProperties`). `None` when the kernel cannot measure it.
    fn principal_properties(&self, _shape: &Shape) -> Option<PrincipalProperties> {
        None
    }

    /// A copy of `shape` with redundant edges/faces unified (`Shape.cleaned`).
    ///
    /// A backend without a cleaning pass reports `None`; the caller keeps the
    /// original shape.
    fn clean(&self, _shape: &Shape) -> Option<Shape> {
        None
    }

    /// Offset a face or a wire by `distance` in the plane it lies in
    /// (`Shape.makeOffset` / `Shape.makeOffset2D`).
    ///
    /// `join` is upstream's `JoinType` (0 = arc, 1 = tangent, 2 = intersection).
    /// The default reports the operation as unsupported.
    fn make_offset(&self, _shape: &Shape, _distance: f64, _join: i32) -> Result<Shape, GeomError> {
        Err(unsupported("make_offset"))
    }

    /// Hollow a solid by `thickness`, leaving `closing_faces` open
    /// (`Shape.makeThickness`).
    fn make_thickness(
        &self,
        _shape: &Shape,
        _closing_faces: &[Shape],
        _thickness: f64,
        _tolerance: f64,
    ) -> Result<Shape, GeomError> {
        Err(unsupported("make_thickness"))
    }

    /// The intersection of `a` and `b` (`Shape.section`).
    fn section(&self, _a: &Shape, _b: &Shape) -> Result<Shape, GeomError> {
        Err(unsupported("section"))
    }

    /// Project `tools` onto `shape` (`Shape.project`).
    ///
    /// The kernel projects each tool along the target's normal; a backend may
    /// restrict this to planar targets. The default reports the operation as
    /// unsupported.
    fn project(&self, _shape: &Shape, _tools: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("project"))
    }

    /// The wires where `shape` meets the plane with normal `direction` that is
    /// `distance` from the origin along that normal (`Shape.slice`).
    fn slice(
        &self,
        _shape: &Shape,
        _direction: Vector3,
        _distance: f64,
    ) -> Result<Vec<Shape>, GeomError> {
        Err(unsupported("slice"))
    }

    /// General fuse of `shapes`: the combined shape and, for each input, the
    /// parts it was split into (`Shape.generalFuse`).
    fn general_fuse(&self, _shapes: &[Shape]) -> Result<(Shape, Vec<Vec<Shape>>), GeomError> {
        Err(unsupported("general_fuse"))
    }

    /// Split `shape` with `tools` (`Part.makeSplitShape`).
    ///
    /// Each pair is `(splitting shape, face being split)`: an edge or wire cuts
    /// the face it lies on. Returns upstream's two groups: the pieces generated
    /// directly by the split (`DirectLeft`) and the pieces left over (`Left`). A
    /// backend without a shape-splitting feature reports the operation as
    /// unsupported.
    fn split_shape(
        &self,
        _shape: &Shape,
        _tools: &[(Shape, Shape)],
        _check_interior: bool,
    ) -> Result<(Vec<Shape>, Vec<Shape>), GeomError> {
        Err(unsupported("split_shape"))
    }

    /// The tight bounds from `BRepBndLib::AddOptimal`
    /// (`Shape.optimalBoundingBox`).
    fn optimal_bounds(
        &self,
        _shape: &Shape,
        _use_triangulation: bool,
        _use_tolerance: bool,
    ) -> Option<(Vector3, Vector3)> {
        None
    }

    /// Whether two shapes are the same sub-shape, ignoring orientation
    /// (`TopoDS_Shape::IsSame`). The default compares handle identity.
    fn same_shape(&self, a: &Shape, b: &Shape) -> bool {
        a.is_same(b)
    }

    /// A deep copy of `shape` (FreeCAD `Shape.copy`): the same geometry and
    /// location in a **new** TShape, so the result is no longer `same_shape` with
    /// the source. The default clones the handle, which is correct for a backend
    /// whose shapes are immutable values.
    fn copy_shape(&self, shape: &Shape) -> Result<Shape, GeomError> {
        Ok(shape.clone())
    }

    /// A hash consistent with [`same_shape`](Self::same_shape): the same TShape
    /// and location hash alike whatever the orientation. The default reports the
    /// operation as unsupported.
    fn shape_hash(&self, _shape: &Shape) -> Result<u64, GeomError> {
        Err(unsupported("shape_hash"))
    }

    /// The number of sub-shapes of `kind` (`Shape.countElement`).
    ///
    /// `kind` is upstream's shape-type spelling (`Face`, `Edge`, `Vertex`,
    /// `Wire`, `Shell`, `Solid`, `Compound`, `CompSolid`); a spelling the
    /// backend does not know counts zero. The default reports the operation as
    /// unsupported, so a backend that cannot enumerate sub-shapes fails
    /// explicitly.
    fn count_sub_shapes(&self, _shape: &Shape, _kind: &str) -> Result<usize, GeomError> {
        Err(unsupported("count_sub_shapes"))
    }

    /// Whether an edge is degenerate (`BRep_Tool::Degenerated`): its 3D curve
    /// collapses to a point, as at a cone apex or a sphere pole.
    ///
    /// The default reports the operation as unsupported.
    fn is_degenerate(&self, _edge: &Shape) -> Result<bool, GeomError> {
        Err(unsupported("is_degenerate"))
    }

    /// Whether `edge` is a seam of `face` (`BRep_Tool::IsClosed(edge, face)`):
    /// the edge has two pcurves in the face because the surface is closed.
    ///
    /// The default reports the operation as unsupported.
    fn is_seam(&self, _edge: &Shape, _face: &Shape) -> Result<bool, GeomError> {
        Err(unsupported("is_seam"))
    }

    /// The parameter range `(first, last)` of an edge (`BRep_Tool::Range`).
    ///
    /// The default reports the operation as unsupported.
    fn edge_parameter_range(&self, _edge: &Shape) -> Result<(f64, f64), GeomError> {
        Err(unsupported("edge_parameter_range"))
    }

    /// The parameter bounds of a face's boundary in its surface's UV space,
    /// `(u0, u1, v0, v1)` (`BRepTools::UVBounds`).
    ///
    /// The default reports the operation as unsupported.
    fn face_parameter_range(&self, _face: &Shape) -> Result<(f64, f64, f64, f64), GeomError> {
        Err(unsupported("face_parameter_range"))
    }

    /// The minimum distance between two shapes and the closest point on each
    /// (`Shape.distToShape`, `BRepExtrema_DistShapeShape`).
    ///
    /// [`ShapeDistance::first`](ShapeDistance) lies on the first shape and
    /// [`second`](ShapeDistance::second) on the second.
    ///
    /// The default reports the operation as unsupported.
    fn dist_to_shape(&self, _first: &Shape, _second: &Shape) -> Result<ShapeDistance, GeomError> {
        Err(unsupported("dist_to_shape"))
    }

    /// Whether `shape` is closed (`Shape.isClosed`).
    ///
    /// Upstream defines this per shape type: a solid or comp-solid when every
    /// shell is closed; a shell, wire or edge (and a face or vertex) via
    /// `BRep_Tool::IsClosed`; and a compound when every shell, and every face,
    /// wire, edge and vertex not contained in a higher element, is closed.
    ///
    /// The default reports the operation as unsupported.
    fn is_closed(&self, _shape: &Shape) -> Result<bool, GeomError> {
        Err(unsupported("is_closed"))
    }

    /// The shape's geometric tolerance (`Shape.getTolerance`).
    ///
    /// The default reports the operation as unsupported.
    fn shape_tolerance(&self, _shape: &Shape) -> Result<f64, GeomError> {
        Err(unsupported("shape_tolerance"))
    }

    /// The 3D point of an edge at curve parameter `t` (`Edge.valueAt`,
    /// `BRepAdaptor_Curve::Value`). A non-edge is invalid input.
    ///
    /// The default reports the operation as unsupported.
    fn edge_value_at(&self, _edge: &Shape, _t: f64) -> Result<Vector3, GeomError> {
        Err(unsupported("edge_value_at"))
    }

    /// The 3D point of a face at surface parameters `(u, v)` (`Face.valueAt`,
    /// `BRepAdaptor_Surface::Value`). A non-face is invalid input.
    ///
    /// The default reports the operation as unsupported.
    fn face_value_at(&self, _face: &Shape, _u: f64, _v: f64) -> Result<Vector3, GeomError> {
        Err(unsupported("face_value_at"))
    }

    /// Whether two shapes are equal (`TopoDS_Shape::IsEqual`): they share the
    /// same TShape and location **and** have the same orientation. Contrast
    /// [`same_shape`](Self::same_shape), which ignores orientation.
    ///
    /// The default reports the operation as unsupported.
    fn is_equal(&self, _a: &Shape, _b: &Shape) -> Result<bool, GeomError> {
        Err(unsupported("is_equal"))
    }

    /// The shape's orientation (`Shape.Orientation`).
    fn orientation(&self, _shape: &Shape) -> Option<&'static str> {
        None
    }

    /// A copy of `shape` with the reversed orientation (`Shape.reversed`).
    fn reversed(&self, _shape: &Shape) -> Result<Shape, GeomError> {
        Err(unsupported("reversed"))
    }

    /// Split an edge at `parameters` (`Edge.split`), returning a chain of edges
    /// in parameter order.
    ///
    /// The parameters are curve parameters on the edge's basis curve, as
    /// upstream's `Edge.split` takes them; they must lie strictly inside the
    /// edge's range. The original edge is not modified. The default reports the
    /// operation as unsupported.
    fn split_edge(&self, _edge: &Shape, _parameters: &[f64]) -> Result<Shape, GeomError> {
        Err(unsupported("split_edge"))
    }

    /// A copy of `shape` with the given sub-shapes removed
    /// (`Shape.removeShape`).
    fn remove_shapes(&self, _shape: &Shape, _shapes: &[Shape]) -> Result<Shape, GeomError> {
        Err(unsupported("remove_shapes"))
    }

    /// A copy of `shape` with each `from` sub-shape replaced by its `to`
    /// (`Shape.replaceShape`).
    fn replace_shapes(
        &self,
        _shape: &Shape,
        _replacements: &[(Shape, Shape)],
    ) -> Result<Shape, GeomError> {
        Err(unsupported("replace_shapes"))
    }

    /// Round the given edges of `shape`.
    fn fillet(
        &self,
        shape: &Shape,
        edges: &[ElementRef],
        radius: f64,
    ) -> Result<OpResult, GeomError>;

    /// Apply a placement (transform) to `shape`.
    fn place(&self, shape: &Shape, placement: &Placement) -> Result<Shape, GeomError>;

    /// Apply a 4x4 matrix to `shape` (`Shape.transformed`).
    fn transform(&self, _shape: &Shape, _matrix: &Matrix4) -> Result<Shape, GeomError> {
        Err(unsupported("transform"))
    }

    /// Apply a general (affine) 4x4 matrix to `shape`.
    ///
    /// [`transform`](GeometryBackend::transform) installs a `gp_Trsf`, which is a
    /// similarity and therefore cannot express shear or a non-uniform scale. A
    /// caller that needs those (an `App::Link` with a non-uniform
    /// `ScaleVector`) uses this operation instead, which OCCT implements with
    /// `BRepBuilderAPI_GTransform`. The matrix's last row must be `(0, 0, 0, 1)`.
    ///
    /// The default reports the operation as unsupported, so a backend that only
    /// has the similarity path keeps compiling and fails explicitly instead of
    /// silently dropping the non-uniform part of the transform.
    fn general_transform(&self, _shape: &Shape, _matrix: &Matrix4) -> Result<Shape, GeomError> {
        Err(unsupported("general_transform"))
    }

    /// Resolve a stable sub-element name against `shape`, if it still exists.
    fn resolve(&self, shape: &Shape, name: &str) -> Option<Shape>;

    /// Serialise a shape for persistence. Core stores the bytes verbatim and never
    /// interprets them; the kernel owns the encoding (OCCT writes BREP).
    fn save_shape(&self, shape: &Shape) -> Vec<u8>;

    /// Reconstruct a shape from [`save_shape`](GeometryBackend::save_shape) bytes,
    /// or `None` if they are not valid.
    fn load_shape(&self, bytes: &[u8]) -> Option<Shape>;

    /// The operations this backend implements.
    ///
    /// The default is [`Capabilities::empty`], so a backend opts in explicitly and a
    /// host can decide which manipulators to offer without probing with failures.
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }

    /// Tessellate `shape` for display, tagging every triangle and segment with the
    /// part id of the sub-element it belongs to.
    ///
    /// The default reports the operation as unsupported; a backend that returns
    /// [`Capabilities::TESSELLATION`] must implement it.
    fn tessellate(
        &self,
        _shape: &Shape,
        _options: &TessellationOptions,
    ) -> Result<Tessellation, GeomError> {
        Err(unsupported("tessellation"))
    }

    /// Enumerate the sub-elements of `kind`, in the kernel's own order.
    ///
    /// Position `i` in the result is the element whose name is `format!("{}{}",
    /// kind.prefix(), i + 1)`, so a positional name round-trips through
    /// [`resolve`](GeometryBackend::resolve).
    fn elements(&self, _shape: &Shape, _kind: ElementKind) -> Vec<ElementRef> {
        Vec::new()
    }

    /// The sub-elements of `kind` that contain (or are contained by) `element` — the
    /// faces of an edge, the edges of a face, and so on.
    fn ancestors(
        &self,
        _shape: &Shape,
        _element: &ElementRef,
        _kind: ElementKind,
    ) -> Vec<ElementRef> {
        Vec::new()
    }

    /// How smoothly `a` and `b` meet, when they are adjacent.
    fn continuity(&self, _shape: &Shape, _a: &ElementRef, _b: &ElementRef) -> Option<Continuity> {
        None
    }

    /// Chamfer `edges`.
    ///
    /// `d2` is `None` for a symmetric chamfer and `Some(distance)` for the
    /// two-distance form, where `d1` and `d2` are measured on the two adjacent
    /// faces.
    fn chamfer(
        &self,
        _shape: &Shape,
        _edges: &[ElementRef],
        _d1: f64,
        _d2: Option<f64>,
    ) -> Result<OpResult, GeomError> {
        Err(unsupported("chamfer"))
    }
}

// ---------------------------------------------------------------------------
// Tangent chains
// ---------------------------------------------------------------------------

/// The edges tangent-continuous with `seed`, in the order the walk reaches them.
///
/// A fillet or chamfer applies to a whole smooth chain, not to the one segment the
/// user happened to click: the edges either side of a rounded corner are tangent, and
/// filleting only one leaves a visible step. The walk is over the kernel's own
/// topology — [`elements`](GeometryBackend::elements) for the candidate edges,
/// [`ancestors`](GeometryBackend::ancestors) for shared vertices, and
/// [`continuity`](GeometryBackend::continuity) for the join — so it works for any
/// backend that reports them.
///
/// Two edges are neighbours when they share a vertex. A neighbour joins the chain when
/// the join is smoother than [`Continuity::C0`] (`G1` or better; OCCT reports tangency
/// only). The seed is always first in the result, even when the backend cannot
/// enumerate it.
pub fn tangent_chain(
    backend: &dyn GeometryBackend,
    shape: &Shape,
    seed: &ElementRef,
) -> Vec<ElementRef> {
    let mut edges = backend.elements(shape, ElementKind::Edge);
    if !edges.iter().any(|edge| edge.name == seed.name) {
        edges.push(seed.clone());
    }

    // Vertex name → the edges that touch it. Names are the kernel's positional ones
    // (`Vertex3`), so they are the same for `elements` and `ancestors`.
    let mut by_vertex: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, edge) in edges.iter().enumerate() {
        for vertex in backend.ancestors(shape, edge, ElementKind::Vertex) {
            by_vertex.entry(vertex.name).or_default().push(index);
        }
    }

    let Some(seed_index) = edges.iter().position(|edge| edge.name == seed.name) else {
        return vec![seed.clone()];
    };

    let mut visited = vec![false; edges.len()];
    visited[seed_index] = true;
    let mut chain = vec![seed_index];
    let mut cursor = 0;
    while cursor < chain.len() {
        let current = chain[cursor];
        cursor += 1;

        let mut candidates = Vec::new();
        for vertex in backend.ancestors(shape, &edges[current], ElementKind::Vertex) {
            if let Some(indices) = by_vertex.get(&vertex.name) {
                candidates.extend(indices.iter().copied());
            }
        }
        candidates.sort_unstable();
        candidates.dedup();

        for candidate in candidates {
            if visited[candidate] {
                continue;
            }
            let smoother_than_sharp = matches!(
                backend.continuity(shape, &edges[current], &edges[candidate]),
                Some(Continuity::G1 | Continuity::C1 | Continuity::G2 | Continuity::C2)
            );
            if smoother_than_sharp {
                visited[candidate] = true;
                chain.push(candidate);
            }
        }
    }

    chain
        .into_iter()
        .map(|index| edges[index].clone())
        .collect()
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

    fn make_edge(&self, curve: &EdgeCurve) -> Result<Shape, GeomError> {
        // The null backend invents no geometry, but it keeps the seam exercised.
        Ok(Shape::new(NullShape {
            kind: format!("edge:{}", curve.kind()),
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

    #[test]
    fn element_kinds_carry_their_freecad_prefixes() {
        assert_eq!(ElementKind::Face.prefix(), "Face");
        assert_eq!(ElementKind::Vertex.prefix(), "Vertex");
        assert_eq!(ElementKind::ALL.len(), 6);
    }

    #[test]
    fn capabilities_compose_and_test_membership() {
        let solid = Capabilities::BOOLEANS | Capabilities::FILLET;
        assert!(solid.contains(Capabilities::FILLET));
        assert!(!solid.contains(Capabilities::CHAMFER));
        assert!(Capabilities::all().contains(Capabilities::all()));
        assert!(Capabilities::empty().is_empty());
        assert!(solid.contains(Capabilities::empty()));
        assert!(!Capabilities::empty().contains(Capabilities::TESSELLATION));
    }

    #[test]
    fn element_map_folds_history() {
        let input = ElementRef {
            shape: Shape::new("in"),
            name: "Face1".to_string(),
        };
        let modified = ElementRef {
            shape: Shape::new("out-m"),
            name: "Face2".to_string(),
        };
        let generated = ElementRef {
            shape: Shape::new("out-g"),
            name: "Face3".to_string(),
        };
        let deleted = ElementRef {
            shape: Shape::new("gone"),
            name: "Face9".to_string(),
        };
        let mut history = History::default();
        history.modified.push((input.clone(), modified.clone()));
        history.generated.push((input, generated));
        history.deleted.push(deleted);

        let mut map = ElementMap::from_history(&history);
        let resolved = map.get("Face1").expect("the input name survives");
        assert!(resolved.is_same(&modified.shape), "the first output wins");
        assert!(map.get("Face9").is_none(), "a deleted element is absent");

        // A later rebuild that deletes the element removes the name.
        let deleted_now = History {
            deleted: vec![ElementRef {
                shape: Shape::new("gone"),
                name: "Face1".to_string(),
            }],
            ..History::default()
        };
        map.apply(&deleted_now);
        assert!(map.get("Face1").is_none(), "the name is retired");
    }

    #[test]
    fn null_backend_reports_no_topology() {
        let backend = NullBackend;
        let shape = backend.make_box(1.0, 1.0, 1.0).unwrap();
        let face = ElementRef {
            shape: shape.clone(),
            name: "Face1".to_string(),
        };

        assert!(backend.capabilities().is_empty());
        assert!(backend.elements(&shape, ElementKind::Face).is_empty());
        assert!(
            backend
                .ancestors(&shape, &face, ElementKind::Edge)
                .is_empty()
        );
        assert!(backend.continuity(&shape, &face, &face).is_none());
        assert!(
            backend
                .tessellate(&shape, &TessellationOptions::default())
                .is_err()
        );
        assert!(backend.chamfer(&shape, &[face], 1.0, None).is_err());
    }

    #[test]
    fn tessellation_reports_parts_and_primitives() {
        let mesh = Tessellation {
            positions: vec![[0.0_f32; 3]; 6],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 3, 4, 5],
            triangle_parts: vec![0, 1],
            edges: vec![0, 1, 2, 3, 4, 5],
            segment_parts: vec![0, 1, 0],
            vertices: vec![0, 2, 4],
            vertex_parts: vec![0, 0, 1],
            parts: vec!["Face1".to_string(), "Face2".to_string()],
        };

        assert!(!mesh.is_empty());
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.segment_count(), 3);
        assert_eq!(mesh.part_id_of_triangle(1), Some(1));
        assert_eq!(mesh.part_id_of_segment(2), Some(0));
        assert_eq!(mesh.part_id_of_vertex(2), Some(1));
        assert_eq!(mesh.part_id_of_vertex(3), None);
        assert_eq!(mesh.part_name(1), Some("Face2"));
        assert_eq!(mesh.part_id("Face2"), Some(1));
        assert_eq!(mesh.part_id("Face3"), None);
        assert_eq!(mesh.part_id_of_triangle(2), None);
    }

    #[test]
    fn selection_refs_describe_whole_objects_and_elements() {
        let whole = SelectionRef::whole(7);
        assert_eq!(whole.object, 7);
        assert!(whole.is_whole());

        let face = SelectionRef::element(7, "Face6");
        assert_eq!(face.elements, vec!["Face6".to_string()]);
        assert!(!face.is_whole());
    }

    #[test]
    fn default_tessellation_options_are_scale_free() {
        let options = TessellationOptions::default();
        assert_eq!(options.linear_deflection, None);
        assert_eq!(options.angular_deflection, 0.5);
        assert!(!options.relative);
        assert_eq!(
            TessellationOptions::absolute(0.1).linear_deflection,
            Some(0.1)
        );
    }

    /// A tiny edge graph, so the tangent-chain walk is tested without a kernel:
    /// `Edge1` and `Edge2` meet at `Vertex1` tangentially, `Edge2` and `Edge3` meet at
    /// `Vertex2` sharply.
    struct GraphBackend;

    fn edge(name: &str) -> ElementRef {
        ElementRef {
            shape: Shape::new(NullShape {
                kind: name.to_string(),
            }),
            name: name.to_string(),
        }
    }

    impl GeometryBackend for GraphBackend {
        fn make_box(&self, _l: f64, _w: f64, _h: f64) -> Result<Shape, GeomError> {
            Err(unsupported("make box"))
        }

        fn fuse(&self, _a: &Shape, _b: &Shape) -> Result<OpResult, GeomError> {
            Err(unsupported("fuse"))
        }

        fn cut(&self, _a: &Shape, _b: &Shape) -> Result<OpResult, GeomError> {
            Err(unsupported("cut"))
        }

        fn fillet(
            &self,
            _shape: &Shape,
            _edges: &[ElementRef],
            _radius: f64,
        ) -> Result<OpResult, GeomError> {
            Err(unsupported("fillet"))
        }

        fn place(&self, _shape: &Shape, _placement: &Placement) -> Result<Shape, GeomError> {
            Err(unsupported("place"))
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

        fn elements(&self, _shape: &Shape, kind: ElementKind) -> Vec<ElementRef> {
            if kind == ElementKind::Edge {
                vec![edge("Edge1"), edge("Edge2"), edge("Edge3")]
            } else {
                Vec::new()
            }
        }

        fn ancestors(
            &self,
            _shape: &Shape,
            element: &ElementRef,
            kind: ElementKind,
        ) -> Vec<ElementRef> {
            if kind != ElementKind::Vertex {
                return Vec::new();
            }
            let vertices: &[&str] = match element.name.as_str() {
                "Edge1" => &["Vertex1"],
                "Edge2" => &["Vertex1", "Vertex2"],
                "Edge3" => &["Vertex2"],
                _ => &[],
            };
            vertices.iter().map(|name| edge(name)).collect()
        }

        fn continuity(&self, _shape: &Shape, a: &ElementRef, b: &ElementRef) -> Option<Continuity> {
            match (a.name.as_str(), b.name.as_str()) {
                ("Edge1", "Edge2") | ("Edge2", "Edge1") => Some(Continuity::G1),
                ("Edge2", "Edge3") | ("Edge3", "Edge2") => Some(Continuity::C0),
                _ => None,
            }
        }
    }

    fn graph_shape() -> Shape {
        Shape::new(NullShape {
            kind: "graph".to_string(),
        })
    }

    #[test]
    fn a_tangent_chain_follows_smooth_neighbours_only() {
        let shape = graph_shape();
        let chain = tangent_chain(&GraphBackend, &shape, &edge("Edge1"));
        assert_eq!(
            chain.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Edge1", "Edge2"],
            "the sharp Edge3 does not join the chain"
        );

        // Starting mid-chain reaches the smooth side and stops at the sharp one.
        let chain = tangent_chain(&GraphBackend, &shape, &edge("Edge2"));
        assert_eq!(
            chain.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Edge2", "Edge1"]
        );
    }

    #[test]
    fn a_tangent_chain_of_an_isolated_edge_is_just_that_edge() {
        let shape = graph_shape();
        let seed = edge("Edge3");
        let names = |chain: Vec<ElementRef>| {
            chain
                .iter()
                .map(|edge| edge.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(tangent_chain(&GraphBackend, &shape, &seed)),
            ["Edge3"]
        );

        // A backend that reports no topology still returns the seed.
        assert_eq!(names(tangent_chain(&NullBackend, &shape, &seed)), ["Edge3"]);
    }
}

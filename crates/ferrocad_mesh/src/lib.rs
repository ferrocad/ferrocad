//! # FerroCAD Mesh workbench (`ferrocad_mesh`)
//!
//! The Rust core of the Mesh workbench: the triangle-mesh payload, its editing
//! and analysis operations, and the `Mesh::Feature` document object. It registers
//! two things with `ferrocad_core`:
//!
//! - **`Mesh::PropertyMeshKernel`** — a property *value* type holding a [`Mesh`], via
//!   [`ferrocad_core::extension`];
//! - **`Mesh::Feature`** — an object type with `Mesh` and `Placement` properties, via
//!   [`ferrocad_core::object_registry::ObjectType`].
//!
//! ```no_run
//! use ferrocad_mesh::{Mesh, MeshProperty, make_mesh_property, mesh_of};
//!
//! ferrocad_mesh::register();
//! let mut doc = ferrocad_core::Document::new();
//! let id = doc.add_object("Box", ferrocad_mesh::FEATURE_TYPE);
//! let mesh = Mesh::box_(10.0, 10.0, 10.0).unwrap();
//! doc.set_property(id, "Mesh", make_mesh_property(MeshProperty::new(mesh)))
//!     .unwrap();
//! let stored = mesh_of(doc.object(id).unwrap().properties.get("Mesh").unwrap())
//!     .unwrap();
//! assert!(stored.mesh().is_solid());
//! ```
//!
//! # Scope
//!
//! This slice is the **Rust core only**. It does not bind Python, does not use
//! OCCT, and does not depend on `ferrocad_geom::Shape`; it is a pure triangle mesh.
//! The dependencies are [`ferrocad_core`] (the property/object registries) and
//! [`ferrocad_types`] (`Vector3`, `Placement`).
//!
//! - **STL** is supported in both directions: binary writing and binary/ASCII
//!   reading, with exact-coordinate welding on read.
//! - **Editing and analysis** live beside the payload: `area`, point/facet
//!   deduplication, degenerate-facet removal, normal harmonisation, connected
//!   components, planar segmentation, self-intersection detection and boundary
//!   loops. They all operate on the local points and keep the placement.
//! - **`.bms`** (SimpleCAD's own job-layer mesh format) is out of scope for this
//!   slice and is neither read nor written.
//! - **`.FCStd` mesh members** are a later slice: `fcstd_members()` returns `None`
//!   because core's `.FCStd` writer currently hardcodes the `PartShapeN` stem.
//!   Native (`.ferrocad`) documents persist the property normally.
//! - **`MeshPart.wireFromSegment`**'s wire construction is out of scope; the Part
//!   bridge needs a kernel and a shape type, which this crate deliberately does
//!   not have. [`Mesh::boundary_loops`] supplies the kernel half (the ordered
//!   point loops) for that binding.

mod boundary;
mod components;
mod intersect;
mod mesh;
mod mesh_property;
mod ops;
mod segments;
mod stl;

use std::sync::OnceLock;

pub use intersect::SelfIntersection;
pub use mesh::{BoundBox, Mesh, MeshError};
pub use mesh_property::{FEATURE_TYPE, MeshProperty, TYPE_NAME, make_mesh_property, mesh_of};

/// Register `Mesh::PropertyMeshKernel` and `Mesh::Feature`. Idempotent.
///
/// An embedding application calls this once at startup before creating a
/// `Mesh::Feature` or restoring a document that contains one.
pub fn register() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(mesh_property::register);
}

//! # FerroCAD core (`ferrocad_core`)
//!
//! The pure-Rust document model behind the `FreeCAD` Python package: typed
//! properties, a document object model with a dependency graph, transactions with
//! undo/redo, observers, expressions, and FreeCAD's geometry and unit types. No
//! Python and no UI.
//!
//! This crate is the *engine*. [`ferrocad_py`](https://crates.io/crates/ferrocad_py)
//! binds it to Python, and the pure-Python `FreeCAD` facade (the `ferrocad`
//! distribution) presents it as the drop-in `import FreeCAD` package.
//!
//! The documentation is organised following [Diátaxis](https://diataxis.fr):
//! a tutorial to get started, how-to guides for common tasks, an explanation of
//! the design, and a reference (the item pages).
//!
//! # Tutorial
//!
//! Build a document, wire a dependency with an expression, and recompute it.
//!
//! ```
//! use ferrocad_core::{Document, Property};
//!
//! let mut doc = Document::new();
//! let width = doc.add_object("Width", "App::Feature");
//! let area = doc.add_object("Area", "App::Feature");
//!
//! // A value on one object…
//! doc.set_property(width, "Value", Property::Float(10.0)).unwrap();
//! // …an expression on another (dependencies are tracked automatically)…
//! doc.set_expression(area, "Result", "Width.Value * 2").unwrap();
//!
//! // …and a recompute in dependency order (returns the objects executed).
//! assert_eq!(doc.recompute().unwrap().len(), 2);
//! assert_eq!(
//!     doc.object(area).unwrap().properties.get("Result"),
//!     Some(&Property::Float(20.0)),
//! );
//! ```
//!
//! # How-to guides
//!
//! - **Add a typed property with flags**: [`Document::add_property`] takes a value
//!   and a `PropertyType` bitmask (see [`prop_status`]); unlike [`Document::set_property`]
//!   it does not touch the object. Setting a property touches it unless it is
//!   `Output`/`NoRecompute`.
//! - **Undo a change**: wrap edits in [`Document::open_transaction`] …
//!   [`Document::commit_transaction`], then [`Document::undo`] / [`Document::redo`].
//! - **Persist a document**: [`Document::save_to_file`] / [`Document::load_from_file`]
//!   (JSON). Properties flagged `NoPersist` are dropped from the saved form.
//! - **React to changes**: register an [`Observer`] with [`Document::add_observer`].
//! - **Name objects**: [`Document::add_object_with`] applies FreeCAD's rules — names
//!   are sanitized ([`sanitize_name`]) and made unique, labels are unique unless
//!   duplicates are allowed.
//!
//! # Explanation
//!
//! A [`Document`] owns a set of [`DocumentObject`]s and a dependency graph over
//! them. Each object is a [`PropertyContainer`] — a name → value map where every
//! property also carries a status bitmask ([`prop_status`]). Recompute walks the
//! graph in dependency order and evaluates each object's expressions.
//!
//! Editing goes through transactions: [`Document::set_property`] records a
//! reversible change so a whole edit can be undone or redone. Assigning a property
//! *touches* the object (sets its `must_execute` flag) so a later recompute knows
//! what is dirty; `Output`/`NoRecompute` properties opt out.
//!
//! The value types mirror FreeCAD's `Base` module: [`Quantity`]/[`Unit`] for values
//! with units, and [`Vector3`]/[`Matrix4`]/[`Rotation`]/[`Placement`] for geometry.
//! Names are FreeCAD-compatible: internal names are sanitized and made unique,
//! display labels are a separate, user-facing string.
//!
//! # Implemented capabilities
//!
//! The crate grew milestone by milestone (see the repository `docs/milestones.md`):
//!
//! - **M2** — the core model: [`Property`]/[`PropertyContainer`], [`Quantity`]/[`Unit`],
//!   the `petgraph` recompute DAG, transaction-backed undo/redo, [`Observer`]s,
//!   and the expression parser/evaluator.
//! - **M4 (slices 1–16)** — the `Base`/`App` surface bound to Python: geometry helpers,
//!   persistence (`SavedDocument`), document metadata, extensions/groups, links,
//!   observers that fire, Python-object/`Proxy` persistence, expressions, and
//!   matrix decomposition / rotation numerics.
//! - **MVP slice A1** — object state and per-property status flags; touch-on-assign.
//! - **MVP slice B1** — name/label semantics ([`sanitize_name`], [`Document::unique_name`]).
//! - **MVP slice A2** — [`Property::Enumeration`] and type-registry validation.
//! - **MVP slice B2** — a general reversible undo/redo change set (property edits,
//!   object add/remove, expressions) with named, id-carrying transactions, an active-object
//!   pointer, and FreeCAD's undo metadata ([`Document::undo_names`],
//!   [`Document::getAvailableUndos`](Document::available_undos),
//!   [`Document::active_object`]).
//!
//! # Reference
//!
//! The item pages (`Document`, `DocumentObject`, `Property`, the geometry and unit
//! types, and the supporting modules) are the reference. The `mod` items below are
//! the module-level documentation.

mod application;
mod document;
mod expr;
mod observer;
mod property;
mod stringhasher;
mod typeregistry;

// The base value types (`Quantity`/`Unit`/geometry) live in `ferrocad_types`, a
// lower leaf crate, and are re-exported here so `ferrocad_core::Quantity` and the
// `crate::geometry::…` paths inside this crate keep working.
pub use ferrocad_types::{geometry, quantity, unit};

pub use application::{application, Application, DocumentHandle, DocumentObserver};
pub use document::{sanitize_name, Document, DocumentObject, ObjectId, SavedDocument};
pub use geometry::{Matrix4, Placement, Rotation, ScaleType, TypeId, Vector3};
pub use observer::Observer;
pub use property::{prop_status, status_from_name, status_names, Property, PropertyContainer};
pub use quantity::{canonical_name, parse_unit, Quantity};
pub use stringhasher::{StringHasher, StringId};
pub use unit::Unit;


#[cfg(test)]
mod tests;

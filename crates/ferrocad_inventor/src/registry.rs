//! The runtime type registry — Coin's `SoType`, the cross-language ABI.
//!
//! Python workbenches instantiate node classes *by name* at runtime:
//!
//! ```python
//! coin.SoType.fromName("SoBrepEdgeSet").createInstance()
//! ```
//!
//! For that to keep working once Coin is gone, the name⇄type lookup must be
//! reproduced faithfully, including the FreeCAD/Part/Mesh classes registered from
//! outside the core. [`TypeRegistry`] is that table: a name maps to a factory that
//! builds the corresponding [`Node`].

use std::collections::HashMap;

use crate::node::Node;

/// A name → factory table mirroring `SoType.fromName(...).createInstance()`.
///
/// FreeCAD's startup registers every custom node class here (the analogue of
/// `SoFCDB::init()`); the `coin` facade then forwards Python lookups to it.
#[derive(Default)]
pub struct TypeRegistry {
    factories: HashMap<String, fn() -> Node>,
}

impl TypeRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name` with a factory that builds a fresh instance.
    pub fn register(&mut self, name: impl Into<String>, factory: fn() -> Node) {
        self.factories.insert(name.into(), factory);
    }

    /// Build an instance for `name`, or `None` if it is not registered
    /// (Coin's `SoType.fromName` returns an unknown type rather than failing).
    pub fn create(&self, name: &str) -> Option<Node> {
        self.factories.get(name).map(|factory| factory())
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// The registered type names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }
}

//! # FerroCAD lineage vocabulary (`ferrocad_lineage`)
//!
//! The kernel-independent vocabulary for provenance: what an operation did to
//! its inputs, in terms any kernel can report, and the stable-name registry
//! derived from it.
//!
//! Everything here is generic over an opaque payload `T`, so this crate is
//! unable to name a geometry type. `ferrocad_geom` instantiates the vocabulary
//! with its `Shape` handle, and tests can run the same algorithms with a
//! `String` payload (see `docs/architecture/lineage-and-naming.md`).
//!
//! It provides three things:
//!
//! - [`ElementRef`] — one sub-element, by a stable name rather than a positional
//!   index.
//! - [`History`] — what one operation did to its inputs.
//! - [`ElementMap`] — the stable-name registry carried with a shape.
//!
//! This crate has no workspace dependencies. It must not learn about geometry,
//! booleans, documents, selection or persistence formats.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// History and element references
// ---------------------------------------------------------------------------

/// A reference to one sub-element of a shape (a face, an edge, …), by a stable
/// name rather than a positional index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementRef<T> {
    /// The sub-element itself.
    pub shape: T,
    /// Its stable name (e.g. `Face6`, or an encoded lineage token).
    pub name: String,
}

/// What one operation did to its inputs, in the kernel's own terms.
///
/// This is the raw material for the element map: a mapper folds it into stable names.
/// The kernel reports exactly what it knows; the mapper can be no better than this.
#[derive(Debug, Clone, PartialEq)]
pub struct History<T> {
    /// `(input, output)` pairs the operation created.
    pub generated: Vec<(ElementRef<T>, ElementRef<T>)>,
    /// `(input, output)` pairs the operation changed.
    pub modified: Vec<(ElementRef<T>, ElementRef<T>)>,
    /// Inputs that do not survive into the output.
    pub deleted: Vec<ElementRef<T>>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            generated: Vec::new(),
            modified: Vec::new(),
            deleted: Vec::new(),
        }
    }
}

impl<T> History<T> {
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
#[derive(Debug, Clone, PartialEq)]
pub struct ElementMap<T> {
    entries: BTreeMap<String, T>,
}

impl<T> Default for ElementMap<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T> ElementMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the current sub-shape for `name`.
    pub fn insert(&mut self, name: impl Into<String>, shape: T) {
        self.entries.insert(name.into(), shape);
    }

    /// The current sub-shape for `name`, if the name still resolves.
    pub fn get(&self, name: &str) -> Option<&T> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(name, shape)` in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Fold an operation's [`History`] into the map.
    ///
    /// Each `generated`/`modified` input name comes to mean its output sub-shape, so a
    /// reference recorded before the operation still resolves after the rebuild. When
    /// one input maps to several outputs, the **first** output wins: a single name
    /// cannot denote a set, and composing names (`Face6a`, `Face6b`) is the mapper's
    /// job (see `docs/architecture/geometry-and-topology.md` §9). An input the operation deleted is
    /// removed, so a stale reference fails to resolve instead of pointing at geometry
    /// that is no longer there.
    ///
    /// Input names are the names the history was built with. With positional names
    /// that is the input shape's own ordinal; a lineage-derived naming scheme can feed
    /// the same method with better names later.
    pub fn apply(&mut self, history: &History<T>)
    where
        T: Clone,
    {
        for (input, output) in history.modified.iter().chain(&history.generated) {
            self.entries
                .entry(input.name.clone())
                .or_insert_with(|| output.shape.clone());
        }
        for deleted in &history.deleted {
            self.entries.remove(&deleted.name);
        }
    }

    /// A map seeded from one operation's history.
    pub fn from_history(history: &History<T>) -> Self
    where
        T: Clone,
    {
        let mut map = Self::new();
        map.apply(history);
        map
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementMap, ElementRef, History};

    fn element(shape: &str, name: &str) -> ElementRef<String> {
        ElementRef {
            shape: shape.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn map_inserts_gets_and_counts() {
        let mut map = ElementMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.insert("Face1", "face-1".to_string());
        map.insert("Face2", "face-2".to_string());

        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
        assert_eq!(map.get("Face1"), Some(&"face-1".to_string()));
        assert_eq!(map.get("Face2"), Some(&"face-2".to_string()));
        assert_eq!(map.get("Face3"), None);
    }

    #[test]
    fn map_iterates_in_name_order() {
        let mut map = ElementMap::new();
        map.insert("Face10", "ten".to_string());
        map.insert("Face2", "two".to_string());
        map.insert("Edge1", "edge".to_string());

        let names: Vec<&str> = map.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["Edge1", "Face10", "Face2"]);

        let values: Vec<&str> = map.iter().map(|(_, shape)| shape.as_str()).collect();
        assert_eq!(values, ["edge", "ten", "two"]);
    }

    #[test]
    fn apply_keeps_the_first_output_for_a_name() {
        let input = element("in", "Face1");
        let first = element("out-1", "Face2");
        let second = element("out-2", "Face3");

        let mut history = History::default();
        // Two outputs for one input: only the first output is recorded.
        history.generated.push((input.clone(), first.clone()));
        history.generated.push((input, second));

        let mut map = ElementMap::new();
        map.apply(&history);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("Face1"), Some(&"out-1".to_string()));
    }

    #[test]
    fn apply_does_not_rebind_an_existing_name() {
        let mut map = ElementMap::new();
        map.insert("Face1", "pinned".to_string());

        let mut history = History::default();
        history
            .modified
            .push((element("in", "Face1"), element("out", "Face2")));
        map.apply(&history);

        assert_eq!(map.get("Face1"), Some(&"pinned".to_string()));
    }

    #[test]
    fn apply_removes_deleted_names() {
        let mut map = ElementMap::new();
        map.insert("Face1", "kept".to_string());
        map.insert("Face2", "gone".to_string());

        let history = History {
            deleted: vec![element("stale", "Face2")],
            ..History::default()
        };
        map.apply(&history);

        assert_eq!(map.len(), 1);
        assert_eq!(map.get("Face1"), Some(&"kept".to_string()));
        assert_eq!(map.get("Face2"), None);
    }

    #[test]
    fn from_history_seeds_all_relations() {
        let mut history = History::default();
        history
            .modified
            .push((element("in-m", "Face1"), element("out-m", "Face2")));
        history
            .generated
            .push((element("in-g", "Edge1"), element("out-g", "Edge2")));
        history.deleted.push(element("gone", "Face9"));

        let map = ElementMap::from_history(&history);

        assert_eq!(map.len(), 2);
        assert_eq!(map.get("Face1"), Some(&"out-m".to_string()));
        assert_eq!(map.get("Edge1"), Some(&"out-g".to_string()));
        assert_eq!(map.get("Face9"), None);
    }

    #[test]
    fn empty_history_is_empty() {
        assert!(History::<String>::default().is_empty());

        let mut history = History::default();
        history.deleted.push(element("gone", "Face1"));
        assert!(!history.is_empty());
    }
}

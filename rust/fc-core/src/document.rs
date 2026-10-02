//! A document object model with a dependency graph, transactions, observers,
//! and expression-driven recompute.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::NodeIndex;
use petgraph::stable_graph::StableGraph;
use serde::{Deserialize, Serialize};

use crate::expr;
use crate::observer::Observer;
use crate::property::{Property, PropertyContainer};
use crate::transaction::{PropertyChange, TransactionManager};

pub type ObjectId = usize;

#[derive(Debug)]
pub struct DocumentObject {
    pub id: ObjectId,
    pub name: String,
    /// Display name; defaults to `name` and is mutable.
    pub label: String,
    pub type_id: String,
    pub properties: PropertyContainer,
    /// Property name → expression source (evaluated on recompute).
    pub expressions: BTreeMap<String, String>,
    /// Dynamic extension type ids added via `addExtension`.
    pub extensions: BTreeSet<String>,
    /// Set by `enforceRecompute`; cleared when the object recomputes.
    pub must_execute: bool,
}

#[derive(Default)]
pub struct Document {
    objects: BTreeMap<ObjectId, DocumentObject>,
    next_id: ObjectId,
    /// Edges point dependency → dependant (dependency recomputes first).
    graph: StableGraph<ObjectId, ()>,
    index: BTreeMap<ObjectId, NodeIndex>,
    tx: TransactionManager,
    observers: Vec<Box<dyn Observer>>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_object(&mut self, name: &str, type_id: &str) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;

        // FreeCAD-style default naming: last segment of the type id + counter.
        let name = if name.is_empty() {
            let base = type_id.rsplit("::").next().unwrap_or("Object");
            let base = if base.is_empty() { "Object" } else { base };
            let mut candidate = base.to_string();
            let mut i = 1;
            while self.objects.values().any(|o| o.name == candidate) {
                candidate = format!("{base}{i:03}");
                i += 1;
            }
            candidate
        } else {
            name.to_string()
        };

        let node = self.graph.add_node(id);
        self.index.insert(id, node);

        // Initialize default properties for known types (FeatureTest, …).
        let mut properties = PropertyContainer::new();
        for (prop_name, prop) in crate::typeregistry::default_properties(type_id) {
            properties.set(prop_name.to_string(), prop);
        }

        self.objects.insert(
            id,
            DocumentObject {
                id,
                label: name.clone(),
                name,
                type_id: type_id.to_string(),
                properties,
                expressions: BTreeMap::new(),
                extensions: BTreeSet::new(),
                must_execute: false,
            },
        );
        for observer in &mut self.observers {
            observer.on_object_added(id);
        }
        id
    }

    pub fn object(&self, id: ObjectId) -> Option<&DocumentObject> {
        self.objects.get(&id)
    }

    pub fn object_ids(&self) -> Vec<ObjectId> {
        self.objects.keys().copied().collect()
    }

    pub fn get_by_name(&self, name: &str) -> Option<ObjectId> {
        self.objects.values().find(|o| o.name == name).map(|o| o.id)
    }

    /// Remove an object (and its graph node/edges). Returns false if absent.
    pub fn remove_object(&mut self, id: ObjectId) -> bool {
        let name = match self.objects.get(&id) {
            Some(o) => o.name.clone(),
            None => return false,
        };
        if self.objects.remove(&id).is_none() {
            return false;
        }
        if let Some(node) = self.index.remove(&id) {
            self.graph.remove_node(node);
        }
        // Drop the removed object from any group's `Group` link list.
        for other in self.objects.values_mut() {
            if let Some(Property::LinkList(links)) = other.properties.get_mut("Group") {
                links.retain(|n| n != &name);
            }
        }
        true
    }

    pub fn set_label(&mut self, id: ObjectId, label: &str) -> bool {
        match self.objects.get_mut(&id) {
            Some(obj) => {
                obj.label = label.to_string();
                true
            }
            None => false,
        }
    }

    pub fn remove_property(&mut self, object: ObjectId, name: &str) -> bool {
        match self.objects.get_mut(&object) {
            Some(obj) => obj.properties.remove(name).is_some(),
            None => false,
        }
    }

    // -- extensions ---------------------------------------------------------

    /// Record a dynamic extension on an object.
    pub fn add_extension(&mut self, id: ObjectId, ext: &str) -> bool {
        match self.objects.get_mut(&id) {
            Some(obj) => obj.extensions.insert(ext.to_string()),
            None => false,
        }
    }

    /// Whether an object has `ext`, considering extension inheritance (e.g.
    /// `App::GroupExtensionPython` derives from `App::GroupExtension`).
    pub fn has_extension(&self, id: ObjectId, ext: &str) -> bool {
        self.object(id)
            .map(|o| o.extensions.iter().any(|e| extension_is_or_derives(e, ext)))
            .unwrap_or(false)
    }

    pub fn remove_extension(&mut self, id: ObjectId, ext: &str) -> bool {
        match self.objects.get_mut(&id) {
            Some(obj) => obj.extensions.remove(ext),
            None => false,
        }
    }

    /// Whether the object acts as a group (document group, part, or has a
    /// group extension) and therefore carries a `Group` link list.
    pub fn is_group_like(&self, id: ObjectId) -> bool {
        self.object(id)
            .map(|o| {
                o.type_id == "App::DocumentObjectGroup"
                    || o.type_id == "App::Part"
                    || o.extensions
                        .iter()
                        .any(|e| extension_is_or_derives(e, "App::GroupExtension"))
            })
            .unwrap_or(false)
    }

    /// Remove `member` from any group that lists it (single-group enforcement).
    pub fn unlink_from_groups(&mut self, member: ObjectId) {
        let name = match self.object(member) {
            Some(o) => o.name.clone(),
            None => return,
        };
        for other in self.objects.values_mut() {
            if let Some(Property::LinkList(links)) = other.properties.get_mut("Group") {
                links.retain(|n| n != &name);
            }
        }
    }

    /// Declare that `object` depends on `depends_on` (the latter recomputes first).
    pub fn add_dependency(&mut self, object: ObjectId, depends_on: ObjectId) {
        if let (Some(&a), Some(&b)) = (self.index.get(&depends_on), self.index.get(&object)) {
            self.graph.add_edge(a, b, ());
        }
    }

    /// Object ids in dependency-first order. `Err` if the graph has a cycle.
    pub fn recompute_order(&self) -> Result<Vec<ObjectId>, String> {
        let nodes = petgraph::algo::toposort(&self.graph, None)
            .map_err(|_| "dependency cycle".to_string())?;
        Ok(nodes.into_iter().map(|n| self.graph[n]).collect())
    }

    // -- properties (transactional + observable) ----------------------------

    pub fn set_property(&mut self, object: ObjectId, name: &str, value: Property) -> Result<(), String> {
        let old = {
            let obj = self
                .objects
                .get_mut(&object)
                .ok_or_else(|| format!("no object {object}"))?;
            obj.properties.get(name).cloned()
        };
        {
            let obj = self.objects.get_mut(&object).unwrap();
            obj.properties.set(name.to_string(), value.clone());
        }

        if self.tx.is_active() || self.tx.has_pending() {
            self.tx.record(PropertyChange {
                object,
                name: name.to_string(),
                old: old.clone(),
                new: value.clone(),
            });
        }
        for observer in &mut self.observers {
            observer.on_property_changed(object, name, old.as_ref(), &value);
        }
        Ok(())
    }

    // -- transactions -------------------------------------------------------

    /// Request a named transaction (created on the first change).
    pub fn open_transaction_named(&mut self, name: &str) {
        self.tx.open_named(name);
    }

    pub fn open_transaction(&mut self) {
        self.tx.open_named("");
    }

    /// If a pending transaction exists, make it active and return its name.
    pub fn begin_transaction_if_pending(&mut self) -> Option<String> {
        self.tx.begin()
    }

    /// Commit the active transaction; returns whether one was committed.
    pub fn commit_transaction(&mut self) -> bool {
        self.tx.commit()
    }

    pub fn has_undo(&self) -> bool {
        self.tx.has_undo()
    }

    pub fn has_redo(&self) -> bool {
        self.tx.has_redo()
    }

    /// Whether a transaction is pending or active.
    pub fn is_in_transaction(&self) -> bool {
        self.tx.has_pending() || self.tx.is_active()
    }

    // -- recompute flags -----------------------------------------------------

    pub fn enforce_recompute(&mut self, id: ObjectId) {
        if let Some(obj) = self.objects.get_mut(&id) {
            obj.must_execute = true;
        }
    }

    /// Take (and clear) the `must_execute` flag for one object.
    pub fn take_must_execute_one(&mut self, id: ObjectId) -> bool {
        match self.objects.get_mut(&id) {
            Some(obj) => std::mem::replace(&mut obj.must_execute, false),
            None => false,
        }
    }

    /// Take (and clear) all `must_execute` flags, in object order.
    pub fn take_must_execute(&mut self) -> Vec<ObjectId> {
        let flagged: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| o.must_execute)
            .map(|o| o.id)
            .collect();
        for id in &flagged {
            if let Some(obj) = self.objects.get_mut(id) {
                obj.must_execute = false;
            }
        }
        flagged
    }

    pub fn abort_transaction(&mut self) -> bool {
        match self.tx.abort() {
            Some(changes) => {
                for change in changes.into_iter().rev() {
                    self.revert(&change);
                }
                true
            }
            None => false,
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.tx.undo() {
            Some(changes) => {
                for change in changes.iter().rev() {
                    self.revert(change);
                }
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.tx.redo() {
            Some(changes) => {
                for change in changes.iter() {
                    self.apply(change);
                }
                true
            }
            None => false,
        }
    }

    fn apply(&mut self, change: &PropertyChange) {
        if let Some(obj) = self.objects.get_mut(&change.object) {
            obj.properties.set(change.name.clone(), change.new.clone());
        }
    }

    fn revert(&mut self, change: &PropertyChange) {
        if let Some(obj) = self.objects.get_mut(&change.object) {
            match &change.old {
                Some(old) => obj.properties.set(change.name.clone(), old.clone()),
                None => {
                    obj.properties.remove(&change.name);
                }
            }
        }
    }

    // -- expressions --------------------------------------------------------

    pub fn set_expression(&mut self, object: ObjectId, prop: &str, source: &str) -> Result<(), String> {
        expr::parse(source)?; // validate early
        let obj = self
            .objects
            .get_mut(&object)
            .ok_or_else(|| format!("no object {object}"))?;
        obj.expressions.insert(prop.to_string(), source.to_string());
        Ok(())
    }

    /// Evaluate every expression (in dependency order) and write the results.
    /// Returns the number of expressions evaluated.
    pub fn recompute(&mut self) -> Result<usize, String> {
        let order = self.recompute_order()?;
        let mut count = 0;
        for id in order {
            let expressions: Vec<(String, String)> = {
                let obj = &self.objects[&id];
                obj.expressions
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            };
            for (prop, source) in expressions {
                let parsed = expr::parse(&source)?;
                let value = parsed.eval(&|name| self.resolve(&id, name))?;
                if let Some(obj) = self.objects.get_mut(&id) {
                    obj.properties.set(prop.clone(), Property::Float(value));
                }
                count += 1;
            }
        }
        Ok(count)
    }

    fn resolve(&self, current: &ObjectId, name: &str) -> Option<f64> {
        if let Some((object_name, prop_name)) = name.split_once('.') {
            let obj = self.objects.values().find(|o| o.name == object_name)?;
            return numeric(obj, prop_name);
        }
        let obj = &self.objects[current];
        numeric(obj, name).or_else(|| self.objects.values().find_map(|o| numeric(o, name)))
    }

    // -- observers ----------------------------------------------------------

    pub fn add_observer(&mut self, observer: Box<dyn Observer>) {
        self.observers.push(observer);
    }

    // -- persistence --------------------------------------------------------

    /// Serialize this document's objects (with `name`) to JSON.
    pub fn to_saved(&self, name: &str) -> SavedDocument {
        let objects = self
            .object_ids()
            .into_iter()
            .filter_map(|id| {
                self.object(id).map(|o| SavedObject {
                    name: o.name.clone(),
                    label: o.label.clone(),
                    type_id: o.type_id.clone(),
                    properties: o
                        .properties
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    expressions: o.expressions.clone(),
                    extensions: o.extensions.iter().cloned().collect(),
                })
            })
            .collect();
        SavedDocument { name: name.to_string(), objects }
    }

    /// Build a fresh document from a `SavedDocument`.
    pub fn from_saved(saved: &SavedDocument) -> Document {
        let mut doc = Document::new();
        for obj in &saved.objects {
            let id = doc.add_object(&obj.name, &obj.type_id);
            doc.set_label(id, &obj.label);
            if let Some(o) = doc.objects.get_mut(&id) {
                o.properties.clear();
                for (k, v) in &obj.properties {
                    o.properties.set(k.clone(), v.clone());
                }
                o.expressions = obj.expressions.clone();
                o.extensions = obj.extensions.iter().cloned().collect();
            }
        }
        doc
    }

    pub fn save_to_file(&self, name: &str, path: &str) -> Result<(), String> {
        let json = serde_json::to_string(&self.to_saved(name)).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_from_file(path: &str) -> Result<SavedDocument, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// Serialize the whole document to bytes (`dumpContent`).
    pub fn dump(&self, name: &str) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.to_saved(name)).map_err(|e| e.to_string())
    }

    /// Replace this document's content from a `dump` payload (`restoreContent`).
    pub fn restore_from_bytes(&mut self, data: &[u8]) -> Result<(), String> {
        let saved: SavedDocument = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        *self = Document::from_saved(&saved);
        Ok(())
    }

    fn saved_object(&self, id: ObjectId) -> Result<SavedObject, String> {
        let o = self.object(id).ok_or_else(|| format!("no object {id}"))?;
        Ok(SavedObject {
            name: o.name.clone(),
            label: o.label.clone(),
            type_id: o.type_id.clone(),
            properties: o
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            expressions: o.expressions.clone(),
            extensions: o.extensions.iter().cloned().collect(),
        })
    }

    /// Serialize one object to bytes (`dumpContent`).
    pub fn dump_object(&self, id: ObjectId) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.saved_object(id)?).map_err(|e| e.to_string())
    }

    /// Restore one object's content from bytes (`restoreContent`).
    pub fn restore_object(&mut self, id: ObjectId, data: &[u8]) -> Result<(), String> {
        let saved: SavedObject = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        let o = self.objects.get_mut(&id).ok_or_else(|| format!("no object {id}"))?;
        o.label = saved.label;
        o.properties.clear();
        for (k, v) in saved.properties {
            o.properties.set(k, v);
        }
        o.expressions = saved.expressions;
        o.extensions = saved.extensions.into_iter().collect();
        Ok(())
    }

    /// Serialize a single property to bytes (`dumpPropertyContent`).
    pub fn dump_property(&self, id: ObjectId, name: &str) -> Result<Vec<u8>, String> {
        let p = self
            .object(id)
            .and_then(|o| o.properties.get(name))
            .ok_or_else(|| format!("no property '{name}'"))?;
        serde_json::to_vec(p).map_err(|e| e.to_string())
    }

    /// Restore a single property from bytes (`restorePropertyContent`).
    pub fn restore_property(&mut self, id: ObjectId, name: &str, data: &[u8]) -> Result<(), String> {
        let p: Property = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        self.set_property(id, name, p)
    }
}

/// A document as persisted to disk (JSON).
#[derive(Serialize, Deserialize)]
pub struct SavedDocument {
    pub name: String,
    pub objects: Vec<SavedObject>,
}

#[derive(Serialize, Deserialize)]
pub struct SavedObject {
    pub name: String,
    pub label: String,
    pub type_id: String,
    pub properties: BTreeMap<String, Property>,
    pub expressions: BTreeMap<String, String>,
    pub extensions: BTreeSet<String>,
}

/// Extension inheritance: true if `ext` is `base` or derives from it.
///
/// Covers the Python-extension → C++-extension pairs relevant to the POC
/// (the `*Python` extension derives from its C++ counterpart).
fn extension_is_or_derives(ext: &str, base: &str) -> bool {
    if ext == base {
        return true;
    }
    matches!(
        (ext, base),
        ("App::GroupExtensionPython", "App::GroupExtension")
            | ("Gui::ViewProviderGroupExtensionPython", "Gui::ViewProviderGroupExtension")
    )
}

fn numeric(obj: &DocumentObject, name: &str) -> Option<f64> {
    match obj.properties.get(name)? {
        Property::Float(f) => Some(*f),
        Property::Quantity(q) => Some(q.value_mm()),
        _ => None,
    }
}

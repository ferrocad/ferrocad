//! The document object model: objects, a dependency graph, observers, and
//! expression-driven recompute.
//!
//! Transactional editing (undo/redo, the reversible change set, FreeCAD's
//! transaction metadata) lives in the [`transaction`] submodule, which adds a
//! second `impl Document`; this module owns the object store the engine mutates.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::NodeIndex;
use petgraph::stable_graph::StableGraph;
use serde::{Deserialize, Serialize};

use crate::expr;
use crate::observer::Observer;
use crate::property::{Property, PropertyContainer};

mod transaction;

use transaction::{Change, PropertyChange, TransactionManager};

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
    /// Set when the object's last recompute failed (reported as `Invalid`).
    pub invalid: bool,
    /// Opaque base64-pickled Python state (instance `__dict__` + `Proxy`).
    pub python_state: Option<String>,
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
    /// The object made active by the last `addObject` (FreeCAD `ActiveObject`).
    active_object: Option<ObjectId>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_object(&mut self, name: &str, type_id: &str) -> ObjectId {
        self.add_object_with(name, type_id, false)
    }

    /// Add an object, applying FreeCAD's name/label rules.
    ///
    /// The internal `name` is always sanitized and made unique. The `label` is the
    /// requested name when `duplicate_labels` is set, otherwise a unique label.
    pub fn add_object_with(
        &mut self,
        name: &str,
        type_id: &str,
        duplicate_labels: bool,
    ) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;

        let (name, label) = if name.is_empty() {
            // FreeCAD-style default naming: last segment of the type id + counter.
            let base = type_id.rsplit("::").next().unwrap_or("Object");
            let base = if base.is_empty() { "Object" } else { base };
            let unique = self.unique_name(base);
            (unique.clone(), unique)
        } else {
            let base = sanitize_name(name);
            let unique = self.unique_name(&base);
            let label = if duplicate_labels {
                base
            } else {
                self.unique_label(&base)
            };
            (unique, label)
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
                label,
                name,
                type_id: type_id.to_string(),
                properties,
                expressions: BTreeMap::new(),
                extensions: BTreeSet::new(),
                must_execute: false,
                invalid: false,
                python_state: None,
            },
        );
        self.active_object = Some(id);
        {
            let saved = SavedObject::from_object(&self.objects[&id]);
            self.record_change(Change::AddObject { id, saved });
        }
        for observer in &mut self.observers {
            observer.on_object_added(id);
        }
        id
    }

    /// Make `base` a unique object *name* (base, base001, base002, …).
    pub fn unique_name(&self, base: &str) -> String {
        if self.get_by_name(base).is_none() {
            return base.to_string();
        }
        let stem = strip_trailing_digits(base);
        let mut i = 1;
        loop {
            let candidate = format!("{stem}{i:03}");
            if self.get_by_name(&candidate).is_none() {
                return candidate;
            }
            i += 1;
        }
    }

    /// Make `base` a unique object *label* (base, base001, base002, …).
    pub fn unique_label(&self, base: &str) -> String {
        if !self.objects.values().any(|o| o.label == base) {
            return base.to_string();
        }
        let stem = strip_trailing_digits(base);
        let mut i = 1;
        loop {
            let candidate = format!("{stem}{i:03}");
            if !self.objects.values().any(|o| o.label == candidate) {
                return candidate;
            }
            i += 1;
        }
    }

    pub fn object(&self, id: ObjectId) -> Option<&DocumentObject> {
        self.objects.get(&id)
    }

    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut DocumentObject> {
        self.objects.get_mut(&id)
    }

    pub fn object_ids(&self) -> Vec<ObjectId> {
        self.objects.keys().copied().collect()
    }

    pub fn get_by_name(&self, name: &str) -> Option<ObjectId> {
        self.objects.values().find(|o| o.name == name).map(|o| o.id)
    }

    /// Remove an object (and its graph node/edges), recording the change so it
    /// can be undone. Returns false if the object is absent.
    pub fn remove_object(&mut self, id: ObjectId) -> bool {
        let saved = match self.objects.get(&id) {
            Some(o) => SavedObject::from_object(o),
            None => return false,
        };
        let name = saved.name.clone();
        // Group link lists that reference the object, before it is stripped.
        let group_before: Vec<(ObjectId, Vec<String>)> = self
            .objects
            .values()
            .filter(|o| o.id != id)
            .filter_map(|o| match o.properties.get("Group") {
                Some(Property::LinkList(links)) if links.contains(&name) => {
                    Some((o.id, links.clone()))
                }
                _ => None,
            })
            .collect();

        self.record_change(Change::RemoveObject { id, saved });
        if !self.remove_object_raw(id, true) {
            return false;
        }
        // Record the link-list edits so undo restores group membership.
        for (group, old) in group_before {
            let new = match self.objects.get(&group).and_then(|o| o.properties.get("Group")) {
                Some(Property::LinkList(links)) => links.clone(),
                _ => continue,
            };
            self.record_change(Change::Property(PropertyChange {
                object: group,
                name: "Group".to_string(),
                old: Some(Property::LinkList(old)),
                new: Property::LinkList(new),
            }));
        }
        true
    }

    /// Remove an object without recording anything. `strip_groups` also drops it
    /// from every group's `Group` link list (used by the public, recording path).
    fn remove_object_raw(&mut self, id: ObjectId, strip_groups: bool) -> bool {
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
        if strip_groups {
            for other in self.objects.values_mut() {
                if let Some(Property::LinkList(links)) = other.properties.get_mut("Group") {
                    links.retain(|n| n != &name);
                }
            }
        }
        if self.active_object == Some(id) {
            self.active_object = None;
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
        let status = self
            .objects
            .get(&object)
            .and_then(|o| o.properties.status(name))
            .unwrap_or(crate::prop_status::NONE);
        {
            let obj = self.objects.get_mut(&object).unwrap();
            obj.properties.set(name.to_string(), value.clone());
            // Assigning a property touches the object, unless it is an output /
            // no-recompute property (FreeCAD `Property::touch`).
            if status & crate::prop_status::NO_TOUCH == 0 {
                obj.must_execute = true;
            }
        }

        self.record_change(Change::Property(PropertyChange {
            object,
            name: name.to_string(),
            old: old.clone(),
            new: value.clone(),
        }));
        for observer in &mut self.observers {
            observer.on_property_changed(object, name, old.as_ref(), &value);
        }
        Ok(())
    }

    /// The object made active by the most recent `addObject` (or `None`).
    pub fn active_object(&self) -> Option<ObjectId> {
        self.active_object
    }

    /// Add a property with an explicit status mask. Unlike `set_property`, this
    /// does not touch the object (FreeCAD `addProperty`).
    pub fn add_property(
        &mut self,
        object: ObjectId,
        name: &str,
        value: Property,
        status: u32,
    ) -> Result<(), String> {
        let obj = self
            .objects
            .get_mut(&object)
            .ok_or_else(|| format!("no object {object}"))?;
        obj.properties.set_with_status(name.to_string(), value, status);
        Ok(())
    }

    /// The status mask of a property (`getPropertyStatus`), if it exists.
    pub fn property_status(&self, object: ObjectId, name: &str) -> Option<u32> {
        self.objects.get(&object).and_then(|o| o.properties.status(name))
    }

    /// Replace a property's status mask (`setPropertyStatus`).
    pub fn set_property_status(&mut self, object: ObjectId, name: &str, status: u32) -> bool {
        match self.objects.get_mut(&object) {
            Some(obj) => obj.properties.set_status(name, status).is_some(),
            None => false,
        }
    }

    /// Mark an object as unchanged (FreeCAD `purgeTouched`).
    pub fn purge_touched(&mut self, object: ObjectId) {
        if let Some(obj) = self.objects.get_mut(&object) {
            obj.must_execute = false;
        }
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

    // -- expressions --------------------------------------------------------

    pub fn set_expression(&mut self, object: ObjectId, prop: &str, source: &str) -> Result<(), String> {
        let parsed = expr::parse(source)?; // validate early
        let target = normalize_path(prop);
        let mut new_deps = BTreeSet::new();
        collect_self_deps(&parsed, &mut new_deps);

        // Build this object's self-dependency graph (existing + new) and reject cycles.
        let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        {
            let obj = self
                .objects
                .get(&object)
                .ok_or_else(|| format!("no object {object}"))?;
            for (p, src) in &obj.expressions {
                let mut deps = BTreeSet::new();
                if let Ok(e) = expr::parse(src) {
                    collect_self_deps(&e, &mut deps);
                }
                graph.insert(normalize_path(p), deps);
            }
        }
        graph.insert(target.clone(), new_deps);
        if graph_reaches(&graph, &target, &target) {
            return Err(format!("cyclic dependency detected for '{prop}'"));
        }

        let old_source = self
            .objects
            .get(&object)
            .and_then(|o| o.expressions.get(prop).cloned());
        let obj = self.objects.get_mut(&object).unwrap();
        obj.expressions.insert(prop.to_string(), source.to_string());
        self.record_change(Change::AddExpression {
            object,
            prop: prop.to_string(),
            old: old_source,
            new: source.to_string(),
        });
        Ok(())
    }

    pub fn remove_expression(&mut self, object: ObjectId, prop: &str) -> bool {
        let old = match self.objects.get_mut(&object) {
            Some(obj) => obj.expressions.remove(prop),
            None => return false,
        };
        if let Some(old) = &old {
            self.record_change(Change::RemoveExpression {
                object,
                prop: prop.to_string(),
                old: old.clone(),
            });
        }
        old.is_some()
    }

    /// The object's expressions as `(property, source)` pairs.
    pub fn expressions(&self, object: ObjectId) -> Vec<(String, String)> {
        self.object(object)
            .map(|o| o.expressions.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Evaluate an expression source in the context of `object`.
    pub fn eval_expression(&self, object: ObjectId, source: &str) -> Result<f64, String> {
        let parsed = expr::parse(source)?;
        parsed.eval(&|name| self.resolve(&object, name))
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
                let parsed = match expr::parse(&source) {
                    Ok(p) => p,
                    Err(e) => {
                        self.mark_invalid(id);
                        return Err(e);
                    }
                };
                let value = match parsed.eval(&|name| self.resolve(&id, name)) {
                    Ok(v) => v,
                    Err(e) => {
                        self.mark_invalid(id);
                        return Err(e);
                    }
                };
                if let Some(obj) = self.objects.get_mut(&id) {
                    obj.properties.set(prop.clone(), Property::Float(value));
                    obj.invalid = false;
                }
                count += 1;
            }
        }
        Ok(count)
    }

    fn mark_invalid(&mut self, id: ObjectId) {
        if let Some(obj) = self.objects.get_mut(&id) {
            obj.invalid = true;
        }
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
            .filter_map(|id| self.object(id).map(SavedObject::from_object))
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
                    let status = obj.property_status.get(k).copied().unwrap_or(crate::prop_status::NONE);
                    o.properties.set_with_status(k.clone(), v.clone(), status);
                }
                o.expressions = obj.expressions.clone();
                o.extensions = obj.extensions.iter().cloned().collect();
                o.python_state = obj.python_state.clone();
                o.must_execute = false;
                o.invalid = false;
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
        Ok(SavedObject::from_object(o))
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
            let status = saved.property_status.get(&k).copied().unwrap_or(crate::prop_status::NONE);
            o.properties.set_with_status(k, v, status);
        }
        o.expressions = saved.expressions;
        o.extensions = saved.extensions.into_iter().collect();
        o.python_state = saved.python_state;
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedDocument {
    pub name: String,
    pub objects: Vec<SavedObject>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedObject {
    pub name: String,
    pub label: String,
    pub type_id: String,
    pub properties: BTreeMap<String, Property>,
    /// Property name → `PropertyType` status mask.
    #[serde(default)]
    pub property_status: BTreeMap<String, u32>,
    pub expressions: BTreeMap<String, String>,
    pub extensions: BTreeSet<String>,
    #[serde(default)]
    pub python_state: Option<String>,
}

impl SavedObject {
    /// Build the persisted form of an object, dropping transient / no-persist
    /// properties (`Prop_Transient`, `Prop_NoPersist`).
    pub(crate) fn from_object(o: &DocumentObject) -> SavedObject {
        let mut properties = BTreeMap::new();
        let mut property_status = BTreeMap::new();
        for (k, v) in o.properties.iter() {
            let status = o.properties.status(k).unwrap_or(crate::prop_status::NONE);
            if status & crate::prop_status::NOT_PERSISTED != 0 {
                continue;
            }
            properties.insert(k.clone(), v.clone());
            property_status.insert(k.clone(), status);
        }
        SavedObject {
            name: o.name.clone(),
            label: o.label.clone(),
            type_id: o.type_id.clone(),
            properties,
            property_status,
            expressions: o.expressions.clone(),
            extensions: o.extensions.iter().cloned().collect(),
            python_state: o.python_state.clone(),
        }
    }
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

/// Strip a leading '.' from a self-relative expression path.
/// FreeCAD `Base::Tools::getIdentifier`: keep identifier characters, replace every
/// other code point with `_`, and prepend `_` if the first character is not a
/// valid identifier start (e.g. a digit).
pub fn sanitize_name(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }
    let is_subsequent = |c: char| c == '_' || c.is_alphanumeric();
    let is_first = |c: char| c == '_' || c.is_alphabetic();
    let mut out = String::with_capacity(name.len() + 1);
    let mut chars = name.chars();
    if let Some(first) = chars.next() {
        let first_ok = is_first(first);
        if !first_ok {
            out.push('_');
        }
        if first_ok || is_subsequent(first) {
            out.push(first);
        }
    }
    for c in chars {
        if is_subsequent(c) {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

/// Strip a trailing run of ASCII digits so `Label001` and `Label` share a stem;
/// an all-digit name is returned unchanged.
fn strip_trailing_digits(name: &str) -> &str {
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() {
        name
    } else {
        trimmed
    }
}

fn normalize_path(path: &str) -> String {
    path.trim_start_matches('.').to_string()
}

/// Collect the self-relative paths an expression depends on (leading '.', or a
/// bare name without a `.`).
fn collect_self_deps(e: &expr::Expr, out: &mut BTreeSet<String>) {
    match e {
        expr::Expr::Number(_) => {}
        expr::Expr::Var(name) => {
            if name.starts_with('.') || !name.contains('.') {
                out.insert(normalize_path(name));
            }
        }
        expr::Expr::UnaryNeg(x) => collect_self_deps(x, out),
        expr::Expr::Binary(l, _, r) => {
            collect_self_deps(l, out);
            collect_self_deps(r, out);
        }
    }
}

/// Whether `goal` is reachable from `start` in the property dependency graph.
fn graph_reaches(
    graph: &BTreeMap<String, BTreeSet<String>>,
    start: &str,
    goal: &str,
) -> bool {
    let mut stack: Vec<String> = graph.get(start).cloned().unwrap_or_default().into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == goal {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(deps) = graph.get(&node) {
            stack.extend(deps.iter().cloned());
        }
    }
    false
}

fn numeric(obj: &DocumentObject, name: &str) -> Option<f64> {
    match obj.properties.get(name)? {
        Property::Float(f) => Some(*f),
        Property::Quantity(q) => Some(q.value_mm()),
        _ => None,
    }
}

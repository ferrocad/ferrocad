//! The document object model: objects, a dependency graph, observers, and
//! expression-driven recompute.
//!
//! Transactional editing (undo/redo, the reversible change set, FreeCAD's
//! transaction metadata) lives in the [`transaction`] submodule, which adds a
//! second `impl Document`; this module owns the object store the engine mutates.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::NodeIndex;
use petgraph::stable_graph::StableGraph;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::{Deserialize, Serialize};

use crate::expr;
use crate::geometry::{Rotation, Vector3};
use crate::observer::Observer;
use crate::property::{prop_status, Property, PropertyContainer};

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
    /// FreeCAD `ObjectStatus::Enforce`: the object itself must execute on the
    /// next recompute (set by `enforceRecompute`, a plain `touch()`, or a
    /// non-`NoRecompute` property change).
    pub must_execute: bool,
    /// FreeCAD `ObjectStatus::Touch`: the object's output may be stale, so its
    /// dependents are enforced on the next recompute. Cleared by `purgeTouched`.
    pub touched: bool,
    /// Set when the object's last recompute failed (reported as `Invalid`).
    pub invalid: bool,
    /// Monotonic counter per property, bumped on every `set_property`. Used to
    /// detect that a geometry handle (e.g. `obj.Placement.Base`) still refers to
    /// the current property value: reassigning the property invalidates old
    /// handles even when the new value compares equal.
    pub property_versions: BTreeMap<String, u64>,
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

        // Initialize default properties: a module-registered type first, otherwise
        // the built-in core types (FeatureTest, …).
        let mut properties = PropertyContainer::new();
        if let Some(behavior) = crate::object_registry::get(type_id) {
            for (prop_name, prop, status) in behavior.default_properties() {
                let (group, doc) = behavior.property_meta(&prop_name).unwrap_or_default();
                properties.set_meta(prop_name.clone(), &group, &doc);
                properties.set_with_status(prop_name, prop, status);
            }
        } else {
            for (prop_name, prop, status) in crate::typeregistry::default_properties(type_id) {
                properties.set_with_status(prop_name.to_string(), prop, status);
                let (group, doc) = crate::typeregistry::property_meta(type_id, prop_name);
                properties.set_meta(prop_name.to_string(), group, doc);
            }
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
                touched: false,
                invalid: false,
                property_versions: BTreeMap::new(),
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

    /// `(from, to)` pairs meaning `to` depends on `from`, so `from` recomputes
    /// first. Derived from link properties and expressions, plus the explicit
    /// edges registered through [`add_dependency`](Self::add_dependency).
    ///
    /// The `Group` property is skipped: group membership does not order
    /// recompute (FreeCAD treats it specially).
    fn dependency_edges(&self) -> Vec<(ObjectId, ObjectId)> {
        let by_name: BTreeMap<&str, ObjectId> =
            self.objects.values().map(|o| (o.name.as_str(), o.id)).collect();
        let mut edges = Vec::new();
        for obj in self.objects.values() {
            {
                let mut add = |name: &str| {
                    if let Some(&from) = by_name.get(name) {
                        if from != obj.id {
                            edges.push((from, obj.id));
                        }
                    }
                };
                for (prop, value) in obj.properties.iter() {
                    if prop == "Group" {
                        continue;
                    }
                    match value {
                        Property::Link(n) => add(n),
                        Property::LinkList(ns) => ns.iter().for_each(|n| add(n)),
                        Property::LinkSub(n, _) => add(n),
                        _ => {}
                    }
                }
                for source in obj.expressions.values() {
                    if let Ok(parsed) = expr::parse(source) {
                        let mut refs = BTreeSet::new();
                        collect_object_refs(&parsed, &mut refs);
                        for name in refs {
                            add(&name);
                        }
                    }
                }
            }
        }
        for edge in self.graph.edge_references() {
            edges.push((self.graph[edge.source()], self.graph[edge.target()]));
        }
        edges
    }

    /// The objects that directly depend on `id`.
    pub fn dependents(&self, id: ObjectId) -> Vec<ObjectId> {
        let mut out: Vec<ObjectId> = self
            .dependency_edges()
            .into_iter()
            .filter(|(from, _)| *from == id)
            .map(|(_, to)| to)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Objects that no other object depends on (FreeCAD `RootObjects`).
    pub fn root_objects(&self) -> Vec<ObjectId> {
        let referenced: BTreeSet<ObjectId> = self
            .dependency_edges()
            .into_iter()
            .map(|(from, _)| from)
            .collect();
        self.object_ids()
            .into_iter()
            .filter(|id| !referenced.contains(id))
            .collect()
    }

    /// Objects in dependents-first topological order
    /// (FreeCAD `TopologicalSortedObjects`).
    pub fn topological_sorted_objects(&self) -> Vec<ObjectId> {
        match self.recompute_order() {
            Ok(mut order) => {
                order.reverse();
                order
            }
            Err(_) => self.object_ids(),
        }
    }

    /// Object ids in dependency-first order. `Err` if the graph has a cycle.
    pub fn recompute_order(&self) -> Result<Vec<ObjectId>, String> {
        let mut graph: petgraph::graph::DiGraph<ObjectId, ()> = petgraph::graph::DiGraph::new();
        let mut nodes: BTreeMap<ObjectId, NodeIndex> = BTreeMap::new();
        for obj in self.objects.values() {
            nodes.insert(obj.id, graph.add_node(obj.id));
        }
        for (from, to) in self.dependency_edges() {
            if let (Some(&a), Some(&b)) = (nodes.get(&from), nodes.get(&to)) {
                graph.add_edge(a, b, ());
            }
        }
        let order = petgraph::algo::toposort(&graph, None)
            .map_err(|_| "dependency cycle".to_string())?;
        Ok(order.into_iter().map(|n| graph[n]).collect())
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
            *obj.property_versions.entry(name.to_string()).or_insert(0) += 1;
            // Assigning a property touches the object unless it is an output
            // property; it also enforces execution unless it is `NoRecompute`
            // (FreeCAD `DocumentObject::onChanged`).
            if status & crate::prop_status::OUTPUT == 0 {
                obj.touched = true;
                if status & crate::prop_status::NORECOMPUTE == 0 {
                    obj.must_execute = true;
                }
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

    /// The monotonic version of a property (see `DocumentObject::property_versions`).
    /// A geometry handle records the version it was created with and refuses to
    /// write back once the property has been reassigned.
    pub fn property_version(&self, object: ObjectId, name: &str) -> u64 {
        self.objects
            .get(&object)
            .and_then(|o| o.property_versions.get(name).copied())
            .unwrap_or(0)
    }

    /// Add a property with an explicit status mask. Unlike `set_property`, this
    /// does not touch the object (FreeCAD `addProperty`).
    pub fn add_property(
        &mut self,
        object: ObjectId,
        name: &str,
        value: Property,
        status: u32,
        group: &str,
        doc: &str,
    ) -> Result<(), String> {
        let obj = self
            .objects
            .get_mut(&object)
            .ok_or_else(|| format!("no object {object}"))?;
        // Mark runtime-added properties as dynamic so that transient ones are
        // still persisted (`PropertyContainer::Save`).
        obj.properties
            .set_with_status(name.to_string(), value, status | prop_status::DYNAMIC);
        obj.properties.set_meta(name.to_string(), group, doc);
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
            obj.touched = false;
        }
    }

    // -- recompute flags -----------------------------------------------------

    /// FreeCAD `DocumentObject::touch(noRecompute)`: mark the object touched;
    /// also enforce its own execution unless `no_recompute` is set.
    pub fn touch(&mut self, id: ObjectId, no_recompute: bool) {
        if let Some(obj) = self.objects.get_mut(&id) {
            obj.touched = true;
            if !no_recompute {
                obj.must_execute = true;
            }
        }
    }

    /// FreeCAD `DocumentObject::enforceRecompute()`: touch and force execution.
    pub fn enforce_recompute(&mut self, id: ObjectId) {
        self.touch(id, false);
    }

    /// Whether the object is marked touched (`ObjectStatus::Touch`).
    pub fn is_touched(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.touched)
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
        // An expression makes the object dirty (FreeCAD `ExpressionEngine`).
        self.enforce_recompute(object);
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
        self.enforce_recompute(object);
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

    /// Recompute the document in dependency order (FreeCAD `Document::recompute`).
    ///
    /// Every expression is evaluated and every object marked to execute runs its
    /// type-specific behaviour. An object whose state changed then enforces its
    /// dependents, so a change propagates along the dependency graph. Returns the
    /// ids of the objects that executed (`objectCount` in FreeCAD).
    pub fn recompute(&mut self) -> Result<Vec<ObjectId>, String> {
        let order = self.recompute_order()?;
        let mut dependents: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
        for (from, to) in self.dependency_edges() {
            dependents.entry(from).or_default().push(to);
        }
        let mut executed = Vec::new();
        for id in order {
            self.eval_expressions(id)?;

            let do_recompute = self.objects.get(&id).is_some_and(|o| o.must_execute);
            if do_recompute {
                self.execute_object(id);
                executed.push(id);
            }

            let propagate = self
                .objects
                .get(&id)
                .is_some_and(|o| o.touched || do_recompute);
            if propagate {
                if let Some(obj) = self.objects.get_mut(&id) {
                    obj.touched = false;
                    obj.must_execute = false;
                    obj.invalid = false;
                }
                if let Some(deps) = dependents.get(&id).cloned() {
                    for dep in deps {
                        self.enforce_recompute(dep);
                    }
                }
            }
        }
        Ok(executed)
    }

    /// Evaluate one object's expressions and write the results back in the
    /// property's own kind (so a `PropertyLength` keeps its unit).
    fn eval_expressions(&mut self, id: ObjectId) -> Result<(), String> {
        let expressions: Vec<(String, String)> = match self.objects.get(&id) {
            Some(obj) => obj
                .expressions
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            None => return Ok(()),
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
            // Write the result back through the (possibly nested) target path.
            let assigned = match self.objects.get_mut(&id) {
                Some(obj) => assign_path(obj, &prop, value),
                None => Ok(()),
            };
            if let Err(e) = assigned {
                self.mark_invalid(id);
                return Err(e);
            }
            if let Some(obj) = self.objects.get_mut(&id) {
                obj.invalid = false;
            }
        }
        Ok(())
    }

    /// Run an object's type-specific `execute()` behaviour.
    ///
    /// `App::FeatureTest*` mirrors the C++ fixture: it bumps `ExecCount` and
    /// sets `ExecResult`. The writes are raw so execution does not re-touch the
    /// object (which would schedule an endless recompute).
    fn execute_object(&mut self, id: ObjectId) {
        let type_id = match self.objects.get(&id) {
            Some(obj) => obj.type_id.clone(),
            None => return,
        };
        // A module-registered behaviour runs first; the built-in FeatureTest fixture
        // behaviour is the fallback for core types.
        if let Some(behavior) = crate::object_registry::get(&type_id) {
            behavior.execute(self, id);
            return;
        }
        if !type_id.starts_with("App::FeatureTest") {
            return;
        }
        let count = self
            .objects
            .get(&id)
            .and_then(|o| o.properties.get("ExecCount"))
            .and_then(|p| match p {
                Property::Integer(n) => Some(*n),
                _ => None,
            })
            .unwrap_or(0);
        if let Some(obj) = self.objects.get_mut(&id) {
            if obj.properties.get("ExecCount").is_some() {
                obj.properties
                    .set("ExecCount".to_string(), Property::Integer(count + 1));
            }
            if obj.properties.get("ExecResult").is_some() {
                obj.properties
                    .set("ExecResult".to_string(), Property::String("Exec".to_string()));
            }
        }
    }

    /// Set a property without marking the object dirty.
    ///
    /// Intended for `execute` implementations (registered
    /// [`crate::object_registry::ObjectType`]s): recompute writes its outputs here so
    /// it does not re-touch the object and schedule another recompute. A normal edit
    /// should go through [`Document::set_property`].
    pub fn set_property_raw(&mut self, id: ObjectId, name: &str, value: Property) -> bool {
        match self.objects.get_mut(&id) {
            Some(obj) => {
                obj.properties.set(name.to_string(), value);
                true
            }
            None => false,
        }
    }

    fn mark_invalid(&mut self, id: ObjectId) {
        if let Some(obj) = self.objects.get_mut(&id) {
            obj.invalid = true;
        }
    }

    fn resolve(&self, current: &ObjectId, name: &str) -> Option<f64> {
        // A leading '.' is a self-relative path (`.Placement.Base.x`).
        if let Some(rest) = name.strip_prefix('.') {
            return self.objects.get(current).and_then(|o| resolve_path(o, rest));
        }
        // Otherwise a leading `Name.` may qualify another object; fall back to a
        // self-relative path when no object matches (e.g. `Placement.Base.x`).
        if let Some((head, tail)) = name.split_once('.') {
            if let Some(obj) = self.objects.values().find(|o| o.name == head) {
                return resolve_path(obj, tail);
            }
        }
        self.objects
            .get(current)
            .and_then(|o| resolve_path(o, name))
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
                // Apply the persisted values *over* the constructor defaults
                // created by `add_object`. We must not clear first: static
                // transient properties are not written to the file, and after a
                // restore they should fall back to their constructor default
                // (upstream `PropertyContainer::Restore`).
                for (k, v) in &obj.properties {
                    let status = obj.property_status.get(k).copied().unwrap_or(crate::prop_status::NONE);
                    o.properties.set_with_status(k.clone(), v.clone(), status);
                }
                // Re-apply statuses recorded without a value (transient
                // placeholders for dynamically added properties).
                for (k, status) in &obj.property_status {
                    if !obj.properties.contains_key(k) {
                        o.properties.set_status(k, *status);
                    }
                }
                o.expressions = obj.expressions.clone();
                o.extensions = obj.extensions.iter().cloned().collect();
                o.python_state = obj.python_state.clone();
                o.must_execute = false;
                o.touched = false;
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
        // As in `from_saved`, keep the existing property set (constructor
        // defaults included) and overlay the restored values.
        for (k, v) in &saved.properties {
            let status = saved.property_status.get(k).copied().unwrap_or(crate::prop_status::NONE);
            o.properties.set_with_status(k.clone(), v.clone(), status);
        }
        for (k, status) in &saved.property_status {
            if !saved.properties.contains_key(k) {
                o.properties.set_status(k, *status);
            }
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
            let status = o.properties.raw_status(k).unwrap_or(crate::prop_status::NONE);
            if status & crate::prop_status::NOT_PERSISTED != 0 {
                continue;
            }
            // Record the status even for static transient properties, but skip
            // the value: upstream stores a valueless `_Property` placeholder so
            // the property reverts to its constructor default on restore.
            // Dynamic (runtime-added) transient properties keep their value.
            property_status.insert(k.clone(), status);
            if status & crate::prop_status::TRANSIENT != 0
                && status & crate::prop_status::DYNAMIC == 0
            {
                continue;
            }
            properties.insert(k.clone(), v.clone());
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

/// Collect the object names an expression references via `Object.Property`
/// paths (the part before the first `.`, excluding self-relative `.<prop>`).
fn collect_object_refs(e: &expr::Expr, out: &mut BTreeSet<String>) {
    match e {
        expr::Expr::Number(_) => {}
        expr::Expr::Var(name) => {
            if name.starts_with('.') {
                return;
            }
            if let Some((object, _)) = name.split_once('.') {
                if !object.is_empty() {
                    out.insert(object.to_string());
                }
            }
        }
        expr::Expr::UnaryNeg(x) => collect_object_refs(x, out),
        expr::Expr::Binary(l, _, r) => {
            collect_object_refs(l, out);
            collect_object_refs(r, out);
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

/// A scalar property (or a constraint) as a number.
fn property_as_number(p: &Property) -> Option<f64> {
    match p {
        Property::Float(f) => Some(*f),
        Property::Integer(n) => Some(*n as f64),
        Property::Quantity(q) => Some(q.value_mm()),
        Property::IntegerConstraint { value, .. } => Some(*value as f64),
        Property::FloatConstraint { value, .. } => Some(*value),
        _ => None,
    }
}

/// Descend one segment of a nested property path (`Placement` → `Base` → `x`).
fn sub_value(value: &Property, seg: &str) -> Option<Property> {
    match value {
        Property::Placement(p) => match seg {
            "Base" | "Position" => Some(Property::Vector(p.base)),
            "Rotation" => Some(Property::Rotation(p.rotation)),
            _ => None,
        },
        Property::Rotation(r) => match seg {
            "Angle" => Some(Property::Float(r.angle())),
            "Axis" | "RawAxis" => Some(Property::Vector(r.axis())),
            _ => None,
        },
        Property::Vector(v) => match seg {
            "x" => Some(Property::Float(v.x)),
            "y" => Some(Property::Float(v.y)),
            "z" => Some(Property::Float(v.z)),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a (possibly nested) property path on one object to a number.
fn resolve_path(obj: &DocumentObject, path: &str) -> Option<f64> {
    let mut segs = path.split('.');
    let first = segs.next()?;
    let mut value = obj.properties.get(first)?.clone();
    for seg in segs {
        value = sub_value(&value, seg)?;
    }
    property_as_number(&value)
}

/// Assign `value` to a (possibly nested) property path on one object.
///
/// A single segment writes the whole property, keeping its kind (a
/// `PropertyLength` stays a length with its unit). Deeper paths descend into
/// `Placement`/`Rotation`/`Vector` sub-objects.
fn assign_path(obj: &mut DocumentObject, path: &str, value: f64) -> Result<(), String> {
    let segs: Vec<&str> = path.split('.').collect();
    let (root, rest) = segs.split_first().ok_or_else(|| "empty path".to_string())?;
    if rest.is_empty() {
        let result = match obj.properties.get(*root) {
            Some(Property::Quantity(existing)) => {
                Property::Quantity(crate::quantity::Quantity::new(value, existing.unit()))
            }
            Some(Property::Integer(_)) => Property::Integer(value as i64),
            Some(Property::IntegerConstraint { min, max, step, .. }) => {
                Property::IntegerConstraint { value: (value as i64).clamp(*min, *max), min: *min, max: *max, step: *step }
            }
            Some(Property::FloatConstraint { min, max, step, .. }) => {
                Property::FloatConstraint { value: value.clamp(*min, *max), min: *min, max: *max, step: *step }
            }
            _ => Property::Float(value),
        };
        obj.properties.set(root.to_string(), result);
        return Ok(());
    }
    let mut prop = obj
        .properties
        .get(*root)
        .cloned()
        .ok_or_else(|| format!("no property '{root}'"))?;
    assign_nested(&mut prop, rest, value)?;
    obj.properties.set(root.to_string(), prop);
    Ok(())
}

fn assign_nested(prop: &mut Property, segs: &[&str], value: f64) -> Result<(), String> {
    let (seg, rest) = segs.split_first().ok_or_else(|| "empty path".to_string())?;
    match prop {
        Property::Placement(p) => match *seg {
            "Base" | "Position" => assign_vector(&mut p.base, rest, value),
            "Rotation" => assign_rotation(&mut p.rotation, rest, value),
            other => Err(format!("no field '{other}' on Placement")),
        },
        Property::Rotation(r) => assign_rotation(r, segs, value),
        Property::Vector(v) => assign_vector(v, segs, value),
        Property::Quantity(q) if rest.is_empty() => {
            *q = crate::quantity::Quantity::new(value, q.unit());
            Ok(())
        }
        _ => Err(format!("cannot assign '{seg}'")),
    }
}

fn assign_vector(v: &mut Vector3, segs: &[&str], value: f64) -> Result<(), String> {
    let (seg, rest) = segs.split_first().ok_or_else(|| "empty path".to_string())?;
    if !rest.is_empty() {
        return Err("path too deep for Vector".to_string());
    }
    match *seg {
        "x" => v.x = value,
        "y" => v.y = value,
        "z" => v.z = value,
        other => return Err(format!("no component '{other}' on Vector")),
    }
    Ok(())
}

fn assign_rotation(r: &mut Rotation, segs: &[&str], value: f64) -> Result<(), String> {
    let (seg, rest) = segs.split_first().ok_or_else(|| "empty path".to_string())?;
    if !rest.is_empty() {
        return Err("path too deep for Rotation".to_string());
    }
    match *seg {
        // Angles are radians, matching the `Rotation.Angle` read-back.
        "Angle" => {
            r.set_angle(value);
            Ok(())
        }
        other => return Err(format!("no field '{other}' on Rotation")),
    }
}

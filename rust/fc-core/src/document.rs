//! A document object model with a dependency graph, transactions, observers,
//! and expression-driven recompute.

use std::collections::BTreeMap;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::expr;
use crate::observer::Observer;
use crate::property::{Property, PropertyContainer};
use crate::transaction::{PropertyChange, TransactionManager};

pub type ObjectId = usize;

#[derive(Debug)]
pub struct DocumentObject {
    pub id: ObjectId,
    pub name: String,
    pub type_id: String,
    pub properties: PropertyContainer,
    /// Property name → expression source (evaluated on recompute).
    pub expressions: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct Document {
    objects: BTreeMap<ObjectId, DocumentObject>,
    next_id: ObjectId,
    /// Edges point dependency → dependant (dependency recomputes first).
    graph: DiGraph<ObjectId, ()>,
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
        let node = self.graph.add_node(id);
        self.index.insert(id, node);
        self.objects.insert(
            id,
            DocumentObject {
                id,
                name: name.to_string(),
                type_id: type_id.to_string(),
                properties: PropertyContainer::new(),
                expressions: BTreeMap::new(),
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

        if self.tx.is_active() {
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

    pub fn open_transaction(&mut self) {
        self.tx.open();
    }

    pub fn commit_transaction(&mut self) {
        self.tx.commit();
    }

    pub fn abort_transaction(&mut self) {
        if let Some(changes) = self.tx.abort() {
            for change in changes.into_iter().rev() {
                self.revert(&change);
            }
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
}

fn numeric(obj: &DocumentObject, name: &str) -> Option<f64> {
    match obj.properties.get(name)? {
        Property::Float(f) => Some(*f),
        Property::Quantity(q) => Some(q.value_mm()),
        _ => None,
    }
}

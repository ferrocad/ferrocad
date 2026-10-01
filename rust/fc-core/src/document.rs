//! A document object model with a dependency graph, producing a topological
//! recompute order (dependencies before dependants).

use std::collections::BTreeMap;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::property::PropertyContainer;

pub type ObjectId = usize;

#[derive(Debug)]
pub struct DocumentObject {
    pub id: ObjectId,
    pub name: String,
    pub type_id: String,
    pub properties: PropertyContainer,
}

#[derive(Debug, Default)]
pub struct Document {
    objects: BTreeMap<ObjectId, DocumentObject>,
    next_id: ObjectId,
    /// Edges point dependency → dependant (dependency recomputes first).
    graph: DiGraph<ObjectId, ()>,
    index: BTreeMap<ObjectId, NodeIndex>,
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
            },
        );
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
        let nodes =
            petgraph::algo::toposort(&self.graph, None).map_err(|_| "dependency cycle".to_string())?;
        Ok(nodes.into_iter().map(|n| self.graph[n]).collect())
    }
}

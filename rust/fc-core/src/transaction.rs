//! Open/commit/abort transactions plus a simple undo/redo stack.

use crate::document::ObjectId;
use crate::property::Property;

#[derive(Debug, Clone)]
pub struct PropertyChange {
    pub object: ObjectId,
    pub name: String,
    pub old: Option<Property>,
    pub new: Property,
}

#[derive(Debug, Default)]
pub struct TransactionManager {
    active: Option<Vec<PropertyChange>>,
    undo: Vec<Vec<PropertyChange>>,
    redo: Vec<Vec<PropertyChange>>,
}

impl TransactionManager {
    pub fn open(&mut self) {
        if self.active.is_none() {
            self.active = Some(Vec::new());
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn record(&mut self, change: PropertyChange) {
        if let Some(active) = &mut self.active {
            active.push(change);
        }
    }

    pub fn commit(&mut self) {
        if let Some(active) = self.active.take() {
            self.undo.push(active);
            self.redo.clear();
        }
    }

    pub fn abort(&mut self) -> Option<Vec<PropertyChange>> {
        self.active.take()
    }

    pub fn undo(&mut self) -> Option<Vec<PropertyChange>> {
        let changes = self.undo.pop()?;
        self.redo.push(changes.clone());
        Some(changes)
    }

    pub fn redo(&mut self) -> Option<Vec<PropertyChange>> {
        let changes = self.redo.pop()?;
        self.undo.push(changes.clone());
        Some(changes)
    }
}

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
    /// A transaction requested by `open`, not yet made active by a change.
    pending: Option<String>,
    active: Option<Vec<PropertyChange>>,
    undo: Vec<Vec<PropertyChange>>,
    redo: Vec<Vec<PropertyChange>>,
}

impl TransactionManager {
    /// Request a (named) transaction. It only becomes `active` on the first
    /// recorded change, mirroring FreeCAD's "pending transaction" behaviour.
    pub fn open_named(&mut self, name: &str) {
        if self.active.is_none() {
            self.pending = Some(name.to_string());
        }
    }

    /// Make the pending transaction active and return its name (once).
    pub fn begin(&mut self) -> Option<String> {
        if self.active.is_none() {
            if let Some(name) = self.pending.take() {
                self.active = Some(Vec::new());
                return Some(name);
            }
        }
        None
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn has_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn has_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn record(&mut self, change: PropertyChange) {
        self.begin();
        if let Some(active) = &mut self.active {
            active.push(change);
        }
    }

    /// Commit the active transaction; returns whether one was committed.
    pub fn commit(&mut self) -> bool {
        self.pending = None;
        match self.active.take() {
            Some(active) => {
                self.undo.push(active);
                self.redo.clear();
                true
            }
            None => false,
        }
    }

    pub fn abort(&mut self) -> Option<Vec<PropertyChange>> {
        self.pending = None;
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

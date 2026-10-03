//! Open/commit/abort transactions plus a named undo/redo stack.
//!
//! A transaction collects a list of reversible [`Change`]s. Opening a
//! transaction only *books* a name; the transaction becomes active — and the
//! redo stack is dropped — on the first recorded change, mirroring FreeCAD's
//! "new transaction clears redo" behaviour.

use crate::document::{ObjectId, SavedObject};
use crate::property::Property;

/// A single property assignment: `old` is `None` when the property was created.
#[derive(Debug, Clone)]
pub struct PropertyChange {
    pub object: ObjectId,
    pub name: String,
    pub old: Option<Property>,
    pub new: Property,
}

/// One reversible edit inside a transaction.
#[derive(Debug, Clone)]
pub enum Change {
    /// A property assignment.
    Property(PropertyChange),
    /// An object was added; `saved` is its full state (for redo / undo-removal).
    AddObject { id: ObjectId, saved: SavedObject },
    /// An object was removed; `saved` is its full state (for undo restore).
    RemoveObject { id: ObjectId, saved: SavedObject },
    /// An expression was set on `(object, prop)`; `old` was the previous source.
    AddExpression {
        object: ObjectId,
        prop: String,
        old: Option<String>,
        new: String,
    },
    /// An expression was removed from `(object, prop)`.
    RemoveExpression {
        object: ObjectId,
        prop: String,
        old: String,
    },
}

/// A committed (or active) transaction: a name plus its ordered changes.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub name: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, Default)]
pub struct TransactionManager {
    /// A transaction requested by `open`, not yet made active by a change.
    pending: Option<String>,
    active: Option<Transaction>,
    /// Stack of committed transactions (oldest first; newest on top).
    undo: Vec<Transaction>,
    /// Stack of undone transactions (top is the next to redo).
    redo: Vec<Transaction>,
}

impl TransactionManager {
    /// Request a (named) transaction. It only becomes `active` on the first
    /// recorded change; if a transaction is already active it is committed when
    /// the *next* change arrives, mirroring FreeCAD's pending-transaction model.
    pub fn open_named(&mut self, name: &str) {
        self.pending = Some(name.to_string());
    }

    /// Make the pending transaction active and return its name (once).
    ///
    /// Any already-active transaction is committed first (so consecutive
    /// `openTransaction` calls without an explicit commit still produce separate
    /// undo entries), and a new active transaction invalidates the redo stack.
    pub fn begin(&mut self) -> Option<String> {
        if let Some(name) = self.pending.take() {
            if let Some(previous) = self.active.take() {
                self.undo.push(previous);
            }
            self.redo.clear();
            self.active = Some(Transaction {
                name: name.clone(),
                changes: Vec::new(),
            });
            return Some(name);
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

    /// The undo transaction names, newest first (FreeCAD `UndoNames`). An
    /// in-progress (active) transaction is included as the newest entry.
    pub fn undo_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(active) = &self.active {
            names.push(active.name.clone());
        }
        names.extend(self.undo.iter().rev().map(|t| t.name.clone()));
        names
    }

    /// The redo transaction names, newest first (FreeCAD `RedoNames`).
    pub fn redo_names(&self) -> Vec<String> {
        self.redo.iter().rev().map(|t| t.name.clone()).collect()
    }

    pub fn record(&mut self, change: Change) {
        self.begin();
        if let Some(active) = &mut self.active {
            active.changes.push(change);
        }
    }

    /// Commit the active transaction; returns whether one was committed.
    pub fn commit(&mut self) -> bool {
        self.pending = None;
        match self.active.take() {
            Some(active) => {
                self.undo.push(active);
                true
            }
            None => false,
        }
    }

    /// Discard the active transaction (the caller reverts its changes).
    pub fn abort(&mut self) -> Option<Transaction> {
        self.pending = None;
        self.active.take()
    }

    /// Pop the newest undo transaction (returning it and pushing it to redo).
    /// An active transaction is committed first so it can be undone too.
    pub fn undo(&mut self) -> Option<Transaction> {
        self.commit();
        let t = self.undo.pop()?;
        self.redo.push(t.clone());
        Some(t)
    }

    /// Pop the newest redo transaction (returning it and pushing it to undo).
    pub fn redo(&mut self) -> Option<Transaction> {
        let t = self.redo.pop()?;
        self.undo.push(t.clone());
        Some(t)
    }

    /// Drop all undo/redo history (`clearUndos`).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

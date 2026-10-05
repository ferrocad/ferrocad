//! The transaction engine: open/commit/abort plus a named undo/redo stack, and
//! the [`Document`] methods that record, apply and revert changes.
//!
//! A transaction collects a list of reversible [`Change`]s. Opening a transaction
//! books a name and a process-unique id; the transaction becomes active — and the
//! redo stack is dropped — on the first recorded change, mirroring FreeCAD's
//! pending-transaction behaviour. The engine is a submodule of [`document`](super)
//! so it can drive [`Document`]'s object store directly (a second `impl Document`).
//!
//! # Lifecycle
//!
//! ```text
//! open_transaction_named("T")
//!         │  (books a name + id; nothing is undoable yet)
//!         ▼
//!    record(Change) ──► active transaction, redo cleared
//!         │
//!         ├── commit_transaction() ──► pushed onto the undo stack
//!         ├── abort_transaction()  ──► changes reverted, no entry left
//!         └── undo() / redo()      ──► move a whole transaction between stacks
//! ```

use std::collections::BTreeMap;

use super::{Document, DocumentObject, ObjectId, SavedObject};
use crate::property::{Property, PropertyContainer};

/// Hands out a process-unique id per transaction (FreeCAD `Transaction::getNewID`).
static NEXT_TRANSACTION_ID: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(1);

fn new_transaction_id() -> usize {
    NEXT_TRANSACTION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

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

/// A committed (or active) transaction: an id, a name and its ordered changes.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: usize,
    pub name: String,
    pub changes: Vec<Change>,
}

/// A transaction requested by `open`, not yet made active by a change.
#[derive(Debug)]
struct Pending {
    name: String,
    id: usize,
}

#[derive(Debug, Default)]
pub struct TransactionManager {
    /// A transaction requested by `open`, not yet made active by a change.
    pending: Option<Pending>,
    active: Option<Transaction>,
    /// Stack of committed transactions (oldest first; newest on top).
    undo: Vec<Transaction>,
    /// Stack of undone transactions (top is the next to redo).
    redo: Vec<Transaction>,
}

impl TransactionManager {
    /// Request a (named) transaction, booking a fresh transaction id. It only
    /// becomes `active` on the first recorded change; if a transaction is already
    /// active it is committed when the *next* change arrives, mirroring FreeCAD's
    /// pending-transaction model.
    pub fn open_named(&mut self, name: &str) {
        self.pending = Some(Pending {
            name: name.to_string(),
            id: new_transaction_id(),
        });
    }

    /// Make the pending transaction active and return its name (once).
    ///
    /// Any already-active transaction is committed first (so consecutive
    /// `openTransaction` calls without an explicit commit still produce separate
    /// undo entries), and a new active transaction invalidates the redo stack.
    pub fn begin(&mut self) -> Option<String> {
        if let Some(pending) = self.pending.take() {
            if let Some(previous) = self.active.take() {
                self.undo.push(previous);
            }
            self.redo.clear();
            let name = pending.name.clone();
            self.active = Some(Transaction {
                id: pending.id,
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

    /// The currently booked transaction id, or 0 if none is open
    /// (FreeCAD `getBookedTransactionID`).
    pub fn booked_transaction_id(&self) -> usize {
        if let Some(pending) = &self.pending {
            return pending.id;
        }
        self.active.as_ref().map(|t| t.id).unwrap_or(0)
    }

    /// The number of undo steps (FreeCAD `getAvailableUndos`). With `id == 0`
    /// this is the total count (including an active transaction); otherwise it
    /// is the 1-based depth of the transaction with that id, or 0 if unknown.
    pub fn available_undos(&self, id: usize) -> usize {
        if id == 0 {
            return self.undo.len() + usize::from(self.active.is_some());
        }
        let mut depth = 0;
        if let Some(active) = &self.active {
            depth += 1;
            if active.id == id {
                return depth;
            }
        }
        for t in self.undo.iter().rev() {
            depth += 1;
            if t.id == id {
                return depth;
            }
        }
        0
    }

    /// The number of redo steps (FreeCAD `getAvailableRedos`). With `id == 0`
    /// this is the total count; otherwise the 1-based depth of the transaction
    /// with that id from the top of the redo stack, or 0 if unknown.
    pub fn available_redos(&self, id: usize) -> usize {
        if id == 0 {
            return self.redo.len();
        }
        let mut depth = 0;
        for t in self.redo.iter().rev() {
            depth += 1;
            if t.id == id {
                return depth;
            }
        }
        0
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
    /// An active transaction is committed first.
    pub fn redo(&mut self) -> Option<Transaction> {
        self.commit();
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

// ---------------------------------------------------------------------------
// The `Document` side of the engine: recording, applying and reverting changes.
// ---------------------------------------------------------------------------

impl Document {
    // -- open / commit / abort -----------------------------------------------

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

    // -- metadata ------------------------------------------------------------

    /// The undo transaction names, newest first (FreeCAD `UndoNames`).
    pub fn undo_names(&self) -> Vec<String> {
        self.tx.undo_names()
    }

    /// The redo transaction names, newest first (FreeCAD `RedoNames`).
    pub fn redo_names(&self) -> Vec<String> {
        self.tx.redo_names()
    }

    /// The currently booked transaction id, or 0 (FreeCAD `getBookedTransactionID`).
    pub fn booked_transaction_id(&self) -> usize {
        self.tx.booked_transaction_id()
    }

    /// The number of undo steps, or the depth of transaction `id`
    /// (FreeCAD `getAvailableUndos`).
    pub fn available_undos(&self, id: usize) -> usize {
        self.tx.available_undos(id)
    }

    /// The number of redo steps, or the depth of transaction `id`
    /// (FreeCAD `getAvailableRedos`).
    pub fn available_redos(&self, id: usize) -> usize {
        self.tx.available_redos(id)
    }

    /// Drop all undo/redo history (FreeCAD `clearUndos`).
    pub fn clear_undos(&mut self) {
        self.tx.clear();
    }

    // -- recording -----------------------------------------------------------

    /// Record a reversible change, but only while a transaction is pending or
    /// active (edits outside a transaction are not undoable). Called by the
    /// object-model mutators in the parent module.
    pub(super) fn record_change(&mut self, change: Change) {
        if self.tx.is_active() || self.tx.has_pending() {
            self.tx.record(change);
        }
    }

    // -- undo / redo ---------------------------------------------------------

    pub fn abort_transaction(&mut self) -> bool {
        match self.tx.abort() {
            Some(tx) => {
                for change in tx.changes.iter().rev() {
                    self.revert(change);
                }
                true
            }
            None => false,
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.tx.undo() {
            Some(tx) => {
                for change in tx.changes.iter().rev() {
                    self.revert(change);
                }
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.tx.redo() {
            Some(tx) => {
                for change in tx.changes.iter() {
                    self.apply(change);
                }
                true
            }
            None => false,
        }
    }

    /// Re-apply a change (the redo direction).
    fn apply(&mut self, change: &Change) {
        match change {
            Change::Property(c) => {
                if let Some(obj) = self.objects.get_mut(&c.object) {
                    obj.properties.set(c.name.clone(), c.new.clone());
                }
            }
            Change::AddObject { id, saved } => self.insert_saved_object(*id, saved),
            Change::RemoveObject { id, .. } => {
                self.remove_object_raw(*id, true);
            }
            Change::AddExpression {
                object,
                prop,
                new,
                ..
            } => self.set_expression_raw(*object, prop, new),
            Change::RemoveExpression { object, prop, .. } => {
                if let Some(obj) = self.objects.get_mut(object) {
                    obj.expressions.remove(prop);
                }
                self.enforce_recompute(*object);
            }
        }
    }

    /// Undo a change (the reverse direction).
    fn revert(&mut self, change: &Change) {
        match change {
            Change::Property(c) => {
                if let Some(obj) = self.objects.get_mut(&c.object) {
                    match &c.old {
                        Some(old) => obj.properties.set(c.name.clone(), old.clone()),
                        None => {
                            obj.properties.remove(&c.name);
                        }
                    }
                }
            }
            Change::AddObject { id, .. } => {
                self.remove_object_raw(*id, false);
            }
            Change::RemoveObject { id, saved } => self.insert_saved_object(*id, saved),
            Change::AddExpression {
                object,
                prop,
                old,
                ..
            } => match old {
                Some(source) => self.set_expression_raw(*object, prop, source),
                None => {
                    if let Some(obj) = self.objects.get_mut(object) {
                        obj.expressions.remove(prop);
                    }
                    self.enforce_recompute(*object);
                }
            },
            Change::RemoveExpression { object, prop, old } => {
                self.set_expression_raw(*object, prop, old);
            }
        }
    }

    /// Insert an expression without validation or recording (undo/redo path).
    fn set_expression_raw(&mut self, object: ObjectId, prop: &str, source: &str) {
        if let Some(obj) = self.objects.get_mut(&object) {
            obj.expressions.insert(prop.to_string(), source.to_string());
        }
        // Setting or restoring an expression dirties the object (FreeCAD
        // `ExpressionEngine::setExpression`), so undo/redo propagates.
        self.enforce_recompute(object);
    }

    /// Re-insert an object (with its original id) from a saved snapshot
    /// (undo of a removal, or redo of an addition).
    fn insert_saved_object(&mut self, id: ObjectId, saved: &SavedObject) {
        let node = self.graph.add_node(id);
        self.index.insert(id, node);
        let mut properties = PropertyContainer::new();
        for (k, v) in &saved.properties {
            let status = saved
                .property_status
                .get(k)
                .copied()
                .unwrap_or(crate::prop_status::NONE);
            properties.set_with_status(k.clone(), v.clone(), status);
        }
        self.objects.insert(
            id,
            DocumentObject {
                id,
                name: saved.name.clone(),
                label: saved.label.clone(),
                type_id: saved.type_id.clone(),
                properties,
                expressions: saved.expressions.clone(),
                extensions: saved.extensions.iter().cloned().collect(),
                must_execute: false,
                touched: false,
                invalid: false,
                property_versions: BTreeMap::new(),
                python_state: saved.python_state.clone(),
            },
        );
        if id >= self.next_id {
            self.next_id = id + 1;
        }
    }
}

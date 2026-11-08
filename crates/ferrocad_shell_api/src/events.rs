//! The document events a script half can observe.
//!
//! This is the first slice of the `mod-events` contract
//! ([`docs/project/tasks/mod-events.md`]): a **fixed, versioned** vocabulary,
//! emitted synchronously after the host state is consistent. Only the events this
//! slice can honestly emit are here — a mod cannot invent an id, and adding one is
//! a reviewable change to this module.
//!
//! [`docs/project/tasks/mod-events.md`]: https://github.com/ferrocad/ferrocad/blob/main/docs/project/tasks/mod-events.md

use serde_json::{Value, json};

/// One event, with the id a subscriber matches on and its JSON payload.
///
/// The payload is exactly the JSON a page receives through
/// `window.__ferrocadJs.onDocumentEvent(id, payloadJson)`, so a script half forwards
/// it without a second mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentEvent {
    /// One of the ids in this module ([`Self::EDITED`], [`Self::RECOMPUTED`]).
    pub id: &'static str,
    /// The event's fields.
    pub payload: Value,
}

impl DocumentEvent {
    /// A transaction committed: the transaction's name and the objects it wrote.
    pub const EDITED: &'static str = "document.edited";

    /// A `recompute()` returned: the objects that executed.
    pub const RECOMPUTED: &'static str = "document.recomputed";

    /// The ids this slice can emit, in emission order.
    pub const IDS: [&'static str; 2] = [Self::EDITED, Self::RECOMPUTED];

    /// `document.edited` for `transaction`, naming the objects it wrote.
    pub fn edited(transaction: &str, objects: &[String]) -> Self {
        Self {
            id: Self::EDITED,
            payload: json!({ "transaction": transaction, "objects": objects }),
        }
    }

    /// `document.recomputed` for the objects that executed.
    pub fn recomputed(objects: &[String]) -> Self {
        Self {
            id: Self::RECOMPUTED,
            payload: json!({ "objects": objects }),
        }
    }
}

/// Where a component with no [`ScriptHost`](crate::app::ScriptHost) announces events.
///
/// The document doorway (`ferrocad_doc_api`) is not a `ScriptHost`, but an addon's
/// own `create`/`set` is still a document edit. A host that wants those observes
/// them by giving the doorway a sink; it is `Send + Sync` because the doorway is.
pub trait DocumentEventSink: Send + Sync {
    /// Announce one event. Must not block or re-enter the document.
    fn document_event(&self, event: &DocumentEvent);
}

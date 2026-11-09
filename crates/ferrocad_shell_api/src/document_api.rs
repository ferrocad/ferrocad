//! How a script runtime reaches the document.
//!
//! The document half owns the document. A script half cannot link it: a second
//! `ferrocad_core` in the same process is a second registry, which is the split image
//! the whole seam exists to prevent. So a runtime that must read or write the document
//! — the JS surface, a wasm addon, a console — is handed one of these by the host and
//! calls it with a JSON request:
//!
//! ```text
//! JS addon / wasm addon ──call(request)──▶ the host's one document
//! ```
//!
//! The verbs, their arguments and the response envelope belong to the
//! implementation (`ferrocad_doc_api`); this trait is only the document API. That keeps
//! `ferrocad_shell_js` free of the engine while still letting it hold a document.
//!
//! **Implementations must be callable re-entrantly.** A script reaches the document
//! from inside a host callback (a command, an evaluate), so an implementation must
//! not hold a lock across the call and must not assume it is only ever entered from
//! the top of the stack.

/// A script runtime's document API into the document.
pub trait DocumentApi: Send + Sync {
    /// Run one request and return its JSON response.
    ///
    /// A failure is a response, not an absent one: `{"ok": false, "error": "…"}`, so
    /// a script can report it and keep running. Implementations must not panic and
    /// must not return an empty string.
    fn call(&self, request: &str) -> String;
}

/// The verbs that **change** the document.
///
/// The one list a sandbox consults: a bundle granted read-only access is refused exactly
/// these, and `ferrocad_doc_api` implements them and nothing else that mutates. Keeping
/// it here rather than in the implementation is what lets a host refuse a write *before*
/// the request reaches the document — on either host, without knowing the payloads.
pub fn is_write(call: &str) -> bool {
    matches!(call, "create" | "set" | "recompute")
}

/// The verbs that only read the document.
///
/// `recompute` is a write on purpose: it executes the dependency graph, and an expression
/// can change a value.
pub fn is_read(call: &str) -> bool {
    matches!(call, "document" | "properties")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_write_verbs_are_the_mutating_ones() {
        // Keep in step with `ferrocad_doc_api`'s dispatch: a new mutating arm belongs
        // here, and a read-only door that missed it would not be read-only.
        for call in ["create", "set", "recompute"] {
            assert!(is_write(call), "{call} writes");
            assert!(!is_read(call), "{call} is not a read");
        }
        for call in ["document", "properties"] {
            assert!(is_read(call), "{call} reads");
            assert!(!is_write(call), "{call} does not write");
        }
    }

    #[test]
    fn an_unknown_verb_is_neither() {
        // It reaches the door and the door answers "unknown call"; it cannot mutate.
        assert!(!is_write("teleport"));
        assert!(!is_read("teleport"));
    }
}

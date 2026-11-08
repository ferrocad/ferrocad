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
//! implementation (`ferrocad_doc_api`); this trait is only the doorway. That keeps
//! `ferrocad_shell_js` free of the engine while still letting it hold a document.
//!
//! **Implementations must be callable re-entrantly.** A script reaches the document
//! from inside a host callback (a command, an evaluate), so an implementation must
//! not hold a lock across the call and must not assume it is only ever entered from
//! the top of the stack.

/// A script runtime's doorway into the document.
pub trait DocumentApi: Send + Sync {
    /// Run one request and return its JSON response.
    ///
    /// A failure is a response, not an absent one: `{"ok": false, "error": "…"}`, so
    /// a script can report it and keep running. Implementations must not panic and
    /// must not return an empty string.
    fn call(&self, request: &str) -> String;
}

//! The application: the registry of open documents.
//!
//! This is the analog of FreeCAD's `App::Application` (`App::GetApplication()`),
//! which owns the `DocMap` of open documents and the active document. It is
//! deliberately **geometry-agnostic**: the kernel lives in a workbench (Part), not
//! here. See `docs/occt-integration.md`.
//!
//! Documents are shared, not copied: the registry and the Python wrappers hold the
//! same [`DocumentHandle`]. That is what lets Rust enumerate documents without going
//! through Python.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::document::Document;

/// A shared, mutable document. The registry and any Python wrapper for it hold the
/// same handle.
pub type DocumentHandle = Arc<Mutex<Document>>;

/// A listener for document lifecycle events (FreeCAD's `App::DocumentObserver`).
///
/// Every method has a default, so an implementation overrides only what it needs.
pub trait DocumentObserver: Send {
    /// A document was created or registered.
    fn on_document_created(&mut self, name: &str) {
        let _ = name;
    }
    /// A document was closed.
    fn on_document_deleted(&mut self, name: &str) {
        let _ = name;
    }
    /// The active document changed.
    fn on_document_activated(&mut self, name: &str) {
        let _ = name;
    }
}

/// The registry of open documents and the active one.
///
/// `Send` (documents are `Send`), so it can live in the process-wide [`application`].
#[derive(Default)]
pub struct Application {
    documents: BTreeMap<String, DocumentHandle>,
    active: Option<String>,
    observers: Vec<Box<dyn DocumentObserver>>,
}

impl Application {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a listener for document lifecycle events.
    pub fn add_observer(&mut self, observer: Box<dyn DocumentObserver>) {
        self.observers.push(observer);
    }

    /// FreeCAD-style unique name: `proposed` if free, else `proposed001`,
    /// `proposed002`, … The base defaults to `Unnamed`.
    pub fn unique_name(&self, proposed: Option<&str>) -> String {
        let base = proposed.unwrap_or("Unnamed");
        if !self.documents.contains_key(base) {
            return base.to_string();
        }
        let mut counter = 1;
        loop {
            let candidate = format!("{base}{counter:03}");
            if !self.documents.contains_key(&candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    /// Create, register and activate a new empty document; returns its final name
    /// and handle.
    pub fn new_document(&mut self, proposed: Option<&str>) -> (String, DocumentHandle) {
        self.insert_handle(proposed, Arc::new(Mutex::new(Document::new())))
    }

    /// Register an already-built document under a unique name and activate it.
    pub fn insert_document(&mut self, proposed: &str, document: Document) -> (String, DocumentHandle) {
        self.insert_handle(Some(proposed), Arc::new(Mutex::new(document)))
    }

    /// Register a shared handle under a unique name and activate it.
    pub fn insert_handle(&mut self, proposed: Option<&str>, handle: DocumentHandle) -> (String, DocumentHandle) {
        let name = self.unique_name(proposed);
        self.documents.insert(name.clone(), Arc::clone(&handle));
        for observer in &mut self.observers {
            observer.on_document_created(&name);
        }
        self.set_active(&name);
        (name, handle)
    }

    /// Close a document. Returns whether a document of that name was open.
    pub fn close_document(&mut self, name: &str) -> bool {
        if self.documents.remove(name).is_none() {
            return false;
        }
        if self.active.as_deref() == Some(name) {
            self.active = None;
        }
        for observer in &mut self.observers {
            observer.on_document_deleted(name);
        }
        true
    }

    /// The shared handle for `name`, if open.
    pub fn get(&self, name: &str) -> Option<DocumentHandle> {
        self.documents.get(name).cloned()
    }

    /// Whether a document of that name is open.
    pub fn contains(&self, name: &str) -> bool {
        self.documents.contains_key(name)
    }

    /// Open document names, in name order.
    pub fn names(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    /// Open documents as `(name, handle)`, in name order.
    pub fn documents(&self) -> Vec<(String, DocumentHandle)> {
        self.documents
            .iter()
            .map(|(name, handle)| (name.clone(), Arc::clone(handle)))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// The active document's name, if any.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Make `name` active. Returns whether it is (or became) the active document.
    pub fn set_active(&mut self, name: &str) -> bool {
        if !self.documents.contains_key(name) {
            return false;
        }
        if self.active.as_deref() == Some(name) {
            return true;
        }
        self.active = Some(name.to_string());
        for observer in &mut self.observers {
            observer.on_document_activated(name);
        }
        true
    }
}

static APPLICATION: OnceLock<Mutex<Application>> = OnceLock::new();

/// The process-wide application (FreeCAD's `App::GetApplication()`).
pub fn application() -> &'static Mutex<Application> {
    APPLICATION.get_or_init(|| Mutex::new(Application::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        log: Arc<Mutex<Vec<String>>>,
    }
    impl DocumentObserver for Recorder {
        fn on_document_created(&mut self, name: &str) {
            self.log.lock().unwrap().push(format!("created:{name}"));
        }
        fn on_document_deleted(&mut self, name: &str) {
            self.log.lock().unwrap().push(format!("deleted:{name}"));
        }
        fn on_document_activated(&mut self, name: &str) {
            self.log.lock().unwrap().push(format!("activated:{name}"));
        }
    }

    #[test]
    fn application_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Application>();
    }

    #[test]
    fn new_document_registers_and_activates() {
        let mut app = Application::new();
        let (name, handle) = app.new_document(Some("Demo"));
        assert_eq!(name, "Demo");
        assert_eq!(app.active(), Some("Demo"));
        assert_eq!(app.len(), 1);
        assert!(Arc::ptr_eq(&handle, &app.get("Demo").unwrap()));
    }

    #[test]
    fn names_are_made_unique() {
        let mut app = Application::new();
        assert_eq!(app.new_document(Some("Doc")).0, "Doc");
        assert_eq!(app.new_document(Some("Doc")).0, "Doc001");
        assert_eq!(app.new_document(Some("Doc")).0, "Doc002");
        assert_eq!(app.new_document(None).0, "Unnamed");
        assert_eq!(app.new_document(None).0, "Unnamed001");
    }

    #[test]
    fn closing_clears_active_and_fires_deleted() {
        let mut app = Application::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        app.add_observer(Box::new(Recorder { log: Arc::clone(&log) }));
        app.new_document(Some("A"));
        let (b, _) = app.new_document(Some("B"));
        assert_eq!(app.active(), Some("B"));

        assert!(app.close_document(&b));
        assert_eq!(app.active(), None);
        assert!(!app.close_document("B"));

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "created:A", "activated:A", "created:B", "activated:B", "deleted:B"
            ]
        );
    }

    #[test]
    fn set_active_only_for_open_documents() {
        let mut app = Application::new();
        app.new_document(Some("A"));
        app.new_document(Some("B"));
        assert!(app.set_active("A"));
        assert_eq!(app.active(), Some("A"));
        assert!(!app.set_active("missing"));
        assert_eq!(app.active(), Some("A"));
    }

    #[test]
    fn insert_document_takes_a_prebuilt_document() {
        let mut app = Application::new();
        let (name, _) = app.insert_document("Loaded", Document::new());
        assert_eq!(name, "Loaded");
        assert!(app.contains("Loaded"));
        assert_eq!(app.active(), Some("Loaded"));
    }
}

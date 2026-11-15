//! The kernel-free document half: a real `ferrocad_core::Document` behind the
//! shell's [`AppBackend`](ferrocad_shell_api::app::AppBackend).
//!
//! This crate is what `ferrocad_shell_rust` used to be, minus the geometry kernel.
//! The document graph, the property rows, the undo history, `.FCStd` serialization,
//! the model tree, the viewport stage and the command catalogue are all kernel-free;
//! only four things are not — registering an object type, building the startup
//! sample, tessellating a stored shape, and constructing the two Part feature edits
//! a manipulator commit asks for. Those live behind [`DocumentEngine`], so a build
//! that links no kernel gets a working document with an empty sample, and
//! `ferrocad_shell_rust` plugs `PartEngine` in.
//!
//! ```text
//! ferrocad_shell_doc -> ferrocad_core, ferrocad_stage, ferrocad_shell_api, serde_json
//!                       must NOT -> ferrocad_part, ferrocad_occt, opencascade-sys
//! ferrocad_shell_rust -> ferrocad_shell_doc, ferrocad_part, ferrocad_occt
//! ```
//!
//! Kinds that hold a `Shape` are drawn only when the engine can mesh them; a script
//! half can still supply its own meshes (the toy edition does), and that is why the
//! base app and the toy edition are honest kernel-free editions rather than broken
//! ones.

mod backend;

pub mod appearance_settings;
pub mod grid_settings;

pub use backend::{hide_object, property_rows, DocumentBackend};

use ferrocad_core::{Document, DocumentObject, ObjectId, Placement};
use ferrocad_shell_api::display::Tessellation;

/// The parts of the document half that need a geometry kernel.
///
/// Every method has a kernel-free default, so [`NoEngine`] is the whole trait: a
/// build with no kernel still gets a real document, an undo history and a model
/// tree, and simply draws no stored shapes.
pub trait DocumentEngine: Send + Sync + 'static {
    /// The kernel this engine computes with, for the console's `hello` line.
    fn kernel(&self) -> &'static str {
        "no kernel"
    }

    /// Register the object types a decoded document or the sample may name.
    ///
    /// Called before every `.FCStd` decode and before [`sample`](Self::sample), so
    /// a type the engine owns is registered before an object of that type is
    /// constructed.
    fn register_types(&self) {}

    /// The startup document for a registry that boots empty.
    ///
    /// Returns the document and, when the sample could not be built, the reason —
    /// the backend reports that as a boot error instead of pretending it loaded.
    fn sample(&self) -> (Document, Option<String>);

    /// Mesh one document object's stored shape for display.
    ///
    /// `None` means "this object has no shape to mesh"; `Some(Err(_))` is a
    /// tessellation refusal, which the backend reports against the object's name.
    /// The default draws nothing, which is what a build with no kernel does.
    fn mesh(
        &self,
        _object: &DocumentObject,
    ) -> Option<Result<(std::sync::Arc<Tessellation>, Placement), String>> {
        None
    }

    /// Build the edge dress-up feature (`Part::Fillet`/`Part::Chamfer`) over
    /// `object`'s `edge`, reusing `feature` when it exists.
    ///
    /// Returns the objects `recompute()` executed. The default refuses: an engine
    /// that can mesh a shape but cannot build the feature would be lying.
    fn dress_up(
        &self,
        _document: &mut Document,
        _object: &str,
        _feature: &str,
        _edge: &str,
        _first: f64,
        _second: f64,
        fillet: bool,
    ) -> Result<Vec<ObjectId>, String> {
        Err(format!(
            "a {} needs the Part workbench, which this build does not link",
            if fillet { "fillet" } else { "chamfer" }
        ))
    }

    /// Build the face push/pull: the extrusion `extrusion`, and the boolean
    /// `feature` that consumes it into `object`.
    ///
    /// Returns the objects `recompute()` executed.
    fn push_pull(
        &self,
        _document: &mut Document,
        object: &str,
        _extrusion: &str,
        _feature: &str,
        _face: &str,
        _distance: f64,
    ) -> Result<Vec<ObjectId>, String> {
        Err(format!(
            "push/pull on {object} needs the Part workbench, which this build does not link"
        ))
    }
}

/// The engine of a build that links no geometry kernel.
///
/// The sample is an empty document, no stored shape is meshed, and a Part
/// manipulator commit is reported as unavailable rather than applied.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEngine;

impl DocumentEngine for NoEngine {
    fn sample(&self) -> (Document, Option<String>) {
        (Document::new(), None)
    }
}

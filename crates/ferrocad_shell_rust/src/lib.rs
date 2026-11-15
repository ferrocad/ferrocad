//! The Part-backed document half: a real document whose features are computed by the
//! OCCT kernel.
//!
//! [`ferrocad_shell_doc`] owns the backend — the document graph, the property rows,
//! the undo history, `.FCStd` I/O, the model tree, the viewport stage and the command
//! catalogue. This crate supplies its [`PartEngine`]: object-type registration, the
//! embedded `Part::Box` + `Part::Fillet` sample, tessellation, and the two Part
//! feature edits a manipulator commit asks for.
//!
//! ```text
//! ferrocad_shell_rust -> ferrocad_shell_doc, ferrocad_part, ferrocad_occt, ferrocad_geom
//!                        must NOT -> gpui, pyo3, wasm-bindgen
//! ```
//!
//! It is the backend both the browser host (`ferrocad_web_gpui`) and a desktop
//! edition install — that is why this crate names no window system and no
//! interpreter. A build that links no kernel uses
//! [`ferrocad_shell_doc::DocumentBackend`] directly and never depends on this crate;
//! that is what keeps the base app and the toy edition kernel-free.
//!
//! # What is real, and what is not
//!
//! Real: `Document::recompute`, the Part object types, the kernel, tessellation into
//! `StageMesh`es, picking (the shell's CPU ray cast over that stage), property edits
//! through [`AppBackend::set_property`](ferrocad_shell_api::app::AppBackend::set_property)
//! followed by a recompute, the undo history and file I/O at the byte level.
//!
//! Not yet: expressions, binary file I/O, and anything scripted — there is no
//! interpreter in this crate, so `evaluate` says so rather than pretending. The two
//! file verbs are the **shell's**: a host installs a `FilePicker`, and the backend
//! only moves bytes (`open_document`/`save_document`).

use std::sync::Arc;

use ferrocad_core::{Document, DocumentObject, ObjectId, Placement, Property, Quantity, Unit};
use ferrocad_geom::{Tessellation, TessellationOptions};
use ferrocad_occt::OcctBackend;
use ferrocad_part::{
    CHAMFER_TYPE, CUT_TYPE, EXTRUSION_TYPE, FILLET_TYPE, FUSE_TYPE, backend, shape_of,
};
use ferrocad_shell_doc::{hide_object, DocumentEngine};
pub use ferrocad_shell_doc::DocumentBackend;

/// The one startup document both hosts boot.
///
/// It is the same checked-in archive `python/ferrocad_shell.bootstrap` opens on the
/// desktop, embedded so a host with no `python/` tree on disk (the browser) still
/// boots the identical document name, object tree and shape.
const SAMPLE_FCSTD: &[u8] = include_bytes!("../../../python/ferrocad_shell/Sample.FCStd");

/// The tessellation tolerance, in millimetres (the probe's).
///
/// It is `ferrocad_part::DISPLAY_DEFLECTION` so the document half and a display hook
/// share one memo entry per shape (see that constant).
pub const TOLERANCE: f64 = ferrocad_part::DISPLAY_DEFLECTION;

/// Part, computed by OCCT, behind [`DocumentBackend`].
///
/// This is the whole of the crate's engine: everything else about the document half
/// is kernel-free and lives in `ferrocad_shell_doc`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PartEngine;

impl PartEngine {
    /// A backend that owns its own sample document (the browser and the tests).
    ///
    /// In a wasm build this must run after the module's static constructors
    /// (`__wasm_call_ctors`): the kernel's C++ runtime has to be initialised before
    /// the first call.
    pub fn backend() -> DocumentBackend {
        DocumentBackend::with_engine(PartEngine)
    }

    /// A backend over the process-wide registry `ferrocad_py` binds to.
    pub fn shared_backend() -> DocumentBackend {
        DocumentBackend::from_shared_registry_with(PartEngine)
    }
}

impl DocumentEngine for PartEngine {
    fn kernel(&self) -> &'static str {
        "OCCT"
    }

    fn register_types(&self) {
        // Registration is idempotent (`ferrocad_part` keeps the first backend), so an
        // opened file and the embedded sample take the same path.
        ferrocad_part::register(Arc::new(OcctBackend::new()));
    }

    fn sample(&self) -> (Document, Option<String>) {
        match self.decode(SAMPLE_FCSTD) {
            Ok(document) => {
                log_shapes(&document);
                (document, None)
            }
            Err(error) => {
                log::error!("ferrocad: the sample document did not build: {error}");
                (Document::new(), Some(error))
            }
        }
    }

    fn mesh(&self, object: &DocumentObject) -> Option<Result<(Arc<Tessellation>, Placement), String>> {
        // A document object with no stored shape draws nothing; one with a shape the
        // kernel cannot mesh is an error the sync reports against the object.
        let property = object.properties.get("Shape").and_then(shape_of)?;
        if property.shape().is_none() {
            return None;
        }
        // The property memoises: a sync of an unchanged document copies, it does not
        // re-mesh. `tessellate_rigid` hands back the frame the mesh was built in plus
        // the move that places it — the rows stay identical across a move, so the
        // viewport patches packet transforms instead of re-emitting every vertex. The
        // memo's `Arc` is what crosses the seam.
        Some(
            property
                .tessellate_rigid(TOLERANCE, TessellationOptions::default().angular_deflection)
                .map_err(|error| error.to_string()),
        )
    }

    fn dress_up(
        &self,
        document: &mut Document,
        object: &str,
        feature: &str,
        edge: &str,
        first: f64,
        second: f64,
        fillet: bool,
    ) -> Result<Vec<ObjectId>, String> {
        let type_id = if fillet { FILLET_TYPE } else { CHAMFER_TYPE };
        let id = match document.get_by_name(feature) {
            Some(id) => id,
            None => document.add_object(feature, type_id),
        };
        document.set_property(
            id,
            "Base",
            Property::LinkSub(object.to_string(), vec![edge.to_string()]),
        )?;
        if fillet {
            document.set_property(id, "Radius", length(first))?;
        } else {
            document.set_property(id, "Distance", length(first))?;
            document.set_property(id, "Distance2", length(second))?;
        }
        document.recompute()
    }

    fn push_pull(
        &self,
        document: &mut Document,
        object: &str,
        extrusion_name: &str,
        feature_name: &str,
        face: &str,
        distance: f64,
    ) -> Result<Vec<ObjectId>, String> {
        let outward = distance > 0.0;
        let Some(base) = document.get_by_name(object) else {
            return Err(format!("no object {object}"));
        };
        // The face's outward normal, in the frame its shape is stored in.
        let shape = document
            .object(base)
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .and_then(|property| property.shape())
            .ok_or_else(|| format!("{object} has no shape"))?;
        let face_shape = backend()
            .resolve(&shape, face)
            .ok_or_else(|| format!("{object} has no sub-shape {face}"))?;
        let normal = backend()
            .planar_normal(&face_shape)
            .ok_or_else(|| format!("{face} is not a planar face"))?;

        let extrusion = match document.get_by_name(extrusion_name) {
            Some(id) => id,
            None => document.add_object(extrusion_name, EXTRUSION_TYPE),
        };
        document.set_property(
            extrusion,
            "Base",
            Property::LinkSub(object.to_string(), vec![face.to_string()]),
        )?;
        document.set_property(extrusion, "Dir", Property::Vector(normal))?;
        document.set_property(extrusion, "Solid", Property::Bool(true))?;
        document.set_property(extrusion, "LengthFwd", Property::Float(distance.abs()))?;
        document.set_property(extrusion, "Reversed", Property::Bool(!outward))?;
        // The boolean is what is drawn; the sweep is scaffolding.
        hide_object(document, extrusion, Some(false));

        let feature_type = if outward { FUSE_TYPE } else { CUT_TYPE };
        let feature = match document.get_by_name(feature_name) {
            Some(id) => id,
            None => document.add_object(feature_name, feature_type),
        };
        document.set_property(feature, "Base", Property::Link(object.to_string()))?;
        document.set_property(feature, "Tool", Property::Link(extrusion_name.to_string()))?;
        document.recompute()
    }
}

impl PartEngine {
    /// Decode a saved document, registering Part first.
    fn decode(&self, bytes: &[u8]) -> Result<Document, String> {
        self.register_types();
        let saved = ferrocad_core::fcstd::decode_fcstd(bytes).map_err(|error| error.to_string())?;
        let mut document = Document::from_saved(&saved);
        document.recompute()?;
        Ok(document)
    }
}

/// A millimetre length as the property a Part feature stores (`App::PropertyLength`).
fn length(value: f64) -> Property {
    Property::Quantity(Quantity::new(value, Unit::Millimeter))
}

/// Report the sample document's shapes: one line per tessellated object.
///
/// A feature the kernel refuses keeps its last good shape and records why; that is
/// document state, so it is logged rather than treated as a failed boot. The triangle
/// counts are the probe's "40 triangles" baseline, and make a silent tessellation
/// change visible in the browser console.
fn log_shapes(document: &Document) {
    let backend = backend();
    for id in document.object_ids() {
        let Some(object) = document.object(id) else {
            continue;
        };
        if object.invalid {
            log::warn!(
                "ferrocad: {} is invalid: {}",
                object.name,
                object.error.as_deref().unwrap_or("no reason recorded")
            );
        }
        let Some(shape) = object
            .properties
            .get("Shape")
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            continue;
        };
        match backend.tessellate(&shape, &TessellationOptions::absolute(TOLERANCE)) {
            Ok(mesh) => log::info!(
                "ferrocad: {} · {} triangles, {} edges",
                object.name,
                mesh.triangles.len() / 3,
                mesh.edges.len() / 2
            ),
            Err(error) => log::error!("ferrocad: {} did not tessellate: {error}", object.name),
        }
    }
}

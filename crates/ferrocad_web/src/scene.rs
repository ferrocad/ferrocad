//! The demo scene: a real `Part` document, not a direct kernel call.
//!
//! `Document::recompute` runs `Part::Box` and `Part::Fillet` through the OCCT
//! backend, and the resulting shape is tessellated for the stage. A fillet the
//! kernel refuses is a document-level state, not a panic: `dress_up` keeps the last
//! good shape in the output property and records the reason, so the page can show
//! the box that still exists next to the sentence explaining why. That is the F1
//! behaviour of `docs/project/tasks/operation-failure-modelling.md`.

use std::sync::Arc;

use ferrocad_core::{Document, ObjectId, Property};
use ferrocad_geom::{ElementKind, TessellationOptions};
use ferrocad_graphics::{DrawPacket, RenderQueue};
use ferrocad_occt::OcctBackend;
use ferrocad_part::{shape_of, BOX_TYPE, FILLET_TYPE};
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};

/// The edge of the box the fillet is applied to.
const EDGE: &str = "Edge1";

/// The tessellation tolerance, in millimetres.
const TOLERANCE: f64 = 0.2;

/// The radius the page starts with, in millimetres.
pub const DEFAULT_RADIUS: f64 = 1.0;

/// A Part document and the draw packets its last recompute produced.
pub struct Scene {
    document: Document,
    fillet: ObjectId,
    radius: f64,
    /// The last successful packets; a failed rebuild leaves them alone.
    queue: RenderQueue,
    faces: u32,
    triangles: u32,
    /// The last rebuild's refusal, when there was one.
    error: Option<String>,
}

impl Scene {
    /// A 10 mm cube with a fillet on one edge, rebuilt at `radius`.
    pub fn new(radius: f64) -> Result<Self, String> {
        // Idempotent: the Part module keeps one backend per process.
        ferrocad_part::register(Arc::new(OcctBackend::new()));

        let mut document = Document::new();
        let base = document.add_object("Box", BOX_TYPE);
        // `make_box` builds from the corner at the origin; centre it so the orbit
        // camera, which pivots on the origin, frames the whole box.
        document.set_property(
            base,
            "Placement",
            Property::Placement(Placement::new(
                Vector3::new(-5.0, -5.0, -5.0),
                Rotation::identity(),
            )),
        )?;

        let fillet = document.add_object("Fillet", FILLET_TYPE);
        document.set_property(
            fillet,
            "Base",
            Property::LinkSub("Box".to_string(), vec![EDGE.to_string()]),
        )?;

        let mut scene = Self {
            document,
            fillet,
            radius,
            queue: RenderQueue::default(),
            faces: 0,
            triangles: 0,
            error: None,
        };
        scene.set_radius(radius)?;
        Ok(scene)
    }

    /// Set the fillet radius and recompute the document.
    ///
    /// A refused fillet is *not* an `Err`: the feature keeps its last good shape
    /// and the object records the reason, which [`error`](Self::error) reports.
    /// Only a structural failure (a property that refuses the value, a dependency
    /// cycle) returns `Err`.
    pub fn set_radius(&mut self, radius: f64) -> Result<(), String> {
        self.radius = radius;
        self.document
            .set_property(self.fillet, "Radius", Property::Float(radius))?;
        self.document.recompute()?;
        self.refresh();
        Ok(())
    }

    /// Re-read the document: the failure state, then the shape it left behind.
    fn refresh(&mut self) {
        let Some(object) = self.document.object(self.fillet) else {
            return;
        };
        self.error = if object.invalid {
            Some(
                object
                    .error
                    .clone()
                    .unwrap_or_else(|| "the fillet failed".to_string()),
            )
        } else {
            None
        };

        let Some(shape) = object
            .properties
            .get("Shape")
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            // Never built successfully: nothing to draw, and the reason is above.
            return;
        };

        let backend = ferrocad_part::backend();
        self.faces = backend.elements(&shape, ElementKind::Face).len() as u32;
        match backend.tessellate(&shape, &TessellationOptions::absolute(TOLERANCE)) {
            Ok(mesh) => {
                self.triangles = (mesh.triangles.len() / 3) as u32;
                self.queue = Stage::new()
                    .with_item(
                        ShapeItem::from_tessellation(1, &mesh)
                            .with_draw_mode(DrawMode::ShadedWithEdges)
                            .with_surface_color([0.62, 0.66, 0.72, 1.0]),
                    )
                    .to_queue();
            }
            // A shape that tessellates is the contract; if it does not, say so
            // rather than drawing a stale mesh as if it were the answer.
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// The packets of the last successful rebuild.
    pub fn packets(&self) -> &[DrawPacket] {
        &self.queue.packets
    }

    /// The radius the document currently holds (valid or not).
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Faces in the last built shape.
    pub fn faces(&self) -> u32 {
        self.faces
    }

    /// Triangles in the last built shape.
    pub fn triangles(&self) -> u32 {
        self.triangles
    }

    /// Why the last rebuild was refused, if it was.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

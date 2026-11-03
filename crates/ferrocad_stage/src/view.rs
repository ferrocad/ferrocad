//! The published view: the camera and stage a host last drew.
//!
//! A canvas-first workbench (SimpleCAD) reaches the viewport through
//! `FreeCADGui.ActiveDocument.ActiveView`: `getPointOnScreen`, `getViewDirection`,
//! `getCameraNode` and `getObjectsInfo`. The Python facade cannot see the shell's
//! `InventorViewport`, so the host **publishes** the two things those calls need:
//!
//! * the camera, on every shell render and on every viewport notification
//!   ([`publish_camera`]);
//! * the compiled stage and its entity/part-name tables, whenever the document's
//!   display changes ([`publish_stage`]).
//!
//! Both are process-global because a process has one window and one active document
//! view. A headless script (or a test) publishes the same two values through
//! `ferrocad_stage.set_view_camera` / `set_view_stage`, so the facade behaves
//! identically without a shell.
//!
//! A host also publishes **transient preview items** here (S4.6,
//! [`set_preview_items`]). They are drawn beside the stage but are not part of the
//! published pick stage, and each item is forced non-selectable, so a preview never
//! answers a click. [`preview_generation`] is the cheap change signal a host polls to
//! redraw without cloning the list.
//!
//! The state lives here, in the pure stage crate, rather than in the Python facade:
//! the shell and the facade are separate crates and a platform-neutral host (the
//! browser) must be able to publish the same queue without linking CPython. The
//! `ferrocad_stage_py` functions are thin wrappers over this module.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_graphics as gfx;

use crate::ShapeItem;

/// The stage a host last drew, compiled once so a pick does not recompile it.
pub struct PublishedStage {
    /// The stage's draw queue: the same packets the viewport picks against.
    queue: gfx::RenderQueue,
    /// Pick id → document object name (`"Slab"`).
    objects: HashMap<u32, String>,
    /// Pick id → sub-element name table (`["", "Face1", …]`).
    parts: HashMap<u32, Vec<String>>,
}

impl PublishedStage {
    /// Bundle a compiled queue with the name tables a pick reports.
    pub fn new(
        queue: gfx::RenderQueue,
        objects: HashMap<u32, String>,
        parts: HashMap<u32, Vec<String>>,
    ) -> Self {
        Self {
            queue,
            objects,
            parts,
        }
    }

    /// The compiled draw queue.
    pub fn queue(&self) -> &gfx::RenderQueue {
        &self.queue
    }

    /// Pick id → document object name.
    pub fn objects(&self) -> &HashMap<u32, String> {
        &self.objects
    }

    /// Pick id → sub-element name table.
    pub fn parts(&self) -> &HashMap<u32, Vec<String>> {
        &self.parts
    }
}

fn camera_cell() -> &'static Mutex<Option<gfx::Camera>> {
    static CELL: OnceLock<Mutex<Option<gfx::Camera>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn stage_cell() -> &'static Mutex<Option<Arc<PublishedStage>>> {
    static CELL: OnceLock<Mutex<Option<Arc<PublishedStage>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The transient preview items a script published (S4.6).
///
/// A preview is drawn over the document but is **not** part of the published stage:
/// it is unpickable by construction (the item's `selectable` is cleared), and the
/// ranked picker therefore never reports it. The generation counter lets a host
/// that owns the draw (the shell) notice a change without cloning the list.
#[derive(Default)]
struct PreviewState {
    items: Vec<ShapeItem>,
    generation: u64,
}

fn preview_cell() -> &'static Mutex<PreviewState> {
    static CELL: OnceLock<Mutex<PreviewState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(PreviewState::default()))
}

/// Replace the transient preview items with `items`.
///
/// Every item is forced non-selectable: a preview is a visual, so a click must pass
/// through it to the geometry underneath (the same rule the grid follows). Returns
/// the new generation.
pub fn set_preview_items(items: Vec<ShapeItem>) -> u64 {
    let mut state = preview_cell().lock().unwrap_or_else(|e| e.into_inner());
    state.items = items.into_iter().map(force_unpickable).collect();
    state.generation += 1;
    state.generation
}

/// Drop every transient preview. Returns the new generation.
pub fn clear_previews() -> u64 {
    let mut state = preview_cell().lock().unwrap_or_else(|e| e.into_inner());
    state.items.clear();
    state.generation += 1;
    state.generation
}

/// The transient preview items, in publication order.
pub fn preview_items() -> Vec<ShapeItem> {
    preview_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items
        .clone()
}

/// A counter that changes whenever the preview set does.
pub fn preview_generation() -> u64 {
    preview_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation
}

fn force_unpickable(item: ShapeItem) -> ShapeItem {
    if item.selectable() {
        item.with_selectable(false)
    } else {
        item
    }
}

/// Publish the camera the shell is drawing with.
///
/// Cheap enough to call every frame: two matrices and the viewport.
pub fn publish_camera(camera: gfx::Camera) {
    *camera_cell().lock().unwrap() = Some(camera);
}

/// Publish the compiled stage and the name tables a pick reports.
///
/// `objects` maps a pick id to its document object name; `parts` optionally overrides
/// the per-item name tables (a host that builds the stage itself leaves it `None`, and
/// the items' own [`ShapeItem::part_names`] are used).
pub fn publish_stage(
    stage: &crate::Stage,
    objects: HashMap<u32, String>,
    parts: Option<HashMap<u32, Vec<String>>>,
) {
    let parts = parts.unwrap_or_else(|| {
        stage
            .items()
            .iter()
            .filter(|item| !item.part_names().is_empty())
            .map(|item| (item.entity_id(), item.part_names().to_vec()))
            .collect()
    });
    let published = PublishedStage {
        queue: stage.to_queue(),
        objects,
        parts,
    };
    *stage_cell().lock().unwrap() = Some(Arc::new(published));
}

/// Forget the published view (document close, or a headless test's teardown).
pub fn clear_view() {
    *camera_cell().lock().unwrap() = None;
    *stage_cell().lock().unwrap() = None;
}

/// The published camera, if a host has published one.
pub fn published_camera() -> Option<gfx::Camera> {
    camera_cell().lock().unwrap().clone()
}

/// The published stage, if a host has published one.
pub fn published_stage() -> Option<Arc<PublishedStage>> {
    stage_cell().lock().unwrap().clone()
}

/// Whether a stage has been published.
///
/// The shell uses this to publish the stage the viewport was **built** with (boot,
/// or a viewport recreated for a new GPU adapter) exactly once, without cloning it on
/// every frame.
pub fn has_stage() -> bool {
    stage_cell().lock().unwrap().is_some()
}

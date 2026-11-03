//! The per-frame view state the shell publishes for scripted consumers.
//!
//! The camera, the compiled stage and the transient previews are *pull* state: a
//! canvas-first workbench reaches them through `FreeCADGui.ActiveDocument.ActiveView`,
//! and the shell publishes them here every frame. The storage is in the pure
//! [`ferrocad_stage::view`] and [`ferrocad_inventor::overlays`] crates, so a
//! platform-neutral host (the browser) gets the same queue without linking CPython;
//! this module is the shell's single import for them.

pub use ferrocad_inventor::overlays::{dirty as overlays_dirty, take_scenes as overlay_scenes};
pub use ferrocad_stage::view::{
    clear_previews, clear_view, has_stage, preview_generation, preview_items, publish_camera,
    publish_stage, published_camera, published_stage, set_preview_items,
};

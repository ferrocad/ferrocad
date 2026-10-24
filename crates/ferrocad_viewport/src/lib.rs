//! The `bite-gpui` viewport: presents an invented scene and drives its interaction.
//!
//! This is the UI side of the render stack — the counterpart of FreeCAD's
//! `View3DInventorViewer` (and `Gui::NaviCube`) over an Inventor scene:
//!
//! - [`inventor_view`] — [`InventorViewport`], a gpui element that presents an
//!   [`InventorView`](ferrocad_inventor::InventorView) (the image path) and routes
//!   pointer events into the scene (pick → listener → recompile). A click is reported
//!   as a [`SelectionEvent`] carrying a kernel-neutral
//!   [`SelectionRef`](ferrocad_geom::SelectionRef): the host maps its object id to a
//!   document object. A workbench manipulator (the Part edge fillet arc, a BIM
//!   thickness grip, …) registers through the generic
//!   [`Manipulator`](ferrocad_stage::Manipulator) seam; the viewport draws its overlay
//!   and reports its [`InteractionCommit`](ferrocad_stage::InteractionCommit), never
//!   naming the workbench.
//! - [`navicube`] — the navigation-cube controller overlay ([`NaviCube`]).
//! - [`scene`] — a sample scene/camera and a hover listener, so the pipeline and the
//!   interaction path are exercised before real document geometry exists.
//!
//! It builds on two crates: the [`ferrocad_graphics`] primitives (`Camera`,
//! `RenderQueue`, `PickId`, `Rect`, `Hit`, picking) and the [`ferrocad_inventor`] scene
//! vocabulary (`Scene`, `Event`/`EventListener`, `ViewSpec`, the `InventorView`
//! capability trait) — and on no concrete renderer or Python. `ferrocad_wgpu`
//! implements `InventorView`; the host (`ferrocad_gpui`) supplies it and owns the window.
//!
//! ```text
//! ferrocad_viewport ── uses ──▶ ferrocad_graphics  (Camera, RenderQueue, PickId, …)
//!                   ── uses ──▶ ferrocad_geom      (SelectionRef)
//!                   ── uses ──▶ ferrocad_inventor  (Scene, Event, InventorView, …)
//!        ▲
//!        │ host supplies the window          backend implements InventorView
//! ferrocad_gpui                        ferrocad_wgpu
//! ```

pub mod inventor_view;
pub mod navicube;
pub mod presenter;
pub mod scene;

pub use inventor_view::{
    HoverReport, InventorViewport, PreviewPhase, SelectionAnchor, SelectionEvent, TransformPreview,
    ViewportEvent,
};
pub use navicube::NaviCube;
pub use presenter::{Presenter, PresenterStatus};

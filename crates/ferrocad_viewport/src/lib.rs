//! The `bite-gpui` viewport: presents an invented scene and drives its interaction.
//!
//! This is the UI side of the render stack — the counterpart of FreeCAD's
//! `View3DInventorViewer` (and `Gui::NaviCube`) over an Inventor scene:
//!
//! - [`inventor_view`] — [`InventorViewport`], a gpui element that presents an
//!   [`InventorView`](ferrocad_inventor::InventorView) (the image path) and routes
//!   pointer events into the scene (pick → listener → recompile).
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
//!                   ── uses ──▶ ferrocad_inventor  (Scene, Event, InventorView, …)
//!        ▲
//!        │ host supplies the window          backend implements InventorView
//! ferrocad_gpui                        ferrocad_wgpu
//! ```

pub mod inventor_view;
pub mod navicube;
pub mod scene;

pub use inventor_view::InventorViewport;
pub use navicube::NaviCube;

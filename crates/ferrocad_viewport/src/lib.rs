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
//! It depends only on the `ferrocad_inventor` **vocabulary** — `Scene`, `Camera`,
//! `RenderQueue`, `Event`/`EventListener`, `PickId`, and the `InventorView` capability
//! trait — and on no concrete renderer or Python. `ferrocad_wgpu` implements
//! `InventorView`; the host (`ferrocad_gpui`) supplies it and owns the window.
//!
//! ```text
//! ferrocad_viewport ── uses ──▶ ferrocad_inventor   (Scene, Camera, InventorView, …)
//!        ▲                              ▲
//!        │ host supplies                │ backend implements
//! ferrocad_gpui (window)          ferrocad_wgpu (InventorView)
//! ```

pub mod inventor_view;
pub mod navicube;
pub mod scene;

pub use inventor_view::InventorViewport;
pub use navicube::NaviCube;

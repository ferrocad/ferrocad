//! # FerroCAD Stage Python surface (`ferrocad_stage_py`)
//!
//! The modern-stack counterpart of the `pivy.coin` facade: the PyO3 bindings over
//! [`ferrocad_stage`], surfaced as the `ferrocad.stage` module (also importable as the
//! top-level `ferrocad_stage`). New workbenches declare visuals with it instead of
//! assembling an imperative `SoSeparator` tree.
//!
//! The surface mirrors the Rust API:
//!
//! - [`ShapeItem`] — a tessellated shape (`set_shape` ingests a contiguous `f32`/`u32`
//!   buffer zero-copy, or a Python sequence) with its display attributes.
//! - [`Stage`] — a flat list of items, compiled with `batch_count()`.
//! - [`PointerEvent`]/[`Hit`] and the [`MouseButton`]/[`PointerKind`] enums.
//! - [`Tool`] — a base class for Python subclasses; [`PyTool`] drives one through the
//!   native [`ferrocad_stage::Tool`] interface so a scripted tool runs in the viewport.
//! - [`TransformGizmo`] (with [`Camera`], [`GizmoMode`], [`GizmoHandle`],
//!   [`GizmoDragEvent`]/[`GizmoCommitEvent`]) — the `SoDragger` replacement.
//!
//! This crate is pure binding glue: every class wraps its [`ferrocad_stage`]/
//! [`ferrocad_graphics`] counterpart, so there is no behaviour here that the Rust API
//! does not already have.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

mod camera;
mod enums;
mod gizmo;
mod input;
mod shape;
mod snap;
mod stage;
mod tool;
mod view;
mod workplane;

pub use camera::Camera;
pub use enums::{DrawMode, GizmoHandle, GizmoMode, MouseButton, PointerKind};
pub use gizmo::{GizmoCommitEvent, GizmoDragEvent, TransformGizmo};
pub use input::{Hit, PointerEvent};
pub use shape::ShapeItem;
pub use snap::{Snap, SnapCandidate, SnapContext, SnapKind};
pub use stage::{Stage, extract_stage};
pub use tool::{PyTool, Tool, extract_tool};
pub use view::{
    PickHit, clear_previews, clear_view, has_stage, preview_generation, preview_items,
    publish_camera, publish_stage, published_camera, set_preview_items,
};
pub use workplane::{Workplane, WorkplaneKind};

/// The `ferrocad_stage` extension module (the surface of `ferrocad.stage`).
#[pymodule]
pub fn ferrocad_stage(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(py_install, m)?)?;
    enums::register(m)?;
    camera::register(m)?;
    input::register(m)?;
    shape::register(m)?;
    stage::register(m)?;
    tool::register(m)?;
    gizmo::register(m)?;
    workplane::register(m)?;
    snap::register(m)?;
    view::register(m)?;
    Ok(())
}

/// `ferrocad_stage.install()` — the scriptable form of [`install`].
#[pyfunction]
#[pyo3(name = "install")]
fn py_install(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    install(py)
}

/// Register the module in the running interpreter as **both** `ferrocad_stage` (the
/// importable extension) and `ferrocad.stage` (the modern-stack namespace). When the
/// `ferrocad` extension is already loaded, `stage` is attached to it, so `from ferrocad
/// import stage` resolves; a host that imports `ferrocad` as part of its boot should
/// call this **after** doing so.
///
/// Unlike `ferrocad_inventor_py::install` (which fabricates a synthetic `pivy` package)
/// this fabricates nothing: `ferrocad` is a real extension module, and creating a
/// synthetic one would shadow it and hide its classes. Idempotent.
pub fn install(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let modules: Bound<'_, PyDict> = py.import("sys")?.getattr("modules")?.downcast_into()?;

    let stage = match modules.get_item("ferrocad.stage")? {
        Some(existing) => existing.downcast_into::<PyModule>()?,
        None => {
            let stage = pyo3::wrap_pymodule!(crate::ferrocad_stage)(py).into_bound(py);
            modules.set_item("ferrocad_stage", &stage)?;
            modules.set_item("ferrocad.stage", &stage)?;
            stage
        }
    };

    // Attach `stage` to the `ferrocad` extension, when it is loaded (a host that imports
    // `ferrocad` during boot should call this afterwards). Done even when the module was
    // already registered, so a later `ferrocad` import still gets the attribute.
    if let Some(ferrocad) = modules.get_item("ferrocad")? {
        if let Ok(ferrocad) = ferrocad.downcast_into::<PyModule>() {
            ferrocad.setattr("stage", &stage)?;
        }
    }
    Ok(stage)
}

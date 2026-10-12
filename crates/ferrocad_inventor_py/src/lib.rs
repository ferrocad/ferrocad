//! # FerroCAD pivy-compatible `coin` facade (`ferrocad_inventor_py`)
//!
//! `from pivy import coin` is how 75 upstream Python files build scene graphs, and
//! `coin.SoType.fromName("SoBrepEdgeSet").createInstance()` is how they instantiate
//! FreeCAD's custom node classes. This extension reproduces that surface on top of
//! [`ferrocad_inventor`], so a workbench's own scene-building code keeps working.
//!
//! The module is surfaced to Python as `coin` (a `pivy` package shim re-exports it,
//! matching the import path workbenches use). The node classes are thin PyO3
//! wrappers; the semantics live in the Rust core.
//!
//! Open: this facade is also where the renderer will be *managed* — the host will
//! hand it the `ferrocad_wgpu` device so scripts can drive frames. That wiring is
//! not part of the initial scaffolding, so the GPU crate is not yet a dependency.

use ferrocad_inventor::Node;
use pyo3::prelude::*;

/// The `coin` extension module (the surface of `pivy.coin`).
#[pymodule]
fn coin(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<SoSeparator>()?;
    Ok(())
}

/// A minimal stand-in for Coin's `SoSeparator`.
///
/// Holds the backing [`Node`] and grows the surface (`SoCoordinate3`, `SoMaterial`,
/// `SoType`, …) as the facade is filled in.
#[pyclass(name = "SoSeparator")]
pub struct SoSeparator {
    /// The backing Inventor node; `addChild` appends to its children.
    node: Node,
}

#[pymethods]
impl SoSeparator {
    /// `coin.SoSeparator()`.
    #[new]
    fn new() -> Self {
        Self { node: Node::default() }
    }

    /// `sep.addChild(child)` — as Coin's `SoGroup::addChild`.
    #[pyo3(name = "addChild")]
    fn add_child(&mut self, child: PyRef<'_, SoSeparator>) {
        self.node.children.push(child.node.clone());
    }
}

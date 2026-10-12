//! Shared test scaffolding: register the `ferrocad.stage` module in the running
//! interpreter (as both `ferrocad_stage` and `ferrocad.stage`).

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Install `ferrocad.stage` and return the module.
pub fn ferrocad_stage(py: Python<'_>) -> Bound<'_, PyModule> {
    ferrocad_stage_py::install(py).expect("install ferrocad.stage")
}

//! Shared test scaffolding: boot the `coin` module and surface it as `pivy.coin`,
//! matching the import path workbenches use (`from pivy import coin`).

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Register `coin`/`pivy` in the running interpreter and return the `coin` module.
pub fn pivy_coin(py: Python<'_>) -> Bound<'_, PyModule> {
    ferrocad_inventor_py::install(py).expect("install the coin module")
}

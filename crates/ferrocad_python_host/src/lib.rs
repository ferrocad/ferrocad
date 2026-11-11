//! The Python script half of the shell, written once.
//!
//! A Python workbench runs against a **shell module** (`python/ferrocad_shell`) and a
//! facade (`FreeCAD`/`FreeCADGui`). The module is one source file, and almost every
//! question the shell asks it is JSON in, JSON out — so the half's whole
//! [`ScriptHost`](ferrocad_shell_api::app::ScriptHost) implementation belongs in one
//! place rather than once per interpreter.
//!
//! This crate is that place. It knows nothing about PyO3 or RustPython: an engine
//! implements [`PythonRuntime`], which is how to call a function (`call`), what the
//! facade supports ([`Contract`]), and the handful of typed hooks that cannot go through
//! JSON (the display rows, tools, the `pivy` scene, the event listener). Braces, GILs,
//! frozen stdlibs and filesystems stay with the engine.
//!
//! The split this replaces had the two halves describe the same 35 methods twice, so the
//! port drifted silently: the RustPython half inherited the trait defaults and returned
//! nothing for two thirds of the seam, and nothing said so until a document happened to
//! look wrong. Now a capability the facade does not implement is a [`Contract`] flag —
//! one value, listed in one place, and the document half answers exactly as it would
//! with no script half at all.

mod host;
mod runtime;

pub use host::PythonHost;
pub use runtime::{Contract, NoopScriptListener, PythonRuntime};

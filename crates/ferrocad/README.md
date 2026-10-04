# ferrocad

This crate reserves the `ferrocad` name on crates.io. It is intentionally
empty at `0.0.0` and exposes nothing yet.

The project lives at **https://github.com/ferrocad/ferrocad**:

- **`ferrocad_core`** — the pure-Rust engine: typed properties, a document object
  model with a recompute dependency graph, transactions with undo/redo, observers,
  expressions, and FreeCAD's geometry and unit types.
- **`ferrocad_widgets`** — reusable `bite-gpui` widgets (window chrome, editable
  field, console text area) shared by the application shell.

The public surface is the **`FreeCAD`** Python package: a drop-in, Rust-backed
reimplementation of FreeCAD's `App` core.

This crate will later become either a facade that re-exports the core crates, or
the base application binary. Until then, depend on `ferrocad_core` and
`ferrocad_widgets` directly.

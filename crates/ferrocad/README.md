# ferrocad

The **base FerroCAD application**: a Rust reimplementation of FreeCAD's `App`
core, exposed as the drop-in `FreeCAD` Python package.

- `cargo install ferrocad` builds the app binary. It boots an embedded CPython
  interpreter and opens the `bite-gpui` shell (model inspector, property editor,
  Python console).
- The library target is a placeholder for a facade that will re-export the core
  crates.

The project lives at **https://github.com/ferrocad/ferrocad**:

- **`ferrocad_core`** — the pure-Rust engine: typed properties, a document object
  model with a recompute dependency graph, transactions with undo/redo, observers,
  expressions, and FreeCAD's geometry and unit types.
- **`ferrocad_widgets`** — reusable `bite-gpui` widgets (window chrome, editable
  field, console text area).
- **`ferrocad_gpui`** — the application shell library (`HostConfig`, `run`).

The public surface is the **`FreeCAD`** Python package.

> **Runtime payload.** The app loads the `FreeCAD` facade, the `ferrocad`
> extension and the workbench scripts from a Python payload. The binary looks for
> it next to the executable or via `FERROCAD_PYTHON_PATH`; the installer/AppImage
> ships it as loose files. It is not embedded in the binary, so `cargo install`
> alone needs the payload supplied separately.

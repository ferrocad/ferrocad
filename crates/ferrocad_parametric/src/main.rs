//! FerroCAD: Parametric — the base app plus the Part workbench.
//!
//! The edition's whole job is the built-in module registration: it links
//! [`ferrocad_part_py`] (which links OCCT) and registers `Part` in the *same
//! image* as the core bindings, so `import Part` and `import FreeCAD` share one
//! document registry and one kernel. Then it delegates to the base app.
//!
//! Run it with `cargo run -p ferrocad_parametric` (needs a display and OCCT).
//! See `docs/repackaging.md` and `docs/occt-integration.md` §4.

fn main() {
    // Register the Part module as a built-in, before the interpreter initialises.
    use ferrocad_part_py::Part as part_module;
    pyo3::append_to_inittab!(part_module);

    ferrocad::run_as("FerroCAD: Parametric");
}

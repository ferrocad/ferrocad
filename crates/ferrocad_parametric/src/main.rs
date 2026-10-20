//! FerroCAD: Parametric — the base app plus the Part workbench.
//!
//! The edition's whole job is composition: it links [`ferrocad_part_py`] (which
//! links OCCT), registers `Part` in the *same image* as the core bindings, and
//! names its own app script (`ferrocad_parametric`), which registers the Part
//! display and dress-up hooks. Then it delegates to the base app's shell.
//!
//! Run it with `cargo run -p ferrocad_parametric` (needs a display and OCCT).
//! See `docs/repackaging.md` and `docs/occt-integration.md` §4.

fn main() {
    // Register the Part module as a built-in, before the interpreter initialises.
    use ferrocad_part_py::Part as part_module;
    pyo3::append_to_inittab!(part_module);

    ferrocad::run_edition("FerroCAD: Parametric", "ferrocad_parametric");
}

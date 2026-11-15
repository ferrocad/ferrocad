//! FerroCAD: Parametric — the base app plus the Part workbench.
//!
//! The edition's whole job is composition: it links [`ferrocad_part_py`] (which
//! links OCCT), registers `Part` in the *same process* as the core bindings, names its
//! own app script (`ferrocad_parametric`) — which registers the Part display and
//! interaction providers — and adds the Part **manipulators** (the edge dress-up arc
//! and the face push/pull stem) to the base contributions. Then it delegates to the
//! base app's shell.
//!
//! The manipulators are the only Rust-side Part code; the shell hosts them through the
//! generic `Manipulator` seam and routes their commits to the app script. The shared
//! translate gizmo comes from [`ferrocad::base_contributions`], so the edition only
//! adds what its workbench owns (and never loses the base triad). A BIM edition
//! would link its own module and contribute no dress-up at all.
//!
//! Run it with `cargo run -p ferrocad_parametric` (needs a display and OCCT).
//!
//! `cargo run -p ferrocad_parametric -- --open model.FCStd` opens a saved model in
//! the shell (the basic `.FCStd` viewer path).
//!
//! `cargo run -p ferrocad_parametric -- --script-host=js` installs the browser's JS page
//! on the desktop **alone** instead of CPython: the same document, a different script
//! half. `js` is a default feature and the default composition is `cpython,js` (CPython
//! primary, the QuickJS page subordinate), so the flag selects a different set rather
//! than enabling an engine. `--script-host=pywasm` (`--features pywasm`) does the same
//! for RustPython, on the 16 MiB worker its compiler needs, and `--script-host=plain`
//! drops the interpreter entirely. A comma-separated list installs a chain —
//! `--script-host=cpython,js` puts the page beside CPython, which stays the primary —
//! with one host per language; see `docs/project/tasks/backend-parity.md` B7–B8.
//!
//! `cargo run -p ferrocad_parametric -- --addons mods/GridOverlay` narrows the boot to
//! that mod: every mod the checkout or payload makes visible loads when the flag is absent,
//! and the named ones are the set when it is given. The same list decides the script halves,
//! what `load_workbenches` scans and which wasm bundles ride along; `--check-init` and
//! `--render` honour it too (see `docs/project/tasks/backend-parity.md` B16).
//!
//! `cargo run -p ferrocad_parametric -- --check-init` boots the interpreter, the
//! linked `Part` module and the payload, then exits without a window (no display,
//! no GPU). Packaging runs it on an unpacked artifact; see
//! `docs/developer/architecture/edition-publishing.md`.
//!
//! `cargo run -p ferrocad_parametric -- --render out.png [--size WxH]` renders the
//! sample Part solid headlessly instead (needs a wgpu adapter, no display); add
//! `--open model.FCStd` to render the file, `--script model.py` to build one, and
//! `--stl out.stl` to export the same tessellation as a binary STL.
//! See `docs/developer/architecture/repackaging.md` and `docs/developer/architecture/occt-integration.md` §4.

/// The `--check-init` payload: assert that all four modules share one kernel and
/// exercise a document by building and closing a `Part::Feature`.
const CHECK_INIT_SCRIPT: &str = r#"
import FreeCAD, Part, Mesh, MeshPart
assert FreeCAD._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id, (
    "the modules do not share one kernel: %r != %r != %r != %r"
    % (FreeCAD._kernel_id, Part._kernel_id, Mesh._kernel_id, MeshPart._kernel_id)
)
name = "__ferrocad_check_init__"
if FreeCAD.getDocument(name) is not None:
    FreeCAD.closeDocument(name)
doc = FreeCAD.newDocument(name)
boxed = doc.addObject("Part::Feature", "Box")
boxed.Shape = Part.makeBox(1, 1, 1)
assert boxed.Shape.ShapeType == "Solid", boxed.Shape.ShapeType
FreeCAD.closeDocument(name)
"#;

/// The edition's shell contributions: Part's two manipulators and the polyline tools.
///
/// Factored out of `main` so the test can inspect the registry the window would use.
fn contributions() -> ferrocad::Contributions {
    ferrocad::base_contributions()
        .with_manipulator(ferrocad_part_gui::fillet_manipulator)
        .with_manipulator(ferrocad_part_gui::push_pull_manipulator)
        // `Part_Polyline` starts the wire tool; `Sketch_Polyline` the sketch one. They
        // share the stage's state machine and differ only in the id their commit carries.
        .with_tool("polyline", || {
            Box::new(ferrocad_stage::TaggedPolyline::new("polyline"))
                as Box<dyn ferrocad_stage::Tool>
        })
        .with_tool("sketch_polyline", || {
            Box::new(ferrocad_stage::TaggedPolyline::new("sketch_polyline"))
                as Box<dyn ferrocad_stage::Tool>
        })
}

fn main() {
    // Register the workbench modules as built-ins, before the interpreter
    // initialises. All of them must share this process: `MeshPart` builds its wires
    // through `Part.makePolygon`, and `Mesh::Feature` lives in the same document
    // registry as the Part objects. The combined-extension crate owns that list, so
    // this edition and the packaged extension cannot drift apart.
    ferrocad_combined_py::register_builtins();

    // Editions parse their own flags, so the base app's `--check-init` branch does
    // not reach them. Parse them here first: the check has to see the same selection
    // (`--addons`) and script halves the window would, or it reports a boot nobody
    // asked for.
    let args = match ferrocad::edition_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ferrocad: {error}");
            std::process::exit(2);
        }
    };

    // Run the same boot here, then import the edition's app script: it is the module
    // that imports `Part` and registers the display hooks, so this is what proves the
    // kernel and the payload work together. `GuiUp` is 0 (the same stub the base check
    // uses), so no window or GPU is involved.
    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        let result = ferrocad::check_init_args(&args).and_then(|()| {
            pyo3::Python::with_gil(|py| -> pyo3::PyResult<()> {
                py.import("ferrocad_parametric")?;
                let script = std::ffi::CString::new(CHECK_INIT_SCRIPT)
                    .expect("the check-init script has no NUL");
                let globals = pyo3::types::PyDict::new(py);
                py.run(&script, Some(&globals), None)?;
                Ok(())
            })
            .map_err(|error| error.to_string())
        });
        match result {
            Ok(()) => println!("ferrocad: init OK"),
            Err(error) => {
                eprintln!("ferrocad: init failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    let contributions = contributions();

    if args.render.is_some() || args.stl.is_some() {
        match ferrocad::render_edition_headless(
            "FerroCAD: Parametric",
            "ferrocad_parametric",
            &args,
        ) {
            Ok(report) => {
                println!(
                    "ferrocad: {} object(s) [{}] from {}",
                    report.objects.len(),
                    report.objects.join(", "),
                    report.document
                );
                if let (Some(out), Some(size)) = (args.render.as_ref(), report.size) {
                    println!(
                        "ferrocad: wrote PNG {} ({}x{})",
                        out.display(),
                        size[0],
                        size[1]
                    );
                }
                if let Some(stl) = &report.stl {
                    println!(
                        "ferrocad: wrote STL {} ({} triangles)",
                        stl.path.display(),
                        stl.triangles
                    );
                }
            }
            Err(error) => {
                eprintln!("ferrocad: headless run failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    ferrocad::run_edition_with_script(
        "FerroCAD: Parametric",
        "ferrocad_parametric",
        contributions,
        args.open,
        args.script_hosts,
        args.addons,
    );
}

#[cfg(test)]
mod tests {
    /// The registry the window installs: the base tools survive, and each command's
    /// `polyline` id is the edition's own.
    #[test]
    fn the_edition_registers_the_polyline_tools() {
        let contributions = super::contributions();
        assert!(
            contributions.tool_for("polyline").is_some(),
            "Part_Polyline arms the wire tool"
        );
        assert!(
            contributions.tool_for("sketch_polyline").is_some(),
            "Sketch_Polyline arms the sketch tool"
        );
        assert!(
            contributions.tool_for("workplane_points").is_some(),
            "the shared workplane pick is kept"
        );
    }
}

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

fn main() {
    // Register the workbench modules as built-ins, before the interpreter
    // initialises. All of them must share this process: `MeshPart` builds its wires
    // through `Part.makePolygon`, and `Mesh::Feature` lives in the same document
    // registry as the Part objects. The combined-extension crate owns that list, so
    // this edition and the packaged extension cannot drift apart.
    ferrocad_combined_py::register_builtins();

    // Editions parse their own flags, so the base app's `--check-init` branch does
    // not reach them. Run the same boot here, then import the edition's app script:
    // it is the module that imports `Part` and registers the display hooks, so this
    // is what proves the kernel and the payload work together. `GuiUp` is 0 (the
    // same stub the base check uses), so no window or GPU is involved.
    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        let result = ferrocad::check_init().and_then(|()| {
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

    let args = match ferrocad::edition_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ferrocad: {error}");
            std::process::exit(2);
        }
    };

    let contributions = ferrocad::base_contributions()
        .with_manipulator(ferrocad_part_gui::fillet_manipulator)
        .with_manipulator(ferrocad_part_gui::push_pull_manipulator);

    if args.render.is_some() || args.stl.is_some() {
        match ferrocad::render_edition_headless(
            "FerroCAD: Parametric",
            "ferrocad_parametric",
            args.render.as_deref(),
            args.stl.as_deref(),
            args.size,
            args.script.as_deref(),
            args.open.as_deref(),
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

    ferrocad::run_edition_with_document(
        "FerroCAD: Parametric",
        "ferrocad_parametric",
        contributions,
        args.open,
    );
}

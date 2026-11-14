//! FerroCAD: Sculpt — the base app plus the Sculpt workbench.
//!
//! The edition's whole job is composition, and it is deliberately the same
//! composition as [`ferrocad_parametric`]: it registers `Part`, `Mesh` and
//! `MeshPart` in the *same process* as the core bindings, names its own app script
//! (`ferrocad_sculpt`) — which re-exports the Parametric script's display and
//! interaction providers, registers the `Sculpt_*` commands and seeds the cage
//! sample — and then delegates to the base app's shell.
//!
//! A `Part::FeaturePython` skin *is* a Part object, so it needs exactly the Part
//! display hooks; re-exporting the Parametric script is what keeps one definition
//! of them instead of two that can drift.
//!
//! The edition contributes **no manipulator of its own yet**: the cage
//! point/edge drag is the next piece, and this is where it will be added —
//! `ferrocad::base_contributions().with_manipulator(…)` — the same way the
//! Parametric edition contributes the edge dress-up and the push/pull stem.
//!
//! Run it with `cargo run -p ferrocad_sculpt` (needs a display and OCCT).
//!
//! `cargo run -p ferrocad_sculpt -- --check-init` boots the interpreter, the
//! linked modules and the payload, builds a cage → skin → body document and runs
//! no window (no display, no GPU). Packaging runs it on an unpacked artifact; see
//! `docs/developer/architecture/edition-publishing.md`.
//!
//! The other flags are the base app's: `--open model.FCStd`, `--script-host=…`,
//! `--addons …`, `--render out.png` and `--stl out.stl`.

/// The `--check-init` payload: one kernel across the modules, the bundle's
/// workflow in a document, and the command registry populated.
const CHECK_INIT_SCRIPT: &str = r#"
import FreeCAD, FreeCADGui, Part, Mesh, MeshPart, Sculpt
assert FreeCAD._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id, (
    "the modules do not share one kernel: %r != %r != %r != %r"
    % (FreeCAD._kernel_id, Part._kernel_id, Mesh._kernel_id, MeshPart._kernel_id)
)
name = "__ferrocad_sculpt_check__"
if FreeCAD.getDocument(name) is not None:
    FreeCAD.closeDocument(name)
doc = FreeCAD.newDocument(name)
points, faces = Sculpt.box_cage(10, 10, 10)
cage = Sculpt.makeCage(doc, "Cage", points, faces, levels=1)
skin = Sculpt.makeSkin(doc, "Skin", cage)
body = doc.addObject("PartDesign::Body", "Body")
roles = Sculpt.nest(body, cage, skin)
doc.recompute()
assert roles == ["support", "base"], roles
assert body.Shape.ShapeType == "Solid", body.Shape.ShapeType
assert body.Shape.isValid(), body.getStatusString()
assert body.Shape.Volume > 0.0, body.Shape.Volume
commands = FreeCADGui.listCommands()
assert "Sculpt_MakeCage" in commands, [name for name in commands if name.startswith("Sculpt_")]
assert "Part_Box" in commands, "the Parametric command set did not arrive"
FreeCAD.closeDocument(name)
"#;

fn main() {
    // Register the workbench modules as built-ins, before the interpreter
    // initialises. They must share this process: the cage is a `Mesh` object, the
    // skin a Part one, and the body that nests them lives in the same document
    // registry.
    ferrocad_combined_py::register_builtins();

    // Editions parse their own flags, so the base app's `--check-init` branch does
    // not reach them. Parse them here first: the check must see the same selection
    // (`--addons`) and script halves the window would.
    let args = match ferrocad::edition_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ferrocad: {error}");
            std::process::exit(2);
        }
    };

    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        let result = ferrocad::check_init_args(&args).and_then(|()| {
            pyo3::Python::with_gil(|py| -> pyo3::PyResult<()> {
                py.import("ferrocad_sculpt")?;
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

    // The cage manipulator will be contributed here (see the crate doc); until
    // then the edition is the base contributions and the script surface.
    let contributions = ferrocad::base_contributions();

    if args.render.is_some() || args.stl.is_some() {
        match ferrocad::render_edition_headless("FerroCAD: Sculpt", "ferrocad_sculpt", &args) {
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
        "FerroCAD: Sculpt",
        "ferrocad_sculpt",
        contributions,
        args.open,
        args.script_hosts,
        args.addons,
    );
}

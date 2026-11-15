//! FerroCAD: SimpleCAD — the base app plus the canvas-first tools.
//!
//! The edition's whole job is composition, and it is deliberately the same
//! composition as [`ferrocad_parametric`] and [`ferrocad_sculpt`]: it registers
//! `Part`, `Mesh` and `MeshPart` in the *same process* as the core bindings, names
//! its own app script (`ferrocad_simplecad`) — which re-exports the Parametric
//! display and interaction providers and registers the `SimpleCAD_*` commands —
//! and then delegates to the base app's shell.
//!
//! A SimpleCAD face *is* a `Part::Feature`, so it needs exactly the Part display
//! hooks; re-exporting the Parametric script is what keeps one definition of them.
//!
//! **What this edition contributes of its own** is the first native tool: the
//! two-click rectangle ([`ferrocad_stage::RectangleTool`]). It is a stage tool, not
//! a PySide one, so it draws its own preview on the GPUI viewport, completes itself
//! on the second press, and commits `rectangle/create` with the four corners; the
//! app script turns that into a face through SimpleCAD's own geometry
//! (`simplecad.core.geometry.rectangle`). That is the shape every later tool takes:
//! *the tool collects world geometry, the script names the object.*
//!
//! Run it with `cargo run -p ferrocad_simplecad` (needs a display and OCCT).
//!
//! `cargo run -p ferrocad_simplecad -- --check-init` boots the interpreter, the
//! linked modules and the payload, then draws one rectangle through the tool and
//! checks the face it made — no window, no display, no GPU.
//!
//! The other flags are the base app's: `--open model.FCStd`, `--script-host=…`,
//! `--addons …`, `--render out.png` and `--stl out.stl`.

use ferrocad_stage::{MouseButton, PointerEvent, PointerKind, Snap, SnapKind};
use pyo3::prelude::*;

/// The `--check-init` payload: one kernel across the modules, one shape from each of
/// the four drawing tools through the real registry and the real script hook, and the
/// four commands registered.
const CHECK_INIT_SCRIPT: &str = r#"
import math
import FreeCAD, FreeCADGui, Part, Mesh, MeshPart
from ferrocad_check_tool import draw
assert FreeCAD._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id, (
    "the modules do not share one kernel: %r != %r != %r != %r"
    % (FreeCAD._kernel_id, Part._kernel_id, Mesh._kernel_id, MeshPart._kernel_id)
)
missing = [
    command for command in
    ("SimpleCAD_Rectangle", "SimpleCAD_Circle", "SimpleCAD_Polygon", "SimpleCAD_Arc")
    if command not in FreeCADGui.listCommands()
]
assert not missing, missing
name = "__ferrocad_simplecad_check__"
if FreeCAD.getDocument(name) is not None:
    FreeCAD.closeDocument(name)
FreeCAD.newDocument(name)

face = draw("rectangle", [(0, 0, 0), (10, 4, 0)])
assert face.ShapeType == "Face", face.ShapeType
assert face.isValid(), "the rectangle is not a valid face"
assert abs(face.Area - 40.0) < 1e-6, face.Area
assert abs(face.BoundBox.XLength - 10.0) < 1e-6, face.BoundBox
assert abs(face.BoundBox.YLength - 4.0) < 1e-6, face.BoundBox

circle = draw("circle", [(0, 0, 0), (5, 0, 0)])
assert circle.ShapeType == "Face", circle.ShapeType
assert abs(circle.Area - 25.0 * math.pi) < 1e-6, circle.Area

# The default polygon has six sides at radius 10.
polygon = draw("polygon", [(0, 0, 0), (10, 0, 0)])
assert polygon.ShapeType == "Face", polygon.ShapeType
assert abs(polygon.Area - 300.0 * math.sin(math.pi / 3.0)) < 1e-6, polygon.Area

arc = draw("arc", [(0, 0, 0), (10, 0, 0), (0, 10, 0)])
assert arc.ShapeType == "Edge", arc.ShapeType
assert abs(arc.Length - 5.0 * math.pi) < 1e-6, arc.Length

objects = sorted(obj.Name for obj in FreeCAD.ActiveDocument.Objects)
assert len(objects) == 4, objects
FreeCAD.closeDocument(name)
"#;

fn main() {
    // Register the workbench modules as built-ins, before the interpreter
    // initialises. They must share this process: a face is a `Part` object and the
    // display hook that draws it is the Parametric one.
    ferrocad_combined_py::register_builtins();

    // Editions parse their own flags, so the base app's `--check-init` branch does
    // not reach them.
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
                py.import("ferrocad_simplecad")?;
                // The tools are Rust, so the check script reaches them as a small
                // module: `draw(id, points)` presses the world points through
                // `Tool::on_event`, finishes through `Tool::finish`, and hands the
                // commit to the real `interaction` hook. Nothing is reimplemented.
                let module = pyo3::types::PyModule::new(py, "ferrocad_check_tool")?;
                module.add_function(wrap_pyfunction!(draw, &module)?)?;
                py.import("sys")?
                    .getattr("modules")?
                    .set_item("ferrocad_check_tool", module)?;
                let script = std::ffi::CString::new(CHECK_INIT_SCRIPT)
                    .expect("the check-init script has no NUL");
                py.run(&script, None, None)?;
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

    // The rectangle tool joins the base contributions (see `lib.rs`); the shell hosts
    // it through the generic `Tool` seam and routes its commit to the app script.
    let contributions = ferrocad_simplecad::contributions();

    if args.render.is_some() || args.stl.is_some() {
        match ferrocad::render_edition_headless("FerroCAD: SimpleCAD", "ferrocad_simplecad", &args) {
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
        "FerroCAD: SimpleCAD",
        "ferrocad_simplecad",
        contributions,
        args.open,
        args.script_hosts,
        args.addons,
    );
}

/// `--check-init`'s window on a drawing tool: the pointer's moves and presses, the
/// commit, the script's `interaction` hook, and the new object's `Shape`.
///
/// The tool comes from the edition's own registry, and this is the sequence the
/// viewport drives — `Tool::on_event` for a move and a press, `Tool::finish` for the
/// commit, the `interaction` entry for the edit — so the check proves the shipped tool
/// rather than a parallel implementation of it. Each point is moved to before it is
/// pressed, because a tool that measures a sweep (the arc) reads it from the pointer's
/// motion.
#[pyfunction]
fn draw(py: Python<'_>, tool: &str, points: Vec<(f32, f32, f32)>) -> PyResult<PyObject> {
    let mut session = ferrocad_simplecad::contributions()
        .tool_for(tool)
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("no such tool {tool}"))
        })?;
    for (x, y, z) in points {
        let point = [x, y, z];
        let snap = Some(Snap {
            point,
            kind: SnapKind::Point,
            local: None,
            major: false,
        });
        for kind in [PointerKind::Move, PointerKind::Down] {
            let event = PointerEvent {
                kind,
                position: [0, 0],
                button: (kind == PointerKind::Down).then_some(MouseButton::Left),
                hit: None,
                projection: None,
            };
            session.on_event(&event, snap);
        }
    }
    let commit = session
        .finish()
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err(format!("the {tool} tool committed nothing")))?;
    // The shell sends `values` as JSON text, so the check does too.
    let values = format!(
        "[{}]",
        commit
            .values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let answer = py.import("ferrocad_simplecad")?.getattr("interaction")?.call1((
        "",
        commit.manipulator.as_str(),
        commit.action.as_str(),
        "",
        values,
    ))?;
    // The shell entry answers JSON text, exactly as it does for the Rust host.
    let raw: String = answer.extract()?;
    let answer = py.import("json")?.call_method1("loads", (raw,))?;
    if answer.is_none() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
            "no provider handled {}/{}",
            commit.manipulator, commit.action
        )));
    }
    let ok: bool = answer.get_item("ok")?.extract()?;
    if !ok {
        let error: String = answer
            .get_item("error")
            .and_then(|error| error.extract())
            .unwrap_or_else(|_| format!("the script refused the {tool}"));
        return Err(pyo3::exceptions::PyRuntimeError::new_err(error));
    }
    let name: String = answer.get_item("feature")?.extract()?;
    let document = py.import("FreeCAD")?.getattr("ActiveDocument")?;
    let made = document.call_method1("getObject", (name,))?;
    Ok(made.getattr("Shape")?.unbind())
}

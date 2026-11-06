//! Temporary measurement: where a commit's stage sync spends its time.
//!
//! Two backends over one registry: the composed desktop backend (whose
//! `stage_meshes` goes to the edition's display hook, through the interpreter) and
//! the document half alone (which tessellates the same document in Rust). The
//! difference is the cost of the display hook.
//!
//! ```sh
//! cargo run -p ferrocad_parametric --example stage_timing -- /path/to/model.FCStd
//! ```

use std::time::Instant;

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_python::{PythonScriptHost, python};
use ferrocad_shell_rust::RustAppBackend;
use ferrocad_stage::InteractionCommit;

fn stage(backend: &RustAppBackend) -> Result<(usize, usize), String> {
    let meshes = backend.stage_meshes()?;
    Ok((
        meshes.len(),
        meshes.iter().map(|mesh| mesh.triangles.len() / 3).sum(),
    ))
}

fn time(label: &str, backend: &RustAppBackend) {
    let started = Instant::now();
    match stage(backend) {
        Ok((items, triangles)) => eprintln!(
            "{label:<34} {:>10.1} ms  ({items} items, {triangles} triangles)",
            started.elapsed().as_secs_f64() * 1000.0
        ),
        Err(error) => eprintln!("{label:<34} failed: {error}"),
    }
}

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1);

    ferrocad_combined_py::register_builtins();
    python::configure("ferrocad_parametric", &[], &[], false);

    // The desktop: the document half with the interpreter as its script half.
    let mut over_python =
        RustAppBackend::from_shared_registry().with_script_host(Box::new(PythonScriptHost));
    let boot = over_python.bootstrap()?;
    eprintln!("boot {} ({})", boot.document, boot.backend);
    if let Some(path) = &path {
        let opened = over_python.open_path(std::path::Path::new(path))?;
        eprintln!("open {} objects", opened.objects);
    }
    // The same registry, no script half: `stage_meshes` tessellates the document.
    let over_document = RustAppBackend::from_shared_registry();

    // Each half twice in a row: the second call of a pair is the memo hit, and the
    // halves use different deflections, so alternating them would measure the
    // single-entry cache thrashing rather than the sync.
    eprintln!("--- before the move");
    time("display hook 1", &over_python);
    time("display hook 2 (memo)", &over_python);
    time("document 1", &over_document);
    time("document 2 (memo)", &over_document);

    // The commit path: the shared transform gizmo, applied by the document half to
    // the first object the stage draws.
    let before_move: Vec<StageMesh> = over_python.stage_meshes()?;
    let Some(name) = before_move.iter().map(|mesh| mesh.object.clone()).next() else {
        return Ok(());
    };
    let mut matrix = [0.0f64; 16];
    for index in 0..4 {
        matrix[index * 5] = 1.0;
    }
    matrix[3] = 10.0;
    let commit =
        InteractionCommit::new(ferrocad_stage::TRANSFORM_ID, 0, "apply").with_values(matrix);
    let started = Instant::now();
    let applied = over_python.apply_interaction(&commit, &name)?;
    eprintln!(
        "--- move {name}: ok={} in {:.1} ms",
        applied.ok,
        started.elapsed().as_secs_f64() * 1000.0
    );
    eprintln!("--- after the move (what `stage sync` pays)");
    time("display hook 1", &over_python);
    time("display hook 2 (memo)", &over_python);
    time("document 1", &over_document);
    time("document 2 (memo)", &over_document);

    // What a rigid move changes in the rows a stage is built from. `set_stage` patches
    // packets instead of re-emitting only when nothing but the transform moved, so these
    // two lines are the precondition for that path (`origin` comes from the placement
    // base, so it moves with it; the question is whether the vertices do too).
    let after_move = over_python.stage_meshes()?;
    let row = |rows: &Vec<StageMesh>, object: &str| -> Option<StageMesh> {
        rows.iter().find(|row| row.object == object).cloned()
    };
    let report = |object: &str| {
        match (
            row(&before_move, object),
            row(&after_move, object),
        ) {
            (Some(before), Some(after)) => eprintln!(
                "row {object:<32} positions {} ({})   origin {:?} -> {:?}",
                if before.positions == after.positions {
                    "identical"
                } else {
                    "CHANGED"
                },
                before.positions.len(),
                before.origin,
                after.origin
            ),
            _ => eprintln!("row {object:<32} missing"),
        }
    };
    eprintln!("--- what the move changed");
    report(&name);
    if let Some(other) = before_move
        .iter()
        .map(|mesh| mesh.object.clone())
        .find(|object| *object != name)
    {
        report(&other);
    }
    Ok(())
}

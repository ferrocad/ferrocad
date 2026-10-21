//! Regression: a booted edition can load the authored grid-overlay mods.
//!
//! The defect this guards was a **stale binary**: the source registered the
//! shader classes, but the binary the user ran predated that work, so `pivy.coin`
//! had no `SoVertexShader` and the Coin mod failed at import. This test boots the
//! same interpreter the toy boots (headless, no window), loads the mods from
//! `mods/`, and compiles the scene the Coin mod registers.

use std::path::PathBuf;
use std::sync::Once;

use ferrocad_gpui::python;

static BOOT: Once = Once::new();

/// The mods directory beside the workspace's `python/` tree.
fn mods_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
}

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        // Mirror the toy binary: GUI on, the workspace `mods/` scanned at boot.
        python::configure("ferrocad_toy", &[], &[mods_dir()], true);
    });
    python::bootstrap().expect("boot the toy app");
}

fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

/// The shader/state classes the Coin mod builds its tree from.
const CLASSES: &[&str] = &[
    "SoVertexShader",
    "SoFragmentShader",
    "SoShaderProgram",
    "SoShaderParameter1f",
    "SoShaderParameter3f",
    "SoShaderParameterArray1f",
    "SoMFFloat",
    "SoMFNode",
    "SoFaceSet",
    "SoPickStyle",
    "SoLightModel",
    "SoDepthBuffer",
];

#[test]
fn the_coin_and_semantic_grid_mods_load_and_register_an_overlay() {
    boot();

    // 1. The shader class surface the Coin mod builds must exist in `pivy.coin`.
    //    A stale/missing registration fails exactly here.
    let output = evaluate(&format!(
        "import pivy.coin as c\n\
         missing = [n for n in {CLASSES:?} if not hasattr(c, n)]\n\
         print(missing)"
    ));
    assert_eq!(output, "[]", "pivy.coin is missing {output}");

    // 2. Open a document before the mods load, as the shell's boot order does.
    evaluate("FreeCAD.newDocument('GridOverlayTest')");

    // 3. Both authored mods load without raising (the loader records, never throws).
    let report = python::load_workbenches().expect("load workbenches");
    assert!(
        report.errors.is_empty(),
        "workbench errors: {:?}",
        report.errors
    );
    for script in [
        "GridOverlay/Init.py",
        "GridOverlay/InitGui.py",
        "GridOverlayCoin/Init.py",
        "GridOverlayCoin/InitGui.py",
    ] {
        assert!(
            report.loaded.iter().any(|loaded| loaded == script),
            "{script} did not load: {:?}",
            report.loaded
        );
    }

    // ...and each registers its command, which also runs without raising.
    let commands = python::commands().expect("commands");
    for id in ["GridOverlay_Settings", "GridOverlayCoin_Settings"] {
        assert!(
            commands.iter().any(|command| command.id == id),
            "{id} not registered: {:?}",
            commands
                .iter()
                .map(|command| command.id.as_str())
                .collect::<Vec<_>>()
        );
        let outcome = python::run_command(id).expect("run command");
        assert!(outcome.ok, "{id} raised: {}", outcome.error);
    }

    // 4. The Coin mod built its grid and registered it through the pull bridge.
    let scenes = ferrocad_inventor_py::overlay_scenes();
    assert!(!scenes.is_empty(), "the Coin mod registered no overlay");

    // 5. Compiling the registered scene yields the program packet the viewport draws.
    let mut program_packets = 0;
    for scene in &scenes {
        let (queue, _directives, error) = ferrocad_inventor::compile_diagnostic(
            scene,
            &ferrocad_inventor::Viewport::default(),
            None,
        );
        assert!(
            error.is_none(),
            "the overlay program failed to compile: {error:?}"
        );
        program_packets += queue
            .iter()
            .filter(|packet| packet.state.program.is_some())
            .count();
    }
    assert!(
        program_packets > 0,
        "the registered overlay compiled to no program packet"
    );
}

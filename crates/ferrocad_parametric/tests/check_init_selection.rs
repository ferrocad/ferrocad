//! `--check-init` checks the boot the command line asked for.
//!
//! The edition binaries parse their flags after the `--check-init` branch, so the
//! diagnostic used to boot the *default* composition: a packager running
//! `--check-init --addons mods/GridOverlay` was told about every mod and the `js` half
//! the directory derived, neither of which the run it was checking would install. That
//! is a process-boundary bug (argument order in `main`), so this test runs the real
//! binary rather than calling the library.
//!
//! Its own test binary: the two runs are child processes, but a future assertion may
//! want the in-process boot, and `python::configure` is once-per-process.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The one package the selection names.
fn grid_overlay() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
        .join("GridOverlay")
}

fn check_init(extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferrocad_parametric"));
    command.arg("--check-init").args(extra);
    // B17a: the user's refusals narrow the boot, so a developer who disabled a mod in the
    // About card would otherwise change what this test boots. A path that cannot exist is
    // the defaults, which is the checked-in state.
    command.env(
        "FERROCAD_PREFS_FILE",
        std::env::temp_dir()
            .join("ferrocad-check-init-test")
            .join("prefs.json"),
    );
    command.output().expect("the edition binary runs")
}

/// The `workbenches: loaded N script(s)` count, or a failure that quotes the run.
fn loaded_count(output: &Output) -> usize {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "check-init failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|line| line.contains("workbenches: loaded"))
        .unwrap_or_else(|| {
            panic!("no workbench count\n--- stdout\n{stdout}\n--- stderr\n{stderr}")
        });
    line.split("loaded ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("unreadable count in {line:?}"))
}

#[test]
fn check_init_selects_the_mod_set() {
    // The selection is exactly the named package: two scripts (its `Init.py` and
    // `InitGui.py`), one language, so nothing derives the `js` half.
    let selected = check_init(&["--addons", &grid_overlay().display().to_string()]);
    assert_eq!(
        loaded_count(&selected),
        2,
        "the flag narrows the diagnostic's boot"
    );
    let stdout = String::from_utf8_lossy(&selected.stdout);
    assert!(
        stdout.contains("provider grid: ferrocad"),
        "the selected mod registered its provider\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&selected.stderr);
    assert!(
        !stderr.contains("installing cpython, js"),
        "a selection must not install a host for a mod it did not select\n{stderr}"
    );

    // Without the flag every mod is the set: the same boot that has always run.
    let all = check_init(&[]);
    let count = loaded_count(&all);
    assert!(
        count > 2,
        "the default is every mod, got {count}\n{}",
        String::from_utf8_lossy(&all.stdout)
    );
}

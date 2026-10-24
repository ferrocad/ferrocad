//! `--open`: the shell loads a saved document before reading its model tree.
//!
//! This is the basic viewer path. It needs no window and no GPU: the same
//! `Shell::boot` the windowed app uses opens the file, drops the bootstrap
//! sample, and lists the file's objects. (`ferrocad_parametric` then displays the
//! Part shapes through its display provider; the toy edition proves the plumbing
//! without OCCT.)

use std::sync::Once;

use ferrocad_gpui::Shell;
use ferrocad_gpui::python;

static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        python::configure("ferrocad_toy", &[], &[], false);
    });
}

#[test]
fn the_shell_opens_a_saved_document_and_lists_its_objects() {
    boot();

    let path = std::env::temp_dir().join("ferrocad_open_document_test.FCStd");
    let _ = std::fs::remove_file(&path);
    let path_text = path.to_string_lossy().into_owned();

    // Build the sample toy document and save it, as the demo scripts do.
    python::bootstrap().expect("bootstrap the toy app");
    let saved = python::evaluate(&format!("doc.saveAs({path_text:?})")).expect("save");
    assert!(saved.ok, "saveAs failed: {}", saved.output);

    // The next boot is what `ferrocad_toy --open <file>` does.
    python::configure_startup_document(Some(&path));
    let model = Shell::boot().expect("boot with a startup document");
    let tree = model.lock().unwrap().tree.clone();

    let opened = tree
        .iter()
        .find(|doc| doc.name == "ferrocad_open_document_test")
        .unwrap_or_else(|| panic!("the opened document is in the tree: {tree:?}"));
    assert!(
        opened.objects.iter().any(|object| object.name == "Slab"),
        "the saved slab is listed: {:?}",
        opened.objects
    );
    assert!(
        !tree.iter().any(|doc| doc.name == "Toy"),
        "the bootstrap sample is closed: {tree:?}"
    );

    let _ = std::fs::remove_file(&path);
}

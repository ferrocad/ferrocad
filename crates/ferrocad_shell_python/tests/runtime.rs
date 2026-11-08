//! `--script-host=` compositions, without a window.
//!
//! The switch is only worth having if each name composes what it says, so these test the
//! composition rather than the parser (that lives in `ferrocad_shell_api`).

use ferrocad_shell_api::AppBackend;
use ferrocad_shell_python::{Runtime, compose};

#[test]
fn plain_is_the_document_half_alone() {
    let backend = compose(Runtime::Plain).expect("always compilable");
    assert!(backend.script_host().is_none());
}

#[test]
fn cpython_is_the_default_composition() {
    let backend = compose(Runtime::Cpython).expect("the edition's engine");
    assert!(backend.script_host().is_some());
}

#[cfg(not(feature = "pywasm"))]
#[test]
fn pywasm_without_the_feature_says_how_to_get_it() {
    let error = compose(Runtime::Pywasm).err().expect("the feature is off");
    assert!(error.contains("--features pywasm"), "{error}");
}

#[cfg(not(feature = "addons"))]
#[test]
fn addons_without_the_feature_say_how_to_get_them() {
    let error = ferrocad_shell_python::compose_with(
        Runtime::Plain,
        &[std::path::PathBuf::from("mods")],
    )
    .err()
    .expect("the feature is off");
    assert!(error.contains("--features addons"), "{error}");
}

#[cfg(feature = "addons")]
#[test]
fn an_addons_directory_with_no_bundles_leaves_the_script_half_alone() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let backend = ferrocad_shell_python::compose_with(Runtime::Cpython, &[temp.path().to_path_buf()])
        .expect("nothing to load is not an error");
    assert!(
        backend.script_host().is_some(),
        "the primary survives an empty addon directory"
    );
}

#[cfg(feature = "addons")]
#[test]
fn plain_plus_addons_still_has_a_document_half() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let backend = ferrocad_shell_python::compose_with(Runtime::Plain, &[temp.path().to_path_buf()])
        .expect("composes");
    assert!(backend.script_host().is_none(), "no bundle, no script half");
}

#[cfg(feature = "pywasm")]
#[test]
fn pywasm_is_the_composition_with_rustpython_behind_it() {
    let backend = compose(Runtime::Pywasm).expect("the interpreter boots on its worker");
    let script = backend.script_host().expect("the script half is installed");
    // The greeting is the proof that this is the interpreter and not the document half.
    let hello = script.hello().expect("a greeting").expect("one, not silence");
    assert!(hello.contains("RustPython"), "{hello}");
}

#[cfg(not(feature = "js"))]
#[test]
fn js_without_the_feature_says_how_to_get_it() {
    let error = compose(Runtime::Js).err().expect("the feature is off");
    assert!(error.contains("--features js"), "{error}");
}

#[cfg(feature = "js")]
#[test]
fn js_is_the_composition_with_the_page_behind_it() {
    // The page boots over `mods/`, which a test binary need not have; an empty page still
    // composes, and the page's own suite proves the module loading.
    let backend = compose(Runtime::Js).expect("the page composes");
    assert!(backend.script_host().is_some());
}

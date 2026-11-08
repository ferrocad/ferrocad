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

#[test]
fn pywasm_says_what_is_still_missing() {
    // Not wired on the desktop yet: a wrong answer here is better than a silent
    // fallback that looks like it worked.
    let error = compose(Runtime::Pywasm).err().expect("not wired yet");
    assert!(error.contains("pywasm"), "{error}");
    assert!(error.contains("worker"), "{error}");
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

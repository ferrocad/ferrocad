//! `--script-host=` compositions, without a window.
//!
//! The switch is only worth having if each name composes what it says, so these test the
//! composition rather than the parser (that lives in `ferrocad_shell_api`).

use ferrocad_shell_python::{Runtime, compose, compose_all};

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

#[test]
fn two_python_engines_are_refused_before_any_host_boots() {
    // B8c: the rule is applied at composition as well as at the parser, so a caller that
    // builds the list by hand cannot install two interpreters for one language.
    let error = compose_all(&[Runtime::Cpython, Runtime::Pywasm], &[])
        .err()
        .expect("two Python engines are one host too many");
    assert!(error.contains("both python"), "{error}");
}

#[test]
fn plain_cannot_be_combined_with_a_host() {
    let error = compose_all(&[Runtime::Plain, Runtime::Cpython], &[])
        .err()
        .expect("plain is the absence of a host");
    assert!(error.contains("plain"), "{error}");
}

#[cfg(all(feature = "addons", feature = "js"))]
#[test]
fn two_hosts_compose_into_one_script_host() {
    // CPython as the primary, the JS page behind it. What this can prove without a
    // configured interpreter is that the chain composes and reaches the backend as one
    // `ScriptHost`; the merge order and the fan-out are `ferrocad_shell_wasm`'s tests,
    // which drive fixtures instead of an interpreter.
    let backend = compose_all(&[Runtime::Cpython, Runtime::Js], &[]).expect("a chain composes");
    assert!(backend.script_host().is_some());
}

#[cfg(all(not(feature = "addons"), feature = "js"))]
#[test]
fn two_hosts_need_the_addons_feature_today() {
    // The chain lives in `ferrocad_shell_wasm`, which is the `addons` feature; the error
    // says so rather than silently installing only the first half.
    let error = compose_all(&[Runtime::Cpython, Runtime::Js], &[])
        .err()
        .expect("no chain without the feature");
    assert!(error.contains("addons"), "{error}");
}

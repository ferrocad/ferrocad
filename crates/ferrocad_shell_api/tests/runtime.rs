//! One name for a scripting half, whichever host asks.
//!
//! The point of these is that the browser's `?runtime=` and the desktop's
//! `--script-host=` cannot drift: the aliases, the canonical spellings and the error
//! text all come from one table.

use ferrocad_shell_api::Runtime;

#[test]
fn the_canonical_names_are_the_switch_names() {
    for runtime in Runtime::ALL {
        assert_eq!(Runtime::from_name(runtime.id()), Ok(runtime));
        assert_eq!(runtime.to_string(), runtime.id());
    }
}

#[test]
fn the_aliases_a_hand_written_url_uses_are_accepted() {
    assert_eq!(Runtime::from_name("none"), Ok(Runtime::Plain));
    assert_eq!(Runtime::from_name("rust"), Ok(Runtime::Plain));
    assert_eq!(Runtime::from_name("javascript"), Ok(Runtime::Js));
    assert_eq!(Runtime::from_name("rustpython"), Ok(Runtime::Pywasm));
}

#[test]
fn python_is_the_python_this_host_has() {
    // `python` names the interpreter a host actually links, so the same word means the
    // same thing to a user on either side of the wasm boundary.
    let expected = if cfg!(target_family = "wasm") {
        Runtime::Pywasm
    } else {
        Runtime::Cpython
    };
    assert_eq!(Runtime::from_name("python"), Ok(expected));
    assert_eq!(Runtime::from_name("py"), Ok(expected));
}

#[test]
fn an_unknown_name_says_what_the_names_are() {
    let error = Runtime::from_name("coffee").unwrap_err();
    assert!(error.contains("coffee"), "{error}");
    for name in ["plain", "cpython", "pywasm", "js"] {
        assert!(error.contains(name), "{error} misses {name}");
    }
}

#[test]
fn only_cpython_is_gated_by_the_target() {
    assert_eq!(
        Runtime::Cpython.on_this_target(),
        !cfg!(target_family = "wasm")
    );
    for runtime in [Runtime::Plain, Runtime::Pywasm, Runtime::Js] {
        assert!(runtime.on_this_target(), "{runtime}");
    }
}

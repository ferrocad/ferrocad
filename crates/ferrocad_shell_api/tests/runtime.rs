//! One name for a scripting half, whichever host asks.
//!
//! The point of these is that the browser's `?runtime=` and the desktop's
//! `--script-host=` cannot drift: the aliases, the canonical spellings and the error
//! text all come from one table.

use ferrocad_shell_api::{EntryKind, Language, Platform, Runtime};

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

#[test]
fn a_runtime_installs_exactly_one_language() {
    assert_eq!(Language::of(Runtime::Plain), None);

    for runtime in Runtime::ALL {
        let Some(language) = Language::of(runtime) else {
            assert_eq!(runtime, Runtime::Plain, "only plain installs no language");
            continue;
        };
        let offered: Vec<Platform> = Platform::ALL
            .into_iter()
            .filter(|platform| language.runtimes(*platform).contains(&runtime))
            .collect();
        assert!(
            !offered.is_empty(),
            "{runtime} installs {language:?} on no platform"
        );
    }
}

#[test]
fn every_cell_lists_only_its_own_language() {
    // The cell match is exhaustive, so *coverage* is the compiler's job; what this catches
    // is a wrong pairing, which the type system cannot express.
    for language in Language::ALL {
        for platform in Platform::ALL {
            for runtime in language.runtimes(platform) {
                assert_eq!(
                    Language::of(*runtime),
                    Some(language),
                    "{language:?} on {platform:?} lists {runtime}"
                );
            }
        }
    }
}

#[test]
fn both_python_engines_are_on_the_desktop_and_one_is_in_the_browser() {
    assert_eq!(
        Language::Python.runtimes(Platform::Desktop),
        [Runtime::Cpython, Runtime::Pywasm]
    );
    // PyO3 is a native C-ABI extension: a browser cannot offer it, and the matrix says so
    // independently of `Runtime::on_this_target`, which asks the target this build is for.
    assert_eq!(
        Language::Python.runtimes(Platform::Browser),
        [Runtime::Pywasm]
    );
}

#[test]
fn javascript_is_the_same_half_on_both_hosts() {
    assert_eq!(Language::JavaScript.runtimes(Platform::Desktop), [Runtime::Js]);
    assert_eq!(Language::JavaScript.runtimes(Platform::Browser), [Runtime::Js]);
}

#[test]
fn wasm_is_a_package_not_a_runtime() {
    // A bundle rides beside the script half (`--addons`/`?addons=`), so no switch value
    // installs it — the B7e decision, asserted rather than remembered.
    for platform in Platform::ALL {
        assert!(Language::Wasm.runtimes(platform).is_empty(), "{platform:?}");
    }
    assert!(
        Runtime::ALL
            .into_iter()
            .all(|runtime| Language::of(runtime) != Some(Language::Wasm))
    );
}

#[test]
fn the_vocabulary_has_distinct_names() {
    let mut languages: Vec<&str> = Language::ALL.iter().map(|language| language.id()).collect();
    languages.sort_unstable();
    languages.dedup();
    assert_eq!(languages.len(), Language::ALL.len());

    let mut platforms: Vec<&str> = Platform::ALL.iter().map(|platform| platform.id()).collect();
    platforms.sort_unstable();
    platforms.dedup();
    assert_eq!(platforms.len(), Platform::ALL.len());
}

#[test]
fn the_current_platform_is_the_target() {
    let expected = if cfg!(target_family = "wasm") {
        Platform::Browser
    } else {
        Platform::Desktop
    };
    assert_eq!(Platform::current(), expected);
}

#[test]
fn a_mod_entry_is_recognised_by_its_file_name() {
    assert_eq!(EntryKind::of_file_name("InitGui.py"), Some(EntryKind::Python));
    assert_eq!(EntryKind::of_file_name("Init.py"), Some(EntryKind::Python));
    assert_eq!(
        EntryKind::of_file_name("InitGui.js"),
        Some(EntryKind::JavaScript)
    );
    assert_eq!(EntryKind::of_file_name("addon.wasm"), Some(EntryKind::Wasm));
    assert_eq!(EntryKind::of_file_name("README.md"), None);
    assert_eq!(EntryKind::of_file_name("InitGui.ts"), None);
}

#[test]
fn a_python_mod_asks_for_that_platforms_python() {
    assert_eq!(
        Runtime::hosts_for(&[EntryKind::Python], Platform::Desktop),
        [Runtime::Cpython]
    );
    assert_eq!(
        Runtime::hosts_for(&[EntryKind::Python], Platform::Browser),
        [Runtime::Pywasm]
    );
}

#[test]
fn a_js_mod_asks_for_the_page_on_both_hosts() {
    for platform in Platform::ALL {
        assert_eq!(
            Runtime::hosts_for(&[EntryKind::JavaScript], platform),
            [Runtime::Js],
            "{platform:?}"
        );
    }
}

#[test]
fn a_wasm_mod_asks_for_the_page_only_in_the_browser() {
    // On the desktop `wasmi` hosts a bundle with no script half; in the browser the page
    // is the host, because `loader.js` is driven by it.
    assert!(Runtime::hosts_for(&[EntryKind::Wasm], Platform::Desktop).is_empty());
    assert_eq!(
        Runtime::hosts_for(&[EntryKind::Wasm], Platform::Browser),
        [Runtime::Js]
    );
}

#[test]
fn python_is_the_primary_when_both_languages_are_present() {
    let hosts = Runtime::hosts_for(
        &[EntryKind::JavaScript, EntryKind::Python],
        Platform::Desktop,
    );
    // The order of the entries does not matter: Python leads.
    assert_eq!(hosts, [Runtime::Cpython, Runtime::Js]);
}

#[test]
fn no_mods_derives_no_hosts() {
    // The caller decides what an empty set means; the derivation does not invent a default.
    for platform in Platform::ALL {
        assert!(Runtime::hosts_for(&[], platform).is_empty(), "{platform:?}");
    }
}

#[test]
fn a_list_names_hosts_in_order() {
    assert_eq!(Runtime::parse_list("js"), Ok(vec![Runtime::Js]));
    assert_eq!(
        Runtime::parse_list("cpython,js"),
        Ok(vec![Runtime::Cpython, Runtime::Js])
    );
    // Whitespace around the names is the page's `?runtime=js, cpython` and a hand-typed
    // command line, so both have to work.
    assert_eq!(
        Runtime::parse_list(" pywasm , js "),
        Ok(vec![Runtime::Pywasm, Runtime::Js])
    );
}

#[test]
fn two_engines_for_one_language_are_refused() {
    // B8c: `cpython,pywasm` is one host with two interpreters — the same workbench files
    // behind two consoles — not a chain.
    let error = Runtime::parse_list("cpython,pywasm").unwrap_err();
    assert!(error.contains("both python"), "{error}");
    assert!(error.contains("cpython"), "{error}");
    assert!(error.contains("pywasm"), "{error}");

    let repeated = Runtime::parse_list("js,js").unwrap_err();
    assert!(repeated.contains("both javascript"), "{repeated}");
}

#[test]
fn plain_is_the_absence_of_a_host_and_cannot_be_combined() {
    assert_eq!(Runtime::parse_list("plain"), Ok(vec![Runtime::Plain]));
    let error = Runtime::parse_list("plain,js").unwrap_err();
    assert!(error.contains("plain"), "{error}");
    assert!(error.contains("alone"), "{error}");
}

#[test]
fn an_empty_or_unknown_list_says_what_the_names_are() {
    let empty = Runtime::parse_list("  ").unwrap_err();
    assert!(empty.contains("no runtime named"), "{empty}");

    let unknown = Runtime::parse_list("js,coffee").unwrap_err();
    assert!(unknown.contains("coffee"), "{unknown}");
}

#[test]
fn the_validation_rule_is_usable_on_a_list_built_by_hand() {
    assert_eq!(Runtime::validate_list(&[]), Ok(()));
    assert_eq!(Runtime::validate_list(&[Runtime::Plain]), Ok(()));
    assert_eq!(
        Runtime::validate_list(&[Runtime::Cpython, Runtime::Js]),
        Ok(())
    );
    assert!(Runtime::validate_list(&[Runtime::Js, Runtime::Js]).is_err());
}

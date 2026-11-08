//! Which scripting half the browser host runs.
//!
//! The browser has one document half — [`ferrocad_shell_rust`]'s `RustAppBackend`,
//! the same one a desktop edition installs — and a choice of scripting halves. This
//! crate is the switch between them, so the host itself stays "window, page bridge,
//! file picker":
//!
//! ```text
//! ?runtime=plain       RustAppBackend                     (document only)
//! ?runtime=pywasm      RustAppBackend + PywasmScriptHost  (RustPython, in the module)
//! ?runtime=js          RustAppBackend + JsScriptHost      (the page's ES module)
//! ?runtime=pywasm,js   both, RustPython first            (a chain, B8)
//! ```
//!
//! `?js=1` and `?pywasm=1` are the short forms; `?runtime=` wins when both are
//! present, and `?js=1` beats `?pywasm=1` because the page installs the module for it.
//! A comma-separated `?runtime=` names a chain, and the first name is the primary — it
//! owns the console, the document lifecycle and the render pipeline. One host per language:
//! `?runtime=cpython,pywasm` is refused, because `ferrocad_py` and RustPython are two
//! engines for the same language.
//! Every other query parameter (`?click=`, `?select=`, `?keys=`, …) is the capture
//! harness's and is ignored. The names themselves — and their aliases — are
//! [`ferrocad_shell_api::runtime::Runtime`], so the desktop's `--script-host=` cannot
//! drift from this one. `cpython` parses and is refused: `ferrocad_py` is a native
//! PyO3 extension, so no browser can serve it.
//!
//! **The pywasm pairing is not arbitrary.** The Python facade reads
//! `FreeCAD.ActiveDocument` from `ferrocad_core`'s process-wide registry, so that
//! configuration must use `RustAppBackend::from_shared_registry` — otherwise the
//! document lives in the backend's own handle and the console sees nothing. The JS
//! configuration uses the same source for the same reason: its page reaches the
//! document through `ferrocad_doc_api::DocApi`, which reads the registry. The plain
//! configuration has no script half, so it builds its sample directly.
//!
//! Nothing here is a host: it returns an `AppBackend`, and [`ferrocad_web_gpui`]
//! installs it.
//!
//! [`ferrocad_web_gpui`]: https://docs.rs/ferrocad_web_gpui

use ferrocad_shell_api::app::AppBackend;

pub use ferrocad_shell_api::runtime::Runtime;

/// The document half on its own.
///
/// The fallback the host installs when the page asked for a runtime that could not
/// boot, so a browser with no interpreter still shows the shell.
pub fn plain() -> Box<dyn AppBackend> {
    Box::new(ferrocad_shell_rust::RustAppBackend::new())
}

/// Compose the backend for a chain of runtimes; the first is the primary.
///
/// `Err` is a request that cannot be served here — a JS half with no page module, an
/// interpreter that did not boot, or two engines for one language — and the caller decides
/// whether to fall back to [`plain`], which the host does so a broken surface never costs
/// the window.
pub fn backend(runtimes: &[Runtime]) -> Result<Box<dyn AppBackend>, String> {
    Runtime::validate_list(runtimes)?;

    let mut hosts: Vec<Box<dyn ferrocad_shell_api::ScriptHost>> = Vec::new();
    for runtime in runtimes {
        match runtime {
            Runtime::Plain => {}
            Runtime::Cpython => {
                return Err(
                    "no CPython in the browser: `ferrocad_py` is a native PyO3 extension, \
                     so the browser's Python is ?runtime=pywasm"
                        .to_string(),
                );
            }
            Runtime::Pywasm => {
                let script = ferrocad_shell_pywasm::PywasmScriptHost::live().map_err(|error| {
                    format!("the wasm-Python interpreter did not boot: {error}")
                })?;
                hosts.push(Box::new(script));
            }
            Runtime::Js => hosts.push(js_host()?),
        }
    }

    // A chain of one is installed directly, so the common configuration does not pay for
    // the composite. The document half is the shared-registry one whenever a script is
    // installed: every browser half reaches the document through the registry.
    let script: Option<Box<dyn ferrocad_shell_api::ScriptHost>> = match hosts.len() {
        0 => None,
        1 => hosts
            .into_iter()
            .next()
            .map(|host| host as Box<dyn ferrocad_shell_api::ScriptHost>),
        _ => {
            let members = hosts
                .into_iter()
                .map(std::sync::Arc::from)
                .collect::<Vec<std::sync::Arc<dyn ferrocad_shell_api::ScriptHost>>>();
            Some(Box::new(ferrocad_shell_api::HostChain::new(members)))
        }
    };

    let backend = match script {
        Some(script) => {
            ferrocad_shell_rust::RustAppBackend::from_shared_registry().with_script_host(script)
        }
        None => ferrocad_shell_rust::RustAppBackend::new(),
    };
    Ok(Box::new(backend))
}

/// The JS half over the page's module, when the page installed one.
///
/// The page gets the document through [`DocumentApi`](ferrocad_shell_api::DocumentApi),
/// so the backend must be the shared-registry one: an isolated document lives in the
/// backend's own handle, where no script can reach it.
#[cfg(target_family = "wasm")]
fn js_host() -> Result<Box<dyn ferrocad_shell_api::ScriptHost>, String> {
    let page: std::sync::Arc<dyn ferrocad_shell_js::Page> =
        std::sync::Arc::new(ferrocad_shell_js::WebPage);
    if !ferrocad_shell_js::available(&*page) {
        return Err(
            "no JS script half: the page did not install window.__ferrocadJs \
             (?js=1 loads js-demo.js, which does)"
                .to_string(),
        );
    }
    let events: std::sync::Arc<dyn ferrocad_shell_api::DocumentEventSink> =
        std::sync::Arc::new(ferrocad_shell_js::PageEvents::new(std::sync::Arc::clone(&page)));
    let document = std::sync::Arc::new(ferrocad_doc_api::DocApi::with_events(events));
    Ok(Box::new(ferrocad_shell_js::JsScriptHost::with_kernel(
        page, document,
    )))
}

/// A page's ES module only exists in a browser.
#[cfg(not(target_family = "wasm"))]
fn js_host() -> Result<Box<dyn ferrocad_shell_api::ScriptHost>, String> {
    Err("the JS script half is a browser module and has no native build".to_string())
}

/// Read the runtimes out of a query string (`?a=1&b=2`, or `a=1`).
///
/// Unknown keys are ignored: the page's capture harness puts its own parameters in the
/// same string. An unknown `runtime=` value is an error rather than a silent fallback,
/// because that is a typo the user should see.
pub fn from_query(query: &str) -> Result<Vec<Runtime>, String> {
    let mut explicit = None;
    let mut js = false;
    let mut pywasm = false;
    for (key, value) in pairs(query) {
        match key {
            "runtime" => {
                // The browser's word for "the Python I have" is RustPython: `ferrocad_py`
                // is a native extension, so `cpython` parses and is refused by `backend`.
                // Folded per name, so `?runtime=python,js` means the same as
                // `?runtime=pywasm,js`.
                let folded: Vec<&str> = value
                    .split(',')
                    .map(|name| match name.trim() {
                        "python" | "py" => "pywasm",
                        other => other,
                    })
                    .collect();
                explicit = Some(Runtime::parse_list(&folded.join(","))?);
            }
            "js" => js = true,
            "pywasm" => pywasm = true,
            _ => {}
        }
    }
    Ok(match (explicit, js, pywasm) {
        (Some(runtimes), _, _) => runtimes,
        (None, true, _) => vec![Runtime::Js],
        (None, false, true) => vec![Runtime::Pywasm],
        (None, false, false) => vec![Runtime::Plain],
    })
}

/// The pairs of a query string, without decoding.
///
/// The keys this reads are ASCII identifiers and the values are plain words, so
/// percent-decoding would only add a failure mode.
fn pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        })
}

/// Compose the backend the page asked for, and say which runtimes it is.
#[cfg(target_family = "wasm")]
pub fn page_backend() -> Result<(Box<dyn AppBackend>, Vec<Runtime>), String> {
    let runtimes = from_query(&page_search())?;
    let backend = backend(&runtimes)?;
    Ok((backend, runtimes))
}

/// `window.location.search`.
#[cfg(target_family = "wasm")]
fn page_search() -> String {
    use wasm_bindgen::JsValue;

    let search = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("location"))
        .ok()
        .and_then(|location| js_sys::Reflect::get(&location, &JsValue::from_str("search")).ok())
        .and_then(|search| search.as_string());
    match search {
        Some(search) => search,
        None => {
            log::warn!("ferrocad: no window.location.search; the document runs alone");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_query_is_the_document_alone() {
        assert_eq!(from_query("").unwrap(), vec![Runtime::Plain]);
        assert_eq!(from_query("?").unwrap(), vec![Runtime::Plain]);
    }

    #[test]
    fn the_shipped_flag_still_selects_the_page_module() {
        assert_eq!(from_query("?js=1").unwrap(), vec![Runtime::Js]);
        // `index.html` keys on `params.has("js")`, so a bare flag is the same request.
        assert_eq!(from_query("?js").unwrap(), vec![Runtime::Js]);
    }

    #[test]
    fn pywasm_has_its_own_flag() {
        assert_eq!(from_query("?pywasm=1").unwrap(), vec![Runtime::Pywasm]);
    }

    #[test]
    fn python_is_the_interpreter_the_browser_has() {
        // The shared table resolves `python` per target; in the browser that is RustPython.
        assert_eq!(from_query("?runtime=python").unwrap(), vec![Runtime::Pywasm]);
    }

    #[test]
    fn runtime_names_the_configuration_exactly() {
        assert_eq!(from_query("?runtime=plain").unwrap(), vec![Runtime::Plain]);
        assert_eq!(from_query("?runtime=pywasm").unwrap(), vec![Runtime::Pywasm]);
        assert_eq!(from_query("?runtime=js").unwrap(), vec![Runtime::Js]);
    }

    #[test]
    fn a_list_names_a_chain_in_order() {
        assert_eq!(
            from_query("?runtime=pywasm,js").unwrap(),
            vec![Runtime::Pywasm, Runtime::Js]
        );
        // The browser's word for its Python folds per name, so a list works too.
        assert_eq!(
            from_query("?runtime=python,js").unwrap(),
            vec![Runtime::Pywasm, Runtime::Js]
        );
        // Order is the primary: whichever name comes first owns the console.
        assert_eq!(
            from_query("?runtime=js,pywasm").unwrap(),
            vec![Runtime::Js, Runtime::Pywasm]
        );
    }

    #[test]
    fn two_engines_for_one_language_are_refused_in_the_query() {
        // `cpython` and `pywasm` are one language with two engines; the seam's rule applies to
        // the page's switch exactly as it does to `--script-host=`.
        let error = from_query("?runtime=cpython,pywasm").unwrap_err();
        assert!(error.contains("both python"), "{error}");
    }

    #[test]
    fn plain_cannot_be_combined_in_the_query() {
        let error = from_query("?runtime=plain,js").unwrap_err();
        assert!(error.contains("alone"), "{error}");
    }

    #[test]
    fn an_explicit_runtime_wins_over_a_flag() {
        assert_eq!(from_query("?js=1&runtime=plain").unwrap(), vec![Runtime::Plain]);
        assert_eq!(from_query("?pywasm=1&runtime=js").unwrap(), vec![Runtime::Js]);
    }

    #[test]
    fn js_wins_when_both_flags_are_present() {
        // `?js=1` is the flag the page itself acts on (it installs the module), so
        // that is the surface the user asked for; `runtime=` is how to be explicit.
        assert_eq!(from_query("?pywasm=1&js=1").unwrap(), vec![Runtime::Js]);
    }

    #[test]
    fn an_unknown_runtime_is_an_error() {
        let error = from_query("?runtime=coffee").unwrap_err();
        assert!(error.contains("coffee"), "{error}");
        assert!(error.contains("pywasm"), "{error}");
    }

    #[test]
    fn cpython_parses_and_is_refused() {
        // The name is shared with the desktop, so the query is not a typo; the assembly
        // simply cannot link a native extension. The refusal has to say that.
        assert_eq!(from_query("?runtime=cpython").unwrap(), vec![Runtime::Cpython]);
        let error = match backend(&[Runtime::Cpython]) {
            Ok(_) => panic!("the browser cannot link a native extension"),
            Err(error) => error,
        };
        assert!(error.contains("CPython"), "{error}");
        assert!(error.contains("pywasm"), "{error}");
    }

    #[test]
    fn the_capture_parameters_are_ignored() {
        // `?click=`, `?select=` and `?keys=` drive `index.html`; they must not change
        // which runtime boots.
        let query = "?click=430,380&select=1004,100&keys=ctrl-k;f;i;t;enter&drag=1";
        assert_eq!(from_query(query).unwrap(), vec![Runtime::Plain]);
    }

    #[test]
    fn the_ids_are_what_the_host_logs() {
        assert_eq!(Runtime::Plain.id(), "plain");
        assert_eq!(Runtime::Pywasm.id(), "pywasm");
        assert_eq!(Runtime::Js.id(), "js");
        assert_eq!(Runtime::Pywasm.to_string(), "pywasm");
    }

    #[test]
    fn a_page_module_cannot_be_composed_natively() {
        assert!(backend(&[Runtime::Js]).is_err());
    }

    /// The configuration the browser runs with `?pywasm=1`, composed for real: one
    /// document, read by both halves.
    #[test]
    fn the_pywasm_configuration_boots_one_document() {
        std::thread::Builder::new()
            // RustPython's compiler recurses; the interpreter's own tests use the same
            // size. The browser gets it from `ferrocad_web_gpui`'s link argument.
            .stack_size(16 << 20)
            .spawn(|| {
                let mut composed = backend(&[Runtime::Pywasm]).expect("the interpreter boots");
                let boot = composed.bootstrap().expect("the sample boots");
                assert_eq!(boot.document, "Sample");

                // The document half answers the tree ...
                let tree = composed.model_tree().expect("a model tree");
                let objects: Vec<&str> = tree
                    .iter()
                    .flat_map(|document| &document.objects)
                    .map(|object| object.label.as_str())
                    .collect();
                assert!(
                    objects.contains(&"Fillet"),
                    "the sample's objects: {objects:?}"
                );

                // ... and the console reads the same one, through the seeded namespace.
                let console = composed
                    .evaluate("FreeCAD.ActiveDocument.Name")
                    .expect("the console runs");
                assert!(console.ok, "{}", console.output);
                assert_eq!(console.output, "'Sample'");

                // The interpreter names itself, so the status line is not lying about
                // which scripting surface booted.
                let hello = composed.hello().expect("a greeting");
                assert!(hello.contains("RustPython"), "{hello}");
            })
            .expect("a worker thread")
            .join()
            .expect("the thread did not panic");
    }
}

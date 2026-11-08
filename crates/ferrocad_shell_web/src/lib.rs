//! Which scripting half the browser host runs.
//!
//! The browser has one document half — [`ferrocad_shell_rust`]'s `RustAppBackend`,
//! the same one a desktop edition installs — and a choice of scripting halves. This
//! crate is the switch between them, so the host itself stays "window, page bridge,
//! file picker":
//!
//! ```text
//! ?runtime=plain    RustAppBackend                          (document only)
//! ?runtime=pywasm   RustAppBackend + PywasmScriptHost       (RustPython, in the module)
//! ?runtime=js       RustAppBackend + JsScriptHost           (the page's ES module)
//! ```
//!
//! `?js=1` and `?pywasm=1` are the short forms; `?runtime=` wins when both are
//! present, and `?js=1` beats `?pywasm=1` because the page installs the module for it.
//! Every other query parameter (`?click=`, `?select=`, `?keys=`, …) is the capture
//! harness's and is ignored.
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

/// One of the browser's scripting configurations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    /// The document half alone: no interpreter, no page module.
    Plain,
    /// RustPython in the module: `python/ferrocad_shell` over the real document.
    Pywasm,
    /// The page's ES module, reached through `window.__ferrocadJs`.
    Js,
}

impl Runtime {
    /// The name the host logs and the query uses.
    pub fn id(self) -> &'static str {
        match self {
            Runtime::Plain => "plain",
            Runtime::Pywasm => "pywasm",
            Runtime::Js => "js",
        }
    }
}

impl std::fmt::Display for Runtime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.id())
    }
}

/// The document half on its own.
///
/// The fallback the host installs when the page asked for a runtime that could not
/// boot, so a browser with no interpreter still shows the shell.
pub fn plain() -> Box<dyn AppBackend> {
    Box::new(ferrocad_shell_rust::RustAppBackend::new())
}

/// Compose the backend for one runtime.
///
/// `Err` is a request that cannot be served here — a JS half with no page module, or
/// an interpreter that did not boot — and the caller decides whether to fall back to
/// [`plain`], which the host does so a broken surface never costs the window.
pub fn backend(runtime: Runtime) -> Result<Box<dyn AppBackend>, String> {
    Ok(match runtime {
        Runtime::Plain => plain(),
        Runtime::Pywasm => {
            let script = ferrocad_shell_pywasm::PywasmScriptHost::live()
                .map_err(|error| format!("the wasm-Python interpreter did not boot: {error}"))?;
            Box::new(
                ferrocad_shell_rust::RustAppBackend::from_shared_registry()
                    .with_script_host(Box::new(script)),
            )
        }
        Runtime::Js => js_backend()?,
    })
}

/// The JS half over the page's module, when the page installed one.
///
/// The page gets the document through [`DocumentApi`](ferrocad_shell_api::DocumentApi),
/// so the backend must be the shared-registry one: an isolated document lives in the
/// backend's own handle, where no script can reach it.
#[cfg(target_family = "wasm")]
fn js_backend() -> Result<Box<dyn AppBackend>, String> {
    if !ferrocad_shell_js::available() {
        return Err(
            "no JS script half: the page did not install window.__ferrocadJs \
             (?js=1 loads js-demo.js, which does)"
                .to_string(),
        );
    }
    let events: std::sync::Arc<dyn ferrocad_shell_api::DocumentEventSink> =
        std::sync::Arc::new(ferrocad_shell_js::PageEvents);
    let document = std::sync::Arc::new(ferrocad_doc_api::DocApi::with_events(events));
    Ok(Box::new(
        ferrocad_shell_rust::RustAppBackend::from_shared_registry().with_script_host(Box::new(
            ferrocad_shell_js::JsScriptHost::with_kernel(document),
        )),
    ))
}

/// A page's ES module only exists in a browser.
#[cfg(not(target_family = "wasm"))]
fn js_backend() -> Result<Box<dyn AppBackend>, String> {
    Err("the JS script half is a browser module and has no native build".to_string())
}

/// Read the runtime out of a query string (`?a=1&b=2`, or `a=1`).
///
/// Unknown keys are ignored: the page's capture harness puts its own parameters in
/// the same string. An unknown `runtime=` value is an error rather than a silent
/// fallback, because that is a typo the user should see.
pub fn from_query(query: &str) -> Result<Runtime, String> {
    let mut explicit = None;
    let mut js = false;
    let mut pywasm = false;
    for (key, value) in pairs(query) {
        match key {
            "runtime" => {
                explicit = Some(match value {
                    "plain" | "none" | "rust" => Runtime::Plain,
                    "pywasm" | "py" | "python" => Runtime::Pywasm,
                    "js" | "javascript" => Runtime::Js,
                    other => {
                        return Err(format!(
                            "runtime: unknown value {other:?}; expected plain, pywasm or js"
                        ));
                    }
                });
            }
            "js" => js = true,
            "pywasm" => pywasm = true,
            _ => {}
        }
    }
    Ok(match (explicit, js, pywasm) {
        (Some(runtime), _, _) => runtime,
        (None, true, _) => Runtime::Js,
        (None, false, true) => Runtime::Pywasm,
        (None, false, false) => Runtime::Plain,
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

/// Compose the backend the page asked for, and say which runtime it is.
#[cfg(target_family = "wasm")]
pub fn page_backend() -> Result<(Box<dyn AppBackend>, Runtime), String> {
    let runtime = from_query(&page_search())?;
    let backend = backend(runtime)?;
    Ok((backend, runtime))
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
        assert_eq!(from_query("").unwrap(), Runtime::Plain);
        assert_eq!(from_query("?").unwrap(), Runtime::Plain);
    }

    #[test]
    fn the_shipped_flag_still_selects_the_page_module() {
        assert_eq!(from_query("?js=1").unwrap(), Runtime::Js);
        // `index.html` keys on `params.has("js")`, so a bare flag is the same request.
        assert_eq!(from_query("?js").unwrap(), Runtime::Js);
    }

    #[test]
    fn pywasm_has_its_own_flag() {
        assert_eq!(from_query("?pywasm=1").unwrap(), Runtime::Pywasm);
    }

    #[test]
    fn runtime_names_the_configuration_exactly() {
        assert_eq!(from_query("?runtime=plain").unwrap(), Runtime::Plain);
        assert_eq!(from_query("?runtime=pywasm").unwrap(), Runtime::Pywasm);
        assert_eq!(from_query("?runtime=js").unwrap(), Runtime::Js);
    }

    #[test]
    fn an_explicit_runtime_wins_over_a_flag() {
        assert_eq!(from_query("?js=1&runtime=plain").unwrap(), Runtime::Plain);
        assert_eq!(from_query("?pywasm=1&runtime=js").unwrap(), Runtime::Js);
    }

    #[test]
    fn js_wins_when_both_flags_are_present() {
        // `?js=1` is the flag the page itself acts on (it installs the module), so
        // that is the surface the user asked for; `runtime=` is how to be explicit.
        assert_eq!(from_query("?pywasm=1&js=1").unwrap(), Runtime::Js);
    }

    #[test]
    fn an_unknown_runtime_is_an_error() {
        let error = from_query("?runtime=rustpython").unwrap_err();
        assert!(error.contains("rustpython"), "{error}");
        assert!(error.contains("pywasm"), "{error}");
    }

    #[test]
    fn the_capture_parameters_are_ignored() {
        // `?click=`, `?select=` and `?keys=` drive `index.html`; they must not change
        // which runtime boots.
        let query = "?click=430,380&select=1004,100&keys=ctrl-k;f;i;t;enter&drag=1";
        assert_eq!(from_query(query).unwrap(), Runtime::Plain);
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
        assert!(backend(Runtime::Js).is_err());
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
                let mut composed = backend(Runtime::Pywasm).expect("the interpreter boots");
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

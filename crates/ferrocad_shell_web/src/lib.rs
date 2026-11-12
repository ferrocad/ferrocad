//! Which scripting half the browser host runs.
//!
//! The browser has one document half — [`ferrocad_shell_rust`]'s `RustAppBackend`,
//! the same one a desktop edition installs — and a choice of scripting halves. This
//! crate is the switch between them, so the host itself stays "window, page bridge,
//! file picker":
//!
//! ```text
//! (no query)           RustAppBackend + pywasm,js        (the default chain)
//! ?runtime=plain       RustAppBackend                     (document only)
//! ?runtime=pywasm      RustAppBackend + PywasmScriptHost  (RustPython, in the module)
//! ?runtime=js          RustAppBackend + JsScriptHost      (the page's ES module)
//! ?runtime=pywasm,js   both, RustPython first            (a chain, B8)
//! ```
//!
//! With no `runtime`, the browser boots both script halves. `?runtime=plain` is the
//! lightweight document-only session; naming any runtime selects exactly that chain.
//!
//! `plain` and `js` have no `Settings` namespace of their own, so the document half
//! answers it: the grid card (`ferrocad_shell_rust::grid_settings`) reads and writes
//! the presentation state the shell mirrors into the backend, and exists precisely
//! when no script half claimed that namespace.
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
use ferrocad_shell_api::settings::SettingsBackend;

pub mod mods;

pub use ferrocad_shell_api::runtime::Runtime;

/// The document half on its own.
///
/// The fallback the host installs when the page asked for a runtime that could not
/// boot, so a browser with no interpreter still shows the shell.
pub fn plain() -> Box<dyn AppBackend> {
    plain_with_settings(None)
}

/// [`plain`] with the host's settings store installed.
pub fn plain_with_settings(settings: Option<Box<dyn SettingsBackend>>) -> Box<dyn AppBackend> {
    let mut backend = ferrocad_shell_rust::RustAppBackend::new();
    if let Some(settings) = settings {
        backend = backend.with_settings_backend(settings);
    }
    Box::new(backend)
}

/// Compose the backend for a chain of runtimes; the first is the primary.
///
/// `Err` is a request that cannot be served here — a JS half with no page module, an
/// interpreter that did not boot, or two engines for one language — and the caller decides
/// whether to fall back to [`plain`], which the host does so a broken surface never costs
/// the window.
pub fn backend(runtimes: &[Runtime]) -> Result<Box<dyn AppBackend>, String> {
    backend_with_settings(runtimes, None)
}

/// [`backend`] with the host's settings store installed before the shell boots.
///
/// The browser passes its `localStorage` store here; the default `backend` keeps the
/// in-memory store, which is what a headless session and the switch tests want.
pub fn backend_with_settings(
    runtimes: &[Runtime],
    settings: Option<Box<dyn SettingsBackend>>,
) -> Result<Box<dyn AppBackend>, String> {
    Runtime::validate_list(runtimes)?;

    let mut hosts: Vec<Box<dyn ferrocad_shell_api::ScriptHost>> = Vec::new();
    let mut installed: Vec<&'static str> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for runtime in runtimes {
        match browser_host(*runtime) {
            Ok(Some(host)) => {
                hosts.push(host);
                installed.push(runtime.id());
            }
            Ok(None) => {}
            Err(error) => skipped.push(error),
        }
    }
    // The same rule as the desktop (B14): `?runtime=` is a request, and one half this
    // build cannot stand up must not cost the halves it can. `?runtime=pywasm,js` without
    // a page module used to leave the interpreter uninstalled too, and the page then ran
    // the document half alone — which looks exactly like "RustPython failed".
    if hosts.is_empty() && !skipped.is_empty() {
        return Err(skipped.into_iter().next().unwrap_or_default());
    }
    for error in &skipped {
        log::warn!("ferrocad: {error}");
    }
    if !skipped.is_empty() {
        log::info!(
            "ferrocad: continuing with the script halves this build has: {}",
            installed.join(", ")
        );
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

    let mut backend = match script {
        Some(script) => {
            ferrocad_shell_rust::RustAppBackend::from_shared_registry().with_script_host(script)
        }
        None => ferrocad_shell_rust::RustAppBackend::new(),
    };
    if let Some(settings) = settings {
        backend = backend.with_settings_backend(settings);
    }
    Ok(Box::new(backend))
}

/// The script half one runtime names, if it names one.
///
/// `Err` is this build not being able to stand that half up (no CPython in a browser, no
/// page module, an interpreter that did not boot). The caller reports it and keeps the
/// other halves, so a chain degrades rather than disappearing.
fn browser_host(
    runtime: Runtime,
) -> Result<Option<Box<dyn ferrocad_shell_api::ScriptHost>>, String> {
    Ok(match runtime {
        Runtime::Plain => None,
        Runtime::Cpython => {
            return Err(
                "no CPython in the browser: `ferrocad_py` is a native PyO3 extension, \
                 so the browser's Python is ?runtime=pywasm"
                    .to_string(),
            );
        }
        Runtime::Pywasm => Some(Box::new(
            ferrocad_shell_pywasm::live()
                .map_err(|error| format!("the wasm-Python interpreter did not boot: {error}"))?,
        )),
        Runtime::Js => Some(js_host()?),
    })
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
    let events: std::sync::Arc<dyn ferrocad_shell_api::DocumentEventSink> = std::sync::Arc::new(
        ferrocad_shell_js::PageEvents::new(std::sync::Arc::clone(&page)),
    );
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
        // No query and no flag: both script halves, RustPython first.
        (None, false, false) => vec![Runtime::Pywasm, Runtime::Js],
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
    page_backend_with_settings(None)
}

/// [`page_backend`] with the page's settings store installed.
#[cfg(target_family = "wasm")]
pub fn page_backend_with_settings(
    settings: Option<Box<dyn SettingsBackend>>,
) -> Result<(Box<dyn AppBackend>, Vec<Runtime>), String> {
    let runtimes = from_query(&page_search())?;
    let backend = backend_with_settings(&runtimes, settings)?;
    Ok((backend, runtimes))
}

/// One query parameter's value, or `None` when the page did not pass it.
///
/// The harness drives a host through the query string (`?manipulator=…`, `?click=…`),
/// so the parsing belongs beside [`from_query`] — and it is a plain function over the
/// string, which keeps it testable without a browser. The value is percent-decoded: a
/// `?script=` carries Python, where `%20`, `%27` and the rest are the only way to write
/// it in a URL.
pub fn parameter(query: &str, name: &str) -> Option<String> {
    pairs(query)
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode(value))
}

/// `%XX` and `+` as a URL decoder, tolerant of a malformed escape.
///
/// A `%` that does not start two hex digits is kept as written rather than dropped: a
/// query the page typed by hand should not lose the characters it did type.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let escape = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match escape.and_then(|digits| u8::from_str_radix(digits, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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

/// The page's query string, for a host that has parameters of its own.
///
/// Empty off the browser: a native caller has no page, and a probe that reads nothing
/// is the right answer there.
#[cfg(target_family = "wasm")]
pub fn page_query() -> String {
    page_search()
}

#[cfg(not(target_family = "wasm"))]
pub fn page_query() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_query_boots_both_script_halves() {
        assert_eq!(from_query("").unwrap(), vec![Runtime::Pywasm, Runtime::Js]);
        assert_eq!(from_query("?").unwrap(), vec![Runtime::Pywasm, Runtime::Js]);
    }

    #[test]
    fn a_parameter_is_read_and_its_absence_is_none() {
        assert_eq!(
            parameter("?manipulator=dress_up:fillet:Box:Edge1:2", "manipulator").as_deref(),
            Some("dress_up:fillet:Box:Edge1:2")
        );
        // Not the first pair, and not the last.
        assert_eq!(
            parameter(
                "?js=1&manipulator=push_pull:extrude:Box:Face1:3&probe=1",
                "manipulator"
            )
            .as_deref(),
            Some("push_pull:extrude:Box:Face1:3")
        );
        assert_eq!(parameter("?js=1", "manipulator"), None);
        assert_eq!(parameter("", "manipulator"), None);
    }

    #[test]
    fn a_parameter_is_percent_decoded() {
        // `?script=` carries Python; only escapes can put a quote, a space or a newline
        // in a URL. `+` is the form-encoded space, and a lone `%` survives.
        assert_eq!(
            parameter(
                "?script=obj.Float%20%3D%202.5%0Aprint%28%27Probe%27%29",
                "script"
            )
            .as_deref(),
            Some("obj.Float = 2.5\nprint('Probe')")
        );
        assert_eq!(
            parameter("?script=a+b&runtime=pywasm", "script").as_deref(),
            Some("a b")
        );
        assert_eq!(
            parameter("?script=100%25", "script").as_deref(),
            Some("100%")
        );
        assert_eq!(parameter("?script=50%", "script").as_deref(), Some("50%"));
        assert_eq!(
            parameter("?script=%zz", "script").as_deref(),
            Some("%zz"),
            "a malformed escape is kept, not dropped"
        );
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
        assert_eq!(
            from_query("?runtime=python").unwrap(),
            vec![Runtime::Pywasm]
        );
    }

    #[test]
    fn runtime_names_the_configuration_exactly() {
        assert_eq!(from_query("?runtime=plain").unwrap(), vec![Runtime::Plain]);
        assert_eq!(
            from_query("?runtime=pywasm").unwrap(),
            vec![Runtime::Pywasm]
        );
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
        assert_eq!(
            from_query("?js=1&runtime=plain").unwrap(),
            vec![Runtime::Plain]
        );
        assert_eq!(
            from_query("?pywasm=1&runtime=js").unwrap(),
            vec![Runtime::Js]
        );
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
        assert_eq!(
            from_query("?runtime=cpython").unwrap(),
            vec![Runtime::Cpython]
        );
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
        assert_eq!(
            from_query(query).unwrap(),
            vec![Runtime::Pywasm, Runtime::Js]
        );
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
        // Nothing installable is still a refusal, and the message is the member's own:
        // that is the one that names the remedy.
        let error = backend(&[Runtime::Js]).err().expect("no page module here");
        assert!(error.contains("JS script half"), "{error}");
    }

    /// A half this build cannot stand up does not discard the ones it can (B14, browser).
    ///
    /// `?runtime=pywasm,js` is a chain the page documents, and `js` needs the page's ES
    /// module. Before this, the missing page returned `Err` for the whole list, so the
    /// interpreter was never installed and the page ran the document half alone — which
    /// reads as "RustPython failed to boot".
    #[test]
    fn a_half_this_build_lacks_does_not_discard_the_ones_it_has() {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(|| {
                let composed = backend(&[Runtime::Pywasm, Runtime::Js])
                    .expect("the interpreter is installed even though the page is not");
                assert!(
                    composed.script_host().is_some(),
                    "RustPython survives the missing JS half"
                );
            })
            .expect("a worker thread")
            .join()
            .expect("the thread did not panic");
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

//! B18b item 1: `python/FreeCAD` itself runs under RustPython.
//!
//! The App facade is installed from `python/` (documents, `ParamGet`, `Units`), and its
//! `ferrocad` dependency is the native module in `src/native.rs`. The document is still
//! read-only — the document half owns the write path — but the *facade* is the desktop's
//! one source, not a second implementation.

use ferrocad_shell_pywasm::PywasmScriptHost;

/// Run `f` against a freshly booted interpreter on a large stack (RustPython's compiler
/// recurses; see `properties.rs` for the same requirement).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// Run `code` in the console and return what it printed, or panic quoting the error.
fn evaluate(host: &PywasmScriptHost, code: &str) -> String {
    let raw = host.call("evaluate", &[code]).expect("the console runs");
    let answer: serde_json::Value = serde_json::from_str(&raw).expect("evaluate answers JSON");
    assert_eq!(answer["ok"], true, "{answer}");
    answer["output"].as_str().unwrap_or_default().to_string()
}

/// The package is the shared source, and its documents are the native binding.
#[test]
fn the_app_facade_is_the_shared_package() {
    with_host(|host| {
        assert_eq!(
            evaluate(host, "print(FreeCAD.__file__)"),
            "FreeCAD/__init__.py",
            "the facade is installed from python/, not written by the host"
        );
        assert_eq!(
            evaluate(host, "print(type(FreeCAD.ActiveDocument).__module__)"),
            "ferrocad",
            "the document is a native binding over the host's payload"
        );
        assert_eq!(
            evaluate(host, "print(FreeCAD.ActiveDocument.getObject('Box').Name)"),
            "Box"
        );
        assert_eq!(
            evaluate(
                host,
                "print(FreeCAD.ActiveDocument.getObject('Box').Length)"
            ),
            "10 mm",
            "a property reads the payload's rendered value"
        );
    });
}

/// Creating, opening and closing are the document half's; a script gets a refusal that
/// says so instead of a silent no-op.
#[test]
fn documents_are_read_only_from_python() {
    with_host(|host| {
        let raw = host
            .call("evaluate", &["FreeCAD.newDocument('Other')"])
            .expect("the console runs");
        let answer: serde_json::Value = serde_json::from_str(&raw).expect("JSON");
        assert_eq!(answer["ok"], false, "{answer}");
        let error = answer["output"].as_str().unwrap_or_default();
        assert!(error.contains("read-only"), "{error}");
        assert!(error.contains("shell"), "{error}");

        // A property write is refused too: the object's dict holds the payload's
        // values, and `__setattr__` is what stops it looking like an edit.
        let raw = host
            .call(
                "evaluate",
                &["FreeCAD.ActiveDocument.getObject('Box').Length = 5"],
            )
            .expect("the console runs");
        let answer: serde_json::Value = serde_json::from_str(&raw).expect("JSON");
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(
            answer["output"]
                .as_str()
                .unwrap_or_default()
                .contains("read-only"),
            "{answer}"
        );
    });
}

/// The shared `ParamGet` is the store, and this host clears its file path: the values
/// are typed, grouped and last for the process, exactly as natively without a file.
#[test]
fn the_parameter_store_is_the_shared_one_and_memory_only() {
    with_host(|host| {
        assert_eq!(
            evaluate(host, "print(FreeCAD._parameter_file_path())"),
            "None"
        );
        evaluate(
            host,
            "group = FreeCAD.ParamGet('User parameter:Test')\ngroup.SetBool('on', True)\n\
             group.SetFloat('size', 2.5)",
        );
        assert_eq!(
            evaluate(
                host,
                "print(group.GetBool('on', False), group.GetFloat('size', 0.0))"
            ),
            "True 2.5"
        );
        assert_eq!(
            evaluate(
                host,
                "print(type(group).__module__ + '.' + type(group).__name__)"
            ),
            "FreeCAD.ParameterGrp",
            "the class is the facade's, not a host stand-in"
        );
    });
}

/// The value types are the engine's structures, so unit conversion and vector algebra
/// mean what they mean natively.
#[test]
fn the_value_types_are_the_engines() {
    with_host(|host| {
        assert_eq!(
            evaluate(host, "print(FreeCAD.Vector(3, 0, 4).Length)"),
            "5.0"
        );
        assert_eq!(
            evaluate(
                host,
                "print(FreeCAD.Vector(0, 0, 1).cross(FreeCAD.Vector(1, 0, 0)).y)"
            ),
            "1.0"
        );
        assert_eq!(
            evaluate(
                host,
                "print(FreeCAD.Units.Quantity('20 cm').getValueAs('mm').Value)"
            ),
            "200.0",
            "a conversion the stand-in refused"
        );
        assert_eq!(
            evaluate(host, "print(str(FreeCAD.Units.Quantity(10.0, 'mm')))"),
            "10 mm"
        );
    });
}

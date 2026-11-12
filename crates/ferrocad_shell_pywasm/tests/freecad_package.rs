//! B18b item 1: `python/FreeCAD` itself runs under RustPython.
//!
//! The App facade is installed from `python/` (documents, `ParamGet`, `Units`), and its
//! `ferrocad` dependency is the native module in `src/native.rs`. This host boots over a
//! **fixed payload**, so its document is read-only (a host that owns the registry writes;
//! see `writes.rs`), but the *facade* is the desktop's one source, not a second
//! implementation.

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
        // A native boot imports `python/` from disk (B18b item 3), so the file is an
        // absolute path; the assertion is about identity, not the install mechanism:
        // it is `python/FreeCAD/__init__.py`, not source the host wrote.
        let file = evaluate(host, "print(FreeCAD.__file__)");
        assert!(
            file.ends_with("FreeCAD/__init__.py"),
            "the facade is the shared source: {file}"
        );
        assert!(
            std::path::Path::new(&file).is_absolute(),
            "a native host imports the tree from disk: {file}"
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

/// The store is the facade's, but the host supplies the file: a persistent host writes
/// the same JSON the desktop's CPython half reads, and a second boot reads it back.
#[test]
fn the_parameter_file_persists_when_the_host_asks_for_it() {
    let dir = std::env::temp_dir().join(format!("ferrocad-params-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp config dir");
    // SAFETY: the other tests in this binary boot memory-only hosts, which never read
    // the environment (the loader clears their parameter path).
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };

    let write = || {
        let host = ferrocad_shell_pywasm::new_persistent().expect("boots");
        evaluate(
            &host,
            "g = FreeCAD.ParamGet('User parameter:Test')\n\
             g.SetString('persisted', 'yes')\n\
             g.SetBool('on', True)",
        );
    };
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(write)
        .expect("a worker thread")
        .join()
        .expect("no panic");

    let file = dir.join("ferrocad/parameters.json");
    let saved = std::fs::read_to_string(&file).expect("the store wrote its file");
    assert!(saved.contains("persisted"), "{saved}");

    let read = || {
        let host = ferrocad_shell_pywasm::new_persistent().expect("boots");
        assert_eq!(
            evaluate(
                &host,
                "g = FreeCAD.ParamGet('User parameter:Test')\n\
                 print(g.GetString('persisted', ''), g.GetBool('on', False))"
            ),
            "yes True",
            "a fresh boot reads the file the previous one wrote"
        );
    };
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(read)
        .expect("a worker thread")
        .join()
        .expect("no panic");

    let _ = std::fs::remove_dir_all(&dir);
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

/// The four classes `python/FreeCAD` re-exports last: the interned-string pair and the
/// two value bags.
#[test]
fn the_remaining_value_classes_are_bound() {
    with_host(|host| {
        assert_eq!(
            evaluate(
                host,
                "h = FreeCAD.StringHasher()\n\
                 s = h.getID('Box')\n\
                 print(s.Value, s.Data, h.isSame(h))"
            ),
            "1 Box True"
        );
        assert_eq!(
            evaluate(
                host,
                "h = FreeCAD.StringHasher()\n\
                 print(h.getID('a').isSame(h.getID('a')), h.getID('a').isSame(h.getID('b')))"
            ),
            "True False"
        );
        assert_eq!(
            evaluate(
                host,
                "b = FreeCAD.BoundBox(0, 0, 0, 1, 2, 3)\n\
                 print(b.XMin, b.YMax, b.ZMax, b.isValid)"
            ),
            "0.0 2.0 3.0 True"
        );
        assert_eq!(
            evaluate(host, "print(FreeCAD.BoundBox().isValid)"),
            "False",
            "an empty box is invalid, as upstream's is"
        );
        assert_eq!(
            evaluate(
                host,
                "m = FreeCAD.Material()\n\
                 m.DiffuseColor = (0.5, 0.25, 0.125, 1.0)\n\
                 print(m.DiffuseColor, m.Transparency)"
            ),
            "(0.5, 0.25, 0.125, 1.0) 0.0"
        );
    });
}

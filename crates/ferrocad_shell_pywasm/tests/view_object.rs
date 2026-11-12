//! `obj.ViewObject` and `Doc.Meta`/`Doc.settings` under RustPython.
//!
//! These were the last two classes `ferrocad_py` had and this half did not. The view
//! state is `ferrocad_core`'s, on the object in the registry, so the assertions here are
//! about the *document* as much as about Python: what a script sets is what the host
//! meshes on the next sync.

use ferrocad_core::Document;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::PywasmScriptHost;

/// The registry and `DOCUMENT_META` are process state; one test at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// Run `f` on RustPython's 16 MiB stack (its compiler recurses; see `properties.rs`).
///
/// `live()` is the host whose document is the registry — the one a write can reach.
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::live().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// Run Python, assert it did not raise, and answer what it printed.
fn output(host: &PywasmScriptHost, code: &str) -> String {
    let answer = host.evaluate(code).expect("evaluate runs");
    assert!(answer.ok, "{code}: {}", answer.output);
    answer.output
}

/// Run Python that must raise, and answer the traceback.
fn refused(host: &PywasmScriptHost, code: &str) -> String {
    let answer = host.evaluate(code).expect("evaluate runs");
    assert!(!answer.ok, "{code} should have raised: {}", answer.output);
    answer.output
}

fn clear_registry() {
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
}

fn open_document(name: &str) {
    ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert_document(name, Document::new());
}

/// The registry's view of one object: the document the viewport meshes.
fn with_object<R>(name: &str, read: impl FnOnce(&Document, ferrocad_core::ObjectId) -> R) -> R {
    let handle = {
        let registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry
            .get(registry.active().expect("an active document"))
            .expect("the document is open")
    };
    let document = handle.lock().unwrap_or_else(|error| error.into_inner());
    let id = document.get_by_name(name).expect("the object exists");
    read(&document, id)
}

#[test]
fn a_view_object_is_the_documents_view_state() {
    let _guard = serial();
    clear_registry();
    with_host(|host| {
        open_document("View");

        // `Part::Box` is registered with the kernel, and the native module registers it
        // on the first `Part.*` call (a document half would have done it at boot).
        let text = output(host, "print(Part.makeBox(1, 1, 1).ShapeType)\n");
        assert_eq!(text, "Solid", "{text}");

        // A fresh object's view state is created on first access, from the shape-colour
        // preferences: FreeCAD's classic `0xCCCCE6FF` when none is stored.
        let text = output(
            host,
            "b = FreeCAD.ActiveDocument.addObject('Part::Box', 'Box')\n\
             vo = b.ViewObject\n\
             print(vo.Visibility, vo.Transparency, vo.DisplayMode, len(vo.ShapeColor), vo.Object.Name)\n\
             print(tuple(round(v, 3) for v in vo.ShapeColor))\n",
        );
        assert_eq!(
            text, "True 0 Flat Lines 4 Box\n(0.8, 0.8, 0.902, 1.0)",
            "{text}"
        );

        // A write is the registry's, so the host's own read — `effective_color`, which
        // `ferrocad_shell_rust` turns into a drawable colour — sees it.
        let text = output(
            host,
            "b.ViewObject.ShapeColor = (1.0, 0.0, 0.0)\n\
             print(tuple(round(v, 3) for v in b.ViewObject.ShapeColor))\n",
        );
        assert_eq!(text, "(1.0, 0.0, 0.0, 1.0)", "{text}");
        let color = with_object("Box", |document, id| {
            ferrocad_core::effective_color(document.object(id).expect("the object"))
                .expect("a view state")
        });
        assert_eq!(color, [1.0, 0.0, 0.0, 1.0]);

        // Transparency is a percentage and clamps, as upstream's does.
        let text = output(
            host,
            "b.ViewObject.Transparency = 150\n\
             print(b.ViewObject.Transparency)\n\
             b.ViewObject.Transparency = -5\n\
             print(b.ViewObject.Transparency)\n",
        );
        assert_eq!(text, "100\n0", "{text}");

        // Hiding is the registry's too: `drawable` is what the host meshes, and a hidden
        // object is not in it.
        let text = output(
            host,
            "b.ViewObject.Visibility = False\n\
             print(b.ViewObject.Visibility)\n",
        );
        assert_eq!(text, "False", "{text}");
        let drawn = with_object("Box", |document, id| {
            ferrocad_core::drawable(document).contains(&id)
        });
        assert!(!drawn, "a hidden object is not drawn");

        // `DisplayMode` is an enumeration, and the pane can ask for its choices.
        let text = output(
            host,
            "print(len(vo.getEnumerationsOfProperty('DisplayMode')), \
                    vo.getEnumerationsOfProperty('DisplayMode')[1])\n\
             b.ViewObject.DisplayMode = 'Shaded'\n\
             print(b.ViewObject.DisplayMode)\n",
        );
        assert_eq!(text, "4 Shaded\nShaded", "{text}");
        let traceback = refused(host, "b.ViewObject.DisplayMode = 'Smooth'\n");
        assert!(
            traceback.contains("not part of the enumeration"),
            "{traceback}"
        );

        // `ShapeMaterial`/`ShapeAppearance` are the same materials the facade re-exports.
        let text = output(
            host,
            "m = vo.ShapeMaterial\n\
             m.DiffuseColor = (0.0, 0.0, 1.0, 1.0)\n\
             vo.ShapeMaterial = m\n\
             print(tuple(round(v, 3) for v in vo.ShapeMaterial.DiffuseColor), len(vo.ShapeAppearance))\n",
        );
        assert_eq!(text, "(0.0, 0.0, 1.0, 1.0) 1", "{text}");

        // One colour per face once there is a shape: a box has six, and each entry is a
        // full RGBA tuple.
        let text = output(
            host,
            "box = FreeCAD.ActiveDocument.addObject('Part::Box', 'Six')\n\
             box.Length = 1\n\
             FreeCAD.ActiveDocument.recompute()\n\
             colors = box.ViewObject.DiffuseColor\n\
             print(len(colors), len(colors[0]))\n",
        );
        assert_eq!(text, "6 4", "{text}");
    });
}

#[test]
fn a_document_object_forwards_view_properties() {
    let _guard = serial();
    clear_registry();
    with_host(|host| {
        open_document("Forward");

        // The kernel also registers `Part::Box` (see the first test).
        let text = output(host, "print(Part.makeBox(1, 1, 1).ShapeType)\n");
        assert_eq!(text, "Solid", "{text}");

        // Upstream allows `box.Visibility = False` in a GUI session; the property is not
        // on the document object, so it is the view object's.
        let text = output(
            host,
            "b = FreeCAD.ActiveDocument.addObject('Part::Box', 'Box')\n\
             b.Visibility = False\n\
             b.ShapeColor = (0.0, 1.0, 0.0)\n\
             print(b.Visibility, tuple(round(v, 3) for v in b.ShapeColor))\n\
             print(b.ViewObject.Visibility, tuple(round(v, 3) for v in b.ViewObject.ShapeColor))\n",
        );
        assert_eq!(
            text, "False (0.0, 1.0, 0.0, 1.0)\nFalse (0.0, 1.0, 0.0, 1.0)",
            "{text}"
        );

        // A real property of the same name is still the document's: `Label` is writable
        // through the doorway, and an unknown name is still an error.
        let traceback = refused(host, "b.NoSuchProperty\n");
        assert!(traceback.contains("NoSuchProperty"), "{traceback}");

        // The document is reachable from the object, which is what the facade's style
        // store keys on — so `Draft_SetStyle`'s properties work here too.
        let text = output(
            host,
            "print(b.Document.Name)\n\
             vo = b.ViewObject\n\
             vo.LineColor = (1.0, 0.0, 0.0)\n\
             vo.LineWidth = 3\n\
             print(tuple(round(v, 3) for v in vo.LineColor), vo.LineWidth)\n\
             print('LineColor' in vo.PropertiesList, vo.listDisplayModes())\n",
        );
        assert_eq!(
            text,
            "Forward\n(1.0, 0.0, 0.0, 1.0) 3\nTrue ['Flat Lines', 'Shaded', 'Wireframe', 'Points']",
            "{text}"
        );
    });
}

#[test]
fn meta_and_settings_are_the_sessions_record() {
    let _guard = serial();
    clear_registry();
    with_host(|host| {
        open_document("Meta");

        let text = output(
            host,
            "doc = FreeCAD.ActiveDocument\n\
             doc.Meta = {'Style': 'a'}\n\
             print(doc.Meta)\n\
             s = doc.settings('FerroCAD.Annotation')\n\
             s.setString('Style', 'b')\n\
             s.setInt('Size', 12)\n\
             s.setFloat('Scale', 1.5)\n\
             s.setBool('Bold', True)\n\
             print(s.getString('Style'), s.getInt('Size'), s.getFloat('Scale'), s.getBool('Bold'))\n\
             print(sorted(doc.Meta.keys()))\n\
             print(s.keys())\n",
        );
        assert_eq!(
            text,
            "{'Style': 'a'}\n\
             b 12 1.5 True\n\
             ['FerroCAD.Annotation.Bold', 'FerroCAD.Annotation.Scale', \
             'FerroCAD.Annotation.Size', 'FerroCAD.Annotation.Style', 'Style']\n\
             ['Bold', 'Scale', 'Size', 'Style']",
            "{text}"
        );

        // Defaults are the caller's, and a missing key is not an error.
        let text = output(
            host,
            "print(s.getString('Missing', 'dflt'), s.getInt('Missing', 7), \
                    s.getFloat('Missing', 2.5), s.getBool('Missing', True))\n",
        );
        assert_eq!(text, "dflt 7 2.5 True", "{text}");

        // Removing a key removes the namespaced entry and nothing else.
        let text = output(
            host,
            "s.remove('Size')\n\
             print(s.keys(), sorted(doc.Meta.keys()))\n",
        );
        assert_eq!(
            text,
            "['Bold', 'Scale', 'Style'] ['FerroCAD.Annotation.Bold', \
             'FerroCAD.Annotation.Scale', 'FerroCAD.Annotation.Style', 'Style']",
            "{text}"
        );

        // A namespace and a key are identifiers, as the CPython binding requires.
        let traceback = refused(host, "doc.settings('bad name')\n");
        assert!(
            traceback.contains("invalid settings namespace"),
            "{traceback}"
        );
        let traceback = refused(host, "s.setString('bad key', 'x')\n");
        assert!(traceback.contains("invalid setting key"), "{traceback}");

        // `Meta` is typed: anything but a dict of strings is refused.
        let traceback = refused(host, "doc.Meta = [1, 2]\n");
        assert!(traceback.contains("Meta must be a dict"), "{traceback}");
    });
}

#[test]
fn a_shape_appearance_entry_writes_back() {
    let _guard = serial();
    clear_registry();
    with_host(|host| {
        open_document("Material");
        // The kernel also registers `Part::Box` (see the first test).
        let text = output(host, "print(Part.makeBox(1, 1, 1).ShapeType)\n");
        assert_eq!(text, "Solid", "{text}");

        // A `ShapeAppearance` entry is a handle, not a copy: writing one field is a
        // view-state write, which is what the CPython binding's slot does.
        let text = output(
            host,
            "b = FreeCAD.ActiveDocument.addObject('Part::Box', 'Box')\n\
             vo = b.ViewObject\n\
             vo.ShapeAppearance[0].DiffuseColor = (0.0, 0.0, 1.0, 1.0)\n\
             print(tuple(round(v, 3) for v in vo.ShapeAppearance[0].DiffuseColor))\n\
             print(tuple(round(v, 3) for v in vo.ShapeColor))\n\
             m = vo.ShapeMaterial\n\
             m.Shininess = 0.25\n\
             print(round(vo.ShapeMaterial.Shininess, 3))\n",
        );
        assert_eq!(
            text, "(0.0, 0.0, 1.0, 1.0)\n(0.0, 0.0, 1.0, 1.0)\n0.25",
            "{text}"
        );
        // The registry is what the viewport draws, so this is the real assertion.
        let color = with_object("Box", |document, id| {
            ferrocad_core::effective_color(document.object(id).expect("the object"))
                .expect("a view state")
        });
        assert_eq!(color, [0.0, 0.0, 1.0, 1.0]);

        // A material the script built itself has no slot and stays local.
        let text = output(
            host,
            "own = FreeCAD.Material()\n\
             own.Shininess = 0.5\n\
             print(round(own.Shininess, 3), round(vo.ShapeMaterial.Shininess, 3))\n",
        );
        assert_eq!(text, "0.5 0.25", "{text}");
    });
}

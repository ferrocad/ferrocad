//! `ferrocad_ui_py` — `FreeCADGui.PySideUic.loadUi` under RustPython (B18b item 2).
//!
//! The module is the last piece `ferrocad_py`'s side had and this half did not: a
//! workbench's Qt Designer `.ui` panel became an `ImportError` here. The state
//! machine is `ferrocad_ui::state`, the same one the PyO3 binding drives, so these
//! tests are about the *surface*: widget lookup by name, the members a class
//! actually has, signals, the dialog result, and the four-part error.

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::PywasmScriptHost;

/// The console namespace is process state; one test at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// Run `f` on RustPython's 16 MiB stack (its compiler recurses; see `properties.rs`).
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

/// The shipped Scale task panel, as an absolute path Python can open.
fn scale_panel() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../python/FerroCADGui/Resources/ui/TaskPanel_Scale.ui");
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn the_module_loads_a_form_and_drives_it() {
    let _guard = serial();
    with_host(|host| {
        // A dialog root, so `accept`/`exec_` exist; one widget of each modelled class.
        let text = output(
            host,
            r#"
import ferrocad_ui_py

source = b'''<ui version="4.0">
 <class>Probe</class>
 <widget class="QDialog" name="Form">
  <property name="windowTitle"><string>Probe</string></property>
  <layout class="QVBoxLayout" name="layout">
   <item><widget class="QLabel" name="label">
    <property name="text"><string>Hello</string></property>
   </widget></item>
   <item><widget class="QSpinBox" name="spin">
    <property name="minimum"><number>0</number></property>
    <property name="maximum"><number>10</number></property>
    <property name="value"><number>5</number></property>
   </widget></item>
   <item><widget class="QCheckBox" name="check"/></item>
   <item><widget class="QComboBox" name="combo">
    <item><property name="text"><string>One</string></property></item>
    <item><property name="text"><string>Two</string></property></item>
   </widget></item>
  </layout>
 </widget>
</ui>'''

form = ferrocad_ui_py.load_ui(source, "probe.ui")
print(form.windowTitle(), form.className(), form.label.text(), form.spin.value(), len(form))
print(form.objectName(), form.layout().count())
"#,
        );
        assert_eq!(text, "Probe Probe Hello 5 5\nForm 4", "{text}");

        // A spin box clamps, and its setter emits `valueChanged`.
        let text = output(
            host,
            "form.spin.setValue(50)\n\
             print(form.spin.value())\n\
             seen = []\n\
             form.spin.valueChanged.connect(lambda value: seen.append(value))\n\
             form.spin.setValue(3)\n\
             print(seen, form.spin.value(), len(form.spin.valueChanged))\n",
        );
        assert_eq!(text, "10\n[3] 3 1", "{text}");

        // Checked state and the combo's index/text.
        let text = output(
            host,
            "form.check.setChecked(True)\n\
             print(form.check.isChecked())\n\
             print(form.combo.currentText(), form.combo.count(), form.combo.itemText(1))\n\
             form.combo.setCurrentIndex(1)\n\
             print(form.combo.currentText(), form.combo.currentIndex())\n",
        );
        assert_eq!(text, "True\nOne 2 Two\nTwo 1", "{text}");

        // `findChild` resolves a name, a class name and a class object.
        let text = output(
            host,
            "print(form.findChild('spin').objectName(), form.findChild('QComboBox').objectName())\n\
             print(form.findChild('nothing') is None)\n",
        );
        assert_eq!(text, "spin combo\nTrue", "{text}");

        // A dialog reports its result without an event loop, and `accept` emits.
        let text = output(
            host,
            "accepted = []\n\
             form.accepted.connect(lambda: accepted.append(True))\n\
             print(form.exec_())\n\
             form.accept()\n\
             print(form.exec_(), form.result(), accepted)\n",
        );
        assert_eq!(text, "0\n1 1 [True]", "{text}");

        // A member the class does not model names form, class, widget and member;
        // an unknown widget names the ones that exist.
        let traceback = refused(host, "form.spin.text()\n");
        assert!(
            traceback.contains("QSpinBox 'spin' has no attribute 'text' (form probe.ui)"),
            "{traceback}"
        );
        let traceback = refused(host, "form.nope\n");
        assert!(traceback.contains("has no widget 'nope'"), "{traceback}");

        // `UiLoadError` is a `ValueError`, as in the CPython module.
        let text = output(
            host,
            "print(issubclass(ferrocad_ui_py.UiLoadError, ValueError))\n",
        );
        assert_eq!(text, "True", "{text}");
    });
}

#[test]
fn the_facade_loads_a_shipped_panel() {
    let _guard = serial();
    let panel = scale_panel();
    with_host(move |host| {
        let text = output(
            host,
            &format!(
                "import FreeCADGui\n\
                 form = FreeCADGui.PySideUic.loadUi({panel:?})\n\
                 print(form.windowTitle(), form.objectName(), form.className())\n\
                 print(form.xValue.value(), form.isCopy.isChecked(), form.lock.isChecked())\n\
                 print(form.findChild('gridLayout').className())\n"
            ),
        );
        assert_eq!(
            text, "Scale Form Form\n1.0 False False\nQGridLayout",
            "{text}"
        );
    });
}

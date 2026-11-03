//! Shell tests: forms.

use super::*;

/// Q3b: the shell hosts a rendered [`FormDialog`] as its modal, above the
/// workspace and with the form's stable selectors. Only one modal (and one
/// backdrop) exists at a time, and `closeDialog` releases it.
#[gpui::test]
fn a_form_dialog_opens_over_the_shell(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);

    let opened = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
    });
    assert!(opened, "the form opened");
    draw(&mut cx);

    for selector in [
        "form-Inline",
        "form-Inline-flag",
        "form-Inline-panel",
        "form-Inline-accept",
        "form-Inline-reject",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} is rendered"
        );
    }
    assert!(
        cx.debug_bounds("dialog-backdrop").is_none(),
        "the form draws the only backdrop"
    );
    assert!(
        cx.debug_bounds("palette").is_none(),
        "no palette is drawn behind the form"
    );

    // The command dialog and the palette both refuse while the form owns the
    // shell, so a second modal cannot stack on the first.
    let second = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_form(inline_form(), "Other", cx)
        })
    });
    assert!(!second, "one modal at a time");
    // `closeDialog` hides it and records no result.
    let closed = cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.close_form(cx)));
    assert!(closed, "close_form released the open form");
    assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.last_form_result()),
        None,
        "close records no result"
    );
    draw(&mut cx);
    assert!(cx.debug_bounds("form-Inline").is_none());
}

/// Q3b: Esc rejects and Enter accepts the open form; both are routed through
/// the shell's key path because the form itself is not focusable.
#[gpui::test]
fn escape_rejects_and_enter_accepts_the_open_form(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);

    let opened = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
    });
    assert!(opened);
    draw(&mut cx);

    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.last_form_result()),
        Some(FormResult::Rejected)
    );
    assert!(cx.debug_bounds("form-Inline").is_none());

    let opened = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
    });
    assert!(opened, "a closed form can be reopened");
    draw(&mut cx);
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.last_form_result()),
        Some(FormResult::Accepted)
    );
    assert!(cx.debug_bounds("form-Inline").is_none());
}

/// Q3b: a registry accelerator must not fire over the form modal, even though
/// it would run when the shell root has focus normally.
#[gpui::test]
fn a_command_accelerator_does_not_fire_over_a_form(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);

    let script = r#"
import FreeCADGui
class _FormShortcut:
    def GetResources(self):
        return {"MenuText": "Form shortcut", "Accel": "Ctrl+Alt+Shift+Y"}
    def Activated(self):
        return {"status": "shortcut ran over the form"}
if "Test_FormShortcut" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_FormShortcut", _FormShortcut())
"#;
    let registered = cx.update(|_window, _cx| crate::python::evaluate(script));
    assert!(registered.unwrap().ok);
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

    let opened = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
    });
    assert!(opened);
    draw(&mut cx);
    cx.simulate_keystrokes("ctrl-alt-shift-y");
    draw(&mut cx);

    let status = model.lock().unwrap().status.clone();
    assert_ne!(status, "shortcut ran over the form");
    assert!(
        shell.read_with(&cx, |shell, _| shell.form_open()),
        "the form still owns the shell"
    );
    assert!(cx.debug_bounds("form-Inline").is_some());
}

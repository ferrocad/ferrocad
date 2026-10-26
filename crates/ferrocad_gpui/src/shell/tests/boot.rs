//! Shell tests: boot.

use super::*;

/// A *stored* visibility switch reaches the first window.
///
/// This is the payoff of "the feature owns the switch": a provider's `activate`
/// no longer writes `enabled`, so the boot appearance query returns the user's
/// values instead of a feature that document activation turned on (opening the
/// sample document used to enable the grid, which is why the boot path stripped
/// visibility for a while).
#[test]
fn a_stored_visibility_switch_reaches_the_first_window() {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // First boot puts the facade on `sys.path`, which the writes below need.
    let _ = boot_shell();
    let result = crate::python::evaluate(
        "import FerroCAD\n\
         FerroCAD.Grid.enabled = False\n\
         FerroCAD.Axis.enabled = False\n\
         FerroCAD.Gizmo.enabled = False\n",
    )
    .expect("the interpreter ran");
    assert!(result.ok, "{}", result.output);

    // Deliberately not `boot_shell`: this boot must see the switches just written.
    let model = Shell::boot().expect("boot shell");
    {
        let view = model.lock().unwrap().view.clone();
        assert!(!view.grid, "the stored grid switch reaches the model");
        assert!(
            !view.axis.enabled,
            "the stored axis switch reaches the model"
        );
        assert!(
            !view.gizmo.enabled,
            "the stored gizmo switch reaches the model"
        );
    }

    // Leave the shared interpreter on the built-in defaults for other tests.
    reset_grid_state();
    let _ = crate::python::evaluate("import FerroCAD\nFerroCAD.Gizmo.reset()\n");
}

/// S1 end-to-end, headless: boot the interpreter, read the sample document
/// through it, round-trip the console, and render the shell.
#[gpui::test]
fn shell_boots_lists_the_sample_document_and_is_renderable(cx: &mut TestAppContext) {
    // Tests share one CPython instance (module-global state); serialize them.
    let _guard = crate::PYTHON_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let model = boot_shell();
    {
        let m = model.lock().unwrap();
        let doc = m
            .tree
            .iter()
            .find(|d| d.name == "Shell")
            .expect("Shell document");
        let names: Vec<&str> = doc.objects.iter().map(|o| o.name.as_str()).collect();
        assert!(names.contains(&"Params"), "objects: {names:?}");
        assert!(names.contains(&"Derived"));
        assert!(m.console.iter().any(|l| l.contains("hello from Python")));
    }

    // The property editor is fed from the live model.
    let props = crate::python::properties("Params").expect("properties");
    assert!(props.iter().any(|p| p.name == "Length"));

    // The console round-trips a snippet through the interpreter.
    let out = crate::python::evaluate("len(doc.Objects)").expect("evaluate");
    assert!(out.ok, "eval failed: {}", out.output);
    assert_eq!(out.output, "3");

    // The shell renders headlessly (no display server, no GPU).
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    // The console lives in Dev Tools now: open it, then drive the same focus path.
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::CONSOLE);
    assert!(
        dev_cx.debug_bounds("console-text-thumb").is_some(),
        "the console text area carries a scrollbar"
    );

    // Typing in the console input and pressing Enter runs the line through the
    // interpreter (S5 previewed): the log gains the echoed line and the result,
    // and the field is cleared.
    let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
    dev_cx.update(|window, cx| window.focus(&console, cx));
    dev_cx.simulate_input("len(doc.Objects)");
    dev_cx.simulate_keystrokes("enter");
    {
        let m = model.lock().unwrap();
        assert!(
            m.console.iter().any(|l| l == ">>> len(doc.Objects)"),
            "console: {:?}",
            m.console
        );
        assert!(m.console.iter().any(|l| l == "3"), "console: {:?}", m.console);
    }
}

/// Slice E: an uncontrolled property field commits on Enter, inside a named
/// transaction, and the change propagates through recompute.
#[gpui::test]
fn editing_a_property_commits_in_a_transaction(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let model = boot_shell();
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    // Select Params so the editor builds one field per editable property. The
    // property editor is in Dev Tools now, so the window must be open and drawn
    // before the field exists.
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            cx.notify();
        });
    });
    // The property editor is a collapsed pane now: expand it before typing, or the
    // field would exist but never be painted (and so never take focus).
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);

    let field = shell.read_with(&cx, |shell, _| {
        shell
            .prop_inputs
            .get("Params.Length")
            .expect("Length field")
            .state
            .clone()
    });
    // The field is rendered in the Dev Tools window; type there.
    dev_cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    dev_cx.simulate_keystrokes("cmd-a");
    dev_cx.simulate_input("25 mm");
    dev_cx.simulate_keystrokes("enter");

    // The selected object's row reflects the committed value...
    let length = shell.read_with(&cx, |shell, _| {
        shell
            .model
            .lock()
            .unwrap()
            .properties
            .iter()
            .find(|row| row.name == "Length")
            .map(|row| row.value.clone())
    });
    assert!(
        length.as_deref().unwrap_or_default().contains("25"),
        "Length = {length:?}"
    );

    // ...and recompute propagated it to the expression-driven other object.
    let derived = crate::python::properties("Derived").expect("properties");
    let result = derived
        .iter()
        .find(|row| row.name == "Result")
        .map(|row| row.value.clone())
        .unwrap_or_default();
    assert!(result.contains("50"), "Derived.Result = {result:?}");
}

/// A document object's own enum and colour properties get the same controls as
/// the `Settings/*` rows. Both surfaces share `properties_rows_panel`, so the
/// row data is the whole difference: the Python layer must supply an
/// enumeration's own options and serialise a colour as `#rrggbb`.
#[gpui::test]
fn a_document_enum_and_colour_use_the_rich_widgets(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let added = crate::python::evaluate(
        "doc.Params.addProperty('App::PropertyColor', 'Color', 'Base', 'Surface colour')\n\
         doc.Params.Color = [(0.26, 0.55, 0.85, 1.0)]\n\
         doc.recompute()\n\
         'ok'",
    )
    .expect("the interpreter ran");
    assert!(added.ok, "{}", added.output);

    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            cx.notify();
        });
    });
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);

    let rows = shell.read_with(&cx, |shell, _| {
        shell.model.lock().unwrap().properties.clone()
    });
    let mode = rows
        .iter()
        .find(|row| row.name == "Mode")
        .expect("Mode row");
    assert_eq!(mode.kind, "enum");
    assert_eq!(
        mode.choices,
        ["Fast".to_string(), "Accurate".to_string()],
        "the row carries the property's own options"
    );
    let color = rows
        .iter()
        .find(|row| row.name == "Color")
        .expect("Color row");
    assert_eq!(color.kind, "color");
    assert_eq!(
        color.value, "#428cd9",
        "the RGBA tuple is serialised as a hex swatch value"
    );

    assert!(
        dev_cx
            .debug_bounds("prop-enum-Params.Mode-select")
            .is_some(),
        "the enumeration is a select box"
    );
    assert!(
        dev_cx.debug_bounds("color-Params.Color-swatch").is_some(),
        "the colour is a swatch"
    );

    // The select box commits through the normal property path.
    click_at(&mut dev_cx, "prop-enum-Params.Mode-select");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        dev_cx.debug_bounds("choose-Params.Mode-choices").is_some(),
        "the dropdown lists Fast and Accurate"
    );
    click_at(&mut dev_cx, "choose-Params.Mode-choice-1");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let mode = crate::python::properties("Params")
        .expect("properties")
        .into_iter()
        .find(|row| row.name == "Mode")
        .map(|row| row.value)
        .unwrap_or_default();
    assert_eq!(mode, "Accurate", "the pick committed the enumeration");

    // The palette commits a hex colour and keeps the property's alpha.
    click_at(&mut dev_cx, "color-Params.Color-swatch");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    click_at(&mut dev_cx, "color-Params.Color-pick-ef4444");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let color = crate::python::properties("Params")
        .expect("properties")
        .into_iter()
        .find(|row| row.name == "Color")
        .map(|row| row.value)
        .unwrap_or_default();
    assert_eq!(color, "#ef4444", "the palette committed the colour");
    let field = shell.read_with(&cx, |shell, cx| {
        shell
            .prop_inputs
            .get("Params.Color")
            .map(|input| input.state.read(cx).text().to_string())
    });
    assert_eq!(
        field.as_deref(),
        Some("#ef4444"),
        "the hex field follows the palette"
    );
}

/// A document quantity declares no range, so its handle is dragged **directly**:
/// a scale appears while the button is held, vertical travel changes the value
/// (two doublings is 96 px), and the release commits with the unit.
#[gpui::test]
fn a_document_quantity_is_scrubbed_directly(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            cx.notify();
        });
    });
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);

    let row = shell.read_with(&cx, |shell, _| {
        shell
            .model
            .lock()
            .unwrap()
            .properties
            .iter()
            .find(|row| row.name == "Length")
            .map(|row| (row.kind.clone(), row.value.clone(), row.min, row.max))
    });
    assert_eq!(
        row.as_ref().map(|(kind, ..)| kind.as_str()),
        Some("quantity")
    );
    assert_eq!(
        row.as_ref().map(|(_, value, ..)| value.as_str()),
        Some("10 mm")
    );
    assert_eq!(
        row.as_ref().map(|(_, _, min, max)| (*min, *max)),
        Some((None, None)),
        "the property declares no range"
    );

    let handle = dev_cx
        .debug_bounds("scrub-Params.Length")
        .expect("an unbounded quantity gets a drag handle");
    assert!(
        dev_cx
            .debug_bounds("slider-Params.Length-trigger")
            .is_none(),
        "no invented range means no slider"
    );

    // Press the handle and travel 96 px up (two doublings). The scale must be
    // on screen while the button is held, before any release.
    let press = handle.center();
    let release = point(press.x, press.y - px(2.0 * scrub::DOUBLING_PX));
    dev_cx.simulate_mouse_down(press, MouseButton::Left, gpui::Modifiers::default());
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        dev_cx.debug_bounds("property-scrub-scale").is_some(),
        "the scale appears as soon as the handle is held"
    );
    let scale = dev_cx
        .debug_bounds("property-scrub-scale")
        .expect("the scale is on screen");
    assert!(
        !scale.contains(&press),
        "the scale never covers the handle it is reporting on (scale {scale:?}, press {press:?})"
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).scrub.is_some()),
        "the gesture started on the press"
    );

    dev_cx.simulate_mouse_move(release, MouseButton::Left, gpui::Modifiers::default());
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    dev_cx.simulate_mouse_up(release, MouseButton::Left, gpui::Modifiers::default());
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        dev_cx.debug_bounds("property-scrub-scale").is_none(),
        "the scale closes on release"
    );

    let value = crate::python::properties("Params")
        .expect("properties")
        .into_iter()
        .find(|row| row.name == "Length")
        .map(|row| row.value)
        .unwrap_or_default();
    assert_eq!(value, "40 mm", "four times 10 mm, with the row's unit");
}

/// A document quantity's unit is the suffix of its displayed value; a schema
/// row declares it. Either way the dial commit keeps it.
#[test]
fn a_quantity_row_reports_the_unit_its_commit_carries() {
    assert_eq!(row_unit(&quantity_row("10 mm", None, None)), "mm");
    assert_eq!(row_unit(&quantity_row("30 deg", None, None)), "deg");
    assert_eq!(row_unit(&quantity_row("42.3", None, None)), "");

    let mut declared = quantity_row("10 mm", None, None);
    declared.unit = Some("mm".into());
    assert_eq!(row_unit(&declared), "mm");
}

/// A2: the shell owns the selection, pushes it to `FreeCADGui.Selection`, and a
/// multi-selection enables the built-ins. The inspector's Ctrl/Shift-click path
/// is `toggle_select`, so this drives the same methods the click handler does.
#[gpui::test]
fn a_multi_selection_is_pushed_and_enables_the_built_ins(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    // Nothing selected: Delete and Move are visible but disabled.
    assert!(!command_enabled("App_Delete"));
    assert!(!command_enabled("App_Move"));
    assert!(!crate::python::commands().unwrap().is_empty());

    // A plain select replaces; a toggle extends (the inspector's modifier path).
    shell.update(cx, |shell, cx| {
        shell.select("Params", cx);
        shell.toggle_select("Derived", cx);
    });
    {
        let m = model.lock().unwrap();
        assert_eq!(m.selection.len(), 2, "selection: {:?}", m.selection);
        assert_eq!(
            m.selected.as_deref(),
            Some("Derived"),
            "the last click is the property editor's subject"
        );
        assert!(m.status.contains('2'), "status: {}", m.status);
    }
    assert!(
        command_enabled("App_Delete"),
        "the snapshot reached the facade"
    );
    assert!(command_enabled("App_Move"));

    // Toggling an object off keeps the rest, and the primary follows.
    shell.update(cx, |shell, cx| shell.toggle_select("Derived", cx));
    {
        let m = model.lock().unwrap();
        assert_eq!(m.selection.len(), 1);
        assert_eq!(m.selected.as_deref(), Some("Params"));
    }

    // Clearing empties the facade too, so IsActive flips back.
    shell.update(cx, |shell, cx| shell.clear_selection(cx));
    assert!(!command_enabled("App_Delete"));
    assert!(model.lock().unwrap().selection.is_empty());
}

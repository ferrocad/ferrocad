//! Shell tests: devtools.

use super::*;

/// P0.5/D3: the main window keeps the frame, the viewport and the status line.
/// The panels are in the second window, not merely hidden.
#[gpui::test]
fn the_main_window_carries_only_the_viewport_and_status(cx: &mut TestAppContext) {
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
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    assert!(cx.debug_bounds("main-shell").is_some());
    assert!(cx.debug_bounds("main-viewport").is_some());
    assert!(
        cx.debug_bounds("dev-command-list").is_none(),
        "the command bar left the main window"
    );
    assert!(cx.debug_bounds("tree-panel").is_none());
    assert!(cx.debug_bounds("properties-panel").is_none());
    assert!(cx.debug_bounds("console-panel").is_none());
}

/// P0.5/D2/D5: the Dev Tools window renders the four panels over the same shell and
/// lists every command with its live enablement (disabled rows stay visible).
#[gpui::test]
fn the_dev_tools_window_lists_commands_with_enablement(cx: &mut TestAppContext) {
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

    // The built-ins are present and disabled until a selection.
    let commands = model.lock().unwrap().commands.clone();
    assert!(!commands.is_empty());
    let delete = commands
        .iter()
        .find(|command| command.id == "App_Delete")
        .expect("App_Delete is registered");
    assert!(!delete.enabled, "nothing is selected yet");
    assert!(commands.iter().all(|command| !command.id.is_empty()));

    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    // The window is an accordion: Commands, Model and Properties start open and
    // share the height; the rest are headers only.
    assert!(dev_cx.debug_bounds("dev-command-list").is_some());
    for selector in [
        "dev-header-providers",
        "dev-header-gpu",
        "dev-header-commands",
        "dev-header-model",
        "dev-header-properties",
        "dev-header-console",
    ] {
        assert!(
            dev_cx.debug_bounds(selector).is_some(),
            "{selector} is a header"
        );
    }
    assert!(
        dev_cx.debug_bounds("tree-panel").is_some(),
        "the model tree is open beside the commands"
    );
    assert!(
        dev_cx.debug_bounds("properties-panel").is_some(),
        "the property editor is open beside the model tree"
    );
    assert!(dev_cx.debug_bounds("console-panel").is_none());
    assert!(
        dev_cx.debug_bounds("dev-command-App_Delete").is_some(),
        "a disabled command is still listed"
    );

    // Toggling a header adds a pane without closing the others.
    click_at(&mut dev_cx, "dev-header-console");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(dev_cx.debug_bounds("console-panel").is_some());
    assert!(
        dev_cx.debug_bounds("dev-command-list").is_some(),
        "opening one pane leaves the others open"
    );

    // Narrowing the set is still available when a pane needs the height.
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);
    assert!(dev_cx.debug_bounds("properties-panel").is_some());
    assert!(
        dev_cx.debug_bounds("dev-properties-thumb").is_some(),
        "the property pane carries the scrollbar gpui never paints"
    );
    assert!(
        dev_cx.debug_bounds("dev-command-list").is_none(),
        "open_only closed the other panes"
    );
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::COMMANDS);
    assert!(dev_cx.debug_bounds("dev-command-list").is_some());
    assert!(dev_cx.debug_bounds("dev-commands-thumb").is_some());

    // Selecting in the inspector feeds the property editor from the same entity.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let has_field = cx.update(|_window, cx| {
        shell.read(cx).prop_inputs.contains_key("Params.Length")
    });
    assert!(has_field, "the Dev Tools property editor built the field");
    let delete_enabled = model
        .lock()
        .unwrap()
        .commands
        .iter()
        .find(|command| command.id == "App_Delete")
        .map(|command| command.enabled)
        .unwrap_or(false);
    assert!(delete_enabled, "the list follows IsActive");

    // A modal opens in the main window (the shell owns it); Esc from the Dev
    // Tools window must still reach it instead of being swallowed.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_Move", cx))
    });
    assert!(
        cx.update(|_window, cx| shell.read(cx).dialog.is_some()),
        "the move dialog opened"
    );
    dev_cx.simulate_keystrokes("escape");
    assert!(
        cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
        "Esc in Dev Tools cancelled the modal"
    );

    // A command run from the shell updates the same model the window reads.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ViewWorkplane", cx))
    });
    assert_eq!(model.lock().unwrap().status, "view: no viewport");
}

/// Clicking a header toggles only that pane; the other open panes keep their
/// bodies. A second click on the same header closes it again.
#[gpui::test]
fn a_dev_tools_header_click_toggles_only_its_pane(cx: &mut TestAppContext) {
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

    assert!(dev_cx.debug_bounds("dev-command-list").is_some());

    click_at(&mut dev_cx, "dev-header-providers");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        dev_cx.debug_bounds("dev-providers").is_some(),
        "the clicked header opens its pane"
    );
    assert!(
        dev_cx.debug_bounds("dev-command-list").is_some(),
        "the other open panes stay open"
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).dev_accordion.is_open(dev_section::PROVIDERS)),
        "the click reached the shell's state"
    );

    click_at(&mut dev_cx, "dev-header-providers");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        dev_cx.debug_bounds("dev-providers").is_none(),
        "clicking the open header collapses it"
    );
    assert!(
        dev_cx.debug_bounds("dev-header-providers").is_some(),
        "the header itself stays"
    );
    assert!(
        dev_cx.debug_bounds("dev-command-list").is_some(),
        "closing one pane leaves the others open"
    );
}

/// P0.5/D4/D5: closing and reopening preserves the model, the selection and the
/// console; the second window never owns a copy.
#[gpui::test]
fn toggling_dev_tools_preserves_the_model(cx: &mut TestAppContext) {
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

    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
    let (console_len, selection) = {
        let m = model.lock().unwrap();
        (m.console.len(), m.selection.clone())
    };

    let _dev_cx = open_dev_tools(&mut cx, &shell);
    assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

    let status = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.toggle_dev_tools(cx))
    });
    assert_eq!(status, "dev tools: closed");
    assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
    assert!(
        cx.windows().len() == 1,
        "only the main window remains: {:?}",
        cx.windows()
    );

    // Nothing in the document, the selection or the transcript changed.
    {
        let m = model.lock().unwrap();
        assert_eq!(m.console.len(), console_len);
        assert_eq!(m.selection, selection);
        // The tree also carries the synthetic `Settings` root (S1); this
        // assertion is about documents, so exclude it by name.
        let documents: Vec<&str> = m
            .tree
            .iter()
            .map(|doc| doc.name.as_str())
            .filter(|name| *name != "Settings")
            .collect();
        assert_eq!(
            documents.len(),
            1,
            "the sample document is intact; open documents: {documents:?}"
        );
    }

    // Reopening builds a new view around the same shell entity and model.
    let _dev_cx = open_dev_tools(&mut cx, &shell);
    assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
    let shared = cx.update(|_window, cx| {
        shell.read(cx).model.lock().unwrap().selection.clone()
    });
    assert_eq!(shared, selection);
}

/// The Dev Tools providers panel switches a backend without opening the Mods
/// dialog; the header popover and this panel share `activate_provider`.
#[gpui::test]
fn the_dev_tools_providers_panel_switches_a_backend(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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

    // The providers pane is collapsed by default; its header opens it.
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROVIDERS);
    assert!(
        dev_cx.debug_bounds("dev-providers").is_some(),
        "the providers panel is in the Dev Tools window"
    );
    assert!(
        dev_cx.debug_bounds("dev-providers-thumb").is_some(),
        "the pane carries a scrollbar"
    );
    assert!(
        dev_cx.debug_bounds("dev-provider-grid-coin").is_some(),
        "the grid backends are offered"
    );
    assert!(dev_cx.debug_bounds("dev-provider-axis-coin").is_some());

    click_at(&mut dev_cx, "dev-provider-grid-coin");
    let summary = cx
        .update(|_window, cx| shell.read(cx).grid_summary.clone())
        .expect("the switch refreshed the summary");
    assert_eq!(summary.backend.as_deref(), Some("coin"));
    assert!(
        summary
            .providers
            .iter()
            .any(|provider| provider.backend == "coin" && provider.active),
        "the Dev Tools switch follows the registry"
    );
    // The losing backend's command stays listed but is not runnable; the switch is
    // the only way to reach it, so the command list must follow the active backend.
    let inactive = cx.update(|_window, cx| {
        crate::chrome::inactive_provider_commands(shell.read(cx).provider_features())
    });
    assert!(
        inactive.contains_key("GridOverlay_Settings"),
        "the native grid command is inactive once coin owns the feature"
    );
    assert!(!inactive.contains_key("GridOverlayCoin_Settings"));

    // Leave the shared interpreter on the default for the other tests.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("ferrocad-provider:grid:ferrocad", cx)
        })
    });
    let inactive = cx.update(|_window, cx| {
        crate::chrome::inactive_provider_commands(shell.read(cx).provider_features())
    });
    assert!(inactive.contains_key("GridOverlayCoin_Settings"));
    assert!(!inactive.contains_key("GridOverlay_Settings"));
}

/// The Dev Tools GPU panel lists `Auto` plus every adapter and marks the
/// persisted choice, using synthetic adapters so the test needs no GPU.
#[gpui::test]
fn the_dev_tools_gpu_panel_marks_the_active_row(cx: &mut TestAppContext) {
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

    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::GPU);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _| {
            shell.gpu_preference = Some("adapter:Llvm".to_string());
            shell.gpu_adapters = Some(vec![
                AdapterSummary {
                    name: "Llvm".to_string(),
                    backend: "vulkan".to_string(),
                    device_type: "cpu".to_string(),
                    driver: "llvmpipe".to_string(),
                },
                AdapterSummary {
                    name: "Discrete".to_string(),
                    backend: "vulkan".to_string(),
                    device_type: "discrete".to_string(),
                    driver: "mesa".to_string(),
                },
            ]);
        })
    });
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    assert!(
        dev_cx.debug_bounds("gpu-panel").is_some(),
        "the GPU panel is between Providers and Commands"
    );
    assert!(dev_cx.debug_bounds("dev-gpu-auto").is_some());
    assert!(dev_cx.debug_bounds("dev-gpu-adapter-0").is_some());
    assert!(dev_cx.debug_bounds("dev-gpu-adapter-1").is_some());
    assert!(
        dev_cx.debug_bounds("dev-gpu-adapter-0-active").is_some(),
        "the persisted adapter is marked active"
    );
    assert!(dev_cx.debug_bounds("dev-gpu-adapter-1-active").is_none());
    assert!(dev_cx.debug_bounds("dev-gpu-auto-active").is_none());
    assert!(
        dev_cx.debug_bounds("gpu-in-use").is_some(),
        "the panel reports that there is no viewport"
    );
}

/// A malformed persisted value falls back to `Auto` and says why in the console.
#[gpui::test]
fn an_unknown_gpu_setting_falls_back_to_auto(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    write_gpu_preference("not-a-real-form");
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    let (choice, console) = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let choice = shell.stored_gpu_choice(cx);
            let console = shell.model.lock().unwrap().console.clone();
            (choice, console)
        })
    });
    assert_eq!(choice, AdapterChoice::Auto);
    assert!(
        console
            .iter()
            .any(|line| line.contains("viewport GPU") && line.contains("using auto")),
        "console: {console:?}"
    );

    write_gpu_preference("auto");
}

/// Selecting a row persists its value and recreates the viewport. Skipped where
/// the machine offers no adapter, because the recreate path needs one.
#[gpui::test]
fn a_gpu_panel_row_persists_the_choice(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let adapters = ferrocad_wgpu::adapter::adapters();
    let Some(adapter) = adapters.first() else {
        return;
    };
    let setting = format!("adapter:{}", adapter.name);

    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command(&format!("ferrocad-gpu:{setting}"), cx)
        })
    });
    assert_eq!(
        crate::python::gpu_adapter().expect("the stored setting"),
        setting
    );
    assert_eq!(
        model.lock().unwrap().status,
        format!("viewport GPU: {setting} (zero-copy keeps the window adapter)")
    );

    write_gpu_preference("auto");
}

/// P0.5/D4: `Dev_ToggleTools` is registered and its headless path reports a status
/// instead of pretending a window exists.
#[gpui::test]
fn the_dev_tools_command_has_a_headless_status(_cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    drop(model);

    let outcome = crate::commands::run(crate::dev_tools::TOGGLE_COMMAND).expect("the bridge");
    assert!(outcome.ok, "{}", outcome.error);
    assert!(
        outcome.status.contains("dev tools"),
        "status: {}",
        outcome.status
    );
    let commands = crate::python::commands().expect("the command list");
    assert!(
        commands
            .iter()
            .any(|command| command.id == crate::dev_tools::TOGGLE_COMMAND),
        "the toggle is in the registry"
    );
}

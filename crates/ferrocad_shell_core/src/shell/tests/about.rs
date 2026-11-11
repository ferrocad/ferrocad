//! Shell tests: the About card (what this build composed).

use super::*;

/// Boot with the composition a host installs, so the card has something to report.
///
/// `boot_shell` installs no composition — a test is not a host — which is itself the
/// "not reported" case; this variant is the reported one.
fn boot_with_composition(cx: &mut TestAppContext) -> Arc<Mutex<ShellModel>> {
    crate::register_test_core();
    cx.update(|cx| {
        crate::app::install_composition(
            cx,
            Composition {
                app: "FerroCAD: Test".to_string(),
                platform: "desktop".to_string(),
                runtimes: vec!["cpython".to_string()],
                addons: vec!["toy.wasm".to_string()],
            },
        );
        crate::app::install_backend(cx, Box::new(ferrocad_shell_python::engine()));
        Shell::boot(cx).expect("boot shell")
    })
}

#[gpui::test]
fn the_boot_reports_the_composition(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_with_composition(cx);
    let composition = model.lock().unwrap().composition.clone();
    assert_eq!(
        composition,
        Composition {
            app: "FerroCAD: Test".to_string(),
            platform: "desktop".to_string(),
            runtimes: vec!["cpython".to_string()],
            addons: vec!["toy.wasm".to_string()],
        },
        "the model carries what the host installed"
    );
}

/// The app menu is where About lives (it has no viewport surface), and its row opens
/// the card with the boot's inventory in it.
#[gpui::test]
fn the_app_menu_opens_the_about_card(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_with_composition(cx);
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();
    draw(&mut cx);

    // A title-bar-only window, as the other app-menu tests use: the popover can be
    // clicked without the viewport in the way.
    let size = cx.update(|window, _| window.viewport_size());
    let bar_shell = shell.clone();
    let bar = cx
        .cx
        .open_window(size, move |_window, _cx| TitleBarHarness {
            shell: bar_shell.clone(),
        });
    let mut bar_cx = VisualTestContext::from_window(bar.into(), &cx);
    draw(&mut bar_cx);
    click_at(&mut bar_cx, "app-menu");
    draw(&mut bar_cx);
    assert!(
        bar_cx.debug_bounds("app-row-ferrocad-about").is_some(),
        "About is in the app menu"
    );

    click_at(&mut bar_cx, "app-row-ferrocad-about");
    assert!(
        !shell.read_with(&cx, |shell, _| shell.popover_open("app")),
        "choosing a row dismisses the menu"
    );

    let (node, rows) = shell.read_with(&cx, |shell, _| {
        let card = shell
            .settings_card
            .as_ref()
            .expect("the About card is open");
        (card.node.clone(), card.rows.clone())
    });
    assert_eq!(node, crate::shell::panels::about::ABOUT);
    let value = |name: &str| {
        rows.iter()
            .find(|row| row.name == name)
            .map(|row| row.value.clone())
    };
    assert_eq!(value("Application").as_deref(), Some("FerroCAD: Test"));
    assert_eq!(value("Script halves").as_deref(), Some("cpython"));
    assert_eq!(value("Addons").as_deref(), Some("toy.wasm"));
    assert!(value("Version").is_some(), "the build names its version");
    assert!(value("Engine").is_some(), "and the engine that answered");
    assert!(value("Mods").is_some(), "and the workbench count");
}

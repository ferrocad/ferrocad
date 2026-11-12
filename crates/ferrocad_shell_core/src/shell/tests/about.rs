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

/// A store that records the last save, so a test can assert what a click persisted.
#[derive(Clone, Default)]
struct RecordingPrefs(std::rc::Rc<std::cell::RefCell<Option<crate::types::Prefs>>>);

impl crate::types::PrefsStore for RecordingPrefs {
    fn load(&self) -> crate::types::Prefs {
        self.0.borrow().clone().unwrap_or_default()
    }

    fn save(&self, prefs: &crate::types::Prefs) -> Result<(), String> {
        *self.0.borrow_mut() = Some(prefs.clone());
        Ok(())
    }

    fn location(&self) -> String {
        "test store".to_string()
    }
}

/// Boot with the composition, a `PrefsReport` and a recording store.
fn boot_with_prefs(
    cx: &mut TestAppContext,
    mods: &[&str],
) -> (Arc<Mutex<ShellModel>>, RecordingPrefs) {
    crate::register_test_core();
    let store = RecordingPrefs::default();
    let saved = store.clone();
    let model = cx.update(|cx| {
        crate::app::install_composition(
            cx,
            Composition {
                app: "FerroCAD: Test".to_string(),
                platform: "desktop".to_string(),
                runtimes: vec!["cpython".to_string()],
                addons: Vec::new(),
            },
        );
        crate::app::install_prefs(
            cx,
            crate::types::PrefsReport {
                prefs: crate::types::Prefs::default(),
                mods: mods.iter().map(|name| name.to_string()).collect(),
                store: String::new(),
            },
            Some(Box::new(store)),
        );
        crate::app::install_backend(cx, Box::new(ferrocad_shell_python::engine()));
        Shell::boot(cx).expect("boot shell")
    });
    (model, saved)
}

/// The card offers one row per mod and per provider, and the clicks go through the
/// generated ids and the host's store (B17a/B19d).
#[gpui::test]
fn the_card_turns_mods_and_providers_off(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (model, saved) = boot_with_prefs(cx, &["GridOverlay", "Fasteners"]);
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    shell.update(&mut cx, |shell, cx| shell.run_command("ferrocad-about", cx));
    let rows_of = |cx: &VisualTestContext| {
        let shell = shell.clone();
        shell.read_with(cx, |shell, _| {
            shell
                .settings_card
                .as_ref()
                .expect("the About card is open")
                .rows
                .clone()
        })
    };

    let rows = rows_of(&cx);
    assert_eq!(
        rows.iter()
            .find(|row| row.name == "Preferences")
            .map(|row| row.value.as_str()),
        Some("test store"),
        "the card names where a change survives"
    );
    let mod_row = |rows: &[crate::types::PropRow]| {
        rows.iter()
            .find(|row| row.command.as_deref() == Some("ferrocad-prefs:mod:GridOverlay"))
            .cloned()
            .expect("a mod row")
    };
    assert_eq!(mod_row(&rows).label, "disable", "it is on right now");

    // The click path: the id is intercepted before the registry, flips the refusal and
    // persists through the host's store.
    shell.update(&mut cx, |shell, cx| {
        shell.run_command("ferrocad-prefs:mod:GridOverlay", cx)
    });
    let persisted = saved.0.borrow().clone().expect("the click saved");
    assert!(!persisted.mod_enabled("GridOverlay"));
    assert!(persisted.mod_enabled("Fasteners"), "only the clicked one");
    assert_eq!(mod_row(&rows_of(&cx)).label, "enable");
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.snapshot().status)
            .contains("after a restart"),
        "a mod change names when it applies"
    );

    // Providers are live: hiding one is refused when it is the backend drawing, and a
    // hidden one cannot be activated again through the provider id.
    shell.update(&mut cx, |shell, _| {
        shell.provider_features = Some(vec![crate::types::ProviderFeature {
            feature: "grid".to_string(),
            providers: vec![
                crate::types::GridProvider {
                    backend: "ferrocad".to_string(),
                    active: true,
                    ..Default::default()
                },
                crate::types::GridProvider {
                    backend: "coin".to_string(),
                    active: false,
                    ..Default::default()
                },
            ],
        }])
    });
    shell.update(&mut cx, |shell, cx| {
        shell.run_command("ferrocad-prefs:provider:grid:ferrocad", cx)
    });
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.snapshot().status)
            .contains("switch away"),
        "the active backend is not hidden"
    );
    shell.update(&mut cx, |shell, cx| {
        shell.run_command("ferrocad-prefs:provider:grid:coin", cx)
    });
    assert!(
        !saved
            .0
            .borrow()
            .clone()
            .expect("saved")
            .provider_enabled("grid", "coin"),
        "the inactive backend is hidden"
    );
    shell.update(&mut cx, |shell, cx| {
        shell.run_command("ferrocad-provider:grid:coin", cx)
    });
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.snapshot().status)
            .contains("hidden in Settings/About"),
        "activating a hidden backend is refused"
    );
    shell.update(&mut cx, |shell, _| {
        shell.provider_features = None;
    });
}

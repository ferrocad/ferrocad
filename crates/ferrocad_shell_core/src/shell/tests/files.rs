//! Shell tests: the native file verbs (Open and Save).
//!
//! The dialog itself belongs to the platform; these tests drive the shell's half with
//! the GPUI test platform's prompt hooks and a backend that records what it was asked
//! to open or save. That keeps them independent of any file on disk and of the
//! interpreter's document registry.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gpui::App;

use crate::app::AppBackend;
use crate::files::{FilePicker, FileTask, OpenOutcome, SaveOutcome};
use crate::types::OpenedDocument;

use super::*;

/// A backend that records the paths the shell hands it.
///
/// Only the file verbs are implemented: everything else keeps the trait's
/// defaults, so the assertions below see exactly what the file verbs did.
#[derive(Default)]
struct RecordingBackend {
    opened: Rc<RefCell<Option<std::path::PathBuf>>>,
    saved: Rc<RefCell<Option<std::path::PathBuf>>>,
    open_bytes: Rc<RefCell<Option<(String, Vec<u8>)>>>,
}

impl AppBackend for RecordingBackend {
    fn open_path(&mut self, path: &Path) -> Result<OpenedDocument, String> {
        *self.opened.borrow_mut() = Some(path.to_path_buf());
        Ok(OpenedDocument {
            document: "Opened".to_string(),
            objects: 3,
        })
    }

    fn save_path(&self, path: &Path) -> Result<(), String> {
        *self.saved.borrow_mut() = Some(path.to_path_buf());
        Ok(())
    }

    fn open_document(&mut self, name: &str, bytes: &[u8]) -> Result<OpenedDocument, String> {
        *self.open_bytes.borrow_mut() = Some((name.to_string(), bytes.to_vec()));
        Ok(OpenedDocument {
            document: "Opened".to_string(),
            objects: 3,
        })
    }

    fn save_document(&self) -> Result<Vec<u8>, String> {
        Ok(b"a document".to_vec())
    }
}

/// A picker that answers with a fixed outcome: the browser's shape (delegating) and
/// the byte-shaped one, without a platform prompt.
#[derive(Default)]
struct FakePicker {
    open: Option<Result<OpenOutcome, String>>,
    save: Option<Result<SaveOutcome, String>>,
    delivered: Rc<RefCell<Vec<(String, Vec<u8>)>>>,
}

impl FilePicker for FakePicker {
    fn open(&mut self, _cx: &App) -> FileTask<Result<OpenOutcome, String>> {
        let outcome = self.open.take().unwrap_or(Ok(OpenOutcome::Cancelled));
        Box::pin(async move { outcome })
    }

    fn save(&mut self, _cx: &App, _suggested: &str) -> FileTask<Result<SaveOutcome, String>> {
        let outcome = self.save.take().unwrap_or(Ok(SaveOutcome::Cancelled));
        Box::pin(async move { outcome })
    }

    fn deliver(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.delivered
            .borrow_mut()
            .push((name.to_string(), bytes.to_vec()));
        Ok(())
    }
}

/// Install the recording backend on the test app, returning the two recorders.
fn install_recording(
    cx: &mut VisualTestContext,
) -> (
    Rc<RefCell<Option<std::path::PathBuf>>>,
    Rc<RefCell<Option<std::path::PathBuf>>>,
) {
    let backend = RecordingBackend::default();
    let opened = backend.opened.clone();
    let saved = backend.saved.clone();
    cx.update(|_, cx| crate::app::install_backend(cx, Box::new(backend)));
    (opened, saved)
}

/// `App_Open` asks for exactly one file, and the picked path is what the backend
/// opens — not a copy, not bytes with a made-up name.
#[gpui::test]
fn app_open_prompts_for_one_file_and_opens_the_picked_path(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    let (opened, _saved) = install_recording(&mut cx);

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Open", cx));
    assert!(
        cx.did_prompt_for_paths(),
        "App_Open shows the platform's open dialog"
    );

    let picked = std::env::temp_dir().join("ferrocad-open.FCStd");
    cx.simulate_path_prompt_response({
        let picked = picked.clone();
        move |options| {
            assert!(options.files, "the dialog accepts files");
            assert!(!options.directories, "…but not directories");
            assert!(!options.multiple, "one document at a time");
            Some(vec![picked.clone()])
        }
    });
    cx.run_until_parked();

    assert_eq!(
        *opened.borrow(),
        Some(picked),
        "the shell hands the picked path to the backend"
    );
    let model = model.lock().unwrap();
    assert_eq!(model.title, "FerroCAD — Opened");
    assert!(
        model.status.starts_with("opened Opened"),
        "status: {}",
        model.status
    );
}

/// `App_Save` asks where to write and hands that path to the backend.
#[gpui::test]
fn app_save_prompts_for_a_new_path_and_saves_it(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);
    let (_opened, saved) = install_recording(&mut cx);

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Save", cx));

    let picked = std::env::temp_dir().join("ferrocad-save.FCStd");
    cx.simulate_new_path_selection({
        let picked = picked.clone();
        move |_directory| Some(picked.clone())
    });
    cx.run_until_parked();

    assert_eq!(
        *saved.borrow(),
        Some(picked),
        "the shell asks the backend to save to the picked path"
    );
}

/// Cancelling the dialog changes nothing: no backend call, no status rewrite.
#[gpui::test]
fn a_cancelled_open_keeps_the_document(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    let (opened, _saved) = install_recording(&mut cx);
    let before = model.lock().unwrap().title.clone();

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Open", cx));
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();

    assert!(*opened.borrow() == None, "a cancelled prompt opens nothing");
    assert_eq!(model.lock().unwrap().title, before);
}

/// A browser-shaped picker **delegates**: the shell asks the host, opens no platform
/// prompt, and leaves the document alone until the page calls `open_document`.
#[gpui::test]
fn a_delegated_open_waits_for_the_page(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    install_recording(&mut cx);
    shell.update(&mut cx, |shell, _cx| {
        shell.set_file_picker(Box::new(FakePicker {
            open: Some(Ok(OpenOutcome::Delegated)),
            ..Default::default()
        }))
    });
    let before = model.lock().unwrap().title.clone();

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Open", cx));
    assert!(!cx.did_prompt_for_paths(), "the host owns the picker");
    cx.run_until_parked();
    assert_eq!(
        model.lock().unwrap().title,
        before,
        "the page opens when it has the file"
    );
}

/// A picker that read the bytes itself opens through `open_document`, so a host with
/// files but no paths (or a test double) still works.
#[gpui::test]
fn a_byte_picker_opens_through_the_backend(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    let backend = RecordingBackend::default();
    let open_bytes = backend.open_bytes.clone();
    cx.update(|_, cx| crate::app::install_backend(cx, Box::new(backend)));
    shell.update(&mut cx, |shell, _cx| {
        shell.set_file_picker(Box::new(FakePicker {
            open: Some(Ok(OpenOutcome::Bytes {
                name: "Dropped".to_string(),
                bytes: vec![7, 8],
            })),
            ..Default::default()
        }))
    });

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Open", cx));
    cx.run_until_parked();

    assert_eq!(
        *open_bytes.borrow(),
        Some(("Dropped".to_string(), vec![7, 8])),
        "the bytes and their name reach the backend"
    );
    assert_eq!(model.lock().unwrap().title, "FerroCAD — Opened");
}

/// A browser-shaped save takes the destination itself: the shell serializes the
/// document and hands the bytes to the picker.
#[gpui::test]
fn a_delegated_save_hands_the_bytes_to_the_picker(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    install_recording(&mut cx);
    let delivered = Rc::new(RefCell::new(Vec::new()));
    shell.update(&mut cx, |shell, _cx| {
        shell.set_file_picker(Box::new(FakePicker {
            save: Some(Ok(SaveOutcome::Delegated)),
            delivered: delivered.clone(),
            ..Default::default()
        }))
    });

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Save", cx));
    cx.run_until_parked();

    let name = model.lock().unwrap().document.clone();
    assert!(!name.is_empty());
    assert_eq!(
        *delivered.borrow(),
        vec![(name.clone(), b"a document".to_vec())],
        "the picker receives the document's name and bytes"
    );
    let status = model.lock().unwrap().status.clone();
    assert!(
        status.starts_with(&format!("saved {name}")),
        "status: {status}"
    );
}

/// A picker that fails (no portal, a blocked page) reports that in the status line
/// instead of failing silently.
#[gpui::test]
fn a_picker_error_reaches_the_status_line(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    install_recording(&mut cx);
    shell.update(&mut cx, |shell, _cx| {
        shell.set_file_picker(Box::new(FakePicker {
            open: Some(Err("open: no portal".to_string())),
            ..Default::default()
        }))
    });

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Open", cx));
    cx.run_until_parked();

    let status = model.lock().unwrap().status.clone();
    assert_eq!(status, "open: no portal");
}

/// The app menu leads with the file verbs, then the diagnostics row. The verbs have
/// no viewport surface, so this menu is how they are found without the accelerator.
#[gpui::test]
fn the_app_menu_lists_the_file_verbs(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);

    // The app menu lives in the title bar; this harness renders one on its own so
    // the popover can be clicked without the viewport.
    let size = cx.update(|window, _| window.viewport_size());
    let bar = cx
        .cx
        .open_window(size, move |_window, _cx| TitleBarHarness {
            shell: shell.clone(),
        });
    let mut bar_cx = VisualTestContext::from_window(bar.into(), &cx);
    draw(&mut bar_cx);
    click_at(&mut bar_cx, "app-menu");
    draw(&mut bar_cx);

    assert!(
        bar_cx.debug_bounds("app-row-App_Open").is_some(),
        "Open is in the app menu"
    );
    assert!(
        bar_cx.debug_bounds("app-row-App_Save").is_some(),
        "Save is in the app menu"
    );
    assert!(
        bar_cx.debug_bounds("app-row-Dev_ToggleTools").is_some(),
        "Dev Tools stays in the menu"
    );
    // The hamburger and each curated row carry a glyph, not just a text label.
    assert!(
        bar_cx.debug_bounds("app-menu-icon").is_some(),
        "the hamburger is drawn as a glyph"
    );
    for row in ["app-row-App_Open-icon", "app-row-App_Save-icon"] {
        assert!(bar_cx.debug_bounds(row).is_some(), "{row} is painted");
    }
}

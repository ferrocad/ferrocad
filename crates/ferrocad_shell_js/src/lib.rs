//! The JS scripting surface: [`ScriptHost`] over a page's ES module (S7 **spike**).
//!
//! The JS backend is a *composition*, not a fork: the document half is
//! [`ferrocad_shell_rust`](https://crates.io/crates/ferrocad_shell_rust)'s
//! `RustAppBackend`, and this crate supplies the script half the shell asks for
//! through `AppBackend::script_host()`:
//!
//! ```text
//! RustAppBackend (document)  +  JsScriptHost (this crate)  =  the JS backend
//! ```
//!
//! The page owns the module. `www/js-demo.js` installs `window.__ferrocadJs` with
//! five functions — `active`, `commands`, `evaluate`, `runCommand`, `stageMeshes` —
//! and the `onEvent` / `onDocumentEvent` callbacks; this crate calls them and maps the
//! answers onto the seam's types. Nothing here knows GPUI or the document: a command
//! reaches the palette because [`ScriptHost::commands`] is merged into the backend's
//! list, and it runs because [`ScriptHost::run_command`] reaches the page.
//!
//! **The document doorway.** A JS workbench that edits the document cannot link
//! `ferrocad_core` — a second registry in the same page is a second document. Instead
//! the host hands this half a [`DocumentApi`], which becomes a JS function
//! (`window.ferrocad.call(request)`) before the first command runs. The page never
//! sees a `Document`; it sees JSON, and the host's one document answers.
//!
//! **Spike scope.** The command list is read once, at boot, from the page; dynamic
//! registration and a JS tool session are the next step. `evaluate`, the event
//! listener, the document doorway, the display hook and the two document events
//! (`document.edited`, `document.recomputed`) are real.

#![cfg(target_family = "wasm")]

use std::sync::Arc;
use std::sync::OnceLock;

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::events::{DocumentEvent, DocumentEventSink};
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;

/// The page's contract: `window.__ferrocadJs`, defined by the ES module the page
/// loads. Every call is `catch`-guarded, so a missing module is a value, not a
/// panic.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = active, catch)]
    fn js_active() -> Result<bool, JsValue>;

    /// A JSON array of `{ id, label, tooltip }`.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = commands, catch)]
    fn js_commands() -> Result<String, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = evaluate, catch)]
    fn js_evaluate(code: &str) -> Result<String, JsValue>;

    /// `true` when the command ran; `false` when it refused.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = runCommand, catch)]
    fn js_run_command(id: &str) -> Result<bool, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = onEvent, catch)]
    fn js_on_event(kind: &str, x: i32, y: i32, entity: u32) -> Result<(), JsValue>;

    /// One document event: its id (`document.edited`, `document.recomputed`) and
    /// its payload as JSON. `catch`-guarded like every page import, so a page with
    /// no hook is silent rather than an error.
    #[wasm_bindgen(
        js_namespace = ["window", "__ferrocadJs"],
        js_name = onDocumentEvent,
        catch
    )]
    fn js_on_document_event(id: &str, payload: &str) -> Result<(), JsValue>;

    /// The page's display hook: a JSON array of stage meshes for the objects the
    /// document cannot mesh, or `[]` when it adds nothing.
    ///
    /// `have` is a JSON array of the object names the document already meshed, so a
    /// hook does not re-tessellate a shape the backend has.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = stageMeshes, catch)]
    fn js_stage_meshes(have: &str) -> Result<String, JsValue>;

    /// The loader's hook: hand the page a callable that reaches the document.
    ///
    /// Defined by the page before the module boots (see `www/index.html`). The call is
    /// `catch`-guarded because a page with no loader is still a valid host: the shell
    /// works, and only the addon path is missing.
    #[wasm_bindgen(js_namespace = "window", js_name = __ferrocadInstallKernel, catch)]
    fn js_install_kernel(callback: &JsValue) -> Result<(), JsValue>;
}

/// Whether the page installed its JS surface. The host asks this before it adds a
/// [`JsScriptHost`], so a page without the module gets the plain Rust backend.
pub fn available() -> bool {
    js_active().unwrap_or(false)
}

/// The script half of the JS backend.
///
/// It owns the document doorway when the host installed one: [`with_kernel`] turns
/// the [`DocumentApi`] into a JS function and hands it to the page, so an addon
/// reaches the host's one document without linking `ferrocad_core` (a second registry
/// would be a second document).
///
/// [`with_kernel`]: Self::with_kernel
pub struct JsScriptHost {
    /// The JS face of the doorway. Kept alive for as long as the host lives: the page
    /// holds a reference to it, so dropping it would leave a dangling function.
    #[allow(dead_code, reason = "alive-for-the-page")]
    kernel: Option<Closure<dyn Fn(String) -> String>>,
}

impl JsScriptHost {
    /// A JS half with no document access: `evaluate` still runs, addons cannot edit.
    pub fn new() -> Self {
        Self { kernel: None }
    }

    /// A JS half that hands the page the document.
    ///
    /// Runs once, when the backend is composed, so `window.ferrocad.call()` is ready
    /// before the first command. If the page never defined the loader hook the half is
    /// logged and left without a document, rather than failing the whole boot: the
    /// shell, the viewport and the console keep working.
    pub fn with_kernel(api: Arc<dyn DocumentApi>) -> Self {
        let closure =
            Closure::<dyn Fn(String) -> String>::new(move |request: String| api.call(&request));
        match js_install_kernel(closure.as_ref()) {
            Ok(()) => Self {
                kernel: Some(closure),
            },
            Err(error) => {
                log::error!("ferrocad js: the page did not take the document: {error:?}");
                Self { kernel: None }
            }
        }
    }
}

impl Default for JsScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

/// The page's event target: `window.__ferrocadJs.onDocumentEvent(id, json)`.
///
/// The document doorway holds one too (`DocApi::with_events`), so an addon's own
/// `create`/`set` is announced exactly like an edit the shell made.
pub struct PageEvents;

impl DocumentEventSink for PageEvents {
    fn document_event(&self, event: &DocumentEvent) {
        forward(event);
    }
}

/// Send one event to the page. A page without the hook is silent, not an error:
/// the import is `catch`-guarded, and this is a debug note at most.
fn forward(event: &DocumentEvent) {
    let Ok(payload) = serde_json::to_string(&event.payload) else {
        return;
    };
    if let Err(error) = js_on_document_event(event.id, &payload) {
        log::debug!("ferrocad js: no onDocumentEvent hook: {error:?}");
    }
}

impl ScriptHost for JsScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        match js_evaluate(code) {
            Ok(output) => Ok(EvalResult { ok: true, output }),
            Err(error) => Ok(EvalResult {
                ok: false,
                output: format!("{error:?}"),
            }),
        }
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        // ES modules are loaded by the page before the shell boots; there is no
        // workbench script to execute here.
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("js: no tool {name} in the spike"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(JsEventListener)
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        forward(event);
    }

    /// The page's display hook, when it installed one.
    ///
    /// A page that only registers commands has no `stageMeshes`, and a failed call is
    /// then not an error — it means "the document's own rows stand", which is what
    /// `None` says. The distinction is cached on the first answer, so a broken hook
    /// reports and a missing one stays quiet.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        static INSTALLED: OnceLock<bool> = OnceLock::new();

        let have = serde_json::to_string(have).ok()?;
        match js_stage_meshes(&have) {
            Ok(json) => {
                let _ = INSTALLED.set(true);
                Some(StageMesh::from_json_array(&json))
            }
            Err(error) => {
                if !*INSTALLED.get_or_init(|| false) {
                    log::debug!("ferrocad js: no stageMeshes hook: {error:?}");
                    return None;
                }
                log::error!("ferrocad js: stageMeshes failed: {error:?}");
                Some(Err(format!("stageMeshes: {error:?}")))
            }
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        page_commands().clone()
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !page_commands().iter().any(|command| command.id == id) {
            return None;
        }
        Some(match js_run_command(id) {
            Ok(true) => CommandOutcome {
                ok: true,
                status: format!("js: ran {id}"),
                ..Default::default()
            },
            Ok(false) => CommandOutcome {
                ok: false,
                error: format!("js: {id} refused"),
                ..Default::default()
            },
            Err(error) => CommandOutcome {
                ok: false,
                error: format!("js: {error:?}"),
                ..Default::default()
            },
        })
    }
}

/// One entry of the page's command list.
#[derive(Deserialize)]
struct PageCommand {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    tooltip: String,
}

impl PageCommand {
    fn into_command(self) -> CommandInfo {
        CommandInfo {
            menu: if self.label.is_empty() {
                self.id.clone()
            } else {
                self.label
            },
            tooltip: self.tooltip,
            cmd_type: "ForEdit".to_string(),
            group: "JS".to_string(),
            surface: "hidden".to_string(),
            enabled: true,
            id: self.id,
            ..Default::default()
        }
    }
}

/// The page's commands, read once. They are static for the session: the page
/// declares them before the module boots.
fn page_commands() -> &'static Vec<CommandInfo> {
    static COMMANDS: OnceLock<Vec<CommandInfo>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let json = match js_commands() {
            Ok(json) => json,
            Err(error) => {
                log::error!("ferrocad js: commands failed: {error:?}");
                return Vec::new();
            }
        };
        match serde_json::from_str::<Vec<PageCommand>>(&json) {
            Ok(entries) => entries.into_iter().map(PageCommand::into_command).collect(),
            Err(error) => {
                log::error!("ferrocad js: bad command list: {error}");
                Vec::new()
            }
        }
    })
}

/// Forwards viewport events to the page.
///
/// `entity` is the picked stage entity (`0` for a miss); the page maps it to an
/// object name if it wants one. Returning `false` means the scene is unchanged.
struct JsEventListener;

impl EventListener for JsEventListener {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        _scene: &mut Scene,
        _directives: &Directives,
    ) -> bool {
        let kind = match event {
            Event::MouseMove { .. } => "move",
            Event::MouseButton { down: true, .. } => "press",
            Event::MouseButton { down: false, .. } => "release",
            Event::MouseLeave => "leave",
        };
        let [x, y] = event.position().unwrap_or([0, 0]);
        let entity = hit.map(|hit| hit.id.0).unwrap_or(0);
        if let Err(error) = js_on_event(kind, x, y, entity) {
            log::error!("ferrocad js: onEvent failed: {error:?}");
            return false;
        }
        false
    }
}

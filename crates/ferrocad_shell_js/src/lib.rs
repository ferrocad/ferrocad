//! The JS scripting surface: [`ScriptHost`] over a page (S7 **spike**).
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
//! and the `onEvent` / `onDocumentEvent` callbacks; the [`Page`] trait is that
//! contract, and this crate maps its answers onto the seam's types. Nothing here
//! knows GPUI or the document: a command reaches the palette because
//! [`ScriptHost::commands`] is merged into the backend's list, and it runs because
//! [`ScriptHost::run_command`] reaches the page.
//!
//! **The page is a trait, not a browser.** [`WebPage`] is the page a browser host
//! has (`cfg(target_family = "wasm")`, `window.__ferrocadJs`), so this crate builds
//! natively too and its tests drive a fake page — the command merge, the display
//! hook, the document API and the event mapping are all testable without a browser. A
//! desktop host supplies a page over an embedded engine (`rquickjs`, decided in
//! `docs/project/tasks/backend-parity.md` B7) and gets the same behaviour.
//!
//! **The document API.** A JS workbench that edits the document cannot link
//! `ferrocad_core` — a second registry in the same page is a second document. Instead
//! the host hands this half a [`DocumentApi`], which
//! [`Page::install_kernel`] turns into a page function
//! (`window.ferrocad.call(request)`) before the first command runs. The page never
//! sees a `Document`; it sees JSON, and the host's one document answers.
//!
//! **Spike scope.** The command list is read once, at boot, from the page; dynamic
//! registration and a JS tool session are the next step. `evaluate`, the event
//! listener, the document API, the display hook and the two document events
//! (`document.edited`, `document.recomputed`) are real.

use std::sync::{Arc, OnceLock};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::events::{DocumentEvent, DocumentEventSink};
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};
use serde::Deserialize;

/// Everything this half asks of a page.
///
/// One trait, three implementations: the browser's `window.__ferrocadJs`
/// ([`WebPage`]), an embedded engine on the desktop, and a test's fake. Every method
/// answers `Result<_, String>` because a page is foreign code — a missing hook is a
/// value, never a panic.
pub trait Page: Send + Sync {
    /// Whether the page installed its JS surface at all.
    fn active(&self) -> Result<bool, String>;

    /// The page's command list: a JSON array of `{ id, label, tooltip }`.
    fn commands(&self) -> Result<String, String>;

    /// Evaluate one console line; the answer is what it printed or returned.
    fn evaluate(&self, code: &str) -> Result<String, String>;

    /// Run one of the page's commands: `true` when it ran, `false` when it refused.
    fn run_command(&self, id: &str) -> Result<bool, String>;

    /// A viewport event, for a page that draws its own scene.
    fn on_event(&self, _kind: &str, _x: i32, _y: i32, _entity: u32) -> Result<(), String> {
        Ok(())
    }

    /// One document event: its id and its payload as JSON. A page with no hook is
    /// silent, not an error.
    fn on_document_event(&self, _id: &str, _payload: &str) -> Result<(), String> {
        Ok(())
    }

    /// The display hook: a JSON array of stage meshes for the objects the document
    /// cannot mesh, or `[]` when it adds nothing.
    ///
    /// `have` is a JSON array of the object names the document already meshed, so a
    /// hook does not re-tessellate a shape the backend has. `Err` means "the page has
    /// no hook"; [`JsScriptHost::stage_meshes`] turns the first answer into the
    /// distinction between a quiet page and a broken hook.
    fn stage_meshes(&self, _have: &str) -> Result<String, String> {
        Err("the page has no stageMeshes hook".to_string())
    }

    /// Hand the page a callable that reaches the host's one document.
    ///
    /// The page keeps it for the session as `window.ferrocad.call` (or its engine's
    /// equivalent). `Err` means the page takes no document — it registers commands
    /// only, which is a valid page.
    fn install_kernel(
        &self,
        _call: Arc<dyn Fn(String) -> String + Send + Sync>,
    ) -> Result<(), String> {
        Err("the page takes no document".to_string())
    }
}

/// The script half of the JS backend.
///
/// It owns the page and the two things read once per session: the page's command
/// list and whether its display hook ever answered.
pub struct JsScriptHost {
    page: Arc<dyn Page>,
    /// The page's commands. They are static for the session: the page declares them
    /// before the module boots.
    commands: OnceLock<Vec<CommandInfo>>,
    /// Whether `stageMeshes` has answered successfully (see [`Self::stage_meshes`]).
    ///
    /// Presence of the value *is* the answer: a `OnceLock<bool>` would latch the first
    /// miss and then report every later failure as "no hook".
    installed: OnceLock<()>,
}

impl JsScriptHost {
    /// A JS half over `page`, with no document access: `evaluate` still runs, a
    /// document API command fails at the page.
    pub fn new(page: Arc<dyn Page>) -> Self {
        Self {
            page,
            commands: OnceLock::new(),
            installed: OnceLock::new(),
        }
    }

    /// A JS half that hands the page the document.
    ///
    /// Runs once, when the backend is composed, so `window.ferrocad.call()` is ready
    /// before the first command. A page that refuses the door (no loader hook) is
    /// logged and left without a document rather than failing the whole boot: the
    /// shell, the viewport and the console keep working.
    pub fn with_kernel(page: Arc<dyn Page>, api: Arc<dyn DocumentApi>) -> Self {
        let call: Arc<dyn Fn(String) -> String + Send + Sync> =
            Arc::new(move |request: String| api.call(&request));
        if let Err(error) = page.install_kernel(call) {
            log::error!("ferrocad js: the page did not take the document: {error}");
        }
        Self::new(page)
    }

    /// The page, for a host that needs to ask it something directly.
    pub fn page(&self) -> &Arc<dyn Page> {
        &self.page
    }

    /// Send one event to the page.
    fn forward(&self, event: &DocumentEvent) {
        let Ok(payload) = serde_json::to_string(&event.payload) else {
            return;
        };
        if let Err(error) = self.page.on_document_event(event.id, &payload) {
            log::debug!("ferrocad js: no onDocumentEvent hook: {error}");
        }
    }
}

/// Whether `page` installed its JS surface. The host asks this before it adds a
/// [`JsScriptHost`], so a page without the module gets the plain Rust backend.
pub fn available(page: &dyn Page) -> bool {
    page.active().unwrap_or(false)
}

/// The page's event target.
///
/// The document API holds one too (`DocApi::with_events`), so an addon's own
/// `create`/`set` is announced exactly like an edit the shell made.
pub struct PageEvents {
    page: Arc<dyn Page>,
}

impl PageEvents {
    /// Send this document's events to `page`.
    pub fn new(page: Arc<dyn Page>) -> Self {
        Self { page }
    }
}

impl DocumentEventSink for PageEvents {
    fn document_event(&self, event: &DocumentEvent) {
        let Ok(payload) = serde_json::to_string(&event.payload) else {
            return;
        };
        if let Err(error) = self.page.on_document_event(event.id, &payload) {
            log::debug!("ferrocad js: no onDocumentEvent hook: {error}");
        }
    }
}

impl ScriptHost for JsScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        match self.page.evaluate(code) {
            Ok(output) => Ok(EvalResult { ok: true, output }),
            Err(error) => Ok(EvalResult {
                ok: false,
                output: error,
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
        Box::new(JsEventListener {
            page: Arc::clone(&self.page),
        })
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        self.forward(event);
    }

    /// The page's display hook, when it installed one.
    ///
    /// A page that only registers commands has no `stageMeshes`, and a failed call is
    /// then not an error — it means "the document's own rows stand", which is what
    /// `None` says. The distinction is cached on the first answer, so a broken hook
    /// reports and a missing one stays quiet.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        let have = serde_json::to_string(have).ok()?;
        match self.page.stage_meshes(&have) {
            Ok(json) => {
                let _ = self.installed.set(());
                Some(StageMesh::from_json_array(&json))
            }
            Err(error) => {
                if self.installed.get().is_none() {
                    log::debug!("ferrocad js: no stageMeshes hook: {error}");
                    return None;
                }
                log::error!("ferrocad js: stageMeshes failed: {error}");
                Some(Err(format!("stageMeshes: {error}")))
            }
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        self.commands.get_or_init(|| read_commands(&*self.page)).clone()
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !self.commands().iter().any(|command| command.id == id) {
            return None;
        }
        Some(match self.page.run_command(id) {
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
                error: format!("js: {error}"),
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

/// Read the page's command list once. A page that cannot answer is a page with no
/// commands, not a boot failure.
fn read_commands(page: &dyn Page) -> Vec<CommandInfo> {
    let json = match page.commands() {
        Ok(json) => json,
        Err(error) => {
            log::error!("ferrocad js: commands failed: {error}");
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
}

/// Forwards viewport events to the page.
///
/// `entity` is the picked stage entity (`0` for a miss); the page maps it to an
/// object name if it wants one. Returning `false` means the scene is unchanged.
struct JsEventListener {
    page: Arc<dyn Page>,
}

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
        if let Err(error) = self.page.on_event(kind, x, y, entity) {
            log::error!("ferrocad js: onEvent failed: {error}");
            return false;
        }
        false
    }
}

/// The page a browser host has: `window.__ferrocadJs`, defined by the ES module the
/// page loads.
#[cfg(target_family = "wasm")]
mod web {
    use std::sync::Arc;

    use wasm_bindgen::JsValue;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::prelude::wasm_bindgen;

    use super::Page;

    /// Every call is `catch`-guarded, so a missing module is a value, not a panic.
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = active, catch)]
        fn js_active() -> Result<bool, JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = commands, catch)]
        fn js_commands() -> Result<String, JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = evaluate, catch)]
        fn js_evaluate(code: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = runCommand, catch)]
        fn js_run_command(id: &str) -> Result<bool, JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = onEvent, catch)]
        fn js_on_event(kind: &str, x: i32, y: i32, entity: u32) -> Result<(), JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = onDocumentEvent,
            catch
        )]
        fn js_on_document_event(id: &str, payload: &str) -> Result<(), JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = stageMeshes, catch)]
        fn js_stage_meshes(have: &str) -> Result<String, JsValue>;

        /// The loader's hook: hand the page a callable that reaches the document.
        ///
        /// Defined by the page before the module boots (see `www/index.html`). The
        /// call is `catch`-guarded because a page with no loader is still a valid
        /// host: the shell works, and only the addon path is missing.
        #[wasm_bindgen(js_namespace = "window", js_name = __ferrocadInstallKernel, catch)]
        fn js_install_kernel(callback: &JsValue) -> Result<(), JsValue>;
    }

    /// A page import's `JsValue` as the message this crate reports.
    fn js_error(error: JsValue) -> String {
        format!("{error:?}")
    }

    /// The browser's page.
    pub struct WebPage;

    impl Page for WebPage {
        fn active(&self) -> Result<bool, String> {
            js_active().map_err(js_error)
        }

        fn commands(&self) -> Result<String, String> {
            js_commands().map_err(js_error)
        }

        fn evaluate(&self, code: &str) -> Result<String, String> {
            js_evaluate(code).map_err(js_error)
        }

        fn run_command(&self, id: &str) -> Result<bool, String> {
            js_run_command(id).map_err(js_error)
        }

        fn on_event(&self, kind: &str, x: i32, y: i32, entity: u32) -> Result<(), String> {
            js_on_event(kind, x, y, entity).map_err(js_error)
        }

        fn on_document_event(&self, id: &str, payload: &str) -> Result<(), String> {
            js_on_document_event(id, payload).map_err(js_error)
        }

        fn stage_meshes(&self, have: &str) -> Result<String, String> {
            js_stage_meshes(have).map_err(js_error)
        }

        /// The page owns the function for the session, so the JS value is handed over
        /// rather than a Rust handle kept: dropping a `Closure` frees it, which would
        /// leave the page's `ferrocad.call` dangling.
        fn install_kernel(
            &self,
            call: Arc<dyn Fn(String) -> String + Send + Sync>,
        ) -> Result<(), String> {
            let function =
                Closure::<dyn Fn(String) -> String>::new(move |request: String| call(request));
            js_install_kernel(&function.into_js_value()).map_err(js_error)
        }
    }
}

#[cfg(target_family = "wasm")]
pub use web::WebPage;

/// The desktop page: the same contract, over an embedded QuickJS.
#[cfg(feature = "desktop")]
pub mod desktop;

#[cfg(feature = "desktop")]
pub use desktop::EnginePage;

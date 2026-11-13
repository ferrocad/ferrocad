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
//! `active`, `commands`, `evaluate`, `runCommand`, `stageMeshes`, the event callbacks
//! and the two dialog callbacks; the [`Page`] trait is that contract, and this crate
//! maps its answers onto the seam's types. Nothing here knows GPUI or the document: a
//! command reaches the palette because [`ScriptHost::commands`] is merged into the
//! backend's list, and it runs because [`ScriptHost::run_command`] reaches the page.
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
//! the host hands this half a [`DocumentApi`], which [`Page::install_kernel`] turns
//! into a page function (`window.ferrocad.call(request)`) before the first command
//! runs. The page never sees a `Document`; it sees JSON, and the host's one document
//! answers.
//!
//! **The presentation channels.** The same kernel also installs
//! `window.ferrocad.overlay(spec)` (a named program overlay described as JSON),
//! `.view(request)` (a presentation request) and `.dialog(spec)` (a declarative dialog).
//! Each is queued in `Session` and rides on the next [`CommandOutcome`], so the shell's
//! existing overlay pull, view apply and modal paths do the work. `mods/AxisOverlayJs`
//! is the shipped example.
//!
//! **Spike scope.** The command list is read once, at boot, from the page; dynamic
//! registration and a JS tool session are the next step. `evaluate`, the event
//! listener, the document API, the display hook, the two document events
//! (`document.edited`, `document.recomputed`), the overlay/view/dialog channels and the
//! dialog round trip are real.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::events::{DocumentEvent, DocumentEventSink};
use ferrocad_shell_api::types::{
    AxisSummary, EvalResult, GridSummary, ProviderFeature, ProviderSkin, WorkbenchReport,
};
use ferrocad_shell_api::view::ViewState;
use ferrocad_shell_api::DocumentApi;
use serde::Deserialize;

pub mod overlay;

pub use overlay::Spec;

/// The callables a page keeps for the session.
///
/// `call` is the document API. `overlay`, `view` and `dialog` are the three
/// presentation channels a page may use without editing the document: an authored
/// program overlay, a `{"view": …}` request, and a declarative dialog. Each answers
/// an empty string on success or the reason it refused, because a JS call cannot
/// return a Rust `Result` across QuickJS/wasm-bindgen without an exception type the
/// page would have to know.
pub struct Kernel {
    /// `window.ferrocad.call(request)`: JSON in, JSON out, the host's one document.
    pub call: Arc<dyn Fn(String) -> String + Send + Sync>,
    /// `window.ferrocad.overlay(spec)`: register, replace or remove a program overlay.
    pub overlay: Arc<dyn Fn(String) -> String + Send + Sync>,
    /// `window.ferrocad.view(request)`: queue a presentation request for the next
    /// command outcome.
    pub view: Arc<dyn Fn(String) -> String + Send + Sync>,
    /// `window.ferrocad.dialog(spec)`: queue a declarative dialog for the next command
    /// outcome.
    pub dialog: Arc<dyn Fn(String) -> String + Send + Sync>,
}

/// The prefix every dialog token this half mints carries (`Session::dialog`).
///
/// `owns_dialog` answers from it, so a chain can route an accept to the JS page even
/// when the Python half is its primary.
const DIALOG_PREFIX: &str = "js-";

/// The per-session state behind the three presentation channels.
///
/// A page has no document and no shell, so it hands the host a description and the
/// host does the work. The queues are drained into the next [`CommandOutcome`]: the
/// shell already knows how to apply a view, render a dialog and pull an overlay, so
/// the bridge adds no second dispatch path.
#[derive(Default)]
struct Session {
    pending_dialog: Mutex<Option<(String, ferrocad_shell_api::DialogSpec)>>,
    pending_view: Mutex<Option<ferrocad_shell_api::view::ViewRequest>>,
    overlays: Mutex<std::collections::HashSet<String>>,
    tokens: AtomicU64,
}

impl Session {
    /// Apply one overlay description, tracking the names this page owns so a page
    /// that goes away does not leave a scene behind.
    fn overlay(&self, text: &str) -> Result<(), String> {
        match overlay::parse(text)? {
            Spec::Put { name, scene } => {
                ferrocad_inventor::overlays::put_named(name.clone(), scene);
                self.overlays.lock().unwrap().insert(name);
            }
            Spec::Remove(name) => {
                ferrocad_inventor::overlays::remove_named(&name);
                self.overlays.lock().unwrap().remove(&name);
            }
            Spec::RemoveAll => {
                let names: Vec<String> = self.overlays.lock().unwrap().drain().collect();
                for name in names {
                    ferrocad_inventor::overlays::remove_named(&name);
                }
            }
        }
        Ok(())
    }

    /// Queue one presentation request. The next command outcome carries it.
    fn view(&self, text: &str) -> Result<(), String> {
        let request = serde_json::from_str(text)
            .map_err(|error| format!("view request is not JSON: {error}"))?;
        *self.pending_view.lock().unwrap() = Some(request);
        Ok(())
    }

    /// Queue one dialog spec and mint its token.
    fn dialog(&self, text: &str) -> Result<(), String> {
        let spec: ferrocad_shell_api::DialogSpec = serde_json::from_str(text)
            .map_err(|error| format!("dialog spec is not JSON: {error}"))?;
        if spec.fields.is_empty() {
            return Err("a dialog needs at least one field".to_string());
        }
        let token = format!(
            "{DIALOG_PREFIX}{}",
            self.tokens.fetch_add(1, Ordering::Relaxed) + 1
        );
        *self.pending_dialog.lock().unwrap() = Some((token, spec));
        Ok(())
    }

    /// Drop anything the page queued before a command runs, so a stale request
    /// cannot attach itself to the wrong outcome.
    fn clear_pending(&self) {
        self.pending_dialog.lock().unwrap().take();
        self.pending_view.lock().unwrap().take();
    }

    /// Move the queued dialog/view into `outcome`.
    fn take_outcome(&self, outcome: &mut CommandOutcome) {
        if let Some((token, spec)) = self.pending_dialog.lock().unwrap().take() {
            outcome.dialog = Some(spec);
            outcome.token = Some(token);
        }
        let view = self.pending_view.lock().unwrap().take();
        if outcome.view.is_none() {
            outcome.view = view;
        }
    }

    /// Drop every overlay this page registered.
    fn clear_overlays(&self) {
        let names: Vec<String> = self.overlays.lock().unwrap().drain().collect();
        for name in names {
            ferrocad_inventor::overlays::remove_named(&name);
        }
    }
}

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

    /// The shell's presentation state, as JSON (`{ "axis": …, "grid": … }`).
    ///
    /// A page that draws a feature (the axis/grid overlay) mirrors the shared policy
    /// from here, so the feature's one switch and style control it too.
    fn on_view_state(&self, _state: &str) -> Result<(), String> {
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

    /// Hand the page the host's channels: the document API and the three presentation
    /// sinks (overlay, view, dialog).
    ///
    /// The page keeps them for the session as `window.ferrocad.call`, `.overlay`,
    /// `.view` and `.dialog` (or the engine's equivalent). `Err` means the page takes
    /// none of them — it registers commands only, which is a valid page.
    fn install_kernel(&self, _kernel: Kernel) -> Result<(), String> {
        Err("the page takes no kernel".to_string())
    }

    /// A dialog the page opened on `token` was accepted with `values` as JSON.
    ///
    /// The answer is a JSON [`CommandOutcome`]; an empty string is `{ok: true}`. A page
    /// with no hook reports it.
    fn dialog_accept(&self, _token: &str, _values: &str) -> Result<String, String> {
        Err("the page has no dialog hook".to_string())
    }

    /// A dialog the page opened on `token` was cancelled.
    fn dialog_cancel(&self, _token: &str) -> Result<String, String> {
        Err("the page has no dialog hook".to_string())
    }

    /// The provider features the page registered, as JSON `ProviderFeature[]`.
    ///
    /// A page with no provider registry answers `Err`, which the host reads as "this
    /// half owns no providers" and leaves the document half's native entries alone.
    fn providers(&self) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }

    /// Switch `feature` to `backend` and return a JSON `CommandOutcome`.
    ///
    /// The page runs its own `deactivate`/`activate` callbacks and returns the merged
    /// `{"view": …}` claim, exactly as the Python registry does.
    fn set_provider(&self, _feature: &str, _backend: &str) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }

    /// Deactivate `feature`'s applied backend without selecting another.
    ///
    /// The chain calls this when a feature moves to a backend this page does not
    /// list: the page runs its active backend's `deactivate` (removing the overlay)
    /// and returns the released `{"view": …}` claim.
    fn release_provider(&self, _feature: &str) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }

    /// Ask the active backend of `feature` to re-read its state.
    fn refresh_provider(&self, _feature: &str) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }

    /// The page's summary for `feature` (`AxisSummary`/`GridSummary` JSON), or `null`.
    fn provider_summary(&self, _feature: &str) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }

    /// The active backend's WGSL skin for `feature`, as `{"source": …}` JSON.
    fn provider_skin(&self, _feature: &str) -> Result<String, String> {
        Err("the page has no provider registry".to_string())
    }
}

impl Drop for JsScriptHost {
    fn drop(&mut self) {
        self.session.clear_overlays();
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
    /// The presentation channels' queues, shared with the kernel's closures.
    session: Arc<Session>,
}

impl JsScriptHost {
    /// A JS half over `page`, with no document access: `evaluate` still runs, a
    /// document API command fails at the page.
    pub fn new(page: Arc<dyn Page>) -> Self {
        Self {
            page,
            commands: OnceLock::new(),
            installed: OnceLock::new(),
            session: Arc::new(Session::default()),
        }
    }

    /// A JS half that hands the page the document and the presentation channels.
    ///
    /// Runs once, when the backend is composed, so `window.ferrocad.call()` is ready
    /// before the first command. A page that refuses the door (no loader hook) is
    /// logged and left without a document rather than failing the whole boot: the
    /// shell, the viewport and the console keep working.
    pub fn with_kernel(page: Arc<dyn Page>, api: Arc<dyn DocumentApi>) -> Self {
        let session = Arc::new(Session::default());
        let call: Arc<dyn Fn(String) -> String + Send + Sync> =
            Arc::new(move |request: String| api.call(&request));

        let overlay = {
            let session = Arc::clone(&session);
            Arc::new(move |text: String| match session.overlay(&text) {
                Ok(()) => String::new(),
                Err(error) => error,
            }) as Arc<dyn Fn(String) -> String + Send + Sync>
        };
        let view = {
            let session = Arc::clone(&session);
            Arc::new(move |text: String| match session.view(&text) {
                Ok(()) => String::new(),
                Err(error) => error,
            }) as Arc<dyn Fn(String) -> String + Send + Sync>
        };
        let dialog = {
            let session = Arc::clone(&session);
            Arc::new(move |text: String| match session.dialog(&text) {
                Ok(()) => String::new(),
                Err(error) => error,
            }) as Arc<dyn Fn(String) -> String + Send + Sync>
        };

        let kernel = Kernel {
            call,
            overlay,
            view,
            dialog,
        };
        if let Err(error) = page.install_kernel(kernel) {
            log::error!("ferrocad js: the page did not take the document: {error}");
        }
        Self {
            page,
            commands: OnceLock::new(),
            installed: OnceLock::new(),
            session,
        }
    }

    /// The page, for a host that needs to ask it something directly.
    pub fn page(&self) -> &Arc<dyn Page> {
        &self.page
    }

    /// A page's JSON outcome as a [`CommandOutcome`], with any overlay/view it queued
    /// during the call folded in.
    fn outcome_from_page(&self, result: Result<String, String>) -> CommandOutcome {
        match result {
            Ok(json) => {
                let mut outcome = parse_outcome(&json);
                self.session.take_outcome(&mut outcome);
                outcome
            }
            Err(error) => CommandOutcome {
                ok: false,
                error: format!("js: {error}"),
                ..Default::default()
            },
        }
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

    /// Mirror the shell's presentation state into the page, so an overlay renders the
    /// feature's one shared policy (and follows its one switch).
    fn on_view_state_changed(&self, state: &ViewState) {
        let payload = view_state_json(state);
        if let Err(error) = self.page.on_view_state(&payload) {
            log::debug!("ferrocad js: no onViewState hook: {error}");
        }
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
        self.commands
            .get_or_init(|| read_commands(&*self.page))
            .clone()
    }

    /// The provider registry the page exposed, or an empty list.
    ///
    /// A JS workbench registers its providers in the page (it cannot call the kernel at
    /// import time), so the host asks the page for the manifests. A page with no
    /// `providers` hook answers `Err` and the list is empty — the document half then
    /// keeps its native entries, which is the "no JS provider" case.
    fn provider_features(&self) -> Vec<ProviderFeature> {
        self.page
            .providers()
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// Switch a feature's active backend through the page's registry.
    ///
    /// `None` when the page does not list `(feature, backend)`: the chain routes a
    /// switch to the one host that can run it, so a page that only implements one
    /// backend must not answer for another.
    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        let listed = self.provider_features().iter().any(|entry| {
            entry.feature == feature
                && entry
                    .providers
                    .iter()
                    .any(|provider| provider.backend == backend)
        });
        if !listed {
            return None;
        }
        self.session.clear_pending();
        Some(self.outcome_from_page(self.page.set_provider(feature, backend)))
    }

    /// Deactivate the page's active backend for `feature` (see `HostChain`).
    fn release_provider(&self, feature: &str) -> Option<CommandOutcome> {
        if !self
            .provider_features()
            .iter()
            .any(|entry| entry.feature == feature)
        {
            return None;
        }
        self.session.clear_pending();
        Some(self.outcome_from_page(self.page.release_provider(feature)))
    }

    /// Ask the page's active backend to re-read its state.
    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        if !self
            .provider_features()
            .iter()
            .any(|entry| entry.feature == feature)
        {
            return None;
        }
        self.session.clear_pending();
        Some(self.outcome_from_page(self.page.refresh_provider(feature)))
    }

    /// The active backend's skin, when the page supplies one.
    fn provider_skin(&self, feature: &str) -> Option<String> {
        self.page
            .provider_skin(feature)
            .ok()
            .and_then(|json| serde_json::from_str::<ProviderSkin>(&json).ok())
            .and_then(|skin| skin.source)
    }

    /// The JS provider's axis summary, or `None` when the native backend is active.
    fn axis_summary(&self) -> Option<AxisSummary> {
        self.page.provider_summary("axis").ok().and_then(|json| {
            serde_json::from_str::<Option<AxisSummary>>(&json)
                .ok()
                .flatten()
        })
    }

    /// The JS provider's grid summary, or `None` when the native backend is active.
    fn grid_summary(&self) -> Option<GridSummary> {
        self.page.provider_summary("grid").ok().and_then(|json| {
            serde_json::from_str::<Option<GridSummary>>(&json)
                .ok()
                .flatten()
        })
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !self.commands().iter().any(|command| command.id == id) {
            return None;
        }
        self.session.clear_pending();
        let mut outcome = match self.page.run_command(id) {
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
        };
        if outcome.ok {
            self.session.take_outcome(&mut outcome);
        }
        Some(outcome)
    }

    /// Accept a dialog the page opened. The page owns the values, so its answer is a
    /// JSON `CommandOutcome`; the queued view (a cross-claim, a repaint trigger) rides
    /// on the same outcome.
    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        let values = serde_json::to_string(values).ok()?;
        match self.page.dialog_accept(token, &values) {
            Ok(text) => {
                let mut outcome = parse_outcome(&text);
                self.session.take_outcome(&mut outcome);
                Some(outcome)
            }
            Err(error) => Some(CommandOutcome {
                ok: false,
                error: format!("js: {error}"),
                ..Default::default()
            }),
        }
    }

    /// Cancel a dialog the page opened. Its only work is to drop any values it held.
    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        self.session.clear_pending();
        match self.page.dialog_cancel(token) {
            Ok(text) => Some(parse_outcome(&text)),
            Err(error) => Some(CommandOutcome {
                ok: false,
                error: format!("js: {error}"),
                ..Default::default()
            }),
        }
    }

    /// This half mints every JS dialog token as `js-{serial}`.
    fn owns_dialog(&self, token: &str) -> bool {
        token.starts_with(DIALOG_PREFIX)
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
    /// The surfaces the shell groups commands by (`"axis"`, `"grid"`, …).
    #[serde(default)]
    group: String,
    /// `"ForEdit"`, `"ForView"`, … (metadata only).
    #[serde(default)]
    cmd_type: String,
    /// `GetResources()["FerroCAD_Surface"]`; absent keeps the command hidden.
    #[serde(default)]
    surface: String,
    /// `GetResources()["FerroCAD_Parent"]`: the popover's primary command.
    #[serde(default)]
    parent: String,
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
            cmd_type: if self.cmd_type.is_empty() {
                "ForEdit".to_string()
            } else {
                self.cmd_type
            },
            group: if self.group.is_empty() {
                "JS".to_string()
            } else {
                self.group
            },
            surface: if self.surface.is_empty() {
                "hidden".to_string()
            } else {
                self.surface
            },
            parent: self.parent,
            enabled: true,
            id: self.id,
            ..Default::default()
        }
    }
}

/// A page's answer to a dialog callback as a [`CommandOutcome`].
///
/// An empty answer is success. Malformed JSON is a refusal the shell can show, not a
/// panic: the page is foreign code.
fn parse_outcome(text: &str) -> CommandOutcome {
    if text.trim().is_empty() {
        return CommandOutcome {
            ok: true,
            ..Default::default()
        };
    }
    match serde_json::from_str::<CommandOutcome>(text) {
        Ok(outcome) => outcome,
        Err(error) => CommandOutcome {
            ok: false,
            error: format!("js: the dialog answer is not a command outcome: {error}"),
            ..Default::default()
        },
    }
}

/// The shell's presentation state as the page's `onViewState` sees it.
///
/// Only the fields an overlay renders are sent: the axis style (linear stem inks and
/// the shared switch) and the grid switch/claim. A page that draws a feature mirrors
/// these and publishes from them, so the feature's one settings form and one switch
/// control every backend.
fn view_state_json(state: &ViewState) -> String {
    let axis = &state.axis;
    serde_json::json!({
        "axis": {
            "enabled": axis.enabled,
            "colors": axis
                .colors
                .iter()
                .map(|color| [color[0], color[1], color[2]])
                .collect::<Vec<[f32; 3]>>(),
            "opacity": axis.opacity,
            "length": axis.length,
            "width": axis.width,
            "origin": axis.origin,
            "labels": axis.labels,
            "ghost_alpha": axis.ghost_alpha,
        },
        "grid": {
            "enabled": state.grid,
            "overlay": state.grid_overlay,
        },
    })
    .to_string()
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
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::prelude::wasm_bindgen;
    use wasm_bindgen::JsValue;

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

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = onViewState,
            catch
        )]
        fn js_on_view_state(state: &str) -> Result<(), JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = stageMeshes, catch)]
        fn js_stage_meshes(have: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = onDialogAccept,
            catch
        )]
        fn js_on_dialog_accept(token: &str, values: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = onDialogCancel,
            catch
        )]
        fn js_on_dialog_cancel(token: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = providers, catch)]
        fn js_providers() -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = setProvider,
            catch
        )]
        fn js_set_provider(feature: &str, backend: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = releaseProvider,
            catch
        )]
        fn js_release_provider(feature: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = refreshProvider,
            catch
        )]
        fn js_refresh_provider(feature: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = providerSummary,
            catch
        )]
        fn js_provider_summary(feature: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(
            js_namespace = ["window", "__ferrocadJs"],
            js_name = providerSkin,
            catch
        )]
        fn js_provider_skin(feature: &str) -> Result<String, JsValue>;

        /// The loader's hook: hand the page the host's channels.
        ///
        /// Defined by the page before the module boots (see `www/index.html`). The
        /// call is `catch`-guarded because a page with no loader is still a valid
        /// host: the shell works, and only the addon path is missing.
        #[wasm_bindgen(js_namespace = "window", js_name = __ferrocadInstallKernel, catch)]
        fn js_install_kernel(
            call: &JsValue,
            overlay: &JsValue,
            view: &JsValue,
            dialog: &JsValue,
        ) -> Result<(), JsValue>;
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

        fn on_view_state(&self, state: &str) -> Result<(), String> {
            js_on_view_state(state).map_err(js_error)
        }

        fn stage_meshes(&self, have: &str) -> Result<String, String> {
            js_stage_meshes(have).map_err(js_error)
        }

        fn dialog_accept(&self, token: &str, values: &str) -> Result<String, String> {
            js_on_dialog_accept(token, values).map_err(js_error)
        }

        fn dialog_cancel(&self, token: &str) -> Result<String, String> {
            js_on_dialog_cancel(token).map_err(js_error)
        }

        fn providers(&self) -> Result<String, String> {
            js_providers().map_err(js_error)
        }

        fn set_provider(&self, feature: &str, backend: &str) -> Result<String, String> {
            js_set_provider(feature, backend).map_err(js_error)
        }

        fn release_provider(&self, feature: &str) -> Result<String, String> {
            js_release_provider(feature).map_err(js_error)
        }

        fn refresh_provider(&self, feature: &str) -> Result<String, String> {
            js_refresh_provider(feature).map_err(js_error)
        }

        fn provider_summary(&self, feature: &str) -> Result<String, String> {
            js_provider_summary(feature).map_err(js_error)
        }

        fn provider_skin(&self, feature: &str) -> Result<String, String> {
            js_provider_skin(feature).map_err(js_error)
        }

        /// The page owns the functions for the session, so the JS values are handed
        /// over rather than Rust handles kept: dropping a `Closure` frees it, which
        /// would leave the page's `ferrocad.call` dangling.
        fn install_kernel(&self, kernel: super::Kernel) -> Result<(), String> {
            let call = Closure::<dyn Fn(String) -> String>::new(move |request: String| {
                (kernel.call)(request)
            });
            let overlay = Closure::<dyn Fn(String) -> String>::new(move |spec: String| {
                (kernel.overlay)(spec)
            });
            let view = Closure::<dyn Fn(String) -> String>::new(move |request: String| {
                (kernel.view)(request)
            });
            let dialog =
                Closure::<dyn Fn(String) -> String>::new(move |spec: String| (kernel.dialog)(spec));
            js_install_kernel(
                &call.into_js_value(),
                &overlay.into_js_value(),
                &view.into_js_value(),
                &dialog.into_js_value(),
            )
            .map_err(js_error)
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

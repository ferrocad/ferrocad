//! Several script hosts behind one primary.
//!
//! A host names one language (B8), but a target may install more than one: the browser
//! puts the page beside RustPython, and the desktop puts its halves beside each other and
//! beside sandboxed wasm bundles. This is the one place that composition rule lives.
//!
//! It lives in the seam rather than in a host crate because **both** hosts need it, and
//! because a host crate that owned it would make the other one depend on an engine it
//! does not link.
//!
//! The first host is the **primary**: the console, the workbenches, the tools, the
//! document lifecycle and the render pipeline are its answers, and its display hook is
//! what decides whether the composition has one at all. The rest are subordinates: they
//! answer commands, contribute display rows, and hear the document's events.

use std::sync::Arc;

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;

use crate::app::{NoopListener, ScriptHost};
use crate::commands::{CommandInfo, CommandOutcome};
use crate::display::StageMesh;
use crate::events::DocumentEvent;
use crate::selection::SelectionItem;
use crate::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    WorkbenchReport,
};
use crate::view::ViewState;

/// One or more script hosts, as one [`ScriptHost`].
///
/// Hold the members as `Arc<dyn ScriptHost>` rather than a boxed type: a host that calls
/// an addon's edit back into the script half (`ferrocad_shell_wasm::announce_to`) needs to
/// share the primary rather than give it up, and the browser's RustPython host is not
/// `Send` — it holds a document payload in an `Rc`.
pub struct HostChain {
    hosts: Vec<Arc<dyn ScriptHost>>,
}

impl HostChain {
    /// Compose `hosts`; the first is the primary.
    pub fn new(hosts: Vec<Arc<dyn ScriptHost>>) -> Self {
        Self { hosts }
    }

    /// Whether there is no host at all — the document half alone.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// How many hosts compose this chain.
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// The primary: the first host, if any.
    fn primary(&self) -> Option<&Arc<dyn ScriptHost>> {
        self.hosts.first()
    }

    /// Every host, in load order: the order a merged answer or a fan-out uses.
    fn every(&self) -> impl Iterator<Item = &dyn ScriptHost> {
        self.hosts
            .iter()
            .map(|host| host.as_ref() as &dyn ScriptHost)
    }
}

impl ScriptHost for HostChain {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        match self.primary() {
            Some(primary) => primary.evaluate(code),
            None => Ok(EvalResult {
                ok: false,
                output: "this build has no console: no script host is installed".to_string(),
            }),
        }
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        match self.primary() {
            Some(primary) => primary.load_workbenches(),
            None => Ok(WorkbenchReport::default()),
        }
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        match self.primary() {
            Some(primary) => primary.tool(name),
            None => Err(format!("no tool {name}: no script host is installed")),
        }
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        self.primary()
            .is_some_and(|primary| primary.tool_command(name, verb))
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        match self.primary() {
            Some(primary) => primary.inventor_scene(),
            None => Ok(Scene::default()),
        }
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        // Every host hears the event; each one asked for its own listener.
        let listeners: Vec<Box<dyn EventListener>> =
            self.every().map(|host| host.event_listener()).collect();
        match listeners.len() {
            0 => Box::new(NoopListener),
            1 => listeners.into_iter().next().expect("one listener"),
            _ => Box::new(FanOut(listeners)),
        }
    }

    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        match self.primary() {
            Some(primary) => primary.bootstrap(),
            None => Ok(None),
        }
    }

    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        match self.primary() {
            Some(primary) => primary.open_startup_document(boot),
            None => Ok(None),
        }
    }

    fn open_path(&self, path: &std::path::Path) -> Option<Result<OpenedDocument, String>> {
        // `None` means "not mine", so the chain asks in order — the same contract a single
        // host already has, applied across hosts.
        self.every().find_map(|host| host.open_path(path))
    }

    fn save_path(&self, path: &std::path::Path) -> Option<Result<(), String>> {
        self.every().find_map(|host| host.save_path(path))
    }

    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        self.primary()
            .and_then(|primary| primary.activate_document(name))
    }

    fn hello(&self) -> Result<Option<String>, String> {
        match self.primary() {
            Some(primary) => primary.hello(),
            None => Ok(Some(format!(
                "FerroCAD shell · {} script host(s)",
                self.hosts.len()
            ))),
        }
    }

    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        // Answers in order. The primary's answer is what decides whether the composition
        // has a display hook at all — a `None` there leaves the backend's own rows in place
        // until some host actually contributes one. One pass only: a display hook may be
        // expensive, and `every()` puts the primary first.
        let has_primary = self.primary().is_some();
        let mut rows = Vec::new();
        let mut failed = None;
        let mut primary_answered = false;
        for (index, host) in self.every().enumerate() {
            match host.stage_meshes(have) {
                Some(Ok(mut meshes)) => {
                    primary_answered |= has_primary && index == 0;
                    rows.append(&mut meshes);
                }
                Some(Err(error)) => {
                    failed.get_or_insert(error);
                }
                None => {}
            }
        }

        match failed {
            // One failing host speaks for the composition: no host can vouch for the row
            // set when a sibling could not produce its own.
            Some(error) => Some(Err(error)),
            None if primary_answered || !rows.is_empty() => Some(Ok(rows)),
            None => None,
        }
    }

    fn apply_interaction(&self, commit: &InteractionCommit, object: &str) -> Option<ApplyResult> {
        self.every()
            .find_map(|host| host.apply_interaction(commit, object))
    }

    fn add_datum_plane(
        &self,
        origin: Vector3,
        u: Vector3,
        v: Vector3,
    ) -> Option<Result<String, String>> {
        self.every()
            .find_map(|host| host.add_datum_plane(origin, u, v))
    }

    fn extra_tree_nodes(&self) -> Vec<DocumentNode> {
        match self.primary() {
            Some(primary) => primary.extra_tree_nodes(),
            None => Vec::new(),
        }
    }

    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        self.every()
            .find_map(|host| host.extra_properties(object))
    }

    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        self.every()
            .find_map(|host| host.set_extra_property(object, property, value))
    }

    fn on_selection_changed(&self, items: &[SelectionItem]) {
        for host in self.every() {
            host.on_selection_changed(items);
        }
    }

    fn take_selection_requests(&self) -> Vec<SelectionRequest> {
        match self.primary() {
            Some(primary) => primary.take_selection_requests(),
            None => Vec::new(),
        }
    }

    fn on_view_state_changed(&self, state: &ViewState) {
        for host in self.every() {
            host.on_view_state_changed(state);
        }
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        for host in self.every() {
            host.on_document_event(event);
        }
    }

    fn appearance_view(&self) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.appearance_view())
    }

    fn grid_summary(&self) -> Option<GridSummary> {
        self.primary().and_then(|primary| primary.grid_summary())
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        self.primary().and_then(|primary| primary.axis_summary())
    }

    fn provider_features(&self) -> Vec<ProviderFeature> {
        match self.primary() {
            Some(primary) => primary.provider_features(),
            None => Vec::new(),
        }
    }

    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.set_provider(feature, backend))
    }

    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.refresh_providers(feature))
    }

    fn provider_skin(&self, feature: &str) -> Option<String> {
        self.primary()
            .and_then(|primary| primary.provider_skin(feature))
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Option<Vec<serde_json::Value>> {
        self.primary()
            .and_then(|primary| primary.manipulator_specs(object, element))
    }

    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.dialog_accept(token, values))
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        self.primary().and_then(|primary| primary.dialog_cancel(token))
    }

    fn gpu_adapter(&self) -> Option<String> {
        self.primary().and_then(|primary| primary.gpu_adapter())
    }

    fn set_gpu_adapter(&self, value: &str) -> Option<GpuPreferenceOutcome> {
        self.primary()
            .and_then(|primary| primary.set_gpu_adapter(value))
    }

    fn commands(&self) -> Vec<CommandInfo> {
        self.every().flat_map(|host| host.commands()).collect()
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        // First host to answer wins, in load order: the primary, then the subordinates.
        self.every().find_map(|host| host.run_command(id))
    }

    fn claims(&self, id: &str) -> Option<CommandOutcome> {
        self.every().find_map(|host| host.claims(id))
    }
}

/// Every host's listener, run in order.
struct FanOut(Vec<Box<dyn EventListener>>);

impl EventListener for FanOut {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        scene: &mut Scene,
        directives: &Directives,
    ) -> bool {
        // All of them, not until the first `true`: the flag says "the scene changed", and
        // one host changing it must not hide another host's edit from the same event.
        let mut changed = false;
        for listener in &mut self.0 {
            changed |= listener.on_event(event, hit, scene, directives);
        }
        changed
    }
}

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
//! answer commands, contribute display rows, and hear the document's events. A dialog is
//! routed by token ownership, so a subordinate can open one even though the console is
//! the primary's.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;

use crate::app::{NoopListener, ScriptHost};
use crate::capability::DocumentGrant;
use crate::commands::{CommandInfo, CommandOutcome};
use crate::display::StageMesh;
use crate::events::DocumentEvent;
use crate::selection::SelectionItem;
use crate::settings::SettingValue;
use crate::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    WorkbenchReport,
};
use crate::view::{ViewRequest, ViewState};

/// One or more script hosts, as one [`ScriptHost`].
///
/// Hold the members as `Arc<dyn ScriptHost>` rather than a boxed type: a host that calls
/// an addon's edit back into the script half (`ferrocad_shell_wasm::announce_to`) needs to
/// share the primary rather than give it up, and the browser's RustPython host is not
/// `Send` — it holds a document payload in an `Rc`.
pub struct HostChain {
    hosts: Vec<Arc<dyn ScriptHost>>,
    /// The backend the chain last switched each feature to.
    ///
    /// A member's registry records only *its* choice, so two hosts can each name an
    /// active backend for the same feature. The chain is the one place that sees both
    /// lists and every switch, so it remembers what it routed and reports that backend
    /// as the single active one. The map is empty at boot, where each host's own choice
    /// (its persisted default, else priority) stands.
    selected: Mutex<BTreeMap<String, String>>,
    /// The native-layer claim the loaded backend makes (`axis.overlay`, `grid_overlay`),
    /// captured from its own `activate`/`refresh`.
    ///
    /// Only the loaded backend can say whether the native cross or ground steps aside.
    /// A command that runs inside the primary (the axis toggle, a settings row) computes
    /// its claim from the primary's registry, which may not be the loaded backend, so a
    /// returned view is reconciled with this cache. That keeps the drawn claim following
    /// the slot instead of whichever host happened to answer.
    claims: Mutex<BTreeMap<String, SlotClaim>>,
    /// Each host's last-reported active backend, per feature.
    ///
    /// `None` means the host lists the feature with no active backend; a missing key
    /// means it did not list the feature. The snapshot is what makes a switch a mod
    /// performs inside its own callback visible: `adopt` compares the host's answer
    /// with this and records a *change*, so a command that leaves the registry alone
    /// cannot undo a switch the chain routed to another host.
    reported: Mutex<Vec<BTreeMap<String, Option<String>>>>,
}

/// The native-layer claim a loaded backend makes, in the shell's own vocabulary.
#[derive(Clone, Copy, Default)]
struct SlotClaim {
    /// `view.axis.overlay`.
    axis_overlay: Option<bool>,
    /// `view.grid_overlay`.
    grid_overlay: Option<bool>,
}

impl SlotClaim {
    /// The claim stated by a provider outcome, or `None` when it states neither.
    fn from_view(view: &Option<ViewRequest>) -> Option<Self> {
        let view = view.as_ref()?;
        let axis_overlay = view.axis.as_ref().and_then(|axis| axis.overlay);
        let grid_overlay = view.grid_overlay;
        (axis_overlay.is_some() || grid_overlay.is_some()).then_some(Self {
            axis_overlay,
            grid_overlay,
        })
    }
}

impl HostChain {
    /// Compose `hosts`; the first is the primary.
    pub fn new(hosts: Vec<Arc<dyn ScriptHost>>) -> Self {
        let reported = vec![BTreeMap::new(); hosts.len()];
        Self {
            hosts,
            selected: Mutex::new(BTreeMap::new()),
            claims: Mutex::new(BTreeMap::new()),
            reported: Mutex::new(reported),
        }
    }

    /// Every member's provider registry, in load order.
    ///
    /// Read once per question: a host answers `provider_features` through its own
    /// interpreter, so a merge that asked twice would pay twice.
    fn registries(&self) -> Vec<Vec<ProviderFeature>> {
        let registries: Vec<Vec<ProviderFeature>> = self
            .hosts
            .iter()
            .map(|host| host.provider_features())
            .collect();
        {
            // Remember what each host said, so a later callback's switch has a baseline.
            let mut reported = self
                .reported
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for (index, features) in registries.iter().enumerate() {
                if let Some(slot) = reported.get_mut(index) {
                    *slot = snapshot(features);
                }
            }
        }
        registries
    }

    /// Fold a switch a host made on its own into the chain's record.
    ///
    /// A mod can switch a backend inside its own command or dialog callback —
    /// `AxisOverlayJs`'s Apply and toggle do — without going through `set_provider`.
    /// The chain only sees registries, so it compares this host's answer with the one
    /// it last remembered: a change is a switch, and it becomes the chain's answer.
    /// Returns the features this host switched *to* a backend, so the caller can make
    /// the other hosts step aside (`release_losers`).
    fn adopt(&self, index: usize) -> Vec<(String, String)> {
        let Some(host) = self.hosts.get(index) else {
            return Vec::new();
        };
        let features = host.provider_features();
        let now = snapshot(&features);
        let mut reported = self
            .reported
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(previous) = reported.get_mut(index) else {
            return Vec::new();
        };
        let mut selected = self
            .selected
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut switched = Vec::new();
        for (feature, backend) in &now {
            if previous.get(feature).is_some_and(|was| was != backend) {
                match backend {
                    Some(backend) => {
                        selected.insert(feature.clone(), backend.clone());
                        switched.push((feature.clone(), backend.clone()));
                    }
                    None => {
                        selected.remove(feature);
                    }
                }
            }
        }
        *previous = now;
        switched
    }

    /// Make every host that is not `winner` stop drawing the backends `switched` to.
    ///
    /// A host's own `set_provider` only deactivates *its* outgoing backend. When the
    /// switch crosses hosts — `js` on the page to `coin` in Python — the page's
    /// overlay stayed registered and drew beside the Coin cross. The loser has no way
    /// to select the incoming backend (it does not list it), so it is told to release
    /// instead. Returns the releases' merged claim so the caller can fold it under the
    /// incoming backend's own view (which wins).
    fn release_losers(&self, switched: &[(String, String)], winner: usize) -> Option<ViewRequest> {
        let mut released: Option<ViewRequest> = None;
        for (feature, backend) in switched {
            for (index, host) in self.hosts.iter().enumerate() {
                if index == winner {
                    continue;
                }
                let features = host.provider_features();
                let Some(entry) = features.iter().find(|entry| entry.feature == *feature) else {
                    continue;
                };
                let active = entry
                    .providers
                    .iter()
                    .find(|provider| provider.active)
                    .map(|provider| provider.backend.clone());
                if active.as_deref() == Some(backend.as_str()) {
                    continue;
                }
                if let Some(outcome) = host.release_provider(feature) {
                    if let Some(view) = outcome.view {
                        released = merge_release(Some(view), released);
                    }
                }
            }
        }
        if released.is_some() {
            // A release can change that host's own registry (the page returns to its
            // native entry). Refresh the baselines so a later command's registry read
            // is not mistaken for another switch.
            let _ = self.registries();
        }
        released
    }

    /// Re-assert the chain's chosen backends after a host re-activated its own.
    ///
    /// A document activation re-runs each host's declared provider `activate`
    /// *inside* that host (the Python registry's document observer, for example),
    /// which can re-register a backend the chain had switched away from. Releasing
    /// the losers again is what stops two overlays drawing side by side after a
    /// document switch.
    fn enforce_selection(&self) -> Option<ViewRequest> {
        let selected: Vec<(String, String)> = self
            .selected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|(feature, backend)| (feature.clone(), backend.clone()))
            .collect();
        if selected.is_empty() {
            return None;
        }
        let registries = self.registries();
        let mut released = None;
        for (feature, backend) in selected {
            let Some(winner) = registries.iter().position(|features| {
                features.iter().any(|entry| {
                    entry.feature == feature
                        && entry
                            .providers
                            .iter()
                            .any(|provider| provider.backend == backend)
                })
            }) else {
                continue;
            };
            if let Some(view) = self.release_losers(&[(feature, backend)], winner) {
                released = merge_release(Some(view), released);
            }
        }
        released
    }

    /// The merged registry for a set of host answers.
    ///
    /// One entry per feature, in first-host order, with each backend listed once (the
    /// first host's label and command win a duplicate) and exactly one backend marked
    /// active.
    fn merged(&self, registries: &[Vec<ProviderFeature>]) -> Vec<ProviderFeature> {
        let mut merged: Vec<ProviderFeature> = Vec::new();
        for features in registries {
            for feature in features {
                match merged
                    .iter_mut()
                    .find(|entry| entry.feature == feature.feature)
                {
                    Some(existing) => {
                        for provider in &feature.providers {
                            if !existing
                                .providers
                                .iter()
                                .any(|seen| seen.backend == provider.backend)
                            {
                                existing.providers.push(provider.clone());
                            }
                        }
                    }
                    None => merged.push(feature.clone()),
                }
            }
        }
        for feature in &mut merged {
            // The chain's recorded switch wins; otherwise the first listed active
            // backend does. Either way only that one stays active, so the popover and
            // Dev Tools never show two dots for one feature.
            let active = self
                .selected_backend(&feature.feature)
                .filter(|backend| {
                    feature
                        .providers
                        .iter()
                        .any(|provider| &provider.backend == backend)
                })
                .or_else(|| {
                    feature
                        .providers
                        .iter()
                        .find(|provider| provider.active)
                        .map(|provider| provider.backend.clone())
                });
            for provider in &mut feature.providers {
                provider.active = active.as_deref() == Some(provider.backend.as_str());
            }
        }
        merged
    }

    /// The registered feature a synthetic `Settings/<Node>` path names.
    ///
    /// The node spells the feature with its display capitalisation (`Settings/Grid`),
    /// while the id is lower case; the facade resolves the same way, so one map serves
    /// both.
    fn feature_entry<'a>(
        &self,
        merged: &'a [ProviderFeature],
        node: &str,
    ) -> Option<&'a ProviderFeature> {
        merged
            .iter()
            .find(|entry| entry.feature.eq_ignore_ascii_case(node))
    }

    /// The rows under `Settings/<Feature>`, merged across the hosts.
    ///
    /// The selector is the composition's — every backend from every host, one loaded —
    /// so a `cpython,js` run offers Python's and the page's providers in one surface at
    /// the same time. The rows are the feature's **canonical** schema (the shared
    /// `FerroCAD.Axis`/`FerroCAD.Grid` policy), with any provider-specific rows the
    /// loaded backend adds appended by name. A backend that ships neither shows its
    /// settings **command** as the last resort.
    fn settings_rows(&self, object: &str, node: &str) -> Option<Vec<PropRow>> {
        let registries = self.registries();
        let merged = self.merged(&registries);
        let entry = self.feature_entry(&merged, node)?;
        let active = entry
            .providers
            .iter()
            .find(|provider| provider.active)
            .map(|provider| provider.backend.clone());
        // 1. The feature's canonical schema: the first host that lists the feature and
        //    answers. The shared policy lives there, so it stays the one form whichever
        //    backend is loaded.
        let mut rows = Vec::new();
        for (index, features) in registries.iter().enumerate() {
            if !features
                .iter()
                .any(|feature| feature.feature.eq_ignore_ascii_case(&entry.feature))
            {
                continue;
            }
            if let Some(candidate) = self.hosts[index].extra_properties(object) {
                if !candidate.is_empty() {
                    rows = candidate;
                    break;
                }
            }
        }
        // 2. Provider-specific extras: the loaded backend's owner appends any row the
        //    canonical schema does not already name. This is the generic channel for a
        //    backend-only knob; it never replaces the shared rows.
        if let Some(backend) = &active {
            for (index, features) in registries.iter().enumerate() {
                let lists = features.iter().any(|feature| {
                    feature.feature.eq_ignore_ascii_case(&entry.feature)
                        && feature
                            .providers
                            .iter()
                            .any(|provider| &provider.backend == backend)
                });
                if !lists {
                    continue;
                }
                if let Some(candidate) = self.hosts[index].extra_properties(object) {
                    for row in candidate {
                        if row.name != "provider"
                            && !rows.iter().any(|existing| existing.name == row.name)
                        {
                            rows.push(row);
                        }
                    }
                }
            }
        }
        let selector = provider_selector_row(&entry.providers);
        match rows.iter().position(|row| row.name == "provider") {
            Some(position) => rows[position] = selector,
            None => rows.insert(0, selector),
        }
        // No schema means the provider's settings entry is its command (a dialog).
        // Surface it, so `Settings/<Feature>` is never just a selector for that backend.
        let has_schema = rows
            .iter()
            .any(|row| row.name != "provider" && row.command.is_none());
        if !has_schema {
            if let Some(row) = active.as_ref().and_then(|backend| {
                entry
                    .providers
                    .iter()
                    .find(|provider| &provider.backend == backend)
                    .and_then(provider_command_row)
            }) {
                rows.push(row);
            }
        }
        Some(rows)
    }

    /// The backend the chain last selected for `feature`, if any.
    fn selected_backend(&self, feature: &str) -> Option<String> {
        self.selected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(feature)
            .cloned()
    }

    /// Remember the claim the loaded backend just stated, or drop a stale one.
    fn record_claim(&self, feature: &str, view: &Option<ViewRequest>) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match SlotClaim::from_view(view) {
            Some(claim) => {
                claims.insert(feature.to_string(), claim);
            }
            None => {
                claims.remove(feature);
            }
        }
    }

    /// Make a returned view state the loaded backend's claim, not its author's.
    fn reconcile_claim(&self, view: &mut ViewRequest) {
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (feature, claim) in claims.iter() {
            match feature.as_str() {
                "axis" => {
                    if let Some(overlay) = claim.axis_overlay {
                        view.axis.get_or_insert_with(Default::default).overlay = Some(overlay);
                    }
                }
                "grid" => {
                    if let Some(overlay) = claim.grid_overlay {
                        view.grid_overlay = Some(overlay);
                    }
                }
                _ => {}
            }
        }
    }

    /// The index of the host that owns `feature`'s active backend.
    ///
    /// The chain's own record of a switch answers first, because it is the only view
    /// that spans the hosts. At boot — before any switch crossed the chain — the first
    /// host to declare an active backend for the feature owns it, which is the same load
    /// order `commands` and `run_command` use.
    fn provider_owner(&self, registries: &[Vec<ProviderFeature>], feature: &str) -> Option<usize> {
        if let Some(backend) = self.selected_backend(feature) {
            if let Some(index) = registries.iter().position(|features| {
                features.iter().any(|entry| {
                    entry.feature == feature
                        && entry
                            .providers
                            .iter()
                            .any(|provider| provider.backend == backend)
                })
            }) {
                return Some(index);
            }
        }
        // No cross-host choice yet (or it names a backend this build no longer lists):
        // the first host with an active provider answers for the feature.
        registries.iter().position(|features| {
            features.iter().any(|entry| {
                entry.feature == feature && entry.providers.iter().any(|provider| provider.active)
            })
        })
    }

    /// The host that answers a `feature` question: the active backend's owner, else the
    /// first host that lists the feature (a registry with no active backend still has a
    /// refresh and a skin to offer).
    fn feature_host(&self, registries: &[Vec<ProviderFeature>], feature: &str) -> Option<usize> {
        self.provider_owner(registries, feature).or_else(|| {
            registries
                .iter()
                .position(|features| features.iter().any(|entry| entry.feature == feature))
        })
    }

    /// Ask the owner of `feature`'s active backend first, then any host that also
    /// claims the feature.
    ///
    /// The fall-through is the native deferral: a backend whose summary is the
    /// document half's answers `None`, and the other host may still have something
    /// truthful to say (the JS page's summary does, once its overlay is active).
    fn summary_from_owner<T>(
        &self,
        feature: &str,
        ask: impl Fn(&dyn ScriptHost) -> Option<T>,
    ) -> Option<T> {
        let registries = self.registries();
        let owner = self.provider_owner(&registries, feature);
        if let Some(index) = owner {
            if let Some(answer) = ask(self.hosts[index].as_ref()) {
                return Some(answer);
            }
        }
        self.hosts
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != owner)
            .find_map(|(_, host)| ask(host.as_ref()))
    }

    /// The merged backend list for one feature, empty when no host registers it.
    fn feature_providers(&self, feature: &str) -> Vec<crate::types::GridProvider> {
        self.provider_features()
            .into_iter()
            .find(|entry| entry.feature == feature)
            .map(|entry| entry.providers)
            .unwrap_or_default()
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

    /// A fetched pack runs in the primary's interpreter, like the scanned tree.
    ///
    /// A subordinate has no loader of its own to give a pack to, and the primary is the
    /// half that owns the console, so the chain asks it exactly as `load_workbenches`
    /// does. Without this forwarding the browser's two-host chain would answer every
    /// pack with the trait default's empty report and never load one (B19).
    fn load_workbench_pack(&self, pack: &str) -> Result<WorkbenchReport, String> {
        match self.primary() {
            Some(primary) => primary.load_workbench_pack(pack),
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
        let result = self
            .primary()
            .and_then(|primary| primary.activate_document(name));
        if result.as_ref().is_some_and(|outcome| outcome.is_ok()) {
            // The host re-attached its declared provider for the new document; keep
            // only the chain's chosen backend drawing.
            let _ = self.enforce_selection();
        }
        result
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

    /// The primary's grant: the console is its, so a console command gets its answer. A
    /// chain with no primary has nothing to answer with, and `Read` is the honest floor.
    fn document_grant(&self) -> DocumentGrant {
        match self.primary() {
            Some(primary) => primary.document_grant(),
            None => DocumentGrant::Read,
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

    /// The rows of a script-owned node, with the provider settings surface merged.
    ///
    /// `Settings/<Feature>` is composition-wide: the selector lists every host's
    /// backends and the rows are the active backend's, so a composed `cpython,js` run
    /// offers both providers from one node. Any other node keeps the first-answer rule.
    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        if let Some(node) = object.strip_prefix(SETTINGS_PREFIX) {
            if let Some(rows) = self.settings_rows(object, node) {
                return Some(rows);
            }
        }
        self.every().find_map(|host| host.extra_properties(object))
    }

    /// Write one row of a script-owned node.
    ///
    /// On `Settings/<Feature>` the provider selector is the chain's switch (routed to
    /// the host that lists the backend), and every other row belongs to the active
    /// backend's owner, whose store and validation are the real ones.
    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        if let Some(node) = object.strip_prefix(SETTINGS_PREFIX) {
            let registries = self.registries();
            let merged = self.merged(&registries);
            if let Some(entry) = self.feature_entry(&merged, node) {
                if property == "provider" {
                    let backend = entry
                        .providers
                        .iter()
                        .find(|provider| {
                            provider.backend == value || provider_label(provider) == value
                        })
                        .map(|provider| provider.backend.clone());
                    let Some(backend) = backend else {
                        return Some(SetResult {
                            ok: false,
                            error: format!("no provider {value:?} for feature {:?}", entry.feature),
                            view: None,
                        });
                    };
                    return Some(match self.set_provider(&entry.feature, &backend) {
                        Some(outcome) => SetResult {
                            ok: outcome.ok,
                            error: outcome.error,
                            view: outcome.view,
                        },
                        None => SetResult {
                            ok: false,
                            error: format!("no provider {value:?} for feature {:?}", entry.feature),
                            view: None,
                        },
                    });
                }
                // A schema row is written by the host that advertises it: the first host
                // that lists the feature and has a row with this name. That is the
                // canonical host for a shared row (the one whose value the form showed)
                // and the loaded backend's owner for a provider-specific extra.
                for (index, features) in registries.iter().enumerate() {
                    if !features
                        .iter()
                        .any(|feature| feature.feature.eq_ignore_ascii_case(&entry.feature))
                    {
                        continue;
                    }
                    let Some(candidate) = self.hosts[index].extra_properties(object) else {
                        continue;
                    };
                    if !candidate.iter().any(|row| row.name == property) {
                        continue;
                    }
                    let Some(mut result) =
                        self.hosts[index].set_extra_property(object, property, value)
                    else {
                        return None;
                    };
                    // The row may be the shared policy, so the loaded backend's claim —
                    // not the author's — is what the viewport applies.
                    if let Some(view) = result.view.as_mut() {
                        self.reconcile_claim(view);
                    }
                    return Some(result);
                }
                return None;
            }
        }
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

    fn set_setting(&self, group: &str, name: &str, value: &SettingValue) {
        // A subordinate may keep its own parameter store too, so the whole chain hears
        // the seed. A host with no store ignores it (the trait default).
        for host in self.every() {
            host.set_setting(group, name, value);
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
        let mut summary = self.summary_from_owner("grid", |host| host.grid_summary())?;
        // Only a summary that names a provider gains the merged list: the closed empty
        // summary must stay empty so the document half's native provider answers.
        if summary.backend.is_some() || summary.label.is_some() {
            let providers = self.feature_providers("grid");
            if !providers.is_empty() {
                summary.providers = providers;
            }
        }
        Some(summary)
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        let mut summary = self.summary_from_owner("axis", |host| host.axis_summary())?;
        if summary.backend.is_some() || summary.label.is_some() {
            let providers = self.feature_providers("axis");
            if !providers.is_empty() {
                summary.providers = providers;
            }
        }
        Some(summary)
    }

    /// The provider registry, merged across the hosts.
    ///
    /// Each host keeps its own registry, so a composed `cpython,js` run has two lists
    /// that name some of the same backends. The composition rule is per **feature**: one
    /// entry per feature, in first-host order, with each backend listed once (the first
    /// host's label and command win) and exactly one backend marked active. Without the
    /// merge a subordinate's providers were invisible, and the JS half's `axis`/`js`
    /// entry never reached `Mods_Settings`.
    fn provider_features(&self) -> Vec<ProviderFeature> {
        let registries = self.registries();
        self.merged(&registries)
    }

    /// Route the switch to the host that lists `(feature, backend)`.
    ///
    /// The owner is the only host that can run that backend's `deactivate`/`activate`,
    /// and the outcome's `view` is the cross-claim the shell applies (hiding the native
    /// triad, for example). Recording it here is what keeps the next `provider_features`
    /// and `*_summary` looking at the right host.
    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        let registries = self.registries();
        let index = registries.iter().position(|features| {
            features.iter().any(|entry| {
                entry.feature == feature
                    && entry
                        .providers
                        .iter()
                        .any(|provider| provider.backend == backend)
            })
        })?;
        let mut outcome = self.hosts[index].set_provider(feature, backend)?;
        if outcome.ok {
            self.selected
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(feature.to_string(), backend.to_string());
            // The target host deactivated its own outgoing backend; any other host
            // that was drawing this feature has to step aside too.
            self.adopt(index);
            let released =
                self.release_losers(&[(feature.to_string(), backend.to_string())], index);
            outcome.view = merge_release(outcome.view, released);
            self.record_claim(feature, &outcome.view);
        }
        Some(outcome)
    }

    /// Ask the host that owns the feature's active backend to re-read its registry.
    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        let registries = self.registries();
        let index = self.feature_host(&registries, feature)?;
        let outcome = self.hosts[index].refresh_providers(feature)?;
        // A refresh is the loaded backend restating its claim after a shared setting
        // changed; keep it as the one the shell applies.
        self.record_claim(feature, &outcome.view);
        Some(outcome)
    }

    /// The active backend's WGSL skin, from the host that owns it.
    ///
    /// No fall-through on `None`: the owner saying "no skin" means draw the native
    /// material, and another host's skin belongs to a different backend.
    fn provider_skin(&self, feature: &str) -> Option<String> {
        let registries = self.registries();
        match self.feature_host(&registries, feature) {
            Some(index) => self.hosts[index].provider_skin(feature),
            // A feature no host lists (the gizmo in a build with no registry): the
            // primary is the compatible answer.
            None => self
                .primary()
                .and_then(|primary| primary.provider_skin(feature)),
        }
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Option<Vec<serde_json::Value>> {
        self.primary()
            .and_then(|primary| primary.manipulator_specs(object, element))
    }

    fn manipulator_bounds(
        &self,
        object: &str,
        element: Option<&str>,
        manipulator: &str,
    ) -> Option<Result<Option<crate::types::ManipulatorBounds>, String>> {
        // The first member that answers owns the kernel for this element; a member that
        // returns `None` has no opinion, so the next one is asked.
        self.every()
            .find_map(|host| host.manipulator_bounds(object, element, manipulator))
    }

    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        // The primary owns the console but not necessarily the dialog: a subordinate can
        // open one too (a JS workbench's own dialog in a `cpython,js` chain). Route by the
        // token's owner, and keep the old primary-only answer when no host claims it.
        let index = self
            .hosts
            .iter()
            .position(|host| host.owns_dialog(token))
            .or(if self.hosts.is_empty() { None } else { Some(0) })?;
        let outcome = self.hosts[index].dialog_accept(token, values);
        let outcome = outcome?;
        // An accepted dialog can switch a provider as part of applying its values.
        let switched = self.adopt(index);
        let released = self.release_losers(&switched, index);
        let mut outcome = CommandOutcome {
            view: merge_release(outcome.view, released),
            ..outcome
        };
        if let Some(view) = outcome.view.as_mut() {
            self.reconcile_claim(view);
        }
        Some(outcome)
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        let index = self
            .hosts
            .iter()
            .position(|host| host.owns_dialog(token))
            .or(if self.hosts.is_empty() { None } else { Some(0) })?;
        let outcome = self.hosts[index].dialog_cancel(token);
        let outcome = outcome?;
        let switched = self.adopt(index);
        let released = self.release_losers(&switched, index);
        let mut outcome = CommandOutcome {
            view: merge_release(outcome.view, released),
            ..outcome
        };
        if let Some(view) = outcome.view.as_mut() {
            self.reconcile_claim(view);
        }
        Some(outcome)
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
        for (index, host) in self.hosts.iter().enumerate() {
            if let Some(outcome) = host.run_command(id) {
                // The command may have switched a backend itself (the JS settings and
                // toggle commands do); fold that in before the next registry read, and
                // make a host that lost the feature to this one step aside.
                let switched = self.adopt(index);
                let released = self.release_losers(&switched, index);
                let mut outcome = CommandOutcome {
                    view: merge_release(outcome.view, released),
                    ..outcome
                };
                if let Some(view) = outcome.view.as_mut() {
                    self.reconcile_claim(view);
                }
                return Some(outcome);
            }
        }
        None
    }

    fn claims(&self, id: &str) -> Option<CommandOutcome> {
        self.every().find_map(|host| host.claims(id))
    }
}

/// The synthetic namespace whose nodes are built from the provider registry.
const SETTINGS_PREFIX: &str = "Settings/";

/// The incoming backend's view wins; a released host's claim fills a gap it leaves.
///
/// A provider callback states the feature's *claim* (`axis.overlay`, `grid_overlay`),
/// not the drawing policy: the incoming backend's `activate` is what describes the
/// drawn cross or ground, so its own values win and the release only clears a claim
/// the target did not state (a native backend that draws no overlay).
fn merge_release(view: Option<ViewRequest>, released: Option<ViewRequest>) -> Option<ViewRequest> {
    let Some(released) = released.map(claims_only) else {
        return view;
    };
    let Some(mut view) = view else {
        return Some(released);
    };
    if let Some(overlay) = released.axis.and_then(|axis| axis.overlay) {
        let target = view.axis.get_or_insert_with(Default::default);
        if target.overlay.is_none() {
            target.overlay = Some(overlay);
        }
    }
    if view.grid_overlay.is_none() {
        view.grid_overlay = released.grid_overlay;
    }
    Some(view)
}

/// The provider claim in a view, without any of the drawing policy.
fn claims_only(view: ViewRequest) -> ViewRequest {
    ViewRequest {
        axis: view
            .axis
            .and_then(|axis| axis.overlay)
            .map(|overlay| crate::view::AxisRequest {
                overlay: Some(overlay),
                ..Default::default()
            }),
        grid_overlay: view.grid_overlay,
        ..Default::default()
    }
}

/// A provider's user-facing name, falling back to its id (the facade does the same).
fn provider_label(provider: &crate::types::GridProvider) -> String {
    if provider.label.is_empty() {
        provider.backend.clone()
    } else {
        provider.label.clone()
    }
}

/// The `provider` selector row of `Settings/<Feature>`, over the merged backends.
fn provider_selector_row(providers: &[crate::types::GridProvider]) -> PropRow {
    let value = providers
        .iter()
        .find(|provider| provider.active)
        .map(provider_label)
        .unwrap_or_default();
    PropRow {
        name: "provider".to_string(),
        label: "Provider".to_string(),
        type_id: "App::PropertyEnumeration".to_string(),
        value,
        status: "provider".to_string(),
        editable: true,
        command: None,
        kind: "enum".to_string(),
        min: None,
        max: None,
        unit: None,
        choices: providers.iter().map(provider_label).collect(),
    }
}

/// The active provider's settings command, when it has no schema of its own.
///
/// This is the documented fallback (`docs/mods/providers.md`): a provider that ships a
/// dialog instead of a settings schema keeps its `command` as its settings entry.
fn provider_command_row(provider: &crate::types::GridProvider) -> Option<PropRow> {
    let command = provider.command.as_ref()?;
    Some(PropRow {
        name: "settings".to_string(),
        label: format!("{} settings…", provider_label(provider)),
        type_id: "FerroCAD::Command".to_string(),
        value: String::new(),
        status: "action".to_string(),
        editable: false,
        command: Some(command.clone()),
        kind: "command".to_string(),
        min: None,
        max: None,
        unit: None,
        choices: Vec::new(),
    })
}

/// One host's registry as `feature → active backend`.
///
/// A listed feature with no active backend is `None`, which is distinct from a
/// feature the host does not list at all (no entry): only a *listed* feature can
/// report a switch, so `adopt` needs the distinction.
fn snapshot(features: &[ProviderFeature]) -> BTreeMap<String, Option<String>> {
    features
        .iter()
        .map(|feature| {
            (
                feature.feature.clone(),
                feature
                    .providers
                    .iter()
                    .find(|provider| provider.active)
                    .map(|provider| provider.backend.clone()),
            )
        })
        .collect()
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

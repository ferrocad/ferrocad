//! The composition rule, on its own: a chain of hosts behind one primary.
//!
//! `ferrocad_shell_wasm` exercises the same type through real wasm bundles and
//! `ferrocad_shell_web` through real interpreters, which is slow and feature-dependent.
//! These are the two rules that are easy to regress and not obvious from a call site:
//! who owns the console, and who decides whether the chain has a display hook.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::settings::SettingValue;
use ferrocad_shell_api::types::{
    AxisSummary, EvalResult, GridProvider, GridSummary, PropRow, ProviderFeature, SetResult,
};
use ferrocad_shell_api::view::{AxisRequest, ViewRequest};
use ferrocad_shell_api::{DocumentEvent, DocumentGrant, HostChain, ScriptHost};

/// A host that answers its own console line and command, and can offer a display hook.
struct Stub {
    name: &'static str,
    /// Whether `stage_meshes` has a hook. It always returns no rows: the point is whether
    /// the *composition* counts an empty answer as having one.
    hook: bool,
    /// What this host can do to the document.
    grant: DocumentGrant,
    seen: Arc<Mutex<Vec<String>>>,
    /// Every setting the first host of a chain seeded into this half.
    settings: Arc<Mutex<Vec<(String, String, SettingValue)>>>,
    /// The providers this host registers, in listing order: `(feature, backends)`.
    providers: Vec<(&'static str, Vec<&'static str>)>,
    /// The backend this host last switched each feature to.
    chosen: Arc<Mutex<BTreeMap<String, String>>>,
    /// The backend each feature is *drawing*: what `set_provider` activates and
    /// `release_provider` clears. A cross-host switch has to release this side, or
    /// two overlays stay registered and draw on top of each other.
    applied: Arc<Mutex<BTreeMap<String, String>>>,
    /// The rows this host answers for `Settings/<feature>`.
    schema: Option<(&'static str, Vec<PropRow>)>,
}

impl Stub {
    fn new(name: &'static str, hook: bool) -> Self {
        Self {
            name,
            hook,
            grant: DocumentGrant::ReadWrite,
            seen: Arc::new(Mutex::new(Vec::new())),
            settings: Arc::new(Mutex::new(Vec::new())),
            providers: Vec::new(),
            chosen: Arc::new(Mutex::new(BTreeMap::new())),
            applied: Arc::new(Mutex::new(BTreeMap::new())),
            schema: None,
        }
    }

    fn granting(mut self, grant: DocumentGrant) -> Self {
        self.grant = grant;
        self
    }

    /// Register `feature` with `backends`; the first is this host's default active.
    fn providing(mut self, feature: &'static str, backends: &[&'static str]) -> Self {
        self.providers.push((feature, backends.to_vec()));
        self
    }

    /// Answer `Settings/<feature>` with `rows` (plus the selector row the chain replaces).
    fn with_schema(mut self, feature: &'static str, rows: Vec<PropRow>) -> Self {
        self.schema = Some((feature, rows));
        self
    }

    fn chosen(&self, feature: &str) -> Option<String> {
        self.chosen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(feature)
            .cloned()
    }

    /// The backend this host is drawing for `feature`, if any.
    fn active_effect(&self, feature: &str) -> Option<String> {
        self.applied
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(feature)
            .cloned()
    }
}

impl ScriptHost for Stub {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: true,
            output: format!("{}: {code}", self.name),
        })
    }

    fn load_workbenches(&self) -> Result<ferrocad_shell_api::types::WorkbenchReport, String> {
        Ok(Default::default())
    }

    /// Echo the pack back under this host's name, so a test can tell *which* host
    /// answered it. The subject is otherwise the same shape the real halves answer.
    fn load_workbench_pack(
        &self,
        pack: &str,
    ) -> Result<ferrocad_shell_api::types::WorkbenchReport, String> {
        Ok(ferrocad_shell_api::types::WorkbenchReport {
            loaded: vec![format!("{}/{pack}", self.name)],
            errors: Vec::new(),
        })
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("no tool {name}"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn document_grant(&self) -> DocumentGrant {
        self.grant
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        struct Quiet;
        impl EventListener for Quiet {
            fn on_event(
                &mut self,
                _event: &Event,
                _hit: Option<&Hit>,
                _scene: &mut Scene,
                _directives: &Directives,
            ) -> bool {
                false
            }
        }
        Box::new(Quiet)
    }

    fn stage_meshes(&self, _have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        self.hook.then(|| Ok(Vec::new()))
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo {
            id: self.name.to_string(),
            menu: self.name.to_string(),
            ..Default::default()
        }]
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        // `switch:<feature>=<backend>` changes this host's own registry, the way a
        // mod's command or dialog callback does, without going through `set_provider`.
        if let Some(spec) = id.strip_prefix("switch:") {
            let (feature, backend) = spec.split_once('=')?;
            self.set_provider(feature, backend)?;
            return Some(CommandOutcome {
                ok: true,
                status: format!("{} switched", self.name),
                ..Default::default()
            });
        }
        // `claim:<feature>=<bool>` returns a view the way a shared toggle does: the
        // command computes it from this host's registry, which may not be the loaded
        // backend.
        if let Some(spec) = id.strip_prefix("claim:") {
            let (feature, value) = spec.split_once('=')?;
            return Some(CommandOutcome {
                ok: true,
                status: format!("{} claimed", self.name),
                view: Some(claim(feature, value == "true")),
                ..Default::default()
            });
        }
        (id == self.name).then(|| CommandOutcome {
            ok: true,
            status: format!("{} ran it", self.name),
            ..Default::default()
        })
    }

    fn owns_dialog(&self, token: &str) -> bool {
        token.starts_with(self.name)
    }

    /// A document activation re-applies this host's declared backend, the way the
    /// Python registry's document observer does.
    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        if name.is_empty() {
            return None;
        }
        for (feature, backends) in &self.providers {
            let backend = self
                .chosen(feature)
                .unwrap_or_else(|| backends[0].to_string());
            self.applied
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(feature.to_string(), backend);
        }
        Some(Ok(()))
    }

    fn dialog_accept(
        &self,
        token: &str,
        _values: &serde_json::Value,
    ) -> Option<CommandOutcome> {
        self.owns_dialog(token).then(|| CommandOutcome {
            ok: true,
            status: format!("{} accepted {token}", self.name),
            ..Default::default()
        })
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        self.owns_dialog(token).then(|| CommandOutcome {
            ok: true,
            status: format!("{} cancelled {token}", self.name),
            ..Default::default()
        })
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event.id.to_string());
    }

    fn provider_features(&self) -> Vec<ProviderFeature> {
        self.providers
            .iter()
            .map(|(feature, backends)| ProviderFeature {
                feature: feature.to_string(),
                providers: backends
                    .iter()
                    .map(|backend| GridProvider {
                        backend: backend.to_string(),
                        label: format!("{} {backend}", self.name),
                        active: match self.chosen(feature) {
                            Some(chosen) => chosen == *backend,
                            None => *backend == backends[0],
                        },
                        command: Some(format!("{}-{}-settings", self.name, backend)),
                    })
                    .collect(),
            })
            .collect()
    }

    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        let known = self.providers.iter().any(|(registered, backends)| {
            *registered == feature && backends.iter().any(|known| *known == backend)
        });
        if !known {
            return None;
        }
        self.chosen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(feature.to_string(), backend.to_string());
        // The switch deactivates this host's outgoing backend and activates the new
        // one; `applied` is the drawing effect a cross-host switch must release.
        self.applied
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(feature.to_string(), backend.to_string());
        Some(CommandOutcome {
            ok: true,
            status: format!("{} switched {feature} to {backend}", self.name),
            // The native backend claims no overlay; every other one does.
            view: Some(claim(feature, backend != "ferrocad")),
            ..Default::default()
        })
    }

    fn release_provider(&self, feature: &str) -> Option<CommandOutcome> {
        if !self
            .providers
            .iter()
            .any(|(registered, _)| *registered == feature)
        {
            return None;
        }
        self.applied
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(feature);
        Some(CommandOutcome {
            ok: true,
            status: format!("{} released {feature}", self.name),
            view: Some(claim(feature, false)),
            ..Default::default()
        })
    }

    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        self.providers
            .iter()
            .any(|(registered, _)| *registered == feature)
            .then(|| CommandOutcome {
                ok: true,
                status: format!("{} refreshed {feature}", self.name),
                ..Default::default()
            })
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        self.providers
            .iter()
            .find(|(feature, _)| *feature == "axis")
            .map(|(_, backends)| AxisSummary {
                feature: "axis".to_string(),
                backend: self
                    .chosen("axis")
                    .or_else(|| backends.first().map(|b| b.to_string())),
                label: Some(self.name.to_string()),
                ..Default::default()
            })
    }

    fn grid_summary(&self) -> Option<GridSummary> {
        self.providers
            .iter()
            .find(|(feature, _)| *feature == "grid")
            .map(|(_, backends)| GridSummary {
                feature: "grid".to_string(),
                backend: self
                    .chosen("grid")
                    .or_else(|| backends.first().map(|b| b.to_string())),
                label: Some(self.name.to_string()),
                ..Default::default()
            })
    }

    /// The rows of `Settings/<feature>`: a host selector the chain replaces, then the
    /// schema this host owns.
    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        let (feature, schema) = self.schema.as_ref()?;
        let node = object.strip_prefix("Settings/")?;
        if !node.eq_ignore_ascii_case(feature) {
            return None;
        }
        let backends = self
            .providers
            .iter()
            .find(|(registered, _)| registered == feature)
            .map(|(_, backends)| backends)
            .cloned()
            .unwrap_or_default();
        let mut rows = vec![PropRow {
            name: "provider".to_string(),
            label: "Provider".to_string(),
            type_id: "App::PropertyEnumeration".to_string(),
            value: format!(
                "{} {}",
                self.name,
                backends.first().copied().unwrap_or_default()
            ),
            status: "provider".to_string(),
            editable: true,
            command: None,
            kind: "enum".to_string(),
            min: None,
            max: None,
            unit: None,
            choices: backends
                .iter()
                .map(|backend| format!("{} {backend}", self.name))
                .collect(),
        }];
        rows.extend(schema.iter().cloned());
        Some(rows)
    }

    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        let (feature, schema) = self.schema.as_ref()?;
        let node = object.strip_prefix("Settings/")?;
        if !node.eq_ignore_ascii_case(feature) {
            return None;
        }
        if property == "provider" || !schema.iter().any(|row| row.name == property) {
            return None;
        }
        Some(SetResult {
            ok: true,
            error: format!("{}: {property} = {value}", self.name),
            view: None,
        })
    }

    fn provider_skin(&self, feature: &str) -> Option<String> {
        self.providers
            .iter()
            .any(|(registered, _)| *registered == feature)
            .then(|| format!("// {} {feature}", self.name))
    }

    fn set_setting(&self, group: &str, name: &str, value: &SettingValue) {
        self.settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((group.to_string(), name.to_string(), value.clone()));
    }
}

fn chain(stubs: &[&Arc<Stub>]) -> HostChain {
    HostChain::new(
        stubs
            .iter()
            .map(|stub| Arc::clone(stub) as Arc<dyn ScriptHost>)
            .collect(),
    )
}

/// A provider outcome's native-layer claim, in the real hosts' vocabulary.
fn claim(feature: &str, overlay: bool) -> ViewRequest {
    match feature {
        "axis" => ViewRequest {
            axis: Some(AxisRequest {
                overlay: Some(overlay),
                ..Default::default()
            }),
            ..Default::default()
        },
        "grid" => ViewRequest {
            grid_overlay: Some(overlay),
            ..Default::default()
        },
        _ => ViewRequest::default(),
    }
}

#[test]
fn the_first_host_is_the_primary() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    // One console, and it is the first host's.
    assert_eq!(chain.evaluate("x").expect("console").output, "first: x");
    assert_eq!(chain.len(), 2);
    assert!(!chain.is_empty());
}

#[test]
fn settings_reach_every_host() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    chain.set_setting(
        "Mods/Providers",
        "grid",
        &SettingValue::String("ferrocad".to_string()),
    );

    for stub in [&first, &second] {
        assert_eq!(
            *stub
                .settings
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![(
                "Mods/Providers".to_string(),
                "grid".to_string(),
                SettingValue::String("ferrocad".to_string())
            )]
        );
    }
}

#[test]
fn commands_concatenate_and_the_first_answer_wins() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    let ids: Vec<String> = chain.commands().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, ["first", "second"]);
    assert_eq!(
        chain.run_command("second").expect("the subordinate").status,
        "second ran it"
    );
    assert!(chain.run_command("nobody").is_none());
}

#[test]
fn the_primary_decides_whether_the_chain_has_a_display_hook() {
    // A primary with no hook: the backend's own rows stay, even if a subordinate offers an
    // (empty) hook. This is the rule the desktop's addon composition had, kept intact.
    let bare = Arc::new(Stub::new("bare", false));
    let hooked = Arc::new(Stub::new("hooked", true));
    assert!(chain(&[&bare, &hooked]).stage_meshes(&[]).is_none());

    // A primary that answers `Ok` — even with no rows — means the chain has a hook.
    match chain(&[&hooked, &bare]).stage_meshes(&[]) {
        Some(Ok(rows)) => assert!(rows.is_empty()),
        other => panic!("the primary's answer decides: {other:?}"),
    }

    // Nothing installed at all is still no hook.
    assert!(HostChain::new(Vec::new()).stage_meshes(&[]).is_none());
}

#[test]
fn a_dialog_goes_to_the_host_that_minted_it() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);
    let values = serde_json::json!({});

    // A subordinate may open a dialog even though the primary owns the console.
    assert_eq!(
        chain
            .dialog_accept("second-1", &values)
            .expect("the subordinate owns it")
            .status,
        "second accepted second-1"
    );
    assert_eq!(
        chain
            .dialog_accept("first-1", &values)
            .expect("the primary owns it")
            .status,
        "first accepted first-1"
    );
    assert!(
        chain.dialog_accept("nobody-1", &values).is_none(),
        "no owner, and the primary does not claim it"
    );
    assert_eq!(
        chain
            .dialog_cancel("second-1")
            .expect("the subordinate owns it")
            .status,
        "second cancelled second-1"
    );
}

#[test]
fn a_fetched_pack_runs_in_the_primary() {
    // `load_workbenches` scans a tree the browser does not have; a fetched pack is the
    // no-filesystem path, and it must reach the same half — the first host. Without the
    // forwarding the trait default would answer an empty report and a browser chain
    // would never load a pack (B19).
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    assert_eq!(
        chain(&[&first, &second])
            .load_workbench_pack("demo")
            .expect("the primary answers")
            .loaded,
        ["first/demo"]
    );
    assert!(
        HostChain::new(Vec::new())
            .load_workbench_pack("demo")
            .expect("an empty chain is not an error")
            .loaded
            .is_empty(),
        "no host loads nothing, the same answer as a fetched-nothing page"
    );
}

#[test]
fn every_host_hears_the_document() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    chain.on_document_event(&DocumentEvent::edited("Create", &["Toy".to_string()]));

    // A subordinate keeps its own state in step with a document it did not write.
    assert_eq!(first.seen.lock().unwrap().as_slice(), ["document.edited"]);
    assert_eq!(second.seen.lock().unwrap().as_slice(), ["document.edited"]);
}

#[test]
fn the_chain_reports_the_primarys_grant() {
    let writer = Arc::new(Stub::new("writer", true));
    let reader = Arc::new(Stub::new("reader", true).granting(DocumentGrant::Read));

    // The console is the primary's, so its grant is the one a console command gets.
    assert_eq!(
        chain(&[&writer, &reader]).document_grant(),
        DocumentGrant::ReadWrite
    );
    assert_eq!(
        chain(&[&reader, &writer]).document_grant(),
        DocumentGrant::Read
    );
}

#[test]
fn an_empty_chain_cannot_write() {
    // No host, no console: `Read` is the honest floor rather than the trait's default.
    assert_eq!(
        HostChain::new(Vec::new()).document_grant(),
        DocumentGrant::Read
    );
}

/// The axis feature as a `cpython,js` composition sees it: both halves register the
/// native backend, each adds one of its own.
fn axis_chain() -> (Arc<Stub>, Arc<Stub>, HostChain) {
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);
    (python, js, chain)
}

#[test]
fn provider_registries_merge_across_the_chain() {
    let (_python, _js, chain) = axis_chain();

    let features = chain.provider_features();
    assert_eq!(features.len(), 1, "one entry per feature: {features:?}");
    let axis = &features[0];
    assert_eq!(
        axis.providers
            .iter()
            .map(|provider| provider.backend.as_str())
            .collect::<Vec<_>>(),
        ["ferrocad", "coin", "js"],
        "each backend once, in first-host order"
    );
    assert_eq!(
        axis.providers[0].label, "python ferrocad",
        "the primary's label wins a duplicate backend"
    );
    assert_eq!(
        axis.providers
            .iter()
            .filter(|provider| provider.active)
            .map(|provider| provider.backend.as_str())
            .collect::<Vec<_>>(),
        ["ferrocad"],
        "the popovers see exactly one active backend"
    );
}

#[test]
fn a_switch_routes_to_the_host_that_lists_the_backend() {
    let (_python, js, chain) = axis_chain();

    let outcome = chain
        .set_provider("axis", "js")
        .expect("the JS half lists it");
    assert_eq!(outcome.status, "js switched axis to js");

    // The subordinate's own switch is now the composition's answer, even though the
    // primary's registry still names `ferrocad` active.
    let axis = chain
        .provider_features()
        .into_iter()
        .find(|feature| feature.feature == "axis")
        .expect("the merged feature");
    assert_eq!(
        axis.providers
            .iter()
            .filter(|provider| provider.active)
            .map(|provider| provider.backend.as_str())
            .collect::<Vec<_>>(),
        ["js"]
    );
    assert!(js.chosen("axis").is_some(), "the page ran the switch");

    // Summaries, refresh and the skin follow the owner of the active backend.
    let summary = chain.axis_summary().expect("the JS summary");
    assert_eq!(summary.backend.as_deref(), Some("js"));
    assert_eq!(summary.label.as_deref(), Some("js"));
    assert_eq!(
        summary
            .providers
            .iter()
            .map(|provider| provider.backend.as_str())
            .collect::<Vec<_>>(),
        ["ferrocad", "coin", "js"],
        "the summary still offers the other backends"
    );
    assert_eq!(
        chain.refresh_providers("axis").expect("refresh").status,
        "js refreshed axis"
    );
    assert_eq!(chain.provider_skin("axis").as_deref(), Some("// js axis"));

    // And back: the primary owns `coin`, so its own registry answers again.
    let outcome = chain
        .set_provider("axis", "coin")
        .expect("the Python half lists it");
    assert_eq!(outcome.status, "python switched axis to coin");
    assert_eq!(
        chain
            .axis_summary()
            .expect("the Python summary")
            .backend
            .as_deref(),
        Some("coin")
    );
}

/// A command that runs inside the primary computes its claim from the primary's
/// registry; the chain must make the loaded backend's claim the one the shell applies.
#[test]
fn a_command_claim_follows_the_loaded_backend() {
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    chain.set_provider("axis", "js").expect("the page lists it");
    let outcome = chain
        .run_command("claim:axis=false")
        .expect("the primary answers");
    assert_eq!(
        outcome
            .view
            .and_then(|view| view.axis)
            .and_then(|axis| axis.overlay),
        Some(true),
        "the loaded JS backend hides the native cross, not the primary's claim"
    );
}

/// A switch that crosses hosts must deactivate the outgoing host, not just the
/// incoming host's own predecessor. Otherwise both overlays stayed registered and
/// drew on top of each other.
#[test]
fn a_cross_host_switch_releases_the_outgoing_backend() {
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    // Activate the page's own backend. Its claim (`overlay: true`) must be the one
    // the shell applies, even though the primary was released in the same switch.
    let outcome = chain.set_provider("axis", "js").expect("the page lists it");
    assert_eq!(js.active_effect("axis").as_deref(), Some("js"));
    assert_eq!(python.active_effect("axis"), None, "the primary released");
    assert_eq!(
        outcome
            .view
            .and_then(|view| view.axis)
            .and_then(|axis| axis.overlay),
        Some(true),
        "the incoming backend's claim wins the merge"
    );

    // Switching to a backend the page does not list has to release its overlay.
    chain
        .set_provider("axis", "coin")
        .expect("the Python half lists it");
    assert_eq!(python.active_effect("axis").as_deref(), Some("coin"));
    assert_eq!(
        js.active_effect("axis"),
        None,
        "the page's overlay was released, not left drawing beside the Coin cross"
    );
}

/// A command can switch a provider inside its own host (the JS toggle and Apply do).
/// That path must release the other host too, exactly like a routed switch.
#[test]
fn a_switch_inside_a_command_releases_the_other_host() {
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    chain
        .set_provider("axis", "ferrocad")
        .expect("the primary applies its native backend");
    assert_eq!(python.active_effect("axis").as_deref(), Some("ferrocad"));

    // The page's toggle switches `axis` to `js` without `HostChain::set_provider`.
    chain
        .run_command("switch:axis=js")
        .expect("the page answers");
    assert_eq!(js.active_effect("axis").as_deref(), Some("js"));
    assert_eq!(
        python.active_effect("axis"),
        None,
        "the page's switch released the primary's native cross"
    );
}

/// A document activation re-runs a host's declared provider inside that host. The
/// chain has to re-release the hosts it is not drawing from, or two overlays come
/// back on the next document switch.
#[test]
fn a_document_switch_releases_the_host_that_re_attached() {
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    chain.set_provider("axis", "js").expect("the page lists it");
    assert_eq!(python.active_effect("axis"), None);

    // The primary's document observer re-applies its declared `ferrocad`.
    chain
        .activate_document("Doc")
        .expect("the primary answers")
        .expect("the document activated");
    assert_eq!(
        python.active_effect("axis"),
        None,
        "the chain released the re-attached backend again"
    );
    assert_eq!(js.active_effect("axis").as_deref(), Some("js"));
}

#[test]
fn a_switch_inside_a_command_reaches_the_chain() {
    let (_python, _js, chain) = axis_chain();
    // The shell reads the registry at boot, which is what gives `adopt` a baseline.
    let _ = chain.provider_features();

    // The JS settings/toggle commands switch inside the page, never through
    // `HostChain::set_provider`; the chain still has to hear about it.
    assert_eq!(
        chain
            .run_command("switch:axis=js")
            .expect("the JS half answers")
            .status,
        "js switched"
    );
    assert_eq!(
        chain
            .axis_summary()
            .expect("the JS summary")
            .backend
            .as_deref(),
        Some("js")
    );

    // A later command on the primary leaves its registry alone, so it must not flip
    // the recorded switch back to the primary's stale default.
    chain.run_command("python").expect("the primary answers");
    assert_eq!(
        chain
            .axis_summary()
            .expect("the JS summary")
            .backend
            .as_deref(),
        Some("js")
    );
}

fn schema_row(name: &str, value: &str) -> PropRow {
    PropRow {
        name: name.to_string(),
        label: String::new(),
        type_id: "App::PropertyString".to_string(),
        value: value.to_string(),
        status: String::new(),
        editable: true,
        command: None,
        kind: "text".to_string(),
        min: None,
        max: None,
        unit: None,
        choices: Vec::new(),
    }
}

#[test]
fn the_settings_node_lists_both_hosts_providers_and_the_active_schema() {
    let python = Arc::new(
        Stub::new("python", true)
            .providing("axis", &["ferrocad", "coin"])
            .with_schema("axis", vec![schema_row("length", "80 px")]),
    );
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    // The node is the composition's: one selector over every host's backends, and the
    // active backend's schema beside it.
    let rows = chain
        .extra_properties("Settings/Axis")
        .expect("the composition builds the node");
    let selector = rows
        .iter()
        .find(|row| row.name == "provider")
        .expect("the selector row");
    assert_eq!(selector.value, "python ferrocad");
    assert_eq!(
        selector.choices,
        ["python ferrocad", "python coin", "js js"],
        "both hosts' backends, one entry for the shared ferrocad"
    );
    assert!(
        rows.iter().any(|row| row.name == "length"),
        "the active provider's schema is shown"
    );
    assert!(
        rows.iter().all(|row| row.command.is_none()),
        "a schema is the settings surface; no dialog row is added"
    );

    // A schema write goes to the backend that owns the rows.
    let written = chain
        .set_extra_property("Settings/Axis", "length", "90 px")
        .expect("the active owner claims it");
    assert_eq!(written.error, "python: length = 90 px");

    // The selector write is the chain's switch, routed to the host that lists it.
    let switched = chain
        .set_extra_property("Settings/Axis", "provider", "js js")
        .expect("the selector is the chain's");
    assert!(switched.ok, "{switched:?}");

    // The page's backend ships no schema of its own: the feature's canonical rows stay
    // in place instead of being replaced by a provider-specific dialog row. One form,
    // whichever backend is loaded.
    let rows = chain
        .extra_properties("Settings/Axis")
        .expect("the composition builds the node");
    let selector = rows
        .iter()
        .find(|row| row.name == "provider")
        .expect("the selector row");
    assert_eq!(selector.value, "js js");
    assert!(
        rows.iter().any(|row| row.name == "length"),
        "the canonical schema is the one form"
    );
    assert!(
        rows.iter().all(|row| row.command.is_none()),
        "no second, provider-specific form is added"
    );
    let written = chain
        .set_extra_property("Settings/Axis", "length", "70 px")
        .expect("the canonical host still owns the row");
    assert_eq!(written.error, "python: length = 70 px");
}

#[test]
fn a_loaded_backend_appends_its_own_rows_to_the_one_form() {
    let python = Arc::new(
        Stub::new("python", true)
            .providing("axis", &["ferrocad", "coin"])
            .with_schema("axis", vec![schema_row("length", "80 px")]),
    );
    let js = Arc::new(
        Stub::new("js", true)
            .providing("axis", &["js"])
            .with_schema(
                "axis",
                vec![schema_row("length", "999 px"), schema_row("trace", "on")],
            ),
    );
    let chain = chain(&[&python, &js]);

    chain.set_provider("axis", "js").expect("js is listed");
    let rows = chain
        .extra_properties("Settings/Axis")
        .expect("the composition builds the node");
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert!(names.contains(&"provider"));
    assert!(names.contains(&"length"), "the canonical row stays");
    assert!(
        names.contains(&"trace"),
        "the loaded backend's extra row is appended"
    );
    let length = rows.iter().find(|row| row.name == "length").expect("length");
    assert_eq!(length.value, "80 px", "the canonical row wins a duplicate");
    assert!(
        rows.iter().all(|row| row.command.is_none()),
        "a schema means no command row"
    );

    // The extra is written by its owner; the shared row by the canonical host.
    let extra = chain
        .set_extra_property("Settings/Axis", "trace", "off")
        .expect("the page owns its extra");
    assert_eq!(extra.error, "js: trace = off");
    let shared = chain
        .set_extra_property("Settings/Axis", "length", "90 px")
        .expect("the canonical host owns the shared row");
    assert_eq!(shared.error, "python: length = 90 px");
}

#[test]
fn python_and_js_providers_are_active_at_the_same_time() {
    let python = Arc::new(
        Stub::new("python", true)
            .providing("grid", &["ferrocad"])
            .providing("axis", &["ferrocad", "coin"]),
    );
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    // The grid stays the Python half's; the axis switches to the page's backend.
    chain
        .set_provider("axis", "js")
        .expect("the JS half owns it");

    assert_eq!(
        chain
            .grid_summary()
            .expect("the Python grid")
            .backend
            .as_deref(),
        Some("ferrocad")
    );
    assert_eq!(
        chain
            .axis_summary()
            .expect("the JS axis")
            .backend
            .as_deref(),
        Some("js")
    );

    let features = chain.provider_features();
    let active = |feature: &str| {
        features
            .iter()
            .find(|entry| entry.feature == feature)
            .and_then(|entry| entry.providers.iter().find(|provider| provider.active))
            .map(|provider| provider.backend.clone())
    };
    assert_eq!(
        active("grid").as_deref(),
        Some("ferrocad"),
        "Python's grid provider is live"
    );
    assert_eq!(
        active("axis").as_deref(),
        Some("js"),
        "and the JS axis provider is live at the same time"
    );
}

#[test]
fn a_switch_to_a_backend_no_host_lists_is_refused() {
    let (_python, _js, chain) = axis_chain();

    assert!(chain.set_provider("axis", "nowhere").is_none());
    // Nothing was recorded, so the merge still names the primary's default.
    assert_eq!(
        chain
            .provider_features()
            .into_iter()
            .flat_map(|feature| feature.providers)
            .find(|provider| provider.active)
            .map(|provider| provider.backend),
        Some("ferrocad".to_string())
    );
}

#[test]
fn a_boot_switch_in_one_registry_wins_over_the_others_default() {
    // A persisted `coin` in the Python half: the merge must not also mark the JS half's
    // default `ferrocad` active, and the summary must be the Python one.
    let python = Arc::new(Stub::new("python", true).providing("axis", &["ferrocad", "coin"]));
    python
        .chosen
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert("axis".to_string(), "coin".to_string());
    let js = Arc::new(Stub::new("js", true).providing("axis", &["ferrocad", "js"]));
    let chain = chain(&[&python, &js]);

    let active: Vec<String> = chain
        .provider_features()
        .into_iter()
        .flat_map(|feature| feature.providers)
        .filter(|provider| provider.active)
        .map(|provider| provider.backend)
        .collect();
    assert_eq!(active, ["coin"]);
    assert_eq!(
        chain
            .axis_summary()
            .expect("the Python summary")
            .backend
            .as_deref(),
        Some("coin")
    );
}

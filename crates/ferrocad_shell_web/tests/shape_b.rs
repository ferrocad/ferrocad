//! Shape B, end to end: a script half that holds **no document** still draws one.
//!
//! This is the JavaScript wiring without a browser. [`DoorwayHost`] does exactly what
//! `www/addons/toy.js` does — it keeps no `Document`, and its `stage_meshes` answers by
//! calling [`DocumentApi`] — so the test exercises the whole chain the browser uses:
//!
//! ```text
//! addon ──▶ DocApi ──▶ ferrocad_core (one registry) ──▶ RustAppBackend::stage_meshes
//! ```
//!
//! The object it draws is `App::FeaturePython`: in the tree, no `Shape`, so the
//! document half cannot mesh it and the hook is the only thing that puts it on screen.

use std::sync::{Arc, Mutex, MutexGuard};

use ferrocad_shell_api::app::{AppBackend, ScriptHost};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};
use ferrocad_shell_api::{DocumentApi, NoopListener};
use serde_json::{Value, json};

/// The document registry is process-wide; these tests take turns with it. Without
/// this, a second `bootstrap()` replaces the open document's contents in place — which
/// is what an open does — and the first test's object disappears mid-run.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

fn clear_registry() {
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
}

/// What a workbench draws: one flat-shaded quad standing `height` millimetres tall.
///
/// Field by field, because this is the literal a page's hook produces: only the
/// arrays and the display hint matter, and everything else falls back.
fn slab(entity: u32, height: f32) -> StageMesh {
    StageMesh {
        entity,
        object: "Toy".to_string(),
        label: "Toy".to_string(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, height],
            [0.0, 10.0, height],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        triangles: vec![0, 1, 2, 0, 2, 3],
        triangle_parts: vec![0, 0],
        edges: Vec::new(),
        segment_parts: Vec::new(),
        vertices: Vec::new(),
        vertex_parts: Vec::new(),
        parts: Vec::new(),
        origin: None,
        placement: None,
        color: None,
        display: Some("shaded".to_string()),
    }
}

/// The script half: no document, one doorway, one object it draws.
struct DoorwayHost {
    api: Arc<dyn DocumentApi>,
    /// Every `have` list the backend passed, so the test cannot assert a guess.
    seen: Arc<Mutex<Vec<Vec<String>>>>,
}

impl DoorwayHost {
    fn call(&self, request: Value) -> Value {
        serde_json::from_str(&self.api.call(&request.to_string())).expect("a JSON response")
    }
}

impl ScriptHost for DoorwayHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: true,
            output: code.to_string(),
        })
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("toy: no tool {name}"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<ferrocad_inventor::Scene, String> {
        Ok(ferrocad_inventor::Scene::default())
    }

    fn event_listener(&self) -> Box<dyn ferrocad_inventor::EventListener> {
        Box::new(NoopListener)
    }

    /// The bundle's display hook: one object, skipped when the document has it.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(have.iter().map(|name| name.to_string()).collect());

        let document = self.call(json!({ "call": "document" }));
        let object = document["result"]["objects"]
            .as_array()?
            .iter()
            .find(|candidate| candidate["name"] == "Toy")?;
        if have.contains(&"Toy") {
            return Some(Ok(Vec::new()));
        }
        let height = object["properties"]
            .as_array()?
            .iter()
            .find(|row| row["name"] == "Height")
            .and_then(|row| row["value"].as_str())
            .and_then(|text| text.split_whitespace().next())
            .and_then(|number| number.parse::<f32>().ok())?;
        let entity = object["entity"].as_u64()? as u32;
        Some(Ok(vec![slab(entity, height)]))
    }
}

fn compose(seen: Arc<Mutex<Vec<Vec<String>>>>) -> (Arc<dyn DocumentApi>, Box<dyn AppBackend>) {
    let api: Arc<dyn DocumentApi> = Arc::new(ferrocad_doc_api::DocApi::new());
    let backend = ferrocad_shell_rust::RustAppBackend::from_shared_registry().with_script_host(
        Box::new(DoorwayHost {
            api: Arc::clone(&api),
            seen,
        }),
    );
    (api, Box::new(backend))
}

/// The bundle's own object: `App::FeaturePython` with a declared height, created
/// through the doorway exactly as the page does.
fn create_toy(api: &Arc<dyn DocumentApi>) {
    let answer: Value = serde_json::from_str(
        &api.call(
            &json!({
                "call": "create",
                "name": "Toy",
                "type": "App::FeaturePython",
                "properties": {
                    "Height": { "type": "App::PropertyLength", "value": "20 mm" }
                }
            })
            .to_string(),
        ),
    )
    .expect("a JSON response");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["name"], "Toy");
}

fn toy<'a>(stage: &'a [StageMesh]) -> &'a StageMesh {
    stage
        .iter()
        .find(|mesh| mesh.object == "Toy")
        .unwrap_or_else(|| panic!("the hook's mesh is in the stage; got {:?}", names(stage)))
}

fn names(stage: &[StageMesh]) -> Vec<&str> {
    stage.iter().map(|mesh| mesh.object.as_str()).collect()
}

#[test]
fn a_script_half_draws_what_the_document_cannot() {
    let _serial = serial();
    clear_registry();
    let (api, mut backend) = compose(Arc::new(Mutex::new(Vec::new())));
    let boot = backend.bootstrap().expect("the sample boots");
    assert_eq!(boot.document, "Sample");
    create_toy(&api);

    let stage = backend.stage_meshes().expect("a stage");
    // The document meshed its own shape ...
    assert!(names(&stage).contains(&"Fillet"), "{:?}", names(&stage));
    // ... and the hook supplied the one it cannot.
    assert_eq!(
        toy(&stage).positions[3][2],
        20.0,
        "the height the doorway reported"
    );

    // Editing through the doorway changes what the hook draws on the next sync.
    let set: Value = serde_json::from_str(
        &api.call(
            &json!({ "call": "set", "object": "Toy", "property": "Height", "value": "40 mm" })
                .to_string(),
        ),
    )
    .expect("a JSON response");
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["value"], "40 mm");

    let stage = backend.stage_meshes().expect("a stage");
    assert_eq!(toy(&stage).positions[3][2], 40.0, "the taller slab");
}

#[test]
fn the_hook_is_told_what_the_document_already_meshed() {
    let _serial = serial();
    clear_registry();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (_api, mut backend) = compose(Arc::clone(&seen));
    backend.bootstrap().expect("the sample boots");

    backend.stage_meshes().expect("a stage");
    let seen = seen.lock().unwrap_or_else(|error| error.into_inner());
    let have = seen.last().expect("the hook was asked");
    assert!(have.contains(&"Fillet".to_string()), "{have:?}");
    assert!(!have.contains(&"Toy".to_string()), "{have:?}");
}

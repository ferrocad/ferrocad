//! The spring workbench: parameters in the document, geometry in the bundle.
//!
//! The document object is an `App::FeaturePython` with three declared properties and no
//! `Shape` at all. That is deliberate: the bundle cannot read geometry, so it derives it
//! from numbers it *can* read, and the property pane edits those numbers in the usual way.
//! The display hook is where the derived mesh goes.
//!
//! Pure: the api is a closure, so this module is tested on the host against the real
//! [`ferrocad_doc_api`]. `abi.rs` supplies the same closure over an import in wasm.

use serde_json::{Value, json};

/// The object this bundle owns.
pub const SPRING: &str = "Spring";

/// The mean radius of the helix, in millimetres.
pub const RADIUS: f64 = 8.0;

/// The axial advance per turn, in millimetres.
pub const PITCH: f64 = 4.0;

/// The number of active turns.
pub const TURNS: f64 = 8.0;

/// The tightest pitch `Spring_Tighten` will produce, in millimetres.
pub const MIN_PITCH: f64 = 1.0;

/// Points per turn in the drawn helix. Coarse on purpose: it is a preview of the
/// parameters, not the wire the kernel would sweep.
const STEPS_PER_TURN: f64 = 24.0;

/// One entry of the bundle's command list.
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub tooltip: &'static str,
}

/// What this bundle registers, in palette order.
pub const COMMANDS: [Command; 2] = [
    Command {
        id: "Spring_Insert",
        label: "Insert Spring",
        tooltip: "Create a parametric spring: three numbers the bundle draws a helix from",
    },
    Command {
        id: "Spring_Tighten",
        label: "Tighten Spring",
        tooltip: "Halve the pitch, down to 1 mm",
    },
];

/// [`COMMANDS`] as the JSON the loader registers.
pub fn commands_json() -> String {
    let commands: Vec<Value> = COMMANDS
        .iter()
        .map(|command| {
            json!({
                "id": command.id,
                "label": command.label,
                "tooltip": command.tooltip,
            })
        })
        .collect();
    Value::Array(commands).to_string()
}

/// Run one command. `Ok(false)` means "not this bundle's id", not a failure.
pub fn run(id: &str, api: &dyn Fn(&str) -> String) -> Result<bool, String> {
    match id {
        "Spring_Insert" => insert(api).map(|()| true),
        "Spring_Tighten" => tighten(api).map(|()| true),
        _ => Ok(false),
    }
}

/// The display hook's answer: a JSON array of meshes (`[]` when there is nothing).
pub fn stage_meshes(have: &[String], api: &dyn Fn(&str) -> String) -> Result<String, String> {
    if have.iter().any(|name| name == SPRING) {
        return Ok("[]".to_string());
    }
    let document = call(api, json!({ "call": "document" }))?;
    let Some(object) = object_named(&document, SPRING) else {
        return Ok("[]".to_string());
    };
    let radius = number_of(object, "Radius").unwrap_or(RADIUS);
    let pitch = number_of(object, "Pitch").unwrap_or(PITCH);
    let turns = number_of(object, "Turns").unwrap_or(TURNS);
    serde_json::to_string(&[helix_mesh(object, radius, pitch, turns)])
        .map_err(|error| format!("the mesh is not JSON: {error}"))
}

/// Create `Spring` with its declared parameters, unless it is already there.
fn insert(api: &dyn Fn(&str) -> String) -> Result<(), String> {
    let existing = call(api, json!({ "call": "properties", "object": SPRING }))?;
    if existing["ok"] == true {
        return Ok(());
    }
    // A declaration, the way `obj.addProperty(...)` works in Python: the type has no
    // such properties, and the workbench defines them.
    let answer = call(
        api,
        json!({
            "call": "create",
            "name": SPRING,
            "type": "App::FeaturePython",
            "properties": {
                "Radius": {
                    "type": "App::PropertyLength",
                    "value": format!("{RADIUS} mm"),
                    "group": "Spring",
                    "doc": "Mean radius of the helix",
                },
                "Pitch": {
                    "type": "App::PropertyLength",
                    "value": format!("{PITCH} mm"),
                    "group": "Spring",
                    "doc": "Axial advance per turn",
                },
                "Turns": {
                    "type": "App::PropertyFloat",
                    "value": format!("{TURNS}"),
                    "group": "Spring",
                    "doc": "Number of active turns",
                },
            }
        }),
    )?;
    ok_or_refusal(&answer)
}

/// Halve the pitch, down to [`MIN_PITCH`].
fn tighten(api: &dyn Fn(&str) -> String) -> Result<(), String> {
    let rows = call(api, json!({ "call": "properties", "object": SPRING }))?;
    if rows["ok"] != true {
        return Err("no spring to tighten: run Spring_Insert first".to_string());
    }
    let pitch = number_in(&rows["result"], "Pitch").unwrap_or(PITCH);
    let next = (pitch / 2.0).max(MIN_PITCH);
    let answer = call(
        api,
        json!({
            "call": "set",
            "object": SPRING,
            "property": "Pitch",
            "value": format!("{next} mm"),
        }),
    )?;
    ok_or_refusal(&answer)
}

fn call(api: &dyn Fn(&str) -> String, request: Value) -> Result<Value, String> {
    let answer = api(&request.to_string());
    serde_json::from_str(&answer)
        .map_err(|error| format!("the api answered invalid JSON: {error}"))
}

fn ok_or_refusal(answer: &Value) -> Result<(), String> {
    if answer["ok"] == true {
        Ok(())
    } else {
        Err(answer["error"]
            .as_str()
            .unwrap_or("the api refused")
            .to_string())
    }
}

fn object_named<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["result"]["objects"]
        .as_array()?
        .iter()
        .find(|object| object["name"] == name)
}

/// The number in `name`'s row of a whole object (`"8 mm"` → `8`).
fn number_of(object: &Value, name: &str) -> Option<f64> {
    number_in(&object["properties"], name)
}

/// The number in `name`'s row of a property-row array.
fn number_in(rows: &Value, name: &str) -> Option<f64> {
    let row = rows
        .as_array()?
        .iter()
        .find(|row| row["name"] == name)?;
    row["value"]
        .as_str()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The helix the document cannot hold: a wire along the three parameters.
///
/// A wireframe, not a swept solid: the point is to show what the numbers mean without
/// pretending the bundle can sweep a kernel it does not have.
fn helix_mesh(object: &Value, radius: f64, pitch: f64, turns: f64) -> Value {
    let radius = radius.max(0.1);
    let pitch = pitch.max(0.1);
    let turns = turns.clamp(0.5, 200.0);
    let segments = ((turns * STEPS_PER_TURN).round() as usize).clamp(8, 4_800);
    let tau = std::f64::consts::TAU;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(segments + 1);
    let mut edges: Vec<u32> = Vec::with_capacity(segments * 2);
    for step in 0..=segments {
        let turn = turns * step as f64 / segments as f64;
        let angle = tau * turn;
        positions.push([
            (radius * angle.cos()) as f32,
            (radius * angle.sin()) as f32,
            (pitch * turn) as f32,
        ]);
        if step > 0 {
            edges.extend_from_slice(&[(step - 1) as u32, step as u32]);
        }
    }

    json!({
        "entity": object["entity"],
        "object": SPRING,
        "label": object["label"],
        "positions": positions,
        // One segment per step, all in one part: the pane names it "Helix".
        "edges": edges,
        "segment_parts": vec![0u32; segments],
        "parts": ["Helix"],
        "color": [0.35, 0.55, 0.85, 1.0],
        "display": "wireframe",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_shell_api::DocumentApi;
    use std::sync::{Mutex, MutexGuard};

    /// The document registry is process-wide; these tests take turns with it.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn boot() -> ferrocad_doc_api::DocApi {
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
        registry.new_document(Some("Bundle"));
        ferrocad_doc_api::DocApi::new()
    }

    fn document_api(api: &ferrocad_doc_api::DocApi) -> impl Fn(&str) -> String + '_ {
        move |request| api.call(request)
    }

    fn rows(stage: &str) -> Vec<Value> {
        serde_json::from_str(stage).expect("the hook answers with a JSON array")
    }

    #[test]
    fn the_catalogue_is_json() {
        let commands: Value = serde_json::from_str(&commands_json()).expect("valid JSON");
        assert_eq!(commands[0]["id"], "Spring_Insert");
        assert_eq!(commands[1]["id"], "Spring_Tighten");
    }

    #[test]
    fn an_id_that_is_not_ours_is_not_a_failure() {
        let _serial = serial();
        let temp = boot();
        let document_api = document_api(&temp);
        assert_eq!(run("Somebody_Else", &document_api), Ok(false));
    }

    #[test]
    fn inserting_declares_the_parameters() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);

        assert_eq!(run("Spring_Insert", &document_api), Ok(true));

        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        let object = object_named(&document, SPRING).expect("the spring is in the document");
        assert_eq!(object["type_id"], "App::FeaturePython");
        // No shape: the bundle draws it, the document stores only what it was told.
        assert_eq!(object["entity"], 1);
        let names: Vec<&str> = object["properties"]
            .as_array()
            .expect("rows")
            .iter()
            .filter_map(|row| row["name"].as_str())
            .collect();
        assert_eq!(names, ["Pitch", "Radius", "Turns"], "{names:?}");
        assert_eq!(number_of(object, "Radius"), Some(RADIUS));
        assert_eq!(number_of(object, "Turns"), Some(TURNS));

        // Inserting again is not an error and does not duplicate the object.
        assert_eq!(run("Spring_Insert", &document_api), Ok(true));
        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        assert_eq!(
            document["result"]["objects"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn the_hook_draws_the_helix_from_the_parameters() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);

        // Before the object exists, the hook adds nothing.
        assert!(rows(&stage_meshes(&[], &document_api).expect("a hook answer")).is_empty());

        assert_eq!(run("Spring_Insert", &document_api), Ok(true));
        let drawn = rows(&stage_meshes(&[], &document_api).expect("a hook answer"));
        assert_eq!(drawn.len(), 1);
        let mesh = &drawn[0];
        assert_eq!(mesh["object"], SPRING);
        assert_eq!(mesh["display"], "wireframe");
        // 8 turns × 24 steps, plus the closing point.
        let positions = mesh["positions"].as_array().expect("positions");
        assert_eq!(positions.len(), 193);
        assert_eq!(mesh["edges"].as_array().map(Vec::len), Some(384));
        assert_eq!(mesh["segment_parts"].as_array().map(Vec::len), Some(192));
        // The standing height is the pitch times the turns.
        let top = positions
            .iter()
            .filter_map(|point| point[2].as_f64())
            .fold(f64::MIN, f64::max);
        assert!((top - PITCH * TURNS).abs() < 1e-6, "{top}");
        // The widest point is the radius.
        let widest = positions
            .iter()
            .filter_map(|point| point[0].as_f64())
            .fold(f64::MIN, f64::max);
        assert!((widest - RADIUS).abs() < 0.5, "{widest}");

        // A host that already meshed the spring is not drawn again.
        assert_eq!(
            stage_meshes(&[SPRING.to_string()], &document_api),
            Ok("[]".to_string())
        );
    }

    #[test]
    fn tightening_halves_the_pitch_and_the_hook_follows() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);
        assert_eq!(run("Spring_Insert", &document_api), Ok(true));

        assert_eq!(run("Spring_Tighten", &document_api), Ok(true));
        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        let object = object_named(&document, SPRING).expect("the spring");
        assert_eq!(number_of(object, "Pitch"), Some(PITCH / 2.0));

        // Down to the floor, and no further.
        for _ in 0..8 {
            assert_eq!(run("Spring_Tighten", &document_api), Ok(true));
        }
        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        let object = object_named(&document, SPRING).expect("the spring");
        assert_eq!(number_of(object, "Pitch"), Some(MIN_PITCH));
    }

    #[test]
    fn tightening_without_a_spring_says_what_to_run() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);
        let error = run("Spring_Tighten", &document_api).unwrap_err();
        assert!(error.contains("Spring_Insert"), "{error}");
    }
}

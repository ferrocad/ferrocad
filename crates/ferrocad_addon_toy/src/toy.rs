//! The toy workbench: one object, two commands, one display hook.
//!
//! Pure. The document is a closure — the api — so this module is tested on the host
//! against the real [`ferrocad_doc_api`] rather than only in a browser. The wasm shell
//! in `abi.rs` supplies the same closure over an import.

use serde_json::{Value, json};

/// The object this bundle owns. It has no `Shape`, so only the hook can draw it.
pub const TOY: &str = "Toy";

/// `Toy_Slab`'s height, in millimetres.
pub const HEIGHT: f64 = 20.0;

/// `Toy_Taller`'s height, in millimetres.
pub const TALLER: f64 = 40.0;

/// The slab's footprint, in millimetres per side.
pub const FOOTPRINT: f64 = 20.0;

/// One entry of the bundle's command list.
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub tooltip: &'static str,
}

/// What this bundle registers, in palette order.
pub const COMMANDS: [Command; 2] = [
    Command {
        id: "Toy_Slab",
        label: "Toy Slab",
        tooltip: "Create (or reset) the 20 × 20 mm toy slab",
    },
    Command {
        id: "Toy_Taller",
        label: "Toy Taller",
        tooltip: "Raise the toy slab to 40 mm",
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
        "Toy_Slab" => set_height(HEIGHT, api).map(|()| true),
        "Toy_Taller" => set_height(TALLER, api).map(|()| true),
        _ => Ok(false),
    }
}

/// The display hook's answer: a JSON array of meshes (`[]` when there is nothing).
///
/// `have` names the objects the document already meshed, so the hook does not
/// re-tessellate a shape the host has.
pub fn stage_meshes(have: &[String], api: &dyn Fn(&str) -> String) -> Result<String, String> {
    if have.iter().any(|name| name == TOY) {
        return Ok("[]".to_string());
    }
    let document = call(api, json!({ "call": "document" }))?;
    let Some(object) = object_named(&document, TOY) else {
        return Ok("[]".to_string());
    };
    let height = height_of(object).unwrap_or(HEIGHT);
    serde_json::to_string(&[slab_mesh(object, height)])
        .map_err(|error| format!("the mesh is not JSON: {error}"))
}

/// Create `Toy` at `height`, or edit the existing one.
fn set_height(height: f64, api: &dyn Fn(&str) -> String) -> Result<(), String> {
    let value = format!("{height} mm");
    let existing = call(api, json!({ "call": "properties", "object": TOY }))?;
    let answer = if existing["ok"] == true {
        call(
            api,
            json!({ "call": "set", "object": TOY, "property": "Height", "value": value }),
        )?
    } else {
        // A declaration, the way `obj.addProperty(...)` works in Python: the type has
        // no `Height` of its own, and the workbench defines it.
        call(
            api,
            json!({
                "call": "create",
                "name": TOY,
                "type": "App::FeaturePython",
                "properties": {
                    "Height": {
                        "type": "App::PropertyLength",
                        "value": value,
                        "group": "Base",
                        "doc": "Height of the slab",
                    }
                }
            }),
        )?
    };
    if answer["ok"] == true {
        Ok(())
    } else {
        Err(answer["error"]
            .as_str()
            .unwrap_or("the api refused")
            .to_string())
    }
}

fn call(api: &dyn Fn(&str) -> String, request: Value) -> Result<Value, String> {
    let answer = api(&request.to_string());
    serde_json::from_str(&answer)
        .map_err(|error| format!("the api answered invalid JSON: {error}"))
}

fn object_named<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["result"]["objects"]
        .as_array()?
        .iter()
        .find(|object| object["name"] == name)
}

/// The number in the `Height` row (`"20 mm"`), when the object has one.
fn height_of(object: &Value) -> Option<f64> {
    let row = object["properties"]
        .as_array()?
        .iter()
        .find(|row| row["name"] == "Height")?;
    row["value"]
        .as_str()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// A flat-shaded box on the origin plane, `height` millimetres tall.
///
/// Six quads with their own vertices and one normal each, so the shading is flat
/// without a normal-average pass. `entity` is the pick id the api reported, which
/// is what makes a click resolve to `Toy`.
fn slab_mesh(object: &Value, height: f64) -> Value {
    let half = FOOTPRINT / 2.0;
    let top = height.max(0.1);
    let corners = [
        [-half, -half, 0.0],
        [half, -half, 0.0],
        [half, half, 0.0],
        [-half, half, 0.0],
        [-half, -half, top],
        [half, -half, top],
        [half, half, top],
        [-half, half, top],
    ];
    // Counter-clockwise from outside, so each normal points away from the body.
    let faces: [([f64; 3], [usize; 4]); 6] = [
        ([0.0, 0.0, -1.0], [0, 3, 2, 1]),
        ([0.0, 0.0, 1.0], [4, 5, 6, 7]),
        ([0.0, -1.0, 0.0], [0, 1, 5, 4]),
        ([1.0, 0.0, 0.0], [1, 2, 6, 5]),
        ([0.0, 1.0, 0.0], [2, 3, 7, 6]),
        ([-1.0, 0.0, 0.0], [3, 0, 4, 7]),
    ];

    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut triangles = Vec::with_capacity(36);
    let mut edges = Vec::with_capacity(48);
    for (normal, face) in faces {
        let base = positions.len() as u32;
        for corner in face {
            positions.push(corners[corner]);
            normals.push(normal);
        }
        triangles.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        // The face's own outline; shared edges are drawn twice, at the same depth.
        edges.extend_from_slice(&[
            base,
            base + 1,
            base + 1,
            base + 2,
            base + 2,
            base + 3,
            base + 3,
            base,
        ]);
    }

    json!({
        "entity": object["entity"],
        "object": TOY,
        "label": object["label"],
        "positions": positions,
        "normals": normals,
        "triangles": triangles,
        "triangle_parts": vec![0u32; triangles.len() / 3],
        "edges": edges,
        "segment_parts": vec![0u32; edges.len() / 2],
        "parts": ["Slab"],
        "color": [0.78, 0.55, 0.30, 1.0],
        "display": "shaded_edges",
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

    /// A clean registry with one empty document, and the real api over it.
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

    fn meshes(stage: &str) -> Vec<Value> {
        serde_json::from_str(stage).expect("the hook answers with a JSON array")
    }

    fn tallest(mesh: &Value) -> f64 {
        let positions = mesh["positions"].as_array().expect("positions");
        positions
            .iter()
            .filter_map(|point| point[2].as_f64())
            .fold(f64::MIN, f64::max)
    }

    #[test]
    fn the_catalogue_is_json() {
        let _serial = serial();
        let commands = meshes(&commands_json());
        assert_eq!(commands[0]["id"], "Toy_Slab");
        assert_eq!(commands[1]["id"], "Toy_Taller");
    }

    #[test]
    fn a_command_creates_its_own_object_through_the_document_api() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);

        assert_eq!(run("Toy_Slab", &document_api), Ok(true));

        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        let object = object_named(&document, TOY).expect("the toy is in the document");
        assert_eq!(object["type_id"], "App::FeaturePython");
        assert_eq!(height_of(object), Some(HEIGHT));
        // The document API's pick identity, and only the property the bundle declared.
        assert_eq!(object["entity"], 1);
        let properties = object["properties"].as_array().expect("rows");
        assert_eq!(properties.len(), 1, "{properties:?}");
        assert_eq!(properties[0]["name"], "Height");
        assert_eq!(properties[0]["kind"], "quantity");
    }

    #[test]
    fn the_hook_draws_the_slab_the_document_cannot() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);

        // Before the object exists, the hook adds nothing.
        assert!(meshes(&stage_meshes(&[], &document_api).expect("a hook answer")).is_empty());

        assert_eq!(run("Toy_Slab", &document_api), Ok(true));
        let stage = stage_meshes(&[], &document_api).expect("a hook answer");
        let rows = meshes(&stage);
        assert_eq!(rows.len(), 1);
        let mesh = &rows[0];
        assert_eq!(mesh["object"], TOY);
        assert_eq!(mesh["entity"], 1);
        assert_eq!(mesh["display"], "shaded_edges");
        assert_eq!(mesh["positions"].as_array().map(Vec::len), Some(24));
        assert_eq!(mesh["triangles"].as_array().map(Vec::len), Some(36));
        // `triangle_parts` is one entry per triangle, `segment_parts` one per edge.
        assert_eq!(mesh["triangle_parts"].as_array().map(Vec::len), Some(12));
        assert_eq!(mesh["segment_parts"].as_array().map(Vec::len), Some(24));
        assert_eq!(tallest(mesh), HEIGHT);

        // The taller command edits the same object; the hook reads the new height.
        assert_eq!(run("Toy_Taller", &document_api), Ok(true));
        let rows = meshes(&stage_meshes(&[], &document_api).expect("a hook answer"));
        assert_eq!(tallest(&rows[0]), TALLER);
        // The object was edited, not duplicated.
        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        assert_eq!(
            document["result"]["objects"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn the_hook_skips_what_the_document_already_meshed() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);
        assert_eq!(run("Toy_Slab", &document_api), Ok(true));

        assert_eq!(
            stage_meshes(&[TOY.to_string()], &document_api),
            Ok("[]".to_string())
        );
    }

    #[test]
    fn someone_elses_command_is_refused_as_absent() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);
        assert_eq!(run("Someone_Else", &document_api), Ok(false));
    }
}

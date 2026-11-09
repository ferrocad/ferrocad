//! The spring workbench: parameters in the document, geometry in the bundle.
//!
//! The document object is an `App::FeaturePython` with four declared properties and no
//! `Shape` at all. That is deliberate: the bundle cannot read geometry, so it derives it
//! from numbers it *can* read, and the property pane edits those numbers in the usual way.
//! The display hook is where the derived mesh goes.
//!
//! The mesh is a **tube**, not a wire: a circle swept along the helix. The bundle has no
//! kernel, but a sweep is arithmetic — a ring of points per station, a quad per pair of
//! stations, a normal per point — so "no kernel" is not a reason to draw a line.
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

/// The diameter of the wire the helix is drawn with, in millimetres.
pub const WIRE: f64 = 1.0;

/// The tightest pitch `Spring_Tighten` will produce, in millimetres.
pub const MIN_PITCH: f64 = 1.0;

/// Stations along the drawn helix. A preview of the parameters rather than the mesh a
/// kernel would export, so this is coarse on purpose.
const STEPS_PER_TURN: f64 = 24.0;

/// Points around the drawn tube. Smooth enough at any zoom a preview is read at, and
/// cheap: the whole default spring is under 5 000 triangles.
const RING: usize = 12;

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
        tooltip: "Create a parametric spring: four numbers the bundle sweeps a tube from",
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
    let wire = number_of(object, "Wire").unwrap_or(WIRE);
    serde_json::to_string(&[tube_mesh(object, radius, pitch, turns, wire)])
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
                "Wire": {
                    "type": "App::PropertyLength",
                    "value": format!("{WIRE} mm"),
                    "group": "Spring",
                    "doc": "Diameter of the wire the helix is drawn with",
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

/// The solid the document cannot hold: a circle swept along the helix.
///
/// A ring of points at every station, a quad between neighbouring stations, and a fan at
/// each end. The frame is the helix's own: the outward radial direction and `tangent ×
/// normal` turn with the curve, so the tube does not twist as it climbs.
fn tube_mesh(object: &Value, radius: f64, pitch: f64, turns: f64, wire: f64) -> Value {
    let radius = radius.max(0.1);
    let pitch = pitch.max(0.1);
    let turns = turns.clamp(0.5, 200.0);
    // Coils that interpenetrate are not a spring. Keep the tube inside the pitch, however
    // thick `Wire` asks for, so tightening cannot fold the surface through itself.
    let wire = (wire / 2.0).clamp(0.05, pitch * 0.3);
    let segments = ((turns * STEPS_PER_TURN).round() as usize).clamp(8, 4_800);
    let tau = std::f64::consts::TAU;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity((segments + 1) * RING + 2);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity((segments + 1) * RING + 2);
    // The centre and the frame at each station, kept for the caps.
    let mut stations: Vec<([f64; 3], [f64; 3], [f64; 3], [f64; 3])> =
        Vec::with_capacity(segments + 1);

    for step in 0..=segments {
        let turn = turns * step as f64 / segments as f64;
        let angle = tau * turn;
        let centre = [radius * angle.cos(), radius * angle.sin(), pitch * turn];
        // `d/dturn`: never parallel to Z, so the cross below is well defined.
        let tangent = unit([-radius * angle.sin(), radius * angle.cos(), pitch / tau]);
        let normal = unit(cross(tangent, [0.0, 0.0, 1.0]));
        let binormal = cross(tangent, normal);
        stations.push((centre, normal, binormal, tangent));
        for point in 0..RING {
            let theta = tau * point as f64 / RING as f64;
            let (sin, cos) = theta.sin_cos();
            // Outward from the helix's centreline, and exactly the vertex normal.
            let offset = [
                cos * normal[0] + sin * binormal[0],
                cos * normal[1] + sin * binormal[1],
                cos * normal[2] + sin * binormal[2],
            ];
            positions.push([
                (centre[0] + wire * offset[0]) as f32,
                (centre[1] + wire * offset[1]) as f32,
                (centre[2] + wire * offset[2]) as f32,
            ]);
            normals.push([offset[0] as f32, offset[1] as f32, offset[2] as f32]);
        }
    }

    // Two triangles per quad, wound so the outward side faces out: around the ring is
    // `∂θ`, along the helix is `∂s`, and `∂θ × ∂s` is the normal above.
    let at = |station: usize, point: usize| (station * RING + point) as u32;
    let mut triangles: Vec<u32> = Vec::with_capacity(segments * RING * 6 + RING * 6);
    for station in 0..segments {
        for point in 0..RING {
            let next = (point + 1) % RING;
            let (a, b) = (at(station, point), at(station, next));
            let (c, d) = (at(station + 1, next), at(station + 1, point));
            triangles.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }

    // The ends, as fans: the start faces back down the helix, the end forward.
    let (centre, _, _, tangent) = stations[0];
    let start = positions.len() as u32;
    positions.push([centre[0] as f32, centre[1] as f32, centre[2] as f32]);
    normals.push([-tangent[0] as f32, -tangent[1] as f32, -tangent[2] as f32]);
    for point in 0..RING {
        triangles.extend_from_slice(&[at(0, point), start, at(0, (point + 1) % RING)]);
    }

    let (centre, _, _, tangent) = stations[segments];
    let end = positions.len() as u32;
    positions.push([centre[0] as f32, centre[1] as f32, centre[2] as f32]);
    normals.push([tangent[0] as f32, tangent[1] as f32, tangent[2] as f32]);
    for point in 0..RING {
        triangles.extend_from_slice(&[
            at(segments, point),
            at(segments, (point + 1) % RING),
            end,
        ]);
    }

    json!({
        "entity": object["entity"],
        "object": SPRING,
        "label": object["label"],
        "positions": positions,
        "normals": normals,
        "triangles": triangles,
        "color": [0.35, 0.55, 0.85, 1.0],
        "display": "shaded",
    })
}

/// `a × b`.
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `v`, scaled to unit length (or left at zero).
fn unit(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 {
        return [0.0, 0.0, 0.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
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
        assert_eq!(names, ["Pitch", "Radius", "Turns", "Wire"], "{names:?}");
        assert_eq!(number_of(object, "Radius"), Some(RADIUS));
        assert_eq!(number_of(object, "Turns"), Some(TURNS));
        assert_eq!(number_of(object, "Wire"), Some(WIRE));

        // Inserting again is not an error and does not duplicate the object.
        assert_eq!(run("Spring_Insert", &document_api), Ok(true));
        let document = call(&document_api, json!({ "call": "document" })).expect("a document");
        assert_eq!(
            document["result"]["objects"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn the_hook_draws_a_tube_from_the_parameters() {
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
        assert_eq!(
            mesh["display"], "shaded",
            "a swept tube is drawn as a surface, not a wire"
        );

        // 8 turns × 24 stations, a dozen points around each, and one centre per cap.
        let positions = mesh["positions"].as_array().expect("positions");
        assert_eq!(positions.len(), (8 * 24 + 1) * 12 + 2);
        assert_eq!(mesh["normals"].as_array().map(Vec::len), Some(positions.len()));
        // A quad per station and ring point, plus a fan at each end.
        let triangles = mesh["triangles"].as_array().expect("triangles");
        assert_eq!(triangles.len(), 8 * 24 * 12 * 6 + 2 * 12 * 3);
        assert!(triangles.iter().all(|index| {
            index.as_u64().is_some_and(|index| index < positions.len() as u64)
        }));

        // The standing height is the pitch times the turns, give or take the wire.
        let top = positions
            .iter()
            .filter_map(|point| point[2].as_f64())
            .fold(f64::MIN, f64::max);
        assert!((top - (PITCH * TURNS + WIRE / 2.0)).abs() < 0.5, "{top}");
        // The widest point is the mean radius plus half the wire.
        let widest = positions
            .iter()
            .filter_map(|point| point[0].as_f64())
            .fold(f64::MIN, f64::max);
        assert!((widest - (RADIUS + WIRE / 2.0)).abs() < 0.5, "{widest}");

        // A host that already meshed the spring is not drawn again.
        assert_eq!(
            stage_meshes(&[SPRING.to_string()], &document_api),
            Ok("[]".to_string())
        );
    }

    #[test]
    fn a_thicker_wire_is_drawn_thicker_but_never_through_itself() {
        let _serial = serial();
        let api = boot();
        let document_api = document_api(&api);
        assert_eq!(run("Spring_Insert", &document_api), Ok(true));

        let widest = |stage: &str| {
            rows(stage)[0]["positions"]
                .as_array()
                .expect("positions")
                .iter()
                .filter_map(|point| point[0].as_f64())
                .fold(f64::MIN, f64::max)
        };

        let thin = widest(&stage_meshes(&[], &document_api).expect("a hook answer"));
        let answer = call(
            &document_api,
            json!({ "call": "set", "object": SPRING, "property": "Wire", "value": "3 mm" }),
        )
        .expect("the set answers");
        assert_eq!(answer["ok"], true, "{answer}");
        let thick = widest(&stage_meshes(&[], &document_api).expect("a hook answer"));
        assert!(thick > thin, "{thick} vs {thin}");
        // `Wire` is 3 mm here but the pitch is 4 mm, so the drawn radius stops at
        // 0.3 × pitch: coils that overlap are not a spring.
        assert!(thick <= RADIUS + PITCH * 0.3 + 0.01, "{thick}");
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

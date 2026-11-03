//! The JSON wire form of a mod-declared handle.
//!
//! A Python mod describes a handle as data; the shell validates that data here and
//! builds the stage's [`DeclarativeHandle`]. The stage owns the behaviour — screen
//! sizing, axis projection, snapping, commit — while this module owns only the
//! transport contract:
//!
//! ```json
//! {
//!   "id": "toy.bolt",
//!   "action": "bolt",
//!   "element": "Face3",
//!   "target": 7,
//!   "frame": { "origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "z": [0, 0, 1] },
//!   "axis": "z",
//!   "kind": "axis",
//!   "radius": 66.0,
//!   "value": 20.0,
//!   "min": -100.0,
//!   "max": 100.0,
//!   "step": 1.0,
//!   "size": 40.0,
//!   "decimals": 2,
//!   "shape": { "positions": [[0, 0, 0]], "normals": [], "triangles": [], "edges": [] },
//!   "program": "<wgsl source>",
//!   "color": "#38bdf8",
//!   "readout": "H {value} mm"
//! }
//! ```
//!
//! Unknown keys are ignored and every optional key has the default shown above or
//! documented on [`DeclarativeHandle`]. Validation is total: a bad declaration
//! fails before any geometry is built, with an error naming the field and the
//! offending value.
//!
//! **The frame belongs to the mod.** Only each basis vector's length is
//! normalised; the three directions are not re-orthogonalised or re-handed. A mod
//! that supplies a skewed frame gets exactly the frame it declared, which keeps a
//! rotation/scale frame from being silently reinterpreted here.

use std::sync::Arc;

use serde::Deserialize;

use ferrocad_stage::grid::parse_srgb_hex;
use ferrocad_stage::{
    DeclarativeHandle, DeclarativeKind, DeclarativeShape, ManipulatorFrame, ShaderProgram,
};
use ferrocad_types::Vector3;

/// The default handle size, in device pixels.
const DEFAULT_SIZE: f64 = 40.0;
/// The default ring radius, in device pixels.
const DEFAULT_RADIUS: f64 = 66.0;
/// The default number of readout decimals.
const DEFAULT_DECIMALS: usize = 2;
/// The largest accepted readout precision.
const MAX_DECIMALS: usize = 6;

/// One handle as it appears on the wire.
///
/// `Option` distinguishes "absent" from a meaningful zero (`size`, `decimals`,
/// `step`), while the container-level `serde(default)` makes every key optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct HandleSpec {
    id: String,
    action: Option<String>,
    element: Option<String>,
    target: Option<u32>,
    frame: FrameSpec,
    axis: Option<String>,
    kind: Option<String>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    size: Option<f64>,
    radius: Option<f64>,
    decimals: Option<usize>,
    shape: ShapeSpec,
    program: Option<String>,
    color: Option<String>,
    readout: Option<String>,
}

/// The world frame the handle lives in. The default is the identity at the origin.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct FrameSpec {
    origin: [f64; 3],
    x: [f64; 3],
    y: [f64; 3],
    z: [f64; 3],
}

impl Default for FrameSpec {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0, 0.0],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
            z: [0.0, 0.0, 1.0],
        }
    }
}

/// The handle mesh as it appears on the wire.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ShapeSpec {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    triangles: Vec<u32>,
    edges: Vec<u32>,
}

/// Build a handle from a JSON value.
///
/// The returned target is the spec's own `target` when present, otherwise the
/// `target` argument (the shell passes the selected entity). A spec-declared
/// target wins because a mod may deliberately steer a handle at another entity;
/// the target is not part of [`DeclarativeHandle`].
///
/// Every field is validated before the handle is built, so a returned error means
/// nothing was constructed.
pub fn handle_from_value(
    value: &serde_json::Value,
    target: u32,
) -> Result<(u32, DeclarativeHandle), String> {
    let spec: HandleSpec =
        serde_json::from_value(value.clone()).map_err(|error| format!("handle: {error}"))?;
    convert(spec, target)
}

/// Validate a decoded spec and build the stage handle.
fn convert(spec: HandleSpec, target: u32) -> Result<(u32, DeclarativeHandle), String> {
    let id = spec.id.trim().to_string();
    if id.is_empty() {
        return Err("handle id is required (it must be a non-empty string)".to_string());
    }
    let label = format!("handle {id:?}: ");

    let action = spec
        .action
        .map(|action| action.trim().to_string())
        .filter(|action| !action.is_empty())
        .unwrap_or_else(|| id.clone());

    let frame = convert_frame(&spec.frame, &label)?;
    let axis = convert_axis(spec.axis.as_deref(), &label)?;
    let kind = convert_kind(spec.kind.as_deref(), &label)?;
    let value = finite(spec.value, "value", &label)?;
    let min = optional_finite(spec.min, "min", &label)?;
    let max = optional_finite(spec.max, "max", &label)?;
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            return Err(format!(
                "{label}min must be <= max, got {min} > {max}"
            ));
        }
    }
    let step = match optional_finite(spec.step, "step", &label)? {
        Some(step) if step < 0.0 => {
            return Err(format!("{label}step must be >= 0, got {step}"));
        }
        Some(step) if step == 0.0 => None,
        step => step,
    };
    let size = match spec.size {
        Some(size) => {
            let size = finite(size, "size", &label)?;
            if size <= 0.0 {
                return Err(format!("{label}size must be > 0, got {size}"));
            }
            size
        }
        None => DEFAULT_SIZE,
    };
    let decimals = spec.decimals.unwrap_or(DEFAULT_DECIMALS);
    if decimals > MAX_DECIMALS {
        return Err(format!(
            "{label}decimals must be <= {MAX_DECIMALS}, got {decimals}"
        ));
    }
    let radius = match spec.radius {
        Some(radius) => {
            let radius = finite(radius, "radius", &label)?;
            if radius <= 0.0 {
                return Err(format!("{label}radius must be > 0, got {radius}"));
            }
            radius
        }
        None => DEFAULT_RADIUS,
    };
    let shape = convert_shape(&spec.shape, &label)?;

    let program = match spec.program {
        Some(source) if !source.trim().is_empty() => {
            Some(Arc::new(ShaderProgram::from_source(source).with_geometry()))
        }
        // An empty or whitespace-only source means "no skin": there is nothing to
        // compile, and treating it as absent keeps a declarative handle on the
        // workspace style's materials.
        _ => None,
    };
    let color = match spec.color.as_deref() {
        Some(text) => Some(
            parse_srgb_hex(text)
                .map_err(|error| format!("{label}color {text:?}: {error}"))?,
        ),
        None => None,
    };
    let element = spec
        .element
        .filter(|element| !element.trim().is_empty());
    let readout = spec.readout.unwrap_or_default();

    let mut handle = DeclarativeHandle::new(id, action, frame, axis, shape);
    handle.element = element;
    handle.kind = kind;
    handle.value = value;
    handle.min = min;
    handle.max = max;
    handle.step = step;
    handle.size = size;
    handle.radius = radius;
    handle.decimals = decimals;
    handle.program = program;
    handle.color = color;
    handle.readout = readout;

    Ok((spec.target.unwrap_or(target), handle))
}

fn convert_frame(frame: &FrameSpec, label: &str) -> Result<ManipulatorFrame, String> {
    let origin = finite_vector(frame.origin, "frame.origin", label)?;
    let basis = [
        unit_vector(frame.x, "frame.x", label)?,
        unit_vector(frame.y, "frame.y", label)?,
        unit_vector(frame.z, "frame.z", label)?,
    ];
    Ok(ManipulatorFrame { origin, basis })
}

fn convert_axis(axis: Option<&str>, label: &str) -> Result<usize, String> {
    match axis {
        None => Ok(2),
        Some(text) => match text.trim().to_ascii_lowercase().as_str() {
            "x" => Ok(0),
            "y" => Ok(1),
            "z" => Ok(2),
            _ => Err(format!(
                "{label}axis must be \"x\", \"y\" or \"z\", got {text:?}"
            )),
        },
    }
}

fn convert_kind(kind: Option<&str>, label: &str) -> Result<DeclarativeKind, String> {
    match kind {
        None => Ok(DeclarativeKind::Axis),
        Some(text) => match text.trim().to_ascii_lowercase().as_str() {
            "axis" => Ok(DeclarativeKind::Axis),
            "ring" => Ok(DeclarativeKind::Ring),
            _ => Err(format!(
                "{label}kind must be \"axis\" or \"ring\", got {text:?}"
            )),
        },
    }
}

fn convert_shape(shape: &ShapeSpec, label: &str) -> Result<DeclarativeShape, String> {
    if shape.positions.is_empty() {
        return Err(format!("{label}positions are required"));
    }

    let mut positions = Vec::with_capacity(shape.positions.len());
    for (index, position) in shape.positions.iter().enumerate() {
        positions.push(point(*position, &format!("positions[{index}]"), label)?);
    }

    if !shape.normals.is_empty() && shape.normals.len() != positions.len() {
        return Err(format!(
            "{label}normals has {} entries but positions has {}",
            shape.normals.len(),
            positions.len()
        ));
    }
    let mut normals = Vec::with_capacity(shape.normals.len());
    for (index, normal) in shape.normals.iter().enumerate() {
        normals.push(point(*normal, &format!("normals[{index}]"), label)?);
    }

    for (index, triangle) in shape.triangles.iter().enumerate() {
        if *triangle as usize >= positions.len() {
            return Err(format!(
                "{label}triangles[{index}] index {triangle} is out of range ({} positions)",
                positions.len()
            ));
        }
    }
    for (index, edge) in shape.edges.iter().enumerate() {
        if *edge as usize >= positions.len() {
            return Err(format!(
                "{label}edges[{index}] index {edge} is out of range ({} positions)",
                positions.len()
            ));
        }
    }

    Ok(DeclarativeShape {
        positions,
        normals,
        triangles: shape.triangles.clone(),
        edges: shape.edges.clone(),
    })
}

fn finite(value: f64, field: &str, label: &str) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{label}{field} is not finite ({value})"))
    }
}

fn optional_finite(
    value: Option<f64>,
    field: &str,
    label: &str,
) -> Result<Option<f64>, String> {
    value.map(|value| finite(value, field, label)).transpose()
}

fn finite_vector(values: [f64; 3], field: &str, label: &str) -> Result<Vector3, String> {
    let mut out = [0.0; 3];
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(format!(
                "{label}{field}[{index}] is not finite ({value})"
            ));
        }
        out[index] = *value;
    }
    Ok(Vector3::new(out[0], out[1], out[2]))
}

fn unit_vector(values: [f64; 3], field: &str, label: &str) -> Result<Vector3, String> {
    let vector = finite_vector(values, field, label)?;
    let length = vector.length();
    if !length.is_finite() {
        return Err(format!("{label}{field} is out of range"));
    }
    if length == 0.0 {
        return Err(format!("{label}{field} is zero-length"));
    }
    Ok(vector.scale(1.0 / length))
}

/// Check a unit-space mesh vertex and fit it into the stage's `f32` storage.
fn point(values: [f64; 3], field: &str, label: &str) -> Result<[f32; 3], String> {
    let vector = finite_vector(values, field, label)?;
    let point = [vector.x as f32, vector.y as f32, vector.z as f32];
    if point.iter().any(|value| !value.is_finite()) {
        return Err(format!("{label}{field} is out of range for a mesh vertex"));
    }
    Ok(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "id": "toy.bolt",
            "shape": { "positions": [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]] }
        })
    }

    fn assert_close(left: f64, right: f64, what: &str) {
        assert!((left - right).abs() < 1e-12, "{what}: {left} != {right}");
    }

    #[test]
    fn a_minimal_spec_uses_the_documented_defaults() {
        let (target, handle) = handle_from_value(&minimal(), 7).expect("a valid handle");

        assert_eq!(target, 7);
        assert_eq!(handle.id, "toy.bolt");
        assert_eq!(handle.action, "toy.bolt", "action defaults to the id");
        assert_eq!(handle.element, None);
        assert_eq!(handle.axis, 2, "the default axis is z");
        assert_eq!(handle.value, 0.0);
        assert_eq!(handle.min, None);
        assert_eq!(handle.max, None);
        assert_eq!(handle.step, None);
        assert_eq!(handle.size, DEFAULT_SIZE);
        assert_eq!(handle.decimals, DEFAULT_DECIMALS);
        assert_eq!(handle.frame.origin, Vector3::zero());
        assert_eq!(handle.frame.basis[0], Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(handle.frame.basis[1], Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(handle.frame.basis[2], Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(handle.shape.positions.len(), 2);
        assert!(handle.shape.normals.is_empty());
        assert!(handle.shape.triangles.is_empty());
        assert!(handle.shape.edges.is_empty());
        assert!(handle.program.is_none());
        assert!(handle.color.is_none());
        assert!(handle.readout.is_empty());
    }

    #[test]
    fn a_fully_populated_spec_round_trips_every_field() {
        let value = serde_json::json!({
            "id": "toy.bolt",
            "action": "bolt",
            "element": "Face3",
            "target": 9,
            "frame": {
                "origin": [1.0, 2.0, 3.0],
                "x": [2.0, 0.0, 0.0],
                "y": [0.0, 3.0, 0.0],
                "z": [0.0, 0.0, 4.0]
            },
            "axis": "X",
            "value": 12.5,
            "min": -5.0,
            "max": 50.0,
            "step": 0.5,
            "size": 64.0,
            "decimals": 3,
            "shape": {
                "positions": [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
                "normals": [[0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
                "triangles": [0, 1, 2],
                "edges": [0, 1, 1, 2, 2, 0]
            },
            "program": "@vertex fn vs_main() {}",
            "color": "#38bdf8",
            "readout": "H {value} mm"
        });

        let (target, handle) = handle_from_value(&value, 1).expect("a valid handle");

        assert_eq!(target, 9, "a spec target wins over the argument");
        assert_eq!(handle.id, "toy.bolt");
        assert_eq!(handle.action, "bolt");
        assert_eq!(handle.element.as_deref(), Some("Face3"));
        assert_eq!(handle.frame.origin, Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(handle.frame.basis[0], Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(handle.frame.basis[1], Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(handle.frame.basis[2], Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(handle.axis, 0);
        assert_close(handle.value, 12.5, "value");
        assert_eq!(handle.min, Some(-5.0));
        assert_eq!(handle.max, Some(50.0));
        assert_eq!(handle.step, Some(0.5));
        assert_eq!(handle.size, 64.0);
        assert_eq!(handle.decimals, 3);
        assert_eq!(handle.shape.positions.len(), 3);
        assert_eq!(handle.shape.normals.len(), 3);
        assert_eq!(handle.shape.triangles, vec![0, 1, 2]);
        assert_eq!(handle.shape.edges, vec![0, 1, 1, 2, 2, 0]);
        let program = handle.program.expect("a program");
        assert!(program.geometry(), "a skin is a geometry program");
        assert_eq!(handle.color, Some(parse_srgb_hex("#38bdf8").unwrap()));
        assert_eq!(handle.readout, "H {value} mm");
    }

    #[test]
    fn the_axis_name_is_case_insensitive_and_rejects_other_letters() {
        for (text, expected) in [("X", 0), ("y", 1), ("z", 2), ("Z", 2)] {
            let mut value = minimal();
            value["axis"] = serde_json::json!(text);
            let (_, handle) = handle_from_value(&value, 1).expect("a valid axis");
            assert_eq!(handle.axis, expected, "axis {text:?}");
        }

        let mut bad = minimal();
        bad["axis"] = serde_json::json!("w");
        let error = handle_from_value(&bad, 1).expect_err("an unknown axis");
        assert!(error.contains("axis"), "{error}");
        assert!(error.contains('w'), "{error}");
    }

    #[test]
    fn the_kind_is_case_insensitive_and_rejects_unknown_values() {
        for (text, expected) in [
            ("axis", DeclarativeKind::Axis),
            ("AXIS", DeclarativeKind::Axis),
            ("ring", DeclarativeKind::Ring),
            ("Ring", DeclarativeKind::Ring),
        ] {
            let mut value = minimal();
            value["kind"] = serde_json::json!(text);
            let (_, handle) = handle_from_value(&value, 1).expect("a valid kind");
            assert_eq!(handle.kind, expected, "kind {text:?}");
        }

        let (_, handle) = handle_from_value(&minimal(), 1).expect("the default kind");
        assert_eq!(handle.kind, DeclarativeKind::Axis, "the default is axis");
        assert_eq!(handle.radius, DEFAULT_RADIUS);

        let mut bad = minimal();
        bad["kind"] = serde_json::json!("arc");
        let error = handle_from_value(&bad, 1).expect_err("an unknown kind");
        assert!(error.contains("kind"), "{error}");
        assert!(error.contains("arc"), "{error}");
    }

    #[test]
    fn the_radius_must_be_positive_and_finite() {
        for bad in [0.0, -5.0] {
            let mut value = minimal();
            value["radius"] = serde_json::json!(bad);
            let error = handle_from_value(&value, 1).expect_err("a non-positive radius");
            assert!(error.contains("radius"), "{error}");
        }

        let spec = HandleSpec {
            id: "toy.bolt".to_string(),
            radius: Some(f64::INFINITY),
            shape: ShapeSpec {
                positions: vec![[0.0, 0.0, 0.0]],
                ..Default::default()
            },
            ..Default::default()
        };
        let error = convert(spec, 1).expect_err("a non-finite radius");
        assert!(error.contains("radius"), "{error}");
    }

    #[test]
    fn a_ring_spec_round_trips_kind_radius_value_step_and_readout() {
        let value = serde_json::json!({
            "id": "toy.ring",
            "axis": "z",
            "kind": "Ring",
            "radius": 80.0,
            "value": 45.0,
            "step": 15.0,
            "readout": "{value} deg",
            "shape": { "positions": [[0.0, 0.0, 0.0]] }
        });
        let (_, handle) = handle_from_value(&value, 1).expect("a valid ring");
        assert_eq!(handle.kind, DeclarativeKind::Ring);
        assert_eq!(handle.radius, 80.0);
        assert_eq!(handle.value, 45.0);
        assert_eq!(handle.step, Some(15.0));
        assert_eq!(handle.readout, "{value} deg");
    }

    #[test]
    fn a_non_unit_basis_is_normalised() {
        let mut value = minimal();
        value["frame"] = serde_json::json!({
            "origin": [0.0, 0.0, 0.0],
            "x": [0.0, 3.0, 0.0],
            "y": [0.0, 0.0, -5.0],
            "z": [2.0, 0.0, 0.0]
        });
        let (_, handle) = handle_from_value(&value, 1).expect("a valid frame");
        assert_eq!(handle.frame.basis[0], Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(handle.frame.basis[1], Vector3::new(0.0, 0.0, -1.0));
        assert_eq!(handle.frame.basis[2], Vector3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn a_zero_length_basis_vector_is_rejected() {
        let mut value = minimal();
        value["frame"] = serde_json::json!({ "y": [0.0, 0.0, 0.0] });
        let error = handle_from_value(&value, 1).expect_err("a zero-length basis");
        assert!(error.contains("frame.y"), "{error}");
        assert!(error.contains("zero-length"), "{error}");
    }

    #[test]
    fn an_empty_id_is_rejected() {
        let mut value = minimal();
        value["id"] = serde_json::json!("");
        let error = handle_from_value(&value, 1).expect_err("an empty id");
        assert!(error.contains("id"), "{error}");
    }

    #[test]
    fn positions_are_required() {
        let value = serde_json::json!({ "id": "toy.bolt", "shape": {} });
        let error = handle_from_value(&value, 1).expect_err("no positions");
        assert!(error.contains("positions"), "{error}");
    }

    #[test]
    fn normals_must_match_the_positions() {
        let value = serde_json::json!({
            "id": "toy.bolt",
            "shape": {
                "positions": [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                "normals": [[0.0, 0.0, 1.0]]
            }
        });
        let error = handle_from_value(&value, 1).expect_err("mismatched normals");
        assert!(error.contains("normals"), "{error}");
    }

    #[test]
    fn triangle_and_edge_indices_must_be_in_range() {
        let tri = serde_json::json!({
            "id": "toy.bolt",
            "shape": { "positions": [[0.0, 0.0, 0.0]], "triangles": [0, 1, 0] }
        });
        let error = handle_from_value(&tri, 1).expect_err("an out-of-range triangle");
        assert!(error.contains("triangles"), "{error}");
        assert!(error.contains('1'), "{error}");

        let edge = serde_json::json!({
            "id": "toy.bolt",
            "shape": { "positions": [[0.0, 0.0, 0.0]], "edges": [0, 4] }
        });
        let error = handle_from_value(&edge, 1).expect_err("an out-of-range edge");
        assert!(error.contains("edges"), "{error}");
        assert!(error.contains('4'), "{error}");
    }

    #[test]
    fn size_must_be_positive() {
        let mut value = minimal();
        value["size"] = serde_json::json!(0.0);
        let error = handle_from_value(&value, 1).expect_err("a zero size");
        assert!(error.contains("size"), "{error}");
    }

    #[test]
    fn the_step_must_not_be_negative_and_zero_is_continuous() {
        let mut value = minimal();
        value["step"] = serde_json::json!(-1.0);
        let error = handle_from_value(&value, 1).expect_err("a negative step");
        assert!(error.contains("step"), "{error}");

        let mut continuous = minimal();
        continuous["step"] = serde_json::json!(0.0);
        let (_, handle) = handle_from_value(&continuous, 1).expect("continuous travel");
        assert_eq!(handle.step, None);
    }

    #[test]
    fn min_must_not_exceed_max() {
        let mut value = minimal();
        value["min"] = serde_json::json!(2.0);
        value["max"] = serde_json::json!(1.0);
        let error = handle_from_value(&value, 1).expect_err("min > max");
        assert!(error.contains("min"), "{error}");
        assert!(error.contains("max"), "{error}");
    }

    #[test]
    fn every_number_must_be_finite() {
        // `serde_json` cannot carry NaN or infinity, so exercise the validator
        // directly with a value that could otherwise slip through.
        let spec = HandleSpec {
            id: "toy.bolt".to_string(),
            value: f64::INFINITY,
            shape: ShapeSpec {
                positions: vec![[0.0, 0.0, 0.0]],
                ..Default::default()
            },
            ..Default::default()
        };
        let error = convert(spec, 1).expect_err("a non-finite value");
        assert!(error.contains("value"), "{error}");
    }

    #[test]
    fn a_bad_colour_is_rejected() {
        let mut value = minimal();
        value["color"] = serde_json::json!("#zzzzzz");
        let error = handle_from_value(&value, 1).expect_err("a bad colour");
        assert!(error.contains("color"), "{error}");
    }

    #[test]
    fn the_colour_parses_to_linear_ink() {
        let mut value = minimal();
        value["color"] = serde_json::json!("#38bdf8");
        let (_, handle) = handle_from_value(&value, 1).expect("a valid colour");
        let ink = handle.color.expect("an ink override");
        assert_eq!(ink, parse_srgb_hex("#38bdf8").unwrap());
        assert_eq!(ink[3], 1.0, "a hex colour is opaque");
        // Linear, not the sRGB fraction: 0x38/255 is 0.22 sRGB but ~0.04 linear.
        assert!(ink[0] < 0.05, "{ink:?} is not linear");
        assert!(ink[2] > 0.9, "{ink:?} is not linear");
    }

    #[test]
    fn an_empty_program_means_no_skin() {
        for source in ["", "   ", "\n\t"] {
            let mut value = minimal();
            value["program"] = serde_json::json!(source);
            let (_, handle) = handle_from_value(&value, 1).expect("an absent skin");
            assert!(handle.program.is_none(), "program {source:?}");
        }
    }

    #[test]
    fn a_spec_target_defaults_to_the_argument_and_wins_when_present() {
        let (target, _) = handle_from_value(&minimal(), 7).expect("a default target");
        assert_eq!(target, 7);

        let mut value = minimal();
        value["target"] = serde_json::json!(12);
        let (target, _) = handle_from_value(&value, 7).expect("a declared target");
        assert_eq!(target, 12);
    }
}

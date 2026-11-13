//! The JS overlay channel: a page describes a program overlay as JSON.
//!
//! A JS workbench cannot build a `pivy.coin` tree — the page has no Coin facade — but
//! the one thing an overlay needs is a shader, a placeholder quad and the fixed
//! parameter block. This module turns that description into a
//! [`ferrocad_inventor::Scene`] and registers it under a stable name, so the shell's
//! existing pull bridge draws it exactly like a Coin overlay.
//!
//! ```json
//! {
//!   "name": "axis-overlay-js",
//!   "visible": true,
//!   "shader": "@vertex … @fragment …",
//!   "parameters": {
//!     "grid_color": [0.72, 0.30, 0.30],
//!     "axis_length": 80.0
//!   },
//!   "points": [[-1e-6, -1e-6, 0], [1e-6, -1e-6, 0],
//!              [1e-6, 1e-6, 0], [-1e-6, 1e-6, 0]]
//! }
//! ```
//!
//! `shader` is the whole WGSL module (both entry points in one file, like the Coin
//! mods). `vertex`/`fragment` are accepted instead when a page wants the Coin stage
//! split. `parameters` maps the fixed ABI's slot names onto a number (a `float`), a
//! three-number array (a `vec3`) or a longer number array (an `array1f`). An unknown
//! name is refused here, not silently ignored: the compiler is the authority, and
//! [`parse`] runs it once so a typo is reported to the page.
//!
//! The scene is a `Separator` holding the program and one four-corner `FaceSet`.
//! `SoShaderProgram` binds to the shapes that follow it, the compiler gives a program
//! packet no pick identity and no depth write, and the shader itself projects the
//! world origin — so the quad's corners only need the right *signs*. `visible: false`
//! is a removal, not an empty scene: a hidden overlay costs the viewport nothing.

use ferrocad_inventor::{
    compile_with_base, Node, NodeKind, Scene, ShaderParameter, ShaderStage, ShaderStageKind,
    Viewport,
};
use serde_json::{Map, Value};

/// The half extent of the default placeholder quad.
///
/// The axis shader takes `sign(position.xy)`, so any non-zero corners work; a
/// micrometre keeps the placeholder inside the origin despite the program replacing
/// its geometry anyway.
pub const DEFAULT_HALF_EXTENT: f32 = 1.0e-6;

/// What one overlay call asks the host to do.
#[derive(Clone, Debug)]
pub enum Spec {
    /// Drop every named overlay the page registered (`null`).
    RemoveAll,
    /// Drop one named overlay (`{"name": …, "visible": false}`).
    Remove(String),
    /// Register or replace a named overlay.
    Put { name: String, scene: Scene },
}

/// Parse one overlay description and validate the program it builds.
pub fn parse(text: &str) -> Result<Spec, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("overlay is not JSON: {error}"))?;
    parse_value(&value)
}

fn parse_value(value: &Value) -> Result<Spec, String> {
    if value.is_null() {
        return Ok(Spec::RemoveAll);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "an overlay must be an object or null".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "an overlay needs a non-empty `name`".to_string())?
        .to_string();
    let visible = match object.get("visible") {
        None => true,
        Some(Value::Bool(visible)) => *visible,
        Some(other) => {
            return Err(format!("`visible` must be a bool, not {other}"));
        }
    };
    if !visible {
        return Ok(Spec::Remove(name));
    }

    let source = shader_source(object)?;
    let parameters = parameters(object.get("parameters"))?;
    let points = points(object.get("points"))?;

    let vertex = Node {
        kind: NodeKind::ShaderStage(ShaderStage {
            kind: ShaderStageKind::Vertex,
            source: Some(source),
            source_program: None,
        }),
        children: parameters,
    };
    let fragment = Node {
        kind: NodeKind::ShaderStage(ShaderStage::fragment()),
        children: Vec::new(),
    };
    let program = Node {
        kind: NodeKind::ShaderProgram,
        children: vec![vertex, fragment],
    };
    let scene = Scene {
        root: Node {
            kind: NodeKind::Separator,
            children: vec![
                program,
                Node {
                    kind: NodeKind::Coordinate3(points.clone()),
                    children: Vec::new(),
                },
                Node {
                    kind: NodeKind::FaceSet {
                        num_vertices: vec![points.len() as u32],
                    },
                    children: Vec::new(),
                },
            ],
        },
    };

    // Validate now so the page gets the typo instead of a silent empty draw. The
    // compiler is the one table of ABI names; a second list here would drift.
    compile_with_base(&scene, &Viewport::default(), None)
        .map_err(|error| format!("overlay {name:?}: {error}"))?;
    Ok(Spec::Put { name, scene })
}

/// The WGSL the description carries.
///
/// `shader` is the one-module form every Coin mod uses. `vertex`/`fragment` are the
/// stage-split form; either may be empty, but at least one has to hold the module.
fn shader_source(object: &Map<String, Value>) -> Result<String, String> {
    if let Some(source) = object.get("shader") {
        let source = text(source, "shader")?;
        if source.trim().is_empty() {
            return Err("`shader` is empty".to_string());
        }
        return Ok(source);
    }
    let vertex = object
        .get("vertex")
        .map(|value| text(value, "vertex"))
        .transpose()?
        .unwrap_or_default();
    let fragment = object
        .get("fragment")
        .map(|value| text(value, "fragment"))
        .transpose()?
        .unwrap_or_default();
    if vertex.trim().is_empty() && fragment.trim().is_empty() {
        return Err("an overlay needs `shader`, or a non-empty `vertex`/`fragment`".to_string());
    }
    // The compiler concatenates stages in (vertex, fragment) order, so the split is
    // preserved by putting each half in its own stage.
    let mut source = String::new();
    if !vertex.trim().is_empty() {
        source.push_str(&vertex);
        if !fragment.trim().is_empty() {
            source.push('\n');
        }
    }
    if !fragment.trim().is_empty() {
        source.push_str(&fragment);
    }
    Ok(source)
}

/// The fixed ABI parameters, by name.
fn parameters(value: Option<&Value>) -> Result<Vec<Node>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let entries: Vec<(String, Value)> = match value {
        // The concise form: `{ "axis_length": 80 }`.
        Value::Object(map) => map
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        // The explicit form: `[{ "name": "axis_length", "value": 80 }]`.
        Value::Array(rows) => rows
            .iter()
            .map(|row| {
                let row = row
                    .as_object()
                    .ok_or_else(|| "a parameter row must be an object".to_string())?;
                let name = row
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "a parameter row needs a `name`".to_string())?;
                let value = row
                    .get("value")
                    .ok_or_else(|| format!("parameter {name:?} has no `value`"))?;
                Ok::<_, String>((name.to_string(), value.clone()))
            })
            .collect::<Result<_, _>>()?,
        other => {
            return Err(format!(
                "`parameters` must be an object or an array, not {other}"
            ))
        }
    };

    let mut nodes = Vec::with_capacity(entries.len());
    for (name, value) in entries {
        let parameter = match value {
            Value::Number(number) => ShaderParameter::Float {
                name,
                value: number
                    .as_f64()
                    .ok_or_else(|| "a parameter number is not finite".to_string())?
                    as f32,
            },
            Value::Array(values) => {
                let numbers = numbers(&values, &format!("parameter {name:?}"))?;
                if numbers.len() == 3 {
                    ShaderParameter::Vec3 {
                        name,
                        value: [numbers[0], numbers[1], numbers[2]],
                    }
                } else {
                    ShaderParameter::Floats {
                        name,
                        values: numbers,
                    }
                }
            }
            other => {
                return Err(format!(
                    "parameter {name:?} must be a number or a number array, not {other}"
                ));
            }
        };
        nodes.push(Node {
            kind: NodeKind::ShaderParameter(parameter),
            children: Vec::new(),
        });
    }
    Ok(nodes)
}

/// The placeholder quad's corners.
fn points(value: Option<&Value>) -> Result<Vec<[f32; 3]>, String> {
    let Some(value) = value else {
        return Ok(default_quad());
    };
    let rows = value
        .as_array()
        .ok_or_else(|| "`points` must be an array of [x, y, z]".to_string())?;
    let mut points = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let values = row
            .as_array()
            .ok_or_else(|| format!("point {index} is not an array"))?;
        let numbers = numbers(values, &format!("point {index}"))?;
        if numbers.len() != 3 {
            return Err(format!("point {index} needs three numbers"));
        }
        points.push([numbers[0], numbers[1], numbers[2]]);
    }
    if points.len() < 3 {
        return Err("an overlay needs at least three points".to_string());
    }
    Ok(points)
}

fn default_quad() -> Vec<[f32; 3]> {
    let h = DEFAULT_HALF_EXTENT;
    vec![[-h, -h, 0.0], [h, -h, 0.0], [h, h, 0.0], [-h, h, 0.0]]
}

fn numbers(values: &[Value], what: &str) -> Result<Vec<f32>, String> {
    values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|number| number as f32)
                .ok_or_else(|| format!("{what} contains a value that is not a number"))
        })
        .collect()
}

fn text(value: &Value, what: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("`{what}` must be a string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str = r#"
@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(sign(position.xy), 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
"#;

    fn spec(extra: Value) -> String {
        let mut object = serde_json::json!({
            "name": "axis",
            "shader": SHADER,
        });
        if let Value::Object(extra) = extra {
            for (key, value) in extra {
                object[key] = value;
            }
        }
        object.to_string()
    }

    #[test]
    fn a_description_becomes_a_program_overlay() {
        let Spec::Put { name, scene } = parse(&spec(serde_json::json!({}))).expect("valid") else {
            panic!("a visible overlay is a put");
        };
        assert_eq!(name, "axis");
        assert_eq!(scene.root.kind, NodeKind::Separator);
        assert_eq!(
            scene.root.children.len(),
            3,
            "program, coordinates, face set"
        );

        let (queue, _) = ferrocad_inventor::compile(&scene);
        assert_eq!(queue.len(), 1);
        let packet = &queue.packets[0];
        assert!(
            packet.state.program.is_some(),
            "the program bound to the quad"
        );
        assert!(!packet.state.pickable, "a program packet is never picked");
        assert!(
            !packet.state.depth.write,
            "a program packet does not write depth"
        );
    }

    #[test]
    fn parameters_map_onto_the_fixed_abi() {
        let spec = spec(serde_json::json!({
            "parameters": {
                "grid_color": [0.72, 0.30, 0.30],
                "axis_length": 80.0,
                "axis_origin": 1.0
            }
        }));
        let Spec::Put { scene, .. } = parse(&spec).expect("valid") else {
            panic!("a put");
        };
        let (queue, _) = ferrocad_inventor::compile(&scene);
        let program = queue.packets[0].state.program.as_ref().expect("program");
        let parameters = program.parameters();
        assert_eq!(parameters.color, [0.72, 0.30, 0.30]);
        assert_eq!(parameters.axis_length, 80.0);
        assert_eq!(parameters.axis_origin, 1.0);
    }

    #[test]
    fn an_unknown_parameter_is_refused_by_name() {
        let spec = spec(serde_json::json!({ "parameters": { "axis_lenght": 80.0 } }));
        let error = parse(&spec).expect_err("a typo is an error");
        assert!(error.contains("axis_lenght"), "{error}");
    }

    #[test]
    fn hidden_removes_and_null_removes_everything() {
        let hidden = spec(serde_json::json!({ "visible": false }));
        assert!(matches!(parse(&hidden).unwrap(), Spec::Remove(name) if name == "axis"));
        assert!(matches!(parse("null").unwrap(), Spec::RemoveAll));
    }

    #[test]
    fn a_missing_name_or_shader_is_reported() {
        let error = parse(r#"{"shader":"x"}"#).expect_err("no name");
        assert!(error.contains("name"), "{error}");

        let error = parse(r#"{"name":"axis"}"#).expect_err("no source");
        assert!(error.contains("shader"), "{error}");

        let error = parse(r#"{"name":"axis","shader":"   "}"#).expect_err("empty source");
        assert!(error.contains("empty"), "{error}");
    }

    #[test]
    fn the_vertex_fragment_split_is_concatenated() {
        let text = serde_json::json!({
            "name": "axis",
            "vertex": "@vertex fn v() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }",
            "fragment": "@fragment fn f() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }"
        });
        let Spec::Put { scene, .. } = parse(&text.to_string()).expect("valid") else {
            panic!("a put");
        };
        let (queue, _) = ferrocad_inventor::compile(&scene);
        assert_eq!(queue.len(), 1);
    }
}

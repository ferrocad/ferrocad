//! Node fields as data.
//!
//! A workbench does not assign whole lists to a node — it mutates fields in place
//! (`coords.point[0] = (x, y, z)`, `faces.numVertices.setValue(4)`), so a Python facade
//! has to answer a *live* field object over the node's buffer. Which buffer a name
//! addresses, and the strings Coin's `SoFieldContainer::set` accepts, are the same
//! rules on every runtime; they live here so the facades differ only in how they adapt
//! a Python value to and from these buffers.
//!
//! Two facades interpret this table today: `ferrocad_inventor_py` (PyO3, the
//! `pivy.coin` module) and `ferrocad_shell_pywasm`'s `coin` module (RustPython, the
//! browser and the desktop preview).

use std::fmt;

use crate::node::{
    Node, NodeKind, ShaderParameter, class_name, depth_function, light_model, material_binding,
    pick_style,
};

/// Which buffer (or child list) of a [`NodeKind`] a field addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    /// `SoCoordinate3.point` — `SoMFVec3f`.
    Point,
    /// `SoNormal.vector` — `SoMFVec3f`.
    Vector,
    /// `SoMaterial.diffuseColor` — `SoMFColor`.
    DiffuseColor,
    /// `SoIndexedShape.coordIndex` — `SoMFInt32`.
    CoordIndex,
    /// `SoIndexedFaceSet.normalIndex` — `SoMFInt32`.
    NormalIndex,
    /// `SoFaceSet.numVertices` — `SoMFInt32`.
    NumVertices,
    /// `SoShaderParameterArray1f.value` — `SoMFFloat`.
    ParameterValues,
    /// `SoShaderProgram.shaderObject` — `SoMFNode` (the program's children).
    ShaderObject,
    /// A shader stage's `parameter` — `SoMFNode` (the stage's children).
    Parameter,
}

impl FieldId {
    /// The field a node of this kind exposes under `name`, if any.
    ///
    /// `None` is the honest answer for a field this slice does not model
    /// (`SoMaterial.transparency` on our `SoMaterial`, `SoFaceSet.startIndex`), so a
    /// facade raises `AttributeError` there instead of inventing a buffer.
    pub fn from_name(kind: &NodeKind, name: &str) -> Option<Self> {
        match (kind, name) {
            (NodeKind::Coordinate3(_), "point") => Some(Self::Point),
            (NodeKind::Normal(_), "vector") => Some(Self::Vector),
            (NodeKind::Material(_), "diffuseColor") => Some(Self::DiffuseColor),
            (NodeKind::Mesh { .. }, "coordIndex") => Some(Self::CoordIndex),
            (NodeKind::Mesh { .. }, "normalIndex") => Some(Self::NormalIndex),
            (NodeKind::FaceSet { .. }, "numVertices") => Some(Self::NumVertices),
            (
                NodeKind::ShaderParameter(ShaderParameter::Floats { .. }),
                "value",
            ) => Some(Self::ParameterValues),
            (NodeKind::ShaderProgram, "shaderObject") => Some(Self::ShaderObject),
            (NodeKind::ShaderStage(_), "parameter") => Some(Self::Parameter),
            _ => None,
        }
    }

    /// The Coin field class name, for the facade's field object and its errors.
    pub fn label(self) -> &'static str {
        match self {
            Self::Point | Self::Vector => "SoMFVec3f",
            Self::DiffuseColor => "SoMFColor",
            Self::CoordIndex | Self::NormalIndex | Self::NumVertices => "SoMFInt32",
            Self::ParameterValues => "SoMFFloat",
            Self::ShaderObject | Self::Parameter => "SoMFNode",
        }
    }

    /// The attribute name this field is read as (`"point"`, `"coordIndex"`, …).
    pub fn name(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Vector => "vector",
            Self::DiffuseColor => "diffuseColor",
            Self::CoordIndex => "coordIndex",
            Self::NormalIndex => "normalIndex",
            Self::NumVertices => "numVertices",
            Self::ParameterValues => "value",
            Self::ShaderObject => "shaderObject",
            Self::Parameter => "parameter",
        }
    }
}

// ---------------------------------------------------------------------------
// The buffers
// ---------------------------------------------------------------------------

/// `SoMFVec3f` positions (`SoCoordinate3.point`, `SoNormal.vector`).
pub fn vec3(node: &Node, field: FieldId) -> Option<&[[f32; 3]]> {
    match (&node.kind, field) {
        (NodeKind::Coordinate3(points), FieldId::Point) => Some(points),
        (NodeKind::Normal(normals), FieldId::Vector) => Some(normals),
        _ => None,
    }
}

/// The same buffer, writable (`coords.point[0] = ...` edits the node).
pub fn vec3_mut(node: &mut Node, field: FieldId) -> Option<&mut Vec<[f32; 3]>> {
    match (&mut node.kind, field) {
        (NodeKind::Coordinate3(points), FieldId::Point) => Some(points),
        (NodeKind::Normal(normals), FieldId::Vector) => Some(normals),
        _ => None,
    }
}

/// `SoMFColor` diffuse colours.
pub fn color(node: &Node, field: FieldId) -> Option<&[[f32; 4]]> {
    match (&node.kind, field) {
        (NodeKind::Material(colors), FieldId::DiffuseColor) => Some(colors),
        _ => None,
    }
}

/// The colour buffer, writable.
pub fn color_mut(node: &mut Node, field: FieldId) -> Option<&mut Vec<[f32; 4]>> {
    match (&mut node.kind, field) {
        (NodeKind::Material(colors), FieldId::DiffuseColor) => Some(colors),
        _ => None,
    }
}

/// `SoMFInt32` indices and counts.
pub fn int(node: &Node, field: FieldId) -> Option<&[u32]> {
    match (&node.kind, field) {
        (NodeKind::Mesh { indices, .. }, FieldId::CoordIndex) => Some(indices),
        (NodeKind::Mesh { normal_indices, .. }, FieldId::NormalIndex) => Some(normal_indices),
        (NodeKind::FaceSet { num_vertices }, FieldId::NumVertices) => Some(num_vertices),
        _ => None,
    }
}

/// The integer buffer, writable.
pub fn int_mut(node: &mut Node, field: FieldId) -> Option<&mut Vec<u32>> {
    match (&mut node.kind, field) {
        (NodeKind::Mesh { indices, .. }, FieldId::CoordIndex) => Some(indices),
        (NodeKind::Mesh { normal_indices, .. }, FieldId::NormalIndex) => Some(normal_indices),
        (NodeKind::FaceSet { num_vertices }, FieldId::NumVertices) => Some(num_vertices),
        _ => None,
    }
}

/// `SoMFFloat` shader parameter values.
pub fn float(node: &Node, field: FieldId) -> Option<&[f32]> {
    match (&node.kind, field) {
        (
            NodeKind::ShaderParameter(ShaderParameter::Floats { values, .. }),
            FieldId::ParameterValues,
        ) => Some(values),
        _ => None,
    }
}

/// The float buffer, writable.
pub fn float_mut(node: &mut Node, field: FieldId) -> Option<&mut Vec<f32>> {
    match (&mut node.kind, field) {
        (
            NodeKind::ShaderParameter(ShaderParameter::Floats { values, .. }),
            FieldId::ParameterValues,
        ) => Some(values),
        _ => None,
    }
}

/// `SoMFNode` child lists (a program's stages, a stage's parameters).
pub fn nodes(node: &Node, field: FieldId) -> Option<&[Node]> {
    match (&node.kind, field) {
        (NodeKind::ShaderProgram, FieldId::ShaderObject) => Some(&node.children),
        (NodeKind::ShaderStage(_), FieldId::Parameter) => Some(&node.children),
        _ => None,
    }
}

/// The child list, writable.
pub fn nodes_mut(node: &mut Node, field: FieldId) -> Option<&mut Vec<Node>> {
    match (&node.kind, field) {
        (NodeKind::ShaderProgram, FieldId::ShaderObject) => Some(&mut node.children),
        (NodeKind::ShaderStage(_), FieldId::Parameter) => Some(&mut node.children),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// SoFieldContainer.set("field VALUE")
// ---------------------------------------------------------------------------

/// Why `set("field VALUE")` failed.
///
/// A facade maps this onto its own exception (`ValueError` in both), so the message
/// carries the whole contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// The spec was neither `"field value"` nor `"field=value"`.
    Malformed {
        /// What the caller passed.
        spec: String,
    },
    /// The node has no such field.
    UnknownField {
        /// The node's Coin class.
        node: String,
        /// The field name as given.
        field: String,
    },
    /// The field exists but the value is not one it accepts.
    UnknownValue {
        /// The field name.
        field: String,
        /// The value as given.
        value: String,
    },
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { spec } => {
                write!(f, "set: expected \"field value\", got {spec:?}")
            }
            Self::UnknownField { node, field } => {
                write!(f, "{node}.set: unknown field {field:?}")
            }
            Self::UnknownValue { field, value } => {
                write!(f, "{field}: unexpected value {value:?}")
            }
        }
    }
}

impl std::error::Error for FieldError {}

/// `node.set("field VALUE")` (or `"field=VALUE"`) — set one field from a string, as
/// Coin's `SoFieldContainer::set` does.
///
/// Supported today: `SoDepthBuffer.test`/`write`/`function`, `SoLightModel.model`,
/// `SoPickStyle.style` and `SoMaterialBinding.value`. An unknown field or value is an
/// error naming it.
pub fn set_from_text(node: &mut Node, spec: &str) -> Result<(), FieldError> {
    let (field, value) = split_field_spec(spec)?;
    match &mut node.kind {
        NodeKind::DepthBuffer {
            test,
            write,
            function,
        } => match field {
            "test" => *test = parse_bool(field, value)?,
            "write" => *write = parse_bool(field, value)?,
            "function" => *function = parse_depth_function(field, value)?,
            _ => return Err(unknown_field(&node.kind, field)),
        },
        NodeKind::LightModel { model } => match field {
            "model" => *model = parse_light_model(field, value)?,
            _ => return Err(unknown_field(&node.kind, field)),
        },
        NodeKind::MaterialBinding { value: binding } => match field {
            "value" => *binding = parse_material_binding(field, value)?,
            _ => return Err(unknown_field(&node.kind, field)),
        },
        NodeKind::PickStyle { style } => match field {
            "style" => *style = parse_pick_style(field, value)?,
            _ => return Err(unknown_field(&node.kind, field)),
        },
        other => return Err(unknown_field(other, field)),
    }
    Ok(())
}

/// Split `"field value"` or `"field=value"` into its two trimmed halves.
pub fn split_field_spec(spec: &str) -> Result<(&str, &str), FieldError> {
    let trimmed = spec.trim();
    let (name, value) = match trimmed.split_once('=') {
        Some(pair) => pair,
        None => {
            let index = trimmed
                .find(char::is_whitespace)
                .ok_or_else(|| FieldError::Malformed {
                    spec: trimmed.to_string(),
                })?;
            (&trimmed[..index], &trimmed[index..])
        }
    };
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty() || value.is_empty() {
        return Err(FieldError::Malformed {
            spec: trimmed.to_string(),
        });
    }
    Ok((name, value))
}

fn unknown_field(kind: &NodeKind, field: &str) -> FieldError {
    FieldError::UnknownField {
        node: class_name(kind).to_string(),
        field: field.to_string(),
    }
}

/// Coin's `TRUE`/`FALSE` (and the numeric forms Coin also accepts).
pub fn parse_bool(field: &str, value: &str) -> Result<bool, FieldError> {
    match value.to_ascii_uppercase().as_str() {
        "TRUE" | "1" | "ON" => Ok(true),
        "FALSE" | "0" | "OFF" => Ok(false),
        _ => Err(unexpected(field, value, "TRUE or FALSE")),
    }
}

/// A `SoPickStyle::Style` name.
pub fn parse_pick_style(field: &str, value: &str) -> Result<i32, FieldError> {
    match value.to_ascii_uppercase().as_str() {
        "SHAPE" => Ok(pick_style::SHAPE),
        "BOUNDING_BOX" => Ok(pick_style::BOUNDING_BOX),
        "UNPICKABLE" => Ok(pick_style::UNPICKABLE),
        _ => Err(unexpected(field, value, "SHAPE, BOUNDING_BOX or UNPICKABLE")),
    }
}

/// A `SoLightModel::Model` name.
pub fn parse_light_model(field: &str, value: &str) -> Result<i32, FieldError> {
    match value.to_ascii_uppercase().as_str() {
        "BASE_COLOR" => Ok(light_model::BASE_COLOR),
        "PHONG" => Ok(light_model::PHONG),
        _ => Err(unexpected(field, value, "BASE_COLOR or PHONG")),
    }
}

/// A `SoDepthBuffer::DepthWriteFunction` name.
pub fn parse_depth_function(field: &str, value: &str) -> Result<i32, FieldError> {
    match value.to_ascii_uppercase().as_str() {
        "NEVER" => Ok(depth_function::NEVER),
        "LESS" => Ok(depth_function::LESS),
        "EQUAL" => Ok(depth_function::EQUAL),
        "LEQUAL" => Ok(depth_function::LEQUAL),
        "GREATER" => Ok(depth_function::GREATER),
        "NOTEQUAL" => Ok(depth_function::NOTEQUAL),
        "GEQUAL" => Ok(depth_function::GEQUAL),
        "ALWAYS" => Ok(depth_function::ALWAYS),
        _ => Err(unexpected(field, value, "a Coin depth function")),
    }
}

/// A `SoMaterialBinding::Binding` name.
pub fn parse_material_binding(field: &str, value: &str) -> Result<i32, FieldError> {
    match value.to_ascii_uppercase().as_str() {
        "OVERALL" => Ok(material_binding::OVERALL),
        "PER_PART" => Ok(material_binding::PER_PART),
        "PER_PART_INDEXED" => Ok(material_binding::PER_PART_INDEXED),
        "PER_FACE" => Ok(material_binding::PER_FACE),
        "PER_FACE_INDEXED" => Ok(material_binding::PER_FACE_INDEXED),
        "PER_VERTEX" => Ok(material_binding::PER_VERTEX),
        "PER_VERTEX_INDEXED" => Ok(material_binding::PER_VERTEX_INDEXED),
        _ => Err(unexpected(field, value, "a Coin material binding")),
    }
}

fn unexpected(field: &str, value: &str, expected: &str) -> FieldError {
    FieldError::UnknownValue {
        field: field.to_string(),
        value: format!("{value:?} (expected {expected})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_node() -> Node {
        Node {
            kind: NodeKind::Material(vec![[0.1, 0.2, 0.3, 1.0]]),
            children: Vec::new(),
        }
    }

    #[test]
    fn a_field_is_the_nodes_own_buffer() {
        let mut node = color_node();
        assert_eq!(
            FieldId::from_name(&node.kind, "diffuseColor"),
            Some(FieldId::DiffuseColor)
        );
        color_mut(&mut node, FieldId::DiffuseColor)
            .expect("the material buffer")
            .push([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(color(&node, FieldId::DiffuseColor).unwrap().len(), 2);
        // A field the kind does not have is absent, not empty.
        assert_eq!(FieldId::from_name(&node.kind, "transparency"), None);
        assert!(vec3(&node, FieldId::Point).is_none());
    }

    #[test]
    fn a_program_owns_its_stage_children() {
        let stage = Node {
            kind: NodeKind::ShaderStage(crate::node::ShaderStage::vertex()),
            children: Vec::new(),
        };
        let program = Node {
            kind: NodeKind::ShaderProgram,
            children: vec![stage],
        };
        assert_eq!(
            nodes(&program, FieldId::ShaderObject).map(<[Node]>::len),
            Some(1)
        );
        assert!(nodes(&program, FieldId::Parameter).is_none());
    }

    #[test]
    fn set_parses_the_fields_the_mods_use() {
        let mut node = Node {
            kind: NodeKind::DepthBuffer {
                test: true,
                write: true,
                function: depth_function::LESS,
            },
            children: Vec::new(),
        };
        set_from_text(&mut node, "write FALSE").expect("write FALSE");
        assert_eq!(
            node.kind,
            NodeKind::DepthBuffer {
                test: true,
                write: false,
                function: depth_function::LESS,
            }
        );
        set_from_text(&mut node, "function=ALWAYS").expect("function=ALWAYS");
        assert!(matches!(
            node.kind,
            NodeKind::DepthBuffer {
                function: depth_function::ALWAYS,
                ..
            }
        ));
        // An unknown field names the class and the field.
        let error = set_from_text(&mut node, "depth 4").expect_err("not a field");
        assert_eq!(
            error.to_string(),
            "SoDepthBuffer.set: unknown field \"depth\""
        );
        // A malformed spec has no field at all.
        assert!(set_from_text(&mut node, "write").is_err());
    }

    #[test]
    fn a_view_root_has_one_material_branch_per_representation() {
        let colors = [[0.5, 0.5, 0.5, 1.0]];
        let root = crate::node::view_root(&colors, 2);
        assert_eq!(root.kind, NodeKind::Separator);
        assert_eq!(root.children.len(), 2);
        let branch = &root.children[0];
        assert_eq!(class_name(&branch.children[0].kind), "SoMaterial");
        assert_eq!(
            branch.children[1].kind,
            NodeKind::MaterialBinding {
                value: material_binding::PER_PART
            }
        );
    }
}

//! Default properties for known FreeCAD object types.
//!
//! Upstream registers each object type's properties via `ADD_PROPERTY` in the
//! C++ constructor. For the POC we hard-code the properties of the test types
//! (and a few common document types) so `addObject("App::FeatureTest")`
//! produces an object that already has its expected properties.

use crate::geometry::{Matrix4, Placement, Vector3};
use crate::property::Property;
use crate::quantity::Quantity;
use crate::unit::Unit;

fn length() -> Property {
    Property::Quantity(Quantity::new(0.0, Unit::Millimeter))
}

fn angle() -> Property {
    Property::Quantity(Quantity::new(0.0, Unit::Millimeter))
}

/// Return the default property list for a type id (empty for unknown types).
pub fn default_properties(type_id: &str) -> Vec<(&'static str, Property)> {
    match type_id {
        "App::FeatureTest" => vec![
            ("Integer", Property::Integer(0)),
            ("Float", Property::Float(0.0)),
            ("Bool", Property::Bool(false)),
            ("BoolList", Property::BoolList(vec![])),
            ("String", Property::String(String::new())),
            ("StringList", Property::StringList(vec![])),
            ("Distance", length()),
            ("Angle", angle()),
            ("Enum", Property::String(String::new())),
            ("ConstraintInt", Property::IntegerList(vec![])),
            ("ConstraintFloat", Property::FloatList(vec![])),
            ("IntegerList", Property::IntegerList(vec![])),
            ("FloatList", Property::FloatList(vec![])),
            ("Link", Property::Link(String::new())),
            ("LinkList", Property::Link(String::new())),
            ("Matrix", Property::Matrix(Matrix4::identity())),
            ("Vector", Property::Vector(Vector3::zero())),
            ("VectorList", Property::VectorList(vec![])),
            ("Placement", Property::Placement(Placement::identity())),
            ("Source1", Property::Link(String::new())),
            ("Source2", Property::Link(String::new())),
            ("SourceN", Property::Link(String::new())),
            ("ExecResult", Property::String(String::new())),
            ("ExceptionType", Property::Integer(0)),
            ("ExecCount", Property::Integer(0)),
            ("TypeHidden", Property::Integer(0)),
            ("TypeReadOnly", Property::Integer(0)),
            ("TypeOutput", Property::Integer(0)),
            ("TypeAll", Property::Integer(0)),
            ("TypeTransient", Property::Integer(0)),
            ("TypeNoRecompute", Property::Integer(0)),
            ("QuantityLength", length()),
            ("QuantityOther", length()),
        ],
        "App::FeatureTestColumn" => vec![
            ("Column", Property::String(String::new())),
            ("Silent", Property::Bool(false)),
            ("Value", Property::Integer(0)),
        ],
        "App::FeatureTestRow" => vec![
            ("Row", Property::String(String::new())),
            ("Silent", Property::Bool(false)),
            ("Value", Property::Integer(0)),
        ],
        "App::FeatureTestAbsAddress" => vec![
            ("Address", Property::String(String::new())),
            ("Valid", Property::Bool(false)),
        ],
        "App::FeatureTestPlacement" => vec![
            ("Input1", Property::Placement(Placement::identity())),
            ("Input2", Property::Placement(Placement::identity())),
            ("MultLeft", Property::Placement(Placement::identity())),
            ("MultRight", Property::Placement(Placement::identity())),
        ],
        "App::FeatureTestAttribute" => vec![
            ("Object", Property::String(String::new())),
            ("Attribute", Property::String(String::new())),
        ],
        "App::DocumentObjectGroup" => vec![("Group", Property::LinkList(vec![]))],
        "App::Part" => vec![
            ("Group", Property::LinkList(vec![])),
            ("Placement", Property::Placement(Placement::identity())),
        ],
        _ => vec![],
    }
}

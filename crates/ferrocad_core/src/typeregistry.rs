//! Default properties for known FreeCAD object types.
//!
//! Upstream registers each object type's properties via `ADD_PROPERTY` in the
//! C++ constructor. For the POC we hard-code the properties of the test types
//! (and a few common document types) so `addObject("App::FeatureTest")`
//! produces an object that already has its expected properties, defaults and
//! status flags.

use crate::geometry::{Matrix4, Placement, Vector3};
use crate::property::{prop_status, Property};
use crate::quantity::Quantity;
use crate::unit::Unit;

fn quantity(value: f64) -> Property {
    Property::Quantity(Quantity::new(value, Unit::Millimeter))
}

/// `(group, documentation)` for a type's property, mirroring the group/doc
/// arguments upstream passes to `ADD_PROPERTY_TYPE` (defaults empty).
pub fn property_meta(type_id: &str, name: &str) -> (&'static str, &'static str) {
    if !type_id.starts_with("App::FeatureTest") {
        return ("", "");
    }
    const GROUP: &str = "Feature Test";
    let doc = match name {
        "Source1" | "Source2" | "SourceN" => "Source for testing links",
        "ExecResult" => "Result of the execution",
        "ExceptionType" => "The type of exception the execution method throws",
        "ExecCount" => "Number of executions",
        "TypeHidden" => "An example property which has the type 'Hidden'",
        "TypeReadOnly" => "An example property which has the type 'ReadOnly'",
        "TypeOutput" => "An example property which has the type 'Output'",
        "TypeTransient" => "An example property which has the type 'Transient'",
        "TypeNoRecompute" => "An example property which has the type 'NoRecompute'",
        "TypeAll" => "An example property which has the types 'Output', 'ReadOnly' and 'Hidden'",
        _ => "",
    };
    let in_group = matches!(
        name,
        "Source1"
            | "Source2"
            | "SourceN"
            | "ExecResult"
            | "ExceptionType"
            | "ExecCount"
            | "TypeHidden"
            | "TypeReadOnly"
            | "TypeOutput"
            | "TypeTransient"
            | "TypeNoRecompute"
            | "TypeAll"
    );
    (if in_group { GROUP } else { "" }, doc)
}

/// `(name, default, status)` for each of the type's properties.
pub fn default_properties(type_id: &str) -> Vec<(&'static str, Property, u32)> {
    use prop_status::{HIDDEN, NONE, NORECOMPUTE, OUTPUT, READONLY, TRANSIENT};
    match type_id {
        // Defaults and status flags mirror `src/App/FeatureTest.cpp` (the C++
        // fixture that `Mod/Test/Document.py` exercises).
        "App::FeatureTest" => vec![
            ("Integer", Property::Integer(4711), NONE),
            ("Float", Property::Float(47.11), NONE),
            ("Bool", Property::Bool(true), NONE),
            ("BoolList", Property::BoolList(vec![]), NONE),
            ("String", Property::String("4711".to_string()), NONE),
            ("StringList", Property::StringList(vec![]), NONE),
            ("Distance", quantity(47.11), NONE),
            ("Angle", quantity(3.0), NONE),
            (
                "Enum",
                Property::Enumeration(
                    ["Zero", "One", "Two", "Three", "Four"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    4,
                ),
                NONE,
            ),
            (
                "ConstraintInt",
                Property::IntegerConstraint {
                    value: 5,
                    min: 0,
                    max: 100,
                    step: 1,
                },
                NONE,
            ),
            (
                "ConstraintFloat",
                Property::FloatConstraint {
                    value: 5.0,
                    min: 0.0,
                    max: 100.0,
                    step: 1.0,
                },
                NONE,
            ),
            ("IntegerList", Property::IntegerList(vec![4711]), NONE),
            ("FloatList", Property::FloatList(vec![47.11]), NONE),
            ("Link", Property::Link(String::new()), NONE),
            ("LinkSub", Property::LinkSub(String::new(), Vec::new()), NONE),
            ("LinkList", Property::LinkList(vec![]), NONE),
            ("LinkSubList", Property::LinkList(vec![]), NONE),
            ("ColourList", Property::ColorList(vec![]), NONE),
            ("Matrix", Property::Matrix(Matrix4::identity()), NONE),
            ("Vector", Property::Vector(Vector3::new(1.0, 2.0, 3.0)), NONE),
            (
                "VectorList",
                Property::VectorList(vec![Vector3::new(3.0, 2.0, 1.0)]),
                NONE,
            ),
            ("Placement", Property::Placement(Placement::identity()), NONE),
            ("Source1", Property::Link(String::new()), NONE),
            ("Source2", Property::Link(String::new()), NONE),
            ("SourceN", Property::Link(String::new()), NONE),
            ("ExecResult", Property::String("empty".to_string()), NONE),
            ("ExceptionType", Property::Integer(0), NONE),
            ("ExecCount", Property::Integer(0), NONE),
            ("TypeHidden", Property::Integer(4711), HIDDEN),
            ("TypeReadOnly", Property::Integer(4711), READONLY),
            ("TypeOutput", Property::Integer(4711), OUTPUT),
            ("TypeTransient", Property::Integer(4711), TRANSIENT),
            ("TypeNoRecompute", Property::Integer(4711), NORECOMPUTE),
            ("TypeAll", Property::Integer(4711), OUTPUT | READONLY | HIDDEN),
            ("QuantityLength", quantity(1.0), NONE),
            ("QuantityOther", quantity(5.0), NONE),
        ],
        "App::FeatureTestColumn" => vec![
            ("Column", Property::String("A".to_string()), NONE),
            ("Silent", Property::Bool(false), NONE),
            ("Value", Property::Integer(0), OUTPUT),
        ],
        "App::FeatureTestRow" => vec![
            ("Row", Property::String("1".to_string()), NONE),
            ("Silent", Property::Bool(false), NONE),
            ("Value", Property::Integer(0), OUTPUT),
        ],
        "App::FeatureTestAbsAddress" => vec![
            ("Address", Property::String(String::new()), NONE),
            ("Valid", Property::Bool(false), OUTPUT | READONLY),
        ],
        "App::FeatureTestPlacement" => vec![
            ("Input1", Property::Placement(Placement::identity()), NONE),
            ("Input2", Property::Placement(Placement::identity()), NONE),
            ("MultLeft", Property::Placement(Placement::identity()), NONE),
            ("MultRight", Property::Placement(Placement::identity()), NONE),
        ],
        "App::FeatureTestAttribute" => vec![
            ("Object", Property::Link(String::new()), NONE),
            ("Attribute", Property::String(String::new()), NONE),
        ],
        "App::DocumentObjectGroup" => vec![("Group", Property::LinkList(vec![]), NONE)],
        "App::Part" => vec![
            ("Group", Property::LinkList(vec![]), NONE),
            ("Placement", Property::Placement(Placement::identity()), NONE),
        ],
        "App::DocumentObjectFileIncluded" => {
            vec![("File", Property::FileIncluded(String::new()), NONE)]
        }
        _ => vec![],
    }
}

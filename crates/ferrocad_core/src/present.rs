//! How a property is **shown and edited**.
//!
//! The property inspector exists twice — the native shell asks Python, the browser
//! backend reads [`Document`](crate::Document) directly — but the *rules* must not:
//! which control a value gets, how it reads, and what an editor string means are
//! properties of the [`Property`] itself, not of a host.
//!
//! This module is deliberately pure data-in/data-out. It has no window, no Python
//! and no kernel, so both hosts (and `ferrocad_py`, which forwards it to Python) can
//! call the same functions.

use crate::document::Color;
use crate::geometry::{Placement, Rotation, Vector3};
use crate::property::Property;
use crate::quantity::Quantity;

/// The control the property pane shows for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Text,
    Bool,
    Int,
    Float,
    Quantity,
    Enum,
    Color,
}

impl EditorKind {
    /// The kind's spelling in a `PropRow.kind` (the UI's own vocabulary).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Quantity => "quantity",
            Self::Enum => "enum",
            Self::Color => "color",
        }
    }

    /// Parse a kind, for a provider that declares one.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" | "string" => Some(Self::Text),
            "bool" | "boolean" => Some(Self::Bool),
            "int" | "integer" => Some(Self::Int),
            "float" => Some(Self::Float),
            "quantity" | "length" | "distance" | "angle" => Some(Self::Quantity),
            "enum" | "enumeration" => Some(Self::Enum),
            "color" | "colour" => Some(Self::Color),
            _ => None,
        }
    }
}

/// The editor a value gets, or `None` when the pane offers no control for it.
///
/// The kind comes from the **value**, never its name: a `Quantity` is a quantity
/// because it carries a unit, which is what keeps a Part box's `Length` and a
/// module's own float from drifting apart.
pub fn kind(value: &Property) -> Option<EditorKind> {
    match value {
        Property::String(_) | Property::FileIncluded(_) => Some(EditorKind::Text),
        Property::Bool(_) => Some(EditorKind::Bool),
        Property::Integer(_) | Property::IntegerConstraint { .. } => Some(EditorKind::Int),
        Property::Float(_) | Property::FloatConstraint { .. } => Some(EditorKind::Float),
        Property::Quantity(_) => Some(EditorKind::Quantity),
        Property::Enumeration(..) => Some(EditorKind::Enum),
        // Only the one-colour form has a single value a swatch and its hex field
        // can edit; a real per-face list stays read-only.
        Property::ColorList(values) if values.len() <= 1 => Some(EditorKind::Color),
        _ => None,
    }
}

/// The editor a registered property **type name** gets (`"App::PropertyLength"`).
///
/// It reads the type registry's own default value and asks [`kind`], so the two
/// spellings of the same rule cannot diverge.
pub fn kind_for_type(type_id: &str) -> Option<EditorKind> {
    crate::property_types::default_for(type_id)
        .as_ref()
        .and_then(kind)
}

/// A value as the inspector shows it.
pub fn format(value: &Property) -> String {
    match value {
        Property::String(value) => value.clone(),
        // Booleans read the way Python and FreeCAD print them.
        Property::Bool(value) => {
            if *value {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Property::Float(value) => number(*value),
        Property::Integer(value) => value.to_string(),
        Property::Quantity(value) => value.to_string(),
        Property::FloatConstraint { value, .. } => number(*value),
        Property::IntegerConstraint { value, .. } => value.to_string(),
        Property::Placement(placement) => format_placement(placement),
        Property::Vector(vector) => format!(
            "({}, {}, {})",
            number(vector.x),
            number(vector.y),
            number(vector.z)
        ),
        Property::Link(name) => name.clone(),
        Property::LinkSub(name, subs) => {
            if subs.is_empty() {
                name.clone()
            } else {
                format!("{name}: {}", subs.join(", "))
            }
        }
        Property::LinkList(names) => names.join(", "),
        Property::LinkSubList(links) => links
            .iter()
            .map(|(name, subs)| {
                if subs.is_empty() {
                    name.clone()
                } else {
                    format!("{name}: {}", subs.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
        Property::Enumeration(values, index) => values.get(*index).cloned().unwrap_or_default(),
        Property::StringList(values) => values.join(", "),
        Property::FloatList(values) => values
            .iter()
            .map(|value| number(*value))
            .collect::<Vec<_>>()
            .join(", "),
        Property::IntegerList(values) => values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        Property::BoolList(values) => values
            .iter()
            .map(|value| if *value { "True" } else { "False" })
            .collect::<Vec<_>>()
            .join(", "),
        Property::IntPairList(values) => values
            .iter()
            .map(|(a, b)| format!("({a}, {b})"))
            .collect::<Vec<_>>()
            .join(", "),
        // The swatch and the hex field ignore alpha; the value keeps it.
        Property::ColorList(values) => values
            .iter()
            .map(|color| hex(*color))
            .collect::<Vec<_>>()
            .join(", "),
        // A kernel shape holds an opaque payload; its bytes are not a value a person
        // reads. The type is in the row's type column already.
        Property::Extension(_) => "<shape>".to_string(),
        Property::PythonObject(_) => "<python object>".to_string(),
        Property::FileIncluded(path) => path.clone(),
        Property::Matrix(_) => "<matrix>".to_string(),
        Property::Rotation(_) => "<rotation>".to_string(),
        Property::RotationList(values) => format!("{} rotation(s)", values.len()),
        Property::PlacementList(values) => format!("{} placement(s)", values.len()),
        Property::VectorList(values) => format!("{} vector(s)", values.len()),
    }
}

/// Turn an editor string into the value a property of `current`'s shape expects.
///
/// The rules are the pane's, not a host's: an integer must be an integer, a
/// quantity parses its unit, and a kind with no editor refuses rather than storing
/// something the pane cannot show.
pub fn coerce(current: &Property, text: &str) -> Result<Property, String> {
    let text = text.trim();
    Ok(match current {
        Property::String(_) | Property::FileIncluded(_) => Property::String(text.to_string()),
        Property::Bool(_) => Property::Bool(matches!(
            text.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )),
        Property::Integer(_) => Property::Integer(
            text.parse()
                .map_err(|_| format!("{text:?} is not an integer"))?,
        ),
        Property::IntegerConstraint { min, max, step, .. } => Property::IntegerConstraint {
            value: text
                .parse::<i64>()
                .map_err(|_| format!("{text:?} is not an integer"))?
                .clamp(*min, *max),
            min: *min,
            max: *max,
            step: *step,
        },
        // A bare float accepts the unit a length was typed with (`25`, `25 mm`);
        // a quantity (below) parses the full expression.
        Property::Float(_) => Property::Float(parse_number(text)?),
        Property::FloatConstraint { min, max, step, .. } => Property::FloatConstraint {
            value: parse_number(text)?.clamp(*min, *max),
            min: *min,
            max: *max,
            step: *step,
        },
        Property::Quantity(_) => {
            Property::Quantity(Quantity::parse(text).map_err(|error| format!("{text:?}: {error}"))?)
        }
        Property::Enumeration(values, _) => {
            let index = values
                .iter()
                .position(|value| value == text)
                .ok_or_else(|| format!("{text:?} is not one of {values:?}"))?;
            Property::Enumeration(values.clone(), index)
        }
        // `#rgb`, `#rrggbb` or `#rrggbbaa`; without an alpha the value keeps the
        // current one, because the swatch and the field only show the colour.
        Property::ColorList(values) => {
            // A real per-face list has no single value to edit; refusing is better
            // than silently truncating it to one colour.
            if values.len() > 1 {
                return Err(format!("{} is not a single colour", current.type_name()));
            }
            let digits = text.strip_prefix('#').unwrap_or(text);
            let color =
                parse_color(text).ok_or_else(|| format!("{text:?} is not a colour (#rrggbb)"))?;
            let color = if digits.len() == 8 {
                color
            } else {
                let mut color = color;
                color[3] = values.first().map(|value| value[3]).unwrap_or(1.0);
                color
            };
            Property::ColorList(vec![color])
        }
        // A base-only string keeps the rotation the object already has, the way a colour
        // without an alpha keeps the current one. The long form replaces both.
        Property::Placement(current) => {
            let (base, rotation) = parse_placement(text)?;
            Property::Placement(Placement::new(base, rotation.unwrap_or(current.rotation)))
        }
        other => return Err(format!("{} is not editable", other.type_name())),
    })
}

/// A float as a person reads it: `10.0`, not `10`.
fn number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// A length as the editor types it: `20`, `20.5`, `20 mm`.
fn parse_number(text: &str) -> Result<f64, String> {
    let text = text.trim();
    let text = text
        .strip_suffix("mm")
        .or_else(|| text.strip_suffix("MM"))
        .unwrap_or(text)
        .trim();
    text.parse::<f64>()
        .map_err(|_| format!("{text:?} is not a number"))
}

/// `(x, y, z)`, or `(x, y, z; ax, ay, az; angle°)` when the rotation is not the identity.
///
/// The short form stays for an unrotated object: it is what the pane has always shown and
/// what an addon that only translates can read and write back unchanged. The angle is in
/// degrees because that is what FreeCAD's placement string means.
fn format_placement(placement: &Placement) -> String {
    if is_identity(&placement.rotation) {
        return format!(
            "({}, {}, {})",
            number(placement.base.x),
            number(placement.base.y),
            number(placement.base.z)
        );
    }
    let axis = placement.rotation.axis();
    format!(
        "({}, {}, {}; {}, {}, {}; {})",
        number(placement.base.x),
        number(placement.base.y),
        number(placement.base.z),
        number(axis.x),
        number(axis.y),
        number(axis.z),
        number(placement.rotation.angle().to_degrees())
    )
}

/// Whether `rotation` is the identity quaternion, either sign.
fn is_identity(rotation: &Rotation) -> bool {
    let [w, x, y, z] = rotation.q;
    (w.abs() - 1.0).abs() < 1e-12 && x.abs() < 1e-12 && y.abs() < 1e-12 && z.abs() < 1e-12
}

/// Parse the pane's placement form.
///
/// The rotation is axis-angle in degrees. Anything else is an error rather than a silent
/// translation-only placement, because dropping a rotation would move an object the user
/// did not ask to move.
fn parse_placement(text: &str) -> Result<(Vector3, Option<Rotation>), String> {
    let inner = text
        .trim()
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| not_a_placement(text))?;
    let mut parts = inner.split(';');
    let base = parse_triple(parts.next().unwrap_or_default(), text)?;
    let rotation = match (parts.next(), parts.next(), parts.next()) {
        (None, ..) => None,
        (Some(axis), Some(angle), None) => {
            let axis = parse_triple(axis, text)?;
            let degrees = parse_number(angle).map_err(|_| not_a_placement(text))?;
            Some(Rotation::from_axis_angle(&axis, degrees.to_radians()))
        }
        _ => return Err(not_a_placement(text)),
    };
    Ok((base, rotation))
}

/// Three comma-separated numbers, or an error naming the whole input.
fn parse_triple(part: &str, whole: &str) -> Result<Vector3, String> {
    let values: Vec<&str> = part.split(',').collect();
    if values.len() != 3 {
        return Err(not_a_placement(whole));
    }
    let mut numbers = [0.0; 3];
    for (slot, value) in numbers.iter_mut().zip(&values) {
        *slot = parse_number(value).map_err(|_| not_a_placement(whole))?;
    }
    Ok(Vector3::new(numbers[0], numbers[1], numbers[2]))
}

fn not_a_placement(text: &str) -> String {
    format!("{text:?} is not a placement: expected (x, y, z) or (x, y, z; ax, ay, az; angle)")
}

/// `#rrggbb` for a 0..1 RGBA colour (alpha is ignored, as in the pane).
fn hex(color: Color) -> String {
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color[0]),
        channel(color[1]),
        channel(color[2])
    )
}

/// Parse a `#rgb`, `#rrggbb` or `#rrggbbaa` editor string.
///
/// The alpha defaults to opaque; [`coerce`] restores the current alpha when the
/// text did not spell one, which is what keeps a colour edit from clearing it.
pub fn parse_color(text: &str) -> Option<Color> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    let digits = match digits.len() {
        3 => digits
            .chars()
            .flat_map(|digit| [digit, digit])
            .collect::<String>(),
        6 | 8 => digits.to_string(),
        _ => return None,
    };
    let channel = |index: usize| {
        u8::from_str_radix(&digits[index..index + 2], 16).map(|byte| f64::from(byte) / 255.0)
    };
    let alpha = if digits.len() == 8 {
        channel(6).ok()?
    } else {
        1.0
    };
    Some([channel(0).ok()?, channel(2).ok()?, channel(4).ok()?, alpha])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Placement, Rotation, Vector3};
    use crate::unit::Unit;

    fn mm(value: f64) -> Property {
        Property::Quantity(Quantity::new(value, Unit::Millimeter))
    }

    #[test]
    fn the_kind_comes_from_the_value_not_its_name() {
        assert_eq!(kind(&mm(10.0)), Some(EditorKind::Quantity));
        assert_eq!(kind(&Property::Float(10.0)), Some(EditorKind::Float));
        assert_eq!(
            kind(&Property::Enumeration(vec!["a".into(), "b".into()], 0)),
            Some(EditorKind::Enum)
        );
        assert_eq!(
            kind(&Property::ColorList(vec![[1.0, 0.5, 0.0, 1.0]])),
            Some(EditorKind::Color)
        );
        assert_eq!(
            kind(&Property::ColorList(vec![
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0]
            ])),
            None,
            "a per-face list has no single editable value"
        );
        assert_eq!(kind(&Property::PythonObject(String::new())), None);
    }

    /// The registered type names resolve through the same rule.
    #[test]
    fn a_type_name_gets_the_kind_its_default_value_has() {
        assert_eq!(
            kind_for_type("App::PropertyLength"),
            Some(EditorKind::Quantity)
        );
        assert_eq!(kind_for_type("App::PropertyFloat"), Some(EditorKind::Float));
        assert_eq!(kind_for_type("App::PropertyBool"), Some(EditorKind::Bool));
        assert_eq!(kind_for_type("App::PartShape"), None);
    }

    #[test]
    fn a_quantity_formats_with_its_unit_and_parses_back() {
        assert_eq!(format(&mm(10.0)), "10 mm");
        let edited = coerce(&mm(10.0), "25 mm").unwrap();
        assert_eq!(edited, mm(25.0));
        assert!(coerce(&mm(10.0), "nope").is_err());
    }

    #[test]
    fn an_integer_refuses_a_fraction_but_a_float_accepts_it() {
        assert!(coerce(&Property::Integer(1), "3.7").is_err());
        assert_eq!(
            coerce(&Property::Float(1.0), "3.7").unwrap(),
            Property::Float(3.7)
        );
        // A length typed with its unit still lands in a plain float.
        assert_eq!(
            coerce(&Property::Float(1.0), "25 mm").unwrap(),
            Property::Float(25.0)
        );
    }

    #[test]
    fn a_constraint_clamps_the_edit() {
        let constrained = Property::FloatConstraint {
            value: 1.0,
            min: 0.0,
            max: 2.0,
            step: 0.5,
        };
        let Property::FloatConstraint { value, .. } = coerce(&constrained, "9").unwrap() else {
            panic!("still a constraint");
        };
        assert_eq!(value, 2.0);
    }

    #[test]
    fn a_colour_edit_keeps_its_alpha() {
        let current = Property::ColorList(vec![[0.0, 0.0, 0.0, 0.25]]);
        let Property::ColorList(edited) = coerce(&current, "#f00").unwrap() else {
            panic!("a colour list");
        };
        assert_eq!(edited, vec![[1.0, 0.0, 0.0, 0.25]]);

        let Property::ColorList(edited) = coerce(&current, "#ff000080").unwrap() else {
            panic!("a colour list");
        };
        assert_eq!(edited[0][3], 128.0 / 255.0);
    }

    #[test]
    fn a_multi_colour_list_refuses_a_single_colour_edit() {
        let current = Property::ColorList(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]]);
        assert!(
            coerce(&current, "#123456")
                .unwrap_err()
                .contains("not a single colour")
        );
    }

    #[test]
    fn a_placement_reads_back_what_it_was_given() {
        // The identity keeps the short form: the pane's usual row, and what an addon that
        // only translates reads and writes back unchanged.
        let flat = Property::Placement(Placement::new(
            Vector3::new(1.0, 2.0, 3.0),
            Rotation::identity(),
        ));
        assert_eq!(format(&flat), "(1.0, 2.0, 3.0)");

        // A rotation makes the long form, and the long form parses back.
        let turned = Placement::new(
            Vector3::new(1.0, 2.0, 3.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        let property = Property::Placement(turned);
        assert_eq!(format(&property), "(1.0, 2.0, 3.0; 0.0, 0.0, 1.0; 90.0)");

        let Property::Placement(back) =
            coerce(&property, &format(&property)).expect("the formatted placement parses")
        else {
            panic!("a placement stays a placement");
        };
        assert!(back.is_same(&turned, 1e-9), "{back:?} vs {turned:?}");
    }

    #[test]
    fn a_base_only_placement_keeps_the_rotation() {
        // Like a colour without an alpha: the text the pane shows for a rotated object is
        // the long form, so a short one means "translate only", not "clear the rotation".
        let turned = Placement::new(
            Vector3::new(0.0, 0.0, 0.0),
            Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 0.5),
        );
        let Property::Placement(moved) = coerce(&Property::Placement(turned), "(5, 6, 7)")
            .expect("a placement")
        else {
            panic!("a placement stays a placement");
        };
        assert_eq!(moved.base, Vector3::new(5.0, 6.0, 7.0));
        assert_eq!(moved.rotation, turned.rotation);
    }

    #[test]
    fn a_malformed_placement_is_an_error() {
        let current = Property::Placement(Placement::identity());
        for text in [
            "",
            "1, 2, 3",
            "(1, 2)",
            "(1, 2, 3; 0, 0, 1)",
            "(1, 2, 3; 0, 0, 1; 90; 4)",
            "(a, b, c)",
        ] {
            assert!(coerce(&current, text).is_err(), "{text}");
        }
        // The message says which forms are accepted.
        let error = coerce(&current, "nope").unwrap_err();
        assert!(error.contains("(x, y, z)"), "{error}");
    }

    #[test]
    fn display_matches_the_native_pane_for_the_common_kinds() {
        assert_eq!(format(&Property::Bool(true)), "True");
        assert_eq!(format(&Property::Float(80.0)), "80.0");
        assert_eq!(format(&Property::Integer(3)), "3");
        assert_eq!(
            format(&Property::Placement(Placement::new(
                Vector3::new(1.0, 2.0, 3.0),
                crate::geometry::Rotation::identity()
            ))),
            "(1.0, 2.0, 3.0)"
        );
        assert_eq!(
            format(&Property::LinkSub("Box".into(), vec!["Edge1".into()])),
            "Box: Edge1"
        );
        assert_eq!(
            format(&Property::ColorList(vec![[
                0x42 as f64 / 255.0,
                0x8c as f64 / 255.0,
                0xd9 as f64 / 255.0,
                1.0
            ]])),
            "#428cd9"
        );
    }
}

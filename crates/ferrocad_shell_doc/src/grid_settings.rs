//! The synthetic `Settings/grid` card the document half owns.
//!
//! The grid is not a document object and its settings are not document properties:
//! they are the shell's presentation state ([`ViewState`]). A build whose script half
//! registers a `grid` provider gets the card from its Gui app (the provider supplies
//! the schema and the parameter store holds the values, see `mods/GridOverlay`). Every
//! other build — `?runtime=plain`, `?runtime=js`, `--script-host=plain`, and a desktop
//! whose mod set has no grid mod — had no `Settings/grid` node at all, so the grid
//! could be toggled and never configured.
//!
//! This module is that card, derived from the one store the document half already
//! mirrors: `AppBackend::set_view_state` is called at boot and after every applied
//! request, so `DocumentBackend` always holds the shell's current [`ViewState`].
//! Nothing is duplicated and nothing is persisted here, which also means the rows
//! and what the viewport draws cannot disagree.
//!
//! A write returns the [`ViewRequest`] the shell applies (`SetResult::view`), and
//! the shell pushes the resulting state back — the same round trip a
//! `FreeCADGui` command's `{"view": …}` takes. Validation is `ViewState::apply`'s,
//! not a second copy of it: a request this module accepts is one the shell accepts.

use ferrocad_core::{Property, Quantity, Unit, present};
use ferrocad_shell_api::settings::{SettingKind, SettingValue, SettingsBackend};
use ferrocad_shell_api::types::{DocumentNode, ObjectNode, PropRow, SetResult};
use ferrocad_shell_api::view::{ViewRequest, ViewState, srgb_hex};

/// The synthetic root, spelled exactly as the app script spells it.
///
/// The shell matches `Settings/...` by prefix (`open_settings_card`,
/// `settings_title`, the popover's chip, the selection filter), so the two halves
/// have to agree on the name rather than each carrying its own constant.
pub const SETTINGS_ROOT: &str = "Settings";

/// The one feature this half implements.
const GRID: &str = "grid";

/// The FreeCAD parameter group the grid settings live in, without the
/// `User parameter:BaseApp/Preferences/` prefix.
///
/// The desktop adapter prepends that prefix; the browser store uses the same string
/// as its localStorage namespace. `FerroCAD.Grid`'s own PREFERENCES path is exactly
/// this group, so a native card edit and a `GridOverlay_Settings` edit touch one store.
pub const GROUP: &str = "Mods/grid/ferrocad";

/// The node's type id: the synthetic type the app script's root uses, so the tree
/// row and the property pane treat both halves identically.
const SETTINGS_TYPE: &str = "FerroCAD::Settings";

/// The chip on the spacing rows while the step follows the layer's extent.
/// The drawn step is then derived, so these two rows are the fallback rather than
/// what is on screen — the same chip the script's card shows.
const ADAPTIVE: &str = "adaptive";
const PROVIDER: &str = "provider";

/// The `Settings` root the document half offers when no script half lists one.
///
/// It holds exactly one child: the grid. The axis cross and the gizmo keep their
/// script halves for now (their schemas are the provider registry's, `mods/
/// AxisOverlay` and `mods/GizmoOverlay`), so they are not listed here — an entry
/// with no rows would be a dead card.
pub fn root_node() -> DocumentNode {
    DocumentNode {
        name: SETTINGS_ROOT.to_string(),
        label: SETTINGS_ROOT.to_string(),
        active: false,
        objects: vec![grid_node()],
    }
}

/// The grid's child node, for a root a script half already listed.
pub fn grid_node() -> ObjectNode {
    ObjectNode {
        name: node_name(),
        label: "Grid".to_string(),
        type_id: SETTINGS_TYPE.to_string(),
    }
}

/// Add the grid to a script half's settings nodes, unless it listed one.
///
/// The namespace is **per feature**, because ownership is: a script that lists
/// `Settings/axis` and not `Settings/grid` has no rows for the grid (its registry
/// returned none), so it owns the axis and nothing else — its root gains the grid
/// child rather than the whole card disappearing because another mod was loaded.
/// A script that lists no `Settings` node at all gets [`root_node`].
pub fn merge(nodes: &mut Vec<DocumentNode>) {
    if lists_grid(nodes) {
        return;
    }
    match nodes.iter().position(|node| node.name == SETTINGS_ROOT) {
        Some(index) => nodes[index].objects.push(grid_node()),
        None => nodes.push(root_node()),
    }
}

/// Whether the script's nodes already list the grid — as a root child or, for a
/// host that lists features at the top level, as the node itself.
fn lists_grid(nodes: &[DocumentNode]) -> bool {
    nodes.iter().any(|node| {
        is_grid_node(&node.name) || node.objects.iter().any(|object| is_grid_node(&object.name))
    })
}

/// Whether `object` is the grid's settings node (`Settings/grid`, either case).
///
/// The shell and the Python resolver both accept the registered id and the
/// title-cased spelling, so this half does too rather than refusing a name the
/// tree itself shows.
pub fn is_grid_node(object: &str) -> bool {
    object
        .strip_prefix("Settings/")
        .is_some_and(|segment| segment.eq_ignore_ascii_case(GRID))
}

/// Whether `object` is in the synthetic namespace this half answers.
pub fn owns(object: &str) -> bool {
    object == SETTINGS_ROOT || object.starts_with("Settings/")
}

/// The rows of `object`, or `None` when the namespace is not this half's.
///
/// The root itself has no rows (it is a fold-out in the tree), and a feature this
/// half does not implement is empty rather than absent — the node exists in the
/// namespace, it just has nothing to offer, which is what the script's card does
/// for an unknown path too.
pub fn rows(object: &str, view: &ViewState) -> Option<Vec<PropRow>> {
    if !owns(object) {
        return None;
    }
    if !is_grid_node(object) {
        return Some(Vec::new());
    }
    let spacing = if view.grid_adaptive {
        ADAPTIVE
    } else {
        PROVIDER
    };
    Some(vec![
        row(
            "enabled",
            "Show grid",
            "App::PropertyBool",
            boolean(view.grid),
            "",
            "bool",
        ),
        row(
            "color",
            "Grid colour",
            "App::PropertyString",
            srgb_hex(view.grid_color),
            "",
            "color",
        ),
        bounded("opacity", "Opacity", view.grid_alpha.into(), 0.0, 1.0),
        row(
            "adaptive",
            "Adaptive spacing",
            "App::PropertyBool",
            boolean(view.grid_adaptive),
            "",
            "bool",
        ),
        choices(
            "minor_style",
            "Minor grid style",
            view.grid_minor_style.as_str(),
            &["dots", "lines"],
        ),
        bounded(
            "major_width",
            "Line width (px)",
            view.grid_major_width,
            1.0,
            8.0,
        ),
        bounded(
            "minor_width",
            "Minor line width (px)",
            view.grid_minor_width,
            1.0,
            8.0,
        ),
        quantity("minor", "Minor spacing", view.grid_minor, 0.001, spacing),
        quantity("major", "Major spacing", view.grid_major, 0.001, spacing),
        quantity(
            "radius",
            "Grid extent",
            view.grid_radius.unwrap_or(0.0),
            0.0,
            "",
        ),
    ])
}

/// The grid presentation request the host's store describes.
///
/// This is what `appearance_view` hands the shell at boot: the shell starts from
/// [`ViewState::default`] and applies the request, so stored values reach the window
/// instead of being overwritten by the shell's initial state.
pub fn request(settings: &dyn SettingsBackend) -> ViewRequest {
    let boolean = |name: &str| {
        settings
            .get(GROUP, name, SettingKind::Bool)
            .and_then(|value| value.as_bool())
    };
    let number = |name: &str| {
        settings
            .get(GROUP, name, SettingKind::Float)
            .and_then(|value| value.as_f64())
    };
    let text = |name: &str| {
        settings
            .get(GROUP, name, SettingKind::String)
            .and_then(|value| value.as_str().map(str::to_string))
    };

    ViewRequest {
        grid: boolean("enabled"),
        grid_color: text("color"),
        grid_alpha: number("opacity").map(|value| value as f32),
        grid_adaptive: boolean("adaptive"),
        grid_minor_style: text("minor_style"),
        grid_major_width: number("major_width"),
        grid_minor_width: number("minor_width"),
        grid_minor: number("minor"),
        grid_major: number("major"),
        // The card spells "camera-derived" as `0`; the presentation state spells it
        // as `None`, so a stored zero keeps the fallback rather than pinning an
        // empty disc.
        grid_radius: number("radius").filter(|radius| *radius > 0.0),
        ..ViewRequest::default()
    }
}

/// Hydrate a presentation state from the host's persisted grid settings.
///
/// Only values the store actually holds are requested; everything else keeps the
/// caller's defaults. Invalid stored values are ignored by [`ViewState::apply`], which
/// validates the whole request before mutating anything.
pub fn load(settings: &dyn SettingsBackend, view: &ViewState) -> ViewState {
    let request = request(settings);
    let mut hydrated = view.clone();
    let _ = hydrated.apply(&request);
    hydrated
}

/// Persist the grid settings the shell just applied.
///
/// The values and keys are exactly the card's rows and `FerroCAD.Grid`'s schema, so a
/// native edit, a `GridOverlay_Settings` edit and a Python `FerroCAD.Grid` write all
/// land in one place.
pub fn store(settings: &dyn SettingsBackend, view: &ViewState) -> Result<(), String> {
    for (name, value) in [
        ("enabled", SettingValue::Bool(view.grid)),
        ("color", SettingValue::String(srgb_hex(view.grid_color))),
        ("opacity", SettingValue::Float(f64::from(view.grid_alpha))),
        ("adaptive", SettingValue::Bool(view.grid_adaptive)),
        (
            "minor_style",
            SettingValue::String(view.grid_minor_style.as_str().to_string()),
        ),
        ("major_width", SettingValue::Float(view.grid_major_width)),
        ("minor_width", SettingValue::Float(view.grid_minor_width)),
        ("minor", SettingValue::Float(view.grid_minor)),
        ("major", SettingValue::Float(view.grid_major)),
        (
            "radius",
            SettingValue::Float(view.grid_radius.unwrap_or(0.0)),
        ),
    ] {
        settings.set(GROUP, name, value)?;
    }
    Ok(())
}

/// Write one row, or `None` when `object` is not this half's.
///
/// The result carries the presentation request the edit implies: the shell applies
/// it (`Shell::apply_view`) and mirrors the new state back, so a card edit repaints
/// the viewport in the same frame rather than at the next unrelated repaint.
pub fn write(object: &str, property: &str, value: &str, view: &ViewState) -> Option<SetResult> {
    if !owns(object) {
        return None;
    }
    if !is_grid_node(object) {
        return Some(refused(format!("no setting {property} on {object}")));
    }
    let request = match request_for(property, value, view) {
        Ok(request) => request,
        Err(error) => return Some(refused(error)),
    };
    // The shell owns the validator; running it here is what makes `ok` mean what it
    // says. `apply` is validation-first, so the probe is a pure check on a copy.
    let mut probe = view.clone();
    match probe.apply(&request) {
        Ok(_) => Some(SetResult {
            ok: true,
            error: String::new(),
            view: Some(request),
        }),
        Err(error) => Some(refused(error)),
    }
}

/// The request one row's text implies, with the pair-healing the script's card
/// applies (a minor step at or above the major one raises the major; a major step
/// equal to the minor one lowers the minor), so typing either row alone cannot
/// leave a pair `apply` refuses or a minor level with no rows of its own.
fn request_for(property: &str, text: &str, view: &ViewState) -> Result<ViewRequest, String> {
    let text = text.trim();
    Ok(match property {
        "enabled" => ViewRequest {
            grid: Some(boolean_text(text)),
            ..Default::default()
        },
        "color" => ViewRequest {
            grid_color: Some(ferrocad_stage::grid::parse_srgb_hex(text).map(srgb_hex)?),
            ..Default::default()
        },
        "opacity" => ViewRequest {
            grid_alpha: Some(number(text, "the grid opacity")? as f32),
            ..Default::default()
        },
        "adaptive" => ViewRequest {
            grid_adaptive: Some(boolean_text(text)),
            ..Default::default()
        },
        "minor_style" => ViewRequest {
            grid_minor_style: Some(text.to_ascii_lowercase()),
            ..Default::default()
        },
        "major_width" => ViewRequest {
            grid_major_width: Some(number(text, "the major line width")?),
            ..Default::default()
        },
        "minor_width" => ViewRequest {
            grid_minor_width: Some(number(text, "the minor line width")?),
            ..Default::default()
        },
        "minor" => {
            let minor = length(text, "the grid minor step")?;
            if minor <= 0.0 {
                return Err(format!(
                    "the grid minor step must be positive, got {text:?}"
                ));
            }
            // The levels are always a subdivision apart: a minor step at or above the
            // major one would leave the major level with no rows.
            let major = if minor >= view.grid_major {
                minor * 10.0
            } else {
                view.grid_major
            };
            ViewRequest {
                grid_minor: Some(minor),
                grid_major: Some(major),
                ..Default::default()
            }
        }
        "major" => {
            let major = length(text, "the grid major step")?;
            if major < view.grid_minor {
                return Err(format!(
                    "the grid major step ({major}) cannot be below the minor one ({})",
                    view.grid_minor
                ));
            }
            // Equal steps have no minor rows at all, so the minor drops a decade —
            // the inverse of what `minor` does to the pair.
            let minor = if (major - view.grid_minor).abs() < 1e-12 {
                major / 10.0
            } else {
                view.grid_minor
            };
            ViewRequest {
                grid_major: Some(major),
                grid_minor: Some(minor),
                ..Default::default()
            }
        }
        "radius" => {
            let radius = length(text, "the grid radius")?;
            if radius < 0.0 {
                return Err(format!(
                    "the grid radius must be zero (auto) or positive, got {text:?}"
                ));
            }
            ViewRequest {
                grid_radius: Some(radius),
                ..Default::default()
            }
        }
        other => return Err(format!("no setting {other}")),
    })
}

/// The node's name, for a command that selects it.
///
/// The shell matches `Settings/…` case-insensitively (`same_node`), because the app
/// script spells the feature's display name (`Settings/Grid`) and the tree spells the
/// registered id; this returns the tree's spelling, so the directive names exactly the
/// row the user can see.
pub fn node_name() -> String {
    format!("{SETTINGS_ROOT}/{GRID}")
}

fn refused(error: String) -> SetResult {
    SetResult {
        ok: false,
        error,
        view: None,
    }
}

/// A row with no bounds and no choices.
fn row(name: &str, label: &str, type_id: &str, value: String, status: &str, kind: &str) -> PropRow {
    PropRow {
        name: name.to_string(),
        label: label.to_string(),
        type_id: type_id.to_string(),
        value,
        status: status.to_string(),
        editable: true,
        command: None,
        kind: kind.to_string(),
        min: None,
        max: None,
        unit: None,
        choices: Vec::new(),
    }
}

/// A numeric row with an inclusive range (the pane offers its slider).
fn bounded(name: &str, label: &str, value: f64, min: f64, max: f64) -> PropRow {
    PropRow {
        min: Some(min),
        max: Some(max),
        ..row(
            name,
            label,
            "App::PropertyFloat",
            number_text(value),
            "",
            "float",
        )
    }
}

/// A length row: the number text carries its unit, and `unit` declares it, so the
/// editor shows `10 mm` and the slider reads the numeric prefix.
fn quantity(name: &str, label: &str, value: f64, min: f64, status: &str) -> PropRow {
    PropRow {
        min: Some(min),
        unit: Some("mm".to_string()),
        ..row(
            name,
            label,
            "App::PropertyQuantity",
            length_text(value),
            status,
            "quantity",
        )
    }
}

/// An enumerated row (a select box).
fn choices(name: &str, label: &str, value: &str, options: &[&str]) -> PropRow {
    PropRow {
        choices: options.iter().map(|option| option.to_string()).collect(),
        ..row(
            name,
            label,
            "App::PropertyEnumeration",
            value.to_string(),
            "",
            "enum",
        )
    }
}

/// `True`/`False`, the way the pane and the document print a boolean.
fn boolean(value: bool) -> String {
    if value {
        "True".to_string()
    } else {
        "False".to_string()
    }
}

/// A number without a trailing `.0` (`1`, not `1.0`).
fn number_text(value: f64) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A length as the pane's quantity text (`10 mm`).
fn length_text(value: f64) -> String {
    Quantity::new(value, Unit::Millimeter).user_string()
}

/// A boolean row's text, read with the pane's rule (`1`, `true`, `yes`, `on`).
fn boolean_text(text: &str) -> bool {
    matches!(
        present::coerce(&Property::Bool(false), text),
        Ok(Property::Bool(true))
    )
}

/// A finite number, never clamped: the shell clamps or refuses, and a refusal here
/// is the message the pane shows.
fn number(text: &str, name: &str) -> Result<f64, String> {
    let value = text
        .parse::<f64>()
        .map_err(|_| format!("{name} must be a number, got {text:?}"))?;
    if !value.is_finite() {
        return Err(format!("{name} must be finite, got {text:?}"));
    }
    Ok(value)
}

/// A length in millimetres.
///
/// A bare number means "that many millimetres" — the declared unit — and anything
/// else is parsed as the full unit expression (`"2 cm"`), then required to be a
/// length rather than, say, an angle.
fn length(text: &str, name: &str) -> Result<f64, String> {
    if let Ok(value) = text.parse::<f64>() {
        if !value.is_finite() {
            return Err(format!("{name} must be finite, got {text:?}"));
        }
        return Ok(value);
    }
    let quantity =
        Quantity::parse(text).map_err(|error| format!("{name} must be a length: {error}"))?;
    if quantity.sig() != Unit::Millimeter.sig {
        return Err(format!("{name} must be a length, got {text:?}"));
    }
    Ok(quantity.canonical_value())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(on: bool) -> ViewState {
        let mut view = ViewState::default();
        view.grid = on;
        view.grid_adaptive = false;
        view.grid_minor = 10.0;
        view.grid_major = 100.0;
        view
    }

    fn value(rows: &[PropRow], name: &str) -> Option<String> {
        rows.iter()
            .find(|row| row.name == name)
            .map(|row| row.value.clone())
    }

    #[test]
    fn the_root_lists_one_grid_node() {
        let root = root_node();
        assert_eq!(root.name, "Settings");
        assert!(!root.active, "the synthetic root is never a document");
        assert_eq!(root.objects.len(), 1);
        assert_eq!(root.objects[0].name, "Settings/grid");
        assert_eq!(root.objects[0].label, "Grid");
        assert_eq!(root.objects[0].type_id, SETTINGS_TYPE);
    }

    fn settings_root(features: &[&str]) -> DocumentNode {
        DocumentNode {
            name: SETTINGS_ROOT.to_string(),
            label: SETTINGS_ROOT.to_string(),
            active: false,
            objects: features
                .iter()
                .map(|feature| ObjectNode {
                    name: format!("{SETTINGS_ROOT}/{feature}"),
                    label: feature.to_string(),
                    type_id: SETTINGS_TYPE.to_string(),
                })
                .collect(),
        }
    }

    fn listed_grid(nodes: &[DocumentNode]) -> usize {
        nodes
            .iter()
            .flat_map(|node| &node.objects)
            .filter(|object| is_grid_node(&object.name))
            .count()
    }

    #[test]
    fn a_script_with_no_settings_gets_the_whole_card() {
        let mut nodes = Vec::new();
        merge(&mut nodes);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, SETTINGS_ROOT);
        assert_eq!(listed_grid(&nodes), 1);
    }

    #[test]
    fn another_features_settings_gain_the_grid_and_not_a_second_root() {
        let mut nodes = vec![settings_root(&["axis"])];
        merge(&mut nodes);
        assert_eq!(nodes.len(), 1, "one root: {nodes:?}");
        assert_eq!(listed_grid(&nodes), 1, "the grid joined it: {nodes:?}");
        assert!(
            nodes[0]
                .objects
                .iter()
                .any(|object| object.name == "Settings/axis"),
            "the script's own feature is untouched"
        );
    }

    #[test]
    fn a_script_that_lists_the_grid_keeps_it() {
        let mut nodes = vec![settings_root(&["axis", "grid"])];
        merge(&mut nodes);
        assert_eq!(listed_grid(&nodes), 1, "no duplicate: {nodes:?}");

        // A host that lists the feature node at the top level (the shape the script
        // host fakes use) is a listing too.
        let mut nodes = vec![DocumentNode {
            name: "Settings/Grid".to_string(),
            label: "Grid".to_string(),
            active: false,
            objects: Vec::new(),
        }];
        merge(&mut nodes);
        assert_eq!(nodes.len(), 1, "{nodes:?}");
        assert_eq!(listed_grid(&nodes), 0, "the node itself, not a child");
    }

    #[test]
    fn the_rows_are_the_presentation_state() {
        let view = grid(true);
        let card = rows("Settings/grid", &view).expect("the grid is this half's");
        let names: Vec<&str> = card.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "enabled",
                "color",
                "opacity",
                "adaptive",
                "minor_style",
                "major_width",
                "minor_width",
                "minor",
                "major",
                "radius",
            ],
            "the script's card order"
        );
        assert_eq!(value(&card, "enabled").as_deref(), Some("True"));
        assert_eq!(value(&card, "adaptive").as_deref(), Some("False"));
        assert_eq!(value(&card, "minor").as_deref(), Some("10 mm"));
        assert_eq!(value(&card, "major").as_deref(), Some("100 mm"));
        assert_eq!(
            value(&card, "radius").as_deref(),
            Some("0 mm"),
            "zero is the camera-derived extent"
        );
        assert_eq!(
            value(&card, "color").as_deref(),
            Some(srgb_hex(ViewState::default().grid_color).as_str()),
            "the ink is the canonical hex the swatch writes back"
        );
        assert!(
            card.iter().all(|row| row.editable),
            "every row the card shows is editable"
        );
    }

    #[test]
    fn the_spacing_rows_say_whether_they_are_what_is_drawn() {
        let view = grid(false);
        let mut adaptive = view.clone();
        adaptive.grid_adaptive = true;
        let card = rows("Settings/grid", &adaptive).unwrap();
        let minor = card.iter().find(|row| row.name == "minor").unwrap();
        assert_eq!(minor.status, ADAPTIVE);
        assert_eq!(minor.unit.as_deref(), Some("mm"));
        assert_eq!(minor.kind, "quantity");
        let card = rows("Settings/grid", &view).unwrap();
        let minor = card.iter().find(|row| row.name == "minor").unwrap();
        assert_eq!(minor.status, PROVIDER, "a fixed step is the provider's");
    }

    #[test]
    fn the_root_and_other_features_answer_with_no_rows() {
        let view = grid(false);
        let root = rows("Settings", &view).expect("the root is this half's");
        assert!(root.is_empty(), "a fold-out has no rows");
        let other = rows("Settings/axis", &view).expect("a node in the namespace");
        assert!(
            other.is_empty(),
            "a feature with no card is empty, not absent"
        );
        assert!(
            rows("Box", &view).is_none(),
            "a document object is not this half's"
        );
    }

    #[test]
    fn a_toggle_writes_the_presentation_request() {
        let view = grid(false);
        let result = write("Settings/grid", "enabled", "true", &view).unwrap();
        assert!(result.ok, "{result:?}");
        let request = result.view.expect("a settings edit asks for a repaint");
        assert_eq!(request.grid, Some(true));
    }

    #[test]
    fn a_step_write_heals_the_pair() {
        let view = grid(false);
        // At the major step: the major moves a decade up, as the script's card does.
        let result = write("Settings/grid", "minor", "100", &view).unwrap();
        assert!(result.ok, "{result:?}");
        let request = result.view.unwrap();
        assert_eq!(request.grid_minor, Some(100.0));
        assert_eq!(request.grid_major, Some(1000.0));

        // A major step equal to the minor one lowers the minor instead.
        let result = write("Settings/grid", "major", "10", &view).unwrap();
        assert!(result.ok, "{result:?}");
        let request = result.view.unwrap();
        assert_eq!(request.grid_major, Some(10.0));
        assert_eq!(request.grid_minor, Some(1.0));

        // Below the minor one there is nothing to heal: it is refused.
        let refused = write("Settings/grid", "major", "5", &view).unwrap();
        assert!(!refused.ok);
        assert!(
            refused.error.contains("cannot be below"),
            "{}",
            refused.error
        );
        assert!(refused.view.is_none(), "a refusal asks for no repaint");
    }

    #[test]
    fn a_length_accepts_the_typed_unit() {
        let view = grid(false);
        let result = write("Settings/grid", "radius", "2 cm", &view).unwrap();
        assert!(result.ok, "{result:?}");
        assert_eq!(result.view.unwrap().grid_radius, Some(20.0));

        let angle = write("Settings/grid", "radius", "45 deg", &view).unwrap();
        assert!(!angle.ok, "a length is not an angle");
        let zero = write("Settings/grid", "radius", "0 mm", &view).unwrap();
        assert!(zero.ok);
        assert_eq!(
            zero.view.unwrap().grid_radius,
            Some(0.0),
            "zero means auto; the shell maps it to `None`"
        );
    }

    #[test]
    fn an_unknown_row_is_refused_without_falling_through() {
        let view = grid(false);
        let refused = write("Settings/grid", "spacing", "5", &view).unwrap();
        assert!(!refused.ok, "`spacing` is an alias, not a row");
        assert!(refused.error.contains("no setting"));
        assert!(
            write("Box", "Length", "5", &view).is_none(),
            "not this half's"
        );
        let other = write("Settings/axis", "enabled", "true", &view).unwrap();
        assert!(!other.ok, "a feature with no rows cannot be written");
    }

    #[test]
    fn a_colour_is_written_in_the_canonical_spelling() {
        let view = grid(false);
        let result = write("Settings/grid", "color", "#ABC", &view).unwrap();
        assert!(result.ok, "{result:?}");
        let request = result.view.unwrap();
        assert_eq!(request.grid_color.as_deref(), Some("#aabbcc"));
        let bad = write("Settings/grid", "color", "sage", &view).unwrap();
        assert!(!bad.ok, "a name is not a colour");
    }

    #[test]
    fn stored_settings_hydrate_the_presentation_state() {
        let settings = ferrocad_shell_api::settings::MemorySettings::default();
        for (name, value) in [
            ("enabled", SettingValue::Bool(true)),
            ("color", SettingValue::String("#f87171".to_string())),
            ("opacity", SettingValue::Float(0.4)),
            ("adaptive", SettingValue::Bool(false)),
            ("minor_style", SettingValue::String("lines".to_string())),
            ("major_width", SettingValue::Float(2.0)),
            ("minor_width", SettingValue::Float(3.0)),
            ("minor", SettingValue::Float(5.0)),
            ("major", SettingValue::Float(50.0)),
            ("radius", SettingValue::Float(20.0)),
        ] {
            settings.set(GROUP, name, value).unwrap();
        }

        let view = load(&settings, &ViewState::default());
        assert!(view.grid);
        assert_eq!(srgb_hex(view.grid_color), "#f87171");
        assert!((view.grid_alpha - 0.4).abs() < 1e-6);
        assert!(!view.grid_adaptive);
        assert_eq!(view.grid_minor_style.as_str(), "lines");
        assert_eq!(view.grid_major_width, 2.0);
        assert_eq!(view.grid_minor_width, 3.0);
        assert_eq!(view.grid_minor, 5.0);
        assert_eq!(view.grid_major, 50.0);
        assert_eq!(view.grid_radius, Some(20.0));
    }

    #[test]
    fn an_empty_store_keeps_the_defaults_and_zero_radius_is_auto() {
        let settings = ferrocad_shell_api::settings::MemorySettings::default();
        let defaults = ViewState::default();
        let empty = load(&settings, &defaults);
        assert_eq!(empty, defaults);

        settings
            .set(GROUP, "radius", SettingValue::Float(0.0))
            .unwrap();
        let zero = load(&settings, &defaults);
        assert_eq!(zero.grid_radius, None, "zero means camera-derived");
    }

    #[test]
    fn the_presentation_state_is_stored_under_the_card_keys() {
        let settings = ferrocad_shell_api::settings::MemorySettings::default();
        let mut view = ViewState::default();
        view.grid = true;
        view.grid_minor = 20.0;
        view.grid_major = 200.0;
        view.grid_alpha = 0.5;
        store(&settings, &view).unwrap();

        assert_eq!(
            settings.get(GROUP, "enabled", SettingKind::Bool),
            Some(SettingValue::Bool(true))
        );
        assert_eq!(
            settings.get(GROUP, "minor", SettingKind::Float),
            Some(SettingValue::Float(20.0))
        );
        assert_eq!(
            settings.get(GROUP, "major", SettingKind::Float),
            Some(SettingValue::Float(200.0))
        );
        assert_eq!(
            settings.get(GROUP, "opacity", SettingKind::Float),
            Some(SettingValue::Float(0.5))
        );
        assert_eq!(
            settings.get(GROUP, "radius", SettingKind::Float),
            Some(SettingValue::Float(0.0))
        );
        assert_eq!(
            settings.get(GROUP, "color", SettingKind::String),
            Some(SettingValue::String(srgb_hex(view.grid_color)))
        );
    }
}

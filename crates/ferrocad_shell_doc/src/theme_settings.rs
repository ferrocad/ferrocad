//! The synthetic `Settings/Theme` card: the token table, read-only.
//!
//! A token is a compile-time constant ([`ferrocad_theme`]), and a stored setting wins
//! over it: `FerroCAD.Grid.color` and `FerroCAD.Axis.color_*` are the user's values,
//! with [`draw::GRID_INK`] and the axis tokens as the defaults behind them. So this
//! card cannot be a settings card in the usual sense — nothing here is persisted, and
//! nothing here can be edited. It exists because the tree is where a dev looks for
//! "what is `GRID_INK` actually, and which ink is that on screen?", and the answer used
//! to be a grep through the crates; `docs/developer/interaction/visual-language.md` is
//! the prose half of the same table.
//!
//! Every row is derived from the constant (`srgb_hex_from_linear`), never from a second
//! list of literals, so retuning a token retunes the card. The symbol comes from
//! `stringify!` for the same reason: a renamed constant cannot leave the row pointing at
//! the old name.
//!
//! A write is refused with the row that *is* the setting where one exists
//! ([`setting_for`]) — the card is the one place to see a token, the Settings tree is
//! the one place to change the ink.

use ferrocad_shell_api::types::{DocumentNode, ObjectNode, PropRow};
use ferrocad_theme::{draw, srgb_hex_from_linear};

use crate::grid_settings::{SETTINGS_ROOT, SETTINGS_TYPE};

/// The node's name in the `Settings` namespace.
pub const NODE: &str = "Settings/Theme";

/// The one list: the symbol, the ink it names, and the constant it is read from.
///
/// The labels are short on purpose — the card shows them as the row's chip, beside the
/// hex, so "grid + axis default" has to fit where "adaptive" or "provider" would.
macro_rules! tokens {
    ($($name:ident => $usage:expr,)*) => {
        &[$( (stringify!($name), $usage, draw::$name), )*]
    };
}

const TOKENS: &[(&str, &str, [f32; 4])] = tokens! {
    CANVAS_VOID => "canvas void",
    CANVAS_FAR => "canvas far",
    GLASS_SHADE => "body sides",
    GLASS_TOP => "body top",
    EDGE_FRONT => "front edges",
    EDGE_OCCLUDED => "occluded edges, ghosts",
    GRID_INK => "grid + axis default",
    AXIS_X => "X stem + badge",
    AXIS_Y => "Y stem + badge",
    AXIS_Z => "Z stem + badge",
    PREVIEW_INK => "ghost perimeter, preview",
    ADDITIVE_INK => "push/pull",
    SUBTRACTIVE_INK => "pocket, cut",
};

/// The theme's child node, for a root the tree already lists.
///
/// It is a `FerroCAD::Settings` like the grid's, so the tree row and the property pane
/// treat the two identically.
pub fn node() -> ObjectNode {
    ObjectNode {
        name: NODE.to_string(),
        label: "Theme".to_string(),
        type_id: SETTINGS_TYPE.to_string(),
    }
}

/// Whether `object` is this card (`Settings/Theme`, either case).
pub fn is_node(object: &str) -> bool {
    object
        .strip_prefix("Settings/")
        .is_some_and(|segment| segment.eq_ignore_ascii_case("theme"))
}

/// Add the theme to the `Settings` root, unless the tree listed it already.
///
/// [`crate::grid_settings::merge`] runs first, so a tree with no settings at all has the
/// root by now. A host that lists its features **at the top level** (`Settings/Grid`, no
/// `Settings` root — the fake host in the tests, and any host that takes that shape) has
/// no root to extend, and inventing one would split the namespace into two roots; that
/// is the case `grid_settings` declines too, so the card rides with the root form the
/// desktop and the browser shells use.
pub fn merge(nodes: &mut Vec<DocumentNode>) {
    let Some(root) = nodes.iter_mut().find(|node| node.name == SETTINGS_ROOT) else {
        return;
    };
    if root.objects.iter().any(|object| is_node(&object.name)) {
        return;
    }
    root.objects.push(node());
}

/// The card's rows, or `None` when `object` is not this node.
///
/// Read-only: the value is the token's sRGB hex (an edge token also shows its alpha),
/// and the chip says which ink it is. The renderer consumes the **linear** form, which
/// is what the token holds; the hex is the authored spelling, so one of them has to be
/// shown and the hex is the one a colour picker or a bug report can be compared against.
pub fn rows(object: &str) -> Option<Vec<PropRow>> {
    if !is_node(object) {
        return None;
    }
    Some(
        TOKENS
            .iter()
            .map(|(name, usage, linear)| token_row(name, usage, *linear))
            .collect(),
    )
}

/// The settings row that owns `token`, when one does.
///
/// A refusal is more useful with a destination: `GRID_INK` is not editable, but the ink
/// it defaults is `Settings/Grid.color`.
pub fn setting_for(token: &str) -> Option<&'static str> {
    match token {
        "GRID_INK" => Some("Settings/Grid.color"),
        "AXIS_X" => Some("Settings/Axis.color_x"),
        "AXIS_Y" => Some("Settings/Axis.color_y"),
        "AXIS_Z" => Some("Settings/Axis.color_z"),
        _ => None,
    }
}

fn token_row(name: &str, usage: &str, linear: [f32; 4]) -> PropRow {
    let hex = srgb_hex_from_linear(linear);
    let value = if linear[3] < 1.0 {
        format!("#{hex:06x} @{:.2}", linear[3])
    } else {
        format!("#{hex:06x}")
    };
    PropRow {
        name: name.to_string(),
        label: String::new(),
        type_id: "App::PropertyString".to_string(),
        value,
        status: usage.to_string(),
        editable: false,
        command: None,
        kind: "color".to_string(),
        min: None,
        max: None,
        unit: None,
        choices: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_theme::srgb_hex_from_linear;

    #[test]
    fn the_card_lists_every_draw_token_once() {
        let rows = rows(NODE).expect("the theme card");
        assert_eq!(rows.len(), TOKENS.len());
        let mut names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), rows.len(), "a symbol appears twice");

        for expected in [
            "GRID_INK",
            "AXIS_X",
            "AXIS_Y",
            "AXIS_Z",
            "PREVIEW_INK",
            "ADDITIVE_INK",
            "SUBTRACTIVE_INK",
            "GLASS_TOP",
            "GLASS_SHADE",
        ] {
            assert!(names.contains(&expected), "{expected} is on the card");
        }
    }

    #[test]
    fn every_row_is_read_only_and_shows_the_hex_its_token_was_authored_from() {
        for (row, (name, _, linear)) in rows(NODE).unwrap().iter().zip(TOKENS) {
            assert_eq!(row.name, *name);
            assert!(!row.editable, "a token is a constant, not a setting");
            assert!(row.command.is_none());
            assert!(
                row.value
                    .starts_with(&format!("#{:06x}", srgb_hex_from_linear(*linear))),
                "{} shows {}",
                row.name,
                row.value
            );
            assert!(!row.status.is_empty(), "{} says which ink it is", row.name);
            assert_eq!(row.kind, "color", "the row names a colour");
        }
    }

    #[test]
    fn an_edge_token_shows_its_alpha() {
        let rows = rows(NODE).unwrap();
        let front = rows.iter().find(|row| row.name == "EDGE_FRONT").unwrap();
        assert_eq!(front.value, "#d6e0f0 @0.62");
        let grid = rows.iter().find(|row| row.name == "GRID_INK").unwrap();
        assert_eq!(grid.value, "#94a3b8", "an opaque token carries no alpha");
    }

    #[test]
    fn only_this_node_answers_and_it_names_the_setting_that_owns_an_ink() {
        assert!(rows("Settings/Theme").is_some());
        assert!(
            rows("Settings/theme").is_some(),
            "the tree's spelling varies"
        );
        assert!(rows("Settings/Grid").is_none());
        assert!(rows("Theme").is_none());

        assert_eq!(setting_for("GRID_INK"), Some("Settings/Grid.color"));
        assert_eq!(setting_for("AXIS_Y"), Some("Settings/Axis.color_y"));
        assert_eq!(setting_for("GLASS_TOP"), None, "a body ink has no row");
    }
}

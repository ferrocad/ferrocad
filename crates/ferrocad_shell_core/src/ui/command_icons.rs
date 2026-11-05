//! The curated **command id → icon key** table.
//!
//! The iconography note (`docs/developer/interaction/iconography.md`) curates which
//! IconMCP glyph a command should carry. This module is that table in code: the shell
//! asks [`icon_for`] for a command's key and hands it to
//! [`ferrocad_widgets::icon::named`], which owns the vendored assets.
//!
//! The key is deliberately **not** a new command resource: upstream already ships a
//! `Pixmap` resource, and the note adopts it rather than inventing a parallel one. A
//! command id is the stable spelling both hosts already agree on (`Draft_Snap_Grid`,
//! `App_Move`), so the table is keyed by id. A command with no row — most workbench
//! commands, and every command a mod registers — simply keeps its label and no icon.
//!
//! Adding a row here is a two-step change: pick the key from the note's table, then
//! vendor it in `crates/ferrocad_widgets/assets/icons/`. A test below keeps the two in
//! step, so a typo cannot ship a blank slot.

use crate::commands::CommandInfo;

/// The vendored icon key for a command, or `None` when the note has not curated one.
pub(crate) fn icon_for(command_id: &str) -> Option<&'static str> {
    let key = match command_id {
        // Application chrome.
        "App_ToggleGrid" => "grid-4x4",
        "App_ToggleAxisCross" => "axes-3d",
        "App_ToggleGizmo" => "gizmo",
        "App_ViewWorkplane" => "workplane",
        "Dev_ToggleTools" => "bug",
        "App_Open" => "folder-open",
        "App_Save" => "save",
        // Object verbs.
        "App_Move" => "move",
        "App_Rotate" => "rotate-object",
        "App_Delete" => "trash",
        // The dock's creation verbs.
        "Toy_MakeSlab" => "cube",
        "Toy_Polyline" => "shape-polyline",
        "Part_Box" => "cube",
        "App_SaveDatumPlane" => "layers-plus",
        // Snap modes. The note draws the ones with no honest glyph (midpoint,
        // origin, vertex, extension, near) and they keep their label alone.
        "Draft_Snap_Lock" => "snap",
        "Draft_Snap_Grid" => "grid-4x4",
        "Draft_Snap_Endpoint" => "anchor-point",
        "Draft_Snap_Center" => "cursor-crosshair",
        "Draft_Snap_Intersection" => "intersection",
        "Draft_Snap_Perpendicular" => "perpendicular",
        "Draft_Snap_Parallel" => "parallel",
        "Draft_Snap_Angle" => "angle",
        "Draft_Snap_Ortho" => "angle-right",
        "Draft_Snap_Dimensions" => "ruler",
        "Draft_Snap_Special" => "star",
        "Draft_Snap_WorkingPlane" => "workplane",
        // Draft geometry. Only the ones with a delivered glyph; the rest keep
        // their label until the note's draw list lands.
        "Draft_BSpline" => "wave-line",
        _ => return None,
    };
    Some(key)
}

/// The icon key for a command, honouring its `Pixmap` resource first.
///
/// Order: a `Pixmap` that names a vendored glyph (a mod's direct choice), then a
/// `Pixmap` that is an id the curated table knows, then the command id itself. This is
/// the note's `Pixmap (or command id) → alias → vendored icon` pipeline.
pub(crate) fn resolve(command: &CommandInfo) -> Option<String> {
    let pixmap = command.pixmap.trim();
    if !pixmap.is_empty() {
        if ferrocad_widgets::icon::asset(pixmap).is_some() {
            return Some(pixmap.to_string());
        }
        if let Some(key) = icon_for(pixmap) {
            return Some(key.to_string());
        }
    }
    icon_for(&command.id).map(str::to_string)
}

/// The icon key for a property row, by its FreeCAD property type.
///
/// Properties are values, not commands, so this is a third table. It matches by
/// substring because the type is often namespaced (`App::PropertyLength`), and the
/// most derived kind must win: `PropertyLength` and `PropertyAngle` both derive from
/// `PropertyQuantity`, so they are tested first.
pub(crate) fn icon_for_property(type_id: &str) -> Option<&'static str> {
    let key = if type_id.contains("PropertyLength")
        || type_id.contains("PropertyDistance")
        || type_id.contains("PropertyArea")
        || type_id.contains("PropertyVolume")
    {
        "ruler"
    } else if type_id.contains("PropertyAngle") {
        "angle"
    } else if type_id.contains("PropertyBool") {
        "toggle-on"
    } else if type_id.contains("PropertyColor") {
        "palette"
    } else if type_id.contains("PropertyVector")
        || type_id.contains("PropertyPlacement")
        || type_id.contains("PropertyRotation")
    {
        "axes-3d"
    } else if type_id.contains("PropertyLink") {
        "chain-link"
    } else if type_id.contains("PropertyEnumeration") {
        "list"
    } else if type_id.contains("PropertyString") || type_id.contains("PropertyText") {
        "type"
    } else if type_id.contains("PropertyInteger")
        || type_id.contains("PropertyFloat")
        || type_id.contains("PropertyQuantity")
    {
        "hash"
    } else {
        return None;
    };
    Some(key)
}

/// The icon key for a model-tree object, by its FreeCAD `TypeId`.
///
/// The tree is a class list, not a command list, so this is a second curated table
/// keyed by type id. Exact primitive types win; everything else falls back to a
/// category icon, so a new workbench's `Part::FeaturePython` still reads as a solid.
pub(crate) fn icon_for_type(type_id: &str) -> Option<&'static str> {
    let key = match type_id {
        "Part::Box" => "cube",
        "Part::Cylinder" => "cylinder",
        "Part::Sphere" => "sphere",
        "Part::Cone" => "cone",
        "Part::Torus" => "torus",
        "Part::Prism" => "prism",
        "Part::Wedge" => "circle-sector",
        "Part::Helix" => "spring-coil",
        "Part::Spiral" => "spiral",
        "Part::Plane" => "workplane",
        "Part::Line" => "line-segment",
        "Part::Point" => "bullet",
        _ if type_id.starts_with("Sketcher::") => "pencil-ruler",
        _ if type_id.starts_with("Draft::Line") => "line-segment",
        _ if type_id.starts_with("Draft::Circle") => "circle",
        _ if type_id.starts_with("Draft::") => "shape-polyline",
        _ if type_id.starts_with("Spreadsheet::") => "table",
        _ if type_id.starts_with("App::Link") => "chain-link",
        _ if type_id.ends_with("Group") => "group",
        _ if type_id.starts_with("Part::") => "cube",
        _ if type_id.starts_with("App::") => "shapes",
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::{icon_for, icon_for_property, icon_for_type, resolve};
    use crate::commands::CommandInfo;

    #[test]
    fn every_curated_key_is_vendored() {
        for id in [
            "App_ToggleGrid",
            "App_ToggleAxisCross",
            "App_ToggleGizmo",
            "App_ViewWorkplane",
            "Dev_ToggleTools",
            "App_Open",
            "App_Save",
            "App_Move",
            "App_Rotate",
            "App_Delete",
            "Toy_MakeSlab",
            "Toy_Polyline",
            "Part_Box",
            "App_SaveDatumPlane",
            "Draft_Snap_Lock",
            "Draft_Snap_Grid",
            "Draft_Snap_Endpoint",
            "Draft_Snap_Center",
            "Draft_Snap_Intersection",
            "Draft_Snap_Perpendicular",
            "Draft_Snap_Parallel",
            "Draft_Snap_Angle",
            "Draft_Snap_Ortho",
            "Draft_Snap_Dimensions",
            "Draft_Snap_Special",
            "Draft_Snap_WorkingPlane",
            "Draft_BSpline",
        ] {
            let key = icon_for(id).unwrap_or_else(|| panic!("{id} is curated"));
            assert!(
                ferrocad_widgets::icon::asset(key).is_some(),
                "{id} -> {key} is not a vendored asset"
            );
        }
    }

    #[test]
    fn an_uncurated_command_has_no_icon() {
        assert!(icon_for("Part_Cylinder").is_none());
        assert!(icon_for("").is_none());
    }

    #[test]
    fn a_pixmap_is_honoured_before_the_command_id() {
        let mut command = CommandInfo {
            id: "App_Move".into(),
            ..Default::default()
        };
        assert_eq!(resolve(&command).as_deref(), Some("move"));

        // An upstream id goes through the curated table.
        command.pixmap = "Draft_Snap_Endpoint".into();
        assert_eq!(resolve(&command).as_deref(), Some("anchor-point"));

        // A mod names a vendored glyph directly.
        command.pixmap = "cube".into();
        assert_eq!(resolve(&command).as_deref(), Some("cube"));

        // An unknown pixmap does not shadow the command's own row.
        command.pixmap = "not-an-icon".into();
        assert_eq!(resolve(&command).as_deref(), Some("move"));
    }

    #[test]
    fn tree_types_resolve_to_vendored_icons() {
        for type_id in [
            "Part::Box",
            "Part::Cylinder",
            "Part::FeaturePython",
            "Sketcher::SketchObject",
            "Draft::Wire",
            "Draft::Line",
            "Spreadsheet::Sheet",
            "App::Link",
            "App::DocumentObjectGroup",
            "App::FeaturePython",
        ] {
            let key =
                icon_for_type(type_id).unwrap_or_else(|| panic!("{type_id} has a category icon"));
            assert!(
                ferrocad_widgets::icon::asset(key).is_some(),
                "{type_id} -> {key} is not a vendored asset"
            );
        }
        assert!(icon_for_type("Unknown::Thing").is_none());
    }

    #[test]
    fn property_types_resolve_to_vendored_icons() {
        for (type_id, expected) in [
            ("App::PropertyLength", "ruler"),
            ("App::PropertyAngle", "angle"),
            ("App::PropertyBool", "toggle-on"),
            ("App::PropertyColor", "palette"),
            ("App::PropertyPlacement", "axes-3d"),
            ("App::PropertyLink", "chain-link"),
            ("App::PropertyEnumeration", "list"),
            ("App::PropertyString", "type"),
            ("App::PropertyFloat", "hash"),
        ] {
            assert_eq!(icon_for_property(type_id), Some(expected), "{type_id}");
            assert!(
                ferrocad_widgets::icon::asset(expected).is_some(),
                "{expected} is vendored"
            );
        }
        assert!(icon_for_property("App::PropertyPartShape").is_none());
    }
}

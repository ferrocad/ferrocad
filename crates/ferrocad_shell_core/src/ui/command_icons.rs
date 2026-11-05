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
        "Part_MakeBox" => "cube",
        "Part_SaveDatumPlane" => "layers-plus",
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
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::icon_for;

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
            "Part_MakeBox",
            "Part_SaveDatumPlane",
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
        assert!(icon_for("Part_Box").is_none());
        assert!(icon_for("").is_none());
    }
}

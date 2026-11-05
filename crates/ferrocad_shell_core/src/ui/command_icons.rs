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
        // Authored draws: a half-tick, a dashed continuation and a proximity asterisk.
        "Draft_Snap_Midpoint" => "midpoint",
        "Draft_Snap_Extension" => "extension",
        "Draft_Snap_Near" => "near",
        // Draft geometry. Ids are validated against upstream's `addCommand`
        // (`freecad-upstream/src/Mod/Draft/draftguitools`). Arc, Arc through 3
        // points and Facebinder stay on the note's draw list, so they keep their label.
        "Draft_Line" => "line-segment",
        "Draft_Wire" => "shape-polyline",
        "Draft_Circle" => "circle",
        "Draft_Ellipse" => "oval",
        "Draft_Rectangle" => "rectangle",
        "Draft_Polygon" => "polygon",
        "Draft_BSpline" => "wave-line",
        "Draft_BezCurve" => "bezier",
        "Draft_Point" => "bullet",
        "Draft_Text" => "text-box",
        "Draft_ShapeString" => "letter-block",
        "Draft_Dimension" => "dimension-line",
        "Draft_Label" => "tag",
        "Draft_Hatch" => "pattern-crosshatch",
        // Draft edit, layers and groups. Ids validated against upstream `addCommand`.
        // The array/mirror rows bake the note's `base + marker` composition into one
        // asset (`mirror`, `polar-array`, ...) so the marker keeps the base's stroke
        // weight at toolbar size.
        // The task-panel buttons (`Draft_Wipe`, `Draft_Finish`, `Draft_Apply`,
        // `Draft_Lock`, `Draft_Construction`) and `Draft_VisGroup`/`Draft_NewLayer`
        // are icon names or removed commands, not registered command ids.
        "Draft_Move" => "move",
        "Draft_Rotate" => "rotate-object",
        "Draft_Scale" => "resize-handle",
        "Draft_Mirror" => "mirror",
        "Draft_Clone" => "duplicate-object",
        "Draft_OrthoArray" => "grid-4x4",
        "Draft_PolarArray" => "polar-array",
        "Draft_PathArray" => "path-array",
        "Draft_PointArray" => "point-array",
        "Draft_Trimex" => "cut-line",
        "Draft_Fillet" => "corner-radius",
        "Draft_Join" => "merge",
        "Draft_Split" => "split",
        "Draft_Upgrade" => "merge",
        "Draft_Downgrade" => "split",
        "Draft_Stretch" => "expand-diagonal",
        "Draft_Heal" => "wrench",
        "Draft_Layer" => "layers",
        "Draft_AddToLayer" => "layers-plus",
        "Draft_AddToGroup" => "group",
        "Draft_SelectGroup" => "group",
        "Draft_AutoGroup" => "group",
        "Draft_FlipDimension" => "flip-horizontal",
        "Draft_SubelementHighlight" => "highlighter",
        // Their `Pixmap` names (`Draft_SwitchMode`, `Draft_Construction`) are not the
        // command ids upstream registers.
        "Draft_ToggleDisplayMode" => "swap-horizontal",
        "Draft_ToggleConstructionMode" => "square-dashed",
        // Part primitives and datums. Box/Cylinder/Sphere/Cone/Torus/Tube and the
        // three datum commands are the GUI commands in 1.1.4; Prism, Wedge, Helix,
        // Spiral and Ellipsoid exist only as `Part::*` objects, so `icon_for_type`
        // covers them for the model tree.
        "Part_Box" => "cube",
        "Part_Cylinder" => "cylinder",
        "Part_Sphere" => "sphere",
        "Part_Cone" => "cone",
        "Part_Torus" => "torus",
        "Part_Tube" => "pipe-flange",
        "Part_DatumPlane" => "workplane",
        "Part_DatumLine" => "line-segment",
        "Part_DatumPoint" => "bullet",
        "Part_Primitives" => "shapes",
        // Part operations. Extrude, Loft, Sweep, Thickness, MakeFace and MakeSolid stay
        // on the note's draw list; `Part_JoinBypass` is not registered in 1.1.4.
        "Part_Revolve" => "rotate-3d",
        "Part_Fillet" => "corner-radius",
        "Part_Chamfer" => "bevel-gauge",
        "Part_Fuse" => "union",
        "Part_Cut" => "boolean-subtract",
        "Part_Common" => "boolean-intersect",
        "Part_Section" => "section",
        "Part_CrossSections" => "pattern-stripes",
        "Part_Compound" => "group",
        "Part_CompoundFilter" => "filter",
        "Part_ExplodeCompound" => "expand",
        "Part_Builder" => "shapes",
        "Part_RefineShape" => "sparkles",
        "Part_ColorPerFace" => "paint-bucket",
        "Part_ProjectionOnSurface" => "projector",
        "Part_ReverseShape" => "flip-horizontal",
        "Part_Mirror" => "mirror",
        "Part_Scale" => "resize-handle",
        "Part_CheckGeometry" => "circle-check",
        "Part_Defeaturing" => "eraser",
        "Part_BooleanFragments" => "split",
        "Part_SliceApart" => "split",
        "Part_JoinConnect" => "merge",
        "Part_JoinEmbed" => "merge",
        "Part_JoinCutout" => "merge",
        // Measure: 1.1.4 registers one command; the eight `Measurement-*` files are its
        // mode icons, not commands.
        "Std_Measure" => "measure-tool",
        // Standard views (Std_View* in src/Gui/CommandView.cpp).
        "Std_ViewFront" => "view-front",
        "Std_ViewTop" => "view-top",
        "Std_ViewRight" => "view-right",
        "Std_ViewRear" => "view-rear",
        // Solid verbs that were on the note's draw list.
        "Part_Extrude" => "extrude",
        "Part_Loft" => "loft",
        "Part_Sweep" => "sweep",
        "Part_Thickness" => "thickness",
        "Part_MakeFace" => "make-face",
        "Part_MakeSolid" => "make-solid",
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
        "Part::Tube" => "pipe-flange",
        "Part::Ellipsoid" => "sphere",
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
            "Draft_Snap_Midpoint",
            "Draft_Snap_Extension",
            "Draft_Snap_Near",
            "Draft_Line",
            "Draft_Wire",
            "Draft_Circle",
            "Draft_Ellipse",
            "Draft_Rectangle",
            "Draft_Polygon",
            "Draft_BSpline",
            "Draft_BezCurve",
            "Draft_Point",
            "Draft_Text",
            "Draft_ShapeString",
            "Draft_Dimension",
            "Draft_Label",
            "Draft_Hatch",
            "Draft_Move",
            "Draft_Rotate",
            "Draft_Scale",
            "Draft_Mirror",
            "Draft_Clone",
            "Draft_OrthoArray",
            "Draft_PolarArray",
            "Draft_PathArray",
            "Draft_PointArray",
            "Draft_Trimex",
            "Draft_Fillet",
            "Draft_Join",
            "Draft_Split",
            "Draft_Upgrade",
            "Draft_Downgrade",
            "Draft_Stretch",
            "Draft_Heal",
            "Draft_Layer",
            "Draft_AddToLayer",
            "Draft_AddToGroup",
            "Draft_SelectGroup",
            "Draft_AutoGroup",
            "Draft_FlipDimension",
            "Draft_SubelementHighlight",
            "Draft_ToggleDisplayMode",
            "Draft_ToggleConstructionMode",
            "Part_Box",
            "Part_Cylinder",
            "Part_Sphere",
            "Part_Cone",
            "Part_Torus",
            "Part_Tube",
            "Part_DatumPlane",
            "Part_DatumLine",
            "Part_DatumPoint",
            "Part_Primitives",
            "Part_Revolve",
            "Part_Fillet",
            "Part_Chamfer",
            "Part_Fuse",
            "Part_Cut",
            "Part_Common",
            "Part_Section",
            "Part_CrossSections",
            "Part_Compound",
            "Part_CompoundFilter",
            "Part_ExplodeCompound",
            "Part_Builder",
            "Part_RefineShape",
            "Part_ColorPerFace",
            "Part_ProjectionOnSurface",
            "Part_ReverseShape",
            "Part_Mirror",
            "Part_Scale",
            "Part_CheckGeometry",
            "Part_Defeaturing",
            "Part_BooleanFragments",
            "Part_SliceApart",
            "Part_JoinConnect",
            "Part_JoinEmbed",
            "Part_JoinCutout",
            "Std_Measure",
            "Std_ViewFront",
            "Std_ViewTop",
            "Std_ViewRight",
            "Std_ViewRear",
            "Part_Extrude",
            "Part_Loft",
            "Part_Sweep",
            "Part_Thickness",
            "Part_MakeFace",
            "Part_MakeSolid",
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
        assert!(icon_for("Part_XOR").is_none());
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
            "Part::Sphere",
            "Part::Cone",
            "Part::Torus",
            "Part::Tube",
            "Part::Ellipsoid",
            "Part::Prism",
            "Part::Wedge",
            "Part::Helix",
            "Part::Spiral",
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

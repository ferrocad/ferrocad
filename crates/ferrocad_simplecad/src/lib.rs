//! FerroCAD: SimpleCAD — the edition's contributions, split out of `main`.
//!
//! The binary is the composition (register the built-ins, parse the flags, run the
//! shell); this library holds what the edition *contributes*, so the round-trip test
//! can inspect the same registry the window installs.

pub use ferrocad_stage::{ArcTool, BoxTool, CircleTool, PolygonTool, RectangleTool};

/// The tool id the two-click rectangle is registered under.
pub const RECTANGLE_TOOL: &str = "rectangle";
/// The tool id the two-click circle is registered under.
pub const CIRCLE_TOOL: &str = "circle";
/// The tool id the two-click regular polygon is registered under.
pub const POLYGON_TOOL: &str = "polygon";
/// The tool id the three-click arc is registered under.
pub const ARC_TOOL: &str = "arc";

/// The edition's shell contributions: the Parametric edition's, plus the rectangle.
///
/// SimpleCAD re-exports the Parametric app script, so its command set *is* the Part
/// one (`Part_Polyline`, `Sketch_Polyline`, the face push/pull stem, the edge dress-up
/// scrubber) with the `SimpleCAD_*` drawing tools added. The tools and manipulators
/// behind those ids therefore have to be registered here too: a command whose
/// `{"tool": id}` the host cannot resolve reports "no such tool", and a selected face
/// with no manipulator factory offers no handle at all.
///
/// The four drawing tools ([`RectangleTool`], [`CircleTool`], [`PolygonTool`],
/// [`ArcTool`]) are the edition's own; everything else is the same object the
/// Parametric edition contributes, taken from the same two crates ([`ferrocad_stage`]
/// and [`ferrocad_part_gui`]) so the two editions cannot drift.
pub fn contributions() -> ferrocad::Contributions {
    ferrocad::base_contributions()
        // The edge dress-up arc (SimpleCAD's Round) and the face push/pull stem.
        .with_manipulator(ferrocad_part_gui::fillet_manipulator)
        .with_manipulator(ferrocad_part_gui::push_pull_manipulator)
        // `Part_Polyline` draws a wire; `Sketch_Polyline` the same points as a sketch.
        .with_tool("polyline", || {
            Box::new(ferrocad_stage::TaggedPolyline::new("polyline"))
                as Box<dyn ferrocad_stage::Tool>
        })
        .with_tool("sketch_polyline", || {
            Box::new(ferrocad_stage::TaggedPolyline::new("sketch_polyline"))
                as Box<dyn ferrocad_stage::Tool>
        })
        // The edition's own tools: two or three clicks, one face or one edge. Each
        // tool is generic; the app script decides what its commit *means*.
        .with_tool(RECTANGLE_TOOL, || {
            Box::new(RectangleTool::new()) as Box<dyn ferrocad_stage::Tool>
        })
        .with_tool(CIRCLE_TOOL, || Box::new(CircleTool::new()) as Box<dyn ferrocad_stage::Tool>)
        .with_tool(POLYGON_TOOL, || Box::new(PolygonTool::new()) as Box<dyn ferrocad_stage::Tool>)
        .with_tool(ARC_TOOL, || Box::new(ArcTool::new()) as Box<dyn ferrocad_stage::Tool>)
}

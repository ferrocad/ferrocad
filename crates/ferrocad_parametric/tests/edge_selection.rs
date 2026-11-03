//! End-to-end: the **Part edition's** Edge selector resolves to a real edge.
//!
//! The kernel's tessellation names its edges (`Edge1`…`Edge12`) and duplicates the
//! positions per primitive, so the stage carries a per-segment part id. This test proves
//! the whole path the viewport uses: `stage_meshes` → `DisplaySync` → `ShapeItem` →
//! `PickScope::Edge` → a `SelectionRef` naming the edge, in shaded **and** wireframe
//! display. It needs OCCT.

use std::sync::{Mutex, Once};

use ferrocad_shell_gpui::DisplaySync;
use ferrocad_shell_gpui::python;
use ferrocad_graphics::{Camera, Projection, RenderQueue, Viewport};
use ferrocad_inventor::{ImageData, InventorView, Scene};
use ferrocad_stage::screen::project_device;
use ferrocad_stage::{DrawMode, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_viewport::{InventorViewport, PickScope};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register both built-in modules before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

/// A viewport with no GPU: the CPU ray cast is the picker, which is what the edge
/// resolver is built on.
struct NullView;

impl InventorView for NullView {
    type Surface = ();
    type Error = std::io::Error;

    fn size(&self) -> [u32; 2] {
        [400, 400]
    }
    fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
        Ok(())
    }
    fn borrow_surface(&self) -> Option<&()> {
        None
    }
    fn image(&mut self) -> Result<ImageData, Self::Error> {
        Ok(ImageData::default())
    }
}

fn camera() -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -100.0), Rotation::identity()).to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 1000.0,
        },
        viewport: Viewport::new([400, 400], 1.0),
    }
}

/// A viewport showing `stage`, framed on it, in the Edge scope.
fn edge_view(stage: Stage) -> InventorViewport<NullView> {
    let mut view = InventorViewport::new(NullView, camera(), Scene::default());
    view.set_stage(stage);
    view.frame_stage();
    view.set_pick_scope(PickScope::Edge);
    view
}

/// The projected midpoint of `segment` (an `edges` index pair) and the device-space
/// direction pointing away from the body's centre.
fn midpoint(
    view: &InventorViewport<NullView>,
    positions: &[[f32; 3]],
    segment: &[u32],
) -> Option<([f32; 2], [f64; 2])> {
    let a = positions[segment[0] as usize];
    let b = positions[segment[1] as usize];
    let mid = Vector3::new(
        f64::from(a[0] + b[0]) / 2.0,
        f64::from(a[1] + b[1]) / 2.0,
        f64::from(a[2] + b[2]) / 2.0,
    );
    let device = project_device(view.camera(), mid)?;
    let logical = view
        .camera()
        .viewport
        .to_logical([device[0].round() as i32, device[1].round() as i32]);
    Some((logical, device))
}

#[test]
fn the_parametric_edge_scope_selects_an_edge() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let box_mesh = meshes
        .iter()
        .find(|mesh| mesh.object == "Box")
        .expect("the seeded Box is displayed");
    let entity = box_mesh.entity;
    assert!(
        box_mesh.parts.iter().any(|part| part == "Edge1"),
        "the kernel names the box's edges"
    );
    assert_eq!(
        box_mesh.segment_parts.len(),
        box_mesh.edges.len() / 2,
        "one part id per drawn segment"
    );
    assert!(
        box_mesh
            .segment_parts
            .iter()
            .all(|id| box_mesh.parts[*id as usize].starts_with("Edge")),
        "every segment is tagged with an edge, not a face"
    );

    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    let item = stage.item(entity).expect("the item").clone();
    let segments: Vec<[u32; 2]> = item
        .edges()
        .chunks_exact(2)
        .map(|segment| [segment[0], segment[1]])
        .collect();

    // Shaded: a hover near each edge resolves to an edge of the Box.
    let mut view = edge_view(Stage::new().with_item(item.clone()));
    let mut resolved = 0usize;
    for segment in &segments {
        let Some((logical, _)) = midpoint(&view, item.positions(), segment) else {
            continue;
        };
        view.handle_pointer(logical);
        let target = view
            .hover_report()
            .target
            .clone()
            .expect("an edge is under the pointer");
        assert_eq!(target.object, entity, "the edge belongs to the Box");
        assert!(
            target
                .elements
                .first()
                .is_some_and(|name| name.starts_with("Edge")),
            "the hover names an edge, got {target:?}"
        );
        resolved += 1;
    }
    assert!(resolved >= 12, "every drawn edge is hoverable ({resolved})");

    // A click on an edge selects it, not the face behind it.
    let (logical, _) = midpoint(&view, item.positions(), &segments[0]).expect("the edge projects");
    view.handle_button(logical, 1, true);
    view.handle_button(logical, 1, false);
    let Some(event) = view.take_selection() else {
        panic!("the click selects");
    };
    let ferrocad_viewport::SelectionEvent::Picked { reference, .. } = event else {
        panic!("not a pick: {event:?}");
    };
    assert!(
        reference
            .elements
            .first()
            .is_some_and(|name| name.starts_with("Edge"))
    );

    // Wireframe draws no surface, so a pointer just beside a line hits nothing; the
    // Edge scope must still resolve the edge it is beside (the reported "unselectable
    // in wireframe" case).
    let wire = item.with_draw_mode(DrawMode::Wireframe);
    let mut view = edge_view(Stage::new().with_item(wire.clone()));
    let centre = segments
        .iter()
        .filter_map(|segment| midpoint(&view, wire.positions(), segment))
        .map(|(_, device)| device)
        .fold([0.0, 0.0], |acc, device| {
            [acc[0] + device[0], acc[1] + device[1]]
        });
    let count = segments.len() as f64;
    let centre = [centre[0] / count, centre[1] / count];
    let (_, device) = midpoint(&view, wire.positions(), &segments[0]).expect("the edge projects");
    let away = [device[0] - centre[0], device[1] - centre[1]];
    let length = away[0].hypot(away[1]).max(1e-9);
    // Outside the 2 px CPU line tolerance, inside the 8 px edge tolerance.
    let outside = [
        device[0] + away[0] / length * 5.0,
        device[1] + away[1] / length * 5.0,
    ];
    let logical = view
        .camera()
        .viewport
        .to_logical([outside[0].round() as i32, outside[1].round() as i32]);
    view.handle_pointer(logical);
    let target = view
        .hover_report()
        .target
        .clone()
        .expect("the nearby edge resolves even though the wire was missed");
    assert!(
        target
            .elements
            .first()
            .is_some_and(|name| name.starts_with("Edge")),
        "the wireframe hover names an edge, got {target:?}"
    );
    view.handle_button(logical, 1, true);
    view.handle_button(logical, 1, false);
    let Some(ferrocad_viewport::SelectionEvent::Picked { reference, .. }) = view.take_selection()
    else {
        panic!("a click beside the wireframe line selects the edge");
    };
    assert!(
        reference
            .elements
            .first()
            .is_some_and(|name| name.starts_with("Edge"))
    );
}

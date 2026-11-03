//! Diagnostic: does the **parametric Box** draw and pick its edges?
//!
//! This isolates the two halves of the Edge selector on the real edition's document:
//!
//! 1. **Display** — highlight one edge (`Edge1`) on the stage, render the frame
//!    offscreen, and write both the plain and the highlighted frame to
//!    `target/edge-probe/`. The changed-pixel report says whether the edge was drawn
//!    (a thin boundary) or the whole face was (a filled area).
//! 2. **Pick** — run the viewport's pick at points around the same edge with the CPU
//!    ray cast and with the GPU G-buffer, and print what each reports. If the pick
//!    names the edge but the frame does not show it, the display is broken; if the
//!    pick names the face, the resolver/geometry is broken.
//!
//! ```sh
//! cargo run -p ferrocad_parametric --example edge_probe
//! ```

use std::sync::Once;

use ferrocad_shell_gpui::{DisplaySync, python};
use ferrocad_graphics::{Camera, RenderQueue};
use ferrocad_inventor::{ImageData, InventorView, Scene, ViewSpec};
use ferrocad_stage::screen::project_device;
use ferrocad_stage::{DrawMode, Stage};
use ferrocad_types::Vector3;
use ferrocad_viewport::{InventorViewport, PickScope};
use ferrocad_wgpu::WgpuRenderer;
use ferrocad_wgpu::snapshot::encode_png;

static BOOT: Once = Once::new();

const SIZE: [u32; 2] = [800, 600];
const OUT_DIR: &str = "target/edge-probe";

/// A viewport with no GPU: the CPU ray cast is the picker.
struct NullView;

impl InventorView for NullView {
    type Surface = ();
    type Error = std::io::Error;

    fn size(&self) -> [u32; 2] {
        SIZE
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

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

fn camera() -> Camera {
    // A three-quarter view so no two box edges project onto each other (a head-on camera
    // makes the back edges hide under the front ones).
    ferrocad_wgpu::SnapshotCamera {
        eye: [72.0, 92.0, 78.0],
        target: [10.0, 10.0, 10.0],
        up: [0.0, 0.0, 1.0],
        fov_y: 45.0f64.to_radians(),
    }
    .to_camera(SIZE)
}

fn write_png(name: &str, image: &ImageData) {
    std::fs::create_dir_all(OUT_DIR).expect("create the output directory");
    let path = format!("{OUT_DIR}/{name}.png");
    let png = encode_png(image).expect("encode the PNG");
    std::fs::write(&path, png).expect("write the PNG");
    println!("  wrote {path} ({}×{})", image.width, image.height);
}

/// `(changed pixel count, bounding box top-left origin)` between two frames.
fn diff(a: &ImageData, b: &ImageData) -> (usize, Option<[u32; 4]>) {
    let mut count = 0;
    let mut bounds: Option<[u32; 4]> = None;
    for (index, (left, right)) in a
        .rgba
        .chunks_exact(4)
        .zip(b.rgba.chunks_exact(4))
        .enumerate()
    {
        if left != right {
            count += 1;
            let x = (index as u32) % a.width;
            let y = (index as u32) / a.width;
            bounds = Some(match bounds {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
    }
    (count, bounds)
}

fn main() {
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let box_mesh = meshes
        .iter()
        .find(|mesh| mesh.object == "Box")
        .expect("the seeded Box");
    let entity = box_mesh.entity;
    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    let item = stage.item(entity).expect("the Box item").clone();
    println!(
        "Box: entity {entity}, {} positions, {} segments, {} parts {:?}",
        item.positions().len(),
        item.edges().len() / 2,
        item.part_names().len(),
        item.part_names()
    );
    // The vertical edge nearest the camera is the most legible one to light up in the
    // shaded snapshot (the far edges are occluded by the solid).
    let eye = [72.0f64, 92.0, 78.0];
    let mut best: Option<(f64, u32, [u32; 2])> = None;
    for segment in item.edges().chunks_exact(2) {
        let part = item.parts()[segment[0] as usize];
        if !item.part_names()[part as usize].starts_with("Edge") {
            continue;
        }
        let a = item.positions()[segment[0] as usize];
        let b = item.positions()[segment[1] as usize];
        let mid = [
            f64::from(a[0] + b[0]) / 2.0,
            f64::from(a[1] + b[1]) / 2.0,
            f64::from(a[2] + b[2]) / 2.0,
        ];
        let score = mid[0] * eye[0] + mid[1] * eye[1] + mid[2] * eye[2];
        if best.is_none_or(|(best, _, _)| score > best) {
            best = Some((score, part, [segment[0], segment[1]]));
        }
    }
    let (_, edge, segment) = best.expect("the Box has drawn edges");
    println!(
        "highlighting {} (part id {edge}), segment {segment:?} → positions {:?} / {:?}",
        item.part_name(edge).unwrap_or("?"),
        item.positions()[segment[0] as usize],
        item.positions()[segment[1] as usize]
    );

    // Frame the stage the same way the app does, then keep that camera for both halves.
    let mut framing = InventorViewport::new(NullView, camera(), Scene::default());
    framing.set_stage(stage.clone());
    framing.frame_stage();
    let camera = framing.camera().clone();

    // ------------------------------------------------------------------
    // 1. Display: render plain and with Edge1 highlighted.
    // ------------------------------------------------------------------
    println!("\ndisplay:");
    let mut renderer = WgpuRenderer::new_default(SIZE).expect("a GPU adapter");
    let plain = {
        let queue = stage.to_queue();
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
                workplane: None,
            }])
            .expect("draw the plain frame");
        renderer.image().expect("read the plain frame")
    };
    write_png("plain", &plain);

    let mut highlighted = stage.clone();
    for item in highlighted.items_mut() {
        if item.entity_id() == entity {
            item.set_highlight([edge]);
        }
    }
    let queue = highlighted.to_queue();
    println!(
        "  queue packets: {} (plain {}), highlighted parts for the Box: {:?}",
        queue.packets.len(),
        stage.to_queue().packets.len(),
        highlighted
            .item(entity)
            .expect("the Box")
            .highlight()
            .to_vec()
    );
    renderer
        .draw_frame(&[ViewSpec {
            queue: &queue,
            camera: &camera,
            rect: None,
            overlay: false,
            workplane: None,
        }])
        .expect("draw the highlighted frame");
    let edge_frame = renderer.image().expect("read the highlighted frame");
    write_png("edge-highlight", &edge_frame);

    let (changed, bounds) = diff(&plain, &edge_frame);
    println!("  changed pixels: {changed} at {bounds:?}");
    if let Some([x0, y0, x1, y1]) = bounds {
        let area = (x1 - x0 + 1) * (y1 - y0 + 1);
        println!(
            "  changed area {}×{} = {area} px (a face fill would be thousands of px)",
            x1 - x0 + 1,
            y1 - y0 + 1
        );
    }

    // Same item in Wireframe mode: the highlight line must still be drawn, because the
    // ordinary wire is what carries it.
    let wire = item.clone().with_draw_mode(DrawMode::Wireframe);
    let mut wire_stage = Stage::new().with_item(wire);
    let wire_plain = {
        let queue = wire_stage.to_queue();
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
                workplane: None,
            }])
            .expect("draw the wireframe frame");
        renderer.image().expect("read the wireframe frame")
    };
    write_png("wireframe", &wire_plain);
    for item in wire_stage.items_mut() {
        item.set_highlight([edge]);
    }
    let queue = wire_stage.to_queue();
    renderer
        .draw_frame(&[ViewSpec {
            queue: &queue,
            camera: &camera,
            rect: None,
            overlay: false,
            workplane: None,
        }])
        .expect("draw the highlighted wireframe frame");
    let wire_highlighted = renderer
        .image()
        .expect("read the highlighted wireframe frame");
    write_png("wireframe-edge-highlight", &wire_highlighted);
    let (wire_changed, wire_bounds) = diff(&wire_plain, &wire_highlighted);
    println!("  wireframe changed pixels: {wire_changed} at {wire_bounds:?}");

    // ------------------------------------------------------------------
    // 2. Pick: CPU ray cast and GPU G-buffer at the same points.
    // ------------------------------------------------------------------
    println!("\npick (device coordinates, bottom-left origin):");
    let a = item.positions()[segment[0] as usize];
    let b = item.positions()[segment[1] as usize];
    let mid = [
        f64::from(a[0] + b[0]) / 2.0,
        f64::from(a[1] + b[1]) / 2.0,
        f64::from(a[2] + b[2]) / 2.0,
    ];
    let centre = item.positions().iter().fold([0.0f64; 3], |mut acc, p| {
        for axis in 0..3 {
            acc[axis] += f64::from(p[axis]);
        }
        acc
    });
    let count = item.positions().len() as f64;
    let centre = [centre[0] / count, centre[1] / count, centre[2] / count];
    let device = project_device(&camera, Vector3::new(mid[0], mid[1], mid[2])).expect("projects");
    let toward = project_device(&camera, Vector3::new(centre[0], centre[1], centre[2]))
        .expect("centre projects");
    let inward = [toward[0] - device[0], toward[1] - device[1]];
    let length = inward[0].hypot(inward[1]).max(1e-9);
    let inward = [inward[0] / length, inward[1] / length];

    let points = [
        ("on the line", device),
        (
            "4 px inside",
            [device[0] + inward[0] * 4.0, device[1] + inward[1] * 4.0],
        ),
        (
            "6 px inside",
            [device[0] + inward[0] * 6.0, device[1] + inward[1] * 6.0],
        ),
        (
            "4 px outside",
            [device[0] - inward[0] * 4.0, device[1] - inward[1] * 4.0],
        ),
        ("face centre", toward),
    ];

    let gpu_view = WgpuRenderer::new_default(SIZE).expect("a GPU adapter");
    let mut cpu = InventorViewport::new(NullView, camera.clone(), Scene::default());
    cpu.set_stage(stage.clone());
    // `set_camera` recompiles the queue a pick reads; without it a pick before the first
    // paint runs against an empty queue.
    cpu.set_camera(camera.clone());
    cpu.set_pick_scope(PickScope::Edge);
    let mut gpu = InventorViewport::new(gpu_view, camera.clone(), Scene::default());
    gpu.set_stage(stage.clone());
    gpu.set_camera(camera.clone());
    gpu.set_pick_scope(PickScope::Edge);

    for (label, at) in points {
        let logical = camera
            .viewport
            .to_logical([at[0].round() as i32, at[1].round() as i32]);
        println!(
            "  {label:>14} at ({:>6.1}, {:>6.1}):  cpu: {}",
            at[0],
            at[1],
            probe(&mut cpu, logical)
        );
        println!(
            "  {:>14} {:>22}   gpu: {}",
            "",
            "",
            probe(&mut gpu, logical)
        );
    }
}

/// Hover and click `view` at `logical`, summarised for the table above.
fn probe<V: InventorView>(view: &mut InventorViewport<V>, logical: [f32; 2]) -> String {
    view.handle_pointer(logical);
    let mut out = format!(
        "hit {:?}/part {:?} → {:?}",
        view.hovered().map(|id| id.0),
        view.hovered_part(),
        view.hover_report().target
    );
    view.handle_button(logical, 1, true);
    view.handle_button(logical, 1, false);
    match view.take_selection() {
        Some(ferrocad_viewport::SelectionEvent::Picked { reference, .. }) => {
            out.push_str(&format!(" (click {reference:?})"));
        }
        Some(_) => out.push_str(" (click: cleared)"),
        None => {}
    }
    out
}

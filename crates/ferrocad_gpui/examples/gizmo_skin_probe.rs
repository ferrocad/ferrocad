//! Headless check for the gizmo shader skin (slice G2).
//!
//! The probe depends on the mod's `Resources/gizmo_skin.wgsl`:
//! `mods/GizmoOverlayShader/Resources/gizmo_skin.wgsl` is the only copy, and the
//! mod now owns the shader (the app exports no constant).
//!
//! Renders two offscreen frames and asserts on the centre pixel:
//!
//! 1. a **geometry** program (`ShaderProgram::with_geometry`) drawing a red triangle —
//!    the skin path the gizmo uses, so a red centre proves the packet's own mesh is
//!    rasterised with the authored shader;
//! 2. a **plain** program — the full-screen placeholder the grid/axis overlays rely
//!    on, so a green centre proves that path is unchanged.
//!
//! ```text
//! # needs a GPU (a software Vulkan ICD is enough):
//! VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
//!   cargo run -p ferrocad_gpui --example gizmo_skin_probe
//! ```

use std::sync::Arc;

use ferrocad_inventor::{InventorView, RenderQueue, ViewSpec, Viewport};
use ferrocad_stage::{DrawMode, ShaderProgram, ShapeItem};
use ferrocad_viewport::scene::sample_camera;
use ferrocad_wgpu::WgpuRenderer;

const SIZE: [u32; 2] = [64, 64];

/// The mod-owned skin source, read at runtime.
fn gizmo_skin_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../mods/GizmoOverlayShader/Resources/gizmo_skin.wgsl");
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("the shader mod must ship {}: {error}", path.display())
    })
}

/// The trivial full-screen placeholder program: it ignores geometry entirely.
/// A plain program still needs the ABI's `vs_main` (the placeholder's corners only
/// matter by sign, as in the grid and axis overlays).
const PLAIN_WGSL: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> @builtin(position) vec4<f32> {
    return vec4<f32>(sign(input.position.xy), 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

fn triangle(program: Arc<ShaderProgram>) -> RenderQueue {
    let item = ShapeItem::new(1)
        .with_shape(
            vec![
                [-2.0, -2.0, 0.0],
                [2.0, -2.0, 0.0],
                [0.0, 2.0, 0.0],
            ],
            // No normals: the scene shader and the skin both handle a degenerate one.
            vec![],
            vec![0, 1, 2],
            vec![],
        )
        .with_draw_mode(DrawMode::Shaded)
        .with_surface_color([0.8, 0.05, 0.05, 1.0])
        .with_program(program);
    let mut queue = RenderQueue::default();
    item.emit(&mut queue);
    queue
}

fn centre(renderer: &mut WgpuRenderer) -> [u8; 4] {
    let image = renderer.image().expect("read back the frame");
    let index = ((image.height / 2) * image.width + image.width / 2) as usize * 4;
    [
        image.rgba[index],
        image.rgba[index + 1],
        image.rgba[index + 2],
        image.rgba[index + 3],
    ]
}

fn main() {
    let mut renderer =
        WgpuRenderer::new_default(SIZE).expect("no GPU adapter (try VK_ICD_FILENAMES=lvp)");
    let camera = sample_camera(Viewport::new(SIZE, 1.0));

    // 1. A geometry program draws the packet's own triangle.
    let geometry = Arc::new(ShaderProgram::from_source(gizmo_skin_source()).with_geometry());
    let queue = triangle(geometry);
    renderer
        .draw_frame(&[ViewSpec {
            queue: &queue,
            camera: &camera,
            rect: None,
            overlay: false,
        }])
        .expect("draw the skinned triangle");
    let skinned = centre(&mut renderer);

    // 2. A plain program still covers the frame with the ABI placeholder.
    let plain = Arc::new(ShaderProgram::from_source(PLAIN_WGSL));
    let queue = triangle(plain);
    renderer
        .draw_frame(&[ViewSpec {
            queue: &queue,
            camera: &camera,
            rect: None,
            overlay: false,
        }])
        .expect("draw the placeholder");
    let placeholder = centre(&mut renderer);

    println!("geometry centre: {skinned:?}");
    println!("placeholder centre: {placeholder:?}");

    let red = skinned[0] > 80 && skinned[0] > skinned[1].saturating_add(40);
    let green = placeholder[1] > 200 && placeholder[0] < 40 && placeholder[2] < 40;
    assert!(red, "the geometry program did not draw its own triangle: {skinned:?}");
    assert!(
        green,
        "the plain program did not draw the placeholder: {placeholder:?}"
    );
    println!("gizmo skin probe OK");
}

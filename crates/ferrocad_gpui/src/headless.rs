//! Headless edition rendering.
//!
//! The editions open a window, but the model path and the renderer are separable:
//! boot the app script with `FreeCAD.GuiUp == false`, ask it for the tessellated
//! document shapes (`stage_meshes`), compile the stage, and draw one offscreen
//! frame. This is the same path the window's viewport uses (`build_stage` →
//! `Stage::to_queue` → `WgpuRenderer`), minus gpui.
//!
//! `ferrocad_toy --render out.png` needs no kernel and runs anywhere with a Vulkan
//! (or other wgpu) adapter, including a software one. `ferrocad_parametric` adds
//! the Part workbench, so its frame is a real OCCT solid.

use std::io::Write;
use std::path::{Path, PathBuf};

use ferrocad_inventor::{Camera, InventorView, Projection, ViewSpec, Viewport};
use ferrocad_stage::Stage;
use ferrocad_types::{Rotation, Vector3};
use ferrocad_viewport::scene::sample_camera;
use ferrocad_wgpu::WgpuRenderer;

use crate::HostConfig;
use crate::display::{DisplaySync, StageMesh};
use crate::python;

/// A binary STL written from the same tessellation the viewport draws.
#[derive(Debug, Clone)]
pub struct StlReport {
    /// The file that was written.
    pub path: PathBuf,
    /// The number of facets (one per tessellated triangle).
    pub triangles: usize,
}

/// What a headless run produced, for the caller to print.
#[derive(Debug, Clone)]
pub struct RenderReport {
    /// The document the app script bootstrapped (or `--open` loaded).
    pub document: String,
    /// The document objects that contributed a mesh, in draw order.
    pub objects: Vec<String>,
    /// The framebuffer size, when a PNG was rendered.
    pub size: Option<[u32; 2]>,
    /// The STL, when one was exported.
    pub stl: Option<StlReport>,
}

/// Boot the edition, optionally run a script, and write a PNG and/or an STL.
///
/// `render` and `stl` are independent: either or both may be given (at least one
/// is required). `script` runs after the edition's `bootstrap` and any `--open`
/// document, before the display meshes are read, so it can build the model. The
/// size is device pixels. A PNG needs a wgpu adapter; a software Vulkan ICD is
/// enough. STL export needs no GPU. `out` parents are created when needed.
pub fn run(
    config: &HostConfig,
    render: Option<&Path>,
    stl: Option<&Path>,
    size: [u32; 2],
    script: Option<&Path>,
) -> Result<RenderReport, String> {
    if render.is_none() && stl.is_none() {
        return Err("nothing to do: pass --render, --stl or both".to_string());
    }

    // No GUI: `FreeCAD.GuiUp` stays false, and no window is created.
    python::configure(
        &config.entry_module,
        &config.python_paths,
        &config.mods_paths,
        false,
    );
    python::configure_startup_document(config.open_document.as_deref());

    let mut boot = python::bootstrap()?;
    python::open_startup_document(&mut boot)?;
    if let Some(script) = script {
        python::run_script(script)?;
    }
    let meshes = python::stage_meshes()?;
    if meshes.is_empty() {
        return Err(format!(
            "{} displayed no shapes (is a display provider registered?)",
            config.entry_module
        ));
    }
    let objects: Vec<String> = meshes.iter().map(|mesh| mesh.object.clone()).collect();
    let stl = stl.map(|path| write_stl(&meshes, path)).transpose()?;

    let mut report = RenderReport {
        document: boot.document,
        objects,
        size: None,
        stl,
    };

    if let Some(out) = render {
        let stage = DisplaySync::new().build(meshes);
        let camera = framed_camera(&stage, size);
        let queue = stage.to_queue();
        let mut renderer =
            WgpuRenderer::new_default(size).map_err(|error| format!("no GPU adapter: {error}"))?;
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .map_err(|error| format!("draw failed: {error}"))?;
        let image = renderer
            .image()
            .map_err(|error| format!("readback failed: {error}"))?;
        let buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
            .ok_or_else(|| "readback buffer size does not match its dimensions".to_string())?;
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
            }
        }
        buffer
            .save(out)
            .map_err(|error| format!("could not write {}: {error}", out.display()))?;
        report.size = Some([image.width, image.height]);
    }

    Ok(report)
}

/// Render only, the original entry point (used by the library example).
pub fn render(
    config: &HostConfig,
    out: &Path,
    size: [u32; 2],
    script: Option<&Path>,
) -> Result<RenderReport, String> {
    run(config, Some(out), None, size, script)
}

/// Write the stage meshes as one binary STL. The meshes are already in world
/// coordinates (the display hooks bake object placements), so an assembly exports
/// in the same pose it renders.
fn write_stl(meshes: &[StageMesh], path: &Path) -> Result<StlReport, String> {
    let mut body = Vec::new();
    let mut count = 0u32;
    for mesh in meshes {
        for triangle in mesh.triangles.chunks_exact(3) {
            let (Some(p0), Some(p1), Some(p2)) = (
                mesh.positions.get(triangle[0] as usize),
                mesh.positions.get(triangle[1] as usize),
                mesh.positions.get(triangle[2] as usize),
            ) else {
                continue;
            };
            let normal = face_normal(*p0, *p1, *p2);
            for value in [normal, *p0, *p1, *p2] {
                for coordinate in value {
                    body.extend_from_slice(&coordinate.to_le_bytes());
                }
            }
            body.extend_from_slice(&0u16.to_le_bytes());
            count += 1;
        }
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
    }
    let mut header = [0u8; 80];
    let label = b"FerroCAD binary STL";
    header[..label.len()].copy_from_slice(label);
    let mut file = std::fs::File::create(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    file.write_all(&header)
        .and_then(|_| file.write_all(&count.to_le_bytes()))
        .and_then(|_| file.write_all(&body))
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;

    Ok(StlReport {
        path: path.to_path_buf(),
        triangles: count as usize,
    })
}

fn face_normal(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> [f32; 3] {
    let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
    let normal = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if length > 1e-12 {
        [normal[0] / length, normal[1] / length, normal[2] / length]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// The sample axonometric camera, moved out far enough to frame the whole stage.
///
/// This is the same fit the viewport's `frame_stage` performs, expressed on the
/// camera alone so a headless render needs no gpui context. Unlike the viewport's
/// origin-centred fit, this also shifts the pivot to the stage's bounding-box
/// centre: an opened `.FCStd` need not be modelled around the origin.
fn framed_camera(stage: &Stage, size: [u32; 2]) -> Camera {
    let mut camera = sample_camera(Viewport::new(size, 1.0));
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for item in stage.items() {
        let matrix = item.placement().to_matrix();
        for position in item.positions() {
            let world = matrix.transform(&Vector3::new(
                f64::from(position[0]),
                f64::from(position[1]),
                f64::from(position[2]),
            ));
            min.x = min.x.min(world.x);
            min.y = min.y.min(world.y);
            min.z = min.z.min(world.z);
            max.x = max.x.max(world.x);
            max.y = max.y.max(world.y);
            max.z = max.z.max(world.z);
        }
    }
    if !min.x.is_finite() || !max.x.is_finite() {
        return camera;
    }
    let centre = Vector3::new(
        (min.x + max.x) * 0.5,
        (min.y + max.y) * 0.5,
        (min.z + max.z) * 0.5,
    );
    let radius = stage.items().iter().fold(0.0_f64, |radius, item| {
        let matrix = item.placement().to_matrix();
        item.positions().iter().fold(radius, |radius, position| {
            let world = matrix.transform(&Vector3::new(
                f64::from(position[0]),
                f64::from(position[1]),
                f64::from(position[2]),
            ));
            radius.max(world.sub(&centre).length())
        })
    });
    if radius <= 1e-9 {
        return camera;
    }
    if let Projection::Perspective { fov_y, .. } = &mut camera.projection {
        let distance = radius / ((*fov_y) / 2.0).sin().max(1e-6) * 1.15;
        let base = Vector3::new(camera.view.m[3], camera.view.m[7], camera.view.m[11]);
        let current = base.length();
        if current > 1e-9 && distance > 1e-9 {
            // Set the eye distance while keeping the sample orientation, then shift
            // the pivot from the origin to the model's centre.
            let scaled = base.scale(distance / current);
            let shift = Rotation::from_matrix(&camera.view)
                .to_matrix()
                .transform(&centre);
            camera.view.m[3] = scaled.x - shift.x;
            camera.view.m[7] = scaled.y - shift.y;
            camera.view.m[11] = scaled.z - shift.z;
        }
    }
    camera.fit_clip();
    camera
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_mesh() -> StageMesh {
        StageMesh {
            entity: 1,
            object: "Triangle".to_string(),
            label: "Triangle".to_string(),
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: Vec::new(),
            origin: None,
            color: None,
            display: None,
        }
    }

    #[test]
    fn stl_export_writes_one_binary_facet() {
        let path = std::env::temp_dir().join("ferrocad_stl_export_test.stl");
        let _ = std::fs::remove_file(&path);
        let report = write_stl(&[triangle_mesh()], &path).expect("STL export");
        assert_eq!(report.triangles, 1);
        let bytes = std::fs::read(&path).expect("read the STL");
        assert_eq!(bytes.len(), 84 + 50, "binary STL header + one facet");
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()), 1);
        // The facet normal is +Z for the CCW triangle in the XY plane.
        let normal = [
            f32::from_le_bytes(bytes[84..88].try_into().unwrap()),
            f32::from_le_bytes(bytes[88..92].try_into().unwrap()),
            f32::from_le_bytes(bytes[92..96].try_into().unwrap()),
        ];
        assert!((normal[0]).abs() < 1e-6);
        assert!((normal[1]).abs() < 1e-6);
        assert!((normal[2] - 1.0).abs() < 1e-6, "normal was {normal:?}");
        let _ = std::fs::remove_file(&path);
    }
}

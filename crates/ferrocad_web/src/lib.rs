//! The minimal browser host: OCCT in wasm, rendered by wgpu on a canvas.
//!
//! `#[wasm_bindgen(start)]` runs on `init()`, finds `#cad`, builds a filleted box
//! with the real kernel, and draws it every animation frame. WebGL2 is the path
//! that must work (WebGPU support on Linux is still uneven); the instance asks for
//! `BROWSER_WEBGPU | GL` and wgpu's helper drops WebGPU when the browser cannot
//! provide an adapter.
//!
//! Everything is gated on `wasm32`: on other targets the crate is empty, so a
//! workspace `cargo check` does not need the browser APIs.

#![cfg(target_arch = "wasm32")]

mod wasi;

use std::{cell::RefCell, rc::Rc};

use ferrocad_geom::{ElementKind, GeomError, GeometryBackend, Tessellation, TessellationOptions};
use ferrocad_graphics::{Camera, Projection, RenderQueue, Viewport};
use ferrocad_occt::OcctBackend;
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_wgpu::{GpuContext, WgpuRenderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    fn __wasm_call_ctors();
}

#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    // wasm-bindgen 0.2.129's glue reads the `__instance_terminated` export.
    // The static is `#[no_mangle]` but `--gc-sections` drops it when nothing
    // references it, so keep it reachable from this exported entry point.
    unsafe { core::ptr::read_volatile(&raw const wasm_bindgen::__rt::__instance_terminated) };
    // Before anything that touches the kernel; also runs under Node, where there
    // is no canvas and `triangles()` is the only thing a test calls.
    unsafe { __wasm_call_ctors() };

    let Some(canvas) = canvas() else {
        return Ok(());
    };

    wasm_bindgen_futures::spawn_local(async move {
        match App::new(canvas).await {
            Ok(app) => request_frame(Rc::new(RefCell::new(app))),
            Err(error) => web_sys::console::error_1(&error),
        }
    });
    Ok(())
}

fn canvas() -> Option<web_sys::HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id("cad")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

struct App {
    surface: wgpu::Surface<'static>,
    /// The format a surface texture view is created with (sRGB, always).
    view_format: wgpu::TextureFormat,
    renderer: WgpuRenderer,
    camera: Camera,
    queue: RenderQueue,
}

impl App {
    async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsValue> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ferrocad web"),
                ..Default::default()
            })
            .await
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        let caps = surface.get_capabilities(&adapter);
        let (surface_format, view_format) = pick_formats(&caps.formats);
        let view_formats = if surface_format == view_format {
            Vec::new()
        } else {
            // WebGPU forbids sRGB canvas formats; it lets a view reinterpret the
            // non-sRGB texture as sRGB, which is what keeps the colours right.
            vec![view_format]
        };

        let size = [canvas.width().max(1), canvas.height().max(1)];
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size[0],
                height: size[1],
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: caps.alpha_modes[0],
                view_formats,
                desired_maximum_frame_latency: 2,
            },
        );

        let renderer = WgpuRenderer::with_sample_count_and_format(
            GpuContext::new(device, queue),
            size,
            // MSAA on WebGL is the one path the research note could not test;
            // one sample draws straight into the canvas view and always works.
            1,
            view_format,
        );
        let camera = Camera {
            view: Placement::new(
                Vector3::new(5.0, 5.0, -28.0),
                Rotation::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0), 0.5),
            )
            .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new(size, 1.0),
        };
        let mesh = filleted_box().map_err(|error| JsValue::from_str(&error.to_string()))?;
        let queue = Stage::new()
            .with_item(
                ShapeItem::from_tessellation(1, &mesh)
                    .with_draw_mode(DrawMode::ShadedWithEdges)
                    .with_surface_color([0.62, 0.66, 0.72, 1.0]),
            )
            .to_queue();

        Ok(Self {
            surface,
            view_format,
            renderer,
            camera,
            queue,
        })
    }

    fn frame(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Timeout/Occluded/Outdated/Lost/Validation: skip this frame.
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        if self
            .renderer
            .render(&view, &self.camera, &self.queue.packets)
            .is_ok()
        {
            frame.present();
        }
    }
}

/// The triangle count of a box filleted on one edge, for the smoke test and for
/// hosts that want to check the kernel before creating a canvas.
#[wasm_bindgen]
pub fn triangles() -> u32 {
    filleted_box()
        .map(|mesh| (mesh.triangles.len() / 3) as u32)
        .unwrap_or(0)
}

fn filleted_box() -> Result<Tessellation, GeomError> {
    let backend = OcctBackend::new();
    let shape = backend.make_box(10.0, 10.0, 10.0)?;
    let edges = backend.elements(&shape, ElementKind::Edge);
    let edge = edges.first().ok_or("the box has no edges")?;
    let filleted = backend.fillet(&shape, std::slice::from_ref(edge), 1.0)?;
    backend.tessellate(&filleted.shape, &TessellationOptions::absolute(0.2))
}

/// Pick the surface and view formats.
///
/// A WebGL surface lists the sRGB format, so configure it directly. A WebGPU
/// surface only allows `bgra8unorm`/`rgba8unorm`/`rgba16float`; configure the
/// non-sRGB counterpart and create an sRGB view, which the renderer's pipelines
/// are built for.
fn pick_formats(formats: &[wgpu::TextureFormat]) -> (wgpu::TextureFormat, wgpu::TextureFormat) {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8Unorm, Rgba8UnormSrgb};
    if formats.contains(&Rgba8UnormSrgb) {
        return (Rgba8UnormSrgb, Rgba8UnormSrgb);
    }
    let base = formats
        .iter()
        .copied()
        .find(|format| matches!(format, Rgba8Unorm | Bgra8Unorm))
        .unwrap_or(Rgba8Unorm);
    let srgb = match base {
        Bgra8Unorm => Bgra8UnormSrgb,
        _ => Rgba8UnormSrgb,
    };
    (base, srgb)
}

fn request_frame(app: Rc<RefCell<App>>) {
    let next = app.clone();
    let callback = Closure::once_into_js(move || {
        app.borrow_mut().frame();
        request_frame(next);
    });
    web_sys::window()
        .expect("window")
        .request_animation_frame(callback.unchecked_ref())
        .expect("requestAnimationFrame");
}

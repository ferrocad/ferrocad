//! The browser viewport: a canvas surface, a wgpu device, the render call and
//! pointer/wheel navigation.
//!
//! The wasm analogue of `ferrocad_viewport`, which is bound to GPUI. Like the
//! renderer it sits on, it owns no scene: the host supplies a [`Camera`] pose and the
//! [`DrawPacket`]s, and this crate binds orbit and zoom to the canvas.
//!
//! WebGL2 is the path that must work (WebGPU support on Linux is still uneven), so
//! the instance asks for `BROWSER_WEBGPU | GL` and wgpu's helper drops WebGPU when
//! the browser cannot provide an adapter.
//!
//! Everything is gated on `wasm32`: on other targets the crate is empty, so a
//! workspace `cargo check` does not need the browser APIs.

#![cfg(target_arch = "wasm32")]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use ferrocad_graphics::{Camera, DrawPacket, Viewport};
use ferrocad_wgpu::{GpuContext, WgpuRenderer};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{prelude::*, JsCast};

/// One wheel line zooms by this factor — the rate `ferrocad_viewport` uses.
const ZOOM_PER_LINE: f64 = 1.15;

/// A wheel event delivers lines or pixels; a mouse notch is about this many pixels.
const PIXELS_PER_LINE: f32 = 40.0;

/// A canvas surface, its device and renderer, and the input that drives the camera.
pub struct WebViewport {
    surface: wgpu::Surface<'static>,
    /// The format a surface texture view is created with (sRGB, always).
    view_format: wgpu::TextureFormat,
    size: [u32; 2],
    renderer: WgpuRenderer,
    /// Shared with the listeners below, which orbit/dolly it as the pointer moves.
    camera: Rc<RefCell<Camera>>,
    /// Registering a listener does not keep the closure alive; these do.
    _listeners: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

impl WebViewport {
    /// Create the surface and device for `canvas`, and bind orbit/zoom to it.
    ///
    /// `camera` supplies the view and projection. The canvas owns the size, so its
    /// `viewport` field is replaced with the canvas size at a scale factor of 1.
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        mut camera: Camera,
    ) -> Result<Self, JsValue> {
        // wgpu-core refuses `create_surface()` when neither the instance nor the
        // target carries a display handle, and on the web there is no display
        // connection to name — the canvas arrives in the window handle. wgpu-core is
        // the only context that can do WebGL2, which is the path Firefox (and any
        // browser without `navigator.gpu`) takes. The pure-JS WebGPU context ignores
        // the field.
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(WebDisplay));
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
                // The adapter's own limits, not `Limits::default()`: a WebGL2
                // adapter reports 0 for the compute limits it has no concept of,
                // and asking for 65535 fails validation.
                required_limits: adapter.limits(),
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
            // MSAA on WebGL is the one path that stays untested; one sample draws
            // straight into the canvas view and always works.
            1,
            view_format,
        );

        camera.viewport = Viewport::new(size, 1.0);
        let camera = Rc::new(RefCell::new(camera));
        let listeners = bind_input(&canvas, &camera)?;

        Ok(Self {
            surface,
            view_format,
            size,
            renderer,
            camera,
            _listeners: listeners,
        })
    }

    /// The camera the listeners orbit and dolly, and the one `draw` renders with.
    pub fn camera(&self) -> Rc<RefCell<Camera>> {
        Rc::clone(&self.camera)
    }

    /// The canvas size in device pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Draw `packets` into the next surface frame and present it.
    ///
    /// Returns whether a frame reached the screen: a lost, outdated or occluded
    /// surface is skipped rather than treated as an error, so a host can keep its
    /// animation loop running.
    pub fn draw(&mut self, packets: &[DrawPacket]) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Timeout/Occluded/Outdated/Lost/Validation: skip this frame.
            _ => return false,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        let camera = self.camera.borrow();
        if self.renderer.render(&view, &camera, packets).is_ok() {
            frame.present();
            true
        } else {
            false
        }
    }
}

/// The empty display handle `raw-window-handle` defines for the web.
///
/// Only its existence matters: wgpu-core rejects `create_surface()` when there is no
/// display handle at all (see [`WebViewport::new`]).
#[derive(Debug)]
struct WebDisplay;

impl wgpu::rwh::HasDisplayHandle for WebDisplay {
    fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
        Ok(wgpu::rwh::DisplayHandle::web())
    }
}

/// Bind pointer-drag orbit and wheel dolly on `canvas`.
///
/// The listeners are returned so the caller can keep them alive. Dragging with the
/// primary button orbits about the world origin (see [`Camera::orbit`]); a wheel
/// notch moves the eye along the view ray (see [`Camera::dolly`]), with the sign a
/// trackpad or mouse wheel is expected to have: scrolling down zooms out.
fn bind_input(
    canvas: &web_sys::HtmlCanvasElement,
    camera: &Rc<RefCell<Camera>>,
) -> Result<Vec<Closure<dyn FnMut(web_sys::Event)>>, JsValue> {
    // The pointer position at the previous move, while the primary button is down.
    let drag: Rc<Cell<Option<(f32, f32)>>> = Rc::new(Cell::new(None));
    let mut listeners = Vec::new();

    let down = {
        let drag = Rc::clone(&drag);
        // Pointer capture keeps the drag alive when the pointer leaves the canvas.
        let target = canvas.clone();
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Ok(pointer) = event.clone().dyn_into::<web_sys::PointerEvent>() else {
                return;
            };
            if pointer.button() != 0 {
                return;
            }
            event.prevent_default();
            drag.set(Some((pointer.client_x() as f32, pointer.client_y() as f32)));
            // A synthetic event (a test) has no active pointer to capture.
            let _ = target.set_pointer_capture(pointer.pointer_id());
        })
    };
    canvas.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    listeners.push(down);

    let moved = {
        let drag = Rc::clone(&drag);
        let camera = Rc::clone(camera);
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Ok(pointer) = event.clone().dyn_into::<web_sys::PointerEvent>() else {
                return;
            };
            let Some((last_x, last_y)) = drag.get() else {
                return;
            };
            let (x, y) = (pointer.client_x() as f32, pointer.client_y() as f32);
            drag.set(Some((x, y)));
            event.prevent_default();
            camera.borrow_mut().orbit([x - last_x, y - last_y]);
        })
    };
    canvas.add_event_listener_with_callback("pointermove", moved.as_ref().unchecked_ref())?;
    listeners.push(moved);

    let up = {
        let drag = Rc::clone(&drag);
        let target = canvas.clone();
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            drag.set(None);
            let Ok(pointer) = event.dyn_into::<web_sys::PointerEvent>() else {
                return;
            };
            let _ = target.release_pointer_capture(pointer.pointer_id());
        })
    };
    for event in ["pointerup", "pointercancel"] {
        canvas.add_event_listener_with_callback(event, up.as_ref().unchecked_ref())?;
    }
    listeners.push(up);

    let wheel = {
        let camera = Rc::clone(camera);
        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Ok(wheel) = event.clone().dyn_into::<web_sys::WheelEvent>() else {
                return;
            };
            // Not passive: the page must not scroll under the zoom.
            event.prevent_default();
            let lines = -(wheel.delta_y() as f32) / PIXELS_PER_LINE;
            camera
                .borrow_mut()
                .dolly(ZOOM_PER_LINE.powf(-f64::from(lines)));
        })
    };
    let options = web_sys::AddEventListenerOptions::new();
    options.set_passive(false);
    canvas.add_event_listener_with_callback_and_add_event_listener_options(
        "wheel",
        wheel.as_ref().unchecked_ref(),
        &options,
    )?;
    listeners.push(wheel);

    Ok(listeners)
}

/// Pick the surface and view formats.
///
/// A WebGL surface lists the sRGB format, so configure it directly. A WebGPU surface
/// only allows `bgra8unorm`/`rgba8unorm`/`rgba16float`; configure the non-sRGB
/// counterpart and create an sRGB view, which the renderer's pipelines are built for.
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

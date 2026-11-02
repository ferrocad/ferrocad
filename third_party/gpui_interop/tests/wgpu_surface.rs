#![cfg(all(target_os = "linux", feature = "wgpu-test"))]
//! Two wgpu hand-offs, both fed by the renderer's own GPU.
//!
//! - **Same-device view.** `Adapter::wgpu` hands a producer the **renderer's own** device and queue,
//!   the ordinary `wgpu` API renders into a [`WgpuSurface`], and `paint_texture` composites that
//!   view with no export, no fence and no copy.
//! - **Exported buffer.** `WgpuSharedBuffer` allocates an exportable, tiled image on the same wgpu
//!   device, wraps it back into wgpu for rendering, and `submit` exports the frame as a dma-buf with
//!   its modifier, plane layout and a `sync_file` fence. `paint_surface` imports that native handle;
//!   a second frame through the same buffer proves the semaphore is reusable.
//!
//! ```text
//! gpu.try_device::<WgpuHeadlessRenderer>()       // the window renderer's device and queue
//!     .adapter().wgpu()                          // cloned, not a second device
//! WgpuSurface::new(..) / WgpuSharedBuffer::new(..)
//! encoder + render pass + submit(..)             // an ordinary wgpu frame
//! paint_texture(..) / paint_surface(..)          // handed to the scene
//! capture_screenshot(..)                         // the pixel comes back
//! ```
//!
//! # Why it is opt-in
//!
//! The same reason as `linux_dmabuf_readback`: driving a window pulls in the facade's platform
//! bundle, and the test needs a real GPU. A machine with no adapter skips rather than fails, but
//! building it needs the windowing stack's system libraries, which the crate's ordinary `cargo test`
//! deliberately does not.
//!
//! On a multi-GPU machine, `GPUI_TEST_DEVICE_ID` selects the adapter by PCI device id (four
//! hexadecimal digits) through `GpuPreference::Device`, and the test prints the adapter it landed on.
//! The variable is read here, not lowered from the platform's `ZED_DEVICE_ID`, so the choice is part
//! of the test. The wgpu context is process-wide, so both tests in one run use the same adapter.
//!
//! ```sh
//! cargo test -p gpui_interop --features wgpu-test --test wgpu_surface
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyWindowHandle, AppContext as _, Context, Corners, IntoElement, Render, Window, div,
    gpu_canvas, prelude::*, px, size,
};
use gpui_interop::{WgpuSharedBuffer, WgpuSurface, attach};
use gpui_wgpu::{CosmicTextSystem, ImportedTextureExt as _, WgpuHeadlessRenderer};

/// The edge of the produced view and of the window, in pixels.
const EDGE: u32 = 64;

/// The render pass clears to opaque red. Both ends are sRGB, so `255, 0, 0` is the byte a correct
/// round trip stores: the pass encodes, the renderer's fragment decodes, and its sRGB target
/// encodes again.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 1.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// What the read-back must contain.
const EXPECTED: [u8; 4] = [255, 0, 0, 255];

/// The GPU this run asked for, e.g. `GPUI_TEST_DEVICE_ID=1346`, or `Auto`.
///
/// The test names the `GpuPreference` the headless entry point takes rather than lowering
/// `ZED_DEVICE_ID`, so the selection is part of the test and a Cargo invocation carries no device
/// variable. The adapter's reported name is printed below, so a run says which GPU it used.
fn gpu_preference() -> gpui::GpuPreference {
    std::env::var("GPUI_TEST_DEVICE_ID")
        .ok()
        .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok())
        .map(|device_id| gpui::GpuPreference::Device(gpui::GpuId::by_device(device_id)))
        .unwrap_or(gpui::GpuPreference::Auto)
}

/// The producer's half: the renderer's device and queue, and the surface they render into.
struct Producer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: WgpuSurface,
}

/// The window's view: one canvas covering it. The producer is built at paint time — the renderer's
/// device exists only then — so the slot is shared and filled on the first paint.
struct View {
    producer: Rc<RefCell<Option<Producer>>>,
    /// Whether the bridge built at all, so the test can tell a skip from a wrong pixel.
    built: Rc<Cell<bool>>,
    /// Whether `Adapter::wgpu` returned the renderer's own device and queue rather than another.
    same_device: Rc<Cell<bool>>,
    /// The adapter the renderer landed on, for the test's output.
    gpu: Rc<RefCell<Option<String>>>,
}

impl Render for View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let producer = self.producer.clone();
        let built = self.built.clone();
        let same_device = self.same_device.clone();
        let gpu_name = self.gpu.clone();
        div().size_full().child(
            gpu_canvas(move |gpu| {
                let mut slot = producer.borrow_mut();
                if slot.is_none() {
                    let Some(lent) = gpu.try_device::<WgpuHeadlessRenderer>() else {
                        return;
                    };
                    // The producer sees the canvas's device through `attach`, exactly as an
                    // application would. The test already holds it, so it can compare identities.
                    let Ok(interop) = attach(Some(lent.clone())) else {
                        return;
                    };
                    let Some((device, queue)) = interop.adapter().wgpu() else {
                        return;
                    };
                    same_device.set(Arc::ptr_eq(&device, &lent.0) && Arc::ptr_eq(&queue, &lent.1));
                    if let Some(id) = interop.adapter().gpu_id() {
                        *gpu_name.borrow_mut() = Some(id.name);
                    }
                    let Ok(surface) = WgpuSurface::new(&device, EDGE, EDGE) else {
                        return;
                    };
                    *slot = Some(Producer {
                        device,
                        queue,
                        surface,
                    });
                    built.set(true);
                }

                let producer = slot.as_mut().expect("built above");
                // A canvas is rebuilt each frame; a same-size surface must not be reallocated.
                let resized = producer
                    .surface
                    .resize(&producer.device, EDGE, EDGE)
                    .expect("a same-size resize");
                assert!(!resized, "a same-size resize should be a no-op");

                let mut encoder =
                    producer
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("gpui_interop_wgpu_surface_test"),
                        });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("clear the produced view"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: producer.surface.view(),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(CLEAR),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                producer.queue.submit(Some(encoder.finish()));

                let handle = producer
                    .surface
                    .view()
                    .to_imported_handle()
                    .expect("the surface is an sRGB, bindable view");
                gpu.paint_texture(handle, Corners::default(), 1.0, false);
            })
            .size_full(),
        )
    }
}

/// A wgpu frame rendered on the renderer's own device composites byte for byte.
#[test]
fn a_wgpu_frame_composites_on_the_renderers_own_device() {
    let preference = gpu_preference();
    if !gpui::current_headless_renderer_with(preference.clone())
        .is_ok_and(|renderer| renderer.is_some())
    {
        eprintln!("skipping: no headless renderer (no Vulkan adapter)");
        return;
    }

    let text_system = Arc::new(CosmicTextSystem::new("fallback"));
    let factory = preference;
    let mut cx = gpui::HeadlessAppContext::with_platform(text_system, Arc::new(()), move || {
        gpui::current_headless_renderer_with(factory.clone())
    });

    let built = Rc::new(Cell::new(false));
    let same_device = Rc::new(Cell::new(false));
    let gpu = Rc::new(RefCell::new(None));
    let window = cx
        .open_window(size(px(EDGE as f32), px(EDGE as f32)), |_window, cx| {
            cx.new(|_| View {
                producer: Rc::new(RefCell::new(None)),
                built: built.clone(),
                same_device: same_device.clone(),
                gpu: gpu.clone(),
            })
        })
        .expect("a window to composite into");
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .expect("draw a frame");

    if !built.get() {
        eprintln!("skipping: the renderer lent no wgpu device");
        return;
    }

    eprintln!(
        "using {}",
        gpu.borrow().as_deref().unwrap_or("an unnamed adapter")
    );

    assert!(
        same_device.get(),
        "Adapter::wgpu should return the renderer's own device and queue, not a second pair"
    );

    let image = cx.capture_screenshot(window).expect("read the frame back");
    assert_eq!(
        image.get_pixel(EDGE / 2, EDGE / 2).0,
        EXPECTED,
        "the wgpu-rendered surface did not composite through the same-device path"
    );
}

/// The producer's half of the exported path: the renderer's wgpu device and an image it allocated
/// with the external-memory flags an export needs.
struct SharedProducer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    buffer: WgpuSharedBuffer,
}

/// The window's view for the exported path.
struct SharedView {
    producer: Rc<RefCell<Option<SharedProducer>>>,
    /// Whether the bridge built at all, so the test can tell a skip from a wrong pixel.
    built: Rc<Cell<bool>>,
    /// Why the export could not be built, when it could not.
    error: Rc<RefCell<Option<String>>>,
    /// Whether the exported image was tiled rather than linear.
    tiled: Rc<Cell<bool>>,
    /// The colour the next frame clears to, in linear light.
    clear: Rc<Cell<[f32; 4]>>,
    /// The adapter the renderer landed on, for the test's output.
    gpu: Rc<RefCell<Option<String>>>,
}

impl Render for SharedView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let producer = self.producer.clone();
        let built = self.built.clone();
        let error = self.error.clone();
        let tiled = self.tiled.clone();
        let clear = self.clear.clone();
        let gpu_name = self.gpu.clone();
        div().size_full().child(
            gpu_canvas(move |gpu| {
                let mut slot = producer.borrow_mut();
                if slot.is_none() {
                    let Some(lent) = gpu.try_device::<WgpuHeadlessRenderer>() else {
                        return;
                    };
                    let Ok(interop) = attach(Some(lent)) else {
                        return;
                    };
                    let Some((device, queue)) = interop.adapter().wgpu() else {
                        return;
                    };
                    if let Some(id) = interop.adapter().gpu_id() {
                        *gpu_name.borrow_mut() = Some(id.name);
                    }
                    match WgpuSharedBuffer::new(
                        &device,
                        EDGE,
                        EDGE,
                        wgpu::TextureFormat::Rgba8UnormSrgb,
                    ) {
                        Ok(buffer) => {
                            *slot = Some(SharedProducer {
                                device,
                                queue,
                                buffer,
                            });
                            built.set(true);
                        }
                        Err(build) => {
                            *error.borrow_mut() = Some(build.to_string());
                            return;
                        }
                    }
                }

                let producer = slot.as_mut().expect("built above");
                let [r, g, b, a] = clear.get();
                let mut encoder =
                    producer
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("gpui_interop_wgpu_shared_buffer_test"),
                        });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("clear the exported view"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: producer.buffer.view(),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: f64::from(r),
                                    g: f64::from(g),
                                    b: f64::from(b),
                                    a: f64::from(a),
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                // The submission that rendered the frame also signals the exported fence.
                let handle = producer
                    .buffer
                    .submit(&producer.queue, Some(encoder.finish()))
                    .expect("an acquire fence");
                assert!(
                    handle.acquire_fence.is_some(),
                    "a submitted frame should carry an acquire fence"
                );
                tiled.set(handle.modifier != 0);
                gpu.paint_surface(handle);
            })
            .size_full(),
        )
    }
}

/// A wgpu frame rendered into an exportable image composites through the native dma-buf path.
#[test]
fn an_exported_wgpu_frame_composites_as_a_dmabuf() {
    let preference = gpu_preference();
    if !gpui::current_headless_renderer_with(preference.clone())
        .is_ok_and(|renderer| renderer.is_some())
    {
        eprintln!("skipping: no headless renderer (no Vulkan adapter)");
        return;
    }

    let text_system = Arc::new(CosmicTextSystem::new("fallback"));
    let factory = preference;
    let mut cx = gpui::HeadlessAppContext::with_platform(text_system, Arc::new(()), move || {
        gpui::current_headless_renderer_with(factory.clone())
    });

    let built = Rc::new(Cell::new(false));
    let error = Rc::new(RefCell::new(None));
    let tiled = Rc::new(Cell::new(false));
    let clear = Rc::new(Cell::new([1.0, 0.0, 0.0, 1.0]));
    let gpu = Rc::new(RefCell::new(None));
    let window = cx
        .open_window(size(px(EDGE as f32), px(EDGE as f32)), |_window, cx| {
            cx.new(|_| SharedView {
                producer: Rc::new(RefCell::new(None)),
                built: built.clone(),
                error: error.clone(),
                tiled: tiled.clone(),
                clear: clear.clone(),
                gpu: gpu.clone(),
            })
        })
        .expect("a window to composite into");
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .expect("draw a frame");

    if !built.get() {
        eprintln!(
            "skipping: the renderer's wgpu device cannot export a dma-buf: {}",
            error.borrow().as_deref().unwrap_or("no device"),
        );
        return;
    }

    eprintln!(
        "using {}",
        gpu.borrow().as_deref().unwrap_or("an unnamed adapter")
    );

    assert!(
        tiled.get(),
        "the exported image should be on a DRM modifier, not LINEAR"
    );

    let image = cx.capture_screenshot(window).expect("read the frame back");
    assert_eq!(
        image.get_pixel(EDGE / 2, EDGE / 2).0,
        EXPECTED,
        "the exported wgpu frame did not composite through the dma-buf path"
    );

    // A second frame through the same buffer: the export consumes the semaphore, and the next
    // submission signals it again — the fence is reusable, not one-shot.
    clear.set([0.0, 1.0, 0.0, 1.0]);
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .expect("draw a second frame");
    let image = cx
        .capture_screenshot(window)
        .expect("read the second frame back");
    assert_eq!(
        image.get_pixel(EDGE / 2, EDGE / 2).0,
        [0, 255, 0, 255],
        "the second exported frame did not composite"
    );
}

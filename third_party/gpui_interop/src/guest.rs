//! The guest runner: headless GPUI on a worker thread, handing a frame to a foreign loop.
//!
//! Guest Mode is the supported way to put a GPUI view inside a host that owns its own window and
//! its own cadence — an embedded player, a plugin host, a game that draws UI with GPUI but
//! presents through another API. There is deliberately no `run_embedded` on a desktop platform:
//! the windowing layers fuse the message loop, presentation pacing and the executors into one
//! runtime, and a host loop would fight all three. The shape that works instead is a **headless**
//! GPUI context on a worker thread:
//!
//! - the host owns the window, the input, and when a frame is wanted;
//! - the guest thread builds a headless [`Application`], opens one offscreen window, and runs its
//!   executors and its view tree normally;
//! - each frame crosses as a **shareable surface** — `SceneRenderer::offscreen_surface_handle`,
//!   the `Arc<dyn Any + Send + Sync>` the [P7 probe] measured — rather than as pixels, so the host
//!   samples it on the same device with no CPU copy. A host that wants bytes can ask its backend
//!   for them, but the runner does not read back.
//!
//! The renderer itself never crosses. It is `!Send` by construction, and the runner is built so
//! that this stays true: only the frame handle, the commands, and plain data travel over channels.
//!
//! # The host's loop
//!
//! ```rust,ignore
//! let mut guest = Guest::spawn(
//!     GuestOptions { size: size(px(1280.), px(720.)) },
//!     || Ok(Box::new(WgpuHeadlessRenderer::new()?) as Box<dyn PlatformRenderer>),
//!     |_window, cx| cx.new(|_| MyView),
//! )?;
//!
//! // Once per host frame:
//! let frame = guest.request_frame(None)?;
//! let surface = frame.surface().downcast_ref::<WgpuOffscreenSurface>();
//! // sample `surface.texture()` on the same wgpu device…
//! ```
//!
//! `request_frame` is host-paced: nothing is drawn until the host asks, and the guest's own
//! animations and async work simply mark the window dirty until the next ask. [`Guest::update`]
//! posts work back to the guest thread — the primitive a host builds input translation on, without
//! the runner having to know a platform's input types.
//!
//! # What produces a frame
//!
//! The guest's renderer has to be one the window can install, so [`Guest::spawn`] takes a factory
//! that builds a [`PlatformRenderer`] — on Linux that is the wgpu headless renderer, on another
//! platform whatever its offscreen renderer is. The runner wraps it in a small tap that forwards
//! `draw` and reports the surface after each presented frame. A renderer that does not expose one
//! is not a failure of the thread: `request_frame` says the renderer produced no shareable
//! surface, which is the honest answer and the reason the CPU path stays a separate choice.
//!
//! [P7 probe]: https://github.com/bite-gpui/bite-gpui-project/blob/main/decisions/guest-thread-probe.md

use std::any::Any;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use anyhow::{Context as _, anyhow, bail};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{
    AnyWindowHandle, App, Application, AsyncApp, Bounds, DevicePixels, Entity, GpuSpecs,
    PixelBuffer, Pixels, PlatformAtlas, PlatformRenderer, Point, Render, RendererFactory,
    RendererTarget, Scene, SceneRenderer, Size, Window, WindowBounds, WindowOptions,
};

/// The window a guest opens: its content size, in logical pixels.
///
/// The headless display reports a scale factor of one, so these are also the frame's device
/// pixels. A host that needs a different size passes it to [`Guest::request_frame`], in the same
/// logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct GuestOptions {
    /// The initial content size.
    pub size: Size<Pixels>,
}

/// One frame, handed from the guest thread to the host loop.
///
/// The surface is the renderer's own opaque handle (`WgpuOffscreenSurface` for the wgpu backend),
/// which the host downcasts to the type its backend publishes. This value keeps the target alive for
/// as long as the host holds it, but a later frame of the same size renders into the same target: a
/// host samples this frame before asking for the next one.
pub struct GuestFrame {
    surface: Arc<dyn Any + Send + Sync>,
    size: Size<DevicePixels>,
}

impl GuestFrame {
    /// The frame's surface, as the renderer handed it over.
    pub fn surface(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.surface
    }

    /// The frame's size, in device pixels.
    pub fn size(&self) -> Size<DevicePixels> {
        self.size
    }
}

/// Work the host posts to the guest thread.
type GuestUpdate = Box<dyn FnOnce(&mut Window, &mut App) + Send>;

/// Work the host posts back to the guest thread.
enum GuestCommand {
    /// Draw and present a frame, optionally resizing the window first.
    Present { size: Option<Size<Pixels>> },
    /// Run `update` against the window and app on the guest thread.
    Update(GuestUpdate),
    /// Stop the guest's run loop and let its thread end.
    Shutdown,
}

/// A headless GPUI context running on its own thread, driven by the host.
///
/// The runner owns the thread and the channels; dropping it shuts the guest down. The rendering
/// context and its executors never leave that thread — only [`GuestFrame`] and plain commands
/// cross — which is what keeps the `!Send` renderer boundary intact.
pub struct Guest {
    commands: UnboundedSender<GuestCommand>,
    frames: Receiver<Option<GuestFrame>>,
    thread: Option<JoinHandle<()>>,
}

impl Guest {
    /// Start a guest.
    ///
    /// `renderer` builds the window's renderer on the guest thread, once the headless platform
    /// has opened the window; it is where a backend's GPU preference is applied. `build_root`
    /// creates the window's root view on the guest thread too, so neither value has to be `Send`.
    ///
    /// Returns once the window exists and the command loop is scheduled, so a failure to build
    /// the renderer or the view is reported here rather than as a hang on the first frame.
    pub fn spawn<V, B, R>(options: GuestOptions, renderer: R, build_root: B) -> anyhow::Result<Self>
    where
        V: Render + 'static,
        B: FnOnce(&mut Window, &mut App) -> Entity<V> + Send + 'static,
        R: Fn() -> anyhow::Result<Box<dyn PlatformRenderer>> + Send + 'static,
    {
        let (commands, command_queue) = unbounded();
        let (frame_sender, frames) = channel();
        let (ready_sender, ready) = channel();

        let thread = thread::Builder::new()
            .name("gpui-guest".to_owned())
            .spawn(move || {
                let application: Application = gpui::headless();
                application.run(move |cx| {
                    let started = open_guest_window(
                        cx,
                        options,
                        renderer,
                        build_root,
                        command_queue,
                        frame_sender,
                    );
                    let failed = started.is_err();
                    let _ = ready_sender.send(started.map_err(|error| format!("{error:#}")));
                    if failed {
                        // Nothing will drive the loop, so stop it before it starts; `spawn` has
                        // already been told why.
                        cx.quit();
                    }
                });
            })
            .context("could not spawn the guest thread")?;

        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                frames,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                thread.join().ok();
                bail!("the guest could not start: {error}")
            }
            Err(_) => {
                thread.join().ok();
                bail!("the guest thread stopped before it was ready")
            }
        }
    }

    /// Draw and present one frame, waiting for it.
    ///
    /// `size` resizes the window first, which is how a host follows its own surface. Returns the
    /// renderer's shareable surface; a renderer that does not expose one is an error rather than
    /// an empty frame, so a CPU-only backend fails where the host asks for the GPU path.
    pub fn request_frame(&mut self, size: Option<Size<Pixels>>) -> anyhow::Result<GuestFrame> {
        // A frame from an earlier draw would otherwise be mistaken for this one.
        while self.frames.try_recv().is_ok() {}
        self.commands
            .unbounded_send(GuestCommand::Present { size })
            .map_err(|_| anyhow!("the guest thread has stopped"))?;
        match self.frames.recv() {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => bail!("the guest's renderer did not produce a shareable surface"),
            Err(_) => bail!("the guest thread stopped before presenting a frame"),
        }
    }

    /// Run `update` against the guest's window on the guest thread.
    ///
    /// Commands are processed in order, so work posted before a [`request_frame`](Self::request_frame)
    /// is visible in that frame. This is the primitive a host uses to feed input, mutate
    /// application state, or schedule guest-side work; the closure runs on the guest thread with
    /// that thread's `Window`, so nothing it names has to cross a thread boundary.
    pub fn update(
        &self,
        update: impl FnOnce(&mut Window, &mut App) + Send + 'static,
    ) -> anyhow::Result<()> {
        self.commands
            .unbounded_send(GuestCommand::Update(Box::new(update)))
            .map_err(|_| anyhow!("the guest thread has stopped"))
    }

    /// Stop the guest's loop and join its thread.
    ///
    /// Calling this is optional: dropping the runner does the same. It exists so a host can
    /// surface a panic in the guest as an error.
    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let _ = self.commands.unbounded_send(GuestCommand::Shutdown);
        thread
            .join()
            .map_err(|_| anyhow!("the guest thread panicked"))?;
        Ok(())
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Open the guest's window and schedule its command loop, on the guest thread.
fn open_guest_window<V, B, R>(
    cx: &mut App,
    options: GuestOptions,
    renderer: R,
    build_root: B,
    commands: UnboundedReceiver<GuestCommand>,
    frames: Sender<Option<GuestFrame>>,
) -> anyhow::Result<()>
where
    V: Render + 'static,
    B: FnOnce(&mut Window, &mut App) -> Entity<V>,
    R: Fn() -> anyhow::Result<Box<dyn PlatformRenderer>> + 'static,
{
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: options.size,
            })),
            focus: false,
            show: false,
            ..WindowOptions::default()
        }
        .with_renderer_factory(GuestRendererFactory { renderer, frames }),
        build_root,
    )?;

    let window: AnyWindowHandle = window.into();
    cx.spawn(async move |cx| guest_loop(cx, window, commands).await)
        .detach();
    Ok(())
}

/// The guest's command loop, on the guest thread's foreground executor.
async fn guest_loop(
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    mut commands: UnboundedReceiver<GuestCommand>,
) {
    while let Some(command) = commands.next().await {
        match command {
            GuestCommand::Present { size } => {
                let presented = window.update(cx, |_, window, cx| {
                    if let Some(size) = size {
                        window.resize(size);
                        // The headless window has no platform frame callback to notice the
                        // resize, so the runner marks the frame it owes itself.
                        window.refresh();
                    }
                    window.draw(cx).clear(cx);
                    // This is what reaches the renderer; `draw` only builds the frame.
                    window.present();
                });
                if presented.is_err() {
                    break;
                }
            }
            GuestCommand::Update(update) => {
                if window
                    .update(cx, |_, window, cx| update(window, cx))
                    .is_err()
                {
                    break;
                }
            }
            GuestCommand::Shutdown => {
                cx.update(|cx| cx.quit());
                return;
            }
        }
    }
}

/// The factory a guest window installs: the caller's renderer, wrapped in a tap.
struct GuestRendererFactory<R> {
    renderer: R,
    frames: Sender<Option<GuestFrame>>,
}

impl<R> RendererFactory for GuestRendererFactory<R>
where
    R: Fn() -> anyhow::Result<Box<dyn PlatformRenderer>> + 'static,
{
    fn create(&self, target: RendererTarget<'_>) -> anyhow::Result<Box<dyn PlatformRenderer>> {
        Ok(Box::new(FrameTap {
            inner: (self.renderer)()?,
            frames: self.frames.clone(),
            size: target.size.to_device_pixels(target.scale_factor),
        }))
    }
}

/// A renderer that forwards everything and reports the surface after each presented frame.
///
/// It sits between the window and the backend so the runner does not have to know the backend's
/// type: `SceneRenderer` already exposes the shareable handle, so observing `draw` is enough, and
/// nothing about the tap is specific to wgpu (or to GPUI's engine).
struct FrameTap {
    inner: Box<dyn PlatformRenderer>,
    frames: Sender<Option<GuestFrame>>,
    size: Size<DevicePixels>,
}

impl SceneRenderer for FrameTap {
    fn draw(&mut self, scene: &Scene) -> bool {
        let presented = self.inner.draw(scene);
        // `None` is reported too: the host asked for a frame and the renderer has no shareable
        // one, which is a fact the host must see rather than a frame it waits for forever.
        let _ = self.frames.send(
            self.inner
                .offscreen_surface_handle()
                .map(|surface| GuestFrame {
                    surface,
                    size: self.size,
                }),
        );
        presented
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.inner.sprite_atlas()
    }

    fn set_viewport_size(&mut self, size: Size<DevicePixels>) {
        self.size = size;
        self.inner.set_viewport_size(size);
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.size = size;
        self.inner.render_scene(scene, size)
    }

    fn read_pixels(&mut self) -> anyhow::Result<PixelBuffer> {
        self.inner.read_pixels()
    }

    fn offscreen_surface_handle(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.inner.offscreen_surface_handle()
    }
}

// A window's renderer must also answer the platform's own hooks; the tap forwards those too, so
// the runner is not Linux-shaped. macOS and Windows add a supertrait where they have one.
#[cfg(target_os = "macos")]
impl gpui::MacSceneRenderer for FrameTap {
    fn layer_ptr(&self) -> *mut std::ffi::c_void {
        self.inner.layer_ptr()
    }

    fn set_presents_with_transaction(&mut self, value: bool) {
        self.inner.set_presents_with_transaction(value);
    }
}

#[cfg(target_os = "windows")]
impl gpui::WinSceneRenderer for FrameTap {
    fn set_background_appearance(&mut self, appearance: gpui::WindowBackgroundAppearance) {
        self.inner.set_background_appearance(appearance);
    }

    fn resize(&mut self, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.inner.resize(size)
    }

    fn mark_drawable(&mut self) {
        self.inner.mark_drawable();
    }
}

impl PlatformRenderer for FrameTap {
    fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.size = size;
        self.inner.update_drawable_size(size);
    }

    fn max_texture_size(&self) -> u32 {
        self.inner.max_texture_size()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.inner.gpu_specs()
    }

    fn set_subpixel_layout(&mut self, is_bgr: bool) {
        self.inner.set_subpixel_layout(is_bgr);
    }

    fn update_transparency(&mut self, transparent: bool) {
        self.inner.update_transparency(transparent);
    }

    fn destroy(&mut self) {
        self.inner.destroy();
    }

    fn device_lost(&self) -> bool {
        self.inner.device_lost()
    }

    fn needs_redraw(&mut self) -> bool {
        self.inner.needs_redraw()
    }

    fn recover(&mut self, target: RendererTarget<'_>) -> anyhow::Result<()> {
        self.inner.recover(target)
    }
}

#![cfg(all(target_os = "linux", feature = "guest-test"))]
//! The guest runner, end to end: a worker thread's headless GPUI app hands a frame to this thread.
//!
//! ```text
//! Guest::spawn(options, || WgpuHeadlessRenderer::with_gpu_preference(..), |_, cx| ..)   // guest
//! guest.request_frame(None)                        // the host asks; the guest draws and presents
//! frame.surface().downcast::<WgpuOffscreenSurface>() // the zero-copy handle crosses
//! // ...this thread copies the guest's texture on the same device and reads a pixel back
//! ```
//!
//! The second request resizes the window, so the row proves the host's cadence shapes the frame
//! rather than the platform's: nothing draws until asked, and a different size arrives as a
//! different target. The work-back channel is asserted in order too, since an update posted before
//! a request must be visible in that frame.
//!
//! The runner is renderer-agnostic, so the factory here is the wgpu headless renderer. A renderer
//! that exposes no shareable surface is refused rather than silently yielding an empty frame; that
//! row needs no GPU at all.
//!
//! ```sh
//! cargo test -p gpui_interop --features guest-test --test guest_runner
//! ```
//!
//! Both rows are host-paced and the guest is joined before the test returns. Pin a GPU with
//! `ZED_DEVICE_ID=1916` / `1346`; the guest and the sampling renderer then share that device.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    AppContext as _, Context, DevicePixels, GpuId, GpuPreference, GpuRenderer, GpuSpecs,
    HeadlessAtlas, IntoElement, PlatformAtlas, PlatformRenderer, Render, Scene, SceneRenderer,
    Size, Window, div, prelude::*, px, size,
};
use gpui_interop::{Guest, GuestOptions};
// `wgpu` at the version the backend links against, not a second copy of the crate.
use gpui_wgpu::wgpu;
use gpui_wgpu::{WgpuHeadlessRenderer, WgpuOffscreenSurface};

/// The colour the view paints and the frame must carry — the fixture solid quad's red.
const RED: [u8; 4] = [255, 0, 0, 255];

/// A plain view that fills its window with red.
struct View;

impl Render for View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(gpui::red())
    }
}

/// A renderer on the guest's GPU, honouring `ZED_DEVICE_ID` the way `gpui::current_headless_renderer`
/// does for the core's rows.
fn renderer() -> anyhow::Result<Box<dyn PlatformRenderer>> {
    let preference = match std::env::var("ZED_DEVICE_ID") {
        Ok(value) => match GpuId::parse_pci_id(&value) {
            Some(device_id) => GpuPreference::Device(GpuId::by_device(device_id)),
            None => anyhow::bail!(
                "ZED_DEVICE_ID: expected a four-digit hexadecimal PCI id, got {value:?}"
            ),
        },
        Err(std::env::VarError::NotPresent) => GpuPreference::Auto,
        Err(error) => anyhow::bail!("ZED_DEVICE_ID: could not be read: {error}"),
    };
    Ok(Box::new(WgpuHeadlessRenderer::with_gpu_preference(
        preference,
    )?))
}

#[test]
fn a_guest_hands_each_requested_frame_to_the_host() -> anyhow::Result<()> {
    let first = size(px(64.0), px(64.0));
    let mut guest = match Guest::spawn(GuestOptions { size: first }, renderer, |_window, cx| {
        cx.new(|_| View)
    }) {
        Ok(guest) => guest,
        Err(error) if error.to_string().contains("No usable headless GPU adapter") => {
            eprintln!("skipping: no usable headless GPU adapter");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    // The host's own work crosses to the guest thread, and commands keep their order: a flip
    // posted before the request is visible by the time the frame comes back.
    let reached = Arc::new(AtomicBool::new(false));
    let flag = reached.clone();
    guest.update(move |_window, _cx| flag.store(true, Ordering::SeqCst))?;

    let frame = guest.request_frame(None)?;
    assert!(
        reached.load(Ordering::SeqCst),
        "the update should run on the guest thread before the frame it precedes"
    );
    assert_eq!(frame.size(), device_size(64, 64));

    // The host reaches the guest's device through a second headless renderer: the context is
    // process-wide, so this is another handle on it, not a second device.
    let host = WgpuHeadlessRenderer::new()?;
    let (device, queue) = host.device().expect("the host renderer has a device");

    let surface = frame
        .surface()
        .downcast_ref::<WgpuOffscreenSurface>()
        .expect("the handle is the wgpu backend's surface");
    assert_eq!(surface.size(), device_size(64, 64));
    let center = read_pixel(&device, &queue, surface.texture(), 32, 32)?;
    assert_eq!(center, RED, "the host samples the guest's frame");

    // The host moves the window; the next frame is rendered at the size it asked for.
    let second = size(px(32.0), px(48.0));
    let frame = guest.request_frame(Some(second))?;
    assert_eq!(frame.size(), device_size(32, 48));
    let surface = frame
        .surface()
        .downcast_ref::<WgpuOffscreenSurface>()
        .expect("the handle is the wgpu backend's surface");
    assert_eq!(surface.size(), device_size(32, 48));
    let center = read_pixel(&device, &queue, surface.texture(), 16, 24)?;
    assert_eq!(center, RED, "the resized frame carries the view");

    guest.shutdown()?;
    Ok(())
}

#[test]
fn a_renderer_without_a_surface_is_a_refusal_not_a_hang() -> anyhow::Result<()> {
    let mut guest = Guest::spawn(
        GuestOptions {
            size: size(px(16.0), px(16.0)),
        },
        || Ok(Box::new(DiscardRenderer::default())),
        |_window, cx| cx.new(|_| View),
    )?;

    let error = match guest.request_frame(None) {
        Ok(_) => panic!("a discard renderer produced a frame out of nothing"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("shareable surface"),
        "the refusal should name what is missing: {error}"
    );

    guest.shutdown()?;
    Ok(())
}

/// A renderer with no device and no target: it records that it was asked to draw and answers the
/// window contract honestly. It stands in for a backend whose offscreen path has no GPU surface to
/// hand over, which is the case the runner must report rather than wait on.
#[derive(Default)]
struct DiscardRenderer {
    atlas: Arc<HeadlessAtlas>,
}

impl SceneRenderer for DiscardRenderer {
    fn draw(&mut self, _scene: &Scene) -> bool {
        true
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

impl PlatformRenderer for DiscardRenderer {
    fn update_drawable_size(&mut self, _size: Size<DevicePixels>) {}

    fn max_texture_size(&self) -> u32 {
        // A headless window never consults this; there is no device to state a real limit for.
        0
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
}

/// Shorthand for one `Size<DevicePixels>`, so the assertions read as pixel sizes.
fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

/// Copy one pixel out of `texture` on `device`, swizzling the backend's BGRA target back to RGBA.
fn read_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
) -> anyhow::Result<[u8; 4]> {
    let width = texture.width();
    let height = texture.height();
    let padded_bytes_per_row = (width * 4)
        .checked_next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .expect("the row size overflows");
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("guest_frame_readback"),
        size: u64::from(padded_bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("guest_frame_readback_encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit(std::iter::once(encoder.finish()));
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(30)),
    })?;
    receiver.recv_timeout(Duration::from_secs(30))??;

    let data = readback.slice(..).get_mapped_range();
    let offset = (y * padded_bytes_per_row + x * 4) as usize;
    let mut pixel = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
    if matches!(
        texture.format(),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        pixel.swap(0, 2);
    }
    drop(data);
    readback.unmap();
    Ok(pixel)
}

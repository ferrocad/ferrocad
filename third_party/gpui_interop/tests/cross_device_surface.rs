#![cfg(target_os = "windows")]
//! The cross-device arm, end to end: a **Direct3D 12** producer's shareable texture, opened on
//! GPUI's **Direct3D 11** renderer and composited byte for byte.
//!
//! This is the cross-device case, and the one the built-in same-device path cannot exercise. The
//! producer renders into a committed texture on a `D3D12_HEAP_FLAG_SHARED` heap, hands over its NT
//! handle, and GPUI's Direct3D 11 device opens it with `OpenSharedResource1` and samples it through
//! a shader resource view (`gpui_interop::SharedSurface`). The two queues are ordered by a shared
//! `ID3D12Fence`, signalled on the producer's queue after the clear and waited **GPU-side** on
//! GPUI's device (`ID3D11DeviceContext4::Wait`) — never a CPU poll.
//!
//! # Why the test lives here, and not in `gpui_windows`
//!
//! The renderer is `directx_renderer::DirectXRenderer`, which is now public, but the test still
//! reaches it the way the whole application does — through the renderer seam the window lends:
//! `PlatformWindow::with_renderer`, downcast with `SceneRenderer::as_renderer`, and read with
//! `GpuRenderer::device`.
//!
//! The producer's own half is raw Direct3D 12 through the `windows` crate, which `gpui_interop`
//! already depends on for `SharedSurface` and `Fence`.
//!
//! # What this test is not
//!
//! It is Windows-only and, on a machine with no Direct3D 12 device (or no DirectComposition), it
//! **skips** rather than fails — the same guard `gpui_windows`'s own renderer tests use. It is
//! deliberately not a Linux/CI-passing test.

use std::mem::ManuallyDrop;

use anyhow::{Context as _, Result};
use gpui::{
    Bounds, ContentMask, DevicePixels, DirectXSource, GpuRenderer, PaintSurface, Platform,
    PlatformWindow, Point, Scene, SharedDirectXFence, SharedDirectXSurface, Size, SurfaceSource,
    TitlebarOptions, WindowId, WindowKind, WindowParams, point, px, size,
};
use gpui_interop::{Fence, SharedSurface};
use gpui_windows::DirectXRenderer;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
    D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue,
    ID3D12DescriptorHeap, ID3D12Device, ID3D12GraphicsCommandList, ID3D12PipelineState,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::core::Interface;

/// The fixture's edge, in pixels. Small, uniform, and read back exactly.
const SIZE: u32 = 8;

/// The codebase's known fixture, as the RGBA bytes a readback must return: the same `[200, 100, 50,
/// 255]` every renderer's round-trip test asserts (`an_imported_texture_round_trips_its_bytes`,
/// `a_surface_view_round_trips_its_bytes`). The producer's texture is `B8G8R8A8`, so its own raw
/// bytes are the BGRA order `[50, 100, 200, 255]`.
const FIXTURE: [u8; 4] = [200, 100, 50, 255];

/// The same fixture as the RGBA normalized floats a Direct3D clear takes. A clear is RGBA whatever
/// the resource format, so the `B8G8R8A8` store ends up BGRA in memory.
const CLEAR_RGBA: [f32; 4] = [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0, 1.0];

/// A Direct3D 12 producer: a committed, shareable texture cleared to the fixture, on the same
/// adapter as GPUI's device so the shared handle opens, with a shared fence signalled once.
struct Producer {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    // These outlive the readback: the command allocator, list and RTV descriptor heap are all
    // referenced by the submitted clear, and Direct3D does not retain them for us.
    _allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    rtv_heap: ID3D12DescriptorHeap,
    surface: SharedSurface,
    fence: Fence,
}

impl Producer {
    /// Build the producer on `adapter`: its Direct3D 12 device, command queue, shareable texture
    /// and shareable fence. `clear` and `signal` do the submitting afterwards.
    fn new(adapter: &IDXGIAdapter) -> Result<Self> {
        let mut device: Option<ID3D12Device> = None;
        unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }
            .context("no Direct3D 12 device on the renderer's adapter")?;
        let device = device.context("D3D12CreateDevice returned no device")?;

        let surface = SharedSurface::new(&device, SIZE, SIZE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .context("creating the shareable Direct3D 12 texture")?;
        let fence = Fence::new(&device).context("creating the shareable fence")?;

        let queue: ID3D12CommandQueue = unsafe {
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: 0,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .context("creating the producer's command queue")?;
        let rtv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .context("creating the RTV descriptor heap")?;
        let allocator: ID3D12CommandAllocator =
            unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
                .context("creating the command allocator")?;
        let list: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &allocator,
                None::<&ID3D12PipelineState>,
            )
        }
        .context("creating the command list")?;

        Ok(Self {
            device,
            queue,
            _allocator: allocator,
            list,
            rtv_heap,
            surface,
            fence,
        })
    }

    /// Clear the shared texture to the fixture and submit it.
    ///
    /// The resource is created in `D3D12_RESOURCE_STATE_COMMON` (the state a shared texture must be
    /// in to cross a device boundary), so the clear brackets it with barriers back to `COMMON`.
    fn clear(&mut self) -> Result<()> {
        let rtv = unsafe { self.rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        unsafe {
            self.device
                .CreateRenderTargetView(self.surface.resource(), None, rtv)
        };

        let to_render_target = transition(
            self.surface.resource(),
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        let to_common = transition(
            self.surface.resource(),
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_COMMON,
        );
        unsafe {
            self.list.ResourceBarrier(&[to_render_target]);
            self.list.ClearRenderTargetView(rtv, &CLEAR_RGBA, None);
            self.list.ResourceBarrier(&[to_common]);
            self.list.Close().context("closing the command list")?;
            let list: ID3D12CommandList = self.list.cast().context("casting the command list")?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
        }
        Ok(())
    }

    /// Signal the next value on the producer's queue after the clear, for the consumer's wait.
    fn signal(&mut self) -> Result<u64> {
        self.fence
            .signal(&self.queue)
            .context("signalling the producer's fence")
    }
}

/// The adapter GPUI's Direct3D 11 device is on.
///
/// The producer and the consumer must share one physical GPU: a `D3D12_HEAP_FLAG_SHARED` handle is
/// same-adapter, so the Direct3D 12 device is made on this adapter rather than the default one.
fn adapter_of(device: &ID3D11Device) -> Result<IDXGIAdapter> {
    let dxgi_device: IDXGIDevice = device
        .cast()
        .context("the Direct3D 11 device is not a DXGI device")?;
    unsafe { dxgi_device.GetAdapter() }.context("the Direct3D 11 device has no adapter")
}

/// A hidden Windows window whose renderer is GPUI's Direct3D 11 one, or `None` where this machine
/// cannot build the platform. A bare Windows runner has WARP, so the guard is a formality rather
/// than the expected path, and it reports why it skipped.
fn hidden_window() -> Option<Box<dyn PlatformWindow>> {
    let platform = match gpui_windows::WindowsPlatform::new(false) {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("no Direct3D 11 platform to open a window with; skipping: {error:#}");
            return None;
        }
    };

    // Never shown, never focused, never movable: the readback path is what makes the test
    // independent of a visible window, exactly as `gpui_windows`' own renderer tests are.
    let params = WindowParams {
        bounds: Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(SIZE as f32), px(SIZE as f32)),
        },
        titlebar: Some(TitlebarOptions::default()),
        kind: WindowKind::Normal,
        is_movable: false,
        app_owns_titlebar_drag: false,
        is_resizable: false,
        is_minimizable: false,
        focus: false,
        show: false,
        icon: None,
        display_id: None,
        app_id: None,
        window_min_size: None,
        renderer_factory: None,
    };
    match platform.open_window(WindowId::from(1), params) {
        Ok(window) => Some(window),
        Err(error) => {
            eprintln!("could not open a Direct3D window; skipping: {error:#}");
            None
        }
    }
}

/// A scene whose only primitive is a surface covering the whole target, carrying the producer's
/// source. This is the shape `draw_surfaces` composites (`SurfaceSource::DirectX`).
fn surface_scene(source: DirectXSource, edge: u32) -> Scene {
    let bounds = Bounds {
        origin: Point {
            x: 0.0.into(),
            y: 0.0.into(),
        },
        size: Size {
            width: (edge as f32).into(),
            height: (edge as f32).into(),
        },
    };
    let mut scene = Scene::default();
    scene.surfaces.push(PaintSurface {
        order: 0,
        bounds,
        content_mask: ContentMask { bounds },
        source: SurfaceSource::DirectX(source),
    });
    scene
}

/// One `D3D12_RESOURCE_BARRIER` transition, the shape `ResourceBarrier` takes.
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    let transition = D3D12_RESOURCE_TRANSITION_BARRIER {
        pResource: ManuallyDrop::new(Some(resource.clone())),
        Subresource: 0,
        StateBefore: before,
        StateAfter: after,
    };
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(transition),
        },
    }
}

/// A Direct3D 12 producer's texture, shared across devices and composited by GPUI's Direct3D 11
/// renderer, comes back byte for byte.
#[test]
fn a_shared_d3d12_surface_round_trips_through_the_d3d11_renderer() -> Result<()> {
    let viewport = Size {
        width: DevicePixels(SIZE as i32),
        height: DevicePixels(SIZE as i32),
    };

    let Some(mut window) = hidden_window() else {
        return Ok(());
    };

    // The consumer's device, reached through the renderer seam the window lends. The producer's
    // Direct3D 12 device is built on this device's adapter in `Producer::new` (via `adapter_of`
    // below).
    let mut device11 = None;
    window.with_renderer(&mut |renderer| {
        device11 = renderer
            .as_renderer::<DirectXRenderer>()
            .and_then(GpuRenderer::device);
    });
    let device11 = device11.context("the window's renderer lends an ID3D11Device")?;

    let adapter = adapter_of(&device11)?;

    // The producer: create, clear to the fixture, and signal. If there is no Direct3D 12 device on
    // this adapter, the machine is the Direct3D 11-only kind the host must also run on, and the
    // cross-device arm cannot be exercised here — skip rather than fail.
    let mut producer = match Producer::new(&adapter) {
        Ok(producer) => producer,
        Err(error) => {
            eprintln!("no cross-device Direct3D 12 producer; skipping: {error:#}");
            return Ok(());
        }
    };
    producer.clear()?;
    let signal = producer.signal()?;

    // The consumer's half is the renderer's: hand it the producer's handles, and it opens the
    // texture on its own device, views it, and waits the fence before the draw. The producer never
    // needs the window's device.
    let scene = surface_scene(
        DirectXSource::Shared(SharedDirectXSurface {
            texture: producer.surface.handle(),
            fence: Some(SharedDirectXFence {
                handle: producer.fence.handle(),
                value: signal,
            }),
            width: SIZE,
            height: SIZE,
        }),
        SIZE,
    );
    let mut rendered = None;
    window.with_renderer(&mut |renderer| {
        rendered = Some(renderer.render_scene_to_image(&scene, viewport));
    });
    let pixels = rendered
        .context("the window's renderer ran the scene")?
        .context("compositing the producer's surface")?;

    assert_eq!((pixels.width(), pixels.height()), (SIZE, SIZE));
    for (index, pixel) in pixels.data().chunks_exact(4).enumerate() {
        assert_eq!(pixel, FIXTURE, "pixel {index} did not round trip");
    }
    Ok(())
}

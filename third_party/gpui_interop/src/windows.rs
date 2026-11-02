//! The Windows module: a Direct3D 12 / `wgpu` producer's surface on GPUI's Direct3D 11 renderer.
//!
//! A producer whose device is not GPUI's renders into a **committed, shared** Direct3D 12 texture
//! and hands GPUI its NT handle. The **renderer** opens it on its own Direct3D 11 device and samples
//! it through a shader resource view — that is the consumer half, `DirectXSource::Shared` — so this
//! module is the **producer** half: allocate the shareable texture, signal the fence, hand over the
//! handles. Ordering is a shared `ID3D12Fence`, which the renderer opens as an `ID3D11Fence` and
//! waits **GPU-side** with `ID3D11DeviceContext4::Wait` — never a CPU poll.
//!
//! Two facts shape this code:
//!
//! - **A placed resource cannot be shared.** `CreateSharedHandle` on a placed resource returns
//!   `E_INVALIDARG`; [`SharedSurface`] therefore allocates a **committed** resource on a
//!   `D3D12_HEAP_FLAG_SHARED` heap.
//! - **The fence is ordered GPU-side**, which is the renderer's wait, not `SetEventOnCompletion`
//!   plus a CPU wait here.

use windows::Win32::Foundation::{CloseHandle, E_POINTER, HANDLE};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_MEMORY_POOL_UNKNOWN, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATE_COMMON, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandQueue, ID3D12Device,
    ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::core::{Error, PCWSTR};

/// `GENERIC_ALL`, the access `CreateSharedHandle` needs for a handle this process opens back.
///
/// Named here rather than imported; the value is the documented one (WinNT.h).
const GENERIC_ALL: u32 = 0x1000_0000;

/// A committed, shareable Direct3D 12 texture — the producer's render target.
///
/// The resource lives on a `D3D12_HEAP_FLAG_SHARED` heap and is committed, because a placed resource
/// cannot be shared: `CreateSharedHandle` rejects it with `E_INVALIDARG`. The NT handle outlives the
/// resource's use by GPUI and is closed when this value drops; other processes or devices keep their
/// own references.
pub struct SharedSurface {
    resource: ID3D12Resource,
    handle: HANDLE,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl SharedSurface {
    /// Allocate a shareable texture of `width` × `height`, render-target capable.
    ///
    /// The resource starts in `D3D12_RESOURCE_STATE_COMMON`, the state a shared texture must be in
    /// when it crosses a device boundary; the producer transitions it for its own passes.
    pub fn new(
        device: &ID3D12Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> windows::core::Result<Self> {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        };

        // SAFETY: every pointer is to a local that outlives the call, and the resource and handle
        // are owned by the returned value.
        unsafe {
            let mut resource: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_SHARED,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?;
            let resource = resource.ok_or_else(|| Error::from_hresult(E_POINTER))?;
            let handle = device.CreateSharedHandle(&resource, None, GENERIC_ALL, PCWSTR::null())?;
            Ok(Self {
                resource,
                handle,
                width,
                height,
                format,
            })
        }
    }

    /// The NT handle the renderer opens with `OpenSharedResource1`.
    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    /// The Direct3D 12 resource, for the producer's own render passes.
    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    /// The texture's width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The texture's height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The texture's format.
    pub fn format(&self) -> DXGI_FORMAT {
        self.format
    }
}

impl Drop for SharedSurface {
    fn drop(&mut self) {
        // SAFETY: `handle` was created by `CreateSharedHandle` and is closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// A shared `ID3D12Fence`, signalled by the producer and waited **GPU-side** by the renderer.
///
/// The producer signals it on its queue after committing a frame; the renderer opens the NT handle
/// as an `ID3D11Fence` and inserts a GPU-side wait, so no CPU thread polls.
pub struct Fence {
    fence: ID3D12Fence,
    handle: HANDLE,
    value: u64,
}

impl Fence {
    /// Create a shareable fence starting at zero.
    pub fn new(device: &ID3D12Device) -> windows::core::Result<Self> {
        // SAFETY: the fence and handle are owned by the returned value; the name is null, so the
        // handle is unnamed and shared by value alone.
        unsafe {
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let handle = device.CreateSharedHandle(&fence, None, GENERIC_ALL, PCWSTR::null())?;
            Ok(Self {
                fence,
                handle,
                value: 0,
            })
        }
    }

    /// The NT handle a Direct3D 11 device opens with [`Fence::open`].
    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    /// The last value signalled.
    pub fn value(&self) -> u64 {
        self.value
    }

    /// Signal the next value on the producer's queue, returning it for the consumer's wait.
    pub fn signal(&mut self, queue: &ID3D12CommandQueue) -> windows::core::Result<u64> {
        self.value += 1;
        // SAFETY: `queue` and `fence` are live, and the value is this fence's next.
        unsafe {
            queue.Signal(&self.fence, self.value)?;
        }
        Ok(self.value)
    }
}

impl Drop for Fence {
    fn drop(&mut self) {
        // SAFETY: `handle` was created by `CreateSharedHandle` and is closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

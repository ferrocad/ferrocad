//! The GPU context: the device and queue the renderer owns.
//!
//! Instance/adapter selection and (for a windowed app) surface configuration happen
//! in the host — this type is the *device handoff* the host gives the renderer, and
//! through it the `pivy.coin` facade when it manages frames.

/// The wgpu device and queue shared by the renderer's passes.
pub struct GpuContext {
    /// The logical device.
    pub device: wgpu::Device,
    /// The command queue for the device.
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Wrap an already-created device/queue pair.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue }
    }
}

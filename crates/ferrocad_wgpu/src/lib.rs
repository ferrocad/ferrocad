//! # FerroCAD wgpu backend (`ferrocad_wgpu`)
//!
//! One backend behind the renderer-agnostic batch boundary defined by
//! [`ferrocad_inventor`]. It consumes the [`DrawBatch`] trait — never the node
//! model — so a D3D or Metal backend can sit in its place without the compiler or
//! scene graph changing.
//!
//! [`WgpuRenderer`] implements [`InventorView`], so a host can present a frame in
//! either of the two variants the trait exposes: borrow the offscreen
//! [`wgpu::Texture`](WgpuRenderer::surface) for zero-copy compositing, or call
//! [`image`](InventorView::image) for a readback [`ImageData`] (the MVP path while
//! `bite-gpui` has no wgpu surface primitive).
//!
//! - [`WgpuRenderer`] — encodes a frame: uploads [`mesh_id`]/[`generation`] cache
//!   misses, binds each packet's [`StateSnapshot`] and draws it.
//! - [`context`] — the [`GpuContext`]: the device/queue the renderer owns.
//! - [`batch`] — uploading fresh geometry into GPU meshes.
//! - [`pipeline`] — the WGSL render pipelines (opaque, transparent, lines, points).
//! - [`uniforms`] — the dynamic-uniform buffer (per-frame globals + per-packet
//!   state in one allocation).
//! - [`pick`] — the g-buffer picking pass that maps a pixel back to a pick id.
//!
//! [`DrawBatch`]: ferrocad_inventor::DrawBatch
//! [`InventorView`]: ferrocad_inventor::InventorView
//! [`ImageData`]: ferrocad_inventor::ImageData
//! [`mesh_id`]: ferrocad_inventor::MeshId
//! [`generation`]: ferrocad_inventor::Generation
//! [`StateSnapshot`]: ferrocad_inventor::StateSnapshot

pub mod batch;
pub mod context;
pub mod pick;
pub mod pipeline;
pub mod renderer;
pub mod textures;
pub mod uniforms;

pub use context::GpuContext;
pub use renderer::{RenderError, WgpuRenderer};

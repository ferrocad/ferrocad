//! The wgpu half of the imported-texture path.
//!
//! The engine transports a texture produced outside GPUI as an erased
//! [`ImportedTextureHandle`], because it owns no GPU type
//! (`crates/gpui_engine/src/custom_render.rs`). The payload and the extension that
//! builds one live here, in the only crate that names a `wgpu` API; the renderer
//! ([`crate::WgpuRenderer`]) is what downcasts it back.

use gpui_engine::ImportedTextureHandle;

/// The wgpu payload an [`ImportedTextureHandle`] carries.
///
/// The renderer binds the view at a fragment sampler slot; the sampler itself is
/// the renderer's, shared by every imported texture, and the per-primitive values
/// (bounds, clip, radii, opacity) travel in the scene's custom primitive rather
/// than here.
#[derive(Clone)]
pub struct WgpuImportedTexture {
    /// The view the imported-texture fragment samples.
    pub view: wgpu::TextureView,
}

/// Builds an [`ImportedTextureHandle`] from a wgpu resource.
pub trait ImportedTextureExt {
    /// Wrap this view as a handle.
    ///
    /// Validates the colour-space and binding invariants up front, at the boundary where the
    /// failure is a producer's mistake and the message can name it. Sampling a non-sRGB view
    /// composites with a gamma error rather than crashing, so it has to be rejected here;
    /// `TEXTURE_BINDING` is likewise a property of the view and is what the renderer's bind group
    /// requires.
    fn to_imported_handle(&self) -> anyhow::Result<ImportedTextureHandle>;
}

/// wasm has no `Send + Sync` texture view yet: `ImportedTextureHandle::new`
/// requires it, and wgpu's wasm handles wrap `Rc`. `gpui_web` never imports a
/// texture, so the impl is native-only until the handle bound is wasm-aware.
#[cfg(not(target_family = "wasm"))]
impl ImportedTextureExt for wgpu::TextureView {
    fn to_imported_handle(&self) -> anyhow::Result<ImportedTextureHandle> {
        let texture = self.texture();
        let format = texture.format();
        anyhow::ensure!(
            matches!(
                format,
                wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
            "an imported texture must sample as sRGB (Rgba8UnormSrgb or Bgra8UnormSrgb), \
             but this view's texture is {format:?}"
        );
        anyhow::ensure!(
            texture
                .usage()
                .contains(wgpu::TextureUsages::TEXTURE_BINDING),
            "an imported texture must be created with wgpu::TextureUsages::TEXTURE_BINDING"
        );

        Ok(ImportedTextureHandle::new(WgpuImportedTexture {
            view: self.clone(),
        }))
    }
}

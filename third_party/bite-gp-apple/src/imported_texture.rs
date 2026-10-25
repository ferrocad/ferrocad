//! The Metal half of the imported-texture path.
//!
//! The engine transports a texture produced outside GPUI as an erased
//! [`ImportedTextureHandle`], because it owns no GPU type. `MetalTexture` is the native handle it
//! names for this backend, and this is the extension that builds one at the boundary; the renderer
//! ([`crate::metal_renderer::MetalRenderer`]) is what downcasts it back.

use anyhow::Result;
use foreign_types::ForeignTypeRef;
use gpui_engine::{ImportedTextureHandle, MetalTexture};
use metal::MTLPixelFormat;

/// Builds an [`ImportedTextureHandle`] from a Metal texture.
///
/// This is the producer's half of the imported-texture path on macOS, and it is public because an application has to
/// be able to call it: the device comes from the canvas's typed door (`GpuCanvasContext::device`),
/// and the token from here.
///
/// What the device does not settle is *which* device: a texture has to be made on the window's own
/// renderer's, which is the same-device rule. Metal has no way to enforce that
/// — a resource does not expose the device that made it, where Direct3D's
/// `CreateShaderResourceView` refuses a mismatch by name — so the rest of the contract is here: the
/// handle carries the `id<MTLTexture>` by pointer rather than retaining it, so the producer has to
/// keep the texture alive for the frame that samples it.
pub trait MetalTextureExt {
    /// Wrap this texture as a handle.
    ///
    /// Validates up front, at the boundary, where the failure is a producer's mistake and the
    /// message can name it. The fragment decodes the sample through an sRGB decode and re-encodes
    /// it, so a texture that is already encoded would composite double-encoded — a visible defect
    /// rather than an error — which is what makes the declaration worth checking here and not only
    /// at the draw.
    fn to_imported_handle(&self) -> Result<ImportedTextureHandle>;
}

impl MetalTextureExt for metal::TextureRef {
    fn to_imported_handle(&self) -> Result<ImportedTextureHandle> {
        let pixel_format = self.pixel_format();
        anyhow::ensure!(
            matches!(
                pixel_format,
                MTLPixelFormat::BGRA8Unorm_sRGB | MTLPixelFormat::RGBA8Unorm_sRGB
            ),
            "an imported texture must be declared sRGB, the colour space the fragment decodes and \
             re-encodes, but this one is {pixel_format:?}"
        );
        anyhow::ensure!(
            self.usage().contains(metal::MTLTextureUsage::ShaderRead),
            "an imported texture must be created with MTLTextureUsage::ShaderRead"
        );

        Ok(ImportedTextureHandle::new(MetalTexture(
            self.as_ptr() as *mut std::ffi::c_void
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};
    use gpui_engine::{CustomRenderPrimitive, GpuRenderer, Scene, SceneRenderer};
    use gpui_platform::{Bounds, ContentMask, Corners, DevicePixels, GpuPreference, Point, Size};
    use metal::{MTLOrigin, MTLRegion, MTLSize, MTLStorageMode, MTLTextureUsage};
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// A headless renderer, or `None` where this machine has no Metal device at all. Every bare
    /// macOS runner has one, so the guard is a formality that keeps a row from failing for a reason
    /// that is not about the code.
    fn test_renderer() -> Option<MetalRenderer> {
        metal::Device::system_default()?;
        Some(
            MetalRenderer::new_headless(
                Arc::new(Mutex::new(InstanceBufferPool::default())),
                &GpuPreference::Auto,
            )
            .expect("a Metal device was just found"),
        )
    }

    /// The renderer's own device, reached the way an application reaches it: through the seam,
    /// which is the whole of what `GpuRenderer::device` exists for.
    fn renderer_device(renderer: &MetalRenderer) -> metal::Device {
        GpuRenderer::device(renderer).expect("the Metal renderer lends its device through the seam")
    }

    /// A 1x1 texture on `device` holding `bgra` in its only pixel, which is what a producer that
    /// wrote one would leave behind. `pixel_format` is a parameter so a test can hand the boundary
    /// something it has to refuse.
    fn imported_texture(
        device: &metal::Device,
        pixel_format: MTLPixelFormat,
        bgra: [u8; 4],
    ) -> metal::Texture {
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(1);
        descriptor.set_height(1);
        descriptor.set_pixel_format(pixel_format);
        descriptor.set_usage(MTLTextureUsage::ShaderRead);
        // CPU-writable, which is how a fixture gets its bytes; `Shared` rather than the renderer's
        // `Managed` target because this one is populated by the CPU and sampled by the GPU.
        descriptor.set_storage_mode(MTLStorageMode::Shared);
        let texture = device.new_texture(&descriptor);
        texture.replace_region(
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
            },
            0,
            bgra.as_ptr() as *const std::ffi::c_void,
            4,
        );
        texture
    }

    /// A scene whose only primitive is an imported texture covering the whole target.
    fn imported_texture_scene(
        handle: ImportedTextureHandle,
        viewport: Size<DevicePixels>,
    ) -> Scene {
        let bounds = Bounds {
            origin: Point {
                x: 0.0.into(),
                y: 0.0.into(),
            },
            size: Size {
                width: (viewport.width.0 as f32).into(),
                height: (viewport.height.0 as f32).into(),
            },
        };
        let mut scene = Scene::default();
        scene.custom.push(CustomRenderPrimitive::Texture {
            order: 0,
            handle,
            bounds,
            content_mask: ContentMask { bounds },
            radii: Corners::default(),
            opacity: 1.0,
            flip_v: false,
        });
        scene
    }

    /// The colour row. A producer's bytes have to come back unchanged: the fragment decodes the
    /// sample through the texture's sRGB declaration and re-encodes it into the non-sRGB target, so
    /// the transfer function cancels -- which needs the exact encoder rather than the power
    /// approximation the rest of the pipeline uses. Either half alone, or the approximation, shifts
    /// the composite rather than failing, which is why this is asserted on pixels.
    #[test]
    fn an_imported_texture_round_trips_its_srgb_bytes() -> anyhow::Result<()> {
        let viewport = Size {
            width: DevicePixels(8),
            height: DevicePixels(8),
        };
        let Some(mut renderer) = test_renderer() else {
            log::warn!("no Metal device available to render offscreen; skipping");
            return Ok(());
        };

        // Distinct per channel, and far from either end, so a transfer function applied once in
        // the wrong direction cannot round back to the same byte.
        let fixture = [200u8, 100, 50, 255];
        let device = renderer_device(&renderer);
        // The texture's bytes are BGRA, the target's order; the readback swaps them to RGBA.
        let texture = imported_texture(
            &device,
            MTLPixelFormat::BGRA8Unorm_sRGB,
            [fixture[2], fixture[1], fixture[0], fixture[3]],
        );
        let scene = imported_texture_scene(texture.as_ref().to_imported_handle()?, viewport);

        // The seam's method rather than the inherent one, because the contract is asserted
        // through is the trait's, and its pixels are the engine's `PixelBuffer`.
        let pixels = SceneRenderer::render_scene_to_image(&mut renderer, &scene, viewport)?;
        assert_eq!((pixels.width(), pixels.height()), (8, 8));
        for (index, pixel) in pixels.data().chunks_exact(4).enumerate() {
            assert_eq!(pixel, fixture, "pixel {index} did not round trip");
        }
        Ok(())
    }

    /// The boundary row. The fragment decodes an sRGB declaration, so a texture that is not
    /// declared sRGB is already encoded and would come out double-encoded; the producer-side
    /// extension has to refuse it at the boundary rather than let the renderer skip it silently.
    #[test]
    fn an_imported_texture_in_the_wrong_format_is_refused() {
        let Some(renderer) = test_renderer() else {
            log::warn!("no Metal device available; skipping");
            return;
        };
        let device = renderer_device(&renderer);
        let texture = imported_texture(&device, MTLPixelFormat::BGRA8Unorm, [50, 100, 200, 255]);

        let error = texture
            .as_ref()
            .to_imported_handle()
            .expect_err("a non-sRGB texture is not an imported texture");
        assert!(
            error.to_string().contains("sRGB"),
            "the refusal should name the declaration it wants: {error}"
        );
    }
}

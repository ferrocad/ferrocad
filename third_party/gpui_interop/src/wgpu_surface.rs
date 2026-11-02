//! A wgpu producer's render target, on the window renderer's own device.
//!
//! A wgpu application draws with the ordinary `wgpu` API — pipelines, bind groups, render passes —
//! but nothing in that API says what GPUI will accept. [`WgpuSurface`] allocates a texture with the
//! two invariants the imported-texture path checks, so a frame rendered into [`view`](Self::view)
//! composites with `GpuCanvasContext::paint_texture` without a copy:
//!
//! - the colour space is sRGB, because the renderer's surfaced fragment samples the texel and writes
//!   it through; a `UNORM` target is rejected by `to_imported_handle`, and one sampled without that
//!   check composites with a gamma error rather than failing, and
//! - the usage carries `TEXTURE_BINDING`, because the renderer makes its bind group from the view.
//!
//! # This is the same-device path
//!
//! The texture is meaningful only on the device that made it, so the device must be the window
//! renderer's. Where the renderer is wgpu, `Adapter::wgpu` returns exactly that device and queue —
//! same logical device, no adapter match and no second queue — which is the ordinary way to reach it:
//!
//! ```rust,ignore
//! let interop = attach(gpu.try_device::<WgpuRenderer>())?;
//! let (device, queue) = interop.adapter().wgpu().expect("the window's own wgpu device");
//! let surface = WgpuSurface::new(&device, width, height)?;
//!
//! // ...render the viewport into surface.view() and submit on `queue`...
//!
//! gpu.paint_texture(surface.view().to_imported_handle()?, Corners::default(), 1.0, false);
//! ```
//!
//! Submit the frame before returning from the canvas. `paint_texture` only records the view in the
//! scene; the renderer submits its own pass after the callback returns, on the same queue, so a
//! producer's work ordered ahead of it is visible to the present.
//!
//! A texture **wgpu allocated** cannot be exported: wgpu allocates without the external-memory
//! flags a dma-buf needs and exposes no export API. When the producer must hand over a native handle
//! instead of a same-device view, use `WgpuSharedBuffer` on Linux: it
//! allocates the image itself on the wgpu device's `VkDevice`, wraps it back into wgpu for
//! rendering, and exports the dma-buf. On Windows the cross-device transport is the Direct3D one
//! (`SharedSurface`).

use anyhow::{Result, bail};

/// A texture a wgpu producer renders into and GPUI composites from the same device.
///
/// The texture is single-mip, single-sample and `D2`; [`new`](Self::new) uses
/// [`DEFAULT_FORMAT`](Self::DEFAULT_FORMAT) and [`with_format`](Self::with_format) takes the other
/// sRGB form. Holding this value keeps the texture alive; a [`TextureView`](wgpu::TextureView) cloned
/// out of [`view`](Self::view) keeps the texture alive on its own, so a producer that only retains
/// the view is still sound.
pub struct WgpuSurface {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

impl WgpuSurface {
    /// The format [`new`](Self::new) allocates. `Rgba8UnormSrgb` is wgpu's portable sRGB form and the
    /// channel order a shader writes without reordering.
    pub const DEFAULT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// Allocate a surface of `width` × `height` in [`DEFAULT_FORMAT`](Self::DEFAULT_FORMAT).
    ///
    /// A zero extent, or a device that cannot make the texture, is an error naming the shortfall
    /// rather than a later render pass that fails while encoding.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        Self::with_format(device, width, height, Self::DEFAULT_FORMAT)
    }

    /// Allocate a surface of `width` × `height` in `format`.
    ///
    /// `format` must be one GPUI can sample — `Rgba8UnormSrgb` or `Bgra8UnormSrgb` — because a
    /// surface is accepted only after the conversion to sRGB the renderer's fragment relies on.
    /// Anything else is refused here, where the mistake is the caller's, rather than at
    /// `to_imported_handle` on a different line.
    pub fn with_format(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        validate(format, width, height)?;
        let (texture, view) = allocate(device, width, height, format);
        Ok(Self {
            texture,
            view,
            format,
            width,
            height,
        })
    }

    /// The texture the producer's render pass attaches.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The view the producer renders through and GPUI samples.
    ///
    /// Clone it for a retained render target; the clone keeps the texture alive.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The sRGB format this surface was allocated in.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The surface's width and height, in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reallocate when the viewport's size changed, reporting whether it did.
    ///
    /// A canvas is rebuilt every frame and its bounds may change with the window; a surface whose
    /// size still matches is left alone, so the common case is a comparison. On error `self` is
    /// unchanged and the caller can keep painting the old surface.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Result<bool> {
        if (width, height) == (self.width, self.height) {
            return Ok(false);
        }
        let next = Self::with_format(device, width, height, self.format)?;
        *self = next;
        Ok(true)
    }
}

/// Refuse a format or extent GPUI's imported-texture path cannot carry.
fn validate(format: wgpu::TextureFormat, width: u32, height: u32) -> Result<()> {
    if !matches!(
        format,
        wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        bail!(
            "a GPUI surface must sample as sRGB (Rgba8UnormSrgb or Bgra8UnormSrgb), \
             but {format:?} was asked for"
        );
    }
    if width == 0 || height == 0 {
        bail!("a GPUI surface must have a non-zero extent, but {width}x{height} was asked for");
    }
    Ok(())
}

/// Make the texture and the view the producer renders through.
fn allocate(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpui_interop_wgpu_surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // `COPY_DST` is not needed to render, but it lets a producer stage bytes with
        // `Queue::write_texture` (a software decoder's fallback) without a second texture.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("gpui_interop_wgpu_surface_view"),
        ..Default::default()
    });
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_srgb_forms_are_surfaces() {
        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            assert!(
                validate(format, 4, 4).is_ok(),
                "{format:?} should be accepted"
            );
        }
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba16Float,
        ] {
            let error = validate(format, 4, 4).expect_err("a non-sRGB format is not a surface");
            assert!(
                error.to_string().contains("sRGB"),
                "the refusal should name the colour-space rule, not {error}"
            );
        }
    }

    #[test]
    fn a_zero_extent_is_refused() {
        assert!(validate(WgpuSurface::DEFAULT_FORMAT, 0, 4).is_err());
        assert!(validate(WgpuSurface::DEFAULT_FORMAT, 4, 0).is_err());
    }
}

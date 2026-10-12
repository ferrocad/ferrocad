//! The viewport and its logical↔device pixel conversions.
//!
//! FreeCAD runs Coin entirely in **device (physical) pixels**: the viewport region
//! it hands the render and event managers is `device_pixel_ratio × logical`, and it
//! converts Qt widget coordinates to that space on the way in — see
//! `InputDevice::toDevicePixelPosition` and `View3DInventorViewer::toQPoint`.
//! [`Viewport`] is that boundary in one place, so the host (which owns the window
//! and its scale factor) and the `pivy.coin` facade share one implementation
//! instead of each doing the arithmetic.
//!
//! Two conventions, both inherited from Coin/Qt:
//!
//! - **Device positions are bottom-left origin** (Coin), **logical positions are
//!   top-left origin** (Qt/gpui), so every conversion flips Y.
//! - Logical↔device is a pure scale by [`Viewport::device_pixel_ratio`].

/// A render viewport: its size in **device** pixels and the display scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    device_size: [u32; 2],
    device_pixel_ratio: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            device_size: [1, 1],
            device_pixel_ratio: 1.0,
        }
    }
}

impl Viewport {
    /// A viewport of `device_size` device pixels at `device_pixel_ratio` (both
    /// clamped to sane minimums: at least 1×1 and a positive ratio).
    pub fn new(device_size: [u32; 2], device_pixel_ratio: f32) -> Self {
        Self {
            device_size: [device_size[0].max(1), device_size[1].max(1)],
            device_pixel_ratio: if device_pixel_ratio > 0.0 {
                device_pixel_ratio
            } else {
                1.0
            },
        }
    }

    /// The size in device (physical) pixels.
    pub fn device_size(&self) -> [u32; 2] {
        self.device_size
    }

    /// The size in logical pixels (`device_size / device_pixel_ratio`).
    pub fn logical_size(&self) -> [f32; 2] {
        [
            self.device_size[0] as f32 / self.device_pixel_ratio,
            self.device_size[1] as f32 / self.device_pixel_ratio,
        ]
    }

    /// Device pixels per logical pixel (the display's scale factor).
    pub fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    /// Convert a logical position (top-left origin) to a device position
    /// (bottom-left origin).
    ///
    /// Mirrors FreeCAD's `InputDevice::toDevicePixelPosition`, including the `-1`
    /// pixel-index term and the Y flip.
    pub fn to_device(&self, logical: [f32; 2]) -> [i32; 2] {
        let scale = self.device_pixel_ratio;
        let device_height = self.device_size[1] as f32;
        let x = (logical[0] * scale).round() as i32;
        // (logical_height - y - 1) * scale, with logical_height = device_height / scale.
        let y = (device_height - logical[1] * scale - scale).round() as i32;
        [x, y]
    }

    /// Convert a device position (bottom-left origin) to a logical position
    /// (top-left origin).
    ///
    /// The inverse of [`to_device`](Self::to_device), mirroring FreeCAD's
    /// `View3DInventorViewer::toQPoint`. Returned unrounded; callers that need a
    /// whole logical pixel round themselves (FreeCAD's `toQPoint` rounds).
    pub fn to_logical(&self, device: [i32; 2]) -> [f32; 2] {
        let scale = self.device_pixel_ratio;
        let device_height = self.device_size[1] as f32;
        [
            device[0] as f32 / scale,
            (device_height - device[1] as f32 - 1.0) / scale,
        ]
    }

    /// The whole viewport as a [`Rect`], for code that treats a sub-rectangle and the
    /// full frame uniformly.
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.device_size[0], self.device_size[1])
    }
}

/// A **device-pixel** rectangle within a render target, in the same convention as
/// [`Viewport`] device positions: **bottom-left origin**.
///
/// Used for overlays (the navigation cube's corner) — both to confine a render pass
/// with `set_viewport`/`set_scissor_rect` and to compute a rect-relative pick
/// ([`Camera::ray_through_rect`](crate::Camera::ray_through_rect)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge, in device pixels.
    pub x: i32,
    /// Bottom edge, in device pixels.
    pub y: i32,
    /// Width, in device pixels.
    pub width: u32,
    /// Height, in device pixels.
    pub height: u32,
}

impl Rect {
    /// A rectangle at `(x, y)` (bottom-left origin) of the given size.
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether the device position lies inside the rectangle.
    pub fn contains(&self, device: [i32; 2]) -> bool {
        device[0] >= self.x
            && device[0] < self.x + self.width as i32
            && device[1] >= self.y
            && device[1] < self.y + self.height as i32
    }

    /// The position relative to the rectangle's bottom-left corner; `None` when the
    /// point is outside.
    pub fn relative(&self, device: [i32; 2]) -> Option<[i32; 2]> {
        self.contains(device)
            .then_some([device[0] - self.x, device[1] - self.y])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_to_device_flips_y() {
        // 800x600 device at 1x = 800x600 logical.
        let vp = Viewport::new([800, 600], 1.0);
        // Top-left logical corner maps to the bottom-left device corner.
        assert_eq!(vp.to_device([0.0, 0.0]), [0, 599]);
        assert_eq!(vp.to_device([10.0, 20.0]), [10, 579]);
    }

    #[test]
    fn device_to_logical_is_the_inverse_at_one_x() {
        let vp = Viewport::new([800, 600], 1.0);
        assert_eq!(vp.to_logical(vp.to_device([10.0, 20.0])), [10.0, 20.0]);
    }

    #[test]
    fn scale_factor_multiplies_positions() {
        // 1600x1200 device at 2x = 800x600 logical.
        let vp = Viewport::new([1600, 1200], 2.0);
        assert_eq!(vp.logical_size(), [800.0, 600.0]);
        // A logical point scales into device pixels, with the Y flip. The pixel-index
        // term means device positions land on halves at 2x, exactly as in FreeCAD.
        assert_eq!(vp.to_device([10.0, 20.0]), [20, 1158]);
        assert_eq!(vp.to_logical([20, 1158]), [10.0, 20.5]);
    }

    #[test]
    fn rejects_a_zero_ratio() {
        assert_eq!(Viewport::new([800, 600], 0.0).device_pixel_ratio(), 1.0);
    }

    #[test]
    fn rect_contains_and_relatives_a_device_point() {
        // A 100x100 overlay in the top-right of an 800x600 target (bottom-left origin).
        let rect = Rect::new(700, 500, 100, 100);
        assert!(rect.contains([750, 550]));
        assert!(!rect.contains([699, 550]));
        assert!(!rect.contains([750, 499]));
        assert_eq!(rect.relative([750, 550]), Some([50, 50]));
        assert_eq!(rect.relative([100, 100]), None);
        // The whole viewport is one rect at 0,0.
        assert_eq!(Viewport::new([800, 600], 1.0).rect(), Rect::new(0, 0, 800, 600));
    }
}

//! Headless view-provider state.
//!
//! Upstream FreeCAD keeps appearance on the Gui side: every document object gets a
//! `ViewProvider` (reachable from Python as `obj.ViewObject`) only when a GUI is
//! up, and the state lives next to that provider rather than in the `App` object.
//! FerroCAD's `App` layer is the same image with or without a viewport, so the
//! small part of that state that `App`-level code reads and writes — visibility,
//! transparency, the shape colour, per-face colours and the shape materials — is
//! stored with the object and persists through save/restore.
//!
//! This is deliberately *only* the data. Building a Coin scene graph, display
//! modes and the actual rendering stay in the GUI layers; [`ViewState`] is the
//! model those layers (and the Python `ViewObject`) agree on.

use serde::{Deserialize, Serialize};

/// An RGBA colour, each component in `0.0..=1.0` (FreeCAD's `Base::Color`).
pub type Color = [f64; 4];

/// The view state of one document object.
///
/// A missing `Option<ViewState>` on [`DocumentObject`](super::DocumentObject) means
/// "no view object was ever created"; the colour defaults are read from the user
/// preferences at that point (FreeCAD reads `DefaultShapeColor` and
/// `DefaultShapeTransparency` once, when the view provider is constructed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// `ViewObject.Visibility`.
    pub visibility: bool,
    /// `ViewObject.Transparency`, in percent (`0` opaque … `100` invisible).
    pub transparency: i32,
    /// The colour derived from the user preferences when the view object was
    /// created. Kept so the object keeps its look across save/restore even if the
    /// preferences change in between.
    pub default_color: Color,
    /// An explicit `ShapeColor`; `None` while the default still applies.
    #[serde(default)]
    pub shape_color: Option<Color>,
    /// Explicit per-face `DiffuseColor`; `None` means "derive from the shape".
    #[serde(default)]
    pub diffuse_color: Option<Vec<Color>>,
    /// `ShapeAppearance` (a material list). Empty means "one material, the
    /// effective colour".
    #[serde(default)]
    pub appearance: Vec<Color>,
    /// `ShapeMaterial`; `None` means "follow the effective colour".
    #[serde(default)]
    pub material: Option<Color>,
    /// `DisplayMode` (a FreeCAD display-mode name).
    #[serde(default = "default_display_mode")]
    pub display_mode: String,
}

fn default_display_mode() -> String {
    "Flat Lines".to_string()
}

impl ViewState {
    /// A fresh state: visible, opaque, painted with `default_color`.
    pub fn new(transparency: i32, default_color: Color) -> Self {
        Self {
            visibility: true,
            transparency: transparency.clamp(0, 100),
            default_color,
            shape_color: None,
            diffuse_color: None,
            appearance: Vec::new(),
            material: None,
            display_mode: default_display_mode(),
        }
    }

    /// The colour the whole shape is painted with (before per-face overrides).
    pub fn effective_color(&self) -> Color {
        self.shape_color.unwrap_or(self.default_color)
    }

    /// `ShapeAppearance`: at least one material, the effective colour by default.
    pub fn appearance(&self) -> Vec<Color> {
        if self.appearance.is_empty() {
            vec![self.effective_color()]
        } else {
            self.appearance.clone()
        }
    }

    /// `ShapeMaterial`: the explicit material, else the effective colour.
    pub fn material(&self) -> Color {
        self.material.unwrap_or_else(|| self.effective_color())
    }

    /// Assign `ShapeColor`, keeping the appearance and material in step with it.
    ///
    /// FreeCAD exposes `ShapeColor`, `ShapeAppearance` and `ShapeMaterial` as
    /// separate properties, but a provider paints all of them from one colour
    /// until one is overridden; updating the derived views here keeps the getters
    /// consistent instead of returning a stale colour.
    pub fn set_shape_color(&mut self, color: Color) {
        self.shape_color = Some(color);
        if let Some(first) = self.appearance.first_mut() {
            *first = color;
        }
    }

    /// Assign one entry of `ShapeAppearance`, materialising the list first.
    pub fn set_appearance_slot(&mut self, index: usize, color: Color) {
        if self.appearance.is_empty() {
            self.appearance = vec![self.effective_color()];
        }
        if index >= self.appearance.len() {
            let fill = self.effective_color();
            self.appearance.resize(index + 1, fill);
        }
        self.appearance[index] = color;
        if index == 0 {
            self.shape_color = Some(color);
        }
    }

    /// Replace `ShapeAppearance`; the first material also becomes `ShapeColor`.
    pub fn set_appearance(&mut self, colors: Vec<Color>) {
        if colors.is_empty() {
            self.appearance.clear();
            return;
        }
        self.shape_color = Some(colors[0]);
        self.appearance = colors;
    }
}

/// Decode a packed colour (`0xRRGGBBAA`) into RGBA components.
///
/// This is FreeCAD's `Base::Color` packing: the high byte is red, the low byte is
/// alpha (Draft builds a preference colour with `value | 0x000000FF`).
///
/// `opaque_if_alpha_zero` models the preference default: the upstream comment on
/// `DefaultShapeColor` notes that a stored `0x00` alpha means "unset" rather than
/// "fully transparent", so it is read as opaque (`0xFF000000` is red, not an
/// invisible red).
pub fn color_from_packed(packed: u32, opaque_if_alpha_zero: bool) -> Color {
    let red = ((packed >> 24) & 0xFF) as f64 / 255.0;
    let green = ((packed >> 16) & 0xFF) as f64 / 255.0;
    let blue = ((packed >> 8) & 0xFF) as f64 / 255.0;
    let alpha = (packed & 0xFF) as f64 / 255.0;
    let alpha = if opaque_if_alpha_zero && alpha == 0.0 {
        1.0
    } else {
        alpha
    };
    [red, green, blue, alpha]
}

/// Encode RGBA components as a packed `0xRRGGBBAA` colour.
pub fn color_to_packed(color: Color) -> u32 {
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(color[0]) << 24) | (byte(color[1]) << 16) | (byte(color[2]) << 8) | byte(color[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_color_is_rgba() {
        assert_eq!(color_from_packed(0xFF00_00FF, false), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(color_from_packed(0x00FF_00FF, false), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(color_from_packed(0x0000_FFFF, false), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(color_from_packed(0xFF00_0000, false), [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn default_color_treats_zero_alpha_as_opaque() {
        // `0xFF000000` is the test's "red": alpha 0 means "unset" for the
        // preference, not "invisible".
        assert_eq!(color_from_packed(0xFF00_0000, true), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn packed_round_trip() {
        let [r, g, b, a] = color_from_packed(0xADB5_BDFF, false);
        assert_eq!(color_to_packed([r, g, b, a]), 0xADB5_BDFF);
    }

    #[test]
    fn shape_color_updates_the_derived_views() {
        let mut state = ViewState::new(0, [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(state.appearance(), vec![[0.8, 0.8, 0.8, 1.0]]);
        assert_eq!(state.material(), [0.8, 0.8, 0.8, 1.0]);

        state.set_shape_color([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(state.effective_color(), [1.0, 0.0, 0.0, 1.0]);
        // `ShapeMaterial` follows until it is explicitly set.
        assert_eq!(state.material(), [1.0, 0.0, 0.0, 1.0]);

        state.set_appearance_slot(0, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(state.appearance(), vec![[0.0, 1.0, 0.0, 1.0]]);
        assert_eq!(state.material(), [0.0, 1.0, 0.0, 1.0]);

        state.material = Some([0.0, 0.0, 1.0, 1.0]);
        assert_eq!(state.material(), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn transparency_is_independent_of_colour() {
        let mut state = ViewState::new(70, [1.0, 1.0, 1.0, 1.0]);
        state.set_appearance_slot(0, [0.5, 0.0, 0.0, 1.0]);
        assert_eq!(state.transparency, 70);
    }
}

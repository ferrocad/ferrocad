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

/// `App::Material`'s defaults, from upstream `App::Material::setType(DEFAULT)`.
pub const DEFAULT_DIFFUSE: Color = [0.8, 0.8, 0.9, 1.0];
pub const DEFAULT_AMBIENT: Color = [0.3333, 0.3333, 0.3333, 1.0];
pub const DEFAULT_SPECULAR: Color = [0.5333, 0.5333, 0.5333, 1.0];
pub const DEFAULT_EMISSIVE: Color = [0.0, 0.0, 0.0, 1.0];
pub const DEFAULT_SHININESS: f64 = 0.9;

/// One appearance slot: a full `App::Material`, not just its colour.
///
/// FreeCAD keeps `DiffuseColor`, `AmbientColor`, `SpecularColor`,
/// `EmissiveColor`, `Shininess` and `Transparency` on every material, and
/// `ShapeAppearance[i]` / `ShapeMaterial` are materials. The view state stores
/// all six so a handle read from `ViewObject.ShapeAppearance[i]` round-trips
/// instead of resetting everything but its colour.
///
/// The diffuse colour's alpha is the opacity (`1 - transparency`), which is the
/// encoding the viewport consumes; [`Material::new`] and the setters keep the
/// two in lock-step so a slot cannot drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// `Material.DiffuseColor`; alpha is `1 - transparency`.
    pub diffuse: Color,
    /// `Material.AmbientColor`.
    pub ambient: Color,
    /// `Material.SpecularColor`.
    pub specular: Color,
    /// `Material.EmissiveColor`.
    pub emissive: Color,
    /// `Material.Shininess` (not clamped, as upstream).
    pub shininess: f64,
    /// `Material.Transparency`, `0.0` (opaque) … `1.0` (invisible).
    pub transparency: f64,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            diffuse: DEFAULT_DIFFUSE,
            ambient: DEFAULT_AMBIENT,
            specular: DEFAULT_SPECULAR,
            emissive: DEFAULT_EMISSIVE,
            shininess: DEFAULT_SHININESS,
            transparency: 0.0,
        }
    }
}

impl Material {
    /// A material with every field spelled out. `diffuse`'s alpha is replaced by
    /// `1 - transparency`, the viewport's encoding.
    pub fn new(
        diffuse: Color,
        ambient: Color,
        specular: Color,
        emissive: Color,
        shininess: f64,
        transparency: f64,
    ) -> Self {
        let transparency = transparency.clamp(0.0, 1.0);
        let mut diffuse = diffuse;
        diffuse[3] = 1.0 - transparency;
        Self {
            diffuse,
            ambient,
            specular,
            emissive,
            shininess,
            transparency,
        }
    }

    /// The default material painted with `color` (the `ShapeColor` case).
    pub fn from_color(color: Color) -> Self {
        Self::new(
            color,
            DEFAULT_AMBIENT,
            DEFAULT_SPECULAR,
            DEFAULT_EMISSIVE,
            DEFAULT_SHININESS,
            1.0 - color[3],
        )
    }

    /// The colour the shape is painted with.
    pub fn color(&self) -> Color {
        self.diffuse
    }

    /// Assign the material colour, keeping `transparency` in step with its alpha.
    pub fn set_color(&mut self, color: Color) {
        self.diffuse = color;
        self.transparency = 1.0 - color[3];
    }

    /// Assign `Material.Transparency`, keeping the colour alpha in step.
    pub fn set_transparency(&mut self, transparency: f64) {
        self.transparency = transparency.clamp(0.0, 1.0);
        self.diffuse[3] = 1.0 - self.transparency;
    }
}

/// A material slot before the shape materials existed, when only its colour was
/// stored. Reading it as `Material::from_color` keeps native documents written by
/// older builds loadable.
#[derive(Deserialize)]
#[serde(untagged)]
enum MaterialRepr {
    Material(Material),
    Color(Color),
}

impl MaterialRepr {
    fn into_material(self) -> Material {
        match self {
            Self::Material(material) => material,
            Self::Color(color) => Material::from_color(color),
        }
    }
}

fn deserialize_appearance<'de, D>(deserializer: D) -> Result<Vec<Material>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let reprs = Vec::<MaterialRepr>::deserialize(deserializer)?;
    Ok(reprs.into_iter().map(MaterialRepr::into_material).collect())
}

fn deserialize_material<'de, D>(deserializer: D) -> Result<Option<Material>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let repr = Option::<MaterialRepr>::deserialize(deserializer)?;
    Ok(repr.map(MaterialRepr::into_material))
}

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
    #[serde(default, deserialize_with = "deserialize_appearance")]
    pub appearance: Vec<Material>,
    /// `ShapeMaterial`; `None` means "follow the effective colour".
    #[serde(default, deserialize_with = "deserialize_material")]
    pub material: Option<Material>,
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
    pub fn appearance(&self) -> Vec<Material> {
        if self.appearance.is_empty() {
            vec![Material::from_color(self.effective_color())]
        } else {
            self.appearance.clone()
        }
    }

    /// `ShapeMaterial`: the explicit material, else the effective colour.
    pub fn material(&self) -> Material {
        self.material
            .clone()
            .unwrap_or_else(|| Material::from_color(self.effective_color()))
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
            first.set_color(color);
        }
    }

    /// Assign one `ShapeAppearance` entry, materialising the list first.
    pub fn set_appearance_slot(&mut self, index: usize, material: Material) {
        if self.appearance.is_empty() {
            self.appearance = vec![Material::from_color(self.effective_color())];
        }
        if index >= self.appearance.len() {
            let fill = Material::from_color(self.effective_color());
            self.appearance.resize(index + 1, fill);
        }
        let color = material.color();
        self.appearance[index] = material;
        if index == 0 {
            self.shape_color = Some(color);
        }
    }

    /// Replace `ShapeAppearance`; the first material also becomes `ShapeColor`.
    pub fn set_appearance(&mut self, materials: Vec<Material>) {
        if materials.is_empty() {
            self.appearance.clear();
            return;
        }
        self.shape_color = Some(materials[0].color());
        self.appearance = materials;
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
        assert_eq!(state.appearance()[0].color(), [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(state.material().color(), [0.8, 0.8, 0.8, 1.0]);

        state.set_shape_color([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(state.effective_color(), [1.0, 0.0, 0.0, 1.0]);
        // `ShapeMaterial` follows until it is explicitly set.
        assert_eq!(state.material().color(), [1.0, 0.0, 0.0, 1.0]);

        state.set_appearance_slot(0, Material::from_color([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(state.appearance()[0].color(), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(state.material().color(), [0.0, 1.0, 0.0, 1.0]);

        state.material = Some(Material::from_color([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(state.material().color(), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn appearance_slots_keep_every_material_field() {
        let mut state = ViewState::new(0, [0.8, 0.8, 0.8, 1.0]);
        let material = Material::new(
            [0.1, 0.2, 0.3, 1.0],
            [0.4, 0.4, 0.4, 1.0],
            [0.5, 0.5, 0.5, 1.0],
            [0.6, 0.0, 0.0, 1.0],
            0.25,
            0.35,
        );
        state.set_appearance_slot(0, material.clone());
        assert_eq!(state.appearance()[0], material);
        // The slot's first material is also the shape colour, with the
        // viewport's alpha encoding (`alpha = 1 - transparency`).
        assert_eq!(state.effective_color(), [0.1, 0.2, 0.3, 0.65]);
        assert_eq!(state.appearance()[0].transparency, 0.35);
    }

    #[test]
    fn transparency_is_independent_of_the_view_transparency() {
        let mut state = ViewState::new(70, [1.0, 1.0, 1.0, 1.0]);
        state.set_appearance_slot(0, Material::new(
            [0.5, 0.0, 0.0, 1.0],
            DEFAULT_AMBIENT,
            DEFAULT_SPECULAR,
            DEFAULT_EMISSIVE,
            DEFAULT_SHININESS,
            0.0,
        ));
        assert_eq!(state.transparency, 70);
    }

    #[test]
    fn materials_round_trip_through_serde() {
        let mut state = ViewState::new(0, [1.0, 1.0, 1.0, 1.0]);
        let material = Material::new(
            [0.1, 0.2, 0.3, 1.0],
            [0.4, 0.4, 0.4, 1.0],
            [0.5, 0.5, 0.5, 1.0],
            [0.6, 0.0, 0.0, 1.0],
            0.25,
            0.35,
        );
        state.set_appearance(vec![material.clone()]);
        state.material = Some(material.clone());

        let json = serde_json::to_string(&state).unwrap();
        let back: ViewState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.appearance()[0], material);
        assert_eq!(back.material(), material);
        assert_eq!(back.appearance()[0].color(), [0.1, 0.2, 0.3, 0.65]);
    }

    #[test]
    fn old_colour_slots_still_deserialize() {
        let state: ViewState = serde_json::from_str(
            r#"{"visibility":true,"transparency":0,"default_color":[1,1,1,1],
                "appearance":[[0.1,0.2,0.3,1.0]],"material":[0.4,0.5,0.6,0.5]}"#,
        )
        .unwrap();
        let appearance = state.appearance();
        assert_eq!(appearance[0].color(), [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(appearance[0].ambient, DEFAULT_AMBIENT);
        let material = state.material();
        assert_eq!(material.color(), [0.4, 0.5, 0.6, 0.5]);
        assert_eq!(material.transparency, 0.5);
    }
}

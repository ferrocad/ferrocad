//! Deterministic offscreen snapshots, for the generated documentation.
//!
//! A documentation workflow describes a scene in JSON — a fixed viewport size, a
//! fixed camera, and the shapes to draw — and [`render`] turns it into pixels on
//! the same offscreen path the viewport uses ([`WgpuRenderer`](crate::renderer::WgpuRenderer)
//! → readback). [`encode_png`] writes those pixels without a timestamp, so the
//! same spec on the same machine produces the same bytes.
//!
//! The spec carries a **mesh**, not a BREP file: this crate is kernel-free (it
//! draws the neutral batch vocabulary, not OCCT shapes), so the workflow that
//! owns the kernel tessellates and records the triangles. A `box` primitive is
//! offered for the case where the picture illustrates dimensions the document
//! only stores as numbers. Both build a [`ferrocad_stage::Stage`], which emits
//! the exact packets a workbench's view provider would.
//!
//! This is deliberately **not** on the CI path: the documentation gates
//! (`tools/userdocs.py --check`, `tools/build_book.py --check`) only read the
//! committed PNGs. Capturing needs a GPU adapter, and
//! [`SnapshotError::is_adapter_missing`] lets the caller say so without failing.

use std::fmt;

use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_inventor::{ImageData, InventorView, ViewSpec};
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Matrix4, Vector3};
use image::ImageEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};

use crate::renderer::{RenderError, WgpuRenderer};

/// The default viewport size for a documentation snapshot (16:10).
pub const DEFAULT_SIZE: [u32; 2] = [960, 600];

/// The default vertical field of view, in radians (about 34°).
pub const DEFAULT_FOV_Y: f64 = 0.6;

/// What can go wrong while validating a spec, rendering it, or encoding it.
#[derive(Debug)]
pub enum SnapshotError {
    /// The spec is internally inconsistent (a bad index, an empty shape, …).
    Invalid(String),
    /// The GPU path failed (including "no adapter on this machine").
    Render(RenderError),
    /// The PNG encoder rejected the pixels.
    Png(image::ImageError),
}

impl SnapshotError {
    /// Whether the machine simply has no usable GPU adapter.
    ///
    /// This is the one failure a documentation capture treats as non-fatal: it
    /// reports it and leaves the committed images alone.
    pub fn is_adapter_missing(&self) -> bool {
        matches!(
            self,
            SnapshotError::Render(RenderError::Adapter | RenderError::AdapterUnavailable(_))
        )
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Invalid(reason) => write!(f, "invalid snapshot spec: {reason}"),
            SnapshotError::Render(error) => write!(f, "{error}"),
            SnapshotError::Png(error) => write!(f, "PNG encoding failed: {error}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<RenderError> for SnapshotError {
    fn from(error: RenderError) -> Self {
        SnapshotError::Render(error)
    }
}

impl From<image::ImageError> for SnapshotError {
    fn from(error: image::ImageError) -> Self {
        SnapshotError::Png(error)
    }
}

fn default_size() -> [u32; 2] {
    DEFAULT_SIZE
}

fn default_up() -> [f64; 3] {
    [0.0, 0.0, 1.0]
}

fn default_fov_y() -> f64 {
    DEFAULT_FOV_Y
}

/// A fixed camera: an eye, a target, an up hint and a vertical field of view.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCamera {
    /// Where the camera sits, in world units.
    pub eye: [f64; 3],
    /// What it looks at, in world units.
    pub target: [f64; 3],
    /// Which way is up (defaults to +Z, the CAD convention).
    #[serde(default = "default_up")]
    pub up: [f64; 3],
    /// The vertical field of view, in radians.
    #[serde(default = "default_fov_y")]
    pub fov_y: f64,
}

impl Default for SnapshotCamera {
    fn default() -> Self {
        Self {
            eye: [0.0, -10.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: default_up(),
            fov_y: DEFAULT_FOV_Y,
        }
    }
}

impl SnapshotCamera {
    /// The renderer camera for a viewport of `size`.
    ///
    /// The view is right-handed about the eye→target axis (the camera looks down
    /// view-space −Z, the convention [`Projection::Perspective`] documents), and
    /// the clip planes follow the eye distance so a zoomed-in or zoomed-out
    /// scene is never clipped.
    pub fn to_camera(&self, size: [u32; 2]) -> Camera {
        let eye = Vector3::new(self.eye[0], self.eye[1], self.eye[2]);
        let target = Vector3::new(self.target[0], self.target[1], self.target[2]);
        let hint = Vector3::new(self.up[0], self.up[1], self.up[2]).normalize();
        let forward = target.sub(&eye).normalize();
        let right = forward.cross(&hint).normalize();
        let up = right.cross(&forward);

        let view = Matrix4::from_values([
            right.x,
            right.y,
            right.z,
            -right.dot(&eye),
            up.x,
            up.y,
            up.z,
            -up.dot(&eye),
            -forward.x,
            -forward.y,
            -forward.z,
            forward.dot(&eye),
            0.0,
            0.0,
            0.0,
            1.0,
        ]);

        let distance = target.sub(&eye).length().max(1e-6);
        let near = (distance / 1000.0).max(1e-4);
        let far = (distance * 1000.0).max(near * 10.0);
        Camera {
            view,
            projection: Projection::Perspective {
                fov_y: self.fov_y,
                near,
                far,
            },
            viewport: Viewport::new(size, 1.0),
        }
    }

    fn validate(&self) -> Result<(), SnapshotError> {
        let values = [self.eye, self.target, self.up].concat();
        if values.iter().any(|value| !value.is_finite()) {
            return Err(SnapshotError::Invalid(
                "camera has a non-finite coordinate".to_string(),
            ));
        }
        let eye = Vector3::new(self.eye[0], self.eye[1], self.eye[2]);
        let target = Vector3::new(self.target[0], self.target[1], self.target[2]);
        let forward = target.sub(&eye);
        if forward.length() <= 1e-9 {
            return Err(SnapshotError::Invalid(
                "camera eye and target are the same point".to_string(),
            ));
        }
        let up = Vector3::new(self.up[0], self.up[1], self.up[2]);
        if up.normalize().cross(&forward.normalize()).length() <= 1e-9 {
            return Err(SnapshotError::Invalid(
                "camera up is parallel to the view direction".to_string(),
            ));
        }
        if !(self.fov_y.is_finite() && self.fov_y > 0.0 && self.fov_y < std::f64::consts::PI) {
            return Err(SnapshotError::Invalid(format!(
                "field of view {} is not in (0, π)",
                self.fov_y
            )));
        }
        Ok(())
    }
}

/// A triangle/edge mesh in world units, already tessellated by the producer.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMesh {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals, or empty for flat shading (the default).
    #[serde(default)]
    pub normals: Vec<[f32; 3]>,
    /// Flat triangle indices, three per triangle.
    #[serde(default)]
    pub triangles: Vec<u32>,
    /// Flat edge-segment indices, two per segment.
    #[serde(default)]
    pub edges: Vec<u32>,
}

/// An axis-aligned box from `origin` to `origin + size`.
///
/// This is the one primitive the spec offers: a document that stores dimensions
/// but no kernel shape can still show what those dimensions describe, and the
/// box is built here with outward winding so its faces shade and cull correctly.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotBox {
    /// Extent along each axis, in world units.
    pub size: [f32; 3],
    /// The corner the box starts at.
    #[serde(default)]
    pub origin: [f32; 3],
}

impl SnapshotBox {
    /// The box's 8 corners, 12 triangles and 12 edges, wound outward.
    fn tessellate(&self) -> (Vec<[f32; 3]>, Vec<u32>, Vec<u32>) {
        let [sx, sy, sz] = self.size;
        let [ox, oy, oz] = self.origin;
        let positions = vec![
            [ox, oy, oz],
            [ox + sx, oy, oz],
            [ox + sx, oy + sy, oz],
            [ox, oy + sy, oz],
            [ox, oy, oz + sz],
            [ox + sx, oy, oz + sz],
            [ox + sx, oy + sy, oz + sz],
            [ox, oy + sy, oz + sz],
        ];
        // Each face is counter-clockwise seen from outside, so back-face culling
        // keeps the outside and the flat normal points away from the solid.
        const FACES: [[u32; 4]; 6] = [
            [0, 3, 2, 1], // −Z
            [4, 5, 6, 7], // +Z
            [0, 1, 5, 4], // −Y
            [1, 2, 6, 5], // +X
            [2, 3, 7, 6], // +Y
            [3, 0, 4, 7], // −X
        ];
        let mut triangles = Vec::with_capacity(36);
        let mut edges = Vec::with_capacity(24);
        for face in FACES {
            triangles.extend([face[0], face[1], face[2], face[0], face[2], face[3]]);
            edges.extend([
                face[0], face[1], face[1], face[2], face[2], face[3], face[3], face[0],
            ]);
        }
        (positions, triangles, edges)
    }
}

/// The geometry of one shape.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotGeometry {
    /// An axis-aligned box.
    Box(SnapshotBox),
    /// A producer-tessellated mesh.
    Mesh(SnapshotMesh),
}

/// How one shape is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotDrawMode {
    /// Shaded surfaces only.
    Shaded,
    /// Edges only.
    Wireframe,
    /// Shaded surfaces with their edges (the default for a snapshot).
    #[serde(alias = "shaded_edges")]
    ShadedWithEdges,
}

impl From<SnapshotDrawMode> for DrawMode {
    fn from(mode: SnapshotDrawMode) -> Self {
        match mode {
            SnapshotDrawMode::Shaded => DrawMode::Shaded,
            SnapshotDrawMode::Wireframe => DrawMode::Wireframe,
            SnapshotDrawMode::ShadedWithEdges => DrawMode::ShadedWithEdges,
        }
    }
}

/// One shape and its display attributes.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotShape {
    /// What to draw.
    pub geometry: SnapshotGeometry,
    /// Surface colour (linear RGBA); the theme's glass shade when absent.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// Edge colour (linear RGBA); the theme's front-edge token when absent.
    #[serde(default)]
    pub edge_color: Option<[f32; 4]>,
    /// How to draw it; shaded with edges when absent.
    #[serde(default)]
    pub draw_mode: Option<SnapshotDrawMode>,
}

impl SnapshotShape {
    /// A box with no explicit colours — the shortest useful spec.
    pub fn box_of(size: [f32; 3], origin: [f32; 3]) -> Self {
        Self {
            geometry: SnapshotGeometry::Box(SnapshotBox { size, origin }),
            color: None,
            edge_color: None,
            draw_mode: None,
        }
    }

    /// A mesh with the display attributes a producer reported.
    pub fn mesh(mesh: SnapshotMesh) -> Self {
        Self {
            geometry: SnapshotGeometry::Mesh(mesh),
            color: None,
            edge_color: None,
            draw_mode: None,
        }
    }
}

/// Everything one snapshot needs.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotSpec {
    /// The output size in pixels.
    #[serde(default = "default_size")]
    pub size: [u32; 2],
    /// The fixed camera.
    pub camera: SnapshotCamera,
    /// The shapes, drawn in order.
    pub shapes: Vec<SnapshotShape>,
}

impl SnapshotSpec {
    /// Check the spec before any GPU work happens, so a typo is reported as a
    /// spec error rather than as a blank frame.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        if self.size[0] == 0 || self.size[1] == 0 {
            return Err(SnapshotError::Invalid(
                "size has a zero dimension".to_string(),
            ));
        }
        self.camera.validate()?;
        if self.shapes.is_empty() {
            return Err(SnapshotError::Invalid("no shapes to draw".to_string()));
        }
        for (index, shape) in self.shapes.iter().enumerate() {
            match &shape.geometry {
                SnapshotGeometry::Box(box_shape) => {
                    if box_shape.size.iter().any(|extent| !(*extent > 0.0)) {
                        return Err(SnapshotError::Invalid(format!(
                            "shape {index}: box size must be positive"
                        )));
                    }
                }
                SnapshotGeometry::Mesh(mesh) => {
                    if mesh.positions.is_empty() {
                        return Err(SnapshotError::Invalid(format!(
                            "shape {index}: mesh has no positions"
                        )));
                    }
                    if !mesh.normals.is_empty() && mesh.normals.len() != mesh.positions.len() {
                        return Err(SnapshotError::Invalid(format!(
                            "shape {index}: {} normals for {} positions",
                            mesh.normals.len(),
                            mesh.positions.len()
                        )));
                    }
                    if mesh.triangles.len() % 3 != 0 || mesh.edges.len() % 2 != 0 {
                        return Err(SnapshotError::Invalid(format!(
                            "shape {index}: triangles are not triples or edges are not pairs"
                        )));
                    }
                    let vertex_count = mesh.positions.len() as u32;
                    if mesh
                        .triangles
                        .iter()
                        .chain(mesh.edges.iter())
                        .any(|index| *index >= vertex_count)
                    {
                        return Err(SnapshotError::Invalid(format!(
                            "shape {index}: an index is outside the position list"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Build the stage the renderer draws.
    pub fn to_stage(&self) -> Stage {
        let mut stage = Stage::new();
        for (index, spec) in self.shapes.iter().enumerate() {
            let mode = spec.draw_mode.unwrap_or(SnapshotDrawMode::ShadedWithEdges);
            let mut item = ShapeItem::new((index as u32) + 1).with_draw_mode(mode.into());
            match &spec.geometry {
                SnapshotGeometry::Box(box_shape) => {
                    let (positions, triangles, edges) = box_shape.tessellate();
                    item.set_shape(positions, Vec::new(), triangles, edges);
                }
                SnapshotGeometry::Mesh(mesh) => {
                    item.set_shape(
                        mesh.positions.clone(),
                        mesh.normals.clone(),
                        mesh.triangles.clone(),
                        mesh.edges.clone(),
                    );
                }
            }
            if let Some(color) = spec.color {
                item = item.with_surface_color(color);
            }
            if let Some(color) = spec.edge_color {
                item = item.with_edge_color(color);
            }
            stage.add_item(item);
        }
        stage
    }

    /// The camera this spec describes for its own viewport size.
    pub fn camera(&self) -> Camera {
        self.camera.to_camera(self.size)
    }
}

/// Render `spec` on its own device and read the frame back.
///
/// Each call creates a fresh adapter/device, so two calls produce independent
/// renders; that is what the determinism test compares.
pub fn render(spec: &SnapshotSpec) -> Result<ImageData, SnapshotError> {
    spec.validate()?;
    let stage = spec.to_stage();
    let queue = stage.to_queue();
    let camera = spec.camera();
    let mut renderer = WgpuRenderer::new_default(spec.size)?;
    renderer.draw_frame(&[ViewSpec {
        queue: &queue,
        camera: &camera,
        rect: None,
        overlay: false,
    }])?;
    Ok(renderer.image()?)
}

/// Encode read-back RGBA8 as a PNG.
///
/// No metadata chunks are written (no timestamp), and the compressor settings
/// are fixed, so identical pixels give identical bytes.
pub fn encode_png(image: &ImageData) -> Result<Vec<u8>, SnapshotError> {
    let expected = image.width as usize * image.height as usize * 4;
    if image.rgba.len() != expected {
        return Err(SnapshotError::Invalid(format!(
            "{} bytes for a {}×{} RGBA image",
            image.rgba.len(),
            image.width,
            image.height
        )));
    }
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            &image.rgba,
            image.width,
            image.height,
            image::ExtendedColorType::Rgba8,
        )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::tests::gpu_guard;

    fn box_spec() -> SnapshotSpec {
        SnapshotSpec {
            size: [96, 64],
            camera: SnapshotCamera {
                eye: [-120.0, -160.0, 120.0],
                target: [10.0, 10.0, 10.0],
                up: [0.0, 0.0, 1.0],
                fov_y: 0.7,
            },
            shapes: vec![SnapshotShape::box_of([20.0, 10.0, 6.0], [0.0, 0.0, 0.0])],
        }
    }

    #[test]
    fn a_box_spec_is_a_closed_outward_wound_mesh() {
        let (positions, triangles, edges) = SnapshotBox {
            size: [2.0, 3.0, 4.0],
            origin: [1.0, 2.0, 3.0],
        }
        .tessellate();
        assert_eq!(positions.len(), 8);
        assert_eq!(triangles.len(), 36, "12 triangles");
        // Like the toy display provider, each of the 12 box edges is emitted once
        // per adjacent face: 24 segments, 48 indices.
        assert_eq!(edges.len(), 48, "12 box edges, twice");
        assert!(
            positions
                .iter()
                .all(|p| p[0] >= 1.0 && p[1] >= 2.0 && p[2] >= 3.0)
        );

        // Every outward face normal points away from the box's centre.
        let centre = [2.0, 3.5, 5.0];
        for tri in triangles.chunks_exact(3) {
            let a = positions[tri[0] as usize];
            let b = positions[tri[1] as usize];
            let c = positions[tri[2] as usize];
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let middle = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let outward = [
                middle[0] - centre[0],
                middle[1] - centre[1],
                middle[2] - centre[2],
            ];
            assert!(
                n[0] * outward[0] + n[1] * outward[1] + n[2] * outward[2] > 0.0,
                "triangle {tri:?} winds inward"
            );
        }
    }

    #[test]
    fn the_camera_looks_down_negative_z_at_the_target() {
        let camera = SnapshotCamera {
            eye: [0.0, 0.0, 10.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y: 0.6,
        }
        .to_camera([100, 100]);
        // The target maps to view-space (0, 0, −10); a point 1 unit right maps +X.
        let view_target = camera.view.transform(&Vector3::zero());
        assert!(view_target.x.abs() < 1e-9);
        assert!(view_target.y.abs() < 1e-9);
        assert!((view_target.z + 10.0).abs() < 1e-9);
        let right = camera.view.transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(right.x > 0.9);
        assert!(right.y.abs() < 1e-9);
    }

    #[test]
    fn a_bad_spec_is_rejected_before_any_gpu_work() {
        let mut spec = box_spec();
        spec.camera.eye = spec.camera.target;
        assert!(matches!(spec.validate(), Err(SnapshotError::Invalid(_))));

        let mut spec = box_spec();
        spec.shapes[0] = SnapshotShape::mesh(SnapshotMesh {
            positions: vec![[0.0, 0.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            edges: Vec::new(),
        });
        assert!(matches!(spec.validate(), Err(SnapshotError::Invalid(_))));

        let mut spec = box_spec();
        spec.shapes.clear();
        assert!(matches!(spec.validate(), Err(SnapshotError::Invalid(_))));
    }

    #[test]
    fn the_spec_round_trips_through_json() {
        let spec = box_spec();
        let json = serde_json::to_string(&spec).expect("serialise");
        let parsed: SnapshotSpec = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed, spec);
        // The Python-facing spellings are accepted.
        let mesh: SnapshotShape = serde_json::from_str(
            r#"{"geometry":{"mesh":{"positions":[[0,0,0],[1,0,0],[0,1,0]],
                "triangles":[0,1,2]}},
                "draw_mode":"shaded_edges"}"#,
        )
        .expect("mesh spec");
        assert_eq!(mesh.draw_mode, Some(SnapshotDrawMode::ShadedWithEdges));
    }

    #[test]
    fn renders_at_the_requested_size_and_repeats_byte_for_byte() {
        let _gpu = gpu_guard();
        let spec = box_spec();
        let first = match render(&spec) {
            Ok(image) => image,
            Err(error) if error.is_adapter_missing() => {
                eprintln!("no GPU adapter available; skipping snapshot render test");
                return;
            }
            Err(error) => panic!("render failed: {error}"),
        };
        assert_eq!([first.width, first.height], spec.size);
        assert_eq!(first.rgba.len(), 96 * 64 * 4);
        let png = encode_png(&first).expect("encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR: width, height, bit depth 8, colour type 6 (RGBA).
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 96);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 64);
        assert_eq!(png[24], 8);
        assert_eq!(png[25], 6);
        // No tIME chunk (it would prefix "tIME" in some ancillary chunk).
        assert!(
            !png.windows(4).any(|window| window == b"tIME"),
            "a snapshot must not carry a timestamp"
        );

        // A second, independent render (fresh adapter and device) is identical.
        let second = render(&spec).expect("second render");
        assert_eq!(first.rgba, second.rgba);
        assert_eq!(png, encode_png(&second).expect("second encode"));

        // The box covers the middle of the frame and not the corner.
        let texel = |x: u32, y: u32| {
            let i = ((y * 96 + x) * 4) as usize;
            [first.rgba[i], first.rgba[i + 1], first.rgba[i + 2]]
        };
        assert_ne!(texel(48, 32), texel(0, 0));
    }
}

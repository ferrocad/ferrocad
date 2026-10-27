//! The published view: the camera and stage a host last drew, and the picks over it.
//!
//! A canvas-first workbench (SimpleCAD) reaches the viewport through
//! `FreeCADGui.ActiveDocument.ActiveView`: `getPointOnScreen`, `getViewDirection`,
//! `getCameraNode` and `getObjectsInfo`. The Python facade cannot see the shell's
//! `InventorViewport`, so the host **publishes** the two things those calls need:
//!
//! * the camera, on every shell render ([`publish_camera`]);
//! * the compiled stage and its entity/part-name tables, whenever the document's
//!   display changes ([`publish_stage`]).
//!
//! Both are process-global because a process has one window and one active document
//! view. A headless script (or a test) publishes the same two values through
//! `ferrocad_stage.set_view_camera` / `set_view_stage`, so the facade behaves
//! identically without a shell.
//!
//! [`pick_at`] is the **ranked** pick S4.2 asks for: `ferrocad_graphics`' picker
//! returns the nearest hit only, while this one returns every distinct
//! `(entity, sub-element)` under the pointer, nearest first — the occlusion order a
//! tool wants when it sorts its own candidates. The rules match
//! `ferrocad_graphics::pick::pick_at_hit` (pickable packets only, no shader-program
//! placeholders, triangles and line segments, a screen-space line tolerance), with
//! one addition: an **untagged** item (`PickId(0)`, what a non-selectable
//! `ShapeItem` emits) is never reported, because a click cannot select it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_graphics as gfx;
use ferrocad_graphics::pick::LINE_PICK_PIXELS;
use ferrocad_stage as fs;
use ferrocad_types::{Matrix4, Vector3};
use pyo3::prelude::*;

use crate::camera::Camera;
use crate::stage::Stage;

/// The stage a host last drew, compiled once so a pick does not recompile it.
struct PublishedStage {
    /// The stage's draw queue: the same packets the viewport picks against.
    queue: gfx::RenderQueue,
    /// Pick id → document object name (`"Slab"`).
    objects: HashMap<u32, String>,
    /// Pick id → sub-element name table (`["", "Face1", …]`).
    parts: HashMap<u32, Vec<String>>,
}

fn camera_cell() -> &'static Mutex<Option<gfx::Camera>> {
    static CELL: OnceLock<Mutex<Option<gfx::Camera>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn stage_cell() -> &'static Mutex<Option<Arc<PublishedStage>>> {
    static CELL: OnceLock<Mutex<Option<Arc<PublishedStage>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Publish the camera the shell is drawing with.
///
/// Cheap enough to call every frame: two matrices and the viewport.
pub fn publish_camera(camera: gfx::Camera) {
    *camera_cell().lock().unwrap() = Some(camera);
}

/// Publish the compiled stage and the name tables a pick reports.
///
/// `objects` maps a pick id to its document object name; `parts` optionally overrides
/// the per-item name tables (a host that builds the stage itself leaves it `None`, and
/// the items' own [`ferrocad_stage::ShapeItem::part_names`] are used).
pub fn publish_stage(
    stage: &fs::Stage,
    objects: HashMap<u32, String>,
    parts: Option<HashMap<u32, Vec<String>>>,
) {
    let parts = parts.unwrap_or_else(|| {
        stage
            .items()
            .iter()
            .filter(|item| !item.part_names().is_empty())
            .map(|item| (item.entity_id(), item.part_names().to_vec()))
            .collect()
    });
    let published = PublishedStage {
        queue: stage.to_queue(),
        objects,
        parts,
    };
    *stage_cell().lock().unwrap() = Some(Arc::new(published));
}

/// Forget the published view (document close, or a headless test's teardown).
pub fn clear_view() {
    *camera_cell().lock().unwrap() = None;
    *stage_cell().lock().unwrap() = None;
}

/// The published camera, if a host has published one.
pub fn published_camera() -> Option<gfx::Camera> {
    camera_cell().lock().unwrap().clone()
}

/// Whether a stage has been published.
///
/// The shell uses this to publish the stage the viewport was **built** with (boot,
/// or a viewport recreated for a new GPU adapter) exactly once, without cloning it on
/// every frame.
pub fn has_stage() -> bool {
    stage_cell().lock().unwrap().is_some()
}

/// One pick result: what the pointer is over and where.
///
/// The facade maps this to SimpleCAD's `{Document, Object, Component, x, y, z}` dict;
/// the fields here are the raw view-side facts, with `object`/`part` already resolved
/// through the tables published with the stage.
#[pyclass(module = "ferrocad_stage", name = "PickHit")]
#[derive(Clone, Debug)]
pub struct PickHit {
    /// The object that was hit (the packet's pick id).
    #[pyo3(get)]
    entity_id: u32,
    /// The sub-element id (`Face6` is `part_idx` 6 on the item's name table).
    #[pyo3(get)]
    part_idx: u32,
    /// The sub-element name (`"Face6"`), when the stage published a name table.
    #[pyo3(get)]
    part: Option<String>,
    /// The document object name (`"Body"`), when the stage published one.
    #[pyo3(get)]
    object: Option<String>,
    /// The world-space intersection point (`f32`, like the GPU G-buffer's pick).
    #[pyo3(get)]
    point: (f32, f32, f32),
    /// The world-space surface normal (`+Z` for a line hit).
    #[pyo3(get)]
    normal: (f32, f32, f32),
    /// The distance along the ray from its origin.
    #[pyo3(get)]
    distance: f64,
    /// The topology of the hit: `"Triangles"` or `"Lines"`.
    #[pyo3(get)]
    topology: &'static str,
    /// The primitive ordinal within its mesh (a triangle or segment index).
    #[pyo3(get)]
    element: u32,
}

#[pymethods]
impl PickHit {
    fn __repr__(&self) -> String {
        format!(
            "PickHit(object={:?}, part={:?}, point={:?}, distance={:.6})",
            self.object, self.part, self.point, self.distance
        )
    }
}

/// A raw hit before the name tables are applied.
struct RawHit {
    entity: u32,
    part: u32,
    element: u32,
    point: [f32; 3],
    normal: [f32; 3],
    distance: f64,
    topology: &'static str,
}

/// Every distinct `(entity, sub-element)` under `device`, nearest first.
///
/// A dense mesh reports one hit per triangle; only the nearest triangle of each
/// sub-element is kept, so the list is a **ranked occlusion stack of sub-elements**
/// rather than hundreds of coplanar fragments of the same face.
fn pick_all(
    view: &PublishedStage,
    camera: &gfx::Camera,
    device: [i32; 2],
    max_hits: usize,
) -> Vec<RawHit> {
    let Some(ray) = camera.ray_through(device) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for packet in &view.queue.packets {
        // Same rule as `ferrocad_graphics`' pick: an authored program draws a
        // placeholder quad, and an unpickable packet opts out outright.
        if packet.state.program.is_some() || !packet.state.pickable {
            continue;
        }
        // An untagged item is not selectable, so a click could not select it either;
        // reporting it would name no object.
        if packet.state.id.0 == 0 {
            continue;
        }
        let Some(geometry) = &packet.geometry else {
            continue;
        };
        let chunk = match packet.topology {
            gfx::Topology::Triangles => 3,
            gfx::Topology::Lines => 2,
            gfx::Topology::Points => continue,
        };
        let world: Vec<[f64; 3]> = geometry
            .vertices
            .iter()
            .map(|vertex| transform(&packet.state.world_transform, vertex.position))
            .collect();
        for (element, indices) in geometry.indices.chunks_exact(chunk).enumerate() {
            let (Some(&a), Some(&b)) = (
                world.get(indices[0] as usize),
                world.get(indices[1] as usize),
            ) else {
                continue;
            };
            let (distance, normal) = if chunk == 3 {
                let Some(&c) = world.get(indices[2] as usize) else {
                    continue;
                };
                let Some(distance) = ray.triangle_distance(a, b, c) else {
                    continue;
                };
                (distance, normal_of(a, b, c))
            } else {
                // The screen-space tolerance the CPU pick uses for a line.
                let middle = Vector3::new(
                    (a[0] + b[0]) * 0.5,
                    (a[1] + b[1]) * 0.5,
                    (a[2] + b[2]) * 0.5,
                );
                let tolerance = fs::screen::scale_for(camera, middle, LINE_PICK_PIXELS);
                let Some(distance) = ray.segment_distance(a, b, tolerance) else {
                    continue;
                };
                (distance, [0.0, 0.0, 1.0])
            };
            let point = [
                ray.origin[0] + ray.direction[0] * distance,
                ray.origin[1] + ray.direction[1] * distance,
                ray.origin[2] + ray.direction[2] * distance,
            ];
            found.push(RawHit {
                entity: packet.state.id.0,
                part: geometry
                    .vertices
                    .get(indices[0] as usize)
                    .map_or(0, |vertex| vertex.part_idx),
                element: element as u32,
                point: [
                    point[0] as f32,
                    point[1] as f32,
                    point[2] as f32,
                ],
                normal: [normal[0] as f32, normal[1] as f32, normal[2] as f32],
                distance,
                topology: match packet.topology {
                    gfx::Topology::Triangles => "Triangles",
                    _ => "Lines",
                },
            });
        }
    }
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let mut seen = HashSet::new();
    found.retain(|hit| seen.insert((hit.entity, hit.part)));
    if max_hits > 0 {
        found.truncate(max_hits);
    }
    found
}

/// One `(entity, part)` hit with its published names.
fn resolve(view: &PublishedStage, raw: RawHit) -> PickHit {
    PickHit {
        entity_id: raw.entity,
        part_idx: raw.part,
        part: view
            .parts
            .get(&raw.entity)
            .and_then(|names| names.get(raw.part as usize))
            .filter(|name| !name.is_empty())
            .cloned(),
        object: view.objects.get(&raw.entity).cloned(),
        point: (raw.point[0], raw.point[1], raw.point[2]),
        normal: (raw.normal[0], raw.normal[1], raw.normal[2]),
        distance: raw.distance,
        topology: raw.topology,
        element: raw.element,
    }
}

/// The ranked hits under device position `(x, y)` (bottom-left origin).
///
/// Empty when no host has published a stage, or when the pointer is over nothing.
/// `max_hits` caps the list; `0` means every hit.
#[pyfunction]
#[pyo3(signature = (x, y, max_hits=0))]
fn pick_at(x: i32, y: i32, max_hits: usize) -> Vec<PickHit> {
    let (Some(camera), Some(stage)) = (
        camera_cell().lock().unwrap().clone(),
        stage_cell().lock().unwrap().clone(),
    ) else {
        return Vec::new();
    };
    pick_all(&stage, &camera, [x, y], max_hits)
        .into_iter()
        .map(|raw| resolve(&stage, raw))
        .collect()
}

/// Publish the camera the facade reports (a script's or a test's stand-in for the shell).
#[pyfunction]
fn set_view_camera(camera: &Camera) {
    publish_camera(camera.inner.clone());
}

/// Publish the stage a pick walks, with the name tables it reports.
///
/// `objects` maps a pick id to a document object name. `parts` maps a pick id to the
/// sub-element name table (`{7: ["", "Face1", …]}`); when omitted, the items' own
/// tables are used, which is what a host that builds the stage from kernel
/// tessellations gets for free.
#[pyfunction]
#[pyo3(signature = (stage, objects=None, parts=None))]
fn set_view_stage(
    stage: &Stage,
    objects: Option<HashMap<u32, String>>,
    parts: Option<HashMap<u32, Vec<String>>>,
) {
    publish_stage(&stage.inner, objects.unwrap_or_default(), parts);
}

/// The published camera, or `None` when no host has published one.
#[pyfunction]
fn view_camera() -> Option<Camera> {
    published_camera().map(Camera::wrap)
}

/// Forget the published camera and stage.
#[pyfunction]
#[pyo3(name = "clear_view")]
fn py_clear_view() {
    clear_view();
}

/// Register this module's functions and classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PickHit>()?;
    m.add_function(wrap_pyfunction!(set_view_camera, m)?)?;
    m.add_function(wrap_pyfunction!(set_view_stage, m)?)?;
    m.add_function(wrap_pyfunction!(view_camera, m)?)?;
    m.add_function(wrap_pyfunction!(pick_at, m)?)?;
    m.add_function(wrap_pyfunction!(py_clear_view, m)?)?;
    Ok(())
}

// --- small vector helpers (f64, row-major) --------------------------------------

fn transform(m: &Matrix4, p: [f32; 3]) -> [f64; 3] {
    let v = Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
    let out = m.transform(&v);
    [out.x, out.y, out.z]
}

fn normal_of(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if length > 1e-12 {
        [n[0] / length, n[1] / length, n[2] / length]
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_stage::ShapeItem;

    fn camera(size: [u32; 2], eye_z: f64) -> gfx::Camera {
        gfx::Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -eye_z),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            projection: gfx::Projection::Orthographic {
                extent: 10.0,
                near: -100.0,
                far: 100.0,
            },
            viewport: gfx::Viewport::new(size, 1.0),
        }
    }

    /// A unit square in the z = 0 plane, one quad per face name.
    fn quad(entity: u32, z: f32, name: &str) -> ShapeItem {
        let positions = vec![
            [-1.0, -1.0, z],
            [1.0, -1.0, z],
            [1.0, 1.0, z],
            [-1.0, 1.0, z],
        ];
        let item = ShapeItem::new(entity).with_mesh(
            positions,
            Vec::new(),
            vec![0, 1, 2, 0, 2, 3],
            Vec::new(),
            vec![1, 1, 1, 1],
        );
        // `set_mesh` clears the name table; the pick reads it, so set it back.
        let mut item = item;
        item.set_tessellation(&ferrocad_geom::Tessellation {
            positions: item.positions().to_vec(),
            normals: Vec::new(),
            triangles: item.triangles().to_vec(),
            triangle_parts: vec![1, 1],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["".to_string(), name.to_string()],
        });
        item
    }

    #[test]
    fn the_nearest_sub_element_ranks_first() {
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        stage.add_item(quad(2, -5.0, "Face2"));
        let mut objects = HashMap::new();
        objects.insert(1, "Near".to_string());
        objects.insert(2, "Far".to_string());
        publish_stage(&stage, objects, None);

        let camera = camera([100, 100], 50.0);
        let hits = pick_all(
            &stage_cell().lock().unwrap().clone().unwrap(),
            &camera,
            [50, 50],
            0,
        );
        assert_eq!(hits.len(), 2, "one hit per sub-element, not per triangle");
        assert_eq!(hits[0].entity, 1, "the nearer quad wins");
        assert_eq!(hits[1].entity, 2);
    }

    #[test]
    fn the_top_hit_agrees_with_the_single_hit_picker() {
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        stage.add_item(quad(2, -5.0, "Face2"));
        let camera = camera([100, 100], 50.0);
        let ranked = pick_all(
            &PublishedStage {
                queue: stage.to_queue(),
                objects: HashMap::new(),
                parts: HashMap::new(),
            },
            &camera,
            [50, 50],
            0,
        );
        let single = gfx::pick::pick_at_hit(&stage.to_queue(), &camera, [50, 50])
            .expect("the CPU picker finds the quads");
        assert_eq!(ranked[0].entity, single.id.0);
        assert_eq!(ranked[0].part, single.part);
        assert!((ranked[0].distance - single.distance).abs() < 1e-9);
    }

    #[test]
    fn empty_space_reports_nothing() {
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        let camera = camera([100, 100], 50.0);
        let hits = pick_all(
            &PublishedStage {
                queue: stage.to_queue(),
                objects: HashMap::new(),
                parts: HashMap::new(),
            },
            &camera,
            [0, 0],
            0,
        );
        assert!(hits.is_empty());
    }
}

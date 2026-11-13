//! The published view: the camera and stage a host last drew, and the picks over it.
//!
//! A canvas-first workbench (SimpleCAD) reaches the viewport through
//! `FreeCADGui.ActiveDocument.ActiveView`: `getPointOnScreen`, `getViewDirection`,
//! `getCameraNode` and `getObjectsInfo`. The Python facade cannot see the shell's
//! `InventorViewport`, so the host **publishes** the two things those calls need:
//!
//! * the camera, on every shell render and on every viewport notification
//!   ([`publish_camera`]);
//! * the compiled stage and its entity/part-name tables, whenever the document's
//!   display changes ([`publish_stage`]).
//!
//! Both are process-global because a process has one window and one active document
//! view. A headless script (or a test) publishes the same two values through
//! `ferrocad_stage.set_view_camera` / `set_view_stage`, so the facade behaves
//! identically without a shell.
//!
//! A host also publishes **transient preview items** here (S4.6,
//! [`set_preview_items`]). They are drawn beside the stage but are not part of the
//! published pick stage, and each item is forced non-selectable, so a preview never
//! answers a click. [`preview_generation`] is the cheap change signal a host polls to
//! redraw without cloning the list.
//!
//! [`pick_at`] is the **ranked** pick S4.2 asks for: `ferrocad_graphics`' picker
//! returns the nearest hit only, while this one returns every distinct
//! `(entity, sub-element)` under the pointer, nearest first — the occlusion order a
//! tool wants when it sorts its own candidates. The rules match
//! `ferrocad_graphics::pick::pick_at_hit` (pickable packets only, no shader-program
//! placeholders, triangles and line segments, a screen-space line tolerance), with
//! one addition: an **untagged** item (`PickId(0)`, what a non-selectable
//! `ShapeItem` emits) is never reported, because a click cannot select it.

use std::collections::HashMap;

use ferrocad_graphics as gfx;
use ferrocad_stage as fs;
use pyo3::prelude::*;

use crate::camera::Camera;
use crate::stage::Stage;
use crate::workplane::{Workplane, WorkplaneState};

/// The published camera and stage live in the pure stage crate so a platform-neutral
/// host can publish them without linking CPython; these are the same functions.
pub use fs::view::{
    clear_previews, clear_view, has_stage, preview_generation, preview_items, publish_camera,
    publish_stage, publish_workplane, published_camera, published_face_frame, published_stage,
    published_workplane, set_preview_items,
};
// The ranked picker's unit tests build a stage by hand; the binding itself no longer
// names the published type (the picker lives in the pure crate now).
#[cfg(test)]
pub use fs::view::PublishedStage;

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

impl PickHit {
    /// The Python view of one shared [`fs::view::RankedHit`].
    fn from_ranked(hit: fs::view::RankedHit) -> Self {
        Self {
            entity_id: hit.entity_id,
            part_idx: hit.part_idx,
            part: hit.part,
            object: hit.object,
            point: (hit.point[0], hit.point[1], hit.point[2]),
            normal: (hit.normal[0], hit.normal[1], hit.normal[2]),
            distance: hit.distance,
            topology: match hit.topology {
                gfx::Topology::Triangles => "Triangles",
                gfx::Topology::Lines => "Lines",
                gfx::Topology::Points => "Points",
            },
            element: hit.element,
        }
    }
}

/// The ranked hits under device position `(x, y)` (bottom-left origin).
///
/// Empty when no host has published a stage, or when the pointer is over nothing.
/// `max_hits` caps the list; `0` means every hit.
#[pyfunction]
#[pyo3(signature = (x, y, max_hits=0))]
fn pick_at(x: i32, y: i32, max_hits: usize) -> Vec<PickHit> {
    fs::view::ranked_pick([x, y], max_hits)
        .into_iter()
        .map(PickHit::from_ranked)
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

/// Publish the transient preview items a script wants drawn (S4.6).
///
/// Each item is forced unpickable, so a preview never answers a click and the ranked
/// picker never reports it. Returns the new generation, which a host polls to redraw
/// without cloning the list.
#[pyfunction]
#[pyo3(name = "set_preview_items")]
fn py_set_preview_items(items: Vec<PyRef<'_, crate::shape::ShapeItem>>) -> u64 {
    set_preview_items(items.iter().map(|item| item.inner.clone()).collect())
}

/// The published transient preview items.
#[pyfunction]
#[pyo3(name = "preview_items")]
fn py_preview_items() -> Vec<crate::shape::ShapeItem> {
    preview_items()
        .into_iter()
        .map(crate::shape::ShapeItem::wrap)
        .collect()
}

/// Drop every transient preview.
#[pyfunction]
#[pyo3(name = "clear_previews")]
fn py_clear_previews() -> u64 {
    clear_previews()
}

/// A counter that changes whenever the preview set does.
#[pyfunction]
#[pyo3(name = "preview_generation")]
fn py_preview_generation() -> u64 {
    preview_generation()
}

/// Publish the active workplane a host is drawing with (S4.4).
///
/// A headless script or test uses this the way the shell does: the plane becomes what
/// `FreeCADGui.ActiveDocument.ActiveView.getWorkplane()` returns. `extent` is the
/// in-plane radius of a bound surface (the grid's patch), and `label` is the pill's
/// name (`"XY"`, `"Face3"`, `"Datum"`).
#[pyfunction]
#[pyo3(signature = (workplane, label="Custom", offset=0.0, extent=None))]
fn set_view_workplane(
    workplane: &Workplane,
    label: &str,
    offset: f64,
    extent: Option<f64>,
) {
    publish_workplane(fs::view::PublishedWorkplane {
        plane: workplane.inner.clone(),
        label: label.to_string(),
        offset,
        extent,
    });
}

/// The published active workplane, or `None` when no host has published one.
#[pyfunction]
fn view_workplane() -> Option<WorkplaneState> {
    published_workplane().map(WorkplaneState::from)
}

/// The pick id whose published object name is `object`, or `None`.
#[pyfunction]
fn entity_of(object: &str) -> Option<u32> {
    published_stage()?.entity_named(object)
}

/// The exact planar frame of `element` on `entity`, resolved from the published stage.
///
/// The frame is the one the viewport's snap context uses: the area-weighted centroid
/// and normal of the element's display triangles, and the extent of its vertices. A
/// named element that is not a planar face — or an entity the current stage no longer
/// has — returns `None`, so a caller cannot read a previous frame after a deletion.
#[pyfunction]
fn face_plane(entity: u32, element: &str) -> Option<WorkplaneState> {
    let frame = published_face_frame(entity, element)?;
    Some(WorkplaneState::from(fs::view::PublishedWorkplane {
        plane: frame.plane,
        label: element.to_string(),
        offset: 0.0,
        extent: Some(frame.radius),
    }))
}

/// Register this module's functions and classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PickHit>()?;
    m.add_function(wrap_pyfunction!(set_view_camera, m)?)?;
    m.add_function(wrap_pyfunction!(set_view_stage, m)?)?;
    m.add_function(wrap_pyfunction!(set_view_workplane, m)?)?;
    m.add_function(wrap_pyfunction!(view_workplane, m)?)?;
    m.add_function(wrap_pyfunction!(entity_of, m)?)?;
    m.add_function(wrap_pyfunction!(face_plane, m)?)?;
    m.add_function(wrap_pyfunction!(view_camera, m)?)?;
    m.add_function(wrap_pyfunction!(pick_at, m)?)?;
    m.add_function(wrap_pyfunction!(py_clear_view, m)?)?;
    m.add_function(wrap_pyfunction!(py_set_preview_items, m)?)?;
    m.add_function(wrap_pyfunction!(py_preview_items, m)?)?;
    m.add_function(wrap_pyfunction!(py_clear_previews, m)?)?;
    m.add_function(wrap_pyfunction!(py_preview_generation, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_stage::ShapeItem;
    use ferrocad_types::Vector3;
    use std::sync::Mutex;

    /// The published view is process-global, so the tests that write it must not
    /// run concurrently with each other (`publish_stage` would make `has_stage`
    /// true under the preview test).
    fn view_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        let _guard = view_lock();
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        stage.add_item(quad(2, -5.0, "Face2"));
        let mut objects = HashMap::new();
        objects.insert(1, "Near".to_string());
        objects.insert(2, "Far".to_string());
        publish_stage(&stage, objects, None);

        let camera = camera([100, 100], 50.0);
        let hits = fs::view::ranked_pick_in(
            &published_stage().expect("the stage was published"),
            &camera,
            [50, 50],
            0,
        );
        assert_eq!(hits.len(), 2, "one hit per sub-element, not per triangle");
        assert_eq!(hits[0].entity_id, 1, "the nearer quad wins");
        assert_eq!(hits[1].entity_id, 2);
    }

    #[test]
    fn the_top_hit_agrees_with_the_single_hit_picker() {
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        stage.add_item(quad(2, -5.0, "Face2"));
        let camera = camera([100, 100], 50.0);
        let ranked = fs::view::ranked_pick_in(
            &PublishedStage::new(stage.to_queue(), HashMap::new(), HashMap::new()),
            &camera,
            [50, 50],
            0,
        );
        let single = gfx::pick::pick_at_hit(&stage.to_queue(), &camera, [50, 50])
            .expect("the CPU picker finds the quads");
        assert_eq!(ranked[0].entity_id, single.id.0);
        assert_eq!(ranked[0].part_idx, single.part);
        assert!((ranked[0].distance - single.distance).abs() < 1e-9);
    }

    #[test]
    fn empty_space_reports_nothing() {
        let mut stage = fs::Stage::new();
        stage.add_item(quad(1, 0.0, "Face1"));
        let camera = camera([100, 100], 50.0);
        let hits = fs::view::ranked_pick_in(
            &PublishedStage::new(stage.to_queue(), HashMap::new(), HashMap::new()),
            &camera,
            [0, 0],
            0,
        );
        assert!(hits.is_empty());
    }

    /// S4.6: a preview is drawn by the host but never published as a pick target, and
    /// the generation counter is what tells the host to redraw.
    #[test]
    fn previews_are_unpickable_and_bump_the_generation() {
        let _guard = view_lock();
        clear_view();
        clear_previews();
        let before = preview_generation();

        // A selectable item is forced unpickable by the channel, so a preview can
        // never answer a click even when the caller forgot.
        let preview = quad(9, 1.0, "Face1");
        assert!(preview.selectable());
        let generation = set_preview_items(vec![preview]);
        assert_ne!(
            generation, before,
            "publishing a preview bumps the generation"
        );

        let items = preview_items();
        assert_eq!(items.len(), 1);
        assert!(!items[0].selectable(), "a preview is never selectable");
        // The published pick stage is the document, not the preview set: a host that
        // only publishes the document stage cannot report a preview.
        assert!(!has_stage());

        clear_previews();
        assert!(preview_items().is_empty());
        assert_ne!(preview_generation(), generation);
        set_preview_items(Vec::new());
    }
}

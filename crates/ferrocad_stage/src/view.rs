//! The published view: the camera and stage a host last drew.
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
//! The **active workplane** is published the same way (S4.4, [`publish_workplane`]):
//! the viewport owns it, and a script (SimpleCAD's snapping) needs its origin, axes and
//! normal without a window. [`published_face_frame`] resolves a named planar face from
//! the published stage with the same [`FaceFrame`] rule the viewport's own snap context
//! uses, so a facade answer agrees with the grid the user sees rather than approximating
//! it from a second source.
//!
//! The state lives here, in the pure stage crate, rather than in the Python facade:
//! the shell and the facade are separate crates and a platform-neutral host (the
//! browser) must be able to publish the same queue without linking CPython. The
//! `ferrocad_stage_py` functions are thin wrappers over this module.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_graphics as gfx;
use ferrocad_graphics::pick::LINE_PICK_PIXELS;
use ferrocad_types::{Matrix4, Vector3};

use crate::workplane::{FaceFrame, Workplane};
use crate::ShapeItem;

/// The stage a host last drew, compiled once so a pick does not recompile it.
pub struct PublishedStage {
    /// The stage's draw queue: the same packets the viewport picks against.
    queue: gfx::RenderQueue,
    /// Pick id → document object name (`"Slab"`).
    objects: HashMap<u32, String>,
    /// Pick id → sub-element name table (`["", "Face1", …]`).
    parts: HashMap<u32, Vec<String>>,
}

impl PublishedStage {
    /// Bundle a compiled queue with the name tables a pick reports.
    pub fn new(
        queue: gfx::RenderQueue,
        objects: HashMap<u32, String>,
        parts: HashMap<u32, Vec<String>>,
    ) -> Self {
        Self {
            queue,
            objects,
            parts,
        }
    }

    /// The compiled draw queue.
    pub fn queue(&self) -> &gfx::RenderQueue {
        &self.queue
    }

    /// Pick id → document object name.
    pub fn objects(&self) -> &HashMap<u32, String> {
        &self.objects
    }

    /// Pick id → sub-element name table.
    pub fn parts(&self) -> &HashMap<u32, Vec<String>> {
        &self.parts
    }

    /// The pick id whose published object name is `object` (the inverse of
    /// [`objects`](Self::objects)).
    pub fn entity_named(&self, object: &str) -> Option<u32> {
        self.objects
            .iter()
            .find_map(|(id, name)| (name == object).then_some(*id))
    }
}

fn camera_cell() -> &'static Mutex<Option<gfx::Camera>> {
    static CELL: OnceLock<Mutex<Option<gfx::Camera>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn stage_cell() -> &'static Mutex<Option<Arc<PublishedStage>>> {
    static CELL: OnceLock<Mutex<Option<Arc<PublishedStage>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The active workplane a host last published (S4.4).
///
/// This mirrors the viewport's `ActiveWorkplane` without making this crate depend on
/// the viewport: the host converts its own type into this value and publishes it. The
/// plane carries the semantic kind ([`WorkplaneKind`](crate::WorkplaneKind)), so a
/// consumer can tell a face-bound frame from a transient one.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedWorkplane {
    /// The plane: origin plus in-plane axes (the normal is `u × v`).
    pub plane: Workplane,
    /// The presentation label without the offset (`"XY"`, `"Face3"`, `"Custom"`).
    pub label: String,
    /// The shift along the plane's own normal, in mm, from the frame it was bound to.
    pub offset: f64,
    /// The in-plane radius of the finite surface the plane is bound to, if any.
    pub extent: Option<f64>,
}

fn workplane_cell() -> &'static Mutex<Option<PublishedWorkplane>> {
    static CELL: OnceLock<Mutex<Option<PublishedWorkplane>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The transient preview items a script published (S4.6).
///
/// A preview is drawn over the document but is **not** part of the published stage:
/// it is unpickable by construction (the item's `selectable` is cleared), and the
/// ranked picker therefore never reports it. The generation counter lets a host
/// that owns the draw (the shell) notice a change without cloning the list.
#[derive(Default)]
struct PreviewState {
    items: Vec<ShapeItem>,
    generation: u64,
}

fn preview_cell() -> &'static Mutex<PreviewState> {
    static CELL: OnceLock<Mutex<PreviewState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(PreviewState::default()))
}

/// Replace the transient preview items with `items`.
///
/// Every item is forced non-selectable: a preview is a visual, so a click must pass
/// through it to the geometry underneath (the same rule the grid follows). Returns
/// the new generation.
pub fn set_preview_items(items: Vec<ShapeItem>) -> u64 {
    let mut state = preview_cell().lock().unwrap_or_else(|e| e.into_inner());
    state.items = items.into_iter().map(force_unpickable).collect();
    state.generation += 1;
    state.generation
}

/// Drop every transient preview. Returns the new generation.
pub fn clear_previews() -> u64 {
    let mut state = preview_cell().lock().unwrap_or_else(|e| e.into_inner());
    state.items.clear();
    state.generation += 1;
    state.generation
}

/// The transient preview items, in publication order.
pub fn preview_items() -> Vec<ShapeItem> {
    preview_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items
        .clone()
}

/// A counter that changes whenever the preview set does.
pub fn preview_generation() -> u64 {
    preview_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation
}

fn force_unpickable(item: ShapeItem) -> ShapeItem {
    if item.selectable() {
        item.with_selectable(false)
    } else {
        item
    }
}

/// Publish the camera the shell is drawing with.
///
/// Cheap enough to call every frame: two matrices and the viewport.
pub fn publish_camera(camera: gfx::Camera) {
    *camera_cell().lock().unwrap() = Some(camera);
}

/// Publish the active workplane the shell is drawing with (S4.4).
///
/// There is always a plane in the viewport; publishing `None` therefore means “no host
/// has published one”, not “the world plane”. A host republishes this alongside the
/// camera, so a script that runs on a click sees the plane the click was made on.
pub fn publish_workplane(workplane: PublishedWorkplane) {
    *workplane_cell()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(workplane);
}

/// The published active workplane, if a host has published one.
pub fn published_workplane() -> Option<PublishedWorkplane> {
    workplane_cell()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Forget the published workplane (the host went back to an unpublished view).
pub fn clear_workplane() {
    *workplane_cell()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

/// Publish the compiled stage and the name tables a pick reports.
///
/// `objects` maps a pick id to its document object name; `parts` optionally overrides
/// the per-item name tables (a host that builds the stage itself leaves it `None`, and
/// the items' own [`ShapeItem::part_names`] are used).
pub fn publish_stage(
    stage: &crate::Stage,
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
    clear_workplane();
}

/// The published camera, if a host has published one.
pub fn published_camera() -> Option<gfx::Camera> {
    camera_cell().lock().unwrap().clone()
}

/// The published stage, if a host has published one.
pub fn published_stage() -> Option<Arc<PublishedStage>> {
    stage_cell().lock().unwrap().clone()
}

/// Whether a stage has been published.
///
/// The shell uses this to publish the stage the viewport was **built** with (boot,
/// or a viewport recreated for a new GPU adapter) exactly once, without cloning it on
/// every frame.
pub fn has_stage() -> bool {
    stage_cell().lock().unwrap().is_some()
}

// ---------------------------------------------------------------------------
// Face frames (S4.4)
// ---------------------------------------------------------------------------

/// The planar frame of `element` on `entity`, resolved from the published stage.
///
/// `None` when the stage has not been published, the entity has no such element, or
/// the element is not a planar face. The frame is the one the viewport's own snap
/// context uses: the area-weighted centroid and normal of the element's triangles, with
/// the extent covering the element's vertices. A face that was deleted after being
/// bound therefore **stops resolving** instead of returning a previous frame.
pub fn published_face_frame(entity: u32, element: &str) -> Option<FaceFrame> {
    let stage = published_stage()?;
    published_face_frame_in(&stage, entity, element)
}

/// [`published_face_frame`] against an explicit published stage.
pub fn published_face_frame_in(
    stage: &PublishedStage,
    entity: u32,
    element: &str,
) -> Option<FaceFrame> {
    let part = stage
        .parts()
        .get(&entity)?
        .iter()
        .position(|name| name == element)? as u32;
    // The surface packet carries the display tessellation the viewport resolved the
    // same face from (the pick skips an authored program's placeholder quad, and a
    // face has no program).
    let packet = stage.queue().packets.iter().find(|packet| {
        packet.state.id.0 == entity && packet.state.program.is_none()
    })?;
    let geometry = packet.geometry.as_ref()?;
    // The queue's vertices are in the item's local space; the packet's world transform
    // is the one the renderer draws with, so the frame comes out in world space.
    let mut positions = Vec::with_capacity(geometry.vertices.len());
    let mut parts = Vec::with_capacity(geometry.vertices.len());
    for vertex in &geometry.vertices {
        let world = transform(&packet.state.world_transform, vertex.position);
        positions.push([world[0] as f32, world[1] as f32, world[2] as f32]);
        parts.push(vertex.part_idx);
    }
    FaceFrame::resolve(entity, element, &positions, &geometry.indices, &parts, part)
}

// ---------------------------------------------------------------------------
// Ranked picking
// ---------------------------------------------------------------------------

/// One `(entity, sub-element)` under the pointer, with its published names.
///
/// This is the **ranked** pick: [`ferrocad_graphics`]' picker returns the nearest hit
/// only, while this reports every distinct sub-element under the device position,
/// nearest first — the occlusion stack a tool sorts its own candidates from.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedHit {
    /// The object that was hit (the packet's pick id).
    pub entity_id: u32,
    /// The sub-element id (`Face6` is `part_idx` 6 on the item's name table).
    pub part_idx: u32,
    /// The sub-element name (`"Face6"`), when the stage published a name table.
    pub part: Option<String>,
    /// The document object name (`"Body"`), when the stage published one.
    pub object: Option<String>,
    /// The world-space intersection point (the GPU G-buffer's pick precision).
    pub point: [f32; 3],
    /// The world-space surface normal (`+Z` for a line hit).
    pub normal: [f32; 3],
    /// The distance along the ray from its origin.
    pub distance: f64,
    /// The topology of the hit.
    pub topology: gfx::Topology,
    /// The primitive ordinal within its mesh (a triangle or segment index).
    pub element: u32,
}

/// Every distinct `(entity, sub-element)` under device position `(x, y)`, nearest
/// first, using the camera and stage a host last published.
///
/// Empty when no host has published a view, or when the pointer is over nothing.
/// `max_hits` caps the list; `0` means every hit.
pub fn ranked_pick(device: [i32; 2], max_hits: usize) -> Vec<RankedHit> {
    let (Some(camera), Some(stage)) = (published_camera(), published_stage()) else {
        return Vec::new();
    };
    ranked_pick_in(&stage, &camera, device, max_hits)
}

/// [`ranked_pick`] against an explicit camera and stage.
///
/// A dense mesh reports one hit per triangle; only the nearest triangle of each
/// sub-element is kept, so the list is a rank-ordered stack of sub-elements rather
/// than hundreds of coplanar fragments of the same face. An **untagged** item
/// (`PickId(0)`, what a non-selectable [`ShapeItem`] emits) is never reported,
/// because a click could not select it.
pub fn ranked_pick_in(
    view: &PublishedStage,
    camera: &gfx::Camera,
    device: [i32; 2],
    max_hits: usize,
) -> Vec<RankedHit> {
    let Some(ray) = camera.ray_through(device) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for packet in &view.queue().packets {
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
                let tolerance = crate::screen::scale_for(camera, middle, LINE_PICK_PIXELS);
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
            found.push(RawRankedHit {
                entity: packet.state.id.0,
                part: geometry
                    .vertices
                    .get(indices[0] as usize)
                    .map_or(0, |vertex| vertex.part_idx),
                element: element as u32,
                point: [point[0] as f32, point[1] as f32, point[2] as f32],
                normal: [normal[0] as f32, normal[1] as f32, normal[2] as f32],
                distance,
                topology: packet.topology,
            });
        }
    }
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let mut seen = HashSet::new();
    found.retain(|hit| seen.insert((hit.entity, hit.part)));
    if max_hits > 0 {
        found.truncate(max_hits);
    }
    found.into_iter().map(|raw| resolve(view, raw)).collect()
}

/// A raw hit before the name tables are applied.
struct RawRankedHit {
    entity: u32,
    part: u32,
    element: u32,
    point: [f32; 3],
    normal: [f32; 3],
    distance: f64,
    topology: gfx::Topology,
}

/// One `(entity, part)` hit with its published names.
fn resolve(view: &PublishedStage, raw: RawRankedHit) -> RankedHit {
    RankedHit {
        entity_id: raw.entity,
        part_idx: raw.part,
        part: view
            .parts()
            .get(&raw.entity)
            .and_then(|names| names.get(raw.part as usize))
            .filter(|name| !name.is_empty())
            .cloned(),
        object: view.objects().get(&raw.entity).cloned(),
        point: raw.point,
        normal: raw.normal,
        distance: raw.distance,
        topology: raw.topology,
        element: raw.element,
    }
}

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
    use ferrocad_geom::Tessellation;
    use std::sync::MutexGuard;

    /// The published view is process-global, so tests that write it must not race.
    fn view_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A unit quad in the plane `z = x` (its normal is `(-1, 0, 1) / √2`).
    fn tilted_quad(entity: u32, name: &str) -> ShapeItem {
        ShapeItem::new(entity).with_tessellation(&Tessellation {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 0.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3],
            triangle_parts: vec![1, 1],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec![String::new(), name.to_string()],
        })
    }

    #[test]
    fn a_published_face_frame_agrees_with_the_resolver() {
        let _guard = view_lock();
        clear_view();
        let mut stage = crate::Stage::new();
        stage.add_item(tilted_quad(7, "Face1"));
        let mut objects = HashMap::new();
        objects.insert(7, "Body".to_string());
        publish_stage(&stage, objects, None);

        let published = published_face_frame(7, "Face1").expect("a planar face resolves");
        // The viewport's own path: the item's buffers straight into the resolver.
        let item = stage.item(7).expect("the item");
        let direct = FaceFrame::resolve(
            7,
            "Face1",
            item.positions(),
            item.triangles(),
            item.parts(),
            1,
        )
        .expect("the same face resolves directly");
        assert_eq!(published.plane.origin, direct.plane.origin);
        assert_eq!(published.plane.n, direct.plane.n);
        assert!((published.radius - direct.radius).abs() < 1e-9);
        // Sanity: the normal is the tilted plane's normal, not the world +Z.
        assert!((published.plane.n.x + std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!(published.plane.n.z > 0.5);
    }

    #[test]
    fn a_stale_face_stops_resolving() {
        let _guard = view_lock();
        clear_view();
        let mut stage = crate::Stage::new();
        stage.add_item(tilted_quad(7, "Face1"));
        let mut objects = HashMap::new();
        objects.insert(7, "Body".to_string());
        publish_stage(&stage, objects, None);
        assert!(published_face_frame(7, "Face1").is_some());

        // The document edited: the entity is gone. The frame is not cached, so the
        // binding cannot answer with a previous frame.
        let mut edited = crate::Stage::new();
        edited.add_item(tilted_quad(8, "Face1"));
        let mut objects = HashMap::new();
        objects.insert(8, "Body".to_string());
        publish_stage(&edited, objects, None);
        assert!(published_face_frame(7, "Face1").is_none());

        // A curved element (an edge name on a face table) has no frame either.
        assert!(published_face_frame(8, "Edge1").is_none());
        clear_view();
    }

    #[test]
    fn the_published_workplane_round_trips_and_clears() {
        let _guard = view_lock();
        clear_view();
        assert!(published_workplane().is_none());
        let plane = Workplane::world();
        publish_workplane(PublishedWorkplane {
            plane: plane.clone(),
            label: "XY".to_string(),
            offset: 5.0,
            extent: None,
        });
        let published = published_workplane().expect("the plane was published");
        assert_eq!(published.plane, plane);
        assert_eq!(published.label, "XY");
        assert_eq!(published.offset, 5.0);

        clear_view();
        assert!(published_workplane().is_none());
    }

    #[test]
    fn an_object_name_maps_back_to_its_entity() {
        let _guard = view_lock();
        clear_view();
        let mut stage = crate::Stage::new();
        stage.add_item(tilted_quad(7, "Face1"));
        let mut objects = HashMap::new();
        objects.insert(7, "Body".to_string());
        publish_stage(&stage, objects, None);
        assert_eq!(published_stage().and_then(|s| s.entity_named("Body")), Some(7));
        assert_eq!(published_stage().and_then(|s| s.entity_named("Nope")), None);
        clear_view();
    }
}

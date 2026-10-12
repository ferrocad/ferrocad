//! The interactive transform gizmo: the replacement for Inventor's `SoDragger` family.
//!
//! A [`TransformGizmo`] draws a triad of handles (translation axes, planar quads and
//! rotation rings) at an object's origin and turns pointer drags into a delta transform,
//! which the host previews on the target item without touching the document. The handle
//! geometry is rebuilt at a **screen-space-invariant** scale: the size follows the
//! camera distance, so the gizmo keeps a fixed pixel footprint as the view zooms.
//!
//! This module is pure math and geometry — no `pyo3`, no `wgpu`. The host (the
//! viewport) picks the handles, feeds the drags, and applies the preview.

use std::f64::consts::TAU;

use ferrocad_inventor::{Camera, Ray};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::shape::{DrawMode, ShapeItem};

/// The pick id of the first gizmo handle (`GizmoHandle::ALL[0]`).
///
/// High, so it cannot collide with document entity ids.
pub const GIZMO_BASE_ID: u32 = 1_000_000;

/// The default target radius, in device pixels.
pub const DEFAULT_PIXELS: f64 = 80.0;

/// Which handles a gizmo shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GizmoMode {
    /// Translation axes and planar quads.
    #[default]
    Translate,
    /// Rotation rings.
    Rotate,
    /// Everything (translation + rotation).
    Universal,
}

/// The class of a handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// A 1D translation axis.
    Axis,
    /// A 2D constrained translation plane.
    Plane,
    /// A rotation ring.
    Ring,
}

/// A gizmo sub-handle (a degree of freedom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoHandle {
    /// Translate along X.
    AxisX,
    /// Translate along Y.
    AxisY,
    /// Translate along Z.
    AxisZ,
    /// Translate in the XY plane.
    PlaneXY,
    /// Translate in the XZ plane.
    PlaneXZ,
    /// Translate in the YZ plane.
    PlaneYZ,
    /// Rotate about X.
    RotateX,
    /// Rotate about Y.
    RotateY,
    /// Rotate about Z.
    RotateZ,
}

impl GizmoHandle {
    /// Every handle, in pick-id order.
    pub const ALL: [GizmoHandle; 9] = [
        GizmoHandle::AxisX,
        GizmoHandle::AxisY,
        GizmoHandle::AxisZ,
        GizmoHandle::PlaneXY,
        GizmoHandle::PlaneXZ,
        GizmoHandle::PlaneYZ,
        GizmoHandle::RotateX,
        GizmoHandle::RotateY,
        GizmoHandle::RotateZ,
    ];

    /// Whether this is a translation axis, a plane or a ring.
    pub fn kind(self) -> HandleKind {
        match self {
            GizmoHandle::AxisX | GizmoHandle::AxisY | GizmoHandle::AxisZ => HandleKind::Axis,
            GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => HandleKind::Plane,
            GizmoHandle::RotateX | GizmoHandle::RotateY | GizmoHandle::RotateZ => HandleKind::Ring,
        }
    }

    /// The handle's axis: the drag axis for an `Axis`, the ring normal for a `Ring`.
    /// `None` for a plane.
    pub fn axis(self) -> Option<Vector3> {
        let axis = match self {
            GizmoHandle::AxisX | GizmoHandle::RotateX => Vector3::new(1.0, 0.0, 0.0),
            GizmoHandle::AxisY | GizmoHandle::RotateY => Vector3::new(0.0, 1.0, 0.0),
            GizmoHandle::AxisZ | GizmoHandle::RotateZ => Vector3::new(0.0, 0.0, 1.0),
            GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => return None,
        };
        Some(axis)
    }

    /// The plane normal, for a `Plane` handle (`XY` → Z, `XZ` → Y, `YZ` → X).
    pub fn plane_normal(self) -> Option<Vector3> {
        let normal = match self {
            GizmoHandle::PlaneXY => Vector3::new(0.0, 0.0, 1.0),
            GizmoHandle::PlaneXZ => Vector3::new(0.0, 1.0, 0.0),
            GizmoHandle::PlaneYZ => Vector3::new(1.0, 0.0, 0.0),
            _ => return None,
        };
        Some(normal)
    }

    /// The pick id this handle carries.
    pub fn pick_id(self) -> u32 {
        GIZMO_BASE_ID + self.index() as u32
    }

    /// The handle carrying `id`, if any.
    pub fn from_pick_id(id: u32) -> Option<GizmoHandle> {
        let index = id.checked_sub(GIZMO_BASE_ID)? as usize;
        GizmoHandle::ALL.get(index).copied()
    }

    fn index(self) -> usize {
        GizmoHandle::ALL
            .iter()
            .position(|handle| *handle == self)
            .expect("handle is one of ALL")
    }
}

/// A gizmo drag tick.
#[derive(Clone, Debug)]
pub struct GizmoDragEvent {
    /// The targeted entity id.
    pub target_id: u32,
    /// The handle being dragged.
    pub active_handle: GizmoHandle,
    /// The transform delta since the previous tick.
    pub delta_transform: Matrix4,
    /// The transform accumulated since the drag began.
    pub total_transform: Matrix4,
}

/// A gizmo drag commit (on release).
#[derive(Clone, Debug)]
pub struct GizmoCommitEvent {
    /// The targeted entity id.
    pub target_id: u32,
    /// The final transform to commit to the document.
    pub total_transform: Matrix4,
}

/// The in-progress drag's bookkeeping.
#[derive(Clone, Debug)]
struct ActiveDrag {
    handle: GizmoHandle,
    start: DragStart,
    total: Matrix4,
}

#[derive(Clone, Debug)]
enum DragStart {
    /// 1D: the parameter along the axis at grab time.
    Axis { start: f64, last: f64 },
    /// 2D: the plane point at grab time.
    Plane { start: Vector3, last: Vector3 },
    /// Arc: the angle at grab time, and the last unwrapped angle.
    Ring { start: f64, last: f64 },
}

/// An interactive transform triad.
#[derive(Clone, Debug)]
pub struct TransformGizmo {
    target_id: u32,
    origin: Vector3,
    mode: GizmoMode,
    /// The target screen-space radius, in device pixels.
    pixels: f64,
    /// The current world radius (recomputed from the camera by [`update`](Self::update)).
    scale: f64,
    hovered: Option<GizmoHandle>,
    active: Option<ActiveDrag>,
    dirty: bool,
    /// Nine fixed slots (one per [`GizmoHandle`]); unused ones are cleared. Stable mesh
    /// ids, so a rebuild is a geometry write, not a new mesh.
    handles: Vec<ShapeItem>,
}

impl TransformGizmo {
    /// A gizmo for `target_id`, centred at `origin`.
    pub fn new(target_id: u32, origin: Vector3, mode: GizmoMode) -> Self {
        Self {
            target_id,
            origin,
            mode,
            pixels: DEFAULT_PIXELS,
            scale: 0.0,
            hovered: None,
            active: None,
            dirty: true,
            handles: (0..GizmoHandle::ALL.len()).map(new_handle).collect(),
        }
    }

    /// The targeted entity id.
    pub fn target_id(&self) -> u32 {
        self.target_id
    }

    /// The gizmo's origin.
    pub fn origin(&self) -> Vector3 {
        self.origin
    }

    /// Move the gizmo (rebuilds the handle geometry on the next [`update`](Self::update)).
    pub fn set_origin(&mut self, origin: Vector3) {
        self.origin = origin;
        self.dirty = true;
    }

    /// The current mode.
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    /// Change the mode (rebuilds on the next [`update`](Self::update)).
    pub fn set_mode(&mut self, mode: GizmoMode) {
        self.mode = mode;
        self.dirty = true;
    }

    /// The current world radius (0 before the first [`update`](Self::update)).
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The handle under the pointer (set by the host from a pick).
    pub fn hovered(&self) -> Option<GizmoHandle> {
        self.hovered
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// The handle slots, for drawing and picking.
    pub fn items(&self) -> &[ShapeItem] {
        &self.handles
    }

    /// Highlight `handle` (or clear). Returns whether the visuals changed.
    pub fn set_hovered(&mut self, handle: Option<GizmoHandle>) -> bool {
        if self.hovered == handle {
            return false;
        }
        self.hovered = handle;
        self.apply_colors();
        true
    }

    /// Recompute the screen-invariant scale for `camera` and rebuild the handles if it
    /// changed (or if the origin/mode did). Returns whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let scale = scale_for(camera, self.origin, self.pixels);
        let changed = (scale - self.scale).abs() > self.scale * 1e-3 + 1e-9;
        if !changed && !self.dirty {
            return false;
        }
        self.scale = scale;
        self.build();
        true
    }

    /// Begin a drag on `handle` at the pointer `device` position.
    pub fn begin_drag(&mut self, handle: GizmoHandle, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(ray) = camera.ray_through(device) else {
            return false;
        };
        let origin = self.origin;
        let start = match handle.kind() {
            HandleKind::Axis => {
                let axis = handle.axis().expect("an axis handle has an axis");
                let Some(s) = closest_axis_parameter(&ray, origin, axis) else {
                    return false;
                };
                DragStart::Axis { start: s, last: s }
            }
            HandleKind::Plane => {
                let normal = handle.plane_normal().expect("a plane handle has a normal");
                let Some(point) = ray_plane(&ray, origin, normal) else {
                    return false;
                };
                DragStart::Plane {
                    start: point,
                    last: point,
                }
            }
            HandleKind::Ring => {
                let axis = handle.axis().expect("a ring handle has an axis");
                let Some(angle) = ring_angle(&ray, origin, axis) else {
                    return false;
                };
                DragStart::Ring {
                    start: angle,
                    last: 0.0,
                }
            }
        };
        self.active = Some(ActiveDrag {
            handle,
            start,
            total: Matrix4::identity(),
        });
        // Highlight the grabbed handle.
        self.hovered = Some(handle);
        self.apply_colors();
        true
    }

    /// Advance the active drag to the pointer's `device` position, returning the tick's
    /// deltas.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<GizmoDragEvent> {
        let ray = camera.ray_through(device)?;
        let origin = self.origin;
        let target_id = self.target_id;
        let active = self.active.as_mut()?;
        let handle = active.handle;

        let (delta, total) = match &mut active.start {
            DragStart::Axis { start, last } => {
                let axis = handle.axis().expect("axis handle");
                let s = closest_axis_parameter(&ray, origin, axis)?;
                let delta = s - *last;
                let total = s - *start;
                *last = s;
                (
                    translate(axis.scale(delta)),
                    translate(axis.scale(total)),
                )
            }
            DragStart::Plane { start, last } => {
                let normal = handle.plane_normal().expect("plane handle");
                let point = ray_plane(&ray, origin, normal)?;
                let delta = point.sub(last);
                let total = point.sub(start);
                *last = point;
                (translate(delta), translate(total))
            }
            DragStart::Ring { start, last } => {
                let axis = handle.axis().expect("ring handle");
                let angle = ring_angle(&ray, origin, axis)?;
                let total = wrap_pi(angle - *start);
                let delta = total - *last;
                *last = total;
                (
                    rotate(axis, delta),
                    rotate(axis, total),
                )
            }
        };
        active.total = total;

        Some(GizmoDragEvent {
            target_id,
            active_handle: handle,
            delta_transform: delta,
            total_transform: total,
        })
    }

    /// End the active drag, returning the commit (the accumulated transform).
    pub fn end_drag(&mut self) -> Option<GizmoCommitEvent> {
        let active = self.active.take()?;
        Some(GizmoCommitEvent {
            target_id: self.target_id,
            total_transform: active.total,
        })
    }

    /// Write every slot's geometry for the current mode and scale.
    fn build(&mut self) {
        let origin = self.origin;
        let scale = self.scale;
        for (index, handle) in GizmoHandle::ALL.iter().enumerate() {
            let slot = &mut self.handles[index];
            if !self.mode.shows(*handle) {
                slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                continue;
            }
            match handle.kind() {
                HandleKind::Axis => {
                    axis_handle(slot, origin, handle.axis().expect("axis"), scale)
                }
                HandleKind::Plane => {
                    plane_handle(slot, origin, *handle, scale)
                }
                HandleKind::Ring => {
                    ring_handle(slot, origin, handle.axis().expect("axis"), scale)
                }
            }
        }
        self.apply_colors();
        self.dirty = false;
    }

    /// Recolour every slot (default, or highlighted when hovered).
    fn apply_colors(&mut self) {
        for (index, handle) in GizmoHandle::ALL.iter().enumerate() {
            let color = color_for(*handle, self.hovered == Some(*handle));
            self.handles[index].set_surface_color(color);
            self.handles[index].set_edge_color(color);
        }
    }
}

impl GizmoMode {
    /// Whether `handle` is shown in this mode.
    fn shows(self, handle: GizmoHandle) -> bool {
        match self {
            GizmoMode::Translate => handle.kind() != HandleKind::Ring,
            GizmoMode::Rotate => handle.kind() == HandleKind::Ring,
            GizmoMode::Universal => true,
        }
    }
}

// --- geometry ---------------------------------------------------------------------

/// A fresh handle slot: the pick id, and the draw mode its kind needs.
fn new_handle(index: usize) -> ShapeItem {
    let handle = GizmoHandle::ALL[index];
    let mode = match handle.kind() {
        HandleKind::Axis => DrawMode::ShadedWithEdges,
        HandleKind::Plane => DrawMode::Shaded,
        HandleKind::Ring => DrawMode::Wireframe,
    };
    ShapeItem::new(GIZMO_BASE_ID + index as u32).with_draw_mode(mode)
}

/// A translation axis: a shaft line (edge) plus a cone arrowhead (triangles).
fn axis_handle(item: &mut ShapeItem, origin: Vector3, axis: Vector3, scale: f64) {
    let tip = origin.add(&axis.scale(scale));
    let base = origin.add(&axis.scale(scale * 0.8));
    let radius = scale * 0.06;
    let (u, v) = orthonormal_basis(axis);
    let segments = 8usize;

    let mut positions = vec![to3(origin), to3(base), to3(tip)];
    for i in 0..segments {
        let angle = TAU * i as f64 / segments as f64;
        let point = base
            .add(&u.scale(radius * angle.cos()))
            .add(&v.scale(radius * angle.sin()));
        positions.push(to3(point));
    }
    let mut triangles = Vec::new();
    for i in 0..segments as u32 {
        let next = (i + 1) % segments as u32;
        triangles.extend_from_slice(&[2, 3 + i, 3 + next]);
    }
    let edges = vec![0, 1];
    item.set_shape(positions, Vec::new(), triangles, edges);
}

/// A planar handle: a small square in the two non-normal axes.
fn plane_handle(item: &mut ShapeItem, origin: Vector3, handle: GizmoHandle, scale: f64) {
    let (u, v) = match handle {
        GizmoHandle::PlaneXY => (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
        GizmoHandle::PlaneXZ => (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
        _ => (Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
    };
    let corner = |a: f64, b: f64| to3(origin.add(&u.scale(a * scale)).add(&v.scale(b * scale)));
    let positions = vec![
        corner(0.35, 0.35),
        corner(0.7, 0.35),
        corner(0.7, 0.7),
        corner(0.35, 0.7),
    ];
    item.set_shape(positions, Vec::new(), vec![0, 1, 2, 0, 2, 3], Vec::new());
}

/// A rotation ring: a circle (line loop) about `axis`.
fn ring_handle(item: &mut ShapeItem, origin: Vector3, axis: Vector3, scale: f64) {
    let (u, v) = orthonormal_basis(axis);
    let segments = 48usize;
    let mut positions = Vec::new();
    for i in 0..segments {
        let angle = TAU * i as f64 / segments as f64;
        let point = origin
            .add(&u.scale(scale * angle.cos()))
            .add(&v.scale(scale * angle.sin()));
        positions.push(to3(point));
    }
    let mut edges = Vec::new();
    for i in 0..segments as u32 {
        edges.push(i);
        edges.push((i + 1) % segments as u32);
    }
    item.set_shape(positions, Vec::new(), Vec::new(), edges);
}

/// The resting colour of a handle (axes/rings saturated; planes translucent).
fn color_for(handle: GizmoHandle, hovered: bool) -> [f32; 4] {
    let plane = handle.kind() == HandleKind::Plane;
    if hovered {
        return [1.0, 0.85, 0.2, if plane { 0.6 } else { 1.0 }];
    }
    match handle {
        GizmoHandle::AxisX | GizmoHandle::RotateX | GizmoHandle::PlaneYZ => {
            [0.9, 0.2, 0.2, 1.0]
        }
        GizmoHandle::AxisY | GizmoHandle::RotateY | GizmoHandle::PlaneXZ => {
            [0.2, 0.8, 0.3, 1.0]
        }
        GizmoHandle::AxisZ | GizmoHandle::RotateZ | GizmoHandle::PlaneXY => {
            [0.3, 0.45, 0.95, 1.0]
        }
    }
}

// --- math -------------------------------------------------------------------------

/// The world-space radius that keeps a `pixels`-radius gizmo on screen, given the camera.
///
/// `radius ∝ distance / (m5 * viewport_height)`, where `m5` is the projection's
/// vertical scale (`1 / tan(fov/2)`), so the apparent size is constant.
pub fn scale_for(camera: &Camera, origin: Vector3, pixels: f64) -> f64 {
    let eye = camera_position(&camera.view);
    let distance = eye.sub(&origin).length().max(1e-6);
    let m5 = camera.projection.m[5].abs().max(1e-6);
    let height = f64::from(camera.viewport.device_size()[1].max(1));
    2.0 * pixels * distance / (m5 * height)
}

/// The camera's world position from its view matrix (`view = [R | t]` → `-Rᵀ·t`).
fn camera_position(view: &Matrix4) -> Vector3 {
    let m = &view.m;
    let t = Vector3::new(m[3], m[7], m[11]);
    // Rᵀ·t
    Vector3::new(
        m[0] * t.x + m[4] * t.y + m[8] * t.z,
        m[1] * t.x + m[5] * t.y + m[9] * t.z,
        m[2] * t.x + m[6] * t.y + m[10] * t.z,
    )
    .scale(-1.0)
}

/// The parameter `s` of the point `origin + s·dir` on the axis line closest to `ray`.
/// `None` when the line and the ray are parallel.
fn closest_axis_parameter(ray: &Ray, origin: Vector3, dir: Vector3) -> Option<f64> {
    let p = to_vec(ray.origin);
    let d = to_vec(ray.direction);
    let r = p.sub(&origin);
    let b = d.dot(&dir);
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-9 {
        return None;
    }
    let dd = d.dot(&r);
    let ee = dir.dot(&r);
    Some((ee - b * dd) / denom)
}

/// Where `ray` crosses the plane through `point` with `normal` (in front of the ray).
fn ray_plane(ray: &Ray, point: Vector3, normal: Vector3) -> Option<Vector3> {
    let p = to_vec(ray.origin);
    let d = to_vec(ray.direction);
    let denom = d.dot(&normal);
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = point.sub(&p).dot(&normal) / denom;
    if t < 0.0 {
        return None;
    }
    Some(p.add(&d.scale(t)))
}

/// The angle of `ray`'s intersection with the ring plane about `axis`, measured from a
/// stable in-plane reference.
fn ring_angle(ray: &Ray, centre: Vector3, axis: Vector3) -> Option<f64> {
    let hit = ray_plane(ray, centre, axis)?;
    let v = hit.sub(&centre);
    let v = v.sub(&axis.scale(v.dot(&axis)));
    if v.length() < 1e-9 {
        return None;
    }
    let v = v.normalize();
    let reference = any_perpendicular(axis);
    let up = axis.cross(&reference).normalize();
    Some(v.dot(&up).atan2(v.dot(&reference)))
}

/// Two unit vectors spanning the plane perpendicular to `axis`.
fn orthonormal_basis(axis: Vector3) -> (Vector3, Vector3) {
    let u = any_perpendicular(axis);
    let v = axis.cross(&u).normalize();
    (u, v)
}

/// Any unit vector perpendicular to `axis`.
fn any_perpendicular(axis: Vector3) -> Vector3 {
    let helper = if axis.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    axis.cross(&helper).normalize()
}

/// Wrap an angle into `(-π, π]`.
fn wrap_pi(angle: f64) -> f64 {
    let mut a = angle;
    while a > std::f64::consts::PI {
        a -= TAU;
    }
    while a <= -std::f64::consts::PI {
        a += TAU;
    }
    a
}

/// A pure translation matrix.
fn translate(delta: Vector3) -> Matrix4 {
    Placement::new(delta, Rotation::identity()).to_matrix()
}

/// A rotation matrix about `axis` through the origin.
fn rotate(axis: Vector3, angle: f64) -> Matrix4 {
    Placement::new(Vector3::zero(), Rotation::from_axis_angle(&axis, angle)).to_matrix()
}

fn to3(v: Vector3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

fn to_vec(v: [f64; 3]) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::Viewport;

    /// A 45° perspective camera at `(0, 0, eye_z)` looking toward the origin.
    fn camera(eye_z: f64) -> Camera {
        let f = 1.0 / (45.0f64.to_radians() / 2.0).tan();
        let (near, far) = (0.1f64, 100.0f64);
        let mut m = [0.0f64; 16];
        m[0] = f;
        m[5] = f;
        m[10] = (far + near) / (near - far);
        m[11] = (2.0 * far * near) / (near - far);
        m[14] = -1.0;
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix(),
            projection: Matrix4::from_values(m),
            viewport: Viewport::new([100, 100], 1.0),
        }
    }

    const CENTRE: [i32; 2] = [50, 50];
    const RIGHT: [i32; 2] = [75, 50];
    const UP: [i32; 2] = [50, 75];

    fn translation(matrix: &Matrix4) -> Vector3 {
        Vector3::new(matrix.m[3], matrix.m[7], matrix.m[11])
    }

    #[test]
    fn handles_carry_distinct_pick_ids() {
        let mut seen = Vec::new();
        for handle in GizmoHandle::ALL {
            let id = handle.pick_id();
            assert!(!seen.contains(&id));
            seen.push(id);
            assert_eq!(GizmoHandle::from_pick_id(id), Some(handle));
        }
        assert_eq!(GizmoHandle::from_pick_id(GIZMO_BASE_ID - 1), None);
        assert_eq!(GizmoHandle::from_pick_id(GIZMO_BASE_ID + 9), None);
    }

    #[test]
    fn the_scale_is_invariant_to_the_camera_distance() {
        let origin = Vector3::zero();
        let near = scale_for(&camera(4.0), origin, DEFAULT_PIXELS);
        let far = scale_for(&camera(8.0), origin, DEFAULT_PIXELS);
        // Twice the distance → twice the world radius (a constant screen footprint).
        assert!((far / near - 2.0).abs() < 1e-6, "{far} vs {near}");
    }

    #[test]
    fn the_mode_controls_which_handles_are_built() {
        let item_count = |mode| {
            let mut gizmo = TransformGizmo::new(1, Vector3::zero(), mode);
            gizmo.update(&camera(5.0));
            gizmo.items()
                .iter()
                .filter(|item| {
                    let mut queue = ferrocad_inventor::RenderQueue::default();
                    item.emit(&mut queue);
                    !queue.is_empty()
                })
                .count()
        };
        assert_eq!(item_count(GizmoMode::Translate), 6);
        assert_eq!(item_count(GizmoMode::Rotate), 3);
        assert_eq!(item_count(GizmoMode::Universal), 9);
    }

    #[test]
    fn an_axis_drag_translates_along_the_axis() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        let event = gizmo.drag_to(&camera(5.0), RIGHT).expect("drag");
        assert_eq!(event.active_handle, GizmoHandle::AxisX);
        // The delta is purely along X (moving the cursor right moves the gizmo +X).
        let delta = translation(&event.delta_transform);
        assert!(delta.x.abs() > 1e-6, "{delta:?}");
        assert!(delta.y.abs() < 1e-9 && delta.z.abs() < 1e-9, "{delta:?}");
        // The accumulated transform is parallel to the delta.
        let total = translation(&event.total_transform);
        assert!((total.x - delta.x).abs() < 1e-9);
    }

    #[test]
    fn a_plane_drag_translates_within_the_plane() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::PlaneXY, &camera(5.0), CENTRE));
        let event = gizmo.drag_to(&camera(5.0), UP).expect("drag");
        let delta = translation(&event.delta_transform);
        // In the XY plane: no Z component.
        assert!(delta.z.abs() < 1e-9, "{delta:?}");
        assert!(delta.length() > 1e-6, "{delta:?}");
    }

    #[test]
    fn a_ring_drag_rotates_about_the_axis() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        // Grab the ring away from its centre (the centre is degenerate: the ray crosses
        // the ring axis there).
        assert!(gizmo.begin_drag(GizmoHandle::RotateZ, &camera(5.0), UP));
        let event = gizmo.drag_to(&camera(5.0), RIGHT).expect("drag");
        let rotation = Rotation::from_matrix(&event.total_transform);
        assert!(rotation.angle().abs() > 1e-4, "the ring should have rotated");
        let axis = rotation.axis();
        assert!(axis.z.abs() > 0.99, "rotation is about Z: {axis:?}");
    }

    #[test]
    fn a_commit_reports_the_accumulated_transform() {
        let mut gizmo = TransformGizmo::new(7, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisY, &camera(5.0), CENTRE));
        let total = gizmo.drag_to(&camera(5.0), UP).expect("drag").total_transform;
        let commit = gizmo.end_drag().expect("commit");
        assert_eq!(commit.target_id, 7);
        assert_eq!(commit.total_transform, total);
        assert!(!gizmo.is_dragging());
        assert!(gizmo.end_drag().is_none(), "a second commit is a no-op");
    }

    #[test]
    fn hovering_recolours_only_the_hovered_handle() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.set_hovered(Some(GizmoHandle::AxisZ)));
        assert!(!gizmo.set_hovered(Some(GizmoHandle::AxisZ)), "no-op when unchanged");
        let colour = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].edge_color()
        };
        assert_ne!(
            colour(&gizmo, GizmoHandle::AxisZ),
            colour(&gizmo, GizmoHandle::AxisX)
        );
    }
}

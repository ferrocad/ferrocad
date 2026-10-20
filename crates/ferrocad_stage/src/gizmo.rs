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

use ferrocad_graphics::{Camera, Ray, screen_ribbon};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::screen::{
    HIT_SLOP, STROKE_PIXELS, camera_forward, camera_position, point_segment_distance,
    project_device, push_ribbon, scale_for, to3,
};
use crate::shape::{DrawMode, ShapeItem};

/// The pick id of the first gizmo handle (`GizmoHandle::ALL[0]`).
///
/// High, so it cannot collide with document entity ids.
pub const GIZMO_BASE_ID: u32 = 1_000_000;

/// The default target radius, in device pixels.
pub const DEFAULT_PIXELS: f64 = 80.0;

/// Active manipulator stroke (`#38BDF8` in sRGB, linear here).
pub const ACTIVE_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 1.0];
/// Inactive manipulator stroke (`#94A3B8` in sRGB, linear here).
pub const INACTIVE_COLOR: [f32; 4] = [0.296, 0.366, 0.479, 0.9];
/// Translucent glass plane (`rgba(56, 189, 248, 0.10)`).
pub const GLASS_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 0.10];
/// Glass plane while hovered.
pub const GLASS_HIGHLIGHT: [f32; 4] = [0.040, 0.509, 0.939, 0.28];
/// Push-pull stem (`#F59E0B`); reserved for the push-pull manipulator.
pub const PUSH_PULL_COLOR: [f32; 4] = [0.913, 0.342, 0.003, 1.0];
/// Subtractive preview (`#EF4444`); reserved for the push-pull manipulator.
pub const SUBTRACT_COLOR: [f32; 4] = [0.863, 0.058, 0.058, 1.0];

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

    /// The handle's axis in the **world** frame: the drag axis for an `Axis`, the ring
    /// normal for a `Ring`. `None` for a plane. Use [`ManipulatorFrame::axis`] when the
    /// gizmo is in a local frame.
    pub fn axis(self) -> Option<Vector3> {
        let axis = match self {
            GizmoHandle::AxisX | GizmoHandle::RotateX => Vector3::new(1.0, 0.0, 0.0),
            GizmoHandle::AxisY | GizmoHandle::RotateY => Vector3::new(0.0, 1.0, 0.0),
            GizmoHandle::AxisZ | GizmoHandle::RotateZ => Vector3::new(0.0, 0.0, 1.0),
            GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => return None,
        };
        Some(axis)
    }

    /// The plane normal in the **world** frame, for a `Plane` handle (`XY` → Z, `XZ` → Y,
    /// `YZ` → X). Use [`ManipulatorFrame::plane_normal`] for a local frame.
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

/// The default rotation detent for the rotation rings: 15°.
const RING_DETENT: f64 = std::f64::consts::PI / 12.0;
/// Above this `|ray.direction · axis|` the axis is too close to the view direction for
/// the parametric solver, so the screen-space fallback takes over.
const VIEW_ALIGNED_COS: f64 = 0.97;
/// Below this projected length (device px) an axis points at the eye and cannot be
/// scrubbed unambiguously; the handle is disabled for that frame.
const MIN_SCREEN_AXIS_PIXELS: f64 = 1.0;
/// Where an axis becomes pickable along its stem, as a fraction of the world scale. The
/// inner part is where all three axes meet; excluding it keeps the triad centre
/// transparent to pointer events.
const AXIS_PICK_START: f64 = 0.18;
/// The number of segments in a ring's hit polyline (matches `ring_handle`).
const RING_PICK_SEGMENTS: usize = 48;
/// Below this `|ray.direction · ring_axis|` the ring is too edge-on for the 3D
/// plane/radius test; the screen-space polyline fallback takes over.
const RING_EDGE_ON_COS: f64 = 0.2;

/// The orthonormal frame a manipulator's handles act in.
///
/// `basis` holds the world directions of the local **X, Y and Z** axes as columns. Use
/// [`world`](Self::world) for the global triad or [`from_rotation`](Self::from_rotation)
/// for an object/selection frame. The frame is captured when a drag begins, so a host
/// cannot drift it mid-gesture; the coordinates are `f64`, matching the rest of the
/// geometry core.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManipulatorFrame {
    /// The frame's origin in world space.
    pub origin: Vector3,
    /// The world directions of the local X, Y and Z axes (unit length, right-handed).
    pub basis: [Vector3; 3],
}

impl ManipulatorFrame {
    /// The global, axis-aligned frame at `origin`.
    pub fn world(origin: Vector3) -> Self {
        Self {
            origin,
            basis: [
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
            ],
        }
    }

    /// A frame at `origin` whose axes are the **columns** of `rotation`'s matrix.
    pub fn from_rotation(origin: Vector3, rotation: Rotation) -> Self {
        let m = rotation.to_matrix();
        Self {
            origin,
            basis: [
                Vector3::new(m.m[0], m.m[4], m.m[8]),
                Vector3::new(m.m[1], m.m[5], m.m[9]),
                Vector3::new(m.m[2], m.m[6], m.m[10]),
            ],
        }
    }

    /// The world direction of the handle's axis: the translation stem for an axis, the
    /// ring normal for a ring. `None` for a plane.
    pub fn axis(&self, handle: GizmoHandle) -> Option<Vector3> {
        match handle {
            GizmoHandle::AxisX | GizmoHandle::RotateX => Some(self.basis[0]),
            GizmoHandle::AxisY | GizmoHandle::RotateY => Some(self.basis[1]),
            GizmoHandle::AxisZ | GizmoHandle::RotateZ => Some(self.basis[2]),
            GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => None,
        }
    }

    /// The two in-plane directions of a plane handle (its local `u` and `v`).
    pub fn plane_axes(&self, handle: GizmoHandle) -> Option<(Vector3, Vector3)> {
        match handle {
            GizmoHandle::PlaneXY => Some((self.basis[0], self.basis[1])),
            GizmoHandle::PlaneXZ => Some((self.basis[0], self.basis[2])),
            GizmoHandle::PlaneYZ => Some((self.basis[1], self.basis[2])),
            _ => None,
        }
    }

    /// The plane handle's normal (the local axis it omits).
    pub fn plane_normal(&self, handle: GizmoHandle) -> Option<Vector3> {
        match handle {
            GizmoHandle::PlaneXY => Some(self.basis[2]),
            GizmoHandle::PlaneXZ => Some(self.basis[1]),
            GizmoHandle::PlaneYZ => Some(self.basis[0]),
            _ => None,
        }
    }
}

/// Keyboard modifiers that affect a gizmo drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GizmoModifiers {
    /// Resolve an axis drag into a plane (Slice T3). Currently unused by the solver.
    pub shift: bool,
    /// Free rotation: bypass the 15° ring detents.
    pub ctrl: bool,
}

/// A screen-space axis used when the parametric (ray–axis) solver is ill-conditioned.
#[derive(Clone, Copy, Debug)]
struct ScreenAxis {
    /// The press position, in device pixels (bottom-left origin).
    press: [f64; 2],
    /// The unit screen direction of the positive axis.
    direction: [f64; 2],
    /// World units per device pixel along the axis.
    world_per_pixel: f64,
}

/// One of the triad's three local axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    /// Local X.
    X,
    /// Local Y.
    Y,
    /// Local Z.
    Z,
}

impl GizmoAxis {
    /// The axis letter used by the numeric readout.
    pub fn letter(self) -> char {
        match self {
            Self::X => 'X',
            Self::Y => 'Y',
            Self::Z => 'Z',
        }
    }
}

/// The geometric constraint a drag is under.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GizmoConstraint {
    /// 1D translation along an axis.
    Axis(GizmoAxis),
    /// 2D translation in the plane with this world-space normal.
    Plane(Vector3),
    /// Rotation about an axis.
    Ring(GizmoAxis),
}

/// The typed numeric value of a drag tick, for the host's readout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GizmoValue {
    /// A 1D translation. `raw` is the unsnapped distance; `applied` is what the
    /// transform used (after grid snapping). Both are world units.
    Linear {
        axis: GizmoAxis,
        raw: f64,
        applied: f64,
    },
    /// A 2D translation. `raw`/`applied` are world-space deltas in the drag plane.
    Planar { raw: Vector3, applied: Vector3 },
    /// A rotation. `raw_deg` is the unsnapped angle; `applied_deg` is what the
    /// transform used (after the 15° detent, unless `Ctrl` was held).
    Angular {
        axis: GizmoAxis,
        raw_deg: f64,
        applied_deg: f64,
    },
}

impl GizmoValue {
    /// A compact tag for the projected numeric readout: `ΔX +24.5 mm`, `ΔθZ +15.0°`
    /// or `ΔX +10.0 mm  ΔY +5.0 mm` for a plane. Units default to millimetres and
    /// degrees; the host may format its own document units instead.
    pub fn readout(&self) -> String {
        match *self {
            Self::Linear { axis, applied, .. } => {
                format!("Δ{} {:+.1} mm", axis.letter(), applied)
            }
            Self::Angular {
                axis, applied_deg, ..
            } => format!("Δθ{} {:+.1}°", axis.letter(), applied_deg),
            Self::Planar { applied, .. } => {
                let mut parts = Vec::new();
                for (axis, value) in [
                    (GizmoAxis::X, applied.x),
                    (GizmoAxis::Y, applied.y),
                    (GizmoAxis::Z, applied.z),
                ] {
                    if value.abs() >= 0.05 {
                        parts.push(format!("Δ{} {:+.1}", axis.letter(), value));
                    }
                }
                if parts.is_empty() {
                    "Δ 0.0 mm".to_string()
                } else {
                    format!("{} mm", parts.join("  "))
                }
            }
        }
    }
}

/// The axis label for an axis or ring handle.
fn axis_of(handle: GizmoHandle) -> Option<GizmoAxis> {
    match handle {
        GizmoHandle::AxisX | GizmoHandle::RotateX => Some(GizmoAxis::X),
        GizmoHandle::AxisY | GizmoHandle::RotateY => Some(GizmoAxis::Y),
        GizmoHandle::AxisZ | GizmoHandle::RotateZ => Some(GizmoAxis::Z),
        GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => None,
    }
}

/// A gizmo drag tick.
#[derive(Clone, Debug)]
pub struct GizmoDragEvent {
    /// The targeted entity id.
    pub target_id: u32,
    /// The handle being dragged.
    pub active_handle: GizmoHandle,
    /// The geometric constraint the drag is under.
    pub constraint: GizmoConstraint,
    /// The typed numeric value of this tick.
    pub value: GizmoValue,
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
    /// The geometric constraint the drag was under.
    pub constraint: GizmoConstraint,
    /// The final numeric value.
    pub value: GizmoValue,
    /// The final transform to commit to the document.
    pub total_transform: Matrix4,
}

/// The in-progress drag's bookkeeping.
#[derive(Clone, Debug)]
struct ActiveDrag {
    handle: GizmoHandle,
    constraint: GizmoConstraint,
    start: DragStart,
    value: GizmoValue,
    total: Matrix4,
}

#[derive(Clone, Debug)]
enum DragStart {
    /// 1D: the parameter along the axis at grab time, or a screen-space fallback when
    /// the axis is too close to the view direction.
    Axis {
        start: f64,
        last: f64,
        screen: Option<ScreenAxis>,
    },
    /// 2D: the plane point at grab time and the plane's fixed normal.
    Plane {
        start: Vector3,
        last: Vector3,
        normal: Vector3,
    },
    /// Arc: the angle at grab time, and the last applied (possibly snapped) angle.
    Ring { start: f64, last: f64 },
}

/// An interactive transform triad.
#[derive(Clone, Debug)]
pub struct TransformGizmo {
    target_id: u32,
    frame: ManipulatorFrame,
    mode: GizmoMode,
    /// The target screen-space radius, in device pixels.
    pixels: f64,
    /// The current world radius (recomputed from the camera by [`update`](Self::update)).
    scale: f64,
    /// The last camera view used to build the screen-facing ribbons.
    view: Option<Matrix4>,
    /// The linear grid step, if grid snapping is enabled.
    linear_snap: Option<f64>,
    hovered: Option<GizmoHandle>,
    active: Option<ActiveDrag>,
    dirty: bool,
    /// Nine fixed slots (one per [`GizmoHandle`]); unused ones are cleared. Stable mesh
    /// ids, so a rebuild is a geometry write, not a new mesh.
    handles: Vec<ShapeItem>,
}

impl TransformGizmo {
    /// A gizmo for `target_id`, centred at `origin` and aligned with the world axes.
    pub fn new(target_id: u32, origin: Vector3, mode: GizmoMode) -> Self {
        Self {
            target_id,
            frame: ManipulatorFrame::world(origin),
            mode,
            pixels: DEFAULT_PIXELS,
            scale: 0.0,
            view: None,
            linear_snap: None,
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

    /// The gizmo's frame (origin and orthonormal axes).
    pub fn frame(&self) -> ManipulatorFrame {
        self.frame
    }

    /// Replace the frame. A frame change cancels an active drag so its frozen press
    /// frame cannot be reinterpreted mid-gesture.
    pub fn set_frame(&mut self, frame: ManipulatorFrame) {
        if self.frame == frame {
            return;
        }
        self.frame = frame;
        self.active = None;
        self.dirty = true;
    }

    /// The gizmo's origin.
    pub fn origin(&self) -> Vector3 {
        self.frame.origin
    }

    /// Move the gizmo (rebuilds the handle geometry on the next [`update`](Self::update)).
    pub fn set_origin(&mut self, origin: Vector3) {
        self.frame.origin = origin;
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

    /// The linear grid step used by axis/plane drags (`None` disables snapping).
    pub fn linear_snap(&self) -> Option<f64> {
        self.linear_snap
    }

    /// Set the linear grid step; `None` disables grid snapping.
    pub fn set_linear_snap(&mut self, grid: Option<f64>) {
        self.linear_snap = grid.filter(|step| *step > 1e-9);
    }

    /// The world radius for the current camera. Always recomputed (rather than reading
    /// the cached draw scale) so an event that arrives before the next frame still hits
    /// the geometry at the camera's current distance.
    fn screen_scale(&self, camera: &Camera) -> f64 {
        scale_for(camera, self.frame.origin, self.pixels)
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

    /// The handle under `device` within the analytic grab padding, or `None` when the
    /// pointer is in the triad's empty interior (which must pass through to the scene).
    ///
    /// The test projects the same axis segments, plane quads and ring polylines the
    /// producers draw, then measures screen-space distance. It is **analytic** rather
    /// than a rasterised pick so the 2 px stroke + 10 px padding does not require extra
    /// pick geometry, which would swallow clicks meant for background features.
    ///
    /// Priority is **axis > plane > ring**, then nearest stroke, then front-most. Only
    /// handles shown by [`GizmoMode`] participate.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> Option<GizmoHandle> {
        let scale = self.screen_scale(camera);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let frame = self.frame;
        let eye = camera_position(&camera.view);
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let Some(ray) = camera.ray_through(device) else {
            return None;
        };
        let mut best: Option<(f64, f64, GizmoHandle)> = None;

        // 1. Axis stems. The pick segment starts `AXIS_PICK_START` along the stem so the
        //    shared centre is not ambiguously claimed by three handles at once.
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(handle) || handle.kind() != HandleKind::Axis {
                continue;
            }
            let axis = frame.axis(handle).expect("axis handle");
            let start = frame.origin.add(&axis.scale(AXIS_PICK_START * scale));
            let end = frame.origin.add(&axis.scale(scale));
            let Some((a, b)) = project_pair(camera, start, end) else {
                continue;
            };
            // A stem seen end-on is a dot on screen and is not grabbable (the solver
            // disables it for the same reason).
            if (b[0] - a[0]).hypot(b[1] - a[1]) < MIN_SCREEN_AXIS_PIXELS {
                continue;
            }
            let (distance, t) = point_segment_distance(pointer, a, b);
            if distance > HIT_SLOP {
                continue;
            }
            let world = start.add(&end.sub(&start).scale(t));
            consider(&mut best, distance, eye.sub(&world).length(), handle);
        }
        if let Some((_, _, handle)) = best {
            return Some(handle);
        }

        // 2. Plane quads: the whole quad is grabbable, plus padding around its rim.
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(handle) || handle.kind() != HandleKind::Plane {
                continue;
            }
            let (u, v) = frame.plane_axes(handle).expect("plane handle");
            let corner = |a: f64, b: f64| {
                frame
                    .origin
                    .add(&u.scale(a * scale))
                    .add(&v.scale(b * scale))
            };
            let corners = [
                corner(0.35, 0.35),
                corner(0.7, 0.35),
                corner(0.7, 0.7),
                corner(0.35, 0.7),
            ];
            let Some(poly) = project_quad(camera, &corners) else {
                continue;
            };
            let distance = if point_in_polygon(pointer, &poly) {
                0.0
            } else {
                let mut nearest = f64::INFINITY;
                for i in 0..4 {
                    let (d, _) = point_segment_distance(pointer, poly[i], poly[(i + 1) % 4]);
                    nearest = nearest.min(d);
                }
                nearest
            };
            if distance > HIT_SLOP {
                continue;
            }
            let centre = frame
                .origin
                .add(&u.scale(0.525 * scale))
                .add(&v.scale(0.525 * scale));
            consider(&mut best, distance, eye.sub(&centre).length(), handle);
        }
        if let Some((_, _, handle)) = best {
            return Some(handle);
        }

        // 3. Rotation rings. A visible ring is tested in its own plane: the ray's hit
        //    must land near the ring radius. An edge-on ring projects to a line through
        //    the centre, so its screen fallback excludes the inner part exactly like the
        //    axis stems (otherwise the centre would be "on" every edge-on ring).
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(handle) || handle.kind() != HandleKind::Ring {
                continue;
            }
            let axis = frame.axis(handle).expect("ring handle");
            let denominator = to_vec(ray.direction).dot(&axis);
            if denominator.abs() > RING_EDGE_ON_COS {
                let Some(hit) = ray_plane(&ray, frame.origin, axis) else {
                    continue;
                };
                let radial = hit.sub(&frame.origin);
                let radius = radial.length();
                if radius < 1e-9 {
                    continue;
                }
                let world_per_pixel = scale_for(camera, hit, 1.0).max(1e-9);
                let error = (radius - scale).abs();
                if error > HIT_SLOP * world_per_pixel {
                    continue;
                }
                let ring_point = frame.origin.add(&radial.scale(scale / radius));
                consider(
                    &mut best,
                    error / world_per_pixel,
                    eye.sub(&ring_point).length(),
                    handle,
                );
            } else {
                let (u, v) = orthonormal_basis(axis);
                let Some(centre) = project_device(camera, frame.origin) else {
                    continue;
                };
                let mut projected = Vec::with_capacity(RING_PICK_SEGMENTS);
                let mut world = Vec::with_capacity(RING_PICK_SEGMENTS);
                for i in 0..RING_PICK_SEGMENTS {
                    let angle = TAU * i as f64 / RING_PICK_SEGMENTS as f64;
                    let point = frame
                        .origin
                        .add(&u.scale(scale * angle.cos()))
                        .add(&v.scale(scale * angle.sin()));
                    world.push(point);
                    projected.push(project_device(camera, point));
                }
                let projected_radius = projected
                    .iter()
                    .filter_map(|point| *point)
                    .map(|point| (point[0] - centre[0]).hypot(point[1] - centre[1]))
                    .fold(0.0_f64, f64::max);
                let pointer_radius = (pointer[0] - centre[0]).hypot(pointer[1] - centre[1]);
                if pointer_radius < AXIS_PICK_START * projected_radius {
                    continue;
                }
                let mut nearest = f64::INFINITY;
                let mut nearest_world = frame.origin;
                for i in 0..RING_PICK_SEGMENTS {
                    let j = (i + 1) % RING_PICK_SEGMENTS;
                    let (Some(a), Some(b)) = (projected[i], projected[j]) else {
                        continue;
                    };
                    let (distance, t) = point_segment_distance(pointer, a, b);
                    if distance < nearest {
                        nearest = distance;
                        nearest_world = world[i].add(&world[j].sub(&world[i]).scale(t));
                    }
                }
                if nearest > HIT_SLOP {
                    continue;
                }
                consider(&mut best, nearest, eye.sub(&nearest_world).length(), handle);
            }
        }

        best.map(|(_, _, handle)| handle)
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
        let scale = scale_for(camera, self.frame.origin, self.pixels);
        let view_changed = self.view != Some(camera.view);
        let changed = (scale - self.scale).abs() > self.scale * 1e-3 + 1e-9 || view_changed;
        if !changed && !self.dirty {
            return false;
        }
        self.scale = scale;
        self.view = Some(camera.view);
        self.build(camera);
        true
    }

    /// Begin a drag on `handle` at the pointer `device` position.
    pub fn begin_drag(&mut self, handle: GizmoHandle, camera: &Camera, device: [i32; 2]) -> bool {
        self.begin_drag_with_modifiers(handle, camera, device, GizmoModifiers::default())
    }

    /// [`begin_drag`](Self::begin_drag) with the keyboard state captured at press time.
    /// `Shift` on an axis stem turns the 1D drag into a plane drag spanned by the stem and
    /// the screen-horizontal.
    pub fn begin_drag_with_modifiers(
        &mut self,
        handle: GizmoHandle,
        camera: &Camera,
        device: [i32; 2],
        modifiers: GizmoModifiers,
    ) -> bool {
        let Some(ray) = camera.ray_through(device) else {
            return false;
        };
        let frame = self.frame;
        let origin = frame.origin;
        let (start, constraint, value) = match handle.kind() {
            HandleKind::Axis if modifiers.shift => {
                let axis = frame.axis(handle).expect("an axis handle has an axis");
                let normal = shift_plane_normal(camera, axis);
                let Some(point) = plane_point(&ray, origin, normal, camera_forward(camera)) else {
                    return false;
                };
                (
                    DragStart::Plane {
                        start: point,
                        last: Vector3::zero(),
                        normal,
                    },
                    GizmoConstraint::Plane(normal),
                    GizmoValue::Planar {
                        raw: Vector3::zero(),
                        applied: Vector3::zero(),
                    },
                )
            }
            HandleKind::Axis => {
                let axis = frame.axis(handle).expect("an axis handle has an axis");
                let axis_label = axis_of(handle).expect("an axis handle has a label");
                // A stem seen almost end-on loses the parametric solve (the closest
                // point between the ray and the axis is ill-conditioned). Fall back to
                // the axis's screen direction; when it projects to under a pixel it
                // points at the eye and cannot be scrubbed at all.
                let start = if to_vec(ray.direction).dot(&axis).abs() > VIEW_ALIGNED_COS {
                    let scale = self.screen_scale(camera);
                    let Some(screen) = screen_axis(camera, origin, axis, scale, device) else {
                        return false;
                    };
                    DragStart::Axis {
                        start: 0.0,
                        last: 0.0,
                        screen: Some(screen),
                    }
                } else {
                    let Some(s) = closest_axis_parameter(&ray, origin, axis) else {
                        return false;
                    };
                    DragStart::Axis {
                        start: s,
                        last: 0.0,
                        screen: None,
                    }
                };
                (
                    start,
                    GizmoConstraint::Axis(axis_label),
                    GizmoValue::Linear {
                        axis: axis_label,
                        raw: 0.0,
                        applied: 0.0,
                    },
                )
            }
            HandleKind::Plane => {
                let normal = frame
                    .plane_normal(handle)
                    .expect("a plane handle has a normal");
                let Some(point) = plane_point(&ray, origin, normal, camera_forward(camera)) else {
                    return false;
                };
                (
                    DragStart::Plane {
                        start: point,
                        last: Vector3::zero(),
                        normal,
                    },
                    GizmoConstraint::Plane(normal),
                    GizmoValue::Planar {
                        raw: Vector3::zero(),
                        applied: Vector3::zero(),
                    },
                )
            }
            HandleKind::Ring => {
                let axis = frame.axis(handle).expect("a ring handle has an axis");
                let axis_label = axis_of(handle).expect("a ring handle has a label");
                let Some(angle) =
                    ring_angle_with_fallback(&ray, origin, axis, camera_forward(camera))
                else {
                    return false;
                };
                (
                    DragStart::Ring {
                        start: angle,
                        last: 0.0,
                    },
                    GizmoConstraint::Ring(axis_label),
                    GizmoValue::Angular {
                        axis: axis_label,
                        raw_deg: 0.0,
                        applied_deg: 0.0,
                    },
                )
            }
        };
        self.active = Some(ActiveDrag {
            handle,
            constraint,
            start,
            value,
            total: Matrix4::identity(),
        });
        // Highlight the grabbed handle.
        self.hovered = Some(handle);
        self.apply_colors();
        true
    }

    /// Advance the active drag to the pointer's `device` position, returning the tick's
    /// deltas. Rotation rings use 15° detents; [`drag_to_with_modifiers`](Self::drag_to_with_modifiers)
    /// lets the host bypass them with `Ctrl`.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<GizmoDragEvent> {
        self.drag_to_with_modifiers(camera, device, GizmoModifiers::default())
    }

    /// [`drag_to`](Self::drag_to) with the current keyboard modifiers.
    pub fn drag_to_with_modifiers(
        &mut self,
        camera: &Camera,
        device: [i32; 2],
        modifiers: GizmoModifiers,
    ) -> Option<GizmoDragEvent> {
        let ray = camera.ray_through(device)?;
        let frame = self.frame;
        let origin = frame.origin;
        let target_id = self.target_id;
        let forward = camera_forward(camera);
        let grid = self.linear_snap;
        let active = self.active.as_mut()?;
        let handle = active.handle;
        let constraint = active.constraint;

        let (delta, total, value) = match &mut active.start {
            DragStart::Axis {
                start,
                last,
                screen,
            } => {
                let axis = frame.axis(handle).expect("axis handle");
                let axis_label = axis_of(handle).expect("axis label");
                let raw = if let Some(screen) = screen {
                    let current = [f64::from(device[0]), f64::from(device[1])];
                    let pixels = (current[0] - screen.press[0]) * screen.direction[0]
                        + (current[1] - screen.press[1]) * screen.direction[1];
                    pixels * screen.world_per_pixel
                } else {
                    closest_axis_parameter(&ray, origin, axis)? - *start
                };
                let applied = snap_linear(raw, grid);
                let delta = applied - *last;
                *last = applied;
                (
                    translate(axis.scale(delta)),
                    translate(axis.scale(applied)),
                    GizmoValue::Linear {
                        axis: axis_label,
                        raw,
                        applied,
                    },
                )
            }
            DragStart::Plane {
                start,
                last,
                normal,
            } => {
                let point = plane_point(&ray, origin, *normal, forward)?;
                let raw = point.sub(start);
                let raw = raw.sub(&normal.scale(raw.dot(normal)));
                let mut applied = snap_vector(raw, grid);
                applied = applied.sub(&normal.scale(applied.dot(normal)));
                let delta = applied.sub(last);
                *last = applied;
                (
                    translate(delta),
                    translate(applied),
                    GizmoValue::Planar { raw, applied },
                )
            }
            DragStart::Ring { start, last } => {
                let axis = frame.axis(handle).expect("ring handle");
                let axis_label = axis_of(handle).expect("ring label");
                let angle = ring_angle_with_fallback(&ray, origin, axis, forward)?;
                let raw = wrap_pi(angle - *start);
                let (_, applied) = ring_detent(raw, modifiers.ctrl);
                let delta = applied - *last;
                *last = applied;
                (
                    rotate(axis, delta),
                    rotate(axis, applied),
                    GizmoValue::Angular {
                        axis: axis_label,
                        raw_deg: raw.to_degrees(),
                        applied_deg: applied.to_degrees(),
                    },
                )
            }
        };
        active.total = total;
        active.value = value;

        Some(GizmoDragEvent {
            target_id,
            active_handle: handle,
            constraint,
            value,
            delta_transform: delta,
            total_transform: total,
        })
    }

    /// Abort the active drag without a commit. Returns whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        self.active.take().is_some()
    }

    /// The device-space anchor for the drag's numeric readout. Axis drags anchor at the
    /// stem tip, plane drags at the anchor, and ring drags return `None` so the host can
    /// follow the pointer instead.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        let active = self.active.as_ref()?;
        let point = match &active.start {
            DragStart::Axis { .. } => {
                let axis = self.frame.axis(active.handle)?;
                self.frame
                    .origin
                    .add(&axis.scale(self.screen_scale(camera) * 1.05))
            }
            DragStart::Plane { .. } => self.frame.origin,
            DragStart::Ring { .. } => return None,
        };
        project_device(camera, point)
    }

    /// End the active drag, returning the commit (the accumulated transform).
    pub fn end_drag(&mut self) -> Option<GizmoCommitEvent> {
        let active = self.active.take()?;
        Some(GizmoCommitEvent {
            target_id: self.target_id,
            constraint: active.constraint,
            value: active.value,
            total_transform: active.total,
        })
    }

    /// Write every slot's geometry for the current mode, scale and camera.
    fn build(&mut self, camera: &Camera) {
        let frame = self.frame;
        let origin = frame.origin;
        let scale = self.scale;
        let world_per_pixel = scale / self.pixels.max(1.0);
        let forward = camera_forward(camera);
        for (index, handle) in GizmoHandle::ALL.iter().enumerate() {
            let slot = &mut self.handles[index];
            if !self.mode.shows(*handle) {
                slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                continue;
            }
            match handle.kind() {
                HandleKind::Axis => axis_handle(
                    slot,
                    origin,
                    frame.axis(*handle).expect("axis"),
                    scale,
                    world_per_pixel,
                    forward,
                ),
                HandleKind::Plane => {
                    let (u, v) = frame.plane_axes(*handle).expect("plane axes");
                    plane_handle(slot, origin, u, v, scale)
                }
                HandleKind::Ring => ring_handle(
                    slot,
                    origin,
                    frame.axis(*handle).expect("ring"),
                    scale,
                    world_per_pixel,
                    forward,
                ),
            }
        }
        self.apply_colors();
        self.dirty = false;
    }

    /// Recolour every slot: active cyan or inactive slate, with a glass fill and rim for
    /// planes.
    fn apply_colors(&mut self) {
        for (index, handle) in GizmoHandle::ALL.iter().enumerate() {
            let (surface, edge) = paint_for(*handle, self.hovered == Some(*handle));
            self.handles[index].set_surface_color(surface);
            self.handles[index].set_edge_color(edge);
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

/// A fresh handle slot: the pick id and the draw mode its kind needs.
fn new_handle(index: usize) -> ShapeItem {
    let handle = GizmoHandle::ALL[index];
    let mode = match handle.kind() {
        HandleKind::Plane => DrawMode::ShadedWithEdges,
        HandleKind::Axis | HandleKind::Ring => DrawMode::Shaded,
    };
    ShapeItem::new(GIZMO_BASE_ID + index as u32).with_draw_mode(mode)
}

/// A translation axis: a 1.5 px shaft ribbon plus a micro-chevron at the tip.
fn axis_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    axis: Vector3,
    scale: f64,
    world_per_pixel: f64,
    forward: Vector3,
) {
    let tip = origin.add(&axis.scale(scale));
    let half = STROKE_PIXELS * 0.5 * world_per_pixel;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();

    push_ribbon(
        &mut positions,
        &mut triangles,
        screen_ribbon(origin, tip, half, forward),
    );

    // A V-shaped chevron just behind the tip, in the plane of the stem and the
    // screen-perpendicular. No bulky cone.
    let depth = 5.0 * world_per_pixel;
    let spread = 4.0 * world_per_pixel;
    let base = tip.sub(&axis.scale(depth));
    let perp = {
        let candidate = axis.cross(&forward);
        if candidate.length() > 1e-9 {
            candidate.normalize()
        } else {
            any_perpendicular(axis)
        }
    };
    let left = base.add(&perp.scale(spread));
    let right = base.sub(&perp.scale(spread));
    push_ribbon(
        &mut positions,
        &mut triangles,
        screen_ribbon(tip, left, half, forward),
    );
    push_ribbon(
        &mut positions,
        &mut triangles,
        screen_ribbon(tip, right, half, forward),
    );

    item.set_shape(positions, Vec::new(), triangles, Vec::new());
}

/// A planar handle: a translucent glass square with a 1 px rim.
fn plane_handle(item: &mut ShapeItem, origin: Vector3, u: Vector3, v: Vector3, scale: f64) {
    let corner = |a: f64, b: f64| to3(origin.add(&u.scale(a * scale)).add(&v.scale(b * scale)));
    let positions = vec![
        corner(0.35, 0.35),
        corner(0.7, 0.35),
        corner(0.7, 0.7),
        corner(0.35, 0.7),
    ];
    item.set_shape(
        positions,
        Vec::new(),
        vec![0, 1, 2, 0, 2, 3],
        vec![0, 1, 1, 2, 2, 3, 3, 0],
    );
}

/// A rotation ring: a 1.5 px ribbon around the circle.
fn ring_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    axis: Vector3,
    scale: f64,
    world_per_pixel: f64,
    forward: Vector3,
) {
    let (u, v) = orthonormal_basis(axis);
    let half = STROKE_PIXELS * 0.5 * world_per_pixel;
    let segments = 48usize;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for i in 0..segments {
        let a = TAU * i as f64 / segments as f64;
        let b = TAU * (i + 1) as f64 / segments as f64;
        let from = origin
            .add(&u.scale(scale * a.cos()))
            .add(&v.scale(scale * a.sin()));
        let to = origin
            .add(&u.scale(scale * b.cos()))
            .add(&v.scale(scale * b.sin()));
        push_ribbon(
            &mut positions,
            &mut triangles,
            screen_ribbon(from, to, half, forward),
        );
    }
    item.set_shape(positions, Vec::new(), triangles, Vec::new());
}

/// The surface and edge colours for a handle. Axes and rings are strokes; planes are a
/// translucent glass fill with a stroke rim.
fn paint_for(handle: GizmoHandle, hovered: bool) -> ([f32; 4], [f32; 4]) {
    let stroke = if hovered {
        ACTIVE_COLOR
    } else {
        INACTIVE_COLOR
    };
    match handle.kind() {
        HandleKind::Plane => (
            if hovered {
                GLASS_HIGHLIGHT
            } else {
                GLASS_COLOR
            },
            stroke,
        ),
        HandleKind::Axis | HandleKind::Ring => (stroke, stroke),
    }
}

// --- math -------------------------------------------------------------------------

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

/// Project both endpoints of a segment.
fn project_pair(camera: &Camera, a: Vector3, b: Vector3) -> Option<([f64; 2], [f64; 2])> {
    Some((project_device(camera, a)?, project_device(camera, b)?))
}

/// Project a plane quad's four corners.
fn project_quad(camera: &Camera, points: &[Vector3; 4]) -> Option<[[f64; 2]; 4]> {
    Some([
        project_device(camera, points[0])?,
        project_device(camera, points[1])?,
        project_device(camera, points[2])?,
        project_device(camera, points[3])?,
    ])
}

/// Whether `point` is inside the convex screen-space `polygon` (winding-agnostic ray
/// crossing).
fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = (polygon[i][0], polygon[i][1]);
        let (xj, yj) = (polygon[j][0], polygon[j][1]);
        if (yi > point[1]) != (yj > point[1])
            && point[0] < (xj - xi) * (point[1] - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Keep the nearer stroke, or the front-most one when two strokes are equally close.
fn consider(
    best: &mut Option<(f64, f64, GizmoHandle)>,
    distance: f64,
    depth: f64,
    handle: GizmoHandle,
) {
    let replace = match best {
        None => true,
        Some((best_distance, best_depth, _)) => {
            distance < *best_distance - 1e-9
                || ((distance - *best_distance).abs() <= 1e-9 && depth < *best_depth)
        }
    };
    if replace {
        *best = Some((distance, depth, handle));
    }
}

/// The screen-space fallback for a near view-aligned axis. `None` when the axis projects
/// to under a pixel (it points at the eye) or the scale is degenerate.
fn screen_axis(
    camera: &Camera,
    origin: Vector3,
    axis: Vector3,
    scale: f64,
    press: [i32; 2],
) -> Option<ScreenAxis> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let start = project_device(camera, origin)?;
    let tip = project_device(camera, origin.add(&axis.scale(scale)))?;
    let (dx, dy) = (tip[0] - start[0], tip[1] - start[1]);
    let length = (dx * dx + dy * dy).sqrt();
    if !length.is_finite() || length < MIN_SCREEN_AXIS_PIXELS {
        return None;
    }
    Some(ScreenAxis {
        press: [f64::from(press[0]), f64::from(press[1])],
        direction: [dx / length, dy / length],
        world_per_pixel: scale / length,
    })
}

/// The pointer's hit on the drag plane, falling back to a view-facing plane when the
/// constrained plane is edge-on. The caller projects the result back onto the plane.
fn plane_point(ray: &Ray, origin: Vector3, normal: Vector3, forward: Vector3) -> Option<Vector3> {
    ray_plane(ray, origin, normal).or_else(|| ray_plane(ray, origin, forward))
}

/// Snap a scalar to the grid step (no-op when the grid is absent).
fn snap_linear(value: f64, grid: Option<f64>) -> f64 {
    match grid {
        Some(step) if step > 1e-9 => (value / step).round() * step,
        _ => value,
    }
}

/// Snap every world component of `value` to the grid step.
fn snap_vector(value: Vector3, grid: Option<f64>) -> Vector3 {
    match grid {
        Some(step) if step > 1e-9 => Vector3::new(
            snap_linear(value.x, Some(step)),
            snap_linear(value.y, Some(step)),
            snap_linear(value.z, Some(step)),
        ),
        _ => value,
    }
}

/// The world direction of screen +X (the view matrix's first row).
fn screen_right(camera: &Camera) -> Vector3 {
    Vector3::new(camera.view.m[0], camera.view.m[1], camera.view.m[2])
}

/// The world direction of screen +Y (the view matrix's second row).
fn screen_up(camera: &Camera) -> Vector3 {
    Vector3::new(camera.view.m[4], camera.view.m[5], camera.view.m[6])
}

/// The normal of the plane spanned by `axis` and the screen-horizontal, used by a
/// Shift-drag on an axis stem. Falls back to the screen-vertical when the stem is itself
/// horizontal on screen.
fn shift_plane_normal(camera: &Camera, axis: Vector3) -> Vector3 {
    let normal = axis.cross(&screen_right(camera));
    if normal.length() > 1e-6 {
        normal.normalize()
    } else {
        axis.cross(&screen_up(camera)).normalize()
    }
}

/// The angle of `ray`'s intersection with the ring plane about `axis`, measured from a
/// stable in-plane reference. Falls back to a view-facing plane when the ring is
/// edge-on, so a ring seen almost edge-on still scrubs in screen space.
fn ring_angle_with_fallback(
    ray: &Ray,
    centre: Vector3,
    axis: Vector3,
    forward: Vector3,
) -> Option<f64> {
    let hit = ray_plane(ray, centre, axis).or_else(|| ray_plane(ray, centre, forward))?;
    angle_at(hit, centre, axis)
}

/// The in-plane angle of `hit` about `axis`.
fn angle_at(hit: Vector3, centre: Vector3, axis: Vector3) -> Option<f64> {
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

/// Snap a ring's raw accumulated angle to relative 15° detents, or leave it free.
/// Returns `(raw, applied)`; the detents are relative to the drag's start, so grabbing a
/// rotated body does not jump on the first pixel.
fn ring_detent(raw: f64, free: bool) -> (f64, f64) {
    let raw = wrap_pi(raw);
    if free {
        (raw, raw)
    } else {
        (raw, (raw / RING_DETENT).round() * RING_DETENT)
    }
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

fn to_vec(v: [f64; 3]) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};

    /// A 45° perspective camera at `(0, 0, eye_z)` looking toward the origin.
    fn camera(eye_z: f64) -> Camera {
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
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
    fn an_orthographic_gizmo_has_a_constant_world_size() {
        let ortho = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::identity(),
            )
            .to_matrix(),
            projection: Projection::Orthographic {
                extent: 2.0,
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        };
        let at_origin = scale_for(&ortho, Vector3::zero(), DEFAULT_PIXELS);
        // Depth along the view axis does not change the world size.
        let deeper = scale_for(&ortho, Vector3::new(0.0, 0.0, 1.0), DEFAULT_PIXELS);
        assert!((at_origin - deeper).abs() < 1e-9);
        // The footprint is `2·extent/height` world units per pixel.
        let expected = 2.0 * 2.0 / 100.0 * DEFAULT_PIXELS;
        assert!((at_origin - expected).abs() < 1e-9, "{at_origin} vs {expected}");
    }

    #[test]
    fn the_mode_controls_which_handles_are_built() {
        let item_count = |mode| {
            let mut gizmo = TransformGizmo::new(1, Vector3::zero(), mode);
            gizmo.update(&camera(5.0));
            gizmo.items()
                .iter()
                .filter(|item| {
                    let mut queue = ferrocad_graphics::RenderQueue::default();
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
    fn the_frame_maps_handles_to_its_basis() {
        let origin = Vector3::new(1.0, 2.0, 3.0);
        let world = ManipulatorFrame::world(origin);
        assert_eq!(
            world.axis(GizmoHandle::AxisX),
            Some(Vector3::new(1.0, 0.0, 0.0))
        );
        assert_eq!(
            world.plane_normal(GizmoHandle::PlaneXZ),
            Some(Vector3::new(0.0, 1.0, 0.0))
        );

        let rotation =
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2);
        let local = ManipulatorFrame::from_rotation(origin, rotation);
        assert!(
            local
                .axis(GizmoHandle::AxisX)
                .unwrap()
                .sub(&Vector3::new(0.0, 1.0, 0.0))
                .length()
                < 1e-9
        );
        assert!(
            local
                .axis(GizmoHandle::AxisY)
                .unwrap()
                .sub(&Vector3::new(-1.0, 0.0, 0.0))
                .length()
                < 1e-9
        );
        assert!(
            local
                .axis(GizmoHandle::AxisZ)
                .unwrap()
                .sub(&Vector3::new(0.0, 0.0, 1.0))
                .length()
                < 1e-9
        );
        let (u, v) = local.plane_axes(GizmoHandle::PlaneXY).unwrap();
        assert!(u.sub(&Vector3::new(0.0, 1.0, 0.0)).length() < 1e-9);
        assert!(v.sub(&Vector3::new(-1.0, 0.0, 0.0)).length() < 1e-9);
    }

    #[test]
    fn a_local_frame_rotates_an_axis_drag() {
        let camera = camera(5.0);
        let frame = ManipulatorFrame::from_rotation(
            Vector3::zero(),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        );
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.set_frame(frame);
        gizmo.update(&camera);
        // In the rotated frame, local X is world +Y, so a screen-up drag is +Y.
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera, CENTRE));
        let event = gizmo.drag_to(&camera, UP).expect("drag");
        let delta = translation(&event.delta_transform);
        assert!(delta.y.abs() > 1e-6, "local X is world +Y: {delta:?}");
        assert!(delta.x.abs() < 1e-9 && delta.z.abs() < 1e-9, "{delta:?}");
    }

    /// The standard camera with a 10° pitch: the +Z stem is within the fallback band but
    /// still projects to more than a pixel.
    fn near_view_aligned_camera() -> Camera {
        let rotation =
            Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 10.0_f64.to_radians());
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -5.0), rotation).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        }
    }

    #[test]
    fn a_view_aligned_axis_falls_back_to_screen_space() {
        let camera = near_view_aligned_camera();
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        assert!(
            gizmo.begin_drag(GizmoHandle::AxisZ, &camera, CENTRE),
            "the screen fallback must accept a near view-aligned stem"
        );
        let event = gizmo.drag_to(&camera, UP).expect("drag");
        let delta = translation(&event.delta_transform);
        assert!(delta.z.abs() > 1e-6, "the drag moves along Z: {delta:?}");
        assert!(delta.x.abs() < 1e-9 && delta.y.abs() < 1e-9, "{delta:?}");
    }

    #[test]
    fn an_axis_pointing_at_the_camera_is_disabled() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        // The default camera looks along −Z, so the +Z stem projects to a point.
        assert!(!gizmo.begin_drag(GizmoHandle::AxisZ, &camera(5.0), CENTRE));
    }

    #[test]
    fn an_edge_on_plane_still_drags_in_its_plane() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        // The XZ plane is edge-on to a camera looking along −Z; the view-facing fallback
        // recovers the in-plane X motion.
        assert!(gizmo.begin_drag(GizmoHandle::PlaneXZ, &camera(5.0), CENTRE));
        let event = gizmo.drag_to(&camera(5.0), RIGHT).expect("drag");
        let delta = translation(&event.delta_transform);
        assert!(delta.x.abs() > 1e-6, "{delta:?}");
        assert!(delta.y.abs() < 1e-9, "the plane normal is Y: {delta:?}");
    }

    #[test]
    fn ring_detents_are_relative_and_ctrl_frees_them() {
        let step = 15.0_f64.to_radians();
        let (raw, snapped) = ring_detent(7.0_f64.to_radians(), false);
        assert!((raw - 7.0_f64.to_radians()).abs() < 1e-12);
        assert!(snapped.abs() < 1e-12, "7° snaps back to the start");

        let (_, snapped) = ring_detent(8.0_f64.to_radians(), false);
        assert!(
            (snapped - step).abs() < 1e-12,
            "8° snaps to the first detent"
        );
        let (_, snapped) = ring_detent(-8.0_f64.to_radians(), false);
        assert!((snapped + step).abs() < 1e-12);

        let (raw, snapped) = ring_detent(7.0_f64.to_radians(), true);
        assert!((snapped - raw).abs() < 1e-12, "Ctrl is free rotation");

        // The raw delta is unwrapped across ±π.
        let (raw, _) = ring_detent(3.0 * std::f64::consts::FRAC_PI_2, false);
        assert!((raw + std::f64::consts::FRAC_PI_2).abs() < 1e-12, "{raw}");
    }

    #[test]
    fn the_triad_interior_passes_through() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        // The axes only start at 0.18·scale and the planes/rings are farther out; the
        // shared centre belongs to no handle, so a click there reaches the scene.
        assert_eq!(gizmo.hit(&camera(5.0), CENTRE), None);
    }

    #[test]
    fn an_axis_is_pickable_along_its_stem() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        assert_eq!(gizmo.hit(&camera(5.0), [80, 50]), Some(GizmoHandle::AxisX));
    }

    #[test]
    fn the_grab_padding_reaches_past_the_stroke() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        // Eight pixels above the X stem: outside the 2 px stroke, inside the 10 px grab
        // padding.
        assert_eq!(gizmo.hit(&camera(5.0), [80, 58]), Some(GizmoHandle::AxisX));
    }

    #[test]
    fn a_point_beyond_the_padding_passes_through() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        assert_eq!(gizmo.hit(&camera(5.0), [30, 30]), None);
    }

    #[test]
    fn a_plane_quad_is_pickable_in_its_interior() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        // The XY quad spans 0.35..0.7·scale in +X/+Y; its centre is ~42 px out.
        assert_eq!(
            gizmo.hit(&camera(5.0), [92, 92]),
            Some(GizmoHandle::PlaneXY)
        );
    }

    #[test]
    fn a_ring_is_pickable_on_its_arc() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        gizmo.update(&camera(5.0));
        // 45° on the Z ring (the X/Y rings are edge-on lines at this view).
        assert_eq!(
            gizmo.hit(&camera(5.0), [107, 107]),
            Some(GizmoHandle::RotateZ)
        );
    }

    #[test]
    fn an_axis_wins_over_an_overlapping_ring() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        // Forty pixels along +X lies on the X stem and on the edge-on Y ring's projected
        // line; the axis has priority.
        assert_eq!(gizmo.hit(&camera(5.0), [90, 50]), Some(GizmoHandle::AxisX));
    }

    #[test]
    fn hidden_handles_are_not_picked() {
        let mut translate = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        translate.update(&camera(5.0));
        assert_eq!(
            translate.hit(&camera(5.0), [80, 80]),
            Some(GizmoHandle::PlaneXY)
        );

        // The same point in rotate mode is between the displayed rings.
        let mut rotate = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        rotate.update(&camera(5.0));
        assert_eq!(rotate.hit(&camera(5.0), [80, 80]), None);
    }

    #[test]
    fn the_hit_does_not_need_a_preceding_update() {
        // `hit` recomputes the screen scale, so an event that arrives before the next
        // frame still lands on the drawn geometry.
        let gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert_eq!(gizmo.hit(&camera(5.0), [80, 50]), Some(GizmoHandle::AxisX));
    }

    #[test]
    fn a_local_frame_hit_follows_the_basis() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.set_frame(ManipulatorFrame::from_rotation(
            Vector3::zero(),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2),
        ));
        gizmo.update(&camera(5.0));
        // Local X is world +Y, so the vertical screen stem is AxisX.
        assert_eq!(gizmo.hit(&camera(5.0), [50, 80]), Some(GizmoHandle::AxisX));
        // Local Y is world −X, so the leftward stem is AxisY.
        assert_eq!(gizmo.hit(&camera(5.0), [20, 50]), Some(GizmoHandle::AxisY));
    }

    #[test]
    fn changing_the_frame_cancels_the_drag() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        gizmo.set_frame(ManipulatorFrame::world(Vector3::new(1.0, 0.0, 0.0)));
        assert!(!gizmo.is_dragging());
        assert!(gizmo.end_drag().is_none());
        assert_eq!(gizmo.frame().origin, Vector3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn the_linear_value_tracks_the_axis_and_readout() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        let event = gizmo.drag_to(&camera(5.0), RIGHT).expect("drag");
        match event.value {
            GizmoValue::Linear {
                axis: GizmoAxis::X,
                raw,
                applied,
            } => {
                assert!(raw.abs() > 1e-6);
                assert!((raw - applied).abs() < 1e-12, "no grid: raw == applied");
                let delta = translation(&event.delta_transform);
                assert!((delta.x - applied).abs() < 1e-9, "{delta:?} vs {applied}");
            }
            other => panic!("expected a linear X value, got {other:?}"),
        }
        assert!(
            event.value.readout().starts_with("ΔX"),
            "{}",
            event.value.readout()
        );
        assert!(event.value.readout().ends_with(" mm"));
    }

    #[test]
    fn the_angular_value_is_in_degrees_and_snaps_to_15() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        assert!(gizmo.begin_drag(GizmoHandle::RotateZ, &camera(5.0), UP));
        let event = gizmo.drag_to(&camera(5.0), [60, 75]).expect("drag");
        match event.value {
            GizmoValue::Angular {
                axis: GizmoAxis::Z,
                raw_deg,
                applied_deg,
            } => {
                assert!(raw_deg.abs() > 1.0, "raw {raw_deg}");
                let expected = (raw_deg / 15.0).round() * 15.0;
                assert!(
                    (applied_deg - expected).abs() < 1e-9,
                    "{applied_deg} vs {expected}"
                );
            }
            other => panic!("expected an angular Z value, got {other:?}"),
        }
        assert!(event.value.readout().contains('°'));
    }

    #[test]
    fn ctrl_frees_the_ring_value() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        assert!(gizmo.begin_drag(GizmoHandle::RotateZ, &camera(5.0), UP));
        let free = GizmoModifiers {
            ctrl: true,
            ..Default::default()
        };
        let event = gizmo
            .drag_to_with_modifiers(&camera(5.0), [60, 75], free)
            .expect("drag");
        match event.value {
            GizmoValue::Angular {
                raw_deg,
                applied_deg,
                ..
            } => assert!(
                (raw_deg - applied_deg).abs() < 1e-9,
                "Ctrl must not snap: {raw_deg} vs {applied_deg}"
            ),
            other => panic!("expected an angular value, got {other:?}"),
        }
    }

    #[test]
    fn linear_grid_snapping_snaps_the_applied_value() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.set_linear_snap(Some(0.5));
        assert_eq!(gizmo.linear_snap(), Some(0.5));
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        let event = gizmo.drag_to(&camera(5.0), [63, 50]).expect("drag");
        match event.value {
            GizmoValue::Linear { raw, applied, .. } => {
                assert!((raw - applied).abs() > 1e-6, "the grid must move the value");
                assert!(
                    ((applied / 0.5).round() * 0.5 - applied).abs() < 1e-9,
                    "{applied} is not on the 0.5 grid"
                );
            }
            other => panic!("expected a linear value, got {other:?}"),
        }
        let total = translation(&event.total_transform);
        assert!((total.x - 0.5).abs() < 1e-9, "{total:?}");
    }

    #[test]
    fn shift_on_an_axis_starts_a_planar_drag() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        let shift = GizmoModifiers {
            shift: true,
            ..Default::default()
        };
        assert!(gizmo.begin_drag_with_modifiers(GizmoHandle::AxisX, &camera(5.0), CENTRE, shift));
        let event = gizmo.drag_to(&camera(5.0), UP).expect("drag");
        match event.constraint {
            GizmoConstraint::Plane(normal) => {
                assert!(
                    normal.z.abs() > 0.99,
                    "the X stem + screen-horizontal is XY: {normal:?}"
                );
            }
            other => panic!("expected a plane constraint, got {other:?}"),
        }
        match event.value {
            GizmoValue::Planar { applied, .. } => {
                assert!(applied.y.abs() > 1e-6, "{applied:?}");
                assert!(event.value.readout().contains("ΔY"));
            }
            other => panic!("expected a planar value, got {other:?}"),
        }
    }

    #[test]
    fn cancel_drag_discards_the_gesture() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        assert!(gizmo.cancel_drag());
        assert!(!gizmo.is_dragging());
        assert!(gizmo.end_drag().is_none());
        assert!(!gizmo.cancel_drag(), "a second cancel is a no-op");
    }

    #[test]
    fn the_axis_readout_anchors_at_the_stem_tip() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera(5.0), CENTRE));
        let anchor = gizmo.readout_anchor(&camera(5.0)).expect("axis anchor");
        assert!(anchor[0] > 50.0, "the X tip is right of centre: {anchor:?}");

        let mut ring = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        assert!(ring.begin_drag(GizmoHandle::RotateZ, &camera(5.0), UP));
        assert!(ring.readout_anchor(&camera(5.0)).is_none());
    }

    #[test]
    fn hovering_recolours_only_the_hovered_handle() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(gizmo.set_hovered(Some(GizmoHandle::AxisZ)));
        assert!(
            !gizmo.set_hovered(Some(GizmoHandle::AxisZ)),
            "no-op when unchanged"
        );
        let colour = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].edge_color()
        };
        assert_ne!(
            colour(&gizmo, GizmoHandle::AxisZ),
            colour(&gizmo, GizmoHandle::AxisX)
        );
    }

    #[test]
    fn the_gizmo_strokes_are_ribbons() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));

        let axis = &gizmo.items()[GizmoHandle::AxisX.index()];
        assert_eq!(axis.draw_mode(), DrawMode::Shaded);
        assert!(!axis.positions().is_empty(), "the axis has ribbon geometry");
        assert!(axis.edges().is_empty(), "the stroke is a triangle ribbon");

        let ring = &gizmo.items()[GizmoHandle::RotateZ.index()];
        assert_eq!(ring.draw_mode(), DrawMode::Shaded);
        assert!(!ring.positions().is_empty());
        assert!(
            ring.edges().is_empty(),
            "the ring is a ribbon, not a line loop"
        );

        let plane = &gizmo.items()[GizmoHandle::PlaneXY.index()];
        assert_eq!(plane.draw_mode(), DrawMode::ShadedWithEdges);
        assert_eq!(plane.edges().len(), 8, "the glass plane keeps its 1 px rim");
    }

    #[test]
    fn the_manipulator_tokens_follow_the_active_state() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        let surface = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].surface_color()
        };
        let edge = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].edge_color()
        };

        assert_eq!(surface(&gizmo, GizmoHandle::AxisX), INACTIVE_COLOR);
        assert_eq!(surface(&gizmo, GizmoHandle::AxisZ), INACTIVE_COLOR);
        assert_eq!(surface(&gizmo, GizmoHandle::PlaneXY), GLASS_COLOR);
        assert_eq!(edge(&gizmo, GizmoHandle::PlaneXY), INACTIVE_COLOR);

        assert!(gizmo.set_hovered(Some(GizmoHandle::AxisZ)));
        assert_eq!(surface(&gizmo, GizmoHandle::AxisZ), ACTIVE_COLOR);
        assert_eq!(surface(&gizmo, GizmoHandle::AxisX), INACTIVE_COLOR);

        assert!(gizmo.set_hovered(Some(GizmoHandle::PlaneXY)));
        assert_eq!(surface(&gizmo, GizmoHandle::PlaneXY), GLASS_HIGHLIGHT);
        assert_eq!(edge(&gizmo, GizmoHandle::PlaneXY), ACTIVE_COLOR);
    }

    #[test]
    fn the_axis_ribbon_is_one_and_a_half_pixels_wide() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        let positions = gizmo.items()[GizmoHandle::AxisX.index()].positions();
        let point = |index: usize| {
            Vector3::new(
                f64::from(positions[index][0]),
                f64::from(positions[index][1]),
                f64::from(positions[index][2]),
            )
        };
        // The first quad is the shaft; corners 0 and 3 straddle the stem.
        let width = point(0).sub(&point(3)).length();
        let expected = STROKE_PIXELS * gizmo.scale() / DEFAULT_PIXELS;
        assert!(
            (width - expected).abs() < 1e-6,
            "the stroke should be {expected} world units, got {width}"
        );
    }
}

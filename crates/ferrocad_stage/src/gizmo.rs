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
use std::sync::Arc;

use ferrocad_graphics::{Camera, Ray, ShaderProgram, screen_ribbon};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::interaction::InteractionCommit;
use crate::screen::{
    RIBBON_UV_CENTRE, camera_forward, camera_position, point_segment_distance, project_device,
    push_ribbon_uv, scale_for, to3,
};
use crate::shape::{DrawMode, ShapeItem};
use crate::snap::FEATURE_PIXELS;

/// The pick id of the first gizmo handle (`GizmoHandle::ALL[0]`).
///
/// High, so it cannot collide with document entity ids.
pub const GIZMO_BASE_ID: u32 = 1_000_000;

/// Above this `|axis · view_forward|` an axis points at the camera and collapses to a
/// dot; it fades out and stops accepting hits.
const AXIS_EDGE_ON_COS: f64 = 0.96;
/// The alignment at which a grazing axis has full alpha (0.90 = ~26° off the view).
const AXIS_CLEAR_COS: f64 = 0.90;
/// Below this `|plane_normal · view_forward|` a plane is edge-on and projects to a
/// sliver; it fades out and stops accepting hits.
const PLANE_EDGE_ON_COS: f64 = 0.15;
/// The alignment at which a grazing plane has full alpha (0.35 = ~20° from edge-on).
const PLANE_CLEAR_COS: f64 = 0.35;
/// A handle below this fade is not pickable (it is visually gone).
const GRAZING_HIT_FLOOR: f32 = 0.05;

/// The default target radius, in device pixels.
pub const DEFAULT_PIXELS: f64 = 110.0;

/// The provider id of the native FerroCAD gizmo visuals.
pub const NATIVE_BACKEND: &str = "ferrocad";

/// The id the shipped shader mod claims. Whether this build can host it depends on
/// the active provider supplying a skin, not on the id itself.
pub const SKIN_BACKEND: &str = "ferrocad-shader";

/// Active manipulator stroke (`#38BDF8` in sRGB, linear here).
pub const ACTIVE_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 1.0];
/// Inactive manipulator stroke (`#94A3B8` in sRGB, linear here).
pub const INACTIVE_COLOR: [f32; 4] = [0.296, 0.366, 0.479, 0.9];
/// Translucent glass plane (`rgba(56, 189, 248, 0.10)`).
pub const GLASS_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 0.10];
/// Glass plane while hovered.
pub const GLASS_HIGHLIGHT: [f32; 4] = [0.040, 0.509, 0.939, 0.50];
/// Push-pull stem (`#F59E0B`); reserved for the push-pull manipulator.
pub const PUSH_PULL_COLOR: [f32; 4] = [0.913, 0.342, 0.003, 1.0];
/// Subtractive preview (`#EF4444`); reserved for the push-pull manipulator.
pub const SUBTRACT_COLOR: [f32; 4] = [0.863, 0.058, 0.058, 1.0];

/// The default rotation detent of the rings, in degrees.
pub const DEFAULT_RING_SNAP_DEG: f64 = 15.0;

/// The default outline colour (`#0F172A`, linear RGBA).
///
/// The halo is the WCAG 1.4.11 non-text-contrast aid: it separates a handle from
/// mid-luminance geometry (the toy's blue slab sits at the same lightness as the
/// cyan stems) without relying on hue alone.
pub const OUTLINE_COLOR: [f32; 4] = [0.0048, 0.0086, 0.0232, 1.0];
/// The default outline width around a handle, in device pixels.
pub const DEFAULT_HALO_WIDTH: f64 = 1.0;
/// The default stroke width of the axes and rings, in device pixels.
///
/// It is deliberately wider than the 1.5 px axis-cross stroke: a gizmo is a primary
/// control, and a 2 px stem keeps its arrowhead and ribbon legible over a busy model.
pub const DEFAULT_GIZMO_WIDTH: f64 = 2.0;
/// The default stroke-width multiplier of the hovered/grabbed handle.
///
/// This is the non-colour cue for hover (WCAG 1.4.1): the accent and the idle ink
/// are too close in luminance to carry the state by colour alone.
pub const DEFAULT_HOVER_SCALE: f64 = 1.5;
/// The minimum analytic grab radius, in device pixels (24 px target, WCAG 2.5.8).
pub const MIN_HIT_RADIUS_PX: f64 = 12.0;
/// The default analytic grab radius, in device pixels.
pub const DEFAULT_HIT_RADIUS_PX: f64 = MIN_HIT_RADIUS_PX;
/// The largest occluded-handle alpha that still reads as "behind" rather than
/// "another front handle".
pub const MAX_GHOST_ALPHA: f32 = 0.5;
/// The default occluded-handle alpha.
pub const DEFAULT_GHOST_ALPHA: f32 = 0.20;

/// The workspace gizmo/manipulator appearance and behaviour.
///
/// This is the shared style contract: the native gizmo, the workbench manipulators
/// (`ferrocad_toy`, `ferrocad_part_gui`) and the Python provider all speak it. Every
/// colour is **linear** RGBA; [`opacity`](Self::opacity) multiplies each handle's alpha
/// when the colours are built. The default `active`/`inactive` tokens carry the legacy
/// `1.0`/`0.9` alphas, so [`Default`] reproduces the pre-style look exactly; a provider
/// may supply alpha-`1.0` tokens and dim through the opacity instead.
///
/// [`backend`](Self::backend) is a provider id, not geometry, and
/// [`show_readout`](Self::show_readout) is a host policy: the gizmo exposes both so the
/// shell can gate its own drawing without the stage knowing a backend.
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoStyle {
    /// Whether the gizmo is shown at all (a host/provider decision, not geometry).
    pub enabled: bool,
    /// The provider id that owns the visuals (the native default is `"ferrocad"`).
    pub backend: String,
    /// The linear RGBA of a hovered/grabbed handle.
    pub active: [f32; 4],
    /// The linear RGBA of an idle handle.
    pub inactive: [f32; 4],
    /// The linear RGBA of the outline/halo behind the handles.
    pub outline: [f32; 4],
    /// The outline width around a handle, in device pixels; `0` disables it.
    pub halo_width: f64,
    /// The stroke-width multiplier of the hovered/grabbed handle (`1.0` disables).
    pub hover_scale: f64,
    /// The analytic grab radius, in device pixels (at least
    /// [`MIN_HIT_RADIUS_PX`]).
    pub hit_radius_px: f64,
    /// The target radius of the triad, in device pixels (screen-invariant).
    pub size: f64,
    /// The stroke width of the axes and rings, in device pixels.
    pub width: f64,
    /// A multiplier over every handle's alpha, in `0.0..=1.0`.
    pub opacity: f32,
    /// The alpha multiplier of the occluded ghost pass, in `0.0..=1.0`.
    pub ghost_alpha: f32,
    /// The rotation-ring detent, in degrees; `0` means continuous rotation.
    pub ring_snap_deg: f64,
    /// Whether the host shows the numeric readout beside the handle.
    pub show_readout: bool,
}

impl Default for GizmoStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: NATIVE_BACKEND.to_string(),
            active: ACTIVE_COLOR,
            inactive: INACTIVE_COLOR,
            outline: OUTLINE_COLOR,
            halo_width: DEFAULT_HALO_WIDTH,
            hover_scale: DEFAULT_HOVER_SCALE,
            hit_radius_px: DEFAULT_HIT_RADIUS_PX,
            size: DEFAULT_PIXELS,
            width: DEFAULT_GIZMO_WIDTH,
            opacity: 1.0,
            ghost_alpha: DEFAULT_GHOST_ALPHA,
            ring_snap_deg: DEFAULT_RING_SNAP_DEG,
            show_readout: true,
        }
    }
}

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
    /// The draggable origin/centre control (not a transform handle).
    Origin,
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

/// A pick target on the gizmo: one of the transform handles, or the separate
/// origin/centre control.
///
/// The origin is **not** a transform handle: it has no axis, plane or ring solver.
/// It is appended after [`GizmoHandle::ALL`] so every existing handle keeps its pick
/// id; [`GizmoTarget::Origin`] takes the next id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoTarget {
    /// A transform handle (axis, plane or ring).
    Handle(GizmoHandle),
    /// The draggable origin/centre control.
    Origin,
}

impl GizmoTarget {
    /// Every target, in pick-id order: the handles first, then the origin.
    pub const ALL: [GizmoTarget; GizmoHandle::ALL.len() + 1] = [
        GizmoTarget::Handle(GizmoHandle::AxisX),
        GizmoTarget::Handle(GizmoHandle::AxisY),
        GizmoTarget::Handle(GizmoHandle::AxisZ),
        GizmoTarget::Handle(GizmoHandle::PlaneXY),
        GizmoTarget::Handle(GizmoHandle::PlaneXZ),
        GizmoTarget::Handle(GizmoHandle::PlaneYZ),
        GizmoTarget::Handle(GizmoHandle::RotateX),
        GizmoTarget::Handle(GizmoHandle::RotateY),
        GizmoTarget::Handle(GizmoHandle::RotateZ),
        GizmoTarget::Origin,
    ];

    /// The class of the target.
    pub fn kind(self) -> HandleKind {
        match self {
            GizmoTarget::Handle(handle) => handle.kind(),
            GizmoTarget::Origin => HandleKind::Origin,
        }
    }

    /// The transform handle this target is, if it is one.
    pub fn handle(self) -> Option<GizmoHandle> {
        match self {
            GizmoTarget::Handle(handle) => Some(handle),
            GizmoTarget::Origin => None,
        }
    }

    /// The pick id this target carries. The origin is appended after the handles, so
    /// the existing ids do not move.
    pub fn pick_id(self) -> u32 {
        match self {
            GizmoTarget::Handle(handle) => handle.pick_id(),
            GizmoTarget::Origin => GIZMO_BASE_ID + GizmoHandle::ALL.len() as u32,
        }
    }

    /// The target carrying `id`, if any.
    pub fn from_pick_id(id: u32) -> Option<GizmoTarget> {
        let index = id.checked_sub(GIZMO_BASE_ID)? as usize;
        GizmoTarget::ALL.get(index).copied()
    }

    #[cfg(test)]
    fn index(self) -> usize {
        GizmoTarget::ALL
            .iter()
            .position(|target| *target == self)
            .expect("target is one of ALL")
    }
}

/// Above this `|ray.direction · axis|` the axis is too close to the view direction for
/// the parametric solver, so the screen-space fallback takes over.
const VIEW_ALIGNED_COS: f64 = 0.97;
/// Below this projected length (device px) an axis points at the eye and cannot be
/// scrubbed unambiguously; the handle is disabled for that frame.
const MIN_SCREEN_AXIS_PIXELS: f64 = 1.0;
/// Where an axis becomes pickable along its stem, as a fraction of the world scale. The
/// inner part is where all three axes meet; excluding it leaves the centre to the
/// separate origin/centre handle (or to the scene while that handle is disabled).
const AXIS_PICK_START: f64 = 0.18;
/// The number of segments in a ring's hit polyline (matches `ring_handle`).
const RING_PICK_SEGMENTS: usize = 48;
/// Below this `|ray.direction · ring_axis|` the ring is too edge-on for the 3D
/// plane/radius test; the screen-space polyline fallback takes over.
const RING_EDGE_ON_COS: f64 = 0.2;
/// The projected radius of the origin/centre disc, as a multiple of the handle stroke.
/// With the default 2 px stroke the disc is 5 px in radius inside the 10 px hit disc
/// and the stems' excluded 18 % band.
const ORIGIN_RADIUS_PER_STROKE: f64 = 2.5;

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
    /// Free rotation: bypass the ring detents.
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

/// The manipulator id a gizmo commit carries (`InteractionCommit::manipulator`).
pub const TRANSFORM_ID: &str = "transform";

impl GizmoCommitEvent {
    /// The neutral commit a host routes to the workbench that owns the object.
    ///
    /// The gizmo is a manipulator like any other: the commit names [`TRANSFORM_ID`],
    /// the action `"apply"`, and carries the 16 row-major elements of the
    /// world-space transform accumulated during the drag. A workbench composes it
    /// with the object's current placement; the shell never needs to know what the
    /// transform means.
    pub fn to_interaction(&self) -> InteractionCommit {
        InteractionCommit::new(TRANSFORM_ID, self.target_id, "apply")
            .with_values(self.total_transform.m)
            .with_readout(self.value.readout())
    }
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
    /// The workspace appearance and behaviour.
    style: GizmoStyle,
    /// The current world radius (recomputed from the camera by [`update`](Self::update)).
    scale: f64,
    /// The last camera view used to build the screen-facing ribbons.
    view: Option<Matrix4>,
    /// The linear grid step, if grid snapping is enabled.
    linear_snap: Option<f64>,
    hovered: Option<GizmoTarget>,
    active: Option<ActiveDrag>,
    /// A draw-only offset from the interaction frame, used while a host previews a
    /// translation during a drag. `drag_to` keeps measuring against `frame`, so moving
    /// the drawing cannot feed back into the gesture.
    display_offset: Option<Vector3>,
    /// Whether the origin/centre control participates. A host disables it while the
    /// floating Move form is open: that HUD is anchored to the visible origin and must
    /// not slide under the user's typing.
    origin_enabled: bool,
    dirty: bool,
    /// The per-handle alpha multiplier from the grazing-angle test (`1.0` square to the
    /// view, `0.0` collapsed to a line or dot). Rebuilt with the geometry.
    fade: [f32; GizmoTarget::ALL.len()],
    /// Ten fixed slots (one per [`GizmoTarget`]); unused ones are cleared. Stable mesh
    /// ids, so a rebuild is a geometry write, not a new mesh.
    handles: Vec<ShapeItem>,
    /// The non-pickable outline pass drawn behind `handles` (a wider ribbon or disc in
    /// [`GizmoStyle::outline`]). Empty slots when the halo is disabled.
    halos: Vec<ShapeItem>,
}

impl TransformGizmo {
    /// A gizmo for `target_id`, centred at `origin` and aligned with the world axes.
    pub fn new(target_id: u32, origin: Vector3, mode: GizmoMode) -> Self {
        Self {
            target_id,
            frame: ManipulatorFrame::world(origin),
            mode,
            style: GizmoStyle::default(),
            scale: 0.0,
            view: None,
            linear_snap: None,
            hovered: None,
            active: None,
            display_offset: None,
            origin_enabled: true,
            dirty: true,
            fade: [1.0; GizmoTarget::ALL.len()],
            handles: (0..GizmoTarget::ALL.len()).map(new_handle).collect(),
            halos: (0..GizmoTarget::ALL.len()).map(new_halo).collect(),
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

    /// The draw-only offset from the interaction frame, if any.
    pub fn display_offset(&self) -> Option<Vector3> {
        self.display_offset
    }

    /// Offset the **drawn** handles without moving the interaction frame.
    ///
    /// A host uses this while a drag previews the target: the triad stays glued to the
    /// moving surface, while [`drag_to`](Self::drag_to) keeps measuring against the
    /// origin captured at the press. `None` clears it. Hit testing and drag math always
    /// use [`origin`](Self::origin), so the offset can never feed back into a gesture.
    pub fn set_display_offset(&mut self, offset: Option<Vector3>) {
        if self.display_offset == offset {
            return;
        }
        self.display_offset = offset;
        self.apply_colors();
    }

    /// The world point the drawing shows for `point` (the frame point plus the
    /// draw-only offset).
    fn display_point(&self, point: Vector3) -> Vector3 {
        match self.display_offset {
            Some(offset) => point.add(&offset),
            None => point,
        }
    }

    /// The origin in device pixels, for a host that floats a HUD beside the handles.
    /// `None` when the origin is behind the camera.
    pub fn origin_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.display_point(self.frame.origin))
    }

    /// Move the gizmo (rebuilds the handle geometry on the next [`update`](Self::update)).
    pub fn set_origin(&mut self, origin: Vector3) {
        if self.frame.origin == origin {
            return;
        }
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

    /// Apply the workspace gizmo style. Returns whether the stored style changed.
    ///
    /// A change recolours the handles immediately and rebuilds their geometry on the
    /// next [`update`](Self::update) (the size and the stroke width are baked into the
    /// ribbons). The default style reproduces the legacy tokens and footprint.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        if self.style == *style {
            return false;
        }
        self.style = style.clone();
        self.dirty = true;
        self.apply_colors();
        true
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        &self.style
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
        scale_for(camera, self.frame.origin, self.style.size)
    }

    /// The transform handle under the pointer (set by the host from a pick).
    ///
    /// The origin/centre control is not a transform handle; use
    /// [`hovered_target`](Self::hovered_target) when it matters.
    pub fn hovered(&self) -> Option<GizmoHandle> {
        self.hovered.and_then(GizmoTarget::handle)
    }

    /// The target under the pointer (a transform handle or the origin control).
    pub fn hovered_target(&self) -> Option<GizmoTarget> {
        self.hovered
    }

    /// Whether the origin/centre control participates (drawn and pickable).
    pub fn origin_enabled(&self) -> bool {
        self.origin_enabled
    }

    /// Enable or disable the origin/centre control. A host disables it while the
    /// floating Move form is open. Returns whether the state changed.
    pub fn set_origin_enabled(&mut self, enabled: bool) -> bool {
        if self.origin_enabled == enabled {
            return false;
        }
        self.origin_enabled = enabled;
        if !enabled && self.hovered == Some(GizmoTarget::Origin) {
            self.hovered = None;
        }
        self.dirty = true;
        self.apply_colors();
        true
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// The handle slots, for drawing and picking.
    pub fn items(&self) -> &[ShapeItem] {
        &self.handles
    }

    /// The outline pass drawn **before** [`items`](Self::items).
    ///
    /// These are wider ribbons in [`GizmoStyle::outline`] and are not selectable, so
    /// the WCAG non-text-contrast halo never becomes an extra pick target.
    pub fn halos(&self) -> &[ShapeItem] {
        &self.halos
    }

    /// The handle items re-materialised with an authored skin program.
    ///
    /// The clone keeps the same mesh identity and generation (so the renderer's upload
    /// cache still hits) and only the material changes; the native [`TransformGizmo`]
    /// remains the hit-test and drag owner.
    pub fn skinned_items(&self, program: &Arc<ShaderProgram>) -> Vec<ShapeItem> {
        self.handles
            .iter()
            .cloned()
            .map(|mut item| {
                item.set_program(Some(Arc::clone(program)));
                item
            })
            .collect()
    }

    /// The gizmo target under `device` within the analytic grab radius, or `None` when
    /// the pointer is in the triad's empty interior (which must pass through to the
    /// scene). The origin/centre disc is a target of its own; the empty interior is the
    /// space just outside it.
    ///
    /// The test projects the same axis segments, plane quads and ring polylines the
    /// producers draw, then measures screen-space distance. It is **analytic** rather
    /// than a rasterised pick so the grab radius does not require extra pick geometry,
    /// which would swallow clicks meant for background features. The radius is
    /// [`GizmoStyle::hit_radius_px`], floored at [`MIN_HIT_RADIUS_PX`] so a thin stem
    /// still has a 24 px target (WCAG 2.5.8).
    ///
    /// Priority is **origin > axis > plane > ring**, then nearest stroke, then
    /// front-most. Only handles shown by [`GizmoMode`] participate, and the origin
    /// control only while [`origin_enabled`](Self::origin_enabled) (a host disables it
    /// while the Move HUD is open).
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> Option<GizmoTarget> {
        let scale = self.screen_scale(camera);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let slop = self.style.hit_radius_px.max(MIN_HIT_RADIUS_PX);
        let frame = self.frame;
        let eye = camera_position(&camera.view);
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let Some(ray) = camera.ray_through(device) else {
            return None;
        };
        let mut best: Option<(f64, f64, GizmoTarget)> = None;

        // 0. The origin/centre control: a 10 px projected disc. It sits inside the axis
        //    stems' excluded 18 % so it steals no stem; being tested first gives it the
        //    documented priority when a smaller gizmo makes the bands overlap.
        if self.origin_enabled && self.mode.shows(GizmoTarget::Origin) {
            if let Some(projected) = project_device(camera, frame.origin) {
                let distance = (projected[0] - pointer[0]).hypot(projected[1] - pointer[1]);
                if distance <= FEATURE_PIXELS {
                    return Some(GizmoTarget::Origin);
                }
            }
        }

        // 1. Axis stems. The pick segment starts `AXIS_PICK_START` along the stem so the
        //    shared centre is not ambiguously claimed by three handles at once.
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(GizmoTarget::Handle(handle)) || handle.kind() != HandleKind::Axis {
                continue;
            }
            if self.grazing_fade(GizmoTarget::Handle(handle), camera) <= GRAZING_HIT_FLOOR {
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
            if distance > slop {
                continue;
            }
            let world = start.add(&end.sub(&start).scale(t));
            consider(
                &mut best,
                distance,
                eye.sub(&world).length(),
                GizmoTarget::Handle(handle),
            );
        }
        if let Some((_, _, target)) = best {
            return Some(target);
        }

        // 2. Plane quads: the whole quad is grabbable, plus padding around its rim.
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(GizmoTarget::Handle(handle)) || handle.kind() != HandleKind::Plane {
                continue;
            }
            if self.grazing_fade(GizmoTarget::Handle(handle), camera) <= GRAZING_HIT_FLOOR {
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
            if distance > slop {
                continue;
            }
            let centre = frame
                .origin
                .add(&u.scale(0.525 * scale))
                .add(&v.scale(0.525 * scale));
            consider(
                &mut best,
                distance,
                eye.sub(&centre).length(),
                GizmoTarget::Handle(handle),
            );
        }
        if let Some((_, _, target)) = best {
            return Some(target);
        }

        // 3. Rotation rings. A visible ring is tested in its own plane: the ray's hit
        //    must land near the ring radius. An edge-on ring projects to a line through
        //    the centre, so its screen fallback excludes the inner part exactly like the
        //    axis stems (otherwise the centre would be "on" every edge-on ring).
        for handle in GizmoHandle::ALL {
            if !self.mode.shows(GizmoTarget::Handle(handle)) || handle.kind() != HandleKind::Ring {
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
                if error > slop * world_per_pixel {
                    continue;
                }
                let ring_point = frame.origin.add(&radial.scale(scale / radius));
                consider(
                    &mut best,
                    error / world_per_pixel,
                    eye.sub(&ring_point).length(),
                    GizmoTarget::Handle(handle),
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
                if nearest > slop {
                    continue;
                }
                consider(
                    &mut best,
                    nearest,
                    eye.sub(&nearest_world).length(),
                    GizmoTarget::Handle(handle),
                );
            }
        }

        best.map(|(_, _, target)| target)
    }

    /// Highlight `handle` (or clear). Returns whether the visuals changed.
    ///
    /// This is the transform-handle form; the origin control is not a handle. A host
    /// that hit-tests the whole gizmo uses [`set_hovered_target`](Self::set_hovered_target).
    pub fn set_hovered(&mut self, handle: Option<GizmoHandle>) -> bool {
        self.set_hovered_target(handle.map(GizmoTarget::Handle))
    }

    /// Highlight a gizmo target (a transform handle or the origin control), or clear.
    /// Returns whether the visuals changed.
    pub fn set_hovered_target(&mut self, target: Option<GizmoTarget>) -> bool {
        if self.hovered == target {
            return false;
        }
        self.hovered = target;
        // The hover cue is also a stroke-width change, which is baked into the ribbon
        // geometry; mark the gizmo for a rebuild as well as recolouring it.
        self.dirty = true;
        self.apply_colors();
        true
    }

    /// Recompute the screen-invariant scale for `camera` and rebuild the handles if it
    /// changed (or if the origin/mode did). Returns whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let scale = scale_for(camera, self.frame.origin, self.style.size);
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
            HandleKind::Origin => return false,
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
        self.hovered = Some(GizmoTarget::Handle(handle));
        self.apply_colors();
        true
    }

    /// Advance the active drag to the pointer's `device` position, returning the tick's
    /// deltas. Rotation rings use the style's detent (15° by default);
    /// [`drag_to_with_modifiers`](Self::drag_to_with_modifiers) lets the host bypass it
    /// with `Ctrl`.
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
        let ring_snap = self.style.ring_snap_deg.to_radians();
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
                let (_, applied) = ring_detent(raw, modifiers.ctrl, ring_snap);
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
                self.display_point(
                    self.frame
                        .origin
                        .add(&axis.scale(self.screen_scale(camera) * 1.05)),
                )
            }
            DragStart::Plane { .. } => self.display_point(self.frame.origin),
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

    /// The alpha multiplier for a handle at the current view: `1.0` when the handle faces
    /// the camera, falling to `0.0` when it collapses to a line or a dot.
    ///
    /// A nearly edge-on axis points into the screen and its arrowhead sits in the origin
    /// cluster; a nearly edge-on plane flattens to a sliver. Both are faded out and
    /// dropped from the analytic hit test so a 2 px artifact cannot be grabbed by accident.
    /// A ring stays live even edge-on: it still projects to an arc.
    fn grazing_fade(&self, target: GizmoTarget, camera: &Camera) -> f32 {
        let forward = camera_forward(camera);
        let Some(handle) = target.handle() else {
            // The origin control is a screen-facing disc and never collapses.
            return 1.0;
        };
        match handle.kind() {
            HandleKind::Axis => {
                let alignment = self
                    .frame
                    .axis(handle)
                    .map(|axis| axis.dot(&forward).abs())
                    .unwrap_or(0.0);
                ((AXIS_EDGE_ON_COS - alignment) / (AXIS_EDGE_ON_COS - AXIS_CLEAR_COS))
                    .clamp(0.0, 1.0) as f32
            }
            HandleKind::Plane => {
                let alignment = self
                    .frame
                    .plane_normal(handle)
                    .map(|normal| normal.dot(&forward).abs())
                    .unwrap_or(1.0);
                ((alignment - PLANE_EDGE_ON_COS) / (PLANE_CLEAR_COS - PLANE_EDGE_ON_COS))
                    .clamp(0.0, 1.0) as f32
            }
            HandleKind::Ring => 1.0,
            HandleKind::Origin => 1.0,
        }
    }

    /// Write every slot's geometry for the current mode, scale and camera.
    fn build(&mut self, camera: &Camera) {
        let frame = self.frame;
        let origin = frame.origin;
        let scale = self.scale;
        let width = self.style.width;
        let hover_scale = self.style.hover_scale.max(1.0);
        let halo = self.style.halo_width.max(0.0);
        let world_per_pixel = scale / self.style.size.max(1e-9);
        let forward = camera_forward(camera);
        for (index, target) in GizmoTarget::ALL.iter().enumerate() {
            let fade = self.grazing_fade(*target, camera);
            self.fade[index] = fade;
            let live = self.mode.shows(*target)
                && fade > 1e-3
                && !(*target == GizmoTarget::Origin && !self.origin_enabled);
            let slot = &mut self.handles[index];
            let halo_slot = &mut self.halos[index];
            if !live {
                slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                halo_slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                continue;
            }
            // The non-colour hover cue: a hovered stem/ring gets a wider stroke (and
            // therefore a wider halo); the origin disc grows the same way.
            let hovered = self.hovered == Some(*target);
            let stroke = if hovered { width * hover_scale } else { width };
            let halo_stroke = stroke + 2.0 * halo;
            let Some(handle) = target.handle() else {
                // The origin/centre control: a camera-facing disc in the same tokens as
                // the rest of the triad, with a wider disc behind it as its casing.
                let radius = stroke * ORIGIN_RADIUS_PER_STROKE;
                origin_handle(slot, origin, forward, radius, world_per_pixel);
                if halo > 0.0 {
                    origin_handle(halo_slot, origin, forward, radius + halo, world_per_pixel);
                } else {
                    halo_slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                }
                continue;
            };
            match handle.kind() {
                HandleKind::Origin => {}
                HandleKind::Axis => {
                    axis_handle(
                        slot,
                        origin,
                        frame.axis(handle).expect("axis"),
                        scale,
                        world_per_pixel,
                        stroke,
                        forward,
                    );
                    if halo > 0.0 {
                        axis_handle(
                            halo_slot,
                            origin,
                            frame.axis(handle).expect("axis"),
                            scale,
                            world_per_pixel,
                            halo_stroke,
                            forward,
                        );
                    } else {
                        halo_slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                    }
                }
                HandleKind::Plane => {
                    let (u, v) = frame.plane_axes(handle).expect("plane axes");
                    plane_handle(slot, origin, u, v, scale, hovered);
                    if halo > 0.0 {
                        plane_halo(
                            halo_slot,
                            origin,
                            u,
                            v,
                            scale,
                            halo / self.style.size.max(1e-9),
                            hovered,
                        );
                    } else {
                        halo_slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                    }
                }
                HandleKind::Ring => {
                    ring_handle(
                        slot,
                        origin,
                        frame.axis(handle).expect("ring"),
                        scale,
                        world_per_pixel,
                        stroke,
                        forward,
                    );
                    if halo > 0.0 {
                        ring_handle(
                            halo_slot,
                            origin,
                            frame.axis(handle).expect("ring"),
                            scale,
                            world_per_pixel,
                            halo_stroke,
                            forward,
                        );
                    } else {
                        halo_slot.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                    }
                }
            }
        }
        self.apply_colors();
        self.dirty = false;
    }

    /// Recolour every slot: active cyan or inactive slate, with a glass fill and rim for
    /// planes. Also re-applies the draw-only preview offset (a rebuild replaces the
    /// handle items' temporary transforms).
    fn apply_colors(&mut self) {
        let offset = self
            .display_offset
            .map(|offset| Matrix4::translation(offset.x, offset.y, offset.z));
        let outline_alpha = self.style.outline[3] * self.style.opacity;
        for (index, target) in GizmoTarget::ALL.iter().enumerate() {
            let fade = self.fade[index];
            let (mut surface, mut edge) =
                paint_for(*target, self.hovered == Some(*target), &self.style);
            surface[3] *= fade;
            edge[3] *= fade;
            self.handles[index].set_surface_color(surface);
            self.handles[index].set_edge_color(edge);
            self.handles[index].set_temporary_transform(offset);
            let outline = [
                self.style.outline[0],
                self.style.outline[1],
                self.style.outline[2],
                outline_alpha * fade,
            ];
            self.halos[index].set_surface_color(outline);
            self.halos[index].set_temporary_transform(offset);
        }
    }
}

impl GizmoMode {
    /// Whether `target` is shown in this mode. The origin/centre control is shown in
    /// every mode; it is the pivot, not a degree of freedom.
    fn shows(self, target: GizmoTarget) -> bool {
        match target {
            GizmoTarget::Origin => true,
            GizmoTarget::Handle(handle) => match self {
                GizmoMode::Translate => handle.kind() != HandleKind::Ring,
                GizmoMode::Rotate => handle.kind() == HandleKind::Ring,
                GizmoMode::Universal => true,
            },
        }
    }
}

// --- geometry ---------------------------------------------------------------------

/// A fresh handle slot: the pick id and the draw mode its kind needs.
///
/// A handle is a **control overlay**, so its surface and wire passes use
/// [`OVERLAY_DEPTH`](crate::shape::OVERLAY_DEPTH): an object-origin triad otherwise sits
/// inside the body, fails the `Less` test, and shows only its occluded ghost pass.
fn new_handle(index: usize) -> ShapeItem {
    let target = GizmoTarget::ALL[index];
    let mode = match target.kind() {
        HandleKind::Plane => DrawMode::ShadedWithEdges,
        HandleKind::Axis | HandleKind::Ring | HandleKind::Origin => DrawMode::Shaded,
    };
    let item = ShapeItem::new(GIZMO_BASE_ID + index as u32)
        .with_draw_mode(mode)
        .with_surface_depth(crate::shape::OVERLAY_DEPTH)
        .with_wire_depth(crate::shape::OVERLAY_DEPTH);
    // Axes, rings and the origin disc are screen-space controls and take the unlit,
    // feathered stage. A plane is a quad whose uv is not a lateral coordinate, so it
    // stays on `fs_main`.
    if matches!(
        target.kind(),
        HandleKind::Axis | HandleKind::Ring | HandleKind::Origin
    ) {
        item.with_ribbon_aa()
    } else {
        item
    }
}

/// The outline slot behind a handle: a wider ribbon in the outline ink. It is
/// non-selectable so the halo never adds a pick target, and its ids sit outside the
/// handle pick range.
fn new_halo(index: usize) -> ShapeItem {
    let target = GizmoTarget::ALL[index];
    let item = ShapeItem::new(GIZMO_BASE_ID + 100 + index as u32)
        .with_draw_mode(DrawMode::Shaded)
        .with_surface_depth(crate::shape::OVERLAY_DEPTH)
        .with_selectable(false);
    if matches!(
        target.kind(),
        HandleKind::Axis | HandleKind::Ring | HandleKind::Origin
    ) {
        item.with_ribbon_aa()
    } else {
        item
    }
}

/// A translation axis: a style-width shaft ribbon plus a filled arrowhead at the tip.
///
/// The arrowhead is one triangle rather than two stroked wings: separate ribbon caps at
/// the tip left a notch where their halos met, which read as a boxy pixel cluster at
/// high zoom. A filled triangle has a single outline and no joint.
fn axis_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    axis: Vector3,
    scale: f64,
    world_per_pixel: f64,
    width: f64,
    forward: Vector3,
) {
    let tip = origin.add(&axis.scale(scale));
    let half = width * 0.5 * world_per_pixel;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut triangles = Vec::new();

    push_ribbon_uv(
        &mut positions,
        &mut uvs,
        &mut triangles,
        screen_ribbon(origin, tip, half, forward),
    );

    // A filled arrowhead just behind the tip, in the plane of the stem and the
    // screen-perpendicular. The wing span is deliberately larger than the stem; the
    // shape is dilated by the ribbon's half-width so the halo pass gets an even outline.
    let depth = 9.0 * world_per_pixel;
    let spread = 7.0 * world_per_pixel;
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
    let (tip, left, right) = dilate_triangle(tip, left, right, half);
    let start = positions.len() as u32;
    for point in [tip, left, right] {
        positions.push(to3(point));
        // A constant lateral centre keeps the fill at full coverage; its silhouette is
        // anti-aliased by MSAA, not by the ribbon feather.
        uvs.push(RIBBON_UV_CENTRE);
    }
    triangles.extend_from_slice(&[start, start + 1, start + 2]);

    item.set_shape_uv(positions, Vec::new(), uvs, triangles, Vec::new());
}

/// Grow a triangle by `grow` world units, scaling it about its centroid by the factor
/// that moves each edge out by `grow` (its inradius is `area / semiperimeter`).
fn dilate_triangle(
    tip: Vector3,
    left: Vector3,
    right: Vector3,
    grow: f64,
) -> (Vector3, Vector3, Vector3) {
    let a = left.sub(&tip);
    let b = right.sub(&tip);
    let area = a.cross(&b).length() * 0.5;
    let sides = a.length() + b.length() + left.sub(&right).length();
    let inradius = (area / (sides * 0.5)).max(1e-12);
    let factor = 1.0 + grow / inradius;
    let centroid = tip.add(&left).add(&right).scale(1.0 / 3.0);
    let outward = |vertex: Vector3| centroid.add(&vertex.sub(&centroid).scale(factor));
    (outward(tip), outward(left), outward(right))
}

/// A planar handle: a translucent glass square with an outer L-corner rim.
///
/// At rest only the two edges at the far corner are drawn: a full four-sided rim made
/// the 0.35..0.7 quad read as a floating rectangle. A **hovered** plane closes the rim
/// so the whole tile reads like an arrow handle (the dark halo below does the same).
fn plane_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    u: Vector3,
    v: Vector3,
    scale: f64,
    hovered: bool,
) {
    let corner = |a: f64, b: f64| to3(origin.add(&u.scale(a * scale)).add(&v.scale(b * scale)));
    let positions = vec![
        corner(0.35, 0.35),
        corner(0.7, 0.35),
        corner(0.7, 0.7),
        corner(0.35, 0.7),
    ];
    let edges = if hovered {
        vec![0, 1, 1, 2, 2, 3, 3, 0]
    } else {
        // The far corner is vertex 2; only its two incident edges form the L.
        vec![1, 2, 2, 3]
    };
    item.set_shape(positions, Vec::new(), vec![0, 1, 2, 0, 2, 3], edges);
}

/// The glass plane's halo: an L-corner band just outside its two outer edges.
///
/// A filled backing quad would draw an opaque dark rectangle *through* the plane's
/// `0.10`-alpha glass and hide the fill; a full four-band frame would close the square
/// again. Only the two bands next to the drawn rim are emitted.
fn plane_halo(
    item: &mut ShapeItem,
    origin: Vector3,
    u: Vector3,
    v: Vector3,
    scale: f64,
    margin: f64,
    hovered: bool,
) {
    let corner = |a: f64, b: f64| to3(origin.add(&u.scale(a * scale)).add(&v.scale(b * scale)));
    let lo = 0.35;
    let hi = 0.7;
    let outer_lo = lo - margin;
    let outer_hi = hi + margin;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut quad = |a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)| {
        let base = positions.len() as u32;
        positions.extend([
            corner(a.0, a.1),
            corner(b.0, b.1),
            corner(c.0, c.1),
            corner(d.0, d.1),
        ]);
        triangles.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    // At rest only the two outer bands, matching the plane's L-corner rim: a full
    // rectangle would close the square again. A hovered plane gets the full 1 px casing
    // so it matches the dark-cased arrow handles.
    if hovered {
        quad(
            (outer_lo, outer_lo),
            (outer_hi, outer_lo),
            (outer_hi, lo),
            (outer_lo, lo),
        );
    }
    quad(
        (outer_lo, hi),
        (outer_hi, hi),
        (outer_hi, outer_hi),
        (outer_lo, outer_hi),
    );
    if hovered {
        quad((outer_lo, lo), (lo, lo), (lo, hi), (outer_lo, hi));
    }
    quad((hi, lo), (outer_hi, lo), (outer_hi, hi), (hi, hi));
    item.set_shape(positions, Vec::new(), triangles, Vec::new());
}

/// A rotation ring: a style-width ribbon around the circle.
fn ring_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    axis: Vector3,
    scale: f64,
    world_per_pixel: f64,
    width: f64,
    forward: Vector3,
) {
    let (u, v) = orthonormal_basis(axis);
    let half = width * 0.5 * world_per_pixel;
    let segments = 48usize;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
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
        push_ribbon_uv(
            &mut positions,
            &mut uvs,
            &mut triangles,
            screen_ribbon(from, to, half, forward),
        );
    }
    item.set_shape_uv(positions, Vec::new(), uvs, triangles, Vec::new());
}

/// The origin/centre control: a camera-facing disc in the handle ink.
///
/// It is a filled fan rather than a ribbon so it stays a stable disc at any view; the
/// host draws its wider halo slot behind it as the same casing the stems use. `radius`
/// is in device pixels and `world_per_pixel` comes from the gizmo's screen-invariant
/// scale, so the disc keeps its footprint as the camera zooms.
fn origin_handle(
    item: &mut ShapeItem,
    origin: Vector3,
    forward: Vector3,
    radius_pixels: f64,
    world_per_pixel: f64,
) {
    let (u, v) = orthonormal_basis(forward);
    let radius = radius_pixels * world_per_pixel;
    let segments = 24usize;
    let mut positions = Vec::with_capacity(segments + 1);
    let mut uvs = Vec::with_capacity(segments + 1);
    positions.push(to3(origin));
    // The filled cap uses the lateral centre everywhere, so the ribbon feather keeps
    // the whole disc at full coverage (only its silhouette is anti-aliased).
    uvs.push(RIBBON_UV_CENTRE);
    for i in 0..segments {
        let angle = TAU * i as f64 / segments as f64;
        let point = origin
            .add(&u.scale(radius * angle.cos()))
            .add(&v.scale(radius * angle.sin()));
        positions.push(to3(point));
        uvs.push(RIBBON_UV_CENTRE);
    }
    let mut triangles = Vec::with_capacity(segments * 3);
    for i in 0..segments {
        let next = (i + 1) % segments;
        triangles.extend_from_slice(&[0, i as u32 + 1, next as u32 + 1]);
    }
    item.set_shape_uv(positions, Vec::new(), uvs, triangles, Vec::new());
}

/// The surface and edge colours for a handle. Axes and rings are strokes; planes are a
/// translucent glass fill with a stroke rim.
///
/// The visible alpha is the token alpha times [`GizmoStyle::opacity`]. The glass fill
/// always uses the style's active RGB, at the legacy `0.10`/`0.28` ink scaled by the
/// opacity, so the inactive stroke does not tint the plane.
fn paint_for(target: GizmoTarget, hovered: bool, style: &GizmoStyle) -> ([f32; 4], [f32; 4]) {
    let token = if hovered {
        style.active
    } else {
        style.inactive
    };
    let stroke = [token[0], token[1], token[2], token[3] * style.opacity];
    match target.kind() {
        HandleKind::Plane => {
            let glass = if hovered {
                GLASS_HIGHLIGHT[3]
            } else {
                GLASS_COLOR[3]
            } * style.opacity;
            (
                [style.active[0], style.active[1], style.active[2], glass],
                stroke,
            )
        }
        HandleKind::Axis | HandleKind::Ring | HandleKind::Origin => (stroke, stroke),
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
    best: &mut Option<(f64, f64, GizmoTarget)>,
    distance: f64,
    depth: f64,
    target: GizmoTarget,
) {
    let replace = match best {
        None => true,
        Some((best_distance, best_depth, _)) => {
            distance < *best_distance - 1e-9
                || ((distance - *best_distance).abs() <= 1e-9 && depth < *best_depth)
        }
    };
    if replace {
        *best = Some((distance, depth, target));
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
pub(crate) fn ring_angle_with_fallback(
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

/// Snap a ring's raw accumulated angle to relative `detent` radians, or leave it free.
/// Returns `(raw, applied)`; the detents are relative to the drag's start, so grabbing a
/// rotated body does not jump on the first pixel. A non-positive `detent` is continuous.
fn ring_detent(raw: f64, free: bool, detent: f64) -> (f64, f64) {
    let raw = wrap_pi(raw);
    if free || !(detent > 1e-9) {
        (raw, raw)
    } else {
        (raw, (raw / detent).round() * detent)
    }
}

/// Two unit vectors spanning the plane perpendicular to `axis`.
pub(crate) fn orthonormal_basis(axis: Vector3) -> (Vector3, Vector3) {
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

    /// A 30° / 35° three-quarter view, so every triad handle is clear of the
    /// grazing-angle fade that an axis-aligned view triggers.
    fn tilted_camera(eye_z: f64) -> Camera {
        let rotation = Rotation::from_axis_angle(
            &Vector3::new(1.0, 0.0, 0.0),
            (-35.0_f64).to_radians(),
        )
        .multiply(&Rotation::from_axis_angle(
            &Vector3::new(0.0, 0.0, 1.0),
            35.0_f64.to_radians(),
        ));
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), rotation).to_matrix(),
            ..camera(eye_z)
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
    fn the_origin_target_is_appended_after_the_handles() {
        // Every existing handle keeps its pick id; the origin takes the next one.
        for handle in GizmoHandle::ALL {
            assert_eq!(
                GizmoTarget::Handle(handle).pick_id(),
                handle.pick_id(),
                "the handle ids are unchanged"
            );
        }
        let origin = GizmoTarget::Origin.pick_id();
        assert_eq!(origin, GIZMO_BASE_ID + GizmoHandle::ALL.len() as u32);
        assert_eq!(GizmoTarget::from_pick_id(origin), Some(GizmoTarget::Origin));

        let mut seen = Vec::new();
        for target in GizmoTarget::ALL {
            let id = target.pick_id();
            assert!(!seen.contains(&id), "pick ids are distinct");
            seen.push(id);
            assert_eq!(GizmoTarget::from_pick_id(id), Some(target));
        }
        assert_eq!(GizmoTarget::from_pick_id(origin + 1), None);
    }

    #[test]
    fn the_origin_handle_is_pickable_inside_its_ten_pixel_disc() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        // Five pixels from the centre: inside the 10 px disc.
        assert_eq!(gizmo.hit(&camera, [55, 50]), Some(GizmoTarget::Origin));
        // Eleven pixels diagonally: outside the disc, and far enough from the stems'
        // pick segments that no handle claims it either.
        assert_eq!(gizmo.hit(&camera, [58, 58]), None);
    }

    #[test]
    fn the_origin_handle_wins_the_hit_priority() {
        let camera = camera(5.0);
        // A small triad makes the origin disc, the stem padding and the plane/ring bands
        // overlap, so the result shows which target is tested first.
        let mut style = GizmoStyle::default();
        style.size = 15.0;
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.set_style(&style);
        gizmo.update(&camera);

        // Seven pixels diagonally: inside the origin disc and within the padded band of
        // the X/Y stems, the XY plane and the Z ring.
        assert_eq!(gizmo.hit(&camera, [55, 55]), Some(GizmoTarget::Origin));
        // With the origin out of the way the same point resolves to a handle, proving the
        // bands really do overlap. (A point inside the origin disc is always within the
        // 12 px stem padding too, so origin > axis also covers the plane band.)
        assert!(gizmo.set_origin_enabled(false));
        gizmo.update(&camera);
        assert!(matches!(
            gizmo.hit(&camera, [55, 55]),
            Some(GizmoTarget::Handle(_))
        ));

        // Rings alone (no axes/planes): 10 px out is exactly on the origin disc and in
        // the edge-on XZ ring's padded band, and the origin still wins.
        let mut rings = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        rings.set_style(&style);
        rings.update(&camera);
        assert_eq!(rings.hit(&camera, [60, 50]), Some(GizmoTarget::Origin));
        assert!(rings.set_origin_enabled(false));
        rings.update(&camera);
        assert_eq!(
            rings.hit(&camera, [60, 50]),
            Some(GizmoTarget::Handle(GizmoHandle::RotateY))
        );
    }

    #[test]
    fn a_disabled_origin_is_not_drawn_or_picked() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        assert_eq!(gizmo.hit(&camera, CENTRE), Some(GizmoTarget::Origin));

        assert!(gizmo.set_origin_enabled(false));
        assert!(!gizmo.set_origin_enabled(false), "unchanged is a no-op");
        gizmo.update(&camera);
        assert_eq!(gizmo.hit(&camera, CENTRE), None);
        assert!(
            gizmo.items()[GizmoTarget::Origin.index()]
                .positions()
                .is_empty(),
            "the hidden origin draws nothing"
        );
        assert!(
            gizmo.halos()[GizmoTarget::Origin.index()]
                .positions()
                .is_empty()
        );

        assert!(gizmo.set_origin_enabled(true));
        gizmo.update(&camera);
        assert_eq!(gizmo.hit(&camera, CENTRE), Some(GizmoTarget::Origin));
    }

    #[test]
    fn the_origin_handle_uses_the_handle_tokens_and_casing() {
        let camera = tilted_camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera);
        let origin = GizmoTarget::Origin.index();
        assert_eq!(gizmo.items()[origin].surface_color(), INACTIVE_COLOR);
        assert!(!gizmo.items()[origin].positions().is_empty());

        // The casing is the same non-pickable outline pass the stems use.
        let halo = &gizmo.halos()[origin];
        assert!(!halo.selectable());
        assert!(!halo.positions().is_empty());
        assert_eq!(halo.surface_color(), OUTLINE_COLOR);

        assert!(gizmo.set_hovered_target(Some(GizmoTarget::Origin)));
        assert_eq!(gizmo.items()[origin].surface_color(), ACTIVE_COLOR);
        assert!(
            gizmo.update(&camera),
            "the hover cue grows the disc, so it rebuilds"
        );
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
            gizmo.update(&tilted_camera(5.0));
            gizmo.items()
                .iter()
                .filter(|item| {
                    let mut queue = ferrocad_graphics::RenderQueue::default();
                    item.emit(&mut queue);
                    !queue.is_empty()
                })
                .count()
        };
        assert_eq!(item_count(GizmoMode::Translate), 7);
        assert_eq!(item_count(GizmoMode::Rotate), 4);
        assert_eq!(item_count(GizmoMode::Universal), 10);
    }

    #[test]
    fn a_display_offset_moves_the_drawing_not_the_drag_frame() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        let camera = camera(5.0);
        gizmo.update(&camera);
        let origin = gizmo.origin();
        let offset = Vector3::new(5.0, 0.0, 2.0);

        gizmo.set_display_offset(Some(offset));
        assert_eq!(gizmo.origin(), origin, "the interaction frame is frozen");
        assert_eq!(gizmo.display_offset(), Some(offset));
        let drawn = gizmo.items()[GizmoHandle::AxisX.index()]
            .temporary_transform()
            .expect("the handles are offset for drawing");
        assert_eq!(drawn.transform(&Vector3::zero()), offset);

        gizmo.set_display_offset(None);
        assert!(
            gizmo.items()[GizmoHandle::AxisX.index()]
                .temporary_transform()
                .is_none()
        );
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
        let (raw, snapped) = ring_detent(7.0_f64.to_radians(), false, step);
        assert!((raw - 7.0_f64.to_radians()).abs() < 1e-12);
        assert!(snapped.abs() < 1e-12, "7° snaps back to the start");

        let (_, snapped) = ring_detent(8.0_f64.to_radians(), false, step);
        assert!(
            (snapped - step).abs() < 1e-12,
            "8° snaps to the first detent"
        );
        let (_, snapped) = ring_detent(-8.0_f64.to_radians(), false, step);
        assert!((snapped + step).abs() < 1e-12);

        let (raw, snapped) = ring_detent(7.0_f64.to_radians(), true, step);
        assert!((snapped - raw).abs() < 1e-12, "Ctrl is free rotation");

        // The raw delta is unwrapped across ±π.
        let (raw, _) = ring_detent(3.0 * std::f64::consts::FRAC_PI_2, false, step);
        assert!((raw + std::f64::consts::FRAC_PI_2).abs() < 1e-12, "{raw}");

        // A non-positive detent is continuous.
        let (raw, snapped) = ring_detent(7.0_f64.to_radians(), false, 0.0);
        assert!((snapped - raw).abs() < 1e-12, "a zero snap has no detent");
    }

    #[test]
    fn the_triad_interior_passes_through() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        // The re-pivot handle owns the shared centre (it is not a transform handle).
        assert_eq!(gizmo.hit(&camera(5.0), CENTRE), Some(GizmoTarget::Origin));
        // It is only a 10 px disc, so the interior just outside it still passes through:
        // the stems only start at 0.18·scale (~20 px) and the planes are farther out.
        assert_eq!(gizmo.hit(&camera(5.0), [65, 65]), None);
    }

    #[test]
    fn an_axis_is_pickable_along_its_stem() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        assert_eq!(
            gizmo.hit(&camera(5.0), [80, 50]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
    }

    #[test]
    fn the_grab_padding_reaches_past_the_stroke() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera(5.0));
        // Eight pixels above the X stem: outside the 2 px stroke, inside the 10 px grab
        // padding.
        assert_eq!(
            gizmo.hit(&camera(5.0), [80, 58]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
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
        // The XY quad spans 0.35..0.7·scale in +X/+Y; [92, 92] is inside that band.
        assert_eq!(
            gizmo.hit(&camera(5.0), [92, 92]),
            Some(GizmoTarget::Handle(GizmoHandle::PlaneXY))
        );
    }

    #[test]
    fn a_ring_is_pickable_on_its_arc() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        gizmo.update(&camera(5.0));
        // 45° on the Z ring at the 110 px radius (the X/Y rings are edge-on here).
        assert_eq!(
            gizmo.hit(&camera(5.0), [128, 128]),
            Some(GizmoTarget::Handle(GizmoHandle::RotateZ))
        );
    }

    #[test]
    fn an_axis_wins_over_an_overlapping_ring() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        // Forty pixels along +X lies on the X stem and on the edge-on Y ring's projected
        // line; the axis has priority.
        assert_eq!(
            gizmo.hit(&camera(5.0), [90, 50]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
    }

    #[test]
    fn hidden_handles_are_not_picked() {
        let mut translate = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        translate.update(&camera(5.0));
        assert_eq!(
            translate.hit(&camera(5.0), [95, 95]),
            Some(GizmoTarget::Handle(GizmoHandle::PlaneXY))
        );

        // The same point in rotate mode is between the displayed rings.
        let mut rotate = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        rotate.update(&camera(5.0));
        assert_eq!(rotate.hit(&camera(5.0), [95, 95]), None);
    }

    #[test]
    fn the_hit_does_not_need_a_preceding_update() {
        // `hit` recomputes the screen scale, so an event that arrives before the next
        // frame still lands on the drawn geometry.
        let gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert_eq!(
            gizmo.hit(&camera(5.0), [80, 50]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
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
        assert_eq!(
            gizmo.hit(&camera(5.0), [50, 80]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
        // Local Y is world −X, so the leftward stem is AxisY.
        assert_eq!(
            gizmo.hit(&camera(5.0), [20, 50]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisY))
        );
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

    /// The world width of an axis ribbon's shaft, measured across its first quad.
    fn shaft_width(item: &ShapeItem) -> f64 {
        let positions = item.positions();
        let point = |index: usize| {
            Vector3::new(
                f64::from(positions[index][0]),
                f64::from(positions[index][1]),
                f64::from(positions[index][2]),
            )
        };
        point(0).sub(&point(3)).length()
    }

    #[test]
    fn the_halo_is_a_wider_non_pickable_pass() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        let handle = &gizmo.items()[GizmoHandle::AxisX.index()];
        let halo = &gizmo.halos()[GizmoHandle::AxisX.index()];
        assert!(!halo.selectable(), "the halo must never be a pick target");
        assert!(!halo.positions().is_empty(), "the halo has a ribbon");
        assert_eq!(halo.surface_color(), OUTLINE_COLOR);

        // The halo is the stroke plus `halo_width` on each side.
        let expected =
            (DEFAULT_HALO_WIDTH * 2.0 + DEFAULT_GIZMO_WIDTH) * gizmo.scale() / DEFAULT_PIXELS;
        let width = shaft_width(halo);
        assert!(
            (width - expected).abs() < 1e-6,
            "the halo should be {expected} world units, got {width}"
        );
        assert!(width > shaft_width(handle));

        // The plane halo is an L-corner band (two quads), matching the rim: a full
        // rectangle would close the square again.
        let plane = &gizmo.halos()[GizmoHandle::PlaneXY.index()];
        assert_eq!(plane.positions().len(), 8, "two framing quads");
        assert_eq!(plane.triangles().len(), 12, "six indices per quad");
    }

    #[test]
    fn a_zero_halo_width_draws_no_halo() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        assert!(
            !gizmo.halos()[GizmoHandle::AxisX.index()]
                .positions()
                .is_empty()
        );

        let mut style = GizmoStyle::default();
        style.halo_width = 0.0;
        assert!(gizmo.set_style(&style));
        gizmo.update(&camera);
        for item in gizmo.halos() {
            assert!(
                item.positions().is_empty(),
                "a zero halo width must clear every slot"
            );
        }
    }

    #[test]
    fn the_grab_radius_follows_the_style() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        // 15 px above the X stem: outside the default 12 px grab radius.
        assert_eq!(gizmo.hit(&camera, [80, 65]), None);

        let mut style = GizmoStyle::default();
        style.hit_radius_px = 20.0;
        assert!(gizmo.set_style(&style));
        gizmo.update(&camera);
        assert_eq!(
            gizmo.hit(&camera, [80, 65]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
    }

    #[test]
    fn a_grab_radius_below_the_wcag_minimum_is_floored() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        let mut style = GizmoStyle::default();
        style.hit_radius_px = 0.0;
        assert!(gizmo.set_style(&style));
        gizmo.update(&camera);
        // 8 px off the stem is still inside the floored 12 px radius.
        assert_eq!(
            gizmo.hit(&camera, [80, 58]),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX))
        );
    }

    #[test]
    fn hovering_widens_the_stroke_and_its_halo() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        let idle = shaft_width(&gizmo.items()[GizmoHandle::AxisX.index()]);
        assert!(gizmo.set_hovered(Some(GizmoHandle::AxisX)));
        assert!(
            gizmo.update(&camera),
            "the non-colour hover cue must rebuild the geometry"
        );
        let hovered = shaft_width(&gizmo.items()[GizmoHandle::AxisX.index()]);
        assert!(
            (hovered / idle - DEFAULT_HOVER_SCALE).abs() < 1e-5,
            "the hover cue scales the stroke by {DEFAULT_HOVER_SCALE}: {hovered} vs {idle}"
        );
        // The halo follows the widened stroke so the outline stays even.
        let expected_halo = (DEFAULT_HALO_WIDTH * 2.0 + DEFAULT_GIZMO_WIDTH * DEFAULT_HOVER_SCALE)
            * gizmo.scale()
            / DEFAULT_PIXELS;
        let halo = shaft_width(&gizmo.halos()[GizmoHandle::AxisX.index()]);
        assert!(
            (halo - expected_halo).abs() < 1e-6,
            "the hovered halo should be {expected_halo} world units, got {halo}"
        );
    }

    #[test]
    fn a_hovered_plane_gets_the_full_casing() {
        let camera = tilted_camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        gizmo.update(&camera);
        let edges =
            |gizmo: &TransformGizmo| gizmo.items()[GizmoHandle::PlaneXY.index()].edges().len();
        let halo = |gizmo: &TransformGizmo| {
            gizmo.halos()[GizmoHandle::PlaneXY.index()]
                .positions()
                .len()
        };
        assert_eq!(edges(&gizmo), 4, "at rest the rim is the outer L");
        assert_eq!(halo(&gizmo), 8, "two halo bands at rest");

        assert!(gizmo.set_hovered(Some(GizmoHandle::PlaneXY)));
        assert!(gizmo.update(&camera), "the rim is baked, so hover rebuilds");
        assert_eq!(edges(&gizmo), 8, "the hovered rim closes the square");
        assert_eq!(halo(&gizmo), 16, "all four halo bands when hovered");
    }

    #[test]
    fn an_edge_on_handle_fades_and_leaves_the_hit_test() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera);

        // Looking down +Z: the Z axis points at the camera, the XZ/YZ planes are
        // edge-on, and the XY plane faces it.
        assert_eq!(
            gizmo.grazing_fade(GizmoTarget::Handle(GizmoHandle::AxisZ), &camera),
            0.0
        );
        assert_eq!(
            gizmo.grazing_fade(GizmoTarget::Handle(GizmoHandle::PlaneXZ), &camera),
            0.0
        );
        assert_eq!(
            gizmo.grazing_fade(GizmoTarget::Handle(GizmoHandle::PlaneYZ), &camera),
            0.0
        );
        assert_eq!(
            gizmo.grazing_fade(GizmoTarget::Handle(GizmoHandle::PlaneXY), &camera),
            1.0
        );
        assert!(
            gizmo.items()[GizmoHandle::AxisZ.index()]
                .positions()
                .is_empty(),
            "a faded axis keeps no geometry"
        );
        assert!(
            gizmo.items()[GizmoHandle::PlaneXZ.index()]
                .positions()
                .is_empty(),
            "a faded plane keeps no geometry"
        );
        assert!(
            !gizmo.items()[GizmoHandle::PlaneXY.index()]
                .positions()
                .is_empty()
        );

        // A three-quarter view brings the Z axis back.
        let tilted = tilted_camera(5.0);
        assert!(gizmo.update(&tilted));
        assert!(gizmo.grazing_fade(GizmoTarget::Handle(GizmoHandle::AxisZ), &tilted) > 0.9);
        assert!(
            !gizmo.items()[GizmoHandle::AxisZ.index()]
                .positions()
                .is_empty()
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
        assert_eq!(plane.edges().len(), 4, "only the outer L-corner is drawn");
    }

    #[test]
    fn the_manipulator_tokens_follow_the_active_state() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&tilted_camera(5.0));
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
    fn the_axis_ribbon_uses_the_default_gizmo_width() {
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
        let expected = DEFAULT_GIZMO_WIDTH * gizmo.scale() / DEFAULT_PIXELS;
        assert!(
            (width - expected).abs() < 1e-6,
            "the stroke should be {expected} world units, got {width}"
        );
    }

    #[test]
    fn the_default_style_carries_the_tokens_and_the_accessibility_defaults() {
        let style = GizmoStyle::default();
        assert!(style.enabled);
        assert_eq!(style.backend, "ferrocad");
        assert_eq!(style.active, ACTIVE_COLOR);
        assert_eq!(style.inactive, INACTIVE_COLOR, "the legacy 0.9 alpha");
        assert_eq!(style.outline, OUTLINE_COLOR);
        assert_eq!(style.size, DEFAULT_PIXELS);
        assert_eq!(style.width, DEFAULT_GIZMO_WIDTH);
        assert_eq!(style.halo_width, DEFAULT_HALO_WIDTH);
        assert_eq!(style.hover_scale, DEFAULT_HOVER_SCALE);
        assert_eq!(style.hit_radius_px, DEFAULT_HIT_RADIUS_PX);
        assert!(style.hit_radius_px >= MIN_HIT_RADIUS_PX, "24 px target");
        assert_eq!(style.opacity, 1.0);
        // Below the legacy 0.35 so an occluded handle stays visibly behind a front one.
        assert_eq!(style.ghost_alpha, DEFAULT_GHOST_ALPHA);
        assert!(style.ghost_alpha <= MAX_GHOST_ALPHA);
        assert_eq!(style.ring_snap_deg, 15.0);
        assert!(style.show_readout);

        let gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert_eq!(gizmo.style(), &style);
        // The footprint is the default 110 px radius.
        let camera = camera(5.0);
        let legacy = scale_for(&camera, Vector3::zero(), DEFAULT_PIXELS);
        assert!((gizmo.screen_scale(&camera) - legacy).abs() < 1e-12);
    }

    #[test]
    fn a_custom_style_changes_the_colours_the_size_the_width_and_the_snap() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        let mut style = GizmoStyle::default();
        style.active = [1.0, 0.0, 0.0, 1.0];
        style.inactive = [0.0, 1.0, 0.0, 0.8];
        style.opacity = 0.5;
        style.size = 40.0;
        style.width = 4.0;
        style.ring_snap_deg = 45.0;
        style.show_readout = false;
        assert!(gizmo.set_style(&style));
        gizmo.update(&camera);

        // The footprint and the stroke scale from the style, not the legacy tokens.
        let scale = scale_for(&camera, Vector3::zero(), 40.0);
        assert!((gizmo.scale() - scale).abs() < 1e-9);
        let positions = gizmo.items()[GizmoHandle::AxisX.index()].positions();
        let point = |index: usize| {
            Vector3::new(
                f64::from(positions[index][0]),
                f64::from(positions[index][1]),
                f64::from(positions[index][2]),
            )
        };
        let width = point(0).sub(&point(3)).length();
        let expected = 4.0 * scale / 40.0;
        assert!(
            (width - expected).abs() < 1e-6,
            "a 4 px stroke at a 40 px size: {width} vs {expected}"
        );

        // Idle ink is the custom inactive colour at `alpha * opacity`; the glass fill is
        // the active RGB at 0.10 * opacity.
        let surface = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].surface_color()
        };
        let edge = |gizmo: &TransformGizmo, handle: GizmoHandle| {
            gizmo.items()[handle.index()].edge_color()
        };
        assert_eq!(surface(&gizmo, GizmoHandle::AxisX), [0.0, 1.0, 0.0, 0.4]);
        assert_eq!(surface(&gizmo, GizmoHandle::PlaneXY), [1.0, 0.0, 0.0, 0.05]);
        assert_eq!(edge(&gizmo, GizmoHandle::PlaneXY), [0.0, 1.0, 0.0, 0.4]);

        // A hovered handle uses the active colour at `alpha * opacity`; the plane's
        // highlighted glass keeps its 0.28 ink, scaled by the opacity.
        assert!(gizmo.set_hovered(Some(GizmoHandle::PlaneXY)));
        assert_eq!(surface(&gizmo, GizmoHandle::PlaneXY), [1.0, 0.0, 0.0, 0.25]);
        assert_eq!(edge(&gizmo, GizmoHandle::PlaneXY), [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(surface(&gizmo, GizmoHandle::AxisX), [0.0, 1.0, 0.0, 0.4]);

        // The custom detent is what the ring drag applies.
        let detent = style.ring_snap_deg.to_radians();
        let (_, snapped) = ring_detent(20.0_f64.to_radians(), false, detent);
        assert!(snapped.abs() < 1e-12, "20° is below the 45° half-step");
        assert!(!gizmo.style().show_readout);
    }

    #[test]
    fn a_zero_ring_snap_is_continuous() {
        let camera = camera(5.0);
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Rotate);
        let mut style = GizmoStyle::default();
        style.ring_snap_deg = 0.0;
        assert!(gizmo.set_style(&style));
        assert!(gizmo.begin_drag(GizmoHandle::RotateZ, &camera, UP));
        let event = gizmo.drag_to(&camera, [60, 75]).expect("drag");
        match event.value {
            GizmoValue::Angular {
                raw_deg,
                applied_deg,
                ..
            } => assert!(
                (raw_deg - applied_deg).abs() < 1e-9,
                "a zero snap must not detent: {raw_deg} vs {applied_deg}"
            ),
            other => panic!("expected an angular value, got {other:?}"),
        }
    }

    #[test]
    fn set_style_reports_whether_anything_changed() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        assert!(
            !gizmo.set_style(&GizmoStyle::default()),
            "the default is a no-op"
        );

        let mut style = GizmoStyle::default();
        style.opacity = 0.6;
        assert!(gizmo.set_style(&style));
        assert_eq!(gizmo.style(), &style);
        assert!(!gizmo.set_style(&style), "the same style is a no-op");
    }

    #[test]
    fn the_triad_draws_on_top_without_a_ghost_pass() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        for (index, item) in gizmo.items().iter().enumerate() {
            assert_eq!(
                item.surface_depth(),
                crate::shape::OVERLAY_DEPTH,
                "handle {index} is a control overlay"
            );
            assert_eq!(item.wire_depth(), crate::shape::OVERLAY_DEPTH);
            assert!(!item.ghost_surface(), "no occluded pass may dim the triad");
        }
        for (index, item) in gizmo.halos().iter().enumerate() {
            assert_eq!(item.surface_depth(), crate::shape::OVERLAY_DEPTH);
            assert!(!item.ghost_surface(), "halo {index} stays on top");
        }

        // `ghost_alpha` no longer touches the triad: it is reserved for the workbench
        // manipulators, which are deliberately occluded by other bodies.
        let mut style = GizmoStyle::default();
        style.ghost_alpha = 0.5;
        assert!(gizmo.set_style(&style));
        for item in gizmo.items() {
            assert!(!item.ghost_surface());
        }
    }

    #[test]
    fn a_colour_only_style_leaves_the_hit_and_the_drag_unchanged() {
        let camera = camera(5.0);
        let mut plain = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        let mut styled = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Translate);
        let mut style = GizmoStyle::default();
        style.active = [0.1, 0.2, 0.3, 0.4];
        style.inactive = [0.5, 0.6, 0.7, 0.8];
        style.opacity = 0.5;
        assert!(styled.set_style(&style));
        styled.update(&camera);

        assert_eq!(styled.hit(&camera, [80, 50]), plain.hit(&camera, [80, 50]));
        assert_eq!(styled.hit(&camera, [92, 92]), plain.hit(&camera, [92, 92]));

        assert!(plain.begin_drag(GizmoHandle::AxisX, &camera, CENTRE));
        let plain_event = plain.drag_to(&camera, RIGHT).expect("plain drag");
        let plain_commit = plain.end_drag().expect("plain commit");

        assert!(styled.begin_drag(GizmoHandle::AxisX, &camera, CENTRE));
        let styled_event = styled.drag_to(&camera, RIGHT).expect("styled drag");
        let styled_commit = styled.end_drag().expect("styled commit");

        assert_eq!(styled_event.value, plain_event.value);
        assert_eq!(styled_event.constraint, plain_event.constraint);
        assert_eq!(styled_commit.constraint, plain_commit.constraint);
        assert_eq!(styled_commit.total_transform, plain_commit.total_transform);
    }

    fn skin_program() -> Arc<ShaderProgram> {
        Arc::new(ShaderProgram::from_source(
            "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }",
        ))
    }

    #[test]
    fn skinned_items_clone_every_handle_with_the_program() {
        let mut gizmo = TransformGizmo::new(1, Vector3::zero(), GizmoMode::Universal);
        gizmo.update(&camera(5.0));
        let program = skin_program();
        let skinned = gizmo.skinned_items(&program);

        assert_eq!(skinned.len(), gizmo.items().len(), "one item per handle");
        for (index, item) in skinned.iter().enumerate() {
            let original = &gizmo.items()[index];
            assert_eq!(item.entity_id(), original.entity_id(), "handle {index}");
            assert_eq!(item.generation(), original.generation());
            let carried = item
                .program()
                .expect("every skinned handle carries the program");
            assert_eq!(carried.hash(), program.hash());
        }
        assert!(
            gizmo.items().iter().all(|item| item.program().is_none()),
            "the native handles stay unskinned"
        );
    }
}

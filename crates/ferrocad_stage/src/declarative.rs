//! A **declarative** manipulator: a handle described entirely by data.
//!
//! The native [`TransformGizmo`](crate::TransformGizmo) and the workbench handles
//! (`ferrocad_toy`'s height grip, `ferrocad_part_gui`'s dress-up arc) are Rust types.
//! A mod written in Python cannot add one of those, so this module hosts the generic
//! shape of such a handle:
//!
//! ```text
//! DeclarativeHandle ──▶ DeclarativeManipulator ──▶ InteractionCommit
//!  (id, mesh, axis,          (a Manipulator)         (neutral data)
//!   value, bounds, …)
//! ```
//!
//! Everything a workbench-specific manipulator does imperatively — build screen-
//! invariant world geometry, project the axis, drag and snap, commit neutral data — is
//! derived here from the declaration. The transport that builds a [`DeclarativeHandle`]
//! from JSON lives outside the stage; this crate only consumes the data.
//!
//! [`TransformGizmo`]: crate::TransformGizmo

use std::f64::consts::TAU;
use std::sync::Arc;

use ferrocad_graphics::{Camera, ShaderProgram};
use ferrocad_types::{Matrix4, Vector3};

use crate::gizmo::{GizmoStyle, ManipulatorFrame, orthonormal_basis, ring_angle_with_fallback};
use crate::interaction::{InteractionCommit, Manipulator};
use crate::screen::{
    HIT_SLOP, camera_forward, point_segment_distance, project_device, scale_for, to3,
};
use crate::shape::{DrawMode, ShapeItem};

/// The default handle size, in device pixels.
const DEFAULT_SIZE: f64 = 40.0;
/// The default ring radius, in device pixels.
const DEFAULT_RADIUS: f64 = 66.0;
/// The default number of readout decimals.
const DEFAULT_DECIMALS: usize = 2;
/// The number of segments in a ring's analytic hit polyline.
const RING_SAMPLES: usize = 32;
/// The fraction of the projected ring radius that stays transparent in the middle.
///
/// An edge-on ring projects to a line through the centre, so a plain segment test would
/// make the interior (and everything behind it) unreachable. Excluding the central
/// fraction matches the native gizmo's ring pick.
const RING_INTERIOR_FRACTION: f64 = 0.18;

/// The pick id of a declarative handle's overlay item.
///
/// The item is never selectable, so it only has to stay clear of document entity ids
/// and the gizmo's range. A single high id suffices for any number of handles.
const DECLARATIVE_PICK_ID: u32 = 4_000_000;

/// A handle mesh in unit space.
///
/// The manipulator scales it to `size` device pixels and places it at the handle's
/// current world position. `positions` is required; `normals` (when empty) means the
/// surface is flat-shaded, `triangles` (when empty) means there is no surface, and
/// `edges` (when empty) means there are no edge strokes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeclarativeShape {
    /// The unit-space vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Optional per-vertex normals; empty for flat shading.
    pub normals: Vec<[f32; 3]>,
    /// Optional triangle indices, three per triangle.
    pub triangles: Vec<u32>,
    /// Optional edge-segment indices, two per segment.
    pub edges: Vec<u32>,
}

/// How a declared handle converts a drag into its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeclarativeKind {
    /// Drag along `axis`: the value is world units.
    #[default]
    Axis,
    /// Rotate about `axis`: the value is degrees.
    Ring,
}

/// Everything a mod declares about one handle.
///
/// `size` is the handle's own screen target and is authoritative; [`GizmoStyle::size`]
/// belongs to the native gizmo's triad. The workspace style still supplies the ink
/// tokens (unless [`color`](Self::color) overrides them), the opacity and the ghost
/// alpha.
#[derive(Clone, Debug)]
pub struct DeclarativeHandle {
    /// The non-empty id the commit reports as its manipulator (the registry key).
    pub id: String,
    /// The non-empty operation the commit reports (`"set"`, `"fillet"`).
    pub action: String,
    /// The sub-element the manipulation applies to, when it acts on one.
    pub element: Option<String>,
    /// The world origin and orthonormal basis the handle lives in.
    pub frame: ManipulatorFrame,
    /// The drag axis: `0..=2`, an index into [`frame.basis`](ManipulatorFrame::basis).
    pub axis: usize,
    /// How a drag converts into [`value`](Self::value).
    pub kind: DeclarativeKind,
    /// The drag axis value in world units (axis) or degrees about `axis` (ring).
    pub value: f64,
    /// The lower bound of the dragged value, if any.
    pub min: Option<f64>,
    /// The upper bound of the dragged value, if any.
    pub max: Option<f64>,
    /// The absolute snap step; `None` is continuous.
    pub step: Option<f64>,
    /// The handle's target size, in device pixels.
    pub size: f64,
    /// The ring radius, in device pixels. Ignored by an [`Axis`](DeclarativeKind::Axis)
    /// handle.
    pub radius: f64,
    /// The number of decimals in the readout.
    pub decimals: usize,
    /// The handle mesh, in unit space.
    pub shape: DeclarativeShape,
    /// An optional geometry program for the handle's skin.
    pub program: Option<Arc<ShaderProgram>>,
    /// An optional ink override; `None` uses the workspace style's tokens.
    pub color: Option<[f32; 4]>,
    /// The readout template. `{value}` is replaced by the formatted value; an empty
    /// template is the bare number.
    pub readout: String,
}

impl DeclarativeHandle {
    /// An axis handle at the default size and decimals, with no bounds or snap step.
    pub fn new(
        id: impl Into<String>,
        action: impl Into<String>,
        frame: ManipulatorFrame,
        axis: usize,
        shape: DeclarativeShape,
    ) -> Self {
        Self {
            id: id.into(),
            action: action.into(),
            element: None,
            frame,
            axis,
            kind: DeclarativeKind::Axis,
            value: 0.0,
            min: None,
            max: None,
            step: None,
            size: DEFAULT_SIZE,
            radius: DEFAULT_RADIUS,
            decimals: DEFAULT_DECIMALS,
            shape,
            program: None,
            color: None,
            readout: String::new(),
        }
    }

    /// Declare how a drag converts into the value.
    pub fn with_kind(mut self, kind: DeclarativeKind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the ring radius, in device pixels.
    pub fn with_radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }
}

/// The drag's captured press point, value and (for a ring) press angle.
#[derive(Clone, Copy, Debug)]
struct DragState {
    press: [f64; 2],
    value: f64,
    /// The in-plane press angle, in radians. `None` for an axis handle.
    angle: Option<f64>,
}

/// A mod-supplied handle hosted by the native manipulator model.
///
/// The declaration is fixed at construction; a drag only changes the scalar
/// [`value`](Self::value). World geometry is rebuilt on [`refresh`](Self::refresh)
/// when the camera or the value changes, and the projected axis converts pointer
/// travel into world units.
#[derive(Clone, Debug)]
pub struct DeclarativeManipulator {
    target: u32,
    handle: DeclarativeHandle,
    value: f64,
    item: ShapeItem,
    drag: Option<DragState>,
    hovered: bool,
    style: GizmoStyle,
    /// The camera view and value the cached geometry was built for.
    view: Option<Matrix4>,
    built: Option<f64>,
}

impl DeclarativeManipulator {
    /// A handle for `target` driven by `handle`.
    pub fn new(target: u32, handle: DeclarativeHandle) -> Self {
        let style = GizmoStyle::default();
        let draw_mode = if handle.shape.edges.is_empty() {
            DrawMode::Shaded
        } else {
            DrawMode::ShadedWithEdges
        };
        let mut item = ShapeItem::new(DECLARATIVE_PICK_ID)
            .with_draw_mode(draw_mode)
            .with_selectable(false)
            .with_blended_surface()
            .with_ghost_surface()
            .with_ghost_alpha(style.ghost_alpha);
        item.set_program(handle.program.clone());
        let value = handle.value;
        let mut manipulator = Self {
            target,
            handle,
            value,
            item,
            drag: None,
            hovered: false,
            style,
            view: None,
            built: None,
        };
        manipulator.apply_color();
        manipulator
    }

    /// The entity the handle edits.
    pub fn target_id(&self) -> u32 {
        self.target
    }

    /// The current value, in world units along the drag axis.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The declaration, as supplied by the mod.
    pub fn handle(&self) -> &DeclarativeHandle {
        &self.handle
    }

    /// The registry id: the declaration's owned id, not a literal.
    pub fn id(&self) -> &str {
        &self.handle.id
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether the handle is highlighted.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The overlay item, for the overlay queue.
    pub fn items(&self) -> &[ShapeItem] {
        std::slice::from_ref(&self.item)
    }

    /// Highlight (or un-highlight) the handle. Returns whether the visuals changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.apply_color();
        true
    }

    /// The resolved ink: an optional handle colour, otherwise the style's idle or
    /// hovered token, with the style opacity applied to the alpha.
    fn ink(&self) -> [f32; 4] {
        let token = self.handle.color.unwrap_or_else(|| {
            if self.hovered || self.is_dragging() {
                self.style.active
            } else {
                self.style.inactive
            }
        });
        [
            token[0],
            token[1],
            token[2],
            (token[3] * self.style.opacity).clamp(0.0, 1.0),
        ]
    }

    /// Recolour the cached item without rebuilding its geometry.
    fn apply_color(&mut self) {
        let ink = self.ink();
        self.item.set_surface_color(ink);
        self.item.set_edge_color(ink);
        self.item.set_ghost_alpha(self.style.ghost_alpha);
    }

    /// Apply the workspace gizmo style. Returns whether the looked-up ink or alpha
    /// changed.
    ///
    /// The handle's own [`size`](DeclarativeHandle::size) is authoritative, so the
    /// style's `size` (the native triad's) never rebuilds this geometry. `enabled` and
    /// `show_readout` are host policies, gated by the viewport, and are ignored here.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        if self.style == *style {
            return false;
        }
        let before = (self.ink(), self.style.ghost_alpha);
        self.style = style.clone();
        let after = (self.ink(), self.style.ghost_alpha);
        // No current style token changes the mesh, but the contract says a style change
        // refreshes the geometry; keep that true for a future geometry-bearing token.
        self.built = None;
        self.apply_color();
        before != after
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        &self.style
    }

    /// The world direction of the drag axis.
    fn axis_world(&self) -> Vector3 {
        self.handle.frame.basis[self.handle.axis.min(2)]
    }

    /// The world position the handle's mesh rides on.
    ///
    /// An axis handle sits at the frame origin plus the axis value. A ring handle sits
    /// on the ring at the value's angle, so grabbing it follows the arc it turns.
    fn origin(&self, camera: &Camera) -> Vector3 {
        match self.handle.kind {
            DeclarativeKind::Axis => self
                .handle
                .frame
                .origin
                .add(&self.axis_world().scale(self.value)),
            DeclarativeKind::Ring => self.ring_point(camera, self.value),
        }
    }

    /// The point on the ring at `degrees` about the drag axis.
    ///
    /// The basis is [`orthonormal_basis`], the same reference the ring drag measures
    /// its angle against, so a rendered point at angle `θ` and a dragged angle `θ` agree.
    fn ring_point(&self, camera: &Camera, degrees: f64) -> Vector3 {
        let origin = self.handle.frame.origin;
        let radius = scale_for(camera, origin, self.handle.radius);
        let (u, v) = orthonormal_basis(self.axis_world());
        let angle = degrees.to_radians();
        origin
            .add(&u.scale(radius * angle.cos()))
            .add(&v.scale(radius * angle.sin()))
    }

    /// Rebuild the geometry for `camera` when the view or the value changed. Returns
    /// whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        if self.view == Some(camera.view) && self.built == Some(self.value) {
            return false;
        }
        self.view = Some(camera.view);
        self.built = Some(self.value);
        self.build(camera);
        true
    }

    /// Bake the unit mesh into world space at the handle's current position.
    ///
    /// `world = position + world_per_px * size * (x·X + y·Y + z·Z)`, so a one-unit mesh
    /// spans `size` device pixels and the handle keeps its screen size at any zoom. An
    /// axis handle's position is `origin + axis * value`; a ring handle's position is the
    /// ring point at the value's angle.
    fn build(&mut self, camera: &Camera) {
        let frame = self.handle.frame;
        let scale = scale_for(camera, frame.origin, 1.0) * self.handle.size;
        let base = self.origin(camera);
        let positions: Vec<[f32; 3]> = self
            .handle
            .shape
            .positions
            .iter()
            .map(|point| to3(base.add(&basis_vector(&frame.basis, *point).scale(scale))))
            .collect();
        let normals: Vec<[f32; 3]> = self
            .handle
            .shape
            .normals
            .iter()
            .map(|normal| to3(normalised(basis_vector(&frame.basis, *normal))))
            .collect();
        self.item.set_shape(
            positions,
            normals,
            self.handle.shape.triangles.clone(),
            self.handle.shape.edges.clone(),
        );
        self.apply_color();
    }

    /// Whether `device` is on the handle.
    ///
    /// The test is analytic against the declaration, then padded by [`HIT_SLOP`]:
    ///
    /// * **Axis** accepts a pointer within [`HIT_SLOP`] of the projected stem from the
    ///   frame origin to the tip (`origin + axis * value`), **and** anywhere in the built
    ///   mesh's projected bounding box. The two regions are distinct on purpose: the stem
    ///   is the natural grab line, while the bbox keeps a wide or oddly shaped mesh
    ///   grabbable even where the thin stem does not pass.
    /// * **Ring** accepts a pointer within [`HIT_SLOP`] of the projected ring polyline
    ///   (`RING_SAMPLES` segments). A ring seen edge-on projects to a line, and the same
    ///   segment distance covers it. The central fraction of the projected ring is
    ///   excluded so the interior stays free to the scene behind it.
    ///
    /// The built geometry is cached, so the host calls [`refresh`](Self::refresh) before
    /// hit-testing (the viewport already does); a ring's analytic region does not need it.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        match self.handle.kind {
            DeclarativeKind::Axis => self.axis_hit(camera, device),
            DeclarativeKind::Ring => self.ring_hit(camera, device),
        }
    }

    /// The axis hull: the projected stem plus the built mesh's projected bbox.
    fn axis_hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let origin = self.handle.frame.origin;
        let tip = origin.add(&self.axis_world().scale(self.value));
        let (Some(a), Some(b)) = (project_device(camera, origin), project_device(camera, tip)) else {
            return self.mesh_hit(camera, device);
        };
        if point_segment_distance(pointer, a, b).0 <= HIT_SLOP {
            return true;
        }
        self.mesh_hit(camera, device)
    }

    /// The ring hull: a [`HIT_SLOP`]-padded band around the projected ring polyline,
    /// with its interior excluded.
    fn ring_hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let origin = self.handle.frame.origin;
        let radius = scale_for(camera, origin, self.handle.radius);
        if !radius.is_finite() || radius <= 0.0 {
            return false;
        }
        let (u, v) = orthonormal_basis(self.axis_world());
        let mut projected = Vec::with_capacity(RING_SAMPLES);
        for i in 0..RING_SAMPLES {
            let angle = TAU * i as f64 / RING_SAMPLES as f64;
            let point = origin
                .add(&u.scale(radius * angle.cos()))
                .add(&v.scale(radius * angle.sin()));
            projected.push(project_device(camera, point));
        }
        let Some(centre) = project_device(camera, origin) else {
            return false;
        };
        let mut nearest = f64::INFINITY;
        let mut projected_radius = 0.0_f64;
        for (index, point) in projected.iter().enumerate() {
            let Some(point) = point else { continue };
            projected_radius =
                projected_radius.max((point[0] - centre[0]).hypot(point[1] - centre[1]));
            let next = projected[(index + 1) % RING_SAMPLES];
            if let Some(next) = next {
                nearest = nearest.min(point_segment_distance(pointer, *point, next).0);
            }
        }
        if !projected_radius.is_finite() || projected_radius <= 0.0 {
            return false;
        }
        let pointer_radius = (pointer[0] - centre[0]).hypot(pointer[1] - centre[1]);
        if pointer_radius < RING_INTERIOR_FRACTION * projected_radius {
            return false;
        }
        nearest <= HIT_SLOP
    }

    /// The built mesh's projected bounding box, padded by [`HIT_SLOP`].
    fn mesh_hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some((min, max)) = bounds(self.item.positions()) else {
            return false;
        };
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        let mut projected = false;
        for corner in box_corners(min, max) {
            let Some(point) = project_device(camera, corner) else {
                continue;
            };
            projected = true;
            low[0] = low[0].min(point[0]);
            low[1] = low[1].min(point[1]);
            high[0] = high[0].max(point[0]);
            high[1] = high[1].max(point[1]);
        }
        if !projected {
            return false;
        }
        let point = [f64::from(device[0]), f64::from(device[1])];
        point[0] >= low[0] - HIT_SLOP
            && point[0] <= high[0] + HIT_SLOP
            && point[1] >= low[1] - HIT_SLOP
            && point[1] <= high[1] + HIT_SLOP
    }

    /// Begin a drag at `device`, if it hits. Records the press point, value and (for a
    /// ring) the press angle so the value only moves by the pointer's travel from there.
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        if !self.hit(camera, device) {
            return false;
        }
        let angle = match self.handle.kind {
            DeclarativeKind::Axis => None,
            DeclarativeKind::Ring => {
                let Some(ray) = camera.ray_through(device) else {
                    return false;
                };
                let Some(angle) = ring_angle_with_fallback(
                    &ray,
                    self.handle.frame.origin,
                    self.axis_world(),
                    camera_forward(camera),
                ) else {
                    return false;
                };
                Some(angle)
            }
        };
        self.drag = Some(DragState {
            press: [f64::from(device[0]), f64::from(device[1])],
            value: self.value,
            angle,
        });
        self.apply_color();
        true
    }

    /// Advance the drag. Returns whether the value moved.
    ///
    /// **Axis:** one world unit projects to `pixels_per_world` device pixels; the
    /// pointer's travel along that screen direction divided by it is the value delta. A
    /// degenerate projection (the axis points at the camera) changes nothing.
    ///
    /// **Ring:** the pointer ray is intersected with the ring plane (falling back to a
    /// view-facing plane when the ring is edge-on) and the angle about the axis is
    /// measured against the same basis the ring is drawn with. The value in degrees is
    /// the press value plus the angle delta, then snapped and clamped.
    ///
    /// `ring_detent` is not reused: it applies a *relative*
    /// detent driven by a Ctrl modifier the [`Manipulator`] trait does not carry, while a
    /// declaration's `step` snaps the absolute value. `wrap_pi` is not applied either:
    /// it would fold a continuous scalar past ±180°, and a mod's degree value is not a
    /// rotation matrix that is equivalent modulo a full turn.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let value = match self.handle.kind {
            DeclarativeKind::Axis => {
                let origin = self.handle.frame.origin;
                let (Some(from), Some(to)) = (
                    project_device(camera, origin),
                    project_device(camera, origin.add(&self.axis_world())),
                ) else {
                    return false;
                };
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                let pixels_per_world = dx.hypot(dy);
                if pixels_per_world <= 1e-9 {
                    return false;
                }
                let direction = [dx / pixels_per_world, dy / pixels_per_world];
                let travel = (f64::from(device[0]) - drag.press[0]) * direction[0]
                    + (f64::from(device[1]) - drag.press[1]) * direction[1];
                self.clamp(self.snap(drag.value + travel / pixels_per_world))
            }
            DeclarativeKind::Ring => {
                let Some(press_angle) = drag.angle else {
                    return false;
                };
                let Some(ray) = camera.ray_through(device) else {
                    return false;
                };
                let Some(angle) = ring_angle_with_fallback(
                    &ray,
                    self.handle.frame.origin,
                    self.axis_world(),
                    camera_forward(camera),
                ) else {
                    return false;
                };
                let delta = (angle - press_angle).to_degrees();
                self.clamp(self.snap(drag.value + delta))
            }
        };
        if (value - self.value).abs() <= 1e-12 {
            return false;
        }
        self.value = value;
        true
    }

    /// End the drag, returning the value to commit.
    pub fn end_drag(&mut self) -> Option<InteractionCommit> {
        self.drag.take()?;
        self.apply_color();
        Some(self.commit())
    }

    /// Cancel the drag, restoring the press value. Returns whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        self.value = drag.value;
        self.apply_color();
        true
    }

    /// The neutral commit for the current value. A ring commits `[degrees]`; an axis
    /// commits `[world units]`.
    fn commit(&self) -> InteractionCommit {
        let mut commit = InteractionCommit::new(
            self.handle.id.clone(),
            self.target,
            self.handle.action.clone(),
        )
        .with_values([self.value])
        .with_readout(self.readout());
        if let Some(element) = &self.handle.element {
            commit = commit.with_element(element.clone());
        }
        commit
    }

    /// The formatted value at the declaration's decimals. A ring's value is already in
    /// degrees, so the same formatting applies.
    fn format_value(&self) -> String {
        format!("{:.*}", self.handle.decimals, self.value)
    }

    /// The readout: the template with `{value}` replaced, or the bare number.
    pub fn readout(&self) -> String {
        if self.handle.readout.is_empty() {
            self.format_value()
        } else {
            self.handle
                .readout
                .replace("{value}", &self.format_value())
        }
    }

    /// Where the readout sits, in device pixels: the handle's current world position
    /// (the ring point, for a ring).
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.origin(camera))
    }

    fn snap(&self, value: f64) -> f64 {
        match self.handle.step {
            Some(step) if step > 0.0 => (value / step).round() * step,
            _ => value,
        }
    }

    fn clamp(&self, value: f64) -> f64 {
        let mut value = if value.is_finite() { value } else { self.value };
        if let Some(min) = self.handle.min {
            value = value.max(min);
        }
        if let Some(max) = self.handle.max {
            value = value.min(max);
            // A declared `min > max` is a mod bug; the lower bound wins so the handle
            // still commits the value it displays.
            if let Some(min) = self.handle.min {
                value = value.max(min);
            }
        }
        value
    }
}

impl Manipulator for DeclarativeManipulator {
    fn id(&self) -> &str {
        DeclarativeManipulator::id(self)
    }

    fn items(&self) -> &[ShapeItem] {
        DeclarativeManipulator::items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        DeclarativeManipulator::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        DeclarativeManipulator::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        DeclarativeManipulator::drag_to(self, camera, device)
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        DeclarativeManipulator::end_drag(self)
    }

    fn drag_cancel(&mut self) -> bool {
        DeclarativeManipulator::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        DeclarativeManipulator::is_dragging(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        DeclarativeManipulator::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        DeclarativeManipulator::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn readout(&self) -> Option<String> {
        Some(DeclarativeManipulator::readout(self))
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        DeclarativeManipulator::readout_anchor(self, camera)
    }
}

/// The world-space direction of a unit-space vector in `basis`.
fn basis_vector(basis: &[Vector3; 3], vector: [f32; 3]) -> Vector3 {
    basis[0]
        .scale(f64::from(vector[0]))
        .add(&basis[1].scale(f64::from(vector[1])))
        .add(&basis[2].scale(f64::from(vector[2])))
}

/// Normalise a direction, leaving a zero vector untouched.
fn normalised(vector: Vector3) -> Vector3 {
    let length = vector.length();
    if length > 1e-12 {
        vector.scale(1.0 / length)
    } else {
        vector
    }
}

/// The world-space bounds `(min, max)` of a position buffer.
fn bounds(positions: &[[f32; 3]]) -> Option<(Vector3, Vector3)> {
    let mut iter = positions.iter();
    let first = iter.next()?;
    let mut min = Vector3::new(
        f64::from(first[0]),
        f64::from(first[1]),
        f64::from(first[2]),
    );
    let mut max = min;
    for [x, y, z] in iter {
        let point = Vector3::new(f64::from(*x), f64::from(*y), f64::from(*z));
        min = Vector3::new(min.x.min(point.x), min.y.min(point.y), min.z.min(point.z));
        max = Vector3::new(max.x.max(point.x), max.y.max(point.y), max.z.max(point.z));
    }
    Some((min, max))
}

/// The eight corners of an axis-aligned box.
fn box_corners(min: Vector3, max: Vector3) -> [Vector3; 8] {
    [
        Vector3::new(min.x, min.y, min.z),
        Vector3::new(max.x, min.y, min.z),
        Vector3::new(min.x, max.y, min.z),
        Vector3::new(max.x, max.y, min.z),
        Vector3::new(min.x, min.y, max.z),
        Vector3::new(max.x, min.y, max.z),
        Vector3::new(min.x, max.y, max.z),
        Vector3::new(max.x, max.y, max.z),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_types::{Placement, Rotation};

    /// A 45° perspective camera at `(0, 0, eye_z)` looking toward the origin.
    fn camera(eye_z: f64) -> Camera {
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 1000.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        }
    }

    /// A unit cube centred on the unit origin, with all twelve edges.
    fn unit_cube() -> DeclarativeShape {
        let positions = vec![
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [-0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
        ];
        let triangles = vec![
            0, 2, 1, 1, 2, 3, // z = -0.5
            4, 5, 6, 5, 7, 6, // z = +0.5
            0, 1, 4, 1, 5, 4, // y = -0.5
            2, 6, 3, 3, 6, 7, // y = +0.5
            0, 4, 2, 2, 4, 6, // x = -0.5
            1, 3, 5, 5, 3, 7, // x = +0.5
        ];
        let edges = vec![0, 1, 1, 3, 3, 2, 2, 0, 4, 5, 5, 7, 7, 6, 6, 4, 0, 4, 1, 5, 2, 6, 3, 7];
        DeclarativeShape {
            positions,
            normals: Vec::new(),
            triangles,
            edges,
        }
    }

    /// A unit triangle with one distinct point at `(1, 0, 0)` and no edges.
    fn marker() -> DeclarativeShape {
        DeclarativeShape {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            edges: Vec::new(),
        }
    }

    fn handle(id: &str, axis: usize) -> DeclarativeHandle {
        DeclarativeHandle::new(
            id,
            "set",
            ManipulatorFrame::world(Vector3::zero()),
            axis,
            unit_cube(),
        )
    }

    fn manipulator(id: &str, axis: usize) -> DeclarativeManipulator {
        DeclarativeManipulator::new(7, handle(id, axis))
    }

    /// The device position of the world origin, rounded to a click point.
    fn centre(camera: &Camera) -> [i32; 2] {
        let point = project_device(camera, Vector3::zero()).expect("the origin projects");
        [point[0].round() as i32, point[1].round() as i32]
    }

    /// Device pixels spanned by one world unit along +X.
    fn pixels_per_world(camera: &Camera) -> f64 {
        let from = project_device(camera, Vector3::zero()).expect("the origin projects");
        let to = project_device(camera, Vector3::new(1.0, 0.0, 0.0)).expect("the unit projects");
        (to[0] - from[0]).hypot(to[1] - from[1])
    }

    /// The resolved ink for a token and opacity.
    fn tint(token: [f32; 4], opacity: f32) -> [f32; 4] {
        [
            token[0],
            token[1],
            token[2],
            (token[3] * opacity).clamp(0.0, 1.0),
        ]
    }

    /// A ring handle about `axis` with a 25 px radius, small enough for the test viewport.
    fn ring_handle(id: &str, axis: usize) -> DeclarativeHandle {
        let mut handle = handle(id, axis);
        handle.kind = DeclarativeKind::Ring;
        handle.radius = 25.0;
        handle.shape = marker();
        handle
    }

    /// A device point at screen angle `degrees` on a circle around the projected origin.
    ///
    /// For the face-on test camera, screen angle and world ring angle differ by a fixed
    /// quarter turn, so a screen sweep of Δ degrees is a value change of Δ degrees.
    fn on_screen_ring(camera: &Camera, radius_px: f64, degrees: f64) -> [i32; 2] {
        let centre = project_device(camera, Vector3::zero()).expect("the origin projects");
        let angle = degrees.to_radians();
        [
            (centre[0] + radius_px * angle.cos()).round() as i32,
            (centre[1] + radius_px * angle.sin()).round() as i32,
        ]
    }

    /// A world point on the ring about `axis` at `degrees`, using the drawn basis.
    fn world_ring_point(camera: &Camera, axis: usize, radius_px: f64, degrees: f64) -> Vector3 {
        let frame = ManipulatorFrame::world(Vector3::zero());
        let radius = scale_for(camera, frame.origin, radius_px);
        let (u, v) = orthonormal_basis(frame.basis[axis]);
        let angle = degrees.to_radians();
        u.scale(radius * angle.cos())
            .add(&v.scale(radius * angle.sin()))
    }

    #[test]
    fn construction_defaults_match_the_contract() {
        let frame = ManipulatorFrame::world(Vector3::new(1.0, 2.0, 3.0));
        let shape = unit_cube();
        let handle = DeclarativeHandle::new("handle", "act", frame, 0, shape.clone());

        assert_eq!(handle.id, "handle");
        assert_eq!(handle.action, "act");
        assert_eq!(handle.frame, frame);
        assert_eq!(handle.shape, shape);
        assert_eq!(handle.axis, 0);
        assert_eq!(handle.value, 0.0);
        assert_eq!(handle.size, DEFAULT_SIZE);
        assert_eq!(handle.decimals, DEFAULT_DECIMALS);
        assert_eq!(handle.step, None);
        assert_eq!(handle.min, None);
        assert_eq!(handle.max, None);
        assert!(handle.element.is_none());
        assert!(handle.program.is_none());
        assert!(handle.color.is_none());
        assert!(handle.readout.is_empty());
    }

    #[test]
    fn refresh_bakes_the_unit_mesh_into_world_space() {
        let camera = camera(100.0);
        let frame = ManipulatorFrame::world(Vector3::zero());
        let mut handle = DeclarativeHandle::new("height", "set", frame, 2, marker());
        handle.value = 10.0;
        handle.size = 40.0;
        let mut manipulator = DeclarativeManipulator::new(1, handle);

        assert!(manipulator.update(&camera), "the first refresh builds");
        assert!(!manipulator.update(&camera), "an unchanged view is cached");

        let item = &manipulator.items()[0];
        assert!(matches!(item.draw_mode(), DrawMode::Shaded));
        let scale = scale_for(&camera, frame.origin, 40.0);
        // The unit point (1, 0, 0) rides 10 world units up +Z and is scaled to 40 px.
        let point = item.positions()[1];
        assert!((f64::from(point[0]) - scale).abs() < 1e-4);
        assert!(f64::from(point[1]).abs() < 1e-4);
        assert!((f64::from(point[2]) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn a_shape_with_edges_is_shaded_with_edges() {
        let camera = camera(100.0);
        let mut manipulator = manipulator("edged", 0);
        assert!(manipulator.update(&camera));
        assert!(matches!(
            manipulator.items()[0].draw_mode(),
            DrawMode::ShadedWithEdges
        ));
    }

    #[test]
    fn the_projected_handle_centre_hits_and_far_away_misses() {
        let camera = camera(100.0);
        let mut manipulator = manipulator("hit", 0);
        assert!(manipulator.update(&camera));

        let centre = centre(&camera);
        assert!(manipulator.hit(&camera, centre));
        assert!(!manipulator.hit(&camera, [centre[0] + 60, centre[1]]));
        assert!(!manipulator.hit(&camera, [centre[0], centre[1] + 60]));
    }

    #[test]
    fn a_pointer_move_along_the_projected_axis_moves_the_value() {
        let camera = camera(100.0);
        let mut manipulator = manipulator("axis", 0);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        let target = [press[0] + 25, press[1]];
        assert!(manipulator.drag_to(&camera, target));

        let travel = f64::from(target[0] - press[0]);
        let expected = travel / pixels_per_world(&camera);
        assert!(
            (manipulator.value() - expected).abs() < 1e-9,
            "got {}, expected {expected}",
            manipulator.value()
        );
    }

    #[test]
    fn a_step_snaps_the_value() {
        let camera = camera(100.0);
        let mut handle = handle("stepped", 0);
        handle.step = Some(0.5);
        let mut manipulator = DeclarativeManipulator::new(7, handle);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        assert!(manipulator.drag_to(&camera, [press[0] + 25, press[1]]));

        let raw = 25.0 / pixels_per_world(&camera);
        let snapped = (raw / 0.5).round() * 0.5;
        assert_eq!(manipulator.value(), snapped);
    }

    #[test]
    fn bounds_clamp_the_value() {
        let camera = camera(100.0);
        let mut handle = handle("bounded", 0);
        handle.min = Some(-1.0);
        handle.max = Some(1.0);
        let mut manipulator = DeclarativeManipulator::new(7, handle);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        assert!(manipulator.drag_to(&camera, [press[0] + 25, press[1]]));
        assert_eq!(manipulator.value(), 1.0);

        assert!(manipulator.drag_to(&camera, [press[0] - 100, press[1]]));
        assert_eq!(manipulator.value(), -1.0);
    }

    #[test]
    fn an_axis_pointing_at_the_camera_does_not_move() {
        let camera = camera(100.0);
        let mut manipulator = manipulator("facing", 2);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        assert!(!manipulator.drag_to(&camera, [press[0] + 40, press[1] + 40]));
        assert_eq!(manipulator.value(), 0.0);
    }

    #[test]
    fn cancelling_restores_the_press_value() {
        let camera = camera(100.0);
        let mut manipulator = manipulator("cancel", 0);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        assert!(manipulator.drag_to(&camera, [press[0] + 25, press[1]]));
        assert!(manipulator.value() > 0.0);
        assert!(manipulator.cancel_drag());
        assert_eq!(manipulator.value(), 0.0);
    }

    #[test]
    fn the_commit_carries_the_owned_id_action_element_and_value() {
        let camera = camera(100.0);
        let id = format!("mod_fillet_{}", 9);
        let mut handle = handle("ignored", 0);
        handle.id = id.clone();
        handle.action = "fillet".to_string();
        handle.element = Some("Edge9".to_string());
        handle.value = 2.5;
        handle.readout = "R {value} mm".to_string();
        let mut manipulator = DeclarativeManipulator::new(11, handle);
        assert!(manipulator.update(&camera));

        let press = centre(&camera);
        assert!(manipulator.begin_drag(&camera, press));
        let commit = manipulator.end_drag().expect("a commit");

        assert_eq!(commit.manipulator, id);
        assert_eq!(commit.target, 11);
        assert_eq!(commit.action, "fillet");
        assert_eq!(commit.element.as_deref(), Some("Edge9"));
        assert_eq!(commit.values, vec![2.5]);
        assert_eq!(commit.readout, "R 2.50 mm");
    }

    #[test]
    fn the_readout_template_formats_the_value() {
        let mut template = handle("template", 0);
        template.value = 2.5;
        template.readout = "R {value} mm".to_string();
        let manipulator = DeclarativeManipulator::new(1, template);
        assert_eq!(manipulator.readout(), "R 2.50 mm");

        let mut bare = handle("bare", 0);
        bare.value = 3.0;
        bare.decimals = 1;
        let manipulator = DeclarativeManipulator::new(1, bare);
        assert_eq!(manipulator.readout(), "3.0");
    }

    #[test]
    fn set_style_recolours_and_is_idempotent() {
        let mut manipulator = manipulator("styled", 0);
        let base = GizmoStyle::default();
        assert_eq!(
            manipulator.items()[0].surface_color(),
            tint(base.inactive, base.opacity)
        );

        let mut style = base.clone();
        style.inactive = [0.1, 0.2, 0.3, 1.0];
        style.active = [0.9, 0.8, 0.7, 1.0];
        style.opacity = 0.5;
        assert!(manipulator.set_style(&style));
        assert_eq!(
            manipulator.items()[0].surface_color(),
            tint(style.inactive, style.opacity)
        );
        assert!(!manipulator.set_style(&style.clone()), "an equal style is inert");

        // Only the resolved ink/alpha matters; the triad's size does not recolour.
        let mut geometry_only = style.clone();
        geometry_only.size += 10.0;
        assert!(!manipulator.set_style(&geometry_only));

        assert!(manipulator.set_hovered(true));
        assert_eq!(
            manipulator.items()[0].surface_color(),
            tint(style.active, style.opacity)
        );
    }

    #[test]
    fn a_handle_colour_overrides_the_style_tokens() {
        let mut handle = handle("coloured", 0);
        handle.color = Some([1.0, 0.0, 0.0, 1.0]);
        let mut manipulator = DeclarativeManipulator::new(1, handle);
        assert_eq!(manipulator.items()[0].surface_color(), [1.0, 0.0, 0.0, 1.0]);

        assert!(manipulator.set_hovered(true));
        assert_eq!(manipulator.items()[0].surface_color(), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn the_id_is_the_owned_handle_id() {
        let id = format!("mod_handle_{}", 42);
        let mut handle = handle("placeholder", 0);
        handle.id = id.clone();
        let manipulator = DeclarativeManipulator::new(1, handle);
        assert_eq!(manipulator.id(), id.as_str());
        assert_eq!(Manipulator::id(&manipulator), id.as_str());
    }

    #[test]
    fn a_new_handle_is_an_axis_at_the_default_radius() {
        let handle = handle("plain", 0);
        assert_eq!(handle.kind, DeclarativeKind::Axis);
        assert_eq!(handle.radius, DEFAULT_RADIUS);
        assert_eq!(DEFAULT_RADIUS, 66.0);

        let ring = handle
            .with_kind(DeclarativeKind::Ring)
            .with_radius(80.0);
        assert_eq!(ring.kind, DeclarativeKind::Ring);
        assert_eq!(ring.radius, 80.0);
    }

    #[test]
    fn a_point_on_the_projected_ring_hits_but_its_interior_does_not() {
        let camera = camera(100.0);
        let mut manipulator = DeclarativeManipulator::new(7, ring_handle("ring", 2));
        assert!(manipulator.update(&camera));

        // About +Z, face-on: the screen top is a point on the ring.
        assert!(
            manipulator.hit(&camera, on_screen_ring(&camera, 25.0, 90.0)),
            "the arc is grabbable"
        );

        let centre = centre(&camera);
        assert!(!manipulator.hit(&camera, centre), "the interior passes through");
        assert!(!manipulator.hit(&camera, [centre[0] + 500, centre[1] + 500]));
    }

    #[test]
    fn an_edge_on_ring_still_hits_near_its_projected_line() {
        let camera = camera(100.0);
        let mut manipulator = DeclarativeManipulator::new(7, ring_handle("edge", 0));
        assert!(manipulator.update(&camera));

        // About +X, seen down −Z: the ring projects to a vertical line through the
        // centre, and world angle 90° lies on −Y (the bottom of that line).
        assert!(
            manipulator.hit(&camera, on_screen_ring(&camera, 25.0, -90.0)),
            "the projected end is grabbable"
        );

        let centre = centre(&camera);
        let near = [centre[0] + HIT_SLOP as i32 - 2, centre[1]];
        let far = [centre[0] + HIT_SLOP as i32 + 8, centre[1]];
        assert!(manipulator.hit(&camera, near), "the projected line is grabbable");
        assert!(!manipulator.hit(&camera, far), "past the slop misses");
        // The projected centre sits on that line but stays transparent.
        assert!(!manipulator.hit(&camera, centre));
    }

    #[test]
    fn an_axis_hits_on_its_stem_and_misses_past_the_slop() {
        let camera = camera(100.0);
        let mut handle = handle("stem", 0);
        handle.shape = marker();
        handle.size = 1.0; // a tiny tip mesh leaves the stem exposed
        handle.value = 50.0;
        let mut manipulator = DeclarativeManipulator::new(7, handle);
        assert!(manipulator.update(&camera));

        let middle =
            project_device(&camera, Vector3::new(25.0, 0.0, 0.0)).expect("the stem projects");
        let press = [middle[0].round() as i32, middle[1].round() as i32];
        assert!(manipulator.hit(&camera, press), "the stem is grabbable");

        let off = [press[0], press[1] + HIT_SLOP as i32 + 8];
        assert!(!manipulator.hit(&camera, off), "past the slop misses");
    }

    #[test]
    fn a_wide_mesh_hits_via_its_projected_bbox() {
        let camera = camera(100.0);
        let mut handle = handle("wide", 0);
        handle.shape = marker();
        handle.size = 40.0;
        handle.value = 0.0;
        let mut manipulator = DeclarativeManipulator::new(7, handle);
        assert!(manipulator.update(&camera));

        let scale = scale_for(&camera, Vector3::zero(), 40.0);
        let corner = project_device(&camera, Vector3::new(scale, scale, 0.0))
            .expect("the mesh corner projects");
        let press = [corner[0].round() as i32, corner[1].round() as i32];

        let origin = project_device(&camera, Vector3::zero()).expect("the origin projects");
        let pointer = [f64::from(press[0]), f64::from(press[1])];
        let (distance, _) = point_segment_distance(pointer, origin, origin);
        assert!(distance > HIT_SLOP, "the stem does not reach the corner");
        assert!(manipulator.hit(&camera, press), "the bbox is grabbable");
    }

    #[test]
    fn a_ring_drag_turns_screen_arc_into_degrees() {
        let camera = camera(100.0);
        let mut manipulator = DeclarativeManipulator::new(7, ring_handle("turned", 2));
        assert!(manipulator.update(&camera));

        assert!(manipulator.begin_drag(&camera, on_screen_ring(&camera, 25.0, 0.0)));
        assert!(manipulator.drag_to(&camera, on_screen_ring(&camera, 25.0, 30.0)));

        assert!(
            (manipulator.value() - 30.0).abs() < 2.0,
            "got {} degrees",
            manipulator.value()
        );
    }

    #[test]
    fn a_ring_step_snaps_and_the_bounds_clamp() {
        let camera = camera(100.0);

        let mut stepped = ring_handle("stepped", 2);
        stepped.step = Some(15.0);
        let mut manipulator = DeclarativeManipulator::new(7, stepped);
        assert!(manipulator.update(&camera));
        assert!(manipulator.begin_drag(&camera, on_screen_ring(&camera, 25.0, 0.0)));
        assert!(manipulator.drag_to(&camera, on_screen_ring(&camera, 25.0, 28.0)));
        assert_eq!(manipulator.value(), 30.0, "28 degrees snaps to 30");

        let mut bounded = ring_handle("bounded", 2);
        bounded.min = Some(10.0);
        bounded.max = Some(20.0);
        let mut manipulator = DeclarativeManipulator::new(7, bounded);
        assert!(manipulator.update(&camera));
        assert!(manipulator.begin_drag(&camera, on_screen_ring(&camera, 25.0, 0.0)));
        assert!(manipulator.drag_to(&camera, on_screen_ring(&camera, 25.0, 30.0)));
        assert_eq!(manipulator.value(), 20.0, "the upper bound wins");
        assert!(manipulator.drag_to(&camera, on_screen_ring(&camera, 25.0, -30.0)));
        assert_eq!(manipulator.value(), 10.0, "the lower bound wins");
    }

    #[test]
    fn ring_geometry_sits_on_the_ring_at_the_value() {
        let camera = camera(100.0);
        let mut handle = ring_handle("geometry", 2);
        handle.value = 90.0;
        let mut manipulator = DeclarativeManipulator::new(7, handle);
        assert!(manipulator.update(&camera));

        let radius = scale_for(&camera, Vector3::zero(), 25.0);
        let expected = world_ring_point(&camera, 2, 25.0, 90.0);
        let point = manipulator.items()[0].positions()[0];
        let placed = Vector3::new(
            f64::from(point[0]),
            f64::from(point[1]),
            f64::from(point[2]),
        );
        assert!((placed.x - expected.x).abs() < 1e-3);
        assert!((placed.y - expected.y).abs() < 1e-3);
        assert!((placed.z - expected.z).abs() < 1e-3);
        assert!((placed.length() - radius).abs() < 1e-3);

        let (u, v) = orthonormal_basis(Vector3::new(0.0, 0.0, 1.0));
        let measured = placed.dot(&v).atan2(placed.dot(&u));
        assert!((measured.to_degrees() - 90.0).abs() < 1e-3);
    }

    #[test]
    fn a_ring_commit_carries_degrees() {
        let camera = camera(100.0);
        let mut handle = ring_handle("committed", 2);
        handle.value = 45.0;
        let mut manipulator = DeclarativeManipulator::new(11, handle);
        assert!(manipulator.update(&camera));

        assert!(manipulator.begin_drag(&camera, on_screen_ring(&camera, 25.0, 0.0)));
        let commit = manipulator.end_drag().expect("a commit");

        assert_eq!(commit.target, 11);
        assert_eq!(commit.values, vec![45.0]);
        assert_eq!(commit.readout, "45.00");
    }
}

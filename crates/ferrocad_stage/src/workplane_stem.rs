//! The active workplane's `+N` offset stem (W2).
//!
//! `WP ▸ Offset workplane…` is a **viewport** gesture, not a document edit: the plane
//! keeps its origin frame and gains a transient offset along its own normal. The stem
//! is the handle that gesture drags.
//!
//! ```text
//!                  ▲  readout: `ΔN 12.3 mm`
//!                  │
//!                  │  ← `+N` stem (screen-invariant length)
//!                  ●  the active plane's origin
//!        ──────────┴──────────  the active plane
//! ```
//!
//! Every plane except a face-bound one draws `U/V/N` (`workplanes.md` §8.2), so the
//! stem is the `N` axis made draggable rather than a second, competing arrow. A
//! face-bound plane keeps its `U/V` bracket and leaves the normal to the face
//! manipulator, so the shell does not arm this stem for one.
//!
//! The drag solver, the screen-space hit test and the `Linear` value are reused from
//! [`TransformGizmo`]: the stem's frame is the plane's own `u`/`v`/`n`, and only the
//! `AxisZ` (local `+N`) grab is started. The commit is neutral data and names no
//! document object:
//!
//! ```text
//! InteractionCommit { manipulator: "workplane_offset", action: "offset",
//!                     values: [delta_mm], readout: "ΔN 12.3 mm", creates: false }
//! ```
//!
//! This module is pure presentation: no `pyo3`, no kernel, no document.

use std::sync::{Arc, Mutex, MutexGuard};

use ferrocad_graphics::Camera;
use ferrocad_types::{Matrix4, Vector3};

use crate::gizmo::{
    GizmoHandle, GizmoStyle, GizmoValue, ManipulatorFrame, TransformGizmo, MIN_HIT_RADIUS_PX,
};
use crate::interaction::{InteractionCommit, Manipulator};
use crate::screen::{
    camera_forward, point_segment_distance, project_device, push_ribbon_uv, scale_for,
};
use crate::shape::{DrawMode, ShapeItem};
use crate::workplane::Workplane;

/// The manipulator's registry id (also the commit's `manipulator`).
pub const WORKPLANE_OFFSET_ID: &str = "workplane_offset";

/// The commit's action.
pub const WORKPLANE_OFFSET_ACTION: &str = "offset";

/// The commit's target. An offset applies to the **active plane**, which is transient
/// viewport state, so there is no document entity to name; the host recognises the
/// commit by [`WORKPLANE_OFFSET_ID`] and never looks a target up.
pub const WORKPLANE_ENTITY: u32 = 0;

/// The stem's overlay id. It is drawn (not picked through the buffer), so it only has
/// to stay clear of the document, gizmo and other overlay ids.
pub const WORKPLANE_STEM_PICK_ID: u32 = 2_200_000;

/// The stem's screen-invariant length, in device pixels: shorter than the triad, so it
/// reads as a handle on the plane rather than a third axis of the body control.
pub const STEM_PIXELS: f64 = 44.0;

/// The smallest projected stem (device px). Below it the normal points at the camera,
/// the stem collapses to a dot, and the handle is not grabbable.
const MIN_STEM_PIXELS: f64 = 1.0;

/// The smallest distance (mm) a release commits. A press with no travel is a click,
/// not an offset.
pub const COMMIT_EPSILON: f64 = 1e-9;

/// A shared read of the stem's live drag total.
///
/// The stem is hosted by the viewport, but the `Offset Workplane…` HUD — the typed
/// half of the same session — belongs to the shell, and a workbench manipulator drag
/// emits no per-tick viewport event (only the gizmo has a deferred preview stream).
/// The session owner keeps one handle, hands a clone to the stem, and reads it when
/// the viewport repaints: every tick that moves the stem notifies the viewport, and
/// the shell already observes that notification for the camera mirror.
///
/// The handle is **per session**, never process-wide: two windows open two sessions
/// and hold two handles, so neither can read the other's drag.
#[derive(Clone, Debug, Default)]
pub struct WorkplaneLive {
    inner: Arc<Mutex<WorkplaneLiveState>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct WorkplaneLiveState {
    /// The session total the stem was armed with; a drag reports travel from here.
    base: f64,
    /// The current drag's signed travel, while a drag is in flight.
    travel: Option<f64>,
}

impl WorkplaneLive {
    /// A handle whose drags report a total relative to `base`.
    pub fn new(base: f64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WorkplaneLiveState {
                base,
                travel: None,
            })),
        }
    }

    /// Re-seed the session total: a typed value moved the plane under the stem.
    pub fn set_base(&self, base: f64) {
        self.lock().base = base;
    }

    /// The live total (`base + travel`) while a drag is in flight, else `None`.
    pub fn total(&self) -> Option<f64> {
        let state = self.lock();
        state.travel.map(|travel| state.base + travel)
    }

    /// Record the in-flight drag's travel.
    fn drag_to(&self, travel: f64) {
        self.lock().travel = Some(travel);
    }

    /// The drag is over: the commit (or the cancel) owns the value now.
    fn clear(&self) {
        self.lock().travel = None;
    }

    fn lock(&self) -> MutexGuard<'_, WorkplaneLiveState> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// A 1D `+N` stem on the active workplane.
///
/// The host arms one per `Offset workplane…` session, draws [`items`](Self::items) as
/// an overlay, routes the pointer through [`hit`](Self::hit)/[`begin_drag`]/
/// [`drag_to`](Self::drag_to)/[`end_drag`](Self::end_drag), and applies the committed
/// distance to the plane.
#[derive(Clone, Debug)]
pub struct WorkplaneStem {
    /// The plane the stem belongs to: its origin is the stem's base, its normal the
    /// drag axis.
    plane: Workplane,
    /// The live signed distance along `+N` for the current drag.
    delta: f64,
    /// The delta when the active drag began, for [`cancel_drag`](Self::cancel_drag).
    press_delta: f64,
    /// The workspace appearance.
    style: GizmoStyle,
    /// The reused axis solver: its local Z is the plane normal, and only the `AxisZ`
    /// grab is ever started.
    solver: TransformGizmo,
    /// The host's per-session read of the live drag total.
    live: WorkplaneLive,
    hovered: bool,
    /// The camera view and frame the cached stroke was built for.
    view: Option<Matrix4>,
    built_origin: Option<Vector3>,
    built_normal: Option<Vector3>,
    dirty: bool,
    /// The draw order: the halo (a wider stem in the outline ink) first, the stem on top.
    items: [ShapeItem; 2],
}

impl WorkplaneStem {
    /// A stem on `plane`: at its origin, along its `+N`.
    pub fn new(plane: &Workplane) -> Self {
        Self::tracked(plane, WorkplaneLive::default())
    }

    /// A stem that also reports its live drag total through `live`.
    ///
    /// The host owns the handle and seeds it with the session total the stem is
    /// armed at, so a drag tick reads `session_total_at_arm + travel`. [`new`](Self::new)
    /// is this with a detached, zero-based handle.
    pub fn tracked(plane: &Workplane, live: WorkplaneLive) -> Self {
        let style = GizmoStyle::default();
        let ghost_alpha = style.ghost_alpha;
        let mut solver = TransformGizmo::translate(WORKPLANE_ENTITY, plane.origin);
        solver.set_frame(ManipulatorFrame::of_workplane(plane));
        Self {
            plane: plane.clone(),
            delta: 0.0,
            press_delta: 0.0,
            style,
            solver,
            live,
            hovered: false,
            view: None,
            built_origin: None,
            built_normal: None,
            dirty: true,
            items: [
                ShapeItem::new(WORKPLANE_STEM_PICK_ID + 1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_blended_surface()
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
                ShapeItem::new(WORKPLANE_STEM_PICK_ID)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
            ],
        }
    }

    /// The plane the stem offsets.
    pub fn plane(&self) -> &Workplane {
        &self.plane
    }

    /// The stem's base: the active plane's origin.
    pub fn origin(&self) -> Vector3 {
        self.plane.origin
    }

    /// The `+N` direction the stem follows.
    pub fn normal(&self) -> Vector3 {
        self.plane.n
    }

    /// The live signed distance along `+N` (zero when no drag is in flight).
    pub fn delta(&self) -> f64 {
        self.delta
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.solver.is_dragging()
    }

    /// Whether the stem is highlighted.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The overlay items, halo first so the stem sits on top of it.
    pub fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        &self.style
    }

    /// The dimension pill text (`ΔN 12.3 mm`).
    pub fn readout(&self) -> String {
        format!("ΔN {:.1} mm", self.delta)
    }

    /// Set the linear grid step used by the reused axis solver (`None` disables snap).
    pub fn set_step(&mut self, step: Option<f64>) {
        self.solver.set_linear_snap(step);
    }

    /// Highlight (or un-highlight) the stem. Returns whether the visuals changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        // The hovered stroke is wider, and the width is baked into the geometry.
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Apply the workspace gizmo style. Returns whether the style changed.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        if self.style == *style {
            return false;
        }
        self.style = style.clone();
        self.solver.set_style(style);
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Re-base the stem on `plane` (the host re-arms it after an applied offset, or a
    /// different plane became active). Returns whether the frame changed.
    pub fn set_plane(&mut self, plane: &Workplane) -> bool {
        if self.plane == *plane {
            return false;
        }
        self.plane = plane.clone();
        self.solver.set_frame(ManipulatorFrame::of_workplane(plane));
        self.delta = 0.0;
        self.press_delta = 0.0;
        self.live.clear();
        self.dirty = true;
        true
    }

    /// Rebuild the stroke for `camera` when the view, the frame or the style changed.
    /// Returns whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let changed = self.dirty
            || self.view != Some(camera.view)
            || self.built_origin != Some(self.plane.origin)
            || self.built_normal != Some(self.plane.n);
        if !changed {
            return false;
        }
        self.view = Some(camera.view);
        self.built_origin = Some(self.plane.origin);
        self.built_normal = Some(self.plane.n);
        self.dirty = false;
        self.build(camera);
        true
    }

    /// The stem's two ends in device pixels: `(base, tip)`. `None` when either end
    /// cannot be projected.
    pub fn handle_points(&self, camera: &Camera) -> Option<([f64; 2], [f64; 2])> {
        let tip = self.tip(camera)?;
        Some((
            project_device(camera, self.plane.origin)?,
            project_device(camera, tip)?,
        ))
    }

    /// Whether the pointer is on the stem within the style's grab radius.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some((from, to)) = self.handle_points(camera) else {
            return false;
        };
        // A stem pointing at the eye projects to a dot: the axis solver would have no
        // usable direction, so the handle is disabled for that frame.
        if (to[0] - from[0]).hypot(to[1] - from[1]) < MIN_STEM_PIXELS {
            return false;
        }
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let (distance, _) = point_segment_distance(pointer, from, to);
        distance <= self.style.hit_radius_px.max(MIN_HIT_RADIUS_PX)
    }

    /// Begin a drag at `device`. The reused axis solver freezes its own press frame.
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        if !self.hit(camera, device) {
            return false;
        }
        if !self.solver.begin_drag(GizmoHandle::AxisZ, camera, device) {
            return false;
        }
        self.live.clear();
        self.press_delta = self.delta;
        self.hovered = true;
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Advance the drag, returning the signed distance along `+N` when it changed.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<f64> {
        let event = self.solver.drag_to(camera, device)?;
        let GizmoValue::Linear { applied, .. } = event.value else {
            return None;
        };
        if !applied.is_finite() || (applied - self.delta).abs() <= 1e-12 {
            return None;
        }
        self.delta = applied;
        self.live.drag_to(self.delta);
        self.dirty = true;
        Some(self.delta)
    }

    /// End the drag, returning the commit for the distance it travelled.
    ///
    /// A press with no travel (or a drag that returned to its start) commits nothing.
    pub fn end_drag(&mut self) -> Option<InteractionCommit> {
        self.live.clear();
        self.solver.end_drag()?;
        let delta = std::mem::take(&mut self.delta);
        self.press_delta = 0.0;
        self.dirty = true;
        self.apply_color();
        self.commit_for(delta)
    }

    /// Abort the drag without a commit, restoring the delta it started from. Returns
    /// whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        if !self.solver.cancel_drag() {
            return false;
        }
        self.delta = self.press_delta;
        self.live.clear();
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Where the dimension pill is anchored, in device pixels: the reused axis
    /// readout's tip while a drag is active, and the stem tip at rest.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        if let Some(anchor) = self.solver.readout_anchor(camera) {
            return Some(anchor);
        }
        self.handle_points(camera)
            .map(|(_, tip)| tip)
            .or_else(|| project_device(camera, self.plane.origin))
    }

    /// The commit that carries `delta`, or `None` when it is too small to be one.
    fn commit_for(&self, delta: f64) -> Option<InteractionCommit> {
        if !delta.is_finite() || delta.abs() < COMMIT_EPSILON {
            return None;
        }
        Some(
            InteractionCommit::new(
                WORKPLANE_OFFSET_ID,
                WORKPLANE_ENTITY,
                WORKPLANE_OFFSET_ACTION,
            )
            .with_values([delta])
            .with_readout(format!("ΔN {delta:.1} mm")),
        )
    }

    /// The stem's world tip: a screen-invariant length along `+N`.
    fn tip(&self, camera: &Camera) -> Option<Vector3> {
        let length = scale_for(camera, self.plane.origin, STEM_PIXELS);
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        Some(self.plane.origin.add(&self.plane.n.scale(length)))
    }

    /// The stem ink: the workspace gizmo's active/inactive token, dimmed by opacity.
    fn stroke_color(&self) -> [f32; 4] {
        let ink = if self.hovered || self.is_dragging() {
            self.style.active
        } else {
            self.style.inactive
        };
        [ink[0], ink[1], ink[2], ink[3] * self.style.opacity]
    }

    /// Recolour the cached items without rebuilding their geometry.
    fn apply_color(&mut self) {
        let stroke = self.stroke_color();
        self.items[1].set_surface_color(stroke);
        self.items[1].set_edge_color(stroke);
        self.items[1].set_ghost_alpha(self.style.ghost_alpha);
        let outline = [
            self.style.outline[0],
            self.style.outline[1],
            self.style.outline[2],
            self.style.outline[3] * self.style.opacity,
        ];
        self.items[0].set_surface_color(outline);
        self.items[0].set_edge_color(outline);
        self.items[0].set_ghost_alpha(self.style.ghost_alpha);
    }

    /// Build the screen-facing stem ribbon and, when the style enables it, its halo.
    fn build(&mut self, camera: &Camera) {
        let Some(tip) = self.tip(camera) else {
            for item in &mut self.items {
                item.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            }
            return;
        };
        let forward = camera_forward(camera);
        let per_pixel = scale_for(camera, self.plane.origin, 1.0);
        let active = self.hovered || self.is_dragging();
        let hover_scale = self.style.hover_scale.max(1.0);
        let halo = self.style.halo_width.max(0.0);
        let stroke_pixels = if active {
            self.style.width * hover_scale
        } else {
            self.style.width
        };
        let stroke_half = stroke_pixels * 0.5 * per_pixel;
        let halo_half = (stroke_pixels + 2.0 * halo) * 0.5 * per_pixel;

        let ribbons = |half: f64| {
            let mut positions = Vec::new();
            let mut uvs = Vec::new();
            let mut triangles = Vec::new();
            push_ribbon_uv(
                &mut positions,
                &mut uvs,
                &mut triangles,
                ferrocad_graphics::screen_ribbon(self.plane.origin, tip, half, forward),
            );
            (positions, uvs, triangles)
        };

        let (stroke_positions, stroke_uvs, stroke_triangles) = ribbons(stroke_half);
        if halo > 0.0 {
            let (halo_positions, halo_uvs, halo_triangles) = ribbons(halo_half);
            self.items[0].set_shape_uv(
                halo_positions,
                Vec::new(),
                halo_uvs,
                halo_triangles,
                Vec::new(),
            );
        } else {
            self.items[0].set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }
        self.items[1].set_shape_uv(
            stroke_positions,
            Vec::new(),
            stroke_uvs,
            stroke_triangles,
            Vec::new(),
        );
        self.apply_color();
    }
}

impl Manipulator for WorkplaneStem {
    fn id(&self) -> &str {
        WORKPLANE_OFFSET_ID
    }

    fn items(&self) -> &[ShapeItem] {
        WorkplaneStem::items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        WorkplaneStem::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        WorkplaneStem::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        WorkplaneStem::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        WorkplaneStem::end_drag(self)
    }

    fn drag_cancel(&mut self) -> bool {
        WorkplaneStem::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        WorkplaneStem::is_dragging(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        WorkplaneStem::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        WorkplaneStem::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn readout(&self) -> Option<String> {
        Some(WorkplaneStem::readout(self))
    }

    fn anchor(&self) -> Option<Vector3> {
        Some(self.plane.origin)
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        WorkplaneStem::readout_anchor(self, camera)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workplane::{StandardPlane, WorkplaneKind};
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_types::{Placement, Rotation};

    /// A perspective camera at `(0, 0, -eye_z)` looking toward `+Z`.
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

    /// The `YZ` plane: its normal is world `+X`, across the camera, so the stem is
    /// fully projected and grabbable.
    fn plane() -> Workplane {
        Workplane::standard(StandardPlane::Right)
    }

    /// The same frame with the in-plane axes swapped, so its normal is `-X`.
    fn reversed_plane() -> Workplane {
        Workplane::new(
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 1.0, 0.0),
            WorkplaneKind::World,
        )
        .expect("a frame")
    }

    /// A device point on the stem's tip.
    fn grab(camera: &Camera, stem: &WorkplaneStem) -> [i32; 2] {
        let (_, tip) = stem.handle_points(camera).expect("a projected stem");
        [tip[0].round() as i32, tip[1].round() as i32]
    }

    #[test]
    fn the_stem_stands_on_the_plane_origin_along_its_normal() {
        let stem = WorkplaneStem::new(&plane());
        assert_eq!(stem.origin(), Vector3::zero());
        assert!((stem.normal().x - 1.0).abs() < 1e-9, "`+N` is world `+X`");
        assert_eq!(stem.id(), WORKPLANE_OFFSET_ID);
    }

    #[test]
    fn the_stem_hits_on_the_stem_and_misses_beside_it() {
        let camera = camera(5.0);
        let stem = WorkplaneStem::new(&plane());
        let (base, tip) = stem.handle_points(&camera).expect("a projected stem");
        let middle = [
            ((base[0] + tip[0]) * 0.5).round() as i32,
            ((base[1] + tip[1]) * 0.5).round() as i32,
        ];
        assert!(stem.hit(&camera, middle), "the middle of the stem is grabbable");

        // Well clear of the stroke, far outside the grab radius.
        let off = [middle[0], middle[1] + 60];
        assert!(!stem.hit(&camera, off), "beside the stem is not a hit");
        assert!(
            !stem.hit(&camera, [base[0] as i32, (base[1] + 60.0) as i32]),
            "off the base is not a hit either"
        );
    }

    #[test]
    fn dragging_along_the_normal_commits_the_distance() {
        let camera = camera(5.0);
        let mut stem = WorkplaneStem::new(&plane());
        let grab = grab(&camera, &stem);
        assert!(Manipulator::drag_begin(&mut stem, &camera, grab));
        assert!(Manipulator::drag_update(
            &mut stem,
            &camera,
            [grab[0] + 20, grab[1]]
        ));
        let commit = Manipulator::drag_end(&mut stem).expect("a commit");

        assert_eq!(commit.manipulator, WORKPLANE_OFFSET_ID);
        assert_eq!(commit.action, WORKPLANE_OFFSET_ACTION);
        assert!(commit.element.is_none(), "the active plane is not an element");
        assert!(!commit.creates, "an offset edits no document object");
        assert_eq!(commit.values.len(), 1);
        assert!(
            commit.values[0] > 0.0,
            "dragging along `+N` is a positive delta: {}",
            commit.values[0]
        );
        assert_eq!(commit.readout, format!("ΔN {:.1} mm", commit.values[0]));
        assert!(commit.readout.starts_with("ΔN "), "{}", commit.readout);

        // The gesture commits exactly once, and a fresh stem starts at zero.
        assert!(Manipulator::drag_end(&mut stem).is_none());
        assert!(stem.delta().abs() < 1e-12);
    }

    #[test]
    fn a_reversed_normal_flips_the_signed_delta_but_not_the_motion() {
        let camera = camera(5.0);
        let forward = {
            let mut stem = WorkplaneStem::new(&plane());
            let grab = grab(&camera, &stem);
            assert!(stem.begin_drag(&camera, grab));
            assert!(stem.drag_to(&camera, [grab[0] + 20, grab[1]]).is_some());
            stem.delta()
        };
        let backward = {
            let mut stem = WorkplaneStem::new(&reversed_plane());
            let grab = grab(&camera, &stem);
            assert!(stem.begin_drag(&camera, grab));
            assert!(stem.drag_to(&camera, [grab[0] + 20, grab[1]]).is_some());
            stem.delta()
        };

        assert!(forward > 0.0, "{forward}");
        assert!(
            (forward + backward).abs() < 1e-9,
            "`+N` and `-N` measure the same screen drag with opposite signs: \
             {forward} vs {backward}"
        );
        // Both move the plane the same way in the world: `n * delta` is the same world
        // vector whichever way the normal points.
        let moved = plane().n.scale(forward);
        let moved_reversed = reversed_plane().n.scale(backward);
        assert!(moved.is_equal(&moved_reversed, 1e-9));
        assert!(moved.x > 0.0, "the pointer moved right, so the plane did too");
    }

    #[test]
    fn a_tracked_stem_reports_its_live_total_each_tick() {
        let camera = camera(5.0);
        // The session already sits 3 mm along `+N`, so a drag adds to that total.
        let live = WorkplaneLive::new(3.0);
        let mut stem = WorkplaneStem::tracked(&plane(), live.clone());
        let grab = grab(&camera, &stem);
        assert_eq!(live.total(), None, "at rest there is no live total");

        assert!(stem.begin_drag(&camera, grab));
        assert!(stem.drag_to(&camera, [grab[0] + 20, grab[1]]).is_some());
        let total = live.total().expect("a live total mid-drag");
        assert!(
            (total - (3.0 + stem.delta())).abs() < 1e-12,
            "the live total is the session base plus the drag: {total}"
        );

        // The release clears the live read; the commit owns the value from here.
        assert!(stem.end_drag().is_some());
        assert_eq!(live.total(), None);
    }

    #[test]
    fn a_zero_drag_commits_nothing() {
        let camera = camera(5.0);
        let mut stem = WorkplaneStem::new(&plane());
        let grab = grab(&camera, &stem);
        // A press and release in place is a click, not an offset.
        assert!(stem.begin_drag(&camera, grab));
        assert_eq!(stem.drag_to(&camera, grab), None);
        assert!(
            Manipulator::drag_end(&mut stem).is_none(),
            "a press with no travel commits nothing"
        );
    }

    #[test]
    fn cancelling_restores_the_press_delta() {
        let camera = camera(5.0);
        let mut stem = WorkplaneStem::new(&plane());
        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        assert!(stem.drag_to(&camera, [grab[0] + 20, grab[1]]).is_some());
        assert!(stem.delta() > 0.0);
        assert!(stem.cancel_drag());
        assert!(stem.delta().abs() < 1e-12, "the drag is undone");
        assert!(!stem.cancel_drag(), "nothing left to cancel");
    }

    #[test]
    fn the_stem_is_a_screen_invariant_ribbon_in_the_gizmo_ink() {
        let camera = camera(5.0);
        let mut stem = WorkplaneStem::new(&plane());
        assert!(stem.update(&camera), "the first update builds");
        assert!(!stem.update(&camera), "an unchanged camera is a no-op");
        assert!(!stem.items()[1].positions().is_empty());

        let color = stem.items()[1].surface_color();
        let ink = GizmoStyle::default().inactive;
        assert!((color[0] - ink[0]).abs() < 1e-6);
        assert!((color[1] - ink[1]).abs() < 1e-6);
        assert!((color[2] - ink[2]).abs() < 1e-6);

        let (base, tip) = stem.handle_points(&camera).expect("projected");
        let projected = (tip[0] - base[0]).hypot(tip[1] - base[1]);
        assert!(
            (projected - STEM_PIXELS).abs() < 1.0,
            "the stem is a fixed pixel length: {projected}"
        );
    }

    #[test]
    fn a_new_plane_rebases_the_stem() {
        let camera = camera(5.0);
        let mut stem = WorkplaneStem::new(&plane());
        let other = Workplane::new(
            Vector3::new(0.0, 0.0, 12.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            WorkplaneKind::World,
        )
        .expect("a frame");
        assert!(stem.set_plane(&other));
        assert_eq!(stem.origin(), Vector3::new(0.0, 0.0, 12.0));
        assert!(!stem.set_plane(&other), "the same plane is a no-op");
        assert!(stem.update(&camera));
    }
}

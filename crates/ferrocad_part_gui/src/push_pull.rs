//! The face push/pull stem: extrude or pocket a planar face along its normal.
//!
//! Selecting a planar face and dragging along its normal is the Part workbench's
//! push/pull gesture. The generic triad cannot express it: a triad commit is a bare
//! placement transform and drops the element name, while this operation is "extrude
//! `Face6` by `d`". This module supplies the **manipulator**: a single amber stem from
//! the face centroid along `+N`, plus the drag that turns into a signed distance.
//!
//! ```text
//!                    ▲  readout: `extrude 12.30 mm`
//!                    │
//!                    │  ← amber stem (screen-invariant length)
//!                    ●  ghost centroid (the swept face, at the pending distance)
//!          ┌─────────┴─────────┐
//!          │      selected     │
//!          │        face       │
//!          └───────────────────┘
//! ```
//!
//! The face frame is derived from the picked item's own tessellation: [`ShapeItem`]
//! carries per-position part ids and the part-name table, so `Face6` resolves to its
//! triangles without a kernel call. Only **planar** faces get a stem; a curved face
//! (whose triangle normals disagree) returns `None`.
//!
//! The stem **rides the ghost**: its base is the swept face's centroid, so the handle sits
//! on the surface the drag is producing and stays under the pointer, the way the Move
//! triad travels with its body. At rest the ghost is the face, so the stem starts there.
//!
//! The axis drag solver, the screen-space fallback for a near view-aligned stem and
//! the `Linear` value are reused from [`ferrocad_stage::TransformGizmo`]: the stem's
//! frame has the face normal as its local Z and only the `AxisZ` grab is used. The
//! commit is neutral data:
//! `InteractionCommit::new("push_pull", entity, "extrude" | "pocket")` with the face
//! element and `[distance]`. The app script owns what that means for the document.
//!
//! While the drag is live — and while the shell holds the released value for `✓`/`✗` —
//! the stem publishes a translucent **swept prism** of the selected face at the pending
//! distance ([`preview_items`](PushPullStem::preview_items)): the face carried along the
//! normal, closed by the walls its border sweeps. It is display-only and unpickable, and
//! the exact boolean result waits for the commit, so the ghost needs no kernel recompute.
//!
//! This module is pure presentation: no `pyo3`, no kernel, no document.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::Camera;
use ferrocad_stage::gizmo::{
    GizmoHandle, GizmoStyle, GizmoValue, ManipulatorFrame, TransformGizmo, MIN_HIT_RADIUS_PX,
    PUSH_PULL_COLOR,
};
use ferrocad_stage::screen::{
    camera_forward, point_segment_distance, project_device, push_ribbon_uv, scale_for,
};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, ShapeItem, Stage};
use ferrocad_types::{Matrix4, Vector3};

/// The manipulator's registry id (`InteractionCommit::manipulator`).
pub const PUSH_PULL_ID: &str = "push_pull";

/// The stem's fixed picking id. It is drawn as an overlay (not picked through the
/// buffer), so this only has to stay clear of document, gizmo and other overlay ids.
pub const PUSH_PULL_PICK_ID: u32 = 2_100_000;

/// The smallest projected stem (device px). Below it the face normal points at the
/// camera, the stem collapses to a dot, and the handle is not grabbable.
const MIN_STEM_PIXELS: f64 = 1.0;

/// How closely every triangle normal must agree with the face normal for the face to
/// count as planar (`dot >= 1 - 1e-4`, about 0.8°).
const NORMAL_ALIGNMENT: f64 = 1.0 - 1e-4;

/// The out-of-plane tolerance, relative to the face's own extent.
const PLANAR_TOLERANCE: f64 = 1e-4;

/// One push/pull commit: the signed distance along the face normal and what it names.
#[derive(Clone, Debug, PartialEq)]
pub struct PushPullChange {
    /// The entity the face belongs to.
    pub target_id: u32,
    /// The selected face name (`Face6`).
    pub face: String,
    /// The signed distance along `+N`: outward is positive (extrude), inward negative
    /// (pocket).
    pub distance: f64,
}

impl PushPullChange {
    /// The operation the sign selects: `extrude` for outward, `pocket` for inward.
    pub fn action(&self) -> &'static str {
        if self.distance < 0.0 {
            "pocket"
        } else {
            "extrude"
        }
    }

    /// The dimension pill text (`extrude 12.30 mm`, `pocket 5.00 mm`).
    pub fn readout(&self) -> String {
        format!("{} {:.2} mm", self.action(), self.distance.abs())
    }

    /// The neutral commit that carries this change.
    pub fn into_commit(self) -> InteractionCommit {
        let readout = self.readout();
        let action = self.action();
        InteractionCommit::new(PUSH_PULL_ID, self.target_id, action)
            .with_element(self.face)
            .with_values([self.distance])
            .with_readout(readout)
    }

    /// The typed change a push/pull commit carries, if it is one.
    pub fn from_commit(commit: &InteractionCommit) -> Option<Self> {
        if commit.manipulator != PUSH_PULL_ID {
            return None;
        }
        Some(Self {
            target_id: commit.target,
            face: commit.element.clone()?,
            distance: *commit.values.first()?,
        })
    }
}

/// A push/pull stem over one planar face.
///
/// The host builds one per selected face, draws [`items`](Self::items) as an overlay,
/// routes the pointer through [`hit`](Self::hit)/[`begin_drag`](Self::begin_drag)/
/// [`drag_to`](Self::drag_to)/[`end_drag`](Self::end_drag), and hands the committed
/// distance to the document layer.
#[derive(Clone, Debug)]
pub struct PushPullStem {
    target_id: u32,
    face: String,
    /// The face region's area-weighted centroid, in world space.
    centroid: Vector3,
    /// The outward unit normal of the face.
    normal: Vector3,
    /// The current signed distance along `+N`.
    distance: f64,
    /// The distance when the active drag began, for [`cancel_drag`](Self::cancel_drag).
    press_distance: f64,
    /// The workspace appearance.
    style: GizmoStyle,
    /// The reused axis solver: its local Z is [`normal`](Self::normal), and only the
    /// `AxisZ` grab is ever started.
    solver: TransformGizmo,
    hovered: bool,
    /// The camera view and face frame the cached stroke was built for.
    view: Option<Matrix4>,
    built_anchor: Option<Vector3>,
    built_normal: Option<Vector3>,
    dirty: bool,
    /// The draw order: the halo (a wider stem in the outline ink) first, the stem on top.
    items: [ShapeItem; 2],
    /// The pending-depth ghost of the selected face, swept along the normal. Shown while a
    /// drag is active and after a release, while the shell holds the commit.
    preview: ShapeItem,
    /// The face's flat mesh (three vertices per triangle), captured from the stage item.
    face_positions: Vec<[f32; 3]>,
    face_triangles: Vec<u32>,
    /// The face's border as directed vertex pairs (indices into
    /// [`face_positions`](Self::face_positions)), in the face's own winding. The swept
    /// walls use it so the ghost closes into a volume instead of a floating plane.
    face_border: Vec<(u32, u32)>,
    /// How far the body extends **behind** the face along `-N`, in world millimetres.
    ///
    /// The numeric row's hint: a pocket deeper than this leaves the body. It is a note,
    /// not a bound — the edit stays legal past it.
    body_depth: f64,
    /// Whether the released value is held for the host's `✓`/`✗`.
    pending: bool,
    /// A note the document half sent with the probe (`editing the existing extrusion`),
    /// printed muted after the row so the gesture's meaning is visible.
    note: Option<String>,
}

impl PushPullStem {
    /// A stem over `face` on `target_id`, anchored at `centroid` with outward `normal`.
    ///
    /// A zero (or non-finite) normal cannot place a stem; the factory already rejects
    /// non-planar faces, so this is only the degenerate-input guard.
    pub fn new(
        target_id: u32,
        face: impl Into<String>,
        centroid: Vector3,
        normal: Vector3,
    ) -> Option<Self> {
        let normal = normal.normalize();
        if normal.is_null()
            || !normal.x.is_finite()
            || !normal.y.is_finite()
            || !normal.z.is_finite()
        {
            return None;
        }
        let style = GizmoStyle::default();
        let ghost_alpha = style.ghost_alpha;
        let mut solver = TransformGizmo::translate(target_id, centroid);
        solver.set_frame(frame_at(centroid, normal));
        Some(Self {
            target_id,
            face: face.into(),
            centroid,
            normal,
            distance: 0.0,
            press_distance: 0.0,
            style,
            solver,
            hovered: false,
            view: None,
            built_anchor: None,
            built_normal: None,
            dirty: true,
            items: [
                ShapeItem::new(PUSH_PULL_PICK_ID + 1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_blended_surface()
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
                ShapeItem::new(PUSH_PULL_PICK_ID)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
            ],
            preview: ShapeItem::new(PUSH_PULL_PICK_ID + 2)
                .with_draw_mode(DrawMode::Shaded)
                .with_selectable(false)
                .with_blended_surface(),
            face_positions: Vec::new(),
            face_triangles: Vec::new(),
            face_border: Vec::new(),
            body_depth: 0.0,
            pending: false,
            note: None,
        })
    }

    /// The entity the stem targets.
    pub fn target_id(&self) -> u32 {
        self.target_id
    }

    /// Give the stem the selected face's flat mesh, so its drag can ghost the face at the
    /// pending depth. The factory supplies it from the stage item; a stem built directly
    /// with [`new`](Self::new) has no ghost geometry and simply draws its handle.
    pub fn set_face_geometry(&mut self, positions: Vec<[f32; 3]>, triangles: Vec<u32>) {
        self.face_border = border_edges(&positions, &triangles);
        self.face_positions = positions;
        self.face_triangles = triangles;
        self.rebuild_preview();
    }

    /// How far the body extends behind the face along `-N` (0 when unknown).
    pub fn body_depth(&self) -> f64 {
        self.body_depth
    }

    /// Note how thick the body is behind the face, for the numeric row's hint.
    ///
    /// The factory measures it from the whole item: the face's own triangles lie on the
    /// plane, so they cannot answer it.
    pub fn set_body_depth(&mut self, depth: f64) {
        self.body_depth = if depth.is_finite() && depth > 0.0 {
            depth
        } else {
            0.0
        };
    }

    /// The display-only ghost of the face at [`distance`](Self::distance).
    pub fn preview_items(&self) -> &[ShapeItem] {
        std::slice::from_ref(&self.preview)
    }

    /// The selected face's name (`Face6`).
    pub fn face_name(&self) -> &str {
        &self.face
    }

    /// The face centroid the sweep starts from (the ghost's datum at zero distance).
    pub fn centroid(&self) -> Vector3 {
        self.centroid
    }

    /// The world point the handle sits at: the **ghost's** centroid.
    ///
    /// The stem rides the swept face, the way the Move triad travels with its body, so
    /// the handle is always on the surface the drag is producing. At rest the ghost is the
    /// face itself, so this is the face centroid.
    pub fn anchor(&self) -> Option<Vector3> {
        Some(self.ghost_anchor())
    }

    /// The ghost's centroid: the face centroid carried along the normal by the distance.
    fn ghost_anchor(&self) -> Vector3 {
        self.centroid.add(&self.normal.scale(self.distance))
    }

    /// The outward unit normal the stem follows.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The current signed distance along `+N`.
    pub fn distance(&self) -> f64 {
        self.distance
    }

    /// Set the signed distance from a **typed** value — the numeric twin of the drag.
    ///
    /// Only finiteness is enforced: the sign picks extrude or pocket and a distance has no
    /// intrinsic bound. A non-zero value is an edit, so the ghost is rebuilt and the value
    /// reports pending; zero puts the stem back at rest with nothing to apply.
    pub fn set_distance(&mut self, distance: f64) -> bool {
        if !distance.is_finite() || (distance - self.distance).abs() <= 1e-12 {
            return false;
        }
        self.distance = distance;
        self.pending = distance != 0.0;
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// Open at `distance` without making it an edit — the length an existing extrusion
    /// already has.
    ///
    /// The stem moves to the existing cap (and the numeric row shows the value), but
    /// nothing is held for `✓`: only a drag or a typed number makes a commit. A handle
    /// the user is already editing keeps its value.
    pub fn seed_distance(&mut self, distance: f64) -> bool {
        if self.pending || self.is_dragging() {
            return false;
        }
        if !distance.is_finite() || (distance - self.distance).abs() <= 1e-12 {
            return false;
        }
        self.distance = distance;
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// The operation the current sign selects.
    pub fn action(&self) -> &'static str {
        self.change().action()
    }

    /// The dimension pill text.
    pub fn readout(&self) -> String {
        self.change().readout()
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.solver.is_dragging()
    }

    /// Whether the released distance is held for the host's `✓`/`✗`.
    pub fn is_pending(&self) -> bool {
        self.pending
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

    /// Set the linear grid step used by the reused axis solver (`None` disables snap).
    pub fn set_step(&mut self, step: Option<f64>) {
        self.solver.set_linear_snap(step);
    }

    /// The builder form of [`set_step`](Self::set_step).
    pub fn with_step(mut self, step: Option<f64>) -> Self {
        self.set_step(step);
        self
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

    /// Move the stem's world anchor (the ghost's centroid) to `anchor`.
    ///
    /// The host moves a handle when the object it sits on is previewed somewhere else, so
    /// the whole sweep travels with it: the face the sweep starts from moves by the same
    /// delta, and the distance is untouched.
    pub fn set_anchor(&mut self, anchor: Vector3) -> bool {
        let delta = anchor.sub(&self.ghost_anchor());
        if delta.is_null() {
            return false;
        }
        self.centroid = self.centroid.add(&delta);
        self.solver.set_frame(frame_at(self.centroid, self.normal));
        self.dirty = true;
        true
    }

    /// Rebuild the stroke for `camera` when the view, the drawn anchor or the style
    /// changed. Returns whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let drawn = self.ghost_anchor();
        let changed = self.dirty
            || self.view != Some(camera.view)
            || self.built_anchor != Some(drawn)
            || self.built_normal != Some(self.normal);
        if !changed {
            return false;
        }
        self.view = Some(camera.view);
        self.built_anchor = Some(drawn);
        self.built_normal = Some(self.normal);
        self.dirty = false;
        self.build(camera);
        true
    }

    /// Whether the pointer is on the stem within the style's grab radius.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(tip) = self.tip(camera) else {
            return false;
        };
        let (Some(from), Some(to)) = (
            project_device(camera, self.ghost_anchor()),
            project_device(camera, tip),
        ) else {
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
        self.press_distance = self.distance;
        self.hovered = true;
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Advance the drag, returning the signed distance when it changed.
    ///
    /// The solver reports the pointer's travel since the press, so the new distance is
    /// measured from the value the stem held when it was grabbed. The first drag starts
    /// at zero; a **second** one continues from a held (pending) distance, which is what
    /// lets a released value be adjusted before `✓` instead of restarting from zero.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<PushPullChange> {
        let event = self.solver.drag_to(camera, device)?;
        let GizmoValue::Linear { applied, .. } = event.value else {
            return None;
        };
        let distance = self.press_distance + applied;
        if !distance.is_finite() || (distance - self.distance).abs() <= 1e-12 {
            return None;
        }
        self.distance = distance;
        self.rebuild_preview();
        Some(self.change())
    }

    /// End the drag, returning the distance to commit.
    pub fn end_drag(&mut self) -> Option<PushPullChange> {
        self.solver.end_drag()?;
        // The release hands the value over; the ghost stays until the host spends it
        // (`✓`) or drops it (`✗` → the shell re-arms a fresh stem).
        self.pending = true;
        self.dirty = true;
        self.apply_color();
        Some(self.change())
    }

    /// Cancel the drag, restoring the distance it started from. Returns whether a drag
    /// was active.
    pub fn cancel_drag(&mut self) -> bool {
        if !self.solver.cancel_drag() {
            return false;
        }
        self.distance = self.press_distance;
        self.dirty = true;
        self.apply_color();
        self.rebuild_preview();
        true
    }

    /// Rebuild the swept ghost at the current distance (world space; no camera needed).
    ///
    /// The preview is the face **swept** to the pending depth rather than a floating copy
    /// of it: the cap at the offset plus the walls its border traces. That is what makes
    /// the ghost read as an extrusion (or a pocket) instead of an unmoored plane, and it
    /// costs no kernel call — the face's own tessellation is the sweep profile.
    fn rebuild_preview(&mut self) {
        if self.face_positions.is_empty() || self.face_triangles.is_empty() || self.distance == 0.0 {
            self.preview
                .set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            return;
        }
        let offset = self.normal.scale(self.distance);
        let (dx, dy, dz) = (offset.x as f32, offset.y as f32, offset.z as f32);
        let base = self.face_positions.len();

        let mut positions = Vec::with_capacity(base * 2);
        positions.extend_from_slice(&self.face_positions);
        positions.extend(
            self.face_positions
                .iter()
                .map(|point| [point[0] + dx, point[1] + dy, point[2] + dz]),
        );

        let mut triangles = Vec::with_capacity(self.face_triangles.len() + self.face_border.len() * 6);
        // The cap at the pending depth keeps the face's own winding.
        triangles.extend(self.face_triangles.iter().map(|index| index + base as u32));
        // The walls: the outside of a `a → b` wall is `(b − a) × N`, so `(a, b, b′)` and
        // `(a, b′, a′)` face away from the swept volume for an outward sweep.
        for &(a, b) in &self.face_border {
            let (at, bt) = (a + base as u32, b + base as u32);
            triangles.extend([a, b, bt, a, bt, at]);
        }

        self.preview
            .set_shape(positions, Vec::new(), triangles, Vec::new());
        let color = [
            PUSH_PULL_COLOR[0],
            PUSH_PULL_COLOR[1],
            PUSH_PULL_COLOR[2],
            // A volume overlaps itself in most views; a plane's alpha would stack into mud.
            0.20,
        ];
        self.preview.set_surface_color(color);
        self.preview.set_edge_color(color);
    }

    /// Where the dimension pill is anchored, in device pixels: the stem's tip, which rides
    /// the ghost. The pill therefore follows the surface the drag is producing, not the face
    /// it started from (the reused solver's own readout anchor is measured from its press
    /// frame, i.e. the starting face, so it is deliberately not used here).
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        self.tip(camera)
            .and_then(|tip| project_device(camera, tip))
            .or_else(|| project_device(camera, self.ghost_anchor()))
    }

    /// The stem's world tip: a screen-invariant length along `+N` from the ghost.
    fn tip(&self, camera: &Camera) -> Option<Vector3> {
        let base = self.ghost_anchor();
        let length = scale_for(camera, base, self.style.size);
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        Some(base.add(&self.normal.scale(length)))
    }

    fn change(&self) -> PushPullChange {
        PushPullChange {
            target_id: self.target_id,
            face: self.face.clone(),
            distance: self.distance,
        }
    }

    /// The stem colour: the push/pull amber token, dimmed by the style's opacity.
    fn stroke_color(&self) -> [f32; 4] {
        [
            PUSH_PULL_COLOR[0],
            PUSH_PULL_COLOR[1],
            PUSH_PULL_COLOR[2],
            PUSH_PULL_COLOR[3] * self.style.opacity,
        ]
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
        let per_pixel = scale_for(camera, self.ghost_anchor(), 1.0);
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
                ferrocad_graphics::screen_ribbon(self.centroid, tip, half, forward),
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

impl Manipulator for PushPullStem {
    fn id(&self) -> &str {
        PUSH_PULL_ID
    }

    fn items(&self) -> &[ShapeItem] {
        PushPullStem::items(self)
    }

    fn preview_items(&self) -> &[ShapeItem] {
        PushPullStem::preview_items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        PushPullStem::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        PushPullStem::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        PushPullStem::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        PushPullStem::end_drag(self).map(PushPullChange::into_commit)
    }

    fn drag_cancel(&mut self) -> bool {
        PushPullStem::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        PushPullStem::is_dragging(self)
    }

    fn is_pending(&self) -> bool {
        PushPullStem::is_pending(self)
    }

    fn numeric_fields(&self) -> Vec<ferrocad_stage::NumericField> {
        let field = ferrocad_stage::NumericField::millimetres("D", self.distance);
        // A distance itself is unbounded (an extrusion can go as far as the user likes and
        // a through-cut is still a solid), so the only model-derived number is the body's
        // thickness behind the face: a note, not a bound.
        let body = (self.body_depth > 1e-6)
            .then(|| format!("body {:.2} mm behind this face", self.body_depth));
        match (&self.note, body) {
            // The probe's note comes first: "this edits what you already have" is the
            // thing the user needs before the depth detail.
            (Some(note), Some(body)) => vec![field.with_hint(format!("{note} · {body}"))],
            (Some(note), None) => vec![field.with_hint(note.clone())],
            (None, Some(body)) => vec![field.with_hint(body)],
            (None, None) => vec![field],
        }
    }

    fn set_note(&mut self, note: Option<String>) -> bool {
        if self.note == note {
            return false;
        }
        self.note = note;
        true
    }

    fn set_values(&mut self, values: &[f64]) -> bool {
        let Some(distance) = values.first().copied() else {
            return false;
        };
        PushPullStem::set_distance(self, distance)
    }

    fn seed_values(&mut self, values: &[f64]) -> bool {
        let Some(distance) = values.first().copied() else {
            return false;
        };
        PushPullStem::seed_distance(self, distance)
    }

    fn pending_commit(&self) -> Option<InteractionCommit> {
        (self.distance != 0.0).then(|| self.change().into_commit())
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        PushPullStem::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        PushPullStem::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn anchor_device(&self, camera: &Camera) -> Option<[f64; 2]> {
        // The stem grows out of the face centroid, and the axis solver measures travel
        // along that line: the direction a probe has to pull along.
        self.anchor()
            .and_then(|anchor| ferrocad_stage::screen::project_device(camera, anchor))
    }

    fn readout(&self) -> Option<String> {
        Some(PushPullStem::readout(self))
    }

    fn anchor(&self) -> Option<Vector3> {
        PushPullStem::anchor(self)
    }

    fn set_anchor(&mut self, anchor: Vector3) -> bool {
        PushPullStem::set_anchor(self, anchor)
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        PushPullStem::readout_anchor(self, camera)
    }
}

/// The stem a selection asks for, when it names **one planar face** of a displayed item.
///
/// This is the presentation half of the push/pull manipulator: a workbench registers it
/// as a manipulator factory and the host calls it on every selection. An edge, vertex,
/// whole-body or unknown selection returns `None`, as does a face whose triangles are
/// not coplanar.
pub fn push_pull_stem_for(selection: &SelectionRef, stage: &Stage) -> Option<PushPullStem> {
    if selection.elements.len() != 1 {
        return None;
    }
    let face = selection.elements.first()?;
    if !face.starts_with("Face") {
        return None;
    }
    let item = stage.item(selection.object)?;
    let (centroid, normal, triangles) = face_frame(item, face)?;
    let mut stem = PushPullStem::new(selection.object, face.clone(), centroid, normal)?;
    // A flat mesh (three vertices per triangle) is enough for the translucent ghost; it
    // needs no normals because the preview is a flat-shaded face.
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut indices = Vec::with_capacity(triangles.len() * 3);
    for triangle in &triangles {
        for point in triangle {
            indices.push(positions.len() as u32);
            positions.push([point.x as f32, point.y as f32, point.z as f32]);
        }
    }
    stem.set_face_geometry(positions, indices);
    stem.set_body_depth(body_depth(item, centroid, normal));
    Some(stem)
}

/// How far the item extends **behind** the face along `-N`, in world millimetres.
///
/// The deepest vertex of the whole tessellation behind the face plane: a pocket deeper
/// than this leaves the body. It is the numeric row's hint, not a bound.
fn body_depth(item: &ShapeItem, centroid: Vector3, normal: Vector3) -> f64 {
    item.positions().iter().fold(0.0f64, |deepest, point| {
        let point = Vector3::new(
            f64::from(point[0]),
            f64::from(point[1]),
            f64::from(point[2]),
        );
        deepest.max(centroid.sub(&point).dot(&normal))
    })
}

/// The Part GUI's push/pull factory: a planar-face selection arms the stem.
///
/// `ferrocad_shell_python::Contributions` accepts a factory of this shape, so an edition wires
/// the manipulator in one line and the base app stays Part-free. It returns `None` for
/// an edge selection, leaving room for [`crate::fillet_manipulator`] beside it.
pub fn push_pull_manipulator(
    selection: &SelectionRef,
    stage: &Stage,
) -> Option<Box<dyn Manipulator>> {
    push_pull_stem_for(selection, stage).map(|stem| Box::new(stem) as Box<dyn Manipulator>)
}

/// The named face's border, as **directed** vertex pairs in the face's own winding.
///
/// The stage hands each triangle its own three vertices, so the border is found by
/// position rather than by index: a directed edge whose reverse does not occur is on the
/// boundary of the region, and keeping its direction is what orients the swept wall
/// outward (`(b − a) × N`). The pairs index the first position with each key, so a caller
/// can sweep them with the face's own vertex array.
fn border_edges(positions: &[[f32; 3]], triangles: &[u32]) -> Vec<(u32, u32)> {
    use std::collections::HashMap;
    // `+ 0.0` normalises `-0.0`, which is the same point but a different bit pattern.
    let key = |index: u32| -> [u32; 3] {
        let point = positions[index as usize];
        [
            (point[0] + 0.0).to_bits(),
            (point[1] + 0.0).to_bits(),
            (point[2] + 0.0).to_bits(),
        ]
    };
    let mut first: HashMap<[u32; 3], u32> = HashMap::new();
    for index in 0..positions.len() as u32 {
        first.entry(key(index)).or_insert(index);
    }
    let mut directed: HashMap<([u32; 3], [u32; 3]), ()> = HashMap::new();
    for triangle in triangles.chunks_exact(3) {
        for corner in 0..3 {
            let a = key(triangle[corner]);
            let b = key(triangle[(corner + 1) % 3]);
            if a != b {
                directed.insert((a, b), ());
            }
        }
    }
    let mut border: Vec<(u32, u32)> = directed
        .keys()
        .filter(|(a, b)| !directed.contains_key(&(*b, *a)))
        .map(|(a, b)| (first[a], first[b]))
        .collect();
    // Stable order, so a rebuilt preview is byte-identical for the same face.
    border.sort_unstable();
    border
}

/// The area-weighted centroid and outward unit normal of the named planar face.
///
/// The item's positions are the display tessellation (the Part display hook bakes the
/// object's `Placement` into them), and each triangle is tagged with its part id, so
/// the face is resolved by name without a kernel call. A face is planar only when every
/// triangle normal agrees with the summed normal **and** every vertex lies in that
/// plane; otherwise there is no honest direction to extrude along and `None` is
/// returned.
fn face_frame(item: &ShapeItem, name: &str) -> Option<(Vector3, Vector3, Vec<[Vector3; 3]>)> {
    let part = item
        .part_names()
        .iter()
        .position(|candidate| candidate == name)? as u32;
    let parts = item.parts();
    let positions = item.positions();
    let triangles = item.triangles();

    let mut area = 0.0;
    let mut weighted = Vector3::zero();
    let mut normal_sum = Vector3::zero();
    let mut triangles_of_face = Vec::new();
    let mut normals = Vec::new();

    for triangle in triangles.chunks_exact(3) {
        let owned = triangle
            .iter()
            .all(|index| parts.get(*index as usize).copied().unwrap_or(0) == part);
        if !owned {
            continue;
        }
        let (Some(a), Some(b), Some(c)) = (
            position(positions, triangle[0]),
            position(positions, triangle[1]),
            position(positions, triangle[2]),
        ) else {
            continue;
        };
        let cross = b.sub(&a).cross(&c.sub(&a));
        let double_area = cross.length();
        if double_area <= 1e-12 {
            continue;
        }
        let triangle_area = double_area * 0.5;
        area += triangle_area;
        weighted = weighted.add(&a.add(&b).add(&c).scale(triangle_area / 3.0));
        normal_sum = normal_sum.add(&cross);
        normals.push(cross.scale(1.0 / double_area));
        triangles_of_face.push([a, b, c]);
    }

    if triangles_of_face.is_empty() || area <= 1e-12 {
        return None;
    }
    let normal = normal_sum.normalize();
    if normal.is_null() {
        return None;
    }
    let centroid = weighted.scale(1.0 / area);
    let extent = triangles_of_face
        .iter()
        .flat_map(|triangle| triangle.iter())
        .map(|point| point.distance(&centroid))
        .fold(0.0, f64::max);
    let plane_tolerance = PLANAR_TOLERANCE * extent.max(1.0);

    for unit in &normals {
        if unit.dot(&normal) < NORMAL_ALIGNMENT {
            return None;
        }
    }
    for triangle in &triangles_of_face {
        for point in triangle {
            if point.sub(&centroid).dot(&normal).abs() > plane_tolerance {
                return None;
            }
        }
    }

    Some((centroid, normal, triangles_of_face))
}

/// The manipulator frame at `origin` with the face normal as its local Z.
fn frame_at(origin: Vector3, normal: Vector3) -> ManipulatorFrame {
    // Any perpendicular works: only `AxisZ` is ever compared against the ray.
    let helper = if normal.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let u = normal.cross(&helper).normalize();
    let v = normal.cross(&u).normalize();
    ManipulatorFrame {
        origin,
        basis: [u, v, normal],
    }
}

fn position(positions: &[[f32; 3]], index: u32) -> Option<Vector3> {
    let point = positions.get(index as usize)?;
    Some(Vector3::new(
        f64::from(point[0]),
        f64::from(point[1]),
        f64::from(point[2]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_geom::Tessellation;
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

    /// A square face in the `x = 0` plane, wound so its outward normal is `+X`.
    fn face() -> Tessellation {
        Tessellation {
            positions: vec![
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3],
            triangle_parts: vec![0, 0],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
        }
    }

    /// The same square wound the other way, so its outward normal is `-X`.
    fn reversed_face() -> Tessellation {
        Tessellation {
            triangles: vec![0, 2, 1, 0, 3, 2],
            ..face()
        }
    }

    /// A bent pair of triangles in one part: two normals 90° apart, not one plane.
    fn bent_face() -> Tessellation {
        Tessellation {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 1, 3, 2],
            triangle_parts: vec![0, 0],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string()],
        }
    }

    fn stage_with(mesh: &Tessellation, entity: u32) -> Stage {
        Stage::new().with_item(ShapeItem::from_tessellation(entity, mesh))
    }

    fn stem() -> PushPullStem {
        let stage = stage_with(&face(), 7);
        push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).expect("a stem")
    }

    /// A device point on the stem's tip.
    fn grab(camera: &Camera, stem: &PushPullStem) -> [i32; 2] {
        let tip = stem.readout_anchor(camera).expect("a projected tip");
        [tip[0].round() as i32, tip[1].round() as i32]
    }

    #[test]
    fn the_stem_starts_at_the_area_weighted_face_centroid() {
        let stem = stem();
        let anchor = stem.anchor().expect("an anchor");
        assert!(anchor.x.abs() < 1e-9);
        assert!((anchor.y - 0.5).abs() < 1e-9);
        assert!((anchor.z - 0.5).abs() < 1e-9);
        assert_eq!(stem.face_name(), "Face1");
        assert_eq!(stem.target_id(), 7);
        assert!(
            stem.centroid().is_equal(&anchor, 1e-12),
            "at rest the ghost is the face itself"
        );
    }

    /// The handle rides the ghost: its base is the swept face, not the face the drag
    /// started from, so it stays on (and under) the surface the drag is producing.
    #[test]
    fn the_stem_rides_the_ghost() {
        let camera = camera(5.0);
        let mut stem = stem();
        let face = stem.centroid();
        let normal = stem.normal();

        assert!(
            stem.anchor()
                .expect("an anchor")
                .is_equal(&face, 1e-12),
            "at rest the handle is on the face"
        );

        // A typed 4 mm extrusion carries the handle (and its grab target) out with it.
        assert!(Manipulator::set_values(&mut stem, &[4.0]));
        assert!(
            stem.anchor()
                .expect("an anchor")
                .is_equal(&face.add(&normal.scale(4.0)), 1e-9),
            "the handle rides the ghost: {:?}",
            stem.anchor()
        );
        assert!(stem.update(&camera));
        let tip = stem
            .readout_anchor(&camera)
            .expect("the moved tip projects");
        assert!(
            stem.hit(&camera, [tip[0].round() as i32, tip[1].round() as i32]),
            "the moved handle is grabbable"
        );

        // A pocket rides the other way.
        assert!(Manipulator::set_values(&mut stem, &[-2.0]));
        assert!(stem
            .anchor()
            .expect("an anchor")
            .is_equal(&face.sub(&normal.scale(2.0)), 1e-9));

        // A host that moves the handle (a previewed object) moves the whole sweep: the
        // distance is untouched and the handle lands exactly where it was asked for.
        let moved = stem.anchor().expect("an anchor").add(&Vector3::new(1.0, 0.0, 0.0));
        assert!(stem.set_anchor(moved));
        assert!(
            stem.anchor().expect("an anchor").is_equal(&moved, 1e-12),
            "the handle follows the host"
        );
        assert!((stem.distance() + 2.0).abs() < 1e-12, "the value is unchanged");
        assert!(
            stem.centroid().is_equal(&face.add(&Vector3::new(1.0, 0.0, 0.0)), 1e-12),
            "the sweep origin moved by the same delta"
        );
    }

    /// During a drag the handle stays under the pointer: the ghost grows by the pointer's
    /// travel and the stem is a fixed screen length past the ghost.
    #[test]
    fn a_drag_keeps_the_handle_with_the_pointer() {
        let camera = camera(5.0);
        let mut stem = stem();
        let face = stem.centroid();
        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        assert!(stem
            .drag_to(&camera, [grab[0] + 20, grab[1]])
            .is_some());

        let ghost = stem.centroid().add(&stem.normal().scale(stem.distance()));
        assert!(
            ghost.sub(&face).x > 0.1,
            "the ghost travelled outward: {}",
            ghost.sub(&face).x
        );
        assert!(
            stem.anchor()
                .expect("an anchor")
                .is_equal(&ghost, 1e-9),
            "the stem base is the ghost"
        );
    }

    #[test]
    fn the_stem_follows_the_face_normal() {
        // A face wound toward +X gets a +X stem.
        let outward = stem();
        assert!((outward.normal().x - 1.0).abs() < 1e-9);
        assert!(outward.normal().y.abs() < 1e-9);
        assert!(outward.normal().z.abs() < 1e-9);

        // The same square wound the other way is -X.
        let stage = stage_with(&reversed_face(), 7);
        let inward =
            push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).expect("a stem");
        assert!((inward.normal().x + 1.0).abs() < 1e-9);
    }

    /// A 1 × 1 × 1 box: its `+X` face is `Face1` and the rest is `Body`.
    fn box_mesh() -> Tessellation {
        Tessellation {
            positions: vec![
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6],
            triangle_parts: vec![0, 0, 1, 1],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Body".to_string()],
        }
    }

    #[test]
    fn the_numeric_twin_reports_the_body_behind_the_face() {
        let stage = stage_with(&box_mesh(), 7);
        let thick =
            push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).expect("a stem");
        assert!(
            (thick.body_depth() - 1.0).abs() < 1e-9,
            "the box is 1 mm thick behind the +X face: {}",
            thick.body_depth()
        );

        let fields = Manipulator::numeric_fields(&thick);
        assert_eq!(fields.len(), 1);
        assert!(
            fields[0].min.is_none() && fields[0].max.is_none(),
            "a distance has no hard bound: an extrusion is unbounded and a through-cut is\
             still a solid"
        );
        let hint = fields[0].hint.as_deref().expect("a geometry note");
        assert!(hint.contains("body 1.00 mm"), "{hint}");

        // A face with nothing behind it (a flat mesh) carries no note rather than a zero.
        let flat = stem();
        assert!(flat.body_depth() < 1e-9);
        assert!(Manipulator::numeric_fields(&flat)[0].hint.is_none());
    }

    #[test]
    fn the_probe_note_leads_the_numeric_rows_hint() {
        let stage = stage_with(&box_mesh(), 7);
        let mut stem =
            push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).expect("a stem");
        let before = Manipulator::numeric_fields(&stem)[0].hint.clone();

        // The document half says the selection already carries an operation; the row has
        // to say so too, ahead of the geometry note.
        assert!(Manipulator::set_note(
            &mut stem,
            Some("editing the existing extrusion".to_string())
        ));
        let hint = Manipulator::numeric_fields(&stem)[0]
            .hint
            .clone()
            .expect("the note is printed");
        assert!(hint.starts_with("editing the existing extrusion"), "{hint}");
        assert!(hint.contains("body 1.00 mm"), "the geometry note survives: {hint}");
        assert_ne!(Some(hint), before, "the note changed the row");

        // A no-op is not a change, and clearing it restores the geometry note alone.
        assert!(!Manipulator::set_note(
            &mut stem,
            Some("editing the existing extrusion".to_string())
        ));
        assert!(Manipulator::set_note(&mut stem, None));
        let hint = Manipulator::numeric_fields(&stem)[0].hint.clone();
        assert_eq!(hint.as_deref(), before.as_deref());
    }

    #[test]
    fn the_numeric_twin_sets_the_distance_and_commits_it() {
        let mut stem = stem();
        let fields = Manipulator::numeric_fields(&stem);
        assert_eq!(fields.len(), 1, "a stem has one number");
        assert_eq!(fields[0].label, "D");
        assert_eq!(fields[0].unit, "mm");
        assert_eq!(fields[0].value, 0.0, "the resting distance");
        assert!(Manipulator::pending_commit(&stem).is_none(), "zero is no edit");

        assert!(Manipulator::set_values(&mut stem, &[4.5]));
        assert!((stem.distance() - 4.5).abs() < 1e-12);
        assert!(stem.is_pending(), "a typed value is pending");
        assert!(
            !stem.preview_items()[0].positions().is_empty(),
            "the ghost follows the field"
        );
        let commit = Manipulator::pending_commit(&stem).expect("a commit");
        assert_eq!(commit.manipulator, PUSH_PULL_ID);
        assert_eq!(commit.action, "extrude");
        assert_eq!(commit.element.as_deref(), Some("Face1"));
        assert_eq!(commit.values, vec![4.5]);
        assert_eq!(commit.readout, "extrude 4.50 mm");

        // An unchanged value is not a change, and a negative one is a pocket.
        assert!(!Manipulator::set_values(&mut stem, &[4.5]));
        assert!(Manipulator::set_values(&mut stem, &[-2.0]));
        assert_eq!(
            Manipulator::pending_commit(&stem).expect("a commit").action,
            "pocket"
        );

        // Typing zero is no edit at all: the ghost goes and there is nothing to apply.
        assert!(Manipulator::set_values(&mut stem, &[0.0]));
        assert!(!stem.is_pending());
        assert!(stem.preview_items()[0].positions().is_empty());
        assert!(Manipulator::pending_commit(&stem).is_none());
    }

    #[test]
    fn a_push_pull_drag_ghosts_the_swept_extrusion() {
        let camera = camera(5.0);
        let mut stem = stem();
        assert!(
            stem.preview_items()[0].positions().is_empty(),
            "no ghost at rest"
        );

        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        let change = stem
            .drag_to(&camera, [grab[0] + 20, grab[1]])
            .expect("a change");
        assert!(change.distance.abs() > 1e-9);

        let ghost = &stem.preview_items()[0];
        let positions = ghost.positions();
        // The face's six vertices twice: the profile it starts from and the same profile
        // at the pending depth. The walls between them are what make it a volume.
        assert_eq!(positions.len(), 12, "{positions:?}");
        let plane = change.distance as f32;
        assert!(
            positions[..6].iter().all(|point| point[0].abs() < 1e-5),
            "the sweep starts on the face: {:?}",
            &positions[..6]
        );
        assert!(
            positions[6..]
                .iter()
                .all(|point| (point[0] - plane).abs() < 1e-5),
            "the cap sits on the pending plane: {:?}",
            &positions[6..]
        );
        // Two cap triangles plus one wall (two triangles) per border edge of the square.
        assert_eq!(ghost.triangles().len(), 2 * 3 + 4 * 2 * 3);

        assert!(stem.cancel_drag());
        assert!(
            stem.preview_items()[0].positions().is_empty(),
            "the cancel removes the ghost"
        );
    }

    /// A release is not a cancel: the swept ghost stays while the shell holds the commit,
    /// and the stem reports itself pending so the viewport keeps drawing it.
    #[test]
    fn a_release_keeps_the_ghost_and_reports_pending() {
        let camera = camera(5.0);
        let mut stem = stem();
        assert!(!stem.is_pending(), "nothing is pending at rest");

        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        stem.drag_to(&camera, [grab[0] + 20, grab[1]])
            .expect("a change");
        assert!(!stem.is_pending(), "a live drag is not pending");

        assert!(Manipulator::drag_end(&mut stem).is_some(), "a commit");
        assert!(!stem.is_dragging(), "the drag is over");
        assert!(stem.is_pending(), "the value is held for ✓/✗");
        assert!(
            !stem.preview_items()[0].positions().is_empty(),
            "the ghost survives the release"
        );
    }

    /// The held value can be adjusted before it is accepted: a second drag continues from
    /// the pending distance instead of restarting at zero.
    #[test]
    fn a_second_drag_adjusts_the_held_distance() {
        let camera = camera(5.0);
        let mut stem = stem();
        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        let first = stem
            .drag_to(&camera, [grab[0] + 20, grab[1]])
            .expect("a change")
            .distance;
        stem.end_drag().expect("a commit");

        assert!(stem.begin_drag(&camera, grab), "the stem is grabbable again");
        let second = stem
            .drag_to(&camera, [grab[0] + 26, grab[1]])
            .expect("a change")
            .distance;
        assert!(
            second > first,
            "the second drag grows the held value: {first} → {second}"
        );
        assert!((second - first).abs() > 1e-6);
        // Cancelling the adjustment returns to the value the drag was grabbed at.
        assert!(stem.cancel_drag());
        assert!((stem.distance() - first).abs() < 1e-9, "{first}");
    }

    #[test]
    fn dragging_along_the_normal_commits_one_extrusion() {
        let camera = camera(5.0);
        let mut stem = stem();
        let grab = grab(&camera, &stem);
        assert!(Manipulator::drag_begin(&mut stem, &camera, grab));
        assert!(Manipulator::drag_update(
            &mut stem,
            &camera,
            [grab[0] + 20, grab[1]]
        ));
        let commit = Manipulator::drag_end(&mut stem).expect("a commit");

        assert_eq!(commit.manipulator, PUSH_PULL_ID);
        assert_eq!(commit.target, 7);
        assert_eq!(commit.element.as_deref(), Some("Face1"));
        assert_eq!(commit.action, "extrude");
        assert_eq!(commit.values.len(), 1);
        assert!(
            commit.values[0] > 0.0,
            "outward is a positive distance: {}",
            commit.values[0]
        );
        assert!(commit.readout.starts_with("extrude "), "{}", commit.readout);

        // The typed change round-trips and the gesture commits exactly once.
        let change = PushPullChange::from_commit(&commit).expect("a push/pull change");
        assert_eq!(change.face, "Face1");
        assert!((change.distance - commit.values[0]).abs() < 1e-12);
        assert_eq!(change.action(), "extrude");
        assert!(Manipulator::drag_end(&mut stem).is_none());
    }

    #[test]
    fn dragging_against_the_normal_commits_one_pocket() {
        let camera = camera(5.0);
        let stage = stage_with(&reversed_face(), 7);
        let mut stem =
            push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).expect("a stem");
        let grab = grab(&camera, &stem);
        assert!(Manipulator::drag_begin(&mut stem, &camera, grab));
        // The +X face's normal points left; moving the pointer right is inward.
        assert!(Manipulator::drag_update(
            &mut stem,
            &camera,
            [grab[0] + 20, grab[1]]
        ));
        let commit = Manipulator::drag_end(&mut stem).expect("a commit");

        assert_eq!(commit.action, "pocket");
        assert!(commit.values[0] < 0.0, "inward is a negative distance");
        assert!(commit.readout.starts_with("pocket "), "{}", commit.readout);
        let change = PushPullChange::from_commit(&commit).expect("a push/pull change");
        assert_eq!(change.action(), "pocket");
    }

    #[test]
    fn a_non_planar_face_has_no_stem() {
        let stage = stage_with(&bent_face(), 7);
        assert!(push_pull_stem_for(&SelectionRef::element(7, "Face1"), &stage).is_none());
    }

    #[test]
    fn the_wrong_element_kind_has_no_stem() {
        let stage = stage_with(&face(), 7);
        assert!(
            push_pull_stem_for(&SelectionRef::element(7, "Edge1"), &stage).is_none(),
            "an edge is the dress-up arc's"
        );
        assert!(push_pull_stem_for(&SelectionRef::whole(7), &stage).is_none());
        assert!(push_pull_stem_for(&SelectionRef::element(9, "Face1"), &stage).is_none());
        assert!(push_pull_stem_for(&SelectionRef::element(7, "Face9"), &stage).is_none());
    }

    #[test]
    fn the_stem_is_a_screen_invariant_amber_ribbon() {
        let camera = camera(5.0);
        let mut stem = stem();
        assert!(stem.update(&camera), "the first update builds");
        assert!(!stem.update(&camera), "an unchanged camera is a no-op");
        assert!(!stem.items()[1].positions().is_empty());

        let color = stem.items()[1].surface_color();
        assert!((color[0] - PUSH_PULL_COLOR[0]).abs() < 1e-6);
        assert!((color[1] - PUSH_PULL_COLOR[1]).abs() < 1e-6);
        assert!((color[2] - PUSH_PULL_COLOR[2]).abs() < 1e-6);

        let anchor = stem.anchor().expect("an anchor");
        let expected = scale_for(&camera, anchor, stem.style().size);
        let tip = anchor.add(&stem.normal().scale(expected));
        let screen_anchor = project_device(&camera, anchor).expect("projected");
        let screen_tip = project_device(&camera, tip).expect("projected");
        let projected = (screen_tip[0] - screen_anchor[0]).hypot(screen_tip[1] - screen_anchor[1]);
        assert!(projected > 1.0, "the stem is not collapsed: {projected} px");
        assert!(stem.hit(&camera, [screen_tip[0] as i32, screen_tip[1] as i32]));
    }

    #[test]
    fn cancelling_restores_the_press_distance() {
        let camera = camera(5.0);
        let mut stem = stem();
        let grab = grab(&camera, &stem);
        assert!(stem.begin_drag(&camera, grab));
        assert!(stem.drag_to(&camera, [grab[0] + 20, grab[1]]).is_some());
        assert!(stem.distance() > 0.0);
        assert!(stem.cancel_drag());
        assert!(stem.distance().abs() < 1e-12);
        assert!(!stem.cancel_drag(), "nothing left to cancel");
    }

    #[test]
    fn the_readout_names_the_operation_and_the_distance() {
        let mut stem = stem();
        assert_eq!(stem.readout(), "extrude 0.00 mm");
        stem.distance = -3.5;
        assert_eq!(stem.readout(), "pocket 3.50 mm");
    }

    #[test]
    fn a_commit_for_another_manipulator_is_not_a_push_pull() {
        let commit = InteractionCommit::new("dress_up", 1, "extrude").with_element("Face1");
        assert!(PushPullChange::from_commit(&commit).is_none());
    }
}

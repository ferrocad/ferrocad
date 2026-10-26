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
//!                    ●  face centroid
//!          ┌─────────┴─────────┐
//!          │     selected      │
//!          │       face        │
//!          └───────────────────┘
//! ```
//!
//! The face frame is derived from the picked item's own tessellation: [`ShapeItem`]
//! carries per-position part ids and the part-name table, so `Face6` resolves to its
//! triangles without a kernel call. Only **planar** faces get a stem; a curved face
//! (whose triangle normals disagree) returns `None`.
//!
//! The axis drag solver, the screen-space fallback for a near view-aligned stem and
//! the `Linear` value are reused from [`ferrocad_stage::TransformGizmo`]: the stem's
//! frame has the face normal as its local Z and only the `AxisZ` grab is used. The
//! commit is neutral data:
//! `InteractionCommit::new("push_pull", entity, "extrude" | "pocket")` with the face
//! element and `[distance]`. The app script owns what that means for the document.
//!
//! This module is pure presentation: no `pyo3`, no kernel, no document.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::Camera;
use ferrocad_stage::gizmo::{
    GizmoHandle, GizmoMode, GizmoStyle, GizmoValue, ManipulatorFrame, TransformGizmo,
    MIN_HIT_RADIUS_PX, PUSH_PULL_COLOR,
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
        let mut solver = TransformGizmo::new(target_id, centroid, GizmoMode::Translate);
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
        })
    }

    /// The entity the stem targets.
    pub fn target_id(&self) -> u32 {
        self.target_id
    }

    /// The selected face's name (`Face6`).
    pub fn face_name(&self) -> &str {
        &self.face
    }

    /// The face centroid the stem starts from.
    pub fn anchor(&self) -> Option<Vector3> {
        Some(self.centroid)
    }

    /// The outward unit normal the stem follows.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The current signed distance along `+N`.
    pub fn distance(&self) -> f64 {
        self.distance
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

    /// Move the stem's world anchor (the face centroid). Returns whether it changed.
    pub fn set_anchor(&mut self, anchor: Vector3) -> bool {
        if self.centroid == anchor {
            return false;
        }
        self.centroid = anchor;
        self.solver.set_frame(frame_at(anchor, self.normal));
        self.dirty = true;
        true
    }

    /// Rebuild the stroke for `camera` when the view, the anchor or the style changed.
    /// Returns whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let changed = self.dirty
            || self.view != Some(camera.view)
            || self.built_anchor != Some(self.centroid)
            || self.built_normal != Some(self.normal);
        if !changed {
            return false;
        }
        self.view = Some(camera.view);
        self.built_anchor = Some(self.centroid);
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
            project_device(camera, self.centroid),
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
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<PushPullChange> {
        let event = self.solver.drag_to(camera, device)?;
        let GizmoValue::Linear { applied, .. } = event.value else {
            return None;
        };
        if !applied.is_finite() || (applied - self.distance).abs() <= 1e-12 {
            return None;
        }
        self.distance = applied;
        Some(self.change())
    }

    /// End the drag, returning the distance to commit.
    pub fn end_drag(&mut self) -> Option<PushPullChange> {
        self.solver.end_drag()?;
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
        true
    }

    /// Where the dimension pill is anchored, in device pixels: the reused axis
    /// readout's tip while a drag is active, and the stem tip at rest (or the centroid
    /// when the stem cannot be projected).
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        if let Some(anchor) = self.solver.readout_anchor(camera) {
            return Some(anchor);
        }
        self.tip(camera)
            .and_then(|tip| project_device(camera, tip))
            .or_else(|| project_device(camera, self.centroid))
    }

    /// The stem's world tip: a screen-invariant length along `+N`.
    fn tip(&self, camera: &Camera) -> Option<Vector3> {
        let length = scale_for(camera, self.centroid, self.style.size);
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        Some(self.centroid.add(&self.normal.scale(length)))
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
        let per_pixel = scale_for(camera, self.centroid, 1.0);
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

    fn set_hovered(&mut self, hovered: bool) -> bool {
        PushPullStem::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        PushPullStem::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
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
    let (centroid, normal) = face_frame(item, face)?;
    PushPullStem::new(selection.object, face.clone(), centroid, normal)
}

/// The Part GUI's push/pull factory: a planar-face selection arms the stem.
///
/// `ferrocad_gpui::Contributions` accepts a factory of this shape, so an edition wires
/// the manipulator in one line and the base app stays Part-free. It returns `None` for
/// an edge selection, leaving room for [`crate::fillet_manipulator`] beside it.
pub fn push_pull_manipulator(
    selection: &SelectionRef,
    stage: &Stage,
) -> Option<Box<dyn Manipulator>> {
    push_pull_stem_for(selection, stage).map(|stem| Box::new(stem) as Box<dyn Manipulator>)
}

/// The area-weighted centroid and outward unit normal of the named planar face.
///
/// The item's positions are the display tessellation (the Part display hook bakes the
/// object's `Placement` into them), and each triangle is tagged with its part id, so
/// the face is resolved by name without a kernel call. A face is planar only when every
/// triangle normal agrees with the summed normal **and** every vertex lies in that
/// plane; otherwise there is no honest direction to extrude along and `None` is
/// returned.
fn face_frame(item: &ShapeItem, name: &str) -> Option<(Vector3, Vector3)> {
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

    Some((centroid, normal))
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

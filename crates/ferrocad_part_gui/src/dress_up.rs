//! The edge dress-up scrubber: the fillet/chamfer arc manipulator (F1).
//!
//! Selecting an edge and dragging a radius is the second half of the dress-up story.
//! [`ferrocad_geom`] supplies the edge (`ElementRef`), `Part::Fillet`/`Part::Chamfer`
//! supply the parametric feature, and this module supplies the **manipulator**: a
//! screen-invariant arc drawn over the selected edge plus the drag that turns into a
//! radius.
//!
//! This is **Part-specific presentation**, so it lives beside the workbench rather than
//! in the generic stage. It implements [`Manipulator`], which means any viewport can
//! host it and the shell can route its commit without naming `Part`:
//! [`DressUpScrubber`] commits an [`InteractionCommit`] with
//! [`DRESS_UP_ID`](Self::DRESS_UP_ID) as the manipulator id, and
//! [`DressUpChange::from_commit`] turns that neutral data back into the typed value the
//! document layer writes to the feature.
//!
//! ```text
//!                  (arc, screen-space stroke)
//!                        ╭━━━━━╮
//!                      ╭─╯     ╰─╮      ← readout: `R 3.50 mm`
//!         ─────────────┴─────────┴─────────────  selected edge
//!                      anchor (edge midpoint)
//! ```
//!
//! The arc lives in the **camera-facing plane** through the edge midpoint, spanned by
//! the edge's projected direction and the screen-perpendicular. Its world radius *is*
//! the fillet radius, so the drawing is a true dimension: growing the radius grows the
//! arc. The radius tick from the anchor to the arc's apex is the grab handle, and the
//! arc plus the tick are the only pick targets — the interior between the edge and the
//! arc is transparent, so a click there still reaches the scene.
//!
//! While the scrub is live — and while the shell holds the released radius for
//! `✓`/`✗` — the scrubber publishes a translucent swath along the edge whose half-width
//! is the pending radius
//! ([`preview_items`](DressUpScrubber::preview_items)). It is display-only and unpickable;
//! the exact fillet surface waits for the commit.
//!
//! This module is pure math and geometry: no `pyo3`, no kernel, no document. The host
//! (the viewport) routes pointer events and hands the committed value to a feature.
//!
//! # Example
//!
//! ```
//! use ferrocad_part_gui::{DressUpMode, DressUpScrubber};
//! use ferrocad_types::Vector3;
//!
//! let scrubber = DressUpScrubber::new(
//!     7,
//!     DressUpMode::Fillet,
//!     "Edge3",
//!     vec![Vector3::new(0.0, 0.0, 0.0), Vector3::new(10.0, 0.0, 0.0)],
//! )
//! .with_value(2.5);
//! assert_eq!(scrubber.readout(), "R 2.50 mm");
//! ```

use std::f64::consts::PI;

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::Camera;
use ferrocad_stage::gizmo::{GizmoStyle, MIN_HIT_RADIUS_PX};
use ferrocad_stage::screen::{
    camera_forward, point_segment_distance, project_device, push_ribbon_uv, scale_for,
};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, ShapeItem, Stage};
use ferrocad_types::{Matrix4, Vector3};

/// The scrubber's registry id (`InteractionCommit::manipulator`).
pub const DRESS_UP_ID: &str = "dress_up";

/// The scrubber's fixed picking id. It is drawn as an overlay (not picked through the
/// buffer), so this only has to stay clear of document and gizmo ids.
pub const DRESS_UP_PICK_ID: u32 = 2_000_000;

/// Segments in the arc stroke and its screen-space hit polyline.
const ARC_SEGMENTS: usize = 32;
/// The minimum on-screen length (device px) of the edge direction; below it the edge
/// points at the camera and the screen-perpendicular fallback is used.
const MIN_EDGE_PIXELS: f64 = 4.0;
/// The smallest radius a scrub can produce: a positive number, so a kernel operation is
/// always valid.
pub const MIN_RADIUS: f64 = 1e-3;

/// Which dress-up operation the scrubber edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DressUpMode {
    /// A rounded edge; the value is the radius.
    Fillet,
    /// A bevel; the value is the first distance.
    Chamfer,
}

/// The typed numeric value of a dress-up scrub, for the host's readout and the feature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DressUpValue {
    /// `radius` is the fillet radius, in model units.
    Fillet { radius: f64 },
    /// `first` is `Distance`; `second` is `Distance2` (`0` means symmetric).
    Chamfer { first: f64, second: f64 },
}

impl DressUpValue {
    /// The value the scrub edits (the radius, or the first chamfer distance).
    pub fn primary(self) -> f64 {
        match self {
            Self::Fillet { radius } => radius,
            Self::Chamfer { first, .. } => first,
        }
    }

    /// A compact tag for the projected dimension pill: `R 3.50 mm`,
    /// `2.00 × 1.00 mm`, or `2.00 mm` for a symmetric chamfer.
    pub fn readout(self) -> String {
        match self {
            Self::Fillet { radius } => format!("R {radius:.2} mm"),
            Self::Chamfer { first, second } if second > 0.0 => {
                format!("{first:.2} × {second:.2} mm")
            }
            Self::Chamfer { first, .. } => format!("{first:.2} mm"),
        }
    }
}

/// One scrub tick or commit: the new value and what it applies to.
#[derive(Clone, Debug, PartialEq)]
pub struct DressUpChange {
    /// The entity the feature belongs to.
    pub target_id: u32,
    /// The selected edge name (`Edge3`).
    pub edge: String,
    /// The value to write to the feature's `Radius`/`Distance` property.
    pub value: DressUpValue,
}

/// The in-progress scrub: the pointer radius at press and the value it started from.
///
/// Scrubbing is a **delta** in screen radius, not an absolute projection, so grabbing
/// the arc anywhere (not just its apex) does not jump the value.
#[derive(Clone, Copy, Debug)]
struct DragState {
    press_pixels: f64,
    press_value: f64,
}

/// An edge-hugging fillet/chamfer radius scrubber.
///
/// The host builds one per selected edge, draws [`items`](Self::items) as an overlay,
/// routes the pointer through [`hit`](Self::hit)/[`begin_drag`](Self::begin_drag)/
/// [`drag_to`](Self::drag_to)/[`end_drag`](Self::end_drag), and writes the committed
/// [`DressUpValue`] to the feature.
#[derive(Clone, Debug)]
pub struct DressUpScrubber {
    target_id: u32,
    edge: String,
    /// The edge's polyline, in world space (from the display tessellation).
    points: Vec<Vector3>,
    mode: DressUpMode,
    value: f64,
    /// The chamfer's second distance; `0` means symmetric and is unused by a fillet.
    second: f64,
    /// An optional absolute snap step for the value.
    step: Option<f64>,
    /// The lower bound (`MIN_RADIUS` by default).
    min: f64,
    /// The upper bound, if any.
    max: Option<f64>,
    hovered: bool,
    active: Option<DragState>,
    /// The workspace appearance.
    style: GizmoStyle,
    /// The camera view and value the cached stroke was built for.
    view: Option<Matrix4>,
    built: Option<f64>,
    dirty: bool,
    /// The draw order: the halo (a wider arc in the outline ink) first, the stroke on top.
    items: [ShapeItem; 2],
    /// The pending-radius swath along the edge, shown while a drag is active and after a
    /// release, while the shell holds the commit.
    preview: ShapeItem,
    /// Whether the released radius is held for the host's `✓`/`✗`.
    pending: bool,
}

impl DressUpScrubber {
    /// A scrubber for `edge` on `target_id`, with `points` sampled along the edge.
    ///
    /// An empty or one-point polyline cannot place an arc; the scrubber then reports no
    /// anchor, no hit and no drag, but keeps its value so the host can still use it as a
    /// numeric editor.
    pub fn new(
        target_id: u32,
        mode: DressUpMode,
        edge: impl Into<String>,
        points: Vec<Vector3>,
    ) -> Self {
        let style = GizmoStyle::default();
        let ghost_alpha = style.ghost_alpha;
        Self {
            target_id,
            edge: edge.into(),
            points,
            mode,
            value: 1.0,
            second: 0.0,
            step: None,
            min: MIN_RADIUS,
            max: None,
            hovered: false,
            active: None,
            pending: false,
            style,
            view: None,
            built: None,
            dirty: true,
            items: [
                ShapeItem::new(DRESS_UP_PICK_ID + 1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_blended_surface()
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
                ShapeItem::new(DRESS_UP_PICK_ID)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
            ],
            preview: ShapeItem::new(DRESS_UP_PICK_ID + 2)
                .with_draw_mode(DrawMode::Shaded)
                .with_selectable(false)
                .with_blended_surface()
                .with_ribbon_aa(),
        }
    }

    /// The builder form of [`set_value`](Self::set_value).
    pub fn with_value(mut self, value: f64) -> Self {
        self.set_value(value);
        self
    }

    /// The builder form of [`set_second`](Self::set_second).
    pub fn with_second(mut self, second: f64) -> Self {
        self.second = second;
        self
    }

    /// The builder form of [`set_step`](Self::set_step).
    pub fn with_step(mut self, step: Option<f64>) -> Self {
        self.step = step;
        self
    }

    /// The builder form of [`set_bounds`](Self::set_bounds).
    pub fn with_bounds(mut self, min: f64, max: Option<f64>) -> Self {
        self.min = min.max(MIN_RADIUS);
        self.max = max;
        self
    }

    /// The entity the scrubber targets.
    pub fn target_id(&self) -> u32 {
        self.target_id
    }

    /// The selected edge's name.
    pub fn edge_name(&self) -> &str {
        &self.edge
    }

    /// The mode being edited.
    pub fn mode(&self) -> DressUpMode {
        self.mode
    }

    /// The current primary value (radius or first distance).
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The current chamfer second distance (`0` when symmetric).
    pub fn second(&self) -> f64 {
        self.second
    }

    /// Open at the value an existing dress-up feature holds.
    ///
    /// The arc moves to that radius and the row shows it, but nothing is pending: only a
    /// drag or a typed number makes a commit. A handle the user is already editing keeps
    /// its value.
    pub fn seed_values(&mut self, values: &[f64]) -> bool {
        if self.pending || self.is_dragging() {
            return false;
        }
        let Some(first) = values.first().copied() else {
            return false;
        };
        if !first.is_finite() {
            return false;
        }
        let before = (self.value, self.second);
        self.set_value(first);
        if self.mode == DressUpMode::Chamfer
            && let Some(second) = values.get(1).copied()
            && second.is_finite()
        {
            self.set_second(second);
        }
        (self.value, self.second) != before
    }

    /// The typed value, ready for a feature property or a readout.
    pub fn typed_value(&self) -> DressUpValue {
        match self.mode {
            DressUpMode::Fillet => DressUpValue::Fillet { radius: self.value },
            DressUpMode::Chamfer => DressUpValue::Chamfer {
                first: self.value,
                second: self.second,
            },
        }
    }

    /// The dimension pill text (`R 3.50 mm`).
    pub fn readout(&self) -> String {
        self.typed_value().readout()
    }

    /// The change the current value describes.
    pub fn change(&self) -> DressUpChange {
        DressUpChange {
            target_id: self.target_id,
            edge: self.edge.clone(),
            value: self.typed_value(),
        }
    }

    /// Set the value from **typed** numbers — the numeric twin of the scrub.
    ///
    /// The first number is the radius (or the chamfer's first distance); in chamfer mode
    /// the second is its second distance. Both are clamped and snapped with the same
    /// rules a drag uses, and a change marks the value pending so the swath stays visible
    /// until `✓` or `✗`.
    pub fn set_values(&mut self, values: &[f64]) -> bool {
        let Some(first) = values.first().copied() else {
            return false;
        };
        if !first.is_finite() {
            return false;
        }
        let before = (self.value, self.second);
        self.set_value(first);
        if self.mode == DressUpMode::Chamfer
            && let Some(second) = values.get(1).copied()
            && second.is_finite()
        {
            self.set_second(second);
        }
        if (self.value, self.second) == before {
            return false;
        }
        self.pending = true;
        self.dirty = true;
        true
    }

    /// Replace the primary value (clamped and snapped like a drag tick).
    pub fn set_value(&mut self, value: f64) {
        let value = self.clamp(value);
        if value != self.value {
            self.value = value;
            self.dirty = true;
        }
    }

    /// Set the chamfer's second distance (`0` for symmetric).
    pub fn set_second(&mut self, second: f64) {
        self.second = second.max(0.0);
        self.dirty = true;
    }

    /// Set the snap step (`None` for free scrubbing).
    pub fn set_step(&mut self, step: Option<f64>) {
        self.step = step.filter(|step| *step > 0.0);
    }

    /// Set the allowed range. `min` is raised to [`MIN_RADIUS`].
    pub fn set_bounds(&mut self, min: f64, max: Option<f64>) {
        self.min = min.max(MIN_RADIUS);
        self.max = max;
        self.value = self.clamp(self.value);
        self.dirty = true;
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// Whether the released radius is held for the host's `✓`/`✗`.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// Whether the arc is highlighted.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The overlay items, halo first so the stroke sits on top of it.
    pub fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    /// The display-only swath of the pending radius, shown only while a drag is active.
    pub fn preview_items(&self) -> &[ShapeItem] {
        std::slice::from_ref(&self.preview)
    }

    /// Highlight (or un-highlight) the arc. Returns whether the visuals changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.dirty = true;
        self.apply_color();
        true
    }

    /// The current stroke colour: active while hovered or dragging, slate otherwise.
    fn stroke_color(&self) -> [f32; 4] {
        let token = if self.hovered || self.is_dragging() {
            self.style.active
        } else {
            self.style.inactive
        };
        [
            token[0],
            token[1],
            token[2],
            token[3] * self.style.opacity,
        ]
    }

    /// Apply the workspace gizmo style. Returns whether the style changed.
    ///
    /// The ribbon width is baked into the cached geometry, so a change marks the arc
    /// dirty and the next [`update`](Self::update) rebuilds it. The default style
    /// reproduces the legacy 1.5 px arc exactly.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        if self.style == *style {
            return false;
        }
        self.style = style.clone();
        self.dirty = true;
        self.apply_color();
        true
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        &self.style
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
        let preview = [
            self.style.active[0],
            self.style.active[1],
            self.style.active[2],
            0.30,
        ];
        self.preview.set_surface_color(preview);
        self.preview.set_edge_color(preview);
    }

    /// The world point the arc is centred on: the midpoint of the edge's middle segment.
    pub fn anchor(&self) -> Option<Vector3> {
        match self.points.len() {
            0 => None,
            1 => Some(self.points[0]),
            n => {
                let index = (n - 1) / 2;
                Some(self.points[index].add(&self.points[index + 1]).scale(0.5))
            }
        }
    }

    /// The anchor's device-pixel position, if it projects. A host uses this to place a
    /// badge or a screen-space companion near the edge.
    pub fn screen_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.anchor()?)
    }

    /// The camera-facing arc basis: `(u, n)`, where `u` is the edge's projected
    /// direction and `n` the in-plane perpendicular.
    fn arc_basis(&self, camera: &Camera) -> Option<(Vector3, Vector3)> {
        let forward = camera_forward(camera);
        let mut u = None;
        if self.points.len() >= 2 {
            let index = (self.points.len() - 1) / 2;
            let from = self.points[index];
            let to = self.points[index + 1];
            let direction = to.sub(&from);
            // An edge seen end-on projects to a dot: its orientation is noise, so the
            // stable screen fallback takes over. An edge with no projected length at
            // all (behind the eye) falls back too.
            let long_enough = match (project_device(camera, from), project_device(camera, to)) {
                (Some(a), Some(b)) => (b[0] - a[0]).hypot(b[1] - a[1]) >= MIN_EDGE_PIXELS,
                _ => false,
            };
            if long_enough && direction.length() > 1e-12 {
                let projected = direction.sub(&forward.scale(direction.dot(&forward)));
                if projected.length() > 1e-9 {
                    u = Some(projected.normalize());
                }
            }
        }
        let u = u.unwrap_or_else(|| fallback_u(forward));
        let n = forward.cross(&u).normalize();
        Some((u, n))
    }

    /// The arc's world points, from one end of the edge's projected direction to the
    /// other, bulging along the screen-perpendicular.
    fn arc_points(&self, camera: &Camera) -> Vec<Vector3> {
        let (Some(anchor), Some((u, n))) = (self.anchor(), self.arc_basis(camera)) else {
            return Vec::new();
        };
        let radius = self.value.max(0.0);
        (0..=ARC_SEGMENTS)
            .map(|i| {
                let angle = PI * i as f64 / ARC_SEGMENTS as f64;
                anchor
                    .add(&u.scale(radius * angle.cos()))
                    .add(&n.scale(radius * angle.sin()))
            })
            .collect()
    }

    /// Rebuild the stroke for `camera` when the view or the value changed. Returns
    /// whether the geometry was rebuilt.
    pub fn update(&mut self, camera: &Camera) -> bool {
        let changed =
            self.dirty || self.view != Some(camera.view) || self.built != Some(self.value);
        if !changed {
            return false;
        }
        self.view = Some(camera.view);
        self.built = Some(self.value);
        self.dirty = false;
        self.build(camera);
        true
    }

    /// Rebuild the stroke geometry: the arc and its radius tick, with a wider halo pass
    /// behind them when the style enables one.
    fn build(&mut self, camera: &Camera) {
        let Some(anchor) = self.anchor() else {
            for item in &mut self.items {
                item.set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            }
            self.preview
                .set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            return;
        };
        let forward = camera_forward(camera);
        let per_pixel = scale_for(camera, anchor, 1.0);
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
            let points = self.arc_points(camera);
            for pair in points.windows(2) {
                push_ribbon_uv(
                    &mut positions,
                    &mut uvs,
                    &mut triangles,
                    ferrocad_graphics::screen_ribbon(pair[0], pair[1], half, forward),
                );
            }
            // The radius tick: anchor → arc apex. This is the dimension's "R" line and
            // the most natural place to grab, so it is part of the stroke.
            if let Some((_, n)) = self.arc_basis(camera) {
                let apex = anchor.add(&n.scale(self.value.max(0.0)));
                push_ribbon_uv(
                    &mut positions,
                    &mut uvs,
                    &mut triangles,
                    ferrocad_graphics::screen_ribbon(anchor, apex, half, forward),
                );
            }
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
        self.build_preview(camera);
    }

    /// Build the pending-radius swath: a screen-facing ribbon along the edge whose
    /// half-width is the current value. It is a property preview, not the recomputed
    /// fillet surface — the exact shape waits for the commit.
    fn build_preview(&mut self, camera: &Camera) {
        let forward = camera_forward(camera);
        let half = self.value.max(0.0);
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut triangles = Vec::new();
        for pair in self.points.windows(2) {
            push_ribbon_uv(
                &mut positions,
                &mut uvs,
                &mut triangles,
                ferrocad_graphics::screen_ribbon(pair[0], pair[1], half, forward),
            );
        }
        self.preview.set_shape_uv(
            positions,
            Vec::new(),
            uvs,
            triangles,
            Vec::new(),
        );
        self.apply_color();
    }

    /// Whether the pointer is on the arc or its radius tick, within the style's grab
    /// radius. The area between the edge and the arc is **not** part of the target, so it
    /// passes through to the scene.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let points: Vec<[f64; 2]> = self
            .arc_points(camera)
            .iter()
            .filter_map(|point| project_device(camera, *point))
            .collect();
        let mut nearest = f64::INFINITY;
        for pair in points.windows(2) {
            let (distance, _) = point_segment_distance(pointer, pair[0], pair[1]);
            nearest = nearest.min(distance);
        }
        // The tick is the arc's radius line; include it (its inner end shares the
        // anchor with the edge, which is exactly where a user grabs).
        if let (Some(anchor), Some((_, n))) = (self.anchor(), self.arc_basis(camera)) {
            let apex = anchor.add(&n.scale(self.value.max(0.0)));
            if let (Some(a), Some(b)) =
                (project_device(camera, anchor), project_device(camera, apex))
            {
                let (distance, _) = point_segment_distance(pointer, a, b);
                nearest = nearest.min(distance);
            }
        }
        nearest <= self.style.hit_radius_px.max(MIN_HIT_RADIUS_PX)
    }

    /// Begin a scrub at `device`. The press is recorded relative to the anchor, so the
    /// value only moves by the pointer's radial travel.
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        if !self.hit(camera, device) {
            return false;
        }
        let Some(anchor) = self
            .anchor()
            .and_then(|anchor| project_device(camera, anchor))
        else {
            return false;
        };
        let press_pixels =
            (f64::from(device[0]) - anchor[0]).hypot(f64::from(device[1]) - anchor[1]);
        self.active = Some(DragState {
            press_pixels,
            press_value: self.value,
        });
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Advance the scrub to `device`, returning the new value when it changed.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<DressUpChange> {
        let drag = self.active?;
        let anchor = self.anchor()?;
        let screen = project_device(camera, anchor)?;
        let radius = (f64::from(device[0]) - screen[0]).hypot(f64::from(device[1]) - screen[1]);
        let world_per_pixel = scale_for(camera, anchor, 1.0).max(1e-12);
        let raw = drag.press_value + (radius - drag.press_pixels) * world_per_pixel;
        let value = self.clamp(self.snap(raw));
        if (value - self.value).abs() <= 1e-12 {
            return None;
        }
        self.value = value;
        self.dirty = true;
        Some(self.change())
    }

    /// End the scrub, returning the value to commit.
    pub fn end_drag(&mut self) -> Option<DressUpChange> {
        self.active.take()?;
        // The release hands the value over; the swath stays until the host spends it
        // (`✓`) or drops it (`✗` → the shell re-arms a fresh scrubber).
        self.pending = true;
        self.dirty = true;
        self.apply_color();
        Some(self.change())
    }

    /// Cancel the scrub, restoring the value it started from. Returns whether a drag
    /// was active.
    pub fn cancel_drag(&mut self) -> bool {
        let Some(drag) = self.active.take() else {
            return false;
        };
        if (self.value - drag.press_value).abs() > 1e-12 {
            self.value = drag.press_value;
        }
        self.dirty = true;
        self.apply_color();
        true
    }

    /// Where the dimension pill is anchored, in device pixels: the arc's apex (or the
    /// anchor when the arc is degenerate).
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        let anchor = self.anchor()?;
        let apex = match self.arc_basis(camera) {
            Some((_, n)) => anchor.add(&n.scale(self.value.max(0.0))),
            None => anchor,
        };
        project_device(camera, apex).or_else(|| project_device(camera, anchor))
    }

    fn snap(&self, value: f64) -> f64 {
        match self.step {
            Some(step) if step > 0.0 => (value / step).round() * step,
            _ => value,
        }
    }

    fn clamp(&self, value: f64) -> f64 {
        let value = if value.is_finite() { value } else { self.min };
        let value = value.max(self.min);
        match self.max {
            Some(max) => value.min(max.max(self.min)),
            None => value,
        }
    }
}

impl DressUpChange {
    /// The neutral commit that carries this change.
    pub fn into_commit(self) -> InteractionCommit {
        let action = match self.value {
            DressUpValue::Fillet { .. } => "fillet",
            DressUpValue::Chamfer { .. } => "chamfer",
        };
        let values = match self.value {
            DressUpValue::Fillet { radius } => vec![radius],
            DressUpValue::Chamfer { first, second } => vec![first, second],
        };
        InteractionCommit::new(DRESS_UP_ID, self.target_id, action)
            .with_element(self.edge)
            .with_values(values)
            .with_readout(self.value.readout())
    }

    /// The typed change a dress-up commit carries, if it is one.
    ///
    /// This is the inverse of [`Manipulator::drag_end`]: the neutral commit names the
    /// operation and orders the numbers; the document layer wants the enum.
    pub fn from_commit(commit: &InteractionCommit) -> Option<Self> {
        if commit.manipulator != DRESS_UP_ID {
            return None;
        }
        let edge = commit.element.clone()?;
        let value = match commit.action.as_str() {
            "fillet" => DressUpValue::Fillet {
                radius: *commit.values.first()?,
            },
            "chamfer" => DressUpValue::Chamfer {
                first: *commit.values.first()?,
                second: commit.values.get(1).copied().unwrap_or(0.0),
            },
            _ => return None,
        };
        Some(Self {
            target_id: commit.target,
            edge,
            value,
        })
    }
}

impl Manipulator for DressUpScrubber {
    fn id(&self) -> &str {
        DRESS_UP_ID
    }

    fn items(&self) -> &[ShapeItem] {
        DressUpScrubber::items(self)
    }

    fn preview_items(&self) -> &[ShapeItem] {
        DressUpScrubber::preview_items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        DressUpScrubber::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        DressUpScrubber::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        DressUpScrubber::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        DressUpScrubber::end_drag(self).map(DressUpChange::into_commit)
    }

    fn drag_cancel(&mut self) -> bool {
        DressUpScrubber::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        DressUpScrubber::is_dragging(self)
    }

    fn is_pending(&self) -> bool {
        DressUpScrubber::is_pending(self)
    }

    fn numeric_fields(&self) -> Vec<ferrocad_stage::NumericField> {
        let min = Some(self.min);
        // Three decimals, because the floor is `MIN_RADIUS` (0.001 mm) and a two-decimal
        // field could not show it.
        match self.mode {
            DressUpMode::Fillet => vec![
                ferrocad_stage::NumericField::millimetres("R", self.value)
                    .decimals(3)
                    .bounded(min, self.max),
            ],
            DressUpMode::Chamfer => vec![
                ferrocad_stage::NumericField::millimetres("D1", self.value)
                    .decimals(3)
                    .bounded(min, self.max),
                ferrocad_stage::NumericField::millimetres("D2", self.second)
                    .decimals(3)
                    .bounded(min, self.max),
            ],
        }
    }

    fn set_values(&mut self, values: &[f64]) -> bool {
        DressUpScrubber::set_values(self, values)
    }

    fn seed_values(&mut self, values: &[f64]) -> bool {
        DressUpScrubber::seed_values(self, values)
    }

    /// Take the kernel's range: it can be tighter than the geometric ceiling, and it can
    /// raise the floor above the scrub's own minimum.
    fn set_value_bounds(&mut self, min: Option<f64>, max: Option<f64>) -> bool {
        let min = min.map_or(self.min, |min| min.max(MIN_RADIUS));
        if self.min == min && self.max == max {
            return false;
        }
        DressUpScrubber::set_bounds(self, min, max);
        true
    }

    fn pending_commit(&self) -> Option<InteractionCommit> {
        Some(self.change().into_commit())
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        DressUpScrubber::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        DressUpScrubber::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn anchor_device(&self, camera: &Camera) -> Option<[f64; 2]> {
        // The arc turns about its anchor, and the scrub measures the pointer's distance
        // from it: this is the direction a probe has to pull along.
        self.anchor()
            .and_then(|anchor| ferrocad_stage::screen::project_device(camera, anchor))
    }

    fn readout(&self) -> Option<String> {
        Some(DressUpScrubber::readout(self))
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        DressUpScrubber::readout_anchor(self, camera)
    }
}

/// The scrubber a selection asks for, when it names an **edge** of a displayed item.
///
/// This is the presentation half of the edge manipulator: a workbench registers it as
/// a manipulator factory and the host calls it on every selection. A face or vertex
/// selection, an unknown entity or a mesh without a wire run returns `None`.
pub fn dress_up_scrubber_for(
    selection: &SelectionRef,
    stage: &Stage,
    mode: DressUpMode,
) -> Option<DressUpScrubber> {
    let edge = selection.elements.first()?;
    if !edge.starts_with("Edge") {
        return None;
    }
    let points = stage.item(selection.object)?.edge_polyline(edge)?;
    if points.len() < 2 {
        return None;
    }
    let points = points
        .into_iter()
        .map(|point| {
            Vector3::new(
                f64::from(point[0]),
                f64::from(point[1]),
                f64::from(point[2]),
            )
        })
        .collect();
    let mut scrubber = DressUpScrubber::new(selection.object, mode, edge.clone(), points);
    // No ceiling from the tessellation: half the body's thinnest extent is not a bound
    // (a single edge of a cube takes a fillet larger than that), and a bound that is not
    // true is worse than none. The kernel answers the ceiling; until it does, only the
    // scrub's own floor applies.
    scrubber.set_bounds(MIN_RADIUS, None);
    Some(scrubber)
}

/// The Part GUI's manipulator factory: an edge selection arms a fillet scrubber.
///
/// `ferrocad_shell_python::Contributions` accepts a factory of this shape, so an edition wires
/// the manipulator in one line and the base app stays Part-free.
pub fn fillet_manipulator(selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>> {
    dress_up_scrubber_for(selection, stage, DressUpMode::Fillet)
        .map(|scrubber| Box::new(scrubber) as Box<dyn Manipulator>)
}

/// A stable screen-horizontal in the camera-facing plane, used when the edge projects
/// too short to define a direction.
fn fallback_u(forward: Vector3) -> Vector3 {
    let helper = if forward.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    forward.cross(&helper).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_types::{Placement, Rotation};

    /// A perspective camera at `(0, 0, -eye_z)` looking toward +Z.
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

    /// An edge in the `z = 0` plane, spanning x = -0.5..0.5, seen face-on.
    fn edge() -> Vec<Vector3> {
        vec![Vector3::new(-0.5, 0.0, 0.0), Vector3::new(0.5, 0.0, 0.0)]
    }

    fn scrubber(mode: DressUpMode) -> DressUpScrubber {
        DressUpScrubber::new(7, mode, "Edge1", edge()).with_value(0.25)
    }

    /// The apex's device position and the outward unit direction from the anchor.
    fn grip(camera: &Camera, scrubber: &DressUpScrubber) -> ([i32; 2], [f64; 2]) {
        use ferrocad_stage::screen::project_device;

        let anchor = scrubber.anchor().expect("an anchor");
        let anchor_px = project_device(camera, anchor).expect("a projected anchor");
        let (_, normal) = scrubber.arc_basis(camera).expect("an arc basis");
        let apex = project_device(camera, anchor.add(&normal.scale(scrubber.value())))
            .expect("a projected apex");
        let (dx, dy) = (apex[0] - anchor_px[0], apex[1] - anchor_px[1]);
        let length = dx.hypot(dy).max(1e-9);
        (
            [apex[0].round() as i32, apex[1].round() as i32],
            [dx / length, dy / length],
        )
    }

    /// The band's cross-width in the test camera (the edge runs along X, so the swath
    /// widens along Y).
    fn band_width(item: &ShapeItem) -> f64 {
        let ys: Vec<f64> = item.positions().iter().map(|point| f64::from(point[1])).collect();
        match (
            ys.iter().cloned().reduce(f64::min),
            ys.iter().cloned().reduce(f64::max),
        ) {
            (Some(min), Some(max)) => max - min,
            _ => 0.0,
        }
    }

    #[test]
    fn a_dress_up_drag_ghosts_the_swath_at_the_pending_radius() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(scrubber.update(&camera));
        let narrow = band_width(&scrubber.preview_items()[0]);
        assert!(narrow > 0.0, "the swath is drawn at the current radius");

        scrubber.set_value(1.0);
        assert!(scrubber.update(&camera));
        let wide = band_width(&scrubber.preview_items()[0]);
        assert!(
            wide > narrow,
            "the swath follows the radius: {narrow} -> {wide}"
        );

        // A drag changes the value; the host's refresh rebuilds the swath at it.
        let (grab, direction) = grip(&camera, &scrubber);
        assert!(scrubber.begin_drag(&camera, grab));
        let far = [
            grab[0] + (direction[0] * 40.0) as i32,
            grab[1] + (direction[1] * 40.0) as i32,
        ];
        let before = scrubber.value();
        assert!(scrubber.drag_to(&camera, far).is_some(), "the scrub changes");
        assert!(scrubber.value() > before, "the drag grows the value");
        assert!(scrubber.update(&camera));
        assert!(band_width(&scrubber.preview_items()[0]) > 0.0);
    }

    #[test]
    fn the_numeric_twin_sets_the_radius_and_the_chamfer_distances() {
        let mut fillet = scrubber(DressUpMode::Fillet);
        let fields = Manipulator::numeric_fields(&fillet);
        assert_eq!(fields.len(), 1, "a fillet has one radius");
        assert_eq!(fields[0].label, "R");
        assert_eq!(fields[0].unit, "mm");
        assert_eq!(fields[0].value, 0.25);
        let commit = Manipulator::pending_commit(&fillet).expect("a commit");
        assert_eq!(commit.action, "fillet");
        assert_eq!(commit.values, vec![0.25]);

        assert!(Manipulator::set_values(&mut fillet, &[2.0]));
        assert!(fillet.is_pending());
        assert!((fillet.value() - 2.0).abs() < 1e-12);
        assert_eq!(
            Manipulator::pending_commit(&fillet).expect("a commit").readout,
            "R 2.00 mm"
        );

        // The scrub's own clamp applies: a negative radius lands on the minimum.
        assert!(Manipulator::set_values(&mut fillet, &[-1.0]));
        assert!(fillet.value() > 0.0);

        let mut chamfer = scrubber(DressUpMode::Chamfer);
        let fields = Manipulator::numeric_fields(&chamfer);
        assert_eq!(fields.len(), 2, "a chamfer has two distances");
        assert_eq!(fields[1].label, "D2");
        assert!(Manipulator::set_values(&mut chamfer, &[1.5, 0.5]));
        assert!((chamfer.second() - 0.5).abs() < 1e-12);
        let commit = Manipulator::pending_commit(&chamfer).expect("a commit");
        assert_eq!(commit.action, "chamfer");
        assert_eq!(commit.values, vec![1.5, 0.5]);
    }

    /// A release is not a cancel: the swath stays at the pending radius and the scrubber
    /// reports itself pending, so the viewport keeps drawing it until `✓`/`✗`.
    #[test]
    fn a_release_keeps_the_swath_and_reports_pending() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(!scrubber.is_pending(), "nothing is pending at rest");

        let (grab, direction) = grip(&camera, &scrubber);
        assert!(scrubber.begin_drag(&camera, grab));
        let far = [
            grab[0] + (direction[0] * 40.0) as i32,
            grab[1] + (direction[1] * 40.0) as i32,
        ];
        assert!(scrubber.drag_to(&camera, far).is_some(), "the scrub changes");
        assert!(!scrubber.is_pending(), "a live scrub is not pending");
        assert!(scrubber.update(&camera), "the swath follows the scrub");

        let radius = scrubber.value();
        assert!(Manipulator::drag_end(&mut scrubber).is_some(), "a commit");
        assert!(!scrubber.is_dragging(), "the scrub is over");
        assert!(scrubber.is_pending(), "the radius is held for ✓/✗");
        assert!((scrubber.value() - radius).abs() < 1e-12);
        assert!(
            band_width(&scrubber.preview_items()[0]) > 0.0,
            "the swath is still built at the held radius"
        );
    }

    #[test]
    fn the_readout_is_a_dimension_pill() {
        let fillet = scrubber(DressUpMode::Fillet);
        assert_eq!(fillet.readout(), "R 0.25 mm");

        let symmetric = scrubber(DressUpMode::Chamfer);
        assert_eq!(symmetric.readout(), "0.25 mm");

        let asymmetric = scrubber(DressUpMode::Chamfer).with_second(0.5);
        assert_eq!(asymmetric.readout(), "0.25 × 0.50 mm");
        assert_eq!(
            asymmetric.typed_value(),
            DressUpValue::Chamfer {
                first: 0.25,
                second: 0.5
            }
        );
    }

    #[test]
    fn the_anchor_is_the_edge_midpoint() {
        let scrubber = scrubber(DressUpMode::Fillet);
        let anchor = scrubber.anchor().expect("an anchor");
        assert!((anchor.x).abs() < 1e-12);
        assert!((anchor.y).abs() < 1e-12);
        assert!((anchor.z).abs() < 1e-12);
    }

    #[test]
    fn the_arc_is_centred_on_the_anchor_and_sized_by_the_value() {
        let camera = camera(5.0);
        let scrubber = scrubber(DressUpMode::Fillet);
        let points = scrubber.arc_points(&camera);
        assert_eq!(points.len(), ARC_SEGMENTS + 1);

        // Every arc point is the value's radius from the anchor (the arc lies in a
        // plane through it, so the 3D distance is the radius).
        for point in &points {
            let radius = point.sub(&Vector3::zero()).length();
            assert!(
                (radius - 0.25).abs() < 1e-9,
                "arc point at {radius}, expected 0.25"
            );
        }
        // The ends lie along the edge direction, the apex along the perpendicular.
        assert!(points[0].x > 0.0, "angle 0 is along +u");
        assert!(points[ARC_SEGMENTS].x < 0.0, "angle π is along -u");
        assert!((points[ARC_SEGMENTS / 2].y.abs() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn the_arc_hit_target_is_the_stroke_not_the_interior() {
        use ferrocad_stage::screen::project_device;

        let camera = camera(5.0);
        let scrubber =
            DressUpScrubber::new(7, DressUpMode::Fillet, "Edge1", edge()).with_value(2.0);

        // The anchor and the apex are on the stroke and grabbable.
        let anchor = scrubber.anchor().unwrap();
        let at = |point: Vector3| {
            let device = project_device(&camera, point).expect("projected");
            [device[0].round() as i32, device[1].round() as i32]
        };
        assert!(
            scrubber.hit(&camera, at(anchor)),
            "the anchor is on the tick"
        );
        let apex = scrubber.readout_anchor(&camera).expect("an apex anchor");
        assert!(scrubber.hit(&camera, [apex[0].round() as i32, apex[1].round() as i32]));

        // Halfway from the anchor to the arc's 45° point is inside the half-disc but
        // clear of both the stroke and the tick.
        let arc = scrubber.arc_points(&camera);
        let diagonal = at(arc[ARC_SEGMENTS / 4]);
        let anchor = at(anchor);
        let interior = [
            ((anchor[0] + diagonal[0]) as f64 * 0.5).round() as i32,
            ((anchor[1] + diagonal[1]) as f64 * 0.5).round() as i32,
        ];
        assert!(
            !scrubber.hit(&camera, interior),
            "the interior between the edge and the arc passes through"
        );
    }

    #[test]
    fn scrubbing_outwards_increases_the_value() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        let (grab, outward) = grip(&camera, &scrubber);
        assert!(scrubber.begin_drag(&camera, grab));
        let event = scrubber
            .drag_to(
                &camera,
                [
                    (f64::from(grab[0]) + outward[0] * 10.0).round() as i32,
                    (f64::from(grab[1]) + outward[1] * 10.0).round() as i32,
                ],
            )
            .expect("a change");
        assert_eq!(event.target_id, 7);
        assert_eq!(event.edge, "Edge1");
        assert!(event.value.primary() > 0.25, "outwards grows the radius");
        assert!(scrubber.value() > 0.25);

        // Inwards shrinks it, but never below the minimum.
        scrubber.drag_to(
            &camera,
            [
                (f64::from(grab[0]) - outward[0] * 1000.0).round() as i32,
                (f64::from(grab[1]) - outward[1] * 1000.0).round() as i32,
            ],
        );
        assert!(scrubber.value() >= MIN_RADIUS);
    }

    #[test]
    fn cancelling_restores_the_press_value() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        let (grab, outward) = grip(&camera, &scrubber);
        assert!(scrubber.begin_drag(&camera, grab));
        scrubber.drag_to(
            &camera,
            [
                (f64::from(grab[0]) + outward[0] * 20.0).round() as i32,
                (f64::from(grab[1]) + outward[1] * 20.0).round() as i32,
            ],
        );
        assert!(scrubber.value() > 0.25);
        assert!(scrubber.cancel_drag());
        assert!((scrubber.value() - 0.25).abs() < 1e-12);
        assert!(!scrubber.is_dragging());
        assert!(!scrubber.cancel_drag(), "nothing left to cancel");
    }

    #[test]
    fn a_step_snaps_the_value() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet).with_step(Some(0.5));
        let (grab, outward) = grip(&camera, &scrubber);
        assert!(scrubber.begin_drag(&camera, grab));
        scrubber.drag_to(
            &camera,
            [
                (f64::from(grab[0]) + outward[0] * 10.0).round() as i32,
                (f64::from(grab[1]) + outward[1] * 10.0).round() as i32,
            ],
        );
        let value = scrubber.value();
        assert!(
            ((value / 0.5 - (value / 0.5).round()).abs()) < 1e-9,
            "{value} is not on the 0.5 grid"
        );
        assert!(value > 0.25, "the drag still moved the value");
    }

    #[test]
    fn the_arc_geometry_is_rebuilt_only_when_needed() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(scrubber.update(&camera), "the first update builds");
        assert!(!scrubber.update(&camera), "an unchanged camera is a no-op");
        assert!(!scrubber.items()[1].positions().is_empty());
        scrubber.set_value(0.5);
        assert!(scrubber.update(&camera), "a value change rebuilds");
    }

    #[test]
    fn a_hovered_arc_is_recoloured() {
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(scrubber.set_hovered(true));
        assert!(scrubber.hovered());
        assert!(!scrubber.set_hovered(true), "unchanged is a no-op");
        let idle = GizmoStyle::default().inactive;
        let active = GizmoStyle::default().active;
        assert_eq!(scrubber.items()[1].surface_color(), active);
        assert!(scrubber.set_hovered(false));
        assert_eq!(scrubber.items()[1].surface_color(), idle);
    }

    #[test]
    fn a_custom_style_tints_and_widens_the_arc() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(scrubber.update(&camera));

        let mut style = GizmoStyle::default();
        style.inactive = [0.0, 1.0, 0.0, 0.8];
        style.active = [1.0, 0.0, 0.0, 1.0];
        style.outline = [0.0, 0.0, 1.0, 1.0];
        style.opacity = 0.5;
        style.width = 4.0;
        style.halo_width = 2.0;
        style.ghost_alpha = 0.6;
        assert!(scrubber.set_style(&style));
        assert_eq!(
            scrubber.items()[1].ghost_alpha(),
            0.6,
            "the ghost alpha reaches the arc immediately"
        );
        assert!(scrubber.update(&camera), "the style forces a rebuild");
        assert!(!scrubber.set_style(&style), "the same style is a no-op");

        assert_eq!(scrubber.items()[1].surface_color(), [0.0, 1.0, 0.0, 0.4]);
        let positions = scrubber.items()[1].positions();
        let point = |index: usize| {
            Vector3::new(
                f64::from(positions[index][0]),
                f64::from(positions[index][1]),
                f64::from(positions[index][2]),
            )
        };
        let width = point(0).sub(&point(3)).length();
        let anchor = scrubber.anchor().expect("an anchor");
        let per_pixel = scale_for(&camera, anchor, 1.0);
        let expected = style.width * per_pixel;
        assert!(
            (width - expected).abs() < 1e-6,
            "a 4 px arc: {width} vs {expected}"
        );

        // The halo is the style's outline ink, dimmed by the opacity, and 2·halo wider.
        let halo = &scrubber.items()[0];
        assert_eq!(halo.surface_color(), [0.0, 0.0, 1.0, 0.5]);
        let halo_positions = halo.positions();
        let halo_point = |index: usize| {
            Vector3::new(
                f64::from(halo_positions[index][0]),
                f64::from(halo_positions[index][1]),
                f64::from(halo_positions[index][2]),
            )
        };
        let halo_width = halo_point(0).sub(&halo_point(3)).length();
        let expected = (style.width + 2.0 * style.halo_width) * per_pixel;
        assert!(
            (halo_width - expected).abs() < 1e-6,
            "the halo is stroke + 2·halo: {halo_width} vs {expected}"
        );

        // Hovering widens the stroke and rebuilds it; the colour is already active.
        assert!(scrubber.set_hovered(true));
        assert!(scrubber.update(&camera), "the width cue forces a rebuild");
        let hovered_positions = scrubber.items()[1].positions();
        let width = Vector3::new(
            f64::from(hovered_positions[0][0]),
            f64::from(hovered_positions[0][1]),
            f64::from(hovered_positions[0][2]),
        )
        .sub(&Vector3::new(
            f64::from(hovered_positions[3][0]),
            f64::from(hovered_positions[3][1]),
            f64::from(hovered_positions[3][2]),
        ))
        .length();
        let expected = style.width * style.hover_scale * per_pixel;
        assert!(
            (width - expected).abs() < 1e-6,
            "the hovered stroke is {}× wider: {width} vs {expected}",
            style.hover_scale
        );
        assert_eq!(scrubber.items()[1].surface_color(), [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(scrubber.style(), &style);
    }

    #[test]
    fn a_zero_halo_width_draws_no_halo() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        assert!(scrubber.update(&camera));
        assert!(!scrubber.items()[0].positions().is_empty());

        let mut style = scrubber.style().clone();
        style.halo_width = 0.0;
        assert!(scrubber.set_style(&style));
        assert!(scrubber.update(&camera));
        assert!(
            scrubber.items()[0].positions().is_empty(),
            "a zero halo width leaves only the stroke"
        );
    }

    #[test]
    fn the_grab_radius_follows_the_style() {
        let camera = camera(5.0);
        let mut scrubber = scrubber(DressUpMode::Fillet);
        let (grab, outward) = grip(&camera, &scrubber);
        // 15 px beyond the arc stroke is outside the default 12 px radius.
        let off = [
            (f64::from(grab[0]) + outward[0] * 15.0).round() as i32,
            (f64::from(grab[1]) + outward[1] * 15.0).round() as i32,
        ];
        assert!(!scrubber.hit(&camera, off));

        let mut style = scrubber.style().clone();
        style.hit_radius_px = 20.0;
        assert!(scrubber.set_style(&style));
        assert!(
            scrubber.hit(&camera, off),
            "the configured radius reaches 15 px"
        );
    }

    #[test]
    fn a_degenerate_edge_has_no_anchor_and_never_hits() {
        let camera = camera(5.0);
        let mut scrubber = DressUpScrubber::new(7, DressUpMode::Fillet, "Edge9", Vec::new());
        assert!(scrubber.anchor().is_none());
        assert!(scrubber.arc_points(&camera).is_empty());
        assert!(!scrubber.hit(&camera, [50, 50]));
        assert!(!scrubber.begin_drag(&camera, [50, 50]));
        assert_eq!(scrubber.readout(), "R 1.00 mm");
    }

    #[test]
    fn an_edge_pointing_at_the_camera_still_gets_an_arc() {
        // An edge along the view direction (the camera looks along +Z).
        let camera = camera(5.0);
        let scrubber = DressUpScrubber::new(
            7,
            DressUpMode::Fillet,
            "Edge1",
            vec![Vector3::new(0.0, 0.0, -0.2), Vector3::new(0.0, 0.0, 0.2)],
        )
        .with_value(0.25);
        assert_eq!(scrubber.arc_points(&camera).len(), ARC_SEGMENTS + 1);
        assert!(scrubber.readout_anchor(&camera).is_some());
    }

    /// A slab whose `Edge1` runs from (0,0,0) to (10,0,0), tagged `Face1`/`Edge1`.
    fn slab(entity: u32) -> Stage {
        use ferrocad_geom::Tessellation;

        let mesh = Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
        };
        Stage::new().with_item(ShapeItem::from_tessellation(entity, &mesh))
    }

    #[test]
    fn an_edge_selection_makes_a_scrubber_over_the_edge() {
        let stage = slab(7);
        let scrubber = dress_up_scrubber_for(
            &SelectionRef::element(7, "Edge1"),
            &stage,
            DressUpMode::Fillet,
        )
        .expect("a scrubber");
        assert_eq!(scrubber.target_id(), 7);
        assert_eq!(scrubber.edge_name(), "Edge1");
        assert_eq!(scrubber.mode(), DressUpMode::Fillet);

        // The edge runs from (0,0,0) to (10,0,0); the anchor is its midpoint.
        let anchor = scrubber.anchor().expect("an anchor");
        assert!((anchor.x - 5.0).abs() < 1e-9);
        assert!(anchor.y.abs() < 1e-9);
    }

    /// A 10 × 4 × 4 box whose `Edge1` runs along `x` at `y = z = 0`.
    fn edged_box() -> Stage {
        use ferrocad_geom::Tessellation;

        let mesh = Tessellation {
            positions: vec![
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
                [0.0, 0.0, 4.0],
                [10.0, 0.0, 4.0],
                [10.0, 4.0, 4.0],
                [0.0, 4.0, 4.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7],
            triangle_parts: vec![0, 0, 0, 0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
        };
        Stage::new().with_item(ShapeItem::from_tessellation(7, &mesh))
    }

    #[test]
    fn the_scrubber_publishes_its_floor_and_takes_the_kernel_ceiling() {
        let stage = edged_box();
        let mut scrubber = dress_up_scrubber_for(
            &SelectionRef::element(7, "Edge1"),
            &stage,
            DressUpMode::Fillet,
        )
        .expect("a scrubber");

        // The tessellation proves a floor and no ceiling: half the body's thinnest extent
        // is not a bound, because a single edge of a cube takes a larger fillet than that.
        let fields = Manipulator::numeric_fields(&scrubber);
        assert_eq!(fields[0].min, Some(MIN_RADIUS));
        assert_eq!(fields[0].max, None, "nothing in the stage proves a ceiling");
        assert_eq!(
            fields[0].decimals, 3,
            "the floor is 0.001, so two decimals could not print it"
        );

        // The kernel's answer adds the ceiling, and it is enforced, not just printed.
        assert!(Manipulator::set_value_bounds(
            &mut scrubber,
            Some(MIN_RADIUS),
            Some(2.0)
        ));
        assert_eq!(Manipulator::numeric_fields(&scrubber)[0].max, Some(2.0));
        assert!(Manipulator::set_values(&mut scrubber, &[50.0]));
        assert!((scrubber.value() - 2.0).abs() < 1e-12);
        assert_eq!(
            Manipulator::pending_commit(&scrubber)
                .expect("a commit")
                .values,
            vec![2.0]
        );
        assert!(
            !Manipulator::set_value_bounds(&mut scrubber, Some(MIN_RADIUS), Some(2.0)),
            "the same range is not a change"
        );

        // A floor the kernel measured that is above the scrub's own minimum is adopted.
        assert!(Manipulator::set_value_bounds(&mut scrubber, Some(0.5), Some(2.0)));
        assert_eq!(Manipulator::numeric_fields(&scrubber)[0].min, Some(0.5));
    }

    #[test]
    fn a_face_or_unknown_selection_makes_no_scrubber() {
        let stage = slab(7);
        assert!(
            dress_up_scrubber_for(
                &SelectionRef::element(7, "Face1"),
                &stage,
                DressUpMode::Fillet
            )
            .is_none()
        );
        assert!(
            dress_up_scrubber_for(&SelectionRef::whole(7), &stage, DressUpMode::Fillet).is_none()
        );
        assert!(
            dress_up_scrubber_for(
                &SelectionRef::element(9, "Edge1"),
                &stage,
                DressUpMode::Fillet
            )
            .is_none()
        );
        assert!(
            dress_up_scrubber_for(
                &SelectionRef::element(7, "Edge9"),
                &stage,
                DressUpMode::Fillet
            )
            .is_none()
        );
    }

    #[test]
    fn the_manipulator_commits_neutral_data_that_round_trips() {
        let mut scrubber = scrubber(DressUpMode::Chamfer).with_second(0.5);
        let camera = camera(5.0);
        let (grab, _) = grip(&camera, &scrubber);
        assert!(Manipulator::drag_begin(&mut scrubber, &camera, grab));
        let commit = Manipulator::drag_end(&mut scrubber).expect("a commit");

        assert_eq!(commit.manipulator, DRESS_UP_ID);
        assert_eq!(commit.target, 7);
        assert_eq!(commit.element.as_deref(), Some("Edge1"));
        assert_eq!(commit.action, "chamfer");
        assert_eq!(commit.values, vec![0.25, 0.5]);
        assert_eq!(commit.readout, "0.25 × 0.50 mm");

        // The typed change comes back, so the document layer never parses the numbers.
        let change = DressUpChange::from_commit(&commit).expect("a dress-up change");
        assert_eq!(change.target_id, 7);
        assert_eq!(change.edge, "Edge1");
        assert_eq!(
            change.value,
            DressUpValue::Chamfer {
                first: 0.25,
                second: 0.5
            }
        );
    }

    #[test]
    fn a_commit_for_another_manipulator_is_not_a_dress_up() {
        let commit = InteractionCommit::new("some_other", 1, "fillet").with_element("Edge1");
        assert!(DressUpChange::from_commit(&commit).is_none());
    }
}

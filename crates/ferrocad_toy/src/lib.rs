//! # FerroCAD Toy (`ferrocad_toy`)
//!
//! A **toy workbench** that exercises the generic interaction model without a kernel.
//! It exists to prove that a manipulator is a workbench contribution, not a shell or
//! stage feature:
//!
//! ```text
//! ferrocad_stage      Manipulator · InteractionCommit                     (generic)
//!       ▲
//! ferrocad_toy        HeightGrip · HeightGizmo · height_grip              (this crate)
//!       ▲
//! its app script      display_mesh (a cube sized by Height) · interaction (writes it)
//! ```
//!
//! The document object is a plain `App::FeaturePython` with a `Height` length
//! property. The app script generates the display mesh and implements the
//! `toy_height` interaction; this crate supplies the draggable grip. The edition
//! binary registers it exactly as the Part edition registers its fillet arc, so the
//! shell routes it without naming either workbench.
//!
//! The toy is also the cheapest full-loop test: no OCCT, no shape kernel, just the
//! interpreter, a mesh generator and a property.
//!
//! See `docs/developer/workbenches/toy-workbench.md` and `docs/developer/architecture.md` §10.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::{Camera, screen_ribbon};
use ferrocad_stage::gizmo::{GizmoStyle, MIN_HIT_RADIUS_PX};
use ferrocad_stage::screen::{
    camera_forward, point_segment_distance, project_device, push_ribbon_uv, scale_for,
};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, ShapeItem, Stage};
use ferrocad_stage::{
    GizmoAxis, GizmoCommitEvent, GizmoConstraint, GizmoDragEvent, GizmoHandle, GizmoKind,
    GizmoTarget, GizmoValue, ManipulatorFrame, TransformGizmo,
};
use ferrocad_types::{Matrix4, Vector3};

/// The manipulator's registry id (`InteractionCommit::manipulator`).
pub const TOY_HEIGHT_ID: &str = "toy_height";

/// The grip's picking id. It is drawn as an overlay, so it only has to stay clear of
/// document and gizmo ids.
pub const TOY_GRIP_PICK_ID: u32 = 3_000_000;

/// The smallest height the grip will produce.
pub const MIN_HEIGHT: f64 = 0.1;

/// The grip stem's screen-invariant length, in device pixels.
const STEM_PIXELS: f64 = 44.0;

/// A vertical grip that drags the slab's `Height` property.
///
/// The anchor is the top centre of the displayed item; the handle is a screen-invariant
/// stem projecting **up** from that anchor into empty space plus a cross at its tip.
/// Dragging up grows the height by the pointer's travel along the stem's axis.
///
/// The drag and the commit are the **shared gizmo's**, not a private path: `solver` is a
/// [`TransformGizmo`] whose frame is the grip's anchor with world axes, so local Z is
/// the stem, and [`begin_drag`](Self::begin_drag) starts the very `AxisZ` axis solver a
/// translate triad uses. On release the grip turns the resulting
/// [`GizmoCommitEvent`](ferrocad_stage::GizmoCommitEvent) into its workbench
/// interaction, exactly as the triad turns its own into `transform/apply` (see
/// `docs/developer/interaction/transform-ux.md` §7).
///
/// A stem seen end-on collapses to a dot, so the axis solver has no direction to scrub;
/// the grip then falls back to the pointer's vertical screen travel, the same
/// screen-invariant mapping, which keeps a top-down resize working.
///
/// The casing behind the stroke is the shared [`GizmoStyle::outline`] halo, sized by
/// [`GizmoStyle::halo_width`]. It is the WCAG 1.4.11 contrast aid: the toy's blue slab
/// sits at the same lightness as the cyan stem, so a dark outline is what keeps the
/// grip readable over it. A hover (or an active drag) also widens the stroke by
/// [`GizmoStyle::hover_scale`], the non-colour cue for WCAG 1.4.1.
#[derive(Clone, Debug)]
pub struct HeightGrip {
    target: u32,
    /// Top centre of the item, in world space.
    top: Vector3,
    height: f64,
    /// The active drag: the press device `y`, the height at the press, and whether the
    /// shared axis solver owns it (otherwise the screen-travel fallback does).
    press: Option<HeightPress>,
    hovered: bool,
    /// The workspace appearance.
    style: GizmoStyle,
    /// The shared axis solver: its frame is the anchor with world axes, so its local Z
    /// is the stem and only the `AxisZ` grab is ever started.
    solver: TransformGizmo,
    /// The camera view and height the cached stroke was built for.
    view: Option<Matrix4>,
    built: Option<f64>,
    /// The draw order: the dark casing first, the coloured stroke on top.
    items: [ShapeItem; 2],
}

/// The bookkeeping of an active height drag.
#[derive(Clone, Copy, Debug)]
struct HeightPress {
    /// The press position's device `y` (bottom-left origin, so up is larger).
    device_y: f64,
    /// The height when the drag began.
    height: f64,
    /// Whether the shared [`TransformGizmo`] axis solver owns the drag. It cannot when
    /// the stem points at the eye (the axis projects to under a pixel).
    solved: bool,
}

impl HeightGrip {
    /// A grip for the item whose top centre is `top`, starting at `height`.
    pub fn new(target: u32, top: Vector3, height: f64) -> Self {
        let style = GizmoStyle::default();
        let ghost_alpha = style.ghost_alpha;
        let mut solver = TransformGizmo::translate(target, top);
        solver.set_style(&style);
        Self {
            target,
            top,
            height: height.max(MIN_HEIGHT),
            press: None,
            hovered: false,
            style,
            solver,
            view: None,
            built: None,
            items: [
                ShapeItem::new(TOY_GRIP_PICK_ID)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_ribbon_aa(),
                ShapeItem::new(TOY_GRIP_PICK_ID + 1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_blended_surface()
                    .with_ghost_surface()
                    .with_ghost_alpha(ghost_alpha)
                    .with_ribbon_aa(),
            ],
        }
    }

    /// The entity the grip edits.
    pub fn target_id(&self) -> u32 {
        self.target
    }

    /// The current height.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// The grip's world anchor (the item's top centre).
    pub fn top(&self) -> Vector3 {
        self.top
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.press.is_some()
    }

    /// Whether the grip is highlighted.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The overlay items, casing first so the stroke sits on top of it.
    pub fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    /// The readout text (`H 20.00 mm`).
    pub fn readout(&self) -> String {
        format!("H {:.2} mm", self.height)
    }

    /// Highlight (or un-highlight) the grip. Returns whether the visuals changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        // The hover cue is also a stroke-width change, which is baked into the cached
        // ribbons; force the next `update` to rebuild them.
        self.built = None;
        self.apply_color();
        true
    }

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
    /// The stroke width is baked into the cached ribbons, so a style change forces the
    /// next [`update`](Self::update) to rebuild them. The default style reproduces the
    /// legacy 1.5 px inactive/active stroke exactly. The shared solver is given the same
    /// style, so its screen-space fallback scales with the grip.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        self.solver.set_style(style);
        if self.style == *style {
            return false;
        }
        self.style = style.clone();
        self.built = None;
        self.apply_color();
        true
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        &self.style
    }

    fn apply_color(&mut self) {
        let color = self.stroke_color();
        self.items[1].set_surface_color(color);
        self.items[1].set_edge_color(color);
        self.items[1].set_ghost_alpha(self.style.ghost_alpha);
        // The casing is the outline/halo ink, dimmed with the rest of the style.
        let outline = [
            self.style.outline[0],
            self.style.outline[1],
            self.style.outline[2],
            self.style.outline[3] * self.style.opacity,
        ];
        self.items[0].set_surface_color(outline);
        self.items[0].set_edge_color(outline);
    }

    /// Whether `device` is on the grip (the top anchor or the stem, within the style's
    /// grab radius).
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(anchor) = project_device(camera, self.top) else {
            return false;
        };
        let Some(tip) = project_device(camera, self.stem_end(camera)) else {
            return false;
        };
        let point = [f64::from(device[0]), f64::from(device[1])];
        let slop = self.style.hit_radius_px.max(MIN_HIT_RADIUS_PX);
        point_segment_distance(point, anchor, tip).0 <= slop
    }

    /// Begin a drag at `device`.
    ///
    /// The shared [`TransformGizmo`] axis solver starts the `AxisZ` drag when the stem
    /// can be scrubbed; a stem that points at the eye (the solver has no direction) falls
    /// back to the pointer's vertical travel. Either way the grip owns the height and the
    /// release produces the same [`GizmoCommitEvent`](ferrocad_stage::GizmoCommitEvent).
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        if !self.hit(camera, device) {
            return false;
        }
        let solved = self
            .solver
            .begin_drag(GizmoHandle::AxisZ, camera, device);
        self.press = Some(HeightPress {
            device_y: f64::from(device[1]),
            height: self.height,
            solved,
        });
        // The grabbed state widens the stroke, so the next update must rebuild it.
        self.built = None;
        self.apply_color();
        true
    }

    /// Advance the drag. The shared axis solver supplies the travel along the stem when
    /// it can; the fallback is the pointer's vertical screen travel (the device origin is
    /// bottom-left, so a larger `y` is **up**).
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<f64> {
        let press = self.press?;
        let delta = if press.solved {
            let event = self.solver.drag_to(camera, device)?;
            match event.value {
                GizmoValue::Linear { applied, .. } => applied,
                _ => return None,
            }
        } else {
            let world_per_pixel = scale_for(camera, self.top, 1.0).max(1e-12);
            (f64::from(device[1]) - press.device_y) * world_per_pixel
        };
        let height = (press.height + delta).max(MIN_HEIGHT);
        if (height - self.height).abs() <= 1e-12 {
            return None;
        }
        self.height = height;
        Some(height)
    }

    /// End the drag, returning the shared **gizmo commit**.
    ///
    /// This is the event a translate triad produces (`Axis`, a `Linear` value and the
    /// world transform), so the height grip and the triad share one commit vocabulary
    /// and one shell route. [`end_drag`](Self::end_drag) turns it into the workbench's
    /// `toy_height/set` interaction.
    pub fn end_gizmo_drag(&mut self) -> Option<GizmoCommitEvent> {
        let press = self.press.take()?;
        // The solver must not keep a stale drag: the grip owns the committed value (it
        // clamps at `MIN_HEIGHT`), not the solver's last tick.
        self.solver.cancel_drag();
        self.built = None;
        let delta = self.height - press.height;
        Some(gizmo_commit(self.target, delta))
    }

    /// End the drag, returning the value to commit.
    pub fn end_drag(&mut self) -> Option<InteractionCommit> {
        let commit = self.end_gizmo_drag()?;
        Some(self.interaction(&commit))
    }

    /// The workbench interaction for a gizmo commit: the grip's own id, action and value.
    fn interaction(&self, commit: &GizmoCommitEvent) -> InteractionCommit {
        commit.to_interaction_with(
            TOY_HEIGHT_ID,
            "set",
            [self.height],
            self.readout(),
        )
    }

    /// Cancel the drag, restoring the press value. Returns whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        self.solver.cancel_drag();
        self.height = press.height;
        self.built = None;
        self.apply_color();
        true
    }

    /// Rebuild the stroke when the view or the value changed.
    pub fn update(&mut self, camera: &Camera) -> bool {
        if self.view == Some(camera.view) && self.built == Some(self.height) {
            return false;
        }
        self.view = Some(camera.view);
        self.built = Some(self.height);
        self.build(camera);
        true
    }

    /// The stem's endpoint: `STEM_PIXELS` of screen-invariant travel straight up from
    /// the top face, into empty space.
    fn stem_end(&self, camera: &Camera) -> Vector3 {
        let length = scale_for(camera, self.top, STEM_PIXELS);
        Vector3::new(self.top.x, self.top.y, self.top.z + length)
    }

    /// Rebuild the geometry: a `width`-wide ribbon from the top centre up into empty
    /// space, plus a crossbar at its tip, over a casing that is `halo_width` wider on
    /// each side. The stem, crossbar, casing and the hover widen are screen-invariant,
    /// so the target stays grabbable at any zoom.
    fn build(&mut self, camera: &Camera) {
        let stem_end = self.stem_end(camera);
        let crossbar_half = scale_for(camera, self.top, 5.0);
        let active = self.hovered || self.is_dragging();
        let hover_scale = self.style.hover_scale.max(1.0);
        let halo = self.style.halo_width.max(0.0);
        let stroke_pixels = if active {
            self.style.width * hover_scale
        } else {
            self.style.width
        };
        let stroke_half = scale_for(camera, self.top, stroke_pixels / 2.0);
        let casing_half = scale_for(camera, self.top, (stroke_pixels + 2.0 * halo) / 2.0);
        let forward = camera_forward(camera);
        let left = Vector3::new(
            stem_end.x - crossbar_half,
            stem_end.y,
            stem_end.z,
        );
        let right = Vector3::new(
            stem_end.x + crossbar_half,
            stem_end.y,
            stem_end.z,
        );

        let ribbons = |half: f64| {
            let mut positions = Vec::new();
            let mut uvs = Vec::new();
            let mut triangles = Vec::new();
            push_ribbon_uv(
                &mut positions,
                &mut uvs,
                &mut triangles,
                screen_ribbon(self.top, stem_end, half, forward),
            );
            push_ribbon_uv(
                &mut positions,
                &mut uvs,
                &mut triangles,
                screen_ribbon(left, right, half, forward),
            );
            (positions, uvs, triangles)
        };

        let (stroke_positions, stroke_uvs, stroke_triangles) = ribbons(stroke_half);
        if halo > 0.0 {
            let (casing_positions, casing_uvs, casing_triangles) = ribbons(casing_half);
            self.items[0].set_shape_uv(
                casing_positions,
                Vec::new(),
                casing_uvs,
                casing_triangles,
                Vec::new(),
            );
        } else {
            self.items[0].set_shape(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }
        // The stroke is coplanar with the casing; nudge it half a pixel toward the eye so
        // the depth test lets the coloured ribbon win deterministically.
        let toward_eye = camera_forward(camera).scale(-scale_for(camera, self.top, 0.5));
        let stroke_positions: Vec<[f32; 3]> = stroke_positions
            .into_iter()
            .map(|point| {
                [
                    point[0] + toward_eye.x as f32,
                    point[1] + toward_eye.y as f32,
                    point[2] + toward_eye.z as f32,
                ]
            })
            .collect();
        self.items[1].set_shape_uv(
            stroke_positions,
            Vec::new(),
            stroke_uvs,
            stroke_triangles,
            Vec::new(),
        );
        self.apply_color();
    }

    /// Where the readout sits, in device pixels: above the stem's tip.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.stem_end(camera))
    }
}

impl Manipulator for HeightGrip {
    fn id(&self) -> &str {
        TOY_HEIGHT_ID
    }

    fn items(&self) -> &[ShapeItem] {
        HeightGrip::items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        HeightGrip::end_drag(self)
    }

    fn drag_cancel(&mut self) -> bool {
        HeightGrip::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        HeightGrip::is_dragging(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        HeightGrip::set_hovered(self, hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        HeightGrip::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn readout(&self) -> Option<String> {
        Some(HeightGrip::readout(self))
    }

    fn anchor(&self) -> Option<Vector3> {
        Some(self.top)
    }

    fn set_anchor(&mut self, anchor: Vector3) -> bool {
        if self.top == anchor {
            return false;
        }
        self.top = anchor;
        // The solver's frame is the stem base, so a moved grip drags along the new stem.
        self.solver.set_frame(ManipulatorFrame::world(anchor));
        // The cached ribbons were built at the old anchor; rebuild on the next refresh.
        self.built = None;
        true
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        HeightGrip::readout_anchor(self, camera)
    }
}

/// The height grip presented as an **axis-handle gizmo**.
///
/// [`HeightGrip`] already owns the whole grammar of a one-degree-of-freedom gizmo: a
/// screen-invariant stem, an analytic grab test, a drag value and a neutral commit.
/// This adapter adds the vocabulary the gizmo registry names — a [`GizmoKind`], the
/// [`GizmoHandle::AxisZ`] the stem stands for, and the [`ManipulatorFrame`] the axis
/// acts in — and it speaks the gizmo **drag/commit** vocabulary too: `begin_drag`/
/// `drag_to` drive the shared axis solver and `end_drag` returns the same
/// [`GizmoCommitEvent`] a translate triad does.
///
/// It is still *registered* through the manipulator route: the gpui registry's
/// `GizmoFactory` returns [`TransformGizmo`] values, whose commit is `transform/apply`,
/// while this gizmo commits `toy_height/set`. The workbench converts the shared commit
/// with [`GizmoCommitEvent::to_interaction_with`], which is the same conversion the triad
/// uses (`to_interaction`). Once a host accepts a workbench gizmo in the hosted set, the
/// adapter moves into the gizmo registry unchanged. See
/// `docs/developer/interaction/transform-ux.md` §7.
#[derive(Clone, Debug)]
pub struct HeightGizmo {
    grip: HeightGrip,
}

impl HeightGizmo {
    /// A grip for the item whose top centre is `top`, starting at `height`.
    pub fn new(target: u32, top: Vector3, height: f64) -> Self {
        Self {
            grip: HeightGrip::new(target, top, height),
        }
    }

    /// The adapter over an existing grip.
    pub fn from_grip(grip: HeightGrip) -> Self {
        Self { grip }
    }

    /// The wrapped grip (its full solver and style surface).
    pub fn grip(&self) -> &HeightGrip {
        &self.grip
    }

    /// Consume the adapter, returning the grip.
    pub fn into_grip(self) -> HeightGrip {
        self.grip
    }

    /// The entity the grip edits.
    pub fn target_id(&self) -> u32 {
        self.grip.target_id()
    }

    /// The gizmo kind: a height grip is a single-axis translation.
    pub fn kind(&self) -> GizmoKind {
        GizmoKind::Translate
    }

    /// The one handle it offers: the local Z axis, pointing up the stem.
    pub fn handle(&self) -> GizmoHandle {
        GizmoHandle::AxisZ
    }

    /// The frame the handle acts in: the stem base is the origin and local Z is world
    /// up (the grip edits the object's height along +Z).
    pub fn frame(&self) -> ManipulatorFrame {
        ManipulatorFrame::world(self.grip.top())
    }

    /// The gizmo pick target at `device`, in the registry's vocabulary.
    ///
    /// This is the same analytic test as [`HeightGrip::hit`]: a hit on the stem or its
    /// crossbar is [`GizmoTarget::Handle`] for [`handle`](Self::handle).
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> Option<GizmoTarget> {
        self.grip
            .hit(camera, device)
            .then_some(GizmoTarget::Handle(self.handle()))
    }

    /// Begin an axis drag on `handle` (only [`GizmoHandle::AxisZ`] exists).
    ///
    /// The grip's shared solver owns the drag, so this is the same entry point a
    /// translate triad's `AxisZ` has.
    pub fn begin_drag(
        &mut self,
        handle: GizmoHandle,
        camera: &Camera,
        device: [i32; 2],
    ) -> bool {
        handle == self.handle() && self.grip.begin_drag(camera, device)
    }

    /// Advance the axis drag, in the gizmo's drag vocabulary.
    ///
    /// `value.applied` is the height gained since the press (the grip's height minus the
    /// press height) and `total_transform` the matching `+Z` translation, so a host that
    /// mirrors a triad drag can mirror this one the same way.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<GizmoDragEvent> {
        let before = self.grip.height();
        let height = self.grip.drag_to(camera, device)?;
        let press = self.grip.press?.height;
        let delta = height - before;
        Some(GizmoDragEvent {
            target_id: self.target_id(),
            active_handle: self.handle(),
            constraint: GizmoConstraint::Axis(GizmoAxis::Z),
            value: GizmoValue::Linear {
                axis: GizmoAxis::Z,
                raw: height - press,
                applied: height - press,
            },
            delta_transform: Matrix4::translation(0.0, 0.0, delta),
            total_transform: Matrix4::translation(0.0, 0.0, height - press),
        })
    }

    /// End the drag, returning the shared **gizmo commit**.
    pub fn end_drag(&mut self) -> Option<GizmoCommitEvent> {
        self.grip.end_gizmo_drag()
    }

    /// Cancel the drag, restoring the press height.
    pub fn cancel_drag(&mut self) -> bool {
        self.grip.cancel_drag()
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.grip.is_dragging()
    }

    /// The overlay items, casing first so the stroke sits on top of it.
    pub fn items(&self) -> &[ShapeItem] {
        self.grip.items()
    }

    /// The readout text (`H 20.00 mm`).
    pub fn readout(&self) -> String {
        self.grip.readout()
    }

    /// Apply the workspace gizmo style. Returns whether the style changed.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        self.grip.set_style(style)
    }

    /// The workspace gizmo style.
    pub fn style(&self) -> &GizmoStyle {
        self.grip.style()
    }

    /// Rebuild the stroke when the view or the value changed.
    pub fn update(&mut self, camera: &Camera) -> bool {
        self.grip.update(camera)
    }

    /// Where the readout sits, in device pixels: above the stem's tip.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        self.grip.readout_anchor(camera)
    }
}

impl Manipulator for HeightGizmo {
    fn id(&self) -> &str {
        TOY_HEIGHT_ID
    }

    fn items(&self) -> &[ShapeItem] {
        HeightGizmo::items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGizmo::hit(self, camera, device).is_some()
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        let handle = self.handle();
        HeightGizmo::begin_drag(self, handle, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGizmo::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        let commit = HeightGizmo::end_drag(self)?;
        Some(self.grip.interaction(&commit))
    }

    fn drag_cancel(&mut self) -> bool {
        HeightGizmo::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        HeightGizmo::is_dragging(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        self.grip.set_hovered(hovered)
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        HeightGizmo::set_style(self, style)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn readout(&self) -> Option<String> {
        Some(HeightGizmo::readout(self))
    }

    fn anchor(&self) -> Option<Vector3> {
        Some(self.grip.top())
    }

    fn set_anchor(&mut self, anchor: Vector3) -> bool {
        self.grip.set_anchor(anchor)
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        HeightGizmo::readout_anchor(self, camera)
    }
}

/// The shared gizmo commit for a height drag: the stem's axis constraint, the linear
/// travel and the `+Z` world translation.
///
/// It is deliberately the **same event type** a translate triad produces, so the grip
/// rides the gizmo commit path rather than inventing a second one; the difference is only
/// the workbench conversion (`toy_height/set` instead of `transform/apply`).
fn gizmo_commit(target: u32, delta: f64) -> GizmoCommitEvent {
    GizmoCommitEvent {
        target_id: target,
        constraint: GizmoConstraint::Axis(GizmoAxis::Z),
        value: GizmoValue::Linear {
            axis: GizmoAxis::Z,
            raw: delta,
            applied: delta,
        },
        total_transform: Matrix4::translation(0.0, 0.0, delta),
    }
}

/// The world-space bounds `(min, max)` of a stage item's positions.
fn bounds(item: &ShapeItem) -> Option<(Vector3, Vector3)> {
    let positions = item.positions();
    if positions.is_empty() {
        return None;
    }
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for [x, y, z] in positions {
        min.x = min.x.min(f64::from(*x));
        min.y = min.y.min(f64::from(*y));
        min.z = min.z.min(f64::from(*z));
        max.x = max.x.max(f64::from(*x));
        max.y = max.y.max(f64::from(*y));
        max.z = max.z.max(f64::from(*z));
    }
    Some((min, max))
}

/// The toy workbench's gizmo factory: a **whole-object** selection arms an
/// axis-handle [`HeightGizmo`] over the item's top centre.
///
/// The grip stays whole-object even though the toy's mesh now names its faces: a box's
/// height is a property of the body, so a face pick is enough to *offer* the grip but the
/// edit still applies to the object. (The Part factory is the one that wants an `EdgeN`.)
/// The adapter is the proof that the grip really
/// is a single-axis translate gizmo: it answers with [`GizmoKind::Translate`],
/// [`GizmoHandle::AxisZ`] and a [`GizmoTarget`], while keeping the grip's readout and
/// `toy_height` commit.
pub fn height_gizmo(selection: &SelectionRef, stage: &Stage) -> Option<HeightGizmo> {
    let item = stage.item(selection.object)?;
    let (min, max) = bounds(item)?;
    let top = Vector3::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, max.z);
    Some(HeightGizmo::new(
        selection.object,
        top,
        max.z - min.z,
    ))
}

/// The toy workbench's manipulator factory: the axis-handle gizmo, registered through
/// the manipulator route until a host can own a workbench gizmo.
///
/// The shell routes both registries identically (a neutral [`InteractionCommit`]), so
/// the adapter changes no pointer, readout or commit behaviour; it only gives the grip
/// the gizmo vocabulary a kinds-aware host will use.
pub fn height_grip(selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>> {
    Some(Box::new(height_gizmo(selection, stage)?))
}

/// The toy workbench's gizmo factory: a selection on a displayed item gets a translate
/// gizmo at the item's own origin.
///
/// A **face** pick is enough: the commit applies to the object, and a click that selects
/// a face must not quietly remove the handles (double-click still promotes the selection
/// to the whole body). The
/// gizmo is the generic move handle; whether an object responds is the workbench's
/// decision. The toy offers translation (its document provider composes the committed
/// matrix with the object's placement). The datum is the same one the base edition uses
/// ([`ferrocad_stage::ShapeItem::anchor`]): the placement origin when the provider reports
/// it, otherwise the bounding box's base corner — so the triad doubles as an origin
/// indicator.
pub fn move_gizmo(selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
    let item = stage.item(selection.object)?;
    let origin = item.anchor()?;
    Some(TransformGizmo::translate(selection.object, origin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_stage::ShapeItem;
    use ferrocad_types::{Placement, Rotation};

    fn camera(eye_z: f64) -> Camera {
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 1000.0,
            },
            viewport: Viewport::new([400, 400], 1.0),
        }
    }

    /// A 30° pitched view, so the vertical stem projects into a visible screen segment
    /// instead of collapsing onto the view axis, and world +Z reads as screen-up.
    fn tilted_camera(eye_z: f64) -> Camera {
        let rotation = Rotation::from_axis_angle(
            &Vector3::new(1.0, 0.0, 0.0),
            (-30.0_f64).to_radians(),
        );
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), rotation).to_matrix(),
            ..camera(eye_z)
        }
    }

    /// A 40 × 40 × 20 slab from z = 0, centred on the origin in x/y.
    fn slab(entity: u32) -> Stage {
        use ferrocad_geom::Tessellation;

        let mesh = Tessellation {
            positions: vec![
                [-20.0, -20.0, 0.0],
                [20.0, -20.0, 0.0],
                [20.0, 20.0, 0.0],
                [-20.0, 20.0, 0.0],
                [-20.0, -20.0, 20.0],
                [20.0, -20.0, 20.0],
                [20.0, 20.0, 20.0],
                [-20.0, 20.0, 20.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3],
            triangle_parts: vec![0, 0],
            edges: vec![0, 1],
            segment_parts: vec![0],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: Vec::new(),
        };
        Stage::new().with_item(ShapeItem::from_tessellation(entity, &mesh))
    }

    /// The grip at `(0, 0, 20)` over the slab.
    fn grip() -> HeightGrip {
        HeightGrip::new(7, Vector3::new(0.0, 0.0, 20.0), 20.0)
    }

    #[test]
    fn the_factory_arms_on_a_whole_selection_and_reads_the_top_centre() {
        let stage = slab(7);
        let grip = height_grip(&SelectionRef::whole(7), &stage).expect("a grip");
        assert_eq!(grip.id(), TOY_HEIGHT_ID);
        let camera = tilted_camera(200.0);
        let top = Vector3::new(0.0, 0.0, 20.0);
        let stem_end = Vector3::new(0.0, 0.0, top.z + scale_for(&camera, top, STEM_PIXELS));
        assert_eq!(
            grip.readout_anchor(&camera),
            project_device(&camera, stem_end)
        );
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let face = project_device(&camera, top).expect("the top face");
        assert!(anchor[1] > face[1], "the readout floats above the stem");
        assert!(grip.readout().expect("a readout").starts_with("H "));
    }

    #[test]
    fn the_height_grip_is_a_single_axis_translate_gizmo() {
        let gizmo = height_gizmo(&SelectionRef::whole(7), &slab(7)).expect("a gizmo");
        assert_eq!(gizmo.kind(), GizmoKind::Translate);
        assert_eq!(gizmo.handle(), GizmoHandle::AxisZ);

        // The frame is the top centre with world axes, so local Z is the stem.
        let frame = gizmo.frame();
        assert_eq!(frame.origin, Vector3::new(0.0, 0.0, 20.0));
        assert_eq!(
            frame.axis(GizmoHandle::AxisZ),
            Some(Vector3::new(0.0, 0.0, 1.0))
        );
        assert_eq!(
            gizmo.handle().axis(),
            Some(Vector3::new(0.0, 0.0, 1.0)),
            "the adapter names the one axis it offers"
        );
    }

    #[test]
    fn the_axis_handle_hit_uses_the_same_stem_and_commit() {
        let mut gizmo = height_gizmo(&SelectionRef::whole(7), &slab(7)).expect("a gizmo");
        let camera = tilted_camera(200.0);
        let top = project_device(&camera, gizmo.grip().top()).expect("the top");
        let tip = gizmo.readout_anchor(&camera).expect("the stem tip");
        let grab = [
            ((top[0] + tip[0]) * 0.5).round() as i32,
            ((top[1] + tip[1]) * 0.5).round() as i32,
        ];

        assert_eq!(
            gizmo.hit(&camera, grab),
            Some(GizmoTarget::Handle(GizmoHandle::AxisZ))
        );
        assert_eq!(gizmo.hit(&camera, [0, 0]), None, "off the stem is a miss");

        // The manipulator route sees the same grab, readout and `toy_height` commit.
        assert!(Manipulator::hit(&gizmo, &camera, grab));
        assert!(gizmo.drag_begin(&camera, grab));
        assert!(gizmo.drag_update(&camera, [grab[0], grab[1] + 20]));
        let commit = gizmo.drag_end().expect("a commit");
        assert_eq!(commit.manipulator, TOY_HEIGHT_ID);
        assert_eq!(commit.action, "set");
        assert_eq!(commit.values.len(), 1);
        assert!(commit.readout.starts_with("H "));
    }

    /// The drag and the release produce the **shared gizmo** vocabulary: the same
    /// `GizmoDragEvent`/`GizmoCommitEvent` a translate triad produces, converted by the
    /// workbench into its own `toy_height/set` interaction.
    #[test]
    fn the_height_drag_commits_the_shared_gizmo_event() {
        let mut gizmo = height_gizmo(&SelectionRef::whole(7), &slab(7)).expect("a gizmo");
        let camera = tilted_camera(200.0);
        let top = project_device(&camera, gizmo.grip().top()).expect("the top");
        let tip = gizmo.readout_anchor(&camera).expect("the stem tip");
        let grab = [
            ((top[0] + tip[0]) * 0.5).round() as i32,
            ((top[1] + tip[1]) * 0.5).round() as i32,
        ];
        let press_height = gizmo.grip().height();

        assert!(HeightGizmo::begin_drag(
            &mut gizmo,
            GizmoHandle::AxisZ,
            &camera,
            grab
        ));
        assert!(
            !HeightGizmo::begin_drag(&mut gizmo, GizmoHandle::AxisX, &camera, grab),
            "the grip offers one axis"
        );
        let tick = HeightGizmo::drag_to(&mut gizmo, &camera, [grab[0], grab[1] + 30])
            .expect("the drag tick");
        assert_eq!(tick.active_handle, GizmoHandle::AxisZ);
        assert_eq!(tick.constraint, GizmoConstraint::Axis(GizmoAxis::Z));
        let GizmoValue::Linear { applied, .. } = tick.value else {
            panic!("an axis drag is a linear value");
        };
        assert!(applied > 0.0, "dragging up grows the height");
        assert!((applied - (gizmo.grip().height() - press_height)).abs() < 1e-9);
        assert_eq!(tick.total_transform, Matrix4::translation(0.0, 0.0, applied));

        let commit = HeightGizmo::end_drag(&mut gizmo).expect("a gizmo commit");
        assert_eq!(commit.target_id, 7);
        assert_eq!(commit.constraint, GizmoConstraint::Axis(GizmoAxis::Z));
        assert_eq!(
            commit.total_transform,
            Matrix4::translation(0.0, 0.0, gizmo.grip().height() - press_height)
        );

        // The workbench conversion is the same one-way funnel translate uses: the grip
        // only names its own id, action and scalar.
        let interaction = commit.to_interaction_with(
            TOY_HEIGHT_ID,
            "set",
            [gizmo.grip().height()],
            gizmo.readout(),
        );
        assert_eq!(interaction.manipulator, TOY_HEIGHT_ID);
        assert_eq!(interaction.action, "set");
        assert_eq!(interaction.values, vec![gizmo.grip().height()]);
        assert!(interaction.values[0] > press_height);
    }

    /// A stem pointing at the eye cannot be scrubbed by the axis solver; the grip keeps
    /// the same commit while falling back to the pointer's vertical travel.
    #[test]
    fn a_head_on_stem_still_commits_a_gizmo_event() {
        let mut gizmo = height_gizmo(&SelectionRef::whole(7), &slab(7)).expect("a gizmo");
        let camera = camera(200.0);
        let anchor = gizmo.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(gizmo.drag_begin(&camera, grab));
        assert!(gizmo.drag_update(&camera, [grab[0], grab[1] + 25]));
        let commit = HeightGizmo::end_drag(&mut gizmo).expect("a gizmo commit");
        assert_eq!(commit.constraint, GizmoConstraint::Axis(GizmoAxis::Z));
        let delta = gizmo.grip().height() - 20.0;
        assert!(delta > 0.0, "dragging up grows the height ({delta})");
        assert_eq!(commit.total_transform, Matrix4::translation(0.0, 0.0, delta));
    }

    #[test]
    fn the_stem_projects_up_from_the_top_into_empty_space() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        assert!(grip.update(&camera));

        // Casing first, then the 1.5 px stroke; each is a stem ribbon and a crossbar
        // ribbon, so two quads (8 vertices, 12 indices) per item.
        assert_eq!(grip.items().len(), 2, "casing + stroke");
        let casing = &grip.items()[0];
        let stroke = &grip.items()[1];
        for item in [casing, stroke] {
            assert_eq!(item.positions().len(), 8, "two ribbon quads");
            assert_eq!(item.triangles().len(), 12, "six indices per quad");
        }
        assert_eq!(casing.surface_color(), GizmoStyle::default().outline);
        assert_eq!(stroke.surface_color(), GizmoStyle::default().inactive);

        // The geometry spans the top-face centroid up to the stem tip; neither ribbon
        // reaches down to the slab's base.
        let top = grip.top();
        let expected = scale_for(&camera, top, STEM_PIXELS);
        let min_z = stroke
            .positions()
            .iter()
            .map(|point| f64::from(point[2]))
            .fold(f64::INFINITY, f64::min);
        let max_z = stroke
            .positions()
            .iter()
            .map(|point| f64::from(point[2]))
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            min_z <= top.z + 1.0,
            "the stem starts at the top face ({min_z} > {})",
            top.z
        );
        assert!(
            max_z >= top.z + expected - 1.0,
            "the stem reaches the screen-invariant tip ({max_z})"
        );
        assert!(
            min_z > top.z - 1.0,
            "the ribbon never reaches the slab base ({min_z})"
        );
    }

    #[test]
    fn a_custom_style_tints_the_stroke_and_widens_the_casing() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        assert!(grip.update(&camera));

        let mut style = GizmoStyle::default();
        style.inactive = [0.0, 1.0, 0.0, 0.8];
        style.active = [1.0, 0.0, 0.0, 1.0];
        style.outline = [0.0, 0.0, 1.0, 1.0];
        style.opacity = 0.5;
        style.width = 5.0;
        style.halo_width = 2.0;
        style.ghost_alpha = 0.6;
        assert!(grip.set_style(&style));
        assert_eq!(
            grip.items()[1].ghost_alpha(),
            0.6,
            "the ghost alpha reaches the stroke immediately"
        );
        assert!(grip.update(&camera), "the style forces a rebuild");
        assert!(!grip.set_style(&style), "the same style is a no-op");

        let point = |item: &ShapeItem, index: usize| {
            let positions = item.positions();
            Vector3::new(
                f64::from(positions[index][0]),
                f64::from(positions[index][1]),
                f64::from(positions[index][2]),
            )
        };
        let per_pixel = scale_for(&camera, grip.top(), 1.0);
        let stroke = &grip.items()[1];
        assert_eq!(stroke.surface_color(), [0.0, 1.0, 0.0, 0.4]);
        let width = point(stroke, 0).sub(&point(stroke, 3)).length();
        let expected = style.width * per_pixel;
        assert!(
            (width - expected).abs() < 1e-6,
            "a 5 px stroke: {width} vs {expected}"
        );

        // The casing is the style's outline ink, dimmed by the opacity.
        let casing = &grip.items()[0];
        assert_eq!(casing.surface_color(), [0.0, 0.0, 1.0, 0.5]);
        let casing_width = point(casing, 0).sub(&point(casing, 3)).length();
        let expected = (style.width + 2.0 * style.halo_width) * per_pixel;
        assert!(
            (casing_width - expected).abs() < 1e-6,
            "the casing is stroke + 2·halo: {casing_width} vs {expected}"
        );

        // The active token is used while hovered, still scaled by the opacity.
        assert!(grip.set_hovered(true));
        assert_eq!(grip.items()[1].surface_color(), [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(grip.style(), &style);
    }

    #[test]
    fn a_click_along_the_stem_begins_a_drag() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        let top = project_device(&camera, grip.top()).expect("the top");
        let tip = project_device(&camera, grip.stem_end(&camera)).expect("the stem tip");
        assert!(tip[1] > top[1], "the stem rises on screen");

        // Halfway up the stem, away from both the top-face anchor and the crossbar.
        let click = [
            ((top[0] + tip[0]) * 0.5).round() as i32,
            ((top[1] + tip[1]) * 0.5).round() as i32,
        ];
        assert!(grip.hit(&camera, click));
        assert!(grip.begin_drag(&camera, click));
        assert!(grip.is_dragging());
    }

    #[test]
    fn a_face_pick_still_offers_the_grip_but_an_unknown_selection_does_not() {
        let stage = slab(7);
        assert!(
            height_grip(&SelectionRef::element(7, "Face3"), &stage).is_some(),
            "a face pick keeps the height grip: the height is the body's"
        );
        assert!(height_grip(&SelectionRef::whole(9), &stage).is_none());
        assert!(height_grip(&SelectionRef::whole(7), &Stage::new()).is_none());
    }

    #[test]
    fn the_gizmo_sits_at_the_items_base_corner() {
        let stage = slab(7);
        let gizmo = move_gizmo(&SelectionRef::whole(7), &stage).expect("a gizmo");
        assert_eq!(gizmo.target_id(), 7);
        let origin = gizmo.origin();
        assert!((origin.x + 20.0).abs() < 1e-9, "x = {}", origin.x);
        assert!((origin.y + 20.0).abs() < 1e-9, "y = {}", origin.y);
        assert!(origin.z.abs() < 1e-9, "z = {}", origin.z);
        assert!(
            move_gizmo(&SelectionRef::element(7, "Face1"), &stage).is_some(),
            "a face pick keeps the handles"
        );
    }

    #[test]
    fn a_grip_anchor_follows_the_object() {
        let mut grip = grip();
        let moved = grip.top().add(&Vector3::new(5.0, 0.0, 2.0));
        assert!(grip.set_anchor(moved), "the anchor moved");
        assert_eq!(grip.top(), moved);
        assert!(!grip.set_anchor(moved), "an unchanged anchor is a no-op");
    }

    #[test]
    fn dragging_up_grows_the_height_and_commits_it() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(grip.is_dragging());
        assert!(grip.drag_to(&camera, [grab[0], grab[1] + 25]).is_some());
        let commit = grip.end_drag().expect("a commit");

        assert_eq!(commit.manipulator, TOY_HEIGHT_ID);
        assert_eq!(commit.target, 7);
        assert_eq!(commit.action, "set");
        assert!(commit.values[0] > 20.0, "dragging up grew the height");
        assert_eq!(commit.readout, format!("H {:.2} mm", commit.values[0]));
        assert!(!grip.is_dragging());
    }

    #[test]
    fn cancelling_restores_the_press_height() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(grip.drag_to(&camera, [grab[0], grab[1] + 40]).is_some());
        assert!(grip.cancel_drag());
        assert!(!grip.is_dragging());
        assert_eq!(grip.height(), 20.0);
        assert_eq!(grip.end_drag(), None);
    }

    #[test]
    fn the_height_never_goes_below_the_minimum() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(
            grip.drag_to(&camera, [grab[0], grab[1] - 100_000])
                .is_some()
        );
        let commit = grip.end_drag().expect("a commit");
        assert!(commit.values[0] >= MIN_HEIGHT);
    }

    #[test]
    fn a_zero_halo_width_draws_no_casing() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        grip.update(&camera);
        assert!(!grip.items()[0].positions().is_empty());

        let mut style = grip.style().clone();
        style.halo_width = 0.0;
        assert!(grip.set_style(&style));
        assert!(grip.update(&camera));
        assert!(
            grip.items()[0].positions().is_empty(),
            "a zero halo width leaves only the stroke"
        );
    }

    #[test]
    fn hovering_widens_the_stroke_and_the_casing() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        grip.update(&camera);
        let width = |grip: &HeightGrip, slot: usize| {
            let positions = grip.items()[slot].positions();
            let point = |index: usize| {
                Vector3::new(
                    f64::from(positions[index][0]),
                    f64::from(positions[index][1]),
                    f64::from(positions[index][2]),
                )
            };
            point(0).sub(&point(3)).length()
        };
        let idle_stroke = width(&grip, 1);

        assert!(grip.set_hovered(true));
        assert!(
            grip.update(&camera),
            "the width cue is baked, so hover must rebuild"
        );
        let scale = grip.style().hover_scale;
        assert!(
            (width(&grip, 1) / idle_stroke - scale).abs() < 1e-5,
            "the hovered stroke is {scale}× wider"
        );
        // The casing follows the widened stroke, so the halo on each side is unchanged.
        let per_pixel = scale_for(&camera, grip.top(), 1.0);
        let expected = width(&grip, 1) + 2.0 * grip.style().halo_width * per_pixel;
        assert!(
            (width(&grip, 0) - expected).abs() < 1e-5,
            "the hovered casing keeps its halo: {} vs {expected}",
            width(&grip, 0)
        );
    }

    #[test]
    fn the_grab_radius_follows_the_style() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        let top = project_device(&camera, grip.top()).expect("the top");
        let tip = project_device(&camera, grip.stem_end(&camera)).expect("the stem tip");
        let middle = [
            ((top[0] + tip[0]) * 0.5).round() as i32,
            ((top[1] + tip[1]) * 0.5).round() as i32,
        ];
        // 15 px to the side of the stem is outside the default 12 px radius.
        let off = [middle[0] + 15, middle[1]];
        assert!(!grip.hit(&camera, off));

        let mut style = grip.style().clone();
        style.hit_radius_px = 20.0;
        assert!(grip.set_style(&style));
        assert!(
            grip.hit(&camera, off),
            "the configured radius reaches 15 px"
        );
    }

    #[test]
    fn a_click_off_the_grip_does_not_start_a_drag() {
        let mut grip = grip();
        let camera = camera(200.0);
        assert!(!grip.begin_drag(&camera, [0, 0]));
        assert!(!grip.is_dragging());
    }
}

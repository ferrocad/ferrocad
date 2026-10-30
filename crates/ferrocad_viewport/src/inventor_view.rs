//! Presenting an [`InventorView`] inside a gpui window (the image path).
//!
//! `bite-gpui` exposes no wgpu shared-surface primitive yet — its `PaintSurface`
//! primitive is macOS/CoreVideo only, and the wgpu backend treats
//! `PrimitiveBatch::Surfaces` as a no-op — so this presents the rendered frame the
//! MVP way: draw, read the pixels back with [`InventorView::image`], wrap them as a
//! gpui [`RenderImage`], and paint them with [`img`].
//!
//! The viewport also owns the interaction loop, mirroring FreeCAD's
//! `View3DInventorViewer`: pointer events arrive in **logical** coordinates, are
//! converted to device pixels through the camera's [`Viewport`], picked against the
//! compiled scene, and delivered to an [`EventListener`] (the `SoEventCallback`
//! analogue) which may edit the scene — for example, to highlight a hovered object.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::{
    Camera, DepthState, DrawPacket, Generation, Geometry, Hit, MeshId, PickId, Projection,
    RenderQueue, SHADOW_MAX, ShaderProgram, ShadowFootprint, StateSnapshot, Topology, Vertex,
    Viewport, apply_shadows, pick_at_hit,
};
use ferrocad_graphics::WorkplaneFrame;
use ferrocad_inventor::{
    Directives, Event, EventListener, ImageData, InventorView, NaviCubeDirective, Node, NodeKind,
    Scene, ViewSpec, compile_diagnostic,
};
use ferrocad_stage::Stage;
use ferrocad_stage::shape::{BLENDED_DEPTH, GHOST_DEPTH, WIRE_DEPTH_BIAS};
use ferrocad_stage::{
    AxisStyle, FaceFrame, GizmoCommitEvent, GizmoMode, GizmoModifiers, GizmoSet, GizmoStyle,
    GizmoTarget, GizmoValue, GridSpec, GridStyle, InteractionCommit, Manipulator,
    MouseButton as StageButton, PointerEvent, PointerKind, SnapCandidate, SnapContext, SnapKind,
    Tool, ToolStatus, TransformGizmo, ViewProjection, Workplane, axis, feedback, grid,
    snap_to_geometry_with,
};
use ferrocad_stage::screen::{camera_position, project_device, scale_for};
use ferrocad_theme::ui::{CHROME_ACTIVE, CHROME_ACTIVE_BG, CHROME_LINE, FG, MUTED};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};
use gpui::{
    AnyElement, AppContext, Bounds, Context, DragMoveEvent, EventEmitter, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Window, div,
    gpu_canvas, img, point, prelude::*, px, rgb, rgba,
};
use image::{Frame, ImageBuffer, Rgba};

use crate::navicube::{CHROME_WIDTH, Corner, NaviCube, NavicubeLabel, YawDirection, SnapAnimation};
use crate::presenter::{Presenter, PresenterStatus};

/// Where a click landed in the world.
///
/// A host anchors a gizmo here rather than at an abstract body centre: the control
/// belongs to the point of contact (the click on a face, edge or vertex), so dragging
/// it and reading its deltas stay local to what the user pointed at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionAnchor {
    /// The world-space intersection point.
    pub point: Vector3,
    /// The world-space surface normal at the hit (zero when the producer has none).
    pub normal: Vector3,
}

/// A selection update the viewport reports after a click.
///
/// The viewport reports **what was hit**, never a document object:
/// [`SelectionRef::object`] is the picked [`PickId`] and its `elements` are the kernel
/// names the mesh carries (`Face6`). The host maps the id to a document object; a
/// viewport never depends on the document or on a kernel.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectionEvent {
    /// The click hit an object, whole or through one of its named sub-elements.
    Picked {
        reference: SelectionRef,
        /// The world point of the click, for anchoring the gizmo. `None` when the
        /// selection did not come from a viewport pick (a tree or command selection).
        anchor: Option<SelectionAnchor>,
        /// How many clicks the press was part of (gpui's `MouseDownEvent::click_count`):
        /// `1` for a single click, `2` for the second click of a double click. The
        /// viewport reports the sub-element it hit; the host uses the count to decide
        /// whether to keep that scope or upgrade to the whole body.
        click_count: usize,
    },
    /// The click hit nothing identifiable, so the selection should clear.
    Cleared,
}

/// What a click under the pointer would do, for the host's status line.
///
/// The viewport reports only neutral data: the scene target a click would select
/// (if any), and the overlay whose **interior** is under the pointer but will not
/// consume the click (`"navicube"` today). The shell turns this into text; no
/// document or workbench concept enters the viewport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverReport {
    /// What the click would hit in the document.
    pub target: Option<SelectionRef>,
    /// The overlay under the pointer that will not consume the click.
    pub through: Option<&'static str>,
}

/// A live gizmo drag, reported to a host that previews the whole selection itself.
#[derive(Clone, Debug, PartialEq)]
pub struct TransformPreview {
    /// The targeted entity.
    pub target_id: u32,
    /// Where the drag is in its lifecycle.
    pub phase: PreviewPhase,
    /// The transform accumulated since the drag began.
    pub total_transform: Matrix4,
}

/// The lifecycle of a [`TransformPreview`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewPhase {
    /// The handle was grabbed; the host captures its current values as a baseline.
    Begin,
    /// The pointer moved; `total_transform` is relative to the drag's start.
    Update,
    /// The button was released. Nothing was committed; the host owns the decision.
    End,
}

/// The overlay anchors captured when a live preview began.
///
/// A preview is absolute (the host reports the distance from the object's document
/// pose), so each tick must offset from the **same** captured pose rather than from
/// the previous tick.
#[derive(Clone, Debug, Default)]
struct PreviewHome {
    /// The hosted gizmo entries that were on offer, with their origins.
    gizmos: Vec<(usize, Vector3)>,
    manipulators: Vec<Option<Vector3>>,
}

/// A state change the viewport reports to its host on gpui's event channel.
///
/// The viewport itself never applies a selection or writes a document; it reports what
/// happened and lets the host (the shell) decide. [`EventEmitter`] queues the emission
/// until the current update returns, so a subscriber may update the viewport in turn.
#[derive(Clone, Debug)]
pub enum ViewportEvent {
    /// A click selected an object or sub-element, or cleared the selection.
    Selection(SelectionEvent),
    /// A workbench manipulator committed a value for the host to apply.
    Interaction(InteractionCommit),
    /// A gizmo drag ended. The host converts it with
    /// [`GizmoCommitEvent::to_interaction`] and routes it like any other commit.
    Transform(GizmoCommitEvent),
    /// A gizmo drag tick while the host has deferred the commit. The host mirrors the
    /// delta into its editable values and previews the whole selection itself.
    TransformPreview(TransformPreview),
    /// The origin/centre handle was dragged and released. `point` is the snapped world
    /// pivot and `kind` names the snap that won. This is **not** a transform commit: the
    /// host stores it as session state (it is not undoable and not a document property).
    GizmoOrigin {
        /// The snapped world point the pivot moved to.
        point: Vector3,
        /// What the point snapped to.
        kind: SnapKind,
    },
    /// The active tool's status changed: it started, gained a point, finished, or was
    /// cancelled. `status.active` is false once the session ended, so the host hides
    /// its tool strip.
    ToolState(ToolStatus),
}

/// The host's grid-step policy: the configured levels, and whether a plane's own extent
/// may override them (the **adaptive** setting).
///
/// One policy serves every layer — the world ground, the bound plane, the hover preview,
/// the program (Coin) frame and the snap context — so switching it cannot make two grids
/// disagree about which step is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPolicy {
    /// The configured minor (dot) step, in world units.
    pub minor: f64,
    /// The configured major (line) step, in world units.
    pub major: f64,
    /// Adaptive spacing: derive the pair from the extent the layer covers — a bound
    /// surface's in-plane radius, or the ground's disc radius.
    pub adaptive: bool,
    /// Whether the minor level is dots or a line lattice.
    pub minor_style: grid::MinorStyle,
    /// The major lines' stroke width, in device pixels.
    pub major_width: f64,
    /// The minor lattice's stroke width, in device pixels.
    pub minor_width: f64,
    /// The ink multiplier over the token alphas (`FerroCAD.Grid.opacity`). The resolved
    /// level alphas travel with the Coin frame, so the shader draws the same weight as
    /// the native generator instead of a strength of its own.
    pub alpha: f32,
}

impl Default for GridPolicy {
    fn default() -> Self {
        Self {
            minor: grid::MINOR_STEP,
            major: grid::MAJOR_STEP,
            adaptive: true,
            minor_style: grid::MinorStyle::default(),
            major_width: 1.0,
            minor_width: 1.0,
            alpha: 1.0,
        }
    }
}

// The viewport's active construction plane: the plane plus what to call it.
///
/// There is **always** an active plane — the world `XY` plane is the default and is
/// present from the first frame. `None` never means “no workplane”; it means “the world
/// plane”, which is why this is not an `Option`.
///
/// `label` is what the `WP` pill and the status line show (`"XY"`, `"Face3"`,
/// `"Datum"`); `plane.kind` stays the semantic source (the label is presentation, and a
/// world-aligned `XZ`/`YZ` plane keeps `kind = World` while reading `"XZ"`).
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveWorkplane {
    /// The plane: origin plus in-plane axes (the normal is `u × v`).
    pub plane: Workplane,
    /// The presentation label **without** the offset: the datum, face or `View`.
    pub label: String,
    /// The distance the plane has been shifted along its own normal, in mm, from the
    /// frame it was bound to. `0` is that frame itself.
    pub offset: f64,
    /// The in-plane radius of the **finite surface** the plane is bound to, if any.
    ///
    /// This is the grid's extent rule, not a preference: a plane on a face draws a patch
    /// around that face instead of the camera-following ground, so a tilted plane cannot
    /// fill the view with a wall of lines. `None` for every plane with no surface behind
    /// it (world, standard, view, three points).
    pub extent: Option<f64>,
}

impl ActiveWorkplane {
    /// The default plane: world `XY` at the origin.
    pub fn world() -> Self {
        Self {
            plane: Workplane::world(),
            label: "XY".to_string(),
            offset: 0.0,
            extent: None,
        }
    }

    /// The plane of a resolved face, labelled by its element (`Face3`) or datum name.
    pub fn of(frame: &FaceFrame) -> Self {
        let label = match &frame.plane.kind {
            ferrocad_stage::WorkplaneKind::Face { element, .. } if !element.is_empty() => {
                element.clone()
            }
            ferrocad_stage::WorkplaneKind::Face { .. } => "Face".to_string(),
            ferrocad_stage::WorkplaneKind::Datum(name) => name.clone(),
            ferrocad_stage::WorkplaneKind::World => "XY".to_string(),
            ferrocad_stage::WorkplaneKind::Custom => "Custom".to_string(),
        };
        Self {
            plane: frame.plane.clone(),
            label,
            offset: 0.0,
            // A face is the one bound surface with a real in-plane size to draw.
            extent: Some(frame.radius),
        }
    }

    /// One of the three world-standard axis planes, labelled `XY`/`XZ`/`YZ`.
    pub fn standard(plane: ferrocad_stage::StandardPlane) -> Self {
        Self {
            plane: ferrocad_stage::Workplane::standard(plane),
            label: plane.label().to_string(),
            offset: 0.0,
            extent: None,
        }
    }

    /// A transient custom frame (the camera's view plane, an offset, a 3-point pick),
    /// labelled `label`.
    pub fn custom(frame: ferrocad_stage::Workplane, label: impl Into<String>) -> Self {
        Self {
            plane: frame,
            label: label.into(),
            offset: 0.0,
            extent: None,
        }
    }

    /// The program (Coin) grid's extent is the **same** radius the native grid draws, so the
    /// two backends show the same patch.
    ///
    /// A bound plane gets `max(3 × extent, 2 × major)` on its own effective major step; an
    /// unbound one gets the user's fixed extent when "Grid extent" is set, else the
    /// camera-following ground — **never** `0`. A zero radius means "unbounded" in the
    /// ABI, but sending it for a vertical or tilted plane left the plane cut only by the
    /// shader's edge-on fade: the locus where the view ray is parallel to the plane drew
    /// as a soft band straight through the model.
    ///
    /// The radius is host state, not a mod parameter, so a mod cannot claim a plane the
    /// viewport is not on.
    pub fn program_patch_radius(
        &self,
        camera: &Camera,
        policy: GridPolicy,
        fixed: Option<f64>,
    ) -> f32 {
        let (_, major) = self.effective_steps(camera, policy, fixed);
        self.radius_for(camera, major, fixed)
    }

    /// The `(minor, major)` steps this plane draws with: a bound surface's own, the
    /// ground's (adaptive) or the configured world steps otherwise, then the screen-space
    /// cull.
    ///
    /// The Coin grid draws exactly these two levels, so this is also what the shader
    /// receives in `Frame.workplane_steps`.
    fn effective_steps(
        &self,
        camera: &Camera,
        policy: GridPolicy,
        fixed: Option<f64>,
    ) -> (f64, f64) {
        let (minor, major) = plane_steps(self.extent, ground_radius(camera, fixed), policy);
        grid::effective_steps(camera, &self.plane, minor, major)
    }

    /// The patch radius for an already-effective `major` step.
    fn radius_for(&self, camera: &Camera, major: f64, fixed: Option<f64>) -> f32 {
        match patch_radius(self.extent, major) {
            Some(patch) => patch as f32,
            None => ground_radius(camera, fixed) as f32,
        }
    }

    /// [`plane`](Self::plane) as the renderer-neutral [`WorkplaneFrame`] a [`ViewSpec`]
    /// carries, with the program grid's patch radius as [`radius`](WorkplaneFrame::radius).
    pub fn view_workplane(
        &self,
        camera: &Camera,
        policy: GridPolicy,
        fixed: Option<f64>,
    ) -> WorkplaneFrame {
        let (minor, major) = self.effective_steps(camera, policy, fixed);
        let plane = &self.plane;
        WorkplaneFrame {
            origin: [
                plane.origin.x as f32,
                plane.origin.y as f32,
                plane.origin.z as f32,
            ],
            u: [plane.u.x as f32, plane.u.y as f32, plane.u.z as f32],
            v: [plane.v.x as f32, plane.v.y as f32, plane.v.z as f32],
            n: [plane.n.x as f32, plane.n.y as f32, plane.n.z as f32],
            minor: minor as f32,
            major: major as f32,
            radius: self.radius_for(camera, major, fixed),
            minor_alpha: base_ink(self.extent, policy).0,
            major_alpha: base_ink(self.extent, policy).1,
            minor_lines: policy.minor_style == grid::MinorStyle::Lines,
            major_width: stroke_width(camera, plane, policy.major_width) as f32,
            minor_width: stroke_width(camera, plane, policy.minor_width) as f32,
        }
    }

    /// What the pill and the status line show: the name, plus the offset when the plane
    /// was shifted along its normal (`XY +25`, `Face3 -4`).
    pub fn display_label(&self) -> String {
        if self.offset == 0.0 {
            return self.label.clone();
        }
        let amount = format_offset(self.offset);
        let sign = if self.offset > 0.0 { "+" } else { "" };
        format!("{} {sign}{amount}", self.label)
    }

    /// Whether this is the default world `XY` plane (the only state that is “unbound”).
    pub fn is_default(&self) -> bool {
        self.offset == 0.0
            && self.label == "XY"
            && matches!(self.plane.kind, ferrocad_stage::WorkplaneKind::World)
    }
}

/// A millimetre amount for a label: up to three decimals, trailing zeros trimmed.
fn format_offset(amount: f64) -> String {
    let rounded = format!("{amount:.3}");
    let trimmed = rounded.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-0" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A gpui view that presents an [`InventorView`] (image path) and routes pointer
/// events into the scene.
pub struct InventorViewport<V: InventorView> {
    view: V,
    camera: Camera,
    scene: Scene,
    /// Declarative items drawn beside the inventor scene (the modern stack). They are
    /// compiled into the same queue, so both render in one pass over one depth buffer.
    stage: Stage,
    /// Set when the stage changed, so the queue is rebuilt before the next draw.
    stage_dirty: bool,
    /// The active interactive tool (the `SoEventCallback` replacement), if any.
    tool: Option<Box<dyn Tool>>,
    /// A tool status waiting to be emitted as [`ViewportEvent::ToolState`] by the next
    /// gpui handler. The pointer/key entry points are headless (no `cx`), so they queue
    /// here and the window handlers drain it.
    tool_state: Option<ToolStatus>,
    /// The tool's transient visuals, rebuilt on every dispatched event. Drawn as an
    /// overlay (on top, depth off) and never picked, so a rubber band cannot shadow the
    /// geometry under the cursor.
    transient: RenderQueue,
    /// The grid spacing for grid snapping, if enabled.
    snap_grid: Option<f64>,
    /// The **hosted set** of transform gizmos, in registry hit-priority order (the first
    /// entry answers the pointer) and narrowed to the session kind. A workbench gizmo and
    /// the shared triad can coexist; the set keeps the kinds it is not currently offering
    /// so [`set_gizmo_mode`](Self::set_gizmo_mode) re-narrows it in place. Drawn on top
    /// and picked **before** the tool and the scene, so a drag on a handle takes priority.
    gizmos: GizmoSet,
    /// The gizmo's handle batches (rebuilt when its scale, origin, mode or hover change).
    gizmo_queue: RenderQueue,
    /// The authored program that skins the gizmo's handles. The WGSL is supplied by
    /// the active provider through the shell (the shader ships with a mod), so this
    /// crate stays GPU- and source-agnostic: the viewport only knows that a skin
    /// program was injected.
    gizmo_skin: Option<Arc<ShaderProgram>>,
    /// The last gizmo commit, for the host to apply to the document.
    gizmo_commit: Option<GizmoCommitEvent>,
    /// The workbench manipulators (the edge fillet arc, a thickness grip, …), in
    /// registration order. They are the most specific overlays: dispatched in reverse
    /// order, before the gizmo, the tool and the scene.
    manipulators: Vec<Box<dyn Manipulator>>,
    /// Their compiled overlay, rebuilt when any of them reports a change.
    manipulator_queue: RenderQueue,
    /// The last manipulator commit, for the host to apply.
    interaction_commit: Option<InteractionCommit>,
    /// The viewport's focus handle, created on the first paint. The Esc handler and the
    /// modifier state make the element focusable.
    focus: Option<FocusHandle>,
    /// The latest gizmo drag readout, while a drag is in progress.
    gizmo_value: Option<GizmoValue>,
    /// The logical pointer position of the latest gizmo drag tick (the ring tag anchor).
    gizmo_pointer: Option<[f32; 2]>,
    /// While set, a gizmo drag reports [`ViewportEvent::TransformPreview`] and never
    /// commits on release: the host previews the whole selection and commits when its
    /// own editor accepts. Set by the host while a translation HUD is open.
    gizmo_deferred: bool,
    /// The in-progress re-pivot: the frame origin at the press (for `Esc`) and the
    /// snapped point/kind under the pointer.
    gizmo_origin_drag: Option<GizmoOriginDrag>,
    /// The released re-pivot waiting to be emitted as [`ViewportEvent::GizmoOrigin`].
    gizmo_origin: Option<(Vector3, SnapKind)>,
    /// A drag tick waiting for the next gpui handler to emit it.
    transform_preview: Option<TransformPreview>,
    /// The overlay anchors a live preview offsets from (see [`Self::preview_translation`]).
    preview_home: Option<PreviewHome>,
    queue: RenderQueue,
    /// The semantic directives the last compile produced (active `SoEventCallback` ids,
    /// navicube overlays). Invisible to the renderer; the viewport consumes them.
    directives: Directives,
    listeners: Vec<Box<dyn EventListener>>,
    /// The navigation-cube overlay, when enabled.
    navicube: Option<NaviCube>,
    /// Overlays built from `Directive::NaviCube` — one per directive, in scene order,
    /// beside the Rust-constructed `navicube`. Rebuilt only when the directive set
    /// changes (a camera move leaves it alone).
    directive_cubes: Vec<(NaviCubeDirective, NaviCube)>,
    /// An in-progress navigation-cube orbit drag (from the first `on_drag_move`).
    navicube_drag: Option<NavicubeDrag>,
    /// An in-progress main-camera orbit drag, started by a press on the scene.
    orbit_drag: Option<OrbitDrag>,
    /// An in-progress compass-ring scrub.
    ring_drag: Option<RingDrag>,
    /// The smoothed orbit velocity while dragging (logical px/s).
    orbit_velocity: [f32; 2],
    /// When the orbit velocity was last sampled.
    last_drag_time: Option<std::time::Instant>,
    /// The inertial orbit running after the pointer was released.
    inertia: Option<Inertia>,
    /// An in-progress animated snap of the camera onto a cube piece.
    snap: Option<SnapAnimation>,
    /// The object currently under the pointer.
    hover: Option<PickId>,
    /// The sub-element id ([`Hit::part`]) under the pointer, mirroring
    /// [`hover`](Self::hover).
    hovered_part: Option<u32>,
    /// The selection update produced by the last click, for the host to consume.
    selection: Option<SelectionEvent>,
    /// gpui's click count for the most recent left press. The release reports it with
    /// the selection so the host can tell a single click from a double click; the
    /// viewport itself only resolves what was hit.
    press_click_count: usize,
    /// A neutral summary of what a click would do under the pointer (A4).
    hover_report: HoverReport,
    /// The world point of the hovered pick, for a host that anchors a contextual
    /// action (the `Space` wheel, a gizmo) where the user is pointing.
    hover_anchor: Option<SelectionAnchor>,
    /// The reference grid's geometry (G2). Always allocated: `grid_enabled` decides
    /// whether the ground field is drawn and `axis.enabled` whether the triads are.
    grid: GridMesh,
    /// The world grid's visibility (`App_ToggleGrid`).
    grid_enabled: bool,
    /// The world triad, its origin node and axis tags (`App_ToggleAxisCross`). On by
    /// default, as upstream's axis cross is. The style also carries the colours, the
    /// screen-invariant geometry and the origin/label toggles.
    axis: AxisStyle,
    /// The workspace gizmo appearance and behaviour, shared by the transform gizmo and
    /// every hosted manipulator. It is also the host policy for `enabled` (draw/pick at
    /// all) and `show_readout` (the numeric pills).
    gizmo_style: GizmoStyle,
    /// The world grid's minor and major steps (mm, G5 preferences). With adaptive spacing
    /// these are the fallback, not the drawn step.
    grid_minor: f64,
    grid_major: f64,
    /// Whether the step follows the plane's extent (`FerroCAD.Grid.adaptive`) instead of
    /// the configured pair above.
    grid_adaptive: bool,
    /// The minor level's form (`FerroCAD.Grid.minor_style`).
    grid_minor_style: grid::MinorStyle,
    /// The major lines' stroke width, in device pixels (`FerroCAD.Grid.major_width`).
    grid_major_width: f64,
    /// The minor lattice's stroke width, in device pixels (`FerroCAD.Grid.minor_width`).
    grid_minor_width: f64,
    /// A fixed world-grid extent (mm); `None` derives it from the camera.
    grid_radius: Option<f64>,
    /// The world grid's linear ink (`FerroCAD.Grid.color`).
    grid_color: [f32; 4],
    /// A multiplier over the token alphas (`FerroCAD.Grid.opacity`).
    grid_alpha: f32,
    /// The active local workplane (G3): sticky until `Esc` or another activation.
    active_workplane: ActiveWorkplane,
    /// The plane to restore when an **ephemeral** binding ends: the first click of a
    /// creation tool on a planar face binds that face for the session (there is no
    /// explicit activation step), and finishing or cancelling restores this.
    /// `None` means no ephemeral binding is in force.
    ephemeral_restore: Option<ActiveWorkplane>,
    /// The face under the pointer, resolved from the stage's display tessellation. Its
    /// grid blooms after [`FACE_DWELL`].
    hover_face: Option<FaceHover>,
    /// The grid's packets. Kept apart from [`queue`](Self::queue) so the grid draws in
    /// the same depth pass but never enters a pick: it is scaffolding, not geometry.
    grid_queue: RenderQueue,
    /// Whether the authored (shader) grid replaces the native ground
    /// (`ViewState.grid_overlay`, G4). When true the native ground (dots and lines) is
    /// hidden; the triad and the workplane are untouched. This flag never gates the
    /// drawn program packets: a registered overlay is drawn whenever it has one.
    grid_overlay: bool,
    /// The scene's program packets — those whose state carries a `ShaderProgram` — split
    /// out of [`queue`](Self::queue) so they draw **after** opaque geometry, in the same
    /// depth-tested pass, and can never enter a pick. Their parameters carry the scene's
    /// ground-shadow block (see [`shadow_footprints`](Self::shadow_footprints)).
    program_queue: RenderQueue,
    /// Explicit ground-shadow footprints (world `x`/`y`), or `None` to derive them from
    /// the [`stage`](Self::stage) on every recompile. A host whose document is not a
    /// `Stage` (the legacy inventor path) sets these with
    /// [`set_shadow_footprints`](Self::set_shadow_footprints).
    shadow_footprints: Option<Vec<ShadowFootprint>>,
    /// The last shader-program assembly error, if any. The frame still renders (the
    /// malformed program is dropped, not the scene); the shell reports and clears it.
    shader_error: Option<String>,
    /// Pull-registered overlay scenes (G5). Merged under a synthetic separator beside
    /// the base scene on every recompile; the base scene itself is never modified.
    overlays: Vec<Scene>,
    /// The last pointer position in **device** pixels (bottom-left origin), for the
    /// workplane origin's proximity bloom.
    pointer: Option<[i32; 2]>,
    /// The snap under a tool's pointer (G4): the aurora, the rays and the pill's data.
    snap_feedback: Option<SnapFeedback>,
    /// The feedback's packets, drawn as an overlay above the scene.
    feedback_queue: RenderQueue,
    /// Stable meshes for the feedback, so a per-frame rebuild re-uploads rather than
    /// leaking a mesh identity.
    feedback_meshes: FeedbackMeshes,
    frame: Option<Arc<RenderImage>>,
    /// A host-supplied zero-copy presenter. When ready, its GPU target replaces the
    /// readback `frame`; `None` (the default) is the image path.
    presenter: Option<Rc<RefCell<Box<dyn Presenter>>>>,
    /// Frames spent waiting for the presenter's first paint. Bounds the startup loop
    /// when the canvas is never painted (a hidden or zero-sized element).
    presenter_frames: u8,
    /// The element's window-space bounds, captured at prepaint. gpui reports pointer
    /// positions relative to the **window**, so this is subtracted to make them
    /// viewport-local (see [`to_local`](Self::to_local)).
    last_bounds: Option<Bounds<Pixels>>,
}

/// A navigation-cube orbit drag in progress.
struct NavicubeDrag {
    /// The last logical pointer position, for the next delta.
    last: [f32; 2],
    /// Accumulated movement, so a click (no drag) can snap instead of orbiting.
    moved: f32,
}

/// A main-camera orbit drag in progress.
struct OrbitDrag {
    /// The last logical pointer position, for the next delta.
    last: [f32; 2],
    /// Accumulated movement, so a click is not treated as a drag.
    moved: f32,
}

/// An inertial orbit after the pointer was released: the last (smoothed) velocity in
/// **logical pixels per second**, and when it was last integrated.
struct Inertia {
    velocity: [f32; 2],
    last: std::time::Instant,
}

/// A compass-ring scrub in progress.
struct RingDrag {
    /// The pointer's angle around the ring center, in radians (CCW from screen +X).
    last_angle: f64,
    /// The projection captured when the press began. The turntable is inside the rotating
    /// subtree, so measuring against the live rotation would feed the yaw we apply back
    /// into the next pointer angle; freezing it makes the scrub integrate exactly.
    reference: Rotation,
}

/// A re-pivot in progress. The frame moves live; `home` is what `Esc` restores.
#[derive(Clone, Copy, Debug)]
struct GizmoOriginDrag {
    /// The hosted entry whose origin handle was grabbed.
    index: usize,
    home: Vector3,
    point: Vector3,
    kind: SnapKind,
}

/// Yaw detents, in degrees: the ring snaps to these magnetic headings on release.
const RING_DETENT_DEGREES: f64 = 90.0;
/// How close (in degrees) a release must be to a detent to be captured.
const RING_DETENT_CAPTURE: f64 = 10.0;

/// The reference grid's cached geometry and its stable mesh identities.
///
/// One [`GridLayer`] is the world ground; two more are local: the **active** workplane
/// and a **hovered** face being previewed. The local layers are allocated once and
/// hidden when unused, so their mesh identities stay stable (a hover that comes and
/// goes never leaks a mesh the renderer has to keep). A layer's ground (dots and major
/// lines) is keyed on its spec and ink, so an orbit does not rebuild it; its triad is
/// keyed on the camera, because its ribbons are screen-facing. A rebuild bumps only
/// that half's generation, so the renderer re-uploads just it.
struct GridMesh {
    world: GridLayer,
    active: GridLayer,
    hover: GridLayer,
    /// The workplane origin's proximity node (G4).
    origin: MarkerLayer,
}

impl GridMesh {
    fn new() -> Self {
        Self {
            world: GridLayer::new(0.0),
            active: GridLayer::new(WIRE_DEPTH_BIAS),
            hover: GridLayer::new(WIRE_DEPTH_BIAS),
            origin: MarkerLayer::new(),
        }
    }

    /// Sync the world layer and the two local layers. Each layer draws its ground field
    /// and its triad independently: `App_ToggleGrid` owns the fields, `App_ToggleAxisCross`
    /// owns the triads.
    fn sync(
        &mut self,
        camera: &Camera,
        world: LayerSpec<'_>,
        active: Option<LayerSpec<'_>>,
        hover: Option<LayerSpec<'_>>,
    ) {
        sync_layer(&mut self.world, camera, Some(world));
        sync_layer(&mut self.active, camera, active);
        sync_layer(&mut self.hover, camera, hover);
    }

    /// Sync the origin node: on the active workplane when one is live (coplanar, so
    /// depth-biased), else on the world frame under the world triad. It belongs to the
    /// axis cross, not the grid.
    fn sync_origin(
        &mut self,
        workplane: &Workplane,
        camera: &Camera,
        bloomed: bool,
        depth_bias: f32,
        visible: bool,
    ) {
        self.origin.depth_bias = depth_bias;
        self.origin.visible = visible;
        self.origin.sync(workplane, camera, bloomed);
    }

    fn emit(&self, queue: &mut RenderQueue) {
        self.world.emit(queue);
        self.active.emit(queue);
        self.hover.emit(queue);
        self.origin.emit(queue);
    }
}

/// What one grid layer should draw this frame.
#[derive(Clone, Copy)]
struct LayerSpec<'a> {
    spec: &'a GridSpec,
    style: &'a GridStyle,
    /// The triad's ink and geometry (the axis-cross style).
    axis: &'a AxisStyle,
    /// The dots and major lines (the grid toggle).
    ground: bool,
    /// The triad stems at the layer's origin (the axis-cross toggle).
    triad: bool,
}

/// A single-mesh layer in the grid queue (the workplane origin node).
struct MarkerLayer {
    mesh: MeshId,
    generation: Generation,
    key: Option<u64>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    depth_bias: f32,
    /// The axis-cross toggle owns this node, not the grid.
    visible: bool,
}

impl MarkerLayer {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1 << 39);
        Self {
            mesh: MeshId(NEXT.fetch_add(1, Ordering::Relaxed)),
            generation: Generation(0),
            key: None,
            vertices: Vec::new(),
            indices: Vec::new(),
            depth_bias: 0.0,
            visible: false,
        }
    }

    fn sync(&mut self, workplane: &Workplane, camera: &Camera, bloomed: bool) {
        let key = feedback::origin_key(workplane, camera, bloomed);
        if self.key == Some(key) {
            return;
        }
        self.vertices = feedback::origin_node(workplane, camera, bloomed);
        self.indices = (0..self.vertices.len() as u32).collect();
        self.generation = Generation(self.generation.0 + 1);
        self.key = Some(key);
    }

    fn emit(&self, queue: &mut RenderQueue) {
        if !self.visible || self.vertices.is_empty() {
            return;
        }
        queue.packets.push(DrawPacket {
            mesh_id: self.mesh,
            generation: self.generation,
            topology: Topology::Lines,
            geometry: Some(Geometry {
                vertices: self.vertices.clone(),
                indices: self.indices.clone(),
            }),
            state: StateSnapshot {
                world_transform: Matrix4::identity(),
                color: [1.0, 1.0, 1.0, 1.0],
                shade: [0.0; 4],
                depth_bias: self.depth_bias,
                texture: None,
                id: PickId(0),
                depth: Default::default(),
                pickable: false,
                ribbon: false,
                program: None,
            },
        });
    }
}

/// Sync one layer, or hide both halves when it has no spec.
fn sync_layer(layer: &mut GridLayer, camera: &Camera, spec: Option<LayerSpec<'_>>) {
    match spec {
        Some(spec) => {
            layer.show_ground = spec.ground;
            layer.show_triad = spec.triad;
            layer.sync(camera, spec.spec, spec.style, spec.axis);
        }
        None => {
            layer.show_ground = false;
            layer.show_triad = false;
        }
    }
}

/// One grid's cached geometry: dots, the minor lattice, the major lines and the triad at
/// its origin.
struct GridLayer {
    dots_mesh: MeshId,
    minor_mesh: MeshId,
    lines_mesh: MeshId,
    triad_mesh: MeshId,
    /// The occluded stem pass (depth `Greater`, no write, 10 % alpha).
    triad_ghost_mesh: MeshId,
    ground_generation: Generation,
    triad_generation: Generation,
    triad_ghost_generation: Generation,
    ground_key: Option<u64>,
    triad_key: Option<u64>,
    dots: Vec<Vertex>,
    dot_indices: Vec<u32>,
    minor_lines: Vec<Vertex>,
    minor_indices: Vec<u32>,
    lines: Vec<Vertex>,
    line_indices: Vec<u32>,
    triad: Vec<Vertex>,
    triad_ghost: Vec<Vertex>,
    triad_indices: Vec<u32>,
    /// The coplanar lift for a face grid; `0` for the world ground.
    depth_bias: f32,
    /// The dots and major lines (`App_ToggleGrid`).
    show_ground: bool,
    /// The triad stems (`App_ToggleAxisCross`).
    show_triad: bool,
}

impl GridLayer {
    fn new(depth_bias: f32) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        // A range above the stage's ids (which start at 1 << 40) is not needed: the
        // stage's allocator is a different counter, so a shared range is fine as long
        // as the values are distinct in practice.
        static NEXT: AtomicU64 = AtomicU64::new(1 << 36);
        let next = || MeshId(NEXT.fetch_add(1, Ordering::Relaxed));
        Self {
            dots_mesh: next(),
            minor_mesh: next(),
            lines_mesh: next(),
            triad_mesh: next(),
            triad_ghost_mesh: next(),
            ground_generation: Generation(0),
            triad_generation: Generation(0),
            triad_ghost_generation: Generation(0),
            ground_key: None,
            triad_key: None,
            dots: Vec::new(),
            dot_indices: Vec::new(),
            minor_lines: Vec::new(),
            minor_indices: Vec::new(),
            lines: Vec::new(),
            line_indices: Vec::new(),
            triad: Vec::new(),
            triad_ghost: Vec::new(),
            triad_indices: Vec::new(),
            depth_bias,
            show_ground: false,
            show_triad: false,
        }
    }

    fn sync(&mut self, camera: &Camera, spec: &GridSpec, style: &GridStyle, axis: &AxisStyle) {
        let key = grid::ground_key(spec, style);
        if self.ground_key != Some(key) {
            let ground = grid::ground(spec, style);
            self.dots = ground.dots;
            self.dot_indices = ground.dot_indices;
            self.minor_lines = ground.minor_lines;
            self.minor_indices = ground.minor_indices;
            self.lines = ground.lines;
            self.line_indices = ground.line_indices;
            self.ground_generation = Generation(self.ground_generation.0 + 1);
            self.ground_key = Some(key);
        }
        let key = axis::triad_key(
            &spec.workplane,
            camera,
            axis,
            matches!(spec.workplane.kind, ferrocad_stage::WorkplaneKind::Face { .. }),
        );
        if self.triad_key != Some(key) {
            // A plane on a face shows the in-plane `U`/`V` bracket: the normal is the
            // face manipulator's to draw (workplanes note, §3.2).
            let (vertices, indices) = match spec.workplane.kind {
                ferrocad_stage::WorkplaneKind::Face { .. } => {
                    axis::triad_in_plane(&spec.workplane, camera, axis)
                }
                _ => axis::triad(&spec.workplane, camera, axis),
            };
            self.triad_ghost = axis::ghost(&vertices, axis.ghost_alpha);
            self.triad = vertices;
            self.triad_indices = indices;
            self.triad_generation = Generation(self.triad_generation.0 + 1);
            self.triad_ghost_generation = Generation(self.triad_ghost_generation.0 + 1);
            self.triad_key = Some(key);
        }
    }

    fn emit(&self, queue: &mut RenderQueue) {
        if self.show_ground {
            self.emit_level(queue, self.dots_mesh, &self.dots, &self.dot_indices, Topology::Points);
            self.emit_level(queue, self.minor_mesh, &self.minor_lines, &self.minor_indices, Topology::Lines);
            self.emit_level(queue, self.lines_mesh, &self.lines, &self.line_indices, Topology::Lines);
        }
        if self.show_triad && !self.triad.is_empty() {
            queue.packets.push(self.packet(
                self.triad_mesh,
                self.triad_generation,
                Topology::Triangles,
                self.triad.clone(),
                self.triad_indices.clone(),
                DepthState::default(),
            ));
            if !self.triad_ghost.is_empty() {
                // The stems behind a solid read at the occluded-axis ink instead of
                // disappearing or floating over the front face.
                queue.packets.push(self.packet(
                    self.triad_ghost_mesh,
                    self.triad_ghost_generation,
                    Topology::Triangles,
                    self.triad_ghost.clone(),
                    self.triad_indices.clone(),
                    GHOST_DEPTH,
                ));
            }
        }
    }

    /// One grid level: its hairline form (`hairline`: a point per dot, two vertices per
    /// line segment), or the triangles of a wide stroke (the
    /// [geometry](grid::GridGeometry) says which by whether it carries indices).
    ///
    /// A wide stroke goes through the *lit* stage, not the ribbon one: the ribbon stage
    /// feathers its last pixel on each side, which on a 2–4 px grid line eats half the
    /// requested width. The quad is the width the user asked for, and the 4× MSAA target
    /// antialiases its edges instead.
    ///
    /// A wide stroke also asks for [`BLENDED_DEPTH`], which selects the renderer's
    /// alpha-blended scene-triangle variant. The opaque base pipeline is
    /// `BlendState::REPLACE`, so a triangle drawn through it ignores its per-vertex alpha
    /// — and every fade the grid has (the radial patch fade, the edge-on `grazing`
    /// factor and the user's `Grid.opacity`) lives in that alpha. Hairlines and dots
    /// already blend; without this they were the only levels that dimmed.
    fn emit_level(
        &self,
        queue: &mut RenderQueue,
        mesh: MeshId,
        vertices: &[Vertex],
        indices: &[u32],
        hairline: Topology,
    ) {
        if vertices.is_empty() {
            return;
        }
        let (topology, indices, depth) = if indices.is_empty() {
            (
                hairline,
                (0..vertices.len() as u32).collect(),
                DepthState::default(),
            )
        } else {
            (Topology::Triangles, indices.to_vec(), BLENDED_DEPTH)
        };
        queue.packets.push(self.packet(
            mesh,
            self.ground_generation,
            topology,
            vertices.to_vec(),
            indices,
            depth,
        ));
    }

    /// The grid is deliberately **not pickable**: it is emitted into its own queue that
    /// no pick path sees, and it carries the untagged id besides.
    #[allow(clippy::too_many_arguments)]
    fn packet(
        &self,
        mesh_id: MeshId,
        generation: Generation,
        topology: Topology,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        depth: DepthState,
    ) -> DrawPacket {
        DrawPacket {
            mesh_id,
            generation,
            topology,
            geometry: Some(Geometry { vertices, indices }),
            state: StateSnapshot {
                world_transform: Matrix4::identity(),
                color: [1.0, 1.0, 1.0, 1.0],
                shade: [0.0; 4],
                depth_bias: self.depth_bias,
                texture: None,
                id: PickId(0),
                depth,
                pickable: false,
                // The grid never uses the ribbon stage: its strokes are geometry, and the
                // MSAA target antialiases their edges without a per-pixel feather.
                ribbon: false,
                program: None,
            },
        }
    }
}

/// `style` with the host policy's stroke: its minor form, and its pixel widths converted
/// to widths in the plane's own units.
///
/// A 1 px request is the renderer's hairline (`0.0`, a `LineList`), so the default look and
/// its geometry are exactly what they were; anything wider becomes an in-plane stripe and
/// is drawn through the anti-aliased ribbon stage.
fn with_stroke(
    mut style: GridStyle,
    camera: &Camera,
    plane: &Workplane,
    policy: GridPolicy,
) -> GridStyle {
    style.minor_style = policy.minor_style;
    style.major_width = stroke_width(camera, plane, policy.major_width);
    style.minor_width = stroke_width(camera, plane, policy.minor_width);
    style
}

/// The token style a plane's two levels start from.
///
/// A plane bound to a finite surface keeps the workplane's `1:2` minor:major ratio; the
/// unbounded ground uses the world's equal pair. The hover preview, the native layer and
/// the Coin frame all start here, so binding a face cannot change its ink and the two
/// backends cannot disagree about it.
fn level_style(extent: Option<f64>) -> GridStyle {
    match extent {
        Some(extent) if extent.is_finite() && extent > 0.0 => GridStyle::workplane(),
        _ => GridStyle::default(),
    }
}

/// The base ink of a plane's two levels: the token alphas the native generator starts
/// from (a bound plane keeps its `1:2` minor:major ratio), times the user's opacity.
///
/// The Coin shader receives these instead of a strength of its own, so the two backends
/// draw the same lattice at the same weight — a shader drawing 4× heavier ink reads as a
/// thicker line even at the same width and colour.
fn base_ink(extent: Option<f64>, policy: GridPolicy) -> (f32, f32) {
    let style = level_style(extent);
    (
        style.minor_alpha * policy.alpha,
        style.major_alpha * policy.alpha,
    )
}

/// A stroke width in device pixels as a width in the plane's units.
fn stroke_width(camera: &Camera, plane: &Workplane, pixels: f64) -> f64 {
    if !(pixels.is_finite() && pixels > 1.0) {
        return 0.0;
    }
    scale_for(camera, plane.origin, pixels)
}

/// The spec and style of the workplane a pointer is hovering: the same steps and the
/// same patch as the plane once it is active, so binding it does not jump the grid.
fn local_grid(camera: &Camera, frame: &FaceFrame, policy: GridPolicy) -> (GridSpec, GridStyle) {
    let (minor, major) = plane_steps(Some(frame.radius), 0.0, policy);
    let (minor, major) = grid::effective_steps(camera, &frame.plane, minor, major);
    (
        GridSpec {
            workplane: frame.plane.clone(),
            minor,
            major,
            radius: patch_radius(Some(frame.radius), major).unwrap_or(frame.radius),
        },
        with_stroke(
            level_style(Some(frame.radius)),
            camera,
            &frame.plane,
            policy,
        ),
    )
}

/// The grid for the active plane: the same generator, the same fade and the same
/// steps **for this plane**. A bound plane derives its step from the face (see
/// [`plane_steps`]) when spacing is adaptive, so it reads as a construction patch around
/// the face rather than the world grid stretched across it.
///
/// `step_extent` is what the *step* is derived from when the plane is bound to a surface;
/// `patch_extent` is what the *radius* is derived from and is deliberately only ever a
/// bound surface's, so an adaptive world ground stays camera-following rather than
/// becoming a patch around the model.
fn active_grid(
    camera: &Camera,
    plane: &Workplane,
    policy: GridPolicy,
    color: [f32; 4],
    alpha: f32,
    patch_extent: Option<f64>,
    ground_radius: f64,
) -> (GridSpec, GridStyle) {
    let (minor, major) = plane_steps(patch_extent, ground_radius, policy);
    let (minor, major) = grid::effective_steps(camera, plane, minor, major);
    let mut style = level_style(patch_extent);
    style.color = color;
    style.minor_alpha *= alpha;
    style.major_alpha *= alpha;
    style = with_stroke(style, camera, plane, policy);
    (
        GridSpec {
            workplane: plane.clone(),
            minor,
            major,
            radius: active_radius(camera, patch_extent, major),
        },
        style,
    )
}

/// The patch radius of a plane bound to a finite surface: `max(3 × extent, 2 × major)`,
/// or `None` when no surface is behind the plane.
///
/// Shared by the native grid (which falls back to the camera-following ground) and the
/// program grid's `Frame.workplane_origin.w` (which is `0`, "no patch"). It is
/// deliberately **not** clamped to [`grid::RADIUS_RANGE`]: that range exists so the world
/// ground does not collapse when zoomed in, and applying it would force a 40 mm face to
/// carry a 500 mm grid.
fn patch_radius(extent: Option<f64>, major: f64) -> Option<f64> {
    extent
        .filter(|extent| extent.is_finite() && *extent > 0.0)
        .map(|extent| (3.0 * extent).max(2.0 * major))
}

/// The `(minor, major)` steps of a plane bound to a finite surface.
///
/// A bound plane is sized in world units by the face it sits on, so its **step** has to
/// follow the face too: about ten major cells across the `3 × extent` patch, on the
/// 1-2-5 ladder. Keeping the world's configured steps here (100 mm major) let the
/// `2 × major` floor dominate the extent — a 20 mm face carried a 200 mm patch, which is
/// the wall that fills the view when the plane is seen at a grazing angle.
///
/// This is also the step rule for the **scene** when spacing is adaptive: the world
/// ground and a face patch then differ only in what `extent` means, so "ten cells across"
/// is one law rather than two.
fn patch_steps(extent: f64) -> (f64, f64) {
    let major = nice_step(0.3 * extent);
    (major / 10.0, major)
}

/// The smallest 1-2-5 step at or above `value`.
///
/// The ladder keeps a patch to a whole number of cells without an arbitrary constant: a
/// face twice as large gets one step up, not twice the cells.
fn nice_step(value: f64) -> f64 {
    if !(value.is_finite() && value > 0.0) {
        return grid::LOCAL_MAJOR_STEP;
    }
    let decade = 10f64.powf(value.log10().floor());
    if !(decade.is_finite() && decade > 0.0) {
        return grid::LOCAL_MAJOR_STEP;
    }
    let scaled = value / decade;
    let mantissa = if scaled <= 1.0 {
        1.0
    } else if scaled <= 2.0 {
        2.0
    } else if scaled <= 5.0 {
        5.0
    } else {
        10.0
    };
    mantissa * decade
}

/// The steps a grid layer uses, from the extent the layer covers and the host's policy.
///
/// **Adaptive** (the default) sizes the pair to the layer's own extent: a bound surface's
/// in-plane radius ([`patch_steps`]) or the ground's disc radius ([`ground_steps`]).
/// **Non-adaptive** uses the configured steps on every plane, exactly: the drawn pitch is
/// the setting, whatever the zoom. A step too fine to cover the whole disc is not
/// coarsened — [`grid::ground`] clips the minor level's *extent* instead (see
/// [`grid::minor_radius`]), so the subdivisions never change under the camera.
///
/// This is the single source of truth for the drawn grid, the hover preview, the program
/// (Coin) patch and [`snap_context`](InventorViewport::snap_context), so a snap always
/// lands on a line that is on screen.
fn plane_steps(extent: Option<f64>, ground_radius: f64, policy: GridPolicy) -> (f64, f64) {
    if !policy.adaptive {
        return (policy.minor, policy.major);
    }
    let (minor, major, radius) = match extent.filter(|extent| extent.is_finite() && *extent > 0.0)
    {
        Some(extent) => {
            let (minor, major) = patch_steps(extent);
            (minor, major, (3.0 * extent).max(2.0 * major))
        }
        None => {
            let (minor, major) = ground_steps(ground_radius);
            (minor, major, ground_radius)
        }
    };
    // Adaptive dots are kept inside the generator's point budget: the dot level is
    // coarsened (a ladder step, never finer than the line level) rather than left so fine
    // that `grid::ground` would clip it away. Both backends receive the result, so the Coin
    // dots and the native dots stay at the same spacing.
    let budget = nice_step(radius / DOT_COLUMNS).min(major);
    (minor.max(budget).min(major), major)
}

/// How many dots a layer may put across its radius before the level is coarsened.
///
/// `grid::ground` clips a dot field to `DOT_COLUMNS` columns, and the guard counts a disc,
/// so a disc of radius `r` holds about `π · (r/minor)²` dots: 112 dot columns per radius is
/// the largest field that fits the point budget. Coarsening the *dot* level keeps the lines
/// where they are, which is what makes an adaptive grid still read as a grid.
const DOT_COLUMNS: f64 = grid::DOT_COLUMNS;

/// The adaptive steps for a ground disc of `radius`.
///
/// The world ground is drawn over a radius derived from the camera, not over the model's
/// bounds, so *that* radius is the extent it covers — and it is the only scale the step
/// can safely follow. Sizing it from the scene instead fails at both ends: a 10 mm part
/// asks for sub-millimetre cells across a 500 mm disc (hundreds of thousands of
/// vertices), and a 1 m part inspected at a 10 mm feature asks for 300 mm cells, i.e. an
/// empty grid. Twenty-five major cells across the disc put about ten across the visible
/// frame at the default field of view, so the ground keeps the configured grid's density
/// at the default zoom without depending on the model's size.
fn ground_steps(radius: f64) -> (f64, f64) {
    let major = nice_step(radius / 25.0);
    (major / 10.0, major)
}

/// The extent of a plane's grid: a patch when the plane is bound to a finite surface,
/// the camera-following ground otherwise.
///
/// A face-bound plane gets `max(3 × face radius, 2 × major step)` so the patch reads as a
/// construction area around the face and cannot become a wall that fills the view. The
/// generator's own smooth fade gives the patch its soft edge, so this is a radius, not a
/// style.
fn active_radius(camera: &Camera, extent: Option<f64>, major: f64) -> f64 {
    patch_radius(extent, major).unwrap_or_else(|| ground_radius(camera, None))
}

/// The ground's disc radius for `camera`: the user's fixed "Grid extent", else the
/// camera-following one, clamped to [`grid::RADIUS_RANGE`].
///
/// This is both the ground's drawn extent and the scale its adaptive step is sized from,
/// so the two can never disagree about how large the ground is.
fn ground_radius(camera: &Camera, fixed: Option<f64>) -> f64 {
    fixed.map_or_else(
        || GridSpec::radius_for(camera),
        |radius| radius.clamp(grid::RADIUS_RANGE.0, grid::RADIUS_RANGE.1),
    )
}

/// Apply the user's ink and alpha multiplier to an existing style. The multiplier scales
/// the token alphas, so a workplane keeps its `1:2` minor:major ratio.
fn with_grid_ink(mut style: GridStyle, color: [f32; 4], alpha: f32) -> GridStyle {
    style.color = color;
    style.minor_alpha *= alpha;
    style.major_alpha *= alpha;
    style
}

/// Dim a grid layer the way the Coin shader does: one factor for the whole plane, from
/// the camera's angle to it. Only the **grid** dims — the triad keeps its ink, because
/// it is what still tells you where the plane is when you are looking along it.
fn with_grazing(mut style: GridStyle, camera: &Camera, plane: &Workplane) -> GridStyle {
    let grazing = grid::grazing(camera, plane);
    style.minor_alpha *= grazing;
    style.major_alpha *= grazing;
    style
}

/// The face under the pointer, resolved to a workplane the pointer may activate.
struct FaceHover {
    entity: u32,
    part: u32,
    /// The stage item's geometry generation at resolution time, so a reshaped item is
    /// re-resolved without resetting the dwell.
    generation: Generation,
    frame: FaceFrame,
    /// When the pointer first rested on this face; the grid blooms after [`FACE_DWELL`].
    since: Instant,
}

/// How long the pointer rests on a face before its grid blooms. An immediate bloom
/// flickers while crossing an edge; the face highlight stays instant.
const FACE_DWELL: Duration = Duration::from_millis(120);

/// The snap a tool is currently working from, for the aurora, the rays and the pill.
struct SnapFeedback {
    snap: ferrocad_stage::Snap,
    /// The plane the snap's local coordinates refer to (the rays lie in it).
    plane: Option<Workplane>,
    /// When this snap point was acquired; the aurora flashes for [`feedback::FLASH`].
    since: Instant,
}

/// Stable mesh identities for the feedback overlay.
struct FeedbackMeshes {
    aurora: MeshId,
    rays: MeshId,
    generation: Generation,
}

impl FeedbackMeshes {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1 << 38);
        let next = || MeshId(NEXT.fetch_add(1, Ordering::Relaxed));
        Self {
            aurora: next(),
            rays: next(),
            generation: Generation(0),
        }
    }
}

/// A cap on the tessellated edge segments considered for an edge-midpoint snap; beyond
/// it the mesh is too dense for a midpoint to mean anything.
const MAX_EDGE_CANDIDATES: usize = 1024;

/// A triad axis tag: `X`/`Y`/`Z` at a workplane's `U`/`V`/`N` stem tip.
#[derive(Clone, Copy, Debug)]
struct AxisLabel {
    text: char,
    /// Element-local logical pixels.
    position: [f32; 2],
    /// sRGB ink.
    color: u32,
    /// The style's opacity, as an 8-bit sRGB alpha.
    alpha: u8,
}

/// Pack a **linear** RGB colour as an sRGB `0xRRGGBB` (the gpui text layer's ink).
fn packed_srgb(color: [f32; 4]) -> u32 {
    let channel = |value: f32| {
        let value = value.clamp(0.0, 1.0);
        let srgb = if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u32
    };
    channel(color[0]) << 16 | channel(color[1]) << 8 | channel(color[2])
}

/// A draw packet for the feedback overlay: untagged (never picked) and depth-free, so
/// the aurora and rays are drawn over the scene.
fn feedback_packet(
    mesh_id: MeshId,
    generation: Generation,
    topology: Topology,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
) -> DrawPacket {
    DrawPacket {
        mesh_id,
        generation,
        topology,
        geometry: Some(Geometry { vertices, indices }),
        state: StateSnapshot {
            world_transform: Matrix4::identity(),
            color: [1.0, 1.0, 1.0, 1.0],
            shade: [0.0; 4],
            depth_bias: 0.0,
            texture: None,
            id: PickId(0),
            depth: Default::default(),
            pickable: false,
            ribbon: false,
            program: None,
        },
    }
}

/// How quickly a released orbit slows down (the velocity's time constant).
const ORBIT_INERTIA_TAU: f32 = 0.18;
/// The minimum release speed (logical px/s) that starts a glide.
const ORBIT_INERTIA_START: f32 = 120.0;
/// The glide stops below this speed (logical px/s).
const ORBIT_INERTIA_STOP: f32 = 12.0;

/// The marker type gpui's drag system tracks for viewport orbit gestures.
struct ViewportDragMarker;

/// The (invisible) preview gpui renders while a viewport drag is active.
struct ViewportDragPreview;

impl Render for ViewportDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl<V: InventorView> InventorViewport<V> {
    /// Wrap `view`, drawing `scene` with `camera`.
    pub fn new(view: V, camera: Camera, scene: Scene) -> Self {
        let stage = Stage::new();
        let (queue, directives, shader_error) = merge_queue(&scene, &stage, &camera.viewport);
        let (queue, mut program_queue) = split_programs(queue);
        apply_program_shadows(&mut program_queue, &stage.shadow_footprints());
        let mut viewport = Self {
            view,
            camera,
            scene,
            stage,
            stage_dirty: false,
            tool: None,
            tool_state: None,
            transient: RenderQueue::default(),
            snap_grid: None,
            gizmos: GizmoSet::default(),
            gizmo_queue: RenderQueue::default(),
            gizmo_skin: None,
            gizmo_commit: None,
            manipulators: Vec::new(),
            manipulator_queue: RenderQueue::default(),
            interaction_commit: None,
            focus: None,
            gizmo_value: None,
            gizmo_pointer: None,
            gizmo_deferred: false,
            gizmo_origin_drag: None,
            gizmo_origin: None,
            transform_preview: None,
            preview_home: None,
            queue,
            directives,
            listeners: Vec::new(),
            navicube: None,
            directive_cubes: Vec::new(),
            navicube_drag: None,
            orbit_drag: None,
            ring_drag: None,
            orbit_velocity: [0.0, 0.0],
            last_drag_time: None,
            inertia: None,
            snap: None,
            hover: None,
            hovered_part: None,
            selection: None,
            press_click_count: 1,
            hover_report: HoverReport::default(),
            hover_anchor: None,
            grid: GridMesh::new(),
            grid_enabled: false,
            axis: AxisStyle::default(),
            gizmo_style: GizmoStyle::default(),
            grid_minor: grid::MINOR_STEP,
            grid_major: grid::MAJOR_STEP,
            grid_adaptive: GridPolicy::default().adaptive,
            grid_minor_style: GridPolicy::default().minor_style,
            grid_major_width: GridPolicy::default().major_width,
            grid_minor_width: GridPolicy::default().minor_width,
            grid_radius: None,
            grid_color: grid::GRID_COLOR,
            grid_alpha: 1.0,
            active_workplane: ActiveWorkplane::world(),
            ephemeral_restore: None,
            hover_face: None,
            grid_queue: RenderQueue::default(),
            grid_overlay: false,
            program_queue,
            shadow_footprints: None,
            shader_error,
            overlays: Vec::new(),
            pointer: None,
            snap_feedback: None,
            feedback_queue: RenderQueue::default(),
            feedback_meshes: FeedbackMeshes::new(),
            frame: None,
            presenter: None,
            presenter_frames: 0,
            last_bounds: None,
        };
        viewport.sync_directive_cubes();
        viewport
    }

    /// Add a listener that reacts to pointer events (a `SoEventCallback`). Every
    /// listener sees every event, in the order added.
    pub fn with_listener(mut self, listener: Box<dyn EventListener>) -> Self {
        self.listeners.push(listener);
        self
    }

    /// Present frames through a host-supplied zero-copy presenter instead of the
    /// readback image path. The presenter falls back to the image path when the
    /// window has no compatible GPU device.
    pub fn with_presenter(mut self, presenter: Box<dyn Presenter>) -> Self {
        self.presenter = Some(Rc::new(RefCell::new(presenter)));
        self
    }

    /// The presenter's lifecycle state, or `None` when the image path is in use.
    pub fn presenter_status(&self) -> Option<PresenterStatus> {
        self.presenter
            .as_ref()
            .map(|presenter| presenter.borrow().status())
    }

    /// The presenter's technical reason for using the image path, if it fell back.
    ///
    /// The shell shows this in its copyable diagnostics notice. `None` while the
    /// presenter is pending or accelerated.
    pub fn presenter_note(&self) -> Option<String> {
        self.presenter
            .as_ref()
            .and_then(|presenter| presenter.borrow().status_note().map(str::to_owned))
    }

    /// Show a navigation cube in the viewport corner.
    pub fn with_navicube(mut self, navicube: NaviCube) -> Self {
        self.navicube = Some(navicube);
        self
    }

    /// Activate an interactive tool: pointer events are dispatched to it and its
    /// transient visuals are drawn over the scene (see [`Tool`]).
    pub fn with_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.set_tool(Some(tool));
        self
    }

    /// Replace (or clear) the active tool. Starting one queues its
    /// [`ToolStatus`] for the host's strip.
    pub fn set_tool(&mut self, tool: Option<Box<dyn Tool>>) {
        if tool.is_none() {
            self.set_snap_feedback(None, None);
            self.restore_ephemeral_workplane();
        }
        self.tool_state = tool.as_ref().map(|tool| tool.status());
        self.tool = tool;
        self.rebuild_transient();
    }

    /// End an ephemeral workplane binding, if one is in force.
    ///
    /// The first click of a creation tool on a planar face binds that face for the
    /// session; ending the session puts the previous plane back (usually world `XY`).
    /// An explicit binding never sets this, so it survives a tool.
    fn restore_ephemeral_workplane(&mut self) -> bool {
        let Some(previous) = self.ephemeral_restore.take() else {
            return false;
        };
        self.set_active_workplane(previous)
    }

    /// The active tool, if any.
    pub fn tool(&self) -> Option<&dyn Tool> {
        self.tool.as_deref()
    }

    /// The active tool's live status, if a tool session is installed.
    pub fn tool_status(&self) -> Option<ToolStatus> {
        self.tool.as_ref().map(|tool| tool.status())
    }

    /// Take the tool status queued for the host (the next gpui handler emits it as
    /// [`ViewportEvent::ToolState`]).
    pub fn take_tool_state(&mut self) -> Option<ToolStatus> {
        self.tool_state.take()
    }

    /// Cancel the active tool (the `Esc` path and the host's Cancel button): clear its
    /// transient geometry, end the session, and queue an inactive status for the host.
    /// No [`InteractionCommit`] is produced. Returns whether a session was active.
    pub fn cancel_tool(&mut self) -> bool {
        let Some(tool) = self.tool.as_mut() else {
            return false;
        };
        let status = tool.status().ended();
        tool.cancel();
        self.tool = None;
        self.restore_ephemeral_workplane();
        self.set_snap_feedback(None, None);
        self.rebuild_transient();
        self.tool_state = Some(status);
        true
    }

    /// Finish the active tool (the `Enter` / **Finish** path): end the session, clear its
    /// transient visuals, and hand the geometry the tool wants committed to the host
    /// ([`take_interaction_commit`](Self::take_interaction_commit)).
    ///
    /// A tool that produces nothing returns no commit, so finishing an empty or
    /// single-point session still writes nothing. Returns whether a session was active.
    pub fn finish_tool(&mut self) -> bool {
        let Some(tool) = self.tool.as_mut() else {
            return false;
        };
        let status = tool.status().ended();
        // A picker may resolve a *frame* instead of geometry (the three-point
        // workplane). Read it off the tool before `finish` ends the session, and apply
        // it below: `finish` still runs and returns nothing for such a tool.
        let plane = tool.workplane();
        // The tool owns its geometry until it ends; this is where it hands it over.
        self.interaction_commit = tool.finish();
        self.tool = None;
        self.restore_ephemeral_workplane();
        if let Some(plane) = plane {
            // This does not fight `restore_ephemeral_workplane` above: that restore only
            // fires when `ephemeral_restore` is set, which only the first-press face
            // binding sets. The resolved frame is an explicit pick, so it is applied
            // after the session's own binding has been undone.
            self.set_workplane_frame(plane.origin, plane.u, plane.v);
        }
        self.set_snap_feedback(None, None);
        self.rebuild_transient();
        self.tool_state = Some(status);
        true
    }

    /// Set the grid snapping spacing (device-independent world units); `None` disables
    /// grid snapping (geometry snapping still applies over a hit). Every hosted gizmo
    /// uses the same step for its linear drags.
    pub fn set_snap_grid(&mut self, grid: Option<f64>) {
        self.snap_grid = grid;
        for gizmo in self.gizmos.gizmos_mut() {
            gizmo.set_linear_snap(grid);
        }
    }

    /// Show a transform gizmo (drawn on top, picked before the tool and the scene).
    ///
    /// This is the **one-gizmo host** form: the control answers every session verb, as
    /// it did before the registry hosted a set ([`GizmoSet::single`]). Use
    /// [`set_gizmos`](Self::set_gizmos) for a registry set.
    pub fn with_gizmo(mut self, gizmo: TransformGizmo) -> Self {
        self.set_gizmo(Some(gizmo));
        self
    }

    /// Replace (or clear) the **hosted set** of transform gizmos, in registry priority
    /// order.
    ///
    /// The set stays narrowed to the session kind the viewport already holds (the wheel
    /// verbs set it through [`set_gizmo_mode`](Self::set_gizmo_mode)), so re-arming a
    /// selection applies the verb that is current rather than the registry's default.
    /// Every entry is given the workspace style, the grid step and the origin policy.
    pub fn set_gizmos(&mut self, gizmos: GizmoSet) {
        let session = self.gizmos.session();
        self.host_gizmos(gizmos, session);
    }

    /// Replace (or clear) a **single** transform gizmo, the historical one-gizmo form.
    ///
    /// A lone control has no alternative kind to offer, so it is registered as
    /// serving every session and takes the current one
    /// ([`GizmoSet::single`]). A registry set goes through
    /// [`set_gizmos`](Self::set_gizmos) instead and keeps only the kinds its edition
    /// declared.
    pub fn set_gizmo(&mut self, gizmo: Option<TransformGizmo>) {
        let session = self.gizmos.session();
        match gizmo {
            Some(gizmo) => {
                let session = gizmo.kind();
                self.host_gizmos(GizmoSet::single(gizmo), session);
            }
            None => self.host_gizmos(GizmoSet::default(), session),
        }
    }

    /// Adopt `gizmos` as the hosted set, narrowed to `session`, with the host policy
    /// applied to every entry.
    fn host_gizmos(&mut self, mut gizmos: GizmoSet, session: GizmoMode) {
        for gizmo in gizmos.gizmos_mut() {
            gizmo.set_linear_snap(self.snap_grid);
            gizmo.set_style(&self.gizmo_style);
            gizmo.set_origin_enabled(!self.gizmo_deferred);
        }
        gizmos.set_session(session);
        self.gizmos = gizmos;
        self.gizmo_commit = None;
        self.gizmo_value = None;
        self.gizmo_pointer = None;
        self.gizmo_origin_drag = None;
        self.gizmo_origin = None;
        self.preview_home = None;
        self.rebuild_gizmo_queue();
    }

    /// The first hosted gizmo's origin in **window-space** logical pixels.
    ///
    /// A host uses this to float a HUD beside the handles without duplicating the
    /// camera projection. `None` when no gizmo is offered or its origin is behind the
    /// camera.
    pub fn gizmo_anchor(&self) -> Option<Point<Pixels>> {
        let index = self.gizmos.primary()?;
        let device = self.gizmos.gizmos()[index].origin_anchor(&self.camera)?;
        let logical = self
            .camera
            .viewport
            .to_logical([device[0].round() as i32, device[1].round() as i32]);
        let origin = self
            .last_bounds
            .as_ref()
            .map(|bounds| point(bounds.origin.x, bounds.origin.y))
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        Some(point(
            px(origin.x.0 + logical[0]),
            px(origin.y.0 + logical[1]),
        ))
    }

    /// The highest-priority hosted gizmo on offer for the session, if any.
    pub fn gizmo(&self) -> Option<&TransformGizmo> {
        self.gizmos.primary().map(|index| &self.gizmos.gizmos()[index])
    }

    /// The hosted set, for a host that draws or inspects several entries.
    pub fn gizmos(&self) -> &GizmoSet {
        &self.gizmos
    }

    /// Which handles the hosted set shows (translate, rotate or both). Returns whether
    /// the hosted handles changed.
    ///
    /// The mode **re-narrows the hosted set in place**: every entry whose registered
    /// kind can serve `mode` takes it, and the others stay hosted but stop being offered
    /// (so switching back needs no re-selection). The mode changes the handle set, so the
    /// compiled overlay is rebuilt; a host that changes it through
    /// [`gizmo_mut`](Self::gizmo_mut) would leave the old handles drawn and pickable.
    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) -> bool {
        let changed = self.gizmos.set_session(mode);
        if !changed {
            return false;
        }
        // The mode changes the handle set: rebuild the geometry from the current camera
        // before compiling it, or the overlay would keep the old handles until the next
        // paint.
        for gizmo in self.gizmos.gizmos_mut() {
            gizmo.update(&self.camera);
        }
        self.rebuild_gizmo_queue();
        true
    }

    /// The session kind the hosted set is narrowed to.
    pub fn gizmo_session(&self) -> GizmoMode {
        self.gizmos.session()
    }

    /// The workspace gizmo style every hosted gizmo and manipulator is given.
    pub fn gizmo_style(&self) -> &GizmoStyle {
        &self.gizmo_style
    }

    /// The injected gizmo skin program, if any.
    pub fn gizmo_skin(&self) -> Option<&Arc<ShaderProgram>> {
        self.gizmo_skin.as_ref()
    }

    /// Inject (or clear) the authored program that skins the gizmo's handles.
    ///
    /// The source is compiled to a *geometry* program
    /// ([`ShaderProgram::with_geometry`]), so the renderer draws the gizmo's own handle
    /// meshes with the authored shader rather than a full-screen placeholder. The source
    /// is provider-supplied through the shell (the shader ships with a mod), so this
    /// crate stays GPU- and source-agnostic: the viewport only knows that a skin program
    /// exists, and draws it whenever the style is enabled.
    ///
    /// The skin changes **materials only**: `gizmo_handle_at`, `drag_begin`/`drag_to`/
    /// `drag_end`, the readout and the commit routing all still use the native
    /// [`TransformGizmo`]. Returns whether the stored program changed; a change rebuilds
    /// the gizmo queue.
    pub fn set_gizmo_skin(&mut self, source: Option<Arc<str>>) -> bool {
        let program = source
            .map(|source| Arc::new(ShaderProgram::from_source(source).with_geometry()));
        let changed = match (&self.gizmo_skin, &program) {
            (None, None) => false,
            (Some(stored), Some(incoming)) => stored != incoming,
            _ => true,
        };
        if changed {
            self.gizmo_skin = program;
            self.rebuild_manipulator_queue();
            self.rebuild_gizmo_queue();
        }
        changed
    }

    /// Defer a gizmo drag's commit to the host.
    ///
    /// While set, a drag reports [`ViewportEvent::TransformPreview`] for each tick and
    /// on release instead of setting [`ViewportEvent::Transform`]. A host uses this
    /// while an editable HUD is open: it mirrors the delta into its values and previews
    /// the **whole** selection itself, then commits once when the user accepts.
    pub fn set_gizmo_preview(&mut self, deferred: bool) {
        self.gizmo_deferred = deferred;
        if !deferred {
            self.transform_preview = None;
        }
        // The Move HUD is anchored to the visible origin, so the pivot handle is
        // disabled while its translation form is open; closing it re-enables the handle.
        self.set_gizmo_origin_enabled(!deferred);
    }

    /// Enable or disable the hosted gizmos' origin/centre handle.
    ///
    /// A host disables it while the floating Move HUD is open: the HUD is anchored to
    /// the visible origin and must not slide under the user's typing. Returns whether
    /// the handle state changed on any hosted entry.
    pub fn set_gizmo_origin_enabled(&mut self, enabled: bool) -> bool {
        if !enabled {
            self.cancel_gizmo_origin_drag();
        }
        let mut changed = false;
        for gizmo in self.gizmos.gizmos_mut() {
            changed |= gizmo.set_origin_enabled(enabled);
        }
        if !changed {
            return false;
        }
        for gizmo in self.gizmos.gizmos_mut() {
            gizmo.update(&self.camera);
        }
        self.rebuild_gizmo_queue();
        true
    }

    /// Take the released re-pivot, for the host to emit as
    /// [`ViewportEvent::GizmoOrigin`].
    pub fn take_gizmo_origin(&mut self) -> Option<(Vector3, SnapKind)> {
        self.gizmo_origin.take()
    }

    /// Whether a gizmo drag is currently deferred to the host.
    pub fn gizmo_preview(&self) -> bool {
        self.gizmo_deferred
    }

    /// Take the queued drag tick (a gpui handler emits it as a
    /// [`ViewportEvent::TransformPreview`]).
    pub fn take_transform_preview(&mut self) -> Option<TransformPreview> {
        self.transform_preview.take()
    }

    /// Emit a queued drag tick. The gpui pointer handlers call this after every update.
    fn flush_transform_preview(&mut self, cx: &mut Context<Self>)
    where
        V: 'static,
    {
        if let Some(preview) = self.take_transform_preview() {
            cx.emit(ViewportEvent::TransformPreview(preview));
        }
    }

    /// Queue a drag tick for the host, when deferral is on.
    fn queue_transform_preview(&mut self, phase: PreviewPhase, total_transform: Matrix4) {
        if !self.gizmo_deferred {
            return;
        }
        let Some(target_id) = self
            .gizmos
            .dragging()
            .or_else(|| self.gizmos.primary())
            .map(|index| self.gizmos.gizmos()[index].target_id())
        else {
            return;
        };
        self.transform_preview = Some(TransformPreview {
            target_id,
            phase,
            total_transform,
        });
    }

    /// Replace the workspace gizmo style and forward it to every hosted gizmo and
    /// manipulator. A style change (including toggling `enabled`) rebuilds both
    /// overlays, so a disabled style draws and picks nothing while keeping the objects
    /// hosted. Returns whether the stored style or any hosted object changed.
    pub fn set_gizmo_style(&mut self, style: GizmoStyle) -> bool {
        let stored_changed = self.gizmo_style != style;
        if stored_changed {
            self.gizmo_style = style;
        }
        let mut changed = stored_changed;
        for manipulator in &mut self.manipulators {
            changed |= manipulator.set_style(&self.gizmo_style);
        }
        for gizmo in self.gizmos.gizmos_mut() {
            changed |= gizmo.set_style(&self.gizmo_style);
        }
        if changed {
            self.rebuild_manipulator_queue();
            self.rebuild_gizmo_queue();
        }
        changed
    }

    /// The latest gizmo drag readout, if a drag is in progress.
    pub fn gizmo_value(&self) -> Option<GizmoValue> {
        self.gizmo_value
    }

    /// Cancel the active gizmo drag and clear its preview and readout, restoring the
    /// target's pre-drag transform. Returns whether a drag was active.
    ///
    /// A re-pivot is a gizmo drag too: cancelling it restores the frame origin from
    /// before the gesture (the host clears its session user origin separately).
    pub fn cancel_gizmo_drag(&mut self) -> bool {
        let origin = self.cancel_gizmo_origin_drag();
        let mut cancelled = false;
        for gizmo in self.gizmos.gizmos_mut() {
            cancelled |= gizmo.cancel_drag();
        }
        if cancelled || origin {
            self.apply_gizmo_preview(None);
            self.gizmo_commit = None;
            self.gizmo_value = None;
            self.gizmo_pointer = None;
            self.transform_preview = None;
            self.rebuild_gizmo_queue();
        }
        cancelled || origin
    }

    /// Begin a re-pivot from the origin/centre handle of hosted entry `index` at
    /// `device`.
    fn begin_gizmo_origin_drag(&mut self, index: usize, device: [i32; 2]) -> bool {
        let Some(home) = self.gizmos.gizmos().get(index).map(TransformGizmo::origin) else {
            return false;
        };
        let hit = self.pick(device);
        let Some((point, kind)) = self.gizmo_origin_snap(device, hit.as_ref()) else {
            return false;
        };
        self.gizmo_origin = None;
        self.gizmo_origin_drag = Some(GizmoOriginDrag {
            index,
            home,
            point,
            kind,
        });
        self.apply_gizmo_origin(point);
        true
    }

    /// Advance a live re-pivot to `device`, moving the **real** frame (a pivot drag has
    /// no solver, so it needs no draw-only offset). Returns whether anything changed.
    fn update_gizmo_origin_drag(&mut self, device: [i32; 2]) -> bool {
        let hit = self.pick(device);
        let Some((point, kind)) = self.gizmo_origin_snap(device, hit.as_ref()) else {
            return false;
        };
        let Some(mut drag) = self.gizmo_origin_drag else {
            return false;
        };
        let changed = drag.point != point || drag.kind != kind;
        drag.point = point;
        drag.kind = kind;
        self.gizmo_origin_drag = Some(drag);
        if !changed {
            return false;
        }
        self.apply_gizmo_origin(point);
        true
    }

    /// Release the re-pivot: queue [`ViewportEvent::GizmoOrigin`] with the snapped point
    /// and kind. Nothing is committed to the document.
    fn finish_gizmo_origin_drag(&mut self) -> bool {
        let Some(drag) = self.gizmo_origin_drag.take() else {
            return false;
        };
        self.gizmo_origin = Some((drag.point, drag.kind));
        self.rebuild_gizmo_queue();
        true
    }

    /// Abort a live re-pivot, restoring the frame origin from the press. Returns whether
    /// a drag was active.
    pub fn cancel_gizmo_origin_drag(&mut self) -> bool {
        let Some(drag) = self.gizmo_origin_drag.take() else {
            return false;
        };
        self.gizmo_origin = None;
        if let Some(gizmo) = self.gizmos.gizmo_mut(drag.index) {
            gizmo.set_origin(drag.home);
            gizmo.update(&self.camera);
        }
        self.rebuild_gizmo_queue();
        true
    }

    /// Move the grabbed frame (the re-pivot's entry, or the highest-priority entry) to
    /// the snapped pivot and redraw the handles there.
    fn apply_gizmo_origin(&mut self, point: Vector3) {
        let index = self
            .gizmo_origin_drag
            .map(|drag| drag.index)
            .or_else(|| self.gizmos.primary());
        if let Some(gizmo) = index.and_then(|index| self.gizmos.gizmo_mut(index)) {
            gizmo.set_origin(point);
            gizmo.update(&self.camera);
        }
        self.rebuild_gizmo_queue();
    }

    /// Resolve the pivot under `device` through the viewport's own snap pipeline: a
    /// topological vertex, edge midpoint, face centre or workplane origin within
    /// [`ferrocad_stage::snap::FEATURE_PIXELS`] wins, then the ray's point on the hovered
    /// body, then the active workplane/grid. `None` when no snap applies.
    fn gizmo_origin_snap(
        &self,
        device: [i32; 2],
        hit: Option<&Hit>,
    ) -> Option<(Vector3, SnapKind)> {
        let event = PointerEvent {
            kind: PointerKind::Move,
            position: device,
            button: None,
            hit: hit.copied(),
            projection: Some(ViewProjection::from_camera(&self.camera)),
        };
        let candidates = self.snap_candidates(hit);
        let context = self.snap_context();
        let snap = snap_to_geometry_with(&event, &self.camera, context.as_ref(), &candidates)?;
        Some((
            Vector3::new(
                f64::from(snap.point[0]),
                f64::from(snap.point[1]),
                f64::from(snap.point[2]),
            ),
            snap.kind,
        ))
    }

    /// Handle a key press. `Esc` cancels the active tool first, then a manipulator
    /// drag, a gizmo drag, and finally an active workplane. `Enter` finishes an active
    /// tool (without committing anything).
    pub fn handle_key(&mut self, event: &KeyDownEvent) -> bool {
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.cancel_tool() {
                    return true;
                }
                if self.cancel_manipulator_drag() {
                    return true;
                }
                if self.cancel_gizmo_drag() {
                    return true;
                }
                self.clear_workplane()
            }
            "enter" => self.finish_tool(),
            key => self.tool_key(key, event.keystroke.modifiers.control),
        }
    }

    /// Forward an editing key the viewport does not own (`Backspace`/`Delete`,
    /// `Ctrl+Z`, …) to the active tool. A tool that ignores it leaves the key to the
    /// host, so a document-level binding still works when no tool session cares.
    fn tool_key(&mut self, key: &str, ctrl: bool) -> bool {
        let Some(tool) = self.tool.as_mut() else {
            return false;
        };
        if !tool.on_key(key, ctrl) {
            return false;
        }
        self.rebuild_transient();
        if let Some(tool) = self.tool.as_ref() {
            self.tool_state = Some(tool.status());
        }
        true
    }

    /// The highest-priority hosted gizmo, mutably (e.g. to move its origin).
    pub fn gizmo_mut(&mut self) -> Option<&mut TransformGizmo> {
        let index = self.gizmos.primary()?;
        self.gizmos.gizmo_mut(index)
    }

    /// The last gizmo commit (a drag's final transform), if a drag has ended.
    pub fn last_gizmo_commit(&self) -> Option<&GizmoCommitEvent> {
        self.gizmo_commit.as_ref()
    }

    /// Take the last gizmo commit, for the host to route to the document.
    pub fn take_gizmo_commit(&mut self) -> Option<GizmoCommitEvent> {
        self.gizmo_commit.take()
    }

    /// Add a workbench manipulator (the edge fillet arc, a thickness grip, …).
    ///
    /// Manipulators are picked in reverse registration order, so the most recently
    /// added one wins. The viewport knows nothing about what a manipulator commits: it
    /// reports [`InteractionCommit`]s and lets the host route them to the workbench
    /// that registered the manipulator.
    pub fn with_manipulator(mut self, manipulator: Box<dyn Manipulator>) -> Self {
        self.add_manipulator(manipulator);
        self
    }

    /// Add a manipulator, replacing any with the same [`Manipulator::id`].
    pub fn add_manipulator(&mut self, mut manipulator: Box<dyn Manipulator>) {
        manipulator.set_style(&self.gizmo_style);
        self.remove_manipulator(manipulator.id());
        self.manipulators.push(manipulator);
        self.interaction_commit = None;
        self.preview_home = None;
        self.rebuild_manipulator_queue();
    }

    /// Replace every manipulator: the shape of a selection (one target, one manipulator).
    pub fn set_manipulators(&mut self, mut manipulators: Vec<Box<dyn Manipulator>>) {
        for manipulator in &mut manipulators {
            manipulator.set_style(&self.gizmo_style);
        }
        self.manipulators = manipulators;
        self.interaction_commit = None;
        self.preview_home = None;
        self.rebuild_manipulator_queue();
    }

    /// Remove the manipulator registered as `id`, if any.
    pub fn remove_manipulator(&mut self, id: &str) {
        let before = self.manipulators.len();
        self.manipulators
            .retain(|manipulator| manipulator.id() != id);
        if self.manipulators.len() != before {
            self.interaction_commit = None;
            self.preview_home = None;
            self.rebuild_manipulator_queue();
        }
    }

    /// The manipulator registered as `id`.
    pub fn manipulator(&self, id: &str) -> Option<&dyn Manipulator> {
        self.manipulators
            .iter()
            .find(|manipulator| manipulator.id() == id)
            .map(|manipulator| &**manipulator)
    }

    /// The manipulator registered as `id`, mutably (e.g. to bound its range from the
    /// document). The overlay is rebuilt before the next draw, so a value written here
    /// is shown.
    pub fn manipulator_mut(&mut self, id: &str) -> Option<&mut dyn Manipulator> {
        for manipulator in &mut self.manipulators {
            if manipulator.id() == id {
                return Some(&mut **manipulator);
            }
        }
        None
    }

    /// The manipulators, in registration order.
    pub fn manipulators(&self) -> &[Box<dyn Manipulator>] {
        &self.manipulators
    }

    /// Drop every manipulator.
    pub fn clear_manipulators(&mut self) {
        if self.manipulators.is_empty() {
            return;
        }
        self.manipulators.clear();
        self.interaction_commit = None;
        self.rebuild_manipulator_queue();
    }

    /// The last manipulator commit, if a drag has ended.
    pub fn last_interaction_commit(&self) -> Option<&InteractionCommit> {
        self.interaction_commit.as_ref()
    }

    /// Take the last manipulator commit, for the host to apply.
    pub fn take_interaction_commit(&mut self) -> Option<InteractionCommit> {
        self.interaction_commit.take()
    }

    /// Cancel an active manipulator drag, restoring its press value. Returns whether a
    /// drag was active.
    pub fn cancel_manipulator_drag(&mut self) -> bool {
        let cancelled = self
            .manipulators
            .iter_mut()
            .find(|manipulator| manipulator.is_dragging())
            .is_some_and(|manipulator| manipulator.drag_cancel());
        if cancelled {
            self.rebuild_manipulator_queue();
        }
        cancelled
    }

    /// The wrapped view (e.g. to borrow its shared surface once that path exists).
    pub fn view(&self) -> &V {
        &self.view
    }

    /// The scene currently being drawn.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The declarative stage drawn beside the inventor scene.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The stage, for adding or removing items. The queue is rebuilt before the next
    /// draw (call this from an event handler that then notifies, e.g. `cx.notify()`).
    pub fn stage_mut(&mut self) -> &mut Stage {
        self.stage_dirty = true;
        &mut self.stage
    }

    /// Replace the stage wholesale; the queue is rebuilt before the next draw.
    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_dirty = true;
    }

    /// Replace the pull-registered overlay scenes (G5).
    ///
    /// Each overlay's root is merged under a synthetic separator beside the base
    /// scene, then compiled through the ordinary [`merge_queue`]/[`split_programs`]
    /// path: an overlay's program packets land in [`program_queue`](Self::program_queue)
    /// exactly like a scene program. The base scene, the stage and the native grid
    /// are untouched.
    pub fn set_overlays(&mut self, overlays: Vec<Scene>) {
        self.overlays = overlays;
        self.recompile();
    }

    /// The overlay scenes currently merged into the draw.
    pub fn overlays(&self) -> &[Scene] {
        &self.overlays
    }

    /// Take the last shader-program error, if any. The frame still rendered (the
    /// malformed program was dropped); the shell reports it once and clears it.
    pub fn take_shader_error(&mut self) -> Option<String> {
        self.shader_error.take()
    }

    /// The ground-shadow footprints the program packets will carry, in the active
    /// plane's `(u, v)` frame.
    ///
    /// By default they are derived from the [`stage`](Self::stage)'s items; a host on
    /// the legacy inventor path can replace them with
    /// [`set_shadow_footprints`](Self::set_shadow_footprints). Empty means the shadow
    /// block is cleared and `shadow_count` is `0`.
    pub fn shadow_footprints(&self) -> Vec<ShadowFootprint> {
        self.effective_shadow_footprints()
    }

    /// Replace the derived stage footprints with explicit ones, **in the active plane's
    /// `(u, v)` frame** (world `(x, y)` on the default plane).
    ///
    /// Use this when the document is not drawn as a [`Stage`] (the legacy inventor
    /// path), or when the host has better bounds than the display tessellation. The
    /// producer clamps the list to the ABI's 64 slots and writes `shadow_count`; an
    /// empty list disables the shadow loop. Returns whether the set changed.
    pub fn set_shadow_footprints(&mut self, footprints: Vec<ShadowFootprint>) -> bool {
        // Keep the stored list bounded: the ABI has 64 slots, so an oversized host list
        // is truncated here as well as at pack time.
        let mut footprints = footprints;
        footprints.truncate(SHADOW_MAX);
        if self.shadow_footprints.as_deref() == Some(footprints.as_slice()) {
            return false;
        }
        self.shadow_footprints = Some(footprints);
        self.recompile();
        true
    }

    /// Return to deriving the footprints from the [`stage`](Self::stage). Returns
    /// whether the override was present.
    pub fn clear_shadow_footprints(&mut self) -> bool {
        if self.shadow_footprints.is_none() {
            return false;
        }
        self.shadow_footprints = None;
        self.recompile();
        true
    }

    /// The explicit override when set, else the stage's per-item footprints **in the
    /// active plane's `(u, v)` frame**. The list is truncated to the ABI's
    /// [`SHADOW_MAX`] slots.
    ///
    /// The frame matters: the shader projects a footprint with `(pu, pv)`, so the same
    /// world `(x, y)` box would land in the wrong place on a vertical workplane — a
    /// slab's shadow appeared beside it on `XZ`. The producer projects into the plane,
    /// so every plane gets its own footprint.
    fn effective_shadow_footprints(&self) -> Vec<ShadowFootprint> {
        let mut footprints = match &self.shadow_footprints {
            Some(footprints) => footprints.clone(),
            None => self
                .stage
                .shadow_footprints_on(&self.active_workplane.plane),
        };
        footprints.truncate(SHADOW_MAX);
        footprints
    }

    /// Frame the stage: move the eye along the current view direction so the document's
    /// bounding sphere around the pivot fits the view. Returns whether the stage has any
    /// geometry.
    ///
    /// A host calls this once, after it has built the stage, so a document whose scene is
    /// much larger or smaller than the default camera is not opened from inside it. The
    /// pivot stays the world origin (the camera's orbit centre) and the clip window
    /// follows the new distance.
    pub fn frame_stage(&mut self) -> bool {
        let radius = self.stage.items().iter().fold(0.0f64, |radius, item| {
            let matrix = item.placement().to_matrix();
            item.positions().iter().fold(radius, |radius, position| {
                let world = matrix.transform(&Vector3::new(
                    f64::from(position[0]),
                    f64::from(position[1]),
                    f64::from(position[2]),
                ));
                radius.max(world.length())
            })
        });
        if radius <= 1e-9 {
            return false;
        }

        match &mut self.camera.projection {
            Projection::Perspective { fov_y, .. } => {
                let distance = radius / ((*fov_y) / 2.0).sin().max(1e-6) * 1.15;
                self.set_camera_distance(distance);
            }
            Projection::Orthographic { extent, .. } => {
                *extent = radius * 1.15;
            }
            Projection::Matrix(_) => return false,
        }
        self.camera.fit_clip();
        self.recompile();
        true
    }

    /// Put the eye `distance` from the pivot along the current view direction, keeping
    /// the orientation. The view matrix's translation is `-R·eye`, so scaling it scales
    /// the distance.
    fn set_camera_distance(&mut self, distance: f64) {
        let base = Vector3::new(
            self.camera.view.m[3],
            self.camera.view.m[7],
            self.camera.view.m[11],
        );
        let current = base.length();
        if current <= 1e-9 || distance <= 1e-9 {
            return;
        }
        self.camera.view = Placement::new(
            base.scale(distance / current),
            Rotation::from_matrix(&self.camera.view),
        )
        .to_matrix();
    }

    /// Replace the camera and recompile (the device-pixel ratio lives in the camera).
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.recompile();
    }

    /// The object currently under the pointer, if any.
    pub fn hovered(&self) -> Option<PickId> {
        self.hover
    }

    /// The sub-element id ([`Hit::part`]) under the pointer, if the pointer is over
    /// geometry. `0` when the producer does not tag sub-elements.
    pub fn hovered_part(&self) -> Option<u32> {
        self.hovered_part
    }

    /// The object under the pointer resolved to a [`SelectionRef`], if it is
    /// identifiable.
    ///
    /// This is the hover mirror of [`take_selection`](Self::take_selection): the same
    /// entity/part resolution, without consuming anything. A host that previews a
    /// selection on hover can read it in `render`.
    pub fn hovered_selection(&self) -> Option<SelectionRef> {
        let id = self.hover?;
        self.selection_ref(id.0, self.hovered_part.unwrap_or(0))
    }

    /// Take the selection update produced by the last click, if any.
    ///
    /// A click is a left press and release that did **not** orbit the camera (or the
    /// navigation cube), did **not** move a gizmo handle, and was not consumed by an
    /// active tool or the chrome. The host maps [`SelectionRef::object`] to a document
    /// object; the viewport only ever reports pick ids and element names.
    pub fn take_selection(&mut self) -> Option<SelectionEvent> {
        self.selection.take()
    }

    /// The last selection update, without consuming it.
    pub fn selection(&self) -> Option<&SelectionEvent> {
        self.selection.as_ref()
    }

    /// What a click under the pointer would do (the neutral hover report).
    ///
    /// `target` is the scene object a click would select and `through` names the
    /// overlay whose interior is under the pointer but will not consume the click.
    /// Empty when the pointer is away, over a consuming overlay, or in a drag.
    pub fn hover_report(&self) -> &HoverReport {
        &self.hover_report
    }

    /// The world point of the pick under the pointer, when one is hovered.
    ///
    /// The companion to [`hover_report`](Self::hover_report): a host that turns a
    /// keyboard gesture into a spatial one anchors it here, matching what a click
    /// would have used.
    pub fn hover_anchor(&self) -> Option<SelectionAnchor> {
        self.hover_anchor
    }

    /// Resolve a hit's `(entity, part)` into a [`SelectionRef`].
    ///
    /// The part id indexes the stage item's name table, so a click on `Face6` reports
    /// `Face6`; a mesh with no names reports the whole object. `PickId(0)` is the
    /// compiler's untagged default (and what a non-selectable
    /// [`ShapeItem`](ferrocad_stage::ShapeItem) emits), so it never resolves to an
    /// object.
    fn selection_ref(&self, entity: u32, part: u32) -> Option<SelectionRef> {
        if entity == PickId::default().0 {
            return None;
        }
        Some(match self.part_name(entity, part) {
            Some(name) => SelectionRef::element(entity, name),
            None => SelectionRef::whole(entity),
        })
    }

    /// The element name a stage item carries for `(entity, part)`.
    ///
    /// Read from the live stage rather than a cache, so a host that assembles a stage
    /// with [`stage_mut`](Self::stage_mut) and clicks before the next paint still gets
    /// the current names.
    fn part_name(&self, entity: u32, part: u32) -> Option<&str> {
        self.stage
            .items()
            .iter()
            .find(|item| item.entity_id() == entity)
            .and_then(|item| item.part_name(part))
    }

    /// Convert a **window-relative** logical point (what gpui reports to an element's
    /// mouse listeners) into a **viewport-local** logical point, by subtracting the
    /// element's window-space origin captured at prepaint.
    ///
    /// Everything below the host — [`handle_pointer`](Self::handle_pointer), picking and
    /// the camera's [`Viewport`] — works in viewport-local coordinates, so this runs at
    /// the gpui boundary. Before the first prepaint (no bounds yet) the point is passed
    /// through unchanged.
    pub(crate) fn to_local(&self, position: Point<Pixels>) -> [f32; 2] {
        match self.last_bounds {
            Some(bounds) => [
                (position.x - bounds.origin.x).0,
                (position.y - bounds.origin.y).0,
            ],
            None => [position.x.0, position.y.0],
        }
    }

    /// Keep the camera's [`Viewport`] (and the backend's render target) at the element's
    /// **physical** size, from the window-space bounds captured at the last prepaint.
    ///
    /// gpui reports an element's bounds in **logical** window pixels; multiplied by the
    /// window scale factor that is the device size the renderer, picking and the overlays
    /// work in. Returns whether the size changed. A no-op until the first prepaint.
    fn sync_size(&mut self, window: &Window) -> bool {
        match self.last_bounds {
            Some(bounds) => self.apply_element_size(
                [bounds.size.width.0, bounds.size.height.0],
                window.scale_factor(),
            ),
            None => false,
        }
    }

    /// Apply a logical element size (and display scale) to the camera viewport, the
    /// render target and the navigation-cube overlay, recompiling if the device-pixel
    /// ratio changed (it seeds screen-space nodes). Returns whether anything changed.
    fn apply_element_size(&mut self, logical: [f32; 2], scale: f32) -> bool {
        let scale = scale.max(1.0);
        let device = [
            (logical[0] * scale).round().max(1.0) as u32,
            (logical[1] * scale).round().max(1.0) as u32,
        ];
        let viewport = Viewport::new(device, scale);
        if viewport == self.camera.viewport {
            return false;
        }
        self.camera.viewport = viewport;
        self.view.resize(device);
        if let Some(presenter) = &self.presenter {
            presenter.borrow_mut().set_size(device);
        }
        if let Some(cube) = self.navicube.as_mut() {
            cube.set_viewport(viewport);
        }
        for (_, cube) in &mut self.directive_cubes {
            cube.set_viewport(viewport);
        }
        self.recompile();
        true
    }

    /// Deliver a pointer move, in **viewport-local logical** pixels (top-left origin).
    ///
    /// Over the navigation cube this hovers its faces; over the gizmo it drags or
    /// hovers a handle; over the scene it picks the main scene. Returns whether the
    /// scene changed (and so needs repainting).
    pub fn handle_pointer(&mut self, logical: [f32; 2]) -> bool {
        self.handle_pointer_with_modifiers(logical, GizmoModifiers::default())
    }

    /// [`handle_pointer`](Self::handle_pointer) from a **window-space** position.
    ///
    /// The render listeners convert every move through
    /// [`to_local`](Self::to_local); a host uses this before a contextual action (for
    /// example a right-press) so the pick reflects that exact position even when no
    /// move preceded it. It runs the full hover pipeline, so the pick is the same one
    /// a click uses — including the presenter's G-buffer when the backend supports it.
    pub fn handle_pointer_at(&mut self, position: Point<Pixels>) -> bool {
        self.handle_pointer_with_modifiers(self.to_local(position), GizmoModifiers::default())
    }

    /// [`handle_pointer`](Self::handle_pointer) with the current keyboard modifiers. The
    /// modifiers drive an active gizmo drag (`Ctrl` frees the ring detents).
    pub fn handle_pointer_with_modifiers(
        &mut self,
        logical: [f32; 2],
        modifiers: GizmoModifiers,
    ) -> bool {
        let device = self.camera.viewport.to_device(logical);
        // Kept even during a drag or over the chrome: the origin node's proximity bloom
        // reads it, and stale coordinates would bloom the wrong node.
        self.pointer = Some(device);
        self.hover_anchor = None;
        // An active drag is owned by `drag_move`: hovering here would rebuild the cube per
        // event and can clear the ring's active state mid-scrub.
        if self.ring_drag.is_some() || self.navicube_drag.is_some() || self.orbit_drag.is_some() {
            self.hover_report = HoverReport::default();
            return false;
        }
        // The chrome bar is gpui, not the 3D scene: keep the pointer out of picking and
        // out of the cube's proximity state.
        if self.chrome_contains(logical) {
            self.hover_report = HoverReport::default();
            let mut changed = false;
            if let Some(cube) = self.navicube.as_mut() {
                changed |= cube.set_hilite(None);
                changed |= cube.set_proximity(false);
            }
            return changed;
        }
        let mut changed = false;
        // A live re-pivot owns the move: the real frame follows the snapped pointer.
        if self.gizmo_origin_drag.is_some() {
            let changed = self.update_gizmo_origin_drag(device);
            self.hover_report = HoverReport::default();
            return changed;
        }
        let mut in_cube = false;
        let mut cube_hit = None;
        let mut ring = false;
        if let Some(cube) = self.navicube.as_mut() {
            in_cube = cube.rect().contains(device);
            changed |= cube.set_proximity(in_cube);
            if in_cube {
                // Pieces win over the ring: their pick targets reach into the ring band
                // near a corner, and a corner click must not start a scrub.
                let hit = cube.hit(device);
                if let Some(piece) = hit {
                    self.hover = None;
                    self.hovered_part = None;
                    changed |= cube.set_hilite(Some(piece));
                    changed |= cube.set_ring_active(false);
                    cube_hit = Some(piece);
                } else {
                    // A scrub keeps the ring lit even when the pointer strays off it.
                    ring = self.ring_drag.is_some() || cube.ring_hit(device);
                    changed |= cube.set_ring_active(ring);
                    changed |= cube.set_hilite(None);
                    self.hover = None;
                    self.hovered_part = None;
                }
            } else {
                changed |= cube.set_hilite(None);
                changed |= cube.set_ring_active(false);
            }
        }
        // A piece or the ring consumes the hover; the negative space around them falls
        // through to the gizmo/tool/scene beneath.
        if in_cube && (ring || cube_hit.is_some()) {
            self.hover_report = HoverReport::default();
            return changed;
        }

        // Workbench manipulators are the most specific overlays: an active drag
        // consumes the move; otherwise each claims only its grab targets, and its
        // interior falls through. The most recently added manipulator owns the hover.
        // A disabled style neither draws nor picks them, so the move falls through.
        if self.gizmo_style.enabled
            && self
                .manipulators
                .iter()
                .any(|manipulator| manipulator.is_dragging())
        {
            self.hover_report = HoverReport::default();
            let changed = self
                .manipulators
                .iter_mut()
                .find(|manipulator| manipulator.is_dragging())
                .is_some_and(|manipulator| manipulator.drag_update(&self.camera, device));
            if changed {
                self.rebuild_manipulator_queue();
            }
            return changed;
        }
        let manipulator_hover = if self.gizmo_style.enabled {
            let hits: Vec<bool> = self
                .manipulators
                .iter()
                .map(|manipulator| manipulator.hit(&self.camera, device))
                .collect();
            let top = hits.iter().rposition(|hit| *hit);
            let mut changed = false;
            for (index, manipulator) in self.manipulators.iter_mut().enumerate() {
                changed |= manipulator.set_hovered(Some(index) == top);
            }
            if changed {
                self.rebuild_manipulator_queue();
            }
            (top.is_some(), changed)
        } else {
            (false, false)
        };
        if manipulator_hover.1 {
            self.hover_report = HoverReport::default();
            return true;
        }
        let manipulator_hover = manipulator_hover.0;

        // The gizmos take priority: an active drag consumes the move; otherwise the
        // highest-priority entry claiming the pointer is hovered. Like the manipulators,
        // they only participate while the style enables them.
        let dragging = self.gizmos.dragging();
        let mut gizmo_hover = self.gizmo_style.enabled && dragging.is_some();
        if let (true, Some(index)) = (self.gizmo_style.enabled, dragging) {
            let event = self
                .gizmos
                .gizmo_mut(index)
                .and_then(|gizmo| gizmo.drag_to_with_modifiers(&self.camera, device, modifiers));
            if let Some(event) = event {
                self.apply_gizmo_preview(Some(event.total_transform));
                self.queue_transform_preview(PreviewPhase::Update, event.total_transform);
                self.gizmo_value = Some(event.value);
                self.gizmo_pointer = Some(logical);
                self.hover_report = HoverReport::default();
                return true;
            }
        } else if self.gizmo_style.enabled && self.gizmos.primary().is_some() {
            let target = self.gizmo_handle_at(device);
            gizmo_hover = target.is_some();
            // Only the entry that claimed the pointer shows the hover cue; an entry the
            // pointer is merely near keeps its idle ink.
            let mut recoloured = false;
            for (index, gizmo) in self.gizmos.gizmos_mut().iter_mut().enumerate() {
                recoloured |= gizmo.set_hovered_target(
                    target
                        .filter(|(hit_index, _)| *hit_index == index)
                        .map(|(_, target)| target),
                );
            }
            if recoloured {
                // The hover cue widens the hovered handle, so rebuild its geometry
                // before recompiling the overlay (the colour change alone would need
                // only the queue rebuild).
                for gizmo in self.gizmos.gizmos_mut() {
                    gizmo.update(&self.camera);
                }
                self.rebuild_gizmo_queue();
                self.hover_report = HoverReport::default();
                return true;
            }
        }

        let hit = self.pick(device);
        let consumed = manipulator_hover || gizmo_hover;
        self.hover_report = HoverReport {
            target: if consumed {
                None
            } else {
                hit.as_ref()
                    .and_then(|hit| self.selection_ref(hit.id.0, hit.part))
            },
            through: if consumed {
                None
            } else if in_cube {
                Some("navicube")
            } else {
                None
            },
        };
        self.hover_anchor = if consumed {
            None
        } else {
            hit.as_ref().map(Self::anchor_of)
        };
        changed |= self.dispatch_tool(PointerKind::Move, device, None, hit.as_ref());
        changed | self.dispatch_hover(device, hit.as_ref())
    }

    /// Deliver a mouse-button change, in **viewport-local logical** pixels.
    ///
    /// A press over the navigation cube starts a cube orbit; a release that did not
    /// orbit snaps the main camera onto the piece under the pointer. A press on the
    /// scene starts a possible main-camera orbit, which becomes real once the drag
    /// passes the threshold. Gizmo handles and active tools keep priority.
    pub fn handle_button(&mut self, logical: [f32; 2], button: u8, down: bool) -> bool {
        self.handle_button_with_modifiers_and_click_count(
            logical,
            button,
            down,
            GizmoModifiers::default(),
            1,
        )
    }

    /// [`handle_button`](Self::handle_button) with the keyboard state captured at the
    /// press. `Shift` on a gizmo axis starts a planar drag; the value is carried into the
    /// drag/commit events.
    pub fn handle_button_with_modifiers(
        &mut self,
        logical: [f32; 2],
        button: u8,
        down: bool,
        modifiers: GizmoModifiers,
    ) -> bool {
        self.handle_button_with_modifiers_and_click_count(logical, button, down, modifiers, 1)
    }

    /// [`handle_button_with_modifiers`](Self::handle_button_with_modifiers) carrying
    /// gpui's click count for the press.
    ///
    /// The release event has no count of its own, so the press stores it and the
    /// selection reports it: a double click can upgrade the pick to the whole body at
    /// the host without the viewport guessing timing or distance.
    pub fn handle_button_with_modifiers_and_click_count(
        &mut self,
        logical: [f32; 2],
        button: u8,
        down: bool,
        modifiers: GizmoModifiers,
        click_count: usize,
    ) -> bool {
        // The selection is built on the release; remember the press's count for it.
        if down && button == 1 {
            self.press_click_count = click_count.max(1);
        }
        // A button change ends the readout; the drag/commit event carries the value.
        self.gizmo_value = None;
        self.gizmo_pointer = None;
        // Whether the gesture was an orbit (past the min-drag threshold) rather than a
        // click: an orbit must not also snap on release.
        let threshold = navicube_drag_threshold(self.camera.viewport.device_pixel_ratio());
        let cube_orbited = self
            .navicube_drag
            .as_ref()
            .is_some_and(|drag| drag.moved >= threshold);
        // Any button event ends the gesture state it observed.
        let was_orbiting = self.navicube_drag.is_some() || self.orbit_drag.is_some();
        // A click that never passed the orbit threshold selects instead of navigating.
        let orbit_clicked = self
            .orbit_drag
            .as_ref()
            .is_some_and(|drag| drag.moved < threshold);
        let ring_was_active = self.ring_drag.is_some();
        self.navicube_drag = None;
        self.orbit_drag = None;
        self.snap = None;
        if down {
            // Grabbing the view stops a glide; velocity starts fresh.
            self.inertia = None;
            self.orbit_velocity = [0.0, 0.0];
            self.last_drag_time = None;
            self.ring_drag = None;
        } else if was_orbiting {
            self.start_inertia();
        }
        if !down && ring_was_active {
            self.ring_drag = None;
            return self.finish_ring_drag();
        }
        let device = self.camera.viewport.to_device(logical);
        // A re-pivot release is not a transform commit: it moves the frame and reports
        // the snapped pivot to the host. Finish it before the chrome/cube/scene gestures
        // so a release that strayed over the chrome still ends the drag.
        if !down && self.gizmo_origin_drag.is_some() {
            self.update_gizmo_origin_drag(device);
            return self.finish_gizmo_origin_drag();
        }
        if self.chrome_contains(logical) {
            return false;
        }
        let in_cube = self
            .navicube
            .as_ref()
            .is_some_and(|cube| cube.rect().contains(device));
        let cube_hit = if in_cube {
            self.navicube.as_ref().and_then(|cube| cube.hit(device))
        } else {
            None
        };
        // The ring only owns points the pieces did not already claim.
        let ring_hit = in_cube
            && cube_hit.is_none()
            && self
                .navicube
                .as_ref()
                .is_some_and(|cube| cube.ring_hit(device));
        if ring_hit {
            if down {
                // A ring scrub owns the gesture; a click without movement does nothing.
                // Freeze the projection at press time (see `RingDrag::reference`).
                let reference = Rotation::from_matrix(&self.camera.view);
                self.ring_drag = self
                    .navicube
                    .as_ref()
                    .and_then(|cube| cube.ring_angle_for(device, reference))
                    .map(|last_angle| RingDrag {
                        last_angle,
                        reference,
                    });
                if let Some(cube) = self.navicube.as_mut() {
                    cube.set_ring_active(true);
                }
            }
            return false;
        }
        let cube_hit = if in_cube {
            self.navicube.as_ref().and_then(|cube| cube.hit(device))
        } else {
            None
        };
        if in_cube && cube_orbited {
            return false; // an orbit does not snap
        }
        if in_cube && cube_hit.is_some() {
            if down {
                // A click (no drag) snaps on release; a drag orbits the cube.
                self.navicube_drag = Some(NavicubeDrag {
                    last: logical,
                    moved: 0.0,
                });
                return false;
            }
            if let Some(piece) = cube_hit {
                // Animate the snap rather than jumping: the camera turns onto the
                // piece's face, edge or corner.
                self.snap = Some(SnapAnimation::to(
                    piece.orientation(),
                    &self.camera,
                    Duration::from_millis(220),
                ));
                return true;
            }
        }
        // Negative space inside the gizmo rect (or a click outside it) continues to the
        // gizmo, tool and scene below.
        // A workbench manipulator takes priority over the gizmo and the scene: it is
        // the interaction the current selection asked for. A disabled style neither
        // draws nor picks them, so the button falls through (a drag interrupted by a
        // disable keeps its state but is not serviced or committed).
        if self.gizmo_style.enabled {
            if down {
                let started = self
                    .manipulators
                    .iter_mut()
                    .rev()
                    .any(|manipulator| manipulator.drag_begin(&self.camera, device));
                if started {
                    self.rebuild_manipulator_queue();
                    return true;
                }
            } else {
                let commit = self
                    .manipulators
                    .iter_mut()
                    .rev()
                    .find(|manipulator| manipulator.is_dragging())
                    .and_then(|manipulator| manipulator.drag_end());
                if let Some(commit) = commit {
                    self.interaction_commit = Some(commit);
                    self.rebuild_manipulator_queue();
                    return true;
                }
            }
        }

        // The hosted gizmos take priority over the tool and the scene. The pointer is
        // dispatched to the entry the set's hit priority named, so a lower-priority
        // gizmo can never steal a press from the one that claimed it.
        if self.gizmo_style.enabled && self.gizmos.primary().is_some() {
            if down {
                if let Some((index, target)) = self.gizmo_handle_at(device) {
                    if target == GizmoTarget::Origin {
                        // The origin/centre handle moves that entry's frame, not the target.
                        if self.begin_gizmo_origin_drag(index, device) {
                            return true;
                        }
                    } else if let Some(handle) = target.handle() {
                        let started = self
                            .gizmos
                            .gizmo_mut(index)
                            .is_some_and(|gizmo| {
                                gizmo.begin_drag_with_modifiers(
                                    handle,
                                    &self.camera,
                                    device,
                                    modifiers,
                                )
                            });
                        if started {
                            self.queue_transform_preview(PreviewPhase::Begin, Matrix4::identity());
                            self.rebuild_gizmo_queue();
                            return true;
                        }
                    }
                }
            } else if let Some(index) = self.gizmos.dragging() {
                let commit = self
                    .gizmos
                    .gizmo_mut(index)
                    .and_then(|gizmo| gizmo.end_drag());
                if let Some(commit) = commit {
                    if self.gizmo_deferred {
                        // The host owns the commit: leave its preview in place and let
                        // it decide when (or whether) to apply the delta.
                        self.queue_transform_preview(PreviewPhase::End, commit.total_transform);
                    } else {
                        self.gizmo_commit = Some(commit);
                        // The host applies the committed transform to the document; clear
                        // the preview it was showing.
                        self.apply_gizmo_preview(None);
                    }
                    self.rebuild_gizmo_queue();
                    return true;
                }
            }
        }

        // A scene drag orbits the camera. The left button leaves the gesture to an
        // active tool; the middle button always navigates.
        if down && (button == 2 || (button == 1 && self.tool.is_none())) {
            self.orbit_drag = Some(OrbitDrag {
                last: logical,
                moved: 0.0,
            });
        }

        let kind = if down {
            PointerKind::Down
        } else {
            PointerKind::Up
        };
        let hit = self.pick(device);
        // Tier 1: a creation tool that starts on a planar face binds that face for the
        // session, with no explicit activation step. Only the session's **first** press
        // binds — a later click on another face must not slide the plane out from under
        // points already placed. An explicitly bound plane is sticky and is never
        // overridden by a click. A **plane-picking** tool is excluded: its points are the
        // input, so binding the first face would flatten the other two onto its plane.
        if down
            && button == 1
            && self.active_workplane.is_default()
            && self
                .tool
                .as_ref()
                .is_some_and(|tool| tool.status().points == 0 && !tool.picks_workplane())
            && let Some(frame) = hit.as_ref().and_then(|hit| self.face_frame_at(hit))
        {
            self.ephemeral_restore = Some(self.active_workplane.clone());
            self.set_workplane(Some(frame));
        }
        let mut changed = self.dispatch_tool(kind, device, stage_button(button), hit.as_ref());
        let event = Event::MouseButton {
            position: device,
            button,
            down,
        };
        changed |= self.dispatch(&event, hit.as_ref());

        // A left click that reached the scene selects what is under the pointer (or
        // clears the selection on empty space). A gizmo commit, the navigation cube, a
        // ring scrub, an orbit drag and an active tool all returned or are excluded
        // above, so their gestures never also select.
        if !down && button == 1 && self.tool.is_none() && orbit_clicked {
            self.selection = Some(self.selection_event(hit.as_ref(), self.press_click_count));
            changed = true;
        }
        changed
    }

    /// Turn a pick into the host-facing selection event.
    ///
    /// The hit's world point and normal travel with the reference so the host can
    /// anchor a gizmo at the point of contact instead of the body's centre.
    fn selection_event(&self, hit: Option<&Hit>, click_count: usize) -> SelectionEvent {
        let Some(reference) = hit.and_then(|hit| self.selection_ref(hit.id.0, hit.part)) else {
            return SelectionEvent::Cleared;
        };
        SelectionEvent::Picked {
            reference,
            anchor: hit.map(Self::anchor_of),
            click_count,
        }
    }

    /// The world point and normal of a hit.
    fn anchor_of(hit: &Hit) -> SelectionAnchor {
        SelectionAnchor {
            point: Vector3::new(
                f64::from(hit.point[0]),
                f64::from(hit.point[1]),
                f64::from(hit.point[2]),
            ),
            normal: Vector3::new(
                f64::from(hit.normal[0]),
                f64::from(hit.normal[1]),
                f64::from(hit.normal[2]),
            ),
        }
    }

    /// Zoom the main camera by `lines` wheel notches (positive = towards the scene).
    ///
    /// The orbit point is the current camera pivot (the origin of its view space), so
    /// zoom keeps the view direction: a perspective camera moves its eye, an
    /// orthographic one scales its extent.
    pub fn handle_scroll(&mut self, lines: f32) -> bool {
        if !lines.is_finite() || lines == 0.0 {
            return false;
        }
        const ZOOM_PER_LINE: f64 = 1.15;
        self.snap = None;
        self.inertia = None;
        let factor = ZOOM_PER_LINE.powf(-f64::from(lines));
        let next = crate::navicube::zoom_view(&self.camera, factor);
        if next == self.camera.view {
            return false;
        }
        self.camera.view = next;
        // Perspective zoom is the eye distance; an orthographic view zooms by its extent.
        self.camera.zoom_projection(factor);
        // Keep the clip window around the new distance: a fixed near/far would cut the
        // scene off when zoomed in (near) or out (far).
        self.camera.fit_clip();
        self.recompile();
        true
    }

    /// The camera the viewport draws with.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Whether the camera is currently orthographic.
    pub fn is_orthographic(&self) -> bool {
        self.camera.is_orthographic()
    }

    /// Replace the projection. Framing is preserved only when the caller chooses
    /// projection parameters consistent with the current eye distance; the chrome uses
    /// [`set_orthographic`](Self::set_orthographic), which does. Returns whether it
    /// changed.
    pub fn set_projection(&mut self, projection: Projection) -> bool {
        if !self.camera.set_projection(projection) {
            return false;
        }
        self.recompile();
        true
    }

    /// Switch perspective/orthographic, preserving the apparent size at the pivot.
    /// Returns whether it changed.
    pub fn set_orthographic(&mut self, orthographic: bool) -> bool {
        if orthographic == self.camera.is_orthographic() {
            return false;
        }
        if !self.camera.toggle_projection() {
            return false;
        }
        self.recompile();
        true
    }

    /// Toggle perspective/orthographic (see [`set_orthographic`](Self::set_orthographic)).
    pub fn toggle_projection(&mut self) -> bool {
        if !self.camera.toggle_projection() {
            return false;
        }
        self.recompile();
        true
    }

    /// Animate the camera back to the home orientation (the chrome's Home control).
    pub fn go_home(&mut self) -> bool {
        self.inertia = None;
        self.snap = Some(SnapAnimation::to(
            crate::navicube::home_orientation(),
            &self.camera,
            Duration::from_millis(220),
        ));
        true
    }

    /// Yaw the view ±90° about the vertical (+Z) axis, animated (the chrome's arrows).
    /// Yaw keeps the horizon level and the turntable horizontal, so the angle readout
    /// changes by ±90° and the navicube's `T` stays upright.
    ///
    /// The target composes from a pending animation's target, not from the partially
    /// animated camera: otherwise two quick opposite clicks would both start from a
    /// half-turned pose and the view would not return to where it started.
    pub fn yaw(&mut self, direction: YawDirection) -> bool {
        self.inertia = None;
        let base = if let Some(snap) = self.snap.as_ref() {
            Camera {
                view: Placement::new(
                    Vector3::new(
                        self.camera.view.m[3],
                        self.camera.view.m[7],
                        self.camera.view.m[11],
                    ),
                    snap.target(),
                )
                .to_matrix(),
                ..self.camera.clone()
            }
        } else {
            self.camera.clone()
        };
        let target = Rotation::from_matrix(&crate::navicube::yaw_view(&base, direction.angle()));
        self.snap = Some(SnapAnimation::to(
            target,
            &self.camera,
            Duration::from_millis(220),
        ));
        true
    }

    /// The chrome bar's logical rect `[left, top, right, bottom]`, if the navigation cube
    /// is placed.
    fn chrome_rect(&self) -> Option<[f32; 4]> {
        self.navicube.as_ref().map(chrome_bar_rect)
    }

    /// Whether `logical` is over the gpui chrome bar. The bar sits inside the viewport
    /// element, so pointer events bubble to the viewport's handlers; the handlers ignore
    /// them here and let the bar's own buttons own the click.
    fn chrome_contains(&self, logical: [f32; 2]) -> bool {
        self.chrome_rect().is_some_and(|[left, top, right, bottom]| {
            logical[0] >= left && logical[0] <= right && logical[1] >= top && logical[1] <= bottom
        })
    }

    /// Deliver a pointer event to the active tool (if any) and rebuild its transient
    /// visuals. The pick is taken from the scene-and-stage queue, so a tool never snaps
    /// to its own transient items. Returns whether a tool is active.
    fn dispatch_tool(
        &mut self,
        kind: PointerKind,
        device: [i32; 2],
        button: Option<StageButton>,
        hit: Option<&Hit>,
    ) -> bool {
        if self.tool.is_none() {
            return false;
        }
        let event = PointerEvent {
            kind,
            position: device,
            button,
            hit: hit.copied(),
            // A tool hit-tests its own geometry through this (screen space, so the
            // threshold is zoom-independent).
            projection: Some(ViewProjection::from_camera(&self.camera)),
        };
        let before_status = self.tool.as_ref().map(|tool| tool.status());
        let context = self.snap_context();
        let candidates = self.snap_candidates(hit);
        let snap = snap_to_geometry_with(&event, &self.camera, context.as_ref(), &candidates);
        self.set_snap_feedback(snap, context.map(|context| context.workplane));
        if let Some(tool) = self.tool.as_mut() {
            tool.on_event(&event, snap);
        }
        self.rebuild_transient();
        // Report a status change (a placed point, a changed hint); a plain cursor move
        // leaves the status equal and emits nothing.
        if let Some(tool) = self.tool.as_ref() {
            let status = tool.status();
            if before_status.as_ref() != Some(&status) {
                self.tool_state = Some(status);
            }
        }
        true
    }

    /// Rebuild the transient queue from the active tool's items.
    fn rebuild_transient(&mut self) {
        let mut queue = RenderQueue::default();
        if let Some(tool) = self.tool.as_ref() {
            for item in tool.items() {
                item.emit(&mut queue);
            }
        }
        self.transient = queue;
    }

    /// The hosted gizmo entry and its target under a device position, if any. The set
    /// owns an **analytic** screen-space hit test (2 px stroke + 10 px grab padding,
    /// plus the 10 px origin disc), so the triad's empty interior never claims a click
    /// meant for the scene. Entries are tried in registry priority order, so the first
    /// one to claim the pointer answers. A disabled style picks nothing, so the click
    /// falls through to the scene beneath.
    fn gizmo_handle_at(&self, device: [i32; 2]) -> Option<(usize, GizmoTarget)> {
        if !self.gizmo_style.enabled {
            return None;
        }
        self.gizmos.hit(&self.camera, device)
    }

    /// The target the hosted set claims at `device` (the highest-priority entry's), for
    /// a caller that only needs to know *what* was hit.
    #[cfg(test)]
    fn gizmo_target_at(&self, device: [i32; 2]) -> Option<GizmoTarget> {
        self.gizmo_handle_at(device).map(|(_, target)| target)
    }

    /// Whether the gizmo queue draws the authored skin rather than the native look.
    ///
    /// The style must be enabled and a skin program must have been injected with
    /// [`set_gizmo_skin`](Self::set_gizmo_skin). The backend id is the shell's business
    /// (it clears the skin when the active provider supplies none), so it does not gate
    /// skinning here. Otherwise the native packets are drawn exactly as before.
    fn skin_active(&self) -> bool {
        self.gizmo_style.enabled && self.gizmo_skin.is_some()
    }

    /// Rebuild the gizmos' handle batches from the hosted set, in priority order. A
    /// disabled style draws nothing (the gizmos stay hosted).
    ///
    /// When [`skin_active`](Self::skin_active) the handles are re-materialised with the
    /// injected skin program; otherwise they draw the native look. The skin changes
    /// materials only: [`gizmo_handle_at`](Self::gizmo_handle_at), the drag entry points,
    /// the readout and the commit routing all still use the native [`TransformGizmo`].
    fn rebuild_gizmo_queue(&mut self) {
        let mut queue = RenderQueue::default();
        if self.gizmo_style.enabled {
            let skin = self.gizmo_skin.as_ref().filter(|_| self.skin_active()).cloned();
            for index in self.gizmos.offered() {
                let gizmo = &self.gizmos.gizmos()[index];
                match skin.as_ref() {
                    Some(program) => {
                        // The outline pass stays native: it is the contrast aid, and a
                        // skin must not be able to remove it.
                        for item in gizmo.halos() {
                            item.emit(&mut queue);
                        }
                        for item in gizmo.skinned_items(program) {
                            item.emit(&mut queue);
                        }
                    }
                    None => {
                        for item in gizmo.halos() {
                            item.emit(&mut queue);
                        }
                        for item in gizmo.items() {
                            item.emit(&mut queue);
                        }
                    }
                }
            }
        }
        self.gizmo_queue = queue;
    }

    /// Rebuild the manipulators' overlay from their current items. A disabled style
    /// draws nothing (the manipulators stay hosted).
    ///
    /// Like the gizmo, an active skin re-materialises every item with the injected
    /// program, so a workspace-wide skin cannot leave one overlay looking native. The
    /// manipulator still supplies the geometry and the readout.
    fn rebuild_manipulator_queue(&mut self) {
        let mut queue = RenderQueue::default();
        if self.gizmo_style.enabled {
            for manipulator in &mut self.manipulators {
                manipulator.refresh(&self.camera);
            }
            let skin = self.gizmo_skin.as_ref().filter(|_| self.skin_active());
            for manipulator in &self.manipulators {
                for item in manipulator.items() {
                    match skin {
                        Some(program) => {
                            let mut skinned = item.clone();
                            skinned.set_program(Some(Arc::clone(program)));
                            skinned.emit(&mut queue);
                        }
                        None => item.emit(&mut queue),
                    }
                }
            }
        }
        self.manipulator_queue = queue;
    }

    /// The gizmo's numeric pill: the active drag's readout text and its logical anchor.
    /// `None` while the gizmo is disabled, readouts are hidden, or no drag is active.
    fn gizmo_readout(&self) -> Option<(String, [f32; 2])> {
        if !self.gizmo_style.enabled || !self.gizmo_style.show_readout {
            return None;
        }
        if let Some(drag) = self.gizmo_origin_drag.as_ref() {
            let anchor = project_device(&self.camera, drag.point)
                .map(|device| {
                    self.camera
                        .viewport
                        .to_logical([device[0].round() as i32, device[1].round() as i32])
                })
                .or(self.gizmo_pointer)
                .unwrap_or([0.0, 0.0]);
            return Some((format!("pivot · {}", snap_name(drag.kind)), anchor));
        }
        let value = self.gizmo_value?;
        let index = self.gizmos.dragging()?;
        let gizmo = self.gizmos.gizmos().get(index)?;
        if !gizmo.is_dragging() {
            return None;
        }
        let anchor = gizmo
            .readout_anchor(&self.camera)
            .map(|device| {
                self.camera
                    .viewport
                    .to_logical([device[0].round() as i32, device[1].round() as i32])
            })
            .or(self.gizmo_pointer)
            .unwrap_or([0.0, 0.0]);
        Some((value.readout(), anchor))
    }

    /// The topmost manipulator's numeric pill: its readout text and logical anchor.
    /// `None` while the overlays are disabled or readouts are hidden.
    fn manipulator_readout(&self) -> Option<(String, [f32; 2])> {
        if !self.gizmo_style.enabled || !self.gizmo_style.show_readout {
            return None;
        }
        let (text, anchor) = self.manipulators.iter().rev().find_map(|manipulator| {
            Some((
                manipulator.readout()?,
                manipulator.readout_anchor(&self.camera)?,
            ))
        })?;
        let logical = self
            .camera
            .viewport
            .to_logical([anchor[0].round() as i32, anchor[1].round() as i32]);
        Some((text, logical))
    }

    /// The numeric readout the viewport would show, if the style allows one.
    ///
    /// The render path asks the two helpers separately (it positions two pills); this is
    /// the unioned form the tests use to assert the `show_readout` gate.
    #[cfg(test)]
    fn readout_text(&self) -> Option<String> {
        self.gizmo_readout()
            .map(|(text, _)| text)
            .or_else(|| self.manipulator_readout().map(|(text, _)| text))
    }

    /// Preview a world-space translation on the given entity ids without touching
    /// the document.
    ///
    /// This is the typed twin of a gizmo drag: it composes a temporary transform after
    /// each item's placement, so nothing is re-uploaded and nothing enters the undo
    /// stack. An empty `entities` clears the preview. Returns whether the stage
    /// changed (the caller still decides when to repaint).
    pub fn preview_translation(&mut self, entities: &[u32], delta: [f64; 3]) -> bool {
        let active = !entities.is_empty();
        let mut changed = if active {
            let target = Vector3::new(delta[0], delta[1], delta[2]);
            // The gizmos are the control while one is being dragged; otherwise they
            // travel with the previewed object. Grips always follow.
            let include_gizmo = self.gizmos.dragging().is_none();
            self.apply_preview_overlays(target, include_gizmo)
        } else {
            self.restore_preview_overlays()
        };
        let transform = active.then(|| Matrix4::translation(delta[0], delta[1], delta[2]));
        let mut touched = false;
        for item in self.stage.items_mut() {
            if entities.contains(&item.entity_id()) {
                item.set_temporary_transform(transform);
                touched = true;
            } else if transform.is_none() {
                // Clearing the preview also drops a temporary transform another path
                // left behind, so a cancelled HUD cannot strand a ghost.
                item.set_temporary_transform(None);
            }
        }
        if touched || transform.is_none() {
            self.stage_dirty = true;
            changed = true;
        }
        if changed {
            // A draw-only gizmo offset changes the emitted packets without marking the
            // gizmo geometry dirty, so rebuild the overlay queue explicitly.
            self.rebuild_gizmo_queue();
            self.rebuild_manipulator_queue();
        }
        changed
    }

    /// Offset the overlays from the pose captured when the preview began.
    ///
    /// Absolute per tick (`home + delta`), so a typed value and a drag cannot
    /// accumulate. Every offered gizmo is skipped while one is the control being dragged;
    /// the manipulators (a height grip on the top face) always follow the object.
    fn apply_preview_overlays(&mut self, delta: Vector3, include_gizmo: bool) -> bool {
        if self.preview_home.is_none() {
            let gizmos = self
                .gizmos
                .offered()
                .into_iter()
                .map(|index| (index, self.gizmos.gizmos()[index].origin()))
                .collect();
            self.preview_home = Some(PreviewHome {
                gizmos,
                manipulators: self
                    .manipulators
                    .iter()
                    .map(|manipulator| manipulator.anchor())
                    .collect(),
            });
        }
        let Some(home) = self.preview_home.as_ref() else {
            return false;
        };
        let home_gizmos = home.gizmos.clone();
        let mut changed = false;
        for (index, origin) in home_gizmos {
            let Some(gizmo) = self.gizmos.gizmo_mut(index) else {
                continue;
            };
            if include_gizmo {
                // Not dragging: the real frame follows, so hit-testing agrees with the draw.
                gizmo.set_display_offset(None);
                gizmo.set_origin(origin.add(&delta));
            } else {
                // A gizmo is the control: freeze its interaction frame and move only the
                // drawing, so the triad stays glued to the surface without feeding back
                // into the drag math. `delta` is measured from the captured home, and the
                // frame may already sit at a previous drag's end (a deferred HUD keeps the
                // preview between drags), so offset by where the preview puts the triad
                // from where it actually is — not by `delta` again.
                let draw = origin.add(&delta);
                gizmo.set_display_offset(Some(draw.sub(&gizmo.origin())));
            }
            changed = true;
        }
        for (manipulator, anchor) in self.manipulators.iter_mut().zip(home.manipulators.iter()) {
            if let Some(anchor) = anchor {
                changed |= manipulator.set_anchor(anchor.add(&delta));
            }
        }
        changed
    }

    /// Restore the overlays to their pre-preview pose and forget the snapshot.
    fn restore_preview_overlays(&mut self) -> bool {
        let Some(home) = self.preview_home.take() else {
            return false;
        };
        let mut changed = false;
        for (index, origin) in home.gizmos {
            if let Some(gizmo) = self.gizmos.gizmo_mut(index) {
                gizmo.set_display_offset(None);
                gizmo.set_origin(origin);
                changed = true;
            }
        }
        for (manipulator, anchor) in self.manipulators.iter_mut().zip(home.manipulators.iter()) {
            if let Some(anchor) = anchor {
                changed |= manipulator.set_anchor(*anchor);
            }
        }
        changed
    }

    /// Preview `transform` on the gizmo's target stage item (or clear the preview).
    fn apply_gizmo_preview(&mut self, transform: Option<Matrix4>) {
        let Some(target) = self
            .gizmos
            .dragging()
            .or_else(|| self.gizmos.primary())
            .map(|index| self.gizmos.gizmos()[index].target_id())
        else {
            return;
        };
        let mut changed = match &transform {
            Some(transform) => {
                let delta = Vector3::new(transform.m[3], transform.m[7], transform.m[11]);
                // The gizmo is the control here: only the grips follow the object.
                self.apply_preview_overlays(delta, false)
            }
            None => self.restore_preview_overlays(),
        };
        let mut touched = false;
        for item in self.stage.items_mut() {
            if item.entity_id() == target {
                item.set_temporary_transform(transform);
                touched = true;
            }
        }
        if touched {
            self.stage_dirty = true;
            changed = true;
        }
        if changed {
            self.rebuild_gizmo_queue();
            self.rebuild_manipulator_queue();
        }
    }

    /// Handle a gpui drag-move: orbit the navigation cube or the main camera,
    /// depending on where the press began. Reports whether the view changed. The
    /// modifier-aware form is the production path; this convenience is for tests.
    #[cfg(test)]
    fn drag_move(&mut self, logical: [f32; 2]) -> bool {
        self.drag_move_with_modifiers(logical, GizmoModifiers::default())
    }

    /// [`drag_move`](Self::drag_move) with the current keyboard modifiers. Gizmo drags
    /// are also serviced here so a pointer that leaves the element still updates them
    /// through gpui's drag system.
    fn drag_move_with_modifiers(&mut self, logical: [f32; 2], modifiers: GizmoModifiers) -> bool {
        if self.chrome_contains(logical) {
            return false;
        }
        // A live re-pivot owns the drag, including moves outside the element.
        if self.gizmo_origin_drag.is_some() {
            let device = self.camera.viewport.to_device(logical);
            return self.update_gizmo_origin_drag(device);
        }
        // An active manipulator drag owns the gesture, even when the pointer leaves the
        // element (gpui keeps delivering the drag). A disabled style stops servicing
        // the drag without dropping its state.
        if self.gizmo_style.enabled
            && self
                .manipulators
                .iter()
                .any(|manipulator| manipulator.is_dragging())
        {
            let device = self.camera.viewport.to_device(logical);
            let changed = self
                .manipulators
                .iter_mut()
                .find(|manipulator| manipulator.is_dragging())
                .is_some_and(|manipulator| manipulator.drag_update(&self.camera, device));
            if changed {
                self.rebuild_manipulator_queue();
            }
            return changed;
        }
        if let (true, Some(index)) = (self.gizmo_style.enabled, self.gizmos.dragging()) {
            let device = self.camera.viewport.to_device(logical);
            let event = self
                .gizmos
                .gizmo_mut(index)
                .and_then(|gizmo| gizmo.drag_to_with_modifiers(&self.camera, device, modifiers));
            if let Some(event) = event {
                self.apply_gizmo_preview(Some(event.total_transform));
                self.queue_transform_preview(PreviewPhase::Update, event.total_transform);
                self.gizmo_value = Some(event.value);
                self.gizmo_pointer = Some(logical);
                return true;
            }
            return false;
        }
        self.snap = None; // a drag cancels any in-progress snap
        self.inertia = None;
        if self.ring_drag.is_some() {
            let device = self.camera.viewport.to_device(logical);
            let (last, reference) = {
                let drag = self.ring_drag.as_ref().expect("ring drag");
                (drag.last_angle, drag.reference)
            };
            let angle = self
                .navicube
                .as_ref()
                .and_then(|cube| cube.ring_angle_for(device, reference));
            let Some(angle) = angle else {
                return false;
            };
            let delta = shortest_angle(angle - last);
            if delta.abs() < 1e-6 {
                return false;
            }
            if let Some(drag) = self.ring_drag.as_mut() {
                drag.last_angle = angle;
            }
            self.camera.view = crate::navicube::yaw_view(&self.camera, delta);
            self.recompile();
            return true;
        }
        let threshold = navicube_drag_threshold(self.camera.viewport.device_pixel_ratio());
        let delta = if let Some(state) = self.navicube_drag.as_mut() {
            let delta = [logical[0] - state.last[0], logical[1] - state.last[1]];
            state.last = logical;
            state.moved += (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
            (state.moved >= threshold).then_some(delta)
        } else if let Some(state) = self.orbit_drag.as_mut() {
            let delta = [logical[0] - state.last[0], logical[1] - state.last[1]];
            state.last = logical;
            state.moved += (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
            (state.moved >= threshold).then_some(delta)
        } else {
            None
        };
        let Some(delta) = delta else {
            return false;
        };
        self.track_orbit_velocity(delta);
        self.camera.view = crate::navicube::drag_view(&self.camera, delta);
        self.recompile();
        true
    }

    /// Settle a finished ring scrub on the nearest 90° heading when the release is close
    /// enough to a detent. Returns whether a snap animation started.
    fn finish_ring_drag(&mut self) -> bool {
        if let Some(cube) = self.navicube.as_mut() {
            cube.set_ring_active(false);
        }
        let (azimuth, _) = camera_angles(&self.camera);
        let target = (azimuth / RING_DETENT_DEGREES).round() * RING_DETENT_DEGREES;
        let delta = (target - azimuth).rem_euclid(360.0);
        let delta = if delta > 180.0 { delta - 360.0 } else { delta };
        if delta.abs() > RING_DETENT_CAPTURE {
            return false;
        }
        if delta.abs() < 1e-6 {
            return false; // already on the detent
        }
        let target_view = crate::navicube::yaw_view(&self.camera, delta.to_radians());
        self.snap = Some(SnapAnimation::to(
            Rotation::from_matrix(&target_view),
            &self.camera,
            Duration::from_millis(180),
        ));
        true
    }

    /// Feed one orbit delta into the smoothed release velocity (logical px/s). A pause
    /// longer than the sample window drops the velocity, so releasing after a stop does
    /// not fling the view.
    fn track_orbit_velocity(&mut self, delta: [f32; 2]) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_drag_time {
            let dt = now.saturating_duration_since(last).as_secs_f32();
            if (1e-3..=0.2).contains(&dt) {
                let sample = [delta[0] / dt, delta[1] / dt];
                const SMOOTHING: f32 = 0.35;
                self.orbit_velocity[0] += (sample[0] - self.orbit_velocity[0]) * SMOOTHING;
                self.orbit_velocity[1] += (sample[1] - self.orbit_velocity[1]) * SMOOTHING;
            } else if dt > 0.2 {
                self.orbit_velocity = [0.0, 0.0];
            }
        }
        self.last_drag_time = Some(now);
    }

    /// Begin an inertial glide from the release velocity, if the pointer was still moving
    /// when it let go.
    fn start_inertia(&mut self) {
        self.inertia = None;
        if self
            .last_drag_time
            .is_none_or(|last| last.elapsed() > Duration::from_millis(150))
        {
            self.orbit_velocity = [0.0, 0.0];
        }
        let [vx, vy] = self.orbit_velocity;
        if (vx * vx + vy * vy).sqrt() >= ORBIT_INERTIA_START {
            self.inertia = Some(Inertia {
                velocity: self.orbit_velocity,
                last: std::time::Instant::now(),
            });
        }
        self.orbit_velocity = [0.0, 0.0];
    }

    /// Advance the inertial orbit by the time since its last step, returning whether it is
    /// still running (the render loop requests frames until it is done).
    fn step_inertia(&mut self) -> bool {
        let Some(inertia) = self.inertia.as_ref() else {
            return false;
        };
        let now = std::time::Instant::now();
        let dt = now.saturating_duration_since(inertia.last);
        if let Some(inertia) = self.inertia.as_mut() {
            inertia.last = now;
        }
        self.step_inertia_by(dt)
    }

    /// The deterministic core of [`step_inertia`](Self::step_inertia).
    fn step_inertia_by(&mut self, dt: Duration) -> bool {
        let Some(inertia) = self.inertia.as_ref() else {
            return false;
        };
        let seconds = dt.as_secs_f32().min(0.1);
        if seconds <= 0.0 {
            return true;
        }
        let velocity = inertia.velocity;
        let delta = [velocity[0] * seconds, velocity[1] * seconds];
        if delta == [0.0, 0.0] {
            self.inertia = None;
            return false;
        }
        let decay = (-seconds / ORBIT_INERTIA_TAU).exp();
        let next = [velocity[0] * decay, velocity[1] * decay];
        let speed = (next[0] * next[0] + next[1] * next[1]).sqrt();
        self.camera.view = crate::navicube::drag_view(&self.camera, delta);
        self.recompile();
        if speed < ORBIT_INERTIA_STOP {
            self.inertia = None;
            false
        } else {
            if let Some(inertia) = self.inertia.as_mut() {
                inertia.velocity = next;
            }
            true
        }
    }

    /// Deliver a pointer leave.
    pub fn handle_pointer_leave(&mut self) -> bool {
        let had_bloom = self.origin_bloomed();
        self.hover = None;
        self.hovered_part = None;
        self.pointer = None;
        self.hover_report = HoverReport::default();
        let had_feedback = self.snap_feedback.is_some();
        self.set_snap_feedback(None, None);
        let mut changed = had_feedback || had_bloom;
        if let Some(cube) = self.navicube.as_mut() {
            changed |= cube.set_hilite(None);
            changed |= cube.set_proximity(false);
            changed |= cube.set_ring_active(false);
        }
        changed | self.dispatch(&Event::MouseLeave, None)
    }

    /// The hit under a device position.
    ///
    /// Priority: the presenter's G-buffer pick (the window's device, which already
    /// holds the frame it drew), then the fallback view's hardware pick, then the CPU
    /// ray cast. All three consume the same scene-and-stage queue and camera, so this
    /// is one pick, not three parallel implementations.
    fn pick(&mut self, device: [i32; 2]) -> Option<Hit> {
        if let Some(presenter) = self.presenter.as_ref() {
            let mut presenter = presenter.borrow_mut();
            if presenter.supports_pick() {
                // Authoritative: `None` here means empty space, not "try elsewhere".
                // The grid lives in its own queue, so the pick never sees it.
                return presenter.pick_at(&self.queue, &self.camera, device);
            }
        }
        if self.view.supports_pick() {
            self.view.pick_at(&self.queue, &self.camera, device)
        } else {
            pick_at_hit(&self.queue, &self.camera, device)
        }
    }

    /// Update the hover target and run the `MouseMove` listeners for `hit`.
    fn dispatch_hover(&mut self, device: [i32; 2], hit: Option<&Hit>) -> bool {
        let next_hover = hit.map(|hit| hit.id);
        let next_part = hit.map(|hit| hit.part);
        let changed = self.hover != next_hover || self.hovered_part != next_part;
        self.hover = next_hover;
        self.hovered_part = next_part;
        self.refresh_hover_face();
        let event = Event::MouseMove { position: device };
        changed | self.dispatch(&event, hit)
    }

    /// Run the listeners on an event and recompile if any changed the scene.
    ///
    /// Every listener sees every event (an observation-only listener, like the
    /// Python `SoEventCallback` bridge, must not stop a later one from running). The
    /// listeners also receive the last compile's directives, so a Python bridge can
    /// route the event to the active callbacks without rescanning the scene.
    fn dispatch(&mut self, event: &Event, hit: Option<&Hit>) -> bool {
        let mut changed = false;
        for listener in &mut self.listeners {
            if listener.on_event(event, hit, &mut self.scene, &self.directives) {
                changed = true;
            }
        }
        if changed {
            self.recompile();
        }
        changed
    }

    fn recompile(&mut self) {
        // Keep the no-overlay path allocation-free: it is the common case and the
        // base scene can be large.
        let (queue, directives, shader_error) = if self.overlays.is_empty() {
            merge_queue(&self.scene, &self.stage, &self.camera.viewport)
        } else {
            let merged = self.merged_scene();
            merge_queue(&merged, &self.stage, &self.camera.viewport)
        };
        let (queue, mut program_queue) = split_programs(queue);
        apply_program_shadows(&mut program_queue, &self.effective_shadow_footprints());
        self.queue = queue;
        self.program_queue = program_queue;
        self.directives = directives;
        self.shader_error = shader_error;
        self.stage_dirty = false;
        self.sync_grid();
        self.sync_directive_cubes();
    }

    /// The base scene with every overlay root under one `Separator`.
    ///
    /// Only called when at least one overlay is registered; the extra level is
    /// introduced then, so the no-overlay path keeps exactly the packets it had
    /// before G5.
    fn merged_scene(&self) -> Scene {
        let mut children = Vec::with_capacity(self.overlays.len() + 1);
        children.push(self.scene.root.clone());
        children.extend(self.overlays.iter().map(|overlay| overlay.root.clone()));
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children,
            },
        }
    }

    /// Rebuild the `Directive::NaviCube` overlays when the set of requests changed.
    ///
    /// The set is stable across camera moves, so this is usually a no-op; a scene that
    /// adds, removes or reconfigures a `SoNaviCube` rebuilds only the affected overlays.
    fn sync_directive_cubes(&mut self) {
        let wanted: Vec<NaviCubeDirective> = self.directives.navicubes().copied().collect();
        let unchanged = wanted.len() == self.directive_cubes.len()
            && wanted
                .iter()
                .zip(&self.directive_cubes)
                .all(|(directive, (current, _))| directive == current);
        if unchanged {
            return;
        }
        self.directive_cubes = wanted
            .into_iter()
            .map(|directive| {
                (
                    directive,
                    NaviCube::from_directive(self.camera.viewport, directive),
                )
            })
            .collect();
    }

    /// Show or hide the reference grid's ground field (G2, `App_ToggleGrid`). The
    /// triads are the axis cross's business, not the grid's. Returns whether it changed.
    pub fn set_grid(&mut self, enabled: bool) -> bool {
        if enabled == self.grid_enabled {
            return false;
        }
        self.grid_enabled = enabled;
        self.recompile();
        true
    }

    /// Whether the reference grid's ground field is shown.
    pub fn grid_enabled(&self) -> bool {
        self.grid_enabled
    }

    /// Show or hide the axis cross: the world triad, its origin node and the axis tags
    /// (`App_ToggleAxisCross`, on by default as upstream's axis cross is). This is the
    /// legacy boolean alias for [`axis`](AxisStyle::enabled). Returns whether it changed.
    pub fn set_axis_cross(&mut self, enabled: bool) -> bool {
        if enabled == self.axis.enabled {
            return false;
        }
        self.axis.enabled = enabled;
        self.sync_grid();
        true
    }

    /// Whether the axis cross is shown.
    pub fn axis_cross(&self) -> bool {
        self.axis.enabled
    }

    /// Replace the axis cross's whole appearance (colours, opacity, geometry, toggles).
    /// Returns whether anything changed.
    pub fn set_axis_style(&mut self, style: AxisStyle) -> bool {
        if style == self.axis {
            return false;
        }
        self.axis = style;
        self.sync_grid();
        true
    }

    /// The axis cross's current style.
    pub fn axis_style(&self) -> &AxisStyle {
        &self.axis
    }

    /// Set the world grid's minor and major steps (mm). Invalid pairs (non-positive, or
    /// a major below the minor) are ignored. Returns whether the grid changed.
    pub fn set_grid_steps(&mut self, minor: f64, major: f64) -> bool {
        if !(minor.is_finite() && minor > 0.0 && major.is_finite() && major > 0.0)
            || major < minor - 1e-9
        {
            return false;
        }
        if (self.grid_minor - minor).abs() < 1e-12 && (self.grid_major - major).abs() < 1e-12 {
            return false;
        }
        self.grid_minor = minor;
        self.grid_major = major;
        self.recompile();
        true
    }

    /// The world grid's `(minor, major)` steps, in millimetres.
    ///
    /// These are the **configured** pair. With adaptive spacing the drawn step is derived
    /// from the plane's extent instead; the request's step is what a plane with no extent
    /// (an empty scene, a very small model) falls back to.
    pub fn grid_steps(&self) -> (f64, f64) {
        (self.grid_minor, self.grid_major)
    }

    /// Whether grid spacing follows the plane's extent (`FerroCAD.Grid.adaptive`).
    ///
    /// Returns whether the grid changed. This is a step policy, so only the grid layers
    /// are rebuilt; the scene queue is untouched.
    pub fn set_grid_adaptive(&mut self, adaptive: bool) -> bool {
        if self.grid_adaptive == adaptive {
            return false;
        }
        self.grid_adaptive = adaptive;
        self.sync_grid();
        true
    }

    /// Whether grid spacing is adaptive.
    pub fn grid_adaptive(&self) -> bool {
        self.grid_adaptive
    }

    /// Set the grid's stroke: the minor level's form and the two line widths, in device
    /// pixels. Returns whether the grid changed.
    pub fn set_grid_stroke(
        &mut self,
        minor_style: grid::MinorStyle,
        major_width: f64,
        minor_width: f64,
    ) -> bool {
        if !(major_width.is_finite() && minor_width.is_finite()) {
            return false;
        }
        let major_width = major_width.clamp(grid::MIN_STROKE_PIXELS, grid::MAX_STROKE_PIXELS);
        let minor_width = minor_width.clamp(grid::MIN_STROKE_PIXELS, grid::MAX_STROKE_PIXELS);
        if self.grid_minor_style == minor_style
            && (self.grid_major_width - major_width).abs() < 1e-9
            && (self.grid_minor_width - minor_width).abs() < 1e-9
        {
            return false;
        }
        self.grid_minor_style = minor_style;
        self.grid_major_width = major_width;
        self.grid_minor_width = minor_width;
        self.sync_grid();
        true
    }

    /// The grid's stroke: `(minor_style, major_width_px, minor_width_px)`.
    pub fn grid_stroke(&self) -> (grid::MinorStyle, f64, f64) {
        (
            self.grid_minor_style,
            self.grid_major_width,
            self.grid_minor_width,
        )
    }

    /// The host's step policy, as the grid, the Coin frame and the snap context read it.
    fn grid_policy(&self) -> GridPolicy {
        GridPolicy {
            minor: self.grid_minor,
            major: self.grid_major,
            adaptive: self.grid_adaptive,
            minor_style: self.grid_minor_style,
            major_width: self.grid_major_width,
            minor_width: self.grid_minor_width,
            alpha: self.grid_alpha,
        }
    }

    /// Fix the world grid's extent (mm), or return to the camera-derived radius with
    /// `None`. A non-positive radius is ignored.
    pub fn set_grid_radius(&mut self, radius: Option<f64>) -> bool {
        if radius.is_some_and(|radius| !(radius.is_finite() && radius > 0.0)) {
            return false;
        }
        if self.grid_radius == radius {
            return false;
        }
        self.grid_radius = radius;
        self.recompile();
        true
    }

    /// The fixed world-grid extent, or `None` when it follows the camera.
    pub fn grid_radius(&self) -> Option<f64> {
        self.grid_radius
    }

    /// Set the world grid's ink (linear RGBA). A non-finite or out-of-range component
    /// is ignored. Returns whether the grid changed.
    pub fn set_grid_color(&mut self, color: [f32; 4]) -> bool {
        if !color
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        {
            return false;
        }
        if self.grid_color == color {
            return false;
        }
        self.grid_color = color;
        self.recompile();
        true
    }

    /// The world grid's linear ink.
    pub fn grid_color(&self) -> [f32; 4] {
        self.grid_color
    }

    /// Set a multiplier over the grid tokens' alphas. Both the world level and a
    /// workplane's own grid scale by it, so the workplane keeps its `1:2`
    /// minor:major ratio. A non-finite or out-of-range value is ignored. Returns
    /// whether the grid changed.
    pub fn set_grid_alpha(&mut self, alpha: f32) -> bool {
        if !(alpha.is_finite() && (0.0..=1.0).contains(&alpha)) {
            return false;
        }
        if (self.grid_alpha - alpha).abs() < f32::EPSILON {
            return false;
        }
        self.grid_alpha = alpha;
        self.recompile();
        true
    }

    /// The multiplier over the grid tokens' alphas.
    pub fn grid_alpha(&self) -> f32 {
        self.grid_alpha
    }

    /// Whether the authored (shader) grid replaces the native ground
    /// (`ViewState.grid_overlay`).
    pub fn grid_overlay(&self) -> bool {
        self.grid_overlay
    }

    /// Select the authored grid: when `enabled`, the native ground (dots and lines) is
    /// hidden on every plane — the world ground and a bound workplane alike — while the
    /// triad, the workplane frame and the origin node are unaffected. Returns whether the
    /// flag changed.
    ///
    /// The flag does **not** gate the program packets. They are held in
    /// [`program_queue`](Self::program_queue) and drawn whenever non-empty — a
    /// registered overlay is visible without a second opt-in — and are never picked.
    pub fn set_grid_overlay(&mut self, enabled: bool) -> bool {
        if self.grid_overlay == enabled {
            return false;
        }
        self.grid_overlay = enabled;
        self.recompile();
        true
    }

    /// Rebuild the reference geometry for the current camera: the effective steps
    /// (screen-space culling), the radius, and the screen-facing triads.
    ///
    /// Three layers can contribute: the world layer, the active workplane and a hovered
    /// face (each when present). `App_ToggleGrid` owns the ground fields, the axis-cross
    /// flag owns the triads and the origin node, so they can be shown independently.
    /// There is **one grid**, moved: while a non-default plane is active the world ground
    /// is not drawn at all (the active layer replaces it), and only the world **triad**
    /// recedes to [`grid::WORLD_TRIAD_DIM`] so the global origin stays locatable. A hover
    /// preview (the default plane still active) keeps the ground and dims it instead.
    /// When the shader
    /// grid overlay is on, the **world ground** is hidden (two coplanar line grids
    /// z-fight); the triad, the workplane's own grid and the origin node stay. The
    /// packets go to their own queue ([`grid_queue`](Self::grid_queue)) so nothing here
    /// is ever picked.
    ///
    /// The native cross is `axis.enabled && !axis.overlay`: an authored (Coin)
    /// backend claims the axis with `view.axis.overlay`, which hides the native triad,
    /// origin node and labels without touching the grid or the user's switch.
    fn sync_grid(&mut self) {
        self.grid_queue = RenderQueue::default();
        self.refresh_hover_face();

        let grid_on = self.grid_enabled;
        // The Coin backend claims the axis with `view.axis.overlay`; the native triad,
        // origin node and labels step aside while the claim stands.
        let cross = self.axis.enabled && !self.axis.overlay;
        let active = self.active_workplane.clone();
        // The default world `XY` plane is drawn by the world layer; only a non-default
        // plane gets a layer of its own (and hides the coplanar world ground).
        let local = !active.is_default();
        // Do not preview the face that is already active, and do not bloom at all unless a
        // click could act on it: the preview is the Tier 1 auto-workplane hint, so it needs
        // an open creation tool **and** the default plane (once a tool binds a face, the
        // plane is no longer the default and the bloom stops). Idle hovering only reports
        // the face; it never adds a second grid.
        let hover = self
            .hover_face
            .as_ref()
            .filter(|_| grid_on && self.tool.is_some() && active.is_default())
            .filter(|face| face.since.elapsed() >= FACE_DWELL)
            .filter(|face| active.plane.kind != face.frame.plane.kind)
            .map(|face| face.frame.clone());

        let world_plane = Workplane::world();
        let policy = self.grid_policy();
        let world_radius = ground_radius(&self.camera, self.grid_radius);
        let (minor, major) = plane_steps(None, world_radius, policy);
        let (minor, major) =
            grid::effective_steps(&self.camera, &world_plane, minor, major);
        let world_spec = GridSpec {
            workplane: world_plane,
            minor,
            major,
            radius: world_radius,
        };
        // A local frame is present when its grid is on, or (for the triad alone) when the
        // axis cross is on with an active workplane.
        let local_present = (grid_on && hover.is_some()) || local;
        let mut world_style = with_stroke(
            level_style(None),
            &self.camera,
            &world_spec.workplane,
            policy,
        );
        world_style.color = self.grid_color;
        world_style.minor_alpha *= self.grid_alpha * grid::grazing(&self.camera, &world_spec.workplane);
        world_style.major_alpha *= self.grid_alpha * grid::grazing(&self.camera, &world_spec.workplane);
        // The world triad uses the same style as a local one, receding when a local frame
        // is present. The world **ground** is not drawn then (see `world` below): one
        // grid, at the active plane, is the whole point of moving the plane.
        let mut world_axis = self.axis.clone();
        if local_present {
            world_style.dim = grid::WORLD_DIM;
            world_axis = world_axis.scaled_opacity(grid::WORLD_TRIAD_DIM);
        }
        let active_layer = (local && (grid_on || cross)).then(|| {
            let (spec, style) = active_grid(
                &self.camera,
                &active.plane,
                policy,
                self.grid_color,
                self.grid_alpha,
                active.extent,
                world_radius,
            );
            (spec, with_grazing(style, &self.camera, &active.plane), grid_on)
        });
        let hover_layer = hover.as_ref().map(|frame| {
            let (spec, style) = local_grid(&self.camera, frame, policy);
            (
                spec,
                with_grazing(
                    with_grid_ink(style, self.grid_color, self.grid_alpha),
                    &self.camera,
                    &frame.plane,
                ),
            )
        });
        // The origin node sits on the active workplane (the world origin at the default).
        let origin_plane = active.plane.clone();
        let bloomed = self.origin_bloomed();
        let origin_bias = if local { WIRE_DEPTH_BIAS } else { 0.0 };

        let world = LayerSpec {
            spec: &world_spec,
            style: &world_style,
            axis: &world_axis,
            // One grid: a non-default active plane *is* the grid, so the world ground
            // steps aside entirely rather than stacking a second, dimmer grid under it.
            ground: grid_on && !self.grid_overlay && !local,
            triad: cross,
        };
        let active = active_layer
            .as_ref()
            .map(|(spec, style, ground)| LayerSpec {
                spec,
                style,
                axis: &self.axis,
                // The authored grid replaces the native ground on **every** plane, not
                // only the world one: a bound plane is still the grid. The triad and the
                // origin node stay, so the frame is still readable under the overlay.
                ground: *ground && !self.grid_overlay,
                triad: cross,
            });
        let hover = hover_layer.as_ref().map(|(spec, style)| LayerSpec {
            spec,
            style,
            axis: &self.axis,
            // The one exception: a hover bloom previews a plane that does not exist yet,
            // and a bound overlay cannot draw a frame the view is not on, so the preview
            // stays native even while the overlay owns the rest.
            ground: true,
            triad: cross,
        });

        let mesh = &mut self.grid;
        mesh.sync(&self.camera, world, active, hover);
        mesh.sync_origin(
            &origin_plane,
            &self.camera,
            bloomed,
            origin_bias,
            cross && self.axis.origin,
        );
        mesh.emit(&mut self.grid_queue);
    }

    /// The planar face under `hit`, resolved to a frame, when the hit is a planar part.
    fn face_frame_at(&self, hit: &Hit) -> Option<FaceFrame> {
        self.face_frame_for(hit.id.0, hit.part)
    }

    /// Item `entity`'s part `part`, resolved to a frame when that part is planar.
    fn face_frame_for(&self, entity: u32, part: u32) -> Option<FaceFrame> {
        let item = self
            .stage
            .items()
            .iter()
            .find(|item| item.entity_id() == entity)?;
        let name = item.part_name(part)?;
        FaceFrame::resolve(
            entity,
            name,
            item.positions(),
            item.triangles(),
            item.parts(),
            part,
        )
    }

    /// Resolve the face under the pointer into a [`FaceFrame`], keeping the dwell timer
    /// across frames unless the face (or its geometry) actually changed.
    fn refresh_hover_face(&mut self) {
        let key = self.hover.map(|id| (id.0, self.hovered_part.unwrap_or(0)));
        let item = key.and_then(|(entity, _)| {
            self.stage
                .items()
                .iter()
                .find(|item| item.entity_id() == entity)
        });
        let generation = item.map(|item| item.generation());
        let same = match key {
            Some(key) => self.hover_face.as_ref().is_some_and(|face| {
                (face.entity, face.part) == key && Some(face.generation) == generation
            }),
            None => self.hover_face.is_none(),
        };
        if same {
            return;
        }

        let since = match (&key, &self.hover_face) {
            (Some(key), Some(face)) if (face.entity, face.part) == *key => face.since,
            _ => Instant::now(),
        };
        self.hover_face = key.and_then(|(entity, part)| {
            let item = item?;
            let frame = self.face_frame_for(entity, part)?;
            Some(FaceHover {
                entity,
                part,
                generation: item.generation(),
                frame,
                since,
            })
        });
    }

    /// The active local workplane and its grid extent, if one is active.
    pub fn active_workplane(&self) -> &ActiveWorkplane {
        &self.active_workplane
    }

    /// Set the active plane directly (`World`, a `XZ`/`YZ` standard, or a custom frame).
    /// Returns whether the active plane changed.
    pub fn set_active_workplane(&mut self, plane: ActiveWorkplane) -> bool {
        if self.active_workplane == plane {
            return false;
        }
        self.active_workplane = plane;
        self.hover_face = None;
        self.recompile();
        true
    }

    /// Activate `frame` as the workplane, or return to the world `XY` plane with `None`.
    /// Returns whether the active plane changed.
    ///
    /// The plane is **sticky**: it survives pointer moves and empty-space clicks, and is
    /// only replaced by another activation or reset by `Esc`.
    pub fn set_workplane(&mut self, frame: Option<FaceFrame>) -> bool {
        match frame {
            Some(frame) => self.set_active_workplane(ActiveWorkplane::of(&frame)),
            None => self.set_active_workplane(ActiveWorkplane::world()),
        }
    }

    /// Activate the face under the pointer, if one resolved. Returns whether a workplane
    /// became active.
    pub fn activate_hovered_workplane(&mut self) -> bool {
        self.refresh_hover_face();
        let Some(frame) = self.hover_face.as_ref().map(|face| face.frame.clone()) else {
            return false;
        };
        self.set_workplane(Some(frame))
    }

    /// Activate one of the three world-standard axis planes (`XY`, `XZ`, `YZ`).
    /// Returns whether the active plane changed.
    pub fn set_standard_plane(&mut self, plane: ferrocad_stage::StandardPlane) -> bool {
        self.set_active_workplane(ActiveWorkplane::standard(plane))
    }

    /// Activate an **explicit frame**: `origin` plus the in-plane axes `u` and `v` (the
    /// normal is their cross product), labelled `Custom`. Returns whether the plane
    /// changed; a degenerate frame (a zero axis, or parallel axes) is refused and leaves
    /// the active plane alone.
    ///
    /// This is the seam behind the `"set"` workplane request and any picking tool that
    /// resolves three points: the caller owns the math, the viewport owns the state.
    pub fn set_workplane_frame(
        &mut self,
        origin: Vector3,
        u: Vector3,
        v: Vector3,
    ) -> bool {
        let Some(frame) = Workplane::new(origin, u, v, ferrocad_stage::WorkplaneKind::Custom)
        else {
            return false;
        };
        self.set_active_workplane(ActiveWorkplane::custom(frame, "Custom"))
    }

    /// Shift the **active plane** along its own normal by `delta` mm — the `Offset
    /// Workplane…` action. The plane keeps its name and orientation; the label gains the
    /// offset (`XY +25`) so the pill says where the grid actually is. Returns whether
    /// the plane changed.
    pub fn offset_workplane(&mut self, delta: f64) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return false;
        }
        let mut next = self.active_workplane.clone();
        next.plane.origin = next.plane.origin.add(&next.plane.n.scale(delta));
        next.offset += delta;
        self.set_active_workplane(next)
    }

    /// Adopt the camera's **view plane**: `u` is screen-right, `v` is screen-up, and the
    /// normal points at the viewer, through the point the camera looks at. Returns
    /// whether the active plane changed.
    ///
    /// This is the plane a 2D sketch drawn “on the screen” lives on, so it is only as
    /// stable as the camera; `Set WP` from a face is the durable way in.
    pub fn set_view_plane(&mut self) -> bool {
        let m = &self.camera.view.m;
        // The view matrix stores the screen basis in its rows (row-major, translation in
        // the fourth column): `right`, `up` and `toward the viewer`.
        let right = Vector3::new(m[0], m[1], m[2]);
        let up = Vector3::new(m[4], m[5], m[6]);
        let toward_viewer = Vector3::new(m[8], m[9], m[10]);
        let eye = camera_position(&self.camera.view);
        // The camera orbits a pivot on its own view axis; put the plane through it so a
        // panned or zoomed view still sketches where the user is looking.
        let origin = eye.sub(&toward_viewer.scale(self.camera.eye_distance()));
        let Some(frame) =
            ferrocad_stage::Workplane::new(origin, right, up, ferrocad_stage::WorkplaneKind::Custom)
        else {
            return false;
        };
        self.set_active_workplane(ActiveWorkplane::custom(frame, "View"))
    }

    /// Activate `entity`'s named element (`"Face6"`) as the workplane, when that element
    /// is a planar face. Returns whether a workplane became active.
    ///
    /// This is the explicit path (“Set from Selection”): the frame comes from the face's
    /// own centroid and normal, not from where the pointer happened to click.
    pub fn set_workplane_from_element(&mut self, entity: u32, element: Option<&str>) -> bool {
        let Some(element) = element else {
            return false;
        };
        let Some(part) = self
            .stage
            .items()
            .iter()
            .find(|item| item.entity_id() == entity)
            .and_then(|item| item.part_names().iter().position(|name| name == element))
        else {
            return false;
        };
        let Some(frame) = self.face_frame_for(entity, part as u32) else {
            return false;
        };
        self.set_workplane(Some(frame))
    }

    /// Reset the active workplane to the world `XY` plane. Returns whether it was
    /// anything else (resetting at the default is a no-op).
    pub fn clear_workplane(&mut self) -> bool {
        self.set_workplane(None)
    }

    /// Orient the camera down the active workplane's normal, with the plane's `V` axis
    /// up (the `View → Workplane` command, G5). The camera keeps its eye distance and
    /// animates there. Returns whether a workplane is active.
    pub fn view_workplane(&mut self) -> bool {
        // There is always an active plane: at the default this looks down world `-Z`,
        // which is the top view.
        let plane = &self.active_workplane.plane;
        let n = plane.n;
        // Roll-free: the screen up is the world vertical (`+Z`, the axis the navicube
        // canonicalises on) projected into the plane. Looking down a face whose normal is
        // nearly the world vertical has no such up, so fall back to the plane's own `V`.
        // A rolled camera could not be represented by the navicube.
        let world_up = Vector3::new(0.0, 0.0, 1.0);
        let mut up = world_up.sub(&n.scale(world_up.dot(&n)));
        if up.length() < 1e-6 {
            up = plane.v.sub(&n.scale(plane.v.dot(&n)));
        }
        if up.length() < 1e-6 {
            up = plane.u;
        }
        let up = up.normalize();
        let right = up.cross(&n).normalize();
        // The view basis: `right`, `up`, and `N` toward the eye (view-space +Z).
        let view = Matrix4 {
            m: [
                right.x, right.y, right.z, 0.0, up.x, up.y, up.z, 0.0, n.x, n.y, n.z, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ],
        };
        let target = Rotation::from_matrix(&view);
        self.snap = Some(SnapAnimation::to(
            target,
            &self.camera,
            Duration::from_millis(220),
        ));
        true
    }

    /// The triad's axis tags (`X`/`Y`/`Z` for the world, `U`/`V`/`N` for a workplane),
    /// projected at the stem tips. Empty while the axis cross (or its labels) is off, or
    /// while an authored backend owns the axis (`overlay`).
    fn axis_labels(&self) -> Vec<AxisLabel> {
        if !self.axis.enabled || self.axis.overlay || !self.axis.labels {
            return Vec::new();
        }
        let (plane, letters) = if self.active_workplane.is_default() {
            (Workplane::world(), ['X', 'Y', 'Z'])
        } else {
            (self.active_workplane.plane.clone(), ['U', 'V', 'N'])
        };
        let colors = self.axis.colors.map(packed_srgb);
        let alpha = (self.axis.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        // A face-bound plane shows the in-plane `U`/`V` bracket only, matching the stems
        // drawn by the grid layer; the `N` letter would label a stem that is not there.
        let in_plane = matches!(
            self.active_workplane.plane.kind,
            ferrocad_stage::WorkplaneKind::Face { .. }
        );
        let axes: &[Vector3] = if in_plane {
            &[plane.u, plane.v]
        } else {
            &[plane.u, plane.v, plane.n]
        };
        // A hair beyond the ribbon tip, so the glyph does not sit on the stem.
        let length = scale_for(&self.camera, plane.origin, self.axis.length * 1.08);
        axes.iter()
            .zip(letters)
            .zip(colors)
            .filter_map(|((axis, text), color)| {
                let tip = plane.origin.add(&axis.scale(length));
                let device = project_device(&self.camera, tip)?;
                Some(AxisLabel {
                    text,
                    position: self
                        .camera
                        .viewport
                        .to_logical([device[0].round() as i32, device[1].round() as i32]),
                    color,
                    alpha,
                })
            })
            .collect()
    }

    /// The snap plane: the active workplane when one is set, else the world grid.
    fn snap_context(&self) -> Option<SnapContext> {
        if self.active_workplane.is_default() {
            // The default plane keeps the world grid's steps (and honours the snap-grid
            // switch); a bound plane uses the step derived from its face.
            return self.snap_grid.map(SnapContext::world);
        }
        let (minor, major) = plane_steps(
            self.active_workplane.extent,
            ground_radius(&self.camera, self.grid_radius),
            self.grid_policy(),
        );
        Some(SnapContext {
            workplane: self.active_workplane.plane.clone(),
            // The plane's own step, not the world's: a snap must land on a line the plane
            // draws. The layer may cull a sub-pixel minor level for screen density, which
            // changes pixels, not spacing.
            minor,
            major,
        })
    }

    /// Whether a hovered face's grid is still waiting out [`FACE_DWELL`]. The renderer
    /// requests frames while this holds, so the bloom arrives without pointer motion.
    fn face_dwell_pending(&self) -> bool {
        // Only the tool-scoped bloom waits out a dwell, so idle hovering must not ask for
        // frames: nothing is going to appear.
        let bloom_pending = self.tool.is_some() && self.active_workplane.is_default();
        bloom_pending
            && self
                .hover_face
                .as_ref()
                .is_some_and(|face| face.since.elapsed() < FACE_DWELL)
    }

    /// The candidates the pointer may snap to: the hovered item's topological vertices
    /// and edge midpoints, its face centre, and the workplane origin. Ordered from most
    /// to least specific, which breaks ties in [`nearest_feature`].
    ///
    /// The item comes from the **current** hit rather than the stored hover: a tool is
    /// dispatched before the hover state is published, and must still snap.
    fn snap_candidates(&self, hit: Option<&Hit>) -> Vec<SnapCandidate> {
        let mut candidates = Vec::new();
        let item = hit.and_then(|hit| {
            self.stage
                .items()
                .iter()
                .find(|item| item.entity_id() == hit.id.0)
        });
        if let (Some(hit), Some(item)) = (hit, item) {
            let positions = item.positions();
            let at = |index: u32| {
                positions.get(index as usize).map(|position| {
                    Vector3::new(
                        f64::from(position[0]),
                        f64::from(position[1]),
                        f64::from(position[2]),
                    )
                })
            };
            for index in item.vertices() {
                if let Some(point) = at(*index) {
                    candidates.push(SnapCandidate {
                        point,
                        kind: SnapKind::Vertex,
                    });
                }
            }
            if item.edges().len() <= 2 * MAX_EDGE_CANDIDATES {
                for pair in item.edges().chunks_exact(2) {
                    if let (Some(a), Some(b)) = (at(pair[0]), at(pair[1])) {
                        candidates.push(SnapCandidate {
                            point: a.add(&b).scale(0.5),
                            kind: SnapKind::EdgeMid,
                        });
                    }
                }
            }
            if let Some(name) = item.part_name(hit.part) {
                if let Some(frame) = FaceFrame::resolve(
                    hit.id.0,
                    name,
                    item.positions(),
                    item.triangles(),
                    item.parts(),
                    hit.part,
                ) {
                    candidates.push(SnapCandidate {
                        point: frame.plane.origin,
                        kind: SnapKind::Centre,
                    });
                }
            }
        }
        let origin = self.active_workplane.plane.origin;
        candidates.push(SnapCandidate {
            point: origin,
            kind: SnapKind::Origin,
        });
        candidates
    }

    /// Record (or clear) the snap a tool is working from, restarting the aurora's flash
    /// only when the snapped point actually changed.
    fn set_snap_feedback(&mut self, snap: Option<ferrocad_stage::Snap>, plane: Option<Workplane>) {
        let same = match (&self.snap_feedback, &snap) {
            (Some(current), Some(next)) => {
                current.snap.point == next.point && current.snap.kind == next.kind
            }
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.snap_feedback = snap.map(|snap| SnapFeedback {
            snap,
            plane,
            since: Instant::now(),
        });
        self.rebuild_feedback();
    }

    /// Rebuild the feedback overlay: the aurora at the snap, and (for a grid snap) the
    /// dashed rays to the workplane's axes.
    fn rebuild_feedback(&mut self) {
        let Some(feedback) = self.snap_feedback.as_ref().map(|feedback| {
            (
                Vector3::new(
                    f64::from(feedback.snap.point[0]),
                    f64::from(feedback.snap.point[1]),
                    f64::from(feedback.snap.point[2]),
                ),
                feedback::aurora_alpha(feedback.since.elapsed()),
                feedback.snap.kind,
                feedback.snap.local,
                feedback.plane.clone(),
            )
        }) else {
            self.feedback_queue = RenderQueue::default();
            return;
        };
        let (point, alpha, kind, local, plane) = feedback;
        self.feedback_meshes.generation = Generation(self.feedback_meshes.generation.0 + 1);
        let generation = self.feedback_meshes.generation;

        let mut queue = RenderQueue::default();
        let (vertices, indices) = feedback::aurora(point, &self.camera, alpha);
        queue.packets.push(feedback_packet(
            self.feedback_meshes.aurora,
            generation,
            Topology::Triangles,
            vertices,
            indices,
        ));
        // The rays only make sense for a point that lies **on** the plane, which a grid
        // snap does by construction.
        if kind == SnapKind::Grid {
            if let (Some(plane), Some(local)) = (plane.as_ref(), local) {
                let vertices = feedback::rays(plane, local, &self.camera);
                let indices = (0..vertices.len() as u32).collect();
                queue.packets.push(feedback_packet(
                    self.feedback_meshes.rays,
                    generation,
                    Topology::Lines,
                    vertices,
                    indices,
                ));
            }
        }
        self.feedback_queue = queue;
    }

    /// Whether the aurora is still in its acquisition flash (the renderer keeps asking
    /// for frames until it settles).
    fn feedback_flash_pending(&self) -> bool {
        self.snap_feedback
            .as_ref()
            .is_some_and(|feedback| feedback.since.elapsed() < feedback::FLASH)
    }

    /// The local `(u, v)` of the tool's current snap, for the host and the dimension
    /// pill. `None` when no tool is snapping or no workplane is known.
    pub fn snap_local(&self) -> Option<[f64; 2]> {
        self.snap_feedback
            .as_ref()
            .and_then(|feedback| feedback.snap.local)
    }

    /// What the tool's pointer snapped to, if anything.
    pub fn snap_kind(&self) -> Option<SnapKind> {
        self.snap_feedback
            .as_ref()
            .map(|feedback| feedback.snap.kind)
    }

    /// The logical anchor of the snap's dimension pill: the snapped point, projected.
    fn snap_pill_anchor(&self) -> Option<[f32; 2]> {
        let feedback = self.snap_feedback.as_ref()?;
        let point = Vector3::new(
            f64::from(feedback.snap.point[0]),
            f64::from(feedback.snap.point[1]),
            f64::from(feedback.snap.point[2]),
        );
        let device = project_device(&self.camera, point)?;
        Some(
            self.camera
                .viewport
                .to_logical([device[0].round() as i32, device[1].round() as i32]),
        )
    }

    /// Whether the pointer is within the origin node's proximity radius (G4).
    fn origin_bloomed(&self) -> bool {
        let Some(device) = self.pointer else {
            return false;
        };
        let origin = self.active_workplane.plane.origin;
        project_device(&self.camera, origin).is_some_and(|projected| {
            (projected[0] - f64::from(device[0])).hypot(projected[1] - f64::from(device[1]))
                <= feedback::ORIGIN_PROXIMITY_PIXELS
        })
    }

    /// Step a running snap animation, applying the interpolated camera. Returns
    /// whether another frame is needed.
    fn step_snap(&mut self) -> bool {
        let Some(snap) = self.snap.as_ref() else {
            return false;
        };
        let view = snap.view_at(snap.elapsed());
        let done = snap.is_done();
        self.camera.view = view;
        self.recompile();
        if done {
            self.snap = None;
            false
        } else {
            true
        }
    }

    /// Draw the scene (and the navigation cube, when enabled) and read it back,
    /// refreshing the cached frame.
    fn refresh(&mut self) {
        if self.stage_dirty {
            self.recompile();
        }
        // The grid and axis cross follow the camera and the hovered face, none of which
        // marks the stage dirty. `sync_grid` is a key comparison when nothing changed.
        self.sync_grid();
        // The aurora's flash decays over 120 ms and the camera sets its size, so the
        // feedback is rebuilt per paint (a handful of vertices).
        self.rebuild_feedback();
        if let Some(cube) = self.navicube.as_mut() {
            cube.sync(&self.camera);
        }
        // A directive-driven overlay with no explicit orientation mirrors the main
        // camera; one with an orientation is fixed (set once from the directive).
        for (directive, cube) in &mut self.directive_cubes {
            if directive.orientation.is_none() {
                cube.sync(&self.camera);
            }
        }

        // The gizmos' screen-invariant scale follows the camera; rebuild when it changes.
        let mut gizmo_rebuilt = false;
        for gizmo in self.gizmos.gizmos_mut() {
            gizmo_rebuilt |= gizmo.update(&self.camera);
        }
        if gizmo_rebuilt {
            self.rebuild_gizmo_queue();
        }
        // A manipulator's overlay is camera-facing, so it rebuilds with the view too.
        let mut manipulators_rebuilt = false;
        for manipulator in &mut self.manipulators {
            manipulators_rebuilt |= manipulator.refresh(&self.camera);
        }
        if manipulators_rebuilt {
            self.rebuild_manipulator_queue();
        }

        // The program (Coin) grid is the only draw that reads the construction plane
        // from the `Frame` uniform; the scene and the native grid already draw in world
        // space. The extent is the native grid's (a face patch, the user's fixed
        // "Grid extent", or the camera-following ground), never `0`.
        let workplane = self.active_workplane.view_workplane(
            &self.camera,
            self.grid_policy(),
            self.grid_radius,
        );

        // One pass: the scene (legacy + stage) first, then the overlays (the tool's
        // transient visuals, the gizmo and the navigation cubes, depth off and on top).
        let mut views = Vec::with_capacity(7);
        views.push(ViewSpec {
            queue: &self.queue,
            camera: &self.camera,
            rect: None,
            overlay: false,
            workplane: None,
        });
        if !self.grid_queue.is_empty() {
            // A second non-overlay view: the same depth pass, so the grid occludes and is
            // occluded by the scene, but it is never part of a pick.
            views.push(ViewSpec {
                queue: &self.grid_queue,
                camera: &self.camera,
                rect: None,
                overlay: false,
                workplane: None,
            });
        }
        if !self.program_queue.is_empty() {
            // Registered program overlays (G5): drawn after opaque geometry and the native
            // grid, in the same depth-tested pass. Their pipeline writes no depth by
            // default, so an opaque box in front of the plane still occludes them.
            // `grid_overlay` only hides the native ground; it never hides an overlay.
            views.push(ViewSpec {
                queue: &self.program_queue,
                camera: &self.camera,
                rect: None,
                overlay: false,
                workplane: Some(workplane),
            });
        }
        if !self.transient.is_empty() {
            views.push(ViewSpec {
                queue: &self.transient,
                camera: &self.camera,
                rect: None,
                overlay: true,
                workplane: None,
            });
        }
        if !self.feedback_queue.is_empty() {
            // The aurora and its rays are pointer feedback: depth-free, on top.
            views.push(ViewSpec {
                queue: &self.feedback_queue,
                camera: &self.camera,
                rect: None,
                overlay: true,
                workplane: None,
            });
        }
        if !self.gizmo_queue.is_empty() {
            views.push(ViewSpec {
                queue: &self.gizmo_queue,
                camera: &self.camera,
                rect: None,
                overlay: true,
                workplane: None,
            });
        }
        if !self.manipulator_queue.is_empty() {
            views.push(ViewSpec {
                queue: &self.manipulator_queue,
                camera: &self.camera,
                rect: None,
                overlay: true,
                workplane: None,
            });
        }
        if let Some(cube) = self.navicube.as_ref() {
            views.push(cube.view());
        }
        for (_, cube) in &self.directive_cubes {
            views.push(cube.view());
        }

        // A ready presenter draws into its own GPU target. Until it is ready (and
        // whenever it is unavailable) keep the image path: those frames double as the
        // fallback while the host negotiates with the window's device.
        let presented = match &self.presenter {
            Some(presenter) => {
                let mut presenter = presenter.borrow_mut();
                presenter.status() == PresenterStatus::Ready && presenter.draw(&views)
            }
            None => false,
        };
        if !presented && self.view.draw_frame(&views).is_ok() {
            if let Ok(image) = self.view.image() {
                self.frame = Some(Arc::new(render_image(image)));
            }
        }
    }
}

impl<V: InventorView + 'static> EventEmitter<ViewportEvent> for InventorViewport<V> {}

impl<V: InventorView + 'static> Render for InventorViewport<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Follow the element's physical size before drawing (the bounds were captured at
        // the previous prepaint). A change needs one more frame to take effect.
        if self.sync_size(window) {
            window.request_animation_frame();
        }
        // MVP: render and read back during paint, on the UI thread. An inertial orbit or
        // a snap animation keeps requesting frames until it settles.
        if self.step_inertia() {
            window.request_animation_frame();
        }
        if self.step_snap() {
            window.request_animation_frame();
        }
        // A hovered face's grid blooms after a dwell; keep painting until it does.
        if self.face_dwell_pending() {
            window.request_animation_frame();
        }
        // The snap aurora's flash decays over 120 ms.
        if self.feedback_flash_pending() {
            window.request_animation_frame();
        }
        // The presenter negotiates with the window's device on its first paint; ask for
        // the frame that can then draw and present into it. Bound the requests so a
        // canvas that never paints cannot spin the window forever.
        const PRESENTER_STARTUP_FRAMES: u8 = 8;
        let presenter_status = self.presenter_status();
        if presenter_status == Some(PresenterStatus::Pending) {
            if self.presenter_frames < PRESENTER_STARTUP_FRAMES {
                self.presenter_frames += 1;
                window.request_animation_frame();
            }
        }
        self.refresh();

        // A tool installed directly (outside a pointer or key handler) reports its
        // start here; the pointer/key handlers drain the same queue themselves.
        if let Some(status) = self.take_tool_state() {
            cx.emit(ViewportEvent::ToolState(status));
        }

        // Face and compass labels are gpui text over the overlay image (crisp, in the
        // app's font). They follow the same view rotation and rect as the 3D overlay.
        let labels = self
            .navicube
            .as_ref()
            .map(NaviCube::labels)
            .unwrap_or_default();

        let content: AnyElement = match (&self.presenter, presenter_status) {
            // Pending presents an empty canvas while `prepare` runs; Unsupported (and a
            // presenter that never got a paint) falls through to the image path.
            (Some(presenter), Some(status))
                if status != PresenterStatus::Unsupported
                    && !(status == PresenterStatus::Pending
                        && self.presenter_frames >= PRESENTER_STARTUP_FRAMES) =>
            {
                let presenter = presenter.clone();
                gpu_canvas(move |gpu| {
                    let mut presenter = presenter.borrow_mut();
                    presenter.prepare(gpu);
                    if presenter.status() == PresenterStatus::Ready {
                        presenter.present(gpu);
                    }
                })
                .w_full()
                .h_full()
                .into_any_element()
            }
            _ => match &self.frame {
                Some(image) => img(image.clone())
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Fill)
                    .into_any_element(),
                None => div().w_full().h_full().into_any_element(),
            },
        };

        let entity = cx.entity();
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let root = div()
            .relative()
            .w_full()
            .h_full()
            // Record the element's window-space bounds each frame, so the window-relative
            // positions gpui reports can be made viewport-local (see `to_local`).
            .on_children_prepainted(move |bounds: Vec<Bounds<Pixels>>, _window, cx| {
                // The canvas fills the element; the chrome bar is a smaller absolutely
                // positioned sibling, so take the largest child as the element's bounds.
                let largest = bounds
                    .iter()
                    .max_by(|a, b| {
                        let area = |bounds: &Bounds<Pixels>| {
                            bounds.size.width.0 * bounds.size.height.0
                        };
                        area(a).total_cmp(&area(b))
                    })
                    .copied();
                if let Some(bounds) = largest {
                    entity.update(cx, |this, _cx| this.last_bounds = Some(bounds));
                }
            })
            .id("ferrocad-inventor-viewport")
            .track_focus(&focus)
            .focusable()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let changed = this.handle_key(event);
                if let Some(status) = this.take_tool_state() {
                    cx.emit(ViewportEvent::ToolState(status));
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                // Scrolling over the chrome bar must not zoom the scene.
                if this.chrome_contains(this.to_local(event.position)) {
                    return;
                }
                // A wheel notch is a line; a trackpad reports pixels.
                let lines = match event.delta {
                    ScrollDelta::Lines(lines) => lines.y,
                    ScrollDelta::Pixels(pixels) => pixels.y.as_f32() / 40.0,
                };
                if this.handle_scroll(lines) {
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let local = this.to_local(event.position);
                let modifiers = gizmo_modifiers(event.modifiers);
                let mut changed = false;
                // gpui's drag system carries a left-button drag (and its moves outside
                // the element) through `on_drag_move`; a middle-button orbit has no
                // such gesture, so it is read from the pressed state here.
                if event.pressed_button == Some(MouseButton::Middle) {
                    changed |= this.drag_move_with_modifiers(local, modifiers);
                }
                changed |= this.handle_pointer_with_modifiers(local, modifiers);
                this.flush_transform_preview(cx);
                if let Some(status) = this.take_tool_state() {
                    cx.emit(ViewportEvent::ToolState(status));
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if let Some(focus) = &this.focus {
                    window.focus(focus, cx);
                }
                let local = this.to_local(event.position);
                let modifiers = gizmo_modifiers(event.modifiers);
                let changed = this.handle_button_with_modifiers_and_click_count(
                    local,
                    mouse_button(event.button),
                    true,
                    modifiers,
                    event.click_count,
                );
                this.flush_transform_preview(cx);
                if let Some(status) = this.take_tool_state() {
                    cx.emit(ViewportEvent::ToolState(status));
                }
                if changed {
                    cx.notify();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                let local = this.to_local(event.position);
                let modifiers = gizmo_modifiers(event.modifiers);
                let changed = this.handle_button_with_modifiers(
                    local,
                    mouse_button(event.button),
                    false,
                    modifiers,
                );
                // Report what the click produced. `emit` queues the effect until this
                // update returns, so the host may update this viewport in response.
                let selection = this.take_selection();
                let commit = this.take_interaction_commit();
                let transform = this.take_gizmo_commit();
                let origin = this.take_gizmo_origin();
                let tool_state = this.take_tool_state();
                this.flush_transform_preview(cx);
                if changed {
                    cx.notify();
                }
                if let Some(status) = tool_state {
                    cx.emit(ViewportEvent::ToolState(status));
                }
                if let Some(selection) = selection {
                    cx.emit(ViewportEvent::Selection(selection));
                }
                if let Some(commit) = commit {
                    cx.emit(ViewportEvent::Interaction(commit));
                }
                if let Some(transform) = transform {
                    cx.emit(ViewportEvent::Transform(transform));
                }
                if let Some((point, kind)) = origin {
                    cx.emit(ViewportEvent::GizmoOrigin { point, kind });
                }
            }))
            .on_drag(ViewportDragMarker, |_, _, _, cx| {
                cx.new(|_| ViewportDragPreview)
            })
            .on_drag_move::<ViewportDragMarker>(cx.listener(
                |this, event: &DragMoveEvent<ViewportDragMarker>, _window, cx| {
                    let position = this.to_local(event.event.position);
                    let changed = this
                        .drag_move_with_modifiers(position, gizmo_modifiers(event.event.modifiers));
                    this.flush_transform_preview(cx);
                    if changed {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_exit(cx.listener(|this, _event: &MouseExitEvent, _window, cx| {
                if this.handle_pointer_leave() {
                    cx.notify();
                }
            }))
            .child(content);

        // Labels sit between the image and the chrome: above the 3D overlay, below the
        // controls. A plain text layer has no hitboxes, so pointer events still reach the
        // viewport handlers.
        let root = if labels.is_empty() {
            root
        } else {
            root.child(navicube_labels(&labels))
        };

        // The triad's `X`/`Y`/`Z` (or `U`/`V`/`N`) tags, at the projected stem tips.
        let axis = self.axis_labels();
        let root = if axis.is_empty() {
            root
        } else {
            root.child(axis_labels(&axis))
        };

        // The gizmo's numeric tag follows the active drag: anchored at the stem tip or
        // plane, or at the pointer for a ring. It is a text layer with no hitboxes.
        // `gizmo_readout` honours the style's `enabled`/`show_readout` policy.
        let root = match self.gizmo_readout() {
            Some((text, anchor)) => root.child(readout_pill(text, anchor)),
            None => root,
        };

        // The snap's dimension pill: `ΔU`/`ΔV` in the active workplane's coordinates
        // (G4). It follows the snapped point, not the raw cursor, so it does not jitter
        // while the pointer moves inside one grid cell.
        let root = match self
            .snap_feedback
            .as_ref()
            .and_then(|feedback| Some((feedback.snap.local?, self.snap_pill_anchor()?)))
        {
            Some((local, anchor)) => root.child(snap_pill(local, anchor)),
            None => root,
        };

        // A manipulator's dimension pill (the edge arc's `R 3.50 mm`): the manipulator
        // supplies both the text and where it sits, so the viewport names none of them.
        // `manipulator_readout` honours the style's readout policy.
        let root = match self.manipulator_readout() {
            Some((text, anchor)) => root.child(readout_pill(text, anchor)),
            None => root,
        };

        // The chrome bar is gpui, beside the 3D overlay. It is positioned from the
        // cube's freshly synced rect, so it tracks the same device-pixel rounding as the
        // overlay. Build it last so its listeners' use of `cx` does not overlap the
        // handlers above.
        if let Some(cube) = self.navicube.as_ref() {
            let rect = chrome_bar_rect(cube);
            let orthographic = self.camera.is_orthographic();
            let angles = camera_angles(&self.camera);
            root.child(navicube_chrome(rect, orthographic, angles, cx))
        } else {
            root
        }
    }
}

/// Compile the inventor scene — its draw packets and its semantic directives — and
/// append the stage's batches, so the two producers share one queue (and thus one
/// render pass and one depth buffer).
fn merge_queue(
    scene: &Scene,
    stage: &Stage,
    viewport: &Viewport,
) -> (RenderQueue, Directives, Option<String>) {
    let (mut queue, directives, error) = compile_diagnostic(scene, viewport, None);
    stage.emit_batches(&mut queue);
    (queue, directives, error.map(|error| error.to_string()))
}

/// Split the program packets out of the scene queue.
///
/// A packet whose state carries a shader program is kept out of every pick and drawn
/// after the opaque scene. A **geometry** program (`ShaderProgram::with_geometry`) draws
/// the packet's own mesh with the authored shader; a plain program draws a full-screen
/// placeholder instead of its own geometry. Either way the packet belongs in the
/// program queue, never in the scene queue.
fn split_programs(queue: RenderQueue) -> (RenderQueue, RenderQueue) {
    let mut scene = RenderQueue::default();
    let mut programs = RenderQueue::default();
    for packet in queue.packets {
        if packet.state.program.is_some() {
            programs.packets.push(packet);
        } else {
            scene.packets.push(packet);
        }
    }
    (scene, programs)
}

/// Write the scene's shadow block into every program packet's parameters.
///
/// The fixed ABI reserves `counts.x` and the 80-`vec4` block for every program, so a
/// producer can populate it without knowing which shader declared it; a program whose
/// `shadow_opacity` is `0` (or which ignores the block) is unaffected. The program is
/// rebuilt only when its packed parameters actually change, so a camera move does not
/// churn the pipeline cache.
fn apply_program_shadows(queue: &mut RenderQueue, footprints: &[ShadowFootprint]) {
    for packet in &mut queue.packets {
        let Some(program) = packet.state.program.as_mut() else {
            continue;
        };
        let mut parameters = *program.parameters();
        apply_shadows(&mut parameters, footprints.iter().copied());
        if parameters != *program.parameters() {
            *program = Arc::new(program.as_ref().clone().with_parameters(parameters));
        }
    }
}

/// The gpui chrome bar under the navigation cube: a projection toggle, Home and the
/// angle readout. Logical pixels; the palette matches the shell's panels.
const CHROME_HEIGHT: f32 = 26.0;
/// The gap between the cube's overlay rect and the pill, toward the viewport centre.
/// Tucked to zero so the pill and the cube read as one column.
const CHROME_GAP: f32 = 0.0;
/// The chrome pill's glass tint: the shell `BG` at 60%.
const CHROME_BG: u32 = 0x0f121899;
const CHROME_FONT: f32 = 11.0;

/// The chrome bar's logical rect `[left, top, right, bottom]` under `cube`.
///
/// The bar is **centred on the cube's painted rect** (WA6): the 248px pill shares the
/// cube's horizontal centre and sits `CHROME_GAP` beyond it, toward the viewport centre,
/// so the cube and its controls read as one column rather than a left-heavy card.
fn chrome_bar_rect(cube: &NaviCube) -> [f32; 4] {
    let [left, top, right, bottom] = cube.logical_rect();
    let x = (left + right) / 2.0 - CHROME_WIDTH / 2.0;
    let y = match cube.config().corner {
        Corner::TopLeft | Corner::TopRight => bottom + CHROME_GAP,
        Corner::BottomLeft | Corner::BottomRight => top - CHROME_GAP - CHROME_HEIGHT,
    };
    [x, y, x + CHROME_WIDTH, y + CHROME_HEIGHT]
}

/// The camera's **azimuth/elevation** in degrees, from the eye's bearing as seen from
/// the pivot (the world origin). The world is **Z-up**, so azimuth is measured in the
/// ground `XY` plane from `+X` (toward `+Y`) and elevation is above that plane. This is the
/// viewpoint angle, not the look direction. A degenerate (identity) view reports
/// `(0, 0)`. At a pole the eye has no bearing, so the screen-right direction supplies one.
pub fn camera_angles(camera: &Camera) -> (f64, f64) {
    let Some(inverse) = camera.view.inverse() else {
        return (0.0, 0.0);
    };
    let eye = inverse.transform(&Vector3::zero());
    let distance = eye.length();
    if distance <= 1e-9 {
        return (0.0, 0.0);
    }
    let axis = eye.scale(1.0 / distance);
    let elevation = axis.z.clamp(-1.0, 1.0).asin().to_degrees();
    // Azimuth is the eye's horizontal bearing. At a pole the eye has no bearing, so use
    // the screen-right direction instead: a top view then keeps a stable heading that
    // the yaw controls still turn, rather than freezing at 000°.
    let horizontal = (axis.x * axis.x + axis.y * axis.y).sqrt();
    let bearing = if horizontal > 1e-9 {
        axis
    } else {
        inverse.transform(&Vector3::new(1.0, 0.0, 0.0))
    };
    let azimuth = bearing.y.atan2(bearing.x).to_degrees().rem_euclid(360.0);
    // A hair below a full turn rounds to 360°; report it as 0°.
    let azimuth = if azimuth >= 360.0 - 1e-9 { 0.0 } else { azimuth };
    (azimuth, elevation)
}

/// A projection segment in the chrome bar. `orthographic` is the projection it selects;
/// the active segment is accented.
fn projection_button<V: InventorView + 'static>(
    id: &'static str,
    label: &'static str,
    active: bool,
    orthographic: bool,
    cx: &mut Context<InventorViewport<V>>,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(5.))
        .rounded_sm()
        .text_size(px(CHROME_FONT))
        .text_color(rgb(if active { CHROME_ACTIVE } else { MUTED }))
        .when(active, |el| el.bg(rgba(CHROME_ACTIVE_BG)))
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(FG)))
        .child(label)
        .on_click(cx.listener(move |this, _ev, _win, cx| {
            if this.set_orthographic(orthographic) {
                cx.notify();
            }
        }))
}

/// The face and compass labels as gpui text over the overlay image: one centred, fixed
/// box per label. The layer has no hitboxes, so it never steals pointer events.
fn navicube_labels(labels: &[NavicubeLabel]) -> impl IntoElement {
    let mut layer = div().absolute().left(px(0.)).top(px(0.)).w_full().h_full();
    for label in labels {
        let size = label.size;
        let box_size = size * 2.0;
        let alpha = (label.alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        layer = layer.child(
            div()
                .absolute()
                .left(px(label.position[0] - box_size / 2.0))
                .top(px(label.position[1] - box_size / 2.0))
                .w(px(box_size))
                .h(px(box_size))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(size))
                .line_height(px(size * 1.15))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgba(0xd8dbe000 | alpha))
                .child(label.text.to_string()),
        );
    }
    layer
}

/// A compact chrome control that yaws the view a quarter turn.
fn yaw_button<V: InventorView + 'static>(
    id: &'static str,
    label: &'static str,
    direction: YawDirection,
    cx: &mut Context<InventorViewport<V>>,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(4.))
        .rounded_sm()
        .text_size(px(CHROME_FONT))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(FG)))
        .child(label)
        .on_click(cx.listener(move |this, _ev, _win, cx| {
            if this.yaw(direction) {
                cx.notify();
            }
        }))
}

/// The gpui chrome bar: `[PERSP | ORTHO] · Home · 063° · 24°` in one compact row.
/// `rect` is its element-local logical rect as computed by [`chrome_bar_rect`], so
/// painting and pointer reservation cannot disagree.
fn navicube_chrome<V: InventorView + 'static>(
    rect: [f32; 4],
    orthographic: bool,
    angles: (f64, f64),
    cx: &mut Context<InventorViewport<V>>,
) -> impl IntoElement {
    let yaws = div()
        .flex()
        .flex_row()
        .items_center()
        .child(yaw_button(
            "ferrocad-yaw-ccw",
            "◀",
            YawDirection::CounterClockwise,
            cx,
        ))
        .child(yaw_button(
            "ferrocad-yaw-cw",
            "▶",
            YawDirection::Clockwise,
            cx,
        ));

    let projections = div()
        .flex()
        .flex_row()
        .items_center()
        .child(projection_button(
            "ferrocad-projection-persp",
            "PERSP",
            !orthographic,
            false,
            cx,
        ))
        .child(projection_button(
            "ferrocad-projection-ortho",
            "ORTHO",
            orthographic,
            true,
            cx,
        ));

    let divider = div().w(px(1.)).h(px(12.)).bg(rgba(CHROME_LINE));

    let home = div()
        .id("ferrocad-navicube-home")
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(5.))
        .rounded_sm()
        .text_size(px(CHROME_FONT))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(FG)))
        .child("Home")
        .on_click(cx.listener(|this, _ev, _win, cx| {
            if this.go_home() {
                cx.notify();
            }
        }));

    let readout = div()
        .flex_1()
        .flex()
        .justify_end()
        .items_center()
        .text_size(px(CHROME_FONT))
        .text_color(rgb(MUTED))
        .child(format!("{:03.0}° · {:02.0}°", angles.0, angles.1));

    div()
        .absolute()
        .left(px(rect[0]))
        .top(px(rect[1]))
        .w(px(CHROME_WIDTH))
        .h(px(CHROME_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(5.))
        .rounded_sm()
        .bg(rgba(CHROME_BG))
        .border_1()
        .border_color(rgba(CHROME_LINE))
        .occlude()
        .child(yaws)
        .child(projections)
        .child(divider)
        .child(home)
        .child(readout)
}

/// The navigation cube's minimum drag distance, in **logical** pixels — upstream's
/// `max(3, round(3·dpr))` device pixels converted back to logical; a shorter movement
/// is a click (a snap on release), not an orbit.
fn navicube_drag_threshold(dpr: f32) -> f32 {
    (3.0 * dpr).round().max(3.0) / dpr
}

/// Wrap an angle difference into `[−π, π)`.
fn shortest_angle(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

/// Map a gpui mouse button to Coin's 1-based button number.
fn mouse_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        _ => 0,
    }
}

/// Map Coin's 1-based button number to a stage mouse button.
fn stage_button(button: u8) -> Option<StageButton> {
    match button {
        1 => Some(StageButton::Left),
        2 => Some(StageButton::Middle),
        3 => Some(StageButton::Right),
        _ => None,
    }
}

/// Map gpui's modifiers to the gizmo's solver flags. `Ctrl` (or the platform's
/// secondary modifier on macOS) frees the rotation detents; `Shift` turns an axis drag
/// into a plane drag.
fn gizmo_modifiers(modifiers: gpui::Modifiers) -> GizmoModifiers {
    GizmoModifiers {
        shift: modifiers.shift,
        ctrl: modifiers.secondary(),
    }
}

/// The gizmo's or scrubber's projected numeric tag. A plain text layer: no id or
/// hitbox, so it never steals pointer events from the viewport or the chrome.
/// The triad's axis tags: 10 px monospace at the stem tips, in the axis hues.
fn axis_labels(labels: &[AxisLabel]) -> impl IntoElement {
    let mut layer = div().absolute().left(px(0.)).top(px(0.)).w_full().h_full();
    for label in labels {
        let alpha = u32::from(label.alpha);
        layer = layer.child(
            div()
                .absolute()
                .left(px(label.position[0] - 5.0))
                .top(px(label.position[1] - 6.0))
                .w(px(10.0))
                .h(px(12.0))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .line_height(px(11.5))
                .font_family("monospace")
                .text_color(rgba(label.color << 8 | alpha))
                .child(label.text.to_string()),
        );
    }
    layer
}

/// The human name of a snap kind, for the pivot readout.
fn snap_name(kind: SnapKind) -> &'static str {
    match kind {
        SnapKind::Point => "surface",
        SnapKind::Grid => "grid",
        SnapKind::Vertex => "vertex",
        SnapKind::EdgeMid => "edge mid",
        SnapKind::Centre => "face centre",
        SnapKind::Origin => "origin",
    }
}

/// A snap's dimension pill: `ΔU` over `ΔV`, in the readout tokens.
fn snap_pill(local: [f64; 2], anchor: [f32; 2]) -> impl IntoElement {
    div()
        .absolute()
        .left(px(anchor[0] + 12.0))
        .top(px(anchor[1] - 40.0))
        .flex()
        .flex_col()
        .px(px(6.))
        .py(px(2.))
        .rounded_sm()
        .bg(rgba(0x0f1218cc))
        .border_1()
        .border_color(rgba(0xffffff14))
        .text_size(px(11.))
        .font_family("monospace")
        .text_color(rgb(CHROME_ACTIVE))
        .child(format!("ΔU: {:.2} mm", local[0]))
        .child(format!("ΔV: {:.2} mm", local[1]))
}

fn readout_pill(text: String, anchor: [f32; 2]) -> impl IntoElement {
    div()
        .absolute()
        .left(px(anchor[0] + 12.0))
        .top(px(anchor[1] - 28.0))
        .px(px(6.))
        .py(px(2.))
        .rounded_sm()
        .bg(rgba(0x0f1218cc))
        .border_1()
        .border_color(rgba(0xffffff14))
        .text_size(px(11.))
        .font_family("monospace")
        .text_color(rgb(CHROME_ACTIVE))
        .child(text)
}

/// Wrap readback pixels as a gpui [`RenderImage`].
///
/// gpui's `RenderImage` holds **BGRA** (see its docs), while [`ImageData`] is RGBA,
/// so the two colour channels are swapped here. The image is not flipped: the wgpu
/// readback is already top-left origin, matching gpui.
pub fn render_image(image: ImageData) -> RenderImage {
    let mut bgra = image.rgba;
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2); // R <-> B
    }
    let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(image.width, image.height, bgra)
            .expect("ImageData length must equal width * height * 4");
    RenderImage::new([Frame::new(buffer)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, ShadowFootprint, Topology};
    use ferrocad_inventor::{Node, NodeKind, compile_with_base};
    use ferrocad_stage::{DrawMode, ShapeItem};
    use ferrocad_types::{Placement, Rotation, Vector3};
    use gpui::{Bounds, point, px, size};

    /// A view that returns fixed pixels, so the gpui side needs no GPU.
    struct FakeView {
        pixels: ImageData,
        resize_calls: u32,
        last_resize: Option<[u32; 2]>,
        image_calls: u32,
    }

    impl InventorView for FakeView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [self.pixels.width, self.pixels.height]
        }
        fn resize(&mut self, size: [u32; 2]) {
            self.resize_calls += 1;
            self.last_resize = Some(size);
        }
        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            self.image_calls += 1;
            Ok(self.pixels.clone())
        }
    }

    fn fake(pixels: ImageData) -> FakeView {
        FakeView {
            pixels,
            resize_calls: 0,
            last_resize: None,
            image_calls: 0,
        }
    }

    /// A view that records the queue lengths of every `draw_frame`, so a test can
    /// see exactly which queues the viewport chose to draw.
    struct RecordingView {
        frames: Rc<RefCell<Vec<Vec<usize>>>>,
    }

    impl InventorView for RecordingView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [100, 100]
        }
        fn draw(
            &mut self,
            _queue: &RenderQueue,
            _camera: &Camera,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Ok(ImageData::default())
        }
        fn draw_frame(&mut self, views: &[ViewSpec<'_>]) -> Result<(), Self::Error> {
            self.frames
                .borrow_mut()
                .push(views.iter().map(|view| view.queue.len()).collect());
            Ok(())
        }
    }

    #[test]
    fn render_image_keeps_size_and_swizzles_to_bgra() {
        let rendered = render_image(ImageData {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 255],
        });
        assert_eq!(rendered.size(0).width.0, 1);
        assert_eq!(rendered.as_bytes(0).unwrap(), &[30, 20, 10, 255]);
    }

    /// A triangle at the origin of an identity camera (device == logical at 1x).
    fn triangle_scene() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    Node {
                        kind: NodeKind::Material(vec![[1.0, 1.0, 1.0, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(vec![
                            [-0.5, -0.5, 0.5],
                            [0.5, -0.5, 0.5],
                            [0.0, 0.5, 0.5],
                        ]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Mesh {
                            indices: vec![0, 1, 2],
                            normal_indices: vec![],
                            topology: Topology::Triangles,
                        },
                        children: vec![],
                    },
                ],
            },
        }
    }

    /// A listener that records whether the pointer was over an object.
    #[derive(Default)]
    struct HoverProbe {
        hovered: bool,
    }

    impl EventListener for HoverProbe {
        fn on_event(
            &mut self,
            _event: &Event,
            hit: Option<&Hit>,
            _scene: &mut Scene,
            _directives: &Directives,
        ) -> bool {
            let next = hit.is_some();
            let changed = next != self.hovered;
            self.hovered = next;
            changed
        }
    }

    /// Records the pick id delivered with a button event.
    #[derive(Default)]
    struct ButtonHitProbe(Rc<std::cell::Cell<Option<u32>>>);

    impl EventListener for ButtonHitProbe {
        fn on_event(
            &mut self,
            event: &Event,
            hit: Option<&Hit>,
            _scene: &mut Scene,
            _directives: &Directives,
        ) -> bool {
            if matches!(event, Event::MouseButton { .. }) {
                self.0.set(hit.map(|hit| hit.id.0));
            }
            false
        }
    }

    #[test]
    fn pointer_events_drive_picking_and_the_listener() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_listener(Box::new(HoverProbe::default()));

        // Centre hits the triangle; a corner misses.
        assert!(viewport.handle_pointer([50.0, 50.0]));
        assert_eq!(viewport.hovered(), Some(PickId(0)));
        assert!(viewport.handle_pointer([5.0, 5.0]));
        assert_eq!(viewport.hovered(), None);
        // Leaving is a no-op once nothing is hovered.
        assert!(!viewport.handle_pointer_leave());
    }

    /// A two-face slab at `z = 0.5` (the identity camera looks along +Z), tagged
    /// `Face1`/`Face2` the way a kernel tessellation lays one out: vertices duplicated
    /// per face, every primitive carrying its part id, names in `parts`.
    fn named_slab() -> ferrocad_stage::ShapeItem {
        named_slab_for(7)
    }

    /// [`named_slab`] carrying a chosen pick id, for tests with more than one item.
    fn named_slab_for(entity: u32) -> ferrocad_stage::ShapeItem {
        use ferrocad_geom::Tessellation;
        use ferrocad_stage::ShapeItem;

        let mesh = Tessellation {
            positions: vec![
                [-0.5, -0.5, 0.5],
                [0.0, -0.5, 0.5],
                [0.0, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [0.0, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.0, 0.5, 0.5],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            triangle_parts: vec![0, 0, 1, 1],
            edges: vec![0, 1, 1, 2, 2, 3, 3, 0, 4, 5, 5, 6, 6, 7, 7, 4],
            segment_parts: vec![0, 0, 0, 0, 1, 1, 1, 1],
            vertices: vec![0, 1, 2, 3, 4, 5, 6, 7],
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_owned(), "Face2".to_owned()],
        };
        ShapeItem::from_tessellation(entity, &mesh)
    }

    /// The identity camera at 1x, where device and world coordinates coincide in XY.
    fn identity_viewport(scene: Scene) -> InventorViewport<FakeView> {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        InventorViewport::new(fake(ImageData::default()), camera, scene)
    }

    /// The sample triangle carrying a pick id, so a scene object is identifiable.
    fn triangle_scene_with_id(id: u32) -> Scene {
        let mut scene = triangle_scene();
        scene.root.children.insert(
            0,
            Node {
                kind: NodeKind::PickId(id),
                children: vec![],
            },
        );
        scene
    }

    #[test]
    fn a_click_selects_the_named_sub_element_under_the_pointer() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(named_slab()));
        view.refresh();

        // A click is a press and release without movement: left half → Face1.
        assert!(!view.handle_button([30.0, 50.0], 1, true));
        assert!(
            view.handle_button([30.0, 50.0], 1, false),
            "the selection update repaints"
        );
        let event = view.take_selection().expect("a selection");
        let SelectionEvent::Picked {
            reference,
            anchor,
            click_count,
        } = event
        else {
            panic!("not a pick: {event:?}");
        };
        assert_eq!(reference, SelectionRef::element(7, "Face1"));
        assert_eq!(click_count, 1, "a plain click reports one click");
        let anchor = anchor.expect("the click carries a world anchor");
        assert!(
            (anchor.point.z - 0.5).abs() < 1e-6,
            "the anchor sits on the clicked face: {anchor:?}"
        );
        assert!(anchor.normal.z > 0.9, "the normal points at the camera");
        // Taking the update consumes it.
        assert_eq!(view.take_selection(), None);

        view.handle_button([70.0, 50.0], 1, true);
        view.handle_button([70.0, 50.0], 1, false);
        let event = view.take_selection().expect("a selection");
        let SelectionEvent::Picked { reference, .. } = event else {
            panic!("not a pick: {event:?}");
        };
        assert_eq!(reference, SelectionRef::element(7, "Face2"));
    }

    /// The viewport reports gpui's click count alongside the raw sub-element pick. A
    /// host uses the count to upgrade a double click to the whole body; the viewport
    /// never guesses a double click from timing or distance.
    #[test]
    fn a_click_reports_the_press_click_count() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(named_slab()));
        view.refresh();

        let modifiers = GizmoModifiers::default();
        view.handle_button_with_modifiers_and_click_count([30.0, 50.0], 1, true, modifiers, 1);
        view.handle_button_with_modifiers([30.0, 50.0], 1, false, modifiers);
        let event = view.take_selection().expect("a selection");
        let SelectionEvent::Picked {
            reference,
            click_count,
            ..
        } = event
        else {
            panic!("not a pick: {event:?}");
        };
        assert_eq!(reference, SelectionRef::element(7, "Face1"));
        assert_eq!(click_count, 1, "a single click reports one");

        // The second press of a double click stores its count; the release reports it
        // with the same element reference, which is what the host upgrades.
        view.handle_button_with_modifiers_and_click_count([30.0, 50.0], 1, true, modifiers, 2);
        view.handle_button_with_modifiers([30.0, 50.0], 1, false, modifiers);
        let event = view.take_selection().expect("a selection");
        let SelectionEvent::Picked {
            reference,
            click_count,
            ..
        } = event
        else {
            panic!("not a pick: {event:?}");
        };
        assert_eq!(
            reference,
            SelectionRef::element(7, "Face1"),
            "the viewport still reports what it hit"
        );
        assert_eq!(click_count, 2, "the host upgrades on the count");
    }

    #[test]
    fn hover_mirrors_the_selection_without_consuming_it() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(named_slab()));
        view.refresh();

        view.handle_pointer([70.0, 50.0]);
        assert_eq!(view.hovered(), Some(PickId(7)));
        assert_eq!(view.hovered_part(), Some(1));
        assert_eq!(
            view.hovered_selection(),
            Some(SelectionRef::element(7, "Face2"))
        );
        assert_eq!(view.take_selection(), None, "hover is not a click");

        view.handle_pointer([98.0, 2.0]);
        assert_eq!(view.hovered(), None);
        assert_eq!(view.hovered_part(), None);
        assert_eq!(view.hovered_selection(), None);
    }

    #[test]
    fn an_untagged_mesh_selects_the_whole_object() {
        let mut view = identity_viewport(triangle_scene_with_id(9));
        view.handle_button([50.0, 50.0], 1, true);
        view.handle_button([50.0, 50.0], 1, false);
        let event = view.take_selection().expect("a selection");
        let SelectionEvent::Picked { reference, anchor, .. } = event else {
            panic!("not a pick: {event:?}");
        };
        assert_eq!(reference, SelectionRef::whole(9));
        assert!(anchor.is_some(), "the click carries its world point");
    }

    #[test]
    fn clicking_empty_space_clears_the_selection() {
        let mut view = identity_viewport(triangle_scene_with_id(9));
        view.handle_button([5.0, 5.0], 1, true);
        assert!(view.handle_button([5.0, 5.0], 1, false));
        assert_eq!(view.take_selection(), Some(SelectionEvent::Cleared));
    }

    #[test]
    fn an_orbit_drag_is_not_a_selection() {
        let mut view = identity_viewport(triangle_scene_with_id(9));
        assert!(!view.handle_button([50.0, 50.0], 1, true));
        assert!(view.drag_move([70.0, 50.0]));
        view.handle_button([70.0, 50.0], 1, false);
        assert_eq!(view.take_selection(), None, "an orbit selects nothing");
    }

    #[test]
    fn a_non_selectable_item_is_not_selected() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(named_slab().with_selectable(false)));
        view.refresh();

        view.handle_button([30.0, 50.0], 1, true);
        view.handle_button([30.0, 50.0], 1, false);
        assert_eq!(view.take_selection(), Some(SelectionEvent::Cleared));
    }

    #[test]
    fn to_local_subtracts_the_element_window_origin() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());

        // gpui reports window-relative positions; with the element at window (80, 40) a
        // report of (100, 60) is element-local (20, 20).
        viewport.last_bounds = Some(Bounds::new(
            point(px(80.0), px(40.0)),
            size(px(640.0), px(480.0)),
        ));
        assert_eq!(viewport.to_local(point(px(100.0), px(60.0))), [20.0, 20.0]);

        // Before the first prepaint there is no origin; pass the point through.
        viewport.last_bounds = None;
        assert_eq!(viewport.to_local(point(px(100.0), px(60.0))), [100.0, 60.0]);
    }

    /// A contextual trigger supplies **window** coordinates; the viewport resolves
    /// them against its element origin and runs the same pick a click uses.
    #[test]
    fn a_window_position_repicks_through_the_element_origin() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(named_slab()));
        view.refresh();
        view.last_bounds = Some(Bounds::new(
            point(px(80.0), px(40.0)),
            size(px(640.0), px(480.0)),
        ));

        // Window (110, 90) is element-local (30, 50): the slab's left face.
        assert!(view.handle_pointer_at(point(px(110.0), px(90.0))));
        assert_eq!(
            view.hover_report().target,
            Some(SelectionRef::element(7, "Face1"))
        );

        // Window (10, 10) is outside the element: empty space, not a stale target.
        view.handle_pointer_at(point(px(10.0), px(10.0)));
        assert_eq!(view.hover_report().target, None);
    }

    /// A typed delta previews as a temporary transform on exactly the named items; it
    /// never rewrites the placement, and clearing drops the ghost.
    #[test]
    fn a_typed_translation_previews_only_the_named_entities() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(
            Stage::new()
                .with_item(named_slab_for(7))
                .with_item(named_slab_for(8)),
        );
        view.refresh();

        assert!(view.preview_translation(&[7], [5.0, 2.0, 0.0]));
        let preview = view
            .stage
            .item(7)
            .and_then(|item| item.temporary_transform())
            .expect("the target previews");
        assert_eq!(
            preview.transform(&Vector3::zero()),
            Vector3::new(5.0, 2.0, 0.0)
        );
        assert!(
            view.stage
                .item(8)
                .and_then(|item| item.temporary_transform())
                .is_none(),
            "an unlisted item is untouched"
        );

        // Clearing is the cancel path: no temp transform survives.
        assert!(view.preview_translation(&[], [0.0; 3]));
        assert!(
            view.stage
                .item(7)
                .and_then(|item| item.temporary_transform())
                .is_none()
        );
    }

    #[test]
    fn the_element_size_drives_the_camera_viewport_and_the_backend() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_navicube(NaviCube::new(Viewport::new([640, 480], 1.0)));
        let cube_before = viewport.navicube.as_ref().unwrap().rect();

        // An 800x600 logical element at 2x is 1600x1200 device pixels.
        assert!(viewport.apply_element_size([800.0, 600.0], 2.0));
        assert_eq!(viewport.camera.viewport.device_size(), [1600, 1200]);
        assert_eq!(viewport.camera.viewport.device_pixel_ratio(), 2.0);
        assert_eq!(viewport.view().last_resize, Some([1600, 1200]));
        assert_eq!(viewport.view().resize_calls, 1);
        assert_ne!(
            viewport.navicube.as_ref().unwrap().rect(),
            cube_before,
            "the navigation cube follows the resize"
        );
        // The overlay works in device pixels: a 100-logical cube gives a 147.79-logical
        // box, i.e. 296 device px at 2x, centred in a 248-logical (496-device) chrome
        // column inset by the 8-logical (16-device) margin, mapping back to 148 logical.
        let cube = viewport.navicube.as_ref().unwrap();
        assert_eq!(cube.rect().width, 296, "physical size is logical × dpr");
        assert_eq!(cube.rect().x, 1600 - 16 - 496 + (496 - 296) / 2);
        let [left, _, right, _] = cube.logical_rect();
        assert!(
            (right - left - 148.0).abs() < 1e-6,
            "the logical extent is unchanged"
        );

        // A redundant report does not touch the backend again.
        assert!(!viewport.apply_element_size([800.0, 600.0], 2.0));
        assert_eq!(viewport.view().resize_calls, 1, "an unchanged size is a no-op");
    }

    /// A presenter with a fixed status, recording how often `draw` ran and returning
    /// `pick` (when set) as its G-buffer hit.
    struct TestPresenter {
        status: PresenterStatus,
        draws: Rc<std::cell::Cell<u32>>,
        size: [u32; 2],
        pick: Option<u32>,
    }

    impl Presenter for TestPresenter {
        fn status(&self) -> PresenterStatus {
            self.status
        }
        fn set_size(&mut self, size: [u32; 2]) {
            self.size = size;
        }
        fn prepare(&mut self, _gpu: &mut gpui::GpuCanvasContext) {}
        fn draw(&mut self, _views: &[ViewSpec<'_>]) -> bool {
            self.draws.set(self.draws.get() + 1);
            true
        }
        fn present(&mut self, _gpu: &mut gpui::GpuCanvasContext) {}
        fn pick_at(
            &mut self,
            _queue: &RenderQueue,
            _camera: &Camera,
            device: [i32; 2],
        ) -> Option<Hit> {
            self.pick.map(|id| Hit {
                id: PickId(id),
                point: [device[0] as f32, device[1] as f32, 0.0],
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            })
        }
        fn supports_pick(&self) -> bool {
            self.pick.is_some()
        }
    }

    #[test]
    fn a_ready_presenter_replaces_the_readback_path() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let draws = Rc::new(std::cell::Cell::new(0));
        let presenter = TestPresenter {
            status: PresenterStatus::Ready,
            draws: draws.clone(),
            size: [0, 0],
            pick: None,
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_presenter(Box::new(presenter));

        assert_eq!(viewport.presenter_status(), Some(PresenterStatus::Ready));
        viewport.apply_element_size([100.0, 100.0], 1.0);
        viewport.refresh();

        assert_eq!(draws.get(), 1, "the presenter draws the frame");
        assert_eq!(
            viewport.view().image_calls, 0,
            "a ready presenter must not trigger a readback"
        );
    }

    #[test]
    fn a_pending_presenter_keeps_the_image_fallback() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let draws = Rc::new(std::cell::Cell::new(0));
        let presenter = TestPresenter {
            status: PresenterStatus::Pending,
            draws: draws.clone(),
            size: [0, 0],
            pick: None,
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_presenter(Box::new(presenter));

        viewport.apply_element_size([100.0, 100.0], 1.0);
        viewport.refresh();

        assert_eq!(draws.get(), 0, "an uninitialized presenter is not drawn");
        assert_eq!(viewport.view().image_calls, 1, "the image path is still used");
    }

    #[test]
    fn a_short_navicube_drag_is_a_click_not_an_orbit() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));

        let rect = view.navicube.as_ref().unwrap().rect();
        let centre = [
            rect.x + rect.width as i32 / 2,
            rect.y + rect.height as i32 / 2,
        ];
        let start = viewport_size.to_logical(centre);
        let before = view.camera.view.m;

        // The press begins the gesture; a click would snap on release. The cube owns
        // the gesture, not the scene.
        assert!(!view.handle_button(start, 1, true));
        assert!(view.navicube_drag.is_some());
        assert!(view.orbit_drag.is_none());

        // Movement under the min-drag threshold is still a click: no orbit.
        assert!(!view.drag_move(start));
        assert!(!view.drag_move([start[0] + 1.0, start[1]]));
        assert_eq!(view.camera.view.m, before, "a short drag must not orbit");

        // Past the threshold the drag orbits the camera.
        assert!(view.drag_move([start[0] + 20.0, start[1]]));
        assert_ne!(view.camera.view.m, before, "a long drag orbits");
    }

    #[test]
    fn navicube_negative_space_reaches_the_scene() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));

        let rect = view.navicube.as_ref().unwrap().rect();
        // Inside the gizmo rect but clear of every piece and of the Home button: the
        // top-left of the rect, above the cube. Device coordinates are bottom-left.
        let device = [rect.x + 2, rect.y + rect.height as i32 - 2];
        assert_eq!(
            view.navicube.as_ref().unwrap().hit(device),
            None,
            "the probe point must be negative space"
        );
        let empty = viewport_size.to_logical(device);

        // The neutral hover report names the overlay the click passes through.
        view.handle_pointer(empty);
        let report = view.hover_report().clone();
        assert_eq!(report.through, Some("navicube"));
        assert!(report.target.is_none(), "the probe point is empty space");

        // The press belongs to the scene: the camera orbit starts, the cube does not
        // capture a drag.
        assert!(!view.handle_button(empty, 1, true));
        assert!(view.navicube_drag.is_none());
        assert!(view.orbit_drag.is_some());
        let before = view.camera.view.m;
        assert!(view.drag_move([empty[0] + 30.0, empty[1]]));
        assert_ne!(view.camera.view.m, before, "the scene drag orbits the view");
    }

    #[test]
    fn camera_angles_report_the_viewpoint() {
        let direction = Vector3::new(1.0, 0.9, 1.0).normalize();
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(direction, Vector3::new(0.0, 0.0, 1.0)),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        let (azimuth, elevation) = camera_angles(&camera);
        // Z-up: (1, 0.9, 1) is atan2(0.9, 1) ≈ 42° in the XY plane and
        // asin(1/√2.81) ≈ 36.6° above it.
        assert!((azimuth - 42.0).abs() < 0.2, "azimuth {azimuth}");
        assert!((elevation - 36.6).abs() < 0.2, "elevation {elevation}");
    }

    #[test]
    fn camera_angles_keep_a_heading_at_the_pole() {
        use crate::navicube::{Piece, snap_view, yaw_view};

        // A Top snap puts the eye exactly on +Z. The eye has no bearing there, so the
        // screen-right direction supplies one: the canonical top view has +X right and
        // reads 0°, and a yaw turns it instead of freezing at 000°.
        let top = Camera {
            view: snap_view(Piece::TOP, &Camera::identity()),
            ..Camera::identity()
        };
        let (azimuth, elevation) = camera_angles(&top);
        assert!((elevation - 90.0).abs() < 1e-6, "elevation {elevation}");
        assert!(azimuth.abs() < 1e-6, "azimuth {azimuth}");

        let yawed = Camera {
            view: yaw_view(&top, std::f64::consts::FRAC_PI_2),
            ..top
        };
        let (turned, elevation) = camera_angles(&yawed);
        assert!((elevation - 90.0).abs() < 1e-6, "elevation {elevation}");
        let delta = (turned - azimuth + 180.0).rem_euclid(360.0) - 180.0;
        assert!(
            (delta.abs() - 90.0).abs() < 1e-6,
            "azimuth {azimuth} -> {turned}"
        );
    }

    #[test]
    fn the_chrome_sits_beside_the_cube_in_every_corner() {
        use crate::navicube::{Corner, NaviCubeConfig};

        let viewport = Viewport::new([640, 480], 1.0);
        let config = |corner| NaviCubeConfig {
            corner,
            cube_size: 132.0,
            margin: 8.0,
        };
        let rect = |corner| chrome_bar_rect(&NaviCube::new(viewport).with_config(config(corner)));
        // A 132px cube needs a 195px overlay box for the turntable; the bar is centred on
        // that box, so its outer edge keeps the margin.
        assert_eq!(rect(Corner::TopRight), [383.5, 203.0, 631.5, 229.0]);
        assert_eq!(rect(Corner::TopLeft), [7.5, 203.0, 255.5, 229.0]);
        assert_eq!(rect(Corner::BottomLeft), [7.5, 251.0, 255.5, 277.0]);
        assert_eq!(rect(Corner::BottomRight), [383.5, 251.0, 631.5, 277.0]);
    }

    #[test]
    fn the_chrome_pill_is_centred_on_the_cube_in_every_corner() {
        use crate::navicube::{Corner, NaviCubeConfig};

        let viewport = Viewport::new([640, 480], 1.0);
        for corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            let cube = NaviCube::new(viewport).with_config(NaviCubeConfig {
                corner,
                cube_size: 132.0,
                margin: 8.0,
            });
            // `logical_rect` is the overlay box; the cube body is drawn centred in it, so
            // its horizontal centre is the painted centre.
            let [left, _, right, _] = cube.logical_rect();
            let bar = chrome_bar_rect(&cube);
            let cube_centre = (left + right) / 2.0;
            let bar_centre = (bar[0] + bar[2]) / 2.0;
            assert!(
                (bar_centre - cube_centre).abs() < 1e-4,
                "{corner:?}: pill centre {bar_centre} != cube centre {cube_centre}"
            );
        }
    }

    #[test]
    fn the_chrome_pill_sits_one_gap_beyond_the_cube() {
        use crate::navicube::{Corner, NaviCubeConfig};

        let viewport = Viewport::new([640, 480], 1.0);
        let cube = |corner| {
            NaviCube::new(viewport).with_config(NaviCubeConfig {
                corner,
                cube_size: 132.0,
                margin: 8.0,
            })
        };
        // A top corner hangs the pill below the cube's logical rect.
        for corner in [Corner::TopLeft, Corner::TopRight] {
            let cube = cube(corner);
            let [_, _, _, bottom] = cube.logical_rect();
            let bar = chrome_bar_rect(&cube);
            assert_eq!(bar[1], bottom + CHROME_GAP, "{corner:?}: pill top");
            assert_eq!(bar[3], bar[1] + CHROME_HEIGHT, "{corner:?}: pill bottom");
        }
        // A bottom corner puts the pill above it.
        for corner in [Corner::BottomLeft, Corner::BottomRight] {
            let cube = cube(corner);
            let [_, top, _, _] = cube.logical_rect();
            let bar = chrome_bar_rect(&cube);
            assert_eq!(bar[3], top - CHROME_GAP, "{corner:?}: pill bottom");
            assert_eq!(
                bar[1],
                top - CHROME_GAP - CHROME_HEIGHT,
                "{corner:?}: pill top"
            );
        }
        assert_eq!(CHROME_GAP, 0.0, "the cube is tucked against the pill");
    }

    #[test]
    fn the_chrome_tracks_the_cubes_device_rounded_rect() {
        // 704×528 device at 1.1× = 640×480 logical. The cube's 100px logical size gives a
        // 147.79px box, which rounds to 163 device pixels; the margin rounds to 9 and the
        // chrome column to 273. The chrome bar must follow the painted rect, exactly as it
        // does at whole ratios.
        let cube = NaviCube::new(Viewport::new([704, 528], 1.1));
        let [left, top, right, bottom] = cube.logical_rect();
        assert!((right - left - 163.0 / 1.1).abs() < 1e-4);
        assert!((left - 477.0 / 1.1).abs() < 1e-4);
        assert!((top - 9.0 / 1.1).abs() < 1e-4);

        let bar = chrome_bar_rect(&cube);
        let centre = (left + right) / 2.0;
        assert!(
            (bar[0] - (centre - CHROME_WIDTH / 2.0)).abs() < 1e-4,
            "the bar is centred on the painted cube"
        );
        assert!((bar[2] - (centre + CHROME_WIDTH / 2.0)).abs() < 1e-4);
        assert_eq!(
            bar[1],
            bottom + CHROME_GAP,
            "the bar sits one gap below the cube"
        );
        assert_eq!(bar[3], bar[1] + CHROME_HEIGHT);
    }

    #[test]
    fn a_press_on_the_chrome_is_reserved_at_two_x() {
        let camera = Camera {
            viewport: Viewport::new([1280, 960], 2.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));

        // 640×480 logical at 2×: the 296px overlay box (a 100px cube plus its turntable) is
        // centred in a 248-logical column inset 8 logical pixels from the corner, and the
        // bar fills that column below it.
        let [left, top, right, bottom] = view.chrome_rect().expect("the cube is placed");
        assert_eq!([left, top, right, bottom], [384.0, 156.0, 632.0, 182.0]);

        let in_bar = [left + 2.0, top + 2.0];
        assert!(view.chrome_contains(in_bar));
        assert!(!view.handle_button(in_bar, 1, true));
        assert!(view.orbit_drag.is_none(), "the chrome owns the press");

        let in_scene = [left + 2.0, bottom + 2.0];
        assert!(!view.chrome_contains(in_scene));
        assert!(!view.handle_button(in_scene, 1, true));
        assert!(view.orbit_drag.is_some(), "below the bar the scene orbits");
    }

    #[test]
    fn the_chrome_does_not_start_a_scene_orbit() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));

        let [x, y, _, _] = view.chrome_rect().expect("the cube is placed");
        let in_bar = [x + 4.0, y + 4.0];
        assert!(view.chrome_contains(in_bar));
        assert!(!view.handle_button(in_bar, 1, true));
        assert!(view.orbit_drag.is_none(), "the chrome owns the press");
        assert!(!view.handle_pointer(in_bar));

        // Outside the bar the scene takes the press as usual.
        let scene = [x + 4.0, y + CHROME_HEIGHT + 4.0];
        assert!(!view.chrome_contains(scene));
        assert!(!view.handle_button(scene, 1, true));
        assert!(view.orbit_drag.is_some());
    }

    #[test]
    fn the_chrome_toggles_projection_without_jumping_the_framing() {
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::identity(),
            )
            .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([640, 480], 1.0),
        };
        let before = camera.projection;
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());

        assert!(!view.is_orthographic());
        assert!(view.set_orthographic(true));
        assert!(view.is_orthographic());
        assert!(!view.set_orthographic(true), "no-op when already orthographic");
        assert!(view.set_orthographic(false));
        assert_eq!(
            view.camera().projection,
            before,
            "toggling back restores the same field of view"
        );
    }

    #[test]
    fn the_chrome_home_starts_a_snap() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        assert!(view.go_home());
        assert!(view.snap.is_some(), "Home animates rather than jumping");
    }

    /// A camera tilted so the ground-plane ring is a proper ellipse (not edge-on).
    fn tilted_camera() -> Camera {
        Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                crate::navicube::view_rotation(std::f64::consts::FRAC_PI_2, 0.5),
            )
            .to_matrix(),
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        }
    }

    #[test]
    fn the_ring_scrubs_yaw_without_rolling() {
        let camera = tilted_camera();
        let viewport_size = camera.viewport;
        let (initial_azimuth, initial_elevation) = camera_angles(&camera);
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        let camera = view.camera().clone();
        view.navicube.as_mut().unwrap().sync(&camera);
        let start = view
            .navicube
            .as_ref()
            .unwrap()
            .ring_point(std::f64::consts::FRAC_PI_2);
        let end = view.navicube.as_ref().unwrap().ring_point(0.0);

        // The press belongs to the projected ring, not the scene.
        assert!(!view.handle_button(start, 1, true));
        assert!(view.ring_drag.is_some());
        assert!(view.orbit_drag.is_none());

        // A quarter turn clockwise around the plane (S -> E) moves the azimuth by −90°.
        assert!(view.drag_move(end));
        let (azimuth, elevation) = camera_angles(view.camera());
        assert!(
            (azimuth - (initial_azimuth - 90.0)).abs() < 2.5,
            "azimuth {azimuth}"
        );
        assert!(
            (elevation - initial_elevation).abs() < 1e-9,
            "elevation {elevation}"
        );

        // Releasing settles on the nearest detent (the pixel-rounded scrub lands ~1° off
        // the exact heading); either way the gesture is over and no scene orbit starts.
        view.handle_button(end, 1, false);
        assert!(view.ring_drag.is_none());
        assert!(view.orbit_drag.is_none());
    }

    #[test]
    fn a_multi_step_ring_scrub_returns_to_its_start() {
        let camera = tilted_camera();
        let viewport_size = camera.viewport;
        let (initial_azimuth, _) = camera_angles(&camera);
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        let camera = view.camera().clone();
        view.navicube.as_mut().unwrap().sync(&camera);

        // The pointer follows the ring **as drawn at press time**: the scrub projects the
        // pointer with the frozen press orientation, so a fixed screen path must return to
        // the same azimuth when the pointer walks back.
        let quarter = std::f64::consts::FRAC_PI_2;
        let path: Vec<[f32; 2]> = (0..=20)
            .map(|i| {
                let angle = quarter * (1.0 - i as f64 / 20.0);
                view.navicube.as_ref().unwrap().ring_point(angle)
            })
            .collect();

        assert!(!view.handle_button(path[0], 1, true));
        for point in &path[1..] {
            assert!(view.drag_move(*point));
            // The render loop re-syncs the cube after every frame; the scrub must not
            // measure its next angle against that rotated ring.
            let camera = view.camera().clone();
            view.navicube.as_mut().unwrap().sync(&camera);
        }
        let (mid, _) = camera_angles(view.camera());
        assert!(
            (mid - (initial_azimuth - 90.0)).abs() < 3.0,
            "quarter turn went to {mid}"
        );

        for point in path.iter().rev().skip(1) {
            assert!(view.drag_move(*point));
            let camera = view.camera().clone();
            view.navicube.as_mut().unwrap().sync(&camera);
        }
        let (end, _) = camera_angles(view.camera());
        assert!(
            (end - initial_azimuth).abs() < 3.0,
            "the scrub did not return to its start ({end} vs {initial_azimuth})"
        );
    }

    #[test]
    fn rapid_opposite_yaws_compose_to_the_start() {
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Rotation::identity()).to_matrix(),
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        let before = Rotation::from_matrix(&view.camera.view).to_matrix();

        assert!(view.yaw(YawDirection::CounterClockwise));
        let turned = view.snap.as_ref().unwrap().target().to_matrix();
        assert_ne!(turned.m, before.m, "the first yaw changes the pose");

        // The second click happens before the animation finishes; it must compose from
        // the pending target, not from the half-turned camera.
        assert!(view.yaw(YawDirection::Clockwise));
        let target = view.snap.as_ref().unwrap().target().to_matrix();
        for (a, b) in target.m.iter().zip(before.m.iter()) {
            assert!((a - b).abs() < 1e-9, "rapid opposite yaws did not cancel");
        }
    }

    #[test]
    fn a_ring_release_near_a_detent_snaps() {
        let camera = tilted_camera();
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        let camera = view.camera().clone();
        view.navicube.as_mut().unwrap().sync(&camera);
        let start = view
            .navicube
            .as_ref()
            .unwrap()
            .ring_point(std::f64::consts::FRAC_PI_2);
        // Five degrees past the 90° detent, in the ring plane.
        let near = view
            .navicube
            .as_ref()
            .unwrap()
            .ring_point(85.0_f64.to_radians());

        assert!(!view.handle_button(start, 1, true));
        assert!(view.drag_move(near));
        let (azimuth, _) = camera_angles(view.camera());
        assert!((azimuth - 85.0).abs() < 3.0, "azimuth {azimuth}");
        assert!(view.handle_button(near, 1, false), "the detent captures");
        assert!(view.snap.is_some());
    }

    #[test]
    fn a_chrome_yaw_starts_a_quarter_turn() {
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Rotation::identity()).to_matrix(),
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let viewport_size = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
            .with_navicube(NaviCube::new(viewport_size));
        let before = view.camera.view;
        let (azimuth_before, elevation_before) = camera_angles(view.camera());

        assert!(view.yaw(YawDirection::Clockwise));
        assert_eq!(view.camera.view, before, "the yaw is animated, not instant");
        assert!(view.ring_drag.is_none());
        assert!(view.navicube_drag.is_none());
        assert!(view.orbit_drag.is_none());

        let snap = view.snap.as_ref().expect("a yaw animation");
        let mut end = view.camera().clone();
        end.view = snap.view_at(Duration::from_millis(220));
        let (azimuth, elevation) = camera_angles(&end);
        // Clockwise turns the eye clockwise: the azimuth drops by 90°, elevation is kept.
        let delta = (azimuth - (azimuth_before - 90.0) + 180.0).rem_euclid(360.0) - 180.0;
        assert!(delta.abs() < 1e-6, "azimuth {azimuth_before} -> {azimuth}");
        assert!((elevation - elevation_before).abs() < 1e-9);
    }

    #[test]
    fn a_released_orbit_glides_and_damps() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        view.orbit_velocity = [600.0, 0.0];
        view.last_drag_time = Some(std::time::Instant::now());
        view.start_inertia();
        assert!(view.inertia.is_some(), "a fast release glides");

        let before = view.camera.view.m;
        assert!(view.step_inertia_by(Duration::from_millis(16)));
        assert_ne!(view.camera.view.m, before, "the glide orbits the camera");

        let mut steps = 1;
        while view.step_inertia_by(Duration::from_millis(16)) {
            steps += 1;
            assert!(steps < 1000, "the glide must decay to a stop");
        }
        assert!(view.inertia.is_none());
    }

    #[test]
    fn releasing_after_a_pause_does_not_glide() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        view.orbit_velocity = [600.0, 0.0];
        view.last_drag_time = Some(std::time::Instant::now() - Duration::from_millis(400));
        view.start_inertia();
        assert!(view.inertia.is_none(), "a paused pointer releases without a fling");
    }

    #[test]
    fn a_press_stops_an_inertial_glide() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        view.inertia = Some(Inertia {
            velocity: [600.0, 0.0],
            last: std::time::Instant::now(),
        });
        assert!(!view.handle_button([20.0, 20.0], 1, true));
        assert!(view.inertia.is_none(), "grabbing the view stops the glide");
        assert!(view.orbit_drag.is_some());
    }

    #[test]
    fn a_scene_drag_orbits_the_camera() {
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        let before = view.camera.view.m;

        // A press on the scene starts a possible orbit; the threshold keeps a click a
        // click, and the next move past it rotates the camera.
        assert!(!view.handle_button([5.0, 5.0], 1, true));
        assert!(!view.drag_move([5.0, 5.0]));
        assert!(view.drag_move([35.0, 5.0]));
        assert_ne!(view.camera.view.m, before, "a scene drag rotates the view");
        view.handle_button([35.0, 5.0], 1, false);
    }

    #[test]
    fn scrolling_zooms_the_camera() {
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::identity(),
            )
            .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 0.8,
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        };
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        let distance = |m: &[f64; 16]| (m[3] * m[3] + m[7] * m[7] + m[11] * m[11]).sqrt();
        let before = distance(&view.camera.view.m);

        // A wheel-up notch moves the eye towards the scene; wheel-down restores it.
        assert!(view.handle_scroll(1.0));
        let zoomed = distance(&view.camera.view.m);
        assert!(zoomed < before, "scroll up zooms in: {zoomed} < {before}");
        assert!(view.handle_scroll(-1.0));
        assert!((distance(&view.camera.view.m) - before).abs() < 1e-9);
    }

    /// A view with a hardware pick path: it reports id `42` wherever there is a queue to
    /// pick, ignoring the scene, so a test can tell it apart from the CPU ray cast (which
    /// would report `PickId(0)` at the centre).
    struct HardwarePickView;

    impl InventorView for HardwarePickView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [100, 100]
        }
        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Ok(ImageData::default())
        }
        fn supports_pick(&self) -> bool {
            true
        }
        fn pick_at(
            &mut self,
            queue: &RenderQueue,
            _camera: &Camera,
            device: [i32; 2],
        ) -> Option<Hit> {
            (!queue.is_empty()).then_some(Hit {
                id: PickId(42),
                point: [device[0] as f32, device[1] as f32, 0.0],
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            })
        }
    }

    #[test]
    fn the_viewport_prefers_a_hardware_pick_when_the_backend_has_one() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(HardwarePickView, camera, triangle_scene())
            .with_listener(Box::new(HoverProbe::default()));

        // The CPU path would report `PickId(0)` over the triangle; the backend's hardware
        // pick reports 42, so seeing 42 proves it was preferred.
        assert!(viewport.handle_pointer([50.0, 50.0]));
        assert_eq!(viewport.hovered(), Some(PickId(42)));
    }

    #[test]
    fn a_ready_presenters_g_buffer_pick_is_preferred() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let presenter = TestPresenter {
            status: PresenterStatus::Ready,
            draws: Rc::new(std::cell::Cell::new(0)),
            size: [0, 0],
            pick: Some(99),
        };
        // `HardwarePickView` would report 42; the presenter's window-device G-buffer
        // reports 99 and must win, for hover and for button events alike.
        let button_hit = Rc::new(std::cell::Cell::new(None));
        let mut viewport = InventorViewport::new(HardwarePickView, camera, triangle_scene())
            .with_presenter(Box::new(presenter))
            .with_listener(Box::new(HoverProbe::default()))
            .with_listener(Box::new(ButtonHitProbe(button_hit.clone())));

        assert!(viewport.handle_pointer([50.0, 50.0]));
        assert_eq!(viewport.hovered(), Some(PickId(99)));

        viewport.handle_button([50.0, 50.0], 1, true);
        assert_eq!(
            button_hit.get(),
            Some(99),
            "the button event used the presenter's G-buffer pick"
        );
    }

    #[test]
    fn viewport_compiles_the_scene_it_is_given() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let scene = triangle_scene();
        let expected = compile_with_base(&scene, &camera.viewport, None)
            .expect("the triangle scene compiles")
            .0
            .len();
        let viewport = InventorViewport::new(fake(ImageData::default()), camera, scene);
        assert_eq!(viewport.queue.len(), expected);
        assert_eq!(viewport.scene().root.children.len(), 3);
    }

    #[test]
    fn a_navicube_directive_hosts_an_overlay() {
        use ferrocad_graphics::Rect;

        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let directive = NaviCubeDirective {
            rect: Rect::new(60, 60, 40, 40),
            orientation: None,
            hilite: None,
            orthographic: true,
        };
        let mut scene = triangle_scene();
        scene.root.children.push(Node {
            kind: NodeKind::NaviCube(directive),
            children: vec![],
        });

        let mut viewport = InventorViewport::new(fake(ImageData::default()), camera, scene);

        // The directive hosts exactly one overlay at its rect, with geometry of its own,
        // while the main queue still holds only the triangle.
        assert_eq!(viewport.directive_cubes.len(), 1);
        assert_eq!(viewport.directive_cubes[0].1.rect(), Rect::new(60, 60, 40, 40));
        assert!(!viewport.directive_cubes[0].1.view().queue.is_empty());
        assert_eq!(viewport.queue.len(), 1);

        // Drawing routes the overlay through the fake view without panicking.
        viewport.refresh();
    }

    #[test]
    fn a_stage_item_is_merged_into_the_viewport_queue() {
        use ferrocad_stage::{DrawMode, ShapeItem, Stage};

        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        let base = viewport.queue.len();
        assert!(base >= 1, "the inventor scene compiles to at least one packet");

        // An item added through the stage accessor is merged on the next draw (refresh
        // rebuilds the queue, then renders it).
        viewport.stage_mut().add_item(
            ShapeItem::new(5)
                .with_draw_mode(DrawMode::Shaded)
                .with_shape(
                    vec![[0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [0.0, 1.0, 0.5]],
                    vec![],
                    vec![0, 1, 2],
                    vec![],
                ),
        );
        viewport.refresh();
        assert_eq!(viewport.queue.len(), base + 1);
        assert!(viewport.queue.packets.iter().any(|p| p.state.id == PickId(5)));

        // Replacing the stage drops the item again.
        viewport.set_stage(Stage::new());
        viewport.refresh();
        assert_eq!(viewport.queue.len(), base);
    }

    #[test]
    fn a_tool_receives_pointer_events_and_its_items_are_drawn() {
        use ferrocad_graphics::Topology;
        use ferrocad_stage::PolylineTool;

        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        )
        .with_tool(Box::new(PolylineTool::new()));

        // Nothing transient is drawn until the tool sees an event.
        assert!(viewport.transient.is_empty());

        // A left press over the scene is dispatched to the tool, which emits a band and a
        // marker (wireframe) for the overlay.
        assert!(viewport.handle_button([50.0, 50.0], 1, true));
        assert!(
            !viewport.transient.is_empty(),
            "the tool should emit transient visuals"
        );
        assert!(
            viewport
                .transient
                .packets
                .iter()
                .all(|packet| packet.topology == Topology::Lines),
            "transient visuals are wireframe"
        );

        // The draw path (scene + transient overlay) runs cleanly on the fake view.
        viewport.refresh();
    }

    /// A 45° perspective camera at `(0, 0, 5)` looking toward the origin.
    fn perspective_camera(size: [u32; 2]) -> Camera {
        use ferrocad_types::{Placement, Rotation, Vector3};
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -5.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new(size, 1.0),
        }
    }

    /// A three-quarter camera for the gizmo tests. An axis-aligned view fades the axis
    /// and the two planes that collapse to a line, so the handle-count and packet-count
    /// tests need every handle live.
    fn gizmo_camera(size: [u32; 2]) -> Camera {
        use ferrocad_types::{Placement, Rotation, Vector3};
        let rotation =
            Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), (-35.0_f64).to_radians())
                .multiply(&Rotation::from_axis_angle(
                    &Vector3::new(0.0, 0.0, 1.0),
                    35.0_f64.to_radians(),
                ));
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -5.0), rotation).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new(size, 1.0),
        }
    }

    /// A 100×100 viewport with a triangle at the origin and one gizmo target.
    fn gizmo_view(
        gizmo: TransformGizmo,
    ) -> (InventorViewport<FakeView>, ferrocad_graphics::Viewport) {
        use ferrocad_stage::ShapeItem;

        let camera = gizmo_camera([100, 100]);
        let viewport = camera.viewport;
        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(42).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![],
        ));
        let mut view = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        view.set_stage(stage);
        view.set_gizmo(Some(gizmo));
        view.refresh();
        (view, viewport)
    }

    fn preview(view: &InventorViewport<FakeView>) -> Option<ferrocad_types::Matrix4> {
        view.stage()
            .items()
            .iter()
            .find(|item| item.entity_id() == 42)
            .and_then(|item| item.temporary_transform())
    }

    /// A mode change is a handle-set change: the compiled overlay must be rebuilt,
    /// not just the stage object, or the old handles stay pickable.
    #[test]
    fn changing_the_gizmo_mode_rebuilds_the_handle_overlay() {
        use ferrocad_stage::GizmoMode;

        let (mut view, _viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let (rotate, _viewport) =
            gizmo_view(TransformGizmo::new(42, Vector3::zero(), GizmoMode::Rotate));
        assert_ne!(
            view.gizmo_queue.len(),
            rotate.gizmo_queue.len(),
            "the two modes compile different overlays"
        );

        assert!(view.set_gizmo_mode(GizmoMode::Rotate));
        assert_eq!(view.gizmo().expect("a gizmo").mode(), GizmoMode::Rotate);
        assert_eq!(
            view.gizmo_queue.len(),
            rotate.gizmo_queue.len(),
            "the compiled overlay matches the ring-only gizmo"
        );
        assert!(
            !view.set_gizmo_mode(GizmoMode::Rotate),
            "setting the same mode is a no-op"
        );
    }

    /// The viewport hosts the registry **set**: the session narrows which kind is
    /// offered, and changing the verb re-narrows in place instead of replacing the one
    /// gizmo, so no registered kind is lost.
    #[test]
    fn hosting_a_set_re_narrows_without_dropping_the_other_kinds() {
        use ferrocad_stage::{GizmoKind, GizmoSet, TransformGizmo};

        let camera = gizmo_camera([100, 100]);
        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(42).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![],
        ));
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        view.set_stage(stage);
        view.set_gizmos(GizmoSet::new(vec![
            TransformGizmo::translate(42, Vector3::zero()),
            TransformGizmo::rotate(42, Vector3::zero()),
            TransformGizmo::universal(42, Vector3::zero()),
        ]));
        view.refresh();

        assert_eq!(view.gizmos().len(), 3, "the registry's whole set is hosted");
        assert_eq!(
            view.gizmos().offered().len(),
            1,
            "one control is offered at the shared origin"
        );
        assert_eq!(
            view.gizmo().expect("a gizmo").kind(),
            GizmoKind::Translate,
            "the session defaults to the registry's specific translate entry"
        );
        let translate_packets = view.gizmo_queue.len();

        assert!(view.set_gizmo_mode(GizmoKind::Rotate));
        assert_eq!(view.gizmos().len(), 3, "re-narrowing keeps every entry");
        assert_eq!(view.gizmo().expect("a gizmo").kind(), GizmoKind::Rotate);
        let rotate_packets = view.gizmo_queue.len();
        assert_ne!(translate_packets, rotate_packets, "the overlay was rebuilt");

        assert!(view.set_gizmo_mode(GizmoKind::Universal));
        assert_eq!(view.gizmo().expect("a gizmo").kind(), GizmoKind::Universal);
        assert!(
            view.gizmo_queue.len() > rotate_packets,
            "universal draws both handle sets"
        );
        assert!(
            !view.set_gizmo_mode(GizmoKind::Universal),
            "the same session is a no-op"
        );
    }

    /// A press is dispatched to the entry the set's priority named, and only that entry
    /// drags. An earlier gizmo's stem shadows a later gizmo's origin handle at the same
    /// pixel.
    #[test]
    fn the_highest_priority_gizmo_takes_the_press() {
        use ferrocad_stage::{GizmoHandle, GizmoSet, GizmoTarget, TransformGizmo};

        let camera = gizmo_camera([100, 100]);
        let viewport = camera.viewport;
        let mut view =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());

        // Put the second entry's origin on the first entry's X stem, so both claim the
        // same pixel: the earlier entry must win.
        let mut first = TransformGizmo::translate(42, Vector3::zero());
        first.update(&view.camera);
        let shared = Vector3::new(first.scale() * 0.6, 0.0, 0.0);
        view.set_gizmos(GizmoSet::new(vec![
            first,
            TransformGizmo::translate(42, shared),
        ]));
        view.refresh();

        let grab = project_device(&view.camera, shared).expect("the shared point projects");
        let grab = [grab[0].round() as i32, grab[1].round() as i32];
        assert_eq!(
            view.gizmos().gizmos()[0].hit(&view.camera, grab),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX)),
            "the first entry's stem claims the shared pixel"
        );
        assert_eq!(
            view.gizmo_target_at(grab),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX)),
            "the set answered with the priority entry's target"
        );
        assert_eq!(
            view.gizmos().gizmos()[1].hit(&view.camera, grab),
            Some(GizmoTarget::Origin),
            "the second entry is also a candidate"
        );

        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(
            view.gizmos().gizmos()[0].is_dragging(),
            "the press went to the priority entry"
        );
        assert!(
            !view.gizmos().gizmos()[1].is_dragging(),
            "the shadowed entry did not also start a drag"
        );
    }

    #[test]
    fn a_gizmo_drag_previews_a_transform_on_the_target_item() {
        use ferrocad_stage::{
            GizmoHandle, GizmoMode, GizmoTarget, ShapeItem, Stage, TransformGizmo,
        };
        use ferrocad_types::Vector3;

        let size = [100u32, 100u32];
        let camera = perspective_camera(size);
        let viewport = camera.viewport;

        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(42).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![],
        ));

        let mut view = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        view.set_stage(stage);
        view.set_gizmo(Some(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();

        // Find a device position that hits the X axis handle through the analytic
        // screen-space test the viewport now uses.
        let mut grab = None;
        for x in 0..100i32 {
            for y in 0..100i32 {
                if view.gizmo().expect("gizmo").hit(&view.camera, [x, y])
                    == Some(GizmoTarget::Handle(GizmoHandle::AxisX))
                {
                    grab = Some([x, y]);
                }
            }
        }
        let grab = grab.expect("the X axis handle should be pickable");

        let preview = |view: &InventorViewport<FakeView>| {
            view.stage()
                .items()
                .iter()
                .find(|item| item.entity_id() == 42)
                .and_then(|item| item.temporary_transform())
        };

        // Press on the handle, then drag right: the target shows a live preview.
        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(view.gizmo().expect("gizmo").is_dragging());
        assert!(view.handle_pointer(viewport.to_logical([grab[0] + 12, grab[1]])));
        assert!(preview(&view).is_some(), "the drag previews a transform");
        assert!(view.last_gizmo_commit().is_none());

        // Release: a commit is recorded and the preview cleared.
        assert!(view.handle_button(viewport.to_logical([grab[0] + 12, grab[1]]), 1, false));
        assert!(view.last_gizmo_commit().is_some());
        assert!(preview(&view).is_none(), "the preview is cleared on commit");
        assert_eq!(
            view.take_selection(),
            None,
            "a gizmo commit is not a click selection"
        );
    }

    /// A large single-face triangle at `z = 0.5`, tagged `Face1`, for the pivot-snap
    /// tests. Its vertices, edge midpoints and face centre are well separated in device
    /// pixels under the identity camera.
    fn snap_triangle() -> ferrocad_stage::ShapeItem {
        use ferrocad_geom::Tessellation;
        use ferrocad_stage::ShapeItem;

        let mesh = Tessellation {
            positions: vec![[-0.6, -0.6, 0.5], [0.6, -0.6, 0.5], [0.0, 0.6, 0.5]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1, 1, 2, 2, 0],
            segment_parts: vec![0, 0, 0],
            vertices: vec![0, 1, 2],
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_owned()],
        };
        ShapeItem::from_tessellation(7, &mesh)
    }

    #[test]
    fn the_origin_snap_prefers_a_feature_then_the_workplane_grid() {
        use ferrocad_stage::Stage;

        let mut view = identity_viewport(Scene::default());
        view.set_stage(Stage::new().with_item(snap_triangle()));
        view.set_snap_grid(Some(0.25));
        let hit = Hit {
            id: PickId(7),
            point: [0.0, 0.0, 0.5],
            normal: [0.0, 0.0, 1.0],
            part: 0,
            element: None,
            topology: Topology::Triangles,
            distance: 1.0,
        };
        let kind = |device| {
            view.gizmo_origin_snap(device, Some(&hit))
                .map(|(_, kind)| kind)
        };

        // The triangle's vertex at (-0.6, -0.6) projects to (20, 20).
        assert_eq!(kind([22, 22]), Some(SnapKind::Vertex));
        // The base edge midpoint projects to (50, 20).
        assert_eq!(kind([50, 22]), Some(SnapKind::EdgeMid));
        // Its area-weighted centroid is (0, -0.2) → (50, 40).
        assert_eq!(kind([50, 40]), Some(SnapKind::Centre));
        // The workplane origin is (50, 50), also inside the face.
        assert_eq!(kind([50, 50]), Some(SnapKind::Origin));

        // With nothing under the pointer the workplane grid takes over; the point is on
        // the z = 0 plane, and no second snap set is invented.
        view.set_stage(Stage::new());
        let (point, grid) = view
            .gizmo_origin_snap([60, 60], None)
            .expect("the grid fallback applies");
        assert_eq!(grid, SnapKind::Grid);
        assert!(point.z.abs() < 1e-9, "the grid point lies on the workplane");
    }

    #[test]
    fn the_origin_handle_moves_the_real_frame_and_reports_the_snap() {
        use ferrocad_stage::{GizmoMode, GizmoTarget, TransformGizmo};

        let camera = gizmo_camera([100, 100]);
        let viewport = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, Scene::default());
        view.set_snap_grid(Some(0.25));
        view.set_gizmo(Some(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();
        let home = view.gizmo().expect("a gizmo").origin();

        // The centre is the re-pivot handle, not a transform handle.
        assert_eq!(view.gizmo_target_at([50, 50]), Some(GizmoTarget::Origin));
        assert!(view.handle_button(viewport.to_logical([50, 50]), 1, true));
        assert!(view.gizmo_origin_drag.is_some());
        assert!(
            view.take_gizmo_origin().is_none(),
            "nothing yet on the press"
        );

        // A move drags the **real** frame (no draw-only offset): the pivot has no solver.
        assert!(view.handle_pointer(viewport.to_logical([62, 58])));
        assert_ne!(view.gizmo().expect("a gizmo").origin(), home);
        assert!(view.gizmo().expect("a gizmo").display_offset().is_none());
        let text = view.readout_text().expect("the pivot readout is live");
        assert!(text.contains("pivot"), "readout: {text}");
        assert!(text.contains("grid"), "the readout names the snap: {text}");

        // Release reports the pivot, not a transform commit.
        assert!(view.handle_button(viewport.to_logical([62, 58]), 1, false));
        let (point, kind) = view.take_gizmo_origin().expect("a pivot event");
        assert_eq!(kind, SnapKind::Grid);
        assert_eq!(view.gizmo().expect("a gizmo").origin(), point);
        assert!(view.take_gizmo_commit().is_none(), "not a transform commit");
        assert!(view.take_selection().is_none(), "not a click selection");
    }

    #[test]
    fn escape_restores_the_pre_drag_pivot_origin() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let camera = gizmo_camera([100, 100]);
        let viewport = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, Scene::default());
        view.set_snap_grid(Some(0.25));
        view.set_gizmo(Some(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();
        let home = view.gizmo().expect("a gizmo").origin();

        assert!(view.handle_button(viewport.to_logical([50, 50]), 1, true));
        assert!(view.handle_pointer(viewport.to_logical([62, 58])));
        assert_ne!(view.gizmo().expect("a gizmo").origin(), home);

        assert!(view.handle_key(&escape_key()));
        assert_eq!(view.gizmo().expect("a gizmo").origin(), home);
        assert!(view.gizmo_origin_drag.is_none());
        assert!(view.take_gizmo_origin().is_none(), "Esc emits no pivot");
    }

    #[test]
    fn the_move_hud_disables_the_origin_handle() {
        let (mut view, _viewport) = translate_gizmo_view();
        let origin = GizmoTarget::ALL
            .iter()
            .position(|target| *target == GizmoTarget::Origin)
            .expect("the origin target");
        assert_eq!(view.gizmo_target_at([50, 50]), Some(GizmoTarget::Origin));

        view.set_gizmo_preview(true);
        assert_eq!(
            view.gizmo_target_at([50, 50]),
            None,
            "no pivot while the HUD is open"
        );
        assert!(
            view.gizmo().expect("a gizmo").items()[origin]
                .positions()
                .is_empty(),
            "the disabled origin is not drawn"
        );

        view.set_gizmo_preview(false);
        assert_eq!(view.gizmo_target_at([50, 50]), Some(GizmoTarget::Origin));
        assert!(
            !view.gizmo().expect("a gizmo").items()[origin]
                .positions()
                .is_empty(),
            "closing the form restores the handle"
        );
    }

    #[test]
    fn the_gizmo_interior_falls_through_to_the_scene() {
        use ferrocad_stage::{GizmoMode, GizmoTarget, ShapeItem, Stage, TransformGizmo};
        use ferrocad_types::Vector3;

        let size = [100u32, 100u32];
        let camera = perspective_camera(size);
        let viewport = camera.viewport;
        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(42).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![],
        ));
        let mut view = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        view.set_stage(stage);
        view.set_gizmo(Some(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Universal,
        )));
        view.refresh();

        assert_eq!(
            view.gizmo().expect("gizmo").hit(&view.camera, [50, 50]),
            Some(GizmoTarget::Origin),
            "the centre is the re-pivot handle now"
        );
        // The 10 px disc does not swallow the empty interior around it. A press there
        // leaves the gizmo alone and starts a scene orbit.
        assert_eq!(
            view.gizmo().expect("gizmo").hit(&view.camera, [60, 60]),
            None
        );
        assert!(!view.handle_button(viewport.to_logical([60, 60]), 1, true));
        assert!(!view.gizmo().expect("gizmo").is_dragging());
        assert!(
            view.orbit_drag.is_some(),
            "the empty interior orbits the scene"
        );
    }

    /// A minimal workbench manipulator: a grab disc at the origin's projection; a
    /// horizontal drag adds one unit per 20 device pixels, and the commit reports the
    /// value. It exists to prove the viewport routes **any** manipulator without naming
    /// one — the Part workbench's arc is not linked here.
    struct FakeManipulator {
        value: f64,
        press: Option<(f64, f64)>,
        hovered: bool,
        view: Option<Matrix4>,
        item: ShapeItem,
        /// The world anchor a preview offsets (a stand-in for a real grip).
        anchor: Option<Vector3>,
        /// The last workspace style the viewport handed it, shared with the test so it
        /// can observe the recorded value without downcasting.
        style: Rc<RefCell<Option<GizmoStyle>>>,
    }

    impl FakeManipulator {
        fn new() -> Self {
            Self::recording().0
        }

        /// The manipulator and a handle to the style it records.
        fn recording() -> (Self, Rc<RefCell<Option<GizmoStyle>>>) {
            let style = Rc::new(RefCell::new(None));
            (
                Self {
                    value: 1.0,
                    press: None,
                    hovered: false,
                    view: None,
                    item: ShapeItem::new(777)
                        .with_draw_mode(DrawMode::Wireframe)
                        .with_selectable(false),
                    anchor: Some(Vector3::zero()),
                    style: style.clone(),
                },
                style,
            )
        }
    }

    impl Manipulator for FakeManipulator {
        fn id(&self) -> &str {
            "fake"
        }

        fn anchor(&self) -> Option<Vector3> {
            self.anchor
        }

        fn set_anchor(&mut self, anchor: Vector3) -> bool {
            if self.anchor == Some(anchor) {
                return false;
            }
            self.anchor = Some(anchor);
            true
        }

        fn items(&self) -> &[ShapeItem] {
            std::slice::from_ref(&self.item)
        }

        fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
            let Some(point) = ferrocad_stage::screen::project_device(camera, Vector3::zero())
            else {
                return false;
            };
            let centre = [point[0].round() as i32, point[1].round() as i32];
            (device[0] - centre[0]).abs() <= 20 && (device[1] - centre[1]).abs() <= 20
        }

        fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
            if !self.hit(camera, device) {
                return false;
            }
            self.press = Some((f64::from(device[0]), self.value));
            true
        }

        fn drag_update(&mut self, _camera: &Camera, device: [i32; 2]) -> bool {
            let Some((press_x, press_value)) = self.press else {
                return false;
            };
            let value = press_value + (f64::from(device[0]) - press_x) / 20.0;
            if (value - self.value).abs() <= 1e-12 {
                return false;
            }
            self.value = value;
            true
        }

        fn drag_end(&mut self) -> Option<InteractionCommit> {
            self.press.take()?;
            Some(
                InteractionCommit::new("fake", 7, "set")
                    .with_values([self.value])
                    .with_readout(format!("{:.2}", self.value)),
            )
        }

        fn drag_cancel(&mut self) -> bool {
            let Some((_, value)) = self.press.take() else {
                return false;
            };
            self.value = value;
            true
        }

        fn is_dragging(&self) -> bool {
            self.press.is_some()
        }

        fn set_hovered(&mut self, hovered: bool) -> bool {
            if self.hovered == hovered {
                return false;
            }
            self.hovered = hovered;
            true
        }

        fn refresh(&mut self, camera: &Camera) -> bool {
            if self.view == Some(camera.view) {
                return false;
            }
            self.view = Some(camera.view);
            self.item.set_shape(
                vec![[0.0, 0.0, 0.0], [0.5, 0.0, 0.0]],
                Vec::new(),
                Vec::new(),
                vec![0, 1],
            );
            true
        }

        fn readout(&self) -> Option<String> {
            Some(format!("{:.2}", self.value))
        }

        fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
            ferrocad_stage::screen::project_device(camera, Vector3::zero())
        }

        fn set_style(&mut self, style: &GizmoStyle) -> bool {
            let mut recorded = self.style.borrow_mut();
            let changed = recorded.as_ref() != Some(style);
            *recorded = Some(style.clone());
            changed
        }
    }

    /// A viewport with the fake manipulator at the origin (device `[50, 50]`).
    fn manipulator_view() -> (InventorViewport<FakeView>, Viewport) {
        let camera = perspective_camera([100, 100]);
        let viewport = camera.viewport;
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, Scene::default())
            .with_manipulator(Box::new(FakeManipulator::new()));
        view.refresh();
        (view, viewport)
    }

    #[test]
    fn a_manipulator_drag_commits_neutral_data() {
        let (mut view, viewport) = manipulator_view();
        assert!(view.handle_button(viewport.to_logical([50, 50]), 1, true));
        assert!(
            view.manipulator("fake")
                .expect("a manipulator")
                .is_dragging()
        );
        assert!(view.handle_pointer(viewport.to_logical([70, 50])));
        assert!(view.handle_button(viewport.to_logical([70, 50]), 1, false));

        let commit = view.last_interaction_commit().expect("a commit");
        assert_eq!(commit.manipulator, "fake");
        assert_eq!(commit.target, 7);
        assert_eq!(commit.action, "set");
        assert!(commit.values[0] > 1.0, "the drag grew the value");
        assert!(
            !view
                .manipulator("fake")
                .expect("a manipulator")
                .is_dragging()
        );
        assert!(view.take_interaction_commit().is_some());
        assert!(
            view.last_interaction_commit().is_none(),
            "taking consumes it"
        );
    }

    #[test]
    fn a_click_away_from_a_manipulator_falls_through_to_the_scene() {
        let (mut view, viewport) = manipulator_view();
        // A corner far from the grab disc.
        assert!(!view.handle_button(viewport.to_logical([5, 5]), 1, true));
        assert!(
            !view
                .manipulator("fake")
                .expect("a manipulator")
                .is_dragging()
        );
        assert!(view.orbit_drag.is_some(), "the scene gesture still starts");
        view.handle_button(viewport.to_logical([5, 5]), 1, false);
        assert!(view.last_interaction_commit().is_none());
    }

    #[test]
    fn the_manipulator_overlay_is_drawn_and_hovers() {
        let (mut view, viewport) = manipulator_view();
        assert!(!view.manipulator_queue.is_empty(), "the overlay is drawn");
        assert!(view.handle_pointer(viewport.to_logical([50, 50])));
        assert!(
            !view.handle_pointer(viewport.to_logical([50, 50])),
            "an unchanged hover is a no-op"
        );
        assert!(
            view.handle_pointer(viewport.to_logical([5, 5])),
            "moving away clears the hover"
        );
    }

    #[test]
    fn cancelling_a_manipulator_drag_ends_it_without_a_commit() {
        let (mut view, viewport) = manipulator_view();
        assert!(view.handle_button(viewport.to_logical([50, 50]), 1, true));
        assert!(view.handle_pointer(viewport.to_logical([70, 50])));
        assert!(view.cancel_manipulator_drag());
        assert!(
            !view
                .manipulator("fake")
                .expect("a manipulator")
                .is_dragging()
        );
        assert!(view.last_interaction_commit().is_none());
    }

    #[test]
    fn a_manipulator_takes_priority_over_the_gizmo() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let (mut view, viewport) = manipulator_view();
        view.set_gizmo(Some(TransformGizmo::new(
            7,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();

        // 15 px from the origin: inside the fake's grab disc, and on the gizmo's X stem.
        assert!(view.handle_button(viewport.to_logical([65, 50]), 1, true));
        assert!(
            view.manipulator("fake")
                .expect("a manipulator")
                .is_dragging(),
            "the manipulator owns the press"
        );
        assert!(!view.gizmo().expect("gizmo").is_dragging());
    }

    #[test]
    fn the_skin_also_re_materialises_hosted_manipulators() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let (mut view, _viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        view.add_manipulator(Box::new(FakeManipulator::recording().0));
        assert!(
            view.manipulator_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.is_none()),
            "the native manipulator carries no program"
        );

        // Supplying the skin re-materialises the manipulators immediately; no backend
        // switch is needed.
        assert!(view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())));
        let program = Arc::clone(view.gizmo_skin().expect("a skin program"));
        assert!(!view.manipulator_queue.packets.is_empty());
        assert!(
            view.manipulator_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.as_ref() == Some(&program)),
            "a workspace skin covers the manipulators too"
        );

        // Clearing the skin restores their native material.
        assert!(view.set_gizmo_skin(None));
        assert!(
            view.manipulator_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.is_none())
        );
    }

    #[test]
    fn set_gizmo_style_reaches_a_hosted_manipulator_and_gizmo() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let (mut view, _viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let (fake, recorded) = FakeManipulator::recording();
        view.add_manipulator(Box::new(fake));

        let mut style = GizmoStyle::default();
        style.active = [0.1, 0.2, 0.3, 1.0];
        style.size = 88.0;
        assert_ne!(style, *view.gizmo_style());
        assert!(view.set_gizmo_style(style.clone()));

        assert_eq!(view.gizmo_style(), &style);
        assert_eq!(
            recorded.borrow().as_ref(),
            Some(&style),
            "the hosted manipulator received the style"
        );
        assert_eq!(
            view.gizmo().expect("gizmo").style(),
            &style,
            "the hosted transform gizmo received the style"
        );
    }

    #[test]
    fn setting_the_same_gizmo_style_is_a_no_op() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let (mut view, _viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        view.add_manipulator(Box::new(FakeManipulator::new()));

        let mut style = view.gizmo_style().clone();
        style.opacity = 0.5;
        assert!(view.set_gizmo_style(style.clone()), "a real change reports true");
        assert!(
            !view.set_gizmo_style(style),
            "setting the stored style again changes nothing"
        );
    }

    #[test]
    fn a_gizmo_or_manipulator_created_after_a_style_change_adopts_it() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        let (mut view, _viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let mut style = GizmoStyle::default();
        style.inactive = [0.2, 0.4, 0.6, 1.0];
        style.size = 64.0;
        assert!(view.set_gizmo_style(style.clone()));

        let (fake, recorded) = FakeManipulator::recording();
        view.add_manipulator(Box::new(fake));
        assert_eq!(
            recorded.borrow().as_ref(),
            Some(&style),
            "a manipulator created later adopts the stored style"
        );

        view.set_gizmo(Some(TransformGizmo::new(
            43,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        assert_eq!(
            view.gizmo().expect("gizmo").style(),
            &style,
            "a gizmo created later adopts the stored style"
        );
    }

    /// A tiny WGSL source for the skin tests. The viewport never compiles it (it only
    /// packages the source into a program); the renderer is the sole compiler.
    const TEST_SKIN_WGSL: &str =
        "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }";

    fn translate_gizmo_view() -> (InventorViewport<FakeView>, ferrocad_graphics::Viewport) {
        use ferrocad_stage::{GizmoMode, TransformGizmo};
        gizmo_view(TransformGizmo::new(42, Vector3::zero(), GizmoMode::Translate))
    }

    fn set_gizmo_backend(view: &mut InventorViewport<FakeView>, backend: &str) {
        let mut style = view.gizmo_style().clone();
        style.backend = backend.to_string();
        assert!(view.set_gizmo_style(style), "a backend change is a real change");
    }

    /// The first device position whose analytic hit test lands on the X axis.
    fn axis_x_grab(view: &InventorViewport<FakeView>) -> [i32; 2] {
        use ferrocad_stage::{GizmoHandle, GizmoTarget};
        let gizmo = view.gizmo().expect("a gizmo");
        for x in 0..100 {
            for y in 0..100 {
                if gizmo.hit(&view.camera, [x, y]) == Some(GizmoTarget::Handle(GizmoHandle::AxisX))
                {
                    return [x, y];
                }
            }
        }
        panic!("the X axis handle should be pickable");
    }

    /// Deferred mode reports a drag's `Begin`/`Update`/`End` instead of committing,
    /// so a host HUD can mirror the delta and own the commit.
    #[test]
    fn a_deferred_drag_reports_previews_and_never_commits() {
        let (mut view, viewport) = translate_gizmo_view();
        let grab = axis_x_grab(&view);
        let release = [grab[0] + 12, grab[1]];

        view.set_gizmo_preview(true);
        assert!(view.gizmo_preview());
        view.handle_button(viewport.to_logical(grab), 1, true);
        let begin = view.take_transform_preview().expect("a begin");
        assert_eq!(begin.phase, PreviewPhase::Begin);
        assert_eq!(begin.total_transform, Matrix4::identity());

        view.handle_pointer(viewport.to_logical(release));
        let update = view.take_transform_preview().expect("an update");
        assert_eq!(update.phase, PreviewPhase::Update);
        assert!(
            update.total_transform.m[3].abs() > 1e-9,
            "the drag moved along X: {update:?}"
        );
        assert_eq!(
            view.gizmo().expect("a gizmo").origin(),
            Vector3::zero(),
            "the interaction frame stays at the grab point"
        );
        assert!(
            view.gizmo().expect("a gizmo").display_offset().is_some(),
            "the drawn triad is glued to the moved surface"
        );

        view.handle_button(viewport.to_logical(release), 1, false);
        let end = view.take_transform_preview().expect("an end");
        assert_eq!(end.phase, PreviewPhase::End);
        assert_eq!(end.total_transform, update.total_transform);
        assert!(
            view.take_gizmo_commit().is_none(),
            "the host owns the commit"
        );

        // The host applies its accumulated delta: the frozen frame moves to the new
        // surface and the draw-only offset goes away.
        let delta = [
            end.total_transform.m[3],
            end.total_transform.m[7],
            end.total_transform.m[11],
        ];
        assert!(view.preview_translation(&[42], delta));
        assert!(view.gizmo().expect("a gizmo").display_offset().is_none());
        assert_eq!(
            view.gizmo().expect("a gizmo").origin(),
            Vector3::new(delta[0], delta[1], delta[2])
        );

        // A second drag on the same deferred HUD: the host sends the accumulated delta
        // back as the new baseline, so the frame must not drift by it again. The draw
        // offset is measured from where the frame already is, not from `delta`.
        let grab2 = axis_x_grab(&view);
        view.handle_button(viewport.to_logical(grab2), 1, true);
        let begin2 = view.take_transform_preview().expect("a second begin");
        assert_eq!(begin2.phase, PreviewPhase::Begin);
        assert!(view.preview_translation(&[42], delta));
        assert_eq!(
            view.gizmo().expect("a gizmo").origin(),
            Vector3::new(delta[0], delta[1], delta[2]),
            "the frame stays where the first drag left it"
        );
        let offset = view
            .gizmo()
            .expect("a gizmo")
            .display_offset()
            .expect("a draw offset");
        assert!(
            offset.length() < 1e-9,
            "the accumulated baseline is not applied twice: {offset:?}"
        );
        view.handle_button(viewport.to_logical(grab2), 1, false);
        assert!(view.preview_translation(&[], [0.0; 3]));

        // Leaving deferred mode restores the ordinary commit path.
        view.set_gizmo_preview(false);
        assert!(!view.gizmo_preview());
        view.handle_button(viewport.to_logical(grab), 1, true);
        view.handle_pointer(viewport.to_logical(release));
        assert!(
            view.gizmo().expect("a gizmo").display_offset().is_some(),
            "even a normal drag draws the triad on the surface"
        );
        view.handle_button(viewport.to_logical(release), 1, false);
        assert!(
            view.gizmo().expect("a gizmo").display_offset().is_none(),
            "the release restores the frozen drawing"
        );
        assert!(view.take_gizmo_commit().is_some(), "a normal drag commits");
        assert!(view.take_transform_preview().is_none());
    }

    #[test]
    fn a_typed_preview_moves_the_gizmo_and_the_grips() {
        let (mut view, _viewport) = translate_gizmo_view();
        view.add_manipulator(Box::new(FakeManipulator::new()));
        view.refresh();
        let gizmo_before = view.gizmo().expect("a gizmo").origin();
        let grip_before = view
            .manipulator("fake")
            .expect("the grip")
            .anchor()
            .expect("an anchor");
        let delta = Vector3::new(5.0, 2.0, 0.0);

        assert!(view.preview_translation(&[42], [delta.x, delta.y, delta.z]));
        assert_eq!(
            view.gizmo().expect("a gizmo").origin(),
            gizmo_before.add(&delta)
        );
        assert_eq!(
            view.manipulator("fake").expect("the grip").anchor(),
            Some(grip_before.add(&delta)),
            "the grip follows the previewed body"
        );

        // Cancelling restores both to the document pose.
        assert!(view.preview_translation(&[], [0.0; 3]));
        assert_eq!(view.gizmo().expect("a gizmo").origin(), gizmo_before);
        assert_eq!(
            view.manipulator("fake").expect("the grip").anchor(),
            Some(grip_before)
        );
    }

    #[test]
    fn a_supplied_skin_re_materialises_the_gizmo_packets() {
        use ferrocad_stage::SKIN_BACKEND;

        let (mut view, _viewport) = translate_gizmo_view();
        // The shell names the provider the skin came from, but the viewport keys
        // skinning on the supplied program alone.
        set_gizmo_backend(&mut view, SKIN_BACKEND);
        let handles = view.gizmo().expect("a gizmo").items().len();
        let native_packets = view.gizmo_queue.len();
        assert!(
            native_packets >= handles,
            "the native queue covers the handle set ({native_packets} packets, {handles} handles)"
        );
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.is_none()),
            "a viewport with no skin draws the native packets"
        );

        // Supplying a skin re-materialises every handle packet immediately, without
        // losing one and without a backend switch.
        assert!(view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())));
        let program = Arc::clone(view.gizmo_skin().expect("a skin program"));
        assert_eq!(
            view.gizmo_queue.len(),
            native_packets,
            "the skin keeps the native packet count"
        );
        let skinned = view
            .gizmo_queue
            .packets
            .iter()
            .filter(|packet| packet.state.id != PickId(0))
            .collect::<Vec<_>>();
        assert!(!skinned.is_empty(), "there are handle packets to skin");
        assert!(
            skinned
                .iter()
                .all(|packet| packet.state.program.as_ref() == Some(&program)),
            "every handle packet carries the skin"
        );
        // The outline pass is the contrast aid, so a skin must not be able to remove it.
        let halos = view
            .gizmo_queue
            .packets
            .iter()
            .filter(|packet| packet.state.id == PickId(0))
            .collect::<Vec<_>>();
        assert!(!halos.is_empty(), "the halo pass is drawn");
        assert!(
            halos.iter().all(|packet| packet.state.program.is_none()),
            "the halo stays native under a skin"
        );

        // Clearing the skin restores the native packets.
        assert!(view.set_gizmo_skin(None));
        assert_eq!(view.gizmo_queue.len(), native_packets);
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.is_none()),
            "the native look is restored"
        );
    }

    #[test]
    fn the_gizmo_halo_draws_behind_the_handles() {
        let (view, _viewport) = translate_gizmo_view();
        let halo = view
            .gizmo_queue
            .packets
            .iter()
            .position(|packet| packet.state.id == PickId(0))
            .expect("the halo pass is drawn");
        let handle = view
            .gizmo_queue
            .packets
            .iter()
            .position(|packet| packet.state.id != PickId(0))
            .expect("the handles are drawn");
        assert!(
            halo < handle,
            "the halo is a background pass and must precede the full-width handles"
        );
    }

    #[test]
    fn setting_the_same_gizmo_skin_is_a_no_op() {
        let (mut view, _viewport) = translate_gizmo_view();
        assert!(view.gizmo_skin().is_none());

        assert!(view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())));
        assert!(
            !view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())),
            "the same source is the same program"
        );
        assert!(
            view.set_gizmo_skin(Some("@vertex fn vs_main() {}".into())),
            "a different source is a new program"
        );
        assert!(view.set_gizmo_skin(None), "clearing a stored program is a change");
        assert!(!view.set_gizmo_skin(None), "clearing again changes nothing");
        assert!(view.gizmo_skin().is_none());
    }

    #[test]
    fn the_skin_falls_back_when_it_is_missing_or_disabled() {
        let (mut view, _viewport) = translate_gizmo_view();
        assert!(!view.gizmo_queue.is_empty(), "the handles still draw");
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .all(|packet| packet.state.program.is_none()),
            "no supplied program means the native packets"
        );

        assert!(view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())));
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .filter(|packet| packet.state.id != PickId(0))
                .all(|packet| packet.state.program.is_some()),
            "the skin is drawn once it exists"
        );
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .filter(|packet| packet.state.id == PickId(0))
                .all(|packet| packet.state.program.is_none()),
            "the halo stays native"
        );

        let mut style = view.gizmo_style().clone();
        style.enabled = false;
        assert!(view.set_gizmo_style(style));
        assert!(view.gizmo_queue.is_empty(), "a disabled style draws nothing");

        let mut style = view.gizmo_style().clone();
        style.enabled = true;
        assert!(view.set_gizmo_style(style));
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .filter(|packet| packet.state.id != PickId(0))
                .all(|packet| packet.state.program.is_some()),
            "the stored skin resumes with the style"
        );
    }

    #[test]
    fn a_skinned_gizmo_still_hits_and_commits_through_the_native_gizmo() {
        use ferrocad_stage::{GizmoHandle, GizmoTarget};

        let (mut view, viewport) = translate_gizmo_view();
        assert!(view.set_gizmo_skin(Some(TEST_SKIN_WGSL.into())));
        assert!(
            view.gizmo_queue
                .packets
                .iter()
                .filter(|packet| packet.state.id != PickId(0))
                .all(|packet| packet.state.program.is_some()),
            "the drag runs against a skinned queue"
        );

        // The analytic hit test is unchanged by the skin.
        let grab = axis_x_grab(&view);
        assert_eq!(
            view.gizmo_target_at(grab),
            Some(GizmoTarget::Handle(GizmoHandle::AxisX)),
            "the native gizmo still owns picking"
        );

        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(view.gizmo().expect("gizmo").is_dragging());
        assert!(view.handle_pointer(viewport.to_logical([grab[0] + 12, grab[1]])));
        assert!(preview(&view).is_some(), "the drag previews the target");
        assert!(view.gizmo_value().is_some(), "the readout still tracks the drag");
        assert!(view.last_gizmo_commit().is_none());

        assert!(view.handle_button(viewport.to_logical([grab[0] + 12, grab[1]]), 1, false));
        assert!(
            view.last_gizmo_commit().is_some(),
            "the native gizmo still commits"
        );
        assert!(preview(&view).is_none(), "the preview is cleared on commit");
    }

    #[test]
    fn a_disabled_style_hides_and_unpicks_the_overlays() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};

        // An empty scene, so any hover report can only come from an overlay.
        let (mut view, viewport) = manipulator_view();
        view.set_gizmo(Some(TransformGizmo::new(
            7,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();
        assert!(!view.manipulator_queue.is_empty());
        assert!(!view.gizmo_queue.is_empty());

        let mut style = view.gizmo_style().clone();
        style.enabled = false;
        assert!(view.set_gizmo_style(style.clone()));

        assert!(view.manipulator_queue.is_empty(), "no overlay is drawn");
        assert!(view.gizmo_queue.is_empty(), "no gizmo is drawn");
        assert_eq!(view.manipulators().len(), 1, "the manipulator stays hosted");
        assert!(view.gizmo().is_some(), "the gizmo stays hosted");

        // A hover over the hosted grab disc falls through to the (empty) scene.
        view.handle_pointer(viewport.to_logical([50, 50]));
        assert_eq!(view.hover_report(), &HoverReport::default());
        assert!(view.hovered().is_none());

        // A press over it likewise falls through: neither overlay starts a drag.
        assert!(!view.handle_button(viewport.to_logical([50, 50]), 1, true));
        assert!(
            !view
                .manipulator("fake")
                .expect("a manipulator")
                .is_dragging()
        );
        assert!(!view.gizmo().expect("gizmo").is_dragging());
        assert!(view.orbit_drag.is_some(), "the press reached the scene");

        // Re-enabling restores the overlays without re-arming the selection.
        style.enabled = true;
        assert!(view.set_gizmo_style(style));
        assert!(!view.manipulator_queue.is_empty());
        assert!(!view.gizmo_queue.is_empty());
        assert_eq!(view.manipulators().len(), 1);
    }

    #[test]
    fn hiding_the_readout_suppresses_the_pill_text() {
        let (mut view, _viewport) = manipulator_view();
        assert_eq!(view.readout_text().as_deref(), Some("1.00"));

        let mut style = view.gizmo_style().clone();
        style.show_readout = false;
        assert!(view.set_gizmo_style(style));
        assert_eq!(
            view.readout_text(),
            None,
            "the readout helper yields no pill text"
        );
        assert_eq!(
            view.manipulator("fake")
                .expect("a manipulator")
                .readout()
                .as_deref(),
            Some("1.00"),
            "the hosted manipulator still reports its value"
        );
    }

    #[test]
    fn the_escape_key_cancels_a_gizmo_drag() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};
        use ferrocad_types::Vector3;

        let (mut view, viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let grab = axis_x_grab(&view);
        let release = [grab[0] + 12, grab[1]];
        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(view.handle_pointer(viewport.to_logical(release)));
        assert!(preview(&view).is_some(), "the drag previews a transform");
        assert!(view.gizmo_value().is_some());

        let escape = KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: "escape".to_string(),
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        };
        let other = KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: "a".to_string(),
                ..Default::default()
            },
            ..escape.clone()
        };
        assert!(!view.handle_key(&other), "only Escape cancels");
        assert!(view.handle_key(&escape));
        assert!(!view.gizmo().expect("gizmo").is_dragging());
        assert!(preview(&view).is_none(), "the preview is restored");
        assert!(view.gizmo_value().is_none());
        assert!(view.last_gizmo_commit().is_none());
    }

    #[test]
    fn the_gizmo_readout_tracks_the_drag_and_clears_on_release() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};
        use ferrocad_types::Vector3;

        let (mut view, viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let grab = axis_x_grab(&view);
        let release = [grab[0] + 12, grab[1]];
        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(view.handle_pointer(viewport.to_logical(release)));
        let value = view.gizmo_value().expect("a readout value");
        assert!(matches!(value, GizmoValue::Linear { .. }));
        assert!(value.readout().contains("ΔX"), "{}", value.readout());

        assert!(view.handle_button(viewport.to_logical(release), 1, false));
        assert!(view.gizmo_value().is_none(), "the tag clears on release");
        let commit = view.last_gizmo_commit().expect("a commit");
        assert!(matches!(commit.value, GizmoValue::Linear { .. }));
        assert!(commit.value.readout().contains("ΔX"));

        // The host route: take it, then convert it to the neutral commit.
        let commit = view.take_gizmo_commit().expect("a taken commit");
        let interaction = commit.to_interaction();
        assert_eq!(interaction.manipulator, ferrocad_stage::TRANSFORM_ID);
        assert_eq!(interaction.action, "apply");
        assert_eq!(interaction.values.len(), 16);
        assert!(view.last_gizmo_commit().is_none(), "taking consumes it");
    }

    #[test]
    fn shift_reaches_the_gizmo_as_a_planar_drag() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};
        use ferrocad_types::Vector3;

        let (mut view, viewport) = gizmo_view(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        ));
        let shift = GizmoModifiers {
            shift: true,
            ..Default::default()
        };
        let grab = axis_x_grab(&view);
        assert!(view.handle_button_with_modifiers(viewport.to_logical(grab), 1, true, shift));
        assert!(view.handle_pointer_with_modifiers(
            viewport.to_logical([grab[0], grab[1] + 15]),
            GizmoModifiers::default()
        ));
        assert!(
            matches!(view.gizmo_value(), Some(GizmoValue::Planar { .. })),
            "Shift on an axis starts a plane drag"
        );
    }

    #[test]
    fn gpui_modifiers_map_to_the_gizmo_flags() {
        let shift = gizmo_modifiers(gpui::Modifiers {
            shift: true,
            ..Default::default()
        });
        assert!(shift.shift && !shift.ctrl);

        let mut secondary = gpui::Modifiers::default();
        #[cfg(target_os = "macos")]
        {
            secondary.platform = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            secondary.control = true;
        }
        assert!(gizmo_modifiers(secondary).ctrl);
    }

    /// An empty inventor scene, for a viewport that draws nothing but the grid.
    fn empty_scene() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![],
            },
        }
    }

    /// An identity-camera viewport with the reference grid on.
    fn grid_view() -> InventorViewport<FakeView> {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(fake(ImageData::default()), camera, empty_scene());
        assert!(viewport.set_grid(true));
        viewport
    }

    #[test]
    fn the_grid_draws_but_is_never_picked() {
        let mut viewport = grid_view();

        // Five packets: the dots, the major lines, the triad ribbons, the occluded
        // triad ghost, and the origin node.
        let topologies: Vec<Topology> = viewport
            .grid_queue
            .packets
            .iter()
            .map(|packet| packet.topology)
            .collect();
        assert_eq!(
            topologies,
            vec![
                Topology::Points,
                Topology::Lines,
                Topology::Triangles,
                Topology::Triangles,
                Topology::Lines
            ]
        );

        // The grid is scaffolding, not geometry: it lives outside the scene queue and
        // carries the untagged id, so no pick path can reach it.
        assert!(viewport.queue.is_empty());
        assert!(
            viewport
                .grid_queue
                .packets
                .iter()
                .all(|packet| packet.state.id == PickId(0))
        );

        // The origin is a dot under the pointer, and a click there resolves to nothing.
        assert!(viewport.pick([50, 50]).is_none());
    }

    /// The native grid dims with the same edge-on law as the Coin shader: a plane seen
    /// along its own surface stops drawing instead of becoming a wall of lines.
    #[test]
    fn the_native_grid_dims_when_the_plane_is_edge_on() {
        let mut view = slab_view();
        view.camera = perspective_camera([100, 100]);
        view.refresh();
        assert!(
            max_alpha(&view.grid_queue.packets[0]) > 0.0,
            "the face-on grid is drawn"
        );

        // Keep the camera's projection and viewport; only turn it to look along `−X`,
        // which lies in the world `XY` plane.
        let mut camera = view.camera;
        camera.view = ferrocad_types::Matrix4 {
            m: [
                0.0, 1.0, 0.0, 0.0, // right = +Y
                0.0, 0.0, 1.0, 0.0, // up    = +Z
                1.0, 0.0, 0.0, 0.0, // back  = +X
                0.0, 0.0, 0.0, 1.0,
            ],
        };
        view.camera = camera;
        view.refresh();
        assert_eq!(
            max_alpha(&view.grid_queue.packets[0]),
            0.0,
            "a grid seen along its own plane is not drawn"
        );
    }

    #[test]
    fn set_grid_overlay_hides_the_native_ground_but_keeps_the_axis_cross() {
        let mut viewport = grid_view();
        assert!(!viewport.grid_overlay(), "the native grid is authoritative");
        assert_eq!(viewport.grid_queue.len(), 5);

        assert!(viewport.set_grid_overlay(true));
        assert!(viewport.grid_overlay());
        assert_eq!(
            viewport.grid_queue.len(),
            3,
            "the world ground is hidden; the triad, its ghost and the origin node remain"
        );
        assert!(!viewport.set_grid_overlay(true), "enabling twice is a no-op");

        assert!(viewport.set_grid_overlay(false));
        assert_eq!(viewport.grid_queue.len(), 5);
    }

    /// The authored grid replaces the native ground on a **bound** plane too, not only on
    /// the world one. Missing this drew the Coin grid and the built-in face patch on top
    /// of each other whenever a workplane was active.
    #[test]
    fn the_overlay_flag_hides_a_bound_plane_grid_too() {
        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane(), "a face plane is active");
        view.refresh();
        let native = view.grid_queue.len();
        assert!(native > 0, "the bound plane draws its grid");

        assert!(view.set_grid_overlay(true));
        let under_overlay = view.grid_queue.len();
        assert!(
            under_overlay < native,
            "the bound plane's ground steps aside: {under_overlay} of {native}"
        );
        assert!(
            under_overlay > 0,
            "the triad and the origin node stay under the overlay"
        );
    }

    /// A scene whose only draw is a program packet (the program binds to the quad that
    /// follows it, but the renderer draws the placeholder instead).
    fn program_scene() -> Scene {
        use ferrocad_inventor::{ShaderStage, ShaderStageKind};
        let leaf = |kind| Node {
            kind,
            children: Vec::new(),
        };
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    leaf(NodeKind::Coordinate3(vec![
                        [-1.0, -1.0, 0.0],
                        [1.0, -1.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [-1.0, 1.0, 0.0],
                    ])),
                    Node {
                        kind: NodeKind::ShaderProgram,
                        children: vec![
                            leaf(NodeKind::ShaderStage(ShaderStage {
                                kind: ShaderStageKind::Vertex,
                                source: Some(
                                    "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }"
                                        .to_owned(),
                                ),
                                source_program: None,
                            })),
                            leaf(NodeKind::ShaderStage(ShaderStage {
                                kind: ShaderStageKind::Fragment,
                                source: Some(String::new()),
                                source_program: None,
                            })),
                        ],
                    },
                    leaf(NodeKind::Mesh {
                        indices: vec![0, 1, 2, 0, 2, 3],
                        normal_indices: Vec::new(),
                        topology: Topology::Triangles,
                    }),
                ],
            },
        }
    }

    /// G5's visibility policy: registering an overlay is enough to draw it. The
    /// `grid_overlay` flag only hides the native ground, so a program packet is drawn
    /// even when the flag is off (the state a freshly booted overlay starts in).
    #[test]
    fn a_registered_program_overlay_draws_without_the_grid_overlay_flag() {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let camera = Camera {
            viewport: Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(
            RecordingView {
                frames: frames.clone(),
            },
            camera,
            empty_scene(),
        );
        viewport.set_overlays(vec![program_scene()]);
        assert!(!viewport.grid_overlay(), "the flag is off by default");

        viewport.refresh();
        let first = frames.borrow().last().cloned().expect("a frame");
        assert_eq!(
            first,
            vec![0, 3, 1],
            "the overlay draws beside the empty scene and the axis cross (front + ghost), flag off"
        );

        // The flag stays meaningful: the native ground is independent of the overlay.
        viewport.set_grid(true);
        viewport.refresh();
        let second = frames.borrow().last().cloned().expect("a frame");
        assert_eq!(
            second,
            vec![0, 5, 1],
            "the native ground draws with the flag off; the overlay is still there"
        );

        // ...and it hides the ground (triad and origin remain), never the overlay.
        assert!(viewport.set_grid_overlay(true));
        viewport.refresh();
        let third = frames.borrow().last().cloned().expect("a frame");
        assert_eq!(
            third,
            vec![0, 3, 1],
            "the world ground is hidden (triad + ghost remain); the registered overlay still draws"
        );
    }

    #[test]
    fn a_program_packet_is_kept_out_of_the_scene_queue_and_the_pick() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, program_scene());

        // The program draws a placeholder, not its authored geometry: it must not sit in
        // the scene queue (which the pick path reads) or be pickable.
        assert!(viewport.queue.is_empty());
        assert_eq!(viewport.program_queue.len(), 1);
        assert!(viewport.program_queue.packets[0].state.program.is_some());
        assert!(!viewport.program_queue.packets[0].state.pickable);
        assert!(viewport.pick([50, 50]).is_none());

        // The flag does not partition the queues: it only hides the native ground.
        assert!(viewport.set_grid_overlay(true));
        assert_eq!(viewport.program_queue.len(), 1);
        assert!(viewport.queue.is_empty());
    }

    /// G5: an overlay scene registered through the bridge is merged beside the base
    /// scene. Its program packet lands in `program_queue`; the base scene keeps its
    /// own packet (and is not mutated by the merge).
    #[test]
    fn overlays_merge_beside_the_base_scene_without_replacing_it() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());
        assert_eq!(viewport.queue.len(), 1, "the base triangle draws");
        assert!(viewport.program_queue.is_empty());

        viewport.set_overlays(vec![program_scene()]);
        assert_eq!(viewport.queue.len(), 1, "the base scene survives the merge");
        assert_eq!(
            viewport.program_queue.len(),
            1,
            "the overlay's program packet is held aside for the overlay pass"
        );
        assert!(viewport.program_queue.packets[0].state.program.is_some());
        assert_eq!(
            viewport.scene().root.children.len(),
            3,
            "the base scene is untouched"
        );
        assert_eq!(viewport.overlays().len(), 1);

        viewport.set_overlays(Vec::new());
        assert_eq!(viewport.queue.len(), 1);
        assert!(viewport.program_queue.is_empty(), "removing drops the overlay");
        assert!(viewport.overlays().is_empty());
    }

    /// The footprints follow the active plane. A world-`(x, y)` footprint projected onto
    /// `XZ` used to land beside the slab; now the producer measures the item in the
    /// plane's own `(u, v)`.
    #[test]
    fn a_bound_workplane_casts_a_footprint_in_its_own_frame() {
        let mut view = slab_view();
        let ground = view.shadow_footprints();
        assert!(!ground.is_empty(), "the world ground casts the slab's shadow");

        assert!(view.set_standard_plane(ferrocad_stage::StandardPlane::Front));
        assert!(
            !view.shadow_footprints().is_empty(),
            "a vertical plane casts its own footprint"
        );
    }

    #[test]
    fn the_stage_supplies_the_program_shadow_block() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(fake(ImageData::default()), camera, empty_scene());
        viewport.set_overlays(vec![program_scene()]);

        let program_parameters = |viewport: &InventorViewport<FakeView>| {
            *viewport.program_queue.packets[0]
                .state
                .program
                .as_ref()
                .expect("the overlay's program")
                .parameters()
        };

        // No stage geometry: the block is clear, so the shader's loop does not run.
        let parameters = program_parameters(&viewport);
        assert_eq!(parameters.shadow_count, 0.0);
        assert!(parameters.shadow_data.iter().flatten().all(|value| *value == 0.0));

        let box_at = |entity: u32, x: f32| {
            ShapeItem::new(entity).with_shape(
                vec![
                    [x, 0.0, 0.0],
                    [x + 2.0, 0.0, 0.0],
                    [x + 2.0, 2.0, 0.0],
                    [x, 2.0, 0.0],
                ],
                vec![],
                vec![],
                vec![],
            )
        };
        viewport.set_stage(Stage::new().with_item(box_at(1, -5.0)).with_item(box_at(2, 3.0)));
        // `set_stage` marks the stage dirty; the next paint recompiles.
        viewport.refresh();

        // Two separated boxes -> two populated slots, in world XY, in stage order.
        let parameters = program_parameters(&viewport);
        assert_eq!(parameters.shadow_count, 2.0, "the shader reads counts.x");
        assert_eq!(parameters.shadow_data[0], [-4.0, 1.0, 1.0, 1.0]);
        assert_eq!(parameters.shadow_data[1][0], 0.25, "slot 0's blur");
        assert_eq!(&parameters.shadow_data[1][1..3], &[4.0, 1.0], "slot 1 centre");
        assert_eq!(parameters.shadow_data[1][3], 1.0, "slot 1 half x");
        assert_eq!(parameters.shadow_data[2][0], 1.0, "slot 1 half y");
        assert_eq!(parameters.shadow_data[2][1], 0.25, "slot 1 blur");

        // A host that owns the document bounds can replace the derived list (the
        // legacy inventor path has no `Stage`).
        assert!(viewport.set_shadow_footprints(vec![ShadowFootprint::from_bounds(
            [0.0, 0.0],
            [4.0, 4.0],
            1.0
        )]));
        let parameters = program_parameters(&viewport);
        assert_eq!(parameters.shadow_count, 1.0);
        assert_eq!(parameters.shadow_data[0], [2.0, 2.0, 2.0, 2.0]);
        assert_eq!(parameters.shadow_data[1][0], 1.0);
        assert!(!viewport.set_shadow_footprints(vec![ShadowFootprint::from_bounds(
            [0.0, 0.0],
            [4.0, 4.0],
            1.0
        )]), "an identical override is a no-op");

        // Clearing returns to the live stage bounds.
        assert!(viewport.clear_shadow_footprints());
        assert_eq!(program_parameters(&viewport).shadow_count, 2.0);
        assert!(!viewport.clear_shadow_footprints(), "already stage-derived");
    }

    #[test]
    fn set_grid_toggles_the_grid_queue() {
        let mut viewport = grid_view();
        assert!(viewport.grid_enabled());
        assert_eq!(
            viewport.grid_queue.len(),
            5,
            "the ground (dots, lines), the triad (front + ghost) and the origin node"
        );

        assert!(!viewport.set_grid(true), "enabling twice is a no-op");

        // The axis cross is separate: hiding the grid leaves the triad and its node.
        assert!(viewport.set_grid(false));
        assert!(!viewport.grid_enabled());
        assert_eq!(viewport.grid_queue.len(), 3);
        assert!(viewport.set_axis_cross(false));
        assert!(viewport.grid_queue.is_empty());
    }

    #[test]
    fn zooming_keeps_the_pivot_inside_the_clip_window() {
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
        };
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, empty_scene());
        view.set_grid(true);
        let clip = |view: &InventorViewport<FakeView>| {
            let Projection::Perspective { near, far, .. } = view.camera().projection else {
                panic!("a perspective camera")
            };
            (view.camera().eye_distance(), near, far)
        };

        // Zoom right in: the near plane follows instead of clipping at its old value.
        for _ in 0..20 {
            view.handle_scroll(3.0);
        }
        let (distance, near, far) = clip(&view);
        assert!(near < distance, "near {near} vs distance {distance}");
        assert!(far > distance, "far {far} vs distance {distance}");

        // And right out: the far plane follows instead of clipping the scene away.
        for _ in 0..60 {
            view.handle_scroll(-3.0);
        }
        let (distance, near, far) = clip(&view);
        assert!(near < distance && far > distance, "{near}..{far} at {distance}");
    }

    #[test]
    fn framing_a_large_stage_pulls_the_camera_back() {
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
        };
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, empty_scene());

        // The toy's slab: 40 x 40 x 20, base at z = 0.
        let mut stage = ferrocad_stage::Stage::new();
        stage.add_item(ferrocad_stage::ShapeItem::new(1).with_shape(
            vec![
                [-20.0, -20.0, 0.0],
                [20.0, -20.0, 0.0],
                [20.0, 20.0, 0.0],
                [-20.0, 20.0, 0.0],
                [-20.0, -20.0, 20.0],
                [20.0, -20.0, 20.0],
                [20.0, 20.0, 20.0],
                [-20.0, 20.0, 20.0],
            ],
            Vec::new(),
            vec![0, 1, 2, 0, 2, 3],
            Vec::new(),
        ));
        view.set_stage(stage);

        assert!(view.frame_stage());
        let distance = view.camera().eye_distance();
        assert!(distance > 34.0, "outside the bounding sphere: {distance}");
        let Projection::Perspective { near, far, .. } = view.camera().projection else {
            panic!("a perspective camera")
        };
        assert!(near < distance && far > distance, "{near}..{far}");
    }

    #[test]
    fn the_axis_cross_is_independent_of_the_grid() {
        let mut viewport = grid_view();
        assert!(viewport.axis_cross(), "on by default, as upstream's is");

        // Cross alone: the triad, its ghost, then the origin node (drawn last).
        assert!(viewport.set_grid(false));
        let topologies: Vec<Topology> = viewport
            .grid_queue
            .packets
            .iter()
            .map(|packet| packet.topology)
            .collect();
        assert_eq!(
            topologies,
            vec![
                Topology::Triangles,
                Topology::Triangles,
                Topology::Lines
            ]
        );
        assert_eq!(
            viewport.axis_labels().len(),
            3,
            "the axis tags belong to the cross"
        );

        // Neither: nothing at all.
        assert!(viewport.set_axis_cross(false));
        assert!(viewport.grid_queue.is_empty());
        assert!(viewport.axis_labels().is_empty());

        // Grid alone: no triad and no tags.
        assert!(viewport.set_grid(true));
        assert_eq!(viewport.grid_queue.len(), 2, "dots and major lines");
        assert!(viewport.axis_labels().is_empty());
    }

    #[test]
    fn an_axis_overlay_hides_the_native_triad_origin_and_labels() {
        let mut viewport = grid_view();
        // Cross alone: the lit triad, its ghost, then the origin node (drawn last).
        assert!(viewport.set_grid(false));
        assert_eq!(viewport.grid_queue.len(), 3);
        assert_eq!(viewport.axis_labels().len(), 3);

        // A `view.axis.overlay` claim is a visibility flag, not geometry: the style is
        // otherwise unchanged and the user's `enabled` switch stays on.
        let mut style = viewport.axis_style().clone();
        style.overlay = true;
        assert!(viewport.set_axis_style(style.clone()));
        assert!(viewport.axis_cross(), "the switch is untouched");
        assert!(
            viewport.grid_queue.is_empty(),
            "no native triad, ghost or origin node"
        );
        assert!(viewport.axis_labels().is_empty(), "no native axis tags");

        // Clearing the claim restores the native drawing.
        style.overlay = false;
        assert!(viewport.set_axis_style(style));
        let topologies: Vec<Topology> = viewport
            .grid_queue
            .packets
            .iter()
            .map(|packet| packet.topology)
            .collect();
        assert_eq!(
            topologies,
            vec![
                Topology::Triangles,
                Topology::Triangles,
                Topology::Lines
            ]
        );
        assert_eq!(viewport.axis_labels().len(), 3);
    }

    #[test]
    fn a_custom_axis_style_reaches_the_triad_the_ghost_and_the_labels() {
        let mut viewport = grid_view();
        // Cross alone: the triad, its ghost, then the origin node.
        assert!(viewport.set_grid(false));
        let mut style = viewport.axis_style().clone();
        style.colors[0] = [0.1, 0.2, 0.3, 1.0];
        style.opacity = 0.4;
        style.length = 40.0;
        style.width = 3.0;
        style.ghost_alpha = 0.25;
        assert!(viewport.set_axis_style(style.clone()));
        assert_eq!(
            viewport.grid_queue.packets[0].topology,
            Topology::Triangles,
            "the lit triad"
        );
        let lit = viewport.grid_queue.packets[0]
            .geometry
            .as_ref()
            .expect("fresh geometry");
        assert!((lit.vertices[0].color[0] - 0.1).abs() < 1e-6);
        assert!((lit.vertices[0].color[1] - 0.2).abs() < 1e-6);
        assert!((lit.vertices[0].color[2] - 0.3).abs() < 1e-6);
        assert!((max_alpha(&viewport.grid_queue.packets[0]) - 0.4).abs() < 1e-6);
        // The occluded pass uses the style's ghost alpha, not the old token.
        assert!((max_alpha(&viewport.grid_queue.packets[1]) - 0.1).abs() < 1e-6);

        let labels = viewport.axis_labels();
        assert_eq!(labels[0].color, packed_srgb(style.colors[0]));
        assert_eq!(labels[0].alpha, 102, "0.4 as an 8-bit alpha");

        // The style's visibility flags own the origin node and the tags.
        style.origin = false;
        style.labels = false;
        assert!(viewport.set_axis_style(style));
        assert!(viewport.axis_labels().is_empty());
        assert!(
            viewport
                .grid_queue
                .packets
                .iter()
                .all(|packet| packet.topology != Topology::Lines),
            "no origin node"
        );
    }

    #[test]
    fn an_orbit_rebuilds_the_triad_but_not_the_ground() {
        let mut viewport = grid_view();
        // A fixed extent, so the disc (which follows the camera's eye distance) cannot be
        // what changes the ground: this test is about the camera *not* moving the lattice.
        assert!(viewport.set_grid_radius(Some(500.0)));
        let generations = |viewport: &InventorViewport<FakeView>| {
            let packets = &viewport.grid_queue.packets;
            (
                packets[0].generation,
                packets[1].generation,
                packets[2].generation,
            )
        };
        let (ground, lines, triad) = generations(&viewport);

        viewport.camera.view =
            Placement::new(Vector3::new(0.0, 0.0, -5.0), Rotation::identity()).to_matrix();
        viewport.recompile();

        let (ground_after, lines_after, triad_after) = generations(&viewport);
        assert_eq!(ground, ground_after, "the dot field is camera-independent");
        assert_eq!(lines, lines_after, "the major lines are camera-independent");
        assert_ne!(
            triad, triad_after,
            "the screen-facing triad follows the camera"
        );
    }

    /// The identity camera with the named slab (`Face1`/`Face2` at `z = 0.5`) and the
    /// grid on.
    fn slab_view() -> InventorViewport<FakeView> {
        let mut view = identity_viewport(Scene::default());
        view.set_stage(ferrocad_stage::Stage::new().with_item(named_slab()));
        view.set_grid(true);
        view.refresh();
        view
    }

    /// The strongest alpha in a packet's vertices.
    fn max_alpha(packet: &DrawPacket) -> f32 {
        packet
            .geometry
            .as_ref()
            .expect("fresh geometry")
            .vertices
            .iter()
            .map(|vertex| vertex.color[3])
            .fold(0.0, f32::max)
    }

    /// How many packets the **local** grid layers emit (a bound plane or a hover
    /// preview). They are the only depth-biased producers, so this is the invariant a
    /// grid-count assertion should use: a hidden minor level emits no dot packet, and
    /// the adaptive step changes how many levels survive the cull.
    fn local_packets(view: &InventorViewport<FakeView>) -> usize {
        view.grid_queue
            .packets
            .iter()
            .filter(|packet| packet.state.depth_bias > 0.0)
            .count()
    }

    fn escape_key() -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: "escape".to_string(),
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn hovering_a_face_resolves_its_workplane_and_blooms_a_grid() {
        use ferrocad_stage::WorkplaneKind;

        // The bloom is the Tier 1 preview, so it needs an open creation tool.
        let mut view = tool_view();
        assert!(view.handle_pointer([30.0, 50.0]), "the hover changed");

        // The frame is resolved from the tessellation: centroid, normal, and the part
        // name it came from.
        let face = view.hover_face.as_ref().expect("Face1 under the pointer");
        assert_eq!(face.entity, 7);
        assert_eq!(
            face.frame.plane.kind,
            WorkplaneKind::Face {
                entity: 7,
                element: "Face1".into()
            }
        );
        assert_eq!(face.frame.plane.n, Vector3::new(0.0, 0.0, 1.0));
        assert!((face.frame.plane.origin.x + 0.25).abs() < 1e-6);
        assert!((face.frame.plane.origin.z - 0.5).abs() < 1e-6);

        // Before the dwell only the world layer is drawn — no depth-biased local
        // layer — at full strength. (Pin the dwell start: under a loaded test runner a
        // scheduling pause longer than the dwell would bloom it before the assert.)
        view.hover_face.as_mut().unwrap().since = std::time::Instant::now();
        view.sync_grid();
        assert_eq!(local_packets(&view), 0, "no face grid before the dwell");
        let world_alpha = max_alpha(&view.grid_queue.packets[0]);

        // Past the dwell the local layer blooms: coplanar with the face, depth-biased,
        // and the world level recedes.
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();
        assert!(
            local_packets(&view) > 0,
            "the local layer adds the face's own grid"
        );
        for packet in view
            .grid_queue
            .packets
            .iter()
            .filter(|packet| packet.state.depth_bias > 0.0)
        {
            assert!(packet.state.depth_bias > 0.0, "a face grid is coplanar");
            for vertex in &packet.geometry.as_ref().unwrap().vertices {
                assert!(
                    (vertex.position[2] - 0.5).abs() < 1e-4,
                    "the local grid lies on the face: {vertex:?}"
                );
            }
        }
        assert!(
            max_alpha(&view.grid_queue.packets[0]) < world_alpha * 0.5,
            "the world grid recedes under a local plane"
        );
    }

    #[test]
    fn a_face_workplane_emits_uv_without_normal_stem() {
        use ferrocad_stage::{AxisStyle, GridSpec, GridStyle, WorkplaneKind};

        // The layer's own emitter decides the stem count from the plane's provenance, so
        // the same frame drawn as a face and as a floating datum differs by exactly one
        // stem (four vertices, six indices). A three-quarter camera keeps every stem
        // visible: an axis-aligned view drops the view-parallel one.
        let camera = gizmo_camera([100, 100]);
        let frame = |kind| {
            Workplane::new(
                Vector3::new(0.0, 0.0, 0.5),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                kind,
            )
            .expect("a frame")
        };
        let spec = |kind| GridSpec {
            workplane: frame(kind),
            minor: 1.0,
            major: 10.0,
            radius: 10.0,
        };
        let style = GridStyle::default();
        let axis = AxisStyle::default();

        let mut face = GridLayer::new(0.0);
        face.sync(
            &camera,
            &spec(WorkplaneKind::Face {
                entity: 7,
                element: "Face1".into(),
            }),
            &style,
            &axis,
        );
        let mut floating = GridLayer::new(0.0);
        floating.sync(&camera, &spec(WorkplaneKind::Datum("Sheet".into())), &style, &axis);

        assert_eq!(face.triad.len() + 4, floating.triad.len(), "no `+N` stem on a face");
        assert_eq!(face.triad_indices.len() + 6, floating.triad_indices.len());
        assert!(!face.triad.is_empty(), "the `U`/`V` bracket is still there");
    }

    #[test]
    fn a_face_workplane_clamps_grid_radius_to_face_extent() {
        // A realistic camera and pinned steps: the identity camera's degenerate
        // projection escalates the effective major step, which would mask the patch.
        let mut view = identity_viewport(Scene::default());
        view.set_stage(ferrocad_stage::Stage::new().with_item(named_slab()));
        view.camera = perspective_camera([100, 100]);
        view.set_grid(true);
        view.set_grid_steps(1.0, 10.0);
        view.refresh();
        assert!(view.set_workplane_from_element(7, Some("Face1")));

        let extent = view.active_workplane().extent.expect("a face has an extent");
        assert!(extent > 0.0 && extent < 1.0, "the slab's half-face radius: {extent}");

        // The plane's step comes from its extent, not from the pinned world steps: about
        // ten major cells across the patch, on the 1-2-5 ladder. Keeping the world's
        // 100 mm major here is what made a 20 mm face carry a 200 mm patch.
        let plane = view.active_workplane().plane.clone();
        let (minor, major) = plane_steps(
            Some(extent),
            ground_radius(&view.camera, view.grid_radius),
            GridPolicy {
                minor: 1.0,
                major: 10.0,
                ..GridPolicy::default()
            },
        );
        assert_eq!(
            (minor, major),
            patch_steps(extent),
            "a bound plane ignores the configured world steps"
        );
        let (_, effective_major) = grid::effective_steps(&view.camera, &plane, minor, major);

        // The patch is `max(3 × extent, 2 × major)` on the plane's own effective step, and
        // it deliberately ignores the world ground's `RADIUS_RANGE` floor.
        let patch = active_radius(&view.camera, Some(extent), effective_major);
        assert!(
            (patch - (3.0 * extent).max(2.0 * effective_major)).abs() < 1e-9,
            "the patch is `max(3 × face, 2 × major)`: {patch}"
        );
        // A face patch is its own size: it ignores the world disc, which at this camera
        // is a different radius entirely.
        assert!(
            patch < active_radius(&view.camera, None, major),
            "a face patch is smaller than the world ground: {patch}"
        );
        assert!(patch < 10.0 * extent, "the patch is a face, not a wall: {patch}");
        assert!(
            (active_radius(&view.camera, None, major) - GridSpec::radius_for(&view.camera)).abs()
                < 1e-9
        );

        // The snap uses the plane's step, so it lands on a line the plane draws.
        let context = view.snap_context().expect("the bound plane snaps");
        assert_eq!((context.minor, context.major), patch_steps(extent));

        // Every vertex of the bound plane's **ground** lies inside that patch. The
        // triad is not part of it: its stems are screen-sized and longer than a small
        // patch, so they are excluded by taking the largest line packet (the ground).
        let origin = plane.origin;
        let ground = view
            .grid_queue
            .packets
            .iter()
            .filter(|packet| {
                packet.topology == Topology::Lines && packet.state.depth_bias > 0.0
            })
            .max_by_key(|packet| packet.geometry.as_ref().map_or(0, |geometry| geometry.vertices.len()))
            .expect("the bound plane's ground");
        for vertex in &ground.geometry.as_ref().unwrap().vertices {
            let point = Vector3::new(
                f64::from(vertex.position[0]),
                f64::from(vertex.position[1]),
                f64::from(vertex.position[2]),
            );
            let local = plane.project(point);
            assert!(
                local[0].hypot(local[1]) <= patch + 1e-6,
                "the grid stays inside the patch: {local:?} > {patch}"
            );
            assert!((point.z - origin.z).abs() < 1e-4, "it lies on the face");
        }
    }

    /// The 20 mm box from the report: a face patch is `3 × extent` with a 5 mm step, not
    /// 200 mm of 100 mm cells. The ladder steps up with the face instead of adding cells.
    #[test]
    fn a_face_patch_scales_with_its_extent() {
        let extent = 15.6;
        let adaptive = GridPolicy {
            minor: grid::MINOR_STEP,
            major: grid::MAJOR_STEP,
            ..GridPolicy::default()
        };
        let (minor, major) = plane_steps(Some(extent), 1000.0, adaptive);
        assert!((major - 5.0).abs() < 1e-9, "the ladder picks 5 mm: {major}");
        assert!((minor - 0.5).abs() < 1e-9, "a decade of minor cells: {minor}");
        let patch = patch_radius(Some(extent), major).expect("a bound plane");
        assert!((patch - 46.8).abs() < 1e-9, "3 × extent: {patch}");

        assert_eq!(
            nice_step(0.3 * 500.0),
            200.0,
            "a large face steps up rather than drawing 500 sub-millimetre cells"
        );
    }

    /// The **adaptive** setting is one rule for every layer: the step fits the extent that
    /// layer covers; turning it off uses the configured pair on every plane.
    #[test]
    fn adaptive_spacing_follows_the_extent_and_off_uses_the_configured_pair() {
        let adaptive = GridPolicy {
            minor: 10.0,
            major: 100.0,
            ..GridPolicy::default()
        };
        let configured = GridPolicy {
            adaptive: false,
            ..adaptive
        };

        // Adaptive: a face patch is sized from the face, the ground from its disc.
        assert_eq!(plane_steps(Some(15.6), 500.0, adaptive), patch_steps(15.6));
        assert_eq!(plane_steps(Some(86.6), 500.0, adaptive), patch_steps(86.6));
        let (minor, major) = plane_steps(None, 500.0, adaptive);
        assert_eq!(
            major,
            ground_steps(500.0).1,
            "the ground's lines are the disc's ladder"
        );
        assert!(
            minor >= ground_steps(500.0).0,
            "the dot level is only coarsened, never refined"
        );
        assert!(
            500.0 / minor <= DOT_COLUMNS + 1e-9,
            "the dot field stays inside the point budget: {minor}"
        );

        // Not adaptive: one pair, everywhere, exactly. That is the point of the switch —
        // the configured 10/100 is what the ground *and* a face draw, at any zoom. A step
        // too fine for the disc is clipped in extent by `grid::ground`, never coarsened.
        assert_eq!(plane_steps(Some(15.6), 500.0, configured), (10.0, 100.0));
        assert_eq!(plane_steps(Some(86.6), 500.0, configured), (10.0, 100.0));
        assert_eq!(plane_steps(None, 500.0, configured), (10.0, 100.0));

        let fine = GridPolicy {
            minor: 1.0,
            major: 10.0,
            adaptive: false,
            ..GridPolicy::default()
        };
        assert_eq!(
            plane_steps(None, 2_500.0, fine),
            (1.0, 10.0),
            "a fine pair is honoured on a large disc: the level clips, the pitch stays"
        );

        // The default is adaptive: the extent rule is the shipped look.
        assert!(GridPolicy::default().adaptive);
    }

    /// The ground's step is sized from the disc it is drawn over — the only extent it
    /// covers — so it stays legible at every zoom and its geometry stays bounded. A step
    /// taken from a tiny model instead asked for ~800k vertices on a 500 mm ground.
    #[test]
    fn the_ground_step_follows_its_disc_and_stays_bounded() {
        for radius in [500.0, 2500.0, 5000.0] {
            let (minor, major) = ground_steps(radius);
            let cells = radius / major;
            assert!(
                (12.0..=25.5).contains(&cells),
                "about ten visible cells at {radius}: {cells}"
            );
            assert!(
                minor >= major / 10.0,
                "the dot level is a decade below at most"
            );
            // [`ground`](grid::ground) subdivides each line at the minor step, so the
            // vertex count is ~`160 · cells²`; keep it in the same range as the
            // configured grid's.
            assert!(
                160.0 * cells * cells <= 100_000.0,
                "bounded geometry: {cells}"
            );
        }

        // Zooming changes the ladder (adaptive), tilting does not (the radius follows the
        // eye distance only).
        assert_ne!(
            ground_steps(500.0),
            ground_steps(5000.0),
            "a nearer ground gets a finer lattice"
        );
        assert_eq!(ground_steps(2500.0), (10.0, 100.0), "the default zoom's steps");
    }

    /// Adaptive spacing is viewport policy: the drawn ground uses the disc ladder, and
    /// switching it off restores the configured pair without touching the scene.
    #[test]
    fn the_viewport_applies_the_adaptive_switch() {
        let mut view = slab_view();
        assert!(view.grid_adaptive(), "the default is adaptive");
        let radius = ground_radius(&view.camera, view.grid_radius);
        let (minor, major) = plane_steps(None, radius, view.grid_policy());
        assert_eq!(major, ground_steps(radius).1);
        assert!(minor >= ground_steps(radius).0);
        assert!(
            view.grid_queue
                .packets
                .iter()
                .any(|packet| packet.topology == Topology::Points),
            "the adaptive ground still draws its dots"
        );

        assert!(view.set_grid_adaptive(false));
        assert_eq!(
            plane_steps(None, radius, view.grid_policy()),
            view.grid_steps(),
            "off draws the configured pair"
        );
        assert!(!view.set_grid_adaptive(false), "a no-op reports no change");
    }

    /// A bound plane keeps a **drawable** minor level, so `minor_style = lines` really
    /// draws a lattice. The minor level is a level of detail: it coarsens up the 1-2-5
    /// ladder when its step is too small to read, and collapses onto the major only when
    /// nothing below the major draws. Collapsing immediately (the older rule) left the
    /// lattice with no rows at all — every row at the major step *is* a major row — so the
    /// setting looked inert on any face at a normal zoom.
    #[test]
    fn a_bound_plane_draws_its_minor_lattice() {
        let mut view = slab_view();
        assert!(view.set_workplane_from_element(7, Some("Face1")));
        assert!(view.set_grid_stroke(grid::MinorStyle::Lines, 1.0, 1.0));

        let active = view.active_workplane();
        let (spec, style) = active_grid(
            &view.camera,
            &active.plane,
            view.grid_policy(),
            [1.0; 4],
            1.0,
            active.extent,
            ground_radius(&view.camera, view.grid_radius),
        );
        assert_eq!(style.minor_style, grid::MinorStyle::Lines);
        assert!(
            spec.minor < spec.major,
            "the minor level survives the cull: {} vs {}",
            spec.minor,
            spec.major
        );

        let ground = grid::ground(&spec, &style);
        assert!(
            !ground.minor_lines.is_empty(),
            "the lattice has rows that are not major rows"
        );
        assert!(
            view.grid_queue.packets.iter().any(|packet| {
                packet.topology == Topology::Lines
                    && packet
                        .geometry
                        .as_ref()
                        .is_some_and(|geometry| {
                            geometry.vertices.len() == ground.minor_lines.len()
                        })
            }),
            "the minor lattice reaches the queue"
        );
    }

    /// The hover preview and the bound grid are the same grid — **including its ink** — so
    /// binding a face cannot change how strong either level draws. They share
    /// [`level_style`], which is also what the Coin frame reads.
    #[test]
    fn a_face_preview_is_the_grid_it_will_bind() {
        let camera = perspective_camera([100, 100]);
        let frame = FaceFrame {
            plane: Workplane::world(),
            radius: 15.6,
        };
        let policy = GridPolicy::default();
        let (preview, preview_style) = local_grid(&camera, &frame, policy);
        let (bound, bound_style) = active_grid(
            &camera,
            &frame.plane,
            policy,
            [1.0; 4],
            1.0,
            Some(frame.radius),
            0.0,
        );
        assert_eq!(preview.minor, bound.minor, "the preview's step is the bound step");
        assert_eq!(preview.major, bound.major, "the preview's step is the bound step");
        assert_eq!(
            preview.radius, bound.radius,
            "the preview's patch is the bound patch"
        );
        assert!(
            preview.radius < 100.0,
            "a face-sized patch, not a wall: {}",
            preview.radius
        );
        assert_eq!(
            preview_style.minor_alpha, bound_style.minor_alpha,
            "binding keeps the minor ink"
        );
        assert_eq!(
            preview_style.major_alpha, bound_style.major_alpha,
            "binding keeps the major ink"
        );
        // A bound plane keeps the workplane's 1:2 ratio; the ground's levels are equal.
        assert!(
            (bound_style.major_alpha - 2.0 * bound_style.minor_alpha).abs() < 1e-6,
            "the workplane ratio: {} / {}",
            bound_style.major_alpha,
            bound_style.minor_alpha
        );
        let (_, ground_style) = active_grid(&camera, &frame.plane, policy, [1.0; 4], 1.0, None, 500.0);
        assert!(
            (ground_style.major_alpha - ground_style.minor_alpha).abs() < 1e-6,
            "the world's levels are equal"
        );
    }

    /// A fixed "Grid extent" bounds the program grid too, exactly as it bounds the
    /// native ground: without this the Coin grid ignored the setting entirely.
    #[test]
    fn a_fixed_grid_radius_reaches_the_program_frame() {
        let mut view = slab_view();
        view.set_grid_radius(Some(800.0));
        let radius = view.active_workplane().program_patch_radius(
            &view.camera,
            GridPolicy {
                minor: 10.0,
                major: 100.0,
                ..GridPolicy::default()
            },
            Some(800.0),
        );
        assert!(
            (f64::from(radius) - 800.0).abs() < 1e-6,
            "the user's extent: {radius}"
        );
    }

    /// The stroke setting reaches the drawn layer **and** the Coin frame: the minor level
    /// switches form, and a width above the hairline becomes ribbon triangles.
    #[test]
    fn the_grid_stroke_reaches_the_layer_and_the_program_frame() {
        let mut view = slab_view();
        // The cross off, so the only triangles in the queue are a wide ground stroke.
        assert!(view.set_axis_cross(false));
        assert_eq!(
            view.grid_stroke(),
            (grid::MinorStyle::Dots, 1.0, 1.0),
            "the default is dots at the hairline"
        );
        assert!(
            view.grid_queue
                .packets
                .iter()
                .all(|packet| packet.topology != Topology::Triangles),
            "a hairline ground is lines and points, not quads"
        );

        assert!(view.set_grid_stroke(grid::MinorStyle::Lines, 3.0, 2.0));
        let packets = &view.grid_queue.packets;
        assert!(
            packets.iter().all(|packet| packet.topology != Topology::Points),
            "the lattice replaces the dots"
        );
        assert!(
            packets
                .iter()
                .filter(|packet| packet.topology == Topology::Triangles)
                .count()
                >= 2,
            "both levels are strokes"
        );
        assert!(
            packets.iter().all(|packet| !packet.state.ribbon),
            "a grid stroke is plain geometry, not a feathered ribbon"
        );
        // ...and it must **blend**: the opaque scene pipeline is `REPLACE`, so a triangle
        // drawn through it ignores the per-vertex alpha that carries the opacity, the
        // radial fade and the edge-on fade. `BLENDED_DEPTH` selects the blended variant.
        assert!(
            packets
                .iter()
                .filter(|packet| packet.topology == Topology::Triangles)
                .all(|packet| packet.state.depth == BLENDED_DEPTH),
            "a wide grid stroke blends its ink"
        );
        // The hairlines keep the depth-writing default: only the quads need the variant.
        assert!(
            packets
                .iter()
                .filter(|packet| packet.topology != Topology::Triangles)
                .all(|packet| packet.state.depth == DepthState::default()),
            "a hairline is unchanged"
        );

        // The same stroke reaches the Coin frame, in the plane's units.
        let frame = view.active_workplane().view_workplane(
            &view.camera,
            view.grid_policy(),
            view.grid_radius,
        );
        assert!(frame.minor_lines);
        assert!(frame.major_width > frame.minor_width, "3 px vs 2 px");
        assert!(frame.minor_width > 0.0);

        assert!(
            !view.set_grid_stroke(grid::MinorStyle::Lines, 3.0, 2.0),
            "a no-op reports no change"
        );
    }

    /// A wide stroke really is the requested width **on screen**: the host converts pixels
    /// to the plane's units at the origin, so the ribbon projects to that many device
    /// pixels where the plane meets its origin.
    #[test]
    fn a_wide_stroke_is_the_requested_pixel_width() {
        let mut view = identity_viewport(Scene::default());
        view.set_stage(ferrocad_stage::Stage::new().with_item(named_slab()));
        view.camera = perspective_camera([100, 100]);
        view.set_grid(true);
        assert!(view.set_axis_cross(false));
        assert!(view.set_grid_stroke(grid::MinorStyle::Dots, 6.0, 1.0));
        view.refresh();

        // The world axis line (`v = 0`) is the one at the origin's depth, so its quad's
        // lateral corners are exactly 6 px apart.
        let packet = view
            .grid_queue
            .packets
            .iter()
            .filter(|packet| packet.topology == Topology::Triangles)
            .max_by_key(|packet| packet.geometry.as_ref().map_or(0, |g| g.vertices.len()))
            .expect("the major stroke");
        let vertices = &packet.geometry.as_ref().unwrap().vertices;
        let at = |v: &Vertex| {
            project_device(
                &view.camera,
                Vector3::new(
                    f64::from(v.position[0]),
                    f64::from(v.position[1]),
                    f64::from(v.position[2]),
                ),
            )
            .expect("a quad corner projects")
        };
        // Corners 0 and 3 are the two sides of the first segment.
        let (a, b) = (at(&vertices[0]), at(&vertices[3]));
        let width = (a[0] - b[0]).hypot(a[1] - b[1]);
        assert!(
            (width - 6.0).abs() < 0.5,
            "6 px requested, {width} px drawn"
        );
    }

    /// A stroke is measured in **device** pixels at any display scale: `scale_for` uses
    /// the render target's device height, and the Coin shader's `fwidth` is per device
    /// pixel too, so a device-pixel-ratio change cannot make the two backends disagree.
    /// (It does mean the number is physical: on a 2× display "4 px" is 2 logical pixels,
    /// in both backends.)
    #[test]
    fn a_stroke_is_measured_in_device_pixels_at_any_scale() {
        use ferrocad_graphics::Viewport;

        let camera = |device: [u32; 2], ratio: f32| Camera {
            viewport: Viewport::new(device, ratio),
            ..perspective_camera(device)
        };
        // The same 4-device-pixel stroke on a 1× and a 2× target: the 2× target doubles
        // the device height, so the world width that projects to 4 device pixels halves.
        let one = stroke_width(&camera([100, 100], 1.0), &Workplane::world(), 4.0);
        let two = stroke_width(&camera([200, 200], 2.0), &Workplane::world(), 4.0);
        assert!(one > 0.0 && two > 0.0);
        assert!(
            (one * 0.5 - two).abs() < 1e-9,
            "4 device px is half the world width at 2×: {one} vs {two}"
        );

        // The hairline is the device-space hairline at every scale: one logical pixel on
        // a 2× display is still drawn by the shader as one *device* pixel.
        assert_eq!(stroke_width(&camera([100, 100], 1.0), &Workplane::world(), 1.0), 0.0);
        assert_eq!(stroke_width(&camera([200, 200], 2.0), &Workplane::world(), 1.0), 0.0);
    }

    /// The Coin frame carries the **same ink** the native generator draws. A shader with
    /// a strength of its own (the mod's 0.65) drew a 1 px line 4× heavier than the native
    /// 0.15, which reads as a thicker line at the same width.
    #[test]
    fn the_coin_frame_carries_the_native_ink() {
        let mut view = slab_view();
        let ink = |view: &InventorViewport<FakeView>| {
            let frame = view.active_workplane().view_workplane(
                &view.camera,
                view.grid_policy(),
                view.grid_radius,
            );
            (frame.minor_alpha, frame.major_alpha)
        };

        // The world ground draws both levels at the token alpha.
        let (minor, major) = ink(&view);
        assert!((minor - grid::GRID_ALPHA).abs() < 1e-6, "{minor}");
        assert!((major - grid::GRID_ALPHA).abs() < 1e-6, "{major}");

        // A bound face keeps the workplane's 1:2 minor:major ratio.
        assert!(view.set_workplane_from_element(7, Some("Face1")));
        let (minor, major) = ink(&view);
        assert!((minor - grid::GRID_ALPHA).abs() < 1e-6);
        assert!((major - 2.0 * grid::GRID_ALPHA).abs() < 1e-6, "{major}");

        // The user's opacity multiplies both, in the shader as in the generator.
        assert!(view.set_grid_alpha(0.5));
        let (minor, major) = ink(&view);
        assert!((minor - 0.5 * grid::GRID_ALPHA).abs() < 1e-6, "{minor}");
        assert!((major - grid::GRID_ALPHA).abs() < 1e-6, "{major}");
    }

    /// The program (Coin) grid's patch radius is the native grid's rule, and `0` for a
    /// plane with no surface behind it: the host writes that one scalar into
    /// `Frame.workplane_origin.w`, so the mod grid and the native layer agree.
    #[test]
    fn the_program_grid_patch_radius_matches_the_native_grid() {
        let mut view = identity_viewport(Scene::default());
        view.set_stage(ferrocad_stage::Stage::new().with_item(named_slab()));
        view.camera = perspective_camera([100, 100]);
        view.set_grid(true);
        view.set_grid_steps(1.0, 10.0);
        view.refresh();

        // The world plane has no finite surface, so it gets the native ground's extent:
        // the camera-following radius, never `0` (an unbounded plane is cut only by the
        // shader's per-pixel horizon fade, which draws a band through the model).
        assert_eq!(
            view.active_workplane()
                .program_patch_radius(&view.camera, view.grid_policy(), None),
            GridSpec::radius_for(&view.camera) as f32
        );

        // A face binds a finite surface. The value is the native layer's patch, computed
        // on the same effective major step.
        assert!(view.set_workplane_from_element(7, Some("Face1")));
        let active = view.active_workplane().clone();
        let (minor, major) = plane_steps(
            active.extent,
            ground_radius(&view.camera, view.grid_radius),
            view.grid_policy(),
        );
        let (_, major) = grid::effective_steps(&view.camera, &active.plane, minor, major);
        let expected = (3.0 * active.extent.expect("a face has an extent")).max(2.0 * major) as f32;
        let radius = active.program_patch_radius(&view.camera, view.grid_policy(), None);
        assert!(
            (radius - expected).abs() < 1e-6,
            "the program patch is the native patch: {radius} vs {expected}"
        );
        assert!(radius > 0.0);

        // A world-standard plane carries no surface, so it returns to the ground extent.
        assert!(view.set_standard_plane(ferrocad_stage::StandardPlane::Top));
        assert_eq!(
            view.active_workplane()
                .program_patch_radius(&view.camera, view.grid_policy(), None),
            GridSpec::radius_for(&view.camera) as f32
        );
    }

    /// A view that records the workplane carried by the frame's program view.
    struct WorkplaneRecordingView {
        frames: Rc<RefCell<Vec<Option<WorkplaneFrame>>>>,
    }

    impl InventorView for WorkplaneRecordingView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [100, 100]
        }
        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Ok(ImageData::default())
        }
        fn draw_frame(&mut self, views: &[ViewSpec<'_>]) -> Result<(), Self::Error> {
            let program = views.iter().find(|view| {
                view.queue
                    .packets
                    .iter()
                    .any(|packet| packet.state.program.is_some())
            });
            self.frames
                .borrow_mut()
                .push(program.and_then(|view| view.workplane));
            Ok(())
        }
    }

    /// W4: the Coin grid reads the active workplane from the frame, not world `XY`. The
    /// viewport hands the program view the plane **and the native grid's extent**, so the
    /// shader draws the same patch the native layer would (an unbounded plane has no
    /// extent to fade against, and its horizon shows as a band).
    #[test]
    fn coin_grid_mod_tracks_the_active_workplane_frame() {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let camera = perspective_camera([100, 100]);
        let mut viewport = InventorViewport::new(
            WorkplaneRecordingView {
                frames: frames.clone(),
            },
            camera,
            empty_scene(),
        );
        viewport.set_overlays(vec![program_scene()]);
        viewport.set_grid_steps(1.0, 10.0);

        viewport.refresh();
        let world = frames.borrow().last().cloned().expect("a program view");
        let world = world.expect("the program view carries a plane");
        assert_eq!(world.origin, [0.0, 0.0, 0.0]);
        assert_eq!(world.u, [1.0, 0.0, 0.0]);
        assert_eq!(world.v, [0.0, 1.0, 0.0]);
        assert_eq!(world.n, [0.0, 0.0, 1.0]);
        assert_eq!(
            world.radius,
            GridSpec::radius_for(&viewport.camera) as f32,
            "the world plane carries the native ground's extent"
        );

        let frame = FaceFrame {
            plane: Workplane::new(
                Vector3::new(10.0, 20.0, 30.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                ferrocad_stage::WorkplaneKind::Face {
                    entity: 7,
                    element: "Face1".to_owned(),
                },
            )
            .expect("a right-handed face frame"),
            radius: 15.0,
        };
        assert!(viewport.set_active_workplane(ActiveWorkplane::of(&frame)));
        viewport.refresh();

        let bound = frames.borrow().last().cloned().expect("a program view");
        let bound = bound.expect("the program view carries a plane");
        assert_eq!(bound.origin, [10.0, 20.0, 30.0]);
        assert_eq!(bound.u, [0.0, 1.0, 0.0]);
        assert_eq!(bound.v, [0.0, 0.0, 1.0]);
        assert_eq!(bound.n, [1.0, 0.0, 0.0]);

        let (minor, major) = {
            let (minor, major) = plane_steps(Some(frame.radius), 0.0, viewport.grid_policy());
            grid::effective_steps(&viewport.camera, &frame.plane, minor, major)
        };
        assert_eq!(bound.minor, minor as f32, "the host's minor step");
        assert_eq!(bound.major, major as f32, "the host's major step");
        let expected = (3.0 * frame.radius).max(2.0 * major) as f32;
        assert!(
            (bound.radius - expected).abs() < 1e-6,
            "the face patch is the native rule: {} vs {expected}",
            bound.radius
        );
        assert!(bound.radius > 0.0);
    }

    #[test]
    fn hovering_a_face_while_idle_does_not_bloom_a_grid() {
        let mut view = slab_view();
        assert!(view.handle_pointer([30.0, 50.0]));
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();

        // Hovering with no tool would bind nothing, so it must not add a second grid —
        // but the face is still resolved, so `Set WP` and the hover report keep working.
        assert_eq!(local_packets(&view), 0, "idle hover adds no second grid");
        assert!(view.hover_face.is_some(), "the face is still resolved");
        assert!(view.active_workplane().is_default());
    }

    #[test]
    fn hovering_a_face_with_an_active_tool_blooms_only_at_world_default() {
        let mut view = tool_view();
        view.handle_pointer([30.0, 50.0]);
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();
        assert!(
            local_packets(&view) > 0,
            "the default plane previews the bind"
        );

        // Bind the face (the click the bloom is previewing), then hover the other face:
        // a bound plane does not preview a different one.
        assert!(view.activate_hovered_workplane());
        let bound = view.grid_queue.len();
        assert!(view.handle_pointer([70.0, 50.0]));
        view.sync_grid();
        assert_eq!(
            view.grid_queue.len(),
            bound,
            "a bound plane never previews another face"
        );
    }

    #[test]
    fn crossing_to_another_face_restarts_the_dwell() {
        // The bloom is the Tier 1 preview, so it needs an open creation tool.
        let mut view = tool_view();
        view.handle_pointer([30.0, 50.0]);
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();
        assert!(local_packets(&view) > 0, "Face1 bloomed");

        // Face2 is a different part, so its grid must wait out its own dwell. Pin the
        // dwell start so a scheduling pause longer than the dwell cannot bloom it.
        assert!(view.handle_pointer([70.0, 50.0]));
        view.hover_face.as_mut().unwrap().since = std::time::Instant::now();
        view.sync_grid();
        assert_eq!(view.hover_face.as_ref().unwrap().part, 1);
        assert_eq!(local_packets(&view), 0, "the new face has not rested yet");
    }

    #[test]
    fn a_second_bloom_reuses_the_local_mesh_ids() {
        // The bloom is the Tier 1 preview, so it needs an open creation tool.
        let mut view = tool_view();
        view.handle_pointer([30.0, 50.0]);
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();
        let local_meshes = |view: &InventorViewport<FakeView>| -> Vec<MeshId> {
            view.grid_queue
                .packets
                .iter()
                .filter(|packet| packet.state.depth_bias > 0.0)
                .map(|packet| packet.mesh_id)
                .collect()
        };
        let first = local_meshes(&view);
        assert!(!first.is_empty());

        // Away, then back onto the same face: a fresh dwell, the same meshes. The local
        // layers are allocated once, so a hover that comes and goes leaks nothing.
        assert!(view.handle_pointer([98.0, 2.0]));
        view.sync_grid();
        assert_eq!(local_packets(&view), 0);
        view.handle_pointer([30.0, 50.0]);
        view.hover_face.as_mut().unwrap().since -= FACE_DWELL;
        view.sync_grid();
        let second = local_meshes(&view);
        assert_eq!(first, second);
    }

    #[test]
    fn activating_the_face_makes_it_the_snap_plane_and_escape_clears_it() {
        use ferrocad_stage::WorkplaneKind;

        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());

        let active = view.active_workplane();
        assert_eq!(
            active.plane.kind,
            WorkplaneKind::Face {
                entity: 7,
                element: "Face1".into()
            }
        );
        let context = view.snap_context().expect("a snap plane");
        assert_eq!(context.workplane.kind, active.plane.kind);
        // The bound plane snaps on the steps its own patch is drawn on, so the grid and
        // the snapping agree.
        let (minor, major) = patch_steps(active.extent.expect("a face has an extent"));
        assert_eq!(context.minor, minor);
        assert_eq!(context.major, major);

        // The active plane is drawn without a dwell, and hovering it adds no second
        // layer. (The packet count is an implementation detail — a hidden minor level
        // emits no dot packet at all — what matters is that a second sync with the
        // pointer still on the face adds nothing.)
        let active_packets = view.grid_queue.len();
        assert!(
            local_packets(&view) > 0,
            "the bound plane adds a layer: {active_packets}"
        );
        view.sync_grid();
        assert_eq!(
            view.grid_queue.len(),
            active_packets,
            "no second grid accumulates"
        );
        // The world ground steps aside: one grid, at the bound plane, not two stacked.
        // A hidden minor level emits no dot packet, so the ground is identified by its
        // line packets: exactly one depth-biased ground (the triad's ghost is 8
        // vertices, the ground's line field is far larger).
        assert_eq!(
            view.grid_queue
                .packets
                .iter()
                .filter(|packet| {
                    packet.topology == Topology::Lines
                        && packet.state.depth_bias > 0.0
                        && packet.geometry.as_ref().is_some_and(|g| g.vertices.len() > 16)
                })
                .count(),
            1,
            "only the bound plane's grid is drawn"
        );
        view.handle_pointer([30.0, 50.0]);
        view.sync_grid();
        assert_eq!(
            view.grid_queue.len(),
            active_packets,
            "the active face is not previewed"
        );

        // Esc clears it: snapping returns to the world and its ground comes back.
        assert!(view.handle_key(&escape_key()));
        assert!(view.active_workplane().is_default());
        assert!(
            view.snap_context().is_none(),
            "no world grid snapping by default"
        );
        assert_eq!(local_packets(&view), 0, "the bound plane's layer is gone");
        assert!(
            view.grid_queue
                .packets
                .iter()
                .any(|packet| packet.topology == Topology::Lines),
            "the world ground is the only grid again"
        );
    }

    /// A slab viewport with a tool active, so pointer moves resolve a snap.
    fn tool_view() -> InventorViewport<FakeView> {
        let mut view = identity_viewport(Scene::default());
        view.set_stage(ferrocad_stage::Stage::new().with_item(named_slab()));
        view.set_grid(true);
        view.set_tool(Some(Box::new(ferrocad_stage::PolylineTool::new())));
        view.refresh();
        view
    }

    /// The **logical** (top-left origin) position of a world XY point under the identity
    /// camera at 1x in a 100×100 viewport.
    fn identity_logical(world: [f64; 2]) -> [f32; 2] {
        [
            ((world[0] * 0.5 + 0.5) * 100.0) as f32,
            ((0.5 - world[1] * 0.5) * 100.0) as f32,
        ]
    }

    #[test]
    fn a_pointer_near_a_vertex_snaps_to_the_vertex_and_flashes() {
        use ferrocad_stage::SnapKind;

        let mut view = tool_view();
        assert!(
            view.handle_pointer(identity_logical([-0.48, -0.48])),
            "the hover changed"
        );

        assert_eq!(view.snap_kind(), Some(SnapKind::Vertex));
        let snap = view.snap_feedback.as_ref().expect("a snap").snap;
        assert!(
            (f64::from(snap.point[0]) + 0.5).abs() < 1e-6
                && (f64::from(snap.point[1]) + 0.5).abs() < 1e-6,
            "the exact vertex, not the pointer: {snap:?}"
        );

        // The aurora is live and starts at its flash alpha. A vertex snap gets no rays.
        assert!(view.feedback_flash_pending());
        assert_eq!(view.feedback_queue.len(), 1);
        assert_eq!(view.feedback_queue.packets[0].topology, Topology::Triangles);
        assert!((max_alpha(&view.feedback_queue.packets[0]) - 0.85).abs() < 1e-5);

        // Past the flash it settles.
        view.snap_feedback.as_mut().unwrap().since -= feedback::FLASH;
        view.rebuild_feedback();
        assert!(
            (max_alpha(&view.feedback_queue.packets[0]) - feedback::AURORA_REST_ALPHA).abs() < 1e-5
        );
    }

    #[test]
    fn a_grid_snap_emits_the_aurora_and_rays_to_both_axes() {
        use ferrocad_stage::SnapKind;

        let mut view = tool_view();
        // Empty space over the world grid; a 0.25 step makes the snap predictable.
        view.set_snap_grid(Some(0.25));
        assert!(view.handle_pointer(identity_logical([0.8, 0.8])));

        assert_eq!(view.snap_kind(), Some(SnapKind::Grid));
        let local = view.snap_local().expect("local coordinates");
        assert!((local[0] - 0.75).abs() < 1e-9, "{local:?}");
        assert!((local[1] - 0.75).abs() < 1e-9, "{local:?}");

        // Aurora + rays, both overlays and both untagged.
        assert_eq!(view.feedback_queue.len(), 2);
        assert_eq!(view.feedback_queue.packets[0].topology, Topology::Triangles);
        assert_eq!(view.feedback_queue.packets[1].topology, Topology::Lines);
        assert!(
            view.feedback_queue
                .packets
                .iter()
                .all(|packet| packet.state.id == PickId(0))
        );
        // The rays stay in the plane and reach for an axis.
        let rays = view.feedback_queue.packets[1]
            .geometry
            .as_ref()
            .expect("fresh geometry");
        assert!(!rays.vertices.is_empty());
        assert!(
            rays.vertices
                .iter()
                .all(|vertex| vertex.position[2].abs() < 1e-6)
        );
        assert!(
            rays.vertices
                .iter()
                .any(|vertex| vertex.position[0].abs() < 1e-6 || vertex.position[1].abs() < 1e-6),
            "a ray should reach an axis"
        );
    }

    #[test]
    fn the_origin_node_blooms_near_the_pointer() {
        let mut view = grid_view();
        // The identity camera maps the world origin to the viewport centre.
        view.handle_pointer([50.0, 50.0]);
        view.sync_grid();
        let marker = view.grid_queue.packets.last().expect("the origin node");
        assert_eq!(marker.topology, Topology::Lines);
        assert_eq!(max_alpha(marker), 1.0, "bloomed within 5 px");

        view.handle_pointer([95.0, 95.0]);
        view.sync_grid();
        let marker = view.grid_queue.packets.last().expect("the origin node");
        assert!(
            (max_alpha(marker) - feedback::ORIGIN_ALPHA).abs() < 1e-6,
            "rests when the pointer is away"
        );
    }

    #[test]
    fn ending_the_tool_clears_the_snap_feedback() {
        let mut view = tool_view();
        view.set_snap_grid(Some(0.25));
        view.handle_pointer(identity_logical([0.8, 0.8]));
        assert!(!view.feedback_queue.is_empty());

        view.set_tool(None);
        assert!(view.snap_feedback.is_none());
        assert!(view.feedback_queue.is_empty());
        assert_eq!(view.snap_kind(), None);
    }

    fn key(key: &str) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: key.to_string(),
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    /// The tool status the host would receive: start, each placed point, and the end.
    #[test]
    fn the_tool_status_is_queued_on_start_add_finish_and_cancel() {
        let mut view = tool_view();
        let started = view.take_tool_state().expect("a start status");
        assert!(started.active);
        assert_eq!(started.id, ferrocad_stage::tool::POLYLINE_TOOL_ID);
        assert_eq!(started.label, "Polyline");
        assert_eq!(started.points, 0);
        assert!(started.hint.contains("Esc to cancel"), "{}", started.hint);

        // A press places a point, so the next status counts it and advertises Enter.
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        let added = view.take_tool_state().expect("an add status");
        assert_eq!(added.points, 1);
        assert!(added.hint.contains("Enter to finish"), "{}", added.hint);

        // A drag-style press adds another; a move does not change the status.
        view.handle_button(identity_logical([0.25, 0.25]), 1, true);
        assert_eq!(view.take_tool_state().expect("an add status").points, 2);
        view.handle_pointer(identity_logical([0.1, 0.1]));
        assert!(view.take_tool_state().is_none(), "a cursor move is not a state change");

        // Enter finishes: the session is gone, the status is inactive, and the two
        // points are handed over as a *creating* commit.
        assert!(view.handle_key(&key("enter")));
        assert!(view.tool().is_none());
        let finished = view.take_tool_state().expect("a finish status");
        assert!(!finished.active);
        assert_eq!(finished.points, 0);
        let commit = view
            .take_interaction_commit()
            .expect("a finished polyline hands its points over");
        assert!(commit.creates, "the host creates the object, it does not edit one");
        assert_eq!(commit.manipulator, "polyline");
        assert_eq!(commit.action, "create");
        assert_eq!(commit.values.len(), 6, "two world-space points");

        // One point is not a polyline: finishing it writes nothing.
        view.set_tool(Some(Box::new(ferrocad_stage::PolylineTool::new())));
        let _ = view.take_tool_state();
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        assert!(view.handle_key(&key("enter")));
        assert!(
            view.take_interaction_commit().is_none(),
            "a single click commits nothing"
        );

        // Cancelling a fresh session reports an inactive status too.
        view.set_tool(Some(Box::new(ferrocad_stage::PolylineTool::new())));
        let _ = view.take_tool_state();
        assert!(view.cancel_tool());
        assert!(!view.take_tool_state().expect("a cancel status").active);
        assert!(!view.cancel_tool(), "no session is left to cancel");
    }

    #[test]
    fn escape_cancels_the_tool_leaving_no_points_and_no_commit() {
        let mut view = tool_view();
        let _ = view.take_tool_state();
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        view.handle_pointer(identity_logical([0.25, 0.25]));
        view.handle_button(identity_logical([0.25, 0.25]), 1, true);

        let status = view.tool_status().expect("an active tool");
        assert!(status.active);
        assert_eq!(status.points, 2);
        assert!(!view.transient.is_empty(), "two points draw a band and a marker");

        assert!(view.handle_key(&escape_key()));
        assert!(view.tool().is_none(), "the session ended");
        assert!(view.transient.is_empty(), "no transient geometry survives");
        assert!(
            view.take_interaction_commit().is_none(),
            "cancelling never writes the document"
        );
        assert!(!view.take_tool_state().expect("the cancel status").active);

        // With no tool, Escape falls through to the old behaviour.
        assert!(!view.handle_key(&escape_key()));
    }

    /// A keystroke with the control modifier held.
    fn ctrl_key(key: &str) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                key: key.to_string(),
                modifiers: gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn a_click_on_a_placed_vertex_selects_it_instead_of_appending() {
        let mut view = tool_view();
        let _ = view.take_tool_state();

        // One point, placed and released.
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        view.handle_button(identity_logical([-0.25, -0.25]), 1, false);
        assert_eq!(view.tool_status().expect("an active tool").points, 1);

        // The same click again lands on the placed vertex: the projection the host
        // attaches lets the tool hit-test its own point and select rather than append.
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        view.handle_button(identity_logical([-0.25, -0.25]), 1, false);
        let status = view.tool_status().expect("an active tool");
        assert_eq!(status.points, 1, "a click on a vertex never appends");
        assert!(
            status.hint.contains("Backspace to delete"),
            "the selected point is reported: {}",
            status.hint
        );
    }

    #[test]
    fn backspace_and_ctrl_z_reach_the_active_tool() {
        let mut view = tool_view();
        let _ = view.take_tool_state();
        view.handle_button(identity_logical([-0.25, -0.25]), 1, true);
        view.handle_button(identity_logical([0.25, 0.25]), 1, true);
        assert_eq!(view.tool_status().expect("an active tool").points, 2);

        assert!(view.handle_key(&key("backspace")), "the tool takes Backspace");
        assert_eq!(view.tool_status().expect("an active tool").points, 1);

        assert!(view.handle_key(&ctrl_key("z")), "the tool takes Ctrl+Z");
        assert_eq!(view.tool_status().expect("an active tool").points, 2);

        // With no tool installed the key is left to the host.
        view.set_tool(None);
        let _ = view.take_tool_state();
        assert!(!view.handle_key(&key("backspace")));
    }

    /// A viewport with the three-point workplane pick active. The snap grid is on (at a
    /// quarter unit) so presses over empty space still resolve a world point.
    fn plane_points_view() -> InventorViewport<FakeView> {
        let mut view = identity_viewport(Scene::default());
        view.set_snap_grid(Some(0.25));
        view.set_tool(Some(Box::new(ferrocad_stage::PlanePointsTool::new())));
        view.refresh();
        view
    }

    /// Press the three corners of an axis-aligned triangle in the world `XY` plane.
    fn pick_triangle(view: &mut InventorViewport<FakeView>) {
        for corner in [[-0.25, -0.25], [0.25, -0.25], [-0.25, 0.25]] {
            view.handle_button(identity_logical(corner), 1, true);
        }
    }

    #[test]
    fn picking_three_points_makes_a_custom_workplane_without_a_commit() {
        let mut view = plane_points_view();
        let _ = view.take_tool_state();
        pick_triangle(&mut view);
        assert_eq!(view.tool_status().expect("an active tool").points, 3);
        assert!(!view.transient.is_empty(), "the pick is drawn");

        // Enter ends the session and applies the frame it resolved.
        assert!(view.handle_key(&key("enter")));
        assert!(view.tool().is_none());
        assert!(
            view.take_interaction_commit().is_none(),
            "a plane pick writes no geometry"
        );

        let active = view.active_workplane();
        assert_eq!(active.label, "Custom");
        assert_eq!(active.plane.kind, ferrocad_stage::WorkplaneKind::Custom);
        assert!(
            (active.plane.n.x).abs() < 1e-6
                && (active.plane.n.y).abs() < 1e-6
                && (active.plane.n.z - 1.0).abs() < 1e-6,
            "the triangle lies in the world XY plane: {:?}",
            active.plane.n
        );
        assert!(!active.is_default(), "the pick replaced the world plane");
    }

    #[test]
    fn a_plane_pick_does_not_bind_the_face_it_starts_on() {
        let mut view = tool_view();
        view.set_tool(Some(Box::new(ferrocad_stage::PlanePointsTool::new())));
        let _ = view.take_tool_state();

        // The first pick lands on the slab's face, but a picking tool must not adopt it:
        // its points are the input, so the other two must not be flattened onto that
        // face's plane.
        view.handle_pointer([30.0, 50.0]);
        view.handle_button([30.0, 50.0], 1, true);
        assert!(
            view.active_workplane().is_default(),
            "a plane pick leaves the session plane alone"
        );
        assert_eq!(view.tool_status().expect("an active tool").points, 1);
    }

    #[test]
    fn escaping_the_three_point_pick_keeps_the_world_plane_and_commits_nothing() {
        let mut view = plane_points_view();
        let _ = view.take_tool_state();
        pick_triangle(&mut view);

        assert!(view.handle_key(&escape_key()));
        assert!(view.tool().is_none());
        assert!(
            view.active_workplane().is_default(),
            "a cancelled pick leaves the world plane"
        );
        assert!(view.take_interaction_commit().is_none());
        assert!(view.transient.is_empty(), "no transient geometry survives");
    }

    #[test]
    fn escape_cancels_the_tool_before_clearing_the_workplane() {
        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());
        view.set_tool(Some(Box::new(ferrocad_stage::PolylineTool::new())));

        // The first Escape ends the tool and leaves the workplane.
        assert!(view.handle_key(&escape_key()));
        assert!(view.tool().is_none());
        assert!(
            !view.active_workplane().is_default(),
            "the tool was cancelled first"
        );

        // The second Escape reaches the fallback and resets the workplane.
        assert!(view.handle_key(&escape_key()));
        assert!(view.active_workplane().is_default());
    }

    #[test]
    fn the_grid_steps_are_a_preference() {
        let mut view = grid_view();
        // A fixed extent: the identity camera sits at the origin, so its disc is
        // degenerate. The configured pair is what this test is about anyway.
        assert!(view.set_grid_radius(Some(500.0)));
        // The configured pair is what this test is about, so turn adaptive spacing off:
        // otherwise the drawn ground is sized from its disc, not from these numbers.
        assert!(view.set_grid_adaptive(false));
        assert_eq!(
            view.grid_steps(),
            (
                ferrocad_stage::grid::MINOR_STEP,
                ferrocad_stage::grid::MAJOR_STEP
            )
        );
        assert!(view.set_grid_steps(20.0, 200.0));
        assert_eq!(view.grid_steps(), (20.0, 200.0));

        // Every dot sits on the new pitch (the major lines are 200).
        let dots = &view.grid_queue.packets[0]
            .geometry
            .as_ref()
            .expect("fresh geometry")
            .vertices;
        assert!(!dots.is_empty());
        assert!(dots.iter().all(|dot| {
            let x = f64::from(dot.position[0]) / 20.0;
            let y = f64::from(dot.position[1]) / 20.0;
            (x - x.round()).abs() < 1e-4 && (y - y.round()).abs() < 1e-4
        }));

        // An invalid pair leaves the preference alone.
        assert!(!view.set_grid_steps(0.0, 100.0));
        assert!(!view.set_grid_steps(100.0, 10.0));
        assert_eq!(view.grid_steps(), (20.0, 200.0));
    }

    /// The reported case: a Toy-sized model, `adaptive` off, a **1 mm minor under a 10 mm
    /// major**. The minor lattice has to appear at 1 mm. The ground's disc used to floor
    /// at 500 mm whatever the camera was, and the generator's density budget
    /// (`DOT_COLUMNS`) is counted on the disc — so a disc 5–12× larger than the view
    /// silently coarsened the minor step (1 mm was drawn at 5 mm).
    #[test]
    fn a_fine_configured_minor_draws_on_a_small_model() {
        use ferrocad_graphics::Projection;
        use ferrocad_types::{Placement, Rotation, Vector3};

        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -40.0), Rotation::identity())
                .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 1_000.0,
            },
            viewport: ferrocad_graphics::Viewport::new([1000, 1000], 1.0),
        };
        let mut view = grid_view();
        view.camera = camera;
        assert!(view.set_grid_adaptive(false));
        assert!(view.set_grid_steps(1.0, 10.0));
        assert!(view.set_grid_stroke(grid::MinorStyle::Lines, 1.0, 1.0));
        assert!(view.set_axis_cross(false));

        let radius = ground_radius(&view.camera, view.grid_radius);
        assert!(
            (radius - 100.0).abs() < 1e-6,
            "2.5 × the eye distance, no 500 mm floor: {radius}"
        );
        let (minor, major) = plane_steps(None, radius, view.grid_policy());
        assert_eq!((minor, major), (1.0, 10.0), "the configured pair, uncoarsened");
        assert_eq!(
            grid::effective_steps(&view.camera, &Workplane::world(), minor, major),
            (1.0, 10.0),
            "a 1 mm step is ~30 px here, so the cull leaves it alone"
        );

        // The cross is off, so the first packet is the minor lattice.
        let lattice = view.grid_queue.packets[0]
            .geometry
            .as_ref()
            .expect("fresh geometry");
        assert!(!lattice.vertices.is_empty(), "the minor lattice draws");
        // Every vertex sits on a row or a column at an integer millimetre, and a row is
        // subdivided at the same pitch — so the set of integer coordinates *is* the set
        // of row offsets. A coarsened step (5 mm, say) would leave about a fifth as many.
        let mut offsets: Vec<i64> = Vec::new();
        for vertex in &lattice.vertices {
            for value in [
                f64::from(vertex.position[0]),
                f64::from(vertex.position[1]),
            ] {
                if (value - value.round()).abs() < 1e-6 {
                    offsets.push(value.round() as i64);
                }
            }
        }
        offsets.sort_unstable();
        offsets.dedup();
        assert!(
            offsets.len() > 120,
            "rows one millimetre apart across a 100 mm radius, not five: {}",
            offsets.len()
        );

        // Zooming out keeps the **pitch**: the disc grows with the eye distance, and the
        // minor level is clipped in *extent* by the generator instead of being coarsened.
        // This is the reported defect — the subdivisions used to change as the camera
        // pulled back.
        for eye in [100.0, 400.0, 1_000.0] {
            view.camera.view = Placement::new(Vector3::new(0.0, 0.0, -eye), Rotation::identity())
                .to_matrix();
            view.refresh();
            let radius = ground_radius(&view.camera, view.grid_radius);
            assert_eq!(
                plane_steps(None, radius, view.grid_policy()),
                (1.0, 10.0),
                "eye {eye}: the configured pair is not coarsened"
            );
            let lattice = view.grid_queue.packets[0]
                .geometry
                .as_ref()
                .expect("fresh geometry");
            assert!(
                !lattice.vertices.is_empty(),
                "eye {eye}: the minor level still draws (clipped, not dropped)"
            );
        }
    }

    #[test]
    fn a_fixed_grid_radius_sets_the_extent() {
        let mut view = grid_view();
        assert_eq!(view.grid_radius(), None, "camera-derived by default");
        assert!(view.set_grid_radius(Some(1000.0)));
        assert_eq!(view.grid_radius(), Some(1000.0));

        let dots = &view.grid_queue.packets[0]
            .geometry
            .as_ref()
            .expect("fresh geometry")
            .vertices;
        let furthest = dots
            .iter()
            .map(|dot| {
                (f64::from(dot.position[0]).powi(2) + f64::from(dot.position[1]).powi(2)).sqrt()
            })
            .fold(0.0, f64::max);
        assert!(
            furthest > 500.0,
            "beyond the identity camera's default: {furthest}"
        );
        assert!(
            furthest <= 1000.0 + 1e-3,
            "inside the fixed radius: {furthest}"
        );

        assert!(view.set_grid_radius(None));
        assert_eq!(view.grid_radius(), None);
        assert!(
            !view.set_grid_radius(Some(-1.0)),
            "a negative radius is ignored"
        );
    }

    #[test]
    fn a_grid_ink_and_alpha_reach_the_ground_vertices() {
        let mut view = grid_view();
        assert_eq!(view.grid_color(), ferrocad_stage::grid::GRID_COLOR);
        assert_eq!(view.grid_alpha(), 1.0);
        assert!(!view.set_grid_color(ferrocad_stage::grid::GRID_COLOR));
        assert!(!view.set_grid_alpha(1.0));

        // The radial fade means the brightest dot sits below the token alpha, so the
        // baseline comes from the geometry rather than from the constant.
        let peak_at_full = {
            let dots = &view.grid_queue.packets[0]
                .geometry
                .as_ref()
                .expect("fresh geometry")
                .vertices;
            assert!(!dots.is_empty());
            let peak = dots.iter().map(|dot| dot.color[3]).fold(0.0, f32::max);
            assert!(peak > 0.0 && peak <= ferrocad_stage::grid::GRID_ALPHA);
            peak
        };

        assert!(view.set_grid_alpha(0.5));
        assert!(view.set_grid_color([1.0, 0.0, 0.0, 1.0]));
        let dots = &view.grid_queue.packets[0]
            .geometry
            .as_ref()
            .expect("fresh geometry")
            .vertices;
        assert!(!dots.is_empty());
        let peak = dots.iter().map(|dot| dot.color[3]).fold(0.0, f32::max);
        assert!(
            (peak - peak_at_full * 0.5).abs() < 1e-6,
            "the alpha is a multiplier over the token: {peak} vs {peak_at_full} * 0.5"
        );
        assert!(
            dots.iter()
                .all(|dot| dot.color[0] == 1.0 && dot.color[1] == 0.0),
            "the ink reaches every dot"
        );

        // Out-of-range values are ignored, like the neighbouring setters.
        assert!(!view.set_grid_alpha(1.5));
        assert!(!view.set_grid_alpha(f32::NAN));
        assert!(!view.set_grid_color([f32::NAN, 0.0, 0.0, 1.0]));
        assert!(!view.set_grid_color([1.0, 2.0, 0.0, 1.0]));
        assert_eq!(view.grid_alpha(), 0.5);
        assert_eq!(view.grid_color(), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn view_workplane_looks_down_the_active_normal() {
        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());
        assert!(view.view_workplane());

        let target = view
            .snap
            .as_ref()
            .expect("a camera snap")
            .target()
            .to_matrix();
        let normal = view.active_workplane().plane.n;
        let in_view = target.transform(&normal);
        assert!(
            (in_view.z - 1.0).abs() < 1e-6 && in_view.x.abs() < 1e-6 && in_view.y.abs() < 1e-6,
            "the normal points at the eye: {in_view:?}"
        );
        // And the view is roll-free, so the navicube can represent it: world +Z has no
        // screen-x component in view space.
        let up = target.transform(&Vector3::new(0.0, 0.0, 1.0));
        assert!(up.x.abs() < 1e-9, "the horizon is level: {up:?}");
    }

    /// The standard planes are upstream's frames, right-handed, and the camera that
    /// looks down them is not mirrored.
    ///
    /// Cross-reference: FreeCAD's `WorkingPlane.PlaneBase.set_to_top` / `_front` /
    /// `_side` set `u`/`v`/`axis` to exactly these triples, and its right view (camera
    /// at `+X`) shows `+Y` to the right with `+Z` up. The `(Right)` label names the
    /// **viewpoint**, not the half of the canvas the plane is drawn on: the standard
    /// rows move the plane, not the camera, so a plane can sit anywhere on screen.
    #[test]
    fn the_standard_planes_match_upstream_and_stay_right_handed() {
        use ferrocad_stage::StandardPlane;

        let x = Vector3::new(1.0, 0.0, 0.0);
        let y = Vector3::new(0.0, 1.0, 0.0);
        let z = Vector3::new(0.0, 0.0, 1.0);
        let negative_y = y.scale(-1.0);

        // `screen_right` / `screen_up`: the world axis the snapped view must put on the
        // screen's right and top. `Top` looks down `+Z`, so its up is the plane's `+Y`.
        let cases = [
            (StandardPlane::Top, x, y, z, x, y),
            (StandardPlane::Front, x, z, negative_y, x, z),
            (StandardPlane::Right, y, z, x, y, z),
        ];

        for (spec, u, v, n, screen_right, screen_up) in cases {
            let mut view = identity_viewport(Scene::default());
            view.set_standard_plane(spec);
            let plane = view.active_workplane().plane.clone();
            assert!((plane.u.sub(&u)).length() < 1e-9, "{spec:?} u: {:?}", plane.u);
            assert!((plane.v.sub(&v)).length() < 1e-9, "{spec:?} v: {:?}", plane.v);
            assert!((plane.n.sub(&n)).length() < 1e-9, "{spec:?} n: {:?}", plane.n);
            // Right-handed, exactly as upstream's `u`, `v`, `axis`.
            assert!(
                (plane.u.cross(&plane.v).sub(&n)).length() < 1e-9,
                "{spec:?} must be right-handed"
            );

            assert!(view.view_workplane());
            let target = view
                .snap
                .as_ref()
                .expect("a camera snap")
                .target()
                .to_matrix();
            let right = target.transform(&screen_right);
            assert!(
                (right.x - 1.0).abs() < 1e-6 && right.y.abs() < 1e-6 && right.z.abs() < 1e-6,
                "{spec:?} mirrors: {screen_right:?} should be screen-right, got {right:?}"
            );
            let up = target.transform(&screen_up);
            assert!(
                (up.y - 1.0).abs() < 1e-6 && up.x.abs() < 1e-6 && up.z.abs() < 1e-6,
                "{spec:?} should keep {screen_up:?} up, got {up:?}"
            );
        }
    }

    #[test]
    fn a_local_workplane_never_re_labels_the_compass() {
        use ferrocad_stage::WorkplaneKind;

        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let cube = NaviCube::new(camera.viewport);
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, Scene::default())
            .with_navicube(cube);
        let before: Vec<(char, [f32; 2])> = view
            .navicube
            .as_ref()
            .expect("a navicube")
            .labels()
            .into_iter()
            .map(|label| (label.text, label.position))
            .collect();

        // A frame whose `U` points along world +Y must not turn the letters: the cube is
        // the world compass (FreeCAD has no local compass either).
        let plane = Workplane::new(
            Vector3::new(0.0, 0.0, 0.5),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
            WorkplaneKind::Datum("Rotated".into()),
        )
        .expect("a frame");
        view.set_workplane(Some(FaceFrame { plane, radius: 5.0 }));
        view.refresh();

        let after: Vec<(char, [f32; 2])> = view
            .navicube
            .as_ref()
            .expect("a navicube")
            .labels()
            .into_iter()
            .map(|label| (label.text, label.position))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn the_triad_tips_carry_axis_tags() {
        use ferrocad_stage::WorkplaneKind;

        let mut view = grid_view();
        let texts: Vec<char> = view.axis_labels().iter().map(|label| label.text).collect();
        assert_eq!(texts, vec!['X', 'Y', 'Z']);

        let plane = Workplane::new(
            Vector3::new(0.0, 0.0, 0.5),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            WorkplaneKind::Datum("Local".into()),
        )
        .expect("a frame");
        view.set_workplane(Some(FaceFrame { plane, radius: 5.0 }));
        let texts: Vec<char> = view.axis_labels().iter().map(|label| label.text).collect();
        assert_eq!(texts, vec!['U', 'V', 'N']);
    }

    #[test]
    fn the_default_workplane_is_the_world_xy_plane() {
        let mut view = slab_view();
        let active = view.active_workplane();
        assert!(active.is_default(), "the world XY plane is the default");
        assert_eq!(active.label, "XY");
        assert_eq!(active.plane.origin, Vector3::zero());
        assert_eq!(active.plane.n, Vector3::new(0.0, 0.0, 1.0));

        // Looking down the default plane is the top view, not a failure.
        assert!(view.view_workplane());
        assert!(view.active_workplane().is_default());
    }

    #[test]
    fn a_bound_face_is_labelled_by_its_element_name() {
        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());
        let active = view.active_workplane();
        assert!(!active.is_default(), "a bound face is not the default plane");
        assert_eq!(active.label, "Face1", "the slab's element name");
    }

    #[test]
    fn the_standard_planes_are_named_placed_and_reset() {
        use ferrocad_stage::StandardPlane;

        let mut view = slab_view();
        assert!(view.set_standard_plane(StandardPlane::Front));
        let active = view.active_workplane();
        assert_eq!(active.label, "XZ");
        assert_eq!(active.plane.origin, Vector3::zero());
        assert_eq!(active.plane.u, Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(active.plane.v, Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(active.plane.n, Vector3::new(0.0, -1.0, 0.0));
        assert!(!active.is_default(), "XZ is a standard, not the default");

        assert!(view.set_standard_plane(StandardPlane::Right));
        let active = view.active_workplane();
        assert_eq!(active.label, "YZ");
        assert_eq!(active.plane.n, Vector3::new(1.0, 0.0, 0.0));

        // `Top` is the world `XY` plane, so it is the default even via this path.
        assert!(view.set_standard_plane(StandardPlane::Top));
        assert!(view.active_workplane().is_default());
        assert!(!view.set_standard_plane(StandardPlane::Top), "already there");
    }

    #[test]
    fn an_explicit_frame_is_labelled_custom_and_a_degenerate_one_is_refused() {
        let mut view = slab_view();
        assert!(view.set_workplane_frame(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        ));
        let active = view.active_workplane();
        assert_eq!(active.label, "Custom");
        assert_eq!(active.plane.origin, Vector3::new(1.0, 2.0, 3.0));
        // `u × v = Y × X = -Z`, so the frame is right-handed as given.
        assert_eq!(active.plane.n, Vector3::new(0.0, 0.0, -1.0));
        assert!(!active.is_default());

        // The axes are normalized; a zero or parallel pair is not a plane.
        let bound = view.active_workplane().clone();
        assert!(!view.set_workplane_frame(
            Vector3::zero(),
            Vector3::zero(),
            Vector3::new(0.0, 1.0, 0.0),
        ));
        assert!(!view.set_workplane_frame(
            Vector3::zero(),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
        ));
        assert_eq!(view.active_workplane(), &bound, "a refusal changes nothing");
    }

    #[test]
    fn moving_the_plane_moves_the_only_grid() {
        use ferrocad_stage::StandardPlane;

        let mut view = slab_view();
        // A three-quarter view: the top view is edge-on to `XZ`, where the grid now
        // (correctly) fades out.
        view.camera = gizmo_camera([100, 100]);
        view.refresh();
        // The cross off: the grid alone draws its own levels (which ones survive the
        // screen-space cull depends on the adaptive step, so count the layer, not the
        // packets).
        assert!(view.set_axis_cross(false));
        let world = view.grid_queue.len();
        assert!(world >= 1, "the world ground");
        assert!(
            view.grid_queue
                .packets
                .iter()
                .any(|packet| packet.topology == Topology::Lines),
            "the world ground's major lines"
        );

        // The plane moving must not stack a second, dimmer grid underneath it: the
        // active plane *is* the one grid, so the packet count stays the same.
        assert!(view.set_standard_plane(StandardPlane::Front));
        assert_eq!(
            view.grid_queue.len(),
            world,
            "one grid, moved — not the world ground plus a local one"
        );
    }

    #[test]
    fn the_view_plane_follows_the_camera() {
        use ferrocad_types::{Placement, Rotation, Vector3};

        // A camera at `+Y` looking toward the origin: its screen-right is `+X`,
        // screen-up is `-Z`, so the plane's normal points back at the viewer (`+Y`).
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -5.0),
                Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 90.0_f64.to_radians()),
            )
            .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
        };
        let mut view = InventorViewport::new(fake(ImageData::default()), camera, Scene::default());

        assert!(view.set_view_plane());
        let active = view.active_workplane();
        assert_eq!(active.label, "View");
        assert_eq!(active.plane.origin, Vector3::zero(), "the camera looks at the origin");
        assert!(active.plane.u.is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-9));
        assert!(active.plane.v.is_equal(&Vector3::new(0.0, 0.0, -1.0), 1e-9));
        assert!(active.plane.n.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));
        assert!(!active.is_default());
        assert_eq!(active.plane.kind, ferrocad_stage::WorkplaneKind::Custom);
    }

    #[test]
    fn a_selected_face_becomes_the_workplane() {
        let mut view = slab_view();
        assert!(view.set_workplane_from_element(7, Some("Face2")));
        let active = view.active_workplane();
        assert_eq!(active.label, "Face2");
        // The slab's right half sits at `x = 0.25` on the `z = 0.5` top.
        assert!((active.plane.origin.x - 0.25).abs() < 1e-9);
        assert!((active.plane.origin.z - 0.5).abs() < 1e-9);

        // An edge is not a plane, and an unselected body has no single plane.
        assert!(!view.set_workplane_from_element(7, Some("Edge1")));
        assert!(!view.set_workplane_from_element(7, None));
        assert!(!view.set_workplane_from_element(99, Some("Face1")));
        assert_eq!(view.active_workplane().label, "Face2", "a failure changes nothing");
    }

    #[test]
    fn resetting_the_default_plane_is_a_no_op() {
        let mut view = slab_view();
        assert!(!view.clear_workplane(), "already at World XY");
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());
        assert!(view.clear_workplane(), "a bound plane resets");
        assert!(view.active_workplane().is_default());
    }

    #[test]
    fn a_tool_binds_the_face_it_starts_on_and_restores_it_afterwards() {
        let mut view = tool_view();
        let _ = view.take_tool_state();

        // The first click lands on the slab's left face (z = 0.5): the face becomes the
        // session's plane, with no explicit activation step.
        view.handle_pointer([30.0, 50.0]);
        view.handle_button([30.0, 50.0], 1, true);
        view.handle_button([30.0, 50.0], 1, false);
        let active = view.active_workplane();
        assert!(!active.is_default(), "the click bound the face");
        assert_eq!(active.label, "Face1");
        assert!((active.plane.origin.z - 0.5).abs() < 1e-6);

        // A click in empty space still projects onto that plane, so both committed
        // points keep the face's z.
        view.handle_pointer([10.0, 10.0]);
        view.handle_button([10.0, 10.0], 1, true);
        assert_eq!(view.tool_status().expect("an active tool").points, 2);

        assert!(view.handle_key(&key("enter")));
        let commit = view.take_interaction_commit().expect("a commit");
        assert_eq!(commit.values.len(), 6, "two points");
        for z in [commit.values[2], commit.values[5]] {
            assert!(
                (z - 0.5).abs() < 1e-6,
                "both points lie on the face plane: {z}"
            );
        }

        // Finishing restores the plane the session started from.
        assert!(view.active_workplane().is_default());
    }

    #[test]
    fn offsetting_the_plane_moves_its_origin_and_says_so() {
        use ferrocad_stage::StandardPlane;

        let mut view = slab_view();
        assert!(!view.offset_workplane(0.0), "zero moves nothing");
        assert!(!view.offset_workplane(f64::NAN), "a NaN is refused");

        assert!(view.offset_workplane(25.0));
        let active = view.active_workplane();
        assert_eq!(active.plane.origin, Vector3::new(0.0, 0.0, 25.0));
        assert_eq!(active.label, "XY", "the datum keeps its name");
        assert_eq!(active.display_label(), "XY +25");
        assert!(!active.is_default(), "a shifted plane is not the default");

        // Offsetting back to zero restores the default state exactly.
        assert!(view.offset_workplane(-25.0));
        assert!(view.active_workplane().is_default());
        assert_eq!(view.active_workplane().display_label(), "XY");

        // A standard plane offsets along **its own** normal (`XZ` looks along `+Y`, so
        // its normal is `-Y`).
        assert!(view.set_standard_plane(StandardPlane::Front));
        assert!(view.offset_workplane(4.0));
        assert_eq!(
            view.active_workplane().plane.origin,
            Vector3::new(0.0, -4.0, 0.0)
        );
        assert_eq!(view.active_workplane().display_label(), "XZ +4");
    }

    #[test]
    fn a_later_click_does_not_move_the_session_plane() {
        let mut view = tool_view();
        let _ = view.take_tool_state();

        // The session starts on Face1…
        view.handle_pointer([30.0, 50.0]);
        view.handle_button([30.0, 50.0], 1, true);
        assert_eq!(view.active_workplane().label, "Face1");

        // …and a click on the neighbouring face mid-session must not rebind: the points
        // already placed belong to the first plane.
        view.handle_pointer([70.0, 50.0]);
        view.handle_button([70.0, 50.0], 1, true);
        assert_eq!(view.active_workplane().label, "Face1");
        assert_eq!(view.tool_status().expect("an active tool").points, 2);
    }

    #[test]
    fn a_tool_started_in_empty_space_never_binds_a_face() {
        let mut view = tool_view();
        let _ = view.take_tool_state();
        // Snapping is what lets an empty-space click land on the plane at all; without
        // it a click off geometry places nothing and the session has not really begun.
        view.set_snap_grid(Some(0.25));

        view.handle_pointer([10.0, 10.0]);
        view.handle_button([10.0, 10.0], 1, true);
        assert!(
            view.active_workplane().is_default(),
            "the first click was empty, so the session stays on World XY"
        );

        view.handle_pointer([30.0, 50.0]);
        view.handle_button([30.0, 50.0], 1, true);
        assert!(
            view.active_workplane().is_default(),
            "the first point settles the session's plane"
        );
    }

    #[test]
    fn an_explicit_plane_survives_a_tool_session() {
        let mut view = slab_view();
        view.handle_pointer([30.0, 50.0]);
        assert!(view.activate_hovered_workplane());
        let explicit = view.active_workplane().clone();

        // A tool started while a plane is bound does not re-bind, and finishing leaves
        // the explicit plane alone.
        view.set_tool(Some(Box::new(ferrocad_stage::PolylineTool::new())));
        view.handle_pointer([70.0, 50.0]);
        view.handle_button([70.0, 50.0], 1, true);
        assert_eq!(
            view.active_workplane(),
            &explicit,
            "an explicit plane is sticky"
        );
        assert!(view.handle_key(&key("enter")));
        assert_eq!(view.active_workplane(), &explicit);
    }
}

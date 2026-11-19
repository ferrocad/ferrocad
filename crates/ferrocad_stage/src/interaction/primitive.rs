//! The round primitives — **cylinder**, **sphere** and **cone** — as placement tools.
//!
//! These are [`BoxTool`](crate::interaction::BoxTool)'s gesture for the other SimpleCAD
//! solids: a press anchors the base (the centre, for a sphere), the strip's numbers are the
//! input, and `Enter`/`✓` commits. The three differ only in their parameters and their
//! preview mesh, so they share one tool: a [`PrimitiveKind`] plus the values, the placement
//! frame and two mesh slots.
//!
//! The base sits on the placement frame's **origin** and the axis runs along its **normal**,
//! so a cylinder on a face grows out of that face. The commit carries that normal
//! (`values` end in `nx, ny, nz`), because `Part::Cylinder` and `Part::Cone` want a
//! direction; a sphere is isotropic and carries only its centre.
//!
//! The ghost is a read, not geometry: a fixed tessellation the eye can follow, never the
//! kernel's tessellation of what the commit will make.

use ferrocad_types::Vector3;

use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::{HoverTarget, InteractionCommit};
use crate::shape::ShapeItem;
use crate::snap::Snap;
use crate::tool::{
    CURSOR_COLOR, HOVER_MARKER_HALF, Tool, ToolField, ToolStatus, ghost_item, set_ghost_loop,
    set_polyline,
};

/// Segments around a preview ring (and meridians on a sphere).
///
/// The silhouette has to stay round at any zoom the grid is legible at, without turning a
/// hover move into tessellation work: 32 is the smallest count whose facets a 1 px edge
/// never reveals.
pub const PRIMITIVE_SEGMENTS: usize = 32;
/// Latitude bands in the sphere preview.
pub const SPHERE_RINGS: usize = 12;
/// The smallest acceptable size, in millimetres.
pub const PRIMITIVE_MIN_SIZE: f64 = 1e-3;
/// The largest acceptable size, in millimetres.
pub const PRIMITIVE_MAX_SIZE: f64 = 1e6;

/// The action every primitive tool's commit carries.
pub const PRIMITIVE_ACTION_CREATE: &str = "create";

/// Which round solid a [`PrimitiveTool`] places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveKind {
    /// A radius and a height.
    Cylinder,
    /// A radius.
    Sphere,
    /// A base radius, a top radius and a height; `R2 = 0` is a cone, otherwise a frustum.
    Cone,
}

impl PrimitiveKind {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::Cylinder, Self::Sphere, Self::Cone];

    /// The tool id, which is also the manipulator id its commit carries.
    pub fn id(self) -> &'static str {
        match self {
            Self::Cylinder => "cylinder",
            Self::Sphere => "sphere",
            Self::Cone => "cone",
        }
    }

    /// The label the completion row shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Cylinder => "Cylinder",
            Self::Sphere => "Sphere",
            Self::Cone => "Cone",
        }
    }

    /// The status hint: the gesture, then the numbers.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Cylinder => "type a size · click to place the base · Enter to create · Esc to cancel",
            Self::Sphere => "type a radius · click to place the centre · Enter to create · Esc to cancel",
            Self::Cone => "type a size · click to place the base · Enter to create · Esc to cancel",
        }
    }
}

/// A cylinder, sphere or cone with its parameters, previewed at the placement frame.
#[derive(Clone, Debug)]
pub struct PrimitiveTool {
    kind: PrimitiveKind,
    /// The radius — the base radius for a cone.
    radius: f64,
    /// The cone's top radius. A cylinder ignores it; a sphere has none.
    top_radius: f64,
    /// The height. A sphere ignores it.
    height: f64,
    /// The placement under the pointer while nothing is placed.
    hover: Option<HoverTarget>,
    /// The base point (the centre, for a sphere) a press anchored, in world space.
    anchor: Option<Vector3>,
    /// Two fixed slots — `[ghost, anchor cue]` — whose mesh identities are stable.
    items: Vec<ShapeItem>,
}

impl Default for PrimitiveTool {
    fn default() -> Self {
        Self::cylinder()
    }
}

impl PrimitiveTool {
    /// A tool for `kind`, with that solid's default size.
    pub fn new(kind: PrimitiveKind) -> Self {
        let (radius, top_radius, height) = match kind {
            PrimitiveKind::Cylinder => (5.0, 5.0, 20.0),
            PrimitiveKind::Sphere => (10.0, 10.0, 20.0),
            // A cone by default: one radius, an apex, the box's height.
            PrimitiveKind::Cone => (10.0, 0.0, 20.0),
        };
        let mut tool = Self {
            kind,
            radius,
            top_radius,
            height,
            hover: None,
            anchor: None,
            items: vec![ghost_item(CURSOR_COLOR), ghost_item(CURSOR_COLOR)],
        };
        // Ready before any pointer event: the preview is the strip's numbers.
        tool.rebuild();
        tool
    }

    /// A cylinder with the Solid tier's default size.
    pub fn cylinder() -> Self {
        Self::new(PrimitiveKind::Cylinder)
    }

    /// A sphere with the Solid tier's default size.
    pub fn sphere() -> Self {
        Self::new(PrimitiveKind::Sphere)
    }

    /// A cone with the Solid tier's default size.
    pub fn cone() -> Self {
        Self::new(PrimitiveKind::Cone)
    }

    /// Which solid this tool places.
    pub fn kind(&self) -> PrimitiveKind {
        self.kind
    }

    /// The base point (a sphere's centre): the press, else the placement frame's origin,
    /// else the world origin.
    pub fn origin(&self) -> Vector3 {
        if let Some(anchor) = self.anchor {
            return anchor;
        }
        self.hover
            .as_ref()
            .map(|target| target.frame.origin)
            .unwrap_or_else(Vector3::zero)
    }

    /// The placement basis: the hovered frame's, or the world's.
    ///
    /// The hover target survives the press (`Tool::hover` is not called once a point is
    /// placed), so an anchored solid keeps the plane its ghost grew out of.
    fn basis(&self) -> [Vector3; 3] {
        self.hover.as_ref().map(|target| target.frame.basis).unwrap_or([
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ])
    }

    /// The world direction the solid's axis runs along (`values`' trailing triplet).
    fn axis(&self) -> Vector3 {
        self.basis()[2]
    }

    /// Whether the parameters describe a solid at all: a positive size, and for a cone at
    /// least one end open enough to make a shape.
    fn is_valid(&self) -> bool {
        let sized = |value: f64| (PRIMITIVE_MIN_SIZE..=PRIMITIVE_MAX_SIZE).contains(&value);
        match self.kind {
            PrimitiveKind::Cylinder => sized(self.radius) && sized(self.height),
            PrimitiveKind::Sphere => sized(self.radius),
            PrimitiveKind::Cone => {
                // At least one end open, and no end thinner than the minimum.
                let open = |value: f64| value == 0.0 || sized(value);
                (self.radius > 0.0 || self.top_radius > 0.0)
                    && sized(self.height)
                    && open(self.radius)
                    && open(self.top_radius)
            }
        }
    }

    /// Rewrite the ghost mesh from the parameters and the placement.
    fn rebuild(&mut self) {
        let [u, v, n] = self.basis();
        let origin = self.origin();
        let (local, triangles, edges) = match self.kind {
            PrimitiveKind::Cylinder => revolve(self.radius, self.radius, self.height),
            PrimitiveKind::Sphere => ball(self.radius),
            PrimitiveKind::Cone => revolve(self.radius, self.top_radius, self.height),
        };
        let positions = local
            .into_iter()
            .map(|[x, y, z]| {
                let world = origin
                    .add(&u.scale(f64::from(x)))
                    .add(&v.scale(f64::from(y)))
                    .add(&n.scale(f64::from(z)));
                [world.x as f32, world.y as f32, world.z as f32]
            })
            .collect();
        self.items[0].set_shape(positions, Vec::new(), triangles, edges);
        // The anchor cue marks the placement the **next** press would use. Once the base is
        // anchored the solid's own base ring is the mark, so the cue goes away.
        let cue = self
            .hover
            .as_ref()
            .map(|target| target.marker(HOVER_MARKER_HALF))
            .unwrap_or_default();
        set_ghost_loop(&mut self.items[1], &cue);
        if self.anchor.is_some() {
            set_polyline(&mut self.items[1], &[]);
        }
    }

    /// The strip's fields, from the current parameters.
    fn fields(&self) -> Vec<ToolField> {
        match self.kind {
            PrimitiveKind::Cylinder => vec![
                radius_field("r", "R", self.radius, PRIMITIVE_MIN_SIZE),
                height_field(self.height),
            ],
            PrimitiveKind::Sphere => vec![radius_field("r", "R", self.radius, PRIMITIVE_MIN_SIZE)],
            PrimitiveKind::Cone => vec![
                // Either end may close to a point; the commit refuses both at once.
                radius_field("r1", "R1", self.radius, 0.0),
                radius_field("r2", "R2", self.top_radius, 0.0),
                height_field(self.height),
            ],
        }
    }

    /// The commit's numbers: the parameters, then the base point, then the axis for a
    /// solid that has one.
    fn values(&self) -> Vec<f64> {
        let origin = self.origin();
        let mut values = match self.kind {
            PrimitiveKind::Cylinder => vec![self.radius, self.height],
            PrimitiveKind::Sphere => vec![self.radius],
            PrimitiveKind::Cone => vec![self.radius, self.top_radius, self.height],
        };
        values.extend_from_slice(&[origin.x, origin.y, origin.z]);
        if self.kind != PrimitiveKind::Sphere {
            let axis = self.axis();
            values.extend_from_slice(&[axis.x, axis.y, axis.z]);
        }
        values
    }

    /// The readout the committing surface shows.
    fn readout(&self) -> String {
        match self.kind {
            PrimitiveKind::Cylinder => {
                format!("r {:.1} × h {:.1} mm", self.radius, self.height)
            }
            PrimitiveKind::Sphere => format!("r {:.1} mm", self.radius),
            PrimitiveKind::Cone => format!(
                "r1 {:.1} · r2 {:.1} × h {:.1} mm",
                self.radius, self.top_radius, self.height
            ),
        }
    }

    /// Set one parameter from typed text, refusing what is not a usable length.
    fn edit(&mut self, id: &str, text: &str) -> bool {
        let Ok(value) = text.trim().parse::<f64>() else {
            return false;
        };
        if !value.is_finite() || !(0.0..=PRIMITIVE_MAX_SIZE).contains(&value) {
            return false;
        }
        let sized = value >= PRIMITIVE_MIN_SIZE;
        // A cone's radii may close to a point — at either end — so they accept zero;
        // `finish` is what refuses a solid with no end at all.
        let sized_or_closed = value == 0.0 || sized;
        match (self.kind, id) {
            (PrimitiveKind::Cylinder | PrimitiveKind::Sphere, "r") if sized => self.radius = value,
            (PrimitiveKind::Cone, "r1") if sized_or_closed => self.radius = value,
            (PrimitiveKind::Cone, "r2") if sized_or_closed => self.top_radius = value,
            (PrimitiveKind::Cylinder | PrimitiveKind::Cone, "h") if sized => self.height = value,
            _ => return false,
        }
        self.rebuild();
        true
    }
}

/// A radius field (`R`, `R1`, `R2`), bounded below by `min`.
fn radius_field(id: &str, label: &str, value: f64, min: f64) -> ToolField {
    ToolField {
        min: Some(min),
        max: Some(PRIMITIVE_MAX_SIZE),
        step: 1.0,
        ..ToolField::number(id, label, value, "mm")
    }
}

/// The height field (`H`).
fn height_field(value: f64) -> ToolField {
    ToolField {
        min: Some(PRIMITIVE_MIN_SIZE),
        max: Some(PRIMITIVE_MAX_SIZE),
        step: 1.0,
        ..ToolField::number("h", "H", value, "mm")
    }
}

/// A preview mesh in the solid's own frame: positions, triangles and edges.
type Mesh = (Vec<[f32; 3]>, Vec<u32>, Vec<u32>);

/// A frustum of revolution about `+Z`: `bottom` radius at `z = 0`, `top` radius at
/// `z = height`.
///
/// `top == 0` is a cone, `bottom == top` a cylinder: one generator covers all three rings
/// the Solid tier needs, so the preview cannot disagree with the parameter list.
fn revolve(bottom: f64, top: f64, height: f64) -> Mesh {
    use std::f64::consts::TAU;

    let n = PRIMITIVE_SEGMENTS;
    // 0 = base centre, 1 = top centre, 2..2+n = base ring, 2+n..2+2n = top ring.
    let mut positions = Vec::with_capacity(2 * n + 2);
    positions.push([0.0, 0.0, 0.0]);
    positions.push([0.0, 0.0, height as f32]);
    for (radius, z) in [(bottom, 0.0), (top, height)] {
        for segment in 0..n {
            let angle = TAU * segment as f64 / n as f64;
            positions.push([
                (radius * angle.cos()) as f32,
                (radius * angle.sin()) as f32,
                z as f32,
            ]);
        }
    }
    let base = |segment: usize| (2 + segment) as u32;
    let cap = |segment: usize| (2 + n + segment) as u32;
    let mut triangles = Vec::with_capacity(6 * n);
    let mut edges = Vec::with_capacity(4 * n + 8);
    for segment in 0..n {
        let next = (segment + 1) % n;
        // The base fan faces −Z, the top fan +Z, and the wall's quads face outward for a
        // ring wound counter-clockwise about the axis.
        triangles.extend_from_slice(&[0, base(next), base(segment)]);
        triangles.extend_from_slice(&[1, cap(segment), cap(next)]);
        triangles.extend_from_slice(&[
            base(segment),
            base(next),
            cap(next),
            base(segment),
            cap(next),
            cap(segment),
        ]);
        edges.extend_from_slice(&[base(segment), base(next)]);
        edges.extend_from_slice(&[cap(segment), cap(next)]);
    }
    // Four wall lines, so the silhouette reads as a solid even where the outline is a
    // single stroke.
    for segment in (0..n).step_by(n / 4) {
        edges.extend_from_slice(&[base(segment), cap(segment)]);
    }
    (positions, triangles, edges)
}

/// A UV sphere about its centre: latitude bands crossed by `PRIMITIVE_SEGMENTS` meridians.
fn ball(radius: f64) -> Mesh {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    let n = PRIMITIVE_SEGMENTS;
    let rings = SPHERE_RINGS;
    let mut positions = Vec::with_capacity((rings + 1) * n);
    for ring in 0..=rings {
        let latitude = -FRAC_PI_2 + PI * ring as f64 / rings as f64;
        let (z, arm) = (radius * latitude.sin(), radius * latitude.cos());
        for segment in 0..n {
            let longitude = TAU * segment as f64 / n as f64;
            positions.push([
                (arm * longitude.cos()) as f32,
                (arm * longitude.sin()) as f32,
                z as f32,
            ]);
        }
    }
    let at = |ring: usize, segment: usize| (ring * n + segment) as u32;
    let mut triangles = Vec::with_capacity(6 * rings * n);
    for ring in 0..rings {
        for segment in 0..n {
            let next = (segment + 1) % n;
            triangles.extend_from_slice(&[
                at(ring, segment),
                at(ring, next),
                at(ring + 1, next),
                at(ring, segment),
                at(ring + 1, next),
                at(ring + 1, segment),
            ]);
        }
    }
    let mut edges = Vec::with_capacity(2 * n + rings * 4);
    // The equator, then four meridians.
    let equator = rings / 2;
    for segment in 0..n {
        edges.extend_from_slice(&[at(equator, segment), at(equator, (segment + 1) % n)]);
    }
    for segment in (0..n).step_by(n / 4) {
        for ring in 0..rings {
            edges.extend_from_slice(&[at(ring, segment), at(ring + 1, segment)]);
        }
    }
    (positions, triangles, edges)
}

impl Tool for PrimitiveTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        if event.kind == PointerKind::Down
            && event.button == Some(MouseButton::Left)
            && let Some(point) = target
        {
            // A press anchors the base (a sphere's centre). It is not a completion:
            // `Enter`/`✓` commits, so the numbers can be typed after the placement (and a
            // second press moves the base rather than creating a second solid).
            self.anchor = Some(crate::frame::vector(point));
            self.rebuild();
        }
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn hover(&mut self, target: Option<&HoverTarget>) {
        if self.anchor.is_some() {
            return;
        }
        self.hover = target.cloned();
        self.rebuild();
    }

    fn anchor(&self) -> Option<Vector3> {
        // The base the solid grows from: the press once it is placed, else the placement
        // the ghost is sitting on.
        self.anchor
            .or_else(|| self.hover.as_ref().map(|target| target.frame.origin))
    }

    fn status(&self) -> ToolStatus {
        ToolStatus::new(
            self.kind.id(),
            self.kind.label(),
            usize::from(self.anchor.is_some()),
            self.kind.hint(),
        )
        .with_fields(self.fields())
    }

    fn set_field(&mut self, id: &str, text: &str) -> bool {
        self.edit(id, text)
    }

    fn cancel(&mut self) -> bool {
        let had = self.hover.is_some() || self.anchor.is_some();
        self.hover = None;
        self.anchor = None;
        self.rebuild();
        had
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        if !self.is_valid() {
            return None;
        }
        let commit = InteractionCommit::new(self.kind.id(), 0, PRIMITIVE_ACTION_CREATE)
            .with_values(self.values())
            .with_readout(self.readout())
            .creating();
        self.hover = None;
        self.anchor = None;
        self.rebuild();
        Some(commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::Tool;

    /// The vertical extent of the ghost, so a test can see where the frame put it.
    fn extent(tool: &PrimitiveTool) -> (f32, f32) {
        tool.items()[0]
            .positions()
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), point| {
                (low.min(point[2]), high.max(point[2]))
            })
    }

    fn click(tool: &mut PrimitiveTool, point: [f32; 3]) {
        tool.on_event(
            &PointerEvent {
                kind: PointerKind::Down,
                position: [0, 0],
                button: Some(MouseButton::Left),
                hit: None,
                projection: None,
            },
            Some(Snap {
                point,
                kind: crate::snap::SnapKind::Point,
                local: None,
                major: false,
            }),
        );
    }

    #[test]
    fn each_kind_offers_its_own_parameters() {
        let cylinder = PrimitiveTool::cylinder();
        assert_eq!(
            cylinder
                .status()
                .fields
                .iter()
                .map(|field| field.id.as_str())
                .collect::<Vec<_>>(),
            vec!["r", "h"]
        );
        assert_eq!(cylinder.status().id, "cylinder");
        assert_eq!(cylinder.status().fields[0].value, "5.00");

        let sphere = PrimitiveTool::sphere();
        assert_eq!(
            sphere
                .status()
                .fields
                .iter()
                .map(|field| field.id.as_str())
                .collect::<Vec<_>>(),
            vec!["r"],
            "a sphere has one number"
        );
        assert_eq!(sphere.status().id, "sphere");

        let cone = PrimitiveTool::cone();
        assert_eq!(
            cone.status()
                .fields
                .iter()
                .map(|field| field.id.as_str())
                .collect::<Vec<_>>(),
            vec!["r1", "r2", "h"]
        );
        assert_eq!(cone.status().fields[1].value, "0.00", "a cone by default");
        // The preview is drawn before any pointer event: the solid is ready immediately.
        assert!(!cylinder.items()[0].positions().is_empty());
        assert!(!sphere.items()[0].positions().is_empty());
        assert!(!cone.items()[0].positions().is_empty());
    }

    #[test]
    fn a_field_write_back_resizes_the_ghost_and_the_commit() {
        let mut tool = PrimitiveTool::cylinder();
        assert!(tool.set_field("r", "2.5"));
        assert!(tool.set_field("h", "7"));
        // The top ring sits `h` above the base at radius `r`.
        let positions = tool.items()[0].positions().to_vec();
        let top = positions[2 + PRIMITIVE_SEGMENTS];
        assert!((top[2] - 7.0).abs() < 1e-6, "the height reached the ghost: {top:?}");
        assert!((top[0].hypot(top[1]) - 2.5).abs() < 1e-6);

        // A refusal leaves the tool as it was: `R2` is not a cylinder's field, a height of
        // zero is not a solid, and a word is not a number.
        assert!(!tool.set_field("r2", "3"));
        assert!(!tool.set_field("h", "0"));
        assert!(!tool.set_field("r", "wide"));
        let commit = tool.finish().expect("a commit");
        assert_eq!(commit.manipulator, "cylinder");
        assert_eq!(commit.action, PRIMITIVE_ACTION_CREATE);
        assert!(commit.creates);
        // r, h, x, y, z, n
        assert_eq!(&commit.values[0..2], &[2.5, 7.0]);
        assert_eq!(&commit.values[2..5], &[0.0, 0.0, 0.0]);
        assert_eq!(&commit.values[5..8], &[0.0, 0.0, 1.0], "the axis is the frame's normal");
        assert_eq!(commit.readout, "r 2.5 × h 7.0 mm");
    }

    #[test]
    fn a_click_anchors_the_base_on_the_placement() {
        let mut tool = PrimitiveTool::sphere();
        assert_eq!(tool.anchor(), None, "nothing placed and no hover: no anchor");

        let target = HoverTarget::of_workplane(&crate::Workplane::world());
        tool.hover(Some(&target));
        assert_eq!(tool.anchor(), Some(target.frame.origin), "the ghost's centre");

        click(&mut tool, [2.0, 3.0, 4.0]);
        assert_eq!(tool.anchor(), Some(Vector3::new(2.0, 3.0, 4.0)));
        assert_eq!(tool.status().points, 1);
        // The sphere is centred on the frame origin, so its extent straddles the press.
        assert_eq!(extent(&tool), (-6.0, 14.0), "the ghost is centred on the press");
        let commit = tool.finish().expect("a commit");
        // A sphere carries its radius and its centre, and no axis.
        assert_eq!(commit.values, vec![10.0, 2.0, 3.0, 4.0]);
        assert_eq!(commit.readout, "r 10.0 mm");
    }

    #[test]
    fn a_cone_is_a_frustum_until_its_top_closes() {
        let mut tool = PrimitiveTool::cone();
        // The default is a cone (R2 = 0) and still commits.
        let commit = tool.finish().expect("an apex cone is a solid");
        assert_eq!(&commit.values[0..3], &[10.0, 0.0, 20.0]);

        // Two closed ends is a degenerate shape: nothing to commit.
        let mut flat = PrimitiveTool::cone();
        assert!(flat.set_field("r1", "0"));
        assert_eq!(flat.finish(), None, "a cone needs one end open");

        // A frustum commits both radii.
        let mut frustum = PrimitiveTool::cone();
        assert!(frustum.set_field("r2", "4"));
        let commit = frustum.finish().expect("a frustum is a solid");
        assert_eq!(&commit.values[0..3], &[10.0, 4.0, 20.0]);
        assert_eq!(commit.readout, "r1 10.0 · r2 4.0 × h 20.0 mm");
    }

    #[test]
    fn every_kind_names_itself() {
        let ids = PrimitiveKind::ALL.map(PrimitiveKind::id);
        assert_eq!(ids, ["cylinder", "sphere", "cone"]);
        // The id is the manipulator the commit carries, so the labels and hints must not
        // collapse onto one kind.
        let mut labels = PrimitiveKind::ALL.map(PrimitiveKind::label).to_vec();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), PrimitiveKind::ALL.len());
        for kind in PrimitiveKind::ALL {
            assert_eq!(PrimitiveTool::new(kind).status().id, kind.id());
        }
    }

    #[test]
    fn a_cancel_clears_the_placement_and_the_anchor() {
        let mut tool = PrimitiveTool::cylinder();
        click(&mut tool, [1.0, 1.0, 1.0]);
        assert!(tool.cancel());
        assert_eq!(tool.status().points, 0);
        assert_eq!(tool.anchor(), None);
        assert!(!tool.cancel(), "a second cancel has nothing left to drop");
    }
}

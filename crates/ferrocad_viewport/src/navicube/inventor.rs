//! The inventor producer: builds the navigation cube as a `pivy.coin`-style [`Scene`].
//!
//! The counterpart of FreeCAD's `SoNaviCube`: the cube subtree (a `Transform` the
//! controller writes the camera orientation into, then the pieces), plus a fixed Home
//! button. It is one implementation of [`NavicubeView`]; the modern stack adds another.

use std::sync::Arc;

use ferrocad_graphics::{RenderQueue, Texture2, Topology, Viewport};
use ferrocad_inventor::{Node, NodeKind, Scene, compile_diagnostic};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use super::model::{
    FACE_FAN, FACE_OUTLINE, Piece, PieceKind, RING_PICK_ID, compass_angle, draw_order, face_glyph,
    ring_band, ring_disc, ring_marker, ring_rim_lines,
};
use super::theme::{NavicubeRole, NavicubeTheme};
use super::view::{NavicubeView, NavicubeVisual};

/// Build the cube scene, drawing every piece **back-to-front** (away faces and rear
/// pieces first) so the cube reads through, and the piece in `visual.hilite` in the
/// theme's highlight colour. Colours come from `theme` (§5 of `docs/developer/interaction/navicube-spec.md`).
///
/// The root holds the **rotating subtree** whose first node is the
/// [`NodeKind::Transform`] the controller writes the camera orientation into (see
/// [`set_orientation`]), then the ground-plane **compass ring**. Home, the projection
/// toggle and the angle readout are gpui chrome outside the 3D overlay.
pub fn scene_with(visual: &NavicubeVisual, theme: &NavicubeTheme) -> Scene {
    let orientation = visual.orientation;
    let (ring_back, ring_front) = ring_nodes(orientation, visual.proximity, visual.ring, theme);
    // The rotating subtree: the transform slot, the far half of the turntable, the
    // visible cube pieces, then the near half of the turntable. The overlay has depth off,
    // so this order is what makes the disc sit under the body and the near rim pass in
    // front of it without ever over-painting the cube.
    let mut cube = vec![field(NodeKind::Transform(Placement::identity())), ring_back];
    let rotation = orientation.to_matrix();
    let inverse = orientation.inverse().to_matrix();
    for piece in draw_order(orientation) {
        // A convex chamfered cube hides every piece whose outward normal points away;
        // culling them (instead of drawing them dimmed) removes the X-ray overlap and
        // lets the opaque front facets read as a solid object.
        if piece.is_hidden(&rotation) {
            continue;
        }
        let role = piece.role(visual.hilite, &visual.away);
        push_piece(
            &mut cube,
            piece,
            role,
            visual.proximity,
            theme,
            &rotation,
            &inverse,
        );
    }
    cube.push(ring_front);
    Scene {
        root: Node {
            kind: NodeKind::Separator,
            children: vec![separator(cube)],
        },
    }
}

/// The compass turntable in the ground plane under the cube: a dark filled floor disc,
/// a thick rim band with inner/outer loops, ticks and a heading marker at the camera's
/// bearing. The viewport's orientation transform makes it a projected ellipse.
///
/// The assembly is split at the view-space z of each quad/segment into a **far** half
/// (drawn before the cube) and a **near** half (drawn after it), so the near rim can pass
/// in front of the glass body while the far rim disappears behind it. The disc itself is
/// always part of the far node: it is the floor the cube stands on.
fn ring_nodes(
    orientation: Rotation,
    proximity: bool,
    active: bool,
    theme: &NavicubeTheme,
) -> (Node, Node) {
    let color = theme.ring_color(proximity, active);
    let mut far_color = color;
    far_color[3] *= 0.2;
    let band_color = theme.ring_fill_color(proximity, active);
    let mut far_band_color = band_color;
    far_band_color[3] *= 0.2;
    let matrix = orientation.to_matrix();
    let mut back = Vec::new();
    let mut front = Vec::new();

    // The floor disc always sits under the cube.
    let (disc, disc_indices) = ring_disc();
    push_draw(
        &mut back,
        RING_PICK_ID.0,
        theme.turntable,
        disc,
        &disc_indices,
        Topology::Triangles,
        None,
        None,
    );

    // The rim band: split each quad by the depth of its centre.
    let (band, _band_indices) = ring_band();
    let (far_band, far_band_indices, near_band, near_band_indices) = split_quads(&band, &matrix);
    for (points, indices, color, target) in [
        (far_band, far_band_indices, far_band_color, &mut back),
        (near_band, near_band_indices, band_color, &mut front),
    ] {
        if indices.is_empty() {
            continue;
        }
        push_draw(
            target,
            RING_PICK_ID.0,
            color,
            points,
            &indices,
            Topology::Triangles,
            None,
            None,
        );
    }

    // The two crisp rim loops and the ticks, split into far/near arcs by depth.
    let (points, indices) = ring_rim_lines();
    let (far_points, far_indices, near_points, near_indices) =
        split_pairs(&points, &indices, &matrix);
    for (points, indices, color, target) in [
        (far_points, far_indices, far_color, &mut back),
        (near_points, near_indices, color, &mut front),
    ] {
        if indices.is_empty() {
            continue;
        }
        push_draw(
            target,
            RING_PICK_ID.0,
            color,
            points,
            &indices,
            Topology::Lines,
            None,
            None,
        );
    }

    let (marker, marker_indices) = ring_marker(compass_angle(&orientation));
    let marker_z = marker
        .iter()
        .map(|point| {
            matrix
                .transform(&Vector3::new(
                    f64::from(point[0]),
                    f64::from(point[1]),
                    f64::from(point[2]),
                ))
                .z
        })
        .sum::<f64>()
        / marker.len() as f64;
    push_draw(
        if marker_z < 0.0 {
            &mut back
        } else {
            &mut front
        },
        RING_PICK_ID.0,
        if marker_z < 0.0 { far_color } else { color },
        marker,
        &marker_indices,
        Topology::Triangles,
        None,
        None,
    );
    (separator(back), separator(front))
}

/// Split a quad list (4 positions per quad) into far/near by the view-space depth of each
/// quad's centre. Every quad is emitted as two triangles.
#[allow(clippy::type_complexity)]
fn split_quads(
    points: &[[f32; 3]],
    matrix: &Matrix4,
) -> (Vec<[f32; 3]>, Vec<u32>, Vec<[f32; 3]>, Vec<u32>) {
    let mut far = (Vec::new(), Vec::new());
    let mut near = (Vec::new(), Vec::new());
    for quad in points.chunks_exact(4) {
        let centre = quad.iter().fold(Vector3::zero(), |sum, point| {
            sum.add(&Vector3::new(
                f64::from(point[0]),
                f64::from(point[1]),
                f64::from(point[2]),
            ))
        });
        let centre = centre.scale(0.25);
        let target = if matrix.transform(&centre).z < 0.0 {
            &mut far
        } else {
            &mut near
        };
        let base = target.0.len() as u32;
        target.0.extend_from_slice(quad);
        target
            .1
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (far.0, far.1, near.0, near.1)
}

/// Split a line list (2 positions per segment) into far/near by the midpoint depth.
#[allow(clippy::type_complexity)]
fn split_pairs(
    points: &[[f32; 3]],
    indices: &[u32],
    matrix: &Matrix4,
) -> (Vec<[f32; 3]>, Vec<u32>, Vec<[f32; 3]>, Vec<u32>) {
    let mut far = (Vec::new(), Vec::new());
    let mut near = (Vec::new(), Vec::new());
    for pair in indices.chunks_exact(2) {
        let a = points[pair[0] as usize];
        let b = points[pair[1] as usize];
        let midpoint = Vector3::new(
            (f64::from(a[0]) + f64::from(b[0])) / 2.0,
            (f64::from(a[1]) + f64::from(b[1])) / 2.0,
            (f64::from(a[2]) + f64::from(b[2])) / 2.0,
        );
        let target = if matrix.transform(&midpoint).z < 0.0 {
            &mut far
        } else {
            &mut near
        };
        let base = target.0.len() as u32;
        target.0.push(a);
        target.0.push(b);
        target.1.push(base);
        target.1.push(base + 1);
    }
    (far.0, far.1, near.0, near.1)
}

/// Write the cube's orientation into `scene`, which must be a scene from [`scene_with`]:
/// the fixup finds the transform slot of the rotating subtree.
pub fn set_orientation(scene: &mut Scene, rotation: Rotation) {
    let slot = scene
        .root
        .children
        .first_mut()
        .and_then(|cube| cube.children.first_mut());
    if let Some(transform) = slot {
        transform.kind = NodeKind::Transform(Placement::new(Vector3::zero(), rotation));
    }
}

/// Append a chamfered-cube piece. A face is a **square**; an edge is a **bevel quad** with
/// twin outline lines; a corner is an **equilateral triangle**. A hovered face adds its
/// square outline (the fill stays crisp), a hovered bevel fills cyan and gains a glow
/// ribbon, and a hovered corner fills cyan while its three adjacent bevels light softly as
/// neighbours. Each packet is isolated in its own [`NodeKind::Separator`] so fields do not
/// leak to the next draw.
#[allow(clippy::too_many_arguments)]
fn push_piece(
    children: &mut Vec<Node>,
    piece: Piece,
    role: NavicubeRole,
    proximity: bool,
    theme: &NavicubeTheme,
    rotation: &Matrix4,
    inverse: &Matrix4,
) {
    let id = piece.pick_id().0;
    let highlighted = matches!(
        role,
        NavicubeRole::FaceHighlighted
            | NavicubeRole::EdgeHighlighted
            | NavicubeRole::CornerHighlighted
    );
    // Hidden pieces are culled before this point, so every emitted piece is front-facing.
    let edge = theme.edge_color(role, proximity);
    match piece.kind() {
        PieceKind::Face => {
            let polygon = piece.polygon();
            push_draw(
                children,
                id,
                theme.face_fill(role, &rotation.transform(&piece.direction())),
                polygon.clone(),
                &FACE_FAN,
                Topology::Triangles,
                None,
                None,
            );
            // The wireframe is owned by the bevel and corner pieces (every octagon edge is
            // shared with a bevel or a corner patch), so a resting face draws no outline.
            // A hovered face brightens its octagon **without** flooding the fill.
            if highlighted {
                push_draw(
                    children,
                    id,
                    edge,
                    polygon,
                    &FACE_OUTLINE,
                    Topology::Lines,
                    None,
                    None,
                );
            }
            // The letter is a filled 3D glyph in the face plane (WA7), so it stays bold
            // and foreshortens with the facet.
            if role != NavicubeRole::FaceAway {
                let (points, indices) = face_glyph(piece);
                push_draw(
                    children,
                    id,
                    theme.label_color(role, proximity),
                    points,
                    &indices,
                    Topology::Triangles,
                    None,
                    None,
                );
            }
        }
        PieceKind::Edge => {
            let bevel = piece.bevel();
            push_draw(
                children,
                id,
                theme.bevel_color(role, false, proximity),
                bevel.clone(),
                &Piece::bevel_fan(),
                Topology::Triangles,
                None,
                None,
            );
            push_draw(
                children,
                id,
                edge,
                bevel,
                &Piece::bevel_outline(),
                Topology::Lines,
                None,
                None,
            );
            // The line pipeline is 1px with no width uniform, so a hovered bevel gets a
            // translucent screen-space ribbon behind it: the chamfer glows.
            if highlighted {
                let glow = piece.edge_glow(rotation, inverse);
                push_draw(
                    children,
                    id,
                    theme.glow,
                    glow,
                    &[0, 1, 2, 0, 2, 3],
                    Topology::Triangles,
                    None,
                    None,
                );
            }
        }
        PieceKind::Corner => {
            let triangle = piece.corner_triangle();
            push_draw(
                children,
                id,
                theme.color(role),
                triangle.clone(),
                &Piece::corner_fan(),
                Topology::Triangles,
                None,
                None,
            );
            push_draw(
                children,
                id,
                edge,
                triangle,
                &Piece::corner_outline(),
                Topology::Lines,
                None,
                None,
            );
        }
    }
}

/// Append one draw packet as an isolated separator.
#[allow(clippy::too_many_arguments)]
fn push_draw(
    children: &mut Vec<Node>,
    pick_id: u32,
    color: [f32; 4],
    coordinates: Vec<[f32; 3]>,
    indices: &[u32],
    topology: Topology,
    texture: Option<&Arc<Texture2>>,
    uvs: Option<&[[f32; 2]]>,
) {
    let mut nodes = vec![
        field(NodeKind::PickId(pick_id)),
        field(NodeKind::Material(vec![color])),
    ];
    if let Some(texture) = texture {
        nodes.push(field(NodeKind::Texture2(texture.clone())));
    }
    if let Some(uvs) = uvs {
        nodes.push(field(NodeKind::TextureCoordinate2(uvs.to_vec())));
    }
    nodes.push(field(NodeKind::Coordinate3(coordinates)));
    nodes.push(field(NodeKind::Mesh {
        indices: indices.to_vec(),
        normal_indices: vec![],
        topology,
    }));
    children.push(separator(nodes));
}

fn separator(children: Vec<Node>) -> Node {
    Node {
        kind: NodeKind::Separator,
        children,
    }
}

fn field(kind: NodeKind) -> Node {
    Node {
        kind,
        children: vec![],
    }
}

/// The navigation cube as a compiled inventor [`Scene`]: the legacy [`NavicubeView`]
/// implementation.
///
/// It holds the last compiled [`RenderQueue`] and the last program-assembly diagnostic;
/// [`rebuild`](NavicubeView::rebuild) walks a fresh [`scene_with`] and compiles it. A
/// stable visual state compiles to the same mesh ids, so the renderer's upload cache
/// hits.
#[derive(Default)]
pub struct InventorProducer {
    queue: RenderQueue,
    /// The last shader-program assembly error, if any (see [`Self::take_shader_error`]).
    /// The cube still draws: only the malformed program is dropped.
    shader_error: Option<String>,
}

impl InventorProducer {
    /// A producer holding an empty queue (call [`NavicubeView::rebuild`] before drawing).
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile `scene` into the cube's queue without panicking on a malformed program.
    ///
    /// This is deliberately [`compile_diagnostic`] rather than
    /// [`compile_with`](ferrocad_inventor::compile_with): the producer is not the
    /// viewport, so it must keep drawing the packets that did assemble and report the
    /// diagnostic instead of aborting the frame. The error is logged and kept for
    /// [`take_shader_error`](Self::take_shader_error); a later successful compile
    /// clears it.
    fn compile_scene(&mut self, scene: &Scene, viewport: &Viewport) -> Option<String> {
        let (queue, _, error) = compile_diagnostic(scene, viewport, None);
        self.queue = queue;
        let error = error.map(|error| {
            let message = error.to_string();
            eprintln!("ferrocad: navigation cube shader program: {message}");
            message
        });
        self.shader_error = error.clone();
        error
    }

    /// Take the last program-assembly diagnostic, if any. The cube still drew (the
    /// malformed program was dropped); a host holding the concrete producer reports it
    /// once and clears it, like [`InventorViewport::take_shader_error`].
    ///
    /// [`InventorViewport::take_shader_error`]: crate::InventorViewport::take_shader_error
    pub fn take_shader_error(&mut self) -> Option<String> {
        self.shader_error.take()
    }
}

impl NavicubeView for InventorProducer {
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual, theme: &NavicubeTheme) {
        let mut scene = scene_with(visual, theme);
        set_orientation(&mut scene, visual.orientation);
        self.compile_scene(&scene, viewport);
    }

    fn queue(&self) -> &RenderQueue {
        &self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::compile;

    use super::super::theme::{
        BASE_COLOR, EDGE_HILITE, GLOW_COLOR, HILITE_COLOR, LABEL_COLOR, NEIGHBOUR_COLOR, RING_COLOR,
        RING_HILITE, RING_PROXIMITY,
    };

    /// The cube scene with every piece opaque (no away-facing faces).
    fn scene(hilite: Option<Piece>) -> Scene {
        scene_with(
            &visual(hilite, &[], Rotation::identity(), false, false),
            &NavicubeTheme::default(),
        )
    }

    fn visual(
        hilite: Option<Piece>,
        away: &[Piece],
        orientation: Rotation,
        proximity: bool,
        ring: bool,
    ) -> NavicubeVisual {
        NavicubeVisual {
            orientation,
            hilite,
            away: away.to_vec(),
            proximity,
            ring,
        }
    }

    #[test]
    fn scene_compiles_to_every_piece_and_the_ring() {
        let (queue, _) = compile(&scene(None));
        // Each piece emits a fill/outline and (for faces) a stroke-label packet; ids are
        // the normative set 1..=26 plus the compass ring's 27.
        let mut ids: Vec<u32> = queue.packets.iter().map(|p| p.state.id.0).collect();
        ids.sort_unstable();
        ids.dedup();
        let rotation = Rotation::identity().to_matrix();
        let mut expected: Vec<u32> = Piece::ALL
            .iter()
            .filter(|piece| !piece.is_hidden(&rotation))
            .map(|piece| piece.pick_id().0)
            .collect();
        expected.push(RING_PICK_ID.0);
        expected.sort_unstable();
        assert_eq!(ids, expected, "visible pieces plus the turntable");
        // The twelve edge pieces are line packets with ids 7..=18. Face and corner
        // outlines add more lines, so count the edge ids specifically.
        let edge_lines = queue
            .packets
            .iter()
            .filter(|p| (7..=18).contains(&p.state.id.0) && p.topology == Topology::Lines)
            .count();
        assert_eq!(edge_lines, 12);
        // The highlight brightens the hovered face's outline without flooding its fill.
        let (queue, _) = compile(&scene(Some(Piece::TOP)));
        let top = queue
            .packets
            .iter()
            .find(|p| p.state.id == Piece::TOP.pick_id())
            .unwrap();
        assert_eq!(top.topology, Topology::Triangles, "the face fill leads");
        assert!(
            queue.packets.iter().any(|p| {
                p.state.id == Piece::TOP.pick_id()
                    && p.topology == Topology::Lines
                    && p.state.color == EDGE_HILITE
            }),
            "a hovered face gains a cyan outline"
        );
    }

    #[test]
    fn a_malformed_program_is_reported_without_panicking() {
        use ferrocad_inventor::{ShaderStage, ShaderStageKind};

        // Two vertex stages cannot assemble. The producer is a caller outside the
        // viewport: its compile path must report the diagnostic and keep the rest of
        // the scene drawable, unlike `compile_with`, which panics.
        let stage = || Node {
            kind: NodeKind::ShaderStage(ShaderStage {
                kind: ShaderStageKind::Vertex,
                source: Some("@vertex fn vs() -> @builtin(position) vec4f { vec4f() }".to_owned()),
                source_program: None,
            }),
            children: vec![],
        };
        let scene = Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    field(NodeKind::Coordinate3(vec![
                        [0.0, 0.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0],
                    ])),
                    Node {
                        kind: NodeKind::ShaderProgram,
                        children: vec![stage(), stage()],
                    },
                    field(NodeKind::Mesh {
                        indices: vec![0, 1, 2],
                        normal_indices: vec![],
                        topology: Topology::Triangles,
                    }),
                ],
            },
        };

        let mut producer = InventorProducer::new();
        // `compile_with` would panic here; the producer's path must not.
        let error = producer
            .compile_scene(&scene, &Viewport::new([64, 64], 1.0))
            .expect("the malformed program is reported");
        assert_eq!(
            producer.queue().len(),
            1,
            "the mesh after the malformed program still draws"
        );
        assert!(error.contains("SoVertexShader"), "names the stage: {error}");
        assert_eq!(
            producer.take_shader_error().as_deref(),
            Some(error.as_str()),
            "the diagnostic is kept for the host"
        );
        assert!(producer.take_shader_error().is_none(), "taking clears it");
    }

    #[test]
    fn pieces_are_emitted_back_to_front() {
        let orientation = Piece { axes: [1, -1, 1] }.orientation();
        let scene = scene_with(
            &visual(None, &[], orientation, false, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&scene);
        let rotation = orientation.to_matrix();
        let mut previous = f64::NEG_INFINITY;
        for packet in &queue.packets {
            // The ring now leads the rotating subtree.
            if packet.state.id == RING_PICK_ID {
                continue;
            }
            let Some(piece) = Piece::from_pick_id(packet.state.id) else {
                continue;
            };
            let depth = rotation.transform(&piece.center()).z;
            assert!(
                depth >= previous - 1e-9,
                "{piece:?} is drawn after a nearer piece ({depth} < {previous})"
            );
            previous = depth;
        }
    }

    #[test]
    fn faces_are_squares_without_textures() {
        let (queue, _) = compile(&scene(None));
        let rotation = Rotation::identity().to_matrix();
        for piece in Piece::ALL
            .iter()
            .filter(|p| p.kind() == PieceKind::Face && !p.is_hidden(&rotation))
        {
            let fill = queue
                .packets
                .iter()
                .find(|p| p.state.id == piece.pick_id())
                .expect("a face packet");
            assert_eq!(fill.topology, Topology::Triangles);
            // The compiler flattens the two fan triangles.
            assert_eq!(
                fill.geometry.as_ref().unwrap().vertices.len(),
                6,
                "square fan"
            );
            assert_eq!(
                fill.geometry.as_ref().unwrap().indices.len(),
                6,
                "two triangles"
            );
            assert!(
                fill.state.texture.is_none(),
                "labels are filled triangle quads now"
            );
        }
    }

    #[test]
    fn hidden_faces_are_culled() {
        let scene = scene_with(
            &visual(None, &[Piece::BOTTOM], Rotation::identity(), false, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&scene);
        let packets = |piece: Piece| {
            queue
                .packets
                .iter()
                .filter(|p| p.state.id == piece.pick_id())
                .collect::<Vec<_>>()
        };
        assert!(
            packets(Piece::BOTTOM).is_empty(),
            "a back-facing face is culled, not drawn dimmed"
        );
        let top = packets(Piece::TOP);
        assert_eq!(
            top.len(),
            2,
            "the glass fill and the filled glyph; the bevels own the outline"
        );
        assert_eq!(top[0].topology, Topology::Triangles);
        assert_eq!(
            top[0].state.color,
            NavicubeTheme::default().face_fill(NavicubeRole::Face, &Piece::TOP.direction())
        );
        assert_eq!(top[1].topology, Topology::Triangles, "the T glyph");
        assert_eq!(top[1].state.color, LABEL_COLOR);
    }

    #[test]
    fn a_hovered_edge_glows() {
        let edge = Piece { axes: [1, 0, 1] };
        let scene = scene_with(
            &visual(Some(edge), &[], Rotation::identity(), true, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&scene);
        let packets: Vec<_> = queue
            .packets
            .iter()
            .filter(|p| p.state.id == edge.pick_id())
            .collect();
        assert_eq!(
            packets[0].topology,
            Topology::Triangles,
            "the pickable bevel surface is the first packet"
        );
        assert_eq!(packets[0].state.color, HILITE_COLOR);
        let outline = packets
            .iter()
            .find(|p| p.topology == Topology::Lines)
            .expect("the bevel outline");
        assert_eq!(outline.geometry.as_ref().unwrap().vertices.len(), 8);
        let glow = packets
            .iter()
            .find(|p| p.state.color == GLOW_COLOR)
            .expect("a glow ribbon behind the bevel");
        assert_eq!(glow.topology, Topology::Triangles);
        // Two triangles are flattened to six vertices by the compiler.
        assert_eq!(glow.geometry.as_ref().unwrap().vertices.len(), 6);
    }

    #[test]
    fn corners_are_flush_triangles_with_a_hover_fill() {
        let corner = Piece { axes: [1, 1, 1] };
        let rest = scene_with(
            &visual(None, &[], Rotation::identity(), false, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&rest);
        let packets: Vec<_> = queue
            .packets
            .iter()
            .filter(|p| p.state.id == corner.pick_id())
            .collect();
        assert_eq!(
            packets.len(),
            2,
            "a corner triangle and its three outline edges"
        );
        assert_eq!(packets[0].topology, Topology::Triangles);
        assert_eq!(packets[0].state.color, BASE_COLOR);
        // One triangle is three vertices.
        assert_eq!(packets[0].geometry.as_ref().unwrap().vertices.len(), 3);
        assert_eq!(packets[1].topology, Topology::Lines);
        assert_eq!(packets[1].geometry.as_ref().unwrap().vertices.len(), 6);

        let hovered = scene_with(
            &visual(Some(corner), &[], Rotation::identity(), true, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&hovered);
        let packets: Vec<_> = queue
            .packets
            .iter()
            .filter(|p| p.state.id == corner.pick_id())
            .collect();
        assert!(
            packets
                .iter()
                .any(|p| p.topology == Topology::Triangles && p.state.color == HILITE_COLOR),
            "the hovered corner fills cyan"
        );
        // Its three incident bevels light softly as neighbours.
        let neighbours = queue
            .packets
            .iter()
            .filter(|p| p.topology == Topology::Triangles && p.state.color == NEIGHBOUR_COLOR)
            .count();
        assert_eq!(neighbours, 3, "the corner's three adjacent bevels glow");
    }

    #[test]
    fn the_compass_ring_is_a_ground_plane_line_and_marker() {
        // A tilted view so the XY turntable is not edge-on; both depth halves exist.
        let orientation = crate::navicube::view_rotation(0.6, 0.4);
        let matrix = orientation.to_matrix();
        let (band, _) = super::super::model::ring_band();
        let (_, far_i, _, near_i) = split_quads(&band, &matrix);
        assert!(
            !far_i.is_empty() && !near_i.is_empty(),
            "band quads: far {} / near {}",
            far_i.len(),
            near_i.len()
        );
        let scene = scene_with(
            &visual(None, &[], orientation, false, false),
            &NavicubeTheme::default(),
        );
        let (queue, _) = compile(&scene);
        let packets: Vec<_> = queue
            .packets
            .iter()
            .filter(|p| p.state.id == RING_PICK_ID)
            .collect();
        assert_eq!(
            packets.len(),
            6,
            "disc, far/near band, far/near rim, marker: {:?}",
            packets
                .iter()
                .map(|p| (p.topology, p.state.color))
                .collect::<Vec<_>>()
        );
        let lines: Vec<_> = packets
            .iter()
            .filter(|p| p.topology == Topology::Lines)
            .collect();
        assert_eq!(lines.len(), 2, "the far arc is a separate, fainter packet");
        assert!(
            lines[0].state.color[3] < lines[1].state.color[3],
            "the far arc fades so it reads as passing behind the cube"
        );
        assert!(
            lines
                .iter()
                .flat_map(|packet| packet.geometry.as_ref().unwrap().vertices.iter())
                .all(
                    |vertex| (f64::from(vertex.position[2]) - super::super::model::RING_Z).abs()
                        < 1e-6
                )
        );
        assert!(
            packets.iter().any(|p| p.topology == Topology::Triangles),
            "the filled disc, band and heading marker"
        );
        assert!(packets.iter().all(|p| p.state.texture.is_none()));

        let ring_line_color = |proximity, ring| {
            let scene = scene_with(
                &visual(None, &[], orientation, proximity, ring),
                &NavicubeTheme::default(),
            );
            let (queue, _) = compile(&scene);
            queue
                .packets
                .iter()
                .filter(|p| p.state.id == RING_PICK_ID && p.topology == Topology::Lines)
                .next_back()
                .expect("the near ring line packet")
                .state
                .color
        };
        assert_eq!(ring_line_color(false, false), RING_COLOR);
        assert_eq!(ring_line_color(true, false), RING_PROXIMITY);
        assert_eq!(ring_line_color(false, true), RING_HILITE);
    }
}

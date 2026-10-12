//! The inventor producer: builds the navigation cube as a `pivy.coin`-style [`Scene`].
//!
//! The counterpart of FreeCAD's `SoNaviCube`: the cube subtree (a `Transform` the
//! controller writes the camera orientation into, then the pieces), plus a fixed Home
//! button. It is one implementation of [`NavicubeView`]; the modern stack adds another.

use ferrocad_graphics::{RenderQueue, Topology, Viewport};
use ferrocad_inventor::{Node, NodeKind, Scene, compile_with};
use ferrocad_types::{Placement, Rotation, Vector3};

use super::model::{
    FACE_UV, HOME_PICK_ID, NaviTarget, Piece, PieceKind, face_letter, home_quad, label,
};
use super::theme::NavicubeTheme;
use super::view::{NavicubeView, NavicubeVisual};

/// Build the cube scene, drawing the faces in `away` (those pointing away from the
/// viewer) first so the cube reads through, and the piece in `hilite` (or the Home
/// button) in the theme's highlight colour. Colours come from `theme` (§5 of
/// `docs/navicube-spec.md`).
///
/// The root has two children: a **rotating subtree** whose first node is the
/// [`NodeKind::Transform`] the controller writes the camera orientation into (see
/// [`set_orientation`]), and the fixed **Home button** (drawn last, so it sits on top).
/// The Home button is a sibling of the rotating subtree, so it stays put as the cube
/// turns.
pub fn scene_with(hilite: Option<NaviTarget>, away: &[Piece], theme: &NavicubeTheme) -> Scene {
    let color_of = |piece: Piece| theme.color(NaviTarget::Piece(piece).role(hilite, away));
    // The rotating subtree: the transform slot, then away-facing faces (translucent,
    // behind), then everything else.
    let mut cube = vec![field(NodeKind::Transform(Placement::identity()))];
    for &piece in away {
        push_piece(&mut cube, piece, color_of(piece));
    }
    for piece in Piece::ALL {
        if away.contains(&piece) {
            continue;
        }
        push_piece(&mut cube, piece, color_of(piece));
    }
    Scene {
        root: Node {
            kind: NodeKind::Separator,
            children: vec![separator(cube), home_button(hilite, theme)],
        },
    }
}

/// Write the cube's orientation into `scene`, which must be a scene from [`scene`] or
/// [`scene_with`]: the fixup finds the transform slot of the rotating subtree.
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

/// The fixed Home button: a small labeled quad below the cube.
fn home_button(hilite: Option<NaviTarget>, theme: &NavicubeTheme) -> Node {
    let color = theme.color(NaviTarget::Home.role(hilite, &[]));
    separator(vec![
        field(NodeKind::PickId(HOME_PICK_ID.0)),
        field(NodeKind::Material(vec![color])),
        field(NodeKind::Texture2(label('H').clone())),
        field(NodeKind::TextureCoordinate2(FACE_UV.to_vec())),
        field(NodeKind::Coordinate3(home_quad())),
        field(NodeKind::Mesh {
            indices: vec![0, 1, 2, 0, 2, 3],
            normal_indices: vec![],
            topology: Topology::Triangles,
        }),
    ])
}

/// Append a piece, isolated in its own [`NodeKind::Separator`] so its fields do not
/// leak to the next piece. Faces carry a baked label texture; edges and corners are
/// untextured (they sample the white default).
fn push_piece(children: &mut Vec<Node>, piece: Piece, color: [f32; 4]) {
    let (coordinates, indices, topology) = match piece.kind() {
        PieceKind::Face => (piece.quad(), vec![0, 1, 2, 0, 2, 3], Topology::Triangles),
        PieceKind::Edge => (piece.segment(), vec![0, 1], Topology::Lines),
        PieceKind::Corner => {
            let (vertices, indices) = piece.corner_cube();
            (vertices, indices, Topology::Triangles)
        }
    };
    let mut nodes = vec![
        field(NodeKind::PickId(piece.pick_id().0)),
        field(NodeKind::Material(vec![color])),
    ];
    if piece.kind() == PieceKind::Face {
        nodes.push(field(NodeKind::Texture2(label(face_letter(piece)).clone())));
        nodes.push(field(NodeKind::TextureCoordinate2(FACE_UV.to_vec())));
    }
    nodes.push(field(NodeKind::Coordinate3(coordinates)));
    nodes.push(field(NodeKind::Mesh {
        indices,
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
/// It holds the last compiled [`RenderQueue`]; [`rebuild`](NavicubeView::rebuild) walks
/// a fresh [`scene_with`] and compiles it. A stable visual state compiles to the same
/// mesh ids, so the renderer's upload cache hits.
#[derive(Default)]
pub struct InventorProducer {
    queue: RenderQueue,
}

impl InventorProducer {
    /// A producer holding an empty queue (call [`NavicubeView::rebuild`] before drawing).
    pub fn new() -> Self {
        Self::default()
    }
}

impl NavicubeView for InventorProducer {
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual, theme: &NavicubeTheme) {
        let mut scene = scene_with(visual.hilite, &visual.away, theme);
        set_orientation(&mut scene, visual.orientation);
        self.queue = compile_with(&scene, viewport).0;
    }

    fn queue(&self) -> &RenderQueue {
        &self.queue
    }

    fn has_labels(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::compile;

    use super::super::theme::{HILITE_COLOR, HOME_COLOR};

    /// The cube scene with every piece opaque (no away-facing faces).
    fn scene(hilite: Option<NaviTarget>) -> Scene {
        scene_with(hilite, &[], &NavicubeTheme::default())
    }

    #[test]
    fn scene_compiles_to_all_pieces_and_the_home_button() {
        let (queue, _) = compile(&scene(None));
        assert_eq!(queue.len(), 27, "26 pieces plus the Home button");
        let ids: Vec<u32> = queue.packets.iter().map(|p| p.state.id.0).collect();
        assert_eq!(ids, (1..=27).collect::<Vec<u32>>());
        // Faces and corners are triangles; edges are lines (the Home button is a
        // triangle, so it does not add to the line count).
        let lines = queue
            .packets
            .iter()
            .filter(|p| p.topology == Topology::Lines)
            .count();
        assert_eq!(lines, 12);
        // The highlight recolours exactly one piece.
        let (queue, _) = compile(&scene(Some(NaviTarget::Piece(Piece::TOP))));
        let top = queue
            .packets
            .iter()
            .find(|p| p.state.id == Piece::TOP.pick_id())
            .unwrap();
        assert_eq!(top.state.color, HILITE_COLOR);
    }

    #[test]
    fn faces_carry_a_label_texture() {
        let (queue, _) = compile(&scene(None));
        for piece in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Face) {
            let packet = queue
                .packets
                .iter()
                .find(|p| p.state.id == piece.pick_id())
                .expect("a packet per face");
            assert!(packet.state.texture.is_some(), "{piece:?} should be labeled");
        }
        // Edges and corners are untextured (they sample the white default).
        let edge = queue
            .packets
            .iter()
            .find(|p| p.state.id == Piece::ALL[6].pick_id())
            .unwrap();
        assert!(edge.state.texture.is_none());
    }

    #[test]
    fn the_home_button_is_a_fixed_labeled_quad() {
        let (queue, _) = compile(&scene(None));
        let home = queue
            .packets
            .iter()
            .find(|p| p.state.id == HOME_PICK_ID)
            .expect("the Home button packet");
        assert_eq!(home.topology, Topology::Triangles);
        assert_eq!(home.state.color, HOME_COLOR);
        assert!(home.state.texture.is_some(), "the Home button is labeled");
        // Its transform is the identity: it is a sibling of the (rotating) cube
        // subtree, so the camera orientation never reaches it.
        let mut oriented = scene(None);
        set_orientation(&mut oriented, Piece::TOP.orientation());
        let home = compile(&oriented)
            .0
            .packets
            .into_iter()
            .find(|p| p.state.id == HOME_PICK_ID)
            .unwrap();
        assert!(home.state.world_transform.is_unity(1e-9), "Home does not rotate");
    }
}

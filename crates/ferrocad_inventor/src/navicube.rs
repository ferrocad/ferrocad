//! The navigation cube: a small orientation gizmo for the viewer corner.
//!
//! The counterpart of FreeCAD's `Gui::NaviCube` + `SoNaviCube`: a cube whose faces,
//! edges and corners point along the world axes, drawn as a screen-space overlay and
//! rotated to mirror the main camera's orientation. Clicking a piece reorients the
//! view onto it (a face looks straight at it; an edge or corner gives a diagonal
//! view).
//!
//! This module is the **pure** part — the cube scene and its orthographic overlay
//! camera. The controller (placement, camera sync, hit-testing, snapping) lives in
//! `ferrocad_gpui::navicube`, and the overlay render pass in `ferrocad_wgpu`.
//!
//! Our axes follow the engine's default view: **+Z toward the viewer**, **+Y up**,
//! **+X right**. (FreeCAD is Z-up; the piece *names* differ, the behaviour does not.)

use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::batch::PickId;
use crate::camera::Camera;
use crate::geometry::Topology;
use crate::node::{Node, NodeKind, Scene};
use crate::pixels::{Rect, Viewport};

/// Half the extent of the cube (each face sits at ±`CUBE_HALF`).
const CUBE_HALF: f64 = 0.5;
/// The half-extent of each face quad, inset from the cube edge so the gaps read as
/// edges.
const FACE_INSET: f64 = 0.42;
/// How far each edge segment stops short of a corner (leaving room for the corner).
const EDGE_END_INSET: f64 = 0.13;
/// Half the size of the small cube drawn at each corner.
const CORNER_HALF: f64 = 0.12;
/// The orthographic camera's half-extent: how much of the cube space fills the rect.
const ORTHO_EXTENT: f64 = 1.1;
/// How far the fixed overlay camera sits along +Z.
const CAMERA_DISTANCE: f64 = 3.0;

/// The cube's resting piece colour.
pub const BASE_COLOR: [f32; 4] = [0.55, 0.6, 0.66, 1.0];
/// The colour a piece takes while hovered.
pub const HILITE_COLOR: [f32; 4] = [0.2, 0.6, 0.95, 1.0];
/// The translucent colour of a face pointing away from the viewer (drawn first, so the
/// cube reads as a solid but see-through box).
pub const INACTIVE_COLOR: [f32; 4] = [0.38, 0.42, 0.48, 0.5];

/// What kind of region a [`Piece`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    /// A cube face (one non-zero axis).
    Face,
    /// A cube edge (two non-zero axes).
    Edge,
    /// A cube corner (three non-zero axes).
    Corner,
}

/// A clickable region of the cube, identified by its signed axis direction: one
/// non-zero component for a face, two for an edge, three for a corner.
///
/// The `PickId` is the 1-based position in [`Piece::ALL`] (faces 1–6, edges 7–18,
/// corners 19–26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    /// The signed axis components (−1, 0 or 1).
    pub axes: [i8; 3],
}

impl Piece {
    /// Toward the viewer (+Z).
    pub const FRONT: Piece = Piece { axes: [0, 0, 1] };
    /// Away from the viewer (−Z).
    pub const BACK: Piece = Piece { axes: [0, 0, -1] };
    /// Up (+Y).
    pub const TOP: Piece = Piece { axes: [0, 1, 0] };
    /// Down (−Y).
    pub const BOTTOM: Piece = Piece { axes: [0, -1, 0] };
    /// Right (+X).
    pub const RIGHT: Piece = Piece { axes: [1, 0, 0] };
    /// Left (−X).
    pub const LEFT: Piece = Piece { axes: [-1, 0, 0] };

    /// Every piece, in pick-id order: the 6 faces, 12 edges, then 8 corners.
    pub const ALL: [Piece; 26] = [
        Self::FRONT,
        Self::BACK,
        Self::TOP,
        Self::BOTTOM,
        Self::RIGHT,
        Self::LEFT,
        // Edges (two non-zero axes).
        Piece { axes: [1, 1, 0] },
        Piece { axes: [-1, 1, 0] },
        Piece { axes: [1, -1, 0] },
        Piece { axes: [-1, -1, 0] },
        Piece { axes: [0, 1, 1] },
        Piece { axes: [0, -1, 1] },
        Piece { axes: [0, 1, -1] },
        Piece { axes: [0, -1, -1] },
        Piece { axes: [1, 0, 1] },
        Piece { axes: [-1, 0, 1] },
        Piece { axes: [1, 0, -1] },
        Piece { axes: [-1, 0, -1] },
        // Corners (three non-zero axes).
        Piece { axes: [1, 1, 1] },
        Piece { axes: [-1, 1, 1] },
        Piece { axes: [1, -1, 1] },
        Piece { axes: [-1, -1, 1] },
        Piece { axes: [1, 1, -1] },
        Piece { axes: [-1, 1, -1] },
        Piece { axes: [1, -1, -1] },
        Piece { axes: [-1, -1, -1] },
    ];

    /// Whether this is a face, an edge or a corner.
    pub fn kind(self) -> PieceKind {
        match self.axes.iter().filter(|axis| **axis != 0).count() {
            1 => PieceKind::Face,
            2 => PieceKind::Edge,
            _ => PieceKind::Corner,
        }
    }

    /// The unit direction this piece points along (the axis, edge or corner diagonal).
    pub fn direction(self) -> Vector3 {
        Vector3::new(
            f64::from(self.axes[0]),
            f64::from(self.axes[1]),
            f64::from(self.axes[2]),
        )
        .normalize()
    }

    /// The pick id this piece carries (1-based).
    pub fn pick_id(self) -> PickId {
        let index = Piece::ALL
            .iter()
            .position(|piece| *piece == self)
            .expect("piece is one of ALL");
        PickId(index as u32 + 1)
    }

    /// The piece carrying `id`, if any.
    pub fn from_pick_id(id: PickId) -> Option<Piece> {
        let index = (id.0 as usize).checked_sub(1)?;
        Piece::ALL.get(index).copied()
    }

    /// The camera **view rotation** that turns this piece toward the viewer (maps its
    /// [`direction`](Self::direction) onto +Z).
    pub fn orientation(self) -> Rotation {
        Rotation::from_vectors(self.direction(), Vector3::new(0.0, 0.0, 1.0))
    }

    /// The index of the single non-zero axis (for faces).
    fn axis(self) -> usize {
        (0..3)
            .find(|&i| self.axes[i] != 0)
            .expect("a piece has at least one axis")
    }

    /// The four corners of a face's (inset) quad, wound so the flat normal points
    /// outward. Only valid for [`PieceKind::Face`].
    fn quad(self) -> Vec<[f32; 3]> {
        let x = Vector3::new(1.0, 0.0, 0.0);
        let y = Vector3::new(0.0, 1.0, 0.0);
        let z = Vector3::new(0.0, 0.0, 1.0);
        let sign = f64::from(self.axes[self.axis()]);
        // In-plane axes (u, v) with u × v = the face normal.
        let (u, v) = match self.axis() {
            0 => (y, z.scale(sign)),
            1 => (z, x.scale(sign)),
            _ => (x, y.scale(sign)),
        };
        let center = self.direction().scale(CUBE_HALF);
        let corner = |su: f64, sv: f64| {
            let p = center
                .add(&u.scale(su * FACE_INSET))
                .add(&v.scale(sv * FACE_INSET));
            [p.x as f32, p.y as f32, p.z as f32]
        };
        vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ]
    }

    /// The two endpoints of an edge segment. Only valid for [`PieceKind::Edge`].
    fn segment(self) -> Vec<[f32; 3]> {
        let along = (0..3)
            .find(|&i| self.axes[i] == 0)
            .expect("an edge has a free axis");
        let end = CUBE_HALF - EDGE_END_INSET;
        let mut a = [0.0_f32; 3];
        let mut b = [0.0_f32; 3];
        for i in 0..3 {
            if i != along {
                let value = f64::from(self.axes[i]) * CUBE_HALF;
                a[i] = value as f32;
                b[i] = value as f32;
            }
        }
        a[along] = -end as f32;
        b[along] = end as f32;
        vec![a, b]
    }

    /// The 8 vertices and 12 triangle indices of a small cube at a corner. Only valid
    /// for [`PieceKind::Corner`].
    fn corner_cube(self) -> (Vec<[f32; 3]>, Vec<u32>) {
        let c = self.direction().scale(CUBE_HALF * 3.0_f64.sqrt());
        let h = CORNER_HALF;
        let vertex = |sx: f64, sy: f64, sz: f64| {
            [
                (c.x + sx * h) as f32,
                (c.y + sy * h) as f32,
                (c.z + sz * h) as f32,
            ]
        };
        let corners = vec![
            vertex(-1.0, -1.0, -1.0),
            vertex(1.0, -1.0, -1.0),
            vertex(1.0, 1.0, -1.0),
            vertex(-1.0, 1.0, -1.0),
            vertex(-1.0, -1.0, 1.0),
            vertex(1.0, -1.0, 1.0),
            vertex(1.0, 1.0, 1.0),
            vertex(-1.0, 1.0, 1.0),
        ];
        let indices = vec![
            4, 5, 6, 4, 6, 7, // +z
            1, 0, 3, 1, 3, 2, // -z
            0, 4, 7, 0, 7, 3, // -x
            5, 1, 2, 5, 2, 6, // +x
            7, 6, 2, 7, 2, 3, // +y
            0, 1, 5, 0, 5, 4, // -y
        ];
        (corners, indices)
    }
}

/// Build the cube scene with every piece opaque.
///
/// The first child is always the [`NodeKind::Transform`] the controller writes the
/// camera orientation into.
pub fn scene(hilite: Option<Piece>) -> Scene {
    scene_with(hilite, &[])
}

/// Build the cube scene, drawing the faces in `away` (those pointing away from the
/// viewer) first and translucent ([`INACTIVE_COLOR`]) so the cube reads through. The
/// piece in `hilite` is drawn in [`HILITE_COLOR`].
pub fn scene_with(hilite: Option<Piece>, away: &[Piece]) -> Scene {
    let mut children = vec![field(NodeKind::Transform(Placement::identity()))];
    let color_of = |piece: Piece| {
        if Some(piece) == hilite {
            HILITE_COLOR
        } else if away.contains(&piece) {
            INACTIVE_COLOR
        } else {
            BASE_COLOR
        }
    };
    // Away-facing faces first (translucent, behind), then everything else.
    for &piece in away {
        push_piece(&mut children, piece, color_of(piece));
    }
    for piece in Piece::ALL {
        if away.contains(&piece) {
            continue;
        }
        push_piece(&mut children, piece, color_of(piece));
    }
    Scene {
        root: Node {
            kind: NodeKind::Separator,
            children,
        },
    }
}

/// Append a piece's field nodes (`PickId`, `Material`, `Coordinate3`, `Mesh`).
fn push_piece(children: &mut Vec<Node>, piece: Piece, color: [f32; 4]) {
    let (coordinates, indices, topology) = match piece.kind() {
        PieceKind::Face => (piece.quad(), vec![0, 1, 2, 0, 2, 3], Topology::Triangles),
        PieceKind::Edge => (piece.segment(), vec![0, 1], Topology::Lines),
        PieceKind::Corner => {
            let (vertices, indices) = piece.corner_cube();
            (vertices, indices, Topology::Triangles)
        }
    };
    children.push(field(NodeKind::PickId(piece.pick_id().0)));
    children.push(field(NodeKind::Material(vec![color])));
    children.push(field(NodeKind::Coordinate3(coordinates)));
    children.push(field(NodeKind::Mesh {
        indices,
        normal_indices: vec![],
        topology,
    }));
}

fn field(kind: NodeKind) -> Node {
    Node {
        kind,
        children: vec![],
    }
}

/// The fixed orthographic overlay camera for `rect`, looking down −Z at the cube at
/// the origin.
pub fn camera(rect: Rect) -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -CAMERA_DISTANCE), Rotation::identity())
            .to_matrix(),
        projection: ortho(ORTHO_EXTENT),
        viewport: Viewport::new([rect.width.max(1), rect.height.max(1)], 1.0),
    }
}

/// A symmetric orthographic projection mapping `[-extent, extent]` to the clip volume
/// (wgpu's `z ∈ [0, 1]`).
fn ortho(extent: f64) -> Matrix4 {
    let (near, far) = (0.1_f64, 100.0_f64);
    let mut m = [0.0_f64; 16];
    m[0] = 1.0 / extent;
    m[5] = 1.0 / extent;
    m[10] = 1.0 / (near - far);
    m[11] = near / (near - far);
    m[15] = 1.0;
    Matrix4::from_values(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;

    #[test]
    fn pick_ids_round_trip() {
        for (index, piece) in Piece::ALL.iter().enumerate() {
            assert_eq!(piece.pick_id(), PickId(index as u32 + 1));
            assert_eq!(Piece::from_pick_id(piece.pick_id()), Some(*piece));
        }
        assert_eq!(Piece::from_pick_id(PickId(0)), None);
        assert_eq!(Piece::from_pick_id(PickId(27)), None);
        assert_eq!(Piece::FRONT.pick_id(), PickId(1));
    }

    #[test]
    fn kinds_are_counted() {
        let counts = |kind| Piece::ALL.iter().filter(|p| p.kind() == kind).count();
        assert_eq!(counts(PieceKind::Face), 6);
        assert_eq!(counts(PieceKind::Edge), 12);
        assert_eq!(counts(PieceKind::Corner), 8);
    }

    #[test]
    fn orientation_turns_the_direction_to_the_viewer() {
        for piece in Piece::ALL {
            let rotated = piece
                .orientation()
                .to_matrix()
                .transform(&piece.direction());
            assert!(
                rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9,
                "{piece:?} maps to {rotated:?}"
            );
        }
    }

    #[test]
    fn scene_compiles_to_all_pieces() {
        let queue = compile(&scene(None));
        assert_eq!(queue.len(), 26, "one packet per piece");
        let ids: Vec<u32> = queue.packets.iter().map(|p| p.state.id.0).collect();
        assert_eq!(ids, (1..=26).collect::<Vec<u32>>());
        // Faces and corners are triangles; edges are lines.
        let lines = queue
            .packets
            .iter()
            .filter(|p| p.topology == Topology::Lines)
            .count();
        assert_eq!(lines, 12);
        // The highlight recolours exactly one piece.
        let queue = compile(&scene(Some(Piece::TOP)));
        let top = queue
            .packets
            .iter()
            .find(|p| p.state.id == Piece::TOP.pick_id())
            .unwrap();
        assert_eq!(top.state.color, HILITE_COLOR);
    }
}

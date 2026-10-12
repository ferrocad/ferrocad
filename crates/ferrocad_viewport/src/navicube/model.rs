//! The navigation-cube model: the piece taxonomy, ids, geometry, labels and overlay
//! camera.
//!
//! Producer-neutral and pure — no scene graph, no GPU, no Python. A **producer**
//! ([`super::inventor`], and later `super::stage`) turns a [`Piece`] into draw packets;
//! the controller ([`super`]) places the cube, mirrors the camera and picks it.
//!
//! Our axes follow the engine's default view: **+Z toward the viewer**, **+Y up**,
//! **+X right**. (FreeCAD is Z-up; the piece *names* differ, the behaviour does not.)

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use ferrocad_graphics::{Camera, PickId, Projection, Rect, Texture2, Viewport};
use ferrocad_types::{Placement, Rotation, Vector3};

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
/// The Home button's face colour.
pub const HOME_COLOR: [f32; 4] = [0.5, 0.55, 0.62, 1.0];

/// The pick id of the Home button (the pieces occupy 1–26).
pub const HOME_PICK_ID: PickId = PickId(27);

/// The Home button's centre, in cube space (below the cube, facing the viewer).
pub(crate) const HOME_CENTER: [f64; 2] = [0.0, -0.82];
/// Half the Home button's width and height, in cube space.
pub(crate) const HOME_HALF: [f64; 2] = [0.24, 0.18];

/// The side of a baked label texture, in pixels (64×4 = 256 bytes/row, which the
/// texture upload wants aligned).
pub const LABEL_SIZE: u32 = 64;

/// The texture coordinates of a unit face quad, in the winding of [`Piece::quad`]
/// (bottom-left, bottom-right, top-right, top-left).
pub(crate) const FACE_UV: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

/// A letter for a face: Front/Back/Top/Down/Left/Right.
pub(crate) fn face_letter(piece: Piece) -> char {
    match piece {
        Piece::FRONT => 'F',
        Piece::BACK => 'B',
        Piece::TOP => 'T',
        Piece::BOTTOM => 'D',
        Piece::RIGHT => 'R',
        Piece::LEFT => 'L',
        _ => '?',
    }
}

/// A 5×7 block-letter bitmap (`#` = ink), scaled into a label texture.
fn glyph(letter: char) -> [&'static str; 7] {
    match letter {
        'F' => ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
        'B' => ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."],
        'T' => ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
        'D' => ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."],
        'L' => ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
        'R' => ["####.", "#...#", "#...#", "####.", "#..#.", "#...#", "#...#"],
        'H' => ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
        _ => ["#####", "#...#", "#...#", "#...#", "#...#", "#...#", "#####"],
    }
}

/// A baked label texture: a light background with a dark letter (so multiplying it
/// into the face colour paints the letter). Cached per letter, so the renderer uploads
/// each once.
pub(crate) fn label(letter: char) -> &'static Arc<Texture2> {
    static CACHE: OnceLock<HashMap<char, Arc<Texture2>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        ['F', 'B', 'T', 'D', 'L', 'R', 'H']
            .into_iter()
            .map(|letter| (letter, Arc::new(bake_label(letter))))
            .collect()
    });
    cache
        .get(&letter)
        .unwrap_or_else(|| cache.get(&'H').expect("the label cache is populated"))
}

/// Rasterize `letter` into a [`LABEL_SIZE`]-square RGBA texture.
fn bake_label(letter: char) -> Texture2 {
    const BG: [u8; 4] = [255, 255, 255, 255];
    const FG: [u8; 4] = [40, 40, 40, 255];
    let size = LABEL_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&BG);
    }
    let rows = glyph(letter);
    let (width, height) = (rows[0].len(), rows.len());
    let cell = 8usize;
    let ox = (size - width * cell) / 2;
    let oy = (size - height * cell) / 2;
    for (r, row) in rows.iter().enumerate() {
        for (c, mark) in row.chars().enumerate() {
            if mark != '#' {
                continue;
            }
            for dy in 0..cell {
                for dx in 0..cell {
                    let x = ox + c * cell + dx;
                    let y = oy + r * cell + dy;
                    let i = (y * size + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&FG);
                }
            }
        }
    }
    Texture2::new(LABEL_SIZE, LABEL_SIZE, rgba)
}

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

    /// The four corners of a face's (inset) quad, wound so the flat normal points
    /// outward. Only valid for [`PieceKind::Face`].
    ///
    /// The in-plane axes come from [`orientation`](Self::orientation) — the *same*
    /// rotation the camera takes when it snaps onto this face — so the baked label
    /// reads upright exactly when the face is turned to the viewer: `u` (the letter's
    /// right) is the world direction that maps to view +X and `v` (its up) to view +Y.
    pub(crate) fn quad(self) -> Vec<[f32; 3]> {
        let inverse = self.orientation().inverse().to_matrix();
        let u = inverse.transform(&Vector3::new(1.0, 0.0, 0.0));
        let v = inverse.transform(&Vector3::new(0.0, 1.0, 0.0));
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
    pub(crate) fn segment(self) -> Vec<[f32; 3]> {
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
    pub(crate) fn corner_cube(self) -> (Vec<[f32; 3]>, Vec<u32>) {
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

/// A clickable navigation target: one of the cube's pieces, or the Home button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NaviTarget {
    /// A cube face, edge or corner.
    Piece(Piece),
    /// The Home button (restore the default view).
    Home,
}

impl NaviTarget {
    /// The pick id this target carries.
    pub fn pick_id(self) -> PickId {
        match self {
            NaviTarget::Piece(piece) => piece.pick_id(),
            NaviTarget::Home => HOME_PICK_ID,
        }
    }

    /// The target carrying `id`, if any (a piece's id, or the Home id).
    pub fn from_pick_id(id: PickId) -> Option<NaviTarget> {
        if id == HOME_PICK_ID {
            Some(NaviTarget::Home)
        } else {
            Piece::from_pick_id(id).map(NaviTarget::Piece)
        }
    }
}

/// The four corners of the Home button's quad, wound counter-clockwise toward the
/// viewer.
pub(crate) fn home_quad() -> Vec<[f32; 3]> {
    let (cx, cy) = (HOME_CENTER[0] as f32, HOME_CENTER[1] as f32);
    let (hw, hh) = (HOME_HALF[0] as f32, HOME_HALF[1] as f32);
    vec![
        [cx - hw, cy - hh, 0.0],
        [cx + hw, cy - hh, 0.0],
        [cx + hw, cy + hh, 0.0],
        [cx - hw, cy + hh, 0.0],
    ]
}

/// Which corner of the viewport the cube sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// Top-left.
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

/// The default "home" orientation the Home button snaps to: an axonometric view with the
/// front-top-right corner toward the viewer. Upstream's Home restores the configurable
/// `NewDocumentCameraOrientation` (default `Top`); a fixed default keeps this build
/// self-contained.
pub fn home_orientation() -> Rotation {
    Piece { axes: [1, 1, 1] }.orientation()
}

/// The fixed orthographic overlay camera for `rect`, looking down −Z at the cube at
/// the origin.
pub fn camera(rect: Rect) -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -CAMERA_DISTANCE), Rotation::identity())
            .to_matrix(),
        projection: Projection::Orthographic {
            extent: ORTHO_EXTENT,
            near: 0.1,
            far: 100.0,
        },
        viewport: Viewport::new([rect.width.max(1), rect.height.max(1)], 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn face_labels_are_upright_in_the_snapped_view() {
        let v = |p: [f32; 3]| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        for piece in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Face) {
            let rotation = piece.orientation().to_matrix();
            let quad = piece.quad();
            // Vertex order is (su, sv) = (-1,-1), (1,-1), (1,1), (-1,1): +u runs v0 -> v1
            // (the letter's right) and +v runs v0 -> v3 (the letter's up).
            let right = v(quad[1]).sub(&v(quad[0])).normalize();
            let up = v(quad[3]).sub(&v(quad[0])).normalize();
            // In the view that snaps onto this face, its right lands on +X and its up on
            // +Y — i.e. the label reads upright.
            let right_view = rotation.transform(&right);
            let up_view = rotation.transform(&up);
            assert!(
                right_view.sub(&Vector3::new(1.0, 0.0, 0.0)).length() < 1e-9,
                "{piece:?} right -> {right_view:?}"
            );
            assert!(
                up_view.sub(&Vector3::new(0.0, 1.0, 0.0)).length() < 1e-9,
                "{piece:?} up -> {up_view:?}"
            );
        }
    }

    #[test]
    fn navi_targets_round_trip_pieces_and_home() {
        for piece in Piece::ALL {
            assert_eq!(
                NaviTarget::from_pick_id(piece.pick_id()),
                Some(NaviTarget::Piece(piece))
            );
        }
        assert_eq!(NaviTarget::from_pick_id(HOME_PICK_ID), Some(NaviTarget::Home));
        assert_eq!(NaviTarget::Home.pick_id(), HOME_PICK_ID);
        assert_eq!(NaviTarget::from_pick_id(PickId(0)), None);
    }
}

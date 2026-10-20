//! The navigation-cube model: the piece taxonomy, ids, geometry, labels and overlay
//! camera.
//!
//! Producer-neutral and pure — no scene graph, no GPU, no Python. A **producer**
//! ([`super::inventor`], and later `super::stage`) turns a [`Piece`] into draw packets;
//! the controller ([`super`]) places the cube, mirrors the camera and picks it.
//!
//! Our axes follow the engine's default view: **+Z toward the viewer**, **+Y up**,
//! **+X right**. (FreeCAD is Z-up; the piece *names* differ, the behaviour does not.)

use ferrocad_graphics::{Camera, PickId, Projection, Rect, Viewport};
use ferrocad_types::{Placement, Rotation, Vector3};

/// Half the extent of the cube (each face sits at ±`CUBE_HALF`).
const CUBE_HALF: f64 = 0.5;
/// The chamfered cube's face square: half-size of each face's four vertices in the
/// tangent plane. The 12 edge bevels connect the shared vertices of adjacent squares and
/// the 8 corners are equilateral triangles (`0.5` plane, `0.38` tangent extent).
const FACE_HALF: f64 = 0.38;
/// Half the screen-space width of the glow ribbon along a hovered bevel.
const EDGE_GLOW_HALF: f64 = 0.024;
/// The face glyph's height in cube units (its strokes use a `[-0.5, 0.5]²` design box).
const GLYPH_SCALE: f64 = 0.38;
/// The face glyph's stroke width in cube units (filled quads, not 1px lines).
const GLYPH_STROKE: f64 = 0.06;
/// The cube's largest projected extent in cube space: its body diagonal, `√3`. A cube of
/// a given logical size therefore always fits inside a square of that size, whatever its
/// orientation; the compass ring and its labels need the larger overlay box below.
pub(crate) const CUBE_SPAN: f64 = 1.732_050_807_568_877_2;

/// The square overlay rect (logical px) needed to draw a cube whose largest projected
/// extent is `cube_size` logical px, together with the ground ring and its labels.
///
/// The widget is sized from the **cube**, not the other way round: the ortho camera has
/// to cover `RING_LABEL_RADIUS` as well, so the box is wider than the cube. Keeping this
/// derivation here means a change to the ring layout moves the box, never the cube.
pub(crate) fn overlay_size(cube_size: f32) -> f32 {
    cube_size * (2.0 * ORTHO_EXTENT / CUBE_SPAN) as f32
}

/// The orthographic camera's half-extent: how much of the cube space fills the rect.
/// Tight around the turntable's outer rim (`RING_RADIUS = 1.22`) so the cube fills the
/// frame instead of floating in it.
pub(crate) const ORTHO_EXTENT: f64 = 1.28;
/// How far the fixed overlay camera sits along +Z.
const CAMERA_DISTANCE: f64 = 3.0;

/// The compass turntable's inner rim radius (the band runs from here to `RING_RADIUS`).
pub(crate) const RING_INNER: f64 = 1.15;
/// The turntable's outer radius (the outer rim and the filled disc).
pub(crate) const RING_RADIUS: f64 = 1.22;
/// The ground plane the ring lies on: coplanar with the cube's base, so at elevation 0
/// the ring flattens into the base line and at higher elevations it opens into an
/// ellipse turntable under the cube.
pub(crate) const RING_Y: f64 = -CUBE_HALF;
/// The radius the `N/E/S/W` glyphs are anchored at: **inside** the rim band, between the
/// cube's corner radius (0.707) and the inner rim (1.15).
const RING_LABEL_RADIUS: f64 = 0.96;
/// The ring's scrub pick band, measured inward/outward from `RING_RADIUS`.
pub(crate) const RING_BAND_IN: f64 = 0.12;
pub(crate) const RING_BAND_OUT: f64 = 0.12;

/// The ring packets' pick id. Not a piece (the pieces are 1–26), so a ring packet never
/// resolves to a piece target.
pub const RING_PICK_ID: PickId = PickId(27);

/// The four edges of a face square as a line list over [`Piece::polygon`]'s vertices.
pub(crate) const FACE_OUTLINE: [u32; 8] = [0, 1, 1, 2, 2, 3, 3, 0];

/// The face square as two triangles.
pub(crate) const FACE_FAN: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A letter for a face: Front/Back/Top/Down/Left/Right. [`face_strokes`] draws it as 3D
/// line geometry in the face plane.
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

/// Straight-line strokes for a face letter in a `[-0.5, 0.5]²` box (u right, v up).
/// The face labels are **3D wireframe glyphs** (WA6), so they foreshorten with the facet
/// instead of sitting upright as a screen billboard.
fn face_strokes(letter: char) -> &'static [(f64, f64, f64, f64)] {
    match letter {
        'F' => &[
            (-0.28, 0.5, -0.28, -0.5),
            (-0.28, 0.5, 0.30, 0.5),
            (-0.28, 0.05, 0.14, 0.05),
        ],
        'T' => &[(-0.40, 0.5, 0.40, 0.5), (0.0, 0.5, 0.0, -0.5)],
        'L' => &[(-0.28, 0.5, -0.28, -0.5), (-0.28, -0.5, 0.32, -0.5)],
        'D' => &[
            (-0.28, 0.5, -0.28, -0.5),
            (-0.28, 0.5, 0.05, 0.5),
            (0.05, 0.5, 0.30, 0.28),
            (0.30, 0.28, 0.30, -0.28),
            (0.30, -0.28, 0.05, -0.5),
            (0.05, -0.5, -0.28, -0.5),
        ],
        'B' => &[
            (-0.30, 0.5, -0.30, -0.5),
            (-0.30, 0.5, 0.18, 0.5),
            (0.18, 0.5, 0.30, 0.36),
            (0.30, 0.36, 0.30, 0.14),
            (0.30, 0.14, 0.18, 0.02),
            (0.18, 0.02, -0.30, 0.02),
            (0.18, 0.02, 0.32, -0.12),
            (0.32, -0.12, 0.32, -0.36),
            (0.32, -0.36, 0.18, -0.5),
            (0.18, -0.5, -0.30, -0.5),
        ],
        'R' => &[
            (-0.30, 0.5, -0.30, -0.5),
            (-0.30, 0.5, 0.16, 0.5),
            (0.16, 0.5, 0.30, 0.36),
            (0.30, 0.36, 0.30, 0.14),
            (0.30, 0.14, 0.16, 0.02),
            (0.16, 0.02, -0.30, 0.02),
            (0.0, 0.02, 0.32, -0.5),
        ],
        _ => &[(-0.3, 0.5, -0.3, -0.5), (-0.3, 0.0, 0.3, 0.0)],
    }
}

/// The face letter as **filled quads** lying on the face plane, a hair above the facet.
/// Each stroke becomes a two-triangle ribbon `GLYPH_STROKE` wide; the strokes use the same
/// `orientation` in-plane axes as [`Piece::polygon`], so the letter is upright in the
/// face's snapped view and foreshortens with the 3D body.
pub(crate) fn face_glyph(piece: Piece) -> (Vec<[f32; 3]>, Vec<u32>) {
    let inverse = piece.orientation().inverse().to_matrix();
    let u = inverse.transform(&Vector3::new(1.0, 0.0, 0.0));
    let v = inverse.transform(&Vector3::new(0.0, 1.0, 0.0));
    let center = piece.direction().scale(CUBE_HALF + 0.002);
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let point = |u_coord: f64, v_coord: f64| {
        let p = center
            .add(&u.scale(u_coord * GLYPH_SCALE))
            .add(&v.scale(v_coord * GLYPH_SCALE));
        [p.x as f32, p.y as f32, p.z as f32]
    };
    for (x1, y1, x2, y2) in face_strokes(face_letter(piece)) {
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 1e-9 {
            continue;
        }
        let half = GLYPH_STROKE / 2.0;
        let (nx, ny) = (-dy / length * half, dx / length * half);
        let base = positions.len() as u32;
        positions.push(point(x1 + nx, y1 + ny));
        positions.push(point(x2 + nx, y2 + ny));
        positions.push(point(x2 - nx, y2 - ny));
        positions.push(point(x1 - nx, y1 - ny));
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (positions, indices)
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

    /// The piece's centre in cube space. Used to order the pieces back-to-front for
    /// the depth-disabled overlay: the overlay camera looks from +Z, so a larger
    /// view-space z is nearer.
    pub(crate) fn center(self) -> Vector3 {
        match self.kind() {
            PieceKind::Face => self.direction().scale(CUBE_HALF),
            PieceKind::Edge => Vector3::new(
                f64::from(self.axes[0]) * (CUBE_HALF + FACE_HALF) / 2.0,
                f64::from(self.axes[1]) * (CUBE_HALF + FACE_HALF) / 2.0,
                f64::from(self.axes[2]) * (CUBE_HALF + FACE_HALF) / 2.0,
            ),
            PieceKind::Corner => Vector3::new(
                f64::from(self.axes[0]) * (CUBE_HALF + 2.0 * FACE_HALF) / 3.0,
                f64::from(self.axes[1]) * (CUBE_HALF + 2.0 * FACE_HALF) / 3.0,
                f64::from(self.axes[2]) * (CUBE_HALF + 2.0 * FACE_HALF) / 3.0,
            ),
        }
    }

    /// The four corners of the face's **square**, wound so the flat normal points outward.
    /// Only valid for [`PieceKind::Face`].
    ///
    /// The in-plane axes come from [`orientation`](Self::orientation) — the same rotation
    /// the camera takes when it snaps onto this face. The square is the 24-vertex
    /// chamfered-cube construction: it shares its corners with the 12 bevel quads' long
    /// edges and the 8 triangular corner facets, so the surface is watertight.
    pub(crate) fn polygon(self) -> Vec<[f32; 3]> {
        let inverse = self.orientation().inverse().to_matrix();
        let u = inverse.transform(&Vector3::new(1.0, 0.0, 0.0));
        let v = inverse.transform(&Vector3::new(0.0, 1.0, 0.0));
        let center = self.direction().scale(CUBE_HALF);
        let a = FACE_HALF;
        [(a, a), (-a, a), (-a, -a), (a, -a)]
            .into_iter()
            .map(|(su, sv)| {
                let p = center.add(&u.scale(su)).add(&v.scale(sv));
                [p.x as f32, p.y as f32, p.z as f32]
            })
            .collect()
    }

    /// The four corners of the edge's **bevel quad**, wound so the flat normal points
    /// outward. Only valid for [`PieceKind::Edge`]. The quad connects the two shared
    /// vertices on each adjacent face square: its long edges are their twin in-plane lines
    /// and its short ends are the corner triangles' edges.
    pub(crate) fn bevel(self) -> Vec<[f32; 3]> {
        let along = (0..3)
            .find(|&i| self.axes[i] == 0)
            .expect("an edge has a free axis");
        let other: Vec<usize> = (0..3).filter(|&i| i != along).collect();
        let (i, j) = (other[0], other[1]);
        let sign = |axis: usize| f64::from(self.axes[axis]);
        let mut points = Vec::with_capacity(4);
        for (along_coord, i_coord, j_coord) in [
            (-FACE_HALF, CUBE_HALF, FACE_HALF),
            (FACE_HALF, CUBE_HALF, FACE_HALF),
            (FACE_HALF, FACE_HALF, CUBE_HALF),
            (-FACE_HALF, FACE_HALF, CUBE_HALF),
        ] {
            let mut point = [0.0_f64; 3];
            point[along] = along_coord;
            point[i] = sign(i) * i_coord;
            point[j] = sign(j) * j_coord;
            points.push([point[0] as f32, point[1] as f32, point[2] as f32]);
        }
        // Orient the quad out of the cube (the two adjacent face normals summed).
        let p = |index: usize| {
            Vector3::new(
                f64::from(points[index][0]),
                f64::from(points[index][1]),
                f64::from(points[index][2]),
            )
        };
        let normal = p(1).sub(&p(0)).cross(&p(2).sub(&p(0)));
        let outward = Vector3::new(sign(0), sign(1), sign(2));
        if normal.dot(&outward) < 0.0 {
            points.reverse();
        }
        points
    }

    /// The bevel's ridge line, midway between its two long edges. Used for the hover glow
    /// ribbon.
    pub(crate) fn bevel_centerline(self) -> Vec<[f32; 3]> {
        let bevel = self.bevel();
        let mid = |a: &[f32; 3], b: &[f32; 3]| {
            [
                (a[0] + b[0]) / 2.0,
                (a[1] + b[1]) / 2.0,
                (a[2] + b[2]) / 2.0,
            ]
        };
        // The long edges are 0-1 and 3-2 in either winding; their midpoints give the
        // ridge endpoints.
        vec![mid(&bevel[0], &bevel[3]), mid(&bevel[1], &bevel[2])]
    }

    /// The bevel quad as a triangle list.
    pub(crate) fn bevel_fan() -> [u32; 6] {
        [0, 1, 2, 0, 2, 3]
    }

    /// The bevel quad's four outline edges as a line list (the twin long edges plus the
    /// two short ends).
    pub(crate) fn bevel_outline() -> [u32; 8] {
        [0, 1, 1, 2, 2, 3, 3, 0]
    }

    /// The three vertices of the corner's **equilateral triangle**, wound so the flat
    /// normal points outward. Only valid for [`PieceKind::Corner`]. One vertex comes from
    /// each adjacent face square (`0.5` on that axis, `0.38` on the other two); the three
    /// bevel quads' short ends are exactly its edges, so the corner is watertight.
    pub(crate) fn corner_triangle(self) -> Vec<[f32; 3]> {
        let s = |i: usize| f64::from(self.axes[i]);
        let mut points: Vec<[f32; 3]> = [
            (CUBE_HALF, FACE_HALF, FACE_HALF),
            (FACE_HALF, CUBE_HALF, FACE_HALF),
            (FACE_HALF, FACE_HALF, CUBE_HALF),
        ]
        .into_iter()
        .map(|(x, y, z)| [(s(0) * x) as f32, (s(1) * y) as f32, (s(2) * z) as f32])
        .collect();
        let p = |index: usize| {
            Vector3::new(
                f64::from(points[index][0]),
                f64::from(points[index][1]),
                f64::from(points[index][2]),
            )
        };
        let normal = p(1).sub(&p(0)).cross(&p(2).sub(&p(0)));
        let outward = Vector3::new(s(0), s(1), s(2));
        if normal.dot(&outward) < 0.0 {
            points.reverse();
        }
        points
    }

    /// The corner triangle as a single triangle.
    pub(crate) fn corner_fan() -> [u32; 3] {
        [0, 1, 2]
    }

    /// The three edges of the corner triangle as a line list.
    pub(crate) fn corner_outline() -> [u32; 6] {
        [0, 1, 1, 2, 2, 0]
    }

    /// A screen-space ribbon along the bevel, for the hover glow. The renderer has no wide
    /// lines, so the glow is a translucent quad whose width is measured **in view
    /// space**: `rotation` maps the ridge to the view, the quad is built there, and
    /// `inverse` maps it back into the piece's local frame (which the packet transform
    /// then rotates again). Only valid for [`PieceKind::Edge`].
    pub(crate) fn edge_glow(
        self,
        rotation: &ferrocad_types::Matrix4,
        inverse: &ferrocad_types::Matrix4,
    ) -> Vec<[f32; 3]> {
        let segment = self.bevel_centerline();
        let (a, b) = (segment[0], segment[1]);
        let a = rotation.transform(&Vector3::new(
            f64::from(a[0]),
            f64::from(a[1]),
            f64::from(a[2]),
        ));
        let b = rotation.transform(&Vector3::new(
            f64::from(b[0]),
            f64::from(b[1]),
            f64::from(b[2]),
        ));
        let d = Vector3::new(b.x - a.x, b.y - a.y, 0.0);
        let length = d.length();
        if length < f64::EPSILON {
            return Vec::new();
        }
        let n = Vector3::new(-d.y / length, d.x / length, 0.0).scale(EDGE_GLOW_HALF);
        let local = |point: &Vector3| {
            let p = inverse.transform(point);
            [p.x as f32, p.y as f32, p.z as f32]
        };
        vec![
            local(&a.add(&n)),
            local(&b.add(&n)),
            local(&b.sub(&n)),
            local(&a.sub(&n)),
        ]
    }

    /// Whether this piece is fully behind the cube from the viewer's side: a face whose
    /// normal points away, an edge whose two adjacent faces both point away, or a corner
    /// whose three adjacent faces all do.
    ///
    /// Silhouette edges and corners stay bright — only geometry the cube itself hides is
    /// dimmed.
    pub(crate) fn is_hidden(self, rotation: &ferrocad_types::Matrix4) -> bool {
        match self.kind() {
            PieceKind::Face => rotation.transform(&self.direction()).z < 0.0,
            PieceKind::Edge | PieceKind::Corner => (0..3)
                .filter(|&i| self.axes[i] != 0)
                .all(|i| {
                    let mut axes = [0i8; 3];
                    axes[i] = self.axes[i];
                    rotation.transform(&Piece { axes }.direction()).z < 0.0
                }),
        }
    }
}

/// The pieces in back-to-front order for the overlay camera looking from +Z.
///
/// The overlay pass has depth testing off, so compositing order *is* visibility: the
/// away faces must come first (they are translucent), and a back edge or corner must
/// be covered by the front faces rather than painted over them. Sorting by the
/// piece's view-space depth gives that for the whole piece set.
///
/// Ties are broken by pick id so both producers (and repeated rebuilds) emit the same
/// stable order.
pub(crate) fn draw_order(orientation: Rotation) -> Vec<Piece> {
    let rotation = orientation.to_matrix();
    let mut pieces = Piece::ALL.to_vec();
    pieces.sort_by(|a, b| {
        let za = rotation.transform(&a.center()).z;
        let zb = rotation.transform(&b.center()).z;
        za.partial_cmp(&zb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.pick_id().0.cmp(&b.pick_id().0))
    });
    pieces
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

/// The default "home" orientation the chrome's Home control snaps to: an axonometric view
/// with the front-top-right corner toward the viewer. Upstream's Home restores the
/// configurable `NewDocumentCameraOrientation` (default `Top`); a fixed default keeps this
/// build self-contained.
pub fn home_orientation() -> Rotation {
    Piece { axes: [1, 1, 1] }.orientation()
}

/// The camera's compass bearing: the eye direction's angle in the horizontal **XZ** plane,
/// measured CCW from +X, in radians. The ring's fixed labels map `+X` to east (0),
/// `+Z` to north (π/2), `−X` to west (π) and `−Z` to south (3π/2).
pub(crate) fn compass_angle(orientation: &Rotation) -> f64 {
    let eye = orientation
        .inverse()
        .to_matrix()
        .transform(&Vector3::new(0.0, 0.0, 1.0));
    let angle = eye.z.atan2(eye.x).rem_euclid(std::f64::consts::TAU);
    // `atan2(-0.0, +x)` wraps to a full turn; report east as 0.
    if angle >= std::f64::consts::TAU - 1e-12 {
        0.0
    } else {
        angle
    }
}

/// The fixed compass labels, `(letter, screen angle)` — north at the top, east right,
/// south bottom, west left.
pub(crate) const COMPASS_LABELS: [(char, f64); 4] = [
    ('N', std::f64::consts::FRAC_PI_2),
    ('E', 0.0),
    ('S', -std::f64::consts::FRAC_PI_2),
    ('W', std::f64::consts::PI),
];

/// The number of segments in the turntable's disc, band and rim loops.
const RING_SEGMENTS: usize = 64;

fn ring_vertex(angle: f64, radius: f64, y: f64) -> [f32; 3] {
    [
        (radius * angle.cos()) as f32,
        y as f32,
        (radius * angle.sin()) as f32,
    ]
}

/// The turntable's **filled floor disc**: a dark, translucent 64-gon just below the cube's
/// base plane. It grounds the cube against the viewport instead of leaving it floating
/// inside an empty wire hoop.
pub(crate) fn ring_disc() -> (Vec<[f32; 3]>, Vec<u32>) {
    let y = RING_Y - 0.005;
    let mut positions = Vec::with_capacity(RING_SEGMENTS + 1);
    positions.push([0.0, y as f32, 0.0]);
    for i in 0..RING_SEGMENTS {
        let angle = std::f64::consts::TAU * i as f64 / RING_SEGMENTS as f64;
        positions.push(ring_vertex(angle, RING_RADIUS, y));
    }
    let mut indices = Vec::with_capacity(RING_SEGMENTS * 3);
    // Wind the fan upward (+Y) so the floor is visible from above.
    for i in 0..RING_SEGMENTS {
        indices.push(0);
        indices.push((((i + 1) % RING_SEGMENTS) + 1) as u32);
        indices.push((i + 1) as u32);
    }
    (positions, indices)
}

/// The **thick rim band** between `RING_INNER` and `RING_RADIUS` as a quad strip in the
/// ground plane: the turntable track the compass letters sit inside.
pub(crate) fn ring_band() -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::with_capacity(RING_SEGMENTS * 4);
    let mut indices = Vec::with_capacity(RING_SEGMENTS * 6);
    for i in 0..RING_SEGMENTS {
        let a = std::f64::consts::TAU * i as f64 / RING_SEGMENTS as f64;
        let b = std::f64::consts::TAU * (i + 1) as f64 / RING_SEGMENTS as f64;
        let base = positions.len() as u32;
        positions.push(ring_vertex(a, RING_INNER, RING_Y));
        positions.push(ring_vertex(b, RING_INNER, RING_Y));
        positions.push(ring_vertex(b, RING_RADIUS, RING_Y));
        positions.push(ring_vertex(a, RING_RADIUS, RING_Y));
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (positions, indices)
}

/// The turntable's crisp **inner and outer rim loops** plus a radial tick every 30° as one
/// line list in the ground plane. The viewport's orientation rotation turns it into the
/// projected ellipse.
pub(crate) fn ring_rim_lines() -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut points = Vec::with_capacity(RING_SEGMENTS * 2 + 24);
    let mut indices = Vec::with_capacity(RING_SEGMENTS * 4 + 24);
    for radius in [RING_INNER, RING_RADIUS] {
        let base = points.len() as u32;
        for i in 0..RING_SEGMENTS {
            let angle = std::f64::consts::TAU * i as f64 / RING_SEGMENTS as f64;
            points.push(ring_vertex(angle, radius, RING_Y));
        }
        for i in 0..RING_SEGMENTS {
            indices.push(base + i as u32);
            indices.push(base + ((i + 1) % RING_SEGMENTS) as u32);
        }
    }
    for k in 0..12 {
        let angle = std::f64::consts::TAU * k as f64 / 12.0;
        let base = points.len() as u32;
        points.push(ring_vertex(angle, RING_INNER, RING_Y));
        points.push(ring_vertex(angle, RING_RADIUS, RING_Y));
        indices.push(base);
        indices.push(base + 1);
    }
    (points, indices)
}

/// The ground-plane anchor for a compass letter. The viewport projects it with the cube's
/// view rotation and draws the letter as gpui text there. The anchor sits **inside** the
/// rim band, between the cube's corner radius and the inner rim.
pub(crate) fn ring_label_position(angle: f64) -> [f32; 3] {
    ring_vertex(angle, RING_LABEL_RADIUS, RING_Y)
}

/// The heading marker: a small diamond lying in the ring plane at `angle`.
pub(crate) fn ring_marker(angle: f64) -> (Vec<[f32; 3]>, Vec<u32>) {
    let (cos, sin) = (angle.cos(), angle.sin());
    let (cx, cz) = (RING_RADIUS * cos, RING_RADIUS * sin);
    let h = 0.06;
    // Tangential and radial half-axes in the plane.
    let (tx, tz) = (-sin * h, cos * h);
    let (rx, rz) = (cos * h, sin * h);
    let point = |dx: f64, dz: f64| [(cx + dx) as f32, RING_Y as f32, (cz + dz) as f32];
    let positions = vec![
        point(-tx, -tz),
        point(-rx, -rz),
        point(tx, tz),
        point(rx, rz),
    ];
    (positions, vec![0, 1, 2, 0, 2, 3])
}

/// The direction a chrome yaw control turns the eye about the vertical (+Y) axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YawDirection {
    /// Move the eye counter-clockwise around the scene (azimuth increases).
    CounterClockwise,
    /// Move the eye clockwise around the scene (azimuth decreases).
    Clockwise,
}

impl YawDirection {
    /// The signed world-Y yaw in radians (CCW positive).
    pub fn angle(self) -> f64 {
        match self {
            Self::CounterClockwise => std::f64::consts::FRAC_PI_2,
            Self::Clockwise => -std::f64::consts::FRAC_PI_2,
        }
    }
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
    fn face_squares_are_flat_and_centred_on_the_face() {
        let v = |p: [f32; 3]| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        for piece in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Face) {
            let polygon = piece.polygon();
            assert_eq!(polygon.len(), 4, "a square face in the 24-vertex chamfer");
            let direction = piece.direction();
            for point in &polygon {
                assert!(
                    (v(*point).dot(&direction) - CUBE_HALF).abs() < 1e-9,
                    "{piece:?} vertex is off the face plane"
                );
            }
            let centroid = polygon
                .iter()
                .fold(Vector3::zero(), |sum, point| sum.add(&v(*point)))
                .scale(1.0 / 4.0);
            assert!((centroid.sub(&direction.scale(CUBE_HALF))).length() < 1e-9);
            // Each side is 2·FACE_HALF.
            for i in 0..4 {
                let side = v(polygon[(i + 1) % 4]).sub(&v(polygon[i])).length();
                assert!((side - 2.0 * FACE_HALF).abs() < 1e-6);
            }
            // The winding points away from the cube.
            let normal = v(polygon[1])
                .sub(&v(polygon[0]))
                .cross(&v(polygon[2]).sub(&v(polygon[0])));
            assert!(normal.dot(&direction) > 0.0, "{piece:?} winds inward");
        }
    }

    #[test]
    fn bevel_quads_bridge_adjacent_face_squares() {
        let v = |p: [f32; 3]| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let short = (CUBE_HALF - FACE_HALF) * std::f64::consts::SQRT_2;
        for edge in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Edge) {
            let quad = edge.bevel();
            assert_eq!(quad.len(), 4);
            for point in &quad {
                assert!(
                    point
                        .iter()
                        .all(|value| value.abs() <= CUBE_HALF as f32 + 1e-6),
                    "{edge:?} protrudes past the cube"
                );
            }
            // Two long twin edges and two short ends connecting the corner triangles.
            let mut lengths: Vec<f64> = [0, 1, 2, 3]
                .iter()
                .map(|&i| v(quad[(i + 1) % 4]).sub(&v(quad[i])).length())
                .collect();
            lengths.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert!((lengths[0] - short).abs() < 1e-6);
            assert!((lengths[3] - 2.0 * FACE_HALF).abs() < 1e-6);
            assert!((lengths[0] - lengths[1]).abs() < 1e-6);
            assert!((lengths[2] - lengths[3]).abs() < 1e-6);
            // The quad is planar and wound out of the cube.
            let normal = v(quad[1])
                .sub(&v(quad[0]))
                .cross(&v(quad[2]).sub(&v(quad[0])));
            assert!(normal.dot(&edge.direction()) > 0.0, "{edge:?} winds inward");
            assert!(
                (v(quad[3]).sub(&v(quad[0])).dot(&normal)).abs() < 1e-6,
                "{edge:?} is not planar"
            );
        }
    }

    #[test]
    fn corner_triangles_are_watertight_and_wound_outward() {
        let v = |p: [f32; 3]| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let plane = CUBE_HALF + 2.0 * FACE_HALF;
        let side = (CUBE_HALF - FACE_HALF) * std::f64::consts::SQRT_2;
        for corner in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Corner) {
            let triangle = corner.corner_triangle();
            assert_eq!(triangle.len(), 3, "an equilateral cut corner");
            for vertex in &triangle {
                assert!(
                    vertex
                        .iter()
                        .all(|value| value.abs() <= CUBE_HALF as f32 + 1e-6),
                    "{corner:?} protrudes past the cube"
                );
                let sum: f64 = vertex.iter().map(|value| f64::from(*value).abs()).sum();
                assert!(
                    (sum - plane).abs() < 1e-6,
                    "{corner:?} is off its cut plane"
                );
            }
            for i in 0..3 {
                let edge = v(triangle[(i + 1) % 3]).sub(&v(triangle[i])).length();
                assert!((edge - side).abs() < 1e-6, "{corner:?} is not equilateral");
            }
            let normal = v(triangle[1])
                .sub(&v(triangle[0]))
                .cross(&v(triangle[2]).sub(&v(triangle[0])));
            assert!(
                normal.dot(&corner.direction()) > 0.0,
                "{corner:?} winds inward"
            );
        }
    }

    #[test]
    fn face_glyphs_are_filled_quads_in_the_face_plane() {
        let v = |p: [f32; 3]| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        for face in Piece::ALL.iter().filter(|p| p.kind() == PieceKind::Face) {
            let (points, indices) = face_glyph(*face);
            assert!(!points.is_empty(), "{face:?} has no glyph");
            assert_eq!(points.len() % 4, 0, "four quads per filled stroke set");
            assert_eq!(indices.len(), points.len() / 4 * 6);
            assert!(indices.iter().all(|&i| (i as usize) < points.len()));
            for point in &points {
                assert!(
                    (v(*point).dot(&face.direction()) - (CUBE_HALF + 0.002)).abs() < 1e-6,
                    "{face:?} glyph is off the face plane"
                );
            }
        }
    }

    #[test]
    fn the_compass_angle_follows_the_eye_in_xz() {
        // Identity: the eye is +Z, which the ring calls north (π/2).
        assert!((compass_angle(&Rotation::identity()) - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // A view yawed by −90° puts the eye on +X (east, angle 0).
        let east = Rotation::from_axis_angle(
            &Vector3::new(0.0, 1.0, 0.0),
            -std::f64::consts::FRAC_PI_2,
        );
        assert!(compass_angle(&east).abs() < 1e-9);
        // ...and +90° puts it on −X (west, π).
        let west = Rotation::from_axis_angle(
            &Vector3::new(0.0, 1.0, 0.0),
            std::f64::consts::FRAC_PI_2,
        );
        assert!((compass_angle(&west) - std::f64::consts::PI).abs() < 1e-9);
    }

    #[test]
    fn the_turntable_has_a_filled_disc_a_band_and_two_rim_loops() {
        let (disc, disc_indices) = ring_disc();
        assert_eq!(disc.len(), 64 + 1, "a centre and a 64-point rim");
        assert_eq!(disc_indices.len(), 64 * 3);
        assert!((f64::from(disc[0][1]) - (RING_Y - 0.005)).abs() < 1e-6);

        let (band, band_indices) = ring_band();
        assert_eq!(band.len(), 64 * 4);
        assert_eq!(band_indices.len(), 64 * 6);
        for quad in band.chunks_exact(4) {
            let radii: Vec<f64> = quad
                .iter()
                .map(|point| (f64::from(point[0]).powi(2) + f64::from(point[2]).powi(2)).sqrt())
                .collect();
            assert!((radii[0] - RING_INNER).abs() < 1e-5);
            assert!((radii[1] - RING_INNER).abs() < 1e-5);
            assert!((radii[2] - RING_RADIUS).abs() < 1e-5);
            assert!((radii[3] - RING_RADIUS).abs() < 1e-5);
        }

        let (points, indices) = ring_rim_lines();
        assert_eq!(points.len(), 64 * 2 + 24);
        assert_eq!(indices.len(), 64 * 4 + 24);
        assert!(indices.iter().all(|&i| (i as usize) < points.len()));
        // The first 64 points are the inner loop, the next 64 the outer one.
        for (index, point) in points[..128].iter().enumerate() {
            assert!((f64::from(point[1]) - RING_Y).abs() < 1e-6);
            let radius = (f64::from(point[0]).powi(2) + f64::from(point[2]).powi(2)).sqrt();
            let expected = if index < 64 { RING_INNER } else { RING_RADIUS };
            assert!((radius - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn the_compass_labels_sit_inside_the_turntable_track() {
        for (_, angle) in COMPASS_LABELS {
            let point = ring_label_position(angle);
            assert!((f64::from(point[1]) - RING_Y).abs() < 1e-6);
            let radius = (f64::from(point[0]).powi(2) + f64::from(point[2]).powi(2)).sqrt();
            // Inside the rim band, outside the cube's corner radius (√2/2).
            assert!(radius > 0.7071);
            assert!(radius < RING_INNER);
        }
    }

    #[test]
    fn yaw_directions_are_opposite_quarter_turns() {
        assert!(
            (YawDirection::CounterClockwise.angle() - std::f64::consts::FRAC_PI_2).abs() < 1e-12
        );
        assert!((YawDirection::Clockwise.angle() + std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn only_geometry_the_cube_hides_is_hidden() {
        let identity = Rotation::identity().to_matrix();
        // Faces point away by their own normal.
        assert!(Piece::BACK.is_hidden(&identity));
        assert!(!Piece::FRONT.is_hidden(&identity));

        // From an axonometric view the -X/-Y/-Z faces all point away, so the edge and
        // corner behind them are hidden; a silhouette edge/corner stays bright.
        let axonometric = Rotation::from_vectors(
            Vector3::new(1.0, 0.9, 1.0).normalize(),
            Vector3::new(0.0, 0.0, 1.0),
        )
        .to_matrix();
        assert!(Piece { axes: [-1, -1, 0] }.is_hidden(&axonometric));
        assert!(!Piece { axes: [1, -1, 0] }.is_hidden(&axonometric));
        assert!(Piece { axes: [-1, -1, -1] }.is_hidden(&axonometric));
        assert!(!Piece { axes: [1, -1, -1] }.is_hidden(&axonometric));
    }
}

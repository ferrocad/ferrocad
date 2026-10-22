//! The stage producer: builds the navigation cube from declarative [`ShapeItem`]s.
//!
//! modern counterpart of the inventor producer — the same cube, compiled straight to
//! the shared draw contract with no scene graph. The items are built **once** (so their
//! mesh ids are stable and a rebuild never re-uploads) and recoloured / re-oriented per
//! rebuild; the cube's rotation is baked into each piece's transform (there is no
//! `Transform` node in the stage). Face letters are filled 3D quads (`model::face_glyph`)
//! emitted as shaded items; only the compass `N/E/S/W` letters remain gpui text drawn by
//! the viewport (`NaviCube::labels`).

use ferrocad_graphics::{RenderQueue, Viewport};
use ferrocad_stage::{DrawMode, ShapeItem};
use ferrocad_types::{Placement, Vector3};

use super::model::{FACE_FAN, FACE_OUTLINE, Piece, PieceKind, draw_order, face_glyph};
use super::theme::NavicubeTheme;
use super::view::{NavicubeView, NavicubeVisual};

/// The navigation cube as a `Stage` of declarative items: the modern [`NavicubeView`].
pub struct StageProducer {
    /// The 26 pieces, indexed by `Piece::ALL` position (built once, so mesh ids are
    /// stable and a rebuild never re-uploads).
    pieces: Vec<ShapeItem>,
    /// One filled 3D glyph item per face (WA7), emitted after the body.
    labels: Vec<(Piece, ShapeItem)>,
    queue: RenderQueue,
}

impl StageProducer {
    /// A producer holding an empty queue (call [`NavicubeView::rebuild`] before drawing).
    pub fn new() -> Self {
        Self {
            pieces: Piece::ALL.iter().copied().map(piece_item).collect(),
            labels: Piece::ALL
                .iter()
                .copied()
                .filter(|piece| piece.kind() == PieceKind::Face)
                .map(|piece| (piece, label_item(piece)))
                .collect(),
            queue: RenderQueue::default(),
        }
    }
}

impl Default for StageProducer {
    fn default() -> Self {
        Self::new()
    }
}

impl NavicubeView for StageProducer {
    fn rebuild(&mut self, _viewport: &Viewport, visual: &NavicubeVisual, theme: &NavicubeTheme) {
        let rotation = Placement::new(Vector3::zero(), visual.orientation).to_matrix();
        // Recolour and re-orient every item (no geometry change, so no re-upload).
        for (index, piece) in Piece::ALL.iter().copied().enumerate() {
            let role = piece.role(visual.hilite, &visual.away);
            let back = piece.is_hidden(&rotation);
            let highlighted = visual.hilite == Some(piece);
            let item = &mut self.pieces[index];
            let edge = if back && !highlighted {
                theme.edge_away
            } else {
                theme.edge_color(role, visual.proximity)
            };
            // The face octagon's edges are shared with the truncated edges and corner
            // triangles, which own the resting wireframe; only a hovered face lights its
            // own outline. Edge and corner items always show their edges.
            let edge = match piece.kind() {
                PieceKind::Face if !highlighted => [0.0; 4],
                _ => edge,
            };
            let surface = if piece.kind() == PieceKind::Face {
                theme.face_fill(role, &rotation.transform(&piece.direction()))
            } else if piece.kind() == PieceKind::Edge {
                theme.bevel_color(role, back, visual.proximity)
            } else {
                theme.color(role)
            };
            item.set_surface_color(surface);
            item.set_edge_color(edge);
            item.set_temporary_transform(Some(rotation));
        }
        for (piece, label) in &mut self.labels {
            let role = piece.role(visual.hilite, &visual.away);
            label.set_surface_color(theme.label_color(role, visual.proximity));
            label.set_temporary_transform(Some(rotation));
        }

        // Emit in the same back-to-front order as the inventor producer, then the filled
        // glyphs on top (no hover cone: WA7 removed it).
        let mut queue = RenderQueue::default();
        for piece in draw_order(visual.orientation) {
            // Strict back-face culling: the chamfered cube is convex, so a piece whose
            // outward normal points away can never contribute to the image.
            if piece.is_hidden(&rotation) {
                continue;
            }
            let index = Piece::ALL
                .iter()
                .position(|candidate| *candidate == piece)
                .expect("a piece from Piece::ALL");
            self.pieces[index].emit(&mut queue);
        }
        for (piece, label) in &self.labels {
            if !piece.is_hidden(&rotation)
                && piece.role(visual.hilite, &visual.away) != super::theme::NavicubeRole::FaceAway
            {
                label.emit(&mut queue);
            }
        }
        self.queue = queue;
    }

    fn queue(&self) -> &RenderQueue {
        &self.queue
    }
}

/// A piece as a stage item: a shaded octagon for faces, a shaded bevel quad with its four
/// outline edges for edges, and a shaded chamfer patch for corners.
fn piece_item(piece: Piece) -> ShapeItem {
    let item = ShapeItem::new(piece.pick_id().0);
    match piece.kind() {
        PieceKind::Face => item.with_draw_mode(DrawMode::ShadedWithEdges).with_shape(
            piece.polygon(),
            vec![],
            FACE_FAN.to_vec(),
            FACE_OUTLINE.to_vec(),
        ),
        PieceKind::Edge => item.with_draw_mode(DrawMode::ShadedWithEdges).with_shape(
            piece.bevel(),
            vec![],
            Piece::bevel_fan().to_vec(),
            Piece::bevel_outline().to_vec(),
        ),
        PieceKind::Corner => item.with_draw_mode(DrawMode::ShadedWithEdges).with_shape(
            piece.corner_triangle(),
            vec![],
            Piece::corner_fan().to_vec(),
            Piece::corner_outline().to_vec(),
        ),
    }
}

/// A face's filled 3D glyph as a shaded stage item (the surface colour carries the ink).
fn label_item(piece: Piece) -> ShapeItem {
    let (positions, indices) = face_glyph(piece);
    ShapeItem::new(piece.pick_id().0)
        .with_draw_mode(DrawMode::Shaded)
        .with_shape(positions, vec![], indices, vec![])
}

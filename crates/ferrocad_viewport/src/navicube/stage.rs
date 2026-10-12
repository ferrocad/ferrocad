//! The stage producer: builds the navigation cube from declarative [`ShapeItem`]s.
//!
//! The modern counterpart of the inventor producer — the same cube, compiled straight to
//! the shared draw contract with no scene graph. The items are built **once** (so their
//! mesh ids are stable and a rebuild never re-uploads) and recoloured / re-oriented per
//! rebuild; the cube's rotation is baked into each piece's transform (there is no
//! `Transform` node in the stage). The Home button is left unrotated. Labels are not
//! drawn (the stage has no per-item texture path), so
//! [`has_labels`](NavicubeView::has_labels) stays `false`.

use ferrocad_graphics::{RenderQueue, Viewport};
use ferrocad_stage::{DrawMode, ShapeItem};
use ferrocad_types::{Placement, Vector3};

use super::model::{
    BASE_COLOR, HILITE_COLOR, HOME_COLOR, HOME_PICK_ID, INACTIVE_COLOR, NaviTarget, Piece, PieceKind,
    home_quad,
};
use super::view::{NavicubeView, NavicubeVisual};

/// The navigation cube as a `Stage` of declarative items: the modern [`NavicubeView`].
pub struct StageProducer {
    /// The 26 pieces, indexed by `Piece::ALL` position (built once, so mesh ids are
    /// stable and a rebuild never re-uploads).
    pieces: Vec<ShapeItem>,
    /// The fixed Home button (a sibling of the pieces; never rotated).
    home: ShapeItem,
    queue: RenderQueue,
}

impl StageProducer {
    /// A producer holding an empty queue (call [`NavicubeView::rebuild`] before drawing).
    pub fn new() -> Self {
        Self {
            pieces: Piece::ALL.iter().copied().map(piece_item).collect(),
            home: ShapeItem::new(HOME_PICK_ID.0)
                .with_draw_mode(DrawMode::Shaded)
                .with_shape(home_quad(), vec![], vec![0, 1, 2, 0, 2, 3], vec![]),
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
    fn rebuild(&mut self, _viewport: &Viewport, visual: &NavicubeVisual) {
        let rotation = Placement::new(Vector3::zero(), visual.orientation).to_matrix();
        let color_of = |piece: Piece| {
            if visual.hilite == Some(NaviTarget::Piece(piece)) {
                HILITE_COLOR
            } else if visual.away.contains(&piece) {
                INACTIVE_COLOR
            } else {
                BASE_COLOR
            }
        };

        // Recolour and re-orient every item (no geometry change, so no re-upload).
        for (index, piece) in Piece::ALL.iter().copied().enumerate() {
            let item = &mut self.pieces[index];
            let color = color_of(piece);
            item.set_surface_color(color);
            item.set_edge_color(color);
            item.set_temporary_transform(Some(rotation));
        }
        let home_color = if visual.hilite == Some(NaviTarget::Home) {
            HILITE_COLOR
        } else {
            HOME_COLOR
        };
        self.home.set_surface_color(home_color);
        self.home.set_temporary_transform(None);

        // Emit in the same order as the inventor producer: away-facing faces first
        // (translucent, behind), then the rest, then the Home button.
        let mut queue = RenderQueue::default();
        for (index, piece) in Piece::ALL.iter().copied().enumerate() {
            if visual.away.contains(&piece) {
                self.pieces[index].emit(&mut queue);
            }
        }
        for (index, piece) in Piece::ALL.iter().copied().enumerate() {
            if !visual.away.contains(&piece) {
                self.pieces[index].emit(&mut queue);
            }
        }
        self.home.emit(&mut queue);
        self.queue = queue;
    }

    fn queue(&self) -> &RenderQueue {
        &self.queue
    }
}

/// A piece as a stage item: a shaded surface for faces and corners, a wireframe for
/// edges.
fn piece_item(piece: Piece) -> ShapeItem {
    let item = ShapeItem::new(piece.pick_id().0);
    match piece.kind() {
        PieceKind::Face => item
            .with_draw_mode(DrawMode::Shaded)
            .with_shape(piece.quad(), vec![], vec![0, 1, 2, 0, 2, 3], vec![]),
        PieceKind::Edge => item
            .with_draw_mode(DrawMode::Wireframe)
            .with_shape(piece.segment(), vec![], vec![], vec![0, 1]),
        PieceKind::Corner => {
            let (positions, triangles) = piece.corner_cube();
            item.with_draw_mode(DrawMode::Shaded)
                .with_shape(positions, vec![], triangles, vec![])
        }
    }
}

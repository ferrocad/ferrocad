//! Semantic, host-facing **directives** produced alongside the draw queue.
//!
//! Some of what a scene or a stack produces is semantic, not drawable: a renderer must
//! ignore it, but the viewport must consume it. [`Directives`] is that channel — the
//! compiler (or a stage) fills it in the same traversal that emits
//! [`DrawPacket`](crate::DrawPacket)s, and it travels beside the
//! [`RenderQueue`](crate::RenderQueue) as `(RenderQueue, Directives)`. The renderer never
//! receives it. See `docs/directives.md`.

use ferrocad_types::Rotation;

use crate::batch::PickId;
use crate::pixels::Rect;

/// Semantic data produced beside the draw queue — the renderer ignores it, the viewport
/// consumes it.
#[derive(Clone, Debug, Default)]
pub struct Directives {
    directives: Vec<Directive>,
}

impl Directives {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a directive.
    pub fn push(&mut self, directive: Directive) {
        self.directives.push(directive);
    }

    /// The number of directives.
    pub fn len(&self) -> usize {
        self.directives.len()
    }

    /// Whether there are no directives.
    pub fn is_empty(&self) -> bool {
        self.directives.is_empty()
    }

    /// Iterate the directives in scene order.
    pub fn iter(&self) -> impl Iterator<Item = &Directive> {
        self.directives.iter()
    }

    /// The navicube overlay requests, in scene order.
    pub fn navicubes(&self) -> impl Iterator<Item = &NaviCubeDirective> {
        self.directives.iter().filter_map(|directive| match directive {
            Directive::NaviCube(navicube) => Some(navicube),
        })
    }
}

/// One semantic directive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Directive {
    /// A host-rendered navicube overlay (produced by a `SoNaviCube` node).
    NaviCube(NaviCubeDirective),
}

/// A neutral navicube overlay request — structural only. Styling is the viewport theme's
/// concern (see `docs/directives.md` §5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NaviCubeDirective {
    /// The device-pixel rectangle the overlay occupies (bottom-left origin, like
    /// [`Rect`](crate::Rect)).
    pub rect: Rect,
    /// The cube's orientation; `None` mirrors the main camera (the modern default).
    pub orientation: Option<Rotation>,
    /// The highlighted piece or Home button, if any.
    pub hilite: Option<PickId>,
    /// Whether the overlay uses an orthographic camera (the perspective fallback is
    /// deferred).
    pub orthographic: bool,
}

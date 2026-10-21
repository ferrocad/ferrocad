//! Semantic, host-facing **directives** produced alongside the draw queue.
//!
//! Some of what a scene or a stack produces is semantic, not drawable: a renderer must
//! ignore it, but the viewport must consume it. [`Directives`] is that channel — the
//! compiler (or a stage) fills it in the same traversal that emits
//! [`DrawPacket`](crate::DrawPacket)s, and it travels beside the
//! [`RenderQueue`](crate::RenderQueue) as `(RenderQueue, Directives)`. The renderer never
//! receives it. See `docs/viewport/directives.md`.

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
            Directive::EventCallback(_) => None,
        })
    }

    /// The active `SoEventCallback` ids, in scene order — the callback nodes the
    /// traversal reached (an inactive `SoSwitch` branch contributes none).
    pub fn event_callbacks(&self) -> impl Iterator<Item = u64> + '_ {
        self.directives.iter().filter_map(|directive| match directive {
            Directive::EventCallback(id) => Some(*id),
            Directive::NaviCube(_) => None,
        })
    }
}

/// One semantic directive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Directive {
    /// A host-rendered navicube overlay (produced by a `SoNaviCube` node).
    NaviCube(NaviCubeDirective),
    /// An active `SoEventCallback` node: the host routes input events to the Python
    /// callables keyed by this id. It draws nothing.
    EventCallback(u64),
}

/// A neutral navicube overlay request — structural only. Styling is the viewport theme's
/// concern (see `docs/viewport/directives.md` §5).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::Rect;

    #[test]
    fn filters_navicubes_and_event_callbacks_by_variant() {
        let mut directives = Directives::new();
        directives.push(Directive::EventCallback(7));
        directives.push(Directive::NaviCube(NaviCubeDirective {
            rect: Rect::new(0, 0, 10, 10),
            orientation: None,
            hilite: None,
            orthographic: true,
        }));
        directives.push(Directive::EventCallback(9));

        assert_eq!(directives.event_callbacks().collect::<Vec<_>>(), vec![7, 9]);
        assert_eq!(directives.navicubes().count(), 1);
        assert_eq!(directives.len(), 3);
        assert!(!directives.is_empty());
    }
}

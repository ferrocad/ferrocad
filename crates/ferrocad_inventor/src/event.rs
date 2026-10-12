//! Device-space input events and the listener that reacts to them.
//!
//! The analogue of Coin's event plumbing: the host turns window input into
//! [`Event`]s — already converted to device pixels by [`Viewport`](crate::Viewport)
//! — and the view delivers them, after picking, to an [`EventListener`] (the
//! counterpart of a `SoEventCallback` node). The listener may edit the scene (for
//! example, to highlight a hovered object) and reports whether it did.

use crate::batch::PickId;
use crate::node::Scene;

/// A device-space input event (bottom-left origin), as the host delivers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pointer moved to `position` (device pixels, bottom-left origin).
    MouseMove {
        /// The pointer position in device pixels.
        position: [i32; 2],
    },
    /// The pointer left the viewport.
    MouseLeave,
}

/// Reacts to [`Event`]s, like a Coin `SoEventCallback`.
pub trait EventListener {
    /// Handle `event`, where `hit` is the [`PickId`] under the pointer, if any.
    ///
    /// Return `true` if `scene` was changed and must be recompiled.
    fn on_event(&mut self, event: &Event, hit: Option<PickId>, scene: &mut Scene) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeKind;

    /// A listener that recolours a hovered material node.
    struct Highlighter {
        hovered: Option<PickId>,
        target: PickId,
    }

    impl EventListener for Highlighter {
        fn on_event(&mut self, event: &Event, hit: Option<PickId>, scene: &mut Scene) -> bool {
            let next = match event {
                Event::MouseMove { .. } => hit.filter(|id| *id == self.target),
                Event::MouseLeave => None,
            };
            if next == self.hovered {
                return false;
            }
            self.hovered = next;
            if let Some(child) = scene.root.children.first_mut() {
                if let NodeKind::Material(color) = &mut child.kind {
                    *color = if self.hovered.is_some() {
                        [1.0, 0.0, 0.0, 1.0]
                    } else {
                        [1.0, 1.0, 1.0, 1.0]
                    };
                }
            }
            true
        }
    }

    fn material_scene() -> Scene {
        Scene {
            root: crate::node::Node {
                kind: NodeKind::Separator,
                children: vec![crate::node::Node {
                    kind: NodeKind::Material([1.0, 1.0, 1.0, 1.0]),
                    children: vec![],
                }],
            },
        }
    }

    #[test]
    fn listener_changes_the_scene_on_hover() {
        let mut listener = Highlighter {
            hovered: None,
            target: PickId(7),
        };
        let mut scene = material_scene();

        let event = Event::MouseMove { position: [1, 2] };
        assert!(listener.on_event(&event, Some(PickId(7)), &mut scene));
        assert_eq!(material(&scene), [1.0, 0.0, 0.0, 1.0]);
        // A second move onto the same object is not a change.
        assert!(!listener.on_event(&event, Some(PickId(7)), &mut scene));
        // Leaving clears the highlight.
        assert!(listener.on_event(&Event::MouseLeave, None, &mut scene));
        assert_eq!(material(&scene), [1.0, 1.0, 1.0, 1.0]);
    }

    fn material(scene: &Scene) -> [f32; 4] {
        match scene.root.children[0].kind {
            NodeKind::Material(color) => color,
            _ => unreachable!(),
        }
    }
}

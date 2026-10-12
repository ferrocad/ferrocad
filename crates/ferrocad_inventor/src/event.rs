//! Device-space input events, the listener that reacts to them, and the
//! `SoEventCallback` lookup.
//!
//! The analogue of Coin's event plumbing: the host turns window input into
//! [`Event`]s — already converted to device pixels by [`Viewport`](crate::Viewport)
//! — and the view delivers them, after picking, to an [`EventListener`] (the
//! counterpart of a `SoEventCallback` node). The listener may edit the scene (for
//! example, to highlight a hovered object) and reports whether it did.
//!
//! [`event_callbacks`] is how a host finds the `SoEventCallback` nodes active in a
//! scene (the ids carried by [`NodeKind::EventCallback`](crate::NodeKind)); the
//! Python callables keyed by those ids live on the `pivy.coin` facade.

use crate::node::{Node, NodeKind, Scene, SO_SWITCH_ALL};
use crate::pick::Hit;

/// A device-space input event (bottom-left origin), as the host delivers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pointer moved to `position` (device pixels, bottom-left origin).
    MouseMove {
        /// The pointer position in device pixels.
        position: [i32; 2],
    },
    /// A mouse button changed state at `position` (`button` is 1-based, `down` is
    /// whether it was pressed).
    MouseButton {
        /// The pointer position in device pixels.
        position: [i32; 2],
        /// The button index (1 = primary).
        button: u8,
        /// `true` on press, `false` on release.
        down: bool,
    },
    /// The pointer left the viewport.
    MouseLeave,
}

impl Event {
    /// The pointer position, if the event carries one.
    pub fn position(&self) -> Option<[i32; 2]> {
        match *self {
            Event::MouseMove { position } | Event::MouseButton { position, .. } => Some(position),
            Event::MouseLeave => None,
        }
    }
}

/// Reacts to [`Event`]s, like a Coin `SoEventCallback`.
pub trait EventListener {
    /// Handle `event`, where `hit` is the [`Hit`] under the pointer, if any.
    ///
    /// Return `true` if `scene` was changed and must be recompiled.
    fn on_event(&mut self, event: &Event, hit: Option<&Hit>, scene: &mut Scene) -> bool;
}

/// The ids of the [`NodeKind::EventCallback`] nodes active in `scene`, in graph
/// order.
///
/// A `SoSwitch` contributes only the child(ren) its `whichChild` selects, so a
/// callback inside an inactive branch is not dispatched.
pub fn event_callbacks(scene: &Scene) -> Vec<u64> {
    let mut ids = Vec::new();
    collect(&scene.root.children, &mut ids);
    ids
}

fn collect(children: &[Node], ids: &mut Vec<u64>) {
    for child in children {
        match &child.kind {
            NodeKind::Switch { which } => match *which {
                SO_SWITCH_ALL => collect(&child.children, ids),
                n if n < 0 => {}
                n => {
                    if let Some(active) = child.children.get(n as usize) {
                        collect(std::slice::from_ref(active), ids);
                    }
                }
            },
            kind => {
                if let NodeKind::EventCallback(id) = kind {
                    ids.push(*id);
                }
                collect(&child.children, ids);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::PickId;

    /// A listener that recolours a hovered material node.
    struct Highlighter {
        hovered: Option<PickId>,
        target: PickId,
    }

    impl EventListener for Highlighter {
        fn on_event(&mut self, event: &Event, hit: Option<&Hit>, scene: &mut Scene) -> bool {
            let next = match event {
                Event::MouseMove { .. } => hit.map(|hit| hit.id).filter(|id| *id == self.target),
                Event::MouseButton { .. } => self.hovered,
                Event::MouseLeave => None,
            };
            if next == self.hovered {
                return false;
            }
            self.hovered = next;
            if let Some(child) = scene.root.children.first_mut() {
                if let NodeKind::Material(colors) = &mut child.kind {
                    *colors = if self.hovered.is_some() {
                        vec![[1.0, 0.0, 0.0, 1.0]]
                    } else {
                        vec![[1.0, 1.0, 1.0, 1.0]]
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
                    kind: NodeKind::Material(vec![[1.0, 1.0, 1.0, 1.0]]),
                    children: vec![],
                }],
            },
        }
    }

    fn hit(id: PickId) -> Hit {
        Hit {
            id,
            point: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            element: 0,
            topology: crate::geometry::Topology::Triangles,
            distance: 1.0,
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
        assert!(listener.on_event(&event, Some(&hit(PickId(7))), &mut scene));
        assert_eq!(material(&scene), [1.0, 0.0, 0.0, 1.0]);
        // A second move onto the same object is not a change.
        assert!(!listener.on_event(&event, Some(&hit(PickId(7))), &mut scene));
        // Leaving clears the highlight.
        assert!(listener.on_event(&Event::MouseLeave, None, &mut scene));
        assert_eq!(material(&scene), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn event_callbacks_skips_inactive_switch_branches() {
        let callback = |id| crate::node::Node {
            kind: NodeKind::EventCallback(id),
            children: vec![],
        };
        let switch = |which, children| crate::node::Node {
            kind: NodeKind::Switch { which },
            children,
        };
        let scene = Scene {
            root: crate::node::Node {
                kind: NodeKind::Separator,
                children: vec![
                    callback(1),
                    switch(1, vec![callback(2), callback(3)]),
                    switch(crate::node::SO_SWITCH_NONE, vec![callback(4)]),
                ],
            },
        };
        // Only the callback outside the switch and the active (index 1) child.
        assert_eq!(event_callbacks(&scene), vec![1, 3]);
    }

    fn material(scene: &Scene) -> [f32; 4] {
        match &scene.root.children[0].kind {
            NodeKind::Material(colors) => colors[0],
            _ => unreachable!(),
        }
    }
}

//! A sample Inventor scene and camera for the app shell's viewport.
//!
//! Until the geometry kernel and the workbench `ViewProvider`s produce real document
//! geometry, the shell's 3D pane draws this fixed cube, so the render pipeline and
//! the interaction path (hover → pick → highlight) are exercised end to end. It is
//! also what the `inventor_roundtrip` example renders.

use ferrocad_graphics::{Camera, Hit, Projection, Topology, Viewport};
use ferrocad_inventor::{Event, EventListener, Node, NodeKind, Scene};
use ferrocad_types::{Placement, Rotation, Vector3};

/// The cube's resting colour.
pub const BASE_COLOR: [f32; 4] = [0.9, 0.6, 0.2, 1.0];
/// The cube's colour while hovered.
pub const HOVER_COLOR: [f32; 4] = [0.2, 0.85, 1.0, 1.0];

/// A rotated cube as an Inventor scene: material, transform, coordinates, mesh.
pub fn sample_scene() -> Scene {
    let h = 1.0_f32;
    let corners = vec![
        [-h, -h, -h],
        [h, -h, -h],
        [h, h, -h],
        [-h, h, -h],
        [-h, -h, h],
        [h, -h, h],
        [h, h, h],
        [-h, h, h],
    ];
    // Twelve triangles (two per face); winding is irrelevant while culling is off.
    let indices = vec![
        4, 5, 6, 4, 6, 7, // +z
        1, 0, 3, 1, 3, 2, // -z
        0, 4, 7, 0, 7, 3, // -x
        5, 1, 2, 5, 2, 6, // +x
        7, 6, 2, 7, 2, 3, // +y
        0, 1, 5, 0, 5, 4, // -y
    ];
    let rotation = Rotation::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0), 0.6);

    Scene {
        root: Node {
            kind: NodeKind::Separator,
            children: vec![
                Node {
                    kind: NodeKind::Material(vec![BASE_COLOR]),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Transform(Placement::new(
                        Vector3::new(0.0, 0.0, 0.0),
                        rotation,
                    )),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Coordinate3(corners),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Mesh {
                        indices,
                        normal_indices: vec![],
                        topology: Topology::Triangles,
                    },
                    children: vec![],
                },
            ],
        },
    }
}

/// A perspective camera looking at the origin from `(0, 0, 4)`.
pub fn sample_camera(viewport: Viewport) -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Rotation::identity()).to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0_f64.to_radians(),
            near: 0.1,
            far: 100.0,
        },
        viewport,
    }
}

/// Highlight the object under the pointer by recolouring its material.
///
/// The `SoEventCallback` role: react to an event (with the pick result) by editing
/// the scene; the viewport recompiles when this returns `true`.
#[derive(Default)]
pub struct HoverHighlight {
    hovered: bool,
}

impl EventListener for HoverHighlight {
    fn on_event(&mut self, event: &Event, hit: Option<&Hit>, scene: &mut Scene) -> bool {
        let now_hovered = matches!(event, Event::MouseMove { .. }) && hit.is_some();
        if now_hovered == self.hovered {
            return false;
        }
        self.hovered = now_hovered;
        set_object_color(scene, if now_hovered { HOVER_COLOR } else { BASE_COLOR });
        true
    }
}

fn set_object_color(scene: &mut Scene, color: [f32; 4]) {
    for child in &mut scene.root.children {
        if let NodeKind::Material(colors) = &mut child.kind {
            *colors = vec![color];
        }
    }
}

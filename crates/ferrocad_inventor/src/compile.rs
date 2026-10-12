//! The mesh compiler: turn a [`Scene`] into a render queue.
//!
//! Compilation is the traversal Coin performs in its render action: walk the scene
//! accumulating inherited state (transform, colour, …) and emit a [`DrawPacket`] per
//! drawable, with that state frozen into a [`StateSnapshot`](crate::StateSnapshot)
//! and the geometry flattened to GPU-ready vertices.
//!
//! Fields follow Coin's scoping rules: a `Material`, `Coordinate3` or `Transform`
//! affects the traversal that *follows* it within its `Separator` (i.e. later
//! siblings), and a `Separator` isolates its subtree from the state around it.

use crate::batch::{DrawPacket, Generation, MeshId, RenderQueue};
use crate::geometry::{Geometry, Topology, Vertex};
use crate::node::{Node, NodeKind, Scene, SO_SWITCH_ALL};
use crate::pixels::Viewport;
use crate::state::State;

/// Compile `scene` into a [`RenderQueue`] at a device-pixel ratio of 1.
///
/// A convenience for callers that have no viewport (tests, headless tools); the
/// host uses [`compile_with`].
pub fn compile(scene: &Scene) -> RenderQueue {
    compile_with(scene, &Viewport::default())
}

/// Compile `scene` into a [`RenderQueue`] for `viewport`.
///
/// Each [`NodeKind::Mesh`] becomes one packet. Geometry is attached **on every
/// compile** (a fresh upload from the compiler's point of view); a renderer keys its
/// upload cache on `(mesh_id, generation)` and, because a stable scene compiles to
/// the same ids and generations each time, only uploads once.
///
/// `viewport`'s device-pixel ratio seeds [`State::device_pixel_ratio`], which
/// screen-space nodes read while building geometry. Because that scaling is baked in
/// at compile time, a change of `device_pixel_ratio` (e.g. moving the window to a
/// display with a different scale factor) requires recompiling the scene.
///
/// [`Scene::root`] is treated as a group: its children are traversed at the top level.
pub fn compile_with(scene: &Scene, viewport: &Viewport) -> RenderQueue {
    let mut state = State::default();
    state.device_pixel_ratio = viewport.device_pixel_ratio();

    let mut ctx = Compiler {
        next_mesh: 0,
        packets: Vec::new(),
    };
    let mut coordinates = Vec::new();
    walk_children(&scene.root.children, &state, &mut coordinates, &mut ctx);
    RenderQueue {
        packets: ctx.packets,
    }
}

struct Compiler {
    next_mesh: u64,
    packets: Vec<DrawPacket>,
}

/// Walk a list of siblings in order, threading `state` and `coordinates` forward
/// across them (so a field node affects the siblings that follow it).
fn walk_children(
    children: &[Node],
    state: &State,
    coordinates: &mut Vec<[f32; 3]>,
    ctx: &mut Compiler,
) {
    let mut state = state.clone();
    for child in children {
        match &child.kind {
            // A separator isolates fields (and the coordinate field) to its subtree.
            NodeKind::Separator => {
                let saved = std::mem::take(coordinates);
                walk_children(&child.children, &state, coordinates, ctx);
                *coordinates = saved;
            }
            NodeKind::Transform(placement) => {
                state.transform = state.transform.mul(&placement.to_matrix());
            }
            // A material list sets the draw colour to its first (diffuse) entry;
            // an empty list leaves the inherited colour in place.
            NodeKind::Material(colors) => {
                if let Some(color) = colors.first() {
                    state.color = *color;
                }
            }
            NodeKind::Coordinate3(points) => {
                *coordinates = points.clone();
            }
            // Normals round-trip through the model but are not consumed yet
            // (flat shading); Phase 3 will thread them into the mesh compiler.
            NodeKind::Normal(_) => {}
            // `SoSwitch` derives from `SoGroup`, so it does not isolate state: the
            // selected child(ren) inherit (and may leak) the current state.
            NodeKind::Switch { which } => match *which {
                SO_SWITCH_ALL => {
                    walk_children(&child.children, &state, coordinates, ctx);
                }
                n if n < 0 => {}
                n => {
                    if let Some(active) = child.children.get(n as usize) {
                        walk_children(std::slice::from_ref(active), &state, coordinates, ctx);
                    }
                }
            },
            NodeKind::Mesh {
                indices, topology, ..
            } => {
                if let Some(geometry) = build_geometry(coordinates, indices, *topology) {
                    let mesh_id = MeshId(ctx.next_mesh);
                    ctx.next_mesh += 1;
                    ctx.packets.push(DrawPacket {
                        mesh_id,
                        generation: Generation(0),
                        topology: *topology,
                        geometry: Some(geometry),
                        state: state.snapshot(),
                    });
                }
            }
            // Custom node classes carry no geometry the core understands yet.
            NodeKind::Custom(_) => {}
            // `SoEventCallback` is a leaf: it draws nothing.
            NodeKind::EventCallback(_) => {}
        }
    }
}

/// Flatten coordinates + indices into flat-shaded vertices (one normal per face).
fn build_geometry(
    coordinates: &[[f32; 3]],
    indices: &[u32],
    topology: Topology,
) -> Option<Geometry> {
    if indices.is_empty() {
        return None;
    }
    let mut vertices = Vec::new();
    let mut indices_out = Vec::new();

    let step = match topology {
        Topology::Triangles => 3,
        Topology::Lines => 2,
        Topology::Points => 1,
    };
    for face in indices.chunks_exact(step) {
        let normal = if step >= 2 {
            let a = coordinates.get(face[0] as usize)?;
            let b = coordinates.get(face[1] as usize)?;
            let c = if step == 3 {
                coordinates.get(face[2] as usize)?
            } else {
                b
            };
            face_normal(a, b, c)
        } else {
            [0.0, 0.0, 1.0]
        };
        for &index in face {
            let position = *coordinates.get(index as usize)?;
            indices_out.push(vertices.len() as u32);
            vertices.push(Vertex { position, normal });
        }
    }

    Some(Geometry {
        vertices,
        indices: indices_out,
    })
}

/// The unit normal of triangle `abc` (the cross product, normalised).
fn face_normal(a: &[f32; 3], b: &[f32; 3], c: &[f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > f32::EPSILON {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINTS: [[f32; 3]; 3] = [[0.0, 0.5, 0.0], [-0.5, -0.5, 0.0], [0.5, -0.5, 0.0]];

    fn mesh() -> Node {
        Node {
            kind: NodeKind::Mesh {
                indices: vec![0, 1, 2],
                normal_indices: vec![],
                topology: Topology::Triangles,
            },
            children: vec![],
        }
    }

    fn separator(children: Vec<Node>) -> Node {
        Node {
            kind: NodeKind::Separator,
            children,
        }
    }

    #[test]
    fn compiles_a_triangle_to_one_packet() {
        let scene = Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let queue = compile(&scene);
        assert_eq!(queue.len(), 1);
        let packet = &queue.packets[0];
        assert_eq!(packet.topology, Topology::Triangles);
        assert_eq!(packet.state.color, [1.0, 1.0, 1.0, 1.0]);
        let geometry = packet.geometry.as_ref().unwrap();
        assert_eq!(geometry.vertices.len(), 3);
        assert_eq!(geometry.indices, vec![0, 1, 2]);
        assert_eq!(geometry.vertices[0].normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn material_scopes_to_following_siblings_in_its_separator() {
        let scene = Scene {
            root: separator(vec![
                // A red material applies to the mesh that follows it...
                separator(vec![
                    Node {
                        kind: NodeKind::Material(vec![[1.0, 0.0, 0.0, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(POINTS.to_vec()),
                        children: vec![],
                    },
                    mesh(),
                ]),
                // ...but not across the separator boundary.
                separator(vec![
                    Node {
                        kind: NodeKind::Coordinate3(POINTS.to_vec()),
                        children: vec![],
                    },
                    mesh(),
                ]),
            ]),
        };
        let queue = compile(&scene);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.packets[0].state.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(queue.packets[1].state.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn material_list_binds_its_first_entry() {
        let scene = Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Material(vec![[0.1, 0.2, 0.3, 1.0], [1.0, 0.0, 0.0, 1.0]]),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let queue = compile(&scene);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.packets[0].state.color, [0.1, 0.2, 0.3, 1.0]);
    }

    /// A `SoSwitch` draws only the child(ren) its `whichChild` selects: an index
    /// picks one, `SO_SWITCH_NONE` draws nothing, `SO_SWITCH_ALL` draws all. It
    /// inherits the sibling coordinates (it is a group, not a separator).
    fn switch_scene(which: i32) -> Scene {
        Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Switch { which },
                    children: vec![mesh(), mesh()],
                },
            ]),
        }
    }

    #[test]
    fn switch_honours_which_child_sentinels() {
        assert_eq!(compile(&switch_scene(0)).len(), 1);
        assert_eq!(compile(&switch_scene(1)).len(), 1);
        assert_eq!(compile(&switch_scene(crate::node::SO_SWITCH_NONE)).len(), 0);
        assert_eq!(compile(&switch_scene(crate::node::SO_SWITCH_ALL)).len(), 2);
    }
}

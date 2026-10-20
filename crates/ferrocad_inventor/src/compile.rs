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

use crate::batch::{DrawPacket, Generation, MeshId, PickId, RenderQueue};
use crate::directive::{Directive, Directives};
use crate::geometry::{Geometry, Topology, Vertex};
use crate::node::{Node, NodeKind, Scene, SO_SWITCH_ALL};
use crate::pixels::Viewport;
use crate::state::State;

/// Compile `scene` into a `(RenderQueue, Directives)` pair at a device-pixel ratio of 1.
///
/// A convenience for callers that have no viewport (tests, headless tools); the
/// host uses [`compile_with`].
pub fn compile(scene: &Scene) -> (RenderQueue, Directives) {
    compile_with(scene, &Viewport::default())
}

/// Compile `scene` into a `(RenderQueue, Directives)` pair for `viewport`.
///
/// Each [`NodeKind::Mesh`] becomes one packet. Geometry is attached **on every
/// compile** (a fresh upload from the compiler's point of view); a renderer keys its
/// upload cache on `(mesh_id, generation)`. `mesh_id` is allocated in traversal order,
/// which can change when a producer reorders or omits packets (the navigation cube
/// sorts back-to-front, and hides some labels), so the generation is a **fingerprint
/// of the geometry bytes**: a reused id whose geometry changed no longer matches the
/// cached upload, while identical geometry still hits.
///
/// Semantic nodes (a [`NodeKind::NaviCube`]) emit **no** packet: they are lifted into
/// the [`Directives`] the viewport consumes (see `docs/directives.md`).
///
/// `viewport`'s device-pixel ratio seeds [`State::device_pixel_ratio`], which
/// screen-space nodes read while building geometry. Because that scaling is baked in
/// at compile time, a change of `device_pixel_ratio` (e.g. moving the window to a
/// display with a different scale factor) requires recompiling the scene.
///
/// [`Scene::root`] is treated as a group: its children are traversed at the top level.
pub fn compile_with(scene: &Scene, viewport: &Viewport) -> (RenderQueue, Directives) {
    let mut state = State::default();
    state.device_pixel_ratio = viewport.device_pixel_ratio();

    let mut ctx = Compiler {
        next_mesh: 0,
        packets: Vec::new(),
        directives: Directives::new(),
    };
    let mut coordinates = Vec::new();
    let mut tex_coords = Vec::new();
    walk_children(
        &scene.root.children,
        &state,
        &mut coordinates,
        &mut tex_coords,
        &mut ctx,
    );
    (
        RenderQueue {
            packets: ctx.packets,
        },
        ctx.directives,
    )
}

struct Compiler {
    next_mesh: u64,
    packets: Vec<DrawPacket>,
    directives: Directives,
}

/// Walk a list of siblings in order, threading `state` and the coordinate/texture
/// buffers forward across them (so a field node affects the siblings that follow it).
fn walk_children(
    children: &[Node],
    state: &State,
    coordinates: &mut Vec<[f32; 3]>,
    tex_coords: &mut Vec<[f32; 2]>,
    ctx: &mut Compiler,
) {
    let mut state = state.clone();
    for child in children {
        match &child.kind {
            // A separator isolates fields (and the coordinate fields) to its subtree.
            NodeKind::Separator => {
                let saved_coords = std::mem::take(coordinates);
                let saved_uv = std::mem::take(tex_coords);
                walk_children(&child.children, &state, coordinates, tex_coords, ctx);
                *coordinates = saved_coords;
                *tex_coords = saved_uv;
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
            NodeKind::TextureCoordinate2(uv) => {
                *tex_coords = uv.clone();
            }
            NodeKind::Texture2(texture) => {
                state.texture = Some(texture.clone());
            }
            // A selection id applies to the shapes that follow it.
            NodeKind::PickId(id) => {
                state.id = PickId(*id);
            }
            // Normals round-trip through the model but are not consumed yet
            // (flat shading); Phase 3 will thread them into the mesh compiler.
            NodeKind::Normal(_) => {}
            // `SoSwitch` derives from `SoGroup`, so it does not isolate state: the
            // selected child(ren) inherit (and may leak) the current state.
            NodeKind::Switch { which } => match *which {
                SO_SWITCH_ALL => {
                    walk_children(&child.children, &state, coordinates, tex_coords, ctx);
                }
                n if n < 0 => {}
                n => {
                    if let Some(active) = child.children.get(n as usize) {
                        walk_children(
                            std::slice::from_ref(active),
                            &state,
                            coordinates,
                            tex_coords,
                            ctx,
                        );
                    }
                }
            },
            NodeKind::Mesh {
                indices, topology, ..
            } => {
                if let Some(geometry) = build_geometry(coordinates, tex_coords, indices, *topology) {
                    let mesh_id = MeshId(ctx.next_mesh);
                    ctx.next_mesh += 1;
                    let generation = Generation(fingerprint(&geometry));
                    ctx.packets.push(DrawPacket {
                        mesh_id,
                        generation,
                        topology: *topology,
                        geometry: Some(geometry),
                        state: state.snapshot(),
                    });
                }
            }
            // A navigation-cube node draws nothing: it becomes a host-facing directive.
            NodeKind::NaviCube(directive) => {
                ctx.directives.push(Directive::NaviCube(*directive));
            }
            // Custom node classes carry no geometry the core understands yet.
            NodeKind::Custom(_) => {}
            // `SoEventCallback` draws nothing; it becomes a host-facing directive so the
            // viewport can route input to the callables keyed by its id (same traversal,
            // so `SoSwitch` state is honoured without a rescan).
            NodeKind::EventCallback(id) => {
                ctx.directives.push(Directive::EventCallback(*id));
            }
        }
    }
}

/// A content fingerprint of `geometry`, used as its [`Generation`].
///
/// The renderer's upload cache is keyed on `(mesh_id, generation)` and treats an equal
/// pair as "the buffers are still valid". Mesh ids are positional, so a producer that
/// changes packet order or count (a depth-sorted overlay, a hidden label) would
/// otherwise hand a new mesh an old upload. Hashing the bytes makes the pair honest
/// without comparing the buffers on the render thread.
fn fingerprint(geometry: &Geometry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for vertex in &geometry.vertices {
        for value in vertex
            .position
            .iter()
            .chain(&vertex.normal)
            .chain(&vertex.uv)
        {
            value.to_bits().hash(&mut hasher);
        }
        vertex.part_idx.hash(&mut hasher);
    }
    geometry.indices.hash(&mut hasher);
    hasher.finish()
}

/// Flatten coordinates + indices into flat-shaded vertices (one normal per face).
fn build_geometry(
    coordinates: &[[f32; 3]],
    tex_coords: &[[f32; 2]],
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
            let uv = tex_coords
                .get(index as usize)
                .copied()
                .unwrap_or([0.0, 0.0]);
            indices_out.push(vertices.len() as u32);
            vertices.push(Vertex {
                position,
                normal,
                uv,
                part_idx: 0,
                color: [1.0, 1.0, 1.0, 1.0],
            });
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
        let (queue, _) = compile(&scene);
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
    fn the_generation_fingerprints_the_geometry() {
        let scene = |points: Vec<[f32; 3]>| Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(points),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let (first, _) = compile(&scene(POINTS.to_vec()));
        let (again, _) = compile(&scene(POINTS.to_vec()));
        assert_eq!(
            first.packets[0].generation, again.packets[0].generation,
            "identical geometry reuses the upload"
        );

        // Mesh ids are positional, so a producer that reorders or hides packets can
        // reuse an id for different geometry; the generation must change with the bytes.
        let mut moved = POINTS;
        moved[0][1] = 0.25;
        let (changed, _) = compile(&scene(moved.to_vec()));
        assert_eq!(first.packets[0].mesh_id, changed.packets[0].mesh_id);
        assert_ne!(
            first.packets[0].generation, changed.packets[0].generation,
            "different geometry must miss the upload cache"
        );
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
        let (queue, _) = compile(&scene);
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
        let (queue, _) = compile(&scene);
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
        assert_eq!(compile(&switch_scene(0)).0.len(), 1);
        assert_eq!(compile(&switch_scene(1)).0.len(), 1);
        assert_eq!(compile(&switch_scene(crate::node::SO_SWITCH_NONE)).0.len(), 0);
        assert_eq!(compile(&switch_scene(crate::node::SO_SWITCH_ALL)).0.len(), 2);
    }

    #[test]
    fn a_navicube_node_becomes_a_directive_not_a_packet() {
        use crate::{NaviCubeDirective, Rect};
        let directive = NaviCubeDirective {
            rect: Rect::new(10, 10, 50, 50),
            orientation: None,
            hilite: None,
            orthographic: true,
        };
        let scene = Scene {
            root: separator(vec![Node {
                kind: NodeKind::NaviCube(directive),
                children: vec![],
            }]),
        };
        let (queue, directives) = compile(&scene);
        assert!(queue.is_empty(), "a navicube node draws nothing");
        assert_eq!(
            directives.navicubes().copied().collect::<Vec<_>>(),
            vec![directive]
        );
    }

    #[test]
    fn event_callback_nodes_become_directives_and_honour_switch_state() {
        let callback = |id| Node {
            kind: NodeKind::EventCallback(id),
            children: vec![],
        };
        let switch = |which, children| Node {
            kind: NodeKind::Switch { which },
            children,
        };
        let scene = Scene {
            root: separator(vec![
                callback(1),
                switch(1, vec![callback(2), callback(3)]),
                switch(crate::node::SO_SWITCH_NONE, vec![callback(4)]),
            ]),
        };
        let (queue, directives) = compile(&scene);
        assert!(queue.is_empty(), "event callbacks draw nothing");
        // Only the callback outside the switch and the active (index 1) child.
        assert_eq!(directives.event_callbacks().collect::<Vec<_>>(), vec![1, 3]);
    }
}

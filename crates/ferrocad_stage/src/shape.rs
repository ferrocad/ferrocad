//! Declarative CAD visual items and their compilation to draw batches.

use std::sync::atomic::{AtomicU64, Ordering};

use ferrocad_graphics::{
    DrawPacket, Generation, Geometry, MeshId, PickId, RenderQueue, StateSnapshot, Topology, Vertex,
};
use ferrocad_types::{Matrix4, Placement};

/// How a [`ShapeItem`] is drawn.
///
/// The three CAD display modes decompose into the existing [`Topology`] packets: a
/// surface is a [`Topology::Triangles`] packet and the edges a [`Topology::Lines`] one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawMode {
    /// Shaded surfaces only.
    #[default]
    Shaded,
    /// Wireframe edges only.
    Wireframe,
    /// Shaded surfaces with their edges (the CAD default).
    ShadedWithEdges,
}

impl DrawMode {
    /// Whether this mode draws the shaded surface.
    fn has_surface(self) -> bool {
        matches!(self, DrawMode::Shaded | DrawMode::ShadedWithEdges)
    }

    /// Whether this mode draws the wireframe edges.
    fn has_edges(self) -> bool {
        matches!(self, DrawMode::Wireframe | DrawMode::ShadedWithEdges)
    }
}

/// A declarative, already-tessellated CAD shape.
///
/// The item owns raw buffers — vertex positions, optional per-vertex normals, triangle
/// indices and edge segment indices — plus display attributes. It carries no document
/// object and no scene-graph node: [`emit`](Self::emit) turns it into
/// [`DrawPacket`](ferrocad_graphics::DrawPacket)s.
///
/// The mesh identity is allocated once at construction and is **stable** across
/// `emit` calls, so a renderer's `(mesh_id, generation)` upload cache hits; changing the
/// geometry with [`set_shape`](Self::set_shape) bumps the generation to force a
/// re-upload.
#[derive(Clone, Debug)]
pub struct ShapeItem {
    entity_id: u32,
    placement: Placement,
    /// An optional preview transform composed after [`placement`](Self::placement)
    /// (`world = placement * temporary`). Used by interactive gizmos to preview a drag
    /// without touching the document; `None` in the resting state.
    temporary_transform: Option<Matrix4>,
    draw_mode: DrawMode,
    surface_color: [f32; 4],
    edge_color: [f32; 4],
    edge_width: f32,
    selectable: bool,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    triangles: Vec<u32>,
    edges: Vec<u32>,
    /// The sub-element id of each position ([`ferrocad_graphics::Vertex::part_idx`]);
    /// empty means every vertex is part `0`.
    parts: Vec<u32>,
    surface_mesh: MeshId,
    wire_mesh: MeshId,
    generation: Generation,
}

impl ShapeItem {
    /// A new, empty shape carrying `entity_id` (the picking identity).
    pub fn new(entity_id: u32) -> Self {
        Self {
            entity_id,
            placement: Placement::identity(),
            temporary_transform: None,
            draw_mode: DrawMode::default(),
            surface_color: [0.8, 0.8, 0.8, 1.0],
            edge_color: [0.1, 0.1, 0.1, 1.0],
            edge_width: 1.0,
            selectable: true,
            positions: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            edges: Vec::new(),
            parts: Vec::new(),
            surface_mesh: next_mesh_id(),
            wire_mesh: next_mesh_id(),
            generation: Generation(0),
        }
    }

    /// Replace the tessellation (the counterpart of the `set_shape(...)` call a
    /// `ViewProvider` makes from an OCCT tessellation).
    ///
    /// * `positions` — `N` vertex positions.
    /// * `normals` — either `N` per-vertex normals (indices then share vertices) or
    ///   empty (the surface is flat-shaded per triangle).
    /// * `triangles` — flat triangle indices, three per triangle.
    /// * `edges` — flat edge segment indices, two per segment.
    pub fn set_shape(
        &mut self,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        triangles: Vec<u32>,
        edges: Vec<u32>,
    ) {
        self.set_mesh(positions, normals, triangles, edges, Vec::new());
    }

    /// Replace the tessellation **and** tag each position with its sub-element id.
    ///
    /// `parts` is indexed by position, matching what a
    /// [`ferrocad_geom::Tessellation`] carries; a shorter (or empty) list leaves the
    /// remaining vertices at part `0`.
    pub fn set_mesh(
        &mut self,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        triangles: Vec<u32>,
        edges: Vec<u32>,
        parts: Vec<u32>,
    ) {
        self.positions = positions;
        self.normals = normals;
        self.triangles = triangles;
        self.edges = edges;
        self.parts = parts;
        // Any geometry change invalidates a renderer's cached upload for this mesh.
        self.generation = Generation(self.generation.0 + 1);
    }

    /// The builder form of [`set_mesh`](Self::set_mesh).
    pub fn with_mesh(
        mut self,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        triangles: Vec<u32>,
        edges: Vec<u32>,
        parts: Vec<u32>,
    ) -> Self {
        self.set_mesh(positions, normals, triangles, edges, parts);
        self
    }

    /// The per-position sub-element ids.
    pub fn parts(&self) -> &[u32] {
        &self.parts
    }

    /// The builder form of [`set_shape`](Self::set_shape).
    pub fn with_shape(
        mut self,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        triangles: Vec<u32>,
        edges: Vec<u32>,
    ) -> Self {
        self.set_shape(positions, normals, triangles, edges);
        self
    }

    /// Set the display mode (`Shaded` / `Wireframe` / `ShadedWithEdges`).
    pub fn with_draw_mode(mut self, mode: DrawMode) -> Self {
        self.draw_mode = mode;
        self
    }

    /// Set the surface colour.
    pub fn with_surface_color(mut self, color: [f32; 4]) -> Self {
        self.surface_color = color;
        self
    }

    /// Set the edge colour.
    pub fn with_edge_color(mut self, color: [f32; 4]) -> Self {
        self.edge_color = color;
        self
    }

    /// Set the edge width. Stored for fidelity; the core wgpu line pipelines draw
    /// one-pixel lines, so this is not consumed yet.
    pub fn with_edge_width(mut self, width: f32) -> Self {
        self.edge_width = width;
        self
    }

    /// Set whether the shape participates in picking.
    pub fn with_selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Set the item's placement (its world transform).
    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// The temporary preview transform (see [`temporary_transform`](Self::temporary_transform)).
    pub fn with_temporary_transform(mut self, transform: Option<Matrix4>) -> Self {
        self.temporary_transform = transform;
        self
    }

    /// Set or clear the temporary preview transform composed after the placement. A
    /// gizmo uses this for a live drag preview; it does not change the mesh, so no
    /// re-upload is triggered.
    pub fn set_temporary_transform(&mut self, transform: Option<Matrix4>) {
        self.temporary_transform = transform;
    }

    /// Set the surface colour (a state change; no re-upload).
    pub fn set_surface_color(&mut self, color: [f32; 4]) {
        self.surface_color = color;
    }

    /// Set the edge colour (a state change; no re-upload).
    pub fn set_edge_color(&mut self, color: [f32; 4]) {
        self.edge_color = color;
    }

    /// The picking identity.
    pub fn entity_id(&self) -> u32 {
        self.entity_id
    }

    /// The item's placement.
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// The temporary preview transform, if any.
    pub fn temporary_transform(&self) -> Option<Matrix4> {
        self.temporary_transform
    }

    /// The display mode.
    pub fn draw_mode(&self) -> DrawMode {
        self.draw_mode
    }

    /// The surface colour.
    pub fn surface_color(&self) -> [f32; 4] {
        self.surface_color
    }

    /// The edge colour.
    pub fn edge_color(&self) -> [f32; 4] {
        self.edge_color
    }

    /// The edge width (stored, not yet consumed).
    pub fn edge_width(&self) -> f32 {
        self.edge_width
    }

    /// Whether the shape is pickable.
    pub fn selectable(&self) -> bool {
        self.selectable
    }

    /// The current geometry generation.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The vertex positions.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// The per-vertex normals (empty when the surface is flat-shaded per triangle).
    pub fn normals(&self) -> &[[f32; 3]] {
        &self.normals
    }

    /// The flat triangle indices, three per triangle.
    pub fn triangles(&self) -> &[u32] {
        &self.triangles
    }

    /// The flat edge segment indices, two per segment.
    pub fn edges(&self) -> &[u32] {
        &self.edges
    }

    /// Emit the item's draw packets (surface and/or wireframe) into `queue`.
    pub fn emit(&self, queue: &mut RenderQueue) {
        // The world transform: the placement, with the temporary preview composed after it.
        let mut world = self.placement.to_matrix();
        if let Some(temporary) = &self.temporary_transform {
            world = world.mul(temporary);
        }
        if self.draw_mode.has_surface() {
            if let Some(geometry) = self.surface_geometry() {
                queue.packets.push(self.packet(
                    self.surface_mesh,
                    Topology::Triangles,
                    geometry,
                    world,
                    self.surface_color,
                ));
            }
        }
        if self.draw_mode.has_edges() {
            if let Some(geometry) = self.wire_geometry() {
                queue.packets.push(self.packet(
                    self.wire_mesh,
                    Topology::Lines,
                    geometry,
                    world,
                    self.edge_color,
                ));
            }
        }
    }

    /// The picking id: the entity id when selectable, else the default (0).
    fn pick_id(&self) -> PickId {
        if self.selectable {
            PickId(self.entity_id)
        } else {
            PickId::default()
        }
    }

    fn packet(
        &self,
        mesh_id: MeshId,
        topology: Topology,
        geometry: Geometry,
        world_transform: ferrocad_types::Matrix4,
        color: [f32; 4],
    ) -> DrawPacket {
        DrawPacket {
            mesh_id,
            generation: self.generation,
            topology,
            geometry: Some(geometry),
            state: StateSnapshot {
                world_transform,
                color,
                depth_bias: 0.0,
                texture: None,
                id: self.pick_id(),
            },
        }
    }

    /// The shaded surface: indexed when per-vertex normals are supplied, flat-shaded
    /// (one unrolled triangle) otherwise.
    fn surface_geometry(&self) -> Option<Geometry> {
        if self.triangles.len() < 3 {
            return None;
        }
        if !self.normals.is_empty() && self.normals.len() == self.positions.len() {
            let vertices = self
                .positions
                .iter()
                .enumerate()
                .zip(&self.normals)
                .map(|((index, position), normal)| Vertex {
                    position: *position,
                    normal: *normal,
                    uv: [0.0, 0.0],
                    part_idx: self.parts.get(index).copied().unwrap_or(0),
                })
                .collect();
            Some(Geometry {
                vertices,
                indices: self.triangles.clone(),
            })
        } else {
            Some(flat_surface(&self.positions, &self.triangles, &self.parts))
        }
    }

    /// The wireframe: the position buffer indexed by the edge segments.
    fn wire_geometry(&self) -> Option<Geometry> {
        if self.edges.len() < 2 {
            return None;
        }
        let vertices = self
            .positions
            .iter()
            .enumerate()
            .map(|(index, position)| Vertex {
                position: *position,
                normal: [0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                part_idx: self.parts.get(index).copied().unwrap_or(0),
            })
            .collect();
        Some(Geometry {
            vertices,
            indices: self.edges.clone(),
        })
    }
}

/// Flat-shade `triangles`: one unrolled vertex per index, each carrying its triangle's
/// face normal. Malformed triangles (an out-of-range index) are skipped.
fn flat_surface(positions: &[[f32; 3]], triangles: &[u32], parts: &[u32]) -> Geometry {
    let mut vertices = Vec::with_capacity(triangles.len());
    let mut indices = Vec::with_capacity(triangles.len());
    for tri in triangles.chunks_exact(3) {
        let (Some(a), Some(b), Some(c)) = (
            positions.get(tri[0] as usize).copied(),
            positions.get(tri[1] as usize).copied(),
            positions.get(tri[2] as usize).copied(),
        ) else {
            continue;
        };
        let normal = face_normal(a, b, c);
        for &index in tri {
            let Some(position) = positions.get(index as usize) else {
                continue;
            };
            indices.push(vertices.len() as u32);
            vertices.push(Vertex {
                position: *position,
                normal,
                uv: [0.0, 0.0],
                part_idx: parts.get(index as usize).copied().unwrap_or(0),
            });
        }
    }
    Geometry { vertices, indices }
}

/// The unit normal of triangle `abc` (the cross product, normalised).
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
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

/// A fresh mesh identity for the stage.
///
/// The ids start at a high base so they never collide with the inventor compiler's
/// per-frame `0..n` mesh ids — both stacks share the renderer's `(MeshId, Generation)`
/// upload cache, and a collision would bind the wrong upload.
fn next_mesh_id() -> MeshId {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 40);
    MeshId(NEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A triangle with its three edges, no per-vertex normals.
    fn triangle_item() -> ShapeItem {
        ShapeItem::new(7).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![0, 1, 1, 2, 2, 0],
        )
    }

    fn emit(item: &ShapeItem) -> RenderQueue {
        let mut queue = RenderQueue::default();
        item.emit(&mut queue);
        queue
    }

    #[test]
    fn shaded_emits_one_triangle_packet() {
        let item = triangle_item().with_draw_mode(DrawMode::Shaded);
        let queue = emit(&item);
        assert_eq!(queue.len(), 1);
        let packet = &queue.packets[0];
        assert_eq!(packet.topology, Topology::Triangles);
        assert_eq!(packet.state.color, item.surface_color());
        assert_eq!(packet.state.id, PickId(7));
        assert!(packet.geometry.is_some(), "a fresh compile is a cache miss");
        // Flat-shaded: one unrolled vertex per index.
        assert_eq!(packet.geometry.as_ref().unwrap().vertices.len(), 3);
        assert_eq!(packet.geometry.as_ref().unwrap().indices, vec![0, 1, 2]);
    }

    #[test]
    fn shaded_with_edges_emits_surface_then_wire() {
        let item = triangle_item().with_draw_mode(DrawMode::ShadedWithEdges);
        let queue = emit(&item);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.packets[0].topology, Topology::Triangles);
        assert_eq!(queue.packets[0].state.color, item.surface_color());
        assert_eq!(queue.packets[1].topology, Topology::Lines);
        assert_eq!(queue.packets[1].state.color, item.edge_color());
    }

    #[test]
    fn wireframe_emits_only_lines() {
        let item = triangle_item().with_draw_mode(DrawMode::Wireframe);
        let queue = emit(&item);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.packets[0].topology, Topology::Lines);
        assert_eq!(queue.packets[0].state.color, item.edge_color());
    }

    #[test]
    fn part_ids_reach_the_surface_and_wire_vertices() {
        let parts = vec![4, 4, 1, 1];
        let item = ShapeItem::new(3)
            .with_mesh(
                vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 1.0, 0.0],
                ],
                Vec::new(),
                vec![0, 1, 2, 1, 3, 2],
                vec![0, 1, 3, 2],
                parts.clone(),
            )
            .with_draw_mode(DrawMode::ShadedWithEdges);
        assert_eq!(item.parts(), parts.as_slice());

        let queue = emit(&item);
        let surface = queue
            .packets
            .iter()
            .find(|packet| packet.topology == Topology::Triangles)
            .expect("a surface packet");
        let geometry = surface.geometry.as_ref().expect("fresh geometry");
        // Flat-shaded: each unrolled vertex carries its source position's part.
        let ids: Vec<u32> = geometry.vertices.iter().map(|v| v.part_idx).collect();
        assert_eq!(ids, vec![4, 4, 1, 4, 1, 1]);

        let wire = queue
            .packets
            .iter()
            .find(|packet| packet.topology == Topology::Lines)
            .expect("a wire packet");
        let geometry = wire.geometry.as_ref().expect("fresh geometry");
        assert_eq!(geometry.vertices[0].part_idx, 4);
        assert_eq!(geometry.vertices[2].part_idx, 1);
    }

    #[test]
    fn supplied_normals_are_indexed() {
        let normals = vec![[0.0, 0.0, 1.0]; 3];
        let item = ShapeItem::new(1).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals.clone(),
            vec![0, 1, 2],
            vec![],
        );
        let queue = emit(&item);
        let geometry = queue.packets[0].geometry.as_ref().unwrap();
        // Indexed: one vertex per position, not unrolled.
        assert_eq!(geometry.vertices.len(), 3);
        assert_eq!(geometry.vertices[1].normal, normals[1]);
    }

    #[test]
    fn mesh_ids_are_unique_stable_and_high() {
        let item = triangle_item().with_draw_mode(DrawMode::ShadedWithEdges);
        let first = emit(&item);
        let second = emit(&item);
        // Stable across compiles, and distinct for the surface and the wire.
        assert_eq!(first.packets[0].mesh_id, second.packets[0].mesh_id);
        assert_eq!(first.packets[1].mesh_id, second.packets[1].mesh_id);
        assert_ne!(first.packets[0].mesh_id, first.packets[1].mesh_id);
        // Distinct from another item, and clear of the inventor compiler's low ids.
        let other = emit(&triangle_item());
        assert_ne!(first.packets[0].mesh_id, other.packets[0].mesh_id);
        assert!(first.packets[0].mesh_id.0 >= 1 << 40);
    }

    #[test]
    fn set_shape_bumps_the_generation() {
        let mut item = triangle_item();
        assert_eq!(item.generation(), Generation(1));
        item.set_shape(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![], vec![0, 1, 2], vec![]);
        assert_eq!(item.generation(), Generation(2));
    }

    #[test]
    fn a_non_selectable_item_has_the_default_pick_id() {
        let item = triangle_item().with_selectable(false);
        assert_eq!(emit(&item).packets[0].state.id, PickId::default());
    }

    #[test]
    fn empty_geometry_emits_nothing() {
        let item = ShapeItem::new(1).with_draw_mode(DrawMode::ShadedWithEdges);
        assert!(emit(&item).is_empty());
    }

    #[test]
    fn the_placement_becomes_the_world_transform() {
        use ferrocad_types::{Rotation, Vector3};
        let placement = Placement::new(Vector3::new(1.0, 2.0, 3.0), Rotation::identity());
        let item = triangle_item().with_placement(placement);
        assert_eq!(emit(&item).packets[0].state.world_transform, placement.to_matrix());
    }
}

//! Declarative CAD visual items and their compilation to draw batches.

use std::sync::atomic::{AtomicU64, Ordering};

use ferrocad_geom::Tessellation;
use ferrocad_graphics::{
    DepthCompare, DepthState, DrawPacket, Generation, Geometry, MeshId, PickId, RenderQueue,
    StateSnapshot, Topology, Vertex, theme,
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

/// The selection highlight wire colour (linear `#38BDF8`, the manipulators' active
/// cyan). It stays opaque so the selected topology reads crisply over the tint.
pub const HIGHLIGHT_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 1.0];

/// The selection highlight face tint: the same linear `#38BDF8` at 12% alpha, so the
/// natural surface and its base edges stay visible through it.
///
/// The scene triangle pipeline is normally `BlendState::REPLACE`. The highlight surface
/// therefore uses a non-default depth state ([`HIGHLIGHT_DEPTH`]), which gives it a
/// blended pipeline variant in the wgpu renderer: alpha blends, but the depth test and
/// the bias still keep it on top of the coplanar surface it marks.
pub const HIGHLIGHT_TINT: [f32; 4] = [0.040, 0.509, 0.939, 0.12];

/// How far towards the camera the highlight is pushed in clip space, so it wins the
/// depth test against the coplanar surface it marks.
pub const HIGHLIGHT_DEPTH_BIAS: f32 = 1e-5;

/// The highlight surface's depth state: test against the scene, but write no depth.
///
/// The non-default state is deliberate. It selects the renderer's blended triangle
/// variant (so the 12% tint actually blends), and no write means the tint cannot hide
/// the base wire or the highlight wire drawn after it.
pub const HIGHLIGHT_DEPTH: DepthState = DepthState {
    test: true,
    write: false,
    compare: DepthCompare::Less,
};

/// The ghost-edge depth state: pass only where opaque geometry occludes the edge.
///
/// A ghost pass writes no depth and alpha-blends, so the hidden portions of an edge
/// read at the occluded-edge token instead of disappearing entirely.
pub const GHOST_DEPTH: DepthState = DepthState {
    test: true,
    write: false,
    compare: DepthCompare::Greater,
};

/// How far towards the camera an edge is pushed in clip space.
///
/// A wireframe lies exactly on the surface it bounds, so without a bias it z-fights with
/// it (and loses the picking depth test to it). A hair in front makes the drawn edge
/// stable and lets an exact click on it select the edge rather than the face.
pub const WIRE_DEPTH_BIAS: f32 = 1e-5;

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
    /// The side/down-facing colour for a two-tone surface (alpha `0` = off).
    shade_color: Option<[f32; 4]>,
    edge_color: [f32; 4],
    /// Emit a second, depth-`Greater` wire pass for the occluded edges. Off by
    /// default so gizmos, the navicube and transient tools keep their overlay look;
    /// a document entity opts in (see `ferrocad_gpui::display`).
    ghost_edges: bool,
    edge_width: f32,
    selectable: bool,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    triangles: Vec<u32>,
    edges: Vec<u32>,
    /// The sub-element id of each position ([`ferrocad_graphics::Vertex::part_idx`]);
    /// empty means every vertex is part `0`.
    parts: Vec<u32>,
    /// Part id → sub-element name (`Face6`), copied from [`Tessellation::parts`].
    part_names: Vec<String>,
    /// The position indices of the topological vertices.
    vertices: Vec<u32>,
    /// Whether to emit [`Topology::Points`] for [`vertices`](Self::vertices).
    show_vertices: bool,
    /// The part ids to highlight (sorted and deduplicated).
    highlight: Vec<u32>,
    surface_mesh: MeshId,
    wire_mesh: MeshId,
    points_mesh: MeshId,
    highlight_surface_mesh: MeshId,
    highlight_wire_mesh: MeshId,
    generation: Generation,
    /// Bumped when the highlight changes, so its sub-meshes re-upload without
    /// invalidating the base mesh.
    highlight_generation: Generation,
}

impl ShapeItem {
    /// A new, empty shape carrying `entity_id` (the picking identity).
    pub fn new(entity_id: u32) -> Self {
        Self {
            entity_id,
            placement: Placement::identity(),
            temporary_transform: None,
            draw_mode: DrawMode::default(),
            surface_color: theme::GLASS_SHADE,
            shade_color: None,
            edge_color: theme::EDGE_FRONT,
            ghost_edges: false,
            edge_width: 1.0,
            selectable: true,
            positions: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            edges: Vec::new(),
            parts: Vec::new(),
            part_names: Vec::new(),
            vertices: Vec::new(),
            show_vertices: false,
            highlight: Vec::new(),
            surface_mesh: next_mesh_id(),
            wire_mesh: next_mesh_id(),
            points_mesh: next_mesh_id(),
            highlight_surface_mesh: next_mesh_id(),
            highlight_wire_mesh: next_mesh_id(),
            generation: Generation(0),
            highlight_generation: Generation(0),
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
        self.part_names.clear();
        self.vertices.clear();
        // Any geometry change invalidates a renderer's cached upload for this mesh.
        self.generation = Generation(self.generation.0 + 1);
        // The highlight is derived from the geometry, so its cache is stale too.
        self.highlight_generation = Generation(self.highlight_generation.0 + 1);
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

    /// Replace the tessellation from a kernel's [`Tessellation`].
    ///
    /// The per-primitive part ids are flattened into per-position ids (the OCCT
    /// tessellation duplicates vertices per face and per edge, so a position belongs
    /// to exactly one part), and the part **names** are kept so a hit's `part` can be
    /// resolved to `Face6`/`Edge3` without the original mesh.
    pub fn set_tessellation(&mut self, mesh: &Tessellation) {
        let normals = if mesh.normals.len() == mesh.positions.len() {
            mesh.normals.clone()
        } else {
            Vec::new()
        };
        let parts = parts_from_tessellation(mesh);
        self.set_mesh(
            mesh.positions.clone(),
            normals,
            mesh.triangles.clone(),
            mesh.edges.clone(),
            parts,
        );
        self.part_names = mesh.parts.clone();
        self.vertices = mesh.vertices.clone();
    }

    /// The builder form of [`set_tessellation`](Self::set_tessellation).
    pub fn with_tessellation(mut self, mesh: &Tessellation) -> Self {
        self.set_tessellation(mesh);
        self
    }

    /// A new item from a kernel tessellation.
    pub fn from_tessellation(entity_id: u32, mesh: &Tessellation) -> Self {
        Self::new(entity_id).with_tessellation(mesh)
    }

    /// The part id → element name table (`Face6`, `Edge3`).
    pub fn part_names(&self) -> &[String] {
        &self.part_names
    }

    /// The element name for part `id`.
    pub fn part_name(&self, id: u32) -> Option<&str> {
        self.part_names.get(id as usize).map(String::as_str)
    }

    /// The position indices of the topological vertices.
    pub fn vertices(&self) -> &[u32] {
        &self.vertices
    }

    /// Emit the vertices as points (off by default).
    pub fn with_vertices(mut self, show: bool) -> Self {
        self.show_vertices = show;
        self
    }

    /// The part ids currently highlighted.
    pub fn highlight(&self) -> &[u32] {
        &self.highlight
    }

    /// Highlight the given parts (an empty iterator clears it).
    pub fn set_highlight(&mut self, parts: impl IntoIterator<Item = u32>) {
        let mut next: Vec<u32> = parts.into_iter().collect();
        next.sort_unstable();
        next.dedup();
        if next == self.highlight {
            return;
        }
        self.highlight = next;
        self.highlight_generation = Generation(self.highlight_generation.0 + 1);
    }

    /// The builder form of [`set_highlight`](Self::set_highlight).
    pub fn with_highlight(mut self, parts: impl IntoIterator<Item = u32>) -> Self {
        self.set_highlight(parts);
        self
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

    /// Set the side/down-facing colour for a two-tone surface.
    ///
    /// The surface colour is the up-facing (glass-top) ink; this is the shade. A
    /// document entity opts in through `ferrocad_gpui::display`.
    pub fn with_shade_color(mut self, color: [f32; 4]) -> Self {
        self.shade_color = Some(color);
        self
    }

    /// Emit the occluded-edge ghost pass. Document entities opt in; overlays do not.
    pub fn with_ghost_edges(mut self) -> Self {
        self.ghost_edges = true;
        self
    }

    /// Whether the occluded-edge ghost pass is emitted.
    pub fn ghost_edges(&self) -> bool {
        self.ghost_edges
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

    /// The model-space polyline of the topological edge named `name`, if the item
    /// carries one.
    ///
    /// Wire segments are tagged with the part id of the edge they belong to (a kernel
    /// tessellation emits one contiguous run per edge), so this collects that run and
    /// chains it end to end. It is what an edge manipulator (the F1 dress-up arc) anchors
    /// on: a click reports `Edge3`, and this resolves it to points. A run that cannot be
    /// chained is returned in emission order rather than dropped.
    pub fn edge_polyline(&self, name: &str) -> Option<Vec<[f32; 3]>> {
        let part = self
            .part_names
            .iter()
            .position(|candidate| candidate == name)? as u32;
        let mut segments = Vec::new();
        for pair in self.edges.chunks_exact(2) {
            let (a, b) = (pair[0] as usize, pair[1] as usize);
            if self.part_at(pair[0]) != part && self.part_at(pair[1]) != part {
                continue;
            }
            let (Some(from), Some(to)) = (self.positions.get(a), self.positions.get(b)) else {
                continue;
            };
            segments.push((*from, *to));
        }
        let (first, second) = segments.first().copied()?;
        let mut points = vec![first, second];
        for (from, to) in segments.iter().skip(1) {
            let last = *points.last().expect("a seeded polyline");
            if *from == last {
                points.push(*to);
            } else if *to == last {
                points.push(*from);
            } else {
                points.push(*from);
                points.push(*to);
            }
        }
        Some(points)
    }

    /// Emit the item's draw packets into `queue`.
    ///
    /// Order: surface, wire, points, then the selection highlight: a subtle tint, then
    /// the opaque highlight wire. Both highlight passes carry [`HIGHLIGHT_DEPTH_BIAS`],
    /// so they win the depth test against the coplanar base geometry they mark.
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
                    0.0,
                    self.generation,
                ));
            }
        }
        if self.draw_mode.has_edges() {
            if let Some(geometry) = self.wire_geometry() {
                queue.packets.push(self.packet(
                    self.wire_mesh,
                    Topology::Lines,
                    geometry.clone(),
                    world,
                    self.edge_color,
                    WIRE_DEPTH_BIAS,
                    self.generation,
                ));
                if self.ghost_edges {
                    // Pass only where the solid already wrote a nearer depth: the
                    // hidden portions of the wire show at the ghost token.
                    queue.packets.push(self.packet_with_depth(
                        self.wire_mesh,
                        Topology::Lines,
                        geometry,
                        world,
                        theme::EDGE_OCCLUDED,
                        WIRE_DEPTH_BIAS,
                        self.generation,
                        GHOST_DEPTH,
                        false,
                    ));
                }
            }
        }
        if self.show_vertices {
            if let Some(geometry) = self.points_geometry() {
                queue.packets.push(self.packet(
                    self.points_mesh,
                    Topology::Points,
                    geometry,
                    world,
                    self.edge_color,
                    0.0,
                    self.generation,
                ));
            }
        }

        if self.highlight.is_empty() {
            return;
        }
        if self.draw_mode.has_surface() {
            if let Some(geometry) = self.highlight_surface_geometry() {
                let mut packet = self.packet_with_depth(
                    self.highlight_surface_mesh,
                    Topology::Triangles,
                    geometry,
                    world,
                    HIGHLIGHT_TINT,
                    HIGHLIGHT_DEPTH_BIAS,
                    self.highlight_generation,
                    HIGHLIGHT_DEPTH,
                    true,
                );
                // Single-colour tint: the item's `shade` would select the untinted
                // glass-shade token for side/down facets. The base surface below still
                // carries the real two-tone material.
                packet.state.shade = [0.0; 4];
                queue.packets.push(packet);
            }
        }
        if self.draw_mode.has_edges() {
            if let Some(geometry) = self.highlight_wire_geometry() {
                queue.packets.push(self.packet(
                    self.highlight_wire_mesh,
                    Topology::Lines,
                    geometry,
                    world,
                    HIGHLIGHT_COLOR,
                    HIGHLIGHT_DEPTH_BIAS,
                    self.highlight_generation,
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
        depth_bias: f32,
        generation: Generation,
    ) -> DrawPacket {
        self.packet_with_depth(
            mesh_id,
            topology,
            geometry,
            world_transform,
            color,
            depth_bias,
            generation,
            DepthState::default(),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn packet_with_depth(
        &self,
        mesh_id: MeshId,
        topology: Topology,
        geometry: Geometry,
        world_transform: ferrocad_types::Matrix4,
        color: [f32; 4],
        depth_bias: f32,
        generation: Generation,
        depth: DepthState,
        pickable: bool,
    ) -> DrawPacket {
        DrawPacket {
            mesh_id,
            generation,
            topology,
            geometry: Some(geometry),
            state: StateSnapshot {
                world_transform,
                color,
                shade: self.shade_color.unwrap_or([0.0; 4]),
                depth_bias,
                texture: None,
                id: self.pick_id(),
                depth,
                pickable,
                program: None,
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
                    color: [1.0, 1.0, 1.0, 1.0],
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
        self.wire_geometry_for(&self.edges)
    }

    /// The wireframe for a subset of segment indices.
    fn wire_geometry_for(&self, indices: &[u32]) -> Option<Geometry> {
        if indices.len() < 2 {
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
                color: [1.0, 1.0, 1.0, 1.0],
            })
            .collect();
        Some(Geometry {
            vertices,
            indices: indices.to_vec(),
        })
    }

    /// The topological vertices, as a point list.
    fn points_geometry(&self) -> Option<Geometry> {
        let mut vertices = Vec::with_capacity(self.vertices.len());
        let mut indices = Vec::with_capacity(self.vertices.len());
        for &position_index in &self.vertices {
            let Some(position) = self.positions.get(position_index as usize) else {
                continue;
            };
            indices.push(vertices.len() as u32);
            vertices.push(Vertex {
                position: *position,
                normal: [0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                part_idx: self
                    .parts
                    .get(position_index as usize)
                    .copied()
                    .unwrap_or(0),
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        (!vertices.is_empty()).then_some(Geometry { vertices, indices })
    }

    /// The part id at a position (the triangles and segments that use it agree).
    fn part_at(&self, position: u32) -> u32 {
        self.parts.get(position as usize).copied().unwrap_or(0)
    }

    fn is_highlighted(&self, part: u32) -> bool {
        self.highlight.binary_search(&part).is_ok()
    }

    /// The highlighted triangles, flat-shaded so each carries its part id.
    fn highlight_surface_geometry(&self) -> Option<Geometry> {
        let mut selected = Vec::new();
        for triangle in self.triangles.chunks_exact(3) {
            if self.is_highlighted(self.part_at(triangle[0])) {
                selected.extend_from_slice(triangle);
            }
        }
        if selected.is_empty() {
            return None;
        }
        Some(flat_surface(&self.positions, &selected, &self.parts))
    }

    /// The highlighted edge segments.
    fn highlight_wire_geometry(&self) -> Option<Geometry> {
        let mut selected = Vec::new();
        for segment in self.edges.chunks_exact(2) {
            if self.is_highlighted(self.part_at(segment[0])) {
                selected.extend_from_slice(segment);
            }
        }
        if selected.is_empty() {
            return None;
        }
        self.wire_geometry_for(&selected)
    }
}

/// Per-position part ids derived from a tessellation's per-primitive ids.
///
/// The OCCT tessellation duplicates vertices per face (and per edge), so a position
/// belongs to exactly one part. If a producer shares a position across parts, the
/// later primitive wins; that is why the seam documents per-face duplication.
fn parts_from_tessellation(mesh: &Tessellation) -> Vec<u32> {
    let mut parts = vec![0; mesh.positions.len()];
    for (index, triangle) in mesh.triangles.chunks_exact(3).enumerate() {
        if let Some(part) = mesh.triangle_parts.get(index) {
            for &position in triangle {
                if let Some(slot) = parts.get_mut(position as usize) {
                    *slot = *part;
                }
            }
        }
    }
    for (index, segment) in mesh.edges.chunks_exact(2).enumerate() {
        if let Some(part) = mesh.segment_parts.get(index) {
            for &position in segment {
                if let Some(slot) = parts.get_mut(position as usize) {
                    *slot = *part;
                }
            }
        }
    }
    for (index, &position) in mesh.vertices.iter().enumerate() {
        if let Some(part) = mesh.vertex_parts.get(index) {
            if let Some(slot) = parts.get_mut(position as usize) {
                *slot = *part;
            }
        }
    }
    parts
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
                color: [1.0, 1.0, 1.0, 1.0],
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
    fn the_default_entity_colors_are_the_tokens() {
        let item = ShapeItem::new(1);
        assert_eq!(item.surface_color(), theme::GLASS_SHADE);
        assert_eq!(item.edge_color(), theme::EDGE_FRONT);
        assert!(!item.ghost_edges(), "overlays and gizmos must opt in");
    }

    #[test]
    fn a_two_tone_surface_carries_its_shade() {
        let item = triangle_item().with_shade_color(theme::GLASS_SHADE);
        let queue = emit(&item);
        assert_eq!(queue.packets[0].state.shade, theme::GLASS_SHADE);
        let plain = emit(&triangle_item());
        assert_eq!(plain.packets[0].state.shade, [0.0; 4]);
    }

    #[test]
    fn ghost_edges_emit_a_depth_greater_pass() {
        let item = triangle_item()
            .with_draw_mode(DrawMode::ShadedWithEdges)
            .with_ghost_edges();
        let queue = emit(&item);
        assert_eq!(queue.len(), 3, "surface, front wire, ghost wire");
        let ghost = &queue.packets[2];
        assert_eq!(ghost.topology, Topology::Lines);
        assert_eq!(ghost.state.depth, GHOST_DEPTH);
        assert_eq!(ghost.state.color, theme::EDGE_OCCLUDED);
        assert!(!ghost.state.pickable, "the ghost pass never picks");
        assert_eq!(
            queue.packets[1].state.depth,
            DepthState::default(),
            "the front wire keeps the normal test"
        );
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

    /// Two faces (parts 0 and 1), one edge (part 2) and one vertex (part 3).
    fn two_part_tessellation() -> ferrocad_geom::Tessellation {
        ferrocad_geom::Tessellation {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 3, 4, 5],
            triangle_parts: vec![0, 1],
            edges: vec![6, 7],
            segment_parts: vec![2],
            vertices: vec![8],
            vertex_parts: vec![3],
            parts: vec![
                "Face1".to_string(),
                "Face2".to_string(),
                "Edge1".to_string(),
                "Vertex1".to_string(),
            ],
        }
    }

    #[test]
    fn a_tessellation_becomes_a_pickable_stage_item() {
        let mesh = two_part_tessellation();
        let item = ShapeItem::from_tessellation(7, &mesh)
            .with_draw_mode(DrawMode::ShadedWithEdges)
            .with_vertices(true);

        assert_eq!(item.entity_id(), 7);
        assert_eq!(item.parts(), &[0, 0, 0, 1, 1, 1, 2, 2, 3]);
        assert_eq!(item.part_name(2), Some("Edge1"));
        assert_eq!(item.part_name(9), None);
        assert_eq!(item.vertices(), &[8]);

        let queue = emit(&item);
        assert_eq!(queue.len(), 3, "surface, wire and points");
        let points = queue
            .packets
            .iter()
            .find(|packet| packet.topology == Topology::Points)
            .expect("a points packet");
        let geometry = points.geometry.as_ref().expect("fresh geometry");
        assert_eq!(geometry.vertices.len(), 1);
        assert_eq!(geometry.vertices[0].part_idx, 3);
    }

    #[test]
    fn an_edge_name_resolves_to_its_polyline() {
        let item = ShapeItem::from_tessellation(7, &two_part_tessellation());
        assert_eq!(
            item.edge_polyline("Edge1"),
            Some(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]])
        );
        assert_eq!(item.edge_polyline("Face1"), None, "a face has no wire run");
        assert_eq!(item.edge_polyline("Edge9"), None, "no such part");
    }

    #[test]
    fn an_edge_polyline_chains_its_segments_in_order() {
        let mesh = ferrocad_geom::Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            normals: vec![],
            triangles: vec![],
            triangle_parts: vec![],
            // The second segment is emitted reversed.
            edges: vec![0, 1, 2, 1],
            segment_parts: vec![0, 0],
            vertices: vec![],
            vertex_parts: vec![],
            parts: vec!["Edge1".to_string()],
        };
        let item = ShapeItem::from_tessellation(7, &mesh);
        assert_eq!(
            item.edge_polyline("Edge1"),
            Some(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]])
        );
    }

    #[test]
    fn a_highlight_draws_only_the_selected_part() {
        let mesh = two_part_tessellation();
        let item = ShapeItem::from_tessellation(7, &mesh)
            .with_draw_mode(DrawMode::ShadedWithEdges)
            .with_highlight([1, 2]);

        let queue = emit(&item);
        assert_eq!(
            queue.len(),
            4,
            "base surface, base wire, highlight tint, highlight wire"
        );
        // The base passes stay, and stay ahead of the highlight so the natural surface
        // and its own edges remain visible through the tint.
        assert_eq!(queue.packets[0].topology, Topology::Triangles);
        assert_eq!(queue.packets[0].state.color, item.surface_color());
        assert_eq!(queue.packets[1].topology, Topology::Lines);
        assert_eq!(queue.packets[1].state.color, item.edge_color());

        let tint = queue
            .packets
            .iter()
            .find(|packet| {
                packet.topology == Topology::Triangles && packet.state.color == HIGHLIGHT_TINT
            })
            .expect("a highlight surface packet");
        assert_eq!(tint.state.depth_bias, HIGHLIGHT_DEPTH_BIAS);
        assert_eq!(tint.state.depth, HIGHLIGHT_DEPTH);
        assert_eq!(
            tint.state.shade, [0.0; 4],
            "the tint is single-colour so every facet gets the cyan"
        );
        assert!(
            !tint.state.depth.write,
            "the tint must not occlude the wire drawn after it"
        );
        let geometry = tint.geometry.as_ref().expect("fresh geometry");
        assert_eq!(geometry.vertices.len(), 3, "only the part-1 triangle");
        assert!(geometry.vertices.iter().all(|vertex| vertex.part_idx == 1));

        let wire = queue
            .packets
            .iter()
            .find(|packet| {
                packet.topology == Topology::Lines && packet.state.color == HIGHLIGHT_COLOR
            })
            .expect("a highlight wire packet");
        assert_eq!(wire.state.depth_bias, HIGHLIGHT_DEPTH_BIAS);
        let geometry = wire.geometry.as_ref().expect("fresh geometry");
        assert_eq!(geometry.indices, vec![6, 7], "only the selected edge");
        assert!(
            geometry
                .indices
                .iter()
                .all(|&index| geometry.vertices[index as usize].part_idx == 2)
        );
    }

    #[test]
    fn clearing_the_highlight_removes_the_packet() {
        let mesh = two_part_tessellation();
        let mut item = ShapeItem::from_tessellation(7, &mesh).with_highlight([0, 1]);
        assert_eq!(emit(&item).len(), 2, "surface + highlight surface");

        item.set_highlight(std::iter::empty());
        assert!(item.highlight().is_empty());
        assert_eq!(emit(&item).len(), 1, "surface only");
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

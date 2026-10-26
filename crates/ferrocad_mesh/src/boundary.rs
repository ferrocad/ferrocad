//! Boundary loops of a facet selection.
//!
//! This is the kernel-side answer used by `MeshPart.wireFromSegment`: given the
//! facet indices of a segment, the directed edges of those facets that are not
//! cancelled by their reverse form the boundary, and chaining them gives the
//! ordered point-index loops.
//!
//! # Orientation
//!
//! The loops follow the selected facets' **winding**: an outward-wound facet
//! contributes its directed edges in that direction. Upstream's
//! `MeshPart.wireFromSegment` builds its wires in the reverse boundary order, so
//! a binding that has to match upstream should reverse each loop it hands to
//! `Part.makePolygon`.
//!
//! A loop is closed implicitly: the last index is followed by the first, and the
//! first index is not repeated at the end. Each loop is returned as a bare list
//! of point indices; callers that need positions index this mesh's local points.

use std::collections::HashMap;

use crate::Mesh;
use crate::mesh::directed_edges;

impl Mesh {
    /// The ordered boundary loops around `facets`.
    ///
    /// Only the directed edges of the selected facets are considered. An edge
    /// `(a, b)` is an interior edge when `(b, a)` also occurs, and each such pair
    /// cancels; what is left is the boundary. Loops are chained with the directed
    /// edge order, starting from the first boundary edge in `facets` order, so
    /// the result is deterministic.
    ///
    /// Facet indices outside this mesh are ignored. A closed selection (every
    /// edge used twice in opposite directions) has no boundary and returns an
    /// empty list.
    pub fn boundary_loops(&self, facets: &[usize]) -> Vec<Vec<u32>> {
        let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
        for &index in facets {
            let Some(facet) = self.facets().get(index) else {
                continue;
            };
            for edge in directed_edges(*facet) {
                *counts.entry(edge).or_insert(0) += 1;
            }
        }
        if counts.is_empty() {
            return Vec::new();
        }

        // Keep one direction per cancelling pair, with the multiplicity of
        // repeated facets. `net(a, b) = count(a, b) - count(b, a)` when positive.
        let mut remaining: HashMap<(u32, u32), usize> = HashMap::new();
        for (&edge, &forward) in &counts {
            let reverse = counts.get(&(edge.1, edge.0)).copied().unwrap_or(0);
            if forward > reverse {
                remaining.insert(edge, forward - reverse);
            }
        }

        // First-seen facet order, then winding order inside each facet. Repeated
        // directed edges (duplicate facets) keep their multiplicity.
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for &index in facets {
            let Some(facet) = self.facets().get(index) else {
                continue;
            };
            for edge in directed_edges(*facet) {
                if let Some(count) = remaining.get_mut(&edge) {
                    if *count > 0 {
                        *count -= 1;
                        edges.push(edge);
                    }
                }
            }
        }

        let mut by_from: HashMap<u32, Vec<usize>> = HashMap::new();
        for (edge_index, &(from, _)) in edges.iter().enumerate() {
            by_from.entry(from).or_default().push(edge_index);
        }

        let mut used = vec![false; edges.len()];
        let mut loops = Vec::new();
        for start_index in 0..edges.len() {
            if used[start_index] {
                continue;
            }
            let start = edges[start_index].0;
            let mut chain = vec![start];
            let mut current = start_index;
            loop {
                used[current] = true;
                let to = edges[current].1;
                chain.push(to);
                if to == start {
                    break;
                }
                let next = by_from
                    .get(&to)
                    .and_then(|candidates| candidates.iter().copied().find(|&edge| !used[edge]));
                match next {
                    Some(edge) => current = edge,
                    // A malformed (non-manifold or truncated) selection can leave
                    // an open chain; keep what was chained rather than dropping it.
                    None => break,
                }
            }
            if chain.len() > 1 && chain.last() == Some(&start) {
                chain.pop();
            }
            loops.push(chain);
        }
        loops
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_types::Vector3;

    fn square() -> Mesh {
        Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap()
    }

    fn flat_grid() -> Mesh {
        let mut points = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                points.push(Vector3::new(x as f64, y as f64, 0.0));
            }
        }
        let mut facets = Vec::new();
        for y in 0..2u32 {
            for x in 0..2u32 {
                let a = y * 3 + x;
                let b = a + 1;
                let c = (y + 1) * 3 + x + 1;
                let d = (y + 1) * 3 + x;
                facets.push([a, b, c]);
                facets.push([a, c, d]);
            }
        }
        Mesh::new(points, facets).unwrap()
    }

    #[test]
    fn an_open_plane_has_one_boundary_loop() {
        let loops = square().boundary_loops(&[0, 1]);
        assert_eq!(loops, vec![vec![0, 1, 2, 3]]);

        let grid = flat_grid();
        let loops = grid.boundary_loops(&(0..grid.count_facets()).collect::<Vec<_>>());
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 8);
    }

    #[test]
    fn a_boxs_six_faces_each_have_a_four_point_loop() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let segments = mesh.planar_segments(1e-6, 0);
        assert_eq!(segments.len(), 6);
        for segment in &segments {
            let loops = mesh.boundary_loops(segment);
            assert_eq!(loops.len(), 1);
            assert_eq!(loops[0].len(), 4);
        }
    }

    #[test]
    fn a_closed_selection_has_no_boundary() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        assert!(
            mesh.boundary_loops(&(0..mesh.count_facets()).collect::<Vec<_>>())
                .is_empty()
        );
    }

    #[test]
    fn out_of_range_facets_are_ignored() {
        let loops = square().boundary_loops(&[0, 1, 99]);
        assert_eq!(loops, vec![vec![0, 1, 2, 3]]);
    }
}

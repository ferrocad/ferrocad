//! Planar facet segmentation.
//!
//! [`Mesh::planar_segments`] is the kernel half of upstream
//! `Mesh.getPlanarSegments(deviation, minFacets)`: it region-grows adjacent
//! facets that lie on one plane, within a **distance** tolerance (not an angle).
//! A neighbour must also keep the seed plane's normal within a small angular
//! tolerance, so two perpendicular facets of a staircase never join merely
//! because their shared edge lies on the plane.

use std::collections::{HashMap, VecDeque};

use ferrocad_types::Vector3;

use crate::Mesh;
use crate::mesh::directed_edges;

/// `cos` of the maximum normal deviation (about `1e-3` rad, 0.057 degrees).
///
/// The distance test alone would let a staircase's riser join a tread whenever
/// the shared edge happens to be in the tread plane; this term rejects it.
const NORMAL_TOLERANCE_COS: f64 = 0.999_999_5;

impl Mesh {
    /// Groups of adjacent facets lying on one plane (upstream
    /// `getPlanarSegments`).
    ///
    /// Region growing starts at the lowest-index unassigned facet and uses its
    /// plane (a unit normal and an offset). A facet sharing an edge with the
    /// region joins when all three of its points are within `deviation` of that
    /// plane and its normal agrees with the region normal within
    /// [`NORMAL_TOLERANCE_COS`]. The seed plane is not refitted as the region
    /// grows, so every member stays within `deviation` of the *same* plane.
    ///
    /// `deviation` is a distance in the mesh's local units; a negative or NaN
    /// value behaves like `0`. Groups smaller than `min_facets` are omitted;
    /// `0` means no minimum. Facet indices inside a group are in breadth-first
    /// discovery order, and groups are in seed order, so the result is
    /// deterministic. A mesh with no facets returns an empty list.
    pub fn planar_segments(&self, deviation: f64, min_facets: usize) -> Vec<Vec<usize>> {
        let count = self.count_facets();
        if count == 0 {
            return Vec::new();
        }
        let deviation = deviation.max(0.0);

        let neighbours = facet_neighbours(self);
        let points = self.points();
        let mut assigned = vec![false; count];
        let mut segments = Vec::new();
        let mut queue = VecDeque::new();

        for seed in 0..count {
            if assigned[seed] {
                continue;
            }
            assigned[seed] = true;
            let mut group = vec![seed];
            queue.clear();

            let seed_normal = facet_normal(points, self.facets()[seed]);
            let length = seed_normal.length();
            if length > 0.0 {
                let normal = seed_normal.scale(1.0 / length);
                let center = facet_center(points, self.facets()[seed]);
                let offset = normal.dot(&center);
                queue.push_back(seed);

                while let Some(current) = queue.pop_front() {
                    for &neighbour in &neighbours[current] {
                        if assigned[neighbour] {
                            continue;
                        }
                        let facet = self.facets()[neighbour];
                        let normal_of_neighbour = facet_normal(points, facet);
                        let neighbour_length = normal_of_neighbour.length();
                        if neighbour_length <= 0.0 {
                            continue;
                        }
                        let unit = normal_of_neighbour.scale(1.0 / neighbour_length);
                        if unit.dot(&normal) < NORMAL_TOLERANCE_COS {
                            continue;
                        }
                        let on_plane = facet.iter().all(|&index| {
                            let point = points[index as usize];
                            (normal.dot(&point) - offset).abs() <= deviation
                        });
                        if !on_plane {
                            continue;
                        }
                        assigned[neighbour] = true;
                        group.push(neighbour);
                        queue.push_back(neighbour);
                    }
                }
            }

            if min_facets == 0 || group.len() >= min_facets {
                segments.push(group);
            }
        }

        segments
    }
}

/// Facet adjacency over shared undirected edges.
fn facet_neighbours(mesh: &Mesh) -> Vec<Vec<usize>> {
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (index, facet) in mesh.facets().iter().enumerate() {
        for (a, b) in directed_edges(*facet) {
            let key = if a < b { (a, b) } else { (b, a) };
            by_edge.entry(key).or_default().push(index);
        }
    }

    let mut neighbours = vec![Vec::new(); mesh.count_facets()];
    for bucket in by_edge.values() {
        for (position, &first) in bucket.iter().enumerate() {
            for &second in &bucket[position + 1..] {
                neighbours[first].push(second);
                neighbours[second].push(first);
            }
        }
    }
    // Sorted, de-duplicated adjacency makes the breadth-first discovery order (and
    // therefore each group's facet order) independent of hash iteration order.
    for list in &mut neighbours {
        list.sort_unstable();
        list.dedup();
    }
    neighbours
}

/// The unnormalised facet normal `(b - a) x (c - a)`.
fn facet_normal(points: &[Vector3], [a, b, c]: [u32; 3]) -> Vector3 {
    let a = points[a as usize];
    let b = points[b as usize];
    let c = points[c as usize];
    b.sub(&a).cross(&c.sub(&a))
}

/// The facet centroid.
fn facet_center(points: &[Vector3], [a, b, c]: [u32; 3]) -> Vector3 {
    points[a as usize]
        .add(&points[b as usize])
        .add(&points[c as usize])
        .scale(1.0 / 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_types::Vector3;

    /// A 2x2-cell flat grid on `z = 0`, two triangles per cell.
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
    fn a_flat_grid_is_one_segment() {
        let segments = flat_grid().planar_segments(1e-6, 0);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].len(), 8);
    }

    #[test]
    fn a_box_has_six_planar_segments() {
        let segments = Mesh::box_(1.0, 1.0, 1.0).unwrap().planar_segments(1e-6, 0);
        assert_eq!(segments.len(), 6);
        assert!(segments.iter().all(|group| group.len() == 2));
        assert_eq!(segments.iter().map(Vec::len).sum::<usize>(), 12);
    }

    #[test]
    fn min_facets_filters_small_groups() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        assert!(mesh.planar_segments(1e-6, 3).is_empty());
        assert_eq!(mesh.planar_segments(1e-6, 2).len(), 6);
    }

    #[test]
    fn a_staircase_does_not_join_across_its_riser() {
        // The step is only 1e-3 high, so the distance test alone would accept the
        // riser within a 1e-2 deviation; the normal check is what keeps the
        // treads and the riser in separate groups.
        let mesh = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 1e-3),
                Vector3::new(1.0, 1.0, 1e-3),
                Vector3::new(0.0, 0.0, 1e-3),
                Vector3::new(0.0, 1.0, 1e-3),
            ],
            vec![
                [0, 1, 2],
                [0, 2, 3],
                [1, 5, 4],
                [1, 2, 5],
                [6, 4, 5],
                [6, 5, 7],
            ],
        )
        .unwrap();
        let segments = mesh.planar_segments(1e-2, 2);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments.iter().map(Vec::len).sum::<usize>(), 6);
    }
}

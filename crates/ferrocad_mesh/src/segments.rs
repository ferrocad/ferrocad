//! Planar facet segmentation.
//!
//! [`Mesh::planar_segments`] is the kernel half of upstream
//! `Mesh.getPlanarSegments(deviation, minFacets)`: it region-grows adjacent
//! facets that lie on one plane, within a **distance** tolerance (not an angle).
//!
//! Upstream's `MeshDistanceGenericSurfaceFitSegment<PlaneSurfaceFit>` starts from
//! a seed facet, puts the seed's three vertices into a least-squares plane fit
//! and then, before each neighbour is tested, refits that plane through the
//! gravity centres of the facets accepted so far. A neighbour joins when all
//! three of its vertices are within `deviation` of that current plane
//! (`Segmentation.cpp`). There is no separate angular test: a perpendicular
//! facet is rejected because its far vertices are farther than `deviation` from
//! the plane, not because of its normal. Groups of one facet are never reported
//! (`MeshSegmentAlgorithm::FindSegments` discards them), and `minFacets` filters
//! the rest.

use std::collections::{HashMap, VecDeque};

use ferrocad_types::Vector3;

use crate::Mesh;
use crate::mesh::directed_edges;

impl Mesh {
    /// Groups of adjacent facets lying on one plane (upstream
    /// `getPlanarSegments`).
    ///
    /// Region growing starts at the lowest-index unassigned facet. The plane fit
    /// begins with that facet's three vertices and is refitted through the
    /// facet centres of the members as the region grows, so noisy data that
    /// still lies within `deviation` of one least-squares plane forms one group.
    /// A facet sharing an edge with the region joins when all three of its
    /// points are within `deviation` of the current fit.
    ///
    /// `deviation` is a distance in the mesh's local units; a negative or NaN
    /// value behaves like `0`. A region must contain at least two facets: a
    /// single isolated facet is not a segment. `min_facets` then drops the
    /// remaining groups smaller than it; `0` means no minimum. Facet indices
    /// inside a group are in breadth-first discovery order, and groups are in
    /// seed order, so the result is deterministic. A mesh with no facets returns
    /// an empty list.
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
            let seed_facet = self.facets()[seed];
            let mut fit = PlaneFit::new(
                [
                    points[seed_facet[0] as usize],
                    points[seed_facet[1] as usize],
                    points[seed_facet[2] as usize],
                ],
                facet_normal(points, seed_facet),
            );

            queue.clear();
            queue.push_back(seed);
            while let Some(current) = queue.pop_front() {
                for &neighbour in &neighbours[current] {
                    if assigned[neighbour] {
                        continue;
                    }
                    fit.refresh();
                    let facet = self.facets()[neighbour];
                    let on_plane = facet
                        .iter()
                        .all(|&index| fit.distance(points[index as usize]).abs() <= deviation);
                    if !on_plane {
                        continue;
                    }
                    assigned[neighbour] = true;
                    group.push(neighbour);
                    fit.add(facet_center(points, facet));
                    queue.push_back(neighbour);
                }
            }

            // Upstream never reports a one-facet region.
            if group.len() > 1 && (min_facets == 0 || group.len() >= min_facets) {
                segments.push(group);
            }
        }

        segments
    }
}

/// A least-squares plane through the seed vertices and accepted facet centres.
///
/// The fit is a covariance eigen-decomposition; the eigenvector of the smallest
/// eigenvalue is the normal. A collinear point set leaves the normal undefined,
/// so the last good fit (initially the seed facet's plane) is kept.
struct PlaneFit {
    count: f64,
    sum: [f64; 3],
    /// `xx, xy, xz, yy, yz, zz`.
    squares: [f64; 6],
    base: Vector3,
    normal: Vector3,
}

impl PlaneFit {
    fn new(vertices: [Vector3; 3], fallback_normal: Vector3) -> Self {
        let length = fallback_normal.length();
        let normal = if length > 0.0 {
            fallback_normal.scale(1.0 / length)
        } else {
            Vector3::new(0.0, 0.0, 1.0)
        };
        let base = vertices[0]
            .add(&vertices[1])
            .add(&vertices[2])
            .scale(1.0 / 3.0);
        let mut fit = PlaneFit {
            count: 0.0,
            sum: [0.0; 3],
            squares: [0.0; 6],
            base,
            normal,
        };
        for vertex in vertices {
            fit.add(vertex);
        }
        fit.refresh();
        fit
    }

    fn add(&mut self, point: Vector3) {
        let [x, y, z] = [point.x, point.y, point.z];
        self.count += 1.0;
        self.sum[0] += x;
        self.sum[1] += y;
        self.sum[2] += z;
        self.squares[0] += x * x;
        self.squares[1] += x * y;
        self.squares[2] += x * z;
        self.squares[3] += y * y;
        self.squares[4] += y * z;
        self.squares[5] += z * z;
    }

    fn refresh(&mut self) {
        if self.count < 3.0 {
            return;
        }
        let n = self.count;
        let [sx, sy, sz] = self.sum;
        let [xx, xy, xz, yy, yz, zz] = self.squares;
        let covariance = [
            [xx - sx * sx / n, xy - sx * sy / n, xz - sx * sz / n],
            [xy - sx * sy / n, yy - sy * sy / n, yz - sy * sz / n],
            [xz - sx * sz / n, yz - sy * sz / n, zz - sz * sz / n],
        ];
        let (values, vectors) = eigen_symmetric(covariance);
        let largest = values[0].max(values[1]).max(values[2]);
        // A line (or a point) has no unique plane; keep the previous fit.
        if values[1] <= 1e-12 * largest.max(1.0) {
            return;
        }
        // `eigen_symmetric` orders the columns with the eigenvalues.
        let normal = Vector3::new(vectors[0][0], vectors[1][0], vectors[2][0]);
        let length = normal.length();
        if length > 0.0 && length.is_finite() {
            self.normal = normal.scale(1.0 / length);
            self.base = Vector3::new(sx / n, sy / n, sz / n);
        }
    }

    fn distance(&self, point: Vector3) -> f64 {
        self.normal.dot(&point.sub(&self.base))
    }
}

/// Eigenvalues (ascending) and eigenvectors (as columns) of a symmetric 3x3
/// matrix, by cyclic Jacobi rotation.
fn eigen_symmetric(mut matrix: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut vectors = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let pairs = [(0usize, 1usize), (0, 2), (1, 2)];

    for _ in 0..64 {
        let diagonal = matrix[0][0].abs() + matrix[1][1].abs() + matrix[2][2].abs();
        let mut largest = 0.0;
        let mut pivot = None;
        for &(p, q) in &pairs {
            if matrix[p][q].abs() > largest {
                largest = matrix[p][q].abs();
                pivot = Some((p, q));
            }
        }
        let Some((p, q)) = pivot else { break };
        if largest <= 1e-18 * diagonal.max(f64::MIN_POSITIVE) {
            break;
        }

        let theta = (matrix[q][q] - matrix[p][p]) / (2.0 * matrix[p][q]);
        let t = if theta >= 0.0 {
            1.0 / (theta + (1.0 + theta * theta).sqrt())
        } else {
            -1.0 / (-theta + (1.0 + theta * theta).sqrt())
        };
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;
        let tau = s / (1.0 + c);

        let entry = matrix[p][q];
        matrix[p][p] -= t * entry;
        matrix[q][q] += t * entry;
        matrix[p][q] = 0.0;
        matrix[q][p] = 0.0;
        for i in 0..3 {
            if i != p && i != q {
                let aip = matrix[i][p];
                let aiq = matrix[i][q];
                matrix[i][p] = aip - s * (aiq + tau * aip);
                matrix[i][q] = aiq + s * (aip - tau * aiq);
                matrix[p][i] = matrix[i][p];
                matrix[q][i] = matrix[i][q];
            }
        }
        for row in &mut vectors {
            let vip = row[p];
            let viq = row[q];
            row[p] = vip - s * (viq + tau * vip);
            row[q] = viq + s * (vip - tau * viq);
        }
    }

    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| matrix[a][a].total_cmp(&matrix[b][b]));
    let values = [
        matrix[order[0]][order[0]],
        matrix[order[1]][order[1]],
        matrix[order[2]][order[2]],
    ];
    let vectors = [
        [
            vectors[0][order[0]],
            vectors[0][order[1]],
            vectors[0][order[2]],
        ],
        [
            vectors[1][order[0]],
            vectors[1][order[1]],
            vectors[1][order[2]],
        ],
        [
            vectors[2][order[0]],
            vectors[2][order[1]],
            vectors[2][order[2]],
        ],
    ];
    (values, vectors)
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
    fn single_facet_regions_are_not_reported() {
        // One facet, and two facets that only meet at a point, have no segment.
        let single = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        assert!(single.planar_segments(0.0, 0).is_empty());

        let corner = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(-1.0, 0.0, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 3, 4]],
        )
        .unwrap();
        assert!(corner.planar_segments(1e-9, 0).is_empty());
    }

    #[test]
    fn a_step_is_judged_by_distance_not_by_its_normal() {
        // The step is 1e-3 high: within a 1e-2 deviation the risers join the
        // treads (upstream has no angular test), but they stay separate below it.
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

        let joined = mesh.planar_segments(1e-2, 2);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].len(), 6);

        let split = mesh.planar_segments(1e-6, 2);
        assert_eq!(split.len(), 3);
        assert_eq!(split.iter().map(Vec::len).sum::<usize>(), 6);
    }

    #[test]
    fn the_fitted_plane_follows_a_shallow_fold() {
        // A two-strip "V": the far end of the second arm is 0.02 above the seed
        // arm's plane, more than the 0.015 deviation. Only the refitted plane,
        // which settles between the arms, contains all 16 facets.
        let mut points = Vec::new();
        for y in -4..=4 {
            let height = if y >= 0 { 0.0 } else { -0.005 * y as f64 };
            for x in 0..2 {
                points.push(Vector3::new(x as f64, y as f64, height));
            }
        }
        let index = |x: usize, y: i64| -> u32 { ((y + 4) * 2 + x as i64) as u32 };
        let mut facets = Vec::new();
        for y in -4..4 {
            let a = index(0, y);
            let b = index(1, y);
            let c = index(1, y + 1);
            let d = index(0, y + 1);
            facets.push([a, b, c]);
            facets.push([a, c, d]);
        }
        let mesh = Mesh::new(points, facets).unwrap();

        let segments = mesh.planar_segments(0.015, 2);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].len(), 16);
    }

    #[test]
    fn the_eigen_solver_is_ordered_and_orthonormal() {
        let (values, vectors) = eigen_symmetric([[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]]);
        assert!(values[0] <= values[1] && values[1] <= values[2]);
        assert!((values[0] - 1.0).abs() < 1e-12);
        assert!((values[1] - 3.0).abs() < 1e-12);
        assert!((values[2] - 5.0).abs() < 1e-12);
        // Columns are unit and mutually perpendicular.
        for column in 0..3 {
            let a = Vector3::new(vectors[0][column], vectors[1][column], vectors[2][column]);
            assert!((a.length() - 1.0).abs() < 1e-12);
        }
        let first = Vector3::new(vectors[0][0], vectors[1][0], vectors[2][0]);
        let second = Vector3::new(vectors[0][1], vectors[1][1], vectors[2][1]);
        assert!(first.dot(&second).abs() < 1e-12);
    }
}

//! Query and cleanup operations on a [`Mesh`]: area, point/facet deduplication,
//! degenerate-facet removal and normal harmonisation.
//!
//! Every mutating operation rebuilds the mesh through [`Mesh::new`] (which
//! re-runs the structural validation) and then restores the placement, so the
//! index invariant documented on [`Mesh`] holds after every call. The points are
//! local; the placement is never baked into them.

use std::collections::{HashMap, HashSet, VecDeque};

use ferrocad_types::Vector3;

use crate::Mesh;
use crate::mesh::directed_edges;

/// The point-merge tolerance, `gp::Resolution()` as used by `Shape.tessellate`
/// and by upstream's `RemoveDuplicatedPoints`.
///
/// Deciding that two points are the same is a point-scale question, so this is
/// an absolute distance (not a fraction of the mesh size), exactly like the
/// tessellator's weld.
const MERGE_TOLERANCE: f64 = 1e-7;

/// `|(b - a) x (c - a)|` at or below this value is a zero-area facet.
///
/// This is twice the triangle area; `1e-15` is the threshold SimpleCAD's glue
/// code already uses for the same purpose (`mesh/glue.py`).
const DEGENERATE_CROSS_LENGTH: f64 = 1e-15;

impl Mesh {
    /// The sum of the facet areas on the local points (FreeCAD `Mesh.Area`).
    ///
    /// The placement is not applied, matching [`volume`](Mesh::volume).
    pub fn area(&self) -> f64 {
        let points = self.points();
        self.facets()
            .iter()
            .map(|facet| {
                let a = points[facet[0] as usize];
                let b = points[facet[1] as usize];
                let c = points[facet[2] as usize];
                b.sub(&a).cross(&c.sub(&a)).length() * 0.5
            })
            .sum()
    }

    /// Merge points closer than [`MERGE_TOLERANCE`] and remap the facets
    /// (upstream `MeshKernel::RemoveDuplicatedPoints`).
    ///
    /// The first point of a cluster is kept as its representative (a greedy
    /// first-seen weld, not a transitive merge). A facet that collapses to fewer
    /// than three distinct points because of the merge is dropped; the mesh
    /// stays valid and keeps its placement.
    ///
    /// Returns whether the point list or the facet list changed.
    pub fn remove_duplicated_points(&mut self) -> bool {
        let points = self.points();
        if points.len() < 2 {
            return false;
        }

        let mut representatives: Vec<Vector3> = Vec::with_capacity(points.len());
        let mut remap = vec![0u32; points.len()];
        let mut cells: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();

        for (index, point) in points.iter().enumerate() {
            let key = grid_key(*point);
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(bucket) = cells.get(&(
                            key.0.saturating_add(dx),
                            key.1.saturating_add(dy),
                            key.2.saturating_add(dz),
                        )) {
                            for &candidate in bucket {
                                let representative = representatives[candidate as usize];
                                // The same per-axis test as the tessellator's weld:
                                // two points are the same when every coordinate
                                // agrees within the tolerance.
                                if (representative.x - point.x).abs() <= MERGE_TOLERANCE
                                    && (representative.y - point.y).abs() <= MERGE_TOLERANCE
                                    && (representative.z - point.z).abs() <= MERGE_TOLERANCE
                                {
                                    found = Some(candidate);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            remap[index] = match found {
                Some(candidate) => candidate,
                None => {
                    let candidate = representatives.len() as u32;
                    representatives.push(*point);
                    cells.entry(key).or_default().push(candidate);
                    candidate
                }
            };
        }

        if representatives.len() == points.len() {
            return false;
        }

        let facets = self
            .facets()
            .iter()
            .filter_map(|facet| {
                let a = remap[facet[0] as usize];
                let b = remap[facet[1] as usize];
                let c = remap[facet[2] as usize];
                (a != b && b != c && a != c).then_some([a, b, c])
            })
            .collect();

        self.replace_topology(representatives, facets);
        true
    }

    /// Drop every later facet that addresses the same three points as an earlier
    /// facet, regardless of rotation or winding (upstream
    /// `MeshKernel::RemoveDuplicatedFacets`).
    ///
    /// Equal point triples necessarily belong to the same connected component,
    /// so a single canonical set is equivalent to the per-component check.
    /// Returns whether any facet was dropped.
    pub fn remove_duplicated_facets(&mut self) -> bool {
        let mut seen: HashSet<[u32; 3]> = HashSet::with_capacity(self.count_facets());
        let mut facets = Vec::with_capacity(self.count_facets());
        let mut changed = false;
        for facet in self.facets() {
            let mut key = *facet;
            key.sort_unstable();
            if seen.insert(key) {
                facets.push(*facet);
            } else {
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        let points = self.points().to_vec();
        self.replace_topology(points, facets);
        true
    }

    /// Drop facets that repeat a point index or have zero area (upstream
    /// `MeshKernel::RemoveDegeneration`).
    ///
    /// Run this after [`remove_duplicated_points`](Mesh::remove_duplicated_points):
    /// a merge can be what makes a facet collapse or become collinear.
    /// Returns whether any facet was dropped.
    pub fn fix_degenerations(&mut self) -> bool {
        let points = self.points();
        let mut facets = Vec::with_capacity(self.count_facets());
        let mut changed = false;
        for facet in self.facets() {
            let [a, b, c] = *facet;
            if a == b || b == c || a == c {
                changed = true;
                continue;
            }
            let pa = points[a as usize];
            let pb = points[b as usize];
            let pc = points[c as usize];
            if pb.sub(&pa).cross(&pc.sub(&pa)).length() <= DEGENERATE_CROSS_LENGTH {
                changed = true;
                continue;
            }
            facets.push(*facet);
        }
        if !changed {
            return false;
        }
        let points = points.to_vec();
        self.replace_topology(points, facets);
        true
    }

    /// Orient every edge-connected component consistently and outward
    /// (upstream `MeshKernel::HarmonizeNormals`).
    ///
    /// Adjacent facets must traverse their shared edge in opposite directions;
    /// a breadth-first walk from each component's first facet flips the
    /// neighbours that disagree. The component's signed volume then decides the
    /// global direction: a negative one is flipped so that a closed component
    /// encloses positive volume.
    ///
    /// Returns whether any facet's winding changed.
    pub fn harmonize_normals(&mut self) -> bool {
        let facets = self.facets();
        let count = facets.len();
        if count == 0 {
            return false;
        }

        // Undirected edge -> the facets that use it and the direction each one
        // traverses it in (true for lower-index -> higher-index).
        let mut by_edge: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
        for (index, facet) in facets.iter().enumerate() {
            for (a, b) in directed_edges(*facet) {
                let key = if a < b { (a, b) } else { (b, a) };
                by_edge.entry(key).or_default().push((index, a < b));
            }
        }

        // (neighbour, same original direction) pairs. Building a star around the
        // first facet keeps an over-used (non-manifold) edge finite.
        let mut neighbours: Vec<Vec<(usize, bool)>> = vec![Vec::new(); count];
        for bucket in by_edge.values() {
            let Some(&(first, first_forward)) = bucket.first() else {
                continue;
            };
            for &(other, other_forward) in &bucket[1..] {
                let same = first_forward == other_forward;
                neighbours[first].push((other, same));
                neighbours[other].push((first, same));
            }
        }
        // Deterministic neighbour order keeps a non-manifold conflict's
        // first-wins resolution stable across runs.
        for list in &mut neighbours {
            list.sort_unstable();
        }

        let points = self.points();
        let mut visited = vec![false; count];
        let mut flip = vec![false; count];
        let mut component = Vec::new();
        let mut queue = VecDeque::new();

        for seed in 0..count {
            if visited[seed] {
                continue;
            }
            visited[seed] = true;
            flip[seed] = false;
            component.clear();
            queue.clear();
            queue.push_back(seed);
            while let Some(current) = queue.pop_front() {
                component.push(current);
                for &(neighbour, same) in &neighbours[current] {
                    if !visited[neighbour] {
                        visited[neighbour] = true;
                        flip[neighbour] = flip[current] ^ same;
                        queue.push_back(neighbour);
                    }
                }
            }

            let mut volume = 0.0;
            for &index in &component {
                let facet = oriented(facets[index], flip[index]);
                volume += tetrahedron_volume(points, facet);
            }
            volume /= 6.0;
            let tolerance = component_volume_tolerance(points, facets, &component);
            if volume < -tolerance {
                for &index in &component {
                    flip[index] = !flip[index];
                }
            }
        }

        if !flip.iter().any(|&changed| changed) {
            return false;
        }

        let facets: Vec<[u32; 3]> = facets
            .iter()
            .enumerate()
            .map(|(index, facet)| oriented(*facet, flip[index]))
            .collect();
        let points = points.to_vec();
        self.replace_topology(points, facets);
        true
    }

    /// Replace the topology and keep the placement.
    ///
    /// Going through [`Mesh::new`] re-runs the structural validation; the
    /// operations above only remove or re-wind facets and remap points, so they
    /// cannot violate it. The new topology has no facet groups: every mutator
    /// changes facet indices, so any group recorded earlier would be stale.
    fn replace_topology(&mut self, points: Vec<Vector3>, facets: Vec<[u32; 3]>) {
        let placement = self.placement();
        let mut mesh =
            Mesh::new(points, facets).expect("mesh operations preserve the structural invariants");
        mesh.set_placement(placement);
        *self = mesh;
    }
}

/// The lattice cell of a point for the 27-neighbour merge search.
fn grid_key(point: Vector3) -> (i64, i64, i64) {
    fn axis(value: f64) -> i64 {
        let cell = (value / MERGE_TOLERANCE).floor();
        if cell.is_finite() { cell as i64 } else { 0 }
    }
    (axis(point.x), axis(point.y), axis(point.z))
}

/// A facet with its first two post-vertices swapped when `flip` is set.
fn oriented(facet: [u32; 3], flip: bool) -> [u32; 3] {
    if flip {
        [facet[0], facet[2], facet[1]]
    } else {
        facet
    }
}

/// `a . (b x c)`, the signed six-times-tetrahedron volume of one facet.
fn tetrahedron_volume(points: &[Vector3], [a, b, c]: [u32; 3]) -> f64 {
    points[a as usize].dot(&points[b as usize].cross(&points[c as usize]))
}

/// A scale-aware threshold below which a component's signed volume is treated as
/// zero, so an open or flat component is not flipped for a meaningless sign.
fn component_volume_tolerance(points: &[Vector3], facets: &[[u32; 3]], component: &[usize]) -> f64 {
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &facet_index in component {
        for &point_index in &facets[facet_index] {
            let point = points[point_index as usize];
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }
    }
    let diagonal = max.sub(&min).length().max(1.0);
    1e-12 * diagonal * diagonal * diagonal
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_types::{Placement, Rotation};

    #[test]
    fn area_of_a_plane_and_a_box() {
        let plane = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        assert_eq!(plane.area(), 1.0);
        assert_eq!(Mesh::box_(1.0, 1.0, 1.0).unwrap().area(), 6.0);
        assert_eq!(Mesh::empty().area(), 0.0);
    }

    #[test]
    fn duplicated_point_and_facet_are_merged() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let original_volume = mesh.volume();

        // A near-coincident copy of point 0 and a facet that becomes a copy of
        // facet 0 once the points are welded.
        let mut points = mesh.points().to_vec();
        let duplicate = points[0].add(&Vector3::new(1e-9, 0.0, 0.0));
        points.push(duplicate);
        let duplicate_index = (points.len() - 1) as u32;
        let mut facets = mesh.facets().to_vec();
        facets.push([duplicate_index, 2, 1]);
        mesh = Mesh::new(points, facets).unwrap();

        let placement = Placement::new(Vector3::new(3.0, 4.0, 5.0), Rotation::identity());
        mesh.set_placement(placement);

        assert!(mesh.remove_duplicated_points());
        assert_eq!(mesh.count_points(), 8);
        assert_eq!(mesh.count_facets(), 13);
        assert!(mesh.remove_duplicated_facets());
        assert_eq!(mesh.count_facets(), 12);
        assert_eq!(mesh.volume(), original_volume);
        assert_eq!(mesh.placement(), placement);

        // Both operations are idempotent.
        assert!(!mesh.remove_duplicated_points());
        assert!(!mesh.remove_duplicated_facets());
    }

    #[test]
    fn a_collapsing_facet_is_dropped_by_the_point_merge() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let mut points = mesh.points().to_vec();
        points.push(points[0]);
        let duplicate = (points.len() - 1) as u32;
        let mut facets = mesh.facets().to_vec();
        // (0, duplicate, 2) is a genuine triangle before the merge and collapses
        // to a repeated index after it.
        facets.push([0, duplicate, 2]);
        mesh = Mesh::new(points, facets).unwrap();

        assert!(mesh.remove_duplicated_points());
        assert_eq!(mesh.count_points(), 8);
        assert_eq!(mesh.count_facets(), 12);
        assert!(mesh.is_valid());
    }

    #[test]
    fn degenerate_facets_are_removed() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let original_volume = mesh.volume();
        let mut points = mesh.points().to_vec();
        points.push(Vector3::new(0.0, 0.0, 0.0));
        points.push(Vector3::new(1.0, 0.0, 0.0));
        points.push(Vector3::new(2.0, 0.0, 0.0));
        let base = (points.len() - 3) as u32;
        let mut facets = mesh.facets().to_vec();
        facets.push([base, base + 1, base + 2]);
        mesh = Mesh::new(points, facets).unwrap();
        assert_eq!(mesh.count_facets(), 13);

        assert!(mesh.fix_degenerations());
        assert_eq!(mesh.count_facets(), 12);
        assert_eq!(mesh.volume(), original_volume);
        assert!(!mesh.fix_degenerations());
    }

    #[test]
    fn harmonize_restores_a_flipped_box() {
        let box_mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let mut facets = box_mesh.facets().to_vec();
        facets[0] = [facets[0][0], facets[0][2], facets[0][1]];
        let mut mesh = Mesh::new(box_mesh.points().to_vec(), facets).unwrap();
        assert!(mesh.has_non_uniform_oriented_facets());

        assert!(mesh.harmonize_normals());
        assert!(!mesh.has_non_uniform_oriented_facets());
        assert!(mesh.signed_volume() > 0.0);
        assert_eq!(mesh.volume(), 1.0);
        assert!(!mesh.harmonize_normals());
    }

    #[test]
    fn harmonize_keeps_a_correct_box_unchanged() {
        let mut mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        assert!(!mesh.harmonize_normals());
        assert!(mesh.signed_volume() > 0.0);
    }
}

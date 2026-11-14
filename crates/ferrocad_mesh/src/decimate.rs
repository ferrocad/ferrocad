//! Quadric-error edge-collapse decimation (FreeCAD `Mesh.decimate`).
//!
//! `Mesh.decimate(tolerance, reduction)` is upstream's `MeshCore::MeshSimplify`:
//! collapse edges cheapest-first under a quadric error metric until the facet
//! count reaches the requested reduction or the cheapest collapse costs more
//! than the tolerance.
//!
//! The error metric is the standard Garland–Heckbert quadric: each facet
//! contributes the plane quadric `n nᵀ` scaled by its area, a point carries the
//! sum of its incident facets, and an edge `(u, v)` is scored by evaluating the
//! combined quadric at the point that minimises it. The value is a *squared*
//! distance (`mm²`) and — as upstream does with its quadric error — it is
//! compared against `tolerance` as given, not `tolerance²`.
//!
//! A collapse is only applied when it preserves the mesh's structure:
//!
//! - the edge's two endpoints share one or two facets (a manifold edge),
//! - the *link condition* holds (the only shared neighbours are the third
//!   corners of those facets), so no non-manifold edge or duplicate facet
//!   appears,
//! - no surviving facet flips or degenerates when its corner moves,
//! - neither endpoint lies on a boundary (an open mesh keeps its rim), which
//!   also keeps a border edge from being collapsed shut.
//!
//! The result is rebuilt through `Mesh::replace_topology`, so it passes the same
//! structural validation as any other mesh and keeps the placement.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use ferrocad_types::Vector3;

use crate::Mesh;

/// A symmetric 4×4 quadric `xᵀQx`, stored as its ten independent coefficients.
#[derive(Clone, Copy, Default)]
struct Quadric {
    a2: f64,
    ab: f64,
    ac: f64,
    ad: f64,
    b2: f64,
    bc: f64,
    bd: f64,
    c2: f64,
    cd: f64,
    d2: f64,
}

impl Quadric {
    /// The plane quadric of a facet: `area · n nᵀ` for the unit normal `n` and
    /// the plane offset `d` (`n · x + d = 0`).
    fn from_plane(normal: Vector3, offset: f64, area: f64) -> Self {
        let (a, b, c, d) = (normal.x, normal.y, normal.z, offset);
        Quadric {
            a2: area * a * a,
            ab: area * a * b,
            ac: area * a * c,
            ad: area * a * d,
            b2: area * b * b,
            bc: area * b * c,
            bd: area * b * d,
            c2: area * c * c,
            cd: area * c * d,
            d2: area * d * d,
        }
    }

    fn add(&mut self, other: &Quadric) {
        self.a2 += other.a2;
        self.ab += other.ab;
        self.ac += other.ac;
        self.ad += other.ad;
        self.b2 += other.b2;
        self.bc += other.bc;
        self.bd += other.bd;
        self.c2 += other.c2;
        self.cd += other.cd;
        self.d2 += other.d2;
    }

    /// `pᵀ Q p`, the area-weighted squared distance to the incident planes.
    fn error(&self, p: Vector3) -> f64 {
        let (x, y, z) = (p.x, p.y, p.z);
        self.a2 * x * x
            + 2.0 * self.ab * x * y
            + 2.0 * self.ac * x * z
            + 2.0 * self.ad * x
            + self.b2 * y * y
            + 2.0 * self.bc * y * z
            + 2.0 * self.bd * y
            + self.c2 * z * z
            + 2.0 * self.cd * z
            + self.d2
    }

    /// The point minimising the quadric, or `fallback` when the 3×3 system is
    /// singular (a flat or open neighbourhood leaves a direction unconstrained).
    fn minimiser(&self, fallback: Vector3) -> Vector3 {
        let mut rows = [
            [self.a2, self.ab, self.ac, -self.ad],
            [self.ab, self.b2, self.bc, -self.bd],
            [self.ac, self.bc, self.c2, -self.cd],
        ];
        // Gaussian elimination with partial pivoting.
        let scale = rows
            .iter()
            .flat_map(|row| row[..3].iter())
            .fold(0.0_f64, |acc, value| acc.max(value.abs()));
        if !(scale > 0.0) || !scale.is_finite() {
            return fallback;
        }
        for column in 0..3 {
            let mut pivot = column;
            for row in column + 1..3 {
                if rows[row][column].abs() > rows[pivot][column].abs() {
                    pivot = row;
                }
            }
            if rows[pivot][column].abs() <= scale * 1e-12 {
                return fallback;
            }
            rows.swap(column, pivot);
            let divisor = rows[column][column];
            for value in column..4 {
                rows[column][value] /= divisor;
            }
            for row in 0..3 {
                if row == column {
                    continue;
                }
                let factor = rows[row][column];
                if factor == 0.0 {
                    continue;
                }
                for value in column..4 {
                    rows[row][value] -= factor * rows[column][value];
                }
            }
        }
        let point = Vector3::new(rows[0][3], rows[1][3], rows[2][3]);
        if point.x.is_finite() && point.y.is_finite() && point.z.is_finite() {
            point
        } else {
            fallback
        }
    }
}

/// The unit normal and offset of a facet plane, with its area, or `None` for a
/// degenerate facet.
fn facet_plane(a: Vector3, b: Vector3, c: Vector3) -> Option<(Vector3, f64, f64)> {
    let cross = b.sub(&a).cross(&c.sub(&a));
    let twice_area = cross.length();
    if !(twice_area > 0.0) || !twice_area.is_finite() {
        return None;
    }
    let normal = cross.scale(1.0 / twice_area);
    Some((normal, -normal.dot(&a), twice_area * 0.5))
}

/// A queued edge, ordered cheapest-first. The stamps make a stale entry
/// detectable, so a collapse never needs a decrease-key operation.
struct Candidate {
    cost: f64,
    u: u32,
    v: u32,
    stamp_u: u32,
    stamp_v: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: `BinaryHeap` is a max-heap, and the cheapest edge is next.
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.u.cmp(&self.u))
            .then_with(|| other.v.cmp(&self.v))
    }
}

impl Mesh {
    /// Collapse edges until `reduction` of the facets are gone (upstream
    /// `Mesh.decimate`).
    ///
    /// `tolerance` is the largest quadric error a collapse may introduce;
    /// `reduction` is the fraction of facets to remove, clamped to `0..=1`, and
    /// the mesh never drops below four facets. Collapsed points are compacted
    /// away and any facet groups are dropped, because the surviving facets are
    /// renumbered; the placement is kept.
    ///
    /// Returns whether the topology changed.
    pub fn decimate(&mut self, tolerance: f64, reduction: f64) -> bool {
        let original = self.facets().len();
        if original == 0 || !tolerance.is_finite() || tolerance <= 0.0 {
            return false;
        }
        if !reduction.is_finite() {
            return false;
        }
        let reduction = reduction.clamp(0.0, 1.0);
        let target = (((1.0 - reduction) * original as f64).round() as usize).max(4);
        if target >= original {
            return false;
        }

        let mut simplifier = Simplifier::new(self.points().to_vec(), self.facets().to_vec());
        simplifier.run(tolerance, target);
        if simplifier.alive_facets >= original {
            return false;
        }
        let (points, facets) = simplifier.finish();
        self.replace_topology(points, facets);
        true
    }
}

/// The mutable working state of one decimation pass.
struct Simplifier {
    points: Vec<Vector3>,
    facets: Vec<[u32; 3]>,
    face_alive: Vec<bool>,
    alive_facets: usize,
    /// Live facets incident to each point.
    vertex_faces: Vec<Vec<usize>>,
    quadric: Vec<Quadric>,
    alive: Vec<bool>,
    /// Points on a boundary edge: the rim of an open mesh is not collapsed.
    locked: Vec<bool>,
    /// Bumped whenever a point's position or quadric changes, so a queued edge
    /// that names an outdated endpoint is skipped.
    stamp: Vec<u32>,
}

impl Simplifier {
    fn new(points: Vec<Vector3>, facets: Vec<[u32; 3]>) -> Self {
        let count = points.len();
        let mut simplifier = Simplifier {
            vertex_faces: vec![Vec::new(); count],
            quadric: vec![Quadric::default(); count],
            alive: vec![true; count],
            locked: vec![false; count],
            stamp: vec![0; count],
            alive_facets: facets.len(),
            face_alive: vec![true; facets.len()],
            points,
            facets,
        };

        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for (index, facet) in simplifier.facets.iter().enumerate() {
            let corners = [
                simplifier.points[facet[0] as usize],
                simplifier.points[facet[1] as usize],
                simplifier.points[facet[2] as usize],
            ];
            if let Some((normal, offset, area)) = facet_plane(corners[0], corners[1], corners[2]) {
                let quadric = Quadric::from_plane(normal, offset, area);
                for corner in facet {
                    simplifier.quadric[*corner as usize].add(&quadric);
                    simplifier.vertex_faces[*corner as usize].push(index);
                }
            }
            for (a, b) in edges_of(facet) {
                let key = if a < b { (a, b) } else { (b, a) };
                *uses.entry(key).or_insert(0) += 1;
            }
        }
        // Anything that is not a clean two-facet edge (a rim, or an over-used
        // non-manifold edge) anchors its endpoints.
        for (key, uses) in uses {
            if uses != 2 {
                simplifier.locked[key.0 as usize] = true;
                simplifier.locked[key.1 as usize] = true;
            }
        }
        simplifier
    }

    /// The edges of the live facets, once each.
    fn edges(&self) -> Vec<(u32, u32)> {
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut edges = Vec::new();
        for (index, facet) in self.facets.iter().enumerate() {
            if !self.face_alive[index] {
                continue;
            }
            for (a, b) in edges_of(facet) {
                let key = if a < b { (a, b) } else { (b, a) };
                if seen.insert(key) {
                    edges.push(key);
                }
            }
        }
        edges
    }

    /// The live facets around `point`.
    fn faces_of(&self, point: u32) -> impl Iterator<Item = usize> + '_ {
        self.vertex_faces[point as usize]
            .iter()
            .copied()
            .filter(|index| self.face_alive[*index])
    }

    /// The points sharing a live facet with `point`.
    fn neighbours_of(&self, point: u32) -> HashSet<u32> {
        let mut neighbours = HashSet::new();
        for index in self.faces_of(point) {
            for corner in self.facets[index] {
                if corner != point {
                    neighbours.insert(corner);
                }
            }
        }
        neighbours
    }

    /// The merged quadric's minimising point and error for edge `(u, v)`.
    fn candidate(&self, u: u32, v: u32) -> (f64, Vector3) {
        let mut quadric = self.quadric[u as usize];
        quadric.add(&self.quadric[v as usize]);
        let a = self.points[u as usize];
        let b = self.points[v as usize];
        let midpoint = a.add(&b).scale(0.5);
        let point = quadric.minimiser(midpoint);
        // A minimiser far outside the edge would fold the neighbourhood; the
        // midpoint is the safe placement when the quadric is flat (two planes)
        // or badly conditioned.
        let edge = a.sub(&b).length().max(1e-9);
        let point = if point.sub(&midpoint).length() > edge * 4.0 {
            midpoint
        } else {
            point
        };
        (quadric.error(point), point)
    }

    /// Push every edge at `point` into the queue with the current stamps.
    fn requeue(&mut self, point: u32, heap: &mut BinaryHeap<Candidate>) {
        for neighbour in self.neighbours_of(point) {
            if !self.alive[neighbour as usize] {
                continue;
            }
            let (low, high) = if point < neighbour {
                (point, neighbour)
            } else {
                (neighbour, point)
            };
            let (cost, _) = self.candidate(low, high);
            heap.push(Candidate {
                cost,
                u: low,
                v: high,
                stamp_u: self.stamp[low as usize],
                stamp_v: self.stamp[high as usize],
            });
        }
    }

    fn run(&mut self, tolerance: f64, target: usize) {
        let mut heap = BinaryHeap::new();
        for (u, v) in self.edges() {
            let (cost, _) = self.candidate(u, v);
            heap.push(Candidate {
                cost,
                u,
                v,
                stamp_u: self.stamp[u as usize],
                stamp_v: self.stamp[v as usize],
            });
        }
        while self.alive_facets > target {
            let Some(entry) = heap.pop() else {
                break;
            };
            if !self.alive[entry.u as usize] || !self.alive[entry.v as usize] {
                continue;
            }
            if self.stamp[entry.u as usize] != entry.stamp_u
                || self.stamp[entry.v as usize] != entry.stamp_v
            {
                continue;
            }
            // The queue is cheapest-first, so the first current entry that
            // costs more than the tolerance ends the pass.
            if entry.cost > tolerance {
                break;
            }
            if self.collapse(entry.u, entry.v) {
                self.requeue(entry.u, &mut heap);
            }
        }
    }

    /// Collapse `v` into `u` if the mesh stays structurally sound.
    fn collapse(&mut self, u: u32, v: u32) -> bool {
        if self.locked[u as usize] || self.locked[v as usize] {
            return false;
        }
        let faces_u: Vec<usize> = self.faces_of(u).collect();
        let faces_v: Vec<usize> = self.faces_of(v).collect();

        // The facets the edge belongs to, and the corner opposite the edge.
        let mut shared: Vec<(usize, u32)> = Vec::new();
        for index in &faces_u {
            if faces_v.contains(index) {
                match self.facets[*index]
                    .iter()
                    .copied()
                    .find(|corner| *corner != u && *corner != v)
                {
                    Some(third) => shared.push((*index, third)),
                    None => return false,
                }
            }
        }
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }

        // Link condition: two points that share more than the corners of the
        // shared facets would fold two sheets together into a non-manifold edge.
        let expected: HashSet<u32> = shared.iter().map(|(_, third)| *third).collect();
        let neighbours_u = self.neighbours_of(u);
        let neighbours_v = self.neighbours_of(v);
        for common in neighbours_u.intersection(&neighbours_v) {
            if !expected.contains(common) {
                return false;
            }
        }

        let (cost, merged) = self.candidate(u, v);
        if !cost.is_finite() {
            return false;
        }

        // Every surviving facet around the edge must keep its orientation and
        // stay non-degenerate.
        for index in faces_u.iter().chain(faces_v.iter()) {
            if shared.iter().any(|(shared_index, _)| shared_index == index) {
                continue;
            }
            let facet = self.facets[*index];
            let old = [
                self.points[facet[0] as usize],
                self.points[facet[1] as usize],
                self.points[facet[2] as usize],
            ];
            let new = facet.map(|corner| {
                if corner == u || corner == v {
                    merged
                } else {
                    self.points[corner as usize]
                }
            });
            let Some((old_normal, _, old_area)) = facet_plane(old[0], old[1], old[2]) else {
                return false;
            };
            let Some((new_normal, _, new_area)) = facet_plane(new[0], new[1], new[2]) else {
                return false;
            };
            if new_area <= old_area * 1e-9 || new_normal.dot(&old_normal) <= 0.0 {
                return false;
            }
        }

        // Apply: move `u` to the merged point, redirect `v`'s facets to it and
        // drop the shared ones.
        self.points[u as usize] = merged;
        let merged_quadric = self.quadric[v as usize];
        self.quadric[u as usize].add(&merged_quadric);
        self.alive[v as usize] = false;
        self.stamp[u as usize] = self.stamp[u as usize].wrapping_add(1);
        for (index, _) in &shared {
            if self.face_alive[*index] {
                self.face_alive[*index] = false;
                self.alive_facets -= 1;
            }
        }
        for index in &faces_v {
            if !self.face_alive[*index] {
                continue;
            }
            for corner in self.facets[*index].iter_mut() {
                if *corner == v {
                    *corner = u;
                }
            }
        }
        // Rebuild `u`'s adjacency so it holds exactly the live facets once each;
        // it otherwise accumulates the dead shared facets and `v`'s old list.
        let mut faces: Vec<usize> = faces_u
            .iter()
            .chain(faces_v.iter())
            .copied()
            .filter(|index| self.face_alive[*index])
            .collect();
        faces.sort_unstable();
        faces.dedup();
        self.vertex_faces[u as usize] = faces;
        true
    }

    /// Compact away the collapsed points and return the surviving topology.
    fn finish(self) -> (Vec<Vector3>, Vec<[u32; 3]>) {
        let mut remap = vec![u32::MAX; self.points.len()];
        let mut points = Vec::new();
        for (facet, alive) in self.facets.iter().zip(self.face_alive.iter()) {
            if !alive {
                continue;
            }
            for corner in facet {
                if remap[*corner as usize] == u32::MAX {
                    remap[*corner as usize] = points.len() as u32;
                    points.push(self.points[*corner as usize]);
                }
            }
        }
        let facets = self
            .facets
            .iter()
            .zip(self.face_alive.iter())
            .filter(|(_, alive)| **alive)
            .map(|(facet, _)| facet.map(|corner| remap[corner as usize]))
            .collect();
        (points, facets)
    }
}

/// The three undirected corners of a facet as `(a, b)` pairs.
fn edges_of([a, b, c]: &[u32; 3]) -> [(u32, u32); 3] {
    [(*a, *b), (*b, *c), (*c, *a)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed octahedron of 8 triangles: enough topology to decimate.
    fn octahedron() -> Mesh {
        let points = vec![
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, -1.0),
        ];
        let facets = vec![
            [0, 2, 4],
            [2, 1, 4],
            [1, 3, 4],
            [3, 0, 4],
            [2, 0, 5],
            [1, 2, 5],
            [3, 1, 5],
            [0, 3, 5],
        ];
        Mesh::new(points, facets).expect("octahedron")
    }

    #[test]
    fn a_generous_tolerance_reduces_a_closed_mesh_and_keeps_it_valid() {
        let mut mesh = octahedron();
        let before = mesh.count_facets();
        let points = mesh.count_points();
        let placement = mesh.placement();
        assert!(mesh.decimate(1.0e6, 0.5));
        assert!(mesh.count_facets() < before, "the reduction target is met");
        assert!(mesh.count_points() < points, "collapsed points are compacted");
        assert!(mesh.is_solid(), "the surface stays closed");
        assert!(mesh.is_valid(), "the result passes the structural check");
        assert_eq!(mesh.placement(), placement, "the placement is kept");
    }

    #[test]
    fn a_bordered_plane_keeps_its_rim() {
        // Two quads (four triangles) in the z = 0 plane. Every corner lies on
        // the rim, and the interior edge's endpoints do too, so nothing may
        // collapse and the call reports that.
        let points = vec![
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 1.0, 0.0),
            Vector3::new(2.0, 1.0, 0.0),
        ];
        let facets = vec![[0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4]];
        let mut mesh = Mesh::new(points, facets).expect("grid");
        assert!(!mesh.decimate(1.0, 0.5));
        assert_eq!(mesh.count_facets(), 4);
        assert_eq!(mesh.count_points(), 6);
    }

    #[test]
    fn a_zero_or_negative_tolerance_changes_nothing() {
        let mut mesh = octahedron();
        assert!(!mesh.decimate(0.0, 1.0));
        assert!(!mesh.decimate(-1.0, 1.0));
        assert_eq!(mesh.count_facets(), 8);
    }

    #[test]
    fn a_tight_tolerance_stops_the_reduction_early() {
        let mut mesh = octahedron();
        // The octahedron's edges are ~1.4 long; a tiny error budget admits no
        // collapse at all.
        assert!(!mesh.decimate(1e-9, 1.0));
        assert_eq!(mesh.count_facets(), 8);
    }

    #[test]
    fn facet_groups_are_dropped_when_facets_are_renumbered() {
        let source = octahedron();
        let mut mesh = Mesh::from_segmented_facets(
            &[(source.points().to_vec(), source.facets().to_vec())],
            0.0,
        )
        .expect("segmented octahedron");
        assert_eq!(mesh.count_segments(), 1);
        assert!(mesh.decimate(1.0e6, 0.5));
        assert_eq!(
            mesh.count_segments(),
            0,
            "a renumbered facet list cannot keep its groups"
        );
        assert!(mesh.is_solid());
    }
}

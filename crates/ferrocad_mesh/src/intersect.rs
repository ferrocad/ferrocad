//! Triangle-pair self-intersection detection.
//!
//! The public surface matches upstream: [`Mesh::has_self_intersections`] is a
//! cheap boolean and [`Mesh::self_intersections`] reports each offending pair
//! with two **local** intersection points.
//!
//! # Acceleration
//!
//! A uniform grid is built over the facets' axis-aligned bounding boxes. Each
//! facet is tested only against the facets already occupying the cells its AABB
//! covers, so a large mesh is not scanned pairwise. A facet whose AABB would
//! cover more than [`MAX_CELLS_PER_TRIANGLE`] cells (a very large or very
//! oblique triangle) is instead tested against every other facet; that keeps the
//! grid from exploding at the cost of an `O(n)` scan for that one triangle.
//!
//! Facets that share a point index are adjacent, not self-intersecting, and are
//! skipped.

use std::collections::{HashMap, HashSet};

use ferrocad_types::Vector3;

use crate::{BoundBox, Mesh};

/// One pair of intersecting facets and two points on their intersection.
///
/// `first` and `second` are 0-based facet indices with `first < second`; the
/// points are on the mesh's local (unplaced) coordinates. The points are ordered
/// along the intersection segment; they coincide when the triangles only touch
/// at a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfIntersection {
    /// The lower-index facet.
    pub first: usize,
    /// The higher-index facet.
    pub second: usize,
    /// Two points on the intersection segment, ordered along it.
    pub points: [Vector3; 2],
}

/// Above this many AABB cells, a facet skips the grid and is scanned directly.
const MAX_CELLS_PER_TRIANGLE: usize = 512;

impl Mesh {
    /// Whether any two non-adjacent facets intersect (upstream
    /// `hasSelfIntersections()`).
    pub fn has_self_intersections(&self) -> bool {
        let mut found = false;
        self.for_each_self_intersection(|_, _, _| {
            found = true;
            false
        });
        found
    }

    /// Every intersecting facet pair, sorted by `(first, second)` (upstream
    /// `getSelfIntersections()`).
    ///
    /// Adjacent facets (any shared point index) are excluded. The reported points
    /// are local, like [`Mesh::points`].
    pub fn self_intersections(&self) -> Vec<SelfIntersection> {
        let mut intersections = Vec::new();
        self.for_each_self_intersection(|first, second, points| {
            intersections.push(SelfIntersection {
                first,
                second,
                points,
            });
            true
        });
        intersections.sort_by_key(|intersection| (intersection.first, intersection.second));
        intersections
    }

    /// Walk candidate pairs, stopping when `visit` returns `false`.
    fn for_each_self_intersection(
        &self,
        mut visit: impl FnMut(usize, usize, [Vector3; 2]) -> bool,
    ) {
        let count = self.count_facets();
        if count < 2 {
            return;
        }

        let bounds = self.bounds();
        let coordinate_scale = bounds
            .min
            .x
            .abs()
            .max(bounds.min.y.abs())
            .max(bounds.min.z.abs())
            .max(bounds.max.x.abs())
            .max(bounds.max.y.abs())
            .max(bounds.max.z.abs())
            .max(1.0);
        let eps = 1e-15 * coordinate_scale + 1e-12;

        let facets = self.facets();
        let points = self.points();
        let triangles: Vec<Triangle> = facets
            .iter()
            .map(|facet| {
                Triangle::new([
                    points[facet[0] as usize],
                    points[facet[1] as usize],
                    points[facet[2] as usize],
                ])
            })
            .collect();

        let grid = Grid::new(&bounds, count);
        let mut buckets: HashMap<[usize; 3], Vec<u32>> = HashMap::new();
        let mut checked: HashSet<(u32, u32)> = HashSet::new();
        let mut oversized: Vec<usize> = Vec::new();

        for index in 0..count {
            let triangle = &triangles[index];
            let (low, high) = grid.range(triangle);
            let cells = (high[0] - low[0] + 1)
                .saturating_mul(high[1] - low[1] + 1)
                .saturating_mul(high[2] - low[2] + 1);

            if cells > MAX_CELLS_PER_TRIANGLE {
                // No grid entry: scanning all previous facets also covers the
                // facets that arrived earlier through the grid.
                for previous in 0..index {
                    if !test_pair(
                        previous,
                        index,
                        &triangles,
                        facets,
                        &mut checked,
                        eps,
                        &mut visit,
                    ) {
                        return;
                    }
                }
                oversized.push(index);
                continue;
            }

            for x in low[0]..=high[0] {
                for y in low[1]..=high[1] {
                    for z in low[2]..=high[2] {
                        if let Some(bucket) = buckets.get(&[x, y, z]) {
                            for &previous in bucket {
                                if !test_pair(
                                    previous as usize,
                                    index,
                                    &triangles,
                                    facets,
                                    &mut checked,
                                    eps,
                                    &mut visit,
                                ) {
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            // Facets that skipped the grid are not in any bucket, so normal
            // facets have to check them explicitly.
            for &previous in &oversized {
                if !test_pair(
                    previous,
                    index,
                    &triangles,
                    facets,
                    &mut checked,
                    eps,
                    &mut visit,
                ) {
                    return;
                }
            }

            for x in low[0]..=high[0] {
                for y in low[1]..=high[1] {
                    for z in low[2]..=high[2] {
                        buckets.entry([x, y, z]).or_default().push(index as u32);
                    }
                }
            }
        }
    }
}

/// Test one pair once, honouring adjacency and AABB rejection.
fn test_pair(
    first: usize,
    second: usize,
    triangles: &[Triangle],
    facets: &[[u32; 3]],
    checked: &mut HashSet<(u32, u32)>,
    eps: f64,
    visit: &mut impl FnMut(usize, usize, [Vector3; 2]) -> bool,
) -> bool {
    if !checked.insert((first as u32, second as u32)) {
        return true;
    }
    if !triangles[first].overlaps(&triangles[second], eps) {
        return true;
    }
    if facets_adjacent(facets[first], facets[second]) {
        return true;
    }
    match triangle_intersection(&triangles[first], &triangles[second], eps) {
        Some(points) => visit(first, second, points),
        None => true,
    }
}

/// Two facets are adjacent when they address at least one common point.
fn facets_adjacent(first: [u32; 3], second: [u32; 3]) -> bool {
    first.iter().any(|index| second.contains(index))
}

/// A triangle with its plane normal and cached AABB.
#[derive(Clone, Copy)]
struct Triangle {
    points: [Vector3; 3],
    normal: Vector3,
    normal_length: f64,
    min: Vector3,
    max: Vector3,
}

impl Triangle {
    fn new(points: [Vector3; 3]) -> Self {
        let normal = points[1].sub(&points[0]).cross(&points[2].sub(&points[0]));
        let min = Vector3::new(
            points[0].x.min(points[1].x).min(points[2].x),
            points[0].y.min(points[1].y).min(points[2].y),
            points[0].z.min(points[1].z).min(points[2].z),
        );
        let max = Vector3::new(
            points[0].x.max(points[1].x).max(points[2].x),
            points[0].y.max(points[1].y).max(points[2].y),
            points[0].z.max(points[1].z).max(points[2].z),
        );
        Triangle {
            points,
            normal,
            normal_length: normal.length(),
            min,
            max,
        }
    }

    fn overlaps(&self, other: &Triangle, eps: f64) -> bool {
        self.min.x <= other.max.x + eps
            && other.min.x <= self.max.x + eps
            && self.min.y <= other.max.y + eps
            && other.min.y <= self.max.y + eps
            && self.min.z <= other.max.z + eps
            && other.min.z <= self.max.z + eps
    }
}

/// The uniform AABB grid.
#[derive(Clone, Copy)]
struct Grid {
    origin: Vector3,
    cell: [f64; 3],
    dimensions: [usize; 3],
}

impl Grid {
    fn new(bounds: &BoundBox, facet_count: usize) -> Self {
        let target = (facet_count as f64).cbrt().ceil().max(1.0) as usize;
        let extent = bounds.max.sub(&bounds.min);
        let extents = [extent.x, extent.y, extent.z];
        let mut cell = [1.0f64; 3];
        let mut dimensions = [1usize; 3];
        for axis in 0..3 {
            if extents[axis].is_finite() && extents[axis] > 0.0 {
                dimensions[axis] = target.max(1);
                cell[axis] = extents[axis] / dimensions[axis] as f64;
            }
        }
        Grid {
            origin: bounds.min,
            cell,
            dimensions,
        }
    }

    fn cell_of(&self, point: Vector3) -> [usize; 3] {
        let coordinates = [point.x, point.y, point.z];
        let origins = [self.origin.x, self.origin.y, self.origin.z];
        let mut cell = [0usize; 3];
        for axis in 0..3 {
            let raw = (coordinates[axis] - origins[axis]) / self.cell[axis];
            let clamped = if raw.is_finite() {
                raw.floor().clamp(0.0, (self.dimensions[axis] - 1) as f64)
            } else {
                0.0
            };
            cell[axis] = clamped as usize;
        }
        cell
    }

    fn range(&self, triangle: &Triangle) -> ([usize; 3], [usize; 3]) {
        (self.cell_of(triangle.min), self.cell_of(triangle.max))
    }
}

/// The intersection segment of two non-degenerate triangles, if any.
fn triangle_intersection(first: &Triangle, second: &Triangle, eps: f64) -> Option<[Vector3; 2]> {
    if first.normal_length <= 0.0 || second.normal_length <= 0.0 {
        return None;
    }
    let first_normal = first.normal.scale(1.0 / first.normal_length);
    let second_normal = second.normal.scale(1.0 / second.normal_length);

    let second_to_first = plane_distances(&first_normal, &first.points[0], &second.points);
    let first_to_second = plane_distances(&second_normal, &second.points[0], &first.points);
    if same_side(&second_to_first, eps) || same_side(&first_to_second, eps) {
        return None;
    }

    let line = first_normal.cross(&second_normal);
    if line.length() <= 1e-12 {
        // Parallel planes; they can only meet when they are the same plane.
        return if second_to_first.iter().all(|distance| distance.abs() <= eps) {
            coplanar_intersection(first, second, first_normal, eps)
        } else {
            None
        };
    }

    let first_clip = clip_by_plane(first, &second_normal, &second.points[0], eps)?;
    let second_clip = clip_by_plane(second, &first_normal, &first.points[0], eps)?;

    let direction = line.normalize();
    let (first_low, first_high) =
        ordered(first_clip.0.dot(&direction), first_clip.1.dot(&direction));
    let (second_low, second_high) =
        ordered(second_clip.0.dot(&direction), second_clip.1.dot(&direction));
    let low = first_low.max(second_low);
    let high = first_high.min(second_high);
    if high < low - eps {
        return None;
    }

    let anchor = first_clip.0;
    let anchor_parameter = anchor.dot(&direction);
    Some([
        anchor.add(&direction.scale(low - anchor_parameter)),
        anchor.add(&direction.scale(high - anchor_parameter)),
    ])
}

/// Signed distances of `points` to the plane `normal . (x - origin)`.
fn plane_distances(normal: &Vector3, origin: &Vector3, points: &[Vector3; 3]) -> [f64; 3] {
    [
        normal.dot(&points[0].sub(origin)),
        normal.dot(&points[1].sub(origin)),
        normal.dot(&points[2].sub(origin)),
    ]
}

fn same_side(distances: &[f64; 3], eps: f64) -> bool {
    distances.iter().all(|&distance| distance > eps)
        || distances.iter().all(|&distance| distance < -eps)
}

fn ordered(first: f64, second: f64) -> (f64, f64) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

/// The part of `triangle` lying on the plane, as a segment (or a point).
fn clip_by_plane(
    triangle: &Triangle,
    normal: &Vector3,
    origin: &Vector3,
    eps: f64,
) -> Option<(Vector3, Vector3)> {
    let distances = plane_distances(normal, origin, &triangle.points);
    if same_side(&distances, eps) {
        return None;
    }

    let mut contacts: Vec<Vector3> = Vec::with_capacity(4);
    for index in 0..3 {
        if distances[index].abs() <= eps {
            contacts.push(triangle.points[index]);
        }
        let next = (index + 1) % 3;
        let from = distances[index];
        let to = distances[next];
        if (from > eps && to < -eps) || (from < -eps && to > eps) {
            let t = from / (from - to);
            contacts.push(
                triangle.points[index]
                    .add(&triangle.points[next].sub(&triangle.points[index]).scale(t)),
            );
        }
    }
    dedupe(&mut contacts, eps);

    match contacts.len() {
        0 => None,
        1 => Some((contacts[0], contacts[0])),
        _ => {
            let mut best = (contacts[0], contacts[1]);
            let mut best_distance = best.0.distance(&best.1);
            for first in 0..contacts.len() {
                for second in (first + 1)..contacts.len() {
                    let distance = contacts[first].distance(&contacts[second]);
                    if distance > best_distance {
                        best_distance = distance;
                        best = (contacts[first], contacts[second]);
                    }
                }
            }
            Some(best)
        }
    }
}

/// Remove contacts within `eps` of an earlier one.
fn dedupe(points: &mut Vec<Vector3>, eps: f64) {
    let mut unique: Vec<Vector3> = Vec::with_capacity(points.len());
    for &point in points.iter() {
        if !unique
            .iter()
            .any(|existing| existing.distance(&point) <= eps)
        {
            unique.push(point);
        }
    }
    *points = unique;
}

type Point2 = (f64, f64);

/// Overlap of two coplanar triangles, as two points of their common region.
fn coplanar_intersection(
    first: &Triangle,
    second: &Triangle,
    normal: Vector3,
    eps: f64,
) -> Option<[Vector3; 2]> {
    let axis = dominant_axis(&normal);
    let projected_first = first.points.map(|point| project(point, axis));
    let projected_second = second.points.map(|point| project(point, axis));

    let mut contacts: Vec<Vector3> = Vec::with_capacity(8);
    for first_edge in 0..3 {
        let next = (first_edge + 1) % 3;
        for second_edge in 0..3 {
            let second_next = (second_edge + 1) % 3;
            if let Some(t) = segment_intersection_2d(
                projected_first[first_edge],
                projected_first[next],
                projected_second[second_edge],
                projected_second[second_next],
                eps,
            ) {
                let from = first.points[first_edge];
                let to = first.points[next];
                contacts.push(from.add(&to.sub(&from).scale(t)));
            }
        }
    }
    for (index, point) in projected_first.iter().enumerate() {
        if point_in_triangle(
            *point,
            projected_second[0],
            projected_second[1],
            projected_second[2],
            eps,
        ) {
            contacts.push(first.points[index]);
        }
    }
    for (index, point) in projected_second.iter().enumerate() {
        if point_in_triangle(
            *point,
            projected_first[0],
            projected_first[1],
            projected_first[2],
            eps,
        ) {
            contacts.push(second.points[index]);
        }
    }
    dedupe(&mut contacts, eps);

    match contacts.len() {
        0 => None,
        1 => Some([contacts[0], contacts[0]]),
        _ => {
            let mut best = (contacts[0], contacts[1]);
            let mut best_distance = best.0.distance(&best.1);
            for first in 0..contacts.len() {
                for second in (first + 1)..contacts.len() {
                    let distance = contacts[first].distance(&contacts[second]);
                    if distance > best_distance {
                        best_distance = distance;
                        best = (contacts[first], contacts[second]);
                    }
                }
            }
            Some([best.0, best.1])
        }
    }
}

/// Drop the axis with the largest normal component for a 2D projection.
fn dominant_axis(normal: &Vector3) -> usize {
    let absolute = [normal.x.abs(), normal.y.abs(), normal.z.abs()];
    if absolute[0] >= absolute[1] && absolute[0] >= absolute[2] {
        0
    } else if absolute[1] >= absolute[2] {
        1
    } else {
        2
    }
}

fn project(point: Vector3, axis: usize) -> Point2 {
    match axis {
        0 => (point.y, point.z),
        1 => (point.x, point.z),
        _ => (point.x, point.y),
    }
}

fn subtract(first: Point2, second: Point2) -> Point2 {
    (first.0 - second.0, first.1 - second.1)
}

fn cross_2d(first: Point2, second: Point2) -> f64 {
    first.0 * second.1 - first.1 * second.0
}

/// The parameter along `first_from -> first_to` where the segments cross.
fn segment_intersection_2d(
    first_from: Point2,
    first_to: Point2,
    second_from: Point2,
    second_to: Point2,
    eps: f64,
) -> Option<f64> {
    let first_direction = subtract(first_to, first_from);
    let second_direction = subtract(second_to, second_from);
    let denominator = cross_2d(first_direction, second_direction);
    // `denominator` is an area; compare it to the square of the distance
    // tolerance so the test scales with the coordinates.
    if denominator.abs() <= eps * eps {
        return None;
    }
    let offset = subtract(second_from, first_from);
    let t = cross_2d(offset, second_direction) / denominator;
    let u = cross_2d(offset, first_direction) / denominator;
    let parameter_eps = 1e-9;
    (t >= -parameter_eps
        && t <= 1.0 + parameter_eps
        && u >= -parameter_eps
        && u <= 1.0 + parameter_eps)
        .then(|| t.clamp(0.0, 1.0))
}

fn point_in_triangle(point: Point2, a: Point2, b: Point2, c: Point2, eps: f64) -> bool {
    let area_eps = eps * eps;
    let first = cross_2d(subtract(b, a), subtract(point, a));
    let second = cross_2d(subtract(c, b), subtract(point, b));
    let third = cross_2d(subtract(a, c), subtract(point, c));
    let negative = first < -area_eps || second < -area_eps || third < -area_eps;
    let positive = first > area_eps || second > area_eps || third > area_eps;
    !(negative && positive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translated_box(offset: Vector3) -> Mesh {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let points = mesh
            .points()
            .iter()
            .map(|point| point.add(&offset))
            .collect();
        Mesh::new(points, mesh.facets().to_vec()).unwrap()
    }

    fn two_box_mesh(first: Mesh, second: Mesh) -> Mesh {
        let mut points = first.points().to_vec();
        let base = points.len() as u32;
        points.extend_from_slice(second.points());
        let facets = first
            .facets()
            .iter()
            .copied()
            .chain(
                second
                    .facets()
                    .iter()
                    .map(|facet| [facet[0] + base, facet[1] + base, facet[2] + base]),
            )
            .collect();
        Mesh::new(points, facets).unwrap()
    }

    #[test]
    fn one_box_has_no_self_intersections() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        assert!(!mesh.has_self_intersections());
        assert!(mesh.self_intersections().is_empty());
    }

    #[test]
    fn penetrating_boxes_intersect_and_separated_boxes_do_not() {
        let first = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let second = translated_box(Vector3::new(0.5, 0.3, 0.2));
        let penetrating = two_box_mesh(first, second);
        assert!(penetrating.has_self_intersections());

        let intersections = penetrating.self_intersections();
        assert!(!intersections.is_empty());
        assert!(intersections.iter().all(|item| item.first < item.second
            && item.second < penetrating.count_facets()
            && !facets_adjacent(
                penetrating.facets()[item.first],
                penetrating.facets()[item.second]
            )));
        // The pairs are sorted and at least one crossing has a real segment.
        assert!(
            intersections
                .windows(2)
                .all(|pair| (pair[0].first, pair[0].second) <= (pair[1].first, pair[1].second))
        );
        assert!(
            intersections
                .iter()
                .any(|item| item.points[0].distance(&item.points[1]) > 1e-6)
        );

        let apart = two_box_mesh(
            Mesh::box_(1.0, 1.0, 1.0).unwrap(),
            translated_box(Vector3::new(10.0, 0.0, 0.0)),
        );
        assert!(!apart.has_self_intersections());
    }

    #[test]
    fn coplanar_overlap_is_reported() {
        // Two overlapping coplanar triangles with disjoint indices.
        let mesh = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
                Vector3::new(0.0, 2.0, 0.0),
                Vector3::new(0.5, 0.5, 0.0),
                Vector3::new(1.5, 0.5, 0.0),
                Vector3::new(0.5, 1.5, 0.0),
            ],
            vec![[0, 1, 2], [3, 4, 5]],
        )
        .unwrap();
        assert!(mesh.has_self_intersections());
        let intersections = mesh.self_intersections();
        assert_eq!(intersections.len(), 1);
        assert!(intersections[0].points[0].distance(&intersections[0].points[1]) > 0.0);
    }
}

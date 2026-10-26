//! The triangle-mesh payload: [`Mesh`], its validation, and its queries.
//!
//! A mesh is a point list plus triangle facets. The three indices of a facet always
//! address points of that mesh; [`Mesh::new`] validates that once and every later
//! query may rely on it. The placement is stored beside the points rather than baked
//! into them, mirroring FreeCAD: [`Mesh::bounds`] and [`Mesh::volume`] report the
//! untransformed mesh, and a consumer that displays or combines it applies the
//! placement.
//!
//! Winding follows the usual right-hand rule: a facet is outward-facing when its
//! vertices are counter-clockwise seen from outside the solid. A closed, manifold
//! mesh with that winding has a positive [`volume`](Mesh::volume).

use std::collections::HashMap;
use std::fmt;

use ferrocad_types::{Placement, Vector3};

/// An axis-aligned bounding box.
///
/// The field names are deliberately neutral: the Python binding maps [`min`](BoundBox::min)
/// and [`max`](BoundBox::max) to FreeCAD's `XMin`/`YMin`/`ZMin` and
/// `XMax`/`YMax`/`ZMax`, and [`diagonal_length`](BoundBox::diagonal_length) to
/// `DiagonalLength`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundBox {
    /// The lower corner.
    pub min: Vector3,
    /// The upper corner.
    pub max: Vector3,
}

impl BoundBox {
    /// A box spanning `min` to `max`.
    pub fn new(min: Vector3, max: Vector3) -> Self {
        BoundBox { min, max }
    }

    /// The length of the box diagonal (FreeCAD `DiagonalLength`).
    pub fn diagonal_length(&self) -> f64 {
        self.max.sub(&self.min).length()
    }
}

/// Why a mesh could not be constructed or read.
#[derive(Debug)]
pub enum MeshError {
    /// A facet references a point that does not exist.
    IndexOutOfRange {
        /// The facet that carries the bad index.
        facet: usize,
        /// The offending point index.
        index: u32,
        /// How many points the mesh has.
        point_count: usize,
    },
    /// A point coordinate is NaN or infinite.
    NonFiniteCoordinate {
        /// The point that carries the bad coordinate.
        point: usize,
    },
    /// A facet repeats one of its point indices.
    DegenerateFacet {
        /// The facet that is degenerate.
        facet: usize,
    },
    /// An STL file could not be understood.
    MalformedStl(String),
    /// Reading the file failed.
    Io(std::io::Error),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::IndexOutOfRange {
                facet,
                index,
                point_count,
            } => write!(
                f,
                "facet {facet} references point {index}, but the mesh has only {point_count} points"
            ),
            MeshError::NonFiniteCoordinate { point } => {
                write!(f, "point {point} has a non-finite coordinate")
            }
            MeshError::DegenerateFacet { facet } => {
                write!(f, "facet {facet} repeats a point index")
            }
            MeshError::MalformedStl(message) => write!(f, "malformed STL: {message}"),
            MeshError::Io(error) => write!(f, "mesh I/O error: {error}"),
        }
    }
}

impl std::error::Error for MeshError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MeshError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for MeshError {
    fn from(error: std::io::Error) -> Self {
        MeshError::Io(error)
    }
}

/// A triangle mesh with an optional placement.
///
/// The points and facets are private so that the index invariant established by
/// [`new`](Mesh::new) holds for every operation. Use [`points`](Mesh::points) and
/// [`facets`](Mesh::facets) to read them; [`is_valid`](Mesh::is_valid) re-runs the
/// structural check for values that did not come from [`new`](Mesh::new).
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    points: Vec<Vector3>,
    facets: Vec<[u32; 3]>,
    placement: Placement,
}

impl Mesh {
    /// An empty mesh (no points, no facets, identity placement).
    pub fn empty() -> Self {
        Mesh {
            points: Vec::new(),
            facets: Vec::new(),
            placement: Placement::identity(),
        }
    }

    /// A mesh from `points` and `facets`, rejecting:
    ///
    /// - a non-finite point coordinate,
    /// - a facet index outside `points`,
    /// - a facet that repeats an index (a degenerate triangle).
    ///
    /// The placement starts at the identity.
    pub fn new(points: Vec<Vector3>, facets: Vec<[u32; 3]>) -> Result<Self, MeshError> {
        validate(&points, &facets)?;
        Ok(Mesh {
            points,
            facets,
            placement: Placement::identity(),
        })
    }

    /// A mesh from a list of triangle point triples, welding coordinates that are
    /// exactly equal (including `0.0` and `-0.0`).
    ///
    /// This is the STL-shaped constructor: a reader supplies the triangle corners
    /// as they appear in the file, and this welds shared corners so the result is a
    /// closed mesh rather than a triangle soup.
    pub fn from_facet_points(facets: impl AsRef<[[Vector3; 3]]>) -> Result<Self, MeshError> {
        let facets = facets.as_ref();
        let mut points = Vec::with_capacity(facets.len() * 3);
        let mut indices = Vec::with_capacity(facets.len());
        let mut welded: HashMap<(u64, u64, u64), u32> = HashMap::with_capacity(facets.len() * 3);

        for triangle in facets {
            let mut facet = [0u32; 3];
            for (slot, point) in triangle.iter().enumerate() {
                let index = *welded.entry(point_key(*point)).or_insert_with(|| {
                    points.push(*point);
                    (points.len() - 1) as u32
                });
                facet[slot] = index;
            }
            indices.push(facet);
        }

        Self::new(points, indices)
    }

    /// A box centred on the origin with edges along +X, +Y and +Z
    /// (FreeCAD's `Mesh.createBox`): 8 points, 12 triangles, outward winding.
    ///
    /// Upstream's `BuildRegularGeoms.Cube` is centred (`-l/2 … +l/2`), not
    /// corner-at-origin; SimpleCAD's slicing tests depend on that.
    ///
    /// The dimensions must be finite; they are normally positive. A non-finite
    /// dimension is rejected by [`new`](Mesh::new), and zero or negative dimensions
    /// produce a degenerate or inward-facing mesh for which
    /// [`is_solid`](Mesh::is_solid) is false.
    pub fn box_(length: f64, width: f64, height: f64) -> Result<Self, MeshError> {
        let (lx, ly, lz) = (length * 0.5, width * 0.5, height * 0.5);
        let points = vec![
            Vector3::new(-lx, -ly, -lz),
            Vector3::new(lx, -ly, -lz),
            Vector3::new(lx, ly, -lz),
            Vector3::new(-lx, ly, -lz),
            Vector3::new(-lx, -ly, lz),
            Vector3::new(lx, -ly, lz),
            Vector3::new(lx, ly, lz),
            Vector3::new(-lx, ly, lz),
        ];
        let facets = vec![
            // -Z (bottom)
            [0, 2, 1],
            [0, 3, 2],
            // +Z (top)
            [4, 5, 6],
            [4, 6, 7],
            // -Y (front)
            [0, 1, 5],
            [0, 5, 4],
            // +Y (back)
            [3, 6, 2],
            [3, 7, 6],
            // -X (left)
            [0, 4, 7],
            [0, 7, 3],
            // +X (right)
            [1, 2, 6],
            [1, 6, 5],
        ];
        Self::new(points, facets)
    }

    /// The mesh points, unplaced.
    pub fn points(&self) -> &[Vector3] {
        &self.points
    }

    /// The triangular facets, as point indices.
    pub fn facets(&self) -> &[[u32; 3]] {
        &self.facets
    }

    /// The number of points.
    pub fn count_points(&self) -> usize {
        self.points.len()
    }

    /// The number of facets.
    pub fn count_facets(&self) -> usize {
        self.facets.len()
    }

    /// The placement stored beside the mesh.
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// Replace the placement. The points are not transformed.
    pub fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
    }

    /// The untransformed axis-aligned bounds.
    ///
    /// An empty mesh has a zero-sized box at the origin.
    pub fn bounds(&self) -> BoundBox {
        let mut iter = self.points.iter();
        let Some(first) = iter.next() else {
            return BoundBox::new(Vector3::zero(), Vector3::zero());
        };
        let mut min = *first;
        let mut max = *first;
        for point in iter {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }
        BoundBox::new(min, max)
    }

    /// The points with the stored placement applied (FreeCAD's `Topology` points).
    ///
    /// `Mesh.Points`, `Facet.Points` and the STL writer use the local points instead;
    /// keep the distinction at the binding.
    pub fn placed_points(&self) -> Vec<Vector3> {
        if self.placement == Placement::identity() {
            return self.points.clone();
        }
        let matrix = self.placement.to_matrix();
        self.points
            .iter()
            .map(|point| matrix.transform(point))
            .collect()
    }

    /// The axis-aligned bounds of the placed points (`Mesh.BoundBox`).
    ///
    /// A rigid placement maps the local AABB's corners onto the placed one, so
    /// re-bounding the placed points is exact and needs no per-facet work.
    pub fn placed_bounds(&self) -> BoundBox {
        if self.placement == Placement::identity() {
            return self.bounds();
        }
        let points = self.placed_points();
        let mut iter = points.iter();
        let Some(first) = iter.next() else {
            return BoundBox::new(Vector3::zero(), Vector3::zero());
        };
        let mut min = *first;
        let mut max = *first;
        for point in iter {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }
        BoundBox::new(min, max)
    }

    /// The absolute volume, from the tetrahedron sum.
    ///
    /// Each triangle contributes `a · (b × c) / 6`; a closed mesh with outward
    /// winding gives a positive sum, an inward-wound one a negative sum, and the
    /// reported value is the absolute value — `Mesh.Volume` is non-negative upstream
    /// (`MeshKernel.cpp`), even for a flipped mesh. Use
    /// [`signed_volume`](Mesh::signed_volume) when the sign matters.
    pub fn volume(&self) -> f64 {
        self.signed_volume().abs()
    }

    /// The signed volume: positive for a closed, outward-wound mesh, negative for an
    /// inward-wound one. An open mesh's value depends on the origin.
    pub fn signed_volume(&self) -> f64 {
        let sum: f64 = self
            .facets
            .iter()
            .map(|facet| {
                let a = self.points[facet[0] as usize];
                let b = self.points[facet[1] as usize];
                let c = self.points[facet[2] as usize];
                a.dot(&b.cross(&c))
            })
            .sum();
        sum / 6.0
    }

    /// Whether any undirected edge is shared by **more than two** facets
    /// (FreeCAD `MeshEvalTopology`/`hasNonManifolds`).
    ///
    /// A boundary edge (one facet) is not a non-manifold; that is what
    /// [`has_border_edges`](Mesh::has_border_edges) reports, and it is the only thing
    /// [`is_solid`](Mesh::is_solid) checks, exactly like upstream.
    pub fn has_non_manifolds(&self) -> bool {
        let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
        for facet in &self.facets {
            for edge in edges(*facet) {
                *counts.entry(edge).or_insert(0) += 1;
            }
        }
        counts.values().any(|&count| count > 2)
    }

    /// Whether any undirected edge belongs to only one facet (a boundary edge).
    pub fn has_border_edges(&self) -> bool {
        let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
        for facet in &self.facets {
            for edge in edges(*facet) {
                *counts.entry(edge).or_insert(0) += 1;
            }
        }
        counts.values().any(|&count| count < 2)
    }

    /// Whether two facets traverse a shared edge in the same direction.
    ///
    /// Two consistently oriented neighbours traverse their common edge in opposite
    /// directions. A repeated direction means one of them is flipped relative to
    /// the other.
    pub fn has_non_uniform_oriented_facets(&self) -> bool {
        // Bit 1: the facet traversed the edge from the lower index to the higher
        // one. Bit 2: the opposite direction.
        let mut seen: HashMap<(u32, u32), u8> = HashMap::new();
        for facet in &self.facets {
            for (a, b) in directed_edges(*facet) {
                let bit = if a < b { 1 } else { 2 };
                let key = if a < b { (a, b) } else { (b, a) };
                let mask = seen.entry(key).or_insert(0);
                if *mask & bit != 0 {
                    return true;
                }
                *mask |= bit;
            }
        }
        false
    }

    /// Whether the mesh has no boundary edges (FreeCAD `isSolid`).
    ///
    /// This deliberately checks **only** for border edges: a closed mesh can still be
    /// non-manifold ([`has_non_manifolds`](Mesh::has_non_manifolds)) or have an
    /// inconsistently wound facet
    /// ([`has_non_uniform_oriented_facets`](Mesh::has_non_uniform_oriented_facets)),
    /// and callers test those separately. An empty mesh has no facets to enclose, so
    /// it is not a solid.
    pub fn is_solid(&self) -> bool {
        !self.facets.is_empty() && !self.has_border_edges()
    }

    /// The structural validation [`new`](Mesh::new) performs, re-run.
    ///
    /// A mesh built by [`new`](Mesh::new) or [`from_facet_points`](Mesh::from_facet_points)
    /// is always valid; this exists for a reader or a future mutator that can
    /// produce an inconsistent value.
    pub fn is_valid(&self) -> bool {
        validate(&self.points, &self.facets).is_ok()
    }
}

/// Check the invariants a mesh must satisfy.
fn validate(points: &[Vector3], facets: &[[u32; 3]]) -> Result<(), MeshError> {
    for (index, point) in points.iter().enumerate() {
        if !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite() {
            return Err(MeshError::NonFiniteCoordinate { point: index });
        }
    }
    for (facet_index, facet) in facets.iter().enumerate() {
        for &index in facet {
            if index as usize >= points.len() {
                return Err(MeshError::IndexOutOfRange {
                    facet: facet_index,
                    index,
                    point_count: points.len(),
                });
            }
        }
        if facet[0] == facet[1] || facet[1] == facet[2] || facet[0] == facet[2] {
            return Err(MeshError::DegenerateFacet { facet: facet_index });
        }
    }
    Ok(())
}

/// The three undirected edges of a facet, each with its ends sorted so shared
/// edges collide in a map.
fn edges(facet: [u32; 3]) -> [(u32, u32); 3] {
    directed_edges(facet).map(|(a, b)| if a < b { (a, b) } else { (b, a) })
}

/// The three directed edges of a facet, in winding order.
pub(crate) fn directed_edges([a, b, c]: [u32; 3]) -> [(u32, u32); 3] {
    [(a, b), (b, c), (c, a)]
}

/// A hash key for exact coordinate equality. `-0.0` and `0.0` are the same point.
fn point_key(point: Vector3) -> (u64, u64, u64) {
    fn bits(value: f64) -> u64 {
        if value == 0.0 {
            0.0f64.to_bits()
        } else {
            value.to_bits()
        }
    }
    (bits(point.x), bits(point.y), bits(point.z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_is_a_closed_outward_solid() {
        let mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        assert_eq!(mesh.count_points(), 8);
        assert_eq!(mesh.count_facets(), 12);
        assert_eq!(mesh.volume(), 24.0);
        assert!(mesh.is_valid());
        assert!(mesh.is_solid());
        assert!(!mesh.has_non_manifolds());
        assert!(!mesh.has_non_uniform_oriented_facets());
        assert_eq!(mesh.placement(), Placement::identity());

        let bounds = mesh.bounds();
        assert_eq!(bounds.min, Vector3::new(-1.0, -1.5, -2.0));
        assert_eq!(bounds.max, Vector3::new(1.0, 1.5, 2.0));
        assert_eq!(
            bounds.diagonal_length(),
            (2.0f64 * 2.0 + 3.0 * 3.0 + 4.0 * 4.0).sqrt()
        );
        assert_eq!(mesh.placed_bounds(), bounds);
    }

    #[test]
    fn an_empty_mesh_is_not_a_solid() {
        let mesh = Mesh::empty();
        assert_eq!(mesh.count_points(), 0);
        assert_eq!(mesh.count_facets(), 0);
        assert_eq!(mesh.volume(), 0.0);
        assert_eq!(
            mesh.bounds(),
            BoundBox::new(Vector3::zero(), Vector3::zero())
        );
        assert!(!mesh.is_solid());
        assert!(mesh.is_valid());
    }

    #[test]
    fn an_open_plane_has_boundary_edges() {
        // Two triangles sharing one edge: a flat fan with four boundary edges.
        let mesh = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        assert!(!mesh.is_solid());
        assert!(
            !mesh.has_non_manifolds(),
            "a boundary edge is not a non-manifold"
        );
        assert!(mesh.has_border_edges());
        assert_eq!(mesh.count_facets(), 2);
        assert_eq!(mesh.bounds().max, Vector3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn a_flipped_facet_is_detected() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        // Reverse the winding of one face by swapping two of its indices.
        let facet = mesh.facets[0];
        mesh.facets[0] = [facet[0], facet[2], facet[1]];
        assert!(mesh.has_non_uniform_oriented_facets());
        // Upstream `isSolid` only asks for border edges, so a flipped facet alone
        // does not make a closed mesh "not solid".
        assert!(mesh.is_solid());
        assert!(!mesh.has_border_edges());
    }

    #[test]
    fn new_rejects_a_dangling_index() {
        let error = Mesh::new(
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 3]],
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                MeshError::IndexOutOfRange {
                    facet: 0,
                    index: 3,
                    point_count: 3
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn new_rejects_a_non_finite_coordinate() {
        let error = Mesh::new(
            vec![
                Vector3::zero(),
                Vector3::new(f64::NAN, 0.0, 0.0),
                Vector3::new(0.0, f64::INFINITY, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap_err();
        assert!(
            matches!(error, MeshError::NonFiniteCoordinate { point: 1 }),
            "{error}"
        );
    }

    #[test]
    fn new_rejects_a_degenerate_facet() {
        let error = Mesh::new(
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 1]],
        )
        .unwrap_err();
        assert!(
            matches!(error, MeshError::DegenerateFacet { facet: 0 }),
            "{error}"
        );
    }

    #[test]
    fn from_facet_points_welds_exact_duplicates() {
        // The same box as `box_`, but as a triangle soup with shared corners
        // written out repeatedly.
        let (l, w, h) = (2.0, 3.0, 4.0);
        let (lx, ly, lz) = (l * 0.5, w * 0.5, h * 0.5);
        let p = [
            Vector3::new(-lx, -ly, -lz),
            Vector3::new(lx, -ly, -lz),
            Vector3::new(lx, ly, -lz),
            Vector3::new(-lx, ly, -lz),
            Vector3::new(-lx, -ly, lz),
            Vector3::new(lx, -ly, lz),
            Vector3::new(lx, ly, lz),
            Vector3::new(-lx, ly, lz),
        ];
        let soup: Vec<[Vector3; 3]> = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 6, 2],
            [3, 7, 6],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ]
        .iter()
        .map(|facet| [p[facet[0]], p[facet[1]], p[facet[2]]])
        .collect();

        let mesh = Mesh::from_facet_points(&soup).unwrap();
        assert_eq!(mesh.count_points(), 8);
        assert_eq!(mesh.count_facets(), 12);
        assert_eq!(mesh.volume(), l * w * h);
        assert!(mesh.is_solid());
        // Welding numbers points in first-seen order, so compare the observable
        // geometry rather than the raw point order.
        assert_eq!(mesh.bounds(), Mesh::box_(l, w, h).unwrap().bounds());
    }

    #[test]
    fn placement_is_stored_but_not_baked_into_the_bounds() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let placement = Placement::new(
            Vector3::new(10.0, 0.0, 0.0),
            ferrocad_types::Rotation::identity(),
        );
        mesh.set_placement(placement);
        assert_eq!(mesh.placement(), placement);
        assert_eq!(mesh.bounds().max, Vector3::new(0.5, 0.5, 0.5));
        assert_eq!(mesh.volume(), 1.0);
        // `placed_bounds` reports the placed box; `bounds` stays local.
        let placed = mesh.placed_bounds();
        assert_eq!(placed.min, Vector3::new(9.5, -0.5, -0.5));
        assert_eq!(placed.max, Vector3::new(10.5, 0.5, 0.5));
        let points = mesh.placed_points();
        assert_eq!(points.len(), 8);
        assert!(points.iter().all(|point| (point.x - 10.0).abs() <= 0.5));
    }
}

//! Wire assembly: an ordered edge list, [`polygon`] and [`sort_edges`].
//!
//! Mirrors upstream's `Part.Wire` / `Part.makePolygon` and Draft's
//! `__sortEdges__` / `Part.sortEdges`. The consumer is Draft's `draftgeoutils`
//! (see `docs/architecture/part-draft-layering.md` §2): it builds wires from
//! analytic curves long before it asks the kernel for a shape.

use ferrocad_types::Vector3;

use crate::curve::{EdgeCurve, LineSegment};
use crate::{CurveError, TOLERANCE};

/// An ordered chain of bounded edges (`Part.Wire`).
///
/// Construction does not reorder: use [`Wire::from_edges`] or [`sort_edges`] for
/// an unordered list. Every edge must have endpoints, so an infinite
/// [`Line`](crate::Line) is rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    edges: Vec<EdgeCurve>,
}

impl Wire {
    /// A wire from edges that are already in order.
    pub fn new(edges: Vec<EdgeCurve>) -> Result<Self, CurveError> {
        if edges.is_empty() {
            return Err(CurveError::TooFewPoints);
        }
        if edges.iter().any(|edge| edge.endpoints().is_none()) {
            return Err(CurveError::NotBounded);
        }
        Ok(Self { edges })
    }

    /// A wire from unordered edges, ordered by [`sort_edges`].
    pub fn from_edges(edges: &[EdgeCurve]) -> Result<Self, CurveError> {
        Self::new(sort_edges(edges, TOLERANCE)?)
    }

    /// The ordered edges.
    pub fn edges(&self) -> &[EdgeCurve] {
        &self.edges
    }

    /// The number of edges.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// A wire always has at least one edge; present for the `len` convention.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The sum of the edge lengths.
    pub fn length(&self) -> f64 {
        self.edges.iter().map(EdgeCurve::length).sum()
    }

    /// Whether the last edge ends where the first one starts (within [`TOLERANCE`]).
    pub fn is_closed(&self) -> bool {
        let Some((start, _)) = self.edges.first().and_then(EdgeCurve::endpoints) else {
            return false;
        };
        let Some((_, end)) = self.edges.last().and_then(EdgeCurve::endpoints) else {
            return false;
        };
        start.distance(&end) <= TOLERANCE
    }
}

/// Straight segments between consecutive `points` (`Part.makePolygon`).
///
/// An open chain needs at least two points; a closed one at least three and an
/// extra segment back to the first point. Consecutive coincident points are
/// rejected (`CurveError::Degenerate`) rather than silently dropped.
pub fn polygon(points: &[Vector3], closed: bool) -> Result<Vec<EdgeCurve>, CurveError> {
    let minimum = if closed { 3 } else { 2 };
    if points.len() < minimum {
        return Err(CurveError::TooFewPoints);
    }
    let mut edges = Vec::with_capacity(points.len());
    for pair in points.windows(2) {
        edges.push(EdgeCurve::LineSegment(LineSegment::new(pair[0], pair[1])?));
    }
    if closed {
        edges.push(EdgeCurve::LineSegment(LineSegment::new(
            *points.last().expect("at least three points"),
            points[0],
        )?));
    }
    Ok(edges)
}

/// Order `edges` into one chain, reversing an edge when it arrives backwards.
///
/// The chain starts at the first edge as given and follows shared endpoints
/// within `tolerance` (upstream's `sortEdges`). An infinite line is rejected, and
/// a gap that no remaining edge bridges is [`CurveError::Disconnected`]. The
/// chain is not required to close: an open wire sorts too.
pub fn sort_edges(edges: &[EdgeCurve], tolerance: f64) -> Result<Vec<EdgeCurve>, CurveError> {
    let mut remaining = edges.to_vec();
    if remaining.is_empty() {
        return Ok(Vec::new());
    }
    if remaining.iter().any(|edge| edge.endpoints().is_none()) {
        return Err(CurveError::NotBounded);
    }

    let first = remaining.remove(0);
    let mut cursor = first.endpoints().expect("bounded").1;
    let mut ordered = vec![first];

    while !remaining.is_empty() {
        let mut next = None;
        for (index, edge) in remaining.iter().enumerate() {
            let (start, end) = edge.endpoints().expect("bounded");
            if start.distance(&cursor) <= tolerance {
                next = Some((index, *edge, end));
                break;
            }
            if end.distance(&cursor) <= tolerance {
                next = Some((index, edge.reversed(), start));
                break;
            }
        }
        let Some((index, edge, end)) = next else {
            return Err(CurveError::Disconnected);
        };
        remaining.remove(index);
        ordered.push(edge);
        cursor = end;
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(ax: f64, ay: f64, bx: f64, by: f64) -> EdgeCurve {
        EdgeCurve::LineSegment(
            LineSegment::new(Vector3::new(ax, ay, 0.0), Vector3::new(bx, by, 0.0)).unwrap(),
        )
    }

    fn ends(edge: &EdgeCurve) -> (Vector3, Vector3) {
        edge.endpoints().expect("bounded")
    }

    #[test]
    fn polygon_builds_open_and_closed_chains() {
        let points = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(4.0, 0.0, 0.0),
            Vector3::new(4.0, 3.0, 0.0),
        ];
        let open = polygon(&points, false).unwrap();
        assert_eq!(open.len(), 2);
        let closed = polygon(&points, true).unwrap();
        assert_eq!(closed.len(), 3);
        assert!((Wire::new(closed).unwrap().length() - 12.0).abs() < 1e-9);
    }

    #[test]
    fn a_polygon_needs_enough_distinct_points() {
        assert_eq!(
            polygon(&[Vector3::zero()], false),
            Err(CurveError::TooFewPoints)
        );
        assert_eq!(
            polygon(&[Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)], true),
            Err(CurveError::TooFewPoints)
        );
        assert_eq!(
            polygon(&[Vector3::zero(), Vector3::zero()], false),
            Err(CurveError::Degenerate)
        );
    }

    #[test]
    fn sort_edges_orders_and_orients_a_shuffled_chain() {
        let a = segment(0.0, 0.0, 1.0, 0.0);
        let b = segment(1.0, 0.0, 2.0, 0.0);
        let c = segment(3.0, 0.0, 2.0, 0.0);

        // The chain starts at the first edge as given (`c`, 3 -> 2); the rest are
        // out of order and `c`'s neighbours arrive backwards.
        let sorted = sort_edges(&[c, a, b], TOLERANCE).unwrap();
        assert_eq!(sorted.len(), 3);
        let mut cursor = ends(&sorted[0]).0;
        assert!(cursor.distance(&Vector3::new(3.0, 0.0, 0.0)) < 1e-9);
        for edge in &sorted {
            let (start, end) = ends(edge);
            assert!(
                start.distance(&cursor) < 1e-9,
                "each edge starts where the previous one ended"
            );
            cursor = end;
        }
        assert!(cursor.distance(&Vector3::zero()) < 1e-9);
    }

    #[test]
    fn sort_edges_tolerates_near_coincident_endpoints() {
        let a = segment(0.0, 0.0, 1.0, 0.0);
        let b = segment(1.0 + 1e-9, 0.0, 2.0, 0.0);
        let sorted = sort_edges(&[a, b], TOLERANCE).unwrap();
        assert_eq!(sorted.len(), 2);
        assert!(ends(&sorted[1]).0.distance(&ends(&sorted[0]).1) <= TOLERANCE);
        assert_eq!(
            sort_edges(&[a, b], 1e-12),
            Err(CurveError::Disconnected),
            "a tighter tolerance rejects the same join"
        );
    }

    #[test]
    fn sort_edges_reports_a_gap_and_an_unbounded_edge() {
        let a = segment(0.0, 0.0, 1.0, 0.0);
        let b = segment(2.0, 0.0, 3.0, 0.0);
        assert_eq!(sort_edges(&[a, b], TOLERANCE), Err(CurveError::Disconnected));

        let line = EdgeCurve::Line(crate::Line::new(
            Vector3::zero(),
            Vector3::new(1.0, 0.0, 0.0),
        ).unwrap());
        assert_eq!(
            sort_edges(&[a, line], TOLERANCE),
            Err(CurveError::NotBounded)
        );
    }

    #[test]
    fn a_wire_knows_whether_it_closes_and_its_length() {
        let a = segment(0.0, 0.0, 1.0, 0.0);
        let b = segment(1.0, 0.0, 1.0, 1.0);
        let c = segment(1.0, 1.0, 0.0, 0.0);
        let open = Wire::new(vec![a, b]).unwrap();
        assert!(!open.is_closed());
        assert!((open.length() - 2.0).abs() < 1e-9);

        let closed = Wire::from_edges(&[c, a, b]).unwrap();
        assert!(closed.is_closed());
        assert!((closed.length() - (2.0 + 2.0f64.sqrt())).abs() < 1e-9);
        assert_eq!(closed.len(), 3);
        assert!(!closed.is_empty());
    }

    #[test]
    fn a_wire_rejects_empty_and_unbounded_input() {
        assert_eq!(Wire::new(Vec::new()), Err(CurveError::TooFewPoints));
        let line = EdgeCurve::Line(crate::Line::new(
            Vector3::zero(),
            Vector3::new(0.0, 1.0, 0.0),
        ).unwrap());
        assert_eq!(Wire::new(vec![line]), Err(CurveError::NotBounded));
    }
}

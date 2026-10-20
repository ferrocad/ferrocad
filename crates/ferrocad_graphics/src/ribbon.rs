//! Screen-facing ribbons.
//!
//! The renderer's line pipeline is 1 px with no width uniform, so a wider "vector"
//! stroke is drawn as a quad around the segment. The quad lies in the plane
//! perpendicular to `view_forward`: its width is measured across that plane, so it
//! stays visually crisp even when the segment runs into the view (where the projected
//! segment shrinks). Callers supply a half-width in the same units as the endpoints;
//! the gizmo derives it from its screen-invariant scale, and the navicube uses a fixed
//! cube-unit width for its hover glow.

use ferrocad_types::Vector3;

/// A quad around the segment `a`–`b`, `half_width` to either side of its **screen**
/// direction. The four corners are returned in order (`a+side`, `b+side`, `b-side`,
/// `a-side`) for a two-triangle ribbon.
///
/// `None` when the segment (or the view direction) is degenerate, or when the segment is
/// parallel to `view_forward`: its projected length is zero and the ribbon has no
/// well-defined orientation.
pub fn screen_ribbon(
    a: Vector3,
    b: Vector3,
    half_width: f64,
    view_forward: Vector3,
) -> Option<[Vector3; 4]> {
    if !half_width.is_finite() || half_width <= 0.0 {
        return None;
    }
    let forward_length = view_forward.length();
    if forward_length < 1e-9 {
        return None;
    }
    let forward = view_forward.scale(1.0 / forward_length);
    let delta = b.sub(&a);
    // Drop the depth component: the perpendicular is measured across the screen.
    let screen = delta.sub(&forward.scale(delta.dot(&forward)));
    if screen.length() < 1e-9 {
        return None;
    }
    let side = screen.cross(&forward);
    if side.length() < 1e-9 {
        return None;
    }
    let side = side.normalize().scale(half_width);
    Some([a.add(&side), b.add(&side), b.sub(&side), a.sub(&side)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vector3, b: Vector3) -> bool {
        a.sub(&b).length() < 1e-9
    }

    #[test]
    fn a_ribbon_straddles_the_segment_across_the_view() {
        // A horizontal segment viewed along −Z is widened along ±Y.
        let quad = screen_ribbon(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            0.1,
            Vector3::new(0.0, 0.0, -1.0),
        )
        .expect("a ribbon");
        assert!(close(quad[0], Vector3::new(0.0, 0.1, 0.0)));
        assert!(close(quad[1], Vector3::new(1.0, 0.1, 0.0)));
        assert!(close(quad[2], Vector3::new(1.0, -0.1, 0.0)));
        assert!(close(quad[3], Vector3::new(0.0, -0.1, 0.0)));
    }

    #[test]
    fn a_view_parallel_segment_has_no_ribbon() {
        // The segment runs along the view direction: its projection is a point.
        assert!(
            screen_ribbon(
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                0.1,
                Vector3::new(0.0, 0.0, -1.0),
            )
            .is_none()
        );
    }

    #[test]
    fn a_zero_width_or_zero_vector_is_rejected() {
        let a = Vector3::new(0.0, 0.0, 0.0);
        let b = Vector3::new(1.0, 0.0, 0.0);
        assert!(screen_ribbon(a, b, 0.0, Vector3::new(0.0, 0.0, -1.0)).is_none());
        assert!(screen_ribbon(a, a, 0.1, Vector3::new(0.0, 0.0, -1.0)).is_none());
        assert!(screen_ribbon(a, b, 0.1, a).is_none());
    }
}

//! The plane a program draws its grid on, carried on a view.
//!
//! The viewport owns the active construction plane (a `ferrocad_stage::Workplane` plus
//! the patch radius the native grid computes), but a view and a backend must both name
//! it without learning the stage vocabulary. This is the neutral carrier they share:
//! origin, in-plane axes, normal, and the patch radius a renderer writes into its own
//! frame uniform.
//!
//! It describes a plane, not a pickable object and not a document feature: the
//! `WorkplaneKind` (world, face, datum, …) never crosses this boundary, because a
//! renderer draws a frame and nothing else.

/// A construction plane in renderer-neutral terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkplaneFrame {
    /// The plane's world origin.
    pub origin: [f32; 3],
    /// The in-plane unit `u` axis (the grid's `x`).
    pub u: [f32; 3],
    /// The in-plane unit `v` axis (the grid's `y`).
    pub v: [f32; 3],
    /// The unit normal, `u × v`.
    pub n: [f32; 3],
    /// The minor (dot) step in world units, as the host resolved it for this camera
    /// (the native grid's effective step).
    pub minor: f32,
    /// The major (line) step in world units, same resolution.
    pub major: f32,
    /// The patch radius in world units around the origin; `0` means no patch, so the
    /// grid stays unbounded.
    pub radius: f32,
    /// The minor level's **base ink** alpha, as the native generator draws it (the token
    /// alpha times the user's opacity, before the edge-on fade). `0` means "no claim":
    /// the program's own parameter is used.
    ///
    /// This is what keeps the two backends' lines equally strong: a shader with its own
    /// alpha draws the same lattice at a different weight, which reads as a different
    /// line *thickness*.
    pub minor_alpha: f32,
    /// The major level's base ink alpha, same resolution.
    pub major_alpha: f32,
    /// Whether the minor level is a line lattice (`true`) or intersection dots (`false`).
    pub minor_lines: bool,
    /// The major lines' stroke width in world units; `0` is the shader's own 1 px
    /// hairline. The stroke lies in the plane, which is what the native generator draws.
    pub major_width: f32,
    /// The minor lattice's stroke width in world units; `0` is a hairline.
    pub minor_width: f32,
}

impl WorkplaneFrame {
    /// The world `XY` plane, unbounded: the default a view draws without a plane.
    pub const fn world() -> Self {
        Self {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            n: [0.0, 0.0, 1.0],
            minor: 10.0,
            major: 100.0,
            radius: 0.0,
            minor_alpha: 0.0,
            major_alpha: 0.0,
            minor_lines: false,
            major_width: 0.0,
            minor_width: 0.0,
        }
    }

    /// The radius a renderer may use: a non-finite or negative value is "no patch", so
    /// a degenerate host value draws the unbounded ground rather than an empty grid.
    pub fn patch_radius(&self) -> f32 {
        if self.radius.is_finite() && self.radius > 0.0 {
            self.radius
        } else {
            0.0
        }
    }
}

impl Default for WorkplaneFrame {
    fn default() -> Self {
        Self::world()
    }
}

//! # FerroCAD analytic curves (`ferrocad_curves`)
//!
//! The kernel-free **analytic** half of `Part`: the Rust home of upstream's
//! `Part::Geom*` classes (`src/Mod/Part/App/Geometry.h`), which derive from
//! `Base::Persistence` and are *not* B-Rep shapes. Only `toShape()` ties them to
//! the kernel; [`ferrocad_geom`](https://crates.io/crates/ferrocad_geom) owns that
//! seam (`make_edge`), so this leaf never names a `Shape`, OCCT or a document.
//!
//! - [`curve`] — [`Vector2`], [`Line`], [`LineSegment`], [`Circle`], [`Arc`],
//!   [`Ellipse`], [`Hyperbola`], [`Parabola`], [`BezierCurve`] and the
//!   [`EdgeCurve`] enum, with OCC/Part parameterisation.
//! - [`wire`] — [`Wire`] as an ordered edge list, [`polygon`] and [`sort_edges`]
//!   (upstream's `__sortEdges__`).
//!
//! Depends only on `ferrocad_types` (for [`Vector3`]). The dependency rules and
//! the reason for a separate crate are in `docs/architecture/part-draft-layering.md`
//! §5; the seam that converts these curves into shapes is P2.2.
//!
//! # Example
//!
//! ```
//! use ferrocad_curves::{Circle, EdgeCurve, LineSegment, Vector3, polygon};
//!
//! let square = polygon(
//!     &[
//!         Vector3::new(0.0, 0.0, 0.0),
//!         Vector3::new(10.0, 0.0, 0.0),
//!         Vector3::new(10.0, 10.0, 0.0),
//!         Vector3::new(0.0, 10.0, 0.0),
//!     ],
//!     true,
//! )
//! .unwrap();
//! assert_eq!(square.len(), 4);
//! assert!((LineSegment::new(Vector3::zero(), Vector3::new(0.0, 1.0, 0.0))
//!     .unwrap()
//!     .length() - 1.0).abs() < 1e-12);
//! assert!(matches!(EdgeCurve::Circle(Circle::new(
//!     Vector3::zero(),
//!     Vector3::new(0.0, 0.0, 1.0),
//!     2.0
//! ).unwrap()), EdgeCurve::Circle(_)));
//! ```

use std::fmt;

pub mod curve;
pub mod wire;

pub use curve::{
    Arc, ArcOfHyperbola, ArcOfParabola, BezierCurve, Circle, EdgeCurve, Ellipse, Hyperbola, Line,
    LineSegment, NurbsCurve, Parabola, Vector2,
};
pub use ferrocad_types::Vector3;
pub use wire::{Wire, polygon, sort_edges};

/// The linear tolerance: FreeCAD's `Precision::Confusion()` (`1e-7`).
pub const TOLERANCE: f64 = 1e-7;

/// The angular tolerance for direction comparisons (`1e-12` rad).
pub const ANGULAR_TOLERANCE: f64 = 1e-12;

/// Why a curve or a wire could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveError {
    /// A direction, radius or axis was zero-length or not finite.
    Degenerate,
    /// A polygon or a wire had too few points or edges.
    TooFewPoints,
    /// A wire contains an edge with no endpoints (an infinite [`Line`]).
    NotBounded,
    /// The edges do not form one chain within the tolerance.
    Disconnected,
    /// A parameter range was empty, reversed, or not finite.
    InvalidParameter,
    /// The polynomial degree exceeds OCCT's maximum (`25`).
    DegreeTooHigh,
}

impl fmt::Display for CurveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurveError::Degenerate => f.write_str("degenerate geometry"),
            CurveError::TooFewPoints => f.write_str("too few points for a wire"),
            CurveError::NotBounded => f.write_str("an infinite line has no endpoints"),
            CurveError::Disconnected => f.write_str("the edges do not form one chain"),
            CurveError::InvalidParameter => f.write_str("invalid parameter range"),
            CurveError::DegreeTooHigh => f.write_str("the polynomial degree is too high"),
        }
    }
}

impl std::error::Error for CurveError {}

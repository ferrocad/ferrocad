//! The OCCT backend's error type.

use std::fmt;

/// A failure from the OCCT backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcctError {
    /// A `ferrocad_geom::Shape` was not produced by this backend (e.g. it came from
    /// `NullBackend`); the erased handle did not downcast to an OCCT shape.
    WrongBackend,
    /// OCCT reported the operation as not done (`IsDone() == false`).
    NotDone { operation: &'static str },
    /// An input could not be interpreted (e.g. a fillet edge ref that is not a shape
    /// of this backend, or is not an edge).
    InvalidInput { what: &'static str },
}

impl fmt::Display for OcctError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcctError::WrongBackend => {
                write!(f, "shape does not belong to the OCCT backend")
            }
            OcctError::NotDone { operation } => {
                write!(f, "OCCT operation `{operation}` did not complete")
            }
            OcctError::InvalidInput { what } => write!(f, "invalid input: {what}"),
        }
    }
}

impl std::error::Error for OcctError {}

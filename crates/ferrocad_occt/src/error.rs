//! The OCCT backend's error type.

use std::fmt;

use ferrocad_geom::{FailureKind, GeomError, InvalidReason};

/// A failure from the OCCT backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcctError {
    /// A `ferrocad_geom::Shape` was not produced by this backend (e.g. it came from
    /// `NullBackend`); the erased handle did not downcast to an OCCT shape.
    WrongBackend,
    /// OCCT reported the operation as not done (`IsDone() == false`).
    NotDone { operation: &'static str },
    /// OCCT raised while running the operation, with the kernel's own message
    /// (`Standard_Failure::GetMessageString`).
    Exception {
        operation: &'static str,
        message: String,
    },
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
            OcctError::Exception { operation, message } => {
                write!(f, "OCCT operation `{operation}` failed: {message}")
            }
            OcctError::InvalidInput { what } => write!(f, "invalid input: {what}"),
        }
    }
}

impl std::error::Error for OcctError {}

/// Classify a seam error as this backend understands it.
///
/// This is the kernel-specific half of the typed failure model: callers above the
/// seam only see [`FailureKind`]. Only `InvalidInput` is a domain error, because
/// that is the one case where the backend rejected what the user asked for before
/// the kernel ran; `NotDone` and `Exception` are the kernel failing to execute,
/// which the user cannot fix.
///
/// The fallback is [`FailureKind::Kernel`] as well — an error this backend did not
/// produce must never be blamed on the user.
pub(crate) fn classify(error: &GeomError) -> FailureKind {
    match error.downcast_ref::<OcctError>() {
        Some(OcctError::InvalidInput { what }) => FailureKind::Invalid {
            reason: invalid_reason(what),
            message: error.to_string(),
        },
        Some(OcctError::NotDone { operation }) => FailureKind::Kernel {
            operation,
            message: error.to_string(),
        },
        Some(OcctError::Exception { operation, message }) => FailureKind::Kernel {
            operation,
            message: message.clone(),
        },
        Some(OcctError::WrongBackend) => FailureKind::Kernel {
            operation: "shape ownership",
            message: error.to_string(),
        },
        None => FailureKind::Kernel {
            operation: "",
            message: error.to_string(),
        },
    }
}

/// Map the backend's own `what` text to a kernel-neutral reason.
///
/// `what` is a closed set of literals written next to each validation in this
/// crate, so these guards are the whole mapping. The categories are systematic
/// (a null handle, a missing list, a numeric bound, an element-kind check),
/// which keeps the mapping short enough to audit; a new validation site that fits
/// none of them falls through to [`InvalidReason::Other`] with its text intact
/// rather than being misfiled as a domain error it is not.
fn invalid_reason(what: &'static str) -> InvalidReason {
    match what {
        // Every empty-handle check names the handle's kind: "null shape",
        // "null edge", "null face", "null wire", "null spine", …
        w if w.starts_with("null ") => InvalidReason::NullShape,

        // Fillet/chamfer ownership checks (the only uses of this wording).
        w if w.ends_with("is not part of the shape") => InvalidReason::EdgeNotOnShape,

        // Numeric domain checks: positivity/finiteness, non-zero lengths and
        // directions, ranges, and the bare `cylinder radius`/`height`/`angle`
        // labels that `surface_ops::finite` takes.
        w if w == "cylinder radius"
            || w == "cylinder height"
            || w == "cylinder angle"
            || w.contains("must be positive")
            || w.contains("must be finite")
            || w.contains("is finite")
            || w.contains("is a finite")
            || w.contains("are finite")
            || w.contains("is a positive")
            || w.contains("non-zero")
            || w.contains("too small")
            || w.contains("must be in (0")
            || w.contains("out of range")
            || w.contains("cannot have a zero") =>
        {
            InvalidReason::NonPositiveValue
        }

        // Inputs the operation needs but was not given: "… needs at least one
        // edge", "… needs wires", "a face needs a wire", "project needs a tool
        // and a target".
        w if w.contains("at least")
            || w.contains("needs wires")
            || w.contains("needs a wire")
            || w.contains("needs a tool and a target") =>
        {
            InvalidReason::EmptySelection
        }

        // Element-kind checks: a reference that is not the sub-shape the
        // operation needs — "make_wire expects edges", "not an edge", "a fillet
        // edge must be an OCCT edge", "boundary wire", …
        w if w.contains("expects")
            || w.starts_with("not a ")
            || w.starts_with("not an ")
            || w.contains("must be an OCCT")
            || w.contains("must be a wire")
            || w.contains("is made of faces")
            || w.contains("needs an outer wire")
            || w.contains("needs an adjacent face")
            || w == "boundary wire"
            || w == "defeaturing face" =>
        {
            InvalidReason::UnresolvableElement
        }

        w => InvalidReason::Other(w.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OcctBackend;
    use ferrocad_geom::GeometryBackend;

    /// Classify through the real trait method, exactly as a workbench would.
    fn kind(error: OcctError) -> FailureKind {
        OcctBackend.classify(&GeomError::from(error))
    }

    #[test]
    fn classify_maps_invalid_input_to_a_domain_failure() {
        // One representative `what` per reason, using the wording the validations
        // in this crate actually produce.
        let cases = [
            (
                "fillet radius must be positive and finite",
                InvalidReason::NonPositiveValue,
            ),
            (
                "the sphere radius must be positive",
                InvalidReason::NonPositiveValue,
            ),
            ("cylinder radius", InvalidReason::NonPositiveValue),
            (
                "a tolerance is a finite, non-negative number",
                InvalidReason::NonPositiveValue,
            ),
            (
                "extrusion needs a non-zero vector",
                InvalidReason::NonPositiveValue,
            ),
            (
                "fillet needs at least one edge",
                InvalidReason::EmptySelection,
            ),
            (
                "a solid needs at least one shell",
                InvalidReason::EmptySelection,
            ),
            ("a face needs a wire", InvalidReason::EmptySelection),
            ("null shape", InvalidReason::NullShape),
            ("null edge", InvalidReason::NullShape),
            (
                "fillet edge is not part of the shape",
                InvalidReason::EdgeNotOnShape,
            ),
            (
                "chamfer edge is not part of the shape",
                InvalidReason::EdgeNotOnShape,
            ),
            (
                "make_wire expects edges",
                InvalidReason::UnresolvableElement,
            ),
            ("not an edge", InvalidReason::UnresolvableElement),
            (
                "a fillet edge must be an OCCT edge",
                InvalidReason::UnresolvableElement,
            ),
            ("boundary wire", InvalidReason::UnresolvableElement),
        ];

        for (what, expected) in cases {
            match kind(OcctError::InvalidInput { what }) {
                FailureKind::Invalid { reason, message } => {
                    assert_eq!(reason, expected, "{what:?}");
                    assert!(
                        message.contains(what),
                        "{message:?} should quote {what:?}"
                    );
                }
                other => panic!("{what:?} should be a domain failure, got {other}"),
            }
        }
    }

    #[test]
    fn classify_keeps_an_unrecognised_input_error_as_other() {
        let what = "an open wire cannot form a face";
        assert_eq!(
            kind(OcctError::InvalidInput { what }),
            FailureKind::Invalid {
                reason: InvalidReason::Other(what.to_string()),
                message: format!("invalid input: {what}"),
            }
        );
    }

    #[test]
    fn classify_maps_not_done_to_a_kernel_failure() {
        assert_eq!(
            kind(OcctError::NotDone { operation: "fillet" }),
            FailureKind::Kernel {
                operation: "fillet",
                message: "OCCT operation `fillet` did not complete".to_string(),
            }
        );
    }

    #[test]
    fn classify_maps_an_exception_to_a_kernel_failure() {
        // The kernel's own text is the message; the operation tag comes from the
        // `OcctError`, not from parsing the message back out.
        assert_eq!(
            kind(OcctError::Exception {
                operation: "cut",
                message: "StdFail_NotDone".to_string(),
            }),
            FailureKind::Kernel {
                operation: "cut",
                message: "StdFail_NotDone".to_string(),
            }
        );
    }

    #[test]
    fn classify_maps_wrong_backend_to_a_kernel_failure() {
        assert_eq!(
            kind(OcctError::WrongBackend),
            FailureKind::Kernel {
                operation: "shape ownership",
                message: "shape does not belong to the OCCT backend".to_string(),
            }
        );
    }

    #[test]
    fn classify_falls_back_to_kernel_for_a_foreign_error() {
        let error: GeomError = Box::new(std::io::Error::other("not from a kernel"));
        assert_eq!(
            OcctBackend.classify(&error),
            FailureKind::Kernel {
                operation: "",
                message: "not from a kernel".to_string(),
            }
        );
    }
}

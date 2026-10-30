//! `Part.makeWireString` — a string as wires, one list per character.
//!
//! Upstream (`FT2FC.cpp`) is a FreeType port. We take the outlines from
//! [`ferrocad_text`] (which uses `skrifa`/`read-fonts`, already in the build
//! graph) and use the kernel only to turn contours into edges and wires. The
//! scale, the advance/kerning order and the `list[list[Wire]]` return shape
//! mirror upstream; see `crates/ferrocad_text` and
//! `docs/project/tasks/simplecad-bringup.md` §SC2.
//!
//! One deliberate divergence: upstream reverses every contour wire's
//! orientation. Outer and inner contours are both reversed, so the relative
//! orientation (what a fill reads) is unchanged; we keep the font's direction.

use std::path::Path;

use ferrocad_curves::{BezierCurve, EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::Shape;
use ferrocad_part::backend;
use ferrocad_text::{Contour, PathCommand, Point2};
use pyo3::prelude::*;
use pyo3::types::PyList;

use crate::PartShape;
use crate::curves::{OCCError, curve_error, kernel_error, shape_from_curve};

/// Two points closer than this (mm) are the same point, matching
/// `ferrocad_text`'s contour builder.
const SAME_POINT: f64 = 1e-9;

/// `Part.makeWireString(text, dir, fontfile, height, tracking=0)`.
///
/// Returns one list per character, in text order, each holding that character's
/// closed wires — upstream's `list[list[Wire]]`. A character with no outline (a
/// space) gives an empty inner list.
#[pyfunction]
#[pyo3(signature = (text, dir, fontfile, height, tracking = 0.0))]
pub(crate) fn makeWireString<'py>(
    py: Python<'py>,
    text: &str,
    dir: &str,
    fontfile: &str,
    height: f64,
    tracking: f64,
) -> PyResult<Bound<'py, PyList>> {
    if !height.is_finite() || height <= 0.0 {
        return Err(OCCError::new_err(
            "makeWireString: the text height must be greater than zero",
        ));
    }
    let path = Path::new(dir).join(fontfile);
    let bytes = std::fs::read(&path).map_err(|error| {
        OCCError::new_err(format!(
            "makeWireString: cannot read the font file {}: {error}",
            path.display()
        ))
    })?;
    let outline = ferrocad_text::outline_text(&bytes, text, height, tracking)
        .map_err(|error| OCCError::new_err(format!("makeWireString: {error}")))?;

    let characters = PyList::empty(py);
    for glyph in &outline.glyphs {
        let wires = PyList::empty(py);
        for contour in &glyph.contours {
            // A contour that cannot be joined is dropped, as upstream drops a
            // character whose glyph produced no wires.
            if let Ok(wire) = contour_wire(contour, glyph.origin) {
                wires.append(PartShape::from_shape(wire))?;
            }
        }
        characters.append(wires)?;
    }
    Ok(characters)
}

/// One contour as a closed wire, placed at `origin`.
fn contour_wire(contour: &Contour, origin: Point2) -> PyResult<Shape> {
    let mut edges = Vec::with_capacity(contour.commands.len());
    let mut current = placed(contour.start, origin);
    for command in &contour.commands {
        match *command {
            PathCommand::LineTo(to) => {
                let to = placed(to, origin);
                if to.distance(&current) > SAME_POINT {
                    let segment = LineSegment::new(current, to).map_err(curve_error)?;
                    edges.push(shape_from_curve(&EdgeCurve::LineSegment(segment))?);
                }
                current = to;
            }
            PathCommand::QuadTo { control, to } => {
                let to = placed(to, origin);
                edges.push(bezier(vec![current, placed(control, origin), to])?);
                current = to;
            }
            PathCommand::CubicTo { c1, c2, to } => {
                let to = placed(to, origin);
                edges.push(bezier(vec![
                    current,
                    placed(c1, origin),
                    placed(c2, origin),
                    to,
                ])?);
                current = to;
            }
        }
    }
    backend().make_wire(&edges).map_err(kernel_error)
}

/// An exact Bézier edge through `poles` (three for a conic, four for a cubic).
fn bezier(poles: Vec<Vector3>) -> PyResult<Shape> {
    let weights = vec![1.0; poles.len()];
    let curve = BezierCurve::new(poles, weights).map_err(curve_error)?;
    shape_from_curve(&EdgeCurve::BezierCurve(curve))
}

/// A contour point in the text's frame.
fn placed(point: Point2, origin: Point2) -> Vector3 {
    Vector3::new(point.x + origin.x, point.y + origin.y, 0.0)
}

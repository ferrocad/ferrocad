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

use std::path::{Path, PathBuf};

use ferrocad_curves::{BezierCurve, EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::Shape;
use ferrocad_part::backend;
use ferrocad_text::{Contour, PathCommand, Point2};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString};

use crate::PartShape;
use crate::curves::{OCCError, curve_error, kernel_error, shape_from_curve};

/// Two points closer than this (mm) are the same point, matching
/// `ferrocad_text`'s contour builder.
const SAME_POINT: f64 = 1e-9;

/// `Part.makeWireString(text, dir, fontfile, height, tracking=0)` or
/// `Part.makeWireString(text, fontspec, height, tracking=0)`.
///
/// Returns one list per character, in text order, each holding that character's
/// closed wires — upstream's `list[list[Wire]]`. A character with no outline (a
/// space) gives an empty inner list.
///
/// Upstream accepts **two** argument shapes (`AppPartPy.cpp:2341`): the old
/// `(text, dir, fontfile, height[, tracking])`, and the newer
/// `(text, fontspec, height[, tracking])` where `fontspec` is a full path. Draft's
/// ShapeString uses the second, so both are supported here and the choice is made
/// by the type of the second argument (a string file name, or a number).
#[pyfunction]
#[pyo3(signature = (text, a, b, c = None, d = None))]
pub(crate) fn makeWireString<'py>(
    py: Python<'py>,
    text: &str,
    a: &Bound<'py, PyAny>,
    b: &Bound<'py, PyAny>,
    c: Option<&Bound<'py, PyAny>>,
    d: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyList>> {
    let (path, height, tracking) = font_and_size(a, b, c, d)?;
    if !height.is_finite() || height <= 0.0 {
        return Err(OCCError::new_err(
            "makeWireString: the text height must be greater than zero",
        ));
    }
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

/// Resolve upstream's two argument shapes to `(path, height, tracking)`.
fn font_and_size(
    a: &Bound<'_, PyAny>,
    b: &Bound<'_, PyAny>,
    c: Option<&Bound<'_, PyAny>>,
    d: Option<&Bound<'_, PyAny>>,
) -> PyResult<(PathBuf, f64, f64)> {
    let as_string = |value: &Bound<'_, PyAny>, what: &str| -> PyResult<String> {
        value
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err(format!("makeWireString: {what} must be a string")))
    };
    let as_number = |value: &Bound<'_, PyAny>, what: &str| -> PyResult<f64> {
        value
            .extract::<f64>()
            .map_err(|_| PyTypeError::new_err(format!("makeWireString: {what} must be a number")))
    };
    let optional_number = |value: Option<&Bound<'_, PyAny>>, what: &str| -> PyResult<f64> {
        match value {
            Some(value) => as_number(value, what),
            None => Ok(0.0),
        }
    };

    if b.is_instance_of::<PyString>() {
        // (text, dir, fontfile, height[, tracking])
        let dir = as_string(a, "the font directory")?;
        let file = as_string(b, "the font file name")?;
        let Some(height) = c else {
            return Err(PyTypeError::new_err(
                "makeWireString: missing height (dir, fontfile, height)",
            ));
        };
        Ok((
            Path::new(&dir).join(file),
            as_number(height, "height")?,
            optional_number(d, "tracking")?,
        ))
    } else {
        // (text, fontspec, height[, tracking])
        if d.is_some() {
            return Err(PyTypeError::new_err(
                "makeWireString takes at most four arguments as (fontspec, height, tracking)",
            ));
        }
        let spec = as_string(a, "the font file")?;
        Ok((
            PathBuf::from(spec),
            as_number(b, "height")?,
            optional_number(c, "tracking")?,
        ))
    }
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

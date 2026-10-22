//! `Part::FaceMakerUnified` on self-intersecting boundaries, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// A closed wire through `points`, each an `(x, y)` pair on the XY plane.
fn polygon<'py>(part: &Bound<'py, PyModule>, points: &[(f64, f64)]) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(part.py(), "ferrocad").expect("import ferrocad");
    let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
    let mut items = Vec::with_capacity(points.len() + 1);
    for &(x, y) in points.iter().chain(points.first()) {
        items.push(vector.call1((x, y, 0.0)).expect("a ferrocad.Vector"));
    }
    let list = PyList::new(part.py(), &items).expect("point list");
    part.call_method1("makePolygon", (&list,))
        .expect("Part.makePolygon(points)")
}

/// A `Part.Wire` around a single edge.
fn wire_of<'py>(part: &Bound<'py, PyModule>, edge: &Bound<'py, PyAny>) -> Bound<'py, PyAny> {
    let list = PyList::new(part.py(), [edge]).expect("edge list");
    part.getattr("Wire")
        .expect("Part.Wire")
        .call1((&list,))
        .expect("Part.Wire([edge])")
}

/// A self-intersecting (figure-eight) B-spline used by the upstream tests.
fn figure_eight_bspline<'py>(part: &Bound<'py, PyModule>) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(part.py(), "ferrocad").expect("import ferrocad");
    let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
    let poles = [
        (0.0, 26.06),
        (14.6, 15.54),
        (0.51, 0.0),
        (-16.13, -17.23),
        (0.0, -24.02),
        (17.15, -9.42),
        (-16.64, 15.20),
        (0.0, 26.06),
    ];
    let points: Vec<_> = poles
        .iter()
        .map(|&(x, y)| vector.call1((x, y, 0.0)).expect("a ferrocad.Vector"))
        .collect();
    let point_list = PyList::new(part.py(), &points).expect("pole list");
    let knot_sequence = PyList::new(part.py(), [1; 8]).expect("knot sequence");
    let curve = part
        .getattr("BSplineCurve")
        .expect("Part.BSplineCurve")
        .call1((
            &point_list,
            part.py().None(),
            part.py().None(),
            false,
            3,
            &knot_sequence,
            false,
        ))
        .expect("Part.BSplineCurve(poles, ...)");
    let edge = curve.call_method0("toShape").expect("BSplineCurve.toShape");
    wire_of(part, &edge)
}

fn make_unified<'py>(part: &Bound<'py, PyModule>, wires: &Bound<'py, PyAny>) -> Bound<'py, PyAny> {
    part.call_method1("makeFace", (wires, "Part::FaceMakerUnified"))
        .expect("Part.makeFace(..., \"Part::FaceMakerUnified\")")
}

fn face_count(shape: &Bound<'_, PyAny>) -> usize {
    shape
        .getattr("Faces")
        .expect("Shape.Faces")
        .len()
        .expect("face count")
}

fn total_area(shape: &Bound<'_, PyAny>) -> f64 {
    let faces = shape.getattr("Faces").expect("Shape.Faces");
    (0..faces.len().expect("face count"))
        .map(|index| {
            faces
                .get_item(index)
                .expect("a face")
                .getattr("Area")
                .expect("Face.Area")
                .extract::<f64>()
                .expect("area as f64")
        })
        .sum()
}

#[test]
fn unified_splits_crossing_polygon() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        // Two edges cross in the middle, so one wire bounds two triangles.
        let wire = polygon(
            &part,
            &[(-10.0, -5.0), (10.0, 5.0), (10.0, -5.0), (-10.0, 5.0)],
        );
        let faces = make_unified(&part, &wire);
        assert!(
            face_count(&faces) >= 2,
            "crossing edges must split into at least two regions, got {}",
            face_count(&faces)
        );
        assert!(total_area(&faces) > 0.0, "the split regions must have area");
    });
}

#[test]
fn unified_splits_figure_eight_polygon() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        // Two diamonds meeting at (5, 0): one wire, two bounded regions.
        let wire = polygon(
            &part,
            &[
                (5.0, 0.0),
                (0.0, 5.0),
                (-5.0, 0.0),
                (0.0, -5.0),
                (5.0, 0.0),
                (10.0, 5.0),
                (15.0, 0.0),
                (10.0, -5.0),
            ],
        );
        let faces = make_unified(&part, &wire);
        assert!(
            face_count(&faces) >= 2,
            "a figure-eight polygon must split into at least two regions, got {}",
            face_count(&faces)
        );
        assert!(total_area(&faces) > 0.0, "the split regions must have area");
    });
}

#[test]
fn unified_splits_figure_eight_bspline() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        // A single closed B-spline edge crossing itself once: one edge, two lobes.
        let wire = figure_eight_bspline(&part);
        let faces = make_unified(&part, &wire);
        assert!(
            face_count(&faces) >= 2,
            "a self-intersecting B-spline must split into at least two regions, got {}",
            face_count(&faces)
        );
        assert!(total_area(&faces) > 0.0, "the lobes must have area");
    });
}

#[test]
fn unified_keeps_circle_regions_with_self_intersecting_bspline() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let circle = |x: f64, y: f64| {
            let center = vector.call1((x, y, 0.0)).expect("center");
            let edge = part
                .call_method1("makeCircle", (10.0, center))
                .expect("Part.makeCircle");
            wire_of(&part, &edge)
        };
        // The B-spline's two lobes plus the three Venn regions of the circles.
        let children = PyList::new(
            py,
            [
                figure_eight_bspline(&part),
                circle(0.0, 0.0),
                circle(8.0, 0.0),
            ],
        )
        .expect("wire list");
        let compound = part
            .getattr("Compound")
            .expect("Part.Compound")
            .call1((&children,))
            .expect("Part.Compound([bspline, circle, circle])");
        let faces = make_unified(&part, &compound);
        assert!(
            face_count(&faces) >= 5,
            "two B-spline lobes plus three circle regions expected, got {}",
            face_count(&faces)
        );
    });
}

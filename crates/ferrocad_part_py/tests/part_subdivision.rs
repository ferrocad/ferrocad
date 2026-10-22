//! `Part::FaceMakerUnified` on open-wire cutters: subdivision end to end.
//!
//! Same boot as `part_unified`: `ferrocad` + `Part` as built-ins, one core, one
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

/// A wire through `points`, each an `(x, y)` pair on the XY plane. Two points
/// give an open line, more (with the last repeating the first) a closed polygon.
fn polyline<'py>(part: &Bound<'py, PyModule>, points: &[(f64, f64)]) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(part.py(), "ferrocad").expect("import ferrocad");
    let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
    let items: Vec<_> = points
        .iter()
        .map(|&(x, y)| vector.call1((x, y, 0.0)).expect("a ferrocad.Vector"))
        .collect();
    let list = PyList::new(part.py(), &items).expect("point list");
    part.call_method1("makePolygon", (&list,))
        .expect("Part.makePolygon(points)")
}

/// A closed rectangle wire from `(x0, y0)` to `(x1, y1)`.
fn rect<'py>(
    part: &Bound<'py, PyModule>,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> Bound<'py, PyAny> {
    let polygon = polyline(part, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]);
    part.getattr("Wire")
        .expect("Part.Wire")
        .call1((polygon,))
        .expect("Part.Wire(polygon)")
}

/// A circle wire of radius `r` centred at `(cx, cy)`.
fn circle<'py>(part: &Bound<'py, PyModule>, cx: f64, cy: f64, r: f64) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(part.py(), "ferrocad").expect("import ferrocad");
    let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
    let center = vector.call1((cx, cy, 0.0)).expect("centre");
    let edge = part
        .call_method1("makeCircle", (r, center))
        .expect("Part.makeCircle");
    part.call_method1("Wire", (edge,)).expect("Part.Wire(edge)")
}

/// `Part.makeFace(Part.Compound(wires), "Part::FaceMakerUnified")`.
fn unified<'py>(
    part: &Bound<'py, PyModule>,
    wires: &[Bound<'py, PyAny>],
) -> Bound<'py, PyAny> {
    let list = PyList::new(part.py(), wires).expect("wire list");
    let compound = part
        .getattr("Compound")
        .expect("Part.Compound")
        .call1((&list,))
        .expect("Part.Compound([wires])");
    part.call_method1("makeFace", (compound, "Part::FaceMakerUnified"))
        .expect("Part.makeFace(..., \"Part::FaceMakerUnified\")")
}

fn faces<'py>(shape: &Bound<'py, PyAny>) -> Vec<Bound<'py, PyAny>> {
    let faces = shape.getattr("Faces").expect("Shape.Faces");
    (0..faces.len().expect("face count"))
        .map(|index| faces.get_item(index).expect("a face"))
        .collect()
}

fn area(face: &Bound<'_, PyAny>) -> f64 {
    face.getattr("Area")
        .expect("Face.Area")
        .extract::<f64>()
        .expect("area as f64")
}

fn wire_count(face: &Bound<'_, PyAny>) -> usize {
    face.getattr("Wires")
        .expect("Face.Wires")
        .len()
        .expect("wire count")
}

fn total_area(shape: &Bound<'_, PyAny>) -> f64 {
    faces(shape).iter().map(area).sum()
}

fn assert_area(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-6 * expected.max(1.0),
        "area {actual} should be {expected}"
    );
}

#[test]
fn cross_of_two_open_lines_splits_the_rectangle_into_quarters() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(0.0, 5.0), (10.0, 5.0)]),
                polyline(&part, &[(5.0, 0.0), (5.0, 10.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 4, "the cutters quarter the rectangle");
        assert_area(total_area(&shape), 100.0);
        for region in &regions {
            assert_area(area(region), 25.0);
        }
    });
}

#[test]
fn diagonal_plus_open_line_leaves_two_halves() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(0.0, 0.0), (10.0, 10.0)]),
                polyline(&part, &[(10.0, 5.0), (7.0, 5.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 2, "the dangling spur does not add a region");
        assert_area(total_area(&shape), 100.0);
    });
}

#[test]
fn dangling_chain_does_not_split_the_rectangle() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(5.0, 0.0), (5.0, 4.0)]),
                polyline(&part, &[(5.0, 4.0), (7.0, 6.0)]),
                polyline(&part, &[(7.0, 6.0), (4.0, 8.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 1, "a chain ending inside the face is a slit");
        assert_area(area(&regions[0]), 100.0);
    });
}

#[test]
fn floating_line_inside_the_rectangle_is_ignored() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(3.0, 3.0), (7.0, 7.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(
            regions.len(),
            1,
            "a line that touches no boundary cannot split the face"
        );
        assert_area(area(&regions[0]), 100.0);
    });
}

#[test]
fn incomplete_intersection_keeps_one_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(5.0, 0.0), (5.0, 5.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(
            regions.len(),
            1,
            "a slit from the boundary to an interior point is not a cut"
        );
        assert_area(area(&regions[0]), 100.0);
    });
}

#[test]
fn t_junction_splits_into_three_regions() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                rect(&part, 0.0, 0.0, 10.0, 10.0),
                polyline(&part, &[(0.0, 0.0), (10.0, 10.0)]),
                polyline(&part, &[(5.0, 0.0), (5.0, 5.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 3, "the spur stops on the diagonal");
        assert_area(total_area(&shape), 100.0);
    });
}

#[test]
fn circle_with_dangling_radius_stays_a_clean_disc() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                circle(&part, 0.0, 0.0, 10.0),
                polyline(&part, &[(0.0, 0.0), (15.0, 0.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 1, "the radius is dangling, not a cut");
        assert_area(area(&regions[0]), std::f64::consts::PI * 100.0);
        assert_eq!(
            wire_count(&regions[0]),
            1,
            "the dangling radius must not survive as an internal wire"
        );
    });
}

#[test]
fn circle_with_line_through_splits_into_two_disc_halves() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = unified(
            &part,
            &[
                circle(&part, 0.0, 0.0, 10.0),
                polyline(&part, &[(-15.0, 0.0), (15.0, 0.0)]),
            ],
        );
        let regions = faces(&shape);
        assert_eq!(regions.len(), 2, "the chord cuts the disc in half");
        assert_area(total_area(&shape), std::f64::consts::PI * 100.0);
        for region in &regions {
            assert_eq!(wire_count(region), 1);
        }
    });
}

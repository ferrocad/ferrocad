//! The OCCT surface-operation seam and the Part consumers wired to it:
//! partial spheres, variable-radius fillets, filled planar offsets, solids with
//! cavities, exact-by-construction helices and pipe shells.
//!
//! The first half calls [`ferrocad_occt::surface_ops`] directly (the seam the
//! Python layer will call); the second half exercises the Part paths that now
//! use those operations (`Part.Solid` cavities, `Part::Helix`, `Part.makeThread`,
//! `Part.Sweep` and partial revolutions).
//!
//! It needs OCCT (see `docs/architecture/occt-bundling.md`).

use std::f64::consts::{PI, TAU};

use ferrocad_curves::{Circle, EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::{ElementKind, ElementRef, GeometryBackend, Shape};
use ferrocad_occt::{surface_ops, OcctBackend};
use ferrocad_types::{Placement, Rotation};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

fn backend() -> OcctBackend {
    OcctBackend::new()
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what} was {actual}, expected {expected}"
    );
}

fn volume(backend: &OcctBackend, shape: &Shape) -> f64 {
    backend
        .mass_properties(shape)
        .unwrap_or_else(|| panic!("mass properties for {shape:?}"))
        .volume
}

fn area(backend: &OcctBackend, shape: &Shape) -> f64 {
    backend
        .mass_properties(shape)
        .unwrap_or_else(|| panic!("mass properties for {shape:?}"))
        .area
}

fn length(backend: &OcctBackend, shape: &Shape) -> f64 {
    backend
        .mass_properties(shape)
        .unwrap_or_else(|| panic!("mass properties for {shape:?}"))
        .length
}

fn bounds(backend: &OcctBackend, shape: &Shape) -> (Vector3, Vector3) {
    backend
        .bounds(shape)
        .unwrap_or_else(|| panic!("bounds for {shape:?}"))
}

fn elements(backend: &OcctBackend, shape: &Shape, kind: ElementKind) -> Vec<ElementRef> {
    backend.elements(shape, kind)
}

/// The single shell of `shape`.
fn shell_of(backend: &OcctBackend, shape: &Shape) -> Shape {
    let mut shells = elements(backend, shape, ElementKind::Shell);
    assert_eq!(shells.len(), 1, "expected one shell");
    shells.remove(0).shape
}

fn placed_box(backend: &OcctBackend, size: f64, base: Vector3) -> Shape {
    let boxed = backend.make_box(size, size, size).expect("make_box");
    backend
        .place(
            &boxed,
            &Placement {
                base,
                rotation: Rotation::identity(),
            },
        )
        .expect("place")
}

fn line_edge(backend: &OcctBackend, start: Vector3, end: Vector3) -> Shape {
    backend
        .make_edge(&EdgeCurve::LineSegment(
            LineSegment::new(start, end).expect("line segment"),
        ))
        .expect("make_edge")
}

fn circle_edge(backend: &OcctBackend, center: Vector3, normal: Vector3, radius: f64) -> Shape {
    let circle = Circle::with_axis(center, normal, Vector3::new(1.0, 0.0, 0.0), radius)
        .expect("circle");
    backend
        .make_edge(&EdgeCurve::Circle(circle))
        .expect("make_edge")
}

fn vertex_points(backend: &OcctBackend, shape: &Shape) -> Vec<Vector3> {
    elements(backend, shape, ElementKind::Vertex)
        .iter()
        .filter_map(|vertex| backend.vertex_point(&vertex.shape))
        .collect()
}

// ---------------------------------------------------------------------------
// The seam itself
// ---------------------------------------------------------------------------

/// The full sphere is the primitive, and every angle pair gives the exact
/// fraction of it.
#[test]
fn sphere_slices_are_exact() {
    let backend = backend();
    let radius = 2.0;
    let full = surface_ops::make_sphere_slice(
        radius,
        Vector3::zero(),
        0.0,
        360.0,
        -90.0,
        90.0,
    )
    .expect("full sphere");
    assert_eq!(elements(&backend, &full, ElementKind::Solid).len(), 1);
    assert!(backend.is_valid(&full));
    assert_close(
        volume(&backend, &full),
        4.0 / 3.0 * PI * radius.powi(3),
        "full sphere volume",
    );
    assert_close(
        area(&backend, &full),
        4.0 * PI * radius * radius,
        "full sphere area",
    );

    // The northern hemisphere: half the volume, z >= 0.
    let hemisphere = surface_ops::make_sphere_slice(
        radius,
        Vector3::zero(),
        0.0,
        360.0,
        0.0,
        90.0,
    )
    .expect("northern hemisphere");
    assert_close(
        volume(&backend, &hemisphere),
        2.0 / 3.0 * PI * radius.powi(3),
        "hemisphere volume",
    );
    let (min, max) = bounds(&backend, &hemisphere);
    assert!(min.z >= -1e-9, "hemisphere zmin was {}", min.z);
    assert_close(max.z, radius, "hemisphere zmax");

    // A quarter wedge: a quarter volume in +X/+Y, and rotating u1 moves it.
    let quarter = surface_ops::make_sphere_slice(
        radius,
        Vector3::zero(),
        0.0,
        90.0,
        -90.0,
        90.0,
    )
    .expect("quarter sphere");
    assert_close(
        volume(&backend, &quarter),
        1.0 / 3.0 * PI * radius.powi(3),
        "quarter sphere volume",
    );
    let (min, max) = bounds(&backend, &quarter);
    assert!(min.x >= -1e-9 && min.y >= -1e-9, "the wedge is in +X/+Y");
    assert_close(max.x, radius, "quarter xmax");
    assert_close(max.y, radius, "quarter ymax");

    let rotated = surface_ops::make_sphere_slice(
        radius,
        Vector3::zero(),
        90.0,
        180.0,
        -90.0,
        90.0,
    )
    .expect("rotated quarter sphere");
    assert_close(
        volume(&backend, &rotated),
        1.0 / 3.0 * PI * radius.powi(3),
        "rotated quarter volume",
    );
    let (min, max) = bounds(&backend, &rotated);
    assert!(max.x <= 1e-9 && min.y >= -1e-9, "the rotated wedge is in -X/+Y");

    // The centre places the slice, not only the full sphere.
    let placed = surface_ops::make_sphere_slice(
        radius,
        Vector3::new(10.0, 0.0, 0.0),
        0.0,
        360.0,
        -90.0,
        90.0,
    )
    .expect("placed sphere");
    let (min, max) = bounds(&backend, &placed);
    assert_close(min.x, 8.0, "placed xmin");
    assert_close(max.x, 12.0, "placed xmax");
}

/// A radius law from 1 to 2 lies between the constant fillets, and every edge
/// can carry its own law.
#[test]
fn variable_fillets_round_the_edges() {
    let backend = backend();
    let boxed = backend.make_box(10.0, 10.0, 10.0).expect("box");
    let edges = elements(&backend, &boxed, ElementKind::Edge);
    assert_eq!(edges.len(), 12, "a box has twelve edges");
    let shapes: Vec<Shape> = edges.iter().map(|edge| edge.shape.clone()).collect();

    let variable = surface_ops::fillet_variable(
        &boxed,
        &shapes,
        &vec![1.0; shapes.len()],
        &vec![2.0; shapes.len()],
    )
    .expect("variable fillet");
    assert!(backend.is_valid(&variable), "the variable fillet is valid");
    let variable_volume = volume(&backend, &variable);

    let small = backend
        .fillet(&boxed, &edges, 1.0)
        .expect("radius 1 fillet")
        .shape;
    let large = backend
        .fillet(&boxed, &edges, 2.0)
        .expect("radius 2 fillet")
        .shape;
    let small_volume = volume(&backend, &small);
    let large_volume = volume(&backend, &large);
    assert!(
        variable_volume < small_volume && variable_volume > large_volume,
        "a 1->2 radius law ({variable_volume}) lies between {small_volume} and {large_volume}"
    );

    // Per-edge laws: the first edge keeps a constant radius, the rest vary.
    let mut starts = vec![1.5; shapes.len()];
    let ends = vec![2.0; shapes.len()];
    starts[0] = 0.75;
    let mixed = surface_ops::fillet_variable(&boxed, &shapes, &starts, &ends)
        .expect("per-edge fillet");
    assert!(backend.is_valid(&mixed));

    // Mismatched lists are rejected before OCCT sees them.
    assert!(surface_ops::fillet_variable(&boxed, &shapes, &[1.0], &[2.0]).is_err());
    assert!(surface_ops::fillet_variable(&boxed, &[], &[], &[]).is_err());
}

/// A filled offset is the band between the source and its offset; an unfilled
/// one is the offset boundary.
#[test]
fn offset2d_fills_the_band() {
    let backend = backend();
    let circle = circle_edge(&backend, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 10.0);
    let wire = backend.make_wire(&[circle]).expect("circle wire");

    let filled = surface_ops::offset2d(&wire, 2.0, 0, true, false).expect("filled offset");
    assert_eq!(elements(&backend, &filled, ElementKind::Face).len(), 1);
    assert!(backend.is_valid(&filled));
    assert_close(
        area(&backend, &filled),
        PI * (12.0 * 12.0 - 10.0 * 10.0),
        "filled offset area",
    );

    let plain = surface_ops::offset2d(&wire, 2.0, 0, false, false).expect("plain offset");
    assert_close(length(&backend, &plain), TAU * 12.0, "offset boundary length");

    // Inward offsets fill the same band from the other side.
    let inward = surface_ops::offset2d(&wire, -2.0, 0, true, false).expect("inward offset");
    assert_close(
        area(&backend, &inward),
        PI * (10.0 * 10.0 - 8.0 * 8.0),
        "inward offset area",
    );

    // An open wire is closed with two connectors when `open_result` is set.
    let first = line_edge(&backend, Vector3::zero(), Vector3::new(10.0, 0.0, 0.0));
    let second = line_edge(
        &backend,
        Vector3::new(10.0, 0.0, 0.0),
        Vector3::new(10.0, 10.0, 0.0),
    );
    let corner = backend.make_wire(&[first, second]).expect("corner wire");
    let band = surface_ops::offset2d(&corner, 2.0, 0, true, true).expect("open band");
    // Two 2-wide strips along the 10-long legs plus the quarter arc at the
    // corner (join kind 0).
    assert_close(area(&backend, &band), 40.0 + PI, "open band area");

    // A filled zero offset is rejected rather than silently accepted.
    assert!(surface_ops::offset2d(&wire, 0.0, 0, true, false).is_err());
    assert!(surface_ops::offset2d(&wire, 1.0, 7, false, false).is_err());
}

/// The first shell is the outer skin and the rest remove material.
#[test]
fn hollow_solids_have_cavities() {
    let backend = backend();
    let outer = backend.make_box(10.0, 10.0, 10.0).expect("outer box");
    let inner = placed_box(&backend, 4.0, Vector3::new(3.0, 3.0, 3.0));
    let solid = surface_ops::make_hollow_solid(&[
        shell_of(&backend, &outer),
        shell_of(&backend, &inner),
    ])
    .expect("hollow solid");
    assert_eq!(elements(&backend, &solid, ElementKind::Solid).len(), 1);
    assert!(backend.is_valid(&solid), "the hollow solid is valid");
    assert_close(volume(&backend, &solid), 1000.0 - 64.0, "hollow volume");

    // With one shell it is the solid of that shell.
    let single = surface_ops::make_hollow_solid(&[shell_of(&backend, &outer)])
        .expect("single-shell solid");
    assert_close(volume(&backend, &single), 1000.0, "single shell volume");
}

/// The helix edge is exact on its analytic surface: endpoints, length and the
/// conical widening all follow from the construction.
#[test]
fn helix_edges_are_exact_by_construction() {
    let backend = backend();
    let helix = surface_ops::make_helix_edge(1.0, 4.0, 2.0, 0.0, false).expect("helix");
    assert_eq!(elements(&backend, &helix, ElementKind::Edge).len(), 1);
    assert!(backend.is_valid(&helix));
    // Four turns of pitch 1 at radius 2.
    let expected = 4.0 * ((TAU * 2.0).powi(2) + 1.0).sqrt();
    assert!(
        (length(&backend, &helix) - expected).abs() < 1e-3,
        "helix length was {}, expected {expected}",
        length(&backend, &helix)
    );
    let (min, max) = bounds(&backend, &helix);
    assert_close(min.z, 0.0, "helix zmin");
    assert_close(max.z, 4.0, "helix zmax");
    assert!((max.x - 2.0).abs() < 1e-3, "helix xmax was {}", max.x);
    let points = vertex_points(&backend, &helix);
    assert!(
        points.iter().any(|p| (p.x - 2.0).abs() < 1e-9
            && p.y.abs() < 1e-9
            && p.z.abs() < 1e-9),
        "the helix starts at (2, 0, 0): {points:?}"
    );

    // A left-handed helix mirrors the turn at the far end.
    let right = surface_ops::make_helix_edge(1.0, 0.75, 2.0, 0.0, false).expect("right helix");
    let left = surface_ops::make_helix_edge(1.0, 0.75, 2.0, 0.0, true).expect("left helix");
    let right_end = vertex_points(&backend, &right)
        .into_iter()
        .find(|point| (point.z - 0.75).abs() < 1e-9)
        .expect("the right helix end");
    let left_end = vertex_points(&backend, &left)
        .into_iter()
        .find(|point| (point.z - 0.75).abs() < 1e-9)
        .expect("the left helix end");
    assert!((right_end.x - left_end.x).abs() < 1e-9);
    assert!(
        right_end.y * left_end.y < 0.0,
        "the handedness changes the final side: {} vs {}",
        right_end.y,
        left_end.y
    );

    // Conical: the radius grows by height * tan(10 degrees) while z stays 4.
    let conical = surface_ops::make_helix_edge(1.0, 4.0, 2.0, 10.0, false).expect("conical helix");
    let (_, max) = bounds(&backend, &conical);
    assert_close(max.z, 4.0, "conical helix zmax");
    assert!(
        (max.x - (2.0 + 4.0 * 10.0_f64.to_radians().tan())).abs() < 0.01,
        "conical xmax was {}, expected about {}",
        max.x,
        2.0 + 4.0 * 10.0_f64.to_radians().tan()
    );

    assert!(surface_ops::make_helix_edge(0.0, 4.0, 2.0, 0.0, false).is_err());
    assert!(surface_ops::make_helix_edge(1.0, 4.0, 0.0, 0.0, false).is_err());
    assert!(surface_ops::make_helix_edge(1.0, -4.0, 2.0, 0.0, false).is_err());
}

/// The pipe shell sweeps one profile into a shell and closes it into a solid.
#[test]
fn pipe_shells_sweep_profiles() {
    let backend = backend();
    let spine = line_edge(&backend, Vector3::zero(), Vector3::new(0.0, 0.0, 10.0));
    let profile = circle_edge(&backend, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 1.0);

    let shell = surface_ops::make_pipe_shell(&spine, &[profile.clone()], false, false)
        .expect("pipe shell");
    assert_eq!(elements(&backend, &shell, ElementKind::Shell).len(), 1);
    assert!(backend.is_valid(&shell));
    assert_close(area(&backend, &shell), TAU * 10.0, "pipe lateral area");

    let solid =
        surface_ops::make_pipe_shell(&spine, &[profile], true, false).expect("pipe solid");
    assert_eq!(elements(&backend, &solid, ElementKind::Solid).len(), 1);
    assert!(backend.is_valid(&solid));
    assert_close(volume(&backend, &solid), PI * 10.0, "pipe volume");

    assert!(surface_ops::make_pipe_shell(&spine, &[], false, false).is_err());
}

/// The spine adaptors evaluate any edge: the straight-edge direction honours
/// orientation, and a B-spline exposes the frame the analytic `curve_of` seam
/// cannot describe. The plane finder gives an extrusion's `Normal` mode.
#[test]
fn spine_frames_and_plane_normals() {
    let backend = backend();

    let line = line_edge(&backend, Vector3::zero(), Vector3::new(0.0, 3.0, 0.0));
    let (start, tangent) = surface_ops::spine_start_frame(&line).expect("line spine frame");
    assert_close(start.x, 0.0, "line start x");
    assert_close(start.y, 0.0, "line start y");
    assert_close(tangent.x, 0.0, "line tangent x");
    assert_close(tangent.y, 1.0, "line tangent y");
    let direction = surface_ops::edge_line_direction(&line).expect("line direction");
    assert_close(direction.y, 3.0, "the line's unnormalised direction");

    // A reversed edge reports the opposite direction, as `fetchAxisLink` does.
    let reversed = backend.reversed(&line).expect("reverse the line");
    let reversed_direction =
        surface_ops::edge_line_direction(&reversed).expect("reversed line direction");
    assert_close(reversed_direction.y, -3.0, "the reversed direction");

    // A line-like interpolating B-spline: the adaptor gives its start frame.
    let bspline = backend
        .make_bspline_edge(
            &[
                Vector3::zero(),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            false,
        )
        .expect("B-spline edge");
    let (start, tangent) = surface_ops::spine_start_frame(&bspline).expect("B-spline frame");
    assert!(start.x.abs() < 1e-6 && start.y.abs() < 1e-6 && start.z.abs() < 1e-6);
    assert!(tangent.x > 0.99, "the B-spline starts along +X: {tangent:?}");

    // A circle lies in a plane whose normal is its axis (up to sign).
    let circle = circle_edge(
        &backend,
        Vector3::zero(),
        Vector3::new(0.0, 0.0, 1.0),
        2.0,
    );
    let normal = surface_ops::shape_plane_normal(&circle).expect("circle plane normal");
    assert_close(normal.z.abs(), 1.0, "the circle's plane normal");
    assert_close(normal.x, 0.0, "the normal has no x component");
    assert_close(normal.y, 0.0, "the normal has no y component");

    // A line is not a wire spine, but it is an edge: a non-edge is rejected.
    let boxed = backend.make_box(1.0, 1.0, 1.0).expect("box");
    assert!(surface_ops::spine_start_frame(&boxed).is_err());
}

// ---------------------------------------------------------------------------
// Part consumers
// ---------------------------------------------------------------------------

fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

fn vector(py: Python<'_>, x: f64, y: f64, z: f64) -> Bound<'_, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a vector")
}

fn number(value: &Bound<'_, PyAny>, what: &str) -> f64 {
    value
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{what} as f64"))
}

fn shape_type(shape: &Bound<'_, PyAny>) -> String {
    shape
        .getattr("ShapeType")
        .expect("Shape.ShapeType")
        .extract::<String>()
        .expect("ShapeType as str")
}

fn valid(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isValid")
        .expect("Shape.isValid")
        .extract::<bool>()
        .expect("isValid as bool")
}

fn py_volume(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Volume").expect("Shape.Volume"), "Volume")
}

fn py_area(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Area").expect("Shape.Area"), "Area")
}

fn py_length(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Length").expect("Shape.Length"), "Length")
}

/// `Part.Solid` with several shells makes a cavity, and `Part.makeSolid` keeps
/// working for the single-shell case.
#[test]
fn a_part_solid_accepts_cavity_shells() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let outer = part
            .call_method1("makeBox", (10.0, 10.0, 10.0))
            .expect("outer box");
        let inner = part
            .call_method1(
                "makeBox",
                (4.0, 4.0, 4.0, vector(py, 3.0, 3.0, 3.0)),
            )
            .expect("inner box");

        let solid = part
            .getattr("Solid")
            .expect("Part.Solid")
            .call1((&outer,))
            .expect("Part.Solid(outer)");
        assert_close(py_volume(&solid), 1000.0, "the outer shell alone");
        solid
            .call_method1("add", (&inner,))
            .expect("Solid.add(cavity)");
        assert_eq!(shape_type(&solid), "Solid");
        assert!(valid(&solid));
        assert_close(py_volume(&solid), 1000.0 - 64.0, "the cavity solid volume");
        let shells = solid
            .call_method0("__len__")
            .expect("len")
            .extract::<usize>()
            .expect("len as usize");
        assert_eq!(shells, 2, "the solid keeps both shells");

        // A compound of both boxes feeds the same constructor.
        let compound = part
            .call_method1("makeCompound", (PyList::new(py, [&outer, &inner]).expect("list"),))
            .expect("makeCompound");
        let from_compound = part
            .getattr("Solid")
            .expect("Part.Solid")
            .call1((&compound,))
            .expect("Part.Solid(compound)");
        assert_close(
            py_volume(&from_compound),
            1000.0 - 64.0,
            "the compound cavity volume",
        );

        // `Part.makeSolid` still promotes a single shell.
        let promoted = part
            .call_method1("makeSolid", (&outer,))
            .expect("Part.makeSolid");
        assert_eq!(shape_type(&promoted), "Solid");
        assert_close(py_volume(&promoted), 1000.0, "makeSolid volume");
    });
}

/// A partial revolution is exact, and asking for a `Part.Solid` promotes the
/// shell when the result is closed.
#[test]
fn partial_revolutions_and_the_solid_request() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let line = part
            .call_method1(
                "makeLine",
                (vector(py, 4.0, 0.0, 0.0), vector(py, 4.0, 0.0, 10.0)),
            )
            .expect("the line edge");

        let quarter = part
            .call_method1("makeRevolution", (&line, py.None(), py.None(), 90.0))
            .expect("a quarter revolution");
        assert_eq!(shape_type(&quarter), "Face");
        assert_close(py_area(&quarter), TAU * 4.0 * 10.0 / 4.0, "quarter area");

        // A face profile revolved 45 degrees is a solid wedge with planar caps.
        let polygon = PyList::new(
            py,
            [
                vector(py, 4.0, 0.0, 0.0),
                vector(py, 5.0, 0.0, 0.0),
                vector(py, 5.0, 0.0, 10.0),
                vector(py, 4.0, 0.0, 10.0),
            ],
        )
        .expect("profile points");
        let wire = part
            .call_method1("makePolygon", (&polygon, true))
            .expect("profile wire");
        let face = part
            .getattr("Face")
            .expect("Part.Face")
            .call1((&wire,))
            .expect("Part.Face(wire)");
        let solid = part
            .call_method1("makeRevolution", (&face, py.None(), py.None(), 45.0))
            .expect("a 45 degree revolution");
        assert_eq!(shape_type(&solid), "Solid");
        assert!(valid(&solid));
        assert_close(
            py_volume(&solid),
            PI * (25.0 - 16.0) * 10.0 / 8.0,
            "the 45 degree volume",
        );

        // Asking for `Part.Solid` explicitly is accepted (already a solid).
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs
            .set_item("shapetype", part.getattr("Solid").expect("Part.Solid"))
            .expect("shapetype keyword");
        let requested = part
            .getattr("makeRevolution")
            .expect("Part.makeRevolution")
            .call((&face, py.None(), py.None(), 45.0), Some(&kwargs))
            .expect("makeRevolution(shapetype=Part.Solid)");
        assert_eq!(shape_type(&requested), "Solid");
        assert_close(py_volume(&requested), py_volume(&solid), "requested volume");
    });
}

/// The `Part::Helix` object and `Part.makeThread` now use the exact helix edge.
#[test]
fn helix_consumers_use_the_exact_edge() {
    boot();
    Python::with_gil(|py| {
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let part = PyModule::import(py, "Part").expect("import Part");

        let expected = 2.0 * ((TAU * 1.0).powi(2) + 1.0).sqrt();
        let helix = part
            .call_method1("makeHelix", (1.0, 2.0, 1.0))
            .expect("Part.makeHelix");
        assert!(
            (py_length(&helix) - expected).abs() < 1e-3,
            "the helix length was {}",
            py_length(&helix)
        );

        let doc = freecad
            .call_method1("newDocument", ("ExactHelix",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::Helix", "Helix"))
            .expect("Part::Helix");
        object.setattr("Pitch", 1.0).expect("Pitch");
        object.setattr("Height", 2.0).expect("Height");
        object.setattr("Radius", 1.0).expect("Radius");
        doc.call_method0("recompute").expect("recompute");
        let shape = object.getattr("Shape").expect("the helix shape");
        assert_eq!(shape_type(&shape), "Edge");
        assert!(
            (py_length(&shape) - expected).abs() < 1e-3,
            "the Part::Helix length was {}",
            py_length(&shape)
        );

        // The thread is the same exact helix swept with a circular profile.
        let thread = part
            .call_method1("makeThread", (1.0, 0.5, 4.0, 2.0))
            .expect("Part.makeThread");
        assert_eq!(shape_type(&thread), "Shell");
        assert!(valid(&thread));

        freecad
            .call_method1("closeDocument", ("ExactHelix",))
            .expect("closeDocument");
    });
}

/// `Part::Sweep` now accepts several sections through the pipe shell.
#[test]
fn a_sweep_with_two_sections_builds() {
    boot();
    Python::with_gil(|py| {
        let freecad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let part = PyModule::import(py, "Part").expect("import Part");
        let doc = freecad
            .call_method1("newDocument", ("MultiSweep",))
            .expect("newDocument");

        let spine = doc
            .call_method1("addObject", ("Part::Feature", "Spine"))
            .expect("spine object");
        let spine_shape = part
            .call_method1(
                "makeLine",
                (vector(py, 0.0, 0.0, 0.0), vector(py, 0.0, 0.0, 10.0)),
            )
            .expect("spine line");
        spine.setattr("Shape", &spine_shape).expect("set Spine");

        let mut sections = Vec::new();
        for (name, z) in [("Bottom", 0.0), ("Top", 10.0)] {
            let section = doc
                .call_method1("addObject", ("Part::Feature", name))
                .expect("section object");
            let points = PyList::new(
                py,
                [
                    vector(py, -1.0, -1.0, z),
                    vector(py, 1.0, -1.0, z),
                    vector(py, 1.0, 1.0, z),
                    vector(py, -1.0, 1.0, z),
                ],
            )
            .expect("section points");
            let wire = part
                .call_method1("makePolygon", (&points, true))
                .expect("section wire");
            section.setattr("Shape", &wire).expect("set section shape");
            sections.push(section);
        }

        let sweep = doc
            .call_method1("addObject", ("Part::Sweep", "Sweep"))
            .expect("sweep object");
        sweep.setattr("Spine", &spine).expect("set Spine");
        let list = PyList::new(py, sections.iter()).expect("sections list");
        sweep.setattr("Sections", list).expect("set Sections");
        sweep.setattr("Solid", true).expect("set Solid");

        doc.call_method0("recompute").expect("recompute");
        let shape = sweep.getattr("Shape").expect("the sweep shape");
        assert_eq!(shape_type(&shape), "Solid");
        assert!(valid(&shape));
        assert_close(py_volume(&shape), 40.0, "the two-section sweep volume");

        freecad
            .call_method1("closeDocument", ("MultiSweep",))
            .expect("closeDocument");
    });
}

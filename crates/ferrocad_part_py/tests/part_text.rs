//! `Part.makeWireString`, end to end: a font's glyphs become closed wires.
//!
//! Same boot as `part_containers`: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. The fixture is the OFL font vendored by `ferrocad_text`
//! (see `crates/ferrocad_text/tests/fixtures/fonts/README.md`), so the test is
//! hermetic and offline.

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// The directory half of upstream's `makeWireString(text, dir, fontfile, …)`.
fn font_dir() -> String {
    format!(
        "{}/../ferrocad_text/tests/fixtures/fonts/",
        env!("CARGO_MANIFEST_DIR")
    )
}

const FONT_FILE: &str = "LiberationMono-Regular.ttf";

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

#[test]
fn a_string_is_one_list_of_closed_wires_per_character() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let letters = part
            .call_method1(
                "makeWireString",
                ("AB", font_dir(), FONT_FILE, 10.0),
            )
            .expect("makeWireString");

        assert_eq!(letters.len().expect("a sequence"), 2, "one list per character");
        for index in 0..2 {
            let wires = letters.get_item(index).expect("a character's list");
            let wires = wires
                .downcast::<PyList>()
                .expect("an inner list of wires");
            assert!(!wires.is_empty(), "character {index} has no wires");
            for wire in wires.iter() {
                assert!(
                    wire.call_method0("isClosed")
                        .expect("Wire.isClosed")
                        .extract::<bool>()
                        .unwrap(),
                    "every contour wire is closed"
                );
                let kind = wire
                    .getattr("ShapeType")
                    .expect("ShapeType")
                    .extract::<String>()
                    .unwrap();
                assert_eq!(kind, "Wire");
            }
        }
    });
}

#[test]
fn tracking_moves_the_next_glyph_by_its_value() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let plain = part
            .call_method1("makeWireString", ("AB", font_dir(), FONT_FILE, 10.0, 0.0))
            .expect("no tracking");
        let tracked = part
            .call_method1("makeWireString", ("AB", font_dir(), FONT_FILE, 10.0, 1.0))
            .expect("tracking 1 mm");

        let x_min = |letters: &Bound<'_, PyAny>, index: usize| -> f64 {
            let wires = letters.get_item(index).unwrap();
            let compound = part
                .getattr("Compound")
                .unwrap()
                .call1((wires,))
                .unwrap();
            let bound_box = compound.getattr("BoundBox").unwrap();
            bound_box.getattr("XMin").unwrap().extract::<f64>().unwrap()
        };

        let shifted = x_min(&tracked, 1) - x_min(&plain, 1);
        assert!(
            (shifted - 1.0).abs() < 1e-9,
            "the second glyph moved {shifted} mm, not 1 mm"
        );
        assert!(
            (x_min(&tracked, 0) - x_min(&plain, 0)).abs() < 1e-12,
            "tracking must not move the first glyph"
        );
    });
}

#[test]
fn a_space_has_an_empty_inner_list() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let letters = part
            .call_method1("makeWireString", (" ", font_dir(), FONT_FILE, 10.0))
            .expect("a space");
        let wires = letters.get_item(0).unwrap();
        let wires = wires.downcast::<PyList>().unwrap();
        assert!(wires.is_empty());
    });
}

#[test]
fn empty_text_is_an_empty_outer_list() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let letters = part
            .call_method1("makeWireString", ("", font_dir(), FONT_FILE, 10.0))
            .expect("empty text");
        assert_eq!(letters.len().unwrap(), 0);
    });
}

#[test]
fn a_missing_font_raises_occerror() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let error = part
            .call_method1("makeWireString", ("A", font_dir(), "no-such-font.ttf", 10.0))
            .expect_err("a missing font must raise");
        let name = error
            .get_type(py)
            .name()
            .expect("the exception has a name")
            .to_string();
        assert_eq!(name, "OCCError", "raised {name}: {error}");
        assert!(
            error.to_string().contains("cannot read the font file"),
            "the message names the file: {error}"
        );
    });
}

#[test]
fn a_non_positive_height_is_refused() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        for height in [0.0, -1.0] {
            let error = part
                .call_method1("makeWireString", ("A", font_dir(), FONT_FILE, height))
                .expect_err("a non-positive height must raise");
            assert_eq!(
                error.get_type(py).name().unwrap().to_string(),
                "OCCError"
            );
        }
    });
}

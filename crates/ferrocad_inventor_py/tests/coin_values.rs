//! The Coin value classes and camera nodes of the `pivy.coin` facade.
//!
//! SimpleCAD's snapping code builds its own `SbVec2f` for a pick ray
//! (`simplecad/gui/snapping.py:29`) and compares a camera's type with
//! `coin.SoPerspectiveCamera.getClassTypeId()` (`:31`, `gui/box_selection.py:95`).
//! Without these names the real snapping path raised `AttributeError` against the
//! shell even though the Python facade answered everything else.
//!
//! The assertions are written as Python (the workbench's own spelling), run against
//! the module the shell installs at boot, so they cover the surface a mod calls. Each
//! script assigns its answer to `result`.

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::PyDict;

mod common;
use common::pivy_coin;

/// Run a Python script with `coin` bound, returning the value it assigned to `result`.
fn script<'py>(py: Python<'py>, code: &str) -> Bound<'py, PyAny> {
    let coin = pivy_coin(py);
    let globals = PyDict::new(py);
    globals.set_item("coin", coin).unwrap();
    let code = CString::new(code).unwrap();
    py.run(&code, Some(&globals), None)
        .unwrap_or_else(|e| panic!("script raised: {e}"));
    globals
        .get_item("result")
        .unwrap()
        .expect("the script assigns `result`")
}

/// `snapping.ray` builds `coin.SbVec2f(u, v)` and reads it back with `getValue()`.
#[test]
fn sbvec2f_builds_and_reads_back() {
    Python::with_gil(|py| {
        let values: (f32, f32) = script(py, "result = coin.SbVec2f(0.25, 0.75).getValue()")
            .extract()
            .unwrap();
        assert_eq!(values, (0.25, 0.75));

        let x: f32 = script(py, "result = coin.SbVec2f(0.25, 0.75).x")
            .extract()
            .unwrap();
        assert_eq!(x, 0.25);
        let second: f32 = script(py, "result = coin.SbVec2f(0.25, 0.75)[1]")
            .extract()
            .unwrap();
        assert_eq!(second, 0.75);

        // Coin's flat setter form, arithmetic and the sequence protocol.
        let values: (f32, f32) = script(
            py,
            "v = coin.SbVec2f()\nv.setValue(0.5, 0.25)\nresult = v.getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(values, (0.5, 0.25));
        let summed: (f32, f32) = script(
            py,
            "result = (coin.SbVec2f(1, 2) + coin.SbVec2f(3, 4)).getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(summed, (4.0, 6.0));
        let listed: bool = script(py, "result = list(coin.SbVec2f(1, 2)) == [1.0, 2.0]")
            .extract()
            .unwrap();
        assert!(listed);
    });
}

/// `V(*near.getValue())` is the spelling the snap code uses on an `SbVec3f`.
#[test]
fn sbvec3f_round_trips_and_does_vector_maths() {
    Python::with_gil(|py| {
        let values: (f32, f32, f32) = script(py, "result = coin.SbVec3f(1, 0, 0).getValue()")
            .extract()
            .unwrap();
        assert_eq!(values, (1.0, 0.0, 0.0));

        let cross: (f32, f32, f32) = script(
            py,
            "result = coin.SbVec3f(1, 0, 0).cross(coin.SbVec3f(0, 1, 0)).getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(cross, (0.0, 0.0, 1.0));

        let length: f32 = script(py, "result = coin.SbVec3f(3, 4, 0).length()")
            .extract()
            .unwrap();
        assert!((length - 5.0).abs() < 1e-6);

        let values: (f32, f32, f32) = script(
            py,
            "result = (coin.SbVec3f(1, 2, 3) - coin.SbVec3f(1, 1, 1)).getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(values, (0.0, 1.0, 2.0));

        // `SbVec3f` is what a workbench stores in a node field, so the buffer form
        // must round-trip through `setValue`.
        let values: (f32, f32, f32) = script(
            py,
            "v = coin.SbVec3f()\nv.setValue((1.5, 2.5, 3.5))\nresult = v.getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(values, (1.5, 2.5, 3.5));
    });
}

/// `camera.orientation.getValue().multVec(coin.SbVec3f(0, 0, -1))` — box_selection
/// and viewport_style both derive the view direction this way.
#[test]
fn sbrotation_rotates_a_vector() {
    Python::with_gil(|py| {
        let values: (f32, f32, f32, f32) = script(
            py,
            "result = coin.SbRotation(0.70710677, 0, 0, 0.70710677).getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(
            values,
            (
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2
            )
        );

        // +90° about X maps -Z onto +Y.
        let rotated: (f32, f32, f32) = script(
            py,
            "r = coin.SbRotation(0.70710677, 0, 0, 0.70710677)\nresult = r.multVec(coin.SbVec3f(0, 0, -1)).getValue()",
        )
        .extract()
        .unwrap();
        assert!(rotated.0.abs() < 1e-5, "{rotated:?}");
        assert!((rotated.1 - 1.0).abs() < 1e-5, "{rotated:?}");
        assert!(rotated.2.abs() < 1e-5, "{rotated:?}");

        // The inverse undoes a rotation.
        let identity: bool = script(
            py,
            "r = coin.SbRotation(0.70710677, 0, 0, 0.70710677)\n\
             v = coin.SbVec3f(0, 0, -1)\n\
             back = r.inverse().multVec(r.multVec(v))\n\
             result = abs(back.x) < 1e-5 and abs(back.y) < 1e-5 and abs(back.z + 1) < 1e-5",
        )
        .extract()
        .unwrap();
        assert!(identity);
    });
}

/// `coin.SoPerspectiveCamera.getClassTypeId()` is the type id the facade's
/// `isOfType` compares against; it must exist on the class and name the type.
#[test]
fn the_camera_classes_expose_their_class_type_ids() {
    Python::with_gil(|py| {
        for class in ["SoPerspectiveCamera", "SoOrthographicCamera"] {
            let id: String = script(
                py,
                &format!("result = coin.{class}.getClassTypeId().getName()"),
            )
            .extract()
            .unwrap();
            assert_eq!(id, class);

            // The instance agrees with the class, and is an `SoCamera`.
            let own: String =
                script(py, &format!("result = coin.{class}().getTypeId().getName()"))
                    .extract()
                    .unwrap();
            assert_eq!(own, class);
            let is_a_camera: bool = script(
                py,
                &format!(
                    "c = coin.{class}()\n\
                     result = (c.isOfType(coin.{class}.getClassTypeId())\n\
                     and c.isOfType(coin.SoType.fromName('SoCamera'))\n\
                     and not c.isOfType(coin.SoType.fromName('SoGroup')))"
                ),
            )
            .extract()
            .unwrap();
            assert!(is_a_camera, "{class} is an SoCamera");

            // `createInstance` is how a workbench instantiates a node from a type id.
            let created: String = script(
                py,
                &format!(
                    "node = coin.SoType.fromName('{class}').createInstance()\n\
                     result = node.getTypeId().getName()"
                ),
            )
            .extract()
            .unwrap();
            assert_eq!(created, class);
        }
    });
}

/// `test_selection.py` constructs a camera and sets `camera.position.setValue(…)`;
/// `box_selection` reads it back with `getValue()`.
#[test]
fn camera_fields_round_trip() {
    Python::with_gil(|py| {
        let position: (f32, f32, f32) = script(
            py,
            "c = coin.SoPerspectiveCamera()\nc.position.setValue(0, 0, 100)\nresult = c.position.getValue().getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(position, (0.0, 0.0, 100.0));

        let focal: f32 = script(
            py,
            "c = coin.SoPerspectiveCamera()\nc.getField('focalDistance').setValue(42)\nresult = c.getField('focalDistance').getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(focal, 42.0);

        // The camel-case getters are the Coin spellings.
        let angle: f32 = script(
            py,
            "c = coin.SoOrthographicCamera()\nc.heightAngle.setValue(0.5)\nresult = c.heightAngle.getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(angle, 0.5);

        let unknown: bool = script(
            py,
            "result = coin.SoPerspectiveCamera().getField('noSuchField') is None",
        )
        .extract()
        .unwrap();
        assert!(unknown);

        // `camera_plan` reads the orientation as an `SbRotation` and rotates with it.
        let direction: (f32, f32, f32) = script(
            py,
            "c = coin.SoPerspectiveCamera()\nc.orientation.setValue(0, 0, 0, 1)\nresult = c.orientation.getValue().multVec(coin.SbVec3f(0, 0, -1)).getValue()",
        )
        .extract()
        .unwrap();
        assert_eq!(direction, (0.0, 0.0, -1.0));
    });
}

/// A camera is a node: it can be added to a group and extracts as an inert kind.
#[test]
fn a_camera_is_a_node_and_extracts_without_drawing() {
    Python::with_gil(|py| {
        let root = script(
            py,
            "root = coin.SoSeparator()\nroot.addChild(coin.SoOrthographicCamera())\nresult = root",
        );
        assert_eq!(
            root.call_method0("getNumChildren")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            1
        );

        let scene = ferrocad_inventor_py::extract_scene(&root).unwrap();
        let (queue, _) = ferrocad_inventor::compile(&scene);
        assert_eq!(queue.len(), 0, "a camera node draws nothing");
    });
}

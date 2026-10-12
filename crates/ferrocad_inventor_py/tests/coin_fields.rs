//! S2 tests: geometry field objects over the `pivy.coin` facade, via embedded CPython.
//!
//! These exercise the sequence protocol that workbench code relies on when it
//! mutates node fields in place (`coords.point[i] = …`, `faces.coordIndex.setValues(…)`),
//! and confirm the edits land on the wrapped `ferrocad_inventor` node (a live field,
//! not a copy).

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule};

mod common;
use common::pivy_coin;

fn new_node<'py>(coin: &Bound<'py, PyModule>, name: &str) -> Bound<'py, PyAny> {
    coin.getattr(name)
        .unwrap_or_else(|_| panic!("coin.{name}"))
        .call0()
        .unwrap_or_else(|_| panic!("coin.{name}()"))
}

#[test]
fn coordinate3_point_sequence_protocol() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let coords = new_node(&coin, "SoCoordinate3");
        let point = coords.getattr("point").unwrap();

        assert_eq!(point.len().unwrap(), 0);

        // setValues(offset, values) grows the buffer.
        point
            .call_method1(
                "setValues",
                (0usize, vec![(0.0f32, 0.0f32, 0.0f32), (1.0f32, 0.0f32, 0.0f32)]),
            )
            .unwrap();
        assert_eq!(point.len().unwrap(), 2);
        assert_eq!(
            point.get_item(0).unwrap().extract::<(f32, f32, f32)>().unwrap(),
            (0.0, 0.0, 0.0)
        );

        // __getitem__ supports Python-style negative indices.
        assert_eq!(
            point.get_item(-1).unwrap().extract::<(f32, f32, f32)>().unwrap(),
            (1.0, 0.0, 0.0)
        );

        // __setitem__ writes through to the node (a fresh field sees the edit).
        point.set_item(1, (2.0f32, 3.0f32, 4.0f32)).unwrap();
        assert_eq!(
            coords
                .getattr("point")
                .unwrap()
                .get_item(1)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (2.0, 3.0, 4.0)
        );

        // __delitem__ and getNum/getValues.
        point.del_item(0).unwrap();
        assert_eq!(point.len().unwrap(), 1);
        assert_eq!(
            point.call_method0("getNum").unwrap().extract::<usize>().unwrap(),
            1
        );
        let values = point.call_method0("getValues").unwrap();
        assert_eq!(values.len().unwrap(), 1);
    });
}

#[test]
fn whole_property_assignment_and_set1_value() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let coords = new_node(&coin, "SoCoordinate3");

        // `coords.point = [...]` replaces the buffer.
        coords
            .setattr("point", vec![(1.0f32, 2.0f32, 3.0f32)])
            .unwrap();
        let point = coords.getattr("point").unwrap();
        assert_eq!(point.len().unwrap(), 1);

        // set1Value grows the field to reach the index.
        point
            .call_method1("set1Value", (2usize, (5.0f32, 6.0f32, 7.0f32)))
            .unwrap();
        assert_eq!(point.len().unwrap(), 3);
        assert_eq!(
            point.get_item(2).unwrap().extract::<(f32, f32, f32)>().unwrap(),
            (5.0, 6.0, 7.0)
        );

        // setValue is the singular overload: length 1.
        point.call_method1("setValue", ((9.0f32, 8.0f32, 7.0f32),)).unwrap();
        assert_eq!(point.len().unwrap(), 1);
    });
}

#[test]
fn material_diffuse_color_accepts_rgb_and_rgba() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let material = new_node(&coin, "SoMaterial");
        let diffuse = material.getattr("diffuseColor").unwrap();

        // A 3-tuple (RGB) gets an opaque alpha.
        diffuse.call_method1("setValue", ((1.0f32, 0.0f32, 0.0f32),)).unwrap();
        assert_eq!(
            diffuse.get_item(0).unwrap().extract::<(f32, f32, f32, f32)>().unwrap(),
            (1.0, 0.0, 0.0, 1.0)
        );

        // A 4-tuple (RGBA) is kept verbatim.
        diffuse.call_method1("setValue", ((0.0f32, 1.0f32, 0.0f32, 0.5f32),)).unwrap();
        assert_eq!(
            diffuse.get_item(0).unwrap().extract::<(f32, f32, f32, f32)>().unwrap(),
            (0.0, 1.0, 0.0, 0.5)
        );
    });
}

#[test]
fn indexed_face_set_coord_index_is_int_field() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let faces = new_node(&coin, "SoIndexedFaceSet");
        let coord_index = faces.getattr("coordIndex").unwrap();

        coord_index
            .call_method1("setValues", (0usize, vec![0u32, 1, 2, 0, 2, 3]))
            .unwrap();
        assert_eq!(coord_index.len().unwrap(), 6);
        assert_eq!(coord_index.get_item(2).unwrap().extract::<u32>().unwrap(), 2);

        // Polygon delimiters (-1) are deferred; a negative index is rejected loudly
        // rather than silently wrapped.
        let err = coord_index.call_method1("set1Value", (0usize, -1i64));
        assert!(err.is_err(), "negative coordIndex should be rejected");
    });
}

#[test]
fn switch_which_child() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let switch = new_node(&coin, "SoSwitch");
        assert_eq!(switch.getattr("whichChild").unwrap().extract::<i32>().unwrap(), 0);

        switch.setattr("whichChild", -1).unwrap();
        assert_eq!(switch.getattr("whichChild").unwrap().extract::<i32>().unwrap(), -1);

        // The sentinels are exposed as module constants, like Coin.
        assert_eq!(
            coin.getattr("SO_SWITCH_NONE").unwrap().extract::<i32>().unwrap(),
            -1
        );
        assert_eq!(
            coin.getattr("SO_SWITCH_ALL").unwrap().extract::<i32>().unwrap(),
            -2
        );
    });
}

#[test]
fn transform_translation_and_rotation() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let transform = new_node(&coin, "SoTransform");

        transform.setattr("translation", (1.0f64, 2.0f64, 3.0f64)).unwrap();
        assert_eq!(
            transform
                .getattr("translation")
                .unwrap()
                .extract::<(f64, f64, f64)>()
                .unwrap(),
            (1.0, 2.0, 3.0)
        );

        // Axis-angle, in radians: a quarter turn about Z.
        transform
            .setattr("rotation", (0.0f64, 0.0f64, 1.0f64, std::f64::consts::FRAC_PI_2))
            .unwrap();
        let (ax, ay, az, angle) = transform
            .getattr("rotation")
            .unwrap()
            .extract::<(f64, f64, f64, f64)>()
            .unwrap();
        assert!((ax - 0.0).abs() < 1e-9 && (ay - 0.0).abs() < 1e-9 && (az - 1.0).abs() < 1e-9);
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    });
}

#[test]
fn geometry_nodes_are_registered_and_are_nodes() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let so_type = coin.getattr("SoType").unwrap();
        let node_type = coin.getattr("SoNode").unwrap();
        let node_type_id = so_type.call_method1("fromName", ("SoNode",)).unwrap();

        for name in ["SoCoordinate3", "SoNormal", "SoMaterial", "SoTransform"] {
            let type_id = so_type.call_method1("fromName", (name,)).unwrap();
            let instance = type_id.call_method0("createInstance").unwrap();
            assert!(
                instance.is_instance(&node_type).unwrap(),
                "{name} instance should be an instance of SoNode"
            );
            assert!(
                type_id
                    .call_method1("isDerivedFrom", (node_type_id.clone(),))
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                "{name} should derive from SoNode"
            );
        }
    });
}

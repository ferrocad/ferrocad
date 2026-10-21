//! Geometry field objects over the `pivy.coin` facade, via embedded CPython.
//!
//! These exercise the sequence protocol that workbench code relies on when it
//! mutates node fields in place (`coords.point[i] = …`, `faces.coordIndex.setValues(…)`),
//! and confirm the edits land on the wrapped `ferrocad_inventor` node (a live field,
//! not a copy).

use ferrocad_inventor::compile;
use pyo3::exceptions::PyValueError;
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

        // The sentinels are exposed as module constants, with Coin's values.
        assert_eq!(
            coin.getattr("SO_SWITCH_NONE")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            -1
        );
        assert_eq!(
            coin.getattr("SO_SWITCH_INHERIT")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            -2
        );
        assert_eq!(
            coin.getattr("SO_SWITCH_ALL")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            -3
        );
    });
}

#[test]
fn set_values_three_arg_overload_and_set_num() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let coords = new_node(&coin, "SoCoordinate3");
        let point = coords.getattr("point").unwrap();

        // Coin's 3-arg setValues(start, num, values): exactly `num` elements are
        // copied and surplus values are ignored.
        point
            .call_method1(
                "setValues",
                (
                    0usize,
                    2usize,
                    vec![
                        (1.0f32, 2.0f32, 3.0f32),
                        (4.0f32, 5.0f32, 6.0f32),
                        (7.0f32, 8.0f32, 9.0f32),
                    ],
                ),
            )
            .unwrap();
        assert_eq!(point.len().unwrap(), 2);
        assert_eq!(
            point
                .get_item(1)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (4.0, 5.0, 6.0)
        );

        // It grows to reach `start + num`, filling the gap with element defaults.
        point
            .call_method1(
                "setValues",
                (4usize, 1usize, vec![(9.0f32, 9.0f32, 9.0f32)]),
            )
            .unwrap();
        assert_eq!(point.len().unwrap(), 5);
        assert_eq!(
            point
                .get_item(2)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            point
                .get_item(4)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (9.0, 9.0, 9.0)
        );

        // A negative `num` is a ValueError, not a type mismatch.
        let err = point
            .call_method1(
                "setValues",
                (0isize, -1isize, vec![(0.0f32, 0.0f32, 0.0f32)]),
            )
            .unwrap_err();
        assert!(err.is_instance_of::<PyValueError>(py));

        // setNum grows with the element default and truncates, preserving values.
        point.call_method1("setNum", (7usize,)).unwrap();
        assert_eq!(point.len().unwrap(), 7);
        assert_eq!(
            point
                .get_item(6)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (0.0, 0.0, 0.0)
        );
        point.call_method1("setNum", (1usize,)).unwrap();
        assert_eq!(point.len().unwrap(), 1);
        assert_eq!(
            point
                .get_item(0)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (1.0, 2.0, 3.0)
        );
        let err = point.call_method1("setNum", (-1isize,)).unwrap_err();
        assert!(err.is_instance_of::<PyValueError>(py));

        // The original two-argument form still replaces from an offset.
        point
            .call_method1("setValues", (0usize, vec![(5.0f32, 5.0f32, 5.0f32)]))
            .unwrap();
        assert_eq!(point.len().unwrap(), 1);
        assert_eq!(
            point
                .get_item(0)
                .unwrap()
                .extract::<(f32, f32, f32)>()
                .unwrap(),
            (5.0, 5.0, 5.0)
        );
    });
}

#[test]
fn face_set_num_vertices_is_int_field() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let face = new_node(&coin, "SoFaceSet");
        let num_vertices = face.getattr("numVertices").unwrap();
        num_vertices.call_method1("setValue", (4u32,)).unwrap();
        assert_eq!(num_vertices.len().unwrap(), 1);
        assert_eq!(
            num_vertices.get_item(0).unwrap().extract::<u32>().unwrap(),
            4
        );
    });
}

#[test]
fn remove_child_removes_the_first_structurally_equal_node() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let group = new_node(&coin, "SoSeparator");
        let make = |x: f64| {
            let transform = new_node(&coin, "SoTransform");
            transform
                .setattr("translation", (x, 0.0f64, 0.0f64))
                .unwrap();
            transform
        };
        let first = make(1.0);
        let middle = make(2.0);
        let last = make(3.0);
        for child in [first.clone(), middle.clone(), last.clone()] {
            group.call_method1("addChild", (child,)).unwrap();
        }
        assert_eq!(group.len().unwrap(), 3);

        group
            .call_method1("removeChild", (middle.clone(),))
            .unwrap();
        assert_eq!(group.len().unwrap(), 2);

        // Removing an absent node is a no-op.
        group.call_method1("removeChild", (middle,)).unwrap();
        assert_eq!(group.len().unwrap(), 2);

        // `addChild` copies, so a distinct node with the same contents removes the
        // first structural match.
        let copy_of_first = make(1.0);
        group.call_method1("removeChild", (copy_of_first,)).unwrap();
        assert_eq!(group.len().unwrap(), 1);
    });
}

#[test]
fn separator_render_culling_is_accepted_and_does_not_hide_the_subtree() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let separator = new_node(&coin, "SoSeparator");
        let separator_type = coin.getattr("SoSeparator").unwrap();
        let off = separator_type
            .getattr("OFF")
            .unwrap()
            .extract::<i32>()
            .unwrap();
        assert!(separator_type.hasattr("ON").unwrap());
        assert!(separator_type.hasattr("AUTO").unwrap());

        // `sep.renderCulling = coin.SoSeparator.OFF` — the addon's exact assignment.
        separator.setattr("renderCulling", off).unwrap();
        assert_eq!(
            separator
                .getattr("renderCulling")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            off
        );

        // Accepted and ignored: the subtree still compiles.
        let coords = new_node(&coin, "SoCoordinate3");
        coords
            .getattr("point")
            .unwrap()
            .call_method1(
                "setValues",
                (
                    0usize,
                    vec![
                        (0.0f32, 0.0f32, 0.0f32),
                        (1.0f32, 0.0f32, 0.0f32),
                        (0.0f32, 1.0f32, 0.0f32),
                    ],
                ),
            )
            .unwrap();
        let faces = new_node(&coin, "SoIndexedFaceSet");
        faces
            .getattr("coordIndex")
            .unwrap()
            .call_method1("setValues", (0usize, vec![0u32, 1, 2]))
            .unwrap();
        separator.call_method1("addChild", (coords,)).unwrap();
        separator.call_method1("addChild", (faces,)).unwrap();

        let scene = ferrocad_inventor_py::extract_scene(&separator).unwrap();
        assert_eq!(compile(&scene).0.len(), 1);
    });
}

#[test]
fn node_set_parses_coin_field_strings() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);

        let depth = new_node(&coin, "SoDepthBuffer");
        depth.call_method1("set", ("write FALSE",)).unwrap();
        assert!(!depth.getattr("write").unwrap().extract::<bool>().unwrap());
        assert!(depth.getattr("test").unwrap().extract::<bool>().unwrap());
        depth.call_method1("set", ("function ALWAYS",)).unwrap();
        let always = coin
            .getattr("SoDepthBuffer")
            .unwrap()
            .getattr("ALWAYS")
            .unwrap()
            .extract::<i32>()
            .unwrap();
        assert_eq!(
            depth.getattr("function").unwrap().extract::<i32>().unwrap(),
            always
        );

        // `name=value` is accepted too.
        let light = new_node(&coin, "SoLightModel");
        light.call_method1("set", ("model=BASE_COLOR",)).unwrap();
        let base_color = coin
            .getattr("SoLightModel")
            .unwrap()
            .getattr("BASE_COLOR")
            .unwrap()
            .extract::<i32>()
            .unwrap();
        assert_eq!(
            light.getattr("model").unwrap().extract::<i32>().unwrap(),
            base_color
        );

        let pick = new_node(&coin, "SoPickStyle");
        pick.call_method1("set", ("style UNPICKABLE",)).unwrap();
        let unpickable = coin
            .getattr("SoPickStyle")
            .unwrap()
            .getattr("UNPICKABLE")
            .unwrap()
            .extract::<i32>()
            .unwrap();
        assert_eq!(
            pick.getattr("style").unwrap().extract::<i32>().unwrap(),
            unpickable
        );

        // Unknown field and unknown value are ValueErrors naming the field.
        let err = depth.call_method1("set", ("nope TRUE",)).unwrap_err();
        assert!(err.is_instance_of::<PyValueError>(py));
        assert!(err.to_string().contains("nope"));
        let err = depth.call_method1("set", ("write MAYBE",)).unwrap_err();
        assert!(err.is_instance_of::<PyValueError>(py));
        assert!(err.to_string().contains("write"));
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

        for name in [
            "SoCoordinate3",
            "SoNormal",
            "SoMaterial",
            "SoTransform",
            "SoFaceSet",
            "SoPickStyle",
            "SoLightModel",
            "SoDepthBuffer",
        ] {
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

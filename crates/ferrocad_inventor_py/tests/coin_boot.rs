//! S1 boot tests for the `pivy.coin` facade, via embedded CPython.
//!
//! FreeCAD brings Coin up through the GUI (`SoDB::init`/`SoFCDB::init` in
//! `Gui::Application`), and workbenches then do `from pivy import coin`. This tests
//! the `coin` surface through that same import path: the built-in `coin` module is
//! registered, then surfaced as `pivy.coin`.

use pyo3::prelude::*;
use pyo3::types::PyModule;

mod common;
use common::pivy_coin;

#[test]
fn imports_through_pivy_coin() {
    Python::with_gil(|py| {
        let _ = pivy_coin(py);
        // The real import path: `from pivy import coin`.
        let pivy = PyModule::import(py, "pivy").expect("import pivy");
        let coin = pivy.getattr("coin").expect("pivy.coin");
        assert!(coin.hasattr("SoSeparator").unwrap());
        assert!(coin.hasattr("SoType").unwrap());
    });
}

#[test]
fn class_spine_passes_isinstance() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let sep = coin.getattr("SoSeparator").unwrap().call0().unwrap();
        for base in [
            "SoSeparator",
            "SoGroup",
            "SoNode",
            "SoFieldContainer",
            "SoBase",
        ] {
            let ty = coin.getattr(base).unwrap();
            assert!(
                sep.is_instance(&ty).unwrap(),
                "a SoSeparator instance should be an instance of {base}"
            );
        }
        // ...and not of a sibling class.
        let switch = coin.getattr("SoSwitch").unwrap().call0().unwrap();
        assert!(!switch
            .is_instance(&coin.getattr("SoSeparator").unwrap())
            .unwrap());
    });
}

#[test]
fn groups_hold_children() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let sep = coin.getattr("SoSeparator").unwrap().call0().unwrap();
        assert_eq!(sep.len().unwrap(), 0);
        let child = coin.getattr("SoSwitch").unwrap().call0().unwrap();
        sep.call_method1("addChild", (child,)).unwrap();
        assert_eq!(sep.len().unwrap(), 1);
        assert_eq!(
            sep.call_method0("getNumChildren")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            1
        );
    });
}

#[test]
fn so_type_registry() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let so_type = coin.getattr("SoType").unwrap();

        // Factory generation.
        let sep_ty = so_type.call_method1("fromName", ("SoSeparator",)).unwrap();
        assert_eq!(
            sep_ty
                .call_method0("getName")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "SoSeparator"
        );
        let instance = sep_ty.call_method0("createInstance").unwrap();
        assert!(instance
            .is_instance(&coin.getattr("SoSeparator").unwrap())
            .unwrap());

        // Derivation.
        let group_ty = so_type.call_method1("fromName", ("SoGroup",)).unwrap();
        assert!(sep_ty
            .call_method1("isDerivedFrom", (group_ty,))
            .unwrap()
            .extract::<bool>()
            .unwrap());

        // Unknown names yield nothing.
        assert!(so_type
            .call_method1("fromName", ("NoSuchType",))
            .unwrap()
            .is_none());
    });
}

#[test]
fn instance_type_id() {
    Python::with_gil(|py| {
        let coin = pivy_coin(py);
        let sep = coin.getattr("SoSeparator").unwrap().call0().unwrap();
        let type_id = sep.call_method0("getTypeId").unwrap();
        assert_eq!(
            type_id
                .call_method0("getName")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "SoSeparator"
        );
    });
}

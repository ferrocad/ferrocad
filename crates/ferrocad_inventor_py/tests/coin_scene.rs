//! The scene-extraction bridge — a Python-built `pivy.coin` tree copied
//! into a `ferrocad_inventor::Scene` and compiled, exactly as the host does.

use ferrocad_inventor::{compile, Topology};
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// A workbench-style scene build: the same cube the Rust sample draws, but scripted.
const SCENE_SCRIPT: &str = r#"
from pivy import coin

root = coin.SoSeparator()

material = coin.SoMaterial()
material.diffuseColor.setValue((0.9, 0.6, 0.2, 1.0))
root.addChild(material)

transform = coin.SoTransform()
transform.rotation = (1.0, 1.0, 0.0, 0.6)
root.addChild(transform)

coords = coin.SoCoordinate3()
coords.point.setValues(0, [
    (-1.0, -1.0, -1.0), (1.0, -1.0, -1.0), (1.0, 1.0, -1.0), (-1.0, 1.0, -1.0),
    (-1.0, -1.0, 1.0), (1.0, -1.0, 1.0), (1.0, 1.0, 1.0), (-1.0, 1.0, 1.0),
])
root.addChild(coords)

faces = coin.SoIndexedFaceSet()
faces.coordIndex.setValues(0, [
    4, 5, 6, 4, 6, 7,
    1, 0, 3, 1, 3, 2,
    0, 4, 7, 0, 7, 3,
    5, 1, 2, 5, 2, 6,
    7, 6, 2, 7, 2, 3,
    0, 1, 5, 0, 5, 4,
])
root.addChild(faces)
"#;

/// Run `SCENE_SCRIPT` under `coin`/`pivy` and return its `root` object.
fn build_root<'py>(py: Python<'py>) -> Bound<'py, pyo3::PyAny> {
    ferrocad_inventor_py::install(py).expect("install coin");
    let globals = PyDict::new(py);
    let code = std::ffi::CString::new(SCENE_SCRIPT).expect("script has no NUL");
    py.run(&code, Some(&globals), None).expect("run scene script");
    globals
        .get_item("root")
        .expect("get_item")
        .expect("script defines root")
}

#[test]
fn python_built_scene_extracts_and_compiles() {
    Python::with_gil(|py| {
        let root = build_root(py);
        let scene = ferrocad_inventor_py::extract_scene(&root).expect("extract");

        // The root is a separator whose children carry the fields the compiler binds.
        assert_eq!(scene.root.children.len(), 4);

        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1, "one mesh in the cube");
        let packet = &queue.packets[0];
        assert_eq!(packet.topology, Topology::Triangles);
        assert_eq!(packet.state.color, [0.9, 0.6, 0.2, 1.0]);
        // The transform from Python reached the compiled state (not the identity).
        assert!(!packet.state.world_transform.is_unity(1e-9));
        let geometry = packet.geometry.as_ref().expect("geometry on a cache miss");
        assert_eq!(geometry.indices.len(), 36, "twelve triangles");
    });
}

#[test]
fn extraction_rejects_a_non_node() {
    Python::with_gil(|py| {
        ferrocad_inventor_py::install(py).expect("install coin");
        let not_a_node = {
            let code = std::ffi::CString::new("42").unwrap();
            py.eval(&code, None, None).expect("eval")
        };
        assert!(ferrocad_inventor_py::extract_scene(&not_a_node).is_err());
    });
}

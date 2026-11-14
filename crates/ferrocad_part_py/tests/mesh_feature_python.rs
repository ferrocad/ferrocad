//! `Mesh::FeaturePython`: a Python proxy drives a parametric mesh.
//!
//! This is the document type the SubD workbench stores its cage on
//! (`CagePoints` + `CageFaces` on a `Mesh::FeaturePython`, with the Catmull-Clark
//! preview assigned to `obj.Mesh`). Upstream it is `Mesh::Feature` plus the
//! `App::FeaturePython` extension, so the proxy path is Part's: the core records a
//! request and the binding runs `proxy.execute(obj)` with the document lock
//! released.
//!
//! Same one-image boot as `mesh_conformance`: `ferrocad` + `Part` + `Mesh` as
//! built-ins, so the object registry, the mesh property and the proxy executor all
//! live in one core.

use std::ffi::CString;
use std::sync::{Mutex, Once};

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_mesh_py::Mesh as mesh_module;
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
        pyo3::append_to_inittab!(mesh_module);
    });
}

/// A cage proxy: `execute` assigns a triangle, `onChanged` records the property.
const PROXY_SOURCE: &str = r#"
class CageProxy:
    def __init__(self):
        self.executes = 0

    def execute(self, obj):
        self.executes += 1
        side = float(getattr(obj, "Side", 1.0))
        obj.Mesh = Mesh.Mesh((
            [FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(side, 0, 0), FreeCAD.Vector(0, side, 0)],
            [(0, 1, 2)],
        ))
"#;

/// Install `name` as an importable module holding `CageProxy`.
fn install_proxy_module<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    let module = PyModule::new(py, name)?;
    module.setattr("Mesh", py.import("Mesh")?)?;
    module.setattr("FreeCAD", py.import("ferrocad")?)?;
    let code = CString::new(PROXY_SOURCE).unwrap();
    py.run(&code, Some(&module.dict()), None)?;
    py.import("sys")?
        .getattr("modules")?
        .downcast::<PyDict>()?
        .set_item(name, &module)?;
    module.getattr("CageProxy")
}

#[test]
fn a_mesh_feature_python_proxy_builds_its_mesh() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let _mesh = PyModule::import(py, "Mesh").expect("import Mesh");
        let proxy_class = install_proxy_module(py, "mesh_cage_proxy_test").expect("proxy module");
        let proxy = proxy_class.call0().expect("CageProxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("MeshFeaturePythonDemo",))
            .expect("newDocument");
        let cage = doc
            .call_method1(
                "addObject",
                ("Mesh::FeaturePython", "Cage", &proxy, py.None()),
            )
            .expect("addObject Mesh::FeaturePython");
        assert!(
            cage.getattr("Proxy").unwrap().is(&proxy),
            "addObject stores the object proxy"
        );
        // The SubD workbench declares the cage properties on the object.
        cage.call_method1(
            "addProperty",
            ("App::PropertyVectorList", "CagePoints", "Cage"),
        )
        .expect("CagePoints");
        cage.call_method1(
            "addProperty",
            ("App::PropertyIntegerList", "CageFaces", "Cage"),
        )
        .expect("CageFaces");
        cage.call_method1("addProperty", ("App::PropertyFloat", "Side", "Cage"))
            .expect("Side property");
        cage.setattr("Side", 2.0).expect("Side");

        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            1,
            "the proxy executed exactly once"
        );
        let mesh = cage.getattr("Mesh").expect("Cage.Mesh");
        assert_eq!(mesh.getattr("CountPoints").unwrap().extract::<usize>().unwrap(), 3);
        assert_eq!(mesh.getattr("CountFacets").unwrap().extract::<usize>().unwrap(), 1);
        // The triangle spans the proxy's `Side`, so the proxy's output is the mesh.
        let box_ = mesh.getattr("BoundBox").unwrap();
        assert!(
            (box_.getattr("XMax").unwrap().extract::<f64>().unwrap() - 2.0).abs() < 1e-9,
            "the proxy's mesh reaches the object"
        );

        // A property edit schedules the proxy again and the new value is used.
        cage.setattr("Side", 3.0).expect("Side = 3");
        doc.call_method0("recompute").expect("recompute again");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            2,
            "a touched property re-runs the proxy"
        );
        let box_ = cage.getattr("Mesh").unwrap().getattr("BoundBox").unwrap();
        assert!(
            (box_.getattr("XMax").unwrap().extract::<f64>().unwrap() - 3.0).abs() < 1e-9
        );

        // `Placement` mirrors into the mesh transform, as `Mesh::Feature` does.
        let vector = ferrocad
            .getattr("Vector")
            .unwrap()
            .call1((5.0, 0.0, 0.0))
            .unwrap();
        let rotation = ferrocad.getattr("Rotation").unwrap().call0().unwrap();
        let placement = ferrocad
            .getattr("Placement")
            .unwrap()
            .call1((vector, rotation))
            .unwrap();
        cage.setattr("Placement", placement).expect("Placement");
        let moved = cage.getattr("Mesh").unwrap().getattr("BoundBox").unwrap();
        assert!(
            (moved.getattr("XMin").unwrap().extract::<f64>().unwrap() - 5.0).abs() < 1e-9,
            "Placement moves the mesh without a recompute"
        );

        ferrocad
            .call_method1("closeDocument", ("MeshFeaturePythonDemo",))
            .expect("closeDocument");
    });
}

//! The headless `obj.ViewObject` model: defaults, colours and persistence.
//!
//! These mirror the App-level assertions of the upstream `ColorTransparency` and
//! `ColorPerFace` tests without requiring a GUI: the Coin scene-graph checks in
//! `ColorPerFaceTest` are exercised separately when `pivy` is importable.
//!
//! Same boot as `part_unified`: `ferrocad` + `Part` as built-ins, so `import Part`
//! and `import FreeCAD` see one core. It needs OCCT (see
//! `docs/architecture/occt-bundling.md`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// The embedded interpreter and the preference store are process-wide, so the
/// tests take turns (each one also resets the View preferences it touches).
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Put the repository's `python/` directory on `sys.path`, so the pure-Python
/// `FreeCAD` facade (and its `ParamGet`) is importable next to the built-ins.
fn use_repo_python(py: Python<'_>) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let python_dir = repo.join("python");
    let sys = py.import("sys").expect("import sys");
    let path = sys.getattr("path").expect("sys.path");
    path.call_method1("insert", (0, python_dir.to_string_lossy().into_owned()))
        .expect("sys.path.insert");
    // Importing `Part` runs its module init, which registers the Part object types
    // (`Part::Box`, `Part::MultiFuse`, …).
    PyModule::import(py, "Part").expect("import Part");
}

fn freecad<'py>(py: Python<'py>) -> Bound<'py, PyModule> {
    PyModule::import(py, "FreeCAD").expect("import FreeCAD")
}

fn view_prefs<'py>(py: Python<'py>) -> Bound<'py, PyAny> {
    freecad(py)
        .call_method1("ParamGet", ("User parameter:BaseApp/Preferences/View",))
        .expect("FreeCAD.ParamGet(view preferences)")
}

/// Every test starts from a known preference state, so the order does not matter.
fn reset_view_prefs(py: Python<'_>) {
    let prefs = view_prefs(py);
    prefs
        .call_method1("SetInt", ("DefaultShapeTransparency", 0))
        .expect("DefaultShapeTransparency = 0");
    prefs
        .call_method1("SetUnsigned", ("DefaultShapeColor", 0xCCCC_E6FFu32))
        .expect("DefaultShapeColor = 0xCCCCE6FF");
}

fn new_document(py: Python<'_>) -> Bound<'_, PyAny> {
    freecad(py)
        .call_method1("newDocument", (py.None(),))
        .expect("FreeCAD.newDocument()")
}

fn close_document(py: Python<'_>, doc: &Bound<'_, PyAny>) {
    let name = doc.getattr("Name").expect("doc.Name");
    let _ = freecad(py).call_method1("closeDocument", (name,));
}

fn add_object<'py>(doc: &Bound<'py, PyAny>, type_id: &str) -> Bound<'py, PyAny> {
    doc.call_method1("addObject", (type_id,))
        .unwrap_or_else(|error| panic!("addObject({type_id}): {error}"))
}

fn view_object<'py>(object: &Bound<'py, PyAny>) -> Bound<'py, PyAny> {
    object.getattr("ViewObject").expect("obj.ViewObject")
}

fn rgba(value: &Bound<'_, PyAny>) -> [f64; 4] {
    value
        .extract::<(f64, f64, f64, f64)>()
        .expect("an RGBA tuple")
        .into()
}

/// `ViewObject.DiffuseColor` as `[f64; 4]` entries.
fn diffuse(value: &Bound<'_, PyAny>) -> Vec<[f64; 4]> {
    value
        .extract::<Vec<(f64, f64, f64, f64)>>()
        .expect("a list of RGBA tuples")
        .into_iter()
        .map(|(r, g, b, a)| [r, g, b, a])
        .collect()
}

fn temp_file(stem: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ferrocad-{stem}-{}.fcstd.json",
        std::process::id()
    ))
}

#[test]
fn view_object_is_stable_per_object() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let first = view_object(&object);
        let second = view_object(&object);
        assert!(first.is(&second), "ViewObject must be stable per object");
        close_document(py, &doc);
    });
}

#[test]
fn link_has_a_stable_view_object_and_links_its_target() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let doc = new_document(py);
        let cube = add_object(&doc, "Part::Box");
        let link = add_object(&doc, "App::Link");
        link.call_method1("setLink", (&cube,)).expect("link.setLink(box)");
        assert!(link.getattr("LinkedObject").expect("LinkedObject").is(&cube));
        let first = view_object(&link);
        assert!(first.is(&view_object(&link)));
        let [r, g, b, a] = rgba(&first.getattr("ShapeColor").expect("ShapeColor"));
        // The default `0xCCCCE6FF` (204, 204, 230) at full opacity.
        assert!((r - 204.0 / 255.0).abs() < 1e-12);
        assert!((g - 204.0 / 255.0).abs() < 1e-12);
        assert!((b - 230.0 / 255.0).abs() < 1e-12);
        assert_eq!(a, 1.0);
        close_document(py, &doc);
    });
}

#[test]
fn default_transparency_comes_from_the_preference() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        view_prefs(py)
            .call_method1("SetInt", ("DefaultShapeTransparency", 70))
            .expect("DefaultShapeTransparency = 70");

        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let view = view_object(&object);
        let transparency: i64 = view
            .getattr("Transparency")
            .expect("ViewObject.Transparency")
            .extract()
            .expect("an int");
        assert_eq!(transparency, 70, "the default transparency must be applied");

        close_document(py, &doc);
    });
}

#[test]
fn default_shape_colour_comes_from_the_preference() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        view_prefs(py)
            .call_method1("SetUnsigned", ("DefaultShapeColor", 0xFF00_0000u32))
            .expect("DefaultShapeColor = 0xFF000000");

        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let view = view_object(&object);

        let appearance = view
            .getattr("ShapeAppearance")
            .expect("ViewObject.ShapeAppearance");
        let material = appearance
            .get_item(0)
            .expect("ShapeAppearance[0]")
            .getattr("DiffuseColor")
            .expect("Material.DiffuseColor");
        assert_eq!(rgba(&material), [1.0, 0.0, 0.0, 1.0]);

        let shape_material = view
            .getattr("ShapeMaterial")
            .expect("ViewObject.ShapeMaterial")
            .getattr("DiffuseColor")
            .expect("ShapeMaterial.DiffuseColor");
        assert_eq!(rgba(&shape_material), [1.0, 0.0, 0.0, 1.0]);

        close_document(py, &doc);
    });
}

#[test]
fn changing_a_material_colour_keeps_the_transparency() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        view_prefs(py)
            .call_method1("SetInt", ("DefaultShapeTransparency", 70))
            .expect("DefaultShapeTransparency = 70");

        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let view = view_object(&object);
        let appearance = view
            .getattr("ShapeAppearance")
            .expect("ViewObject.ShapeAppearance");
        let material = appearance.get_item(0).expect("ShapeAppearance[0]");
        material
            .setattr("DiffuseColor", (0.5, 0.0, 0.0))
            .expect("Material.DiffuseColor = (0.5, 0, 0)");

        let transparency: i64 = view
            .getattr("Transparency")
            .expect("ViewObject.Transparency")
            .extract()
            .expect("an int");
        assert_eq!(
            transparency, 70,
            "changing a material's colour must not reset the transparency"
        );
        // The handle writes back into the view state.
        let written = appearance
            .get_item(0)
            .expect("ShapeAppearance[0]")
            .getattr("DiffuseColor")
            .expect("Material.DiffuseColor");
        assert_eq!(rgba(&written), [0.5, 0.0, 0.0, 1.0]);

        close_document(py, &doc);
    });
}

#[test]
fn shape_colour_accepts_tuples_and_packed_integers() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let view = view_object(&object);

        view.setattr("ShapeColor", (1.0, 0.0, 0.0))
            .expect("ShapeColor = (1, 0, 0)");
        assert_eq!(
            rgba(&view.getattr("ShapeColor").expect("ShapeColor")),
            [1.0, 0.0, 0.0, 1.0]
        );
        // Packed colours are 0xRRGGBBAA (FreeCAD's `Base::Color`).
        view.setattr("ShapeColor", 0x00FF_00FFu32)
            .expect("ShapeColor = 0x00FF00FF");
        assert_eq!(
            rgba(&view.getattr("ShapeColor").expect("ShapeColor")),
            [0.0, 1.0, 0.0, 1.0]
        );
        // The derived views follow the shape colour.
        let appearance = view.getattr("ShapeAppearance").expect("ShapeAppearance");
        let material = appearance
            .get_item(0)
            .expect("ShapeAppearance[0]")
            .getattr("DiffuseColor")
            .expect("Material.DiffuseColor");
        assert_eq!(rgba(&material), [0.0, 1.0, 0.0, 1.0]);
        let shape_material = view
            .getattr("ShapeMaterial")
            .expect("ShapeMaterial")
            .getattr("DiffuseColor")
            .expect("ShapeMaterial.DiffuseColor");
        assert_eq!(rgba(&shape_material), [0.0, 1.0, 0.0, 1.0]);

        close_document(py, &doc);
    });
}

#[test]
fn shape_material_colour_round_trips() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        let view = view_object(&object);

        let material = view.getattr("ShapeMaterial").expect("ShapeMaterial");
        material
            .setattr("DiffuseColor", (0.2, 0.4, 0.6, 0.8))
            .expect("ShapeMaterial.DiffuseColor = ...");
        let read_back = view
            .getattr("ShapeMaterial")
            .expect("ShapeMaterial")
            .getattr("DiffuseColor")
            .expect("ShapeMaterial.DiffuseColor");
        let [r, g, b, a] = rgba(&read_back);
        assert!((r - 0.2).abs() < 1e-12 && (g - 0.4).abs() < 1e-12);
        assert!((b - 0.6).abs() < 1e-12 && (a - 0.8).abs() < 1e-12);

        close_document(py, &doc);
    });
}

#[test]
fn origin_planes_stay_opaque() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        view_prefs(py)
            .call_method1("SetInt", ("DefaultShapeTransparency", 70))
            .expect("DefaultShapeTransparency = 70");

        let doc = new_document(py);
        let _origin = add_object(&doc, "App::Origin");
        let planes = doc
            .call_method1("findObjects", ("App::Plane",))
            .expect("doc.findObjects('App::Plane')");
        let plane = planes.get_item(0).expect("at least one App::Plane");
        let transparency: i64 = view_object(&plane)
            .getattr("Transparency")
            .expect("Plane.ViewObject.Transparency")
            .extract()
            .expect("an int");
        assert_eq!(transparency, 0, "an App::Plane is drawn opaque");

        close_document(py, &doc);
    });
}

#[test]
fn per_face_colours_and_visibility_survive_save_and_reopen() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let path = temp_file("per-face");

        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        doc.call_method0("recompute").expect("doc.recompute()");
        let view = view_object(&object);
        let colors = vec![
            (1.0, 0.0, 0.0, 1.0),
            (1.0, 0.0, 0.0, 1.0),
            (1.0, 0.0, 0.0, 1.0),
            (1.0, 0.0, 0.0, 1.0),
            (1.0, 1.0, 0.0, 1.0),
            (1.0, 1.0, 0.0, 1.0),
        ];
        view.setattr("DiffuseColor", colors.clone())
            .expect("ViewObject.DiffuseColor = 6 colours");
        // Upstream sets the *document object* attribute (`box.Visibility`); it is
        // forwarded to the view object.
        object.setattr("Visibility", false).expect("box.Visibility = False");

        doc.call_method1("saveAs", (path.to_string_lossy().into_owned(),))
            .expect("doc.saveAs");
        close_document(py, &doc);

        let reopened = freecad(py)
            .call_method1("openDocument", (path.to_string_lossy().into_owned(),))
            .expect("FreeCAD.openDocument");
        let cube = reopened.getattr("Box").expect("reopened doc.Box");
        let view = view_object(&cube);
        assert_eq!(
            diffuse(&view.getattr("DiffuseColor").expect("DiffuseColor")),
            colors
                .into_iter()
                .map(|(r, g, b, a)| [r, g, b, a])
                .collect::<Vec<_>>()
        );
        let visibility: bool = view
            .getattr("Visibility")
            .expect("Visibility")
            .extract()
            .expect("a bool");
        assert!(!visibility, "the saved visibility must be restored");

        close_document(py, &reopened);
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn diffuses_cover_every_face_of_a_boolean_result() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        reset_view_prefs(py);
        let doc = new_document(py);
        let cube = add_object(&doc, "Part::Box");
        let cylinder = add_object(&doc, "Part::Cylinder");
        cube.getattr("ViewObject")
            .expect("box.ViewObject")
            .setattr("ShapeColor", (1.0, 0.0, 0.0, 1.0))
            .expect("box ShapeColor = red");
        cylinder
            .getattr("ViewObject")
            .expect("cylinder.ViewObject")
            .setattr("ShapeColor", (1.0, 0.0, 0.0, 1.0))
            .expect("cylinder ShapeColor = red");
        doc.call_method0("recompute").expect("doc.recompute()");

        let fuse = add_object(&doc, "Part::MultiFuse");
        let shapes = pyo3::types::PyList::new(py, [&cube, &cylinder]).expect("Shapes list");
        fuse.setattr("Shapes", shapes).expect("fuse.Shapes = [box, cylinder]");
        doc.call_method0("recompute").expect("doc.recompute()");

        let faces = fuse
            .getattr("Shape")
            .expect("fuse.Shape")
            .getattr("Faces")
            .expect("Shape.Faces")
            .len()
            .expect("face count");
        assert!(faces > 0, "the fused shape must have faces");

        let colors = diffuse(
            &fuse
                .getattr("ViewObject")
                .expect("fuse.ViewObject")
                .getattr("DiffuseColor")
                .expect("DiffuseColor"),
        );
        assert_eq!(
            colors.len(),
            faces,
            "one diffuse colour per face of the fused shape"
        );
        assert!(
            colors.iter().all(|color| *color == [1.0, 0.0, 0.0, 1.0]),
            "both operands are red, so the fuse must be red (got {colors:?})"
        );

        close_document(py, &doc);
    });
}

#[test]
fn root_node_mirrors_the_material_when_coin_is_available() {
    let _guard = test_lock();
    boot();
    Python::with_gil(|py| {
        use_repo_python(py);
        // `RootNode` is only available with the pivy bindings; skip otherwise.
        if PyModule::import(py, "pivy.coin").is_err() {
            eprintln!("pivy is not importable; skipping the Coin scene-graph check");
            return;
        }
        reset_view_prefs(py);
        let doc = new_document(py);
        let object = add_object(&doc, "Part::Box");
        doc.call_method0("recompute").expect("doc.recompute()");
        let view = view_object(&object);
        let colors = vec![(1.0, 0.0, 0.0, 1.0); 6];
        view.setattr("DiffuseColor", colors)
            .expect("ViewObject.DiffuseColor");

        let coin = py.import("pivy.coin").expect("pivy.coin");
        let search = coin.getattr("SoSearchAction").expect("SoSearchAction").call0()
            .expect("SoSearchAction()");
        search
            .call_method1("setType", (coin.getattr("SoMaterial").expect("SoMaterial").call_method0("getClassTypeId").expect("getClassTypeId"),))
            .expect("setType(SoMaterial)");
        search
            .call_method1("setInterest", (coin.getattr("SoSearchAction").expect("SoSearchAction").getattr("ALL").expect("ALL"),))
            .expect("setInterest(ALL)");
        search
            .call_method1("apply", (view.getattr("RootNode").expect("RootNode"),))
            .expect("apply(RootNode)");
        let paths = search.call_method0("getPaths").expect("getPaths()");
        assert!(
            paths.call_method0("getLength").expect("getLength").extract::<usize>().expect("length") >= 2,
            "the two display representations each carry a material"
        );
        let material = paths
            .call_method1("get", (1,))
            .expect("paths.get(1)")
            .call_method0("getTail")
            .expect("getTail()");
        let count: usize = material
            .getattr("diffuseColor")
            .expect("diffuseColor")
            .call_method0("getNum")
            .expect("getNum()")
            .extract()
            .expect("count");
        assert_eq!(count, 6, "the material mirrors the six per-face colours");

        close_document(py, &doc);
    });
}

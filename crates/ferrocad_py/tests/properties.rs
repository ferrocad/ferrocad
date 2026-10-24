//! Property coercion, the `Vector` helpers Draft uses, dynamic extension
//! dispatch, and the Python group type.

use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};

use ferrocad_core::{Document as CoreDocument, ObjectId, Property};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyTuple};
use pyo3::IntoPyObjectExt;

fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::prepare_freethreaded_python();
    });
}

/// The Python binding keeps process-wide registries; serialise the tests that
/// exercise them so they do not observe each other's documents/extensions.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(source: &str) {
    Python::with_gil(|py| {
        py.run(&CString::new(source).expect("no NUL"), None, None)
            .expect("property script");
    });
}

#[test]
fn link_property_coercions() {
    let _guard = lock();
    boot();
    run(
        r#"
import ferrocad as App

doc = App.newDocument("Coercions")
a = doc.addObject("App::FeaturePython", "A")
b = doc.addObject("App::FeaturePython", "B")

# LinkSubList: addProperty returns the object, so upstream chains work.
obj = doc.addObject("App::FeaturePython", "Obj")
holder = obj.addProperty("App::PropertyLinkSubList", "Faces", "Draft", "")
assert holder is obj, holder
assert obj.Faces == [], obj.Faces

# A list of (object, subname) pairs, a single pair, and a single object.
obj.Faces = [(a, "Face1"), (b, ["Face2", "Face3"])]
assert len(obj.Faces) == 2, obj.Faces
assert obj.Faces[0][0] is a and obj.Faces[0][1] == ("Face1",), obj.Faces
assert obj.Faces[1][0] is b and obj.Faces[1][1] == ("Face2", "Face3"), obj.Faces

obj.Faces = (a, "Face4")
assert obj.Faces == [(a, ("Face4",))], obj.Faces

obj.Faces = a
assert obj.Faces == [(a, ("",))], obj.Faces

# LinkList accepts a single object too.
obj.addProperty("App::PropertyLinkList", "Others")
obj.Others = b
assert obj.Others == [b], obj.Others

# StringList accepts a bare string and stores it as one element.
obj.addProperty("App::PropertyStringList", "Text")
obj.Text = "Original"
assert obj.Text == ["Original"], obj.Text

# `Immutable` is accepted as a status name (folded onto `ReadOnly`).
obj.setPropertyStatus("Text", "Immutable")
assert "ReadOnly" in obj.getPropertyStatus("Text")
obj.setPropertyStatus("Text", "-Immutable")
assert "ReadOnly" not in obj.getPropertyStatus("Text")

# None/empty clears the link list.
obj.Faces = None
assert obj.Faces == [], obj.Faces

App.closeDocument("Coercions")
"#,
    );
}

#[test]
fn vector_helpers_used_by_draft() {
    let _guard = lock();
    boot();
    run(
        r#"
import math
import ferrocad as App

v = App.Vector(1, 2, 3)
v.scale(2)
assert v == App.Vector(2, 4, 6), v

v = App.Vector(1, 2, 3)
v.scale(2, 3, 4)
assert v == App.Vector(2, 6, 12), v

v = App.Vector(1, 2, 3)
v.multiply(0.5)
assert v == App.Vector(0.5, 1.0, 1.5), v

# `normalize()` mutates in place (upstream semantics).
v = App.Vector(3, 0, 0)
v.normalize()
assert abs(v.Length - 1.0) < 1e-12 and abs(v.x - 1.0) < 1e-12, v

# Used by Draft's dimension/fillet helpers.
assert abs(App.Vector(0, 5, 0).distanceToLine(App.Vector(0, 0, 0), App.Vector(1, 0, 0)) - 5.0) < 1e-12
p = App.Vector(1, 2, 3)
p.projectToPlane(App.Vector(0, 0, 0), App.Vector(0, 0, 1))
assert p == App.Vector(1, 2, 0), p
"#,
    );
}

#[test]
fn rotation_inverse_and_neighbouring_operations() {
    let _guard = lock();
    boot();
    run(
        r#"
import math
import ferrocad as App

r = App.Rotation(App.Vector(0, 0, 1), 90)
inv = r.inverted()

# The inverse composes back to identity from either side.
assert r.multiply(inv).isSame(App.Rotation()), r.multiply(inv).Q
assert inv.multiply(r).isSame(App.Rotation()), inv.multiply(r).Q

# inverted() returns a new rotation: the original keeps its angle/axis, and the
# inverse rotates the other way.
assert abs(r.Angle - math.pi / 2) < 1e-12
assert abs(inv.Angle - math.pi / 2) < 1e-12
assert r.Axis == App.Vector(0, 0, 1), r.Axis
assert inv.Axis == App.Vector(0, 0, -1), inv.Axis
back = inv.multVec(App.Vector(1, 0, 0))
assert abs(back.x) < 1e-12 and abs(back.y + 1) < 1e-12, back

# The neighbouring surface Draft/Part use is present.
assert len(r.Q) == 4
assert r.Q[2] > 0 and r.Q[3] > 0
assert r.isSame(App.Rotation(App.Vector(0, 0, 1), 90))
assert r.multiply(App.Rotation()).isSame(r)
"#,
    );
}

static PROVIDER_CALLS: AtomicUsize = AtomicUsize::new(0);

fn provide_widget(doc: &mut CoreDocument, id: ObjectId) -> bool {
    PROVIDER_CALLS.fetch_add(1, Ordering::SeqCst);
    doc.add_property(id, "Gadget", Property::Integer(7), 0, "", "")
        .is_ok()
}

fn change_attacher_type(
    _doc: &mut CoreDocument,
    _id: ObjectId,
    _method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let tuple = args.downcast::<PyTuple>()?;
    let mode: String = tuple.get_item(0)?.extract()?;
    Ok(mode.into_py_any(args.py())?)
}

fn config_link_property(
    _doc: &mut CoreDocument,
    _id: ObjectId,
    _method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let tuple = args.downcast::<PyTuple>()?;
    let property: String = tuple.get_item(0)?.extract()?;
    let links: Bound<'_, PyDict> = tuple.get_item(1)?.downcast_into::<PyDict>()?;
    let linked_object: String = links
        .get_item("LinkedObject")?
        .expect("LinkedObject keyword")
        .extract()?;
    Ok((property, linked_object).into_py_any(args.py())?)
}

fn position_by_support(
    _doc: &mut CoreDocument,
    _id: ObjectId,
    _method: &str,
    _args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    Ok("positioned".into_py_any(_args.py())?)
}

#[test]
fn extension_registry_dispatches_provider_and_methods() {
    let _guard = lock();
    boot();
    ferrocad_py::register_extension_provider("Test::WidgetExtension", provide_widget);
    ferrocad_py::register_extension_method(
        "Test::WidgetExtension",
        "changeAttacherType",
        change_attacher_type,
    );
    ferrocad_py::register_extension_method(
        "Test::WidgetExtension",
        "configLinkProperty",
        config_link_property,
    );
    ferrocad_py::register_extension_method(
        "Test::WidgetExtension",
        "positionBySupport",
        position_by_support,
    );

    run(
        r#"
import ferrocad as App

doc = App.newDocument("Extensions")
obj = doc.addObject("App::FeaturePython", "Obj")

# Without the extension the methods are absent.
try:
    obj.changeAttacherType("Plane")
except AttributeError as error:
    assert "no attribute 'changeAttacherType'" in str(error), str(error)
else:
    raise AssertionError("expected AttributeError without the extension")
assert not hasattr(obj, "positionBySupport")

obj.addExtension("Test::WidgetExtension")
assert obj.hasExtension("Test::WidgetExtension")
assert obj.Gadget == 7, obj.Gadget
assert obj.changeAttacherType("Plane") == "Plane"
assert obj.configLinkProperty("Placement", LinkedObject="Base") == ("Placement", "Base")
# A method the binding never hard-coded is dispatched from the registry.
assert obj.positionBySupport() == "positioned"

App.closeDocument("Extensions")
"#,
    );
    assert_eq!(PROVIDER_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn a_python_group_is_a_group() {
    let _guard = lock();
    boot();
    run(
        r#"
import ferrocad as App

doc = App.newDocument("PythonGroup")

class GroupProxy:
    def execute(self, obj):
        obj.Count = obj.Count + 1

group = doc.addObject("App::DocumentObjectGroupPython", "Group", GroupProxy())
group.addProperty("App::PropertyInteger", "Count")
child = doc.addObject("App::FeaturePython", "Child")

# The group's proxy executes like any other Python-feature type.
doc.recompute()
assert group.Count == 1, group.Count

assert group.Group == [], group.Group
group.addObject(child)
assert [o.Name for o in group.Group] == ["Child"], group.Group
assert child.getParent() is group
assert group.hasObject(child)
assert group.removeObject(child) == [child]
assert group.Group == []
assert child.getParent() is None

# A plain feature is still not a group.
try:
    child.addObject(group)
except AttributeError as error:
    assert "is not a group" in str(error), str(error)
else:
    raise AssertionError("expected a non-group to refuse addObject")

App.closeDocument("PythonGroup")
"#,
    );
}

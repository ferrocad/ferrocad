//! Group membership and parent queries on container objects.
//!
//! Covers the two observable gaps in the Part suite: `App::Part` must expose
//! `removeObject`, and `PartDesign::Body` must be a group (so `body.addObject`
//! works) whose members report it as their parent.

use std::ffi::CString;

use pyo3::prelude::*;

/// Register `ferrocad` as a built-in module and start CPython.
///
/// `append_to_inittab!` must run before the interpreter initialises; without the
/// `auto-initialize` feature `prepare_freethreaded_python` does that explicitly.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::prepare_freethreaded_python();
    });
}

#[test]
fn group_membership_and_parents() {
    boot();
    Python::with_gil(|py| {
        let source = r#"
import ferrocad as App

doc = App.newDocument("Groups")

# --- App::Part: addObject/removeObject and parent agreement -----------------
part = doc.addObject("App::Part", "Part")
box = doc.addObject("App::FeatureTest", "Box")
part.addObject(box)

assert [o.Name for o in part.Group] == ["Box"], part.Group
assert box.getParent() is part
assert box.getParentGeoFeatureGroup() is part
assert box.getParentGroup() is None
assert part.hasObject(box)

removed = part.removeObject(box)
assert [o.Name for o in removed] == ["Box"], removed
assert part.Group == [], part.Group
assert box.getParent() is None
assert box.getParentGeoFeatureGroup() is None
assert not part.hasObject(box)

# A non-member is a no-op and reports an empty removal list (upstream
# GroupExtension::removeObjects filters/returns what it actually removed).
again = part.removeObject(box)
assert again == [], again
assert part.Group == []

# --- PartDesign::Body: an App::Part-like group container ---------------------
body = doc.addObject("PartDesign::Body", "Body")
assert body.Group == []
assert body.AllowCompound is True
assert body.Tip is None

box2 = doc.addObject("App::FeatureTest", "Box2")
body.addObject(box2)
assert [o.Name for o in body.Group] == ["Box2"], body.Group
assert body.hasObject(box2)
assert box2.getParent() is body
assert box2.getParentGeoFeatureGroup() is body
assert box2.getParentGroup() is None

body.removeObject(box2)
assert body.Group == []
assert box2.getParent() is None

# Single-group membership: adding to another container moves the object.
box3 = doc.addObject("App::FeatureTest", "Box3")
body.addObject(box3)
part.addObject(box3)
assert body.Group == [], body.Group
assert [o.Name for o in part.Group] == ["Box3"], part.Group
assert box3.getParent() is part

# --- A non-group refuses removeObject, like addObject ------------------------
feature = doc.addObject("App::FeatureTest", "Feature")
try:
    feature.removeObject(box3)
except AttributeError as error:
    assert "no attribute 'removeObject'" in str(error), str(error)
else:
    raise AssertionError("expected AttributeError from removeObject on a non-group")

try:
    feature.addObject(box3)
except AttributeError as error:
    assert "is not a group" in str(error), str(error)
else:
    raise AssertionError("expected AttributeError from addObject on a non-group")

App.closeDocument("Groups")
"#;
        py.run(&CString::new(source).expect("no NUL"), None, None)
            .expect("group membership script");
    });
}

//! `SoGroup`'s child lifecycle and the type queries an overlay consumer uses.
//!
//! A Coin preview or overlay builds a subtree, inspects it and tears only part of it
//! down (`getChild`, `insertChild`, `removeChild`, `removeAllChildren`, `findChild`),
//! and mods filter nodes with `isOfType(node.getClassTypeId())`. FerroCAD copies on
//! `addChild`, so the search/serialisation methods compare **copied nodes** rather
//! than Coin's pointer identity — that is the one documented difference.

use pyo3::prelude::*;
use pyo3::types::PyDict;

mod common;
use common::pivy_coin;

/// The lifecycle SimpleCAD's previews use: build, inspect, re-order, tear down.
const GROUPS_SCRIPT: &str = r#"
from pivy import coin

root = coin.SoSeparator()

coords = coin.SoCoordinate3()
coords.point.setValues(0, [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)])
root.addChild(coords)
root.addChild(coin.SoMaterial())
assert root.getNumChildren() == 2
assert len(root) == 2

# getChild returns a detached copy that still answers the type queries.
child = root.getChild(0)
assert child is not None
assert child.isOfType(coin.SoCoordinate3.getClassTypeId())
assert not child.isOfType(coin.SoSeparator.getClassTypeId())
assert child.isOfType(child.getTypeId())
assert child.isOfType(coin.SoNode.getClassTypeId())

# findChild finds the structurally equal copy (the documented copy boundary).
assert root.findChild(child) == 0
assert root.findChild(root.getChild(1)) == 1

# insertChild copies in at the index; an index past the end appends.
root.insertChild(coords, 0)
assert root.getNumChildren() == 3
assert root.findChild(coords) == 0
root.insertChild(coords, 99)
assert root.getNumChildren() == 4
assert root.getChild(3).isOfType(coin.SoCoordinate3.getClassTypeId())

# removeChild accepts an index and a node.
root.removeChild(3)
assert root.getNumChildren() == 3
root.removeChild(root.getChild(2))
assert root.getNumChildren() == 2

root.removeAllChildren()
assert root.getNumChildren() == 0
"#;

#[test]
fn group_children_lifecycle_and_type_queries() {
    Python::with_gil(|py| {
        pivy_coin(py);
        let globals = PyDict::new(py);
        let code = std::ffi::CString::new(GROUPS_SCRIPT).expect("script has no NUL");
        py.run(&code, Some(&globals), None)
            .expect("the SoGroup lifecycle script ran");
    });
}
